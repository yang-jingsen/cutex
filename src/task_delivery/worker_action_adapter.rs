//! Strict worker-action facade for the bounded Stage 3 execution slice.

use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::agent_bus::model::{
    TaskWorkerActionKind, TaskWorkerActionNoWrite, TaskWorkerActionReceipt,
    TaskWorkerActionRequest, TaskWorkerPhase, TaskWorkerResult, TASK_WORKER_RESULT_MAX_BYTES,
};
use crate::session::model::{CutexSessionArchiveState, CutexSessionRecord};
use crate::session::service::cutex_session_key_for_user_id_including_retired;
use crate::session::store::load_cutex_session_store;
use crate::task_service::{sha256_bytes, TaskCommand, TaskPhase, TransitionEvidence};

use super::{
    committed_transition, signed_envelope, task_service_fence, CutexSessionId, DurableRevision,
    PilotError, PilotStoreRevision, PilotTaskPhase, PilotTaskSnapshot,
    PilotWorkerReceiptObservation, ReceiptId, RuntimeAgentId, RuntimeGeneration, Sha256,
    TaskDeliveryPilot, TaskId, TaskRevision,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TaskWorkerRosterSender {
    pub runtime_agent_id: RuntimeAgentId,
    pub roster_session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskWorkerAuthorizedOwner {
    pub sender_runtime_agent_id: RuntimeAgentId,
    pub sender_roster_session_id: String,
    pub sender_cutex_session_id: CutexSessionId,
    pub sender_durable_revision: DurableRevision,
    pub sender_runtime_generation: RuntimeGeneration,
}

#[derive(Clone, Debug)]
pub(crate) struct ValidatedTaskWorkerAction {
    pub(crate) request: TaskWorkerActionRequest,
    pub(crate) result_bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub(crate) struct TaskWorkerAuthorizedAction {
    pub(crate) request: TaskWorkerActionRequest,
    pub(crate) result_bytes: Option<Vec<u8>>,
    pub(crate) owner: TaskWorkerAuthorizedOwner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TaskWorkerTransitionResult {
    Committed(TaskWorkerActionReceipt),
    NoWrite(TaskWorkerActionNoWrite),
    PersistenceUnknown,
}

pub(crate) fn validate_task_worker_action_request(
    request: TaskWorkerActionRequest,
) -> Result<ValidatedTaskWorkerAction, TaskWorkerActionNoWrite> {
    if request.task_id != request.attempt_fence.task_id
        || request.task_revision != request.attempt_fence.task_revision
    {
        return Err(TaskWorkerActionNoWrite::StaleFence);
    }
    let result_bytes = match (request.action, request.result.as_ref()) {
        (TaskWorkerActionKind::Accept | TaskWorkerActionKind::Start, None) => None,
        (TaskWorkerActionKind::Complete, Some(result)) => {
            let bytes = decode_result(result)?;
            if bytes.len() > TASK_WORKER_RESULT_MAX_BYTES {
                return Err(TaskWorkerActionNoWrite::ResultTooLarge);
            }
            if &sha256_bytes(&bytes) != result.sha256() {
                return Err(TaskWorkerActionNoWrite::ResultHashMismatch);
            }
            Some(bytes)
        }
        _ => return Err(TaskWorkerActionNoWrite::InvalidActionShape),
    };
    Ok(ValidatedTaskWorkerAction {
        request,
        result_bytes,
    })
}

fn decode_result(result: &TaskWorkerResult) -> Result<Vec<u8>, TaskWorkerActionNoWrite> {
    match result {
        TaskWorkerResult::Utf8 { text, .. } => Ok(text.as_bytes().to_vec()),
        TaskWorkerResult::Base64 { data, .. } => {
            let decoded = BASE64
                .decode(data.as_bytes())
                .map_err(|_| TaskWorkerActionNoWrite::InvalidActionShape)?;
            if BASE64.encode(&decoded) != *data {
                return Err(TaskWorkerActionNoWrite::InvalidActionShape);
            }
            Ok(decoded)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerSessionSnapshotError {
    Unavailable,
    NotFound,
}

pub(crate) trait WorkerSessionSnapshotBoundary: Send + Sync {
    fn load_for_roster_session(
        &self,
        roster_session_id: &str,
    ) -> Result<CutexSessionRecord, WorkerSessionSnapshotError>;
}

struct DurableWorkerSessionSnapshotBoundary;

impl WorkerSessionSnapshotBoundary for DurableWorkerSessionSnapshotBoundary {
    fn load_for_roster_session(
        &self,
        roster_session_id: &str,
    ) -> Result<CutexSessionRecord, WorkerSessionSnapshotError> {
        let store =
            load_cutex_session_store().map_err(|_| WorkerSessionSnapshotError::Unavailable)?;
        let key = cutex_session_key_for_user_id_including_retired(&store, roster_session_id)
            .ok_or(WorkerSessionSnapshotError::NotFound)?;
        store
            .sessions
            .get(&key)
            .cloned()
            .ok_or(WorkerSessionSnapshotError::NotFound)
    }
}

pub struct TaskWorkerActionAdapter {
    pilot: Arc<TaskDeliveryPilot>,
    sessions: Arc<dyn WorkerSessionSnapshotBoundary>,
}

impl TaskWorkerActionAdapter {
    pub fn open_recovered(root: impl Into<PathBuf>) -> Result<Self, PilotError> {
        let pilot = Arc::new(TaskDeliveryPilot::open(root)?);
        pilot.recover()?;
        Ok(Self {
            pilot,
            sessions: Arc::new(DurableWorkerSessionSnapshotBoundary),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_pilot_and_sessions(
        pilot: Arc<TaskDeliveryPilot>,
        sessions: Arc<dyn WorkerSessionSnapshotBoundary>,
    ) -> Self {
        Self { pilot, sessions }
    }

    pub(crate) fn authorize(
        &self,
        sender: TaskWorkerRosterSender,
        validated: ValidatedTaskWorkerAction,
    ) -> Result<TaskWorkerAuthorizedAction, TaskWorkerActionNoWrite> {
        let session = self.load_and_validate_session(&sender, &validated.request)?;
        self.validate_current_attempt(&validated.request)?;
        Ok(TaskWorkerAuthorizedAction {
            request: validated.request,
            result_bytes: validated.result_bytes,
            owner: TaskWorkerAuthorizedOwner {
                sender_runtime_agent_id: sender.runtime_agent_id,
                sender_roster_session_id: sender.roster_session_id,
                sender_cutex_session_id: CutexSessionId::new(session.cutex_session_id.clone())
                    .map_err(|_| TaskWorkerActionNoWrite::SessionIdentityMismatch)?,
                sender_durable_revision: DurableRevision::new(session.durable_revision())
                    .map_err(|_| TaskWorkerActionNoWrite::DurableRevisionMismatch)?,
                sender_runtime_generation: RuntimeGeneration::new(session.runtime_generation)
                    .map_err(|_| TaskWorkerActionNoWrite::RuntimeGenerationMissing)?,
            },
        })
    }

    pub(crate) fn authorize_stored_owner(
        &self,
        sender: &TaskWorkerRosterSender,
        owner: &TaskWorkerAuthorizedOwner,
        task_id: &TaskId,
        task_revision: TaskRevision,
        fence: &super::PilotAttemptFence,
    ) -> Result<(), TaskWorkerActionNoWrite> {
        if sender.runtime_agent_id != owner.sender_runtime_agent_id
            || sender.roster_session_id != owner.sender_roster_session_id
        {
            return Err(TaskWorkerActionNoWrite::UncertaintyBlocked);
        }
        let request = TaskWorkerActionRequest {
            schema: crate::agent_bus::model::TaskWorkerActionSchema::V1,
            action: TaskWorkerActionKind::Accept,
            task_id: task_id.clone(),
            task_revision,
            attempt_fence: fence.clone(),
            expected_store_revision: PilotStoreRevision::new(1).expect("revision one is valid"),
            action_id: ReceiptId::new("stored-owner-validation")
                .expect("fixed receipt ID is valid"),
            result: None,
        };
        let session = self.load_and_validate_session(sender, &request)?;
        if session.cutex_session_id != owner.sender_cutex_session_id.as_str()
            || session.durable_revision() != owner.sender_durable_revision.get()
            || session.runtime_generation != owner.sender_runtime_generation.get()
        {
            return Err(TaskWorkerActionNoWrite::UncertaintyBlocked);
        }
        self.validate_current_attempt(&request)
    }

    fn load_and_validate_session(
        &self,
        sender: &TaskWorkerRosterSender,
        request: &TaskWorkerActionRequest,
    ) -> Result<CutexSessionRecord, TaskWorkerActionNoWrite> {
        let session = match self
            .sessions
            .load_for_roster_session(&sender.roster_session_id)
        {
            Ok(session) => session,
            Err(WorkerSessionSnapshotError::Unavailable) => {
                return Err(TaskWorkerActionNoWrite::SessionSnapshotUnavailable)
            }
            Err(WorkerSessionSnapshotError::NotFound) => {
                return Err(TaskWorkerActionNoWrite::SessionNotFound)
            }
        };
        validate_session(sender, request, &session)?;
        Ok(session)
    }

    fn validate_current_attempt(
        &self,
        request: &TaskWorkerActionRequest,
    ) -> Result<(), TaskWorkerActionNoWrite> {
        let task = match self
            .pilot
            .task(request.task_id.clone(), request.task_revision)
        {
            Ok(Some(task)) => task,
            Ok(None) => return Err(TaskWorkerActionNoWrite::TaskNotFound),
            Err(error) => return Err(map_pilot_no_write(error)),
        };
        validate_task_attempt(request, &task)
    }

    pub(crate) fn inspect_receipt(
        &self,
        action_id: &ReceiptId,
    ) -> Result<PilotWorkerReceiptObservation, TaskWorkerActionNoWrite> {
        self.pilot
            .inspect_worker_receipt(action_id)
            .map_err(map_pilot_no_write)
    }

    pub(crate) fn receipt_from_observation(
        &self,
        authorized: &TaskWorkerAuthorizedAction,
        transport_record_id: &ReceiptId,
        observation: &PilotWorkerReceiptObservation,
    ) -> Result<Option<TaskWorkerActionReceipt>, TaskWorkerActionNoWrite> {
        let PilotWorkerReceiptObservation::Committed(record) = observation else {
            return Ok(None);
        };
        let envelope = action_envelope(authorized, transport_record_id)?;
        if record.request_digest_sha256 != envelope.request_digest_sha256
            || record.receipt_id != authorized.request.action_id
        {
            return Err(TaskWorkerActionNoWrite::ActionConflict);
        }
        Ok(Some(receipt_from_record(
            authorized,
            transport_record_id,
            record,
        )?))
    }

    pub(crate) fn transition_once(
        &self,
        authorized: &TaskWorkerAuthorizedAction,
        transport_record_id: &ReceiptId,
    ) -> TaskWorkerTransitionResult {
        let envelope = match action_envelope(authorized, transport_record_id) {
            Ok(envelope) => envelope,
            Err(error) => return TaskWorkerTransitionResult::NoWrite(error),
        };
        match committed_transition(
            self.pilot.service.transition(&envelope),
            &authorized.request.action_id,
        ) {
            Ok(response) => {
                let expected = expected_edge(authorized.request.action);
                if response.task_id != authorized.request.task_id
                    || response.task_revision != authorized.request.task_revision
                    || response.attempt_number
                        != Some(authorized.request.attempt_fence.attempt_number)
                    || response.prior_phase != Some(expected.0)
                    || response.resulting_phase != expected.1
                {
                    return TaskWorkerTransitionResult::NoWrite(
                        TaskWorkerActionNoWrite::DurableRequestRejected,
                    );
                }
                TaskWorkerTransitionResult::Committed(TaskWorkerActionReceipt {
                    action_id: response.receipt_id,
                    task_id: response.task_id,
                    task_revision: response.task_revision,
                    attempt_number: response
                        .attempt_number
                        .expect("validated attempted transition response"),
                    prior_phase: worker_phase(expected.0)
                        .expect("the worker edge has a worker phase"),
                    resulting_phase: worker_phase(expected.1)
                        .expect("the worker edge has a worker phase"),
                    committed_store_revision: response.committed_store_revision,
                    committed_at: response.committed_at,
                    transport_record_id: transport_record_id.clone(),
                    result_sha256: result_sha256(authorized),
                })
            }
            Err(PilotError::ReconciliationRequired { .. }) => {
                TaskWorkerTransitionResult::PersistenceUnknown
            }
            Err(error) => TaskWorkerTransitionResult::NoWrite(map_pilot_no_write(error)),
        }
    }
}

fn action_envelope(
    authorized: &TaskWorkerAuthorizedAction,
    transport_record_id: &ReceiptId,
) -> Result<crate::task_service::TransitionEnvelope, TaskWorkerActionNoWrite> {
    let evidence = TransitionEvidence {
        external_receipt_id: Some(transport_record_id.clone()),
        observed_at: None,
        evidence_sha256: result_sha256(authorized),
    };
    let command = match authorized.request.action {
        TaskWorkerActionKind::Accept => TaskCommand::Accept(evidence),
        TaskWorkerActionKind::Start => TaskCommand::Start(evidence),
        TaskWorkerActionKind::Complete => TaskCommand::CompleteRunning(evidence),
    };
    signed_envelope(
        authorized.request.action_id.clone(),
        authorized.request.expected_store_revision,
        Some(task_service_fence(&authorized.request.attempt_fence)),
        command,
    )
    .map_err(map_pilot_no_write)
}

fn result_sha256(authorized: &TaskWorkerAuthorizedAction) -> Option<Sha256> {
    authorized.result_bytes.as_ref().and_then(|_| {
        authorized
            .request
            .result
            .as_ref()
            .map(|result| result.sha256().clone())
    })
}

fn receipt_from_record(
    authorized: &TaskWorkerAuthorizedAction,
    transport_record_id: &ReceiptId,
    record: &super::PilotWorkerReceiptRecord,
) -> Result<TaskWorkerActionReceipt, TaskWorkerActionNoWrite> {
    let expected = expected_edge(authorized.request.action);
    if record.task_id != authorized.request.task_id
        || record.task_revision != authorized.request.task_revision
        || record.attempt_number != authorized.request.attempt_fence.attempt_number
        || record.prior_phase != pilot_worker_phase(expected.0)?
        || record.resulting_phase != pilot_worker_phase(expected.1)?
    {
        return Err(TaskWorkerActionNoWrite::ActionConflict);
    }
    Ok(TaskWorkerActionReceipt {
        action_id: record.receipt_id.clone(),
        task_id: record.task_id.clone(),
        task_revision: record.task_revision,
        attempt_number: record.attempt_number,
        prior_phase: worker_phase(expected.0)?,
        resulting_phase: worker_phase(expected.1)?,
        committed_store_revision: record.committed_store_revision,
        committed_at: record.committed_at.clone(),
        transport_record_id: transport_record_id.clone(),
        result_sha256: result_sha256(authorized),
    })
}

fn validate_session(
    sender: &TaskWorkerRosterSender,
    request: &TaskWorkerActionRequest,
    session: &CutexSessionRecord,
) -> Result<(), TaskWorkerActionNoWrite> {
    let owner = &request.attempt_fence.owner;
    if session.cutex_session_id != owner.cutex_session_id.as_str() {
        return Err(TaskWorkerActionNoWrite::SessionIdentityMismatch);
    }
    if session.archive_state != CutexSessionArchiveState::Active {
        return Err(TaskWorkerActionNoWrite::SessionInactive);
    }
    if session.durable_revision() != owner.durable_revision.get() {
        return Err(TaskWorkerActionNoWrite::DurableRevisionMismatch);
    }
    if sender.runtime_agent_id != owner.runtime_agent_id
        || session.current_runtime_agent_id.as_deref() != Some(sender.runtime_agent_id.as_str())
    {
        return Err(TaskWorkerActionNoWrite::RuntimeAgentMismatch);
    }
    if session.runtime_generation == 0 {
        return Err(TaskWorkerActionNoWrite::RuntimeGenerationMissing);
    }
    if session.runtime_generation != owner.runtime_generation.get() {
        return Err(TaskWorkerActionNoWrite::RuntimeGenerationMismatch);
    }
    Ok(())
}

fn validate_task_attempt(
    request: &TaskWorkerActionRequest,
    task: &PilotTaskSnapshot,
) -> Result<(), TaskWorkerActionNoWrite> {
    if task.specification.task_id != request.task_id
        || task.specification.task_revision != request.task_revision
    {
        return Err(TaskWorkerActionNoWrite::StaleFence);
    }
    let current = task
        .fence
        .as_ref()
        .ok_or(TaskWorkerActionNoWrite::StaleFence)?;
    if current.owner.durable_revision != request.attempt_fence.owner.durable_revision {
        return Err(TaskWorkerActionNoWrite::DurableRevisionMismatch);
    }
    if current.owner.runtime_agent_id != request.attempt_fence.owner.runtime_agent_id {
        return Err(TaskWorkerActionNoWrite::RuntimeAgentMismatch);
    }
    if current.owner.runtime_generation != request.attempt_fence.owner.runtime_generation {
        return Err(TaskWorkerActionNoWrite::RuntimeGenerationMismatch);
    }
    if current != &request.attempt_fence {
        return Err(TaskWorkerActionNoWrite::StaleFence);
    }
    Ok(())
}

fn expected_edge(action: TaskWorkerActionKind) -> (TaskPhase, TaskPhase) {
    match action {
        TaskWorkerActionKind::Accept => (TaskPhase::Delivered, TaskPhase::Accepted),
        TaskWorkerActionKind::Start => (TaskPhase::Accepted, TaskPhase::Running),
        TaskWorkerActionKind::Complete => (TaskPhase::Running, TaskPhase::Completed),
    }
}

fn worker_phase(phase: TaskPhase) -> Result<TaskWorkerPhase, TaskWorkerActionNoWrite> {
    match phase {
        TaskPhase::Delivered => Ok(TaskWorkerPhase::Delivered),
        TaskPhase::Accepted => Ok(TaskWorkerPhase::Accepted),
        TaskPhase::Running => Ok(TaskWorkerPhase::Running),
        TaskPhase::Completed => Ok(TaskWorkerPhase::Completed),
        _ => Err(TaskWorkerActionNoWrite::DurableRequestRejected),
    }
}

fn pilot_worker_phase(phase: TaskPhase) -> Result<PilotTaskPhase, TaskWorkerActionNoWrite> {
    match phase {
        TaskPhase::Delivered => Ok(PilotTaskPhase::Delivered),
        TaskPhase::Accepted => Ok(PilotTaskPhase::Accepted),
        TaskPhase::Running => Ok(PilotTaskPhase::Running),
        TaskPhase::Completed => Ok(PilotTaskPhase::Completed),
        _ => Err(TaskWorkerActionNoWrite::DurableRequestRejected),
    }
}

fn map_pilot_no_write(error: PilotError) -> TaskWorkerActionNoWrite {
    match error {
        PilotError::RecoveryRequired => TaskWorkerActionNoWrite::RecoveryRequired,
        PilotError::InvalidRequest(_) => TaskWorkerActionNoWrite::DurableRequestRejected,
        PilotError::StoreRevisionConflict { expected, actual } => {
            TaskWorkerActionNoWrite::StoreRevisionConflict { expected, actual }
        }
        PilotError::ReceiptConflict { .. } => TaskWorkerActionNoWrite::ActionConflict,
        PilotError::TaskNotFound => TaskWorkerActionNoWrite::TaskNotFound,
        PilotError::StaleAttempt => TaskWorkerActionNoWrite::StaleFence,
        PilotError::IllegalPilotPhase => TaskWorkerActionNoWrite::IllegalPhase,
        PilotError::PersistenceUnavailable => TaskWorkerActionNoWrite::PersistenceUnavailable,
        PilotError::ReconciliationRequired { .. } => {
            TaskWorkerActionNoWrite::PersistenceUnavailable
        }
    }
}
