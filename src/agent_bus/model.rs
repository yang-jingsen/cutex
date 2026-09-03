//! Core agent bus protocol enums shared by CLI, management, and routing code.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::agent_bus::delivery::AgentDeliveryMode;

/// Maximum UTF-8 byte length carried in a protected Task Service assignment.
/// The provider may store larger contracts; those must be revised to a smaller
/// task revision before they can cross the model-input transport boundary.
pub const TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskServiceAssignmentContractError {
    Missing,
    Empty,
    TooLarge { actual: usize, maximum: usize },
    DigestMismatch,
    SummaryDuplicatesContract,
}

impl std::fmt::Display for TaskServiceAssignmentContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Task Service assignment contract error: {self:?}"
        )
    }
}

impl std::error::Error for TaskServiceAssignmentContractError {}

pub fn validate_task_service_assignment_contract(
    contract: &str,
    expected_sha256: &crate::role_revision::Sha256,
) -> Result<(), TaskServiceAssignmentContractError> {
    if contract.is_empty() {
        return Err(TaskServiceAssignmentContractError::Empty);
    }
    let actual = contract.len();
    if actual > TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES {
        return Err(TaskServiceAssignmentContractError::TooLarge {
            actual,
            maximum: TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES,
        });
    }
    if &crate::task_service::sha256_bytes(contract.as_bytes()) != expected_sha256 {
        return Err(TaskServiceAssignmentContractError::DigestMismatch);
    }
    Ok(())
}

pub fn validate_task_service_assignment_summary(
    summary: &str,
    contract: &str,
) -> Result<(), TaskServiceAssignmentContractError> {
    if summary.contains(contract) {
        return Err(TaskServiceAssignmentContractError::SummaryDuplicatesContract);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageKind {
    #[default]
    Agent,
    User,
    Owner,
    TaskServiceSystem,
}

impl AgentMessageKind {
    pub fn is_agent(&self) -> bool {
        matches!(self, AgentMessageKind::Agent)
    }

    pub fn is_task_service_system(&self) -> bool {
        matches!(self, AgentMessageKind::TaskServiceSystem)
    }

    pub fn sender_label(&self) -> &'static str {
        match self {
            AgentMessageKind::Agent => "cutex",
            AgentMessageKind::User => "user",
            AgentMessageKind::Owner => "owner",
            AgentMessageKind::TaskServiceSystem => "task_service",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceAssignmentMetadata {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    #[serde(default)]
    pub coordinator_cutex_session: Option<crate::role_revision::CutexSessionId>,
    pub assignment_id: crate::task_service::AssignmentId,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub contract_sha256: crate::role_revision::Sha256,
    /// Absent only on assignment messages committed before contract delivery
    /// was introduced. Every newly queued protected message requires `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opaque_contract: Option<String>,
    pub send_attempt_id: crate::task_service::SendAttemptId,
}

impl TaskServiceAssignmentMetadata {
    pub fn validate_contract_if_present(
        &self,
    ) -> Result<Option<&str>, TaskServiceAssignmentContractError> {
        let Some(contract) = self.opaque_contract.as_deref() else {
            return Ok(None);
        };
        validate_task_service_assignment_contract(contract, &self.contract_sha256)?;
        Ok(Some(contract))
    }

    pub fn require_valid_contract(&self) -> Result<&str, TaskServiceAssignmentContractError> {
        self.validate_contract_if_present()?
            .ok_or(TaskServiceAssignmentContractError::Missing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceCompletionMetadata {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub notification_id: crate::task_service::NotificationId,
    pub assignment_id: crate::task_service::AssignmentId,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_number: Option<crate::role_revision::AttemptNumber>,
    pub transition_action_id: crate::task_service::ActionId,
    pub kind: crate::task_service::CompletionNotificationKind,
    pub target_seat_id: crate::task_service::SeatId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceWorkerFollowupMetadata {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub notification_id: crate::task_service::NotificationId,
    pub assignment_id: crate::task_service::AssignmentId,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_number: crate::role_revision::AttemptNumber,
    pub decision_reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UserSubmitMode {
    Queue,
    #[default]
    NextToolCall,
    Interrupt,
}

impl UserSubmitMode {
    pub fn label(&self) -> &'static str {
        match self {
            UserSubmitMode::Queue => "queue",
            UserSubmitMode::NextToolCall => "next_tool_call",
            UserSubmitMode::Interrupt => "interrupt",
        }
    }

    pub fn delivery_boundary(&self) -> &'static str {
        match self {
            UserSubmitMode::Queue => "after_current_turn",
            UserSubmitMode::NextToolCall => "next_model_boundary",
            UserSubmitMode::Interrupt => "interrupt_current_turn",
        }
    }

    pub fn turn_acceptance(&self) -> &'static str {
        match self {
            UserSubmitMode::Queue => "after_turn",
            UserSubmitMode::NextToolCall => "next_boundary",
            UserSubmitMode::Interrupt => "interrupt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentBusEnvelopeKind {
    #[default]
    Message,
    Control,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentRegistrationClass {
    Persistent,
    Ephemeral,
    #[default]
    LocalOnly,
}

impl AgentRegistrationClass {
    pub fn label(&self) -> &'static str {
        match self {
            AgentRegistrationClass::Persistent => "persistent",
            AgentRegistrationClass::Ephemeral => "ephemeral",
            AgentRegistrationClass::LocalOnly => "local_only",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusAgent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub base_name: Option<String>,
    #[serde(default, alias = "thread_name")]
    pub thread_name: Option<String>,
    #[serde(default)]
    pub path_key: Option<String>,
    #[serde(default, alias = "session_id")]
    pub session_id: Option<String>,
    /// Stable durable identity projected only for the current runtime endpoint.
    #[serde(
        default,
        alias = "cutexSessionId",
        skip_serializing_if = "Option::is_none"
    )]
    pub cutex_session_id: Option<String>,
    pub profile: String,
    pub cwd: String,
    pub pid: u32,
    #[serde(default, alias = "hostId", skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, alias = "registration_class")]
    pub registration_class: AgentRegistrationClass,
    pub last_seen_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusRegisterRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub base_name: Option<String>,
    #[serde(default, alias = "thread_name")]
    pub thread_name: Option<String>,
    #[serde(default)]
    pub path_key: Option<String>,
    #[serde(default, alias = "session_id")]
    pub session_id: Option<String>,
    pub profile: String,
    pub cwd: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default, alias = "host_id")]
    pub host_id: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, alias = "registration_class")]
    pub registration_class: AgentRegistrationClass,
}

/// Returns the stable logical recipient bytes used by the ordinary-message
/// semantic digest on both sides of the Agent Bus bridge.
///
/// Runtime IDs and display names may change when a durable Agent is restarted.
/// A nonblank base name is therefore authoritative when the registration
/// provides one. The remaining fallbacks preserve registrations that predate
/// base names without inferring identity from presentation metadata.
pub fn canonical_recipient_label<'a>(
    base_name: Option<&'a str>,
    registration_name: &'a str,
    runtime_id: &'a str,
) -> &'a str {
    base_name
        .filter(|label| !label.trim().is_empty())
        .or_else(|| (!registration_name.trim().is_empty()).then_some(registration_name))
        .unwrap_or(runtime_id)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AgentBusRegistry {
    #[serde(default)]
    pub agents: std::collections::HashMap<String, AgentBusAgent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentGroupUpdateMode {
    #[default]
    Set,
    Add,
    Remove,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusGroupUpdateRequest {
    pub target: String,
    pub groups: Vec<String>,
    #[serde(default)]
    pub mode: AgentGroupUpdateMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusGroupUpdateResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default, alias = "agentId")]
    pub agent_id: Option<String>,
    #[serde(default, alias = "agentName")]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusHeartbeatRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusUnregisterRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusUnregisterResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusMessage {
    pub id: String,
    #[serde(default)]
    pub kind: AgentBusEnvelopeKind,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cutex_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_cutex_session_id: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub delivery_mode: AgentDeliveryMode,
    pub trigger_turn: bool,
    pub created_at_epoch_secs: u64,
    #[serde(default)]
    pub sender_kind: AgentMessageKind,
    #[serde(default)]
    pub display_source: Option<String>,
    #[serde(default)]
    pub submit_mode: Option<UserSubmitMode>,
    #[serde(default, alias = "controlType")]
    pub control_type: Option<String>,
    #[serde(default, alias = "controlPayload")]
    pub control_payload: Option<Value>,
    #[serde(default, alias = "externalActionId")]
    pub external_action_id: Option<String>,
    #[serde(default, alias = "externalMessageId")]
    pub external_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusSendRequest {
    pub to: String,
    #[serde(default, alias = "allGroups")]
    pub all_groups: bool,
    /// Historical send behavior searches Bridgeboard peers unless explicitly disabled.
    #[serde(
        default = "default_agent_bus_send_all_hosts",
        alias = "allHosts",
        skip_serializing_if = "agent_bus_send_all_hosts_is_default"
    )]
    pub all_hosts: bool,
    #[serde(default)]
    pub kind: AgentBusEnvelopeKind,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default, alias = "fromAgentId")]
    pub from_agent_id: Option<String>,
    #[serde(default, alias = "fromSessionId")]
    pub from_session_id: Option<String>,
    #[serde(default, alias = "toSessionId")]
    pub to_session_id: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default, alias = "deliveryMode")]
    pub delivery_mode: Option<AgentDeliveryMode>,
    #[serde(default, alias = "queueOnly")]
    pub queue_only: Option<bool>,
    #[serde(default, alias = "triggerTurn")]
    pub trigger_turn: Option<bool>,
    #[serde(
        default,
        alias = "senderKind",
        alias = "sender_type",
        alias = "senderType"
    )]
    pub sender_kind: Option<AgentMessageKind>,
    #[serde(default, alias = "displaySource")]
    pub display_source: Option<String>,
    #[serde(default, alias = "submitMode")]
    pub submit_mode: Option<UserSubmitMode>,
    #[serde(default, alias = "controlType")]
    pub control_type: Option<String>,
    #[serde(default, alias = "controlPayload")]
    pub control_payload: Option<Value>,
    #[serde(default, alias = "externalActionId")]
    pub external_action_id: Option<String>,
    #[serde(default, alias = "externalMessageId")]
    pub external_message_id: Option<String>,
}

fn default_agent_bus_send_all_hosts() -> bool {
    true
}

fn agent_bus_send_all_hosts_is_default(value: &bool) -> bool {
    *value
}

impl AgentBusSendRequest {
    pub fn resolved_delivery_mode(&self) -> AgentDeliveryMode {
        if let Some(mode) = self.delivery_mode.clone() {
            return mode;
        }
        if self.queue_only == Some(true) {
            return AgentDeliveryMode::Passive;
        }
        match self.trigger_turn {
            Some(false) => AgentDeliveryMode::Passive,
            Some(true) | None => AgentDeliveryMode::Soon,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentBusRecentSend {
    pub id: String,
    pub kind: AgentBusEnvelopeKind,
    pub from: String,
    pub to: String,
    pub to_name: String,
    pub delivery_mode: AgentDeliveryMode,
    pub trigger_turn: bool,
    pub queued: bool,
    pub created_at_epoch_secs: u64,
    pub sender_kind: AgentMessageKind,
    pub display_source: Option<String>,
    pub submit_mode: Option<UserSubmitMode>,
    pub control_type: Option<String>,
    #[allow(dead_code)]
    pub control_payload: Option<Value>,
    pub external_action_id: Option<String>,
    pub external_message_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentBusSendOutcome {
    pub record: AgentBusRecentSend,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusPollResponse {
    pub messages: Vec<AgentBusMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBusAckRequest {
    pub agent_id: String,
    pub message_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBusAckResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub acked: usize,
}

#[derive(Debug, Deserialize)]
pub struct AgentBusSendResponse {
    pub id: String,
    #[serde(default)]
    pub from: Option<String>,
    pub to: String,
    #[serde(default)]
    pub to_name: Option<String>,
    #[serde(default)]
    pub from_session_id: Option<String>,
    #[serde(default)]
    pub to_session_id: Option<String>,
    #[serde(default)]
    pub from_runtime_agent_id: Option<String>,
    #[serde(default)]
    pub to_runtime_agent_id: Option<String>,
    #[serde(default)]
    pub from_cutex_session_id: Option<String>,
    #[serde(default)]
    pub to_cutex_session_id: Option<String>,
    #[serde(default)]
    pub delivery_mode: Option<AgentDeliveryMode>,
    pub trigger_turn: bool,
    pub queued: bool,
    #[serde(default)]
    pub queue_durability: Option<String>,
    #[serde(default)]
    pub delivery_state: Option<String>,
    #[serde(default)]
    pub required_ack_level: Option<String>,
    #[serde(default)]
    pub deduplicated: bool,
    #[serde(default)]
    pub external_action_id: Option<String>,
    #[serde(default)]
    pub external_message_id: Option<String>,
}

pub const TASK_WORKER_ACTION_MAX_BODY_BYTES: usize = 256 * 1024;
pub const TASK_WORKER_RESULT_MAX_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskWorkerActionSchema {
    #[serde(rename = "cutex/task-worker-action/v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerActionKind {
    Accept,
    Start,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskWorkerResult {
    Utf8 {
        text: String,
        sha256: crate::role_revision::Sha256,
    },
    Base64 {
        data: String,
        sha256: crate::role_revision::Sha256,
    },
}

impl TaskWorkerResult {
    pub fn sha256(&self) -> &crate::role_revision::Sha256 {
        match self {
            Self::Utf8 { sha256, .. } | Self::Base64 { sha256, .. } => sha256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerActionRequest {
    pub schema: TaskWorkerActionSchema,
    pub action: TaskWorkerActionKind,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_fence: crate::task_delivery::PilotAttemptFence,
    pub expected_store_revision: crate::role_revision::StoreRevision,
    pub action_id: crate::role_revision::ReceiptId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskWorkerResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskWorkerReconciliationSchema {
    #[serde(rename = "cutex/task-worker-reconciliation/v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TaskWorkerReconciliationOperation {
    Inspect {
        uncertainty_id: crate::role_revision::ReceiptId,
        action_id: crate::role_revision::ReceiptId,
    },
    Ack {
        uncertainty_id: crate::role_revision::ReceiptId,
        action_id: crate::role_revision::ReceiptId,
        resolution_id: crate::role_revision::ReceiptId,
        resolution_sha256: crate::role_revision::Sha256,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerReconciliationRequest {
    pub schema: TaskWorkerReconciliationSchema,
    pub operation: TaskWorkerReconciliationOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskWorkerActionResponseSchema {
    #[serde(rename = "cutex/task-worker-action-response/v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskWorkerReconciliationResponseSchema {
    #[serde(rename = "cutex/task-worker-reconciliation-response/v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerPhase {
    Delivered,
    Accepted,
    Running,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerActionReceipt {
    pub action_id: crate::role_revision::ReceiptId,
    pub task_id: crate::role_revision::TaskId,
    pub task_revision: crate::role_revision::TaskRevision,
    pub attempt_number: crate::role_revision::AttemptNumber,
    pub prior_phase: TaskWorkerPhase,
    pub resulting_phase: TaskWorkerPhase,
    pub committed_store_revision: crate::role_revision::StoreRevision,
    pub committed_at: crate::role_revision::Rfc3339,
    pub transport_record_id: crate::role_revision::ReceiptId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_sha256: Option<crate::role_revision::Sha256>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerReceiptAbsence {
    pub observed_store_revision: crate::role_revision::StoreRevision,
    pub observed_journal_cursor: crate::task_service::JournalCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerCommittedReceiptEvidence {
    pub receipt: TaskWorkerActionReceipt,
    pub request_digest_sha256: crate::role_revision::Sha256,
    pub event_cursor: crate::task_service::JournalCursor,
    pub observed_store_revision: crate::role_revision::StoreRevision,
    pub observed_journal_cursor: crate::task_service::JournalCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerResolutionStatus {
    Committed,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "evidence", rename_all = "snake_case")]
pub enum TaskWorkerResolutionEvidence {
    Committed(TaskWorkerCommittedReceiptEvidence),
    Absent(TaskWorkerReceiptAbsence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerResolution {
    pub resolution_id: crate::role_revision::ReceiptId,
    pub resolution_sha256: crate::role_revision::Sha256,
    pub resolved_at: crate::role_revision::Rfc3339,
    pub evidence: TaskWorkerResolutionEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum TaskWorkerActionNoWrite {
    InvalidBody,
    BodyTooLarge,
    InvalidActionShape,
    ResultTooLarge,
    ResultHashMismatch,
    SenderHeaderMissing,
    SenderNotRegistered,
    FederatedSenderRejected,
    RosterSessionMissing,
    SessionSnapshotUnavailable,
    SessionNotFound,
    SessionIdentityMismatch,
    SessionInactive,
    DurableRevisionMismatch,
    RuntimeAgentMismatch,
    RuntimeGenerationMissing,
    RuntimeGenerationMismatch,
    TaskNotFound,
    StaleFence,
    IllegalPhase,
    ActionConflict,
    UncertaintyBlocked,
    StoreRevisionConflict {
        expected: crate::role_revision::StoreRevision,
        actual: crate::role_revision::StoreRevision,
    },
    RecoveryRequired,
    PersistenceUnavailable,
    DurableRequestRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskWorkerActionOutcome {
    Committed(TaskWorkerActionReceipt),
    NoWrite(TaskWorkerActionNoWrite),
    ReconciliationRequired {
        uncertainty_id: crate::role_revision::ReceiptId,
        action_id: crate::role_revision::ReceiptId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerActionResponse {
    pub schema: TaskWorkerActionResponseSchema,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_id: Option<crate::role_revision::ReceiptId>,
    pub outcome: TaskWorkerActionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskServiceActionResponseSchema {
    #[serde(rename = "cutex/task-service-action-response/v2")]
    V2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskServiceActionOutcome {
    Committed(crate::task_service::ProviderReceipt),
    NoWrite { code: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceActionResponse {
    pub schema: TaskServiceActionResponseSchema,
    pub action_id: crate::task_service::ActionId,
    pub outcome: TaskServiceActionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskServiceQueryResponseSchema {
    #[serde(rename = "cutex/task-service-query-response/v2")]
    V2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskServiceQueryOutcome {
    Snapshot(crate::task_service::TaskServiceSnapshot),
    AssigneeSnapshot(crate::task_service::AssigneeTaskServiceSnapshot),
    Watch(Vec<crate::task_service::WatchEvent>),
    NoWrite { code: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceQueryResponse {
    pub schema: TaskServiceQueryResponseSchema,
    pub outcome: TaskServiceQueryOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskServiceWorkerContextOutcome {
    Context(crate::task_service::WorkerContext),
    NoWrite { code: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceWorkerContextResponse {
    pub schema: crate::task_service::WorkerContextResponseSchema,
    pub outcome: TaskServiceWorkerContextOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskServiceWorkerPrepareOutcome {
    Prepared(crate::task_service::WorkerProviderActionEnvelope),
    Committed(crate::task_service::ProviderReceipt),
    NoWrite { code: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskServiceWorkerPrepareResponse {
    pub schema: crate::task_service::WorkerPrepareResponseSchema,
    pub outcome: TaskServiceWorkerPrepareOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskWorkerReconciliationNoWrite {
    Rejected,
    PersistenceUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
pub enum TaskWorkerReconciliationOutcome {
    Unknown,
    Resolved(TaskWorkerResolution),
    Acknowledged,
    NoWrite(TaskWorkerReconciliationNoWrite),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWorkerReconciliationResponse {
    pub schema: TaskWorkerReconciliationResponseSchema,
    pub outcome: TaskWorkerReconciliationOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_contract_validation_is_exact_utf8_and_byte_bounded() {
        let unicode = "合同 λ 🧭";
        let digest = crate::task_service::sha256_bytes(unicode.as_bytes());
        assert_eq!(
            validate_task_service_assignment_contract(unicode, &digest),
            Ok(())
        );
        assert_eq!(
            validate_task_service_assignment_contract("合同 λ 🧭!", &digest),
            Err(TaskServiceAssignmentContractError::DigestMismatch)
        );

        let boundary = "x".repeat(TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES);
        let boundary_digest = crate::task_service::sha256_bytes(boundary.as_bytes());
        assert_eq!(
            validate_task_service_assignment_contract(&boundary, &boundary_digest),
            Ok(())
        );

        let oversized = format!("{boundary}x");
        let oversized_digest = crate::task_service::sha256_bytes(oversized.as_bytes());
        assert_eq!(
            validate_task_service_assignment_contract(&oversized, &oversized_digest),
            Err(TaskServiceAssignmentContractError::TooLarge {
                actual: TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES + 1,
                maximum: TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES,
            })
        );
        assert_eq!(
            validate_task_service_assignment_summary(
                "preface then exact-contract",
                "exact-contract",
            ),
            Err(TaskServiceAssignmentContractError::SummaryDuplicatesContract)
        );
        assert_eq!(
            validate_task_service_assignment_summary("short summary", "exact-contract"),
            Ok(())
        );
    }

    #[test]
    fn agent_message_kind_default_and_labels_match_wire_compatibility() {
        assert_eq!(AgentMessageKind::default(), AgentMessageKind::Agent);
        assert_eq!(AgentMessageKind::Agent.sender_label(), "cutex");
        assert_eq!(AgentMessageKind::User.sender_label(), "user");
        assert_eq!(AgentMessageKind::Owner.sender_label(), "owner");
        assert_eq!(
            AgentMessageKind::TaskServiceSystem.sender_label(),
            "task_service"
        );
        assert!(AgentMessageKind::Agent.is_agent());
        assert!(!AgentMessageKind::User.is_agent());
        assert!(AgentMessageKind::TaskServiceSystem.is_task_service_system());
    }

    #[test]
    fn agent_registration_class_default_and_labels_match_wire_values() {
        assert_eq!(
            AgentRegistrationClass::default(),
            AgentRegistrationClass::LocalOnly
        );
        assert_eq!(AgentRegistrationClass::Persistent.label(), "persistent");
        assert_eq!(AgentRegistrationClass::Ephemeral.label(), "ephemeral");
        assert_eq!(AgentRegistrationClass::LocalOnly.label(), "local_only");
    }

    #[test]
    fn user_submit_mode_default_and_labels_match_observer_payloads() {
        assert_eq!(UserSubmitMode::default(), UserSubmitMode::NextToolCall);
        assert_eq!(UserSubmitMode::Queue.label(), "queue");
        assert_eq!(
            UserSubmitMode::Queue.delivery_boundary(),
            "after_current_turn"
        );
        assert_eq!(UserSubmitMode::Queue.turn_acceptance(), "after_turn");
        assert_eq!(
            UserSubmitMode::NextToolCall.delivery_boundary(),
            "next_model_boundary"
        );
        assert_eq!(
            UserSubmitMode::Interrupt.delivery_boundary(),
            "interrupt_current_turn"
        );
    }

    #[test]
    fn send_request_resolves_legacy_delivery_mode_fields() {
        let request: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "worker",
            "triggerTurn": false
        }))
        .expect("legacy request should parse");
        assert_eq!(request.resolved_delivery_mode(), AgentDeliveryMode::Passive);
        assert!(
            request.all_hosts,
            "omitted field preserves cross-host behavior"
        );

        let local_only: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "worker",
            "allHosts": false
        }))
        .expect("explicit host scope should parse");
        assert!(!local_only.all_hosts);
        assert_eq!(
            serde_json::to_value(&local_only).expect("serialize host scope")["all_hosts"],
            false
        );

        let request: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "worker",
            "deliveryMode": "after_turn"
        }))
        .expect("deliveryMode request should parse");
        assert_eq!(
            request.resolved_delivery_mode(),
            AgentDeliveryMode::AfterTurn
        );
    }
}
