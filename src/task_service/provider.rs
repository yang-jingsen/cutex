//! Revised deterministic Task Service provider contract.
//!
//! The provider deliberately separates semantic task state from communication
//! state. All caller identity is supplied by an authenticated integration
//! boundary. Model-facing Worker documents contain only stable semantic
//! fields; trusted provider envelopes add exact attempt handles and
//! aggregate-local compare-and-swap revisions mechanically.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant, SystemTime};

use chrono::SecondsFormat;
use fs2::FileExt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256 as Sha256Hasher};
use uuid::Uuid;

use crate::role_revision::{
    AttemptNumber, CutexSessionId, Rfc3339, Sha256, TaskId, TaskRevision, MAX_JSON_SAFE_INTEGER,
};

pub const TASK_SERVICE_PROVIDER_CONTRACT: &str = "cutex/task-service-provider/v2";
pub const TASK_SERVICE_PROVIDER_ACTION_SCHEMA: &str = "cutex/task-service-action/v2";
pub const TASK_SERVICE_WORKER_PROVIDER_SCHEMA: &str = "cutex/task-service-worker-provider/v2";
pub const TASK_SERVICE_WORKER_CONTEXT_SCHEMA: &str = "cutex/task-service-worker-context/v2";
pub const TASK_SERVICE_WORKER_PREPARE_SCHEMA: &str = "cutex/task-service-worker-prepare/v2";
pub const TASK_SERVICE_PROVIDER_RECEIPT_SCHEMA: &str = "cutex/task-service-receipt/v2";
pub const TASK_SERVICE_PROVIDER_CONTRACT_JSON: &str = include_str!("task-service-provider-v2.json");
const STORE_FILE: &str = "task-service-provider-v2.json";
const JOURNAL_FILE: &str = "task-service-provider-v2.events.jsonl";
const LOCK_FILE: &str = "task-service-provider-v2.lock";
const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_CONTRACT_BYTES: usize = 1024 * 1024;
const MAX_EVIDENCE_BYTES: usize = 512 * 1024;
const MAX_BLOCKER_SUMMARY_BYTES: usize = 2048;
const MAX_DECISION_REFERENCE_BYTES: usize = 4096;
const MAX_WATCH_LIMIT: usize = 1000;
const MAX_PREPARED_WORKER_ACTIONS: usize = 4096;
const MAX_QUERY_DURATION: Duration = Duration::from_secs(2);
const QUERY_LOCK_RETRY: Duration = Duration::from_millis(5);
const QUERY_READ_CHUNK_BYTES: usize = 64 * 1024;

macro_rules! provider_id {
    ($name:ident, $invalid:literal) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ProviderError> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > 256
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
                    })
                {
                    return Err(ProviderError::InvalidRequest($invalid));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

provider_id!(WorkflowId, "invalid_workflow_id");
provider_id!(SeatId, "invalid_seat_id");
provider_id!(AssignmentId, "invalid_assignment_id");
provider_id!(SendAttemptId, "invalid_send_attempt_id");
provider_id!(NotificationId, "invalid_notification_id");
provider_id!(ActionId, "invalid_action_id");
provider_id!(ProviderAttemptToken, "invalid_attempt_token");

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProviderStoreSchema {
    #[serde(rename = "cutex/task-service-store/v2")]
    V2,
    #[serde(rename = "cutex/task-service-store/v3")]
    V3,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProviderActionSchema {
    #[serde(rename = "cutex/task-service-action/v2")]
    V2,
    #[serde(rename = "cutex/task-service-action/v3")]
    V3,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ProviderReceiptSchema {
    #[serde(rename = "cutex/task-service-receipt/v2")]
    V2,
    #[serde(rename = "cutex/task-service-receipt/v3")]
    V3,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CoordinatorRequestSchema {
    #[serde(rename = "cutex/task-service-coordinator/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TerminalRequestSchema {
    #[serde(rename = "cutex/task-service-terminal/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QueryRequestSchema {
    #[serde(rename = "cutex/task-service-query/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkerProviderRequestSchema {
    #[serde(rename = "cutex/task-service-worker-provider/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkerContextRequestSchema {
    #[serde(rename = "cutex/task-service-worker-context/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkerContextResponseSchema {
    #[serde(rename = "cutex/task-service-worker-context-response/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkerPrepareRequestSchema {
    #[serde(rename = "cutex/task-service-worker-prepare/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkerPrepareResponseSchema {
    #[serde(rename = "cutex/task-service-worker-prepare-response/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionPolicyKind {
    ReleaseReview,
    DirectorAcceptance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionPolicy {
    pub kind: CompletionPolicyKind,
    pub authority_seat_id: SeatId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRevisionRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
    pub completion_policy: CompletionPolicy,
    pub workflow_id: WorkflowId,
    pub created_at: Rfc3339,
    pub created_by_cutex_session: CutexSessionId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentState {
    AwaitingAck,
    Active,
    RetryPending,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureReason {
    Completed,
    Failed,
    Cancelled,
    Declined,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentClosure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub reason: ClosureReason,
    pub terminal_attempt: Option<AttemptNumber>,
    pub closed_at: Rfc3339,
    pub closure_action_id: ActionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryAuthorization {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub action_id: ActionId,
    pub authorized_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub assignee_cutex_session: CutexSessionId,
    pub state: AssignmentState,
    pub local_revision: u64,
    pub created_at: Rfc3339,
    pub acknowledged_at: Option<Rfc3339>,
    pub active_attempt: Option<AttemptNumber>,
    pub retry_authorization: Option<RetryAuthorization>,
    pub closure: Option<AssignmentClosure>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    Running,
    Blocked,
    ReviewReady,
    Completed,
    Failed,
    Cancelled,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub action_id: ActionId,
    pub summary: String,
    pub evidence_sha256: Option<Sha256>,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResultReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub action_id: ActionId,
    pub result_sha256: Sha256,
    pub result_reference: String,
    pub submitted_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub attempt_number: AttemptNumber,
    pub attempt_token: ProviderAttemptToken,
    pub phase: AttemptPhase,
    pub local_revision: u64,
    pub started_at: Rfc3339,
    pub updated_at: Rfc3339,
    pub status_receipts: Vec<StatusReceipt>,
    pub result_receipts: Vec<ResultReceipt>,
    pub terminal_action_id: Option<ActionId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommunicationEventKind {
    SendPrepared,
    BusQueued,
    ContextInserted,
    SendUncertain,
    SendRetryScheduled,
    RetriesExhausted,
    StatusRequestSent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationEvent {
    pub kind: CommunicationEventKind,
    pub receipt_reference: Option<String>,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SendAttempt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub send_attempt_id: SendAttemptId,
    pub assignment_id: AssignmentId,
    pub retry_ordinal: u32,
    pub external_message_id: String,
    pub local_revision: u64,
    pub events: Vec<CommunicationEvent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionNotificationKind {
    ReviewReady,
    Blocked,
    Declined,
    AttemptAborted,
    RetriesExhausted,
    OwnerActionRequired,
    TerminalClosure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionNotificationDeliveryMode {
    AfterTurn,
    Soon,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionNotificationFactKind {
    Queued,
    Delivered,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionNotificationFact {
    pub kind: CompletionNotificationFactKind,
    pub reference: Option<String>,
    pub recorded_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub notification_id: NotificationId,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: Option<AttemptNumber>,
    pub transition_action_id: ActionId,
    pub kind: CompletionNotificationKind,
    pub target_seat_id: SeatId,
    pub delivery_mode: CompletionNotificationDeliveryMode,
    pub external_message_id: String,
    pub human_readable_content: String,
    pub local_revision: u64,
    pub created_at: Rfc3339,
    pub facts: Vec<CompletionNotificationFact>,
}

/// Durable Task Service-owned follow-up for the exact Worker assigned to an
/// attempt. Unlike completion notifications this never resolves through a
/// logical seat: the persisted durable Cutex session is the routing authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFollowupNotification {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub notification_id: NotificationId,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: AttemptNumber,
    pub transition_action_id: ActionId,
    pub target_cutex_session: CutexSessionId,
    pub decision_reference: String,
    pub external_message_id: String,
    pub local_revision: u64,
    pub created_at: Rfc3339,
    pub facts: Vec<CompletionNotificationFact>,
}

impl WorkerFollowupNotification {
    pub fn is_delivered(&self) -> bool {
        self.facts
            .iter()
            .any(|fact| fact.kind == CompletionNotificationFactKind::Delivered)
    }
}

impl CompletionNotification {
    pub fn is_delivered(&self) -> bool {
        self.facts
            .iter()
            .any(|fact| fact.kind == CompletionNotificationFactKind::Delivered)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowExecutionGuard {
    Open,
    Quiescing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub workflow_id: WorkflowId,
    pub coordinator_seat_id: SeatId,
    pub local_revision: u64,
    pub execution_guard: WorkflowExecutionGuard,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedPrincipal {
    kind: AuthenticatedPrincipalKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AuthenticatedPrincipalKind {
    Session {
        cutex_session_id: CutexSessionId,
        current_seat_id: Option<SeatId>,
        seat_epoch: Option<u64>,
    },
    TaskServiceSystem,
}

impl AuthenticatedPrincipal {
    /// Constructed only after the integration boundary authenticates the
    /// durable session and, when applicable, its current seat membership.
    pub fn session(cutex_session_id: CutexSessionId) -> Self {
        Self {
            kind: AuthenticatedPrincipalKind::Session {
                cutex_session_id,
                current_seat_id: None,
                seat_epoch: None,
            },
        }
    }

    pub(crate) fn seated_session(
        cutex_session_id: CutexSessionId,
        current_seat_id: SeatId,
        seat_epoch: u64,
    ) -> Result<Self, ProviderError> {
        if seat_epoch == 0 || seat_epoch > MAX_JSON_SAFE_INTEGER {
            return Err(ProviderError::InvalidRequest("invalid_seat_epoch"));
        }
        Ok(Self {
            kind: AuthenticatedPrincipalKind::Session {
                cutex_session_id,
                current_seat_id: Some(current_seat_id),
                seat_epoch: Some(seat_epoch),
            },
        })
    }

    /// This principal is never represented in a model-authored request. The
    /// Task Delivery integration holds it in process after protocol auth.
    pub(crate) fn task_service_system() -> Self {
        Self {
            kind: AuthenticatedPrincipalKind::TaskServiceSystem,
        }
    }

    fn session_id(&self) -> Result<&CutexSessionId, ProviderError> {
        match &self.kind {
            AuthenticatedPrincipalKind::Session {
                cutex_session_id, ..
            } => Ok(cutex_session_id),
            AuthenticatedPrincipalKind::TaskServiceSystem => Err(ProviderError::Unauthorized),
        }
    }

    pub(crate) fn authenticated_session_id(&self) -> Result<&CutexSessionId, ProviderError> {
        self.session_id()
    }

    fn seat(&self) -> Result<(&SeatId, u64), ProviderError> {
        match &self.kind {
            AuthenticatedPrincipalKind::Session {
                current_seat_id: Some(seat),
                seat_epoch: Some(epoch),
                ..
            } => Ok((seat, *epoch)),
            _ => Err(ProviderError::Unauthorized),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRevisionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub workflow_id: WorkflowId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
    pub completion_policy: CompletionPolicy,
}

/// Staged v3 create surface. The accepted v2 request remains an explicitly
/// unscoped legacy write until a paired producer activates this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectRevisionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub project_id: crate::agent_management::ProjectId,
    pub workflow_id: WorkflowId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
    pub completion_policy: CompletionPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignAndDispatchRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub assignee_cutex_session: CutexSessionId,
    pub send_attempt_id: SendAttemptId,
    pub external_message_id: String,
}

/// Staged v3 assignment surface with an exact project lineage fence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignProjectAndDispatchRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub project_id: crate::agent_management::ProjectId,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub assignee_cutex_session: CutexSessionId,
    pub send_attempt_id: SendAttemptId,
    pub external_message_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryDeliveryRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub send_attempt_id: SendAttemptId,
    pub external_message_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorDispatchRequest {
    pub request: AssignAndDispatchRequest,
    pub human_readable_content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorRetryDeliveryRequest {
    pub request: RetryDeliveryRequest,
    pub human_readable_content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationEventRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub send_attempt_id: SendAttemptId,
    pub expected_send_attempt_revision: u64,
    pub kind: CommunicationEventKind,
    pub receipt_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletionNotificationFactRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub notification_id: NotificationId,
    pub expected_notification_revision: u64,
    pub kind: CompletionNotificationFactKind,
    pub reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerFollowupFactRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub notification_id: NotificationId,
    pub expected_notification_revision: u64,
    pub kind: CompletionNotificationFactKind,
    pub reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentActionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StatusActionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_sha256: Option<Sha256>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlockActionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitActionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub result_sha256: Sha256,
    pub result_reference: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", content = "body", rename_all = "snake_case")]
pub enum WorkerActionRequest {
    Start(AssignmentActionRequest),
    ReportStatus(StatusActionRequest),
    Block(BlockActionRequest),
    Resume(AssignmentActionRequest),
    Submit(SubmitActionRequest),
    Decline(AssignmentActionRequest),
    AbortAttempt(AssignmentActionRequest),
}

impl WorkerActionRequest {
    pub fn action_id(&self) -> &ActionId {
        match self {
            Self::Start(value)
            | Self::Resume(value)
            | Self::Decline(value)
            | Self::AbortAttempt(value) => &value.action_id,
            Self::Block(value) => &value.action_id,
            Self::ReportStatus(value) => &value.action_id,
            Self::Submit(value) => &value.action_id,
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            Self::Start(_) => "start",
            Self::ReportStatus(_) => "report_status",
            Self::Block(_) => "block",
            Self::Resume(_) => "resume",
            Self::Submit(_) => "submit",
            Self::Decline(_) => "decline",
            Self::AbortAttempt(_) => "abort_attempt",
        }
    }

    fn assignment_id(&self) -> &AssignmentId {
        match self {
            Self::Start(value)
            | Self::Resume(value)
            | Self::Decline(value)
            | Self::AbortAttempt(value) => &value.assignment_id,
            Self::Block(value) => &value.assignment_id,
            Self::ReportStatus(value) => &value.assignment_id,
            Self::Submit(value) => &value.assignment_id,
        }
    }

    fn requires_attempt_binding(&self) -> bool {
        !matches!(self, Self::Start(_) | Self::Decline(_))
    }
}

/// Mechanical attempt identity and aggregate-local CAS supplied only by the
/// trusted native-tool/provider boundary. This type is never embedded in an
/// assignment message or model-visible Worker action document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptMechanicalContext {
    pub attempt_number: AttemptNumber,
    pub attempt_token: ProviderAttemptToken,
    pub expected_attempt_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerMechanicalContext {
    pub expected_assignment_revision: u64,
    pub attempt: Option<AttemptMechanicalContext>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerProviderActionEnvelope {
    pub schema: WorkerProviderRequestSchema,
    pub action: WorkerActionRequest,
    pub context: WorkerMechanicalContext,
}

impl WorkerProviderActionEnvelope {
    pub fn action_id(&self) -> &ActionId {
        self.action.action_id()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerContextRequest {
    pub schema: WorkerContextRequestSchema,
    pub assignment_id: AssignmentId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerContext {
    pub assignment_id: AssignmentId,
    pub context: WorkerMechanicalContext,
}

/// Semantic-only authenticated preparation request. Mechanical values are
/// resolved and persisted by the provider, never accepted from this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPrepareRequest {
    pub schema: WorkerPrepareRequestSchema,
    pub action: WorkerActionRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum WorkerPrepareOutcome {
    Prepared(WorkerProviderActionEnvelope),
    Committed(ProviderReceipt),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalActionRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub decision_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "operation",
    content = "body",
    rename_all = "snake_case"
)]
pub enum TerminalAuthorityRequest {
    AcceptResult(TerminalActionRequest),
    RequestChanges(TerminalActionRequest),
    FailResult(TerminalActionRequest),
    Cancel(TerminalActionRequest),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CloseAssignmentRequest {
    pub schema: ProviderActionSchema,
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "operation",
    content = "body",
    rename_all = "snake_case"
)]
pub enum CoordinatorOperation {
    CreateRevision(CreateRevisionRequest),
    AssignAndDispatch(CoordinatorDispatchRequest),
    RetryDelivery(CoordinatorRetryDeliveryRequest),
    CancelAssignment(AssignmentActionRequest),
    AuthorizeAttemptRetry(AssignmentActionRequest),
    CloseAssignment(CloseAssignmentRequest),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "operation",
    content = "body",
    rename_all = "snake_case"
)]
pub enum CoordinatorMechanicalContext {
    CreateRevision {
        expected_workflow_revision: Option<u64>,
    },
    AssignAndDispatch {
        expected_workflow_revision: u64,
    },
    RetryDelivery {
        expected_assignment_revision: u64,
    },
    CancelAssignment {
        expected_assignment_revision: u64,
        active_attempt: Option<AttemptMechanicalContext>,
    },
    AuthorizeAttemptRetry {
        expected_assignment_revision: u64,
    },
    CloseAssignment {
        expected_assignment_revision: u64,
        attempt: AttemptMechanicalContext,
    },
}

impl CoordinatorOperation {
    pub fn action_id(&self) -> &ActionId {
        match self {
            Self::CreateRevision(request) => &request.action_id,
            Self::AssignAndDispatch(request) => &request.request.action_id,
            Self::RetryDelivery(request) => &request.request.action_id,
            Self::CancelAssignment(request) | Self::AuthorizeAttemptRetry(request) => {
                &request.action_id
            }
            Self::CloseAssignment(request) => &request.action_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorActionRequest {
    pub schema: CoordinatorRequestSchema,
    pub command: CoordinatorOperation,
    pub context: CoordinatorMechanicalContext,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalActionEnvelope {
    pub schema: TerminalRequestSchema,
    pub command: TerminalAuthorityRequest,
    pub context: WorkerMechanicalContext,
}

impl TerminalActionEnvelope {
    pub fn action_id(&self) -> &ActionId {
        match &self.command {
            TerminalAuthorityRequest::AcceptResult(request)
            | TerminalAuthorityRequest::RequestChanges(request)
            | TerminalAuthorityRequest::FailResult(request)
            | TerminalAuthorityRequest::Cancel(request) => &request.action_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "operation",
    content = "body",
    rename_all = "snake_case"
)]
pub enum TaskServiceQueryOperation {
    Snapshot,
    Watch { after_sequence: u64, limit: usize },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceQueryRequest {
    pub schema: QueryRequestSchema,
    pub query: TaskServiceQueryOperation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum ProviderResult {
    TaskRevision(TaskRevisionRecord),
    Assignment {
        assignment: Assignment,
        send_attempt: Option<SendAttempt>,
    },
    Attempt(Attempt),
    SendAttempt(SendAttempt),
    CompletionNotification(CompletionNotification),
    WorkerFollowupNotification(WorkerFollowupNotification),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderReceipt {
    pub schema: ProviderReceiptSchema,
    pub action_id: ActionId,
    pub request_sha256: Sha256,
    /// Durable original attempt identity. Aggregate-local revisions are never
    /// part of this binding or the stable request digest.
    pub attempt_binding: Option<DurableAttemptBinding>,
    pub committed_at: Rfc3339,
    pub journal_sequence: u64,
    pub result: ProviderResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DurableAttemptBinding {
    pub attempt_number: AttemptNumber,
    pub attempt_token: ProviderAttemptToken,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedWorkerAction {
    pub action_id: ActionId,
    pub assignment_id: AssignmentId,
    pub authenticated_cutex_session: CutexSessionId,
    pub request_sha256: Sha256,
    pub attempt_binding: Option<DurableAttemptBinding>,
    pub context: WorkerMechanicalContext,
    pub prepared_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceSnapshot {
    pub schema: ProviderStoreSchema,
    pub journal_sequence: u64,
    pub journal_sha256: Sha256,
    #[serde(deserialize_with = "deserialize_task_revision_map")]
    pub task_revisions: BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRevisionRecord>>,
    pub assignments: BTreeMap<AssignmentId, Assignment>,
    #[serde(deserialize_with = "deserialize_attempt_number_map")]
    pub attempts: BTreeMap<AssignmentId, BTreeMap<AttemptNumber, Attempt>>,
    pub send_attempts: BTreeMap<SendAttemptId, SendAttempt>,
    #[serde(default)]
    pub completion_notifications: BTreeMap<NotificationId, CompletionNotification>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub worker_followup_notifications: BTreeMap<NotificationId, WorkerFollowupNotification>,
    pub workflows: BTreeMap<WorkflowId, Workflow>,
    pub receipts: BTreeMap<ActionId, ProviderReceipt>,
    pub prepared_worker_actions: BTreeMap<ActionId, PreparedWorkerAction>,
}

/// Compact semantic projection for an authenticated assignee. It deliberately
/// excludes durable identity, attempt tokens, local revisions and receipts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssigneeTaskServiceSnapshot {
    pub journal_sequence: u64,
    pub journal_sha256: Sha256,
    pub assignments: BTreeMap<AssignmentId, AssigneeAssignmentView>,
    #[serde(deserialize_with = "deserialize_attempt_number_map")]
    pub attempts: BTreeMap<AssignmentId, BTreeMap<AttemptNumber, AssigneeAttemptView>>,
}

fn deserialize_task_revision_map<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRevisionRecord>>, D::Error>
where
    D: Deserializer<'de>,
{
    let encoded =
        BTreeMap::<TaskId, BTreeMap<String, TaskRevisionRecord>>::deserialize(deserializer)?;
    encoded
        .into_iter()
        .map(|(task_id, revisions)| {
            deserialize_positive_map_keys(revisions, TaskRevision::new)
                .map(|revisions| (task_id, revisions))
        })
        .collect()
}

fn deserialize_attempt_number_map<'de, D, V>(
    deserializer: D,
) -> Result<BTreeMap<AssignmentId, BTreeMap<AttemptNumber, V>>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    let encoded = BTreeMap::<AssignmentId, BTreeMap<String, V>>::deserialize(deserializer)?;
    encoded
        .into_iter()
        .map(|(assignment_id, attempts)| {
            deserialize_positive_map_keys(attempts, AttemptNumber::new)
                .map(|attempts| (assignment_id, attempts))
        })
        .collect()
}

fn deserialize_positive_map_keys<K, V, ConstructionError, DecodeError>(
    encoded: BTreeMap<String, V>,
    construct: impl Fn(u64) -> Result<K, ConstructionError>,
) -> Result<BTreeMap<K, V>, DecodeError>
where
    K: Ord,
    DecodeError: serde::de::Error,
{
    encoded
        .into_iter()
        .map(|(key, value)| {
            let number = parse_canonical_positive_map_key(&key).map_err(DecodeError::custom)?;
            construct(number)
                .map_err(|_| DecodeError::custom("numeric map key is outside the supported range"))
                .map(|key| (key, value))
        })
        .collect()
}

fn parse_canonical_positive_map_key(value: &str) -> Result<u64, &'static str> {
    if !matches!(value.as_bytes(), [b'1'..=b'9', rest @ ..] if rest.iter().all(u8::is_ascii_digit))
    {
        return Err("numeric map key must be a canonical positive decimal integer");
    }
    value
        .parse()
        .map_err(|_| "numeric map key is outside the supported range")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssigneeAssignmentView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub state: AssignmentState,
    pub active_attempt: Option<AttemptNumber>,
    pub closure_reason: Option<ClosureReason>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssigneeAttemptView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub attempt_number: AttemptNumber,
    pub phase: AttemptPhase,
}

impl TaskServiceSnapshot {
    fn empty() -> Self {
        Self {
            schema: ProviderStoreSchema::V2,
            journal_sequence: 0,
            journal_sha256: Sha256::new(ZERO_SHA256).expect("zero hash"),
            task_revisions: BTreeMap::new(),
            assignments: BTreeMap::new(),
            attempts: BTreeMap::new(),
            send_attempts: BTreeMap::new(),
            completion_notifications: BTreeMap::new(),
            worker_followup_notifications: BTreeMap::new(),
            workflows: BTreeMap::new(),
            receipts: BTreeMap::new(),
            prepared_worker_actions: BTreeMap::new(),
        }
    }

    pub fn assignment(&self, assignment_id: &AssignmentId) -> Option<&Assignment> {
        self.assignments.get(assignment_id)
    }

    pub fn active_attempt(&self, assignment_id: &AssignmentId) -> Option<&Attempt> {
        let number = self.assignments.get(assignment_id)?.active_attempt?;
        self.attempts.get(assignment_id)?.get(&number)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedJournalRecord {
    schema: ProviderStoreSchema,
    sequence: u64,
    previous_event_sha256: Sha256,
    event_sha256: Sha256,
    operation: String,
    occurred_at: Rfc3339,
    resulting_state: TaskServiceSnapshot,
    /// Recovery-only evidence of the exact historical hash shape. This field
    /// is never part of the persisted journal record itself.
    #[serde(skip)]
    completion_notifications_was_present: bool,
}

#[derive(Debug)]
struct JournalTail {
    complete_record: Option<Vec<u8>>,
    complete_len: u64,
    file_len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
struct SnapshotImage {
    bytes: Vec<u8>,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchEvent {
    pub sequence: u64,
    pub event_sha256: Sha256,
    pub operation: String,
    pub occurred_at: Rfc3339,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    InvalidRequest(&'static str),
    Unauthorized,
    NotFound(&'static str),
    Conflict(&'static str),
    IllegalState(&'static str),
    RecoveryRequired,
    PersistenceUnavailable,
    InvalidStore,
    Io(io::ErrorKind),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "task service provider error: {self:?}")
    }
}

impl std::error::Error for ProviderError {}

impl From<io::Error> for ProviderError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.kind())
    }
}

#[derive(Clone)]
pub struct TaskServiceProvider {
    root: Arc<PathBuf>,
    process_lock: Arc<Mutex<()>>,
}

impl TaskServiceProvider {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProviderError> {
        let root = root.into();
        prepare_private_root(&root)?;
        Ok(Self {
            root: Arc::new(root),
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn recover(&self) -> Result<TaskServiceSnapshot, ProviderError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ProviderError::PersistenceUnavailable)?;
        self.with_store_lock(true, |lock| recover_locked(&self.root, lock))
    }

    pub fn query(&self) -> Result<TaskServiceSnapshot, ProviderError> {
        self.query_cancellable(|| false)
    }

    /// Captures the atomic snapshot and authenticated journal tail under a
    /// bounded lock, then releases every provider lock before parsing and
    /// validation. Old or interrupted stores fall back to a bounded full-chain
    /// copy. The cancellation probe is intended for HTTP disconnect/timeout
    /// checks and never changes provider state.
    pub fn query_cancellable(
        &self,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<TaskServiceSnapshot, ProviderError> {
        let deadline = Instant::now() + MAX_QUERY_DURATION;
        let (snapshot, tail) = self.capture_checkpoint_for_query(deadline, &mut cancelled)?;
        if let Some(snapshot) = snapshot {
            if query_stopped(deadline, &mut cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            if let Ok(state) = serde_json::from_slice::<TaskServiceSnapshot>(&snapshot.bytes) {
                if let Ok(state) = recover_checkpoint(state, snapshot.modified, &tail) {
                    if query_stopped(deadline, &mut cancelled) {
                        return Err(ProviderError::PersistenceUnavailable);
                    }
                    return Ok(state);
                }
            }
        }

        // Missing, stale, or inconsistent checkpoints remain compatible with
        // pre-checkpoint and crash-interrupted stores. The fallback copies a
        // consistent journal image and validates the complete chain without
        // holding either provider lock.
        let journal = self.capture_journal_for_query(deadline, &mut cancelled)?;
        let records = read_journal_bytes(&journal, deadline, &mut cancelled)?;
        recover_records(records, deadline, &mut cancelled)
    }

    pub fn query_assignee(
        &self,
        principal: &AuthenticatedPrincipal,
    ) -> Result<AssigneeTaskServiceSnapshot, ProviderError> {
        let session = principal.session_id()?;
        let state = self.query()?;
        let mut assignments = BTreeMap::new();
        let mut attempts = BTreeMap::new();
        for (assignment_id, assignment) in &state.assignments {
            if &assignment.assignee_cutex_session != session {
                continue;
            }
            assignments.insert(
                assignment_id.clone(),
                AssigneeAssignmentView {
                    project_id: assignment.project_id.clone(),
                    assignment_id: assignment_id.clone(),
                    task_id: assignment.task_id.clone(),
                    task_revision: assignment.task_revision,
                    state: assignment.state,
                    active_attempt: assignment.active_attempt,
                    closure_reason: assignment.closure.as_ref().map(|closure| closure.reason),
                },
            );
            if let Some(source) = state.attempts.get(assignment_id) {
                attempts.insert(
                    assignment_id.clone(),
                    source
                        .iter()
                        .map(|(number, attempt)| {
                            (
                                *number,
                                AssigneeAttemptView {
                                    project_id: attempt.project_id.clone(),
                                    assignment_id: assignment_id.clone(),
                                    attempt_number: *number,
                                    phase: attempt.phase,
                                },
                            )
                        })
                        .collect(),
                );
            }
        }
        Ok(AssigneeTaskServiceSnapshot {
            journal_sequence: state.journal_sequence,
            journal_sha256: state.journal_sha256,
            assignments,
            attempts,
        })
    }

    pub fn watch(
        &self,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<WatchEvent>, ProviderError> {
        if after_sequence > MAX_JSON_SAFE_INTEGER || limit == 0 || limit > MAX_WATCH_LIMIT {
            return Err(ProviderError::InvalidRequest(
                "invalid_watch_cursor_or_limit",
            ));
        }
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ProviderError::PersistenceUnavailable)?;
        self.with_store_lock(false, |_lock| {
            let (records, _) = read_journal(&self.root)?;
            Ok(records
                .into_iter()
                .filter(|record| record.sequence > after_sequence)
                .take(limit)
                .map(|record| WatchEvent {
                    sequence: record.sequence,
                    event_sha256: record.event_sha256,
                    operation: record.operation,
                    occurred_at: record.occurred_at,
                })
                .collect())
        })
    }

    pub fn create_revision(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &CreateRevisionRequest,
        expected_workflow_revision: Option<u64>,
    ) -> Result<ProviderReceipt, ProviderError> {
        let (seat, _) = principal.seat()?;
        let actor = principal.session_id()?.clone();
        validate_contract(&request.opaque_contract, &request.contract_sha256)?;
        let digest = request_digest("create_revision", principal, request)?;
        self.mutate(
            "create_revision",
            &request.action_id,
            digest,
            None,
            |state, now| {
                if state.schema == ProviderStoreSchema::V3 {
                    return Err(ProviderError::InvalidRequest(
                        "legacy_writes_disabled_after_v3_activation",
                    ));
                }
                if let Some(workflow) = state.workflows.get(&request.workflow_id) {
                    let expected = expected_workflow_revision
                        .ok_or(ProviderError::Conflict("workflow_presence_conflict"))?;
                    require_local_revision(
                        expected,
                        workflow.local_revision,
                        "workflow_revision_conflict",
                    )?;
                    if &workflow.coordinator_seat_id != seat {
                        return Err(ProviderError::Unauthorized);
                    }
                    if workflow.execution_guard != WorkflowExecutionGuard::Open {
                        return Err(ProviderError::IllegalState("workflow_quiescing"));
                    }
                    state
                        .workflows
                        .get_mut(&request.workflow_id)
                        .ok_or(ProviderError::InvalidStore)?
                        .local_revision = next_local(workflow.local_revision)?;
                } else {
                    if expected_workflow_revision.is_some() {
                        return Err(ProviderError::Conflict("workflow_presence_conflict"));
                    }
                    state.workflows.insert(
                        request.workflow_id.clone(),
                        Workflow {
                            project_id: None,
                            workflow_id: request.workflow_id.clone(),
                            coordinator_seat_id: seat.clone(),
                            local_revision: 1,
                            execution_guard: WorkflowExecutionGuard::Open,
                        },
                    );
                }
                let revisions = state
                    .task_revisions
                    .entry(request.task_id.clone())
                    .or_default();
                if revisions.contains_key(&request.task_revision) {
                    return Err(ProviderError::Conflict("task_revision_exists"));
                }
                if let Some(latest) = revisions.keys().next_back() {
                    if request.task_revision <= *latest {
                        return Err(ProviderError::Conflict("task_revision_not_increasing"));
                    }
                }
                let record = TaskRevisionRecord {
                    project_id: None,
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    contract_sha256: request.contract_sha256.clone(),
                    opaque_contract: request.opaque_contract.clone(),
                    completion_policy: request.completion_policy.clone(),
                    workflow_id: request.workflow_id.clone(),
                    created_at: now.clone(),
                    created_by_cutex_session: actor,
                };
                revisions.insert(request.task_revision, record.clone());
                Ok(ProviderResult::TaskRevision(record))
            },
        )
    }

    pub fn create_project_revision(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &CreateProjectRevisionRequest,
        expected_workflow_revision: Option<u64>,
    ) -> Result<ProviderReceipt, ProviderError> {
        if request.schema != ProviderActionSchema::V3 {
            return Err(ProviderError::InvalidRequest("project_create_requires_v3"));
        }
        let (seat, _) = principal.seat()?;
        let actor = principal.session_id()?.clone();
        validate_contract(&request.opaque_contract, &request.contract_sha256)?;
        let digest = request_digest("create_project_revision", principal, request)?;
        self.mutate(
            "create_project_revision",
            &request.action_id,
            digest,
            None,
            |state, now| {
                if let Some(workflow) = state.workflows.get(&request.workflow_id) {
                    if workflow.project_id.as_ref() != Some(&request.project_id) {
                        return Err(ProviderError::Conflict("workflow_project_conflict"));
                    }
                    let expected = expected_workflow_revision
                        .ok_or(ProviderError::Conflict("workflow_presence_conflict"))?;
                    require_local_revision(
                        expected,
                        workflow.local_revision,
                        "workflow_revision_conflict",
                    )?;
                    if &workflow.coordinator_seat_id != seat {
                        return Err(ProviderError::Unauthorized);
                    }
                    if workflow.execution_guard != WorkflowExecutionGuard::Open {
                        return Err(ProviderError::IllegalState("workflow_quiescing"));
                    }
                    state
                        .workflows
                        .get_mut(&request.workflow_id)
                        .ok_or(ProviderError::InvalidStore)?
                        .local_revision = next_local(workflow.local_revision)?;
                } else {
                    if expected_workflow_revision.is_some() {
                        return Err(ProviderError::Conflict("workflow_presence_conflict"));
                    }
                    state.workflows.insert(
                        request.workflow_id.clone(),
                        Workflow {
                            project_id: Some(request.project_id.clone()),
                            workflow_id: request.workflow_id.clone(),
                            coordinator_seat_id: seat.clone(),
                            local_revision: 1,
                            execution_guard: WorkflowExecutionGuard::Open,
                        },
                    );
                }
                let revisions = state
                    .task_revisions
                    .entry(request.task_id.clone())
                    .or_default();
                if revisions
                    .values()
                    .any(|revision| revision.project_id.as_ref() != Some(&request.project_id))
                {
                    return Err(ProviderError::Conflict("task_project_conflict"));
                }
                if revisions.contains_key(&request.task_revision) {
                    return Err(ProviderError::Conflict("task_revision_exists"));
                }
                if let Some(latest) = revisions.keys().next_back() {
                    if request.task_revision <= *latest {
                        return Err(ProviderError::Conflict("task_revision_not_increasing"));
                    }
                }
                let record = TaskRevisionRecord {
                    project_id: Some(request.project_id.clone()),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    contract_sha256: request.contract_sha256.clone(),
                    opaque_contract: request.opaque_contract.clone(),
                    completion_policy: request.completion_policy.clone(),
                    workflow_id: request.workflow_id.clone(),
                    created_at: now.clone(),
                    created_by_cutex_session: actor,
                };
                revisions.insert(request.task_revision, record.clone());
                state.schema = ProviderStoreSchema::V3;
                Ok(ProviderResult::TaskRevision(record))
            },
        )
    }

    pub fn assign_and_dispatch(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &AssignAndDispatchRequest,
        expected_workflow_revision: u64,
        human_readable_content: &str,
    ) -> Result<ProviderReceipt, ProviderError> {
        validate_text(
            human_readable_content,
            MAX_EVIDENCE_BYTES,
            "assignment_content",
        )?;
        let digest = request_digest(
            "assign_and_dispatch",
            principal,
            &(request, human_readable_content),
        )?;
        self.mutate(
            "assign_and_dispatch",
            &request.action_id,
            digest,
            None,
            |state, now| {
                if state.schema == ProviderStoreSchema::V3 {
                    return Err(ProviderError::InvalidRequest(
                        "legacy_writes_disabled_after_v3_activation",
                    ));
                }
                if state.assignments.contains_key(&request.assignment_id) {
                    return Err(ProviderError::Conflict("assignment_exists"));
                }
                if state.send_attempts.contains_key(&request.send_attempt_id) {
                    return Err(ProviderError::Conflict("send_attempt_exists"));
                }
                let revision = task_revision(state, &request.task_id, request.task_revision)?;
                if revision.project_id.is_some() {
                    return Err(ProviderError::Conflict("project_assignment_requires_v3"));
                }
                authorize_workflow_coordinator(state, principal, &revision.workflow_id)?;
                require_local_revision(
                    expected_workflow_revision,
                    state
                        .workflows
                        .get(&revision.workflow_id)
                        .ok_or(ProviderError::InvalidStore)?
                        .local_revision,
                    "workflow_revision_conflict",
                )?;
                let assignment = Assignment {
                    project_id: None,
                    assignment_id: request.assignment_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    assignee_cutex_session: request.assignee_cutex_session.clone(),
                    state: AssignmentState::AwaitingAck,
                    local_revision: 1,
                    created_at: now.clone(),
                    acknowledged_at: None,
                    active_attempt: None,
                    retry_authorization: None,
                    closure: None,
                };
                let send_attempt = SendAttempt {
                    project_id: None,
                    send_attempt_id: request.send_attempt_id.clone(),
                    assignment_id: request.assignment_id.clone(),
                    retry_ordinal: 1,
                    external_message_id: nonempty(
                        &request.external_message_id,
                        "external_message_id",
                    )?,
                    local_revision: 1,
                    events: vec![CommunicationEvent {
                        kind: CommunicationEventKind::SendPrepared,
                        receipt_reference: None,
                        recorded_at: now.clone(),
                    }],
                };
                state
                    .assignments
                    .insert(request.assignment_id.clone(), assignment.clone());
                state
                    .send_attempts
                    .insert(request.send_attempt_id.clone(), send_attempt.clone());
                Ok(ProviderResult::Assignment {
                    assignment,
                    send_attempt: Some(send_attempt),
                })
            },
        )
    }

    pub fn assign_project_and_dispatch(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &AssignProjectAndDispatchRequest,
        expected_workflow_revision: u64,
        human_readable_content: &str,
    ) -> Result<ProviderReceipt, ProviderError> {
        if request.schema != ProviderActionSchema::V3 {
            return Err(ProviderError::InvalidRequest("project_assign_requires_v3"));
        }
        validate_text(
            human_readable_content,
            MAX_EVIDENCE_BYTES,
            "assignment_content",
        )?;
        let digest = request_digest(
            "assign_project_and_dispatch",
            principal,
            &(request, human_readable_content),
        )?;
        self.mutate(
            "assign_project_and_dispatch",
            &request.action_id,
            digest,
            None,
            |state, now| {
                if state.assignments.contains_key(&request.assignment_id) {
                    return Err(ProviderError::Conflict("assignment_exists"));
                }
                if state.send_attempts.contains_key(&request.send_attempt_id) {
                    return Err(ProviderError::Conflict("send_attempt_exists"));
                }
                let revision = task_revision(state, &request.task_id, request.task_revision)?;
                if revision.project_id.as_ref() != Some(&request.project_id) {
                    return Err(ProviderError::Conflict("task_project_conflict"));
                }
                authorize_workflow_coordinator(state, principal, &revision.workflow_id)?;
                let workflow = state
                    .workflows
                    .get(&revision.workflow_id)
                    .ok_or(ProviderError::InvalidStore)?;
                if workflow.project_id.as_ref() != Some(&request.project_id) {
                    return Err(ProviderError::Conflict("workflow_project_conflict"));
                }
                require_local_revision(
                    expected_workflow_revision,
                    workflow.local_revision,
                    "workflow_revision_conflict",
                )?;
                let assignment = Assignment {
                    project_id: Some(request.project_id.clone()),
                    assignment_id: request.assignment_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    assignee_cutex_session: request.assignee_cutex_session.clone(),
                    state: AssignmentState::AwaitingAck,
                    local_revision: 1,
                    created_at: now.clone(),
                    acknowledged_at: None,
                    active_attempt: None,
                    retry_authorization: None,
                    closure: None,
                };
                let send_attempt = SendAttempt {
                    project_id: Some(request.project_id.clone()),
                    send_attempt_id: request.send_attempt_id.clone(),
                    assignment_id: request.assignment_id.clone(),
                    retry_ordinal: 1,
                    external_message_id: nonempty(
                        &request.external_message_id,
                        "external_message_id",
                    )?,
                    local_revision: 1,
                    events: vec![CommunicationEvent {
                        kind: CommunicationEventKind::SendPrepared,
                        receipt_reference: None,
                        recorded_at: now.clone(),
                    }],
                };
                state
                    .assignments
                    .insert(request.assignment_id.clone(), assignment.clone());
                state
                    .send_attempts
                    .insert(request.send_attempt_id.clone(), send_attempt.clone());
                state.schema = ProviderStoreSchema::V3;
                Ok(ProviderResult::Assignment {
                    assignment,
                    send_attempt: Some(send_attempt),
                })
            },
        )
    }

    pub fn retry_delivery(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &RetryDeliveryRequest,
        expected_assignment_revision: u64,
        human_readable_content: &str,
    ) -> Result<ProviderReceipt, ProviderError> {
        validate_text(
            human_readable_content,
            MAX_EVIDENCE_BYTES,
            "assignment_content",
        )?;
        let digest = request_digest(
            "retry_delivery",
            principal,
            &(request, human_readable_content),
        )?;
        self.mutate(
            "retry_delivery",
            &request.action_id,
            digest,
            None,
            |state, now| {
                require_scoped_mutation_after_v3(state, &request.assignment_id)?;
                let assignment = assignment(state, &request.assignment_id)?.clone();
                require_local_revision(
                    expected_assignment_revision,
                    assignment.local_revision,
                    "assignment_revision_conflict",
                )?;
                if assignment.state == AssignmentState::Closed {
                    return Err(ProviderError::IllegalState("assignment_closed"));
                }
                let revision = task_revision(state, &assignment.task_id, assignment.task_revision)?;
                authorize_workflow_coordinator(state, principal, &revision.workflow_id)?;
                if state.send_attempts.contains_key(&request.send_attempt_id) {
                    return Err(ProviderError::Conflict("send_attempt_exists"));
                }
                if state.send_attempts.values().any(|attempt| {
                    attempt.external_message_id.trim() == request.external_message_id.trim()
                }) {
                    return Err(ProviderError::Conflict("external_message_id_reused"));
                }
                let ordinal = state
                    .send_attempts
                    .values()
                    .filter(|attempt| attempt.assignment_id == request.assignment_id)
                    .map(|attempt| attempt.retry_ordinal)
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(ProviderError::Conflict("send_attempt_ordinal_overflow"))?;
                let send_attempt = SendAttempt {
                    project_id: assignment.project_id.clone(),
                    send_attempt_id: request.send_attempt_id.clone(),
                    assignment_id: request.assignment_id.clone(),
                    retry_ordinal: ordinal,
                    external_message_id: nonempty(
                        &request.external_message_id,
                        "external_message_id",
                    )?,
                    local_revision: 1,
                    events: vec![CommunicationEvent {
                        kind: CommunicationEventKind::SendPrepared,
                        receipt_reference: None,
                        recorded_at: now.clone(),
                    }],
                };
                state
                    .send_attempts
                    .insert(request.send_attempt_id.clone(), send_attempt.clone());
                Ok(ProviderResult::SendAttempt(send_attempt))
            },
        )
    }

    pub fn record_communication_event(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &CommunicationEventRequest,
    ) -> Result<ProviderReceipt, ProviderError> {
        if !matches!(
            &principal.kind,
            AuthenticatedPrincipalKind::TaskServiceSystem
        ) {
            return Err(ProviderError::Unauthorized);
        }
        let digest = request_digest(
            "record_communication_event",
            principal,
            &(
                &request.schema,
                &request.action_id,
                &request.send_attempt_id,
                &request.kind,
                &request.receipt_reference,
            ),
        )?;
        self.mutate(
            "record_communication_event",
            &request.action_id,
            digest,
            None,
            |state, now| {
                let assignment_id = state
                    .send_attempts
                    .get(&request.send_attempt_id)
                    .ok_or(ProviderError::NotFound("send_attempt"))?
                    .assignment_id
                    .clone();
                require_scoped_mutation_after_v3(state, &assignment_id)?;
                let (result, assignment_id) = {
                    let send = state
                        .send_attempts
                        .get_mut(&request.send_attempt_id)
                        .ok_or(ProviderError::NotFound("send_attempt"))?;
                    require_local_revision(
                        request.expected_send_attempt_revision,
                        send.local_revision,
                        "send_attempt_revision_conflict",
                    )?;
                    send.local_revision = next_local(send.local_revision)?;
                    send.events.push(CommunicationEvent {
                        kind: request.kind,
                        receipt_reference: request.receipt_reference.clone(),
                        recorded_at: now.clone(),
                    });
                    (
                        ProviderResult::SendAttempt(send.clone()),
                        send.assignment_id.clone(),
                    )
                };
                if request.kind == CommunicationEventKind::RetriesExhausted {
                    schedule_completion_notification(
                        state,
                        &assignment_id,
                        state
                            .assignment(&assignment_id)
                            .and_then(|value| value.active_attempt),
                        &request.action_id,
                        CompletionNotificationKind::RetriesExhausted,
                        NotificationTarget::Coordinator,
                        CompletionNotificationDeliveryMode::Soon,
                        now,
                    )?;
                }
                Ok(result)
            },
        )
    }

    pub fn record_completion_notification_fact(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &CompletionNotificationFactRequest,
    ) -> Result<ProviderReceipt, ProviderError> {
        if !matches!(
            principal.kind,
            AuthenticatedPrincipalKind::TaskServiceSystem
        ) {
            return Err(ProviderError::Unauthorized);
        }
        let digest = request_digest(
            "record_completion_notification_fact",
            principal,
            &(
                &request.schema,
                &request.action_id,
                &request.notification_id,
                &request.kind,
                &request.reference,
            ),
        )?;
        self.mutate(
            "record_completion_notification_fact",
            &request.action_id,
            digest,
            None,
            |state, now| {
                let assignment_id = state
                    .completion_notifications
                    .get(&request.notification_id)
                    .ok_or(ProviderError::NotFound("completion_notification"))?
                    .assignment_id
                    .clone();
                require_scoped_mutation_after_v3(state, &assignment_id)?;
                let notification = state
                    .completion_notifications
                    .get_mut(&request.notification_id)
                    .ok_or(ProviderError::NotFound("completion_notification"))?;
                require_local_revision(
                    request.expected_notification_revision,
                    notification.local_revision,
                    "completion_notification_revision_conflict",
                )?;
                notification.local_revision = next_local(notification.local_revision)?;
                notification.facts.push(CompletionNotificationFact {
                    kind: request.kind,
                    reference: request.reference.clone(),
                    recorded_at: now.clone(),
                });
                Ok(ProviderResult::CompletionNotification(notification.clone()))
            },
        )
    }

    pub fn record_worker_followup_fact(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &WorkerFollowupFactRequest,
    ) -> Result<ProviderReceipt, ProviderError> {
        if !matches!(
            principal.kind,
            AuthenticatedPrincipalKind::TaskServiceSystem
        ) {
            return Err(ProviderError::Unauthorized);
        }
        let digest = request_digest(
            "record_worker_followup_fact",
            principal,
            &(
                &request.schema,
                &request.action_id,
                &request.notification_id,
                &request.kind,
                &request.reference,
            ),
        )?;
        self.mutate(
            "record_worker_followup_fact",
            &request.action_id,
            digest,
            None,
            |state, now| {
                let assignment_id = state
                    .worker_followup_notifications
                    .get(&request.notification_id)
                    .ok_or(ProviderError::NotFound("worker_followup_notification"))?
                    .assignment_id
                    .clone();
                require_scoped_mutation_after_v3(state, &assignment_id)?;
                let notification = state
                    .worker_followup_notifications
                    .get_mut(&request.notification_id)
                    .ok_or(ProviderError::NotFound("worker_followup_notification"))?;
                require_local_revision(
                    request.expected_notification_revision,
                    notification.local_revision,
                    "worker_followup_notification_revision_conflict",
                )?;
                notification.local_revision = next_local(notification.local_revision)?;
                notification.facts.push(CompletionNotificationFact {
                    kind: request.kind,
                    reference: request.reference.clone(),
                    recorded_at: now.clone(),
                });
                Ok(ProviderResult::WorkerFollowupNotification(
                    notification.clone(),
                ))
            },
        )
    }

    /// Returns the exact mechanical binding for one assignment and only to
    /// its authenticated durable assignee. The semantic Worker document is
    /// deliberately not part of this response.
    pub fn worker_context(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &WorkerContextRequest,
    ) -> Result<WorkerContext, ProviderError> {
        let session = principal.session_id()?;
        let state = self.query()?;
        let assignment = assignment(&state, &request.assignment_id)?;
        if &assignment.assignee_cutex_session != session {
            return Err(ProviderError::Unauthorized);
        }
        Ok(WorkerContext {
            assignment_id: request.assignment_id.clone(),
            context: worker_mechanical_context(&state, &request.assignment_id)?,
        })
    }

    /// Atomically probes committed semantic identity or durably prepares one
    /// new Worker action. Repeated preparation refreshes only aggregate-local
    /// CAS values; the original assignment/attempt identity cannot retarget.
    pub fn prepare_worker_action(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &WorkerPrepareRequest,
    ) -> Result<WorkerPrepareOutcome, ProviderError> {
        let session = principal.session_id()?.clone();
        let action_id = request.action.action_id().clone();
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ProviderError::PersistenceUnavailable)?;
        self.with_store_lock(true, |lock| {
            let mut state = recover_checkpoint_locked(&self.root, lock)?;
            if let Some(receipt) = state.receipts.get(&action_id) {
                let digest = worker_request_digest(
                    request.action.operation(),
                    principal,
                    &request.action,
                    receipt.attempt_binding.as_ref(),
                )?;
                return if receipt.request_sha256 == digest {
                    Ok(WorkerPrepareOutcome::Committed(receipt.clone()))
                } else {
                    Err(ProviderError::Conflict("action_id_payload_conflict"))
                };
            }
            require_scoped_mutation_after_v3(&state, request.action.assignment_id())?;

            if let Some(prepared) = state.prepared_worker_actions.get(&action_id).cloned() {
                if prepared.authenticated_cutex_session != session {
                    return Err(ProviderError::Unauthorized);
                }
                let digest = worker_request_digest(
                    request.action.operation(),
                    principal,
                    &request.action,
                    prepared.attempt_binding.as_ref(),
                )?;
                if digest != prepared.request_sha256 {
                    return Err(ProviderError::Conflict("action_id_payload_conflict"));
                }
                let (context, binding) = prepare_worker_context(
                    &state,
                    &session,
                    &request.action,
                    prepared.attempt_binding.as_ref(),
                )?;
                if binding != prepared.attempt_binding {
                    return Err(ProviderError::Conflict("attempt_handle_conflict"));
                }
                let envelope = WorkerProviderActionEnvelope {
                    schema: WorkerProviderRequestSchema::V2,
                    action: request.action.clone(),
                    context,
                };
                if envelope.context == prepared.context {
                    return Ok(WorkerPrepareOutcome::Prepared(envelope));
                }
                let current = state
                    .prepared_worker_actions
                    .get_mut(&action_id)
                    .ok_or(ProviderError::InvalidStore)?;
                current.context = envelope.context.clone();
                append_and_snapshot(&self.root, state, "refresh_worker_action", now())?;
                return Ok(WorkerPrepareOutcome::Prepared(envelope));
            }

            let (context, attempt_binding) =
                prepare_worker_context(&state, &session, &request.action, None)?;
            if state.prepared_worker_actions.len() >= MAX_PREPARED_WORKER_ACTIONS {
                return Err(ProviderError::Conflict("prepared_action_capacity"));
            }
            let request_sha256 = worker_request_digest(
                request.action.operation(),
                principal,
                &request.action,
                attempt_binding.as_ref(),
            )?;
            let envelope = WorkerProviderActionEnvelope {
                schema: WorkerProviderRequestSchema::V2,
                action: request.action.clone(),
                context,
            };
            let prepared_at = now();
            state.prepared_worker_actions.insert(
                action_id.clone(),
                PreparedWorkerAction {
                    action_id,
                    assignment_id: request.action.assignment_id().clone(),
                    authenticated_cutex_session: session,
                    request_sha256,
                    attempt_binding,
                    context: envelope.context.clone(),
                    prepared_at: prepared_at.clone(),
                },
            );
            append_and_snapshot(&self.root, state, "prepare_worker_action", prepared_at)?;
            Ok(WorkerPrepareOutcome::Prepared(envelope))
        })
    }

    pub fn execute_worker_action(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &WorkerProviderActionEnvelope,
    ) -> Result<ProviderReceipt, ProviderError> {
        let action = &request.action;
        let operation = action.operation();
        let assignment_id = action.assignment_id();
        let session = principal.session_id()?.clone();
        // Resolve the durable binding before hashing. Caller-supplied mechanics
        // are only a transport copy and cannot redefine prepared or committed
        // semantic identity.
        let recovered = self.query()?;
        if !recovered.receipts.contains_key(request.action_id()) {
            require_scoped_mutation_after_v3(&recovered, assignment_id)?;
        }
        let known_binding = recovered
            .receipts
            .get(request.action_id())
            .map(|receipt| receipt.attempt_binding.clone())
            .or_else(|| {
                recovered
                    .prepared_worker_actions
                    .get(request.action_id())
                    .map(|prepared| prepared.attempt_binding.clone())
            });
        let attempt_binding = match known_binding {
            Some(binding) => binding,
            None => worker_envelope_attempt_binding(request)?,
        };
        let digest = worker_request_digest(operation, principal, action, attempt_binding.as_ref())?;
        let prepared_digest = digest.clone();
        let prepared_binding = attempt_binding.clone();
        self.mutate(
            operation,
            request.action_id(),
            digest,
            attempt_binding,
            |state, now| {
                require_scoped_mutation_after_v3(state, assignment_id)?;
                let prepared = state
                    .prepared_worker_actions
                    .get(request.action_id())
                    .ok_or(ProviderError::Conflict("worker_action_not_prepared"))?;
                if prepared.authenticated_cutex_session != session {
                    return Err(ProviderError::Unauthorized);
                }
                if prepared.request_sha256 != prepared_digest
                    || prepared.attempt_binding != prepared_binding
                    || prepared.context != request.context
                {
                    return Err(ProviderError::Conflict("prepared_binding_conflict"));
                }
                state.prepared_worker_actions.remove(request.action_id());
                let current = assignment(state, assignment_id)?;
                if current.assignee_cutex_session != session {
                    return Err(ProviderError::Unauthorized);
                }
                match action {
                    WorkerActionRequest::Start(value) => {
                        require_no_attempt_context(&request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        start_attempt(state, value, now)
                    }
                    WorkerActionRequest::Decline(value) => {
                        require_no_attempt_context(&request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        let result = decline_assignment(state, value, now)?;
                        schedule_completion_notification(
                            state,
                            assignment_id,
                            None,
                            &value.action_id,
                            CompletionNotificationKind::Declined,
                            NotificationTarget::Coordinator,
                            CompletionNotificationDeliveryMode::Soon,
                            now,
                        )?;
                        Ok(result)
                    }
                    WorkerActionRequest::ReportStatus(value) => {
                        validate_text(&value.summary, MAX_EVIDENCE_BYTES, "status_summary")?;
                        require_worker_attempt_context(state, assignment_id, &request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        let attempt = bound_attempt_mut(
                            state,
                            assignment_id,
                            request.context.attempt.as_ref().expect("validated context"),
                        )?;
                        if !matches!(attempt.phase, AttemptPhase::Running | AttemptPhase::Blocked) {
                            return Err(ProviderError::IllegalState("status_not_allowed"));
                        }
                        attempt.local_revision = next_local(attempt.local_revision)?;
                        attempt.updated_at = now.clone();
                        attempt.status_receipts.push(StatusReceipt {
                            project_id: attempt.project_id.clone(),
                            action_id: value.action_id.clone(),
                            summary: value.summary.clone(),
                            evidence_sha256: value.evidence_sha256.clone(),
                            recorded_at: now.clone(),
                        });
                        Ok(ProviderResult::Attempt(attempt.clone()))
                    }
                    WorkerActionRequest::Block(value) => {
                        validate_text(
                            &value.summary,
                            MAX_BLOCKER_SUMMARY_BYTES,
                            "blocker_summary",
                        )?;
                        require_worker_attempt_context(state, assignment_id, &request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        let result = transition_bound_attempt(
                            state,
                            &value.assignment_id,
                            request.context.attempt.as_ref().expect("validated context"),
                            AttemptPhase::Running,
                            AttemptPhase::Blocked,
                            now,
                        )?;
                        schedule_blocked_notification(
                            state,
                            assignment_id,
                            request
                                .context
                                .attempt
                                .as_ref()
                                .map(|value| value.attempt_number),
                            &value.action_id,
                            &value.summary,
                            now,
                        )?;
                        Ok(result)
                    }
                    WorkerActionRequest::Resume(value) => {
                        require_worker_attempt_context(state, assignment_id, &request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        transition_bound_attempt(
                            state,
                            &value.assignment_id,
                            request.context.attempt.as_ref().expect("validated context"),
                            AttemptPhase::Blocked,
                            AttemptPhase::Running,
                            now,
                        )
                    }
                    WorkerActionRequest::Submit(value) => {
                        validate_text(
                            &value.result_reference,
                            MAX_EVIDENCE_BYTES,
                            "result_reference",
                        )?;
                        require_worker_attempt_context(state, assignment_id, &request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        let attempt = bound_attempt_mut(
                            state,
                            assignment_id,
                            request.context.attempt.as_ref().expect("validated context"),
                        )?;
                        if attempt.phase != AttemptPhase::Running {
                            return Err(ProviderError::IllegalState("submit_requires_running"));
                        }
                        attempt.phase = AttemptPhase::ReviewReady;
                        attempt.local_revision = next_local(attempt.local_revision)?;
                        attempt.updated_at = now.clone();
                        attempt.result_receipts.push(ResultReceipt {
                            project_id: attempt.project_id.clone(),
                            action_id: value.action_id.clone(),
                            result_sha256: value.result_sha256.clone(),
                            result_reference: value.result_reference.clone(),
                            submitted_at: now.clone(),
                        });
                        let result = ProviderResult::Attempt(attempt.clone());
                        schedule_completion_notification(
                            state,
                            assignment_id,
                            request
                                .context
                                .attempt
                                .as_ref()
                                .map(|value| value.attempt_number),
                            &value.action_id,
                            CompletionNotificationKind::ReviewReady,
                            NotificationTarget::CompletionAuthority,
                            CompletionNotificationDeliveryMode::AfterTurn,
                            now,
                        )?;
                        Ok(result)
                    }
                    WorkerActionRequest::AbortAttempt(value) => {
                        require_worker_attempt_context(state, assignment_id, &request.context)?;
                        require_assignment_revision(state, assignment_id, &request.context)?;
                        let result = {
                            let attempt = bound_attempt_mut(
                                state,
                                assignment_id,
                                request.context.attempt.as_ref().expect("validated context"),
                            )?;
                            if !matches!(
                                attempt.phase,
                                AttemptPhase::Running
                                    | AttemptPhase::Blocked
                                    | AttemptPhase::ReviewReady
                            ) {
                                return Err(ProviderError::IllegalState("abort_not_allowed"));
                            }
                            attempt.phase = AttemptPhase::Aborted;
                            attempt.local_revision = next_local(attempt.local_revision)?;
                            attempt.updated_at = now.clone();
                            attempt.terminal_action_id = Some(value.action_id.clone());
                            attempt.clone()
                        };
                        let assignment = assignment_mut(state, &value.assignment_id)?;
                        assignment.state = AssignmentState::RetryPending;
                        assignment.local_revision = next_local(assignment.local_revision)?;
                        assignment.retry_authorization = None;
                        schedule_completion_notification(
                            state,
                            assignment_id,
                            request
                                .context
                                .attempt
                                .as_ref()
                                .map(|value| value.attempt_number),
                            &value.action_id,
                            CompletionNotificationKind::AttemptAborted,
                            NotificationTarget::Coordinator,
                            CompletionNotificationDeliveryMode::Soon,
                            now,
                        )?;
                        Ok(ProviderResult::Attempt(result))
                    }
                }
            },
        )
    }

    pub fn execute_terminal_action(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &TerminalActionEnvelope,
    ) -> Result<ProviderReceipt, ProviderError> {
        let (operation, body) = match &request.command {
            TerminalAuthorityRequest::AcceptResult(value) => ("accept_result", value),
            TerminalAuthorityRequest::RequestChanges(value) => ("request_changes", value),
            TerminalAuthorityRequest::FailResult(value) => ("fail_result", value),
            TerminalAuthorityRequest::Cancel(value) => ("cancel", value),
        };
        let attempt_binding =
            request
                .context
                .attempt
                .as_ref()
                .map(|attempt| DurableAttemptBinding {
                    attempt_number: attempt.attempt_number,
                    attempt_token: attempt.attempt_token.clone(),
                });
        let digest = request_digest(operation, principal, &(&request.command, &attempt_binding))?;
        self.mutate(
            operation,
            &body.action_id,
            digest,
            attempt_binding,
            |state, now| {
                require_scoped_mutation_after_v3(state, &body.assignment_id)?;
                authorize_terminal(state, principal, &body.assignment_id)?;
                match &request.command {
                    TerminalAuthorityRequest::RequestChanges(_) => {
                        let decision_reference = body
                            .decision_reference
                            .as_deref()
                            .ok_or(ProviderError::InvalidRequest("decision_reference_required"))?;
                        validate_decision_reference(decision_reference)?;
                        require_worker_attempt_context(
                            state,
                            &body.assignment_id,
                            &request.context,
                        )?;
                        require_assignment_revision(state, &body.assignment_id, &request.context)?;
                        let resumed = {
                            let attempt = bound_attempt_mut(
                                state,
                                &body.assignment_id,
                                request.context.attempt.as_ref().expect("validated context"),
                            )?;
                            if attempt.phase != AttemptPhase::ReviewReady {
                                return Err(ProviderError::IllegalState(
                                    "changes_require_review_ready",
                                ));
                            }
                            attempt.phase = AttemptPhase::Running;
                            attempt.local_revision = next_local(attempt.local_revision)?;
                            attempt.updated_at = now.clone();
                            attempt.clone()
                        };
                        schedule_worker_followup_notification(
                            state,
                            &body.assignment_id,
                            resumed.attempt_number,
                            &body.action_id,
                            decision_reference,
                            now,
                        )?;
                        Ok(ProviderResult::Attempt(resumed))
                    }
                    TerminalAuthorityRequest::AcceptResult(_) => {
                        require_worker_attempt_context(
                            state,
                            &body.assignment_id,
                            &request.context,
                        )?;
                        require_assignment_revision(state, &body.assignment_id, &request.context)?;
                        let completed = {
                            let attempt = bound_attempt_mut(
                                state,
                                &body.assignment_id,
                                request.context.attempt.as_ref().expect("validated context"),
                            )?;
                            if attempt.phase != AttemptPhase::ReviewReady {
                                return Err(ProviderError::IllegalState(
                                    "accept_requires_review_ready",
                                ));
                            }
                            attempt.phase = AttemptPhase::Completed;
                            attempt.local_revision = next_local(attempt.local_revision)?;
                            attempt.updated_at = now.clone();
                            attempt.terminal_action_id = Some(body.action_id.clone());
                            attempt.clone()
                        };
                        close_assignment_internal(
                            state,
                            &body.assignment_id,
                            ClosureReason::Completed,
                            Some(completed.attempt_number),
                            &body.action_id,
                            now,
                        )?;
                        schedule_completion_notification(
                            state,
                            &body.assignment_id,
                            Some(completed.attempt_number),
                            &body.action_id,
                            CompletionNotificationKind::TerminalClosure,
                            NotificationTarget::Coordinator,
                            CompletionNotificationDeliveryMode::AfterTurn,
                            now,
                        )?;
                        Ok(ProviderResult::Attempt(completed))
                    }
                    TerminalAuthorityRequest::FailResult(_) => {
                        require_worker_attempt_context(
                            state,
                            &body.assignment_id,
                            &request.context,
                        )?;
                        require_assignment_revision(state, &body.assignment_id, &request.context)?;
                        let failed = {
                            let attempt = bound_attempt_mut(
                                state,
                                &body.assignment_id,
                                request.context.attempt.as_ref().expect("validated context"),
                            )?;
                            if attempt.phase != AttemptPhase::ReviewReady {
                                return Err(ProviderError::IllegalState(
                                    "fail_requires_review_ready",
                                ));
                            }
                            attempt.phase = AttemptPhase::Failed;
                            attempt.local_revision = next_local(attempt.local_revision)?;
                            attempt.updated_at = now.clone();
                            attempt.terminal_action_id = Some(body.action_id.clone());
                            attempt.clone()
                        };
                        let assignment = assignment_mut(state, &body.assignment_id)?;
                        assignment.state = AssignmentState::RetryPending;
                        assignment.local_revision = next_local(assignment.local_revision)?;
                        assignment.retry_authorization = None;
                        schedule_completion_notification(
                            state,
                            &body.assignment_id,
                            Some(failed.attempt_number),
                            &body.action_id,
                            CompletionNotificationKind::OwnerActionRequired,
                            NotificationTarget::Coordinator,
                            CompletionNotificationDeliveryMode::Soon,
                            now,
                        )?;
                        Ok(ProviderResult::Attempt(failed))
                    }
                    TerminalAuthorityRequest::Cancel(_) => {
                        require_cancel_context(state, &body.assignment_id, &request.context)?;
                        let result = cancel_assignment_internal(
                            state,
                            &body.assignment_id,
                            &body.action_id,
                            now,
                        )?;
                        schedule_completion_notification(
                            state,
                            &body.assignment_id,
                            request
                                .context
                                .attempt
                                .as_ref()
                                .map(|value| value.attempt_number),
                            &body.action_id,
                            CompletionNotificationKind::TerminalClosure,
                            NotificationTarget::Coordinator,
                            CompletionNotificationDeliveryMode::Soon,
                            now,
                        )?;
                        Ok(result)
                    }
                }
            },
        )
    }

    pub fn cancel_assignment(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &AssignmentActionRequest,
        expected_assignment_revision: u64,
        active_attempt_context: Option<&AttemptMechanicalContext>,
    ) -> Result<ProviderReceipt, ProviderError> {
        let context = WorkerMechanicalContext {
            expected_assignment_revision,
            attempt: active_attempt_context.cloned(),
        };
        let attempt_binding = context
            .attempt
            .as_ref()
            .map(|attempt| DurableAttemptBinding {
                attempt_number: attempt.attempt_number,
                attempt_token: attempt.attempt_token.clone(),
            });
        let digest = request_digest("cancel_assignment", principal, &(request, &attempt_binding))?;
        self.mutate(
            "cancel_assignment",
            &request.action_id,
            digest,
            attempt_binding,
            |state, now| {
                require_scoped_mutation_after_v3(state, &request.assignment_id)?;
                authorize_assignment_coordinator(state, principal, &request.assignment_id)?;
                require_cancel_context(state, &request.assignment_id, &context)?;
                let result = cancel_assignment_internal(
                    state,
                    &request.assignment_id,
                    &request.action_id,
                    now,
                )?;
                schedule_completion_notification(
                    state,
                    &request.assignment_id,
                    context.attempt.as_ref().map(|value| value.attempt_number),
                    &request.action_id,
                    CompletionNotificationKind::TerminalClosure,
                    NotificationTarget::Coordinator,
                    CompletionNotificationDeliveryMode::Soon,
                    now,
                )?;
                Ok(result)
            },
        )
    }

    pub fn authorize_attempt_retry(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &AssignmentActionRequest,
        expected_assignment_revision: u64,
    ) -> Result<ProviderReceipt, ProviderError> {
        let digest = request_digest("authorize_attempt_retry", principal, request)?;
        self.mutate(
            "authorize_attempt_retry",
            &request.action_id,
            digest,
            None,
            |state, now| {
                require_scoped_mutation_after_v3(state, &request.assignment_id)?;
                authorize_assignment_coordinator(state, principal, &request.assignment_id)?;
                let assignment = assignment_mut(state, &request.assignment_id)?;
                require_local_revision(
                    expected_assignment_revision,
                    assignment.local_revision,
                    "assignment_revision_conflict",
                )?;
                if assignment.state != AssignmentState::RetryPending {
                    return Err(ProviderError::IllegalState("retry_requires_retry_pending"));
                }
                if assignment.retry_authorization.is_some() {
                    return Err(ProviderError::Conflict("retry_already_authorized"));
                }
                assignment.local_revision = next_local(assignment.local_revision)?;
                assignment.retry_authorization = Some(RetryAuthorization {
                    project_id: assignment.project_id.clone(),
                    action_id: request.action_id.clone(),
                    authorized_at: now.clone(),
                });
                Ok(ProviderResult::Assignment {
                    assignment: assignment.clone(),
                    send_attempt: None,
                })
            },
        )
    }

    pub fn close_assignment(
        &self,
        principal: &AuthenticatedPrincipal,
        request: &CloseAssignmentRequest,
        expected_assignment_revision: u64,
        attempt_context: &AttemptMechanicalContext,
    ) -> Result<ProviderReceipt, ProviderError> {
        let context = WorkerMechanicalContext {
            expected_assignment_revision,
            attempt: Some(attempt_context.clone()),
        };
        let attempt_binding = Some(DurableAttemptBinding {
            attempt_number: attempt_context.attempt_number,
            attempt_token: attempt_context.attempt_token.clone(),
        });
        let digest = request_digest("close_assignment", principal, &(request, &attempt_binding))?;
        self.mutate(
            "close_assignment",
            &request.action_id,
            digest,
            attempt_binding,
            |state, now| {
                require_scoped_mutation_after_v3(state, &request.assignment_id)?;
                authorize_assignment_coordinator(state, principal, &request.assignment_id)?;
                require_worker_attempt_context(state, &request.assignment_id, &context)?;
                require_assignment_revision(state, &request.assignment_id, &context)?;
                let attempt = state
                    .attempts
                    .get(&request.assignment_id)
                    .and_then(|attempts| attempts.get(&attempt_context.attempt_number))
                    .ok_or(ProviderError::NotFound("attempt"))?
                    .clone();
                let reason = match attempt.phase {
                    AttemptPhase::Failed => ClosureReason::Failed,
                    AttemptPhase::Aborted => ClosureReason::Aborted,
                    _ => {
                        return Err(ProviderError::IllegalState(
                            "close_requires_failed_or_aborted",
                        ))
                    }
                };
                let assignment = close_assignment_internal(
                    state,
                    &request.assignment_id,
                    reason,
                    Some(attempt.attempt_number),
                    &request.action_id,
                    now,
                )?;
                schedule_completion_notification(
                    state,
                    &request.assignment_id,
                    Some(attempt.attempt_number),
                    &request.action_id,
                    CompletionNotificationKind::TerminalClosure,
                    NotificationTarget::Coordinator,
                    CompletionNotificationDeliveryMode::Soon,
                    now,
                )?;
                Ok(ProviderResult::Assignment {
                    assignment,
                    send_attempt: None,
                })
            },
        )
    }

    fn mutate(
        &self,
        operation: &str,
        action_id: &ActionId,
        request_sha256: Sha256,
        attempt_binding: Option<DurableAttemptBinding>,
        apply: impl FnOnce(&mut TaskServiceSnapshot, &Rfc3339) -> Result<ProviderResult, ProviderError>,
    ) -> Result<ProviderReceipt, ProviderError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ProviderError::PersistenceUnavailable)?;
        self.with_store_lock(true, |lock| {
            let mut state = recover_checkpoint_locked(&self.root, lock)?;
            if let Some(receipt) = state.receipts.get(action_id) {
                return if receipt.request_sha256 == request_sha256 {
                    Ok(receipt.clone())
                } else {
                    Err(ProviderError::Conflict("action_id_payload_conflict"))
                };
            }
            let now = now();
            let result = apply(&mut state, &now)?;
            let sequence = state
                .journal_sequence
                .checked_add(1)
                .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                .ok_or(ProviderError::Conflict("journal_sequence_overflow"))?;
            let receipt = ProviderReceipt {
                schema: match state.schema {
                    ProviderStoreSchema::V2 => ProviderReceiptSchema::V2,
                    ProviderStoreSchema::V3 => ProviderReceiptSchema::V3,
                },
                action_id: action_id.clone(),
                request_sha256,
                attempt_binding,
                committed_at: now.clone(),
                journal_sequence: sequence,
                result,
            };
            state.receipts.insert(action_id.clone(), receipt.clone());
            append_and_snapshot(&self.root, state, operation, now)?;
            Ok(receipt)
        })
    }

    fn with_store_lock<T>(
        &self,
        create: bool,
        operation: impl FnOnce(&File) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create);
        set_private_open_options(&mut options);
        let lock = match options.open(path) {
            Ok(lock) => lock,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                return operation(&File::open(self.root.as_ref()).map_err(ProviderError::from)?)
            }
            Err(error) => return Err(error.into()),
        };
        lock.lock_exclusive().map_err(ProviderError::from)?;
        operation(&lock)
    }

    fn capture_journal_for_query(
        &self,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Vec<u8>, ProviderError> {
        let _process = loop {
            if query_stopped(deadline, cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            match self.process_lock.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) => std::thread::sleep(QUERY_LOCK_RETRY),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(ProviderError::PersistenceUnavailable)
                }
            }
        };
        let path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        set_private_open_options(&mut options);
        let lock = options.open(path)?;
        loop {
            if query_stopped(deadline, cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if lock_is_contended(&error) => std::thread::sleep(QUERY_LOCK_RETRY),
                Err(error) => return Err(error.into()),
            }
        }
        let mut journal = match File::open(self.root.join(JOURNAL_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; QUERY_READ_CHUNK_BYTES];
        loop {
            if query_stopped(deadline, cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            let read = journal.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        Ok(bytes)
    }

    fn capture_checkpoint_for_query(
        &self,
        deadline: Instant,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<(Option<SnapshotImage>, JournalTail), ProviderError> {
        let _process = loop {
            if query_stopped(deadline, cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            match self.process_lock.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) => std::thread::sleep(QUERY_LOCK_RETRY),
                Err(TryLockError::Poisoned(_)) => {
                    return Err(ProviderError::PersistenceUnavailable)
                }
            }
        };
        let path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        set_private_open_options(&mut options);
        let lock = options.open(path)?;
        loop {
            if query_stopped(deadline, cancelled) {
                return Err(ProviderError::PersistenceUnavailable);
            }
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if lock_is_contended(&error) => std::thread::sleep(QUERY_LOCK_RETRY),
                Err(error) => return Err(error.into()),
            }
        }

        let snapshot =
            read_optional_file_bounded(&self.root.join(STORE_FILE), deadline, cancelled)?;
        let mut stopped = || query_stopped(deadline, cancelled);
        let tail = read_journal_tail(&self.root, &mut stopped)?;
        Ok((snapshot, tail))
    }
}

fn prepare_worker_context(
    state: &TaskServiceSnapshot,
    session: &CutexSessionId,
    action: &WorkerActionRequest,
    original_binding: Option<&DurableAttemptBinding>,
) -> Result<(WorkerMechanicalContext, Option<DurableAttemptBinding>), ProviderError> {
    let assignment_id = action.assignment_id();
    let current_assignment = assignment(state, assignment_id)?;
    if &current_assignment.assignee_cutex_session != session {
        return Err(ProviderError::Unauthorized);
    }
    validate_worker_action_semantics(action)?;
    let context = worker_mechanical_context(state, assignment_id)?;
    let binding = if action.requires_attempt_binding() {
        let current = context
            .attempt
            .as_ref()
            .ok_or(ProviderError::NotFound("active_attempt"))?;
        let binding = DurableAttemptBinding {
            attempt_number: current.attempt_number,
            attempt_token: current.attempt_token.clone(),
        };
        if original_binding.is_some_and(|original| original != &binding) {
            return Err(ProviderError::Conflict("attempt_handle_conflict"));
        }
        Some(binding)
    } else {
        if original_binding.is_some() {
            return Err(ProviderError::Conflict("attempt_handle_conflict"));
        }
        None
    };
    validate_worker_action_legal(state, action)?;
    let context = if action.requires_attempt_binding() {
        context
    } else {
        WorkerMechanicalContext {
            expected_assignment_revision: context.expected_assignment_revision,
            attempt: None,
        }
    };
    Ok((context, binding))
}

fn validate_worker_action_semantics(action: &WorkerActionRequest) -> Result<(), ProviderError> {
    match action {
        WorkerActionRequest::ReportStatus(value) => {
            validate_text(&value.summary, MAX_EVIDENCE_BYTES, "status_summary")
        }
        WorkerActionRequest::Block(value) => {
            validate_text(&value.summary, MAX_BLOCKER_SUMMARY_BYTES, "blocker_summary")
        }
        WorkerActionRequest::Submit(value) => validate_text(
            &value.result_reference,
            MAX_EVIDENCE_BYTES,
            "result_reference",
        ),
        _ => Ok(()),
    }
}

fn validate_worker_action_legal(
    state: &TaskServiceSnapshot,
    action: &WorkerActionRequest,
) -> Result<(), ProviderError> {
    let assignment = assignment(state, action.assignment_id())?;
    match action {
        WorkerActionRequest::Start(_) => match assignment.state {
            AssignmentState::AwaitingAck => Ok(()),
            AssignmentState::RetryPending if assignment.retry_authorization.is_some() => Ok(()),
            AssignmentState::RetryPending => {
                Err(ProviderError::IllegalState("retry_not_authorized"))
            }
            _ => Err(ProviderError::IllegalState("start_not_allowed")),
        },
        WorkerActionRequest::Decline(_) => {
            if assignment.state == AssignmentState::AwaitingAck {
                Ok(())
            } else {
                Err(ProviderError::IllegalState("decline_requires_awaiting_ack"))
            }
        }
        WorkerActionRequest::ReportStatus(_) => {
            let phase = state
                .active_attempt(action.assignment_id())
                .ok_or(ProviderError::NotFound("active_attempt"))?
                .phase;
            if matches!(phase, AttemptPhase::Running | AttemptPhase::Blocked) {
                Ok(())
            } else {
                Err(ProviderError::IllegalState("status_not_allowed"))
            }
        }
        WorkerActionRequest::Block(_) => {
            require_prepare_phase(state, action, AttemptPhase::Running)
        }
        WorkerActionRequest::Resume(_) => {
            require_prepare_phase(state, action, AttemptPhase::Blocked)
        }
        WorkerActionRequest::Submit(_) => {
            require_prepare_phase(state, action, AttemptPhase::Running).map_err(|error| match error
            {
                ProviderError::IllegalState(_) => {
                    ProviderError::IllegalState("submit_requires_running")
                }
                other => other,
            })
        }
        WorkerActionRequest::AbortAttempt(_) => {
            let phase = state
                .active_attempt(action.assignment_id())
                .ok_or(ProviderError::NotFound("active_attempt"))?
                .phase;
            if matches!(
                phase,
                AttemptPhase::Running | AttemptPhase::Blocked | AttemptPhase::ReviewReady
            ) {
                Ok(())
            } else {
                Err(ProviderError::IllegalState("abort_not_allowed"))
            }
        }
    }
}

fn require_prepare_phase(
    state: &TaskServiceSnapshot,
    action: &WorkerActionRequest,
    expected: AttemptPhase,
) -> Result<(), ProviderError> {
    let phase = state
        .active_attempt(action.assignment_id())
        .ok_or(ProviderError::NotFound("active_attempt"))?
        .phase;
    if phase == expected {
        Ok(())
    } else {
        Err(ProviderError::IllegalState("attempt_phase_conflict"))
    }
}

fn worker_envelope_attempt_binding(
    request: &WorkerProviderActionEnvelope,
) -> Result<Option<DurableAttemptBinding>, ProviderError> {
    if !request.action.requires_attempt_binding() {
        return Ok(None);
    }
    let attempt = request
        .context
        .attempt
        .as_ref()
        .ok_or(ProviderError::InvalidRequest("attempt_context_required"))?;
    Ok(Some(DurableAttemptBinding {
        attempt_number: attempt.attempt_number,
        attempt_token: attempt.attempt_token.clone(),
    }))
}

fn start_attempt(
    state: &mut TaskServiceSnapshot,
    request: &AssignmentActionRequest,
    now: &Rfc3339,
) -> Result<ProviderResult, ProviderError> {
    let (number, token) = {
        let assignment = assignment(state, &request.assignment_id)?;
        match assignment.state {
            AssignmentState::AwaitingAck => (
                AttemptNumber::new(1).map_err(|_| ProviderError::InvalidStore)?,
                deterministic_attempt_token(&request.assignment_id, 1, &request.action_id)?,
            ),
            AssignmentState::RetryPending if assignment.retry_authorization.is_some() => {
                let next = assignment
                    .active_attempt
                    .ok_or(ProviderError::InvalidStore)?
                    .checked_next()
                    .map_err(|_| ProviderError::Conflict("attempt_number_overflow"))?;
                (
                    next,
                    deterministic_attempt_token(
                        &request.assignment_id,
                        next.get(),
                        &request.action_id,
                    )?,
                )
            }
            AssignmentState::RetryPending => {
                return Err(ProviderError::IllegalState("retry_not_authorized"))
            }
            _ => return Err(ProviderError::IllegalState("start_not_allowed")),
        }
    };
    let project_id = assignment(state, &request.assignment_id)?
        .project_id
        .clone();
    let attempt = Attempt {
        project_id,
        assignment_id: request.assignment_id.clone(),
        attempt_number: number,
        attempt_token: token,
        phase: AttemptPhase::Running,
        local_revision: 1,
        started_at: now.clone(),
        updated_at: now.clone(),
        status_receipts: Vec::new(),
        result_receipts: Vec::new(),
        terminal_action_id: None,
    };
    state
        .attempts
        .entry(request.assignment_id.clone())
        .or_default()
        .insert(number, attempt.clone());
    let assignment = assignment_mut(state, &request.assignment_id)?;
    assignment.state = AssignmentState::Active;
    assignment.local_revision = next_local(assignment.local_revision)?;
    assignment
        .acknowledged_at
        .get_or_insert_with(|| now.clone());
    assignment.active_attempt = Some(number);
    assignment.retry_authorization = None;
    Ok(ProviderResult::Attempt(attempt))
}

fn decline_assignment(
    state: &mut TaskServiceSnapshot,
    request: &AssignmentActionRequest,
    now: &Rfc3339,
) -> Result<ProviderResult, ProviderError> {
    if assignment(state, &request.assignment_id)?.state != AssignmentState::AwaitingAck {
        return Err(ProviderError::IllegalState("decline_requires_awaiting_ack"));
    }
    let assignment = close_assignment_internal(
        state,
        &request.assignment_id,
        ClosureReason::Declined,
        None,
        &request.action_id,
        now,
    )?;
    Ok(ProviderResult::Assignment {
        assignment,
        send_attempt: None,
    })
}

fn worker_mechanical_context(
    state: &TaskServiceSnapshot,
    assignment_id: &AssignmentId,
) -> Result<WorkerMechanicalContext, ProviderError> {
    let assignment = assignment(state, assignment_id)?;
    let attempt = match assignment.active_attempt {
        Some(number) => {
            let attempt = state
                .attempts
                .get(assignment_id)
                .and_then(|attempts| attempts.get(&number))
                .ok_or(ProviderError::InvalidStore)?;
            Some(AttemptMechanicalContext {
                attempt_number: attempt.attempt_number,
                attempt_token: attempt.attempt_token.clone(),
                expected_attempt_revision: attempt.local_revision,
            })
        }
        None => None,
    };
    Ok(WorkerMechanicalContext {
        expected_assignment_revision: assignment.local_revision,
        attempt,
    })
}

fn require_local_revision(
    expected: u64,
    actual: u64,
    code: &'static str,
) -> Result<(), ProviderError> {
    if expected == 0 || expected > MAX_JSON_SAFE_INTEGER {
        return Err(ProviderError::InvalidRequest(
            "invalid_expected_local_revision",
        ));
    }
    if expected != actual {
        return Err(ProviderError::Conflict(code));
    }
    Ok(())
}

fn require_assignment_revision(
    state: &TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    context: &WorkerMechanicalContext,
) -> Result<(), ProviderError> {
    require_local_revision(
        context.expected_assignment_revision,
        assignment(state, assignment_id)?.local_revision,
        "assignment_revision_conflict",
    )
}

fn require_no_attempt_context(context: &WorkerMechanicalContext) -> Result<(), ProviderError> {
    if context.attempt.is_some() {
        return Err(ProviderError::InvalidRequest("unexpected_attempt_context"));
    }
    Ok(())
}

fn require_worker_attempt_context(
    state: &TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    context: &WorkerMechanicalContext,
) -> Result<(), ProviderError> {
    let binding = context
        .attempt
        .as_ref()
        .ok_or(ProviderError::InvalidRequest("attempt_context_required"))?;
    let active_number = assignment(state, assignment_id)?
        .active_attempt
        .ok_or(ProviderError::NotFound("active_attempt"))?;
    if active_number != binding.attempt_number {
        return Err(ProviderError::Conflict("attempt_handle_conflict"));
    }
    let attempt = state
        .attempts
        .get(assignment_id)
        .and_then(|attempts| attempts.get(&binding.attempt_number))
        .ok_or(ProviderError::NotFound("attempt"))?;
    if attempt.attempt_token != binding.attempt_token {
        return Err(ProviderError::Conflict("attempt_handle_conflict"));
    }
    require_local_revision(
        binding.expected_attempt_revision,
        attempt.local_revision,
        "attempt_revision_conflict",
    )
}

fn require_cancel_context(
    state: &TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    context: &WorkerMechanicalContext,
) -> Result<(), ProviderError> {
    match (
        assignment(state, assignment_id)?.active_attempt,
        context.attempt.as_ref(),
    ) {
        (Some(_), Some(_)) => require_worker_attempt_context(state, assignment_id, context)?,
        (None, None) => {}
        _ => return Err(ProviderError::Conflict("attempt_handle_conflict")),
    }
    require_assignment_revision(state, assignment_id, context)
}

fn bound_attempt_mut<'a>(
    state: &'a mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    binding: &AttemptMechanicalContext,
) -> Result<&'a mut Attempt, ProviderError> {
    state
        .attempts
        .get_mut(assignment_id)
        .and_then(|attempts| attempts.get_mut(&binding.attempt_number))
        .ok_or(ProviderError::NotFound("attempt"))
}

fn transition_bound_attempt(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    binding: &AttemptMechanicalContext,
    expected: AttemptPhase,
    resulting: AttemptPhase,
    now: &Rfc3339,
) -> Result<ProviderResult, ProviderError> {
    let attempt = bound_attempt_mut(state, assignment_id, binding)?;
    if attempt.phase != expected {
        return Err(ProviderError::IllegalState("attempt_phase_conflict"));
    }
    attempt.phase = resulting;
    attempt.local_revision = next_local(attempt.local_revision)?;
    attempt.updated_at = now.clone();
    Ok(ProviderResult::Attempt(attempt.clone()))
}

#[derive(Clone, Copy)]
enum NotificationTarget {
    CompletionAuthority,
    Coordinator,
}

fn schedule_blocked_notification(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    attempt_number: Option<AttemptNumber>,
    transition_action_id: &ActionId,
    blocker_summary: &str,
    now: &Rfc3339,
) -> Result<NotificationId, ProviderError> {
    validate_text(
        blocker_summary,
        MAX_BLOCKER_SUMMARY_BYTES,
        "blocker_summary",
    )?;
    schedule_completion_notification_with_detail(
        state,
        assignment_id,
        attempt_number,
        transition_action_id,
        CompletionNotificationKind::Blocked,
        NotificationTarget::Coordinator,
        CompletionNotificationDeliveryMode::Soon,
        Some(blocker_summary.trim()),
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn schedule_completion_notification(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    attempt_number: Option<AttemptNumber>,
    transition_action_id: &ActionId,
    kind: CompletionNotificationKind,
    target: NotificationTarget,
    delivery_mode: CompletionNotificationDeliveryMode,
    now: &Rfc3339,
) -> Result<NotificationId, ProviderError> {
    schedule_completion_notification_with_detail(
        state,
        assignment_id,
        attempt_number,
        transition_action_id,
        kind,
        target,
        delivery_mode,
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn schedule_completion_notification_with_detail(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    attempt_number: Option<AttemptNumber>,
    transition_action_id: &ActionId,
    kind: CompletionNotificationKind,
    target: NotificationTarget,
    delivery_mode: CompletionNotificationDeliveryMode,
    blocker_summary: Option<&str>,
    now: &Rfc3339,
) -> Result<NotificationId, ProviderError> {
    let assignment = assignment(state, assignment_id)?.clone();
    let revision = task_revision(state, &assignment.task_id, assignment.task_revision)?.clone();
    let target_seat_id = match target {
        NotificationTarget::CompletionAuthority => revision.completion_policy.authority_seat_id,
        NotificationTarget::Coordinator => state
            .workflows
            .get(&revision.workflow_id)
            .ok_or(ProviderError::InvalidStore)?
            .coordinator_seat_id
            .clone(),
    };
    let correlation = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{kind:?}",
        assignment.assignment_id.as_str(),
        assignment.task_id.as_str(),
        assignment.task_revision.get(),
        attempt_number.map(AttemptNumber::get).unwrap_or(0),
        transition_action_id.as_str(),
    );
    let digest = format!("{:x}", Sha256Hasher::digest(correlation.as_bytes()));
    let notification_id = NotificationId::new(format!("tsn-{digest}"))?;
    let external_message_id = notification_id.as_str().to_string();
    let attempt_label = attempt_number
        .map(|number| number.get().to_string())
        .unwrap_or_else(|| "none".to_string());
    let human_readable_content = match (kind, blocker_summary) {
        (CompletionNotificationKind::Blocked, Some(summary)) => format!(
            "Task Service reports assignment {} is blocked (task {} revision {}, attempt {}).\nBlocker summary: {}\nTransition action identity: {}.\nDirector action required: reply with guidance or resolve the blocker, then resume the assignment.",
            assignment.assignment_id.as_str(),
            assignment.task_id.as_str(),
            assignment.task_revision.get(),
            attempt_label,
            summary,
            transition_action_id.as_str(),
        ),
        (CompletionNotificationKind::Blocked, None) => {
            return Err(ProviderError::InvalidRequest("blocker_summary"));
        }
        (_, Some(_)) => return Err(ProviderError::InvalidRequest("notification_detail")),
        (_, None) => format!(
            "Task Service transition {kind:?} for assignment {} (task {} revision {}, attempt {}).",
            assignment.assignment_id.as_str(),
            assignment.task_id.as_str(),
            assignment.task_revision.get(),
            attempt_label,
        ),
    };
    let notification = CompletionNotification {
        project_id: assignment.project_id,
        notification_id: notification_id.clone(),
        assignment_id: assignment.assignment_id,
        task_id: assignment.task_id,
        task_revision: assignment.task_revision,
        attempt_number,
        transition_action_id: transition_action_id.clone(),
        kind,
        target_seat_id,
        delivery_mode,
        external_message_id,
        human_readable_content,
        local_revision: 1,
        created_at: now.clone(),
        facts: Vec::new(),
    };
    match state.completion_notifications.get(&notification_id) {
        Some(existing) if existing == &notification => {}
        Some(_) => return Err(ProviderError::Conflict("notification_correlation_conflict")),
        None => {
            state
                .completion_notifications
                .insert(notification_id.clone(), notification);
        }
    }
    Ok(notification_id)
}

fn schedule_worker_followup_notification(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    attempt_number: AttemptNumber,
    transition_action_id: &ActionId,
    decision_reference: &str,
    now: &Rfc3339,
) -> Result<NotificationId, ProviderError> {
    let assignment = assignment(state, assignment_id)?.clone();
    let correlation = format!(
        "worker-followup\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        assignment.assignment_id.as_str(),
        assignment.task_id.as_str(),
        assignment.task_revision.get(),
        attempt_number.get(),
        transition_action_id.as_str(),
    );
    let digest = format!("{:x}", Sha256Hasher::digest(correlation.as_bytes()));
    let notification_id = NotificationId::new(format!("tsf-{digest}"))?;
    let notification = WorkerFollowupNotification {
        project_id: assignment.project_id,
        notification_id: notification_id.clone(),
        assignment_id: assignment.assignment_id,
        task_id: assignment.task_id,
        task_revision: assignment.task_revision,
        attempt_number,
        transition_action_id: transition_action_id.clone(),
        target_cutex_session: assignment.assignee_cutex_session,
        decision_reference: decision_reference.to_string(),
        external_message_id: notification_id.as_str().to_string(),
        local_revision: 1,
        created_at: now.clone(),
        facts: Vec::new(),
    };
    match state.worker_followup_notifications.get(&notification_id) {
        Some(existing) if existing == &notification => {}
        Some(_) => {
            return Err(ProviderError::Conflict(
                "worker_followup_correlation_conflict",
            ))
        }
        None => {
            state
                .worker_followup_notifications
                .insert(notification_id.clone(), notification);
        }
    }
    Ok(notification_id)
}

fn cancel_assignment_internal(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    action_id: &ActionId,
    now: &Rfc3339,
) -> Result<ProviderResult, ProviderError> {
    if assignment(state, assignment_id)?.state == AssignmentState::Closed {
        return Err(ProviderError::IllegalState("assignment_closed"));
    }
    let terminal_attempt = assignment(state, assignment_id)?.active_attempt;
    let cancelled = if terminal_attempt.is_some() {
        let attempt = active_attempt_mut(state, assignment_id)?;
        if matches!(
            attempt.phase,
            AttemptPhase::Completed
                | AttemptPhase::Failed
                | AttemptPhase::Cancelled
                | AttemptPhase::Aborted
        ) {
            attempt.clone()
        } else {
            attempt.phase = AttemptPhase::Cancelled;
            attempt.local_revision = next_local(attempt.local_revision)?;
            attempt.updated_at = now.clone();
            attempt.terminal_action_id = Some(action_id.clone());
            attempt.clone()
        }
    } else {
        let assignment = close_assignment_internal(
            state,
            assignment_id,
            ClosureReason::Cancelled,
            None,
            action_id,
            now,
        )?;
        return Ok(ProviderResult::Assignment {
            assignment,
            send_attempt: None,
        });
    };
    close_assignment_internal(
        state,
        assignment_id,
        ClosureReason::Cancelled,
        terminal_attempt,
        action_id,
        now,
    )?;
    Ok(ProviderResult::Attempt(cancelled))
}

fn close_assignment_internal(
    state: &mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
    reason: ClosureReason,
    terminal_attempt: Option<AttemptNumber>,
    action_id: &ActionId,
    now: &Rfc3339,
) -> Result<Assignment, ProviderError> {
    let assignment = assignment_mut(state, assignment_id)?;
    if assignment.state == AssignmentState::Closed {
        return Err(ProviderError::IllegalState("assignment_closed"));
    }
    assignment.state = AssignmentState::Closed;
    assignment.local_revision = next_local(assignment.local_revision)?;
    assignment.retry_authorization = None;
    assignment.closure = Some(AssignmentClosure {
        project_id: assignment.project_id.clone(),
        reason,
        terminal_attempt,
        closed_at: now.clone(),
        closure_action_id: action_id.clone(),
    });
    Ok(assignment.clone())
}

fn assignment<'a>(
    state: &'a TaskServiceSnapshot,
    assignment_id: &AssignmentId,
) -> Result<&'a Assignment, ProviderError> {
    state
        .assignments
        .get(assignment_id)
        .ok_or(ProviderError::NotFound("assignment"))
}

fn assignment_mut<'a>(
    state: &'a mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
) -> Result<&'a mut Assignment, ProviderError> {
    state
        .assignments
        .get_mut(assignment_id)
        .ok_or(ProviderError::NotFound("assignment"))
}

fn require_scoped_mutation_after_v3(
    state: &TaskServiceSnapshot,
    assignment_id: &AssignmentId,
) -> Result<(), ProviderError> {
    let assignment = assignment(state, assignment_id)?;
    if state.schema == ProviderStoreSchema::V3 && assignment.project_id.is_none() {
        return Err(ProviderError::Conflict("legacy_assignment_immutable"));
    }
    Ok(())
}

fn active_attempt_mut<'a>(
    state: &'a mut TaskServiceSnapshot,
    assignment_id: &AssignmentId,
) -> Result<&'a mut Attempt, ProviderError> {
    let number = state
        .assignments
        .get(assignment_id)
        .and_then(|assignment| assignment.active_attempt)
        .ok_or(ProviderError::NotFound("active_attempt"))?;
    state
        .attempts
        .get_mut(assignment_id)
        .and_then(|attempts| attempts.get_mut(&number))
        .ok_or(ProviderError::NotFound("active_attempt"))
}

fn task_revision<'a>(
    state: &'a TaskServiceSnapshot,
    task_id: &TaskId,
    revision: TaskRevision,
) -> Result<&'a TaskRevisionRecord, ProviderError> {
    state
        .task_revisions
        .get(task_id)
        .and_then(|revisions| revisions.get(&revision))
        .ok_or(ProviderError::NotFound("task_revision"))
}

fn authorize_workflow_coordinator(
    state: &TaskServiceSnapshot,
    principal: &AuthenticatedPrincipal,
    workflow_id: &WorkflowId,
) -> Result<(), ProviderError> {
    let (seat, _) = principal.seat()?;
    let workflow = state
        .workflows
        .get(workflow_id)
        .ok_or(ProviderError::NotFound("workflow"))?;
    if &workflow.coordinator_seat_id != seat {
        return Err(ProviderError::Unauthorized);
    }
    if workflow.execution_guard != WorkflowExecutionGuard::Open {
        return Err(ProviderError::IllegalState("workflow_quiescing"));
    }
    Ok(())
}

fn authorize_assignment_coordinator(
    state: &TaskServiceSnapshot,
    principal: &AuthenticatedPrincipal,
    assignment_id: &AssignmentId,
) -> Result<(), ProviderError> {
    let assignment = assignment(state, assignment_id)?;
    let revision = task_revision(state, &assignment.task_id, assignment.task_revision)?;
    authorize_workflow_coordinator(state, principal, &revision.workflow_id)
}

fn authorize_terminal(
    state: &TaskServiceSnapshot,
    principal: &AuthenticatedPrincipal,
    assignment_id: &AssignmentId,
) -> Result<(), ProviderError> {
    let (seat, _) = principal.seat()?;
    let assignment = assignment(state, assignment_id)?;
    let revision = task_revision(state, &assignment.task_id, assignment.task_revision)?;
    if &revision.completion_policy.authority_seat_id != seat {
        return Err(ProviderError::Unauthorized);
    }
    Ok(())
}

fn next_local(current: u64) -> Result<u64, ProviderError> {
    current
        .checked_add(1)
        .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
        .ok_or(ProviderError::Conflict("aggregate_revision_overflow"))
}

fn deterministic_attempt_token(
    assignment: &AssignmentId,
    attempt: u64,
    action: &ActionId,
) -> Result<ProviderAttemptToken, ProviderError> {
    let material = format!(
        "cutex/task-attempt/v2\0{}\0{attempt}\0{}",
        assignment.as_str(),
        action.as_str()
    );
    ProviderAttemptToken::new(hex_sha256(material.as_bytes()).as_str())
}

fn nonempty(value: &str, code: &'static str) -> Result<String, ProviderError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 {
        return Err(ProviderError::InvalidRequest(code));
    }
    Ok(value.to_string())
}

fn validate_text(value: &str, max: usize, code: &'static str) -> Result<(), ProviderError> {
    if value.trim().is_empty() || value.len() > max {
        return Err(ProviderError::InvalidRequest(code));
    }
    Ok(())
}

fn validate_decision_reference(value: &str) -> Result<(), ProviderError> {
    if value.trim().is_empty()
        || value.len() > MAX_DECISION_REFERENCE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::InvalidRequest("invalid_decision_reference"));
    }
    Ok(())
}

fn validate_contract(value: &str, digest: &Sha256) -> Result<(), ProviderError> {
    if value.len() > MAX_CONTRACT_BYTES || &hex_sha256(value.as_bytes()) != digest {
        return Err(ProviderError::InvalidRequest("contract_sha256_mismatch"));
    }
    Ok(())
}

#[derive(Serialize)]
struct DigestMaterial<'a, T> {
    domain: &'static str,
    operation: &'a str,
    principal: DigestPrincipal<'a>,
    request: &'a T,
}

/// Stable pre-omission digest shape for the only optional Worker action field.
/// Provider transport omits absent evidence, while already prepared actions
/// retain the semantic identity calculated when the field serialized as null.
#[derive(Serialize)]
#[serde(tag = "operation", content = "body", rename_all = "snake_case")]
enum WorkerActionDigest<'a> {
    ReportStatus(StatusActionDigest<'a>),
}

#[derive(Serialize)]
struct StatusActionDigest<'a> {
    schema: &'a ProviderActionSchema,
    action_id: &'a ActionId,
    assignment_id: &'a AssignmentId,
    summary: &'a str,
    evidence_sha256: &'a Option<Sha256>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DigestPrincipal<'a> {
    Session {
        cutex_session_id: &'a CutexSessionId,
        current_seat_id: &'a Option<SeatId>,
        seat_epoch: &'a Option<u64>,
    },
    Seat {
        current_seat_id: &'a SeatId,
    },
    TaskServiceSystem,
}

fn request_digest<T: Serialize>(
    operation: &str,
    principal: &AuthenticatedPrincipal,
    request: &T,
) -> Result<Sha256, ProviderError> {
    let principal = match &principal.kind {
        AuthenticatedPrincipalKind::Session {
            cutex_session_id,
            current_seat_id,
            seat_epoch,
        } => match (current_seat_id, seat_epoch) {
            (Some(current_seat_id), Some(_)) => DigestPrincipal::Seat { current_seat_id },
            _ => DigestPrincipal::Session {
                cutex_session_id,
                current_seat_id,
                seat_epoch,
            },
        },
        AuthenticatedPrincipalKind::TaskServiceSystem => DigestPrincipal::TaskServiceSystem,
    };
    let bytes = serde_json::to_vec(&DigestMaterial {
        domain: "cutex/task-service-action/v2",
        operation,
        principal,
        request,
    })
    .map_err(|_| ProviderError::InvalidRequest("unserializable_request"))?;
    Ok(hex_sha256(&bytes))
}

fn worker_request_digest(
    operation: &str,
    principal: &AuthenticatedPrincipal,
    action: &WorkerActionRequest,
    attempt_binding: Option<&DurableAttemptBinding>,
) -> Result<Sha256, ProviderError> {
    if action.requires_attempt_binding() != attempt_binding.is_some() {
        return Err(ProviderError::Conflict("attempt_handle_conflict"));
    }
    match action {
        WorkerActionRequest::ReportStatus(value) => request_digest(
            operation,
            principal,
            &(
                WorkerActionDigest::ReportStatus(StatusActionDigest {
                    schema: &value.schema,
                    action_id: &value.action_id,
                    assignment_id: &value.assignment_id,
                    summary: &value.summary,
                    evidence_sha256: &value.evidence_sha256,
                }),
                attempt_binding,
            ),
        ),
        _ => request_digest(operation, principal, &(action, attempt_binding)),
    }
}

fn now() -> Rfc3339 {
    Rfc3339::new(chrono::Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true))
        .expect("UTC timestamp")
}

fn hex_sha256(bytes: &[u8]) -> Sha256 {
    let digest = Sha256Hasher::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("string write");
    }
    Sha256::new(encoded).expect("sha256")
}

#[derive(Serialize)]
struct JournalHashMaterial<'a> {
    schema: ProviderStoreSchema,
    sequence: u64,
    previous_event_sha256: &'a Sha256,
    operation: &'a str,
    occurred_at: &'a Rfc3339,
    resulting_state: &'a TaskServiceSnapshot,
}

/// Exact TaskServiceSnapshot serialization used before the completion outbox
/// field existed. Historical event hashes commit to this field set and order.
#[derive(Serialize)]
struct LegacyTaskServiceSnapshotHashMaterial<'a> {
    schema: ProviderStoreSchema,
    journal_sequence: u64,
    journal_sha256: &'a Sha256,
    task_revisions: &'a BTreeMap<TaskId, BTreeMap<TaskRevision, TaskRevisionRecord>>,
    assignments: &'a BTreeMap<AssignmentId, Assignment>,
    attempts: &'a BTreeMap<AssignmentId, BTreeMap<AttemptNumber, Attempt>>,
    send_attempts: &'a BTreeMap<SendAttemptId, SendAttempt>,
    workflows: &'a BTreeMap<WorkflowId, Workflow>,
    receipts: &'a BTreeMap<ActionId, ProviderReceipt>,
    prepared_worker_actions: &'a BTreeMap<ActionId, PreparedWorkerAction>,
}

impl<'a> From<&'a TaskServiceSnapshot> for LegacyTaskServiceSnapshotHashMaterial<'a> {
    fn from(state: &'a TaskServiceSnapshot) -> Self {
        Self {
            schema: state.schema,
            journal_sequence: state.journal_sequence,
            journal_sha256: &state.journal_sha256,
            task_revisions: &state.task_revisions,
            assignments: &state.assignments,
            attempts: &state.attempts,
            send_attempts: &state.send_attempts,
            workflows: &state.workflows,
            receipts: &state.receipts,
            prepared_worker_actions: &state.prepared_worker_actions,
        }
    }
}

#[derive(Serialize)]
struct LegacyJournalHashMaterial<'a> {
    schema: ProviderStoreSchema,
    sequence: u64,
    previous_event_sha256: &'a Sha256,
    operation: &'a str,
    occurred_at: &'a Rfc3339,
    resulting_state: LegacyTaskServiceSnapshotHashMaterial<'a>,
}

fn journal_hash(
    sequence: u64,
    previous_event_sha256: &Sha256,
    operation: &str,
    occurred_at: &Rfc3339,
    resulting_state: &TaskServiceSnapshot,
) -> Result<Sha256, ProviderError> {
    let bytes = serde_json::to_vec(&JournalHashMaterial {
        schema: resulting_state.schema,
        sequence,
        previous_event_sha256,
        operation,
        occurred_at,
        resulting_state,
    })
    .map_err(|_| ProviderError::InvalidStore)?;
    Ok(hex_sha256(&bytes))
}

fn legacy_journal_hash(
    sequence: u64,
    previous_event_sha256: &Sha256,
    operation: &str,
    occurred_at: &Rfc3339,
    resulting_state: &TaskServiceSnapshot,
) -> Result<Sha256, ProviderError> {
    if !resulting_state.completion_notifications.is_empty() {
        return Err(ProviderError::InvalidStore);
    }
    let bytes = serde_json::to_vec(&LegacyJournalHashMaterial {
        schema: ProviderStoreSchema::V2,
        sequence,
        previous_event_sha256,
        operation,
        occurred_at,
        resulting_state: resulting_state.into(),
    })
    .map_err(|_| ProviderError::InvalidStore)?;
    Ok(hex_sha256(&bytes))
}

fn append_and_snapshot(
    root: &Path,
    mut state: TaskServiceSnapshot,
    operation: &str,
    occurred_at: Rfc3339,
) -> Result<(), ProviderError> {
    validate_state(&state)?;
    let sequence = state
        .journal_sequence
        .checked_add(1)
        .ok_or(ProviderError::InvalidStore)?;
    let previous = state.journal_sha256.clone();
    state.journal_sequence = sequence;
    let event_sha256 = journal_hash(sequence, &previous, operation, &occurred_at, &state)?;
    let record = PersistedJournalRecord {
        schema: state.schema,
        sequence,
        previous_event_sha256: previous,
        event_sha256: event_sha256.clone(),
        operation: operation.to_string(),
        occurred_at,
        resulting_state: state.clone(),
        completion_notifications_was_present: true,
    };
    let mut line = serde_json::to_vec(&record).map_err(|_| ProviderError::InvalidStore)?;
    line.push(b'\n');
    let mut options = OpenOptions::new();
    options.create(true).append(true).write(true);
    set_private_open_options(&mut options);
    let mut journal = options.open(root.join(JOURNAL_FILE))?;
    journal.write_all(&line)?;
    journal.sync_all()?;
    state.journal_sha256 = event_sha256;
    atomic_snapshot(root, &state)
}

/// Recovers the authenticated current-state checkpoint used by normal
/// mutations. A missing, stale, partially written, or inconsistent checkpoint
/// falls back to the original complete journal recovery, which also repairs
/// the snapshot and any incomplete journal tail.
fn recover_checkpoint_locked(
    root: &Path,
    lock: &File,
) -> Result<TaskServiceSnapshot, ProviderError> {
    let snapshot = match load_snapshot_image(root) {
        Ok(snapshot) => snapshot,
        Err(ProviderError::InvalidStore) => None,
        Err(error) => return Err(error),
    };
    if let Some(snapshot) = snapshot {
        let mut never_stopped = || false;
        let tail = read_journal_tail(root, &mut never_stopped)?;
        if let Ok(state) = serde_json::from_slice::<TaskServiceSnapshot>(&snapshot.bytes) {
            if let Ok(state) = recover_checkpoint(state, snapshot.modified, &tail) {
                return Ok(state);
            }
        }
    }
    recover_locked(root, lock)
}

/// Verifies that an atomic snapshot is exactly the state authenticated by the
/// journal's latest complete record. This deliberately authenticates only the
/// current checkpoint; [`recover_locked`] remains the full-chain audit and
/// compatibility recovery boundary.
fn recover_checkpoint(
    snapshot: TaskServiceSnapshot,
    snapshot_modified: Option<SystemTime>,
    tail: &JournalTail,
) -> Result<TaskServiceSnapshot, ProviderError> {
    validate_state(&snapshot)?;
    if tail.file_len != tail.complete_len {
        return Err(ProviderError::InvalidStore);
    }
    if snapshot.journal_sequence == 0 {
        return if tail.complete_record.is_none() && tail.file_len == 0 {
            Ok(snapshot)
        } else {
            Err(ProviderError::InvalidStore)
        };
    }
    if !matches!(
        (tail.modified, snapshot_modified),
        (Some(journal), Some(snapshot)) if journal <= snapshot
    ) {
        return Err(ProviderError::InvalidStore);
    }

    let line = tail
        .complete_record
        .as_deref()
        .ok_or(ProviderError::InvalidStore)?;
    let record = parse_journal_record(line)?;
    let recovered_hash = if record.completion_notifications_was_present {
        journal_hash(
            record.sequence,
            &record.previous_event_sha256,
            &record.operation,
            &record.occurred_at,
            &record.resulting_state,
        )?
    } else {
        legacy_journal_hash(
            record.sequence,
            &record.previous_event_sha256,
            &record.operation,
            &record.occurred_at,
            &record.resulting_state,
        )?
    };
    if record.schema != snapshot.schema
        || record.schema != record.resulting_state.schema
        || record.sequence != snapshot.journal_sequence
        || record.resulting_state.journal_sequence != record.sequence
        || record.resulting_state.journal_sha256 != record.previous_event_sha256
        || record.event_sha256 != snapshot.journal_sha256
        || recovered_hash != record.event_sha256
    {
        return Err(ProviderError::InvalidStore);
    }
    let mut resulting_state = record.resulting_state;
    resulting_state.journal_sha256 = record.event_sha256;
    if resulting_state != snapshot {
        return Err(ProviderError::InvalidStore);
    }
    Ok(snapshot)
}

fn recover_locked(root: &Path, _lock: &File) -> Result<TaskServiceSnapshot, ProviderError> {
    let (records, complete_len) = read_journal(root)?;
    let mut state = TaskServiceSnapshot::empty();
    let mut previous = Sha256::new(ZERO_SHA256).expect("zero hash");
    let mut expected_sequence = 1u64;
    for record in records {
        let recovered_hash = if record.completion_notifications_was_present {
            journal_hash(
                record.sequence,
                &record.previous_event_sha256,
                &record.operation,
                &record.occurred_at,
                &record.resulting_state,
            )?
        } else {
            legacy_journal_hash(
                record.sequence,
                &record.previous_event_sha256,
                &record.operation,
                &record.occurred_at,
                &record.resulting_state,
            )?
        };
        if record.schema != record.resulting_state.schema
            || record.sequence != expected_sequence
            || record.previous_event_sha256 != previous
            || record.resulting_state.journal_sequence != record.sequence
            || record.resulting_state.journal_sha256 != record.previous_event_sha256
            || recovered_hash != record.event_sha256
        {
            return Err(ProviderError::InvalidStore);
        }
        state = record.resulting_state;
        state.journal_sha256 = record.event_sha256.clone();
        previous = record.event_sha256;
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(ProviderError::InvalidStore)?;
    }
    validate_state(&state)?;
    truncate_partial_tail(root, complete_len)?;
    let snapshot = load_snapshot(root)?;
    if snapshot.as_ref() != Some(&state) {
        atomic_snapshot(root, &state)?;
    }
    Ok(state)
}

fn query_stopped(deadline: Instant, cancelled: &mut dyn FnMut() -> bool) -> bool {
    Instant::now() >= deadline || cancelled()
}

fn lock_is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn parse_journal_record(line: &[u8]) -> Result<PersistedJournalRecord, ProviderError> {
    let mut record = serde_json::from_slice::<PersistedJournalRecord>(line)
        .map_err(|_| ProviderError::InvalidStore)?;
    let encoded = serde_json::from_slice::<serde_json::Value>(line)
        .map_err(|_| ProviderError::InvalidStore)?;
    record.completion_notifications_was_present = encoded
        .get("resulting_state")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|state| state.contains_key("completion_notifications"));
    Ok(record)
}

fn read_optional_file_bounded(
    path: &Path,
    deadline: Instant,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<Option<SnapshotImage>, ProviderError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; QUERY_READ_CHUNK_BYTES];
    loop {
        if query_stopped(deadline, cancelled) {
            return Err(ProviderError::PersistenceUnavailable);
        }
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(Some(SnapshotImage {
                bytes,
                modified: file.metadata()?.modified().ok(),
            }));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// Reads only the last complete JSONL record. The returned lengths preserve
/// evidence of a crash-interrupted partial tail so callers cannot silently use
/// or append after it.
fn read_journal_tail(
    root: &Path,
    stopped: &mut dyn FnMut() -> bool,
) -> Result<JournalTail, ProviderError> {
    let mut file = match File::open(root.join(JOURNAL_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(JournalTail {
                complete_record: None,
                complete_len: 0,
                file_len: 0,
                modified: None,
            })
        }
        Err(error) => return Err(error.into()),
    };
    let file_len = file.seek(SeekFrom::End(0))?;
    if file_len == 0 {
        return Ok(JournalTail {
            complete_record: None,
            complete_len: 0,
            file_len: 0,
            modified: file.metadata()?.modified().ok(),
        });
    }

    let mut cursor = file_len;
    let mut chunks = Vec::new();
    let mut newline_count = 0_usize;
    while cursor > 0 && newline_count < 2 {
        if stopped() {
            return Err(ProviderError::PersistenceUnavailable);
        }
        let read_len = usize::try_from(cursor.min(QUERY_READ_CHUNK_BYTES as u64))
            .map_err(|_| ProviderError::InvalidStore)?;
        cursor -= read_len as u64;
        file.seek(SeekFrom::Start(cursor))?;
        let mut chunk = vec![0_u8; read_len];
        file.read_exact(&mut chunk)?;
        newline_count += chunk.iter().filter(|byte| **byte == b'\n').count();
        chunks.push(chunk);
    }
    chunks.reverse();
    let mut bytes = Vec::new();
    for chunk in chunks {
        bytes.extend_from_slice(&chunk);
    }
    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return Ok(JournalTail {
            complete_record: None,
            complete_len: 0,
            file_len,
            modified: file.metadata()?.modified().ok(),
        });
    };
    let complete_len = cursor
        .checked_add(last_newline as u64)
        .and_then(|value| value.checked_add(1))
        .ok_or(ProviderError::InvalidStore)?;
    let start = bytes[..last_newline]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    Ok(JournalTail {
        complete_record: Some(bytes[start..=last_newline].to_vec()),
        complete_len,
        file_len,
        modified: file.metadata()?.modified().ok(),
    })
}

fn read_journal_bytes(
    bytes: &[u8],
    deadline: Instant,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<Vec<PersistedJournalRecord>, ProviderError> {
    let mut records = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if query_stopped(deadline, cancelled) {
            return Err(ProviderError::PersistenceUnavailable);
        }
        if !line.ends_with(b"\n") {
            break;
        }
        records.push(parse_journal_record(line)?);
    }
    Ok(records)
}

fn recover_records(
    records: Vec<PersistedJournalRecord>,
    deadline: Instant,
    cancelled: &mut dyn FnMut() -> bool,
) -> Result<TaskServiceSnapshot, ProviderError> {
    let mut state = TaskServiceSnapshot::empty();
    let mut previous = Sha256::new(ZERO_SHA256).expect("zero hash");
    let mut expected_sequence = 1_u64;
    for record in records {
        if query_stopped(deadline, cancelled) {
            return Err(ProviderError::PersistenceUnavailable);
        }
        let recovered_hash = if record.completion_notifications_was_present {
            journal_hash(
                record.sequence,
                &record.previous_event_sha256,
                &record.operation,
                &record.occurred_at,
                &record.resulting_state,
            )?
        } else {
            legacy_journal_hash(
                record.sequence,
                &record.previous_event_sha256,
                &record.operation,
                &record.occurred_at,
                &record.resulting_state,
            )?
        };
        if record.schema != record.resulting_state.schema
            || record.sequence != expected_sequence
            || record.previous_event_sha256 != previous
            || record.resulting_state.journal_sequence != record.sequence
            || record.resulting_state.journal_sha256 != record.previous_event_sha256
            || recovered_hash != record.event_sha256
        {
            return Err(ProviderError::InvalidStore);
        }
        state = record.resulting_state;
        state.journal_sha256 = record.event_sha256.clone();
        previous = record.event_sha256;
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(ProviderError::InvalidStore)?;
    }
    validate_state(&state)?;
    Ok(state)
}

fn read_journal(root: &Path) -> Result<(Vec<PersistedJournalRecord>, u64), ProviderError> {
    let path = root.join(JOURNAL_FILE);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(error) => return Err(error.into()),
    };
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut complete_len = 0u64;
    loop {
        let mut line = Vec::new();
        let count = reader.read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        if !line.ends_with(b"\n") {
            break;
        }
        // Parse the strict typed record first so duplicate or unknown fields
        // remain fail-closed; Value is used only to retain one field-presence
        // bit that serde(default) necessarily erases.
        let record = parse_journal_record(&line)?;
        complete_len = complete_len
            .checked_add(count as u64)
            .ok_or(ProviderError::InvalidStore)?;
        records.push(record);
    }
    Ok((records, complete_len))
}

fn truncate_partial_tail(root: &Path, complete_len: u64) -> Result<(), ProviderError> {
    let path = root.join(JOURNAL_FILE);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    set_private_open_options(&mut options);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if file.seek(SeekFrom::End(0))? > complete_len {
        file.set_len(complete_len)?;
        file.sync_all()?;
    }
    Ok(())
}

fn load_snapshot(root: &Path) -> Result<Option<TaskServiceSnapshot>, ProviderError> {
    let Some(image) = load_snapshot_image(root)? else {
        return Ok(None);
    };
    let snapshot = serde_json::from_slice(&image.bytes).map_err(|_| ProviderError::InvalidStore)?;
    Ok(Some(snapshot))
}

fn load_snapshot_image(root: &Path) -> Result<Option<SnapshotImage>, ProviderError> {
    let mut bytes = Vec::new();
    let mut file = match File::open(root.join(STORE_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    file.read_to_end(&mut bytes)?;
    Ok(Some(SnapshotImage {
        bytes,
        modified: file.metadata()?.modified().ok(),
    }))
}

fn atomic_snapshot(root: &Path, state: &TaskServiceSnapshot) -> Result<(), ProviderError> {
    let temp = root.join(format!(".{STORE_FILE}.{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    set_private_open_options(&mut options);
    let mut file = options.open(&temp)?;
    let bytes = serde_json::to_vec(state).map_err(|_| ProviderError::InvalidStore)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        fs::rename(&temp, root.join(STORE_FILE))?;
        File::open(root)?.sync_all()?;
    }
    #[cfg(windows)]
    {
        let (directory, identity) = crate::platform::private_fs::open_validated_directory(root)
            .map_err(|error| ProviderError::Io(error.io_kind()))?;
        let source = temp
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(ProviderError::InvalidStore)?;
        crate::platform::private_fs::replace_child(root, identity, source, STORE_FILE)
            .map_err(|error| ProviderError::Io(error.io_kind()))?;
        crate::platform::private_fs::sync_directory(&directory)
            .map_err(|error| ProviderError::Io(error.io_kind()))?;
    }
    Ok(())
}

fn validate_state(state: &TaskServiceSnapshot) -> Result<(), ProviderError> {
    if state.journal_sequence > MAX_JSON_SAFE_INTEGER
        || (state.journal_sequence == 0 && state.journal_sha256.as_str() != ZERO_SHA256)
        || state.prepared_worker_actions.len() > MAX_PREPARED_WORKER_ACTIONS
    {
        return Err(ProviderError::InvalidStore);
    }
    for (task_id, revisions) in &state.task_revisions {
        if revisions.is_empty() {
            return Err(ProviderError::InvalidStore);
        }
        for (number, revision) in revisions {
            if task_id != &revision.task_id
                || number != &revision.task_revision
                || !state.workflows.contains_key(&revision.workflow_id)
                || state.workflows[&revision.workflow_id].project_id != revision.project_id
                || (state.schema == ProviderStoreSchema::V2 && revision.project_id.is_some())
            {
                return Err(ProviderError::InvalidStore);
            }
            validate_contract(&revision.opaque_contract, &revision.contract_sha256)
                .map_err(|_| ProviderError::InvalidStore)?;
        }
    }
    for (id, assignment) in &state.assignments {
        if id != &assignment.assignment_id
            || assignment.local_revision == 0
            || task_revision(state, &assignment.task_id, assignment.task_revision).is_err()
            || (assignment.state == AssignmentState::Closed) != assignment.closure.is_some()
            || task_revision(state, &assignment.task_id, assignment.task_revision)
                .is_ok_and(|revision| revision.project_id != assignment.project_id)
            || assignment
                .closure
                .as_ref()
                .is_some_and(|closure| closure.project_id != assignment.project_id)
        {
            return Err(ProviderError::InvalidStore);
        }
        if let Some(number) = assignment.active_attempt {
            if state
                .attempts
                .get(id)
                .and_then(|attempts| attempts.get(&number))
                .is_none()
            {
                return Err(ProviderError::InvalidStore);
            }
        }
    }
    for (assignment_id, attempts) in &state.attempts {
        for (number, attempt) in attempts {
            if assignment_id != &attempt.assignment_id
                || number != &attempt.attempt_number
                || attempt.local_revision == 0
                || state
                    .assignments
                    .get(assignment_id)
                    .map(|value| &value.project_id)
                    != Some(&attempt.project_id)
                || attempt
                    .status_receipts
                    .iter()
                    .any(|receipt| receipt.project_id != attempt.project_id)
                || attempt
                    .result_receipts
                    .iter()
                    .any(|receipt| receipt.project_id != attempt.project_id)
            {
                return Err(ProviderError::InvalidStore);
            }
        }
    }
    for (id, send) in &state.send_attempts {
        let assignment = state
            .assignments
            .get(&send.assignment_id)
            .ok_or(ProviderError::InvalidStore)?;
        if id != &send.send_attempt_id
            || send.project_id != assignment.project_id
            || send.local_revision == 0
            || send.retry_ordinal == 0
        {
            return Err(ProviderError::InvalidStore);
        }
    }
    for (id, notification) in &state.completion_notifications {
        let assignment = state
            .assignments
            .get(&notification.assignment_id)
            .ok_or(ProviderError::InvalidStore)?;
        if id != &notification.notification_id
            || notification.local_revision == 0
            || assignment.task_id != notification.task_id
            || assignment.task_revision != notification.task_revision
            || assignment.project_id != notification.project_id
            || notification.external_message_id != notification.notification_id.as_str()
            || notification.human_readable_content.trim().is_empty()
            || notification.human_readable_content.len() > 4096
        {
            return Err(ProviderError::InvalidStore);
        }
    }
    for (id, notification) in &state.worker_followup_notifications {
        let assignment = state
            .assignments
            .get(&notification.assignment_id)
            .ok_or(ProviderError::InvalidStore)?;
        let attempt = state
            .attempts
            .get(&notification.assignment_id)
            .and_then(|attempts| attempts.get(&notification.attempt_number))
            .ok_or(ProviderError::InvalidStore)?;
        if id != &notification.notification_id
            || notification.local_revision == 0
            || assignment.task_id != notification.task_id
            || assignment.task_revision != notification.task_revision
            || assignment.project_id != notification.project_id
            || assignment.assignee_cutex_session != notification.target_cutex_session
            || attempt.assignment_id != notification.assignment_id
            || notification.external_message_id != notification.notification_id.as_str()
            || validate_decision_reference(&notification.decision_reference).is_err()
        {
            return Err(ProviderError::InvalidStore);
        }
    }
    for (id, workflow) in &state.workflows {
        if id != &workflow.workflow_id
            || workflow.local_revision == 0
            || (state.schema == ProviderStoreSchema::V2 && workflow.project_id.is_some())
        {
            return Err(ProviderError::InvalidStore);
        }
    }
    for (id, receipt) in &state.receipts {
        if id != &receipt.action_id || receipt.journal_sequence > state.journal_sequence + 1 {
            return Err(ProviderError::InvalidStore);
        }
    }
    for (id, prepared) in &state.prepared_worker_actions {
        if id != &prepared.action_id
            || state.receipts.contains_key(id)
            || prepared.context.expected_assignment_revision == 0
        {
            return Err(ProviderError::InvalidStore);
        }
        let assignment =
            assignment(state, &prepared.assignment_id).map_err(|_| ProviderError::InvalidStore)?;
        let context_binding =
            prepared
                .context
                .attempt
                .as_ref()
                .map(|attempt| DurableAttemptBinding {
                    attempt_number: attempt.attempt_number,
                    attempt_token: attempt.attempt_token.clone(),
                });
        if assignment.assignee_cutex_session != prepared.authenticated_cutex_session
            || context_binding != prepared.attempt_binding
            || prepared
                .context
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.expected_attempt_revision == 0)
        {
            return Err(ProviderError::InvalidStore);
        }
        if let Some(binding) = &prepared.attempt_binding {
            let attempt = state
                .attempts
                .get(&prepared.assignment_id)
                .and_then(|attempts| attempts.get(&binding.attempt_number))
                .ok_or(ProviderError::InvalidStore)?;
            if attempt.attempt_token != binding.attempt_token {
                return Err(ProviderError::InvalidStore);
            }
        }
    }
    Ok(())
}

fn prepare_private_root(root: &Path) -> Result<(), ProviderError> {
    if root.exists() {
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ProviderError::InvalidRequest("root_not_direct_directory"));
        }
    } else {
        fs::create_dir_all(root)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::private_fs::secure_tree(root)
        .map_err(|error| ProviderError::Io(error.io_kind()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_open_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

#[cfg(not(unix))]
fn set_private_open_options(_options: &mut OpenOptions) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn id<T>(value: &str, make: impl FnOnce(String) -> Result<T, ProviderError>) -> T {
        make(value.to_string()).unwrap()
    }

    fn root(name: &str) -> PathBuf {
        let root = std::env::var_os("CUTEX_TASK_SERVICE_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap()
                    .join("target")
                    .join("task-service-provider-tests")
            })
            .join(format!("{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn sha(text: &str) -> Sha256 {
        hex_sha256(text.as_bytes())
    }

    fn action(value: &str) -> ActionId {
        id(value, ActionId::new)
    }

    fn assignment_id() -> AssignmentId {
        id("assignment-1", AssignmentId::new)
    }

    fn session(value: &str) -> CutexSessionId {
        CutexSessionId::new(value).unwrap()
    }

    struct Fixture {
        provider: TaskServiceProvider,
        coordinator: AuthenticatedPrincipal,
        worker: AuthenticatedPrincipal,
        authority: AuthenticatedPrincipal,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let provider = TaskServiceProvider::open(root(name)).unwrap();
            provider.recover().unwrap();
            Self {
                provider,
                coordinator: AuthenticatedPrincipal::seated_session(
                    session("cutex-director-r10"),
                    id("director", SeatId::new),
                    10,
                )
                .unwrap(),
                worker: AuthenticatedPrincipal::session(session("cutex-worker-r1")),
                authority: AuthenticatedPrincipal::seated_session(
                    session("cutex-release-r4"),
                    id("release", SeatId::new),
                    4,
                )
                .unwrap(),
            }
        }

        fn provision(&self) {
            self.provider
                .create_revision(
                    &self.coordinator,
                    &CreateRevisionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("create"),
                        workflow_id: id("workflow-1", WorkflowId::new),
                        task_id: TaskId::new("CUTEX-test").unwrap(),
                        task_revision: TaskRevision::new(1).unwrap(),
                        contract_sha256: sha("contract"),
                        opaque_contract: "contract".into(),
                        completion_policy: CompletionPolicy {
                            kind: CompletionPolicyKind::ReleaseReview,
                            authority_seat_id: id("release", SeatId::new),
                        },
                    },
                    None,
                )
                .unwrap();
            self.provider
                .assign_and_dispatch(
                    &self.coordinator,
                    &AssignAndDispatchRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("assign"),
                        assignment_id: assignment_id(),
                        task_id: TaskId::new("CUTEX-test").unwrap(),
                        task_revision: TaskRevision::new(1).unwrap(),
                        assignee_cutex_session: session("cutex-worker-r1"),
                        send_attempt_id: id("send-1", SendAttemptId::new),
                        external_message_id: "message-1".into(),
                    },
                    1,
                    "assignment content",
                )
                .unwrap();
        }

        fn worker_action(&self, operation: WorkerActionRequest) -> ProviderReceipt {
            let envelope = self.worker_envelope(operation);
            self.provider
                .execute_worker_action(&self.worker, &envelope)
                .unwrap()
        }

        fn worker_context(&self) -> WorkerMechanicalContext {
            self.provider
                .worker_context(
                    &self.worker,
                    &WorkerContextRequest {
                        schema: WorkerContextRequestSchema::V2,
                        assignment_id: assignment_id(),
                    },
                )
                .unwrap()
                .context
        }

        fn worker_envelope(&self, action: WorkerActionRequest) -> WorkerProviderActionEnvelope {
            match self
                .provider
                .prepare_worker_action(
                    &self.worker,
                    &WorkerPrepareRequest {
                        schema: WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap()
            {
                WorkerPrepareOutcome::Prepared(envelope) => envelope,
                WorkerPrepareOutcome::Committed(_) => panic!("expected a new prepared action"),
            }
        }

        fn terminal_action(&self, command: TerminalAuthorityRequest) -> ProviderReceipt {
            self.provider
                .execute_terminal_action(
                    &self.authority,
                    &TerminalActionEnvelope {
                        schema: TerminalRequestSchema::V2,
                        command,
                        context: self.worker_context(),
                    },
                )
                .unwrap()
        }

        fn start(&self, action_id: &str) -> ProviderReceipt {
            self.worker_action(WorkerActionRequest::Start(AssignmentActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action(action_id),
                assignment_id: assignment_id(),
            }))
        }

        fn submit(&self, action_id: &str, result: &str) -> ProviderReceipt {
            self.worker_action(WorkerActionRequest::Submit(SubmitActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action(action_id),
                assignment_id: assignment_id(),
                result_sha256: sha(result),
                result_reference: result.into(),
            }))
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_provider_full_assignment_lifecycle_smoke() {
        let fixture = Fixture::new("windows-full-lifecycle");
        fixture.provision();
        fixture.start("windows-start");
        fixture.worker_action(WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("windows-status"),
            assignment_id: assignment_id(),
            summary: "windows isolated progress".into(),
            evidence_sha256: None,
        }));
        fixture.submit("windows-submit", "windows-result");
        fixture.terminal_action(TerminalAuthorityRequest::AcceptResult(
            TerminalActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("windows-accept"),
                assignment_id: assignment_id(),
                decision_reference: Some("windows isolated acceptance".into()),
            },
        ));
        let state = fixture.provider.query().unwrap();
        assert_eq!(
            state.assignments[&assignment_id()].state,
            AssignmentState::Closed
        );
    }

    #[test]
    fn project_v3_lineage_replays_conflicts_and_survives_restart() {
        let fixture = Fixture::new("project-v3-lineage");
        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let create = CreateProjectRevisionRequest {
            schema: ProviderActionSchema::V3,
            action_id: action("project-create"),
            project_id: project.clone(),
            workflow_id: id("project-workflow", WorkflowId::new),
            task_id: TaskId::new("CUTEX-project-task").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            contract_sha256: sha("project contract"),
            opaque_contract: "project contract".into(),
            completion_policy: CompletionPolicy {
                kind: CompletionPolicyKind::ReleaseReview,
                authority_seat_id: id("release", SeatId::new),
            },
        };
        let first = fixture
            .provider
            .create_project_revision(&fixture.coordinator, &create, None)
            .unwrap();
        assert_eq!(first.schema, ProviderReceiptSchema::V3);
        assert_eq!(
            fixture
                .provider
                .create_project_revision(&fixture.coordinator, &create, None)
                .unwrap(),
            first
        );
        let mut changed_project = create.clone();
        changed_project.project_id =
            crate::agent_management::ProjectId::new("project-beta").unwrap();
        assert!(matches!(
            fixture
                .provider
                .create_project_revision(&fixture.coordinator, &changed_project, None),
            Err(ProviderError::Conflict("action_id_payload_conflict"))
        ));
        let sequence_before_conflict = fixture.provider.query().unwrap().journal_sequence;
        let mut cross_project = create.clone();
        cross_project.action_id = action("project-create-cross-project");
        cross_project.project_id = crate::agent_management::ProjectId::new("project-beta").unwrap();
        cross_project.workflow_id = id("project-workflow-beta", WorkflowId::new);
        cross_project.task_revision = TaskRevision::new(2).unwrap();
        assert!(matches!(
            fixture
                .provider
                .create_project_revision(&fixture.coordinator, &cross_project, None),
            Err(ProviderError::Conflict("task_project_conflict"))
        ));
        assert_eq!(
            fixture.provider.query().unwrap().journal_sequence,
            sequence_before_conflict
        );

        let assignment = AssignmentId::new("project-assignment").unwrap();
        fixture
            .provider
            .assign_project_and_dispatch(
                &fixture.coordinator,
                &AssignProjectAndDispatchRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: action("project-assign"),
                    project_id: project.clone(),
                    assignment_id: assignment.clone(),
                    task_id: create.task_id.clone(),
                    task_revision: create.task_revision,
                    assignee_cutex_session: session("cutex-worker-r1"),
                    send_attempt_id: id("project-send", SendAttemptId::new),
                    external_message_id: "project-message".into(),
                },
                1,
                "project assignment",
            )
            .unwrap();
        let start = WorkerActionRequest::Start(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("project-start"),
            assignment_id: assignment.clone(),
        });
        let prepared = fixture
            .provider
            .prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: start,
                },
            )
            .unwrap();
        let WorkerPrepareOutcome::Prepared(envelope) = prepared else {
            panic!("prepared")
        };
        fixture
            .provider
            .execute_worker_action(&fixture.worker, &envelope)
            .unwrap();
        let submit = WorkerActionRequest::Submit(SubmitActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("project-submit"),
            assignment_id: assignment.clone(),
            result_sha256: sha("project result"),
            result_reference: "evidence/project-result.md".into(),
        });
        let WorkerPrepareOutcome::Prepared(envelope) = fixture
            .provider
            .prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: submit,
                },
            )
            .unwrap()
        else {
            panic!("prepared")
        };
        fixture
            .provider
            .execute_worker_action(&fixture.worker, &envelope)
            .unwrap();
        let context = fixture
            .provider
            .worker_context(
                &fixture.worker,
                &WorkerContextRequest {
                    schema: WorkerContextRequestSchema::V2,
                    assignment_id: assignment.clone(),
                },
            )
            .unwrap()
            .context;
        fixture
            .provider
            .execute_terminal_action(
                &fixture.authority,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("project-accept"),
                        assignment_id: assignment.clone(),
                        decision_reference: Some("accepted-project-result".into()),
                    }),
                    context,
                },
            )
            .unwrap();

        assert!(matches!(
            fixture.provider.create_revision(
                &fixture.coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-after-v3"),
                    workflow_id: id("legacy-after-v3-workflow", WorkflowId::new),
                    task_id: TaskId::new("CUTEX-legacy-after-v3").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("legacy-after-v3"),
                    opaque_contract: "legacy-after-v3".into(),
                    completion_policy: create.completion_policy.clone(),
                },
                None,
            ),
            Err(ProviderError::InvalidRequest(
                "legacy_writes_disabled_after_v3_activation"
            ))
        ));

        let reopened = TaskServiceProvider::open((*fixture.provider.root).clone()).unwrap();
        let snapshot = reopened.query().unwrap();
        assert_eq!(snapshot.schema, ProviderStoreSchema::V3);
        assert_eq!(
            snapshot.task_revisions[&create.task_id][&create.task_revision]
                .project_id
                .as_ref(),
            Some(&project)
        );
        assert_eq!(
            snapshot.assignments[&assignment].project_id.as_ref(),
            Some(&project)
        );
        assert_eq!(
            snapshot.attempts[&assignment][&AttemptNumber::new(1).unwrap()]
                .project_id
                .as_ref(),
            Some(&project)
        );
        let attempt = &snapshot.attempts[&assignment][&AttemptNumber::new(1).unwrap()];
        assert_eq!(attempt.phase, AttemptPhase::Completed);
        assert!(attempt
            .result_receipts
            .iter()
            .all(|receipt| receipt.project_id.as_ref() == Some(&project)));
        assert!(snapshot
            .completion_notifications
            .values()
            .filter(|notification| notification.assignment_id == assignment)
            .all(|notification| notification.project_id.as_ref() == Some(&project)));
        assert_eq!(
            snapshot.assignments[&assignment]
                .closure
                .as_ref()
                .and_then(|closure| closure.project_id.as_ref()),
            Some(&project)
        );
        assert_eq!(
            snapshot
                .send_attempts
                .values()
                .next()
                .unwrap()
                .project_id
                .as_ref(),
            Some(&project)
        );
        assert!(snapshot
            .task_revisions
            .values()
            .flat_map(|revisions| revisions.values())
            .all(|task| task.project_id.as_ref() == Some(&project)));
    }

    #[test]
    fn v3_activation_makes_every_legacy_assignment_mutation_no_write() {
        let fixture = Fixture::new("v3-legacy-immutable");
        fixture.provision();
        let start = WorkerActionRequest::Start(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("legacy-prepared-before-v3"),
            assignment_id: assignment_id(),
        });
        let prepared = fixture.worker_envelope(start.clone());
        fixture
            .provider
            .create_project_revision(
                &fixture.coordinator,
                &CreateProjectRevisionRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: action("activate-v3"),
                    project_id: crate::agent_management::ProjectId::new("project-alpha").unwrap(),
                    workflow_id: id("project-workflow", WorkflowId::new),
                    task_id: TaskId::new("CUTEX-project-after-legacy").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("project contract"),
                    opaque_contract: "project contract".into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: id("release", SeatId::new),
                    },
                },
                None,
            )
            .unwrap();
        let before = fixture.provider.query().unwrap();

        assert!(matches!(
            fixture.provider.prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: start,
                },
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &prepared),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-communication-after-v3"),
                    send_attempt_id: id("send-1", SendAttemptId::new),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some("legacy-message".into()),
                },
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.retry_delivery(
                &fixture.coordinator,
                &RetryDeliveryRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-delivery-retry-after-v3"),
                    assignment_id: assignment_id(),
                    send_attempt_id: id("legacy-send-retry-after-v3", SendAttemptId::new),
                    external_message_id: "legacy-message-retry-after-v3".into(),
                },
                1,
                "legacy retry",
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.cancel_assignment(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-cancel-after-v3"),
                    assignment_id: assignment_id(),
                },
                1,
                None,
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.authorize_attempt_retry(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-authorize-retry-after-v3"),
                    assignment_id: assignment_id(),
                },
                1,
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.execute_terminal_action(
                &fixture.authority,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: TerminalAuthorityRequest::Cancel(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("legacy-terminal-after-v3"),
                        assignment_id: assignment_id(),
                        decision_reference: None,
                    }),
                    context: prepared.context.clone(),
                },
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert!(matches!(
            fixture.provider.close_assignment(
                &fixture.coordinator,
                &CloseAssignmentRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("legacy-close-after-v3"),
                    assignment_id: assignment_id(),
                },
                1,
                &AttemptMechanicalContext {
                    attempt_number: AttemptNumber::new(1).unwrap(),
                    attempt_token: ProviderAttemptToken::new("legacy-attempt-token").unwrap(),
                    expected_attempt_revision: 1,
                },
            ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));

        let after = fixture.provider.query().unwrap();
        assert_eq!(after.journal_sequence, before.journal_sequence);
        assert_eq!(
            after.assignments[&assignment_id()],
            before.assignments[&assignment_id()]
        );
        assert_eq!(
            after.prepared_worker_actions,
            before.prepared_worker_actions
        );
        assert!(after.attempts.get(&assignment_id()).is_none());

        let notification_fixture = Fixture::new("v3-legacy-notification-immutable");
        notification_fixture.provision();
        notification_fixture.worker_action(WorkerActionRequest::Decline(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("legacy-decline-before-v3"),
            assignment_id: assignment_id(),
        }));
        notification_fixture
            .provider
            .create_project_revision(
                &notification_fixture.coordinator,
                &CreateProjectRevisionRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: action("activate-v3-after-notification"),
                    project_id: crate::agent_management::ProjectId::new("project-beta").unwrap(),
                    workflow_id: id("project-beta-workflow", WorkflowId::new),
                    task_id: TaskId::new("CUTEX-project-after-notification").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("project beta contract"),
                    opaque_contract: "project beta contract".into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: id("release", SeatId::new),
                    },
                },
                None,
            )
            .unwrap();
        let before_notification = notification_fixture.provider.query().unwrap();
        let notification = before_notification
            .completion_notifications
            .values()
            .next()
            .unwrap();
        assert!(matches!(
            notification_fixture
                .provider
                .record_completion_notification_fact(
                    &AuthenticatedPrincipal::task_service_system(),
                    &CompletionNotificationFactRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("legacy-notification-fact-after-v3"),
                        notification_id: notification.notification_id.clone(),
                        expected_notification_revision: notification.local_revision,
                        kind: CompletionNotificationFactKind::Queued,
                        reference: Some("legacy-notification".into()),
                    },
                ),
            Err(ProviderError::Conflict("legacy_assignment_immutable"))
        ));
        assert_eq!(
            notification_fixture.provider.query().unwrap(),
            before_notification
        );
    }

    #[test]
    fn query_lock_acquisition_is_bounded_and_releases_process_ownership() {
        let fixture = Fixture::new("bounded-query-lock");
        let external_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(fixture.provider.root.join(LOCK_FILE))
            .unwrap();
        external_lock.lock_exclusive().unwrap();
        let started = Instant::now();
        assert_eq!(
            fixture.provider.query(),
            Err(ProviderError::PersistenceUnavailable)
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        FileExt::unlock(&external_lock).unwrap();
        fixture.provision();
        assert_eq!(fixture.provider.query().unwrap().journal_sequence, 2);
    }

    #[test]
    fn checkpoint_query_reads_only_authenticated_tail_and_mutation_stays_bounded() {
        let fixture = Fixture::new("checkpoint-bounded");
        fixture.provision();
        let mut state = fixture.provider.query().unwrap();
        let task = state
            .task_revisions
            .get_mut(&TaskId::new("CUTEX-test").unwrap())
            .unwrap()
            .get_mut(&TaskRevision::new(1).unwrap())
            .unwrap();
        task.opaque_contract = "x".repeat(64 * 1024);
        task.contract_sha256 = hex_sha256(task.opaque_contract.as_bytes());
        append_and_snapshot(&fixture.provider.root, state, "checkpoint_seed", now()).unwrap();
        for _ in 0..40 {
            let state = load_snapshot(&fixture.provider.root).unwrap().unwrap();
            append_and_snapshot(&fixture.provider.root, state, "checkpoint_growth", now()).unwrap();
        }

        let journal_len = fs::metadata(fixture.provider.root.join(JOURNAL_FILE))
            .unwrap()
            .len();
        let deadline = Instant::now() + MAX_QUERY_DURATION;
        let (snapshot, tail) = fixture
            .provider
            .capture_checkpoint_for_query(deadline, &mut || false)
            .unwrap();
        assert!(snapshot.is_some());
        let tail_len = tail.complete_record.as_ref().unwrap().len() as u64;
        assert!(journal_len > tail_len * 20, "{journal_len} <= {tail_len}");

        let query_started = Instant::now();
        let before = fixture.provider.query().unwrap();
        assert!(query_started.elapsed() < MAX_QUERY_DURATION);
        let mutation_started = Instant::now();
        fixture.start("checkpoint-start");
        assert!(mutation_started.elapsed() < MAX_QUERY_DURATION);
        assert_eq!(
            fixture.provider.query().unwrap().journal_sequence,
            before.journal_sequence + 2
        );
    }

    #[test]
    fn missing_checkpoint_falls_back_to_old_store_recovery_before_mutation() {
        let fixture = Fixture::new("checkpoint-missing");
        fixture.provision();
        let expected = fixture.provider.query().unwrap();
        fs::remove_file(fixture.provider.root.join(STORE_FILE)).unwrap();

        assert_eq!(fixture.provider.query().unwrap(), expected);
        assert!(!fixture.provider.root.join(STORE_FILE).exists());
        fixture.start("checkpoint-recovered-start");
        assert!(fixture.provider.root.join(STORE_FILE).exists());
        assert_eq!(
            fixture.provider.query().unwrap().journal_sequence,
            expected.journal_sequence + 2
        );
    }

    #[test]
    fn modified_historical_journal_invalidates_checkpoint_and_detects_tamper() {
        let fixture = Fixture::new("checkpoint-tamper");
        fixture.provision();
        std::thread::sleep(Duration::from_millis(2));
        let journal_path = fixture.provider.root.join(JOURNAL_FILE);
        let journal = fs::read_to_string(&journal_path).unwrap();
        let tampered = journal.replacen(
            "\"operation\":\"create_revision\"",
            "\"operation\":\"tamper_revision\"",
            1,
        );
        assert_ne!(tampered, journal);
        fs::write(&journal_path, tampered).unwrap();

        assert_eq!(fixture.provider.query(), Err(ProviderError::InvalidStore));
        assert_eq!(fixture.provider.recover(), Err(ProviderError::InvalidStore));
    }

    fn write_legacy_store(root: &Path, mut state: TaskServiceSnapshot) -> TaskServiceSnapshot {
        state.journal_sequence = 1;
        state.journal_sha256 = Sha256::new(ZERO_SHA256).unwrap();
        state.completion_notifications.clear();
        let occurred_at = Rfc3339::new("2026-08-28T00:00:00Z").unwrap();
        let event_sha256 = legacy_journal_hash(
            1,
            &state.journal_sha256,
            "legacy_fixture",
            &occurred_at,
            &state,
        )
        .unwrap();
        let record = PersistedJournalRecord {
            schema: ProviderStoreSchema::V2,
            sequence: 1,
            previous_event_sha256: state.journal_sha256.clone(),
            event_sha256: event_sha256.clone(),
            operation: "legacy_fixture".to_string(),
            occurred_at,
            resulting_state: state.clone(),
            completion_notifications_was_present: false,
        };
        let mut encoded_record = serde_json::to_value(record).unwrap();
        encoded_record["resulting_state"]
            .as_object_mut()
            .unwrap()
            .remove("completion_notifications");
        let mut journal = serde_json::to_vec(&encoded_record).unwrap();
        journal.push(b'\n');
        fs::write(root.join(JOURNAL_FILE), journal).unwrap();

        state.journal_sha256 = event_sha256;
        let mut encoded_snapshot = serde_json::to_value(&state).unwrap();
        encoded_snapshot
            .as_object_mut()
            .unwrap()
            .remove("completion_notifications");
        fs::write(
            root.join(STORE_FILE),
            serde_json::to_vec(&encoded_snapshot).unwrap(),
        )
        .unwrap();
        state
    }

    #[test]
    fn legacy_store_recovers_mixed_chain_and_replays_after_restart() {
        let source = Fixture::new("legacy-store-source");
        source.provision();
        let source_root = source.provider.root.as_ref().clone();
        let legacy_root = root("legacy-store-mixed-chain");
        let expected = write_legacy_store(&legacy_root, source.provider.query().unwrap());
        let snapshot_before = fs::read(legacy_root.join(STORE_FILE)).unwrap();
        let journal_before = fs::read(legacy_root.join(JOURNAL_FILE)).unwrap();

        let provider = TaskServiceProvider::open(&legacy_root).unwrap();
        assert_eq!(provider.recover().unwrap(), expected);
        assert_eq!(
            fs::read(legacy_root.join(STORE_FILE)).unwrap(),
            snapshot_before
        );
        assert_eq!(
            fs::read(legacy_root.join(JOURNAL_FILE)).unwrap(),
            journal_before
        );

        let send = expected.send_attempts.values().next().unwrap();
        let request = CommunicationEventRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("legacy-mixed-bus-queued"),
            send_attempt_id: send.send_attempt_id.clone(),
            expected_send_attempt_revision: send.local_revision,
            kind: CommunicationEventKind::BusQueued,
            receipt_reference: Some("legacy-mixed-message".to_string()),
        };
        let first = provider
            .record_communication_event(&AuthenticatedPrincipal::task_service_system(), &request)
            .unwrap();
        let lines = fs::read_to_string(legacy_root.join(JOURNAL_FILE)).unwrap();
        let records = lines
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert!(records[0]["resulting_state"]
            .get("completion_notifications")
            .is_none());
        assert!(records[1]["resulting_state"]
            .get("completion_notifications")
            .is_some());

        drop(provider);
        let reopened = TaskServiceProvider::open(&legacy_root).unwrap();
        let recovered = reopened.recover().unwrap();
        let replay = reopened
            .record_communication_event(&AuthenticatedPrincipal::task_service_system(), &request)
            .unwrap();
        assert_eq!(replay, first);
        assert_eq!(reopened.query().unwrap(), recovered);

        drop(source);
        fs::remove_dir_all(source_root).unwrap();
        fs::remove_dir_all(legacy_root).unwrap();
    }

    #[test]
    fn legacy_hash_compatibility_rejects_tampered_complete_record_without_writes() {
        let source = Fixture::new("legacy-corrupt-source");
        source.provision();
        let source_root = source.provider.root.as_ref().clone();
        let corrupt_root = root("legacy-corrupt-store");
        write_legacy_store(&corrupt_root, source.provider.query().unwrap());

        let journal_path = corrupt_root.join(JOURNAL_FILE);
        let mut encoded: serde_json::Value =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        encoded["operation"] = serde_json::json!("tampered_operation");
        let mut tampered = serde_json::to_vec(&encoded).unwrap();
        tampered.push(b'\n');
        fs::write(&journal_path, &tampered).unwrap();
        let snapshot_before = fs::read(corrupt_root.join(STORE_FILE)).unwrap();

        let provider = TaskServiceProvider::open(&corrupt_root).unwrap();
        assert_eq!(provider.recover(), Err(ProviderError::InvalidStore));
        assert_eq!(fs::read(&journal_path).unwrap(), tampered);
        assert_eq!(
            fs::read(corrupt_root.join(STORE_FILE)).unwrap(),
            snapshot_before
        );

        drop(source);
        fs::remove_dir_all(source_root).unwrap();
        fs::remove_dir_all(corrupt_root).unwrap();
    }

    #[test]
    fn start_replay_is_exact_and_runtime_occurrence_is_not_an_input() {
        let fixture = Fixture::new("start-replay");
        fixture.provision();
        let semantic_request = WorkerActionRequest::Start(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("start"),
            assignment_id: assignment_id(),
        });
        let request = fixture.worker_envelope(semantic_request.clone());
        let first = fixture
            .provider
            .execute_worker_action(&fixture.worker, &request)
            .unwrap();
        let restarted_runtime = AuthenticatedPrincipal::session(session("cutex-worker-r1"));
        let replay = fixture
            .provider
            .execute_worker_action(&restarted_runtime, &request)
            .unwrap();
        assert_eq!(first, replay);
        let state = fixture.provider.query().unwrap();
        assert_eq!(state.attempts[&assignment_id()].len(), 1);
        let json = serde_json::to_string(&semantic_request).unwrap();
        for forbidden in ["runtime", "generation", "attempt_token", "revision"] {
            assert!(
                !json.contains(forbidden),
                "forbidden field {forbidden}: {json}"
            );
        }
    }

    #[test]
    fn submit_atomically_persists_after_turn_outbox_and_replay_is_exact() {
        let fixture = Fixture::new("completion-submit-outbox");
        fixture.provision();
        fixture.start("start");
        let request = fixture.worker_envelope(WorkerActionRequest::Submit(SubmitActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("submit"),
            assignment_id: assignment_id(),
            result_sha256: sha("result"),
            result_reference: "result".into(),
        }));
        let first = fixture
            .provider
            .execute_worker_action(&fixture.worker, &request)
            .unwrap();
        let replay = fixture
            .provider
            .execute_worker_action(&fixture.worker, &request)
            .unwrap();
        assert_eq!(first, replay);
        let state = fixture.provider.query().unwrap();
        assert_eq!(
            state.active_attempt(&assignment_id()).unwrap().phase,
            AttemptPhase::ReviewReady
        );
        assert_eq!(state.completion_notifications.len(), 1);
        let notification = state.completion_notifications.values().next().unwrap();
        assert_eq!(notification.kind, CompletionNotificationKind::ReviewReady);
        assert_eq!(
            notification.delivery_mode,
            CompletionNotificationDeliveryMode::AfterTurn
        );
        assert_eq!(notification.target_seat_id.as_str(), "release");
        assert!(notification.facts.is_empty());

        let reopened = TaskServiceProvider::open(fixture.provider.root.as_ref()).unwrap();
        assert_eq!(
            reopened.query().unwrap().completion_notifications,
            state.completion_notifications
        );
    }

    #[test]
    fn progress_is_wake_free_while_block_and_abort_schedule_urgent_coordinator_wakes() {
        let fixture = Fixture::new("completion-urgent-outbox");
        fixture.provision();
        fixture.start("start");
        fixture.worker_action(WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("status"),
            assignment_id: assignment_id(),
            summary: "ordinary progress".into(),
            evidence_sha256: None,
        }));
        assert!(fixture
            .provider
            .query()
            .unwrap()
            .completion_notifications
            .is_empty());
        fixture.worker_action(WorkerActionRequest::Block(BlockActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("block"),
            assignment_id: assignment_id(),
            summary: "normal Windows service lifecycle cannot stop the managed runtime".into(),
        }));
        fixture.worker_action(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("abort"),
            assignment_id: assignment_id(),
        }));
        let state = fixture.provider.query().unwrap();
        assert_eq!(state.completion_notifications.len(), 2);
        for notification in state.completion_notifications.values() {
            assert_eq!(
                notification.delivery_mode,
                CompletionNotificationDeliveryMode::Soon
            );
            assert_eq!(notification.target_seat_id.as_str(), "director");
        }
        let blocked = state
            .completion_notifications
            .values()
            .find(|notification| notification.kind == CompletionNotificationKind::Blocked)
            .expect("blocked notification");
        assert_eq!(blocked.transition_action_id.as_str(), "block");
        assert!(blocked.human_readable_content.contains(
            "Blocker summary: normal Windows service lifecycle cannot stop the managed runtime"
        ));
        assert!(blocked
            .human_readable_content
            .contains("Transition action identity: block."));
        assert!(blocked
            .human_readable_content
            .contains("Director action required:"));
        assert!(blocked.human_readable_content.len() <= 4096);
    }

    #[test]
    fn decline_and_retries_exhausted_schedule_urgent_coordinator_wakes() {
        let declined = Fixture::new("completion-declined-outbox");
        declined.provision();
        declined.worker_action(WorkerActionRequest::Decline(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("decline"),
            assignment_id: assignment_id(),
        }));
        let state = declined.provider.query().unwrap();
        let notification = state.completion_notifications.values().next().unwrap();
        assert_eq!(notification.kind, CompletionNotificationKind::Declined);
        assert_eq!(
            notification.delivery_mode,
            CompletionNotificationDeliveryMode::Soon
        );

        let exhausted = Fixture::new("completion-retries-exhausted-outbox");
        exhausted.provision();
        exhausted
            .provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("retries-exhausted"),
                    send_attempt_id: id("send-1", SendAttemptId::new),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::RetriesExhausted,
                    receipt_reference: Some("retry-policy".into()),
                },
            )
            .unwrap();
        let state = exhausted.provider.query().unwrap();
        let notification = state.completion_notifications.values().next().unwrap();
        assert_eq!(
            notification.kind,
            CompletionNotificationKind::RetriesExhausted
        );
        assert_eq!(notification.target_seat_id.as_str(), "director");
        assert_eq!(
            notification.delivery_mode,
            CompletionNotificationDeliveryMode::Soon
        );
    }

    #[test]
    fn nonempty_snapshot_query_response_roundtrips_numeric_map_keys() {
        let fixture = Fixture::new("snapshot-query-roundtrip");
        fixture.provision();
        fixture.start("start");
        let response = crate::agent_bus::model::TaskServiceQueryResponse {
            schema: crate::agent_bus::model::TaskServiceQueryResponseSchema::V2,
            outcome: crate::agent_bus::model::TaskServiceQueryOutcome::Snapshot(
                fixture.provider.query().expect("query nonempty snapshot"),
            ),
        };

        let wire = serde_json::to_vec(&response).expect("serialize query response");
        let decoded: crate::agent_bus::model::TaskServiceQueryResponse =
            serde_json::from_slice(&wire).expect("decode query response from wire JSON");
        assert_eq!(decoded, response);

        let value = serde_json::to_value(&response).expect("serialize query response value");
        let value_wire = serde_json::to_vec(&value).expect("serialize query response JSON value");
        let decoded: crate::agent_bus::model::TaskServiceQueryResponse =
            serde_json::from_slice(&value_wire).expect("decode query response JSON value wire");
        assert_eq!(decoded, response);
        let decoded: crate::agent_bus::model::TaskServiceQueryResponse =
            serde_json::from_value(value.clone()).expect("decode query response JSON value");
        assert_eq!(decoded, response);

        for invalid_key in ["0", "01", "+1", " 1", "1.0", "9007199254740992"] {
            let mut invalid = value.clone();
            let revisions = invalid["outcome"]["body"]["task_revisions"]["CUTEX-test"]
                .as_object_mut()
                .expect("task revision map");
            let record = revisions.remove("1").expect("revision one");
            revisions.insert(invalid_key.to_string(), record);
            assert!(
                serde_json::from_value::<crate::agent_bus::model::TaskServiceQueryResponse>(
                    invalid.clone()
                )
                .is_err(),
                "accepted noncanonical task revision key: {invalid_key}"
            );
            let wire = serde_json::to_vec(&invalid).expect("serialize invalid query response");
            assert!(
                serde_json::from_slice::<crate::agent_bus::model::TaskServiceQueryResponse>(&wire)
                    .is_err(),
                "accepted noncanonical task revision wire key: {invalid_key}"
            );
        }

        let mut invalid_attempt = value;
        let attempts = invalid_attempt["outcome"]["body"]["attempts"]["assignment-1"]
            .as_object_mut()
            .expect("attempt map");
        let attempt = attempts.remove("1").expect("attempt one");
        attempts.insert("01".to_string(), attempt);
        assert!(
            serde_json::from_value::<crate::agent_bus::model::TaskServiceQueryResponse>(
                invalid_attempt
            )
            .is_err()
        );
    }

    #[test]
    fn communication_events_never_advance_semantic_state_or_stale_worker_action() {
        let fixture = Fixture::new("communication-separate");
        fixture.provision();
        let before = fixture.provider.query().unwrap().assignments[&assignment_id()].clone();
        fixture
            .provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("bus-queued"),
                    send_attempt_id: id("send-1", SendAttemptId::new),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some("bus-receipt".into()),
                },
            )
            .unwrap();
        let after = fixture.provider.query().unwrap().assignments[&assignment_id()].clone();
        assert_eq!(before, after);
        fixture.start("start");
        assert_eq!(
            fixture.provider.query().unwrap().assignments[&assignment_id()].state,
            AssignmentState::Active
        );
    }

    #[test]
    fn changed_payload_with_same_action_id_conflicts() {
        let fixture = Fixture::new("payload-conflict");
        fixture.provision();
        fixture.start("start");
        let first =
            fixture.worker_envelope(WorkerActionRequest::ReportStatus(StatusActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("status"),
                assignment_id: assignment_id(),
                summary: "one".into(),
                evidence_sha256: None,
            }));
        fixture
            .provider
            .execute_worker_action(&fixture.worker, &first)
            .unwrap();
        let mut changed = first;
        let WorkerActionRequest::ReportStatus(changed_action) = &mut changed.action else {
            unreachable!()
        };
        changed_action.summary = "two".into();
        let conflict = fixture
            .provider
            .execute_worker_action(&fixture.worker, &changed);
        assert_eq!(
            conflict,
            Err(ProviderError::Conflict("action_id_payload_conflict"))
        );
    }

    #[test]
    fn review_changes_and_accept_complete_one_attempt() {
        let fixture = Fixture::new("review-loop");
        fixture.provision();
        fixture.start("start");
        fixture.submit("submit-1", "result-1");
        let changes = TerminalAuthorityRequest::RequestChanges(TerminalActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("changes"),
            assignment_id: assignment_id(),
            decision_reference: Some("fix".into()),
        });
        let first = fixture.terminal_action(changes.clone());
        let replay = fixture.terminal_action(changes);
        assert_eq!(first, replay);
        let changed_replay = fixture.provider.execute_terminal_action(
            &fixture.authority,
            &TerminalActionEnvelope {
                schema: TerminalRequestSchema::V2,
                command: TerminalAuthorityRequest::RequestChanges(TerminalActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("changes"),
                    assignment_id: assignment_id(),
                    decision_reference: Some("different request".into()),
                }),
                context: fixture.worker_context(),
            },
        );
        assert_eq!(
            changed_replay,
            Err(ProviderError::Conflict("action_id_payload_conflict"))
        );
        let resumed = fixture.provider.query().unwrap();
        assert_eq!(resumed.worker_followup_notifications.len(), 1);
        let followup = resumed
            .worker_followup_notifications
            .values()
            .next()
            .unwrap();
        assert_eq!(followup.assignment_id, assignment_id());
        assert_eq!(followup.task_id.as_str(), "CUTEX-test");
        assert_eq!(followup.task_revision.get(), 1);
        assert_eq!(followup.attempt_number.get(), 1);
        assert_eq!(followup.target_cutex_session, session("cutex-worker-r1"));
        assert_eq!(followup.decision_reference, "fix");
        assert!(followup.facts.is_empty());
        let reopened = TaskServiceProvider::open(fixture.provider.root.as_ref()).unwrap();
        assert_eq!(
            reopened.query().unwrap().worker_followup_notifications,
            resumed.worker_followup_notifications
        );
        fixture.submit("submit-2", "result-2");
        fixture.terminal_action(TerminalAuthorityRequest::AcceptResult(
            TerminalActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("accept"),
                assignment_id: assignment_id(),
                decision_reference: None,
            },
        ));
        let state = fixture.provider.query().unwrap();
        assert_eq!(state.attempts[&assignment_id()].len(), 1);
        assert_eq!(
            state.active_attempt(&assignment_id()).unwrap().phase,
            AttemptPhase::Completed
        );
        assert_eq!(
            state.assignments[&assignment_id()].state,
            AssignmentState::Closed
        );
        assert_eq!(
            state.assignments[&assignment_id()]
                .closure
                .as_ref()
                .unwrap()
                .reason,
            ClosureReason::Completed
        );
        let closure = state
            .completion_notifications
            .values()
            .find(|notification| notification.kind == CompletionNotificationKind::TerminalClosure)
            .expect("terminal closure notifies coordinator");
        assert_eq!(closure.target_seat_id.as_str(), "director");
        assert_eq!(
            closure.delivery_mode,
            CompletionNotificationDeliveryMode::AfterTurn
        );
        assert_eq!(state.worker_followup_notifications.len(), 1);
    }

    #[test]
    fn request_changes_requires_bounded_control_free_decision_reference() {
        for (name, decision_reference) in [
            ("missing", None),
            ("blank", Some("   ".to_string())),
            ("control", Some("fix\nthen retry".to_string())),
            (
                "oversized",
                Some("x".repeat(MAX_DECISION_REFERENCE_BYTES + 1)),
            ),
        ] {
            let fixture = Fixture::new(&format!("request-changes-invalid-{name}"));
            fixture.provision();
            fixture.start("start");
            fixture.submit("submit", "result");
            let result = fixture.provider.execute_terminal_action(
                &fixture.authority,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: TerminalAuthorityRequest::RequestChanges(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("changes"),
                        assignment_id: assignment_id(),
                        decision_reference,
                    }),
                    context: fixture.worker_context(),
                },
            );
            assert!(matches!(result, Err(ProviderError::InvalidRequest(_))));
            let state = fixture.provider.query().unwrap();
            assert_eq!(
                state.active_attempt(&assignment_id()).unwrap().phase,
                AttemptPhase::ReviewReady
            );
            assert!(state.worker_followup_notifications.is_empty());
        }
    }

    #[test]
    fn abort_and_fail_require_explicit_close_or_retry() {
        let fixture = Fixture::new("abort-retry");
        fixture.provision();
        fixture.start("start");
        fixture.worker_action(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("abort"),
            assignment_id: assignment_id(),
        }));
        assert_eq!(
            fixture.provider.query().unwrap().assignments[&assignment_id()].state,
            AssignmentState::RetryPending
        );
        let unauthorized_start = fixture.provider.prepare_worker_action(
            &fixture.worker,
            &WorkerPrepareRequest {
                schema: WorkerPrepareRequestSchema::V2,
                action: WorkerActionRequest::Start(AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("start-without-retry"),
                    assignment_id: assignment_id(),
                }),
            },
        );
        assert_eq!(
            unauthorized_start,
            Err(ProviderError::IllegalState("retry_not_authorized"))
        );
        let close_context = fixture.worker_context();
        fixture
            .provider
            .close_assignment(
                &fixture.coordinator,
                &CloseAssignmentRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("close-aborted"),
                    assignment_id: assignment_id(),
                },
                close_context.expected_assignment_revision,
                close_context.attempt.as_ref().unwrap(),
            )
            .unwrap();
        assert_eq!(
            fixture.provider.query().unwrap().assignments[&assignment_id()]
                .closure
                .as_ref()
                .unwrap()
                .reason,
            ClosureReason::Aborted
        );
    }

    #[test]
    fn explicit_retry_creates_attempt_two_and_recovery_preserves_every_aggregate() {
        let fixture = Fixture::new("retry-recovery");
        fixture.provision();
        fixture.start("start-1");
        fixture.worker_action(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("abort"),
            assignment_id: assignment_id(),
        }));
        let retry_context = fixture.worker_context();
        fixture
            .provider
            .authorize_attempt_retry(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("authorize-retry"),
                    assignment_id: assignment_id(),
                },
                retry_context.expected_assignment_revision,
            )
            .unwrap();
        fixture.start("start-2");
        let before = fixture.provider.query().unwrap();
        let reopened = TaskServiceProvider::open(fixture.provider.root.as_ref().clone()).unwrap();
        let after = reopened.recover().unwrap();
        assert_eq!(before, after);
        assert_eq!(after.attempts[&assignment_id()].len(), 2);
        let watch = reopened.watch(0, 1000).unwrap();
        assert_eq!(watch.len() as u64, after.journal_sequence);
        assert!(watch
            .windows(2)
            .all(|pair| pair[0].sequence + 1 == pair[1].sequence));
    }

    #[test]
    fn decline_cancel_and_forged_authority_fail_closed() {
        let decline = Fixture::new("decline");
        decline.provision();
        decline.worker_action(WorkerActionRequest::Decline(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("decline"),
            assignment_id: assignment_id(),
        }));
        assert_eq!(
            decline.provider.query().unwrap().assignments[&assignment_id()]
                .closure
                .as_ref()
                .unwrap()
                .reason,
            ClosureReason::Declined
        );

        let cancel = Fixture::new("cancel");
        cancel.provision();
        let cancel_context = cancel.worker_context();
        cancel
            .provider
            .cancel_assignment(
                &cancel.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("cancel"),
                    assignment_id: assignment_id(),
                },
                cancel_context.expected_assignment_revision,
                None,
            )
            .unwrap();
        assert_eq!(
            cancel.provider.query().unwrap().assignments[&assignment_id()]
                .closure
                .as_ref()
                .unwrap()
                .reason,
            ClosureReason::Cancelled
        );

        let forged = AuthenticatedPrincipal::session(session("cutex-release-r4"));
        assert_eq!(
            cancel.provider.record_communication_event(
                &forged,
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("forged-system"),
                    send_attempt_id: id("send-1", SendAttemptId::new),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: None,
                }
            ),
            Err(ProviderError::Unauthorized)
        );

        let forged_worker_fixture = Fixture::new("forged-worker");
        forged_worker_fixture.provision();
        let wrong_worker = AuthenticatedPrincipal::session(session("cutex-forged-worker"));
        assert_eq!(
            forged_worker_fixture.provider.prepare_worker_action(
                &wrong_worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: WorkerActionRequest::Start(AssignmentActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("forged-worker"),
                        assignment_id: assignment_id(),
                    }),
                },
            ),
            Err(ProviderError::Unauthorized)
        );
    }

    #[test]
    fn reviewed_failure_enters_retry_pending_until_coordinator_closes() {
        let fixture = Fixture::new("reviewed-failure");
        fixture.provision();
        fixture.start("start");
        fixture.submit("submit", "failed-result");
        fixture.terminal_action(TerminalAuthorityRequest::FailResult(
            TerminalActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("fail-result"),
                assignment_id: assignment_id(),
                decision_reference: Some("does not meet contract".into()),
            },
        ));
        let pending = fixture.provider.query().unwrap();
        assert_eq!(
            pending.assignments[&assignment_id()].state,
            AssignmentState::RetryPending
        );
        assert_eq!(
            pending.active_attempt(&assignment_id()).unwrap().phase,
            AttemptPhase::Failed
        );
        let close_context = fixture.worker_context();
        fixture
            .provider
            .close_assignment(
                &fixture.coordinator,
                &CloseAssignmentRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("close-failed"),
                    assignment_id: assignment_id(),
                },
                close_context.expected_assignment_revision,
                close_context.attempt.as_ref().unwrap(),
            )
            .unwrap();
        assert_eq!(
            fixture.provider.query().unwrap().assignments[&assignment_id()]
                .closure
                .as_ref()
                .unwrap()
                .reason,
            ClosureReason::Failed
        );
    }

    #[test]
    fn management_seats_drive_one_shared_director_worker_release_result() {
        let root = root("authenticated-product-flow");
        let provider = TaskServiceProvider::open(root.join("provider")).unwrap();
        provider.recover().unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let director_session = session("cutex-director-session");
        let worker_session = session("cutex-worker-session");
        let release_session = session("cutex-release-session");
        let director_seat = id("cutex-director", SeatId::new);
        let release_seat = id("cutex-release", SeatId::new);
        let director_binding = seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: action("management-bind-director"),
                seat_id: director_seat,
                occupant_cutex_session: director_session.clone(),
            })
            .unwrap();
        let release_binding = seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: action("management-bind-release"),
                seat_id: release_seat.clone(),
                occupant_cutex_session: release_session.clone(),
            })
            .unwrap();
        assert_eq!(director_binding.occupancy.epoch, 1);
        assert_eq!(release_binding.occupancy.epoch, 1);
        let director = seats.resolve_principal(&director_session).unwrap();
        let release = seats.resolve_principal(&release_session).unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session.clone());
        assert_eq!(
            seats.resolve_principal(&session("unseated-session")),
            Err(crate::seat::SeatAuthorityError::Unauthorized)
        );

        let contract = "authenticated causal contract";
        provider
            .create_revision(
                &director,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("director-create"),
                    workflow_id: id("workflow-authenticated", WorkflowId::new),
                    task_id: TaskId::new("CUTEX-authenticated").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha(contract),
                    opaque_contract: contract.into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: release_seat,
                    },
                },
                None,
            )
            .unwrap();
        provider
            .assign_and_dispatch(
                &director,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("director-assign"),
                    assignment_id: assignment_id(),
                    task_id: TaskId::new("CUTEX-authenticated").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: worker_session,
                    send_attempt_id: id("authenticated-send", SendAttemptId::new),
                    external_message_id: "authenticated-message".into(),
                },
                1,
                "authenticated assignment content",
            )
            .unwrap();
        let start_envelope = match provider
            .prepare_worker_action(
                &worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: WorkerActionRequest::Start(AssignmentActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("worker-start"),
                        assignment_id: assignment_id(),
                    }),
                },
            )
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        provider
            .execute_worker_action(&worker, &start_envelope)
            .unwrap();
        let submit_envelope = match provider
            .prepare_worker_action(
                &worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: WorkerActionRequest::Submit(SubmitActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("worker-submit"),
                        assignment_id: assignment_id(),
                        result_sha256: sha("causal result"),
                        result_reference: "causal result".into(),
                    }),
                },
            )
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        provider
            .execute_worker_action(&worker, &submit_envelope)
            .unwrap();
        let context_request = WorkerContextRequest {
            schema: WorkerContextRequestSchema::V2,
            assignment_id: assignment_id(),
        };
        let terminal_context = provider
            .worker_context(&worker, &context_request)
            .unwrap()
            .context;
        provider
            .execute_terminal_action(
                &release,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("release-accept"),
                        assignment_id: assignment_id(),
                        decision_reference: Some("release review passed".into()),
                    }),
                    context: terminal_context,
                },
            )
            .unwrap();

        let query_as = |principal: &AuthenticatedPrincipal| {
            principal.authenticated_session_id().unwrap();
            provider.query().unwrap()
        };
        let director_view = query_as(&director);
        let worker_view = query_as(&worker);
        let release_view = query_as(&release);
        assert_eq!(director_view, worker_view);
        assert_eq!(worker_view, release_view);
        assert_eq!(
            director_view.assignments[&assignment_id()].state,
            AssignmentState::Closed
        );
        assert_eq!(
            director_view
                .active_attempt(&assignment_id())
                .unwrap()
                .phase,
            AttemptPhase::Completed
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn coordinator_rebind_revokes_predecessor_and_preserves_seat_idempotency() {
        let root = root("coordinator-rebind");
        let provider = TaskServiceProvider::open(root.join("provider")).unwrap();
        provider.recover().unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let predecessor_session = session("director-predecessor");
        let successor_session = session("director-successor");
        let coordinator_seat = id("cutex-director", SeatId::new);
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: action("bind-predecessor"),
                seat_id: coordinator_seat.clone(),
                occupant_cutex_session: predecessor_session.clone(),
            })
            .unwrap();
        let predecessor = seats.resolve_principal(&predecessor_session).unwrap();
        let contract = "seat-stable contract";
        let request = CreateRevisionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("seat-stable-create"),
            workflow_id: id("seat-stable-workflow", WorkflowId::new),
            task_id: TaskId::new("CUTEX-seat-stable").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            contract_sha256: sha(contract),
            opaque_contract: contract.into(),
            completion_policy: CompletionPolicy {
                kind: CompletionPolicyKind::DirectorAcceptance,
                authority_seat_id: coordinator_seat.clone(),
            },
        };
        let first = provider
            .create_revision(&predecessor, &request, None)
            .unwrap();
        let rebound = seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: action("bind-successor"),
                seat_id: coordinator_seat,
                occupant_cutex_session: successor_session.clone(),
            })
            .unwrap();
        assert_eq!(rebound.occupancy.epoch, 2);
        assert_eq!(
            seats.resolve_principal(&predecessor_session),
            Err(crate::seat::SeatAuthorityError::Unauthorized)
        );
        let successor = seats.resolve_principal(&successor_session).unwrap();
        assert_eq!(
            provider
                .create_revision(&successor, &request, None)
                .unwrap(),
            first
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authority_documents_separate_mechanical_cas_from_identity_claims() {
        let coordinator = CoordinatorActionRequest {
            schema: CoordinatorRequestSchema::V2,
            command: CoordinatorOperation::CreateRevision(CreateRevisionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("strict-create"),
                workflow_id: id("strict-workflow", WorkflowId::new),
                task_id: TaskId::new("CUTEX-strict").unwrap(),
                task_revision: TaskRevision::new(1).unwrap(),
                contract_sha256: sha("strict contract"),
                opaque_contract: "strict contract".into(),
                completion_policy: CompletionPolicy {
                    kind: CompletionPolicyKind::ReleaseReview,
                    authority_seat_id: id("cutex-release", SeatId::new),
                },
            }),
            context: CoordinatorMechanicalContext::CreateRevision {
                expected_workflow_revision: None,
            },
        };
        let terminal = TerminalActionEnvelope {
            schema: TerminalRequestSchema::V2,
            command: TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("strict-accept"),
                assignment_id: assignment_id(),
                decision_reference: None,
            }),
            context: WorkerMechanicalContext {
                expected_assignment_revision: 2,
                attempt: Some(AttemptMechanicalContext {
                    attempt_number: AttemptNumber::new(1).unwrap(),
                    attempt_token: id("strict-attempt-token", ProviderAttemptToken::new),
                    expected_attempt_revision: 2,
                }),
            },
        };
        let query = TaskServiceQueryRequest {
            schema: QueryRequestSchema::V2,
            query: TaskServiceQueryOperation::Watch {
                after_sequence: 0,
                limit: 100,
            },
        };
        for encoded in [
            serde_json::to_string(&coordinator.command).unwrap(),
            serde_json::to_string(&terminal.command).unwrap(),
            serde_json::to_string(&query).unwrap(),
        ] {
            for forbidden in [
                "runtime_agent_id",
                "runtime_generation",
                "caller_cutex_session",
                "seat_id_claim",
                "seat_epoch",
                "expected_store_revision",
            ] {
                assert!(!encoded.contains(forbidden), "{forbidden}: {encoded}");
            }
        }
        for forbidden in [
            "runtime_agent_id",
            "cutex_session_id",
            "seat_id",
            "seat_epoch",
            "attempt_token",
            "expected_store_revision",
        ] {
            let mut coordinator_value = serde_json::to_value(&coordinator).unwrap();
            coordinator_value[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<CoordinatorActionRequest>(coordinator_value).is_err());
            let mut coordinator_command = serde_json::to_value(&coordinator).unwrap();
            coordinator_command["command"][forbidden] = serde_json::json!("forged");
            assert!(
                serde_json::from_value::<CoordinatorActionRequest>(coordinator_command).is_err()
            );
            let mut terminal_value = serde_json::to_value(&terminal).unwrap();
            terminal_value[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TerminalActionEnvelope>(terminal_value).is_err());
            let mut terminal_command = serde_json::to_value(&terminal).unwrap();
            terminal_command["command"][forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TerminalActionEnvelope>(terminal_command).is_err());
            let mut query_value = serde_json::to_value(&query).unwrap();
            query_value[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TaskServiceQueryRequest>(query_value).is_err());
            let mut query_command = serde_json::to_value(&query).unwrap();
            query_command["query"][forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<TaskServiceQueryRequest>(query_command).is_err());
        }
    }

    #[test]
    fn revision_three_worker_request_bytes_remain_compatible() {
        let request = WorkerActionRequest::Start(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("worker-byte-compatible"),
            assignment_id: assignment_id(),
        });
        assert_eq!(
            serde_json::to_vec(&request).unwrap(),
            br#"{"operation":"start","body":{"schema":"cutex/task-service-action/v2","action_id":"worker-byte-compatible","assignment_id":"assignment-1"}}"#
        );
        let prepare = WorkerPrepareRequest {
            schema: WorkerPrepareRequestSchema::V2,
            action: request.clone(),
        };
        assert_eq!(
            serde_json::to_vec(&prepare).unwrap(),
            br#"{"schema":"cutex/task-service-worker-prepare/v2","action":{"operation":"start","body":{"schema":"cutex/task-service-action/v2","action_id":"worker-byte-compatible","assignment_id":"assignment-1"}}}"#
        );
        let envelope = WorkerProviderActionEnvelope {
            schema: WorkerProviderRequestSchema::V2,
            action: request,
            context: WorkerMechanicalContext {
                expected_assignment_revision: 1,
                attempt: None,
            },
        };
        assert_eq!(
            serde_json::to_vec(&envelope).unwrap(),
            br#"{"schema":"cutex/task-service-worker-provider/v2","action":{"operation":"start","body":{"schema":"cutex/task-service-action/v2","action_id":"worker-byte-compatible","assignment_id":"assignment-1"}},"context":{"expected_assignment_revision":1,"attempt":null}}"#
        );
        let context_request = WorkerContextRequest {
            schema: WorkerContextRequestSchema::V2,
            assignment_id: assignment_id(),
        };
        assert_eq!(
            serde_json::to_vec(&context_request).unwrap(),
            br#"{"schema":"cutex/task-service-worker-context/v2","assignment_id":"assignment-1"}"#
        );
    }

    #[test]
    fn report_status_omitted_evidence_roundtrips_and_replays_without_null() {
        let fixture = Fixture::new("status-optional-evidence");
        fixture.provision();
        fixture.start("start");

        let omitted = serde_json::json!({
            "schema": "cutex/task-service-worker-prepare/v2",
            "action": {
                "operation": "report_status",
                "body": {
                    "schema": "cutex/task-service-action/v2",
                    "action_id": "status-omitted-evidence",
                    "assignment_id": "assignment-1",
                    "summary": "production progress"
                }
            }
        });
        let request: WorkerPrepareRequest = serde_json::from_value(omitted.clone()).unwrap();
        let WorkerActionRequest::ReportStatus(status) = &request.action else {
            unreachable!()
        };
        assert_eq!(status.evidence_sha256, None);

        let mut legacy_null = omitted.clone();
        legacy_null["action"]["body"]["evidence_sha256"] = serde_json::Value::Null;
        assert_eq!(
            serde_json::from_value::<WorkerPrepareRequest>(legacy_null).unwrap(),
            request
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), omitted);

        let envelope = match fixture
            .provider
            .prepare_worker_action(&fixture.worker, &request)
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        let serialized_envelope = serde_json::to_value(&envelope).unwrap();
        assert_eq!(serialized_envelope["action"], omitted["action"]);
        assert!(serialized_envelope["action"]["body"]
            .get("evidence_sha256")
            .is_none());

        let committed = fixture
            .provider
            .execute_worker_action(&fixture.worker, &envelope)
            .unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &envelope)
                .unwrap(),
            committed
        );
        assert_eq!(
            fixture
                .provider
                .prepare_worker_action(&fixture.worker, &request)
                .unwrap(),
            WorkerPrepareOutcome::Committed(committed)
        );

        let evidence = sha("present evidence");
        let present = serde_json::json!({
            "schema": "cutex/task-service-worker-prepare/v2",
            "action": {
                "operation": "report_status",
                "body": {
                    "schema": "cutex/task-service-action/v2",
                    "action_id": "status-present-evidence",
                    "assignment_id": "assignment-1",
                    "summary": "progress with evidence",
                    "evidence_sha256": evidence.as_str()
                }
            }
        });
        let present_request: WorkerPrepareRequest =
            serde_json::from_value(present.clone()).unwrap();
        let present_envelope = match fixture
            .provider
            .prepare_worker_action(&fixture.worker, &present_request)
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        assert_eq!(
            serde_json::to_value(&present_envelope).unwrap()["action"],
            present["action"]
        );

        let mut changed_summary = omitted.clone();
        changed_summary["action"]["body"]["summary"] = serde_json::json!("changed");
        let mut changed_evidence = omitted;
        changed_evidence["action"]["body"]["evidence_sha256"] =
            serde_json::to_value(evidence).unwrap();
        for changed in [changed_summary, changed_evidence] {
            let changed_request: WorkerPrepareRequest = serde_json::from_value(changed).unwrap();
            let before = fixture.provider.query().unwrap();
            assert_eq!(
                fixture
                    .provider
                    .prepare_worker_action(&fixture.worker, &changed_request),
                Err(ProviderError::Conflict("action_id_payload_conflict"))
            );
            assert_eq!(fixture.provider.query().unwrap(), before);
        }
    }

    #[test]
    fn report_status_digest_preserves_pre_omission_prepared_identity() {
        let action = WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("status-omitted-evidence"),
            assignment_id: assignment_id(),
            summary: "production progress".into(),
            evidence_sha256: None,
        });
        let principal = AuthenticatedPrincipal::session(session("cutex-worker-r1"));
        let binding = DurableAttemptBinding {
            attempt_number: AttemptNumber::new(1).unwrap(),
            attempt_token: id("attempt-assignment-1-1-start", ProviderAttemptToken::new),
        };
        assert_eq!(
            worker_request_digest("report_status", &principal, &action, Some(&binding))
                .unwrap()
                .as_str(),
            "8191bc442b732ccd46272d6dfde18ef8a99f843f677185bcd8333ee19a5e807f"
        );
    }

    #[test]
    fn block_requires_a_bounded_summary_before_durable_preparation() {
        let missing = serde_json::json!({
            "operation": "block",
            "body": {
                "schema": "cutex/task-service-action/v2",
                "action_id": "missing-blocker-summary",
                "assignment_id": "assignment-1"
            }
        });
        assert!(serde_json::from_value::<WorkerActionRequest>(missing).is_err());

        let fixture = Fixture::new("blocker-summary-bound");
        fixture.provision();
        fixture.start("start");
        for (label, summary) in [
            ("empty", "   ".to_string()),
            ("oversized", "x".repeat(MAX_BLOCKER_SUMMARY_BYTES + 1)),
        ] {
            let request = WorkerPrepareRequest {
                schema: WorkerPrepareRequestSchema::V2,
                action: WorkerActionRequest::Block(BlockActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action(&format!("block-{label}")),
                    assignment_id: assignment_id(),
                    summary,
                }),
            };
            let before = fixture.provider.query().unwrap();
            assert_eq!(
                fixture
                    .provider
                    .prepare_worker_action(&fixture.worker, &request),
                Err(ProviderError::InvalidRequest("blocker_summary"))
            );
            assert_eq!(fixture.provider.query().unwrap(), before);
        }
    }

    #[test]
    fn strict_worker_schema_rejects_mechanical_fields() {
        for forbidden in [
            "runtime_agent_id",
            "runtime_generation",
            "attempt_token",
            "attempt_number",
            "expected_assignment_revision",
            "expected_attempt_revision",
            "expected_store_revision",
        ] {
            let value = serde_json::json!({
                "operation": "start",
                "body": {
                    "schema": "cutex/task-service-action/v2",
                    "action_id": "start",
                    "assignment_id": "assignment-1",
                    forbidden: "forged"
                }
            });
            assert!(serde_json::from_value::<WorkerActionRequest>(value).is_err());
        }
        let envelope = WorkerProviderActionEnvelope {
            schema: WorkerProviderRequestSchema::V2,
            action: WorkerActionRequest::Start(AssignmentActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("strict-provider-envelope"),
                assignment_id: assignment_id(),
            }),
            context: WorkerMechanicalContext {
                expected_assignment_revision: 1,
                attempt: None,
            },
        };
        let mut forged = serde_json::to_value(&envelope).unwrap();
        forged["context"]["global_journal_sequence"] = serde_json::json!(7);
        assert!(serde_json::from_value::<WorkerProviderActionEnvelope>(forged).is_err());
        let mut forged_prepare = serde_json::to_value(WorkerPrepareRequest {
            schema: WorkerPrepareRequestSchema::V2,
            action: envelope.action,
        })
        .unwrap();
        forged_prepare["context"] = serde_json::json!({
            "expected_assignment_revision": 1,
            "attempt": null
        });
        assert!(serde_json::from_value::<WorkerPrepareRequest>(forged_prepare).is_err());
        assert!(!TASK_SERVICE_PROVIDER_CONTRACT_JSON.contains("expected_store_revision"));
        assert!(!TASK_SERVICE_PROVIDER_CONTRACT_JSON.contains("runtime_agent_id"));
        assert!(TASK_SERVICE_PROVIDER_CONTRACT_JSON.contains("attempt_token"));
        assert!(TASK_SERVICE_PROVIDER_CONTRACT_JSON.contains("assignment_contract_delivery"));
        assert!(TASK_SERVICE_PROVIDER_CONTRACT_JSON.contains("262144 bytes"));
    }

    #[test]
    fn stale_assignment_and_every_attempt_handle_component_are_no_write() {
        for operation in ["start", "decline"] {
            let fixture = Fixture::new(&format!("stale-assignment-{operation}"));
            fixture.provision();
            let action = match operation {
                "start" => WorkerActionRequest::Start(AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-assignment-start"),
                    assignment_id: assignment_id(),
                }),
                _ => WorkerActionRequest::Decline(AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-assignment-decline"),
                    assignment_id: assignment_id(),
                }),
            };
            let mut envelope = fixture.worker_envelope(action);
            envelope.context.expected_assignment_revision += 1;
            let before = fixture.provider.query().unwrap();
            assert_eq!(
                fixture
                    .provider
                    .execute_worker_action(&fixture.worker, &envelope),
                Err(ProviderError::Conflict("prepared_binding_conflict"))
            );
            assert_eq!(fixture.provider.query().unwrap(), before);
        }

        for operation in [
            "report_status",
            "block",
            "resume",
            "submit",
            "abort_attempt",
        ] {
            for fault in ["number", "token", "revision"] {
                let fixture = Fixture::new(&format!("attempt-{operation}-{fault}"));
                fixture.provision();
                fixture.start("start");
                if operation == "resume" {
                    fixture.worker_action(WorkerActionRequest::Block(BlockActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("prepare-blocked"),
                        assignment_id: assignment_id(),
                        summary: "prepare the blocked-state fixture".into(),
                    }));
                }
                let action = match operation {
                    "report_status" => WorkerActionRequest::ReportStatus(StatusActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("probe-report-status"),
                        assignment_id: assignment_id(),
                        summary: "still working".into(),
                        evidence_sha256: None,
                    }),
                    "block" => WorkerActionRequest::Block(BlockActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("probe-block"),
                        assignment_id: assignment_id(),
                        summary: "mechanical binding probe is blocked".into(),
                    }),
                    "resume" => WorkerActionRequest::Resume(AssignmentActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("probe-resume"),
                        assignment_id: assignment_id(),
                    }),
                    "submit" => WorkerActionRequest::Submit(SubmitActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("probe-submit"),
                        assignment_id: assignment_id(),
                        result_sha256: sha("probe result"),
                        result_reference: "probe result".into(),
                    }),
                    _ => WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("probe-abort"),
                        assignment_id: assignment_id(),
                    }),
                };
                let mut envelope = fixture.worker_envelope(action);
                let binding = envelope.context.attempt.as_mut().unwrap();
                let expected = match fault {
                    "number" => {
                        binding.attempt_number = AttemptNumber::new(2).unwrap();
                        ProviderError::Conflict("prepared_binding_conflict")
                    }
                    "token" => {
                        binding.attempt_token =
                            id("wrong-attempt-token", ProviderAttemptToken::new);
                        ProviderError::Conflict("prepared_binding_conflict")
                    }
                    _ => {
                        binding.expected_attempt_revision += 1;
                        ProviderError::Conflict("prepared_binding_conflict")
                    }
                };
                let before = fixture.provider.query().unwrap();
                assert_eq!(
                    fixture
                        .provider
                        .execute_worker_action(&fixture.worker, &envelope),
                    Err(expected),
                    "{operation}/{fault}"
                );
                assert_eq!(fixture.provider.query().unwrap(), before);
            }
        }
    }

    #[test]
    fn committed_replay_precedes_cas_and_unrelated_send_revision_does_not_stale_attempt() {
        let fixture = Fixture::new("replay-before-cas");
        fixture.provision();
        fixture.start("start");
        let status =
            fixture.worker_envelope(WorkerActionRequest::ReportStatus(StatusActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("status-before-send-event"),
                assignment_id: assignment_id(),
                summary: "progress".into(),
                evidence_sha256: None,
            }));
        fixture
            .provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("send-event"),
                    send_attempt_id: id("send-1", SendAttemptId::new),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some("queued".into()),
                },
            )
            .unwrap();
        let committed = fixture
            .provider
            .execute_worker_action(&fixture.worker, &status)
            .unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &status)
                .unwrap(),
            committed
        );
        let mut changed_binding = status.clone();
        changed_binding.context.expected_assignment_revision += 1;
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &changed_binding)
                .unwrap(),
            committed
        );
        let stale_send = fixture.provider.record_communication_event(
            &AuthenticatedPrincipal::task_service_system(),
            &CommunicationEventRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("stale-send-event"),
                send_attempt_id: id("send-1", SendAttemptId::new),
                expected_send_attempt_revision: 1,
                kind: CommunicationEventKind::ContextInserted,
                receipt_reference: None,
            },
        );
        assert_eq!(
            stale_send,
            Err(ProviderError::Conflict("send_attempt_revision_conflict"))
        );
    }

    #[test]
    fn durable_prepare_and_committed_probe_survive_provider_reopen() {
        let fixture = Fixture::new("durable-prepare-reopen");
        fixture.provision();
        let start_action = WorkerActionRequest::Start(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("durable-start"),
            assignment_id: assignment_id(),
        });
        let start_envelope = fixture.worker_envelope(start_action.clone());
        let root = fixture.provider.root.as_ref().clone();
        let reopened = TaskServiceProvider::open(root.clone()).unwrap();
        assert_eq!(
            &reopened.query().unwrap().prepared_worker_actions[&action("durable-start")].context,
            &start_envelope.context
        );
        let start_receipt = reopened
            .execute_worker_action(&fixture.worker, &start_envelope)
            .unwrap();

        let restarted = TaskServiceProvider::open(root.clone()).unwrap();
        assert_eq!(
            restarted
                .prepare_worker_action(
                    &fixture.worker,
                    &WorkerPrepareRequest {
                        schema: WorkerPrepareRequestSchema::V2,
                        action: start_action,
                    },
                )
                .unwrap(),
            WorkerPrepareOutcome::Committed(start_receipt.clone())
        );
        assert!(!restarted
            .query()
            .unwrap()
            .prepared_worker_actions
            .contains_key(&action("durable-start")));

        let status_action = WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("durable-status"),
            assignment_id: assignment_id(),
            summary: "durable progress".into(),
            evidence_sha256: None,
        });
        let status_envelope = match restarted
            .prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: status_action.clone(),
                },
            )
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        let restarted_again = TaskServiceProvider::open(root).unwrap();
        let status_receipt = restarted_again
            .execute_worker_action(&fixture.worker, &status_envelope)
            .unwrap();
        assert_eq!(
            restarted_again
                .prepare_worker_action(
                    &fixture.worker,
                    &WorkerPrepareRequest {
                        schema: WorkerPrepareRequestSchema::V2,
                        action: status_action,
                    },
                )
                .unwrap(),
            WorkerPrepareOutcome::Committed(status_receipt)
        );
    }

    #[test]
    fn same_attempt_prepare_refreshes_only_cas_and_changed_semantics_conflict() {
        let fixture = Fixture::new("durable-prepare-refresh");
        fixture.provision();
        fixture.start("start");
        let prepared_action = WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("refreshable-status"),
            assignment_id: assignment_id(),
            summary: "original semantic bytes".into(),
            evidence_sha256: None,
        });
        let first = fixture.worker_envelope(prepared_action.clone());
        let first_binding = first.context.attempt.clone().unwrap();

        fixture.worker_action(WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("intervening-status"),
            assignment_id: assignment_id(),
            summary: "advances only attempt CAS".into(),
            evidence_sha256: None,
        }));
        let before_stale_execute = fixture.provider.query().unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &first),
            Err(ProviderError::Conflict("attempt_revision_conflict"))
        );
        assert_eq!(fixture.provider.query().unwrap(), before_stale_execute);
        let refreshed = match fixture
            .provider
            .prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: prepared_action.clone(),
                },
            )
            .unwrap()
        {
            WorkerPrepareOutcome::Prepared(envelope) => envelope,
            WorkerPrepareOutcome::Committed(_) => unreachable!(),
        };
        let refreshed_binding = refreshed.context.attempt.clone().unwrap();
        assert_eq!(
            first_binding.attempt_number,
            refreshed_binding.attempt_number
        );
        assert_eq!(first_binding.attempt_token, refreshed_binding.attempt_token);
        assert!(
            refreshed_binding.expected_attempt_revision > first_binding.expected_attempt_revision
        );

        let mut changed = prepared_action;
        let WorkerActionRequest::ReportStatus(changed_body) = &mut changed else {
            unreachable!()
        };
        changed_body.summary = "changed semantic bytes".into();
        let before_conflict = fixture.provider.query().unwrap();
        assert_eq!(
            fixture.provider.prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: changed,
                },
            ),
            Err(ProviderError::Conflict("action_id_payload_conflict"))
        );
        assert_eq!(fixture.provider.query().unwrap(), before_conflict);
        let committed = fixture
            .provider
            .execute_worker_action(&fixture.worker, &refreshed)
            .unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &first)
                .unwrap(),
            committed
        );
    }

    #[test]
    fn uncommitted_attempt_one_prepare_never_retargets_attempt_two_after_reopen() {
        let fixture = Fixture::new("durable-attempt-binding");
        fixture.provision();
        fixture.start("start-one");
        let old_action = WorkerActionRequest::ReportStatus(StatusActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("durable-unused-attempt-one"),
            assignment_id: assignment_id(),
            summary: "bound to attempt one".into(),
            evidence_sha256: None,
        });
        let old_envelope = fixture.worker_envelope(old_action.clone());
        let original_record = fixture.provider.query().unwrap().prepared_worker_actions
            [&action("durable-unused-attempt-one")]
            .clone();
        fixture.worker_action(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("end-attempt-one"),
            assignment_id: assignment_id(),
        }));
        let retry = fixture.worker_context();
        fixture
            .provider
            .authorize_attempt_retry(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("authorize-attempt-two-durable"),
                    assignment_id: assignment_id(),
                },
                retry.expected_assignment_revision,
            )
            .unwrap();
        fixture.start("start-two");

        let reopened = TaskServiceProvider::open(fixture.provider.root.as_ref().clone()).unwrap();
        let before = reopened.query().unwrap();
        assert_eq!(
            reopened.prepare_worker_action(
                &fixture.worker,
                &WorkerPrepareRequest {
                    schema: WorkerPrepareRequestSchema::V2,
                    action: old_action,
                },
            ),
            Err(ProviderError::Conflict("attempt_handle_conflict"))
        );
        assert_eq!(reopened.query().unwrap(), before);
        assert_eq!(
            before.prepared_worker_actions[&action("durable-unused-attempt-one")],
            original_record
        );
        assert_eq!(
            reopened.execute_worker_action(&fixture.worker, &old_envelope),
            Err(ProviderError::Conflict("attempt_handle_conflict"))
        );
        assert_eq!(reopened.query().unwrap(), before);
    }

    #[test]
    fn coordinator_terminal_and_system_replay_ignore_only_cas_revisions() {
        let fixture = Fixture::new("stable-non-worker-digest");
        fixture.provision();
        let create_request = CreateRevisionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("create"),
            workflow_id: id("workflow-1", WorkflowId::new),
            task_id: TaskId::new("CUTEX-test").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            contract_sha256: sha("contract"),
            opaque_contract: "contract".into(),
            completion_policy: CompletionPolicy {
                kind: CompletionPolicyKind::ReleaseReview,
                authority_seat_id: id("release", SeatId::new),
            },
        };
        let original_create = fixture.provider.query().unwrap().receipts[&action("create")].clone();
        assert_eq!(
            fixture
                .provider
                .create_revision(&fixture.coordinator, &create_request, Some(999))
                .unwrap(),
            original_create
        );

        let system_request = CommunicationEventRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("stable-system-send"),
            send_attempt_id: id("send-1", SendAttemptId::new),
            expected_send_attempt_revision: 1,
            kind: CommunicationEventKind::BusQueued,
            receipt_reference: Some("stable-system-receipt".into()),
        };
        let system_receipt = fixture
            .provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &system_request,
            )
            .unwrap();
        let mut fresh_system_cas = system_request;
        fresh_system_cas.expected_send_attempt_revision = 2;
        assert_eq!(
            fixture
                .provider
                .record_communication_event(
                    &AuthenticatedPrincipal::task_service_system(),
                    &fresh_system_cas,
                )
                .unwrap(),
            system_receipt
        );

        fixture.start("stable-terminal-start");
        fixture.submit("stable-terminal-submit", "stable terminal result");
        let command = TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("stable-terminal-accept"),
            assignment_id: assignment_id(),
            decision_reference: Some("accepted".into()),
        });
        let first_context = fixture.worker_context();
        let terminal_receipt = fixture
            .provider
            .execute_terminal_action(
                &fixture.authority,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: command.clone(),
                    context: first_context,
                },
            )
            .unwrap();
        let fresh_context = fixture.worker_context();
        assert_eq!(
            fixture
                .provider
                .execute_terminal_action(
                    &fixture.authority,
                    &TerminalActionEnvelope {
                        schema: TerminalRequestSchema::V2,
                        command,
                        context: fresh_context,
                    },
                )
                .unwrap(),
            terminal_receipt
        );
    }

    #[test]
    fn old_unused_attempt_one_action_cannot_cross_into_attempt_two() {
        let fixture = Fixture::new("attempt-crossing-counterexample");
        fixture.provision();
        fixture.start("start-one");
        let old_unused =
            fixture.worker_envelope(WorkerActionRequest::ReportStatus(StatusActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("unused-attempt-one-status"),
                assignment_id: assignment_id(),
                summary: "attempt one response uncertain".into(),
                evidence_sha256: None,
            }));
        fixture.worker_action(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
            schema: ProviderActionSchema::V2,
            action_id: action("terminal-attempt-one"),
            assignment_id: assignment_id(),
        }));
        let retry_context = fixture.worker_context();
        fixture
            .provider
            .authorize_attempt_retry(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("authorize-attempt-two"),
                    assignment_id: assignment_id(),
                },
                retry_context.expected_assignment_revision,
            )
            .unwrap();
        fixture.start("start-two");
        let before = fixture.provider.query().unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &old_unused),
            Err(ProviderError::Conflict("attempt_handle_conflict"))
        );
        assert_eq!(fixture.provider.query().unwrap(), before);
        assert_eq!(
            before
                .active_attempt(&assignment_id())
                .unwrap()
                .attempt_number,
            AttemptNumber::new(2).unwrap()
        );
    }

    #[test]
    fn worker_context_is_assignee_only_and_semantic_query_hides_mechanics() {
        let fixture = Fixture::new("worker-context-secrecy");
        fixture.provision();
        fixture.start("start");
        let request = WorkerContextRequest {
            schema: WorkerContextRequestSchema::V2,
            assignment_id: assignment_id(),
        };
        let context = fixture
            .provider
            .worker_context(&fixture.worker, &request)
            .unwrap();
        assert!(context.context.attempt.is_some());
        assert_eq!(
            fixture.provider.worker_context(
                &AuthenticatedPrincipal::session(session("different-worker")),
                &request
            ),
            Err(ProviderError::Unauthorized)
        );
        let semantic = fixture.provider.query_assignee(&fixture.worker).unwrap();
        let encoded = serde_json::to_string(&semantic).unwrap();
        for secret in [
            "attempt_token",
            "local_revision",
            "assignee_cutex_session",
            "request_sha256",
        ] {
            assert!(!encoded.contains(secret), "secret {secret}: {encoded}");
        }
        let mut forged = serde_json::to_value(&request).unwrap();
        forged["expected_assignment_revision"] = serde_json::json!(1);
        assert!(serde_json::from_value::<WorkerContextRequest>(forged).is_err());
    }

    #[test]
    fn coordinator_and_terminal_boundaries_enforce_aggregate_local_cas() {
        let fixture = Fixture::new("coordinator-cas");
        fixture.provision();
        let before = fixture.provider.query().unwrap();
        assert_eq!(
            fixture.provider.create_revision(
                &fixture.coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-workflow-create"),
                    workflow_id: id("workflow-1", WorkflowId::new),
                    task_id: TaskId::new("CUTEX-test").unwrap(),
                    task_revision: TaskRevision::new(2).unwrap(),
                    contract_sha256: sha("revision two"),
                    opaque_contract: "revision two".into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: id("release", SeatId::new),
                    },
                },
                Some(2),
            ),
            Err(ProviderError::Conflict("workflow_revision_conflict"))
        );
        assert_eq!(
            fixture.provider.assign_and_dispatch(
                &fixture.coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-workflow-assign"),
                    assignment_id: id("assignment-stale", AssignmentId::new),
                    task_id: TaskId::new("CUTEX-test").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: session("cutex-worker-r1"),
                    send_attempt_id: id("send-stale-assignment", SendAttemptId::new),
                    external_message_id: "stale-assignment-message".into(),
                },
                2,
                "stale assignment content",
            ),
            Err(ProviderError::Conflict("workflow_revision_conflict"))
        );
        assert_eq!(
            fixture.provider.retry_delivery(
                &fixture.coordinator,
                &RetryDeliveryRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-retry-delivery"),
                    assignment_id: assignment_id(),
                    send_attempt_id: id("send-stale", SendAttemptId::new),
                    external_message_id: "message-stale".into(),
                },
                2,
                "stale retry content",
            ),
            Err(ProviderError::Conflict("assignment_revision_conflict"))
        );
        assert_eq!(fixture.provider.query().unwrap(), before);

        // Start first so the abort preparation has a durable valid handle.
        fixture.start("start-for-stale-abort");
        let abort =
            fixture.worker_envelope(WorkerActionRequest::AbortAttempt(AssignmentActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: action("stale-multi-abort"),
                assignment_id: assignment_id(),
            }));
        let cancel_context = fixture.worker_context();
        assert_eq!(
            fixture.provider.cancel_assignment(
                &fixture.coordinator,
                &AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-coordinator-cancel"),
                    assignment_id: assignment_id(),
                },
                cancel_context.expected_assignment_revision + 1,
                cancel_context.attempt.as_ref(),
            ),
            Err(ProviderError::Conflict("assignment_revision_conflict"))
        );
        let mut abort = WorkerProviderActionEnvelope {
            context: fixture.worker_context(),
            ..abort
        };
        abort.context.expected_assignment_revision += 1;
        let before_abort = fixture.provider.query().unwrap();
        assert_eq!(
            fixture
                .provider
                .execute_worker_action(&fixture.worker, &abort),
            Err(ProviderError::Conflict("prepared_binding_conflict"))
        );
        assert_eq!(fixture.provider.query().unwrap(), before_abort);

        for operation in ["request_changes", "accept_result", "fail_result", "cancel"] {
            let terminal = Fixture::new(&format!("terminal-cas-{operation}"));
            terminal.provision();
            terminal.start("start");
            if operation != "cancel" {
                terminal.submit("submit", "terminal result");
            }
            let command = match operation {
                "request_changes" => {
                    TerminalAuthorityRequest::RequestChanges(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: action("stale-request-changes"),
                        assignment_id: assignment_id(),
                        decision_reference: Some("bounded change request".into()),
                    })
                }
                "accept_result" => TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-accept"),
                    assignment_id: assignment_id(),
                    decision_reference: None,
                }),
                "fail_result" => TerminalAuthorityRequest::FailResult(TerminalActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-fail"),
                    assignment_id: assignment_id(),
                    decision_reference: None,
                }),
                _ => TerminalAuthorityRequest::Cancel(TerminalActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("stale-terminal-cancel"),
                    assignment_id: assignment_id(),
                    decision_reference: None,
                }),
            };
            let mut envelope = TerminalActionEnvelope {
                schema: TerminalRequestSchema::V2,
                command,
                context: terminal.worker_context(),
            };
            let expected = if matches!(operation, "request_changes" | "fail_result") {
                envelope
                    .context
                    .attempt
                    .as_mut()
                    .unwrap()
                    .expected_attempt_revision += 1;
                ProviderError::Conflict("attempt_revision_conflict")
            } else {
                envelope.context.expected_assignment_revision += 1;
                ProviderError::Conflict("assignment_revision_conflict")
            };
            let before = terminal.provider.query().unwrap();
            assert_eq!(
                terminal
                    .provider
                    .execute_terminal_action(&terminal.authority, &envelope),
                Err(expected),
                "{operation}"
            );
            assert_eq!(terminal.provider.query().unwrap(), before);
        }
    }
}
