mod common;

use common::*;
use persistent_runtime_host::*;

#[test]
fn start_waits_for_dependencies_and_stopping_a_service_keeps_its_dependencies() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("database", &[]), "register-database");
    register(&host, definition("api", &["database"]), "register-api");

    ensure_running(&host, "api", "start-api");
    assert_eq!(host.recorded_start_order(), ids(&["database", "api"]));

    let response = ensure_stopped(&host, "api", false, "stop-api");
    ok(response);
    assert_eq!(host.recorded_stop_order(), ids(&["api"]));
    assert_eq!(
        host.service_snapshot(&ServiceId::from("database"))
            .unwrap()
            .runtime
            .observed_state,
        ObservedState::Running
    );
}

#[test]
fn dependency_stop_is_rejected_by_default_and_cascade_is_reverse_ordered() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("database", &[]), "register-database");
    register(&host, definition("api", &["database"]), "register-api");
    register(&host, definition("worker", &["api"]), "register-worker");
    ensure_running(&host, "worker", "start-worker");
    host.clear_order_history();

    let rejected = ensure_stopped(&host, "database", false, "stop-database-rejected");
    let rejected = error(rejected);
    assert_eq!(rejected.code, ErrorCode::DependencyInUse);
    assert_eq!(
        host.service_snapshot(&ServiceId::from("database"))
            .unwrap()
            .runtime
            .observed_state,
        ObservedState::Running
    );

    ok(ensure_stopped(
        &host,
        "database",
        true,
        "stop-database-cascade",
    ));
    assert_eq!(
        host.recorded_stop_order(),
        ids(&["worker", "api", "database"])
    );
}

#[test]
fn restart_of_dependency_requires_explicit_cascade_and_restores_dependents() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("database", &[]), "register-database");
    register(&host, definition("api", &["database"]), "register-api");
    ensure_running(&host, "api", "start-api");
    let original_db_run = current_run(&host, "database");
    let original_api_run = current_run(&host, "api");

    let rejected = host.handle(RequestEnvelope::v1(
        "restart-database-rejected",
        Request::Restart(RestartParams {
            service_id: ServiceId::from("database"),
            cascade: false,
            mutation: mutation("restart-database-rejected"),
        }),
    ));
    assert_eq!(error(rejected).code, ErrorCode::DependencyInUse);

    restart(&host, "database", true, "restart-database-cascade");
    assert_ne!(current_run(&host, "database"), original_db_run);
    assert_ne!(current_run(&host, "api"), original_api_run);
}

#[test]
fn cycle_introduced_by_update_is_rejected() {
    let host = FakeHost::new(FakeHostConfig::default());
    register(&host, definition("a", &[]), "register-a");
    register(&host, definition("b", &["a"]), "register-b");
    let response = host.handle(RequestEnvelope::v1(
        "cycle-update",
        Request::UpdateService(UpdateServiceParams {
            service_id: ServiceId::from("a"),
            definition: definition("a", &["b"]),
            mutation: mutation_at("cycle-update", 1),
        }),
    ));
    assert_eq!(error(response).code, ErrorCode::DependencyCycle);
    assert!(host
        .service_snapshot(&ServiceId::from("a"))
        .unwrap()
        .definition
        .dependencies
        .is_empty());
}
