//! Strict Release-only rotation contracts.

use serde::{Deserialize, Serialize};

use crate::agent_bus::model::AgentRegistrationClass;
use crate::role_revision::{CutexSessionId, Rfc3339, Sha256};
use crate::session::model::{CutexSessionQuickActionMode, CutexSessionRuntimeBackend};
use crate::task_service::{ActionId, SeatId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseTemplateSchema {
    #[serde(rename = "cutex/release-template/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseTemplateCommandSchema {
    #[serde(rename = "cutex/release-template-command/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseRotationCommandSchema {
    #[serde(rename = "cutex/release-rotation-command/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseRotationRecordSchema {
    #[serde(rename = "cutex/release-rotation-record/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseRotationReceiptSchema {
    #[serde(rename = "cutex/release-rotation-receipt/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseRotationResponseSchema {
    #[serde(rename = "cutex/release-rotation-response/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRolePackage {
    pub reference: String,
    pub sha256: Sha256,
}

/// Owner-controlled durable defaults for exactly one new Release session.
///
/// Volatile runtime identity, PID/generation and all predecessor thread data
/// are deliberately absent from this contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTemplate {
    pub schema: ReleaseTemplateSchema,
    pub version: u64,
    pub successor_name: String,
    pub cwd: String,
    pub managed_cwd: Option<String>,
    pub runtime_backend: CutexSessionRuntimeBackend,
    pub role_package: ReleaseRolePackage,
    pub agent_groups: Vec<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub permissions: Option<String>,
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub exposed_to_backend: bool,
    pub quick_action: CutexSessionQuickActionMode,
    pub registration_class: AgentRegistrationClass,
    pub default_cli_args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureReleaseTemplateRequest {
    pub schema: ReleaseTemplateCommandSchema,
    pub action_id: ActionId,
    pub expected_current_version: Option<u64>,
    pub template: ReleaseTemplate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTemplateReceipt {
    pub action_id: ActionId,
    pub request_sha256: Sha256,
    pub template_version: u64,
    pub template_sha256: Sha256,
    pub configured_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRotationRequest {
    pub schema: ReleaseRotationCommandSchema,
    pub action_id: ActionId,
    pub target_seat: SeatId,
    pub expected_predecessor_cutex_session: CutexSessionId,
    pub expected_seat_epoch: u64,
    pub expected_template_version: u64,
    pub expected_template_sha256: Sha256,
    pub starting_message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRotationStatus {
    Running,
    Blocked,
    Complete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRotationBoundary {
    SeatRevoked,
    PredecessorOfflined,
    PredecessorRetired,
    SuccessorSessionCreated,
    SuccessorThreadStarted,
    SuccessorRuntimeOnline,
    SuccessorBound,
    DirectorMessageDelivered,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseRotationExternalStep {
    OfflinePredecessor,
    RetirePredecessor,
    CreateSuccessorSession,
    StartSuccessorThread,
    LaunchSuccessorRuntime,
    DeliverDirectorMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRotationRecord {
    pub schema: ReleaseRotationRecordSchema,
    pub action_id: ActionId,
    pub request_sha256: Sha256,
    pub director_cutex_session: CutexSessionId,
    pub target_seat: SeatId,
    pub predecessor_cutex_session: CutexSessionId,
    pub predecessor_seat_epoch: u64,
    pub template: ReleaseTemplate,
    pub template_sha256: Sha256,
    pub starting_message: String,
    pub status: ReleaseRotationStatus,
    pub completed_boundary: ReleaseRotationBoundary,
    pub pending_external_step: Option<ReleaseRotationExternalStep>,
    pub successor_cutex_session: Option<CutexSessionId>,
    pub successor_thread_id: Option<String>,
    pub successor_seat_epoch: Option<u64>,
    pub delivered_message_id: Option<String>,
    pub blocked_reason: Option<String>,
    pub created_at: Rfc3339,
    pub updated_at: Rfc3339,
    pub completed_at: Option<Rfc3339>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRotationReceipt {
    pub schema: ReleaseRotationReceiptSchema,
    pub action_id: ActionId,
    pub request_sha256: Sha256,
    pub status: ReleaseRotationStatus,
    pub completed_boundary: ReleaseRotationBoundary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_external_step: Option<ReleaseRotationExternalStep>,
    pub predecessor_cutex_session: CutexSessionId,
    pub predecessor_seat_epoch: u64,
    pub successor_cutex_session: Option<CutexSessionId>,
    pub successor_thread_id: Option<String>,
    pub successor_seat_epoch: Option<u64>,
    pub template_version: u64,
    pub template_sha256: Sha256,
    pub delivered_message_id: Option<String>,
    pub blocked_reason: Option<String>,
    pub updated_at: Rfc3339,
}

impl From<&ReleaseRotationRecord> for ReleaseRotationReceipt {
    fn from(record: &ReleaseRotationRecord) -> Self {
        Self {
            schema: ReleaseRotationReceiptSchema::V1,
            action_id: record.action_id.clone(),
            request_sha256: record.request_sha256.clone(),
            status: record.status,
            completed_boundary: record.completed_boundary,
            pending_external_step: record.pending_external_step,
            predecessor_cutex_session: record.predecessor_cutex_session.clone(),
            predecessor_seat_epoch: record.predecessor_seat_epoch,
            successor_cutex_session: record.successor_cutex_session.clone(),
            successor_thread_id: record.successor_thread_id.clone(),
            successor_seat_epoch: record.successor_seat_epoch,
            template_version: record.template.version,
            template_sha256: record.template_sha256.clone(),
            delivered_message_id: record.delivered_message_id.clone(),
            blocked_reason: record.blocked_reason.clone(),
            updated_at: record.updated_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetryReleaseRotationRequest {
    pub schema: ReleaseRotationCommandSchema,
    pub action_id: ActionId,
    pub expected_request_sha256: Sha256,
    pub expected_completed_boundary: ReleaseRotationBoundary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_pending_external_step: Option<ReleaseRotationExternalStep>,
    #[serde(default)]
    pub corrected_successor_cutex_session: Option<CutexSessionId>,
    #[serde(default)]
    pub corrected_successor_thread_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseRotationOutcome {
    Complete { receipt: ReleaseRotationReceipt },
    Blocked { receipt: ReleaseRotationReceipt },
    NoWrite { code: String, reason: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRotationResponse {
    pub schema: ReleaseRotationResponseSchema,
    pub action_id: ActionId,
    pub outcome: ReleaseRotationOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRotationInvocation {
    pub director_cutex_session: CutexSessionId,
    pub director_runtime_agent_id: String,
    pub predecessor_has_nonterminal_assignment: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementReleaseRotationRequest {
    pub invocation: ReleaseRotationInvocation,
    pub request: ReleaseRotationRequest,
}
