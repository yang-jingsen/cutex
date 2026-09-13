mod common;

use common::*;
use persistent_runtime_host::*;
use std::sync::Arc;
use std::time::Duration;

struct TimeoutBackend;

impl ProcessBackend for TimeoutBackend {
    fn name(&self) -> &'static str {
        "timeout_fixture"
    }

    fn start(
        &self,
        definition: &ServiceDefinition,
        run_id: &RunId,
    ) -> Result<ProcessIdentity, ProcessBackendError> {
        Ok(ProcessIdentity {
            pid: if definition.id.as_str() == "stuck" {
                41_001
            } else {
                41_002
            },
            birth_marker: format!("timeout-fixture-{run_id}"),
        })
    }

    fn stop(
        &self,
        service_id: &ServiceId,
        _run_id: &RunId,
        _shutdown_policy: &ShutdownPolicy,
    ) -> Result<StopOutcome, ProcessBackendError> {
        Ok(if service_id.as_str() == "stuck" {
            StopOutcome::TimedOut
        } else {
            StopOutcome::Graceful
        })
    }
}

#[test]
fn shutdown_fences_starts_and_stops_in_reverse_dependency_order() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("database", &[]), "register-database");
    register(&host, definition("cache", &[]), "register-cache");
    register(
        &host,
        definition("api", &["database", "cache"]),
        "register-api",
    );
    register(&host, definition("worker", &["api"]), "register-worker");
    ensure_running(&host, "worker", "start-worker");
    host.clear_order_history();

    let request = Request::ShutdownHost(ShutdownHostParams {
        mutation: mutation("shutdown-once"),
    });
    let first = host.handle(RequestEnvelope::v1("shutdown-first", request.clone()));
    let Response::ShutdownHost(first_result) = ok(first) else {
        panic!("unexpected shutdown response");
    };
    assert_eq!(first_result.final_phase, HostPhase::Stopped);
    assert_eq!(
        host.recorded_stop_order(),
        ids(&["worker", "api", "database"])
            .into_iter()
            .chain(ids(&["cache"]))
            .collect::<Vec<_>>()
    );

    // A retry with the same key remains safe after the shutdown fence closed.
    let retry = host.handle(RequestEnvelope::v1("shutdown-retry", request));
    let Response::ShutdownHost(retry_result) = ok(retry) else {
        panic!("unexpected shutdown retry response");
    };
    assert_eq!(retry_result.operation.id, first_result.operation.id);

    let rejected = host.handle(RequestEnvelope::v1(
        "start-after-shutdown",
        Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::from("api"),
            mutation: mutation("start-after-shutdown"),
        }),
    ));
    assert_eq!(error(rejected).code, ErrorCode::HostShuttingDown);
}

#[test]
fn operation_is_queryable_and_event_replay_contains_all_phases() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("api", &[]), "register-api");
    let started = ensure_running(&host, "api", "start-api");
    assert_eq!(started.operation.phase, OperationPhase::Succeeded);
    assert_eq!(
        started.service.runtime.operation_phase,
        Some(OperationPhase::Succeeded)
    );

    let Response::GetOperation(operation) = ok(host.handle(RequestEnvelope::v1(
        "get-operation",
        Request::GetOperation(GetOperationParams {
            operation_id: started.operation.id.clone(),
        }),
    ))) else {
        panic!("unexpected operation response");
    };
    assert_eq!(operation, started.operation);

    let Response::SubscribeEvents(subscription) = ok(host.handle(RequestEnvelope::v1(
        "subscribe-events",
        Request::SubscribeEvents(SubscribeEventsParams {
            after_sequence: None,
            replay_limit: 1_000,
        }),
    ))) else {
        panic!("unexpected subscription response");
    };
    let phases: Vec<_> = subscription
        .replay
        .iter()
        .filter_map(|event| match &event.event {
            HostEventKind::OperationChanged { operation }
                if operation.id == started.operation.id =>
            {
                Some(operation.phase)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![
            OperationPhase::Queued,
            OperationPhase::Running,
            OperationPhase::Succeeded,
        ]
    );
}

#[test]
fn event_subscription_has_a_live_handoff_and_detectable_replay_gaps() {
    let host = FakeHost::new(FakeHostConfig {
        event_history_capacity: 2,
        ..FakeHostConfig::default()
    });
    register(&host, definition("api", &[]), "register-api");
    let Response::SubscribeEvents(subscription) = ok(host.handle(RequestEnvelope::v1(
        "subscribe-after-eviction",
        Request::SubscribeEvents(SubscribeEventsParams {
            after_sequence: Some(0),
            replay_limit: 100,
        }),
    ))) else {
        panic!("unexpected subscription response");
    };
    assert!(subscription.replay_gap);
    assert_eq!(subscription.replay.len(), 2);
    assert!(subscription.oldest_available_sequence > 1);
    let stream = host
        .take_event_stream(&subscription.subscription_id)
        .expect("transport should take the stream once");
    assert!(host
        .take_event_stream(&subscription.subscription_id)
        .is_none());

    ensure_running(&host, "api", "start-api");
    let mut saw_service_state = false;
    for _ in 0..8 {
        let event = stream.recv_timeout(Duration::from_millis(50)).unwrap();
        if matches!(event.event, HostEventKind::ServiceStateChanged { .. }) {
            saw_service_state = true;
            break;
        }
    }
    assert!(saw_service_state);
}

#[test]
fn reconcile_starts_on_host_start_services_and_their_manual_dependencies() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("database", &[]), "register-database");
    let mut api = definition("api", &["database"]);
    api.start_policy = StartPolicy::OnHostStart;
    register(&host, api, "register-api");
    assert!(host
        .service_snapshot(&ServiceId::from("api"))
        .unwrap()
        .runtime
        .run_id
        .is_none());

    let response = host.handle(RequestEnvelope::v1(
        "reconcile-all",
        Request::Reconcile(ReconcileParams {
            service_id: None,
            mutation: mutation("reconcile-all"),
        }),
    ));
    let Response::Reconcile(result) = ok(response) else {
        panic!("unexpected reconcile response");
    };
    assert_eq!(result.operation.phase, OperationPhase::Succeeded);
    assert_eq!(host.recorded_start_order(), ids(&["database", "api"]));
}

#[test]
fn a_new_host_applies_persisted_on_host_start_policy() {
    let registry = Arc::new(MemoryRegistry::new());
    {
        let host = FakeHost::with_registry(FakeHostConfig::default(), registry.clone()).unwrap();
        register(&host, definition("database", &[]), "register-database");
        let mut api = definition("api", &["database"]);
        api.start_policy = StartPolicy::OnHostStart;
        register(&host, api, "register-api");
    }

    let restarted = FakeHost::with_registry(FakeHostConfig::default(), registry).unwrap();
    assert_eq!(restarted.recorded_start_order(), ids(&["database", "api"]));
    assert!(restarted
        .service_snapshot(&ServiceId::from("api"))
        .unwrap()
        .runtime
        .run_id
        .is_some());
}

#[test]
fn shutdown_continues_after_a_timeout_and_finishes_when_cleanup_is_observed() {
    let host = FakeHost::with_process_backend(
        FakeHostConfig::default(),
        Arc::new(MemoryRegistry::new()),
        Arc::new(TimeoutBackend),
    )
    .unwrap();
    register(&host, definition("other", &[]), "register-other");
    register(&host, definition("stuck", &[]), "register-stuck");
    ensure_running(&host, "other", "start-other");
    ensure_running(&host, "stuck", "start-stuck");
    let stuck_run = current_run(&host, "stuck");

    let failure = error(host.handle(RequestEnvelope::v1(
        "shutdown-with-timeout",
        Request::ShutdownHost(ShutdownHostParams {
            mutation: mutation("shutdown-with-timeout"),
        }),
    )));
    assert_eq!(failure.code, ErrorCode::Internal);
    assert_eq!(host.host_phase(), HostPhase::ShuttingDown);
    assert!(host
        .service_snapshot(&ServiceId::from("other"))
        .unwrap()
        .runtime
        .run_id
        .is_none());
    assert_eq!(
        host.service_snapshot(&ServiceId::from("stuck"))
            .unwrap()
            .runtime
            .last_stop_outcome,
        Some(StopOutcome::TimedOut)
    );

    assert!(host
        .simulate_exit(&ServiceId::from("stuck"), &stuck_run, None)
        .unwrap());
    assert!(host.complete_shutdown_if_quiescent());
    assert_eq!(host.host_phase(), HostPhase::Stopped);
}
