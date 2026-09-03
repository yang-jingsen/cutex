//! Agent Bus transport adapter for the delivery-only pilot.

use std::fmt;
use std::io;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent_bus::client::agent_bus_http_json;
use crate::agent_bus::delivery::AgentDeliveryMode;
use crate::agent_bus::model::{
    AgentBusEnvelopeKind, AgentBusSendRequest, AgentBusSendResponse, AgentMessageKind,
};
use crate::agent_bus::service::{agent_bus_base_url, agent_bus_port};
use crate::config::env::CUTEX_AGENT_ID_ENV_VAR;
use crate::profiles::model::CodezConfig;
use crate::session::model::{CutexSessionArchiveState, CutexSessionRecord};
use crate::session::store::load_cutex_session_store;
use crate::task_service::sha256_bytes;

use super::{
    CutexSessionId, DeliveryId, PilotAttemptFence, PilotDeliveryRequest, PilotOwnerSnapshot,
    PublishedTask, ReceiptId, RuntimeAgentId, RuntimeGeneration, Sha256,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TaskDeliveryEnvelopeSchema {
    #[serde(rename = "cutex/task-delivery/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDeliveryEnvelopeV1 {
    pub schema: TaskDeliveryEnvelopeSchema,
    pub task_id: super::TaskId,
    pub task_revision: super::TaskRevision,
    pub opaque_contract: String,
    pub contract_sha256: Sha256,
    pub attempt_fence: PilotAttemptFence,
    pub delivery_action_id: DeliveryId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PilotDeliveryMode {
    AfterTurn,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentBusDeliveryReceiptV1 {
    pub delivery_action_id: DeliveryId,
    pub agent_bus_message_id: String,
    pub target_cutex_session_id: CutexSessionId,
    pub target_runtime_agent_id: RuntimeAgentId,
    pub target_runtime_generation: RuntimeGeneration,
    pub delivery_mode: PilotDeliveryMode,
    pub queued: bool,
    pub deduplicated: bool,
    pub envelope_sha256: Sha256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryPreconditionError {
    SessionNotFound,
    SessionIdentityMismatch,
    SessionInactive,
    DurableRevisionMismatch,
    RuntimeAgentMismatch,
    RuntimeGenerationMissing,
    RuntimeGenerationMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentBusDeliveryResponseError {
    InvalidJson,
    DeduplicationStatusMissing,
    EmptyMessageId,
    InvalidMessageId,
    TargetMismatch,
    TargetSessionMismatch,
    DeliveryModeMismatch,
    TriggerBehaviorMismatch,
    NotQueued,
    DeliveryActionMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentBusDeliveryError {
    SessionSnapshotUnavailable,
    Precondition(DeliveryPreconditionError),
    Serialization,
    TransportRejected,
    ResponseRejected(AgentBusDeliveryResponseError),
    ReconciliationRequired,
}

impl fmt::Display for AgentBusDeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "agent bus delivery error: {self:?}")
    }
}

impl std::error::Error for AgentBusDeliveryError {}

/// Transport-adjacent adapter. It never mutates Task Service state and it
/// performs exactly one Agent Bus send after the fresh session fence passes.
pub struct AgentBusAdapter {
    sessions: Arc<dyn SessionSnapshotBoundary>,
    bus: Arc<dyn AgentBusBoundary>,
}

impl AgentBusAdapter {
    pub fn from_config(config: &CodezConfig) -> Self {
        Self {
            sessions: Arc::new(DurableSessionSnapshotBoundary),
            bus: Arc::new(HttpAgentBusBoundary {
                config: config.clone(),
            }),
        }
    }

    pub fn send(
        &self,
        request: &PilotDeliveryRequest,
    ) -> Result<AgentBusDeliveryReceiptV1, AgentBusDeliveryError> {
        let published = &request.published;
        if published.specification.task_id != published.fence.task_id
            || published.specification.task_revision != published.fence.task_revision
        {
            return Err(AgentBusDeliveryError::Precondition(
                DeliveryPreconditionError::SessionIdentityMismatch,
            ));
        }
        let record = self
            .sessions
            .load(&published.fence.owner.cutex_session_id)
            .map_err(|error| match error {
                SessionSnapshotError::Unavailable => {
                    AgentBusDeliveryError::SessionSnapshotUnavailable
                }
                SessionSnapshotError::NotFound => {
                    AgentBusDeliveryError::Precondition(DeliveryPreconditionError::SessionNotFound)
                }
            })?;
        validate_session_snapshot(&published.fence.owner, &record)?;

        let envelope_bytes = delivery_envelope_bytes(published, &request.delivery_action_id)?;
        let envelope_sha256 = sha256_bytes(&envelope_bytes);
        let content =
            String::from_utf8(envelope_bytes).map_err(|_| AgentBusDeliveryError::Serialization)?;
        let send_request = AgentBusSendRequest {
            to: published.fence.owner.runtime_agent_id.as_str().to_string(),
            all_groups: false,
            all_hosts: false,
            kind: AgentBusEnvelopeKind::Message,
            from: Some("cutex-task-delivery".to_string()),
            from_agent_id: std::env::var(CUTEX_AGENT_ID_ENV_VAR)
                .ok()
                .filter(|value| !value.trim().is_empty()),
            from_session_id: None,
            to_session_id: Some(published.fence.owner.cutex_session_id.as_str().to_string()),
            content,
            delivery_mode: Some(AgentDeliveryMode::AfterTurn),
            queue_only: None,
            trigger_turn: Some(true),
            sender_kind: Some(AgentMessageKind::Agent),
            display_source: None,
            submit_mode: None,
            control_type: None,
            control_payload: None,
            external_action_id: None,
            external_message_id: Some(request.delivery_action_id.as_str().to_string()),
        };
        let request_body =
            serde_json::to_vec(&send_request).map_err(|_| AgentBusDeliveryError::Serialization)?;
        let response_body = self
            .bus
            .send_once(&request_body)
            .map_err(|error| match error {
                AgentBusBoundaryError::Rejected => AgentBusDeliveryError::TransportRejected,
                AgentBusBoundaryError::Uncertain => AgentBusDeliveryError::ReconciliationRequired,
            })?;
        validate_response(
            &response_body,
            &published.fence.owner,
            &request.delivery_action_id,
            envelope_sha256,
        )
    }

    #[cfg(test)]
    pub(super) fn with_boundaries(
        sessions: Arc<dyn SessionSnapshotBoundary>,
        bus: Arc<dyn AgentBusBoundary>,
    ) -> Self {
        Self { sessions, bus }
    }
}

pub(crate) fn delivery_envelope_sha256(
    published: &PublishedTask,
    delivery_action_id: &DeliveryId,
) -> Result<Sha256, AgentBusDeliveryError> {
    delivery_envelope_bytes(published, delivery_action_id).map(|bytes| sha256_bytes(&bytes))
}

fn delivery_envelope_bytes(
    published: &PublishedTask,
    delivery_action_id: &DeliveryId,
) -> Result<Vec<u8>, AgentBusDeliveryError> {
    let envelope = TaskDeliveryEnvelopeV1 {
        schema: TaskDeliveryEnvelopeSchema::V1,
        task_id: published.specification.task_id.clone(),
        task_revision: published.specification.task_revision,
        opaque_contract: published.specification.opaque_contract.clone(),
        contract_sha256: published.specification.contract_sha256.clone(),
        attempt_fence: published.fence.clone(),
        delivery_action_id: delivery_action_id.clone(),
    };
    serde_json::to_vec(&envelope).map_err(|_| AgentBusDeliveryError::Serialization)
}

fn validate_session_snapshot(
    expected: &PilotOwnerSnapshot,
    record: &CutexSessionRecord,
) -> Result<(), AgentBusDeliveryError> {
    if record.cutex_session_id != expected.cutex_session_id.as_str() {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::SessionIdentityMismatch,
        ));
    }
    if record.archive_state != CutexSessionArchiveState::Active {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::SessionInactive,
        ));
    }
    if record.durable_revision() != expected.durable_revision.get() {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::DurableRevisionMismatch,
        ));
    }
    if record.current_runtime_agent_id.as_deref() != Some(expected.runtime_agent_id.as_str()) {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::RuntimeAgentMismatch,
        ));
    }
    if record.runtime_generation == 0 {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::RuntimeGenerationMissing,
        ));
    }
    if record.runtime_generation != expected.runtime_generation.get() {
        return Err(AgentBusDeliveryError::Precondition(
            DeliveryPreconditionError::RuntimeGenerationMismatch,
        ));
    }
    Ok(())
}

fn validate_response(
    bytes: &[u8],
    target: &PilotOwnerSnapshot,
    delivery_action_id: &DeliveryId,
    envelope_sha256: Sha256,
) -> Result<AgentBusDeliveryReceiptV1, AgentBusDeliveryError> {
    let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| {
        AgentBusDeliveryError::ResponseRejected(AgentBusDeliveryResponseError::InvalidJson)
    })?;
    if !matches!(raw.get("deduplicated"), Some(serde_json::Value::Bool(_))) {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::DeduplicationStatusMissing,
        ));
    }
    let response: AgentBusSendResponse = serde_json::from_value(raw).map_err(|_| {
        AgentBusDeliveryError::ResponseRejected(AgentBusDeliveryResponseError::InvalidJson)
    })?;
    if response.id.trim().is_empty() {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::EmptyMessageId,
        ));
    }
    if ReceiptId::new(response.id.clone()).is_err() {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::InvalidMessageId,
        ));
    }
    if response.to != target.runtime_agent_id.as_str() {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::TargetMismatch,
        ));
    }
    if response.to_session_id.as_deref() != Some(target.cutex_session_id.as_str()) {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::TargetSessionMismatch,
        ));
    }
    if response.delivery_mode != Some(AgentDeliveryMode::AfterTurn) {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::DeliveryModeMismatch,
        ));
    }
    if !response.trigger_turn {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::TriggerBehaviorMismatch,
        ));
    }
    if !response.queued {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::NotQueued,
        ));
    }
    if response.external_message_id.as_deref() != Some(delivery_action_id.as_str()) {
        return Err(AgentBusDeliveryError::ResponseRejected(
            AgentBusDeliveryResponseError::DeliveryActionMismatch,
        ));
    }
    Ok(AgentBusDeliveryReceiptV1 {
        delivery_action_id: delivery_action_id.clone(),
        agent_bus_message_id: response.id,
        target_cutex_session_id: target.cutex_session_id.clone(),
        target_runtime_agent_id: target.runtime_agent_id.clone(),
        target_runtime_generation: target.runtime_generation,
        delivery_mode: PilotDeliveryMode::AfterTurn,
        queued: response.queued,
        deduplicated: response.deduplicated,
        envelope_sha256,
    })
}

#[derive(Clone, Copy)]
pub(super) enum SessionSnapshotError {
    Unavailable,
    NotFound,
}

pub(super) trait SessionSnapshotBoundary: Send + Sync {
    fn load(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<CutexSessionRecord, SessionSnapshotError>;
}

struct DurableSessionSnapshotBoundary;

impl SessionSnapshotBoundary for DurableSessionSnapshotBoundary {
    fn load(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<CutexSessionRecord, SessionSnapshotError> {
        let store = load_cutex_session_store().map_err(|_| SessionSnapshotError::Unavailable)?;
        store
            .sessions
            .get(cutex_session_id.as_str())
            .cloned()
            .ok_or(SessionSnapshotError::NotFound)
    }
}

#[derive(Clone, Copy)]
pub(super) enum AgentBusBoundaryError {
    Rejected,
    Uncertain,
}

pub(super) trait AgentBusBoundary: Send + Sync {
    fn send_once(&self, request_body: &[u8]) -> Result<Vec<u8>, AgentBusBoundaryError>;
}

struct HttpAgentBusBoundary {
    config: CodezConfig,
}

impl AgentBusBoundary for HttpAgentBusBoundary {
    fn send_once(&self, request_body: &[u8]) -> Result<Vec<u8>, AgentBusBoundaryError> {
        let response = agent_bus_http_json(
            &agent_bus_base_url(agent_bus_port(&self.config)),
            "POST",
            "/api/messages/send",
            self.config.agent_bus_token.as_deref(),
            Some(request_body),
        )
        .map_err(classify_client_error)?;
        serde_json::to_vec(&response).map_err(|_| AgentBusBoundaryError::Uncertain)
    }
}

fn classify_client_error(error: anyhow::Error) -> AgentBusBoundaryError {
    let definitely_not_sent = error.chain().any(|source| {
        source.downcast_ref::<io::Error>().is_some_and(|io_error| {
            matches!(
                io_error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::AddrNotAvailable
                    | io::ErrorKind::NotFound
            )
        })
    });
    if definitely_not_sent {
        AgentBusBoundaryError::Rejected
    } else {
        AgentBusBoundaryError::Uncertain
    }
}
