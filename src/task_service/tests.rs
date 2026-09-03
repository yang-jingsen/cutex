use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::digest::canonical_command_digest;
use super::model::*;
use super::operations::TaskService;
use super::persist::FaultPoint;
use super::store::TrustedClock;

const SNAPSHOT: &str = "task-service-v1.json";
const JOURNAL: &str = "task-service-v1.events.jsonl";
const RECOVERY: &str = "task-service-v1.recovery";
const LOCK: &str = "task-service-v1.lock";

#[derive(Default)]
struct CountingClock {
    calls: AtomicUsize,
}

impl CountingClock {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TrustedClock for CountingClock {
    fn now(&self) -> Rfc3339 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Rfc3339::new("2026-08-21T00:00:00Z").expect("fixed time")
    }
}

fn service_root(label: &str) -> PathBuf {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let base = std::env::var_os("CUTEX_TASK_SERVICE_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("{label}-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).expect("create service root");
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private service root");
    #[cfg(windows)]
    crate::platform::private_fs::secure_directory(&root).expect("private service root");
    root
}

fn clocked_service(root: &Path, clock: Arc<CountingClock>) -> TaskService {
    TaskService::with_clock(root.to_path_buf(), clock).expect("valid service root")
}

fn faulted_service(root: &Path, clock: Arc<CountingClock>, fault: FaultPoint) -> TaskService {
    TaskService::with_clock_and_fault(root.to_path_buf(), clock, fault).expect("valid service root")
}

fn task_id(value: &str) -> TaskId {
    TaskId::new(value).expect("task ID")
}

fn receipt_id(value: &str) -> ReceiptId {
    ReceiptId::new(value).expect("receipt ID")
}

fn revision(value: u64) -> TaskRevision {
    TaskRevision::new(value).expect("task revision")
}

fn store_revision(value: u64) -> StoreRevision {
    StoreRevision::new(value).expect("store revision")
}

fn evidence() -> TransitionEvidence {
    TransitionEvidence::default()
}

fn specification(task: &str, revision_value: u64, body: &str) -> TaskSpecification {
    TaskSpecification {
        schema: SpecificationSchema::V1,
        task_id: task_id(task),
        task_revision: revision(revision_value),
        contract_sha256: sha256_bytes(body.as_bytes()),
        opaque_contract: body.to_owned(),
    }
}

fn signed_envelope(
    receipt: &str,
    expected_revision: u64,
    fence: Option<AttemptFence>,
    command: TaskCommand,
) -> TransitionEnvelope {
    let mut envelope = TransitionEnvelope {
        schema: EnvelopeSchema::V1,
        receipt_id: receipt_id(receipt),
        request_digest_sha256: zero_sha256(),
        expected_store_revision: store_revision(expected_revision),
        fence,
        command,
    };
    envelope.request_digest_sha256 =
        canonical_command_digest(&envelope).expect("canonical command digest");
    envelope
}

fn create_envelope(
    task: &str,
    revision_value: u64,
    receipt: &str,
    expected: u64,
) -> TransitionEnvelope {
    signed_envelope(
        receipt,
        expected,
        None,
        TaskCommand::CreateDraft(CreateDraftCommand {
            specification: specification(
                task,
                revision_value,
                &format!("opaque:{task}:{revision_value}"),
            ),
        }),
    )
}

fn publish_envelope(
    task: &str,
    revision_value: u64,
    receipt: &str,
    expected: u64,
) -> (TransitionEnvelope, AttemptFence) {
    let token = AttemptToken::new(format!("token:{task}:{revision_value}")).expect("attempt token");
    let owner = CutexSessionId::new(format!("owner:{task}")).expect("owner session");
    let generation = RuntimeGeneration::new(1).expect("generation");
    let fence = AttemptFence {
        task_id: task_id(task),
        task_revision: revision(revision_value),
        attempt_number: AttemptNumber::new(1).expect("attempt one"),
        attempt_token: token.clone(),
        owner_session_id: owner.clone(),
        runtime_generation: generation,
    };
    let envelope = signed_envelope(
        receipt,
        expected,
        None,
        TaskCommand::Publish(PublishCommand {
            task_id: task_id(task),
            task_revision: revision(revision_value),
            attempt_token: token,
            owner_session_id: owner,
            owner_durable_revision: DurableRevision::new(7).expect("durable revision"),
            runtime_generation: generation,
            runtime_agent_id: RuntimeAgentId::new(format!("runtime:{task}"))
                .expect("runtime agent ID"),
        }),
    );
    (envelope, fence)
}

fn transition(
    receipt: &str,
    expected: u64,
    fence: &AttemptFence,
    command: TaskCommand,
) -> TransitionEnvelope {
    signed_envelope(receipt, expected, Some(fence.clone()), command)
}

fn committed(outcome: TransitionOutcome) -> TransitionResponse {
    match outcome {
        TransitionOutcome::Committed(response) => response,
        other => panic!("expected committed transition, got {other:?}"),
    }
}

fn create_and_publish(
    service: &TaskService,
    task: &str,
    first_expected: u64,
    create_receipt: &str,
    publish_receipt: &str,
) -> AttemptFence {
    committed(service.transition(&create_envelope(task, 1, create_receipt, first_expected)));
    let (publish, fence) = publish_envelope(task, 1, publish_receipt, first_expected + 1);
    committed(service.transition(&publish));
    fence
}

fn advance_to_running(
    service: &TaskService,
    fence: &AttemptFence,
    mut expected: u64,
    prefix: &str,
) -> u64 {
    committed(service.transition(&transition(
        &format!("{prefix}-delivery"),
        expected,
        fence,
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id(&format!("{prefix}-external-delivery")),
            observed_at: None,
        }),
    )));
    expected += 1;
    committed(service.transition(&transition(
        &format!("{prefix}-accept"),
        expected,
        fence,
        TaskCommand::Accept(evidence()),
    )));
    expected += 1;
    committed(service.transition(&transition(
        &format!("{prefix}-start"),
        expected,
        fence,
        TaskCommand::Start(evidence()),
    )));
    expected + 1
}

fn assert_phase_path(label: &str, commands: Vec<TaskCommand>, expected_phase: TaskPhase) {
    let root = service_root(label);
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    let fence = create_and_publish(
        &service,
        label,
        1,
        &format!("{label}-create"),
        &format!("{label}-publish"),
    );
    let command_count = commands.len();
    for (index, command) in commands.into_iter().enumerate() {
        committed(service.transition(&transition(
            &format!("{label}-edge-{index}"),
            index as u64 + 3,
            &fence,
            command,
        )));
    }
    assert_eq!(
        service
            .get_task(&task_id(label), Some(revision(1)))
            .expect("query phase path")
            .expect("phase task")
            .phase,
        expected_phase
    );
    assert_eq!(clock.calls(), command_count + 2);
}

fn page_request(cursor: JournalCursor, task: Option<&str>, limit: u16) -> EventPageRequest {
    EventPageRequest {
        schema: PageSchema::V1,
        cursor,
        task_id: task.map(task_id),
        limit,
    }
}

fn subscription_request(
    cursor: JournalCursor,
    task: Option<&str>,
    limit: u16,
    capacity: u16,
) -> SubscriptionRequest {
    SubscriptionRequest {
        schema: SubscriptionSchema::V1,
        page: page_request(cursor, task, limit),
        capacity,
    }
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn durable_pair(root: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        read_optional(&root.join(SNAPSHOT)),
        read_optional(&root.join(JOURNAL)),
    )
}

#[derive(Debug, Eq, PartialEq)]
struct DurableRootImage {
    entries: Vec<(String, Vec<u8>)>,
    #[cfg(unix)]
    directory: (u32, u32, u32, u64, u64, u64, i64, i64, i64, i64),
    #[cfg(windows)]
    directory: crate::platform::private_fs::FileIdentity,
}

#[cfg(unix)]
fn durable_root_image(root: &Path) -> DurableRootImage {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let mut entries: Vec<_> = fs::read_dir(root)
        .expect("read durable root")
        .map(|entry| {
            let entry = entry.expect("durable root entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = fs::read(entry.path()).expect("regular durable entry");
            (name, bytes)
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let metadata = fs::metadata(root).expect("durable root metadata");
    DurableRootImage {
        entries,
        directory: (
            metadata.permissions().mode() & 0o7777,
            metadata.uid(),
            metadata.gid(),
            metadata.dev(),
            metadata.ino(),
            metadata.nlink(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ),
    }
}

#[cfg(windows)]
fn durable_root_image(root: &Path) -> DurableRootImage {
    let mut entries: Vec<_> = fs::read_dir(root)
        .expect("read durable root")
        .map(|entry| {
            let entry = entry.expect("durable root entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = fs::read(entry.path()).expect("regular durable entry");
            (name, bytes)
        })
        .collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let (directory, identity) =
        crate::platform::private_fs::open_validated_directory(root).expect("durable private root");
    drop(directory);
    DurableRootImage {
        entries,
        directory: identity,
    }
}

fn assert_page_recovery_read_only(root: &Path, service: &TaskService) {
    let page = page_request(JournalCursor::genesis(), None, 10);
    let before_page = durable_root_image(root);
    assert_eq!(
        service.page_events(&page),
        Err(TaskServiceError::RecoveryRequired)
    );
    assert_eq!(durable_root_image(root), before_page);

    let before_subscription = durable_root_image(root);
    assert!(matches!(
        service.page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2,)),
        Err(TaskServiceError::RecoveryRequired)
    ));
    assert_eq!(durable_root_image(root), before_subscription);
}

// T-EMPTY-RESTART: real CreateDraft event; a fresh service is the observer;
// the prebuilt specification/store is the oracle; an empty query is no-write.
#[test]
fn t_empty_restart() {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let root = service_root("empty-restart");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    assert_eq!(service.load().expect("empty load"), empty_store());
    assert_eq!(fs::read_dir(&root).expect("read root").count(), 0);

    let envelope = create_envelope("empty-task", 1, "empty-create", 1);
    let response = committed(service.transition(&envelope));
    assert_eq!(response.committed_store_revision, store_revision(2));
    let expected = service.load().expect("committed store");
    let before = durable_pair(&root);
    drop(service);

    let reopened = clocked_service(&root, clock.clone());
    assert_eq!(reopened.load().expect("reopened store"), expected);
    assert_eq!(durable_pair(&root), before);
    assert_eq!(clock.calls(), 1);
    #[cfg(unix)]
    for name in [LOCK, JOURNAL, SNAPSHOT] {
        let metadata = fs::metadata(root.join(name)).expect("owned private file");
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    }

    #[cfg(unix)]
    {
        let insecure_root = service_root("insecure-root");
        fs::set_permissions(&insecure_root, fs::Permissions::from_mode(0o755))
            .expect("insecure root fixture");
        assert!(matches!(
            TaskService::new(&insecure_root),
            Err(TaskServiceError::RootModeMismatch)
        ));
        assert_eq!(
            fs::metadata(&insecure_root)
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o755
        );

        let insecure_file_root = service_root("insecure-file");
        fs::write(insecure_file_root.join(LOCK), b"").expect("insecure lock fixture");
        fs::set_permissions(
            insecure_file_root.join(LOCK),
            fs::Permissions::from_mode(0o644),
        )
        .expect("insecure lock mode");
        let insecure_service = TaskService::new(&insecure_file_root).expect("root itself is valid");
        assert!(matches!(
            insecure_service.load(),
            Err(TaskServiceError::PrivateFileModeMismatch)
        ));
        assert_eq!(
            fs::metadata(insecure_file_root.join(LOCK))
                .expect("lock metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o644
        );
    }
}

// T-CAS: two real CreateDraft bodies race; journal bytes are the observer;
// one-event/store-revision-2 is immutable oracle; losing CAS is the control.
#[test]
fn t_cas() {
    let root = service_root("cas");
    let clock = Arc::new(CountingClock::default());
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for (task, receipt) in [("cas-a", "cas-ra"), ("cas-b", "cas-rb")] {
        let service = clocked_service(&root, clock.clone());
        let barrier = barrier.clone();
        let envelope = create_envelope(task, 1, receipt, 1);
        handles.push(thread::spawn(move || {
            barrier.wait();
            service.transition(&envelope)
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, TransitionOutcome::Committed(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(
                outcome,
                TransitionOutcome::NoWrite(TaskServiceError::StoreRevisionConflict { .. })
            ))
            .count(),
        1
    );
    let service = clocked_service(&root, clock.clone());
    let page = service
        .page_events(&page_request(JournalCursor::genesis(), None, 10))
        .expect("journal page");
    assert_eq!(page.records.len(), 1);
    assert_eq!(
        service.load().expect("store").store_revision,
        store_revision(2)
    );
    assert_eq!(clock.calls(), 1);
}

// T-FENCE: five independently mutated fence fields are real inputs; direct
// file bytes/clock are observers; the published fence is oracle/control.
#[test]
fn t_fence() {
    let root = service_root("fence");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    let fence = create_and_publish(&service, "fence-task", 1, "f-create", "f-publish");
    let baseline = durable_pair(&root);
    let clock_baseline = clock.calls();
    let mut stale = Vec::new();
    let mut wrong_revision = fence.clone();
    wrong_revision.task_revision = revision(2);
    stale.push(wrong_revision);
    let mut wrong_attempt = fence.clone();
    wrong_attempt.attempt_number = AttemptNumber::new(2).expect("attempt two");
    stale.push(wrong_attempt);
    let mut wrong_token = fence.clone();
    wrong_token.attempt_token = AttemptToken::new("wrong-token").expect("token");
    stale.push(wrong_token);
    let mut wrong_owner = fence.clone();
    wrong_owner.owner_session_id = CutexSessionId::new("wrong-owner").expect("owner");
    stale.push(wrong_owner);
    let mut wrong_generation = fence.clone();
    wrong_generation.runtime_generation = RuntimeGeneration::new(2).expect("generation");
    stale.push(wrong_generation);

    for (index, stale_fence) in stale.into_iter().enumerate() {
        let envelope = transition(
            &format!("f-stale-{index}"),
            3,
            &stale_fence,
            TaskCommand::RecordDelivery(DeliveryCommand {
                external_delivery_receipt_id: receipt_id(&format!("f-external-{index}")),
                observed_at: None,
            }),
        );
        assert_eq!(
            service.transition(&envelope),
            TransitionOutcome::NoWrite(TaskServiceError::StaleFence)
        );
        assert_eq!(durable_pair(&root), baseline);
        assert_eq!(clock.calls(), clock_baseline);
    }
    committed(service.transition(&transition(
        "f-control",
        3,
        &fence,
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id("f-control-external"),
            observed_at: None,
        }),
    )));
}

// T-FENCE-TASK-ID: a separately altered task authority is the real input;
// durable bytes and clock are observers; the exact published fence is oracle;
// the nonexistent alternate task ID is the typed no-write control.
#[test]
fn t_fence_task_id() {
    let root = service_root("fence-task-id");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    let fence = create_and_publish(&service, "fence-task-id", 1, "fti-create", "fti-publish");
    let baseline = durable_root_image(&root);
    let clock_baseline = clock.calls();
    let mut wrong_task = fence.clone();
    wrong_task.task_id = task_id("fence-task-id-other");
    let stale = transition(
        "fti-stale",
        3,
        &wrong_task,
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id("fti-stale-external"),
            observed_at: None,
        }),
    );
    assert_eq!(
        service.transition(&stale),
        TransitionOutcome::NoWrite(TaskServiceError::StaleFence)
    );
    assert_eq!(durable_root_image(&root), baseline);
    assert_eq!(clock.calls(), clock_baseline);

    committed(service.transition(&transition(
        "fti-control",
        3,
        &fence,
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id("fti-control-external"),
            observed_at: None,
        }),
    )));
}

// T-RECEIPT: replay of an old real CreateDraft after Publish; file hashes and
// clock are observers; stored response is oracle; changed material conflicts.
#[test]
fn t_receipt() {
    let root = service_root("receipt");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    let create = create_envelope("receipt-task", 1, "receipt-create", 1);
    let original = committed(service.transition(&create));
    let (publish, _) = publish_envelope("receipt-task", 1, "receipt-publish", 2);
    committed(service.transition(&publish));
    let baseline = durable_pair(&root);
    let clock_baseline = clock.calls();
    assert_eq!(committed(service.transition(&create)), original);
    assert_eq!(durable_pair(&root), baseline);
    assert_eq!(clock.calls(), clock_baseline);

    let conflicting = create_envelope("receipt-task", 1, "receipt-create", 3);
    assert_eq!(
        service.transition(&conflicting),
        TransitionOutcome::NoWrite(TaskServiceError::ReceiptConflict)
    );
    assert_eq!(durable_pair(&root), baseline);
    assert_eq!(clock.calls(), clock_baseline);
}

// T-DIGEST-SCHEMA: fixed typed body and independent compact material are the
// oracle; serde rejection and dishonest digest are no-write controls.
#[test]
fn t_digest_schema() {
    let body = "golden-opaque";
    let envelope = signed_envelope(
        "golden-receipt",
        1,
        None,
        TaskCommand::CreateDraft(CreateDraftCommand {
            specification: specification("golden-task", 1, body),
        }),
    );
    assert_eq!(
        envelope.request_digest_sha256.as_str(),
        "3e8457b5286ef56b58e81dc6651aca42cca1d88685a2699b4706cebe8f4ecda2"
    );
    let value = serde_json::to_value(&envelope).expect("serialize envelope");
    let mut unknown = value.clone();
    unknown
        .as_object_mut()
        .expect("object")
        .insert("unknown".to_owned(), serde_json::json!(true));
    assert!(serde_json::from_value::<TransitionEnvelope>(unknown).is_err());
    let mut command_unknown = serde_json::to_value(&envelope).expect("serialize envelope");
    command_unknown["command"]["body"]
        .as_object_mut()
        .expect("command body")
        .insert("unknown".to_owned(), serde_json::json!(true));
    assert!(serde_json::from_value::<TransitionEnvelope>(command_unknown).is_err());
    let mut unsupported = value;
    unsupported["schema"] = serde_json::json!("cutex/task-transition-envelope/v2");
    assert!(serde_json::from_value::<TransitionEnvelope>(unsupported).is_err());

    let root = service_root("digest-schema");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    let mut dishonest = envelope;
    dishonest.request_digest_sha256 = zero_sha256();
    assert_eq!(
        service.transition(&dishonest),
        TransitionOutcome::NoWrite(TaskServiceError::RequestDigestMismatch)
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
    assert_eq!(clock.calls(), 0);

    let stale_cas = create_envelope("stale-empty", 1, "stale-empty-r", 2);
    assert_eq!(
        service.transition(&stale_cas),
        TransitionOutcome::NoWrite(TaskServiceError::StoreRevisionConflict {
            expected: store_revision(2),
            actual: store_revision(1),
        })
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
    assert_eq!(clock.calls(), 0);

    let oversized_body = "x".repeat(MAX_SPECIFICATION_BYTES + 1);
    let oversized = signed_envelope(
        "oversized-receipt",
        1,
        None,
        TaskCommand::CreateDraft(CreateDraftCommand {
            specification: specification("oversized-task", 1, &oversized_body),
        }),
    );
    assert_eq!(
        service.transition(&oversized),
        TransitionOutcome::NoWrite(TaskServiceError::InvalidEnvelope {
            code: ValidationCode::SpecificationTooLarge
        })
    );
    assert_eq!(fs::read_dir(&root).expect("root").count(), 0);
    assert_eq!(clock.calls(), 0);
}

// T-JOURNAL-REPLAY: an AfterJournalSync transition is real authoritative
// input; a fresh process facade observes it; its embedded response is oracle;
// middle-byte corruption is the fail-closed control.
#[test]
fn t_journal_replay() {
    let root = service_root("journal-replay");
    let clock = Arc::new(CountingClock::default());
    let service = faulted_service(&root, clock.clone(), FaultPoint::AfterJournalSync);
    let envelope = create_envelope("replay-task", 1, "replay-create", 1);
    assert!(matches!(
        service.transition(&envelope),
        TransitionOutcome::PersistenceUnknown {
            phase: PersistencePhase::JournalSync,
            ..
        }
    ));
    assert!(read_optional(&root.join(SNAPSHOT)).is_none());
    let fresh = clocked_service(&root, clock.clone());
    let store = fresh.load().expect("replay complete event");
    assert_eq!(store.store_revision, store_revision(2));
    assert!(matches!(
        fresh.get_receipt(&envelope),
        ReceiptLookup::Committed(_)
    ));
    assert_eq!(clock.calls(), 1);

    let corrupt_root = service_root("journal-corrupt");
    let corrupt_clock = Arc::new(CountingClock::default());
    let corrupt_service = clocked_service(&corrupt_root, corrupt_clock);
    committed(corrupt_service.transition(&create_envelope("corrupt-a", 1, "corrupt-ra", 1)));
    committed(corrupt_service.transition(&create_envelope("corrupt-b", 1, "corrupt-rb", 2)));
    let path = corrupt_root.join(JOURNAL);
    let mut bytes = fs::read(&path).expect("journal bytes");
    let first_body_byte = bytes
        .iter()
        .position(|byte| *byte == b'{')
        .expect("JSON object");
    bytes[first_body_byte] = b'[';
    fs::write(&path, &bytes).expect("causal corruption fixture");
    let before = durable_pair(&corrupt_root);
    assert!(matches!(
        corrupt_service.load(),
        Err(TaskServiceError::InvalidJournal { .. })
    ));
    assert_eq!(durable_pair(&corrupt_root), before);
}

// T-PERSIST: each closed recovery fault leaves a real disk crash state; a
// new repository object is observer; revision-1/system-sequence-1 is oracle;
// original expected-revision retry is the negative/positive control.
#[test]
fn t_persist() {
    let preappend_root = service_root("persist-preappend");
    let preappend_clock = Arc::new(CountingClock::default());
    let setup = clocked_service(&preappend_root, preappend_clock.clone());
    committed(setup.transition(&create_envelope(
        "preappend-existing",
        1,
        "preappend-existing-r",
        1,
    )));
    let before = durable_pair(&preappend_root);
    let clock_before = preappend_clock.calls();
    let preappend = faulted_service(
        &preappend_root,
        preappend_clock.clone(),
        FaultPoint::BeforeJournalAppend,
    );
    assert_eq!(
        preappend.transition(&create_envelope("preappend-new", 1, "preappend-new-r", 2,)),
        TransitionOutcome::NoWrite(TaskServiceError::InjectedDefiniteNoWrite)
    );
    assert_eq!(durable_pair(&preappend_root), before);
    assert_eq!(preappend_clock.calls(), clock_before);

    let unknown_faults = [
        (
            FaultPoint::AfterJournalWrite,
            PersistencePhase::JournalWrite,
        ),
        (FaultPoint::AfterJournalSync, PersistencePhase::JournalSync),
        (
            FaultPoint::BeforeSnapshotRename,
            PersistencePhase::SnapshotReplace,
        ),
        (
            FaultPoint::AfterSnapshotRename,
            PersistencePhase::SnapshotReplace,
        ),
        (
            FaultPoint::AfterSnapshotParentSync,
            PersistencePhase::SnapshotParentSync,
        ),
    ];
    for (index, (fault, phase)) in unknown_faults.into_iter().enumerate() {
        let root = service_root(&format!("persist-unknown-{index}"));
        let clock = Arc::new(CountingClock::default());
        let service = faulted_service(&root, clock.clone(), fault);
        let subscription = service
            .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
            .expect("pre-unknown subscription")
            .subscription
            .expect("caught up");
        let envelope = create_envelope(
            &format!("unknown-persist-{index}"),
            1,
            &format!("unknown-persist-r-{index}"),
            1,
        );
        assert_eq!(
            service.transition(&envelope),
            TransitionOutcome::PersistenceUnknown {
                receipt_id: envelope.receipt_id.clone(),
                phase,
            }
        );
        assert_eq!(subscription.cursor(), JournalCursor::genesis());
        assert_eq!(
            subscription.try_recv().expect("resync"),
            Some(WatchItem::ResyncRequired {
                reason: ResyncReason::PersistenceUnknown
            })
        );
        let fresh = clocked_service(&root, clock.clone());
        assert_eq!(
            fresh.load().expect("reconcile unknown").store_revision,
            store_revision(2)
        );
        assert!(matches!(
            fresh.get_receipt(&envelope),
            ReceiptLookup::Committed(_)
        ));
        assert_eq!(clock.calls(), 1);
    }

    let recovery_faults = [
        FaultPoint::AfterRecoveryIntentRename,
        FaultPoint::AfterRecoveryIntentParentSync,
        FaultPoint::AfterRecoveryTruncate,
        FaultPoint::PartialRecoveryRecordWrite,
        FaultPoint::AfterRecoveryRecordSync,
        FaultPoint::AfterRecoverySnapshotRename,
        FaultPoint::AfterRecoveryIntentRemove,
    ];
    for (index, recovery_fault) in recovery_faults.into_iter().enumerate() {
        let root = service_root(&format!("persist-{index}"));
        let clock = Arc::new(CountingClock::default());
        let envelope = create_envelope(
            &format!("persist-task-{index}"),
            1,
            &format!("persist-receipt-{index}"),
            1,
        );
        let partial = faulted_service(&root, clock.clone(), FaultPoint::PartialJournalWrite);
        assert!(matches!(
            partial.transition(&envelope),
            TransitionOutcome::PersistenceUnknown {
                phase: PersistencePhase::JournalWrite,
                ..
            }
        ));
        let partial_store_revision = store_revision(1);
        let crashing = faulted_service(&root, clock.clone(), recovery_fault);
        assert!(matches!(
            crashing.load(),
            Err(TaskServiceError::RecoveryStopped { .. })
        ));

        let recovered = clocked_service(&root, clock.clone());
        let store = recovered.load().expect("fresh disk-only recovery");
        assert_eq!(store.store_revision, partial_store_revision);
        assert!(!root.join(RECOVERY).exists());
        let page = recovered
            .page_events(&page_request(JournalCursor::genesis(), None, 10))
            .expect("recovery page");
        assert_eq!(page.records.len(), 1);
        assert!(matches!(
            page.records[0].event,
            JournalEvent::SystemJournalTailRecovered(_)
        ));
        assert_eq!(clock.calls(), 1);
        assert!(matches!(
            recovered.get_receipt(&envelope),
            ReceiptLookup::NotFound
        ));
        committed(recovered.transition(&envelope));
        let final_page = recovered
            .page_events(&page_request(JournalCursor::genesis(), None, 10))
            .expect("recovery plus retry page");
        assert_eq!(final_page.records.len(), 2);
        assert!(matches!(
            final_page.records[0].event,
            JournalEvent::SystemJournalTailRecovered(_)
        ));
        assert!(matches!(
            final_page.records[1].event,
            JournalEvent::Transition(_)
        ));
        assert_eq!(
            recovered.load().expect("final store").store_revision,
            store_revision(2)
        );
    }
}

// T-WATCH-PAGE-LIVE-HANDOFF: records 1-3 plus later record 4 are real;
// returned pages/subscription are observer; sequence order is oracle; partial
// pages returning no subscription are the negative control.
#[test]
fn t_watch_page_live_handoff() {
    let root = service_root("watch-handoff");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock);
    for index in 1..=3 {
        committed(service.transition(&create_envelope(
            &format!("handoff-{index}"),
            1,
            &format!("handoff-r{index}"),
            index,
        )));
    }
    let first = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 1, 4))
        .expect("first handoff page");
    assert_eq!(first.page.records[0].sequence, 1);
    assert!(!first.page.reached_head);
    assert!(first.subscription.is_none());
    let second = service
        .page_and_subscribe(&subscription_request(first.page.continuation, None, 1, 4))
        .expect("second handoff page");
    assert_eq!(second.page.records[0].sequence, 2);
    assert!(!second.page.reached_head);
    assert!(second.subscription.is_none());
    let third = service
        .page_and_subscribe(&subscription_request(second.page.continuation, None, 1, 4))
        .expect("caught-up handoff page");
    assert_eq!(third.page.records[0].sequence, 3);
    assert!(third.page.reached_head);
    let subscription = third.subscription.expect("caught-up subscription");
    let retained = subscription.cursor();
    committed(service.transition(&create_envelope("handoff-4", 1, "handoff-r4", 4)));
    assert_eq!(subscription.cursor(), retained);
    let item = subscription
        .try_recv()
        .expect("watch receive")
        .expect("event");
    let WatchItem::Event(record) = item else {
        panic!("expected live event")
    };
    assert_eq!(record.sequence, 4);
    assert_eq!(subscription.cursor(), record.cursor());
}

// T-WATCH-EXACT-FULL: exact page count is real input; locked page result is
// observer; filter-specific backlog is oracle; no-subscription backlog is the
// negative control and exact-full caught-up page is the positive control.
#[test]
fn t_watch_exact_full() {
    let root = service_root("watch-exact-full");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock);
    committed(service.transition(&create_envelope("exact-1", 1, "exact-r1", 1)));
    committed(service.transition(&create_envelope("exact-2", 1, "exact-r2", 2)));
    let partial = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 1, 1))
        .expect("partial exact-full page");
    assert_eq!(partial.page.records.len(), 1);
    assert!(!partial.page.reached_head);
    assert!(partial.subscription.is_none());
    let caught = service
        .page_and_subscribe(&subscription_request(partial.page.continuation, None, 1, 1))
        .expect("caught exact-full page");
    assert_eq!(caught.page.records.len(), 1);
    assert!(caught.page.reached_head);
    assert!(caught.subscription.is_some());
}

// T-WATCH-QUEUED-RESYNC: capacity-one queue and two committed events are real;
// receive API/cursor are observer; queue-first order is oracle; sender-side
// cursor advancement is the negative control.
#[test]
fn t_watch_queued_resync() {
    let root = service_root("watch-queued");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock);
    committed(service.transition(&create_envelope("queued-1", 1, "queued-r1", 1)));
    let caught = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 1, 1))
        .expect("caught page");
    let subscription = caught.subscription.expect("subscription");
    let pre_unread = subscription.cursor();
    committed(service.transition(&create_envelope("queued-2", 1, "queued-r2", 2)));
    committed(service.transition(&create_envelope("queued-3", 1, "queued-r3", 3)));
    assert_eq!(subscription.cursor(), pre_unread);
    let first = subscription
        .try_recv()
        .expect("receive")
        .expect("queued event");
    let WatchItem::Event(first) = first else {
        panic!("event must precede resync")
    };
    assert_eq!(first.sequence, 2);
    let terminal = subscription.try_recv().expect("terminal").expect("resync");
    assert_eq!(
        terminal,
        WatchItem::ResyncRequired {
            reason: ResyncReason::ReceiverFull
        }
    );
    let replay = service
        .page_events(&page_request(subscription.cursor(), None, 10))
        .expect("page after retained cursor");
    assert_eq!(
        replay
            .records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        vec![3]
    );
}

// T-WATCH-FILTER-SYSTEM: unrelated transition and real recovery audit are
// inputs; filtered page is observer; system-only delivery is oracle; unrelated
// transition suppression/unchanged genesis cursor is the negative control.
#[test]
fn t_watch_filter_system() {
    let root = service_root("watch-filter");
    let clock = Arc::new(CountingClock::default());
    let setup = clocked_service(&root, clock.clone());
    committed(setup.transition(&create_envelope("unrelated-a", 1, "filter-r1", 1)));
    let partial = faulted_service(&root, clock.clone(), FaultPoint::PartialJournalWrite);
    let caught = partial
        .page_and_subscribe(&subscription_request(
            JournalCursor::genesis(),
            Some("selected-task"),
            10,
            2,
        ))
        .expect("filtered caught-up page");
    assert!(caught.page.records.is_empty());
    assert_eq!(caught.page.continuation, JournalCursor::genesis());
    let subscription = caught.subscription.expect("filtered subscription");
    let envelope = create_envelope("unrelated-b", 1, "filter-r2", 2);
    assert!(matches!(
        partial.transition(&envelope),
        TransitionOutcome::PersistenceUnknown { .. }
    ));
    assert_eq!(subscription.cursor(), JournalCursor::genesis());
    assert!(matches!(
        subscription.try_recv().expect("resync"),
        Some(WatchItem::ResyncRequired { .. })
    ));
    let recovered = clocked_service(&root, clock);
    recovered.load().expect("apply recovery");
    let page = recovered
        .page_events(&page_request(
            JournalCursor::genesis(),
            Some("selected-task"),
            10,
        ))
        .expect("filtered recovery page");
    assert_eq!(page.records.len(), 1);
    assert!(matches!(
        page.records[0].event,
        JournalEvent::SystemJournalTailRecovered(_)
    ));
    assert_eq!(page.continuation, page.records[0].cursor());
}

// T-WATCH-UNKNOWN-PARTIAL: real partial append and genesis subscriber are
// inputs; fresh disk recovery/page are observer; recovery-before-retry order
// is oracle; checkpoint-free retained genesis is the negative control.
#[test]
fn t_watch_unknown_partial() {
    let root = service_root("watch-unknown-partial");
    let clock = Arc::new(CountingClock::default());
    let service = faulted_service(&root, clock.clone(), FaultPoint::PartialJournalWrite);
    let caught = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("genesis subscription");
    let subscription = caught.subscription.expect("subscription");
    let envelope = create_envelope("unknown-partial", 1, "unknown-partial-r", 1);
    assert!(matches!(
        service.transition(&envelope),
        TransitionOutcome::PersistenceUnknown {
            phase: PersistencePhase::JournalWrite,
            ..
        }
    ));
    assert_eq!(subscription.cursor(), JournalCursor::genesis());
    assert_eq!(
        subscription.try_recv().expect("receive"),
        Some(WatchItem::ResyncRequired {
            reason: ResyncReason::PersistenceUnknown
        })
    );
    let fresh = clocked_service(&root, clock);
    fresh.load().expect("disk-only recovery");
    let recovery_page = fresh
        .page_events(&page_request(JournalCursor::genesis(), None, 10))
        .expect("recovery page");
    assert_eq!(recovery_page.records.len(), 1);
    assert!(matches!(
        recovery_page.records[0].event,
        JournalEvent::SystemJournalTailRecovered(_)
    ));
    committed(fresh.transition(&envelope));
    let retry_page = fresh
        .page_events(&page_request(recovery_page.continuation, None, 10))
        .expect("retry page");
    assert_eq!(retry_page.records.len(), 1);
    assert!(matches!(
        retry_page.records[0].event,
        JournalEvent::Transition(_)
    ));
}

// T-WATCH-UNKNOWN-COMPLETE: synced complete event with unknown return is real;
// fresh strict page is observer; exact one transition is oracle; subscriber's
// intended-event checkpoint never appears in terminal resync.
#[test]
fn t_watch_unknown_complete() {
    let root = service_root("watch-unknown-complete");
    let clock = Arc::new(CountingClock::default());
    let service = faulted_service(&root, clock.clone(), FaultPoint::AfterJournalSync);
    let subscription = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("subscription")
        .subscription
        .expect("caught up");
    let envelope = create_envelope("unknown-complete", 1, "unknown-complete-r", 1);
    assert!(matches!(
        service.transition(&envelope),
        TransitionOutcome::PersistenceUnknown {
            phase: PersistencePhase::JournalSync,
            ..
        }
    ));
    assert_eq!(subscription.cursor(), JournalCursor::genesis());
    assert!(matches!(
        subscription.try_recv().expect("resync"),
        Some(WatchItem::ResyncRequired { .. })
    ));
    let fresh = clocked_service(&root, clock);
    let page = fresh
        .page_events(&page_request(JournalCursor::genesis(), None, 10))
        .expect("strict replay page");
    assert_eq!(page.records.len(), 1);
    assert!(matches!(page.records[0].event, JournalEvent::Transition(_)));
    assert!(matches!(
        fresh.get_receipt(&envelope),
        ReceiptLookup::Committed(_)
    ));
}

// T-WATCH-RELOAD-RECOVERY: nonzero delivered cursor plus partial append are
// inputs; recovery page is observer; sequence-2 system record is oracle;
// resync retaining sequence 1 is the negative control.
#[test]
fn t_watch_reload_recovery() {
    let root = service_root("watch-reload");
    let clock = Arc::new(CountingClock::default());
    let setup = clocked_service(&root, clock.clone());
    committed(setup.transition(&create_envelope("reload-1", 1, "reload-r1", 1)));
    let service = faulted_service(&root, clock.clone(), FaultPoint::PartialJournalWrite);
    let caught = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("caught-up page");
    let subscription = caught.subscription.expect("subscription");
    let retained = subscription.cursor();
    assert_eq!(retained.sequence, 1);
    assert!(matches!(
        service.transition(&create_envelope("reload-2", 1, "reload-r2", 2)),
        TransitionOutcome::PersistenceUnknown { .. }
    ));
    assert_eq!(subscription.cursor(), retained);
    assert!(matches!(
        subscription.try_recv().expect("resync"),
        Some(WatchItem::ResyncRequired { .. })
    ));
    let fresh = clocked_service(&root, clock);
    fresh.load().expect("recover partial append");
    let page = fresh
        .page_events(&page_request(retained, None, 10))
        .expect("page from retained cursor");
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].sequence, 2);
    assert!(matches!(
        page.records[0].event,
        JournalEvent::SystemJournalTailRecovered(_)
    ));
}

// T-WATCH-CURSOR-INVALID: wrong hash, ahead cursor, and non-genesis zero-hash
// are inputs; read/subscription APIs are observers; unchanged valid subscriber
// cursor is oracle; the valid full cursor is the positive control.
#[test]
fn t_watch_cursor_invalid() {
    let root = service_root("watch-invalid-cursor");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock);
    committed(service.transition(&create_envelope("cursor-1", 1, "cursor-r1", 1)));
    let caught = service
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("valid cursor page");
    let subscription = caught.subscription.expect("valid subscription");
    let valid = subscription.cursor();
    let invalid = [
        JournalCursor {
            sequence: 1,
            event_sha256: sha256_bytes(b"wrong-hash"),
        },
        JournalCursor {
            sequence: 2,
            event_sha256: sha256_bytes(b"ahead-hash"),
        },
        JournalCursor {
            sequence: 0,
            event_sha256: sha256_bytes(b"not-genesis"),
        },
        JournalCursor {
            sequence: 1,
            event_sha256: zero_sha256(),
        },
    ];
    for cursor in invalid {
        assert_eq!(
            service.page_events(&page_request(cursor.clone(), None, 10)),
            Err(TaskServiceError::InvalidCursor)
        );
        assert!(matches!(
            service.page_and_subscribe(&subscription_request(cursor, None, 10, 2)),
            Err(TaskServiceError::InvalidCursor)
        ));
        assert_eq!(subscription.cursor(), valid);
    }
    let empty = service
        .page_events(&page_request(valid.clone(), None, 10))
        .expect("valid full cursor");
    assert!(empty.records.is_empty());
    assert_eq!(empty.continuation, valid);
}

// T-PAGE-READ-ONLY: a real partial append and a durable recovery-intent crash
// state are inputs; page-only calls and complete root images are observers;
// typed RecoveryRequired is oracle; explicit load is the sole positive writer.
#[test]
fn t_page_read_only() {
    let tail_root = service_root("page-read-only-tail");
    let tail_clock = Arc::new(CountingClock::default());
    committed(
        clocked_service(&tail_root, tail_clock.clone()).transition(&create_envelope(
            "page-tail-base",
            1,
            "page-tail-base-r",
            1,
        )),
    );
    let tail_writer = faulted_service(
        &tail_root,
        tail_clock.clone(),
        FaultPoint::PartialJournalWrite,
    );
    assert!(matches!(
        tail_writer.transition(&create_envelope("page-tail", 1, "page-tail-r", 2)),
        TransitionOutcome::PersistenceUnknown { .. }
    ));
    assert!(!tail_root.join(RECOVERY).exists());
    let tail_reader = clocked_service(&tail_root, tail_clock);
    assert_page_recovery_read_only(&tail_root, &tail_reader);
    tail_reader.load().expect("explicit tail recovery");
    assert!(!tail_root.join(RECOVERY).exists());
    assert_eq!(
        tail_reader
            .page_events(&page_request(JournalCursor::genesis(), None, 10))
            .expect("page after explicit recovery")
            .records
            .len(),
        2
    );

    let intent_root = service_root("page-read-only-intent");
    let intent_clock = Arc::new(CountingClock::default());
    committed(
        clocked_service(&intent_root, intent_clock.clone()).transition(&create_envelope(
            "page-intent-base",
            1,
            "page-intent-base-r",
            1,
        )),
    );
    let intent_writer = faulted_service(
        &intent_root,
        intent_clock.clone(),
        FaultPoint::PartialJournalWrite,
    );
    assert!(matches!(
        intent_writer.transition(&create_envelope("page-intent", 1, "page-intent-r", 2)),
        TransitionOutcome::PersistenceUnknown { .. }
    ));
    let intent_crash = faulted_service(
        &intent_root,
        intent_clock.clone(),
        FaultPoint::AfterRecoveryIntentRename,
    );
    assert!(matches!(
        intent_crash.load(),
        Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::IntentParentSync
        })
    ));
    assert!(intent_root.join(RECOVERY).exists());
    let intent_reader = clocked_service(&intent_root, intent_clock);
    assert_page_recovery_read_only(&intent_root, &intent_reader);
    intent_reader.load().expect("explicit intent recovery");
    assert!(!intent_root.join(RECOVERY).exists());
}

// T-CORRUPT-PRECHECK: a valid-but-disagreeing snapshot and a malformed
// complete line, each beside a tail, are inputs; complete root images observe
// that explicit recovery fails before intent creation, truncation, or append.
#[test]
fn t_corrupt_precheck() {
    let mismatch_root = service_root("corrupt-precheck-snapshot");
    let mismatch_clock = Arc::new(CountingClock::default());
    let mismatch = clocked_service(&mismatch_root, mismatch_clock);
    committed(mismatch.transition(&create_envelope(
        "corrupt-precheck",
        1,
        "corrupt-precheck-r",
        1,
    )));
    let snapshot_path = mismatch_root.join(SNAPSHOT);
    let mut snapshot: TaskStore =
        serde_json::from_slice(&fs::read(&snapshot_path).expect("snapshot fixture bytes"))
            .expect("snapshot fixture schema");
    let specification = &mut snapshot
        .tasks
        .get_mut(&task_id("corrupt-precheck"))
        .expect("fixture task")
        .get_mut(&revision(1))
        .expect("fixture revision")
        .specification;
    specification.opaque_contract = "valid-but-different".to_owned();
    specification.contract_sha256 = sha256_bytes(specification.opaque_contract.as_bytes());
    fs::write(
        &snapshot_path,
        serde_json::to_vec(&snapshot).expect("snapshot fixture encoding"),
    )
    .expect("write mismatching snapshot");
    fs::OpenOptions::new()
        .append(true)
        .open(mismatch_root.join(JOURNAL))
        .expect("open tail fixture")
        .write_all(b"{unterminated-tail")
        .expect("append tail fixture");
    let mismatch_before = durable_root_image(&mismatch_root);
    assert_eq!(
        mismatch.load(),
        Err(TaskServiceError::InvalidStore {
            code: ValidationCode::InvalidStoreRevision
        })
    );
    assert_eq!(durable_root_image(&mismatch_root), mismatch_before);
    assert!(!mismatch_root.join(RECOVERY).exists());

    let malformed_root = service_root("corrupt-precheck-journal");
    let malformed_clock = Arc::new(CountingClock::default());
    let malformed = clocked_service(&malformed_root, malformed_clock);
    committed(malformed.transition(&create_envelope(
        "corrupt-complete",
        1,
        "corrupt-complete-r",
        1,
    )));
    let journal_path = malformed_root.join(JOURNAL);
    let mut journal_bytes = fs::read(&journal_path).expect("complete journal fixture");
    let object_start = journal_bytes
        .iter()
        .position(|byte| *byte == b'{')
        .expect("journal object");
    journal_bytes[object_start] = b'[';
    journal_bytes.extend_from_slice(b"{unterminated-tail");
    fs::write(&journal_path, journal_bytes).expect("write malformed complete record");
    let malformed_before = durable_root_image(&malformed_root);
    assert!(matches!(
        malformed.load(),
        Err(TaskServiceError::InvalidJournal { .. })
    ));
    assert_eq!(durable_root_image(&malformed_root), malformed_before);
    assert!(!malformed_root.join(RECOVERY).exists());
}

// T-EXTERNAL-RELOAD-RESYNC: an external repository commits a transition or
// applies recovery; the original instance's page is observer; retained cursor
// plus RepositoryReloaded is oracle; page delivery never advances that cursor.
#[test]
fn t_external_reload_resync() {
    let event_root = service_root("external-reload-event");
    let event_clock = Arc::new(CountingClock::default());
    let local = clocked_service(&event_root, event_clock.clone());
    committed(local.transition(&create_envelope("external-a", 1, "external-a-r", 1)));
    let subscription = local
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("local caught-up subscription")
        .subscription
        .expect("local subscription");
    let retained = subscription.cursor();
    let external = clocked_service(&event_root, event_clock);
    committed(external.transition(&create_envelope("external-b", 1, "external-b-r", 2)));
    let event_bytes = durable_root_image(&event_root);
    let page = local
        .page_events(&page_request(retained.clone(), None, 10))
        .expect("page externally committed event");
    assert_eq!(page.records.len(), 1);
    assert_eq!(durable_root_image(&event_root), event_bytes);
    assert_eq!(subscription.cursor(), retained);
    assert_eq!(
        subscription.try_recv().expect("external event resync"),
        Some(WatchItem::ResyncRequired {
            reason: ResyncReason::RepositoryReloaded
        })
    );

    let recovery_root = service_root("external-reload-recovery");
    let recovery_clock = Arc::new(CountingClock::default());
    let local = clocked_service(&recovery_root, recovery_clock.clone());
    committed(local.transition(&create_envelope("recovery-a", 1, "recovery-a-r", 1)));
    let subscription = local
        .page_and_subscribe(&subscription_request(JournalCursor::genesis(), None, 10, 2))
        .expect("recovery caught-up subscription")
        .subscription
        .expect("recovery subscription");
    let retained = subscription.cursor();
    let partial = faulted_service(
        &recovery_root,
        recovery_clock.clone(),
        FaultPoint::PartialJournalWrite,
    );
    assert!(matches!(
        partial.transition(&create_envelope("recovery-b", 1, "recovery-b-r", 2)),
        TransitionOutcome::PersistenceUnknown { .. }
    ));
    clocked_service(&recovery_root, recovery_clock)
        .load()
        .expect("external explicit recovery");
    let recovery_bytes = durable_root_image(&recovery_root);
    let page = local
        .page_events(&page_request(retained.clone(), None, 10))
        .expect("page externally recovered event");
    assert_eq!(page.records.len(), 1);
    assert!(matches!(
        page.records[0].event,
        JournalEvent::SystemJournalTailRecovered(_)
    ));
    assert_eq!(durable_root_image(&recovery_root), recovery_bytes);
    assert_eq!(subscription.cursor(), retained);
    assert_eq!(
        subscription.try_recv().expect("external recovery resync"),
        Some(WatchItem::ResyncRequired {
            reason: ResyncReason::RepositoryReloaded
        })
    );
}

// T-ROOT-ANCHOR: a validated caller root is renamed and replaced; complete
// images of both directories and the clock are observers; RootBindingChanged
// is oracle; neither the original inode nor replacement receives a write.
#[test]
fn t_root_anchor() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let root = service_root("root-anchor");
    let moved = root.with_extension("validated-root");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());
    committed(service.transition(&create_envelope("anchor-a", 1, "anchor-a-r", 1)));
    fs::rename(&root, &moved).expect("rename validated root");
    fs::create_dir(&root).expect("create replacement root");
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("private replacement root");
    #[cfg(windows)]
    crate::platform::private_fs::secure_directory(&root).expect("private replacement root");
    fs::write(root.join("replacement-sentinel"), b"replacement").expect("replacement sentinel");

    let original_before = durable_root_image(&moved);
    let replacement_before = durable_root_image(&root);
    let clock_before = clock.calls();
    assert_eq!(
        service.transition(&create_envelope("anchor-b", 1, "anchor-b-r", 2)),
        TransitionOutcome::NoWrite(TaskServiceError::RootBindingChanged)
    );
    assert_eq!(durable_root_image(&moved), original_before);
    assert_eq!(durable_root_image(&root), replacement_before);
    assert_eq!(clock.calls(), clock_before);
}

// T-NONINTERFERENCE: seeded sibling/unknown bytes and a real task transition
// are inputs; independent hashes/directories are observers; exact seeded bytes
// are oracle; absence of transport/other-store files is the negative control.
#[test]
fn t_noninterference() {
    let parent = service_root("noninterference-parent");
    let root = parent.join("task-root");
    fs::create_dir(&root).expect("task root");
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private task root");
    #[cfg(windows)]
    crate::platform::private_fs::secure_directory(&root).expect("private task root");
    let sibling = parent.join("session-store.json");
    let unknown = root.join("management.events.jsonl");
    fs::write(&sibling, b"session-seed").expect("sibling seed");
    fs::write(&unknown, b"management-seed").expect("unknown seed");
    let sibling_before = fs::read(&sibling).expect("sibling bytes");
    let unknown_before = fs::read(&unknown).expect("unknown bytes");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock);
    committed(service.transition(&create_envelope("noninterference", 1, "non-r1", 1)));
    assert_eq!(fs::read(&sibling).expect("sibling bytes"), sibling_before);
    assert_eq!(fs::read(&unknown).expect("unknown bytes"), unknown_before);
    let mut names: Vec<_> = fs::read_dir(&root)
        .expect("task root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "management.events.jsonl".to_owned(),
            JOURNAL.to_owned(),
            SNAPSHOT.to_owned(),
            LOCK.to_owned(),
        ]
    );
}

// T-PHASES: explicit commands exercise the closed graph; stored phase is the
// observer; the contract table is immutable oracle; terminal reopen and an
// illegal Accepted->ReviewReady edge are byte/clock-free controls.
#[test]
fn t_phases() {
    let root = service_root("phases");
    let clock = Arc::new(CountingClock::default());
    let service = clocked_service(&root, clock.clone());

    let rejected = create_and_publish(&service, "phase-rejected", 1, "pr-c", "pr-p");
    committed(service.transition(&transition(
        "pr-d",
        3,
        &rejected,
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id("pr-ext"),
            observed_at: None,
        }),
    )));
    committed(service.transition(&transition(
        "pr-r",
        4,
        &rejected,
        TaskCommand::Reject(evidence()),
    )));

    let running = create_and_publish(&service, "phase-running", 5, "pn-c", "pn-p");
    let mut expected = advance_to_running(&service, &running, 7, "pn");
    committed(service.transition(&transition(
        "pn-wait",
        expected,
        &running,
        TaskCommand::EnterWaiting(evidence()),
    )));
    expected += 1;
    committed(service.transition(&transition(
        "pn-block",
        expected,
        &running,
        TaskCommand::BlockWaiting(evidence()),
    )));
    expected += 1;
    committed(service.transition(&transition(
        "pn-resume",
        expected,
        &running,
        TaskCommand::ResumeBlocked(evidence()),
    )));
    expected += 1;
    committed(service.transition(&transition(
        "pn-review",
        expected,
        &running,
        TaskCommand::MarkReviewReady(evidence()),
    )));
    expected += 1;
    committed(service.transition(&transition(
        "pn-complete",
        expected,
        &running,
        TaskCommand::CompleteReview(evidence()),
    )));
    expected += 1;
    assert_eq!(
        service
            .get_task(&task_id("phase-running"), Some(revision(1)))
            .expect("query")
            .expect("task")
            .phase,
        TaskPhase::Completed
    );
    let terminal_bytes = durable_pair(&root);
    let terminal_clock = clock.calls();
    assert_eq!(
        service.transition(&transition(
            "pn-reopen",
            expected,
            &running,
            TaskCommand::ResumeBlocked(evidence()),
        )),
        TransitionOutcome::NoWrite(TaskServiceError::IllegalPhase {
            actual: TaskPhase::Completed
        })
    );
    assert_eq!(durable_pair(&root), terminal_bytes);
    assert_eq!(clock.calls(), terminal_clock);

    let cancelled_draft = signed_envelope(
        "pcd-create",
        expected,
        None,
        TaskCommand::CreateDraft(CreateDraftCommand {
            specification: specification("phase-cancel-draft", 1, "draft-cancel"),
        }),
    );
    committed(service.transition(&cancelled_draft));
    expected += 1;
    committed(service.transition(&signed_envelope(
        "pcd-cancel",
        expected,
        None,
        TaskCommand::CancelDraft(CancelDraftCommand {
            task_id: task_id("phase-cancel-draft"),
            task_revision: revision(1),
        }),
    )));

    let delivered = || {
        TaskCommand::RecordDelivery(DeliveryCommand {
            external_delivery_receipt_id: receipt_id("phase-path-external"),
            observed_at: None,
        })
    };
    assert_phase_path(
        "phase-cancel-published",
        vec![TaskCommand::CancelPublished(evidence())],
        TaskPhase::Cancelled,
    );
    assert_phase_path(
        "phase-cancel-delivered",
        vec![delivered(), TaskCommand::CancelDelivered(evidence())],
        TaskPhase::Cancelled,
    );
    assert_phase_path(
        "phase-cancel-accepted",
        vec![
            delivered(),
            TaskCommand::Accept(evidence()),
            TaskCommand::CancelAccepted(evidence()),
        ],
        TaskPhase::Cancelled,
    );
    let running_prefix = || {
        vec![
            delivered(),
            TaskCommand::Accept(evidence()),
            TaskCommand::Start(evidence()),
        ]
    };
    let mut direct_block = running_prefix();
    direct_block.push(TaskCommand::EnterBlocked(evidence()));
    direct_block.push(TaskCommand::CancelBlocked(evidence()));
    assert_phase_path("phase-direct-block", direct_block, TaskPhase::Cancelled);
    let mut direct_complete = running_prefix();
    direct_complete.push(TaskCommand::CompleteRunning(evidence()));
    assert_phase_path(
        "phase-direct-complete",
        direct_complete,
        TaskPhase::Completed,
    );
    let mut direct_fail = running_prefix();
    direct_fail.push(TaskCommand::FailRunning(evidence()));
    assert_phase_path("phase-direct-fail", direct_fail, TaskPhase::Failed);
    let mut direct_cancel = running_prefix();
    direct_cancel.push(TaskCommand::CancelRunning(evidence()));
    assert_phase_path("phase-direct-cancel", direct_cancel, TaskPhase::Cancelled);
    let mut waiting_resume = running_prefix();
    waiting_resume.push(TaskCommand::EnterWaiting(evidence()));
    waiting_resume.push(TaskCommand::ResumeWaiting(evidence()));
    waiting_resume.push(TaskCommand::CancelRunning(evidence()));
    assert_phase_path("phase-waiting-resume", waiting_resume, TaskPhase::Cancelled);
    let mut waiting_cancel = running_prefix();
    waiting_cancel.push(TaskCommand::EnterWaiting(evidence()));
    waiting_cancel.push(TaskCommand::CancelWaiting(evidence()));
    assert_phase_path("phase-waiting-cancel", waiting_cancel, TaskPhase::Cancelled);
    let mut review_fail = running_prefix();
    review_fail.push(TaskCommand::MarkReviewReady(evidence()));
    review_fail.push(TaskCommand::FailReview(evidence()));
    assert_phase_path("phase-review-fail", review_fail, TaskPhase::Failed);
    let mut review_cancel = running_prefix();
    review_cancel.push(TaskCommand::MarkReviewReady(evidence()));
    review_cancel.push(TaskCommand::CancelReview(evidence()));
    assert_phase_path("phase-review-cancel", review_cancel, TaskPhase::Cancelled);

    let revision_root = service_root("phase-revisions");
    let revision_clock = Arc::new(CountingClock::default());
    let revision_service = clocked_service(&revision_root, revision_clock.clone());
    committed(revision_service.transition(&create_envelope(
        "revision-task",
        1,
        "revision-create-1",
        1,
    )));
    let revision_before = durable_pair(&revision_root);
    let revision_clock_before = revision_clock.calls();
    assert_eq!(
        revision_service.transition(&create_envelope("revision-task", 2, "revision-active-2", 2,)),
        TransitionOutcome::NoWrite(TaskServiceError::ActiveRevisionExists)
    );
    assert_eq!(durable_pair(&revision_root), revision_before);
    assert_eq!(revision_clock.calls(), revision_clock_before);
    committed(revision_service.transition(&signed_envelope(
        "revision-cancel-1",
        2,
        None,
        TaskCommand::CancelDraft(CancelDraftCommand {
            task_id: task_id("revision-task"),
            task_revision: revision(1),
        }),
    )));
    committed(revision_service.transition(&create_envelope(
        "revision-task",
        2,
        "revision-create-2",
        3,
    )));
    assert_eq!(
        revision_service
            .get_task(&task_id("revision-task"), None)
            .expect("revision query")
            .expect("latest revision")
            .specification
            .task_revision,
        revision(2)
    );
}
