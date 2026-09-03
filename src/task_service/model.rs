use std::collections::BTreeMap;
use std::fmt;
use std::io;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256 as Sha256Hasher};

pub use crate::role_revision::{
    AttemptNumber, CutexSessionId, DurableRevision, ReceiptId, Rfc3339, RuntimeAgentId,
    RuntimeGeneration, Sha256, StoreRevision, TaskId, TaskRevision,
};

pub const MAX_SPECIFICATION_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE_LIMIT: u16 = 1000;
pub const MAX_JSON_SAFE_INTEGER: u64 = crate::role_revision::MAX_JSON_SAFE_INTEGER;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct AttemptToken(String);

impl AttemptToken {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationCode> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 256
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
            })
        {
            return Err(ValidationCode::InvalidAttemptToken);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for AttemptToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AttemptToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SpecificationSchema {
    #[serde(rename = "cutex/task-specification/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EnvelopeSchema {
    #[serde(rename = "cutex/task-transition-envelope/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ResponseSchema {
    #[serde(rename = "cutex/task-transition-response/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReceiptSchema {
    #[serde(rename = "cutex/task-receipt/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StoreSchema {
    #[serde(rename = "cutex/task-store/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum JournalSchema {
    #[serde(rename = "cutex/task-journal-record/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RecoverySchema {
    #[serde(rename = "cutex/task-journal-recovery/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QuerySchema {
    #[serde(rename = "cutex/task-query/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PageSchema {
    #[serde(rename = "cutex/task-event-page/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SubscriptionSchema {
    #[serde(rename = "cutex/task-subscription/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpecification {
    pub schema: SpecificationSchema,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Draft,
    Published,
    Delivered,
    Accepted,
    Running,
    Waiting,
    Blocked,
    ReviewReady,
    Completed,
    Failed,
    Cancelled,
    Rejected,
}

impl TaskPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Rejected
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptFence {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: AttemptNumber,
    pub attempt_token: AttemptToken,
    pub owner_session_id: CutexSessionId,
    pub runtime_generation: RuntimeGeneration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAttempt {
    pub attempt_number: AttemptNumber,
    pub attempt_token: AttemptToken,
    pub owner_session_id: CutexSessionId,
    pub owner_durable_revision: DurableRevision,
    pub runtime_generation: RuntimeGeneration,
    pub runtime_agent_id: RuntimeAgentId,
    pub publication_receipt_id: ReceiptId,
    pub delivery_receipt_id: Option<ReceiptId>,
    pub acceptance_receipt_id: Option<ReceiptId>,
    pub start_receipt_id: Option<ReceiptId>,
    pub result_receipt_id: Option<ReceiptId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRecord {
    pub specification: TaskSpecification,
    pub phase: TaskPhase,
    pub attempt: Option<TaskAttempt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDraftCommand {
    pub specification: TaskSpecification,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishCommand {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_token: AttemptToken,
    pub owner_session_id: CutexSessionId,
    pub owner_durable_revision: DurableRevision,
    pub runtime_generation: RuntimeGeneration,
    pub runtime_agent_id: RuntimeAgentId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CancelDraftCommand {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryCommand {
    pub external_delivery_receipt_id: ReceiptId,
    pub observed_at: Option<Rfc3339>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEvidence {
    pub external_receipt_id: Option<ReceiptId>,
    pub observed_at: Option<Rfc3339>,
    pub evidence_sha256: Option<Sha256>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum TaskCommand {
    CreateDraft(CreateDraftCommand),
    Publish(PublishCommand),
    CancelDraft(CancelDraftCommand),
    RecordDelivery(DeliveryCommand),
    CancelPublished(TransitionEvidence),
    Accept(TransitionEvidence),
    Reject(TransitionEvidence),
    CancelDelivered(TransitionEvidence),
    Start(TransitionEvidence),
    CancelAccepted(TransitionEvidence),
    EnterWaiting(TransitionEvidence),
    EnterBlocked(TransitionEvidence),
    MarkReviewReady(TransitionEvidence),
    CompleteRunning(TransitionEvidence),
    FailRunning(TransitionEvidence),
    CancelRunning(TransitionEvidence),
    ResumeWaiting(TransitionEvidence),
    BlockWaiting(TransitionEvidence),
    CancelWaiting(TransitionEvidence),
    ResumeBlocked(TransitionEvidence),
    CancelBlocked(TransitionEvidence),
    CompleteReview(TransitionEvidence),
    FailReview(TransitionEvidence),
    CancelReview(TransitionEvidence),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEnvelope {
    pub schema: EnvelopeSchema,
    pub receipt_id: ReceiptId,
    pub request_digest_sha256: Sha256,
    pub expected_store_revision: StoreRevision,
    pub fence: Option<AttemptFence>,
    pub command: TaskCommand,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionResponse {
    pub schema: ResponseSchema,
    pub receipt_id: ReceiptId,
    pub committed_store_revision: StoreRevision,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: Option<AttemptNumber>,
    pub prior_phase: Option<TaskPhase>,
    pub resulting_phase: TaskPhase,
    pub committed_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalCursor {
    pub sequence: u64,
    pub event_sha256: Sha256,
}

impl JournalCursor {
    pub fn genesis() -> Self {
        Self {
            sequence: 0,
            event_sha256: zero_sha256(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptRecord {
    pub schema: ReceiptSchema,
    pub receipt_id: ReceiptId,
    pub request_digest_sha256: Sha256,
    pub response: TransitionResponse,
    pub event_cursor: JournalCursor,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStore {
    pub schema: StoreSchema,
    pub store_revision: StoreRevision,
    pub journal_checkpoint: JournalCursor,
    pub tasks: BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRecord>>,
    pub receipts: BTreeMap<ReceiptId, ReceiptRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionEvent {
    pub envelope: TransitionEnvelope,
    pub response: TransitionResponse,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalTailRecovered {
    pub discarded_byte_count: u64,
    pub discarded_suffix_sha256: Sha256,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum JournalEvent {
    Transition(TransitionEvent),
    SystemJournalTailRecovered(JournalTailRecovered),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord {
    pub schema: JournalSchema,
    pub sequence: u64,
    pub previous_event_sha256: Sha256,
    pub event_sha256: Sha256,
    pub store_revision: StoreRevision,
    pub event: JournalEvent,
}

impl JournalRecord {
    pub fn cursor(&self) -> JournalCursor {
        JournalCursor {
            sequence: self.sequence,
            event_sha256: self.event_sha256.clone(),
        }
    }

    pub fn task_id(&self) -> Option<&TaskId> {
        match &self.event {
            JournalEvent::Transition(event) => Some(&event.response.task_id),
            JournalEvent::SystemJournalTailRecovered(_) => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryIntent {
    pub schema: RecoverySchema,
    pub complete_prefix_length: u64,
    pub suffix_byte_count: u64,
    pub suffix_sha256: Sha256,
    pub suffix_base64: String,
    pub previous_event_sha256: Sha256,
    pub target_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskQuery {
    pub schema: QuerySchema,
    pub task_id: TaskId,
    pub task_revision: Option<TaskRevision>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptQuery {
    pub schema: QuerySchema,
    pub receipt_id: ReceiptId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventPageRequest {
    pub schema: PageSchema,
    pub cursor: JournalCursor,
    pub task_id: Option<TaskId>,
    pub limit: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub schema: PageSchema,
    pub records: Vec<JournalRecord>,
    pub continuation: JournalCursor,
    pub reached_head: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionRequest {
    pub schema: SubscriptionSchema,
    pub page: EventPageRequest,
    pub capacity: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    PersistenceUnknown,
    RepositoryReloaded,
    RecoveryApplied,
    RecoveryStopped,
    ReceiverFull,
    ReceiverDisconnected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum WatchItem {
    Event(JournalRecord),
    ResyncRequired { reason: ResyncReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchReceiveError {
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoStage {
    InspectRoot,
    OpenLock,
    Lock,
    OpenSnapshot,
    ReadSnapshot,
    OpenJournal,
    ReadJournal,
    AppendJournal,
    SyncJournal,
    TruncateJournal,
    CreateTemp,
    WriteTemp,
    SyncTemp,
    ReplaceSnapshot,
    ReplaceRecoveryIntent,
    OpenRecoveryIntent,
    ReadRecoveryIntent,
    RemoveRecoveryIntent,
    SyncRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PersistencePhase {
    JournalWrite,
    JournalSync,
    SnapshotReplace,
    SnapshotParentSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryPhase {
    IntentWrite,
    IntentParentSync,
    JournalTruncate,
    RecoveryRecordWrite,
    RecoveryRecordSync,
    SnapshotReplace,
    IntentRemoval,
    IntentRemovalParentSync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationCode {
    InvalidAttemptToken,
    SpecificationTooLarge,
    SpecificationHashMismatch,
    KeyBodyMismatch,
    InvalidAttempt,
    InvalidPhaseAttemptShape,
    ActiveEarlierRevision,
    ReceiptKeyMismatch,
    ReceiptResponseMismatch,
    ReceiptAfterCheckpoint,
    InvalidCursor,
    InvalidSequence,
    InvalidEventHash,
    InvalidPreviousHash,
    InvalidStoreRevision,
    InvalidTransitionEvent,
    InvalidRecoveryIntent,
    InvalidPageLimit,
    InvalidSubscriptionCapacity,
    InvalidJson,
}

impl fmt::Display for ValidationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskServiceError {
    UnsupportedPlatform,
    RootNotDirectory,
    RootOwnerMismatch,
    RootModeMismatch,
    RootBindingChanged,
    PrivateFileNotRegular,
    PrivateFileOwnerMismatch,
    PrivateFileModeMismatch,
    Io {
        stage: IoStage,
        kind: io::ErrorKind,
    },
    InvalidJson,
    InvalidEnvelope {
        code: ValidationCode,
    },
    InvalidStore {
        code: ValidationCode,
    },
    InvalidJournal {
        code: ValidationCode,
    },
    InvalidRecoveryIntent {
        code: ValidationCode,
    },
    RequestDigestMismatch,
    ReceiptConflict,
    StoreRevisionConflict {
        expected: StoreRevision,
        actual: StoreRevision,
    },
    StoreRevisionOverflow,
    RevisionConflict,
    RevisionNotIncreasing,
    ActiveRevisionExists,
    TaskNotFound,
    AttemptNotFound,
    StaleFence,
    FenceNotAllowed,
    FenceRequired,
    IllegalPhase {
        actual: TaskPhase,
    },
    Serialization,
    SnapshotAheadOfJournal,
    RecoveryRequired,
    RecoveryStopped {
        phase: RecoveryPhase,
    },
    InvalidCursor,
    InvalidPageLimit,
    InvalidSubscriptionCapacity,
    InjectedDefiniteNoWrite,
}

impl fmt::Display for TaskServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "task service error: {self:?}")
    }
}

impl std::error::Error for TaskServiceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionOutcome {
    Committed(TransitionResponse),
    NoWrite(TaskServiceError),
    PersistenceUnknown {
        receipt_id: ReceiptId,
        phase: PersistencePhase,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptLookup {
    NotFound,
    Committed(TransitionResponse),
    ReceiptConflict,
    Unavailable(TaskServiceError),
}

pub fn zero_sha256() -> Sha256 {
    Sha256::new("0".repeat(64)).expect("the zero digest is valid")
}

pub fn sha256_bytes(bytes: &[u8]) -> Sha256 {
    let digest = Sha256Hasher::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Sha256::new(encoded).expect("a SHA-256 digest is valid")
}

pub fn empty_store() -> TaskStore {
    TaskStore {
        schema: StoreSchema::V1,
        store_revision: StoreRevision::new(1).expect("revision one is valid"),
        journal_checkpoint: JournalCursor::genesis(),
        tasks: BTreeMap::new(),
        receipts: BTreeMap::new(),
    }
}

pub fn validate_specification(specification: &TaskSpecification) -> Result<(), ValidationCode> {
    if specification.opaque_contract.len() > MAX_SPECIFICATION_BYTES {
        return Err(ValidationCode::SpecificationTooLarge);
    }
    if sha256_bytes(specification.opaque_contract.as_bytes()) != specification.contract_sha256 {
        return Err(ValidationCode::SpecificationHashMismatch);
    }
    Ok(())
}

pub fn validate_cursor_shape(cursor: &JournalCursor) -> Result<(), ValidationCode> {
    if cursor.sequence > MAX_JSON_SAFE_INTEGER {
        return Err(ValidationCode::InvalidCursor);
    }
    if cursor.sequence == 0 {
        if cursor.event_sha256 != zero_sha256() {
            return Err(ValidationCode::InvalidCursor);
        }
    } else if cursor.event_sha256 == zero_sha256() {
        return Err(ValidationCode::InvalidCursor);
    }
    Ok(())
}

pub fn validate_store(store: &TaskStore) -> Result<(), ValidationCode> {
    validate_cursor_shape(&store.journal_checkpoint)?;
    for (task_id, revisions) in &store.tasks {
        if revisions.is_empty() {
            return Err(ValidationCode::KeyBodyMismatch);
        }
        let latest_revision = revisions.keys().next_back();
        for (task_revision, record) in revisions {
            validate_specification(&record.specification)?;
            if &record.specification.task_id != task_id
                || &record.specification.task_revision != task_revision
            {
                return Err(ValidationCode::KeyBodyMismatch);
            }
            if !record.phase.is_terminal() && Some(task_revision) != latest_revision {
                return Err(ValidationCode::ActiveEarlierRevision);
            }
            match (&record.attempt, record.phase) {
                (None, TaskPhase::Draft | TaskPhase::Cancelled) => {}
                (None, _) => return Err(ValidationCode::InvalidPhaseAttemptShape),
                (Some(attempt), TaskPhase::Draft) => {
                    let _ = attempt;
                    return Err(ValidationCode::InvalidPhaseAttemptShape);
                }
                (Some(attempt), _) => {
                    if attempt.attempt_number.get() != 1 {
                        return Err(ValidationCode::InvalidAttempt);
                    }
                }
            }
        }
    }
    for (receipt_id, receipt) in &store.receipts {
        if receipt_id != &receipt.receipt_id || receipt_id != &receipt.response.receipt_id {
            return Err(ValidationCode::ReceiptKeyMismatch);
        }
        if receipt.response.committed_store_revision.get() > store.store_revision.get() {
            return Err(ValidationCode::ReceiptResponseMismatch);
        }
        validate_cursor_shape(&receipt.event_cursor)?;
        if receipt.event_cursor.sequence > store.journal_checkpoint.sequence {
            return Err(ValidationCode::ReceiptAfterCheckpoint);
        }
    }
    Ok(())
}

pub fn validate_envelope(envelope: &TransitionEnvelope) -> Result<(), ValidationCode> {
    match &envelope.command {
        TaskCommand::CreateDraft(command) => validate_specification(&command.specification)?,
        TaskCommand::Publish(_) | TaskCommand::CancelDraft(_) => {}
        _ => {}
    }
    Ok(())
}

pub fn validate_page_request(request: &EventPageRequest) -> Result<(), TaskServiceError> {
    validate_cursor_shape(&request.cursor).map_err(|_| TaskServiceError::InvalidCursor)?;
    if request.limit == 0 || request.limit > MAX_PAGE_LIMIT {
        return Err(TaskServiceError::InvalidPageLimit);
    }
    Ok(())
}

pub fn validate_subscription_request(
    request: &SubscriptionRequest,
) -> Result<(), TaskServiceError> {
    validate_page_request(&request.page)?;
    if request.capacity == 0 || request.capacity > MAX_PAGE_LIMIT {
        return Err(TaskServiceError::InvalidSubscriptionCapacity);
    }
    Ok(())
}
