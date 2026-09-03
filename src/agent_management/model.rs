use std::fmt;

use serde::{Deserialize, Serialize};

use crate::role_revision::{CutexSessionId, Rfc3339, Sha256};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentActionId(String);

impl AgentActionId {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentManagementError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 256
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
            });
        if !valid {
            return Err(AgentManagementError::InvalidRequest("invalid_action_id"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AgentActionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AgentActionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentManagementSchema {
    #[serde(rename = "cutex/agent-management/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentManagementStoreSchema {
    #[serde(rename = "cutex/agent-management-store/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentManagementReceiptSchema {
    #[serde(rename = "cutex/agent-management-receipt/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentManagementFailureSchema {
    #[serde(rename = "cutex/agent-management-failure/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LegacyDirectorOwnershipImportSchema {
    #[serde(rename = "cutex/agent-management/legacy-director-ownership-import/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum LegacyDirectorOwnershipImportReceiptSchema {
    #[serde(rename = "cutex/agent-management/legacy-director-ownership-import-receipt/v1")]
    V1,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn new(value: impl Into<String>) -> Result<Self, AgentManagementError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 128
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':'));
        if !valid {
            return Err(AgentManagementError::InvalidRequest("invalid_project_id"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStartMode {
    BootstrapOnly,
    CustomMessage,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentReplacePolicy {
    CloseBeforeCreate,
    CloseAfterReady,
    KeepOld,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorRotateMode {
    ClosePredecessorThenCreateWithMessage,
    RetainPredecessorWithMessage,
    RetainPredecessorBootstrapOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentSpec {
    pub name: String,
    pub cwd: String,
    pub profile: String,
    pub runtime_backend: String,
    pub model: String,
    pub reasoning: String,
    pub permissions: String,
    pub approval_policy: String,
    pub sandbox_mode: String,
    pub groups: Vec<String>,
    #[serde(default)]
    pub expose_to_im: bool,
    #[serde(default)]
    pub pin: bool,
}

impl ManagedAgentSpec {
    pub fn validate(&self) -> Result<(), AgentManagementError> {
        for (value, code) in [
            (&self.name, "invalid_agent_name"),
            (&self.cwd, "invalid_agent_cwd"),
            (&self.profile, "invalid_profile"),
            (&self.runtime_backend, "invalid_runtime_backend"),
            (&self.model, "invalid_model"),
            (&self.reasoning, "invalid_reasoning"),
            (&self.permissions, "invalid_permissions"),
            (&self.approval_policy, "invalid_approval_policy"),
            (&self.sandbox_mode, "invalid_sandbox_mode"),
        ] {
            if value.trim().is_empty() {
                return Err(AgentManagementError::InvalidRequest(code));
            }
        }
        let path = std::path::Path::new(&self.cwd);
        if !path.is_absolute() || path.parent().is_none() {
            return Err(AgentManagementError::InvalidRequest("invalid_agent_cwd"));
        }
        let normalized = crate::agent_bus::identity::normalize_agent_groups(self.groups.clone());
        if normalized.is_empty() || normalized != self.groups {
            return Err(AgentManagementError::InvalidRequest("invalid_agent_groups"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentOperation {
    Create {
        spec: ManagedAgentSpec,
        start_mode: AgentStartMode,
        #[serde(default)]
        frozen_message: Option<String>,
    },
    QueryManaged,
    Online {
        cutex_session_id: CutexSessionId,
    },
    Offline {
        cutex_session_id: CutexSessionId,
    },
    Restart {
        cutex_session_id: CutexSessionId,
    },
    Close {
        cutex_session_id: CutexSessionId,
    },
    Replace {
        predecessor_cutex_session_id: CutexSessionId,
        policy: AgentReplacePolicy,
        successor: ManagedAgentSpec,
        start_mode: AgentStartMode,
        #[serde(default)]
        frozen_message: Option<String>,
    },
    DirectorRotate {
        expected_predecessor_cutex_session: CutexSessionId,
        expected_authority_epoch: u64,
        mode: DirectorRotateMode,
        successor: ManagedAgentSpec,
        #[serde(default)]
        frozen_message: Option<String>,
    },
}

impl AgentOperation {
    pub fn kind(&self) -> AgentOperationKind {
        match self {
            Self::Create { .. } => AgentOperationKind::Create,
            Self::QueryManaged => AgentOperationKind::QueryManaged,
            Self::Online { .. } => AgentOperationKind::Online,
            Self::Offline { .. } => AgentOperationKind::Offline,
            Self::Restart { .. } => AgentOperationKind::Restart,
            Self::Close { .. } => AgentOperationKind::Close,
            Self::Replace { .. } => AgentOperationKind::Replace,
            Self::DirectorRotate { .. } => AgentOperationKind::DirectorRotate,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOperationKind {
    Create,
    QueryManaged,
    Online,
    Offline,
    Restart,
    Close,
    Replace,
    DirectorRotate,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentManagementRequest {
    pub schema: AgentManagementSchema,
    pub action_id: AgentActionId,
    /// Optional project selector. This value narrows authority already held by
    /// the authenticated caller; it never grants authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    #[serde(flatten)]
    pub operation: AgentOperation,
}

#[derive(Deserialize)]
struct AgentManagementRequestWire {
    schema: AgentManagementSchema,
    action_id: AgentActionId,
    #[serde(default)]
    project_id: Option<ProjectId>,
    #[serde(flatten)]
    operation: AgentOperation,
}

impl<'de> Deserialize<'de> for AgentManagementRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let object = value.as_object().ok_or_else(|| {
            serde::de::Error::custom("Agent Management request must be an object")
        })?;
        let operation = object
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("operation must be a string"))?;
        let operation_fields: &[&str] = match operation {
            "create" => &["spec", "start_mode", "frozen_message"],
            "query_managed" => &[],
            "online" | "offline" | "restart" | "close" => &["cutex_session_id"],
            "replace" => &[
                "predecessor_cutex_session_id",
                "policy",
                "successor",
                "start_mode",
                "frozen_message",
            ],
            "director_rotate" => &[
                "expected_predecessor_cutex_session",
                "expected_authority_epoch",
                "mode",
                "successor",
                "frozen_message",
            ],
            _ => {
                return Err(serde::de::Error::custom(
                    "unknown Agent Management operation",
                ))
            }
        };
        for field in object.keys() {
            if !matches!(
                field.as_str(),
                "schema" | "action_id" | "project_id" | "operation"
            ) && !operation_fields.contains(&field.as_str())
            {
                return Err(serde::de::Error::custom(format!(
                    "unknown Agent Management request field `{field}`"
                )));
            }
        }
        let wire: AgentManagementRequestWire =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            schema: wire.schema,
            action_id: wire.action_id,
            project_id: wire.project_id,
            operation: wire.operation,
        })
    }
}

impl AgentManagementRequest {
    pub fn validate(&self) -> Result<(), AgentManagementError> {
        match &self.operation {
            AgentOperation::Create {
                spec,
                start_mode,
                frozen_message,
            } => {
                spec.validate()?;
                validate_start(*start_mode, frozen_message.as_deref())
            }
            AgentOperation::QueryManaged
            | AgentOperation::Online { .. }
            | AgentOperation::Offline { .. }
            | AgentOperation::Restart { .. }
            | AgentOperation::Close { .. } => Ok(()),
            AgentOperation::Replace {
                successor,
                start_mode,
                frozen_message,
                ..
            } => {
                successor.validate()?;
                validate_start(*start_mode, frozen_message.as_deref())
            }
            AgentOperation::DirectorRotate {
                expected_authority_epoch,
                mode,
                successor,
                frozen_message,
                ..
            } => {
                if *expected_authority_epoch == 0 {
                    return Err(AgentManagementError::InvalidRequest(
                        "invalid_authority_epoch",
                    ));
                }
                successor.validate()?;
                let start_mode = match mode {
                    DirectorRotateMode::RetainPredecessorBootstrapOnly => {
                        AgentStartMode::BootstrapOnly
                    }
                    DirectorRotateMode::ClosePredecessorThenCreateWithMessage
                    | DirectorRotateMode::RetainPredecessorWithMessage => {
                        AgentStartMode::CustomMessage
                    }
                };
                validate_start(start_mode, frozen_message.as_deref())
            }
        }
    }
}

fn validate_start(mode: AgentStartMode, message: Option<&str>) -> Result<(), AgentManagementError> {
    match (
        mode,
        message.map(str::trim).filter(|value| !value.is_empty()),
    ) {
        (AgentStartMode::BootstrapOnly, None) | (AgentStartMode::CustomMessage, Some(_)) => Ok(()),
        (AgentStartMode::BootstrapOnly, Some(_)) => Err(AgentManagementError::InvalidRequest(
            "bootstrap_only_forbids_custom_message",
        )),
        (AgentStartMode::CustomMessage, None) => Err(AgentManagementError::InvalidRequest(
            "custom_message_requires_frozen_message",
        )),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementInvocation {
    pub caller_cutex_session: CutexSessionId,
    pub caller_runtime_agent_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementMessageMetadata {
    pub schema: AgentManagementSchema,
    pub requested_by_director: CutexSessionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAuthorityRequest {
    pub schema: AgentManagementSchema,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub authorized_director_session: CutexSessionId,
    #[serde(default)]
    pub expected_authorized_director_session: Option<CutexSessionId>,
    #[serde(default)]
    pub expected_authority_epoch: Option<u64>,
}

/// Root-only one-time migration request for a Director whose project authority
/// predates explicit Agent Management ownership. Runtime/spec facts are loaded
/// from the authoritative durable session store and are intentionally absent
/// from this request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyDirectorOwnershipImportRequest {
    pub schema: LegacyDirectorOwnershipImportSchema,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub director_cutex_session_id: CutexSessionId,
    pub expected_authorized_director_session: CutexSessionId,
    pub expected_authority_epoch: u64,
}

/// Provider input produced only by the trusted root administration adapter
/// after exact durable-session lookup and lifecycle validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyDirectorOwnershipEvidence {
    pub director_cutex_session_id: CutexSessionId,
    pub native_session_id: String,
    pub durable_session_revision: u64,
    pub runtime_generation: u64,
    pub spec: ManagedAgentSpec,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAuthority {
    pub project_id: ProjectId,
    pub authorized_director_session: CutexSessionId,
    pub authority_epoch: u64,
    pub updated_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedAgentRecord {
    pub project_id: ProjectId,
    pub created_by_director_session: CutexSessionId,
    pub cutex_session_id: CutexSessionId,
    pub native_session_id: String,
    pub spec: ManagedAgentSpec,
    pub created_at: Rfc3339,
    #[serde(default)]
    pub retired_at: Option<Rfc3339>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRuntimeObservation {
    pub cutex_session_id: CutexSessionId,
    pub native_session_id: String,
    pub active: bool,
    pub cwd: String,
    pub profile: String,
    pub runtime_backend: String,
    pub model: String,
    pub reasoning: String,
    pub permissions: String,
    pub approval_policy: String,
    pub sandbox_mode: String,
    pub groups: Vec<String>,
    pub runtime_generation: u64,
    #[serde(default)]
    pub runtime_agent_ids: Vec<String>,
    pub app_server_runtime: bool,
    #[serde(default)]
    pub agent_bus_endpoint_ids: Vec<String>,
}

/// Provider-owned identity for one durable runtime occurrence.
///
/// Historical actions that predate this evidence may only acquire an
/// all-absent fence after every authoritative runtime source has been checked.
/// The fence is then committed with the reopened action and compared again
/// immediately before any lifecycle effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeOccurrenceFence {
    pub runtime_generation: u64,
    #[serde(default)]
    pub current_runtime_agent_id: Option<String>,
    #[serde(default)]
    pub agent_bus_endpoint_ids: Vec<String>,
    #[serde(default)]
    pub pending_launch_id: Option<String>,
    #[serde(default)]
    pub app_server_launch_claim_id: Option<String>,
    #[serde(default)]
    pub alden_session_name: Option<String>,
    #[serde(default)]
    pub alden_pid: Option<u32>,
    #[serde(default)]
    pub runtime_pid: Option<u32>,
    #[serde(default)]
    pub app_server_pid: Option<u32>,
    #[serde(default)]
    pub app_server_endpoint: Option<String>,
    #[serde(default)]
    pub app_server_connected: bool,
}

impl RuntimeOccurrenceFence {
    pub fn is_proven_absent(&self) -> bool {
        self.current_runtime_agent_id.is_none()
            && self.agent_bus_endpoint_ids.is_empty()
            && self.pending_launch_id.is_none()
            && self.app_server_launch_claim_id.is_none()
            && self.alden_pid.is_none()
            && self.runtime_pid.is_none()
            && self.app_server_pid.is_none()
            && self.app_server_endpoint.is_none()
            && !self.app_server_connected
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActionPhase {
    Prepared,
    PrivateCwdReady,
    NativeBootstrapPending,
    NativeSessionCaptured,
    Adopted,
    Configured,
    Online,
    Ready,
    MessagePending,
    MessageQueued,
    PredecessorClosing,
    PredecessorClosed,
    AuthorityTransferPending,
    AuthorityTransferred,
    SuccessorReady,
    Complete,
    NoWrite,
    OwnerActionRequired,
    Failure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActionRecord {
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub operation: AgentOperationKind,
    pub project_id: ProjectId,
    pub caller_cutex_session: CutexSessionId,
    pub phase: AgentActionPhase,
    #[serde(default)]
    pub phase_sequence: u64,
    #[serde(default)]
    pub reserved_agent_name: Option<String>,
    #[serde(default)]
    pub reserved_agent_cwd: Option<String>,
    #[serde(default)]
    pub known_successor_cutex_session: Option<CutexSessionId>,
    #[serde(default)]
    pub known_native_session_id: Option<String>,
    /// The provider proved that no native session/runtime was created, either
    /// at a pre-spawn boundary or by authoritative historical reconciliation.
    /// It consumes this bit durably before permitting one launch attempt.
    #[serde(default)]
    pub native_bootstrap_retryable: bool,
    /// Exact provider-owned occurrence fence committed before reopening a
    /// historical lifecycle action. Missing legacy evidence never authorizes
    /// an effect by itself.
    #[serde(default)]
    pub historical_runtime_occurrence_fence: Option<RuntimeOccurrenceFence>,
    #[serde(default)]
    pub external_message_id: Option<String>,
    #[serde(default)]
    pub response: Option<AgentManagementResponse>,
    pub created_at: Rfc3339,
    pub updated_at: Rfc3339,
}

/// One exact, durably committed phase of an Agent Management action. This is
/// provider state, not presentation state: consumers may enrich it, but must
/// never infer phases that do not occur here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementPhaseEvent {
    pub event_id: String,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub operation: AgentOperationKind,
    pub phase: AgentActionPhase,
    pub phase_sequence: u64,
    pub committed_at: Rfc3339,
    pub presentation_owner_cutex_session_id: CutexSessionId,
    /// The lifecycle subject is distinct from the Director/TUI presentation
    /// owner. Before a create-like operation has captured a durable session,
    /// the frozen requested name remains the authoritative display identity.
    #[serde(default)]
    pub subject_cutex_session_id: Option<CutexSessionId>,
    #[serde(default)]
    pub subject_agent_name: Option<String>,
    #[serde(default)]
    pub predecessor_cutex_session_id: Option<CutexSessionId>,
    #[serde(default)]
    pub successor_cutex_session_id: Option<CutexSessionId>,
    #[serde(default)]
    pub replace_policy: Option<AgentReplacePolicy>,
    #[serde(default)]
    pub rotation_mode: Option<DirectorRotateMode>,
    #[serde(default)]
    pub authority_epoch: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentManagementResult {
    Created {
        agent: ManagedAgentRecord,
        observation: AgentRuntimeObservation,
        #[serde(default)]
        message_id: Option<String>,
    },
    QueryManaged {
        authority: ProjectAuthority,
        agents: Vec<ManagedAgentRecord>,
    },
    Lifecycle {
        agent: ManagedAgentRecord,
        observation: AgentRuntimeObservation,
    },
    Replaced {
        predecessor_cutex_session_id: CutexSessionId,
        successor: ManagedAgentRecord,
        observation: AgentRuntimeObservation,
        #[serde(default)]
        message_id: Option<String>,
    },
    DirectorRotated {
        predecessor_cutex_session_id: CutexSessionId,
        successor: ManagedAgentRecord,
        observation: AgentRuntimeObservation,
        authority: ProjectAuthority,
        #[serde(default)]
        message_id: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementReceipt {
    pub schema: AgentManagementReceiptSchema,
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub operation: AgentOperationKind,
    pub project_id: ProjectId,
    pub completed_at: Rfc3339,
    pub result: AgentManagementResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureRoutingStatus {
    Routable,
    Unrouted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementFailureEvent {
    pub schema: AgentManagementFailureSchema,
    pub event_id: String,
    pub action_id: AgentActionId,
    pub project_id: ProjectId,
    pub operation: AgentOperationKind,
    pub code: String,
    pub detail: String,
    pub routing_status: FailureRoutingStatus,
    #[serde(default)]
    pub route_to_director_session: Option<CutexSessionId>,
    /// Exact durable target from an authenticated lifecycle request, when the
    /// request already contained one. Pre-identity operations intentionally
    /// leave this absent rather than fabricating a timeline identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_cutex_session_id: Option<CutexSessionId>,
    pub created_at: Rfc3339,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentManagementOutcome {
    Complete {
        receipt: AgentManagementReceipt,
    },
    NoWrite {
        code: String,
        detail: String,
    },
    OwnerActionRequired {
        failure: AgentManagementFailureEvent,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementResponse {
    pub schema: AgentManagementSchema,
    pub action_id: AgentActionId,
    pub outcome: AgentManagementOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAuthorityReceipt {
    pub schema: AgentManagementReceiptSchema,
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub authority: ProjectAuthority,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectAuthorityOutcome {
    Complete { receipt: ProjectAuthorityReceipt },
    NoWrite { code: String, detail: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAuthorityResponse {
    pub schema: AgentManagementSchema,
    pub action_id: AgentActionId,
    pub outcome: ProjectAuthorityOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyDirectorOwnershipImportReceipt {
    pub schema: LegacyDirectorOwnershipImportReceiptSchema,
    pub action_id: AgentActionId,
    pub request_sha256: Sha256,
    pub authority: ProjectAuthority,
    pub agent: ManagedAgentRecord,
    pub durable_session_revision: u64,
    pub runtime_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum LegacyDirectorOwnershipImportOutcome {
    Complete {
        receipt: LegacyDirectorOwnershipImportReceipt,
        replayed: bool,
    },
    NoWrite {
        code: String,
        detail: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyDirectorOwnershipImportResponse {
    pub schema: LegacyDirectorOwnershipImportSchema,
    pub action_id: AgentActionId,
    pub outcome: LegacyDirectorOwnershipImportOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentManagementError {
    InvalidRequest(&'static str),
    Unauthorized,
    NotAuthorizedDirector,
    ProjectSelectionRequired,
    ProjectNotAuthorized,
    NotFound(&'static str),
    Conflict(&'static str),
    OwnerActionRequired(String),
    PersistenceUnavailable,
    InvalidStore,
    External(String),
}

impl AgentManagementError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::Unauthorized => "unauthorized",
            Self::NotAuthorizedDirector => "not_authorized_director",
            Self::ProjectSelectionRequired => "project_selection_required",
            Self::ProjectNotAuthorized => "project_not_authorized",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::OwnerActionRequired(_) => "owner_action_required",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::InvalidStore => "invalid_store",
            Self::External(_) => "external_failure",
        }
    }
}

impl fmt::Display for AgentManagementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(reason) | Self::NotFound(reason) | Self::Conflict(reason) => {
                write!(formatter, "{}: {reason}", self.code())
            }
            Self::OwnerActionRequired(reason) | Self::External(reason) => {
                write!(formatter, "{}: {reason}", self.code())
            }
            _ => formatter.write_str(self.code()),
        }
    }
}

impl std::error::Error for AgentManagementError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_action_body_allows_only_an_optional_non_authoritative_project_selector() {
        let mut value = serde_json::json!({
            "schema": "cutex/agent-management/v1",
            "action_id": "query-1",
            "operation": "query_managed"
        });
        assert!(serde_json::from_value::<AgentManagementRequest>(value.clone()).is_ok());
        value["project_id"] = serde_json::json!("cutex-project");
        assert!(serde_json::from_value::<AgentManagementRequest>(value.clone()).is_ok());
        for forbidden in [
            "caller_cutex_session",
            "caller_runtime_agent_id",
            "authorized_director_session",
            "authority_epoch",
            "seat",
        ] {
            value[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<AgentManagementRequest>(value.clone()).is_err());
            value.as_object_mut().unwrap().remove(forbidden);
        }
    }

    #[test]
    fn project_selector_is_strict_when_present() {
        assert!(ProjectId::new("project:alpha-1").is_ok());
        for invalid in ["", "project alpha", "../alpha", "/absolute"] {
            assert!(ProjectId::new(invalid).is_err());
            assert!(serde_json::from_value::<ProjectId>(serde_json::json!(invalid)).is_err());
        }
    }

    #[test]
    fn custom_message_metadata_preserves_typed_director_provenance() {
        let metadata = AgentManagementMessageMetadata {
            schema: AgentManagementSchema::V1,
            requested_by_director: CutexSessionId::new("cutex.director-r11").unwrap(),
        };
        let encoded = serde_json::to_value(&metadata).unwrap();
        assert_eq!(encoded["schema"], "cutex/agent-management/v1");
        assert_eq!(encoded["requested_by_director"], "cutex.director-r11");
        assert_eq!(
            serde_json::from_value::<AgentManagementMessageMetadata>(encoded).unwrap(),
            metadata
        );
        assert!(
            serde_json::from_value::<AgentManagementMessageMetadata>(serde_json::json!({
                "schema": "cutex/agent-management/v1",
                "requested_by_director": "cutex.director-r11",
                "authority": "system"
            }))
            .is_err()
        );
    }

    #[test]
    fn legacy_director_ownership_import_request_is_closed_and_contains_only_cas_identity() {
        let value = serde_json::json!({
            "schema": "cutex/agent-management/legacy-director-ownership-import/v1",
            "action_id": "import-legacy-director-01",
            "project_id": "cutex-project",
            "director_cutex_session_id": "cutex.director-r2",
            "expected_authorized_director_session": "cutex.director-r2",
            "expected_authority_epoch": 7
        });
        let request = serde_json::from_value::<LegacyDirectorOwnershipImportRequest>(value.clone())
            .expect("strict import request");
        assert_eq!(request.expected_authority_epoch, 7);
        for forbidden in [
            "spec",
            "native_session_id",
            "runtime_agent_id",
            "groups",
            "cwd",
            "prose",
        ] {
            let mut changed = value.clone();
            changed[forbidden] = serde_json::json!("forged");
            assert!(
                serde_json::from_value::<LegacyDirectorOwnershipImportRequest>(changed).is_err(),
                "accepted forbidden field {forbidden}"
            );
        }
    }
}
