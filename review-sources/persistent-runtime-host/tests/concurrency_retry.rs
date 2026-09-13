mod common;

use common::*;
use persistent_runtime_host::*;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn retry_after_lost_response_returns_the_original_operation_and_run() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("api", &[]), "register-api");
    let request = Request::EnsureRunning(EnsureRunningParams {
        service_id: ServiceId::from("api"),
        mutation: mutation("start-api-once"),
    });

    // The first response is deliberately treated as disconnected/lost.
    let first = host.handle(RequestEnvelope::v1("first-connection", request.clone()));
    let second = host.handle(RequestEnvelope::v1("retry-connection", request));
    let Response::EnsureRunning(first) = ok(first) else {
        panic!("unexpected first response");
    };
    let Response::EnsureRunning(second) = ok(second) else {
        panic!("unexpected retry response");
    };
    assert_eq!(first.operation.id, second.operation.id);
    assert_eq!(first.service.runtime.run_id, second.service.runtime.run_id);
    assert_eq!(host.recorded_start_order(), ids(&["api"]));
}

#[test]
fn reusing_an_idempotency_key_for_different_input_is_a_typed_conflict() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("a", &[]), "register-a");
    register(&host, definition("b", &[]), "register-b");
    ensure_running(&host, "a", "shared-key");

    let response = host.handle(RequestEnvelope::v1(
        "different-request",
        Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::from("b"),
            mutation: mutation("shared-key"),
        }),
    ));
    assert_eq!(error(response).code, ErrorCode::IdempotencyConflict);
    assert!(host
        .service_snapshot(&ServiceId::from("b"))
        .unwrap()
        .runtime
        .run_id
        .is_none());
}

#[test]
fn concurrent_start_requests_converge_on_one_run_generation() {
    const CALLERS: usize = 24;
    let host = Arc::new(FakeHost::new(FakeHostConfig::default()));
    register(&host, definition("api", &[]), "register-api");
    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut threads = Vec::new();
    for caller in 0..CALLERS {
        let host = host.clone();
        let barrier = barrier.clone();
        threads.push(thread::spawn(move || {
            barrier.wait();
            let response = host.handle(RequestEnvelope::v1(
                format!("concurrent-{caller}"),
                Request::EnsureRunning(EnsureRunningParams {
                    service_id: ServiceId::from("api"),
                    mutation: mutation(&format!("concurrent-key-{caller}")),
                }),
            ));
            let Response::EnsureRunning(result) = ok(response) else {
                panic!("unexpected concurrent response");
            };
            result.service.runtime.run_id.unwrap()
        }));
    }

    let runs: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(runs.iter().all(|run| run == &runs[0]));
    assert_eq!(host.recorded_start_order(), ids(&["api"]));
}
