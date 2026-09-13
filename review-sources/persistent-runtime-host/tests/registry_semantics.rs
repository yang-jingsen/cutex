mod common;

use common::*;
use persistent_runtime_host::registry::{validate_dependency_graph, DependencyGraphError};
use persistent_runtime_host::*;
use std::collections::BTreeMap;
use std::sync::Arc;

struct RejectingRegistry;

impl RegistryStore for RejectingRegistry {
    fn load(
        &self,
    ) -> Result<RegistrySnapshot, persistent_runtime_host::registry::RegistryStoreError> {
        Ok(RegistrySnapshot::default())
    }

    fn replace(
        &self,
        expected_registry_revision: u64,
        _definitions: BTreeMap<ServiceId, StoredDefinition>,
    ) -> Result<RegistrySnapshot, persistent_runtime_host::registry::RegistryStoreError> {
        Err(
            persistent_runtime_host::registry::RegistryStoreError::Conflict {
                expected: expected_registry_revision,
                actual: expected_registry_revision + 1,
            },
        )
    }
}

#[test]
fn memory_registry_compare_and_swap_rejects_lost_update() {
    let registry = MemoryRegistry::new();
    let first_view = registry.load().unwrap();
    let second_view = registry.load().unwrap();

    let first = registry
        .replace(first_view.revision, BTreeMap::new())
        .unwrap();
    assert_eq!(first.revision, 1);

    let error = registry
        .replace(second_view.revision, BTreeMap::new())
        .unwrap_err();
    assert_eq!(
        error,
        persistent_runtime_host::registry::RegistryStoreError::Conflict {
            expected: 0,
            actual: 1,
        }
    );
}

#[test]
fn graph_validation_reports_missing_dependency_and_cycle() {
    let mut definitions = BTreeMap::new();
    definitions.insert(
        ServiceId::from("api"),
        StoredDefinition {
            definition_revision: 1,
            definition: definition("api", &["database"]),
        },
    );
    assert_eq!(
        validate_dependency_graph(&definitions),
        Err(DependencyGraphError::Missing {
            service_id: ServiceId::from("api"),
            dependency_id: ServiceId::from("database"),
        })
    );

    definitions.insert(
        ServiceId::from("database"),
        StoredDefinition {
            definition_revision: 1,
            definition: definition("database", &["api"]),
        },
    );
    let Err(DependencyGraphError::Cycle { path }) = validate_dependency_graph(&definitions) else {
        panic!("expected a dependency cycle");
    };
    assert_eq!(path.first(), path.last());
    assert!(path.contains(&ServiceId::from("api")));
    assert!(path.contains(&ServiceId::from("database")));
}

#[test]
fn definition_revisions_are_monotonic_and_stale_writes_fail() {
    let registry = Arc::new(MemoryRegistry::new());
    let host = FakeHost::with_registry(FakeHostConfig::default(), registry.clone()).unwrap();
    let registered = register(&host, definition("api", &[]), "register-api");
    assert_eq!(registered.service.runtime.definition_revision, 1);
    assert_eq!(registered.registry_revision, 1);

    let mut updated_definition = definition("api", &[]);
    updated_definition.display_name = "Updated API".to_owned();
    let updated = host.handle(RequestEnvelope::v1(
        "update-api",
        Request::UpdateService(UpdateServiceParams {
            service_id: ServiceId::from("api"),
            definition: updated_definition.clone(),
            mutation: mutation_at("update-api-key", 1),
        }),
    ));
    let Response::UpdateService(updated) = ok(updated) else {
        panic!("unexpected update response");
    };
    assert_eq!(updated.service.runtime.definition_revision, 2);
    assert_eq!(updated.registry_revision, 2);

    updated_definition.display_name = "Stale write".to_owned();
    let stale = host.handle(RequestEnvelope::v1(
        "stale-update-api",
        Request::UpdateService(UpdateServiceParams {
            service_id: ServiceId::from("api"),
            definition: updated_definition,
            mutation: mutation_at("stale-update-key", 1),
        }),
    ));
    assert_eq!(error(stale).code, ErrorCode::RevisionMismatch);
    assert_eq!(
        host.service_snapshot(&ServiceId::from("api"))
            .unwrap()
            .definition
            .display_name,
        "Updated API"
    );
    assert_eq!(registry.load().unwrap().revision, 2);
}

#[test]
fn invalid_and_missing_dependency_definitions_are_rejected_before_persistence() {
    let registry = Arc::new(MemoryRegistry::new());
    let host = FakeHost::with_registry(FakeHostConfig::default(), registry.clone()).unwrap();
    let mut invalid = definition("api", &["missing"]);
    invalid.executable = "relative/program".to_owned();
    let response = host.handle(RequestEnvelope::v1(
        "invalid-definition",
        Request::RegisterService(RegisterServiceParams {
            definition: invalid,
            mutation: mutation("invalid-definition"),
        }),
    ));
    assert_eq!(error(response).code, ErrorCode::InvalidArgument);

    let response = host.handle(RequestEnvelope::v1(
        "missing-dependency",
        Request::RegisterService(RegisterServiceParams {
            definition: definition("api", &["missing"]),
            mutation: mutation("missing-dependency"),
        }),
    ));
    assert_eq!(error(response).code, ErrorCode::DependencyMissing);
    assert_eq!(registry.load().unwrap().revision, 0);
}

#[test]
fn persistence_failure_finishes_the_started_operation_as_failed() {
    let host =
        FakeHost::with_registry(FakeHostConfig::default(), Arc::new(RejectingRegistry)).unwrap();
    let response = host.handle(RequestEnvelope::v1(
        "register-with-conflict",
        Request::RegisterService(RegisterServiceParams {
            definition: definition("api", &[]),
            mutation: mutation("register-with-conflict"),
        }),
    ));
    assert_eq!(error(response).code, ErrorCode::RegistryConflict);

    let Response::SubscribeEvents(events) = ok(host.handle(RequestEnvelope::v1(
        "operation-events",
        Request::SubscribeEvents(SubscribeEventsParams {
            after_sequence: None,
            replay_limit: 100,
        }),
    ))) else {
        panic!("unexpected event response");
    };
    let phases: Vec<_> = events
        .replay
        .iter()
        .filter_map(|event| match &event.event {
            HostEventKind::OperationChanged { operation }
                if operation.kind == OperationKind::RegisterService =>
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
            OperationPhase::Failed,
        ]
    );
}
