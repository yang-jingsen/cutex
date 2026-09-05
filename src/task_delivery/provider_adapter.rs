//! Authenticated Task Service to Agent Bus dispatch boundary.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::agent_bus::identity::task_service_system_principal;
use crate::agent_bus::model::TaskServiceCompletionMetadata;
use crate::agent_bus::model::TaskServiceWorkerFollowupMetadata;
use crate::agent_bus::model::{
    validate_task_service_assignment_contract, validate_task_service_assignment_summary,
    TaskServiceAssignmentContractError, TaskServiceAssignmentMetadata,
};
use crate::agent_bus::queue::{
    enqueue_task_service_completion_message_once, enqueue_task_service_system_message_once,
    enqueue_task_service_worker_followup_message_once,
};
use crate::agent_bus::store::AgentBusState;
use crate::config::paths::runtime_dir;
use crate::session::model::CutexSessionArchiveState;
use crate::session::service::cutex_session_key_for_user_id_including_retired;
use crate::session::store::load_cutex_session_store;
use crate::task_delivery::TaskWorkerRosterSender;
use crate::task_service::{
    ActionId, AssignAndDispatchRequest, AssignProjectAndDispatchRequest, AuthenticatedPrincipal,
    CommunicationEventKind, CommunicationEventRequest, ProviderActionSchema, ProviderError,
    ProviderReceipt, ProviderResult, RetryDeliveryRequest, TaskServiceProvider,
    TaskServiceSnapshot, TASK_SERVICE_PROVIDER_ACTION_SCHEMA,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionNotificationDispatchSummary {
    pub queued: usize,
    pub deduplicated: usize,
    pub uncertain: usize,
    pub target_unavailable: usize,
    /// Pending unscoped records frozen by V3 activation. They remain durable
    /// and untouched, but do not block dispatch for independent scoped items.
    pub legacy_quarantined: usize,
    pub legacy_quarantined_notification_ids: BTreeSet<String>,
    pub unavailable_target_seats: BTreeSet<String>,
    pub unavailable_target_sessions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentDispatchOutcome {
    pub assignment_receipt: ProviderReceipt,
    pub communication_receipt: ProviderReceipt,
    pub agent_bus_message_id: String,
    pub target_runtime_diagnostic: String,
    pub deduplicated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentDispatchError {
    Provider(ProviderError),
    Contract(TaskServiceAssignmentContractError),
    TargetUnavailable,
    AgentBusUnavailable,
    InvalidCommittedShape,
}

impl fmt::Display for AssignmentDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Task Service assignment dispatch error: {self:?}"
        )
    }
}

impl std::error::Error for AssignmentDispatchError {}

pub struct TaskServiceAgentBusDispatcher;

const TASK_SERVICE_PROVIDER_PATH: &[&str] = &[
    "task-service",
    "task-worker-actions-v1",
    "task-service",
    "provider-v2",
];

pub fn default_task_service_provider_root() -> Result<PathBuf, ProviderError> {
    let mut root = runtime_dir().map_err(|_| ProviderError::PersistenceUnavailable)?;
    for component in TASK_SERVICE_PROVIDER_PATH {
        root.push(component);
    }
    Ok(root)
}

/// Records the first durable proof that a Task Service payload entered the
/// assignee's native app-server context. The stable action identity makes an
/// Agent Bus redelivery an exact replay rather than a second semantic event.
pub fn record_context_inserted(
    metadata: &TaskServiceAssignmentMetadata,
    agent_bus_message_id: &str,
    native_submission_id: &str,
) -> Result<ProviderReceipt, ProviderError> {
    if agent_bus_message_id.trim().is_empty() || native_submission_id.trim().is_empty() {
        return Err(ProviderError::InvalidRequest(
            "invalid_context_insertion_receipt",
        ));
    }
    let provider = TaskServiceProvider::open(default_task_service_provider_root()?)?;
    record_context_inserted_with_provider(
        &provider,
        metadata,
        agent_bus_message_id,
        native_submission_id,
    )
}

/// Validates the protected assignment envelope against the authoritative
/// committed assignment and task revision before any model submission.
pub fn validate_assignment_metadata(
    metadata: &TaskServiceAssignmentMetadata,
) -> Result<(), ProviderError> {
    let provider = TaskServiceProvider::open(default_task_service_provider_root()?)?;
    validate_assignment_metadata_with_provider(&provider, metadata).map(|_| ())
}

fn validate_assignment_metadata_with_provider(
    provider: &TaskServiceProvider,
    metadata: &TaskServiceAssignmentMetadata,
) -> Result<TaskServiceSnapshot, ProviderError> {
    let snapshot = provider.query()?;
    let send_attempt = snapshot
        .send_attempts
        .get(&metadata.send_attempt_id)
        .ok_or(ProviderError::NotFound("send_attempt"))?;
    if send_attempt.assignment_id != metadata.assignment_id {
        return Err(ProviderError::Conflict("send_attempt_assignment_conflict"));
    }
    let assignment = snapshot
        .assignments
        .get(&metadata.assignment_id)
        .ok_or(ProviderError::NotFound("assignment"))?;
    if assignment.project_id != metadata.project_id
        || assignment.task_id != metadata.task_id
        || assignment.task_revision != metadata.task_revision
    {
        return Err(ProviderError::Conflict("assignment_task_conflict"));
    }
    let task = snapshot
        .task_revisions
        .get(&assignment.task_id)
        .and_then(|revisions| revisions.get(&assignment.task_revision))
        .ok_or(ProviderError::NotFound("task_revision"))?;
    if task.contract_sha256 != metadata.contract_sha256 {
        return Err(ProviderError::Conflict(
            "assignment_contract_digest_conflict",
        ));
    }
    if task.project_id != metadata.project_id {
        return Err(ProviderError::Conflict("assignment_project_conflict"));
    }
    if let Some(contract) = metadata
        .validate_contract_if_present()
        .map_err(|_| ProviderError::Conflict("assignment_contract_invalid"))?
    {
        if contract != task.opaque_contract {
            return Err(ProviderError::Conflict("assignment_contract_conflict"));
        }
    }
    Ok(snapshot)
}

fn record_context_inserted_with_provider(
    provider: &TaskServiceProvider,
    metadata: &TaskServiceAssignmentMetadata,
    agent_bus_message_id: &str,
    native_submission_id: &str,
) -> Result<ProviderReceipt, ProviderError> {
    let snapshot = validate_assignment_metadata_with_provider(provider, metadata)?;
    let send_attempt = snapshot
        .send_attempts
        .get(&metadata.send_attempt_id)
        .ok_or(ProviderError::NotFound("send_attempt"))?;
    let action_id = ActionId::new(format!(
        "context-inserted:{}:{}",
        metadata.send_attempt_id.as_str(),
        agent_bus_message_id,
    ))?;
    provider.record_communication_event(
        &AuthenticatedPrincipal::task_service_system(),
        &CommunicationEventRequest {
            schema: ProviderActionSchema::V2,
            action_id,
            send_attempt_id: metadata.send_attempt_id.clone(),
            expected_send_attempt_revision: send_attempt.local_revision,
            kind: CommunicationEventKind::ContextInserted,
            receipt_reference: Some(native_submission_id.to_string()),
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerPrincipalError {
    SessionSnapshotUnavailable,
    SessionNotFound,
    SessionInactive,
    RuntimeNotCurrent,
    InvalidStableIdentity,
}

pub(crate) fn authenticate_worker_principal(
    sender: &TaskWorkerRosterSender,
) -> Result<AuthenticatedPrincipal, WorkerPrincipalError> {
    let store =
        load_cutex_session_store().map_err(|_| WorkerPrincipalError::SessionSnapshotUnavailable)?;
    let key = cutex_session_key_for_user_id_including_retired(&store, &sender.roster_session_id)
        .ok_or(WorkerPrincipalError::SessionNotFound)?;
    let session = store
        .sessions
        .get(&key)
        .ok_or(WorkerPrincipalError::SessionNotFound)?;
    if session.archive_state != CutexSessionArchiveState::Active {
        return Err(WorkerPrincipalError::SessionInactive);
    }
    if session.current_runtime_agent_id.as_deref() != Some(sender.runtime_agent_id.as_str()) {
        return Err(WorkerPrincipalError::RuntimeNotCurrent);
    }
    let stable = crate::role_revision::CutexSessionId::new(session.cutex_session_id.clone())
        .map_err(|_| WorkerPrincipalError::InvalidStableIdentity)?;
    Ok(AuthenticatedPrincipal::session(stable))
}

impl TaskServiceAgentBusDispatcher {
    /// Drains notifications which have not reached the durable Agent Bus. The
    /// semantic transition and outbox record are already committed; every
    /// failure below is recorded as an uncertain fact and remains retryable.
    pub(crate) fn dispatch_pending_completion_notifications(
        provider: &TaskServiceProvider,
        seats: &crate::seat::SeatOccupancyStore,
        state: &Arc<Mutex<AgentBusState>>,
        now_epoch_secs: u64,
    ) -> Result<CompletionNotificationDispatchSummary, ProviderError> {
        let snapshot = provider.query()?;
        let seat_snapshot = seats
            .query()
            .map_err(|_| ProviderError::PersistenceUnavailable)?;
        let mut summary = CompletionNotificationDispatchSummary::default();
        for notification in snapshot.completion_notifications.values() {
            if notification.is_delivered() {
                continue;
            }
            // V3 activation freezes every unscoped legacy aggregate. Keep the
            // item immutable and explicitly project it as quarantined before
            // seat resolution or Agent Bus enqueue. Dispatch continues at the
            // next independently scoped record, avoiding global head-of-line
            // blocking without acknowledging, retargeting, or discarding the
            // legacy fact.
            if snapshot.schema == crate::task_service::ProviderStoreSchema::V3
                && notification.project_id.is_none()
            {
                summary.legacy_quarantined += 1;
                summary
                    .legacy_quarantined_notification_ids
                    .insert(notification.notification_id.as_str().to_string());
                continue;
            }
            let target_session = seat_snapshot
                .occupancies
                .get(&notification.target_seat_id)
                .map(|occupancy| occupancy.occupant_cutex_session.as_str());
            let target =
                target_session.and_then(|session| resolve_current_runtime_target(state, session));
            let Some((target_id, target_name)) = target else {
                summary.target_unavailable += 1;
                summary
                    .unavailable_target_seats
                    .insert(notification.target_seat_id.as_str().to_string());
                record_notification_uncertain_once(provider, notification, "target_unavailable")?;
                summary.uncertain += 1;
                continue;
            };
            let metadata = TaskServiceCompletionMetadata {
                schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
                project_id: notification.project_id.clone(),
                notification_id: notification.notification_id.clone(),
                assignment_id: notification.assignment_id.clone(),
                task_id: notification.task_id.clone(),
                task_revision: notification.task_revision,
                attempt_number: notification.attempt_number,
                transition_action_id: notification.transition_action_id.clone(),
                kind: notification.kind,
                target_seat_id: notification.target_seat_id.clone(),
            };
            let mode = match notification.delivery_mode {
                crate::task_service::CompletionNotificationDeliveryMode::AfterTurn => {
                    crate::agent_bus::delivery::AgentDeliveryMode::AfterTurn
                }
                crate::task_service::CompletionNotificationDeliveryMode::Soon => {
                    crate::agent_bus::delivery::AgentDeliveryMode::Soon
                }
            };
            let outcome = enqueue_task_service_completion_message_once(
                state,
                &task_service_system_principal(),
                &target_id,
                &target_name,
                &notification.human_readable_content,
                &metadata,
                mode,
                notification.transition_action_id.as_str(),
                &notification.external_message_id,
                now_epoch_secs,
            );
            match outcome {
                Ok(outcome) => {
                    let already_recorded = notification.facts.iter().any(|fact| {
                        fact.kind == crate::task_service::CompletionNotificationFactKind::Queued
                            && fact.reference.as_deref() == Some(outcome.record.id.as_str())
                    });
                    if !already_recorded {
                        record_notification_fact(
                            provider,
                            notification,
                            crate::task_service::CompletionNotificationFactKind::Queued,
                            Some(outcome.record.id),
                        )?;
                    }
                    summary.queued += 1;
                    summary.deduplicated += usize::from(outcome.deduplicated);
                }
                Err(_) => {
                    record_notification_uncertain_once(
                        provider,
                        notification,
                        "agent_bus_unavailable",
                    )?;
                    summary.uncertain += 1;
                }
            }
        }
        for notification in snapshot.worker_followup_notifications.values() {
            if notification.is_delivered() {
                continue;
            }
            if snapshot.schema == crate::task_service::ProviderStoreSchema::V3
                && notification.project_id.is_none()
            {
                summary.legacy_quarantined += 1;
                summary
                    .legacy_quarantined_notification_ids
                    .insert(notification.notification_id.as_str().to_string());
                continue;
            }
            let target =
                resolve_current_runtime_target(state, notification.target_cutex_session.as_str());
            let Some((target_id, target_name)) = target else {
                summary.target_unavailable += 1;
                summary
                    .unavailable_target_sessions
                    .insert(notification.target_cutex_session.as_str().to_string());
                record_worker_followup_uncertain_once(
                    provider,
                    notification,
                    "target_unavailable",
                )?;
                summary.uncertain += 1;
                continue;
            };
            let metadata = TaskServiceWorkerFollowupMetadata {
                schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
                project_id: notification.project_id.clone(),
                notification_id: notification.notification_id.clone(),
                assignment_id: notification.assignment_id.clone(),
                task_id: notification.task_id.clone(),
                task_revision: notification.task_revision,
                attempt_number: notification.attempt_number,
                decision_reference: notification.decision_reference.clone(),
            };
            match enqueue_task_service_worker_followup_message_once(
                state,
                &task_service_system_principal(),
                &target_id,
                &target_name,
                &notification.decision_reference,
                &metadata,
                notification.transition_action_id.as_str(),
                &notification.external_message_id,
                now_epoch_secs,
            ) {
                Ok(outcome) => {
                    let already_recorded = notification.facts.iter().any(|fact| {
                        fact.kind == crate::task_service::CompletionNotificationFactKind::Queued
                            && fact.reference.as_deref() == Some(outcome.record.id.as_str())
                    });
                    if !already_recorded {
                        record_worker_followup_fact(
                            provider,
                            notification,
                            crate::task_service::CompletionNotificationFactKind::Queued,
                            Some(outcome.record.id),
                        )?;
                    }
                    summary.queued += 1;
                    summary.deduplicated += usize::from(outcome.deduplicated);
                }
                Err(_) => {
                    record_worker_followup_uncertain_once(
                        provider,
                        notification,
                        "agent_bus_unavailable",
                    )?;
                    summary.uncertain += 1;
                }
            }
        }
        Ok(summary)
    }

    /// Persists the assignment and SendAttempt first, then queues through the
    /// opaque Task Service system principal. A routing failure leaves the
    /// assignment at `AWAITING_ACK` with its durable `SEND_PREPARED` fact.
    pub fn assign_and_dispatch(
        provider: &TaskServiceProvider,
        coordinator: &AuthenticatedPrincipal,
        state: &Arc<Mutex<AgentBusState>>,
        request: &AssignAndDispatchRequest,
        expected_workflow_revision: u64,
        human_readable_content: &str,
        now_epoch_secs: u64,
    ) -> Result<AssignmentDispatchOutcome, AssignmentDispatchError> {
        if human_readable_content.trim().is_empty() {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let before = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?;
        if !before.receipts.contains_key(&request.action_id) {
            let task = before
                .task_revisions
                .get(&request.task_id)
                .and_then(|revisions| revisions.get(&request.task_revision))
                .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
            validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
                .map_err(AssignmentDispatchError::Contract)?;
            validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
                .map_err(AssignmentDispatchError::Contract)?;
        }
        let assignment_receipt = provider
            .assign_and_dispatch(
                coordinator,
                request,
                expected_workflow_revision,
                human_readable_content,
            )
            .map_err(AssignmentDispatchError::Provider)?;
        let (assignment, send_attempt) = match &assignment_receipt.result {
            ProviderResult::Assignment {
                assignment,
                send_attempt: Some(send_attempt),
            } => (assignment, send_attempt),
            _ => return Err(AssignmentDispatchError::InvalidCommittedShape),
        };
        if assignment.assignment_id != request.assignment_id
            || assignment.task_id != request.task_id
            || assignment.task_revision != request.task_revision
            || send_attempt.assignment_id != assignment.assignment_id
            || send_attempt.send_attempt_id != request.send_attempt_id
        {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let task = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?
            .task_revisions
            .get(&assignment.task_id)
            .and_then(|revisions| revisions.get(&assignment.task_revision))
            .cloned()
            .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
        validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
            .map_err(AssignmentDispatchError::Contract)?;
        validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
            .map_err(AssignmentDispatchError::Contract)?;
        let coordinator_cutex_session = coordinator
            .authenticated_session_id()
            .map_err(AssignmentDispatchError::Provider)?
            .clone();
        let (target_id, target_name) =
            resolve_current_runtime_target(state, assignment.assignee_cutex_session.as_str())
                .ok_or(AssignmentDispatchError::TargetUnavailable)?;
        let metadata = TaskServiceAssignmentMetadata {
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            project_id: None,
            coordinator_cutex_session: Some(coordinator_cutex_session),
            assignment_id: assignment.assignment_id.clone(),
            task_id: assignment.task_id.clone(),
            task_revision: assignment.task_revision,
            contract_sha256: task.contract_sha256,
            opaque_contract: Some(task.opaque_contract),
            send_attempt_id: send_attempt.send_attempt_id.clone(),
        };
        let system = task_service_system_principal();
        let outcome = enqueue_task_service_system_message_once(
            state,
            &system,
            &target_id,
            &target_name,
            human_readable_content,
            &metadata,
            request.action_id.as_str(),
            &send_attempt.external_message_id,
            now_epoch_secs,
        )
        .map_err(|_| AssignmentDispatchError::AgentBusUnavailable)?;
        let communication_action_id = ActionId::new(format!(
            "bus-queued:{}:{}",
            send_attempt.send_attempt_id.as_str(),
            outcome.record.id
        ))
        .map_err(AssignmentDispatchError::Provider)?;
        let communication_receipt = provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: communication_action_id,
                    send_attempt_id: send_attempt.send_attempt_id.clone(),
                    expected_send_attempt_revision: send_attempt.local_revision,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some(outcome.record.id.clone()),
                },
            )
            .map_err(AssignmentDispatchError::Provider)?;
        Ok(AssignmentDispatchOutcome {
            assignment_receipt,
            communication_receipt,
            agent_bus_message_id: outcome.record.id,
            target_runtime_diagnostic: target_id,
            deduplicated: outcome.deduplicated,
        })
    }

    /// V3 counterpart of `assign_and_dispatch`; project identity is persisted
    /// before delivery and repeated in the typed protected message metadata.
    pub fn assign_project_and_dispatch(
        provider: &TaskServiceProvider,
        coordinator: &AuthenticatedPrincipal,
        state: &Arc<Mutex<AgentBusState>>,
        request: &AssignProjectAndDispatchRequest,
        expected_workflow_revision: u64,
        human_readable_content: &str,
        now_epoch_secs: u64,
    ) -> Result<AssignmentDispatchOutcome, AssignmentDispatchError> {
        if human_readable_content.trim().is_empty() {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let before = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?;
        if !before.receipts.contains_key(&request.action_id) {
            let task = before
                .task_revisions
                .get(&request.task_id)
                .and_then(|revisions| revisions.get(&request.task_revision))
                .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
            if task.project_id.as_ref() != Some(&request.project_id) {
                return Err(AssignmentDispatchError::InvalidCommittedShape);
            }
            validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
                .map_err(AssignmentDispatchError::Contract)?;
            validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
                .map_err(AssignmentDispatchError::Contract)?;
        }
        let assignment_receipt = provider
            .assign_project_and_dispatch(
                coordinator,
                request,
                expected_workflow_revision,
                human_readable_content,
            )
            .map_err(AssignmentDispatchError::Provider)?;
        let (assignment, send_attempt) = match &assignment_receipt.result {
            ProviderResult::Assignment {
                assignment,
                send_attempt: Some(send_attempt),
            } => (assignment, send_attempt),
            _ => return Err(AssignmentDispatchError::InvalidCommittedShape),
        };
        if assignment.project_id.as_ref() != Some(&request.project_id)
            || assignment.assignment_id != request.assignment_id
            || assignment.task_id != request.task_id
            || assignment.task_revision != request.task_revision
            || send_attempt.assignment_id != assignment.assignment_id
            || send_attempt.send_attempt_id != request.send_attempt_id
        {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let task = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?
            .task_revisions
            .get(&assignment.task_id)
            .and_then(|revisions| revisions.get(&assignment.task_revision))
            .cloned()
            .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
        if task.project_id != assignment.project_id {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
            .map_err(AssignmentDispatchError::Contract)?;
        validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
            .map_err(AssignmentDispatchError::Contract)?;
        let coordinator_cutex_session = coordinator
            .authenticated_session_id()
            .map_err(AssignmentDispatchError::Provider)?
            .clone();
        let (target_id, target_name) =
            resolve_current_runtime_target(state, assignment.assignee_cutex_session.as_str())
                .ok_or(AssignmentDispatchError::TargetUnavailable)?;
        let metadata = TaskServiceAssignmentMetadata {
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            project_id: Some(request.project_id.clone()),
            coordinator_cutex_session: Some(coordinator_cutex_session),
            assignment_id: assignment.assignment_id.clone(),
            task_id: assignment.task_id.clone(),
            task_revision: assignment.task_revision,
            contract_sha256: task.contract_sha256,
            opaque_contract: Some(task.opaque_contract),
            send_attempt_id: send_attempt.send_attempt_id.clone(),
        };
        let system = task_service_system_principal();
        let outcome = enqueue_task_service_system_message_once(
            state,
            &system,
            &target_id,
            &target_name,
            human_readable_content,
            &metadata,
            request.action_id.as_str(),
            &send_attempt.external_message_id,
            now_epoch_secs,
        )
        .map_err(|_| AssignmentDispatchError::AgentBusUnavailable)?;
        let communication_action_id = ActionId::new(format!(
            "bus-queued:{}:{}",
            send_attempt.send_attempt_id.as_str(),
            outcome.record.id
        ))
        .map_err(AssignmentDispatchError::Provider)?;
        let communication_receipt = provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: communication_action_id,
                    send_attempt_id: send_attempt.send_attempt_id.clone(),
                    expected_send_attempt_revision: send_attempt.local_revision,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some(outcome.record.id.clone()),
                },
            )
            .map_err(AssignmentDispatchError::Provider)?;
        Ok(AssignmentDispatchOutcome {
            assignment_receipt,
            communication_receipt,
            agent_bus_message_id: outcome.record.id,
            target_runtime_diagnostic: target_id,
            deduplicated: outcome.deduplicated,
        })
    }

    /// Persists a new SendAttempt before retrying delivery through the same
    /// authenticated Task Service system sender used by initial dispatch.
    pub fn retry_delivery(
        provider: &TaskServiceProvider,
        coordinator: &AuthenticatedPrincipal,
        state: &Arc<Mutex<AgentBusState>>,
        request: &RetryDeliveryRequest,
        expected_assignment_revision: u64,
        human_readable_content: &str,
        now_epoch_secs: u64,
    ) -> Result<AssignmentDispatchOutcome, AssignmentDispatchError> {
        if human_readable_content.trim().is_empty() {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let before = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?;
        if !before.receipts.contains_key(&request.action_id) {
            let assignment = before
                .assignments
                .get(&request.assignment_id)
                .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
            let task = before
                .task_revisions
                .get(&assignment.task_id)
                .and_then(|revisions| revisions.get(&assignment.task_revision))
                .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
            validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
                .map_err(AssignmentDispatchError::Contract)?;
            validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
                .map_err(AssignmentDispatchError::Contract)?;
        }
        let assignment_receipt = provider
            .retry_delivery(
                coordinator,
                request,
                expected_assignment_revision,
                human_readable_content,
            )
            .map_err(AssignmentDispatchError::Provider)?;
        let send_attempt = match &assignment_receipt.result {
            ProviderResult::SendAttempt(send_attempt) => send_attempt,
            _ => return Err(AssignmentDispatchError::InvalidCommittedShape),
        };
        let snapshot = provider
            .query()
            .map_err(AssignmentDispatchError::Provider)?;
        let assignment = snapshot
            .assignments
            .get(&request.assignment_id)
            .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
        if send_attempt.assignment_id != assignment.assignment_id
            || send_attempt.send_attempt_id != request.send_attempt_id
        {
            return Err(AssignmentDispatchError::InvalidCommittedShape);
        }
        let task = snapshot
            .task_revisions
            .get(&assignment.task_id)
            .and_then(|revisions| revisions.get(&assignment.task_revision))
            .cloned()
            .ok_or(AssignmentDispatchError::InvalidCommittedShape)?;
        validate_task_service_assignment_contract(&task.opaque_contract, &task.contract_sha256)
            .map_err(AssignmentDispatchError::Contract)?;
        validate_task_service_assignment_summary(human_readable_content, &task.opaque_contract)
            .map_err(AssignmentDispatchError::Contract)?;
        let coordinator_cutex_session = coordinator
            .authenticated_session_id()
            .map_err(AssignmentDispatchError::Provider)?
            .clone();
        let (target_id, target_name) =
            resolve_current_runtime_target(state, assignment.assignee_cutex_session.as_str())
                .ok_or(AssignmentDispatchError::TargetUnavailable)?;
        let metadata = TaskServiceAssignmentMetadata {
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            project_id: assignment.project_id.clone(),
            coordinator_cutex_session: Some(coordinator_cutex_session),
            assignment_id: assignment.assignment_id.clone(),
            task_id: assignment.task_id.clone(),
            task_revision: assignment.task_revision,
            contract_sha256: task.contract_sha256,
            opaque_contract: Some(task.opaque_contract),
            send_attempt_id: send_attempt.send_attempt_id.clone(),
        };
        let system = task_service_system_principal();
        let outcome = enqueue_task_service_system_message_once(
            state,
            &system,
            &target_id,
            &target_name,
            human_readable_content,
            &metadata,
            request.action_id.as_str(),
            &send_attempt.external_message_id,
            now_epoch_secs,
        )
        .map_err(|_| AssignmentDispatchError::AgentBusUnavailable)?;
        let communication_action_id = ActionId::new(format!(
            "bus-queued:{}:{}",
            send_attempt.send_attempt_id.as_str(),
            outcome.record.id
        ))
        .map_err(AssignmentDispatchError::Provider)?;
        let communication_receipt = provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: if assignment.project_id.is_some() {
                        ProviderActionSchema::V3
                    } else {
                        ProviderActionSchema::V2
                    },
                    action_id: communication_action_id,
                    send_attempt_id: send_attempt.send_attempt_id.clone(),
                    expected_send_attempt_revision: send_attempt.local_revision,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some(outcome.record.id.clone()),
                },
            )
            .map_err(AssignmentDispatchError::Provider)?;
        Ok(AssignmentDispatchOutcome {
            assignment_receipt,
            communication_receipt,
            agent_bus_message_id: outcome.record.id,
            target_runtime_diagnostic: target_id,
            deduplicated: outcome.deduplicated,
        })
    }
}

fn record_notification_fact(
    provider: &TaskServiceProvider,
    notification: &crate::task_service::CompletionNotification,
    kind: crate::task_service::CompletionNotificationFactKind,
    reference: Option<String>,
) -> Result<ProviderReceipt, ProviderError> {
    let label = match kind {
        crate::task_service::CompletionNotificationFactKind::Queued => "queued",
        crate::task_service::CompletionNotificationFactKind::Delivered => "delivered",
        crate::task_service::CompletionNotificationFactKind::Uncertain => "uncertain",
    };
    let action_id = ActionId::new(format!(
        "notification-{label}:{}:{}",
        notification.notification_id.as_str(),
        notification.local_revision,
    ))?;
    provider.record_completion_notification_fact(
        &AuthenticatedPrincipal::task_service_system(),
        &crate::task_service::CompletionNotificationFactRequest {
            schema: ProviderActionSchema::V2,
            action_id,
            notification_id: notification.notification_id.clone(),
            expected_notification_revision: notification.local_revision,
            kind,
            reference,
        },
    )
}

fn record_notification_uncertain_once(
    provider: &TaskServiceProvider,
    notification: &crate::task_service::CompletionNotification,
    reason: &str,
) -> Result<Option<ProviderReceipt>, ProviderError> {
    if notification.facts.last().is_some_and(|fact| {
        fact.kind == crate::task_service::CompletionNotificationFactKind::Uncertain
            && fact.reference.as_deref() == Some(reason)
    }) {
        return Ok(None);
    }
    record_notification_fact(
        provider,
        notification,
        crate::task_service::CompletionNotificationFactKind::Uncertain,
        Some(reason.to_string()),
    )
    .map(Some)
}

fn record_worker_followup_fact(
    provider: &TaskServiceProvider,
    notification: &crate::task_service::WorkerFollowupNotification,
    kind: crate::task_service::CompletionNotificationFactKind,
    reference: Option<String>,
) -> Result<ProviderReceipt, ProviderError> {
    let label = match kind {
        crate::task_service::CompletionNotificationFactKind::Queued => "queued",
        crate::task_service::CompletionNotificationFactKind::Delivered => "delivered",
        crate::task_service::CompletionNotificationFactKind::Uncertain => "uncertain",
    };
    provider.record_worker_followup_fact(
        &AuthenticatedPrincipal::task_service_system(),
        &crate::task_service::WorkerFollowupFactRequest {
            schema: ProviderActionSchema::V2,
            action_id: ActionId::new(format!(
                "worker-followup-{label}:{}:{}",
                notification.notification_id.as_str(),
                notification.local_revision,
            ))?,
            notification_id: notification.notification_id.clone(),
            expected_notification_revision: notification.local_revision,
            kind,
            reference,
        },
    )
}

fn record_worker_followup_uncertain_once(
    provider: &TaskServiceProvider,
    notification: &crate::task_service::WorkerFollowupNotification,
    reason: &str,
) -> Result<Option<ProviderReceipt>, ProviderError> {
    if notification.facts.last().is_some_and(|fact| {
        fact.kind == crate::task_service::CompletionNotificationFactKind::Uncertain
            && fact.reference.as_deref() == Some(reason)
    }) {
        return Ok(None);
    }
    record_worker_followup_fact(
        provider,
        notification,
        crate::task_service::CompletionNotificationFactKind::Uncertain,
        Some(reason.to_string()),
    )
    .map(Some)
}

pub fn record_completion_context_inserted(
    metadata: &TaskServiceCompletionMetadata,
    agent_bus_message_id: &str,
    native_submission_id: &str,
) -> Result<ProviderReceipt, ProviderError> {
    if agent_bus_message_id.trim().is_empty() || native_submission_id.trim().is_empty() {
        return Err(ProviderError::InvalidRequest(
            "invalid_completion_context_insertion_receipt",
        ));
    }
    let provider = TaskServiceProvider::open(default_task_service_provider_root()?)?;
    record_completion_context_inserted_with_provider(
        &provider,
        metadata,
        agent_bus_message_id,
        native_submission_id,
    )
}

fn record_completion_context_inserted_with_provider(
    provider: &TaskServiceProvider,
    metadata: &TaskServiceCompletionMetadata,
    agent_bus_message_id: &str,
    native_submission_id: &str,
) -> Result<ProviderReceipt, ProviderError> {
    let snapshot = provider.query()?;
    let notification = snapshot
        .completion_notifications
        .get(&metadata.notification_id)
        .ok_or(ProviderError::NotFound("completion_notification"))?;
    if notification.assignment_id != metadata.assignment_id
        || notification.project_id != metadata.project_id
    {
        return Err(ProviderError::Conflict("notification_assignment_conflict"));
    }
    provider.record_completion_notification_fact(
        &AuthenticatedPrincipal::task_service_system(),
        &crate::task_service::CompletionNotificationFactRequest {
            schema: ProviderActionSchema::V2,
            action_id: ActionId::new(format!(
                "completion-delivered:{}:{agent_bus_message_id}",
                metadata.notification_id.as_str()
            ))?,
            notification_id: metadata.notification_id.clone(),
            expected_notification_revision: notification.local_revision,
            kind: crate::task_service::CompletionNotificationFactKind::Delivered,
            reference: Some(format!("{agent_bus_message_id}:{native_submission_id}")),
        },
    )
}

pub fn validate_worker_followup_metadata(
    metadata: &TaskServiceWorkerFollowupMetadata,
    recipient_cutex_session: &str,
) -> Result<(), ProviderError> {
    let provider = TaskServiceProvider::open(default_task_service_provider_root()?)?;
    validate_worker_followup_metadata_with_provider(&provider, metadata, recipient_cutex_session)
}

fn validate_worker_followup_metadata_with_provider(
    provider: &TaskServiceProvider,
    metadata: &TaskServiceWorkerFollowupMetadata,
    recipient_cutex_session: &str,
) -> Result<(), ProviderError> {
    let snapshot = provider.query()?;
    let notification = snapshot
        .worker_followup_notifications
        .get(&metadata.notification_id)
        .ok_or(ProviderError::NotFound("worker_followup_notification"))?;
    let assignment = snapshot
        .assignments
        .get(&notification.assignment_id)
        .ok_or(ProviderError::NotFound("assignment"))?;
    if notification.assignment_id != metadata.assignment_id
        || notification.project_id != metadata.project_id
        || notification.task_id != metadata.task_id
        || notification.task_revision != metadata.task_revision
        || notification.attempt_number != metadata.attempt_number
        || notification.decision_reference != metadata.decision_reference
        || assignment.assignee_cutex_session != notification.target_cutex_session
        || notification.target_cutex_session.as_str() != recipient_cutex_session
    {
        return Err(ProviderError::Conflict("worker_followup_metadata_conflict"));
    }
    Ok(())
}

pub fn record_worker_followup_context_inserted(
    metadata: &TaskServiceWorkerFollowupMetadata,
    recipient_cutex_session: &str,
    agent_bus_message_id: &str,
    native_submission_id: &str,
) -> Result<ProviderReceipt, ProviderError> {
    if agent_bus_message_id.trim().is_empty() || native_submission_id.trim().is_empty() {
        return Err(ProviderError::InvalidRequest(
            "invalid_worker_followup_context_insertion_receipt",
        ));
    }
    let provider = TaskServiceProvider::open(default_task_service_provider_root()?)?;
    validate_worker_followup_metadata_with_provider(&provider, metadata, recipient_cutex_session)?;
    let snapshot = provider.query()?;
    let notification = snapshot
        .worker_followup_notifications
        .get(&metadata.notification_id)
        .ok_or(ProviderError::NotFound("worker_followup_notification"))?;
    provider.record_worker_followup_fact(
        &AuthenticatedPrincipal::task_service_system(),
        &crate::task_service::WorkerFollowupFactRequest {
            schema: ProviderActionSchema::V2,
            action_id: ActionId::new(format!(
                "worker-followup-delivered:{}:{agent_bus_message_id}",
                metadata.notification_id.as_str()
            ))?,
            notification_id: metadata.notification_id.clone(),
            expected_notification_revision: notification.local_revision,
            kind: crate::task_service::CompletionNotificationFactKind::Delivered,
            reference: Some(format!("{agent_bus_message_id}:{native_submission_id}")),
        },
    )
}

pub(crate) fn resolve_current_runtime_target(
    state: &Arc<Mutex<AgentBusState>>,
    durable_session_id: &str,
) -> Option<(String, String)> {
    let sessions = load_cutex_session_store().ok();
    let current_runtime = sessions.as_ref().and_then(|store| {
        store
            .sessions
            .values()
            .find(|record| record.cutex_session_id == durable_session_id)
            .and_then(|record| record.current_runtime_agent_id.as_deref())
    });
    let state = state.lock().ok()?;
    let matches_durable = |agent: &crate::agent_bus::model::AgentBusAgent| {
        let roster_session = agent.session_id.as_deref()?;
        let direct = roster_session == durable_session_id;
        let mapped = if direct {
            true
        } else {
            sessions
                .as_ref()
                .and_then(|store| {
                    let key =
                        cutex_session_key_for_user_id_including_retired(store, roster_session)?;
                    store
                        .sessions
                        .get(&key)
                        .map(|record| record.cutex_session_id == durable_session_id)
                })
                .unwrap_or(false)
        };
        mapped.then_some(())
    };
    if let Some(current_runtime) = current_runtime {
        let agent = state.agents.get(current_runtime)?;
        matches_durable(agent)?;
        return Some((agent.id.clone(), agent.name.clone()));
    }
    // Legacy stores without a current-runtime fence remain usable only when
    // the roster has exactly one unambiguous durable-session candidate.
    let mut candidates = state
        .agents
        .values()
        .filter(|agent| matches_durable(agent).is_some());
    let candidate = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some((candidate.id.clone(), candidate.name.clone()))
}

pub(crate) fn completion_target_is_current(
    state: &Arc<Mutex<AgentBusState>>,
    durable_session_id: &str,
) -> bool {
    resolve_current_runtime_target(state, durable_session_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_revision::{Sha256, TaskId, TaskRevision};
    use crate::task_service::{
        CompletionPolicy, CompletionPolicyKind, CreateProjectRevisionRequest,
        CreateRevisionRequest, ProviderActionSchema, RetryDeliveryRequest, SeatId, SendAttemptId,
        WorkflowId,
    };
    use sha2::{Digest, Sha256 as Sha256Hasher};

    fn roster(id: &str, session_id: &str) -> crate::agent_bus::model::AgentBusAgent {
        crate::agent_bus::model::AgentBusAgent {
            id: id.to_string(),
            name: id.to_string(),
            base_name: Some(id.to_string()),
            thread_name: None,
            path_key: None,
            session_id: Some(session_id.to_string()),
            cutex_session_id: None,
            profile: "test".to_string(),
            cwd: "/tmp".to_string(),
            pid: 1,
            host_id: Some("test-host".to_string()),
            groups: Vec::new(),
            registration_class: crate::agent_bus::model::AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        }
    }

    fn sha(text: &str) -> Sha256 {
        Sha256::new(format!("{:x}", Sha256Hasher::digest(text.as_bytes()))).unwrap()
    }

    #[test]
    fn protected_assignment_carries_exact_contract_and_replays_after_provider_reopen() {
        let root = std::env::temp_dir().join(format!(
            "cutex-assignment-contract-delivery-{}",
            uuid::Uuid::new_v4()
        ));
        let provider = TaskServiceProvider::open(&root).unwrap();
        let coordinator_session =
            crate::role_revision::CutexSessionId::new("director-session").unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            coordinator_session,
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let contract = "# Opaque 合同\nRun exactly once. 🧭";
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-contract-task").unwrap(),
                    workflow_id: WorkflowId::new("contract-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-contract-delivery").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha(contract),
                    opaque_contract: contract.to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        let request = AssignAndDispatchRequest {
            schema: ProviderActionSchema::V2,
            action_id: ActionId::new("assign-contract-task").unwrap(),
            assignment_id: crate::task_service::AssignmentId::new("assignment-contract").unwrap(),
            task_id: TaskId::new("CUTEX-contract-delivery").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            assignee_cutex_session: crate::role_revision::CutexSessionId::new("worker-session")
                .unwrap(),
            send_attempt_id: SendAttemptId::new("send-contract").unwrap(),
            external_message_id: "external-contract-message".to_string(),
        };
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state.lock().unwrap().agents.insert(
            "runtime-worker".to_string(),
            roster("runtime-worker", "worker-session"),
        );

        let mut duplicate_summary_request = request.clone();
        duplicate_summary_request.action_id = ActionId::new("assign-duplicate-summary").unwrap();
        duplicate_summary_request.assignment_id =
            crate::task_service::AssignmentId::new("assignment-duplicate-summary").unwrap();
        duplicate_summary_request.send_attempt_id =
            SendAttemptId::new("send-duplicate-summary").unwrap();
        duplicate_summary_request.external_message_id = "external-duplicate-summary".to_string();
        assert_eq!(
            TaskServiceAgentBusDispatcher::assign_and_dispatch(
                &provider,
                &coordinator,
                &state,
                &duplicate_summary_request,
                1,
                contract,
                41,
            ),
            Err(AssignmentDispatchError::Contract(
                TaskServiceAssignmentContractError::SummaryDuplicatesContract
            ))
        );
        assert!(provider.query().unwrap().assignments.is_empty());

        let first = TaskServiceAgentBusDispatcher::assign_and_dispatch(
            &provider,
            &coordinator,
            &state,
            &request,
            1,
            "short human summary",
            42,
        )
        .unwrap();
        assert!(!first.deduplicated);
        let queued = state.lock().unwrap().messages["runtime-worker"][0].clone();
        assert_eq!(queued.content, "short human summary");
        let metadata: TaskServiceAssignmentMetadata =
            serde_json::from_value(queued.control_payload.unwrap()).unwrap();
        assert_eq!(metadata.opaque_contract.as_deref(), Some(contract));
        assert_eq!(metadata.contract_sha256, sha(contract));

        drop(provider);
        let reopened = TaskServiceProvider::open(&root).unwrap();
        let mut changed_replay = request.clone();
        changed_replay.task_id = TaskId::new("CUTEX-different-task").unwrap();
        assert_eq!(
            TaskServiceAgentBusDispatcher::assign_and_dispatch(
                &reopened,
                &coordinator,
                &state,
                &changed_replay,
                1,
                "short human summary",
                42,
            ),
            Err(AssignmentDispatchError::Provider(ProviderError::Conflict(
                "action_id_payload_conflict"
            )))
        );
        let replay = TaskServiceAgentBusDispatcher::assign_and_dispatch(
            &reopened,
            &coordinator,
            &state,
            &request,
            1,
            "short human summary",
            42,
        )
        .unwrap();
        assert!(replay.deduplicated);
        assert_eq!(replay.agent_bus_message_id, first.agent_bus_message_id);
        assert_eq!(state.lock().unwrap().messages["runtime-worker"].len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_retry_preserves_protected_scope_rebinds_and_replays_exactly_once() {
        let root = std::env::temp_dir().join(format!(
            "cutex-project-retry-delivery-{}",
            uuid::Uuid::new_v4()
        ));
        let provider = TaskServiceProvider::open(&root).unwrap();
        let coordinator_session =
            crate::role_revision::CutexSessionId::new("project-director").unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            coordinator_session,
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let contract = "project-scoped contract";
        provider
            .create_project_revision(
                &coordinator,
                &CreateProjectRevisionRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: ActionId::new("create-project-task").unwrap(),
                    project_id: project.clone(),
                    workflow_id: WorkflowId::new("project-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-project-retry").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha(contract),
                    opaque_contract: contract.to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id = crate::task_service::AssignmentId::new("project-assignment").unwrap();
        let initial_request = AssignProjectAndDispatchRequest {
            schema: ProviderActionSchema::V3,
            action_id: ActionId::new("assign-project-task").unwrap(),
            project_id: project.clone(),
            assignment_id: assignment_id.clone(),
            task_id: TaskId::new("CUTEX-project-retry").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            assignee_cutex_session: crate::role_revision::CutexSessionId::new("project-worker")
                .unwrap(),
            send_attempt_id: SendAttemptId::new("project-send-initial").unwrap(),
            external_message_id: "project-message-initial".to_string(),
        };
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state.lock().unwrap().agents.insert(
            "runtime-old".to_string(),
            roster("runtime-old", "project-worker"),
        );

        TaskServiceAgentBusDispatcher::assign_project_and_dispatch(
            &provider,
            &coordinator,
            &state,
            &initial_request,
            1,
            "initial project assignment",
            10,
        )
        .unwrap();
        let initial_metadata: TaskServiceAssignmentMetadata = serde_json::from_value(
            state.lock().unwrap().messages["runtime-old"][0]
                .control_payload
                .clone()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(initial_metadata.project_id.as_ref(), Some(&project));
        validate_assignment_metadata_with_provider(&provider, &initial_metadata).unwrap();

        {
            let mut bus = state.lock().unwrap();
            bus.agents.remove("runtime-old");
            bus.agents.insert(
                "runtime-current".to_string(),
                roster("runtime-current", "project-worker"),
            );
        }
        let retry_request = RetryDeliveryRequest {
            schema: ProviderActionSchema::V3,
            action_id: ActionId::new("retry-project-task").unwrap(),
            assignment_id: assignment_id.clone(),
            send_attempt_id: SendAttemptId::new("project-send-retry").unwrap(),
            external_message_id: "project-message-retry".to_string(),
        };
        let retry = TaskServiceAgentBusDispatcher::retry_delivery(
            &provider,
            &coordinator,
            &state,
            &retry_request,
            1,
            "retried project assignment",
            11,
        )
        .unwrap();
        assert_eq!(retry.target_runtime_diagnostic, "runtime-current");
        assert_eq!(
            retry.communication_receipt.schema,
            crate::task_service::ProviderReceiptSchema::V3
        );
        let retry_message = state.lock().unwrap().messages["runtime-current"][0].clone();
        let retry_metadata: TaskServiceAssignmentMetadata =
            serde_json::from_value(retry_message.control_payload.clone().unwrap()).unwrap();
        assert_eq!(retry_metadata.project_id, initial_metadata.project_id);
        validate_assignment_metadata_with_provider(&provider, &retry_metadata).unwrap();

        let mut missing_project = retry_metadata.clone();
        missing_project.project_id = None;
        assert_eq!(
            validate_assignment_metadata_with_provider(&provider, &missing_project),
            Err(ProviderError::Conflict("assignment_task_conflict"))
        );
        let mut forged_project = retry_metadata.clone();
        forged_project.project_id =
            Some(crate::agent_management::ProjectId::new("project-forged").unwrap());
        assert_eq!(
            validate_assignment_metadata_with_provider(&provider, &forged_project),
            Err(ProviderError::Conflict("assignment_task_conflict"))
        );

        let inserted = record_context_inserted_with_provider(
            &provider,
            &retry_metadata,
            &retry_message.id,
            "native-submission-project-retry",
        )
        .unwrap();
        assert_eq!(
            record_context_inserted_with_provider(
                &provider,
                &retry_metadata,
                &retry_message.id,
                "native-submission-project-retry",
            )
            .unwrap(),
            inserted
        );
        assert_eq!(
            provider.query().unwrap().send_attempts[&retry_request.send_attempt_id]
                .events
                .iter()
                .filter(|event| event.kind == CommunicationEventKind::ContextInserted)
                .count(),
            1
        );

        let replay = TaskServiceAgentBusDispatcher::retry_delivery(
            &provider,
            &coordinator,
            &state,
            &retry_request,
            1,
            "retried project assignment",
            12,
        )
        .unwrap();
        assert!(replay.deduplicated);
        assert_eq!(replay.agent_bus_message_id, retry.agent_bus_message_id);
        assert_eq!(state.lock().unwrap().messages["runtime-current"].len(), 1);

        let legacy_root = std::env::temp_dir().join(format!(
            "cutex-legacy-retry-delivery-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy_provider = TaskServiceProvider::open(&legacy_root).unwrap();
        legacy_provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-legacy-task").unwrap(),
                    workflow_id: WorkflowId::new("legacy-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-legacy-retry").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("legacy contract"),
                    opaque_contract: "legacy contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        let legacy_state = Arc::new(Mutex::new(AgentBusState::default()));
        legacy_state.lock().unwrap().agents.insert(
            "runtime-legacy".to_string(),
            roster("runtime-legacy", "legacy-worker"),
        );
        TaskServiceAgentBusDispatcher::assign_and_dispatch(
            &legacy_provider,
            &coordinator,
            &legacy_state,
            &AssignAndDispatchRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new("assign-legacy-task").unwrap(),
                assignment_id: crate::task_service::AssignmentId::new("legacy-assignment").unwrap(),
                task_id: TaskId::new("CUTEX-legacy-retry").unwrap(),
                task_revision: TaskRevision::new(1).unwrap(),
                assignee_cutex_session: crate::role_revision::CutexSessionId::new("legacy-worker")
                    .unwrap(),
                send_attempt_id: SendAttemptId::new("legacy-send-initial").unwrap(),
                external_message_id: "legacy-message-initial".to_string(),
            },
            1,
            "initial legacy assignment",
            20,
        )
        .unwrap();
        let legacy_retry = TaskServiceAgentBusDispatcher::retry_delivery(
            &legacy_provider,
            &coordinator,
            &legacy_state,
            &RetryDeliveryRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new("retry-legacy-task").unwrap(),
                assignment_id: crate::task_service::AssignmentId::new("legacy-assignment").unwrap(),
                send_attempt_id: SendAttemptId::new("legacy-send-retry").unwrap(),
                external_message_id: "legacy-message-retry".to_string(),
            },
            1,
            "retried legacy assignment",
            21,
        )
        .unwrap();
        assert_eq!(
            legacy_retry.communication_receipt.schema,
            crate::task_service::ProviderReceiptSchema::V2
        );
        let legacy_metadata: TaskServiceAssignmentMetadata = serde_json::from_value(
            legacy_state.lock().unwrap().messages["runtime-legacy"][1]
                .control_payload
                .clone()
                .unwrap(),
        )
        .unwrap();
        assert!(legacy_metadata.project_id.is_none());
        assert_eq!(legacy_metadata.schema, TASK_SERVICE_PROVIDER_ACTION_SCHEMA);
        validate_assignment_metadata_with_provider(&legacy_provider, &legacy_metadata).unwrap();
        std::fs::remove_dir_all(legacy_root).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_assignment_contract_fails_typed_before_assignment_mutation() {
        let root = std::env::temp_dir().join(format!(
            "cutex-oversized-assignment-contract-{}",
            uuid::Uuid::new_v4()
        ));
        let provider = TaskServiceProvider::open(&root).unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            crate::role_revision::CutexSessionId::new("director-session").unwrap(),
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let contract =
            "x".repeat(crate::agent_bus::model::TASK_SERVICE_ASSIGNMENT_CONTRACT_MAX_BYTES + 1);
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-oversized-task").unwrap(),
                    workflow_id: WorkflowId::new("oversized-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-oversized-contract").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha(&contract),
                    opaque_contract: contract,
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        let request = AssignAndDispatchRequest {
            schema: ProviderActionSchema::V2,
            action_id: ActionId::new("assign-oversized-task").unwrap(),
            assignment_id: crate::task_service::AssignmentId::new("assignment-oversized").unwrap(),
            task_id: TaskId::new("CUTEX-oversized-contract").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            assignee_cutex_session: crate::role_revision::CutexSessionId::new("worker-session")
                .unwrap(),
            send_attempt_id: SendAttemptId::new("send-oversized").unwrap(),
            external_message_id: "external-oversized-message".to_string(),
        };
        let result = TaskServiceAgentBusDispatcher::assign_and_dispatch(
            &provider,
            &coordinator,
            &Arc::new(Mutex::new(AgentBusState::default())),
            &request,
            1,
            "summary",
            42,
        );
        assert!(matches!(
            result,
            Err(AssignmentDispatchError::Contract(
                TaskServiceAssignmentContractError::TooLarge { .. }
            ))
        ));
        let snapshot = provider.query().unwrap();
        assert!(snapshot.assignments.is_empty());
        assert!(snapshot.send_attempts.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn context_inserted_is_durable_and_exact_replay_does_not_duplicate() {
        let root =
            std::env::temp_dir().join(format!("cutex-context-inserted-{}", uuid::Uuid::new_v4()));
        let provider = TaskServiceProvider::open(&root).unwrap();
        let coordinator_session =
            crate::role_revision::CutexSessionId::new("cutex.director").unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            coordinator_session.clone(),
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create").unwrap(),
                    workflow_id: WorkflowId::new("workflow-1").unwrap(),
                    task_id: TaskId::new("CUTEX-test").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("contract"),
                    opaque_contract: "contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id = crate::task_service::AssignmentId::new("assignment-1").unwrap();
        let send_attempt_id = SendAttemptId::new("send-1").unwrap();
        provider
            .assign_and_dispatch(
                &coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign").unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id: TaskId::new("CUTEX-test").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: crate::role_revision::CutexSessionId::new(
                        "cutex.worker",
                    )
                    .unwrap(),
                    send_attempt_id: send_attempt_id.clone(),
                    external_message_id: "external-message-1".to_string(),
                },
                1,
                "assignment",
            )
            .unwrap();
        provider
            .record_communication_event(
                &AuthenticatedPrincipal::task_service_system(),
                &CommunicationEventRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("bus-queued").unwrap(),
                    send_attempt_id: send_attempt_id.clone(),
                    expected_send_attempt_revision: 1,
                    kind: CommunicationEventKind::BusQueued,
                    receipt_reference: Some("bus-message-1".to_string()),
                },
            )
            .unwrap();
        let metadata = TaskServiceAssignmentMetadata {
            schema: TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            project_id: None,
            coordinator_cutex_session: Some(coordinator_session),
            assignment_id,
            task_id: TaskId::new("CUTEX-test").unwrap(),
            task_revision: TaskRevision::new(1).unwrap(),
            contract_sha256: sha("contract"),
            opaque_contract: Some("contract".to_string()),
            send_attempt_id: send_attempt_id.clone(),
        };

        let mut tampered = metadata.clone();
        tampered.opaque_contract = Some("tampered".to_string());
        assert_eq!(
            record_context_inserted_with_provider(
                &provider,
                &tampered,
                "bus-message-1",
                "native-submission-tampered",
            ),
            Err(ProviderError::Conflict("assignment_contract_invalid"))
        );
        let mut mismatched_revision = metadata.clone();
        mismatched_revision.task_revision = TaskRevision::new(2).unwrap();
        assert_eq!(
            validate_assignment_metadata_with_provider(&provider, &mismatched_revision),
            Err(ProviderError::Conflict("assignment_task_conflict"))
        );

        let first = record_context_inserted_with_provider(
            &provider,
            &metadata,
            "bus-message-1",
            "native-submission-1",
        )
        .unwrap();
        let replay = record_context_inserted_with_provider(
            &provider,
            &metadata,
            "bus-message-1",
            "native-submission-1",
        )
        .unwrap();
        assert_eq!(first, replay);
        let send = &provider.query().unwrap().send_attempts[&send_attempt_id];
        assert_eq!(
            send.events
                .iter()
                .filter(|event| event.kind == CommunicationEventKind::ContextInserted)
                .count(),
            1
        );
        assert_eq!(
            send.events.last().unwrap().receipt_reference.as_deref(),
            Some("native-submission-1")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completion_dispatch_retries_after_failure_rebinds_seat_and_deduplicates_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "cutex-completion-dispatch-{}",
            uuid::Uuid::new_v4()
        ));
        let provider_root = root.join("provider");
        let provider = TaskServiceProvider::open(&provider_root).unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let director_session =
            crate::role_revision::CutexSessionId::new("director-session").unwrap();
        let worker_session = crate::role_revision::CutexSessionId::new("worker-session").unwrap();
        let release_old = crate::role_revision::CutexSessionId::new("release-old").unwrap();
        let release_new = crate::role_revision::CutexSessionId::new("release-new").unwrap();
        let director_seat = SeatId::new("cutex-director").unwrap();
        let release_seat = SeatId::new("cutex-release").unwrap();
        let coordinator =
            AuthenticatedPrincipal::seated_session(director_session, director_seat.clone(), 1)
                .unwrap();
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create").unwrap(),
                    workflow_id: WorkflowId::new("workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-completion").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("completion contract"),
                    opaque_contract: "completion contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: release_seat.clone(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id =
            crate::task_service::AssignmentId::new("assignment-completion").unwrap();
        provider
            .assign_and_dispatch(
                &coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign").unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id: TaskId::new("CUTEX-completion").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: worker_session.clone(),
                    send_attempt_id: SendAttemptId::new("send-completion").unwrap(),
                    external_message_id: "assignment-message".to_string(),
                },
                1,
                "assignment",
            )
            .unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session);
        for action in [
            crate::task_service::WorkerActionRequest::Start(
                crate::task_service::AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("start").unwrap(),
                    assignment_id: assignment_id.clone(),
                },
            ),
            crate::task_service::WorkerActionRequest::Submit(
                crate::task_service::SubmitActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("submit").unwrap(),
                    assignment_id: assignment_id.clone(),
                    result_sha256: sha("result"),
                    result_reference: "result".to_string(),
                },
            ),
        ] {
            let prepared = provider
                .prepare_worker_action(
                    &worker,
                    &crate::task_service::WorkerPrepareRequest {
                        schema: crate::task_service::WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap();
            let crate::task_service::WorkerPrepareOutcome::Prepared(envelope) = prepared else {
                panic!("new action must prepare")
            };
            provider.execute_worker_action(&worker, &envelope).unwrap();
        }
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("bind-old").unwrap(),
                seat_id: release_seat.clone(),
                occupant_cutex_session: release_old,
            })
            .unwrap();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let first = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &provider, &seats, &state, 1,
        )
        .unwrap();
        assert_eq!(first.uncertain, 1);
        assert_eq!(provider.query().unwrap().completion_notifications.len(), 1);

        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("bind-new").unwrap(),
                seat_id: release_seat,
                occupant_cutex_session: release_new.clone(),
            })
            .unwrap();
        state.lock().unwrap().agents.insert(
            "release-runtime".to_string(),
            roster("release-runtime", release_new.as_str()),
        );
        let reopened = TaskServiceProvider::open(&provider_root).unwrap();
        let second = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &reopened, &seats, &state, 2,
        )
        .unwrap();
        assert_eq!(second.queued, 1);
        let queued = state.lock().unwrap().messages["release-runtime"][0].clone();
        assert_eq!(
            queued.delivery_mode,
            crate::agent_bus::delivery::AgentDeliveryMode::AfterTurn
        );
        assert_eq!(
            queued.sender_kind,
            crate::agent_bus::model::AgentMessageKind::TaskServiceSystem
        );
        assert_eq!(
            queued.control_type.as_deref(),
            Some("cutex.task_service.completion.v1")
        );
        let third = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &reopened, &seats, &state, 3,
        )
        .unwrap();
        assert_eq!(third.queued, 1);
        assert_eq!(third.deduplicated, 1);
        assert_eq!(state.lock().unwrap().messages["release-runtime"].len(), 1);

        let restarted_state = Arc::new(Mutex::new(AgentBusState::default()));
        restarted_state.lock().unwrap().agents.insert(
            "release-runtime".to_string(),
            roster("release-runtime", release_new.as_str()),
        );
        let after_restart =
            TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
                &reopened,
                &seats,
                &restarted_state,
                4,
            )
            .unwrap();
        assert_eq!(after_restart.queued, 1);
        assert_eq!(after_restart.deduplicated, 0);
        assert_eq!(
            restarted_state.lock().unwrap().messages["release-runtime"][0].id,
            queued.id
        );
        let metadata: TaskServiceCompletionMetadata =
            serde_json::from_value(queued.control_payload.clone().unwrap()).unwrap();
        let delivered = record_completion_context_inserted_with_provider(
            &reopened,
            &metadata,
            &queued.id,
            "native-submission-1",
        )
        .unwrap();
        let replay = record_completion_context_inserted_with_provider(
            &reopened,
            &metadata,
            &queued.id,
            "native-submission-1",
        )
        .unwrap();
        assert_eq!(delivered, replay);
        let final_notification = reopened
            .query()
            .unwrap()
            .completion_notifications
            .get(&metadata.notification_id)
            .unwrap()
            .clone();
        assert_eq!(
            final_notification
                .facts
                .iter()
                .filter(|fact| {
                    fact.kind == crate::task_service::CompletionNotificationFactKind::Delivered
                })
                .count(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blocked_completion_wakes_the_director_with_actionable_bounded_context() {
        let root = std::env::temp_dir().join(format!(
            "cutex-blocked-completion-dispatch-{}",
            uuid::Uuid::new_v4()
        ));
        let provider = TaskServiceProvider::open(root.join("provider")).unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let director_session =
            crate::role_revision::CutexSessionId::new("director-session").unwrap();
        let worker_session = crate::role_revision::CutexSessionId::new("worker-session").unwrap();
        let director_seat = SeatId::new("cutex-director").unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            director_session.clone(),
            director_seat.clone(),
            1,
        )
        .unwrap();
        let task_id = TaskId::new("CUTEX-blocked-completion").unwrap();
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-blocked").unwrap(),
                    workflow_id: WorkflowId::new("blocked-workflow").unwrap(),
                    task_id: task_id.clone(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("blocked contract"),
                    opaque_contract: "blocked contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: director_seat.clone(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id = crate::task_service::AssignmentId::new("assignment-blocked").unwrap();
        provider
            .assign_and_dispatch(
                &coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign-blocked").unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id,
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: worker_session.clone(),
                    send_attempt_id: SendAttemptId::new("send-blocked").unwrap(),
                    external_message_id: "assignment-blocked-message".to_string(),
                },
                1,
                "assignment",
            )
            .unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session);
        for action in [
            crate::task_service::WorkerActionRequest::Start(
                crate::task_service::AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("start-blocked").unwrap(),
                    assignment_id: assignment_id.clone(),
                },
            ),
            crate::task_service::WorkerActionRequest::Block(
                crate::task_service::BlockActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("block-windows-lifecycle").unwrap(),
                    assignment_id: assignment_id.clone(),
                    summary: "typed Windows service stop is unavailable".to_string(),
                },
            ),
        ] {
            let crate::task_service::WorkerPrepareOutcome::Prepared(envelope) = provider
                .prepare_worker_action(
                    &worker,
                    &crate::task_service::WorkerPrepareRequest {
                        schema: crate::task_service::WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap()
            else {
                panic!("new Worker action must prepare")
            };
            provider.execute_worker_action(&worker, &envelope).unwrap();
        }
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("bind-director").unwrap(),
                seat_id: director_seat,
                occupant_cutex_session: director_session.clone(),
            })
            .unwrap();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state.lock().unwrap().agents.insert(
            "director-runtime".to_string(),
            roster("director-runtime", director_session.as_str()),
        );

        let dispatched = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &provider, &seats, &state, 1,
        )
        .unwrap();
        assert_eq!(dispatched.queued, 1);
        let message = state.lock().unwrap().messages["director-runtime"][0].clone();
        assert_eq!(
            message.delivery_mode,
            crate::agent_bus::delivery::AgentDeliveryMode::Soon
        );
        assert!(message.trigger_turn);
        assert_eq!(
            message.external_action_id.as_deref(),
            Some("block-windows-lifecycle")
        );
        assert!(message
            .content
            .contains("Blocker summary: typed Windows service stop is unavailable"));
        assert!(message
            .content
            .contains("Transition action identity: block-windows-lifecycle."));
        assert!(message.content.contains("Director action required:"));
        let metadata: TaskServiceCompletionMetadata =
            serde_json::from_value(message.control_payload.unwrap()).unwrap();
        assert_eq!(
            metadata.kind,
            crate::task_service::CompletionNotificationKind::Blocked
        );
        assert_eq!(
            metadata.transition_action_id.as_str(),
            "block-windows-lifecycle"
        );
        assert_eq!(metadata.target_seat_id.as_str(), "cutex-director");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_followup_retries_exact_assignee_and_context_insert_is_exactly_once() {
        let root = std::env::temp_dir().join(format!(
            "cutex-worker-followup-dispatch-{}",
            uuid::Uuid::new_v4()
        ));
        let provider_root = root.join("provider");
        let provider = TaskServiceProvider::open(&provider_root).unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let director_session =
            crate::role_revision::CutexSessionId::new("director-session").unwrap();
        let worker_session = crate::role_revision::CutexSessionId::new("worker-session").unwrap();
        let director_seat = SeatId::new("director").unwrap();
        let authority =
            AuthenticatedPrincipal::seated_session(director_session, director_seat.clone(), 1)
                .unwrap();
        provider
            .create_revision(
                &authority,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-followup").unwrap(),
                    workflow_id: WorkflowId::new("workflow-followup").unwrap(),
                    task_id: TaskId::new("CUTEX-followup").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("followup contract"),
                    opaque_contract: "followup contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: director_seat,
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id = crate::task_service::AssignmentId::new("assignment-followup").unwrap();
        provider
            .assign_and_dispatch(
                &authority,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign-followup").unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id: TaskId::new("CUTEX-followup").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: worker_session.clone(),
                    send_attempt_id: SendAttemptId::new("send-followup").unwrap(),
                    external_message_id: "assignment-followup-message".to_string(),
                },
                1,
                "assignment",
            )
            .unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session.clone());
        for action in [
            crate::task_service::WorkerActionRequest::Start(
                crate::task_service::AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("start-followup").unwrap(),
                    assignment_id: assignment_id.clone(),
                },
            ),
            crate::task_service::WorkerActionRequest::Submit(
                crate::task_service::SubmitActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("submit-followup").unwrap(),
                    assignment_id: assignment_id.clone(),
                    result_sha256: sha("result"),
                    result_reference: "result".to_string(),
                },
            ),
        ] {
            let crate::task_service::WorkerPrepareOutcome::Prepared(envelope) = provider
                .prepare_worker_action(
                    &worker,
                    &crate::task_service::WorkerPrepareRequest {
                        schema: crate::task_service::WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap()
            else {
                panic!("new Worker action must prepare")
            };
            provider.execute_worker_action(&worker, &envelope).unwrap();
        }
        let context = provider
            .worker_context(
                &worker,
                &crate::task_service::WorkerContextRequest {
                    schema: crate::task_service::WorkerContextRequestSchema::V2,
                    assignment_id: assignment_id.clone(),
                },
            )
            .unwrap()
            .context;
        provider
            .execute_terminal_action(
                &authority,
                &crate::task_service::TerminalActionEnvelope {
                    schema: crate::task_service::TerminalRequestSchema::V2,
                    command: crate::task_service::TerminalAuthorityRequest::RequestChanges(
                        crate::task_service::TerminalActionRequest {
                            schema: ProviderActionSchema::V2,
                            action_id: ActionId::new("request-changes-followup").unwrap(),
                            assignment_id: assignment_id.clone(),
                            decision_reference: Some("repair the bounded regression".to_string()),
                        },
                    ),
                    context,
                },
            )
            .unwrap();

        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let offline = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &provider, &seats, &state, 1,
        )
        .unwrap();
        assert_eq!(offline.target_unavailable, 2); // review-ready seat plus Worker
        assert!(offline
            .unavailable_target_sessions
            .contains(worker_session.as_str()));
        state.lock().unwrap().agents.insert(
            "worker-runtime".to_string(),
            roster("worker-runtime", worker_session.as_str()),
        );
        let queued = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &provider, &seats, &state, 2,
        )
        .unwrap();
        assert_eq!(queued.queued, 1);
        let message = state.lock().unwrap().messages["worker-runtime"][0].clone();
        assert_eq!(
            message.delivery_mode,
            crate::agent_bus::delivery::AgentDeliveryMode::Soon
        );
        assert_eq!(
            message.control_type.as_deref(),
            Some("cutex.task_service.worker_followup.v1")
        );
        let metadata: TaskServiceWorkerFollowupMetadata =
            serde_json::from_value(message.control_payload.clone().unwrap()).unwrap();
        assert_eq!(metadata.assignment_id, assignment_id);
        assert_eq!(metadata.attempt_number.get(), 1);
        assert_eq!(metadata.decision_reference, "repair the bounded regression");
        assert_eq!(
            validate_worker_followup_metadata_with_provider(&provider, &metadata, "other-worker"),
            Err(ProviderError::Conflict("worker_followup_metadata_conflict"))
        );
        validate_worker_followup_metadata_with_provider(
            &provider,
            &metadata,
            worker_session.as_str(),
        )
        .unwrap();
        let first = {
            validate_worker_followup_metadata_with_provider(
                &provider,
                &metadata,
                worker_session.as_str(),
            )
            .unwrap();
            let snapshot = provider.query().unwrap();
            let notification = &snapshot.worker_followup_notifications[&metadata.notification_id];
            provider
                .record_worker_followup_fact(
                    &AuthenticatedPrincipal::task_service_system(),
                    &crate::task_service::WorkerFollowupFactRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: ActionId::new(format!(
                            "worker-followup-delivered:{}:{}",
                            metadata.notification_id.as_str(),
                            message.id
                        ))
                        .unwrap(),
                        notification_id: metadata.notification_id.clone(),
                        expected_notification_revision: notification.local_revision,
                        kind: crate::task_service::CompletionNotificationFactKind::Delivered,
                        reference: Some(format!("{}:native-1", message.id)),
                    },
                )
                .unwrap()
        };
        let replay = provider
            .record_worker_followup_fact(
                &AuthenticatedPrincipal::task_service_system(),
                &crate::task_service::WorkerFollowupFactRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new(format!(
                        "worker-followup-delivered:{}:{}",
                        metadata.notification_id.as_str(),
                        message.id
                    ))
                    .unwrap(),
                    notification_id: metadata.notification_id.clone(),
                    expected_notification_revision: 1,
                    kind: crate::task_service::CompletionNotificationFactKind::Delivered,
                    reference: Some(format!("{}:native-1", message.id)),
                },
            )
            .unwrap();
        assert_eq!(first, replay);
        let final_notification = provider.query().unwrap().worker_followup_notifications
            [&metadata.notification_id]
            .clone();
        assert_eq!(
            final_notification
                .facts
                .iter()
                .filter(|fact| fact.kind
                    == crate::task_service::CompletionNotificationFactKind::Delivered)
                .count(),
            1
        );
        let restarted_state = Arc::new(Mutex::new(AgentBusState::default()));
        restarted_state.lock().unwrap().agents.insert(
            "worker-runtime".to_string(),
            roster("worker-runtime", worker_session.as_str()),
        );
        let reopened = TaskServiceProvider::open(&provider_root).unwrap();
        let recovered = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &reopened,
            &seats,
            &restarted_state,
            3,
        )
        .unwrap();
        assert_eq!(recovered.queued, 0);
        assert!(restarted_state.lock().unwrap().messages.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v3_dispatch_quarantines_legacy_aggregate_and_drains_unrelated_scoped_records() {
        let root = std::env::temp_dir().join(format!(
            "cutex-legacy-completion-dispatch-{}",
            uuid::Uuid::new_v4()
        ));
        let provider = TaskServiceProvider::open(root.join("provider")).unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats")).unwrap();
        let director_session =
            crate::role_revision::CutexSessionId::new("legacy-director-session").unwrap();
        let worker_session =
            crate::role_revision::CutexSessionId::new("legacy-worker-session").unwrap();
        let director_seat = SeatId::new("cutex-director").unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            director_session.clone(),
            director_seat.clone(),
            1,
        )
        .unwrap();
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("legacy-create").unwrap(),
                    workflow_id: WorkflowId::new("legacy-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-legacy-completion").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("legacy completion contract"),
                    opaque_contract: "legacy completion contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: director_seat.clone(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id =
            crate::task_service::AssignmentId::new("legacy-completion-assignment").unwrap();
        provider
            .assign_and_dispatch(
                &coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("legacy-assign").unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id: TaskId::new("CUTEX-legacy-completion").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: worker_session.clone(),
                    send_attempt_id: SendAttemptId::new("legacy-completion-send").unwrap(),
                    external_message_id: "legacy-completion-message".to_string(),
                },
                1,
                "legacy assignment",
            )
            .unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session);
        for action in [
            crate::task_service::WorkerActionRequest::Start(
                crate::task_service::AssignmentActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("legacy-start").unwrap(),
                    assignment_id: assignment_id.clone(),
                },
            ),
            crate::task_service::WorkerActionRequest::Submit(
                crate::task_service::SubmitActionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("legacy-submit").unwrap(),
                    assignment_id: assignment_id.clone(),
                    result_sha256: sha("legacy-result"),
                    result_reference: "legacy-result".to_string(),
                },
            ),
        ] {
            let prepared = provider
                .prepare_worker_action(
                    &worker,
                    &crate::task_service::WorkerPrepareRequest {
                        schema: crate::task_service::WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap();
            let crate::task_service::WorkerPrepareOutcome::Prepared(envelope) = prepared else {
                panic!("legacy action must prepare")
            };
            provider.execute_worker_action(&worker, &envelope).unwrap();
        }
        let legacy_context = provider
            .worker_context(
                &worker,
                &crate::task_service::WorkerContextRequest {
                    schema: crate::task_service::WorkerContextRequestSchema::V2,
                    assignment_id: assignment_id.clone(),
                },
            )
            .unwrap()
            .context;
        provider
            .execute_terminal_action(
                &coordinator,
                &crate::task_service::TerminalActionEnvelope {
                    schema: crate::task_service::TerminalRequestSchema::V2,
                    command: crate::task_service::TerminalAuthorityRequest::RequestChanges(
                        crate::task_service::TerminalActionRequest {
                            schema: ProviderActionSchema::V2,
                            action_id: ActionId::new("legacy-request-changes").unwrap(),
                            assignment_id: assignment_id.clone(),
                            decision_reference: Some("legacy correction".to_string()),
                        },
                    ),
                    context: legacy_context,
                },
            )
            .unwrap();
        let legacy = provider.query().unwrap();
        assert_eq!(legacy.completion_notifications.len(), 1);
        assert_eq!(legacy.worker_followup_notifications.len(), 1);
        assert!(
            legacy
                .completion_notifications
                .values()
                .all(|notification| notification.project_id.is_none()
                    && notification.facts.is_empty())
        );
        assert!(
            legacy
                .worker_followup_notifications
                .values()
                .all(|notification| notification.project_id.is_none()
                    && notification.facts.is_empty())
        );

        provider
            .create_project_revision(
                &coordinator,
                &crate::task_service::CreateProjectRevisionRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: ActionId::new("activate-v3").unwrap(),
                    project_id: crate::agent_management::ProjectId::new("project-alpha").unwrap(),
                    workflow_id: WorkflowId::new("project-workflow").unwrap(),
                    task_id: TaskId::new("CUTEX-project-activation").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("project activation contract"),
                    opaque_contract: "project activation contract".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: director_seat.clone(),
                    },
                },
                None,
            )
            .unwrap();
        let project_id = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let scoped_assignment_id =
            crate::task_service::AssignmentId::new("scoped-completion-assignment").unwrap();
        let scoped_worker_session =
            crate::role_revision::CutexSessionId::new("scoped-worker-session").unwrap();
        provider
            .assign_project_and_dispatch(
                &coordinator,
                &AssignProjectAndDispatchRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: ActionId::new("scoped-assign").unwrap(),
                    project_id,
                    assignment_id: scoped_assignment_id.clone(),
                    task_id: TaskId::new("CUTEX-project-activation").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    assignee_cutex_session: scoped_worker_session.clone(),
                    send_attempt_id: SendAttemptId::new("scoped-send").unwrap(),
                    external_message_id: "scoped-assignment-message".to_string(),
                },
                1,
                "scoped assignment",
            )
            .unwrap();
        let scoped_worker = AuthenticatedPrincipal::session(scoped_worker_session);
        let prepared = provider
            .prepare_worker_action(
                &scoped_worker,
                &crate::task_service::WorkerPrepareRequest {
                    schema: crate::task_service::WorkerPrepareRequestSchema::V2,
                    action: crate::task_service::WorkerActionRequest::Decline(
                        crate::task_service::AssignmentActionRequest {
                            schema: ProviderActionSchema::V2,
                            action_id: ActionId::new("scoped-decline").unwrap(),
                            assignment_id: scoped_assignment_id,
                        },
                    ),
                },
            )
            .unwrap();
        let crate::task_service::WorkerPrepareOutcome::Prepared(envelope) = prepared else {
            panic!("scoped decline must prepare")
        };
        provider
            .execute_worker_action(&scoped_worker, &envelope)
            .unwrap();
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("bind-live-director").unwrap(),
                seat_id: director_seat,
                occupant_cutex_session: director_session.clone(),
            })
            .unwrap();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state.lock().unwrap().agents.insert(
            "live-director-runtime".to_string(),
            roster("live-director-runtime", director_session.as_str()),
        );
        let provider_before = provider.query().unwrap();
        let legacy_ids = provider_before
            .completion_notifications
            .values()
            .filter(|notification| notification.project_id.is_none())
            .map(|notification| notification.notification_id.as_str().to_string())
            .chain(
                provider_before
                    .worker_followup_notifications
                    .values()
                    .filter(|notification| notification.project_id.is_none())
                    .map(|notification| notification.notification_id.as_str().to_string()),
            )
            .collect::<BTreeSet<_>>();
        assert_eq!(legacy_ids.len(), 2);
        assert!(state.lock().unwrap().messages.is_empty());
        assert!(state.lock().unwrap().recent_sends.is_empty());

        let first = TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            &provider, &seats, &state, 10,
        )
        .unwrap();
        assert_eq!(first.legacy_quarantined, 2);
        assert_eq!(first.legacy_quarantined_notification_ids, legacy_ids);
        assert_eq!(first.queued, 1);
        assert_eq!(
            state.lock().unwrap().messages["live-director-runtime"].len(),
            1
        );

        let after_first = provider.query().unwrap();
        for id in &legacy_ids {
            if let Ok(id) = crate::task_service::NotificationId::new(id.clone()) {
                if let Some(notification) = after_first.completion_notifications.get(&id) {
                    assert!(notification.facts.is_empty());
                }
                if let Some(notification) = after_first.worker_followup_notifications.get(&id) {
                    assert!(notification.facts.is_empty());
                }
            }
        }

        drop(provider);
        let reopened = Arc::new(TaskServiceProvider::open(root.join("provider")).unwrap());
        let seats = Arc::new(seats);
        let mut threads = Vec::new();
        for now_epoch_secs in [11, 12] {
            let provider = Arc::clone(&reopened);
            let seats = Arc::clone(&seats);
            let state = Arc::clone(&state);
            threads.push(std::thread::spawn(move || {
                TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
                    &provider,
                    &seats,
                    &state,
                    now_epoch_secs,
                )
                .unwrap()
            }));
        }
        for thread in threads {
            let summary = thread.join().unwrap();
            assert_eq!(summary.legacy_quarantined, 2);
            assert_eq!(summary.queued, 1);
            assert_eq!(summary.deduplicated, 1);
        }
        assert_eq!(
            state.lock().unwrap().messages["live-director-runtime"].len(),
            1
        );
        let after_restart = reopened.query().unwrap();
        assert_eq!(
            after_restart
                .completion_notifications
                .values()
                .filter(|notification| notification.project_id.is_none())
                .collect::<Vec<_>>(),
            provider_before
                .completion_notifications
                .values()
                .filter(|notification| notification.project_id.is_none())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            after_restart
                .worker_followup_notifications
                .values()
                .filter(|notification| notification.project_id.is_none())
                .collect::<Vec<_>>(),
            provider_before
                .worker_followup_notifications
                .values()
                .filter(|notification| notification.project_id.is_none())
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
