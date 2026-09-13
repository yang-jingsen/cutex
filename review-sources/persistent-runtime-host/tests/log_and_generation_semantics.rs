mod common;

use common::*;
use persistent_runtime_host::*;

#[test]
fn stale_exit_notification_cannot_mutate_a_new_run() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("api", &[]), "register-api");
    ensure_running(&host, "api", "start-api");
    let first_run = current_run(&host, "api");
    restart(&host, "api", false, "restart-api");
    let second_run = current_run(&host, "api");
    assert_ne!(first_run, second_run);

    assert!(!host
        .simulate_exit(&ServiceId::from("api"), &first_run, Some(17))
        .unwrap());
    assert_eq!(current_run(&host, "api"), second_run);

    host.emit_log(
        &ServiceId::from("api"),
        &first_run,
        LogStream::Stdout,
        b"old occurrence",
    )
    .unwrap();
    host.emit_log(
        &ServiceId::from("api"),
        &second_run,
        LogStream::Stdout,
        b"new occurrence",
    )
    .unwrap();
    let response = host.handle(RequestEnvelope::v1(
        "read-new-run",
        Request::ReadLogs(ReadLogsParams {
            service_id: ServiceId::from("api"),
            run_id: Some(second_run.clone()),
            after_sequence: None,
            limit: 100,
        }),
    ));
    let Response::ReadLogs(page) = ok(response) else {
        panic!("unexpected log response");
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].run_id, second_run);
    assert_eq!(page.entries[0].data, "new occurrence");
}

#[test]
fn unhealthy_probe_does_not_claim_process_absence_or_create_another_instance() {
    let host = FakeHost::new(FakeHostConfig::default());
    let mut service = definition("api", &[]);
    service.readiness_probe = Some(ReadinessProbe::Tcp {
        host: "127.0.0.1".to_owned(),
        port: 4310,
        interval_ms: 500,
        timeout_ms: 100,
    });
    register(&host, service, "register-api");
    ensure_running(&host, "api", "start-api");
    let run_id = current_run(&host, "api");
    let process = host
        .service_snapshot(&ServiceId::from("api"))
        .unwrap()
        .runtime
        .process
        .unwrap();

    assert!(host
        .set_health(&ServiceId::from("api"), &run_id, HealthState::Unhealthy)
        .unwrap());
    ensure_running(&host, "api", "ensure-unhealthy-api");
    let state = host
        .service_snapshot(&ServiceId::from("api"))
        .unwrap()
        .runtime;
    assert_eq!(state.run_id, Some(run_id));
    assert_eq!(state.process, Some(process));
    assert_eq!(state.health, HealthState::Unhealthy);
    assert_eq!(host.recorded_start_order(), ids(&["api"]));
}

#[test]
fn bounded_restart_policy_stops_after_its_budget() {
    let host = FakeHost::new(FakeHostConfig::default());
    let mut service = definition("api", &[]);
    service.restart_policy = RestartPolicy::BoundedOnFailure {
        max_restarts: 1,
        window_ms: 60_000,
        backoff_ms: 0,
    };
    register(&host, service, "register-api");
    ensure_running(&host, "api", "start-api");
    let first_run = current_run(&host, "api");
    assert!(host
        .simulate_exit(&ServiceId::from("api"), &first_run, Some(1))
        .unwrap());
    let second_run = current_run(&host, "api");
    assert_ne!(first_run, second_run);
    assert_eq!(
        host.service_snapshot(&ServiceId::from("api"))
            .unwrap()
            .runtime
            .restart_count,
        1
    );

    assert!(host
        .simulate_exit(&ServiceId::from("api"), &second_run, Some(2))
        .unwrap());
    let state = host
        .service_snapshot(&ServiceId::from("api"))
        .unwrap()
        .runtime;
    assert!(state.run_id.is_none());
    assert_eq!(state.desired_state, DesiredState::Running);
    assert_eq!(state.observed_state, ObservedState::Failed);
}

#[test]
fn slow_log_subscriber_never_blocks_drain_and_loss_is_observable() {
    let host = FakeHost::new(FakeHostConfig {
        log_buffer: LogBufferConfig {
            max_entries_per_service: 3,
            max_bytes_per_service: 128,
        },
        ..FakeHostConfig::default()
    });
    register(&host, definition("api", &[]), "register-api");
    ensure_running(&host, "api", "start-api");
    let run_id = current_run(&host, "api");
    let subscriber = host.subscribe_logs(ServiceId::from("api"), Some(run_id.clone()), 1);

    for index in 0..10 {
        host.emit_log(
            &ServiceId::from("api"),
            &run_id,
            LogStream::Stdout,
            format!("line-{index}").as_bytes(),
        )
        .unwrap();
    }
    assert_eq!(subscriber.try_recv().unwrap().data, "line-0");

    let response = host.handle(RequestEnvelope::v1(
        "read-logs",
        Request::ReadLogs(ReadLogsParams {
            service_id: ServiceId::from("api"),
            run_id: Some(run_id),
            after_sequence: None,
            limit: 100,
        }),
    ));
    let Response::ReadLogs(page) = ok(response) else {
        panic!("unexpected log response");
    };
    assert_eq!(
        page.entries
            .iter()
            .map(|entry| entry.data.as_str())
            .collect::<Vec<_>>(),
        vec!["line-7", "line-8", "line-9"]
    );
    assert_eq!(page.diagnostics.evicted_entries, 7);
    assert_eq!(page.diagnostics.slow_subscriber_drops, 9);
}

#[test]
fn binary_output_is_encoded_oversize_output_is_bounded_and_failures_are_visible() {
    let host = FakeHost::new(FakeHostConfig {
        log_buffer: LogBufferConfig {
            max_entries_per_service: 10,
            max_bytes_per_service: 4,
        },
        ..FakeHostConfig::default()
    });
    register(&host, definition("api", &[]), "register-api");
    ensure_running(&host, "api", "start-api");
    let run_id = current_run(&host, "api");
    let entry = host
        .emit_log(
            &ServiceId::from("api"),
            &run_id,
            LogStream::Stderr,
            &[0xff, 0x00, 0xfe, 0x41, 0x42],
        )
        .unwrap();
    assert_eq!(entry.encoding, LogEncoding::Base64);
    assert_eq!(entry.data, "/wD+QQ==");
    assert!(entry.truncated);

    host.report_log_failure(&ServiceId::from("api"), &run_id, "fixture sink failed")
        .unwrap();
    let Response::ReadLogs(page) = ok(host.handle(RequestEnvelope::v1(
        "read-failed-logs",
        Request::ReadLogs(ReadLogsParams {
            service_id: ServiceId::from("api"),
            run_id: Some(run_id),
            after_sequence: None,
            limit: 10,
        }),
    ))) else {
        panic!("unexpected log response");
    };
    assert_eq!(
        page.diagnostics.last_error.as_deref(),
        Some("fixture sink failed")
    );
    assert_eq!(page.diagnostics.evicted_bytes, 1);
}
