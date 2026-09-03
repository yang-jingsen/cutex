use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;
use crate::role_revision::*;

fn private_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("role-seat-{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).expect("create test root");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root mode");
    }
    root
}

fn project() -> ProjectId {
    ProjectId::new("project-a").unwrap()
}

fn family_id() -> RoleFamilyId {
    RoleFamilyId::new("family-a").unwrap()
}

fn initialization() -> InitializationId {
    InitializationId::new("initialization-a").unwrap()
}

fn timestamp() -> Rfc3339 {
    Rfc3339::new("2026-08-17T00:00:00Z").unwrap()
}

fn sha(character: char) -> Sha256 {
    Sha256::new(character.to_string().repeat(64)).unwrap()
}

fn incumbent() -> DurableSessionRef {
    DurableSessionRef {
        cutex_session_id: CutexSessionId::new("cutex-incumbent").unwrap(),
        durable_revision: DurableRevision::new(3).unwrap(),
    }
}

fn receipt(kind: EvidenceKind, subjects: Vec<IdentityRef>, suffix: &str) -> EvidenceRef {
    EvidenceRef {
        kind,
        receipt_id: ReceiptId::new(format!("receipt-{suffix}")).unwrap(),
        receipt_sha256: sha('a'),
        subjects,
        occurred_at: EvidenceTime::Known {
            rfc3339: timestamp(),
        },
    }
}

fn initialize_input() -> InitializeFamilyRequest {
    let incumbent = incumbent();
    InitializeFamilyRequest {
        project_id: project(),
        role_family_id: family_id(),
        role_key: RoleKey::new("runtime").unwrap(),
        initialization_id: initialization(),
        chosen_root_revision: RoleRevisionNumber::new(7).unwrap(),
        incumbent: incumbent.clone(),
        human_approval_id: HumanApprovalId::new("approval-a").unwrap(),
        approval_evidence: receipt(
            EvidenceKind::HumanApproval,
            vec![
                IdentityRef::HumanApproval {
                    id: HumanApprovalId::new("approval-a").unwrap(),
                },
                IdentityRef::Project { id: project() },
                IdentityRef::RoleFamily { id: family_id() },
                IdentityRef::CutexSession {
                    id: incumbent.cutex_session_id.clone(),
                },
            ],
            "approval",
        ),
        initialization_evidence: receipt(
            EvidenceKind::RootInitialization,
            vec![
                IdentityRef::Project { id: project() },
                IdentityRef::RoleFamily { id: family_id() },
                IdentityRef::CutexSession {
                    id: incumbent.cutex_session_id,
                },
            ],
            "initialization",
        ),
        effective_at: EvidenceTime::Known {
            rfc3339: timestamp(),
        },
    }
}

fn request(id: &str, expected: u64) -> RequestEnvelope {
    let mut request = RequestEnvelope {
        schema: RequestSchema::V1,
        request_id: RequestId::new(id).unwrap(),
        request_digest_sha256: sha('0'),
        expected_store_revision: StoreRevision::new(expected).unwrap(),
        request: MutationRequest::InitializeFamily(initialize_input()),
    };
    request.request_digest_sha256 = canonical_request_digest(&request).unwrap();
    request
}

fn initialized_family() -> RoleFamily {
    let input = initialize_input();
    let root_number = input.chosen_root_revision;
    let root_initialization = RootInitialization {
        initialization_id: input.initialization_id.clone(),
        chosen_root_revision: root_number,
        incumbent: input.incumbent.clone(),
        approval_evidence: input.approval_evidence,
        initialization_evidence: input.initialization_evidence,
        effective_at: input.effective_at.clone(),
        recorded_at: timestamp(),
    };
    let mut revisions = BTreeMap::new();
    revisions.insert(
        root_number,
        RoleRevision {
            role_revision: root_number,
            session: Some(input.incumbent.clone()),
            state: RoleRevisionState::InitializedCurrent,
            intended_predecessor: None,
            successful_predecessor: None,
            root_revision: Some(root_number),
            allocated_at: timestamp(),
            terminal_attempt: None,
        },
    );
    RoleFamily {
        role_family_id: input.role_family_id,
        project_id: input.project_id,
        role_key: input.role_key,
        root_initialization,
        next_role_revision: root_number.checked_next().unwrap(),
        current_authority: CurrentAuthority {
            role_revision: root_number,
            cutex_session_id: input.incumbent.cutex_session_id,
            authority_epoch: AuthorityEpoch::new(1).unwrap(),
            effective_at: input.effective_at,
            established_by: EstablishedBy::RootInitialization {
                initialization_id: input.initialization_id,
            },
        },
        active_rotation: None,
        revisions,
        transitions: BTreeMap::new(),
    }
}

fn initialize_plan() -> PlannedMutation {
    PlannedMutation {
        family: Some(initialized_family()),
        result: MutationResult::InitializeFamily {
            role_family_id: family_id(),
            root_revision: RoleRevisionNumber::new(7).unwrap(),
            authority_epoch: AuthorityEpoch::new(1).unwrap(),
        },
    }
}

fn expect_committed(outcome: MutationOutcome) -> MutationResponse {
    match outcome {
        MutationOutcome::Committed(response) => response,
        other => panic!("expected committed response, got {other:?}"),
    }
}

fn private_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0
    }
}

#[test]
fn empty_load_first_commit_and_reopen_validate() {
    let root = private_root("reopen");
    let repository = RoleSeatRepository::new(&root).unwrap();
    assert_eq!(repository.load().unwrap().store_revision.get(), 1);
    assert!(fs::read_dir(&root).unwrap().next().is_none());

    let first = request("request-first", 1);
    let response = expect_committed(repository.mutate(&first, |_| Ok(initialize_plan())));
    assert_eq!(response.committed_store_revision.get(), 2);
    let reopened = RoleSeatRepository::new(&root).unwrap();
    let store = reopened.load().unwrap();
    assert_eq!(store.store_revision.get(), 2);
    validate_store(&store).unwrap();
    assert_eq!(private_mode(&root.join(STORE_FILE)), 0o600);
    assert_eq!(private_mode(&root.join(LOCK_FILE)), 0o600);
}

#[test]
fn replay_after_later_commit_returns_original_and_writes_nothing() {
    let root = private_root("replay");
    let repository = RoleSeatRepository::new(&root).unwrap();
    let first = request("request-first", 1);
    let original = expect_committed(repository.mutate(&first, |_| Ok(initialize_plan())));
    let later = request("request-later", 2);
    let later_response = expect_committed(repository.mutate(&later, |store| {
        Ok(PlannedMutation {
            family: store.family.clone(),
            result: initialize_plan().result,
        })
    }));
    assert_eq!(later_response.committed_store_revision.get(), 3);
    let before_replay = fs::read(root.join(STORE_FILE)).unwrap();
    let replay = expect_committed(repository.mutate(&first, |_| panic!("replay invoked planner")));
    assert_eq!(replay, original);
    assert_eq!(fs::read(root.join(STORE_FILE)).unwrap(), before_replay);

    let mut conflict = first.clone();
    conflict.expected_store_revision = StoreRevision::new(3).unwrap();
    conflict.request_digest_sha256 = canonical_request_digest(&conflict).unwrap();
    assert_eq!(
        repository.mutate(&conflict, |_| panic!("conflict invoked planner")),
        MutationOutcome::NoWrite(RepositoryError::RequestConflict)
    );
}

#[test]
fn writers_serialize_and_stale_cas_does_not_plan_or_write() {
    let root = private_root("writers");
    let barrier = Arc::new(Barrier::new(3));
    let planned = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for id in ["writer-a", "writer-b"] {
        let root = root.clone();
        let barrier = Arc::clone(&barrier);
        let planned = Arc::clone(&planned);
        handles.push(thread::spawn(move || {
            let repository = RoleSeatRepository::new(root).unwrap();
            let request = request(id, 1);
            barrier.wait();
            repository.mutate(&request, |_| {
                planned.fetch_add(1, Ordering::SeqCst);
                Ok(initialize_plan())
            })
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, MutationOutcome::Committed(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                MutationOutcome::NoWrite(RepositoryError::StoreRevisionConflict { .. })
            ))
            .count(),
        1
    );
    assert_eq!(planned.load(Ordering::SeqCst), 1);

    let repository = RoleSeatRepository::new(&root).unwrap();
    let before = fs::read(root.join(STORE_FILE)).unwrap();
    let stale = request("writer-stale", 1);
    assert!(matches!(
        repository.mutate(&stale, |_| panic!("stale CAS invoked planner")),
        MutationOutcome::NoWrite(RepositoryError::StoreRevisionConflict { .. })
    ));
    assert_eq!(fs::read(root.join(STORE_FILE)).unwrap(), before);
}

#[test]
fn insecure_root_and_invalid_store_are_rejected_without_target_change() {
    let insecure = private_root("insecure");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&insecure, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(matches!(
        RoleSeatRepository::new(&insecure),
        Err(RepositoryError::RootModeMismatch)
    ));

    let root = private_root("invalid");
    let target = root.join(STORE_FILE);
    fs::write(&target, b"{invalid-json").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let before = fs::read(&target).unwrap();
    let repository = RoleSeatRepository::new(&root).unwrap();
    assert_eq!(repository.load().unwrap_err(), RepositoryError::InvalidJson);
    assert!(matches!(
        repository.mutate(&request("invalid-store", 1), |_| Ok(initialize_plan())),
        MutationOutcome::NoWrite(RepositoryError::InvalidJson)
    ));
    assert_eq!(fs::read(target).unwrap(), before);
}

#[test]
fn pre_replace_failure_leaves_target_unchanged() {
    let root = private_root("pre-replace");
    let repository =
        RoleSeatRepository::with_test_fault(&root, persist::FaultPoint::BeforeReplace).unwrap();
    assert_eq!(
        repository.mutate(&request("pre-replace", 1), |_| Ok(initialize_plan())),
        MutationOutcome::NoWrite(RepositoryError::InjectedPreReplaceFailure)
    );
    assert!(!root.join(STORE_FILE).exists());
}

#[test]
fn post_replace_unknown_reconciles_by_same_request_without_second_commit() {
    let root = private_root("post-replace");
    let request = request("post-replace", 1);
    let repository =
        RoleSeatRepository::with_test_fault(&root, persist::FaultPoint::AfterReplace).unwrap();
    assert_eq!(
        repository.mutate(&request, |_| Ok(initialize_plan())),
        MutationOutcome::PersistenceUnknown {
            request_id: request.request_id.clone(),
            phase: PersistencePhase::AfterReplace,
        }
    );
    let reopened = RoleSeatRepository::new(&root).unwrap();
    let reconciled = match reopened.get_request_result(&request) {
        RequestLookup::Committed(response) => response,
        other => panic!("expected reconciliation, got {other:?}"),
    };
    assert_eq!(reconciled.committed_store_revision.get(), 2);
    let before_replay = fs::read(root.join(STORE_FILE)).unwrap();
    assert_eq!(
        expect_committed(reopened.mutate(&request, |_| panic!("reconciliation replay planned"))),
        reconciled
    );
    assert_eq!(reopened.load().unwrap().store_revision.get(), 2);
    assert_eq!(fs::read(root.join(STORE_FILE)).unwrap(), before_replay);
}

#[test]
fn dishonest_digest_is_rejected_before_any_repository_write() {
    let root = private_root("digest");
    let repository = RoleSeatRepository::new(&root).unwrap();
    let mut request = request("dishonest", 1);
    request.request_digest_sha256 = sha('f');
    assert_eq!(
        repository.mutate(&request, |_| panic!("dishonest request planned")),
        MutationOutcome::NoWrite(RepositoryError::RequestDigestMismatch)
    );
    assert!(fs::read_dir(root).unwrap().next().is_none());
}
