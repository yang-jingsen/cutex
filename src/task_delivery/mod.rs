//! Curated in-process task-delivery and bounded worker-action pilot.
//!
//! The public boundary exposes only the direct durable path through
//! `completed`. The generic transition graph and all other phases remain
//! crate-private implementation details.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::task_service::{
    canonical_command_digest, sha256_bytes, AttemptFence, CreateDraftCommand, DeliveryCommand,
    EnvelopeSchema, PublishCommand, QuerySchema, ReceiptQuery, ReceiptSchema, StoreRevision,
    TaskCommand, TaskPhase, TaskQuery, TaskRecord, TaskService, TaskServiceError,
    TaskSpecification, TransitionEnvelope, TransitionOutcome, TransitionResponse,
};

pub mod agent_bus_adapter;
pub mod provider_adapter;
pub mod worker_action_adapter;

pub use provider_adapter::{
    AssignmentDispatchError, AssignmentDispatchOutcome, TaskServiceAgentBusDispatcher,
};

pub use crate::role_revision::{
    AttemptNumber, CutexSessionId, DeliveryId, DurableRevision, ReceiptId, RuntimeAgentId,
    RuntimeGeneration, Sha256, StoreRevision as PilotStoreRevision, TaskId, TaskRevision,
};
pub use crate::task_service::LegacyAttemptToken as AttemptToken;
pub use agent_bus_adapter::{
    AgentBusDeliveryError, AgentBusDeliveryReceiptV1, AgentBusDeliveryResponseError,
    DeliveryPreconditionError, PilotDeliveryMode, TaskDeliveryEnvelopeSchema,
    TaskDeliveryEnvelopeV1,
};
pub use worker_action_adapter::TaskWorkerActionAdapter;
pub(crate) use worker_action_adapter::{
    validate_task_worker_action_request, TaskWorkerAuthorizedAction, TaskWorkerAuthorizedOwner,
    TaskWorkerRosterSender, TaskWorkerTransitionResult,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotTaskSpecification {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotOwnerSnapshot {
    pub cutex_session_id: CutexSessionId,
    pub durable_revision: DurableRevision,
    pub runtime_agent_id: RuntimeAgentId,
    pub runtime_generation: RuntimeGeneration,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotAttemptFence {
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: AttemptNumber,
    pub attempt_token: AttemptToken,
    pub owner: PilotOwnerSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotPublishRequest {
    pub specification: PilotTaskSpecification,
    pub create_receipt_id: ReceiptId,
    pub publish_receipt_id: ReceiptId,
    pub expected_store_revision: PilotStoreRevision,
    pub attempt_token: AttemptToken,
    pub owner: PilotOwnerSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedTask {
    specification: PilotTaskSpecification,
    fence: PilotAttemptFence,
    publication_receipt_id: ReceiptId,
    committed_store_revision: PilotStoreRevision,
}

impl PublishedTask {
    pub fn specification(&self) -> &PilotTaskSpecification {
        &self.specification
    }

    pub fn fence(&self) -> &PilotAttemptFence {
        &self.fence
    }

    pub fn publication_receipt_id(&self) -> &ReceiptId {
        &self.publication_receipt_id
    }

    pub fn committed_store_revision(&self) -> PilotStoreRevision {
        self.committed_store_revision
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotDeliveryRequest {
    published: PublishedTask,
    delivery_action_id: DeliveryId,
    transition_receipt_id: ReceiptId,
}

impl PilotDeliveryRequest {
    pub fn new(
        published: PublishedTask,
        delivery_action_id: DeliveryId,
        transition_receipt_id: ReceiptId,
    ) -> Self {
        Self {
            published,
            delivery_action_id,
            transition_receipt_id,
        }
    }

    pub fn published(&self) -> &PublishedTask {
        &self.published
    }

    pub fn delivery_action_id(&self) -> &DeliveryId {
        &self.delivery_action_id
    }

    pub fn transition_receipt_id(&self) -> &ReceiptId {
        &self.transition_receipt_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveredTask {
    published: PublishedTask,
    transition_receipt_id: ReceiptId,
    delivery_receipt: AgentBusDeliveryReceiptV1,
    committed_store_revision: PilotStoreRevision,
}

impl DeliveredTask {
    pub fn published(&self) -> &PublishedTask {
        &self.published
    }

    pub fn transition_receipt_id(&self) -> &ReceiptId {
        &self.transition_receipt_id
    }

    pub fn delivery_receipt(&self) -> &AgentBusDeliveryReceiptV1 {
        &self.delivery_receipt
    }

    pub fn committed_store_revision(&self) -> PilotStoreRevision {
        self.committed_store_revision
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PilotTaskPhase {
    Draft,
    Published,
    Delivered,
    Accepted,
    Running,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotTaskSnapshot {
    pub specification: PilotTaskSpecification,
    pub phase: PilotTaskPhase,
    pub fence: Option<PilotAttemptFence>,
    pub agent_bus_message_id: Option<ReceiptId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PilotReceipt {
    pub receipt_id: ReceiptId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub prior_phase: Option<PilotTaskPhase>,
    pub resulting_phase: PilotTaskPhase,
    pub committed_store_revision: PilotStoreRevision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PilotWorkerReceiptRecord {
    pub request_digest_sha256: Sha256,
    pub receipt_id: ReceiptId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub attempt_number: AttemptNumber,
    pub prior_phase: PilotTaskPhase,
    pub resulting_phase: PilotTaskPhase,
    pub committed_store_revision: PilotStoreRevision,
    pub committed_at: crate::role_revision::Rfc3339,
    pub event_cursor: crate::task_service::JournalCursor,
    pub observed_store_revision: PilotStoreRevision,
    pub observed_journal_cursor: crate::task_service::JournalCursor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PilotWorkerReceiptObservation {
    Committed(PilotWorkerReceiptRecord),
    Absent {
        observed_store_revision: PilotStoreRevision,
        observed_journal_cursor: crate::task_service::JournalCursor,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PilotValidationError {
    SpecificationHashMismatch,
    InconsistentPublishedTask,
    DeliveryReceiptMismatch,
    InvalidAgentBusMessageId,
    DurableRequestRejected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PilotError {
    RecoveryRequired,
    InvalidRequest(PilotValidationError),
    StoreRevisionConflict {
        expected: PilotStoreRevision,
        actual: PilotStoreRevision,
    },
    ReceiptConflict {
        receipt_id: ReceiptId,
    },
    TaskNotFound,
    StaleAttempt,
    IllegalPilotPhase,
    PersistenceUnavailable,
    ReconciliationRequired {
        receipt_id: ReceiptId,
    },
}

impl fmt::Display for PilotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "task delivery pilot error: {self:?}")
    }
}

impl std::error::Error for PilotError {}

/// One-process coordinator for the only stabilized Stage 2 path.
pub struct TaskDeliveryPilot {
    service: TaskService,
    recovered: AtomicBool,
}

impl TaskDeliveryPilot {
    /// Opens and descriptor-binds the caller-owned private root without
    /// performing recovery. Call [`Self::recover`] before every other method.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, PilotError> {
        Ok(Self {
            service: TaskService::new(root).map_err(map_service_error)?,
            recovered: AtomicBool::new(false),
        })
    }

    /// Replays or causally recovers the durable store at process startup.
    pub fn recover(&self) -> Result<(), PilotError> {
        self.service.recover().map_err(map_service_error)?;
        self.recovered.store(true, Ordering::Release);
        Ok(())
    }

    /// Commits deterministic `create_draft` and `publish` operations.
    pub fn publish(&self, request: PilotPublishRequest) -> Result<PublishedTask, PilotError> {
        self.require_recovered()?;
        validate_publish_request(&request)?;

        let create = signed_envelope(
            request.create_receipt_id.clone(),
            request.expected_store_revision,
            None,
            TaskCommand::CreateDraft(CreateDraftCommand {
                specification: TaskSpecification {
                    schema: crate::task_service::SpecificationSchema::V1,
                    task_id: request.specification.task_id.clone(),
                    task_revision: request.specification.task_revision,
                    contract_sha256: request.specification.contract_sha256.clone(),
                    opaque_contract: request.specification.opaque_contract.clone(),
                },
            }),
        )?;
        let create_response =
            committed_transition(self.service.transition(&create), &request.create_receipt_id)?;
        if create_response.resulting_phase != TaskPhase::Draft
            || create_response.task_id != request.specification.task_id
            || create_response.task_revision != request.specification.task_revision
        {
            return Err(PilotError::InvalidRequest(
                PilotValidationError::DurableRequestRejected,
            ));
        }

        let publish = signed_envelope(
            request.publish_receipt_id.clone(),
            create_response.committed_store_revision,
            None,
            TaskCommand::Publish(PublishCommand {
                task_id: request.specification.task_id.clone(),
                task_revision: request.specification.task_revision,
                attempt_token: request.attempt_token,
                owner_session_id: request.owner.cutex_session_id,
                owner_durable_revision: request.owner.durable_revision,
                runtime_generation: request.owner.runtime_generation,
                runtime_agent_id: request.owner.runtime_agent_id,
            }),
        )?;
        let publish_response = committed_transition(
            self.service.transition(&publish),
            &request.publish_receipt_id,
        )?;
        if publish_response.resulting_phase != TaskPhase::Published {
            return Err(PilotError::InvalidRequest(
                PilotValidationError::DurableRequestRejected,
            ));
        }
        let record = self
            .service
            .query_task(&TaskQuery {
                schema: QuerySchema::V1,
                task_id: publish_response.task_id,
                task_revision: Some(publish_response.task_revision),
            })
            .map_err(map_service_error)?
            .ok_or(PilotError::TaskNotFound)?;
        published_from_record(record, publish_response.committed_store_revision)
    }

    /// Records only a validated fenced `published -> delivered` transition.
    pub fn deliver(
        &self,
        request: PilotDeliveryRequest,
        receipt: AgentBusDeliveryReceiptV1,
    ) -> Result<DeliveredTask, PilotError> {
        self.require_recovered()?;
        validate_published(&request.published)?;
        validate_delivery_receipt(&request, &receipt)?;
        let external_receipt_id =
            ReceiptId::new(receipt.agent_bus_message_id.clone()).map_err(|_| {
                PilotError::InvalidRequest(PilotValidationError::InvalidAgentBusMessageId)
            })?;
        let fence = task_service_fence(&request.published.fence);
        let envelope = signed_envelope(
            request.transition_receipt_id.clone(),
            request.published.committed_store_revision,
            Some(fence),
            TaskCommand::RecordDelivery(DeliveryCommand {
                external_delivery_receipt_id: external_receipt_id,
                observed_at: None,
            }),
        )?;
        let response = committed_transition(
            self.service.transition(&envelope),
            &request.transition_receipt_id,
        )?;
        if response.resulting_phase != TaskPhase::Delivered
            || response.task_id != request.published.specification.task_id
            || response.task_revision != request.published.specification.task_revision
        {
            return Err(PilotError::InvalidRequest(
                PilotValidationError::DurableRequestRejected,
            ));
        }
        Ok(DeliveredTask {
            published: request.published,
            transition_receipt_id: request.transition_receipt_id,
            delivery_receipt: receipt,
            committed_store_revision: response.committed_store_revision,
        })
    }

    pub fn task(
        &self,
        task_id: TaskId,
        task_revision: TaskRevision,
    ) -> Result<Option<PilotTaskSnapshot>, PilotError> {
        self.require_recovered()?;
        self.service
            .query_task(&TaskQuery {
                schema: QuerySchema::V1,
                task_id,
                task_revision: Some(task_revision),
            })
            .map_err(map_service_error)?
            .map(snapshot_from_record)
            .transpose()
    }

    pub fn receipt(&self, receipt_id: ReceiptId) -> Result<Option<PilotReceipt>, PilotError> {
        self.require_recovered()?;
        self.service
            .query_receipt(&ReceiptQuery {
                schema: QuerySchema::V1,
                receipt_id,
            })
            .map_err(map_service_error)?
            .map(|record| {
                if record.schema != ReceiptSchema::V1 {
                    return Err(PilotError::InvalidRequest(
                        PilotValidationError::DurableRequestRejected,
                    ));
                }
                Ok(PilotReceipt {
                    receipt_id: record.receipt_id,
                    task_id: record.response.task_id,
                    task_revision: record.response.task_revision,
                    prior_phase: record.response.prior_phase.map(pilot_phase).transpose()?,
                    resulting_phase: pilot_phase(record.response.resulting_phase)?,
                    committed_store_revision: record.response.committed_store_revision,
                })
            })
            .transpose()
    }

    /// Loads exactly one Task Service snapshot and projects only the named
    /// worker receipt or a cursor-bound absence. No write authority escapes.
    pub(crate) fn inspect_worker_receipt(
        &self,
        receipt_id: &ReceiptId,
    ) -> Result<PilotWorkerReceiptObservation, PilotError> {
        self.require_recovered()?;
        let store = self.service.load().map_err(map_service_error)?;
        let observed_store_revision = store.store_revision;
        let observed_journal_cursor = store.journal_checkpoint.clone();
        let Some(record) = store.receipts.get(receipt_id) else {
            return Ok(PilotWorkerReceiptObservation::Absent {
                observed_store_revision,
                observed_journal_cursor,
            });
        };
        if record.schema != ReceiptSchema::V1
            || record.receipt_id != *receipt_id
            || record.response.receipt_id != *receipt_id
        {
            return Err(PilotError::InvalidRequest(
                PilotValidationError::DurableRequestRejected,
            ));
        }
        Ok(PilotWorkerReceiptObservation::Committed(
            PilotWorkerReceiptRecord {
                request_digest_sha256: record.request_digest_sha256.clone(),
                receipt_id: record.receipt_id.clone(),
                task_id: record.response.task_id.clone(),
                task_revision: record.response.task_revision,
                attempt_number: record.response.attempt_number.ok_or_else(|| {
                    PilotError::InvalidRequest(PilotValidationError::DurableRequestRejected)
                })?,
                prior_phase: pilot_phase(record.response.prior_phase.ok_or_else(|| {
                    PilotError::InvalidRequest(PilotValidationError::DurableRequestRejected)
                })?)?,
                resulting_phase: pilot_phase(record.response.resulting_phase)?,
                committed_store_revision: record.response.committed_store_revision,
                committed_at: record.response.committed_at.clone(),
                event_cursor: record.event_cursor.clone(),
                observed_store_revision,
                observed_journal_cursor,
            },
        ))
    }

    fn require_recovered(&self) -> Result<(), PilotError> {
        if self.recovered.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(PilotError::RecoveryRequired)
        }
    }
}

fn validate_publish_request(request: &PilotPublishRequest) -> Result<(), PilotError> {
    if sha256_bytes(request.specification.opaque_contract.as_bytes())
        != request.specification.contract_sha256
    {
        return Err(PilotError::InvalidRequest(
            PilotValidationError::SpecificationHashMismatch,
        ));
    }
    Ok(())
}

fn validate_published(published: &PublishedTask) -> Result<(), PilotError> {
    if published.specification.task_id != published.fence.task_id
        || published.specification.task_revision != published.fence.task_revision
    {
        return Err(PilotError::InvalidRequest(
            PilotValidationError::InconsistentPublishedTask,
        ));
    }
    Ok(())
}

fn validate_delivery_receipt(
    request: &PilotDeliveryRequest,
    receipt: &AgentBusDeliveryReceiptV1,
) -> Result<(), PilotError> {
    let expected_envelope_sha256 = agent_bus_adapter::delivery_envelope_sha256(
        &request.published,
        &request.delivery_action_id,
    )
    .map_err(|_| PilotError::InvalidRequest(PilotValidationError::DeliveryReceiptMismatch))?;
    let fence = &request.published.fence;
    if receipt.delivery_action_id != request.delivery_action_id
        || receipt.target_cutex_session_id != fence.owner.cutex_session_id
        || receipt.target_runtime_agent_id != fence.owner.runtime_agent_id
        || receipt.target_runtime_generation != fence.owner.runtime_generation
        || receipt.delivery_mode != PilotDeliveryMode::AfterTurn
        || !receipt.queued
        || receipt.envelope_sha256 != expected_envelope_sha256
    {
        return Err(PilotError::InvalidRequest(
            PilotValidationError::DeliveryReceiptMismatch,
        ));
    }
    Ok(())
}

fn signed_envelope(
    receipt_id: ReceiptId,
    expected_store_revision: StoreRevision,
    fence: Option<AttemptFence>,
    command: TaskCommand,
) -> Result<TransitionEnvelope, PilotError> {
    let mut envelope = TransitionEnvelope {
        schema: EnvelopeSchema::V1,
        receipt_id,
        request_digest_sha256: crate::task_service::zero_sha256(),
        expected_store_revision,
        fence,
        command,
    };
    envelope.request_digest_sha256 =
        canonical_command_digest(&envelope).map_err(map_service_error)?;
    Ok(envelope)
}

fn committed_transition(
    outcome: TransitionOutcome,
    operation_receipt_id: &ReceiptId,
) -> Result<TransitionResponse, PilotError> {
    match outcome {
        TransitionOutcome::Committed(response) => Ok(response),
        TransitionOutcome::NoWrite(error) => Err(match error {
            TaskServiceError::ReceiptConflict => PilotError::ReceiptConflict {
                receipt_id: operation_receipt_id.clone(),
            },
            other => map_service_error(other),
        }),
        TransitionOutcome::PersistenceUnknown { receipt_id, .. } => {
            Err(PilotError::ReconciliationRequired { receipt_id })
        }
    }
}

fn task_service_fence(fence: &PilotAttemptFence) -> AttemptFence {
    AttemptFence {
        task_id: fence.task_id.clone(),
        task_revision: fence.task_revision,
        attempt_number: fence.attempt_number,
        attempt_token: fence.attempt_token.clone(),
        owner_session_id: fence.owner.cutex_session_id.clone(),
        runtime_generation: fence.owner.runtime_generation,
    }
}

fn published_from_record(
    record: TaskRecord,
    committed_store_revision: StoreRevision,
) -> Result<PublishedTask, PilotError> {
    if record.phase != TaskPhase::Published {
        return Err(PilotError::IllegalPilotPhase);
    }
    let specification = PilotTaskSpecification {
        task_id: record.specification.task_id.clone(),
        task_revision: record.specification.task_revision,
        contract_sha256: record.specification.contract_sha256,
        opaque_contract: record.specification.opaque_contract,
    };
    let attempt = record.attempt.ok_or(PilotError::StaleAttempt)?;
    let publication_receipt_id = attempt.publication_receipt_id.clone();
    let fence = PilotAttemptFence {
        task_id: specification.task_id.clone(),
        task_revision: specification.task_revision,
        attempt_number: attempt.attempt_number,
        attempt_token: attempt.attempt_token,
        owner: PilotOwnerSnapshot {
            cutex_session_id: attempt.owner_session_id,
            durable_revision: attempt.owner_durable_revision,
            runtime_agent_id: attempt.runtime_agent_id,
            runtime_generation: attempt.runtime_generation,
        },
    };
    Ok(PublishedTask {
        specification,
        fence,
        publication_receipt_id,
        committed_store_revision,
    })
}

fn snapshot_from_record(record: TaskRecord) -> Result<PilotTaskSnapshot, PilotError> {
    let specification = PilotTaskSpecification {
        task_id: record.specification.task_id.clone(),
        task_revision: record.specification.task_revision,
        contract_sha256: record.specification.contract_sha256,
        opaque_contract: record.specification.opaque_contract,
    };
    let phase = pilot_phase(record.phase)?;
    let (fence, agent_bus_message_id) = match record.attempt {
        None => (None, None),
        Some(attempt) => {
            let fence = PilotAttemptFence {
                task_id: specification.task_id.clone(),
                task_revision: specification.task_revision,
                attempt_number: attempt.attempt_number,
                attempt_token: attempt.attempt_token,
                owner: PilotOwnerSnapshot {
                    cutex_session_id: attempt.owner_session_id,
                    durable_revision: attempt.owner_durable_revision,
                    runtime_agent_id: attempt.runtime_agent_id,
                    runtime_generation: attempt.runtime_generation,
                },
            };
            (Some(fence), attempt.delivery_receipt_id)
        }
    };
    Ok(PilotTaskSnapshot {
        specification,
        phase,
        fence,
        agent_bus_message_id,
    })
}

fn pilot_phase(phase: TaskPhase) -> Result<PilotTaskPhase, PilotError> {
    match phase {
        TaskPhase::Draft => Ok(PilotTaskPhase::Draft),
        TaskPhase::Published => Ok(PilotTaskPhase::Published),
        TaskPhase::Delivered => Ok(PilotTaskPhase::Delivered),
        TaskPhase::Accepted => Ok(PilotTaskPhase::Accepted),
        TaskPhase::Running => Ok(PilotTaskPhase::Running),
        TaskPhase::Completed => Ok(PilotTaskPhase::Completed),
        _ => Err(PilotError::IllegalPilotPhase),
    }
}

fn map_service_error(error: TaskServiceError) -> PilotError {
    match error {
        TaskServiceError::StoreRevisionConflict { expected, actual } => {
            PilotError::StoreRevisionConflict { expected, actual }
        }
        TaskServiceError::ReceiptConflict => {
            PilotError::InvalidRequest(PilotValidationError::DurableRequestRejected)
        }
        TaskServiceError::TaskNotFound => PilotError::TaskNotFound,
        TaskServiceError::AttemptNotFound | TaskServiceError::StaleFence => {
            PilotError::StaleAttempt
        }
        TaskServiceError::IllegalPhase { .. } => PilotError::IllegalPilotPhase,
        TaskServiceError::RecoveryRequired => PilotError::RecoveryRequired,
        TaskServiceError::InvalidEnvelope { .. }
        | TaskServiceError::RequestDigestMismatch
        | TaskServiceError::FenceNotAllowed
        | TaskServiceError::FenceRequired => {
            PilotError::InvalidRequest(PilotValidationError::DurableRequestRejected)
        }
        _ => PilotError::PersistenceUnavailable,
    }
}

#[cfg(test)]
mod tests;
