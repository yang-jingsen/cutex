#![allow(dead_code)]

use persistent_runtime_host::*;
use std::collections::BTreeMap;

pub fn definition(id: &str, dependencies: &[&str]) -> ServiceDefinition {
    ServiceDefinition {
        id: ServiceId::from(id),
        display_name: format!("Test {id}"),
        description: Some(format!("isolated fixture for {id}")),
        executable: format!("/fixtures/{id}"),
        arguments: vec!["--fixture".to_owned(), id.to_owned()],
        working_directory: format!("/tmp/prh-fixtures/{id}"),
        environment: BTreeMap::from([("PRH_FIXTURE".to_owned(), id.to_owned())]),
        start_policy: StartPolicy::Manual,
        restart_policy: RestartPolicy::Never,
        dependencies: dependencies.iter().copied().map(ServiceId::from).collect(),
        readiness_probe: None,
        shutdown_policy: ShutdownPolicy {
            graceful_timeout_ms: 1_000,
            force_kill_timeout_ms: 1_000,
        },
        metadata: BTreeMap::new(),
    }
}

pub fn mutation(key: &str) -> MutationOptions {
    MutationOptions::new(key)
}

pub fn mutation_at(key: &str, revision: u64) -> MutationOptions {
    MutationOptions {
        idempotency_key: key.to_owned(),
        expected_revision: Some(revision),
    }
}

pub fn register(
    host: &FakeHost,
    definition: ServiceDefinition,
    key: &str,
) -> DefinitionMutationResult {
    match ok(host.handle(RequestEnvelope::v1(
        format!("request-{key}"),
        Request::RegisterService(RegisterServiceParams {
            definition,
            mutation: mutation(key),
        }),
    ))) {
        Response::RegisterService(result) => result,
        response => panic!("unexpected response: {response:?}"),
    }
}

pub fn ensure_running(host: &FakeHost, id: &str, key: &str) -> ServiceMutationResult {
    match ok(host.handle(RequestEnvelope::v1(
        format!("request-{key}"),
        Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::from(id),
            mutation: mutation(key),
        }),
    ))) {
        Response::EnsureRunning(result) => result,
        response => panic!("unexpected response: {response:?}"),
    }
}

pub fn ensure_stopped(host: &FakeHost, id: &str, cascade: bool, key: &str) -> ResponseEnvelope {
    host.handle(RequestEnvelope::v1(
        format!("request-{key}"),
        Request::EnsureStopped(EnsureStoppedParams {
            service_id: ServiceId::from(id),
            cascade,
            mutation: mutation(key),
        }),
    ))
}

pub fn restart(host: &FakeHost, id: &str, cascade: bool, key: &str) -> ServiceMutationResult {
    match ok(host.handle(RequestEnvelope::v1(
        format!("request-{key}"),
        Request::Restart(RestartParams {
            service_id: ServiceId::from(id),
            cascade,
            mutation: mutation(key),
        }),
    ))) {
        Response::Restart(result) => result,
        response => panic!("unexpected response: {response:?}"),
    }
}

pub fn ok(envelope: ResponseEnvelope) -> Response {
    match envelope.outcome {
        ResponseOutcome::Ok { response } => *response,
        ResponseOutcome::Error { error } => panic!("unexpected API error: {error:?}"),
    }
}

pub fn error(envelope: ResponseEnvelope) -> ApiError {
    match envelope.outcome {
        ResponseOutcome::Error { error } => error,
        ResponseOutcome::Ok { response } => panic!("unexpected success: {response:?}"),
    }
}

pub fn current_run(host: &FakeHost, id: &str) -> RunId {
    host.service_snapshot(&ServiceId::from(id))
        .and_then(|service| service.runtime.run_id)
        .expect("service should have a current run")
}

pub fn ids(ids: &[&str]) -> Vec<ServiceId> {
    ids.iter().copied().map(ServiceId::from).collect()
}
