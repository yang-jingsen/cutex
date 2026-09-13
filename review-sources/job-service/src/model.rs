use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CONTRACT: &str = "cutex/job-service-core/v1";
pub const GRANT_CONTRACT: &str = "cutex/job-execution-grant/v1";
pub const CALLER_GRANT_CONTRACT: &str = "cutex/job-caller-grant/v1";
pub const COMPLETION_CONTRACT: &str = "cutex.job_service.completion.v1";
pub const COMPLETION_CONTRACT_V2: &str = "cutex.job_service.completion.v2";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionWireVersion {
    #[default]
    V1,
    V2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobRequest {
    pub action_id: String,
    pub argv: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub subscriber_cutex_session_id: String,
    pub origin: ExecutionOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionOrigin {
    pub runtime_agent_id: String,
    pub native_thread_id: String,
    pub permission_profile_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersistedJobRequest {
    pub action_id: String,
    pub argument_count: usize,
    pub cwd: String,
    pub environment_names: Vec<String>,
    pub subscriber_cutex_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<ExecutionOrigin>,
}

impl From<&JobRequest> for PersistedJobRequest {
    fn from(request: &JobRequest) -> Self {
        Self {
            action_id: request.action_id.clone(),
            argument_count: request.argv.len(),
            cwd: request.cwd.clone(),
            environment_names: request.environment.keys().cloned().collect(),
            subscriber_cutex_session_id: request.subscriber_cutex_session_id.clone(),
            origin: Some(request.origin.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionGrantPayload {
    pub schema: String,
    pub grant_id: String,
    pub request_sha256: String,
    pub subject_cutex_session_id: String,
    pub operating_system_uid: u32,
    pub cwd: String,
    pub sandbox_state: Value,
    pub launcher_path: String,
    pub launcher_sha256: String,
    pub issued_at_epoch_secs: u64,
    pub expires_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionGrant {
    pub payload: ExecutionGrantPayload,
    pub hmac_sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CallerOperation {
    Query,
    Cancel,
    ReadOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallerGrantPayload {
    pub schema: String,
    pub grant_id: String,
    pub subject_cutex_session_id: String,
    pub operating_system_uid: u32,
    pub operation: CallerOperation,
    pub job_id: String,
    pub issued_at_epoch_secs: u64,
    pub expires_at_epoch_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CallerGrant {
    pub payload: CallerGrantPayload,
    pub hmac_sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    LaunchPending,
    Running,
    Exited,
    Failed,
    Cancelled,
    Interrupted,
    LaunchUnknown,
}

impl JobState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Failed | Self::Cancelled | Self::Interrupted | Self::LaunchUnknown
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StreamSummary {
    pub retained_bytes: u64,
    pub observed_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionObservation {
    pub basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_observed_at_epoch_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_observed_at_epoch_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_run_duration_millis: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionFactsV1 {
    pub facts_version: u8,
    pub action_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionObservation>,
    pub stdout: StreamSummary,
    pub stderr: StreamSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenCompletionRequestV2 {
    pub schema: String,
    pub event_id: String,
    pub job_id: String,
    pub job_revision: u64,
    pub terminal_status: String,
    pub result_sha256: String,
    pub target_cutex_session_id: String,
    pub facts: CompletionFactsV1,
    pub output_reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub schema: String,
    pub job_id: String,
    pub revision: u64,
    pub request_sha256: String,
    pub request: PersistedJobRequest,
    pub state: JobState,
    pub created_at_epoch_secs: u64,
    pub updated_at_epoch_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start_ticks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionObservation>,
    pub stdout: StreamSummary,
    pub stderr: StreamSummary,
    pub output_reference: String,
    #[serde(default)]
    pub completion_delivery: CompletionDeliverySummary,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionDeliveryState {
    #[default]
    Disabled,
    Ready,
    Sending,
    RetryPending,
    AcceptedPending,
    Archived,
    Delivered,
    Orphaned,
    Unavailable,
    AuthorizationFailed,
    Conflict,
    OperatorRequired,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionDeliverySummary {
    pub state: CompletionDeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutboxRecord {
    pub event_id: String,
    pub job_id: String,
    pub job_revision: u64,
    pub subscriber_cutex_session_id: String,
    pub terminal_state: JobState,
    pub result_sha256: String,
    pub output_reference: String,
    /// Absent means the exact legacy v1 request builder and digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_version: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_request_v2: Option<FrozenCompletionRequestV2>,
    pub acknowledged: bool,
    #[serde(default)]
    pub delivery_state: CompletionDeliveryState,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub next_attempt_at_epoch_millis: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_epoch_millis: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<CompletionReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletionReceipt {
    pub schema: String,
    pub status: String,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub disposition: String,
    pub deduplicated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a4_receipt: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubmitReceipt {
    pub status: String,
    pub job: JobRecord,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutputPage {
    pub job_id: String,
    pub stream: String,
    pub from_offset: u64,
    pub next_offset: u64,
    pub bytes_hex: String,
    pub gap: bool,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization failure: {0}")]
    Serde(#[from] serde_json::Error),
}
