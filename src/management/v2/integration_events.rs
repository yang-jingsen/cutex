//! Owner-scoped typed facts emitted by Cutex integration services.

use anyhow::Context;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};

use crate::agent_management::{
    AgentActionPhase, AgentManagementOutcome, AgentManagementPhaseEvent, AgentManagementResponse,
    AgentOperationKind, AgentReplacePolicy, DirectorRotateMode,
};
use crate::app_server::commands::ParticipantPresentationMetadata;
use crate::app_server::participants::{
    ParticipantMetadataResolver, RegistryParticipantMetadataResolver,
};
use crate::platform::host::current_host_name;
use crate::role_revision::CutexSessionId;
use crate::task_service::{
    AssignmentId, ClosureReason, ProviderReceipt, ProviderResult, TaskServiceSnapshot,
};

use super::model::{CutexMessage, EventCorrelation, EventEnvelope, EventSource, PendingEvent};
use super::repository::management_v2_repository;
use super::repository::{EventRepository, ReplayQuery};

const INTEGRATION_DEDUPE_STATE: &str = "integration-event-dedupe-v1.json";
const INTEGRATION_DEDUPE_LOCK: &str = "integration-event-dedupe-v1.lock";

pub const AGENT_MANAGEMENT_PHASE_TRANSITION_METHOD: &str =
    "cutex/agentManagement/actionPhaseTransitionCommitted";
pub const AGENT_MANAGEMENT_PHASE_TRANSITION_SCHEMA: &str =
    "cutex/agent-management-phase-transition/v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentManagementPhaseTransitionFact {
    pub schema: String,
    pub phase_event_id: String,
    pub action_id: String,
    pub project_id: String,
    pub operation: AgentOperationKind,
    pub phase: AgentActionPhase,
    pub phase_sequence: u64,
    pub committed_at: String,
    pub primary_presentation_target_cutex_session_id: String,
    pub primary_presentation_target_metadata: Option<ParticipantPresentationMetadata>,
    #[serde(default)]
    pub subject_cutex_session_id: Option<String>,
    #[serde(default)]
    pub subject_agent_name: Option<String>,
    pub predecessor_cutex_session_id: Option<String>,
    pub predecessor_metadata: Option<ParticipantPresentationMetadata>,
    pub successor_cutex_session_id: Option<String>,
    pub successor_metadata: Option<ParticipantPresentationMetadata>,
    pub replace_policy: Option<AgentReplacePolicy>,
    pub rotation_mode: Option<DirectorRotateMode>,
    pub authority_epoch: Option<u64>,
}

pub fn append_agent_management_phase(
    phase: &AgentManagementPhaseEvent,
) -> anyhow::Result<EventEnvelope> {
    let resolver = RegistryParticipantMetadataResolver;
    let owner = phase.presentation_owner_cutex_session_id.as_str();
    let fact = AgentManagementPhaseTransitionFact {
        schema: AGENT_MANAGEMENT_PHASE_TRANSITION_SCHEMA.to_string(),
        phase_event_id: phase.event_id.clone(),
        action_id: phase.action_id.as_str().to_string(),
        project_id: phase.project_id.as_str().to_string(),
        operation: phase.operation,
        phase: phase.phase,
        phase_sequence: phase.phase_sequence,
        committed_at: phase.committed_at.as_str().to_string(),
        primary_presentation_target_cutex_session_id: owner.to_string(),
        primary_presentation_target_metadata: Some(resolver.resolve(owner)),
        subject_cutex_session_id: phase
            .subject_cutex_session_id
            .as_ref()
            .map(|session| session.as_str().to_string()),
        subject_agent_name: phase.subject_agent_name.clone(),
        predecessor_cutex_session_id: phase
            .predecessor_cutex_session_id
            .as_ref()
            .map(|session| session.as_str().to_string()),
        predecessor_metadata: phase
            .predecessor_cutex_session_id
            .as_ref()
            .map(|session| resolver.resolve(session.as_str())),
        successor_cutex_session_id: phase
            .successor_cutex_session_id
            .as_ref()
            .map(|session| session.as_str().to_string()),
        successor_metadata: phase
            .successor_cutex_session_id
            .as_ref()
            .map(|session| resolver.resolve(session.as_str())),
        replace_policy: phase.replace_policy,
        rotation_mode: phase.rotation_mode,
        authority_epoch: phase.authority_epoch,
    };
    let repository = management_v2_repository()?;
    append_owner_event_once_with_repository(
        repository,
        owner,
        AGENT_MANAGEMENT_PHASE_TRANSITION_METHOD,
        &phase.event_id,
        serde_json::to_value(fact)?,
        EventCorrelation {
            // Phase identity, rather than only the parent action, makes
            // cross-process recovery idempotent for every committed phase.
            management_request_id: Some(phase.event_id.clone()),
            ..Default::default()
        },
    )
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IntegrationEventDedupeState {
    #[serde(default)]
    committed: BTreeSet<String>,
    #[serde(default)]
    pending: BTreeSet<String>,
}

pub fn append_agent_management_outcome(
    caller: &CutexSessionId,
    response: &AgentManagementResponse,
) -> anyhow::Result<Option<EventEnvelope>> {
    let (method, params, action_id) = match &response.outcome {
        AgentManagementOutcome::Complete { receipt } => (
            "cutex/agentManagement/actionCompleted",
            serde_json::to_value(receipt)?,
            receipt.action_id.as_str(),
        ),
        AgentManagementOutcome::OwnerActionRequired { failure } => (
            "cutex/agentManagement/actionFailed",
            serde_json::to_value(failure)?,
            failure.action_id.as_str(),
        ),
        AgentManagementOutcome::NoWrite { .. } => return Ok(None),
    };
    append_owner_event(
        caller.as_str(),
        method,
        params,
        EventCorrelation {
            management_request_id: Some(action_id.to_string()),
            ..Default::default()
        },
    )
    .map(Some)
}

pub fn append_task_service_assignment(
    coordinator: &CutexSessionId,
    receipt: &ProviderReceipt,
) -> anyhow::Result<EventEnvelope> {
    if !matches!(receipt.result, ProviderResult::Assignment { .. }) {
        anyhow::bail!("assignmentCommitted requires an assignment receipt");
    }
    append_owner_event(
        coordinator.as_str(),
        "cutex/taskService/assignmentCommitted",
        serde_json::to_value(receipt)?,
        EventCorrelation {
            management_request_id: Some(receipt.action_id.as_str().to_string()),
            ..Default::default()
        },
    )
}

pub fn append_task_service_communication(
    coordinator: &CutexSessionId,
    receipt: &ProviderReceipt,
    agent_bus_message_id: Option<&str>,
) -> anyhow::Result<EventEnvelope> {
    if !matches!(receipt.result, ProviderResult::SendAttempt(_)) {
        anyhow::bail!("communicationRecorded requires a SendAttempt receipt");
    }
    append_owner_event(
        coordinator.as_str(),
        "cutex/taskService/communicationRecorded",
        serde_json::to_value(receipt)?,
        EventCorrelation {
            management_request_id: Some(receipt.action_id.as_str().to_string()),
            agent_bus_message_id: agent_bus_message_id.map(str::to_string),
            ..Default::default()
        },
    )
}

pub const TASK_SERVICE_ASSIGNMENT_TRANSITION_METHOD: &str =
    "cutex/taskService/assignmentTransitionCommitted";
pub const TASK_SERVICE_ASSIGNMENT_TRANSITION_SCHEMA: &str =
    "cutex/task-service-assignment-transition/v1";
pub const TASK_WATCHDOG_FIRST_STALE_METHOD: &str = "cutex/taskWatchdog/firstStale";
pub const TASK_WATCHDOG_DIRECTOR_ESCALATED_METHOD: &str = "cutex/taskWatchdog/directorEscalated";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAssignmentTransitionKind {
    AttemptStarted,
    AttemptAcknowledged,
    AttemptProgressed,
    AttemptBlocked,
    AttemptResumed,
    ReviewReady,
    RetryScheduled,
    Completed,
    Failed,
    Closed,
    Declined,
    Aborted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskAssignmentTransitionFact {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub transition: TaskAssignmentTransitionKind,
    pub action_id: String,
    pub assignment_id: String,
    pub task_id: String,
    pub assignee_cutex_session_id: String,
    pub attempt_number: Option<u64>,
    pub closure_reason: Option<ClosureReason>,
    pub detail: Option<String>,
    pub committed_at: String,
    pub journal_sequence: u64,
}

pub fn append_task_service_transition(
    coordinator: &CutexSessionId,
    transition: TaskAssignmentTransitionKind,
    receipt: &ProviderReceipt,
    snapshot: &TaskServiceSnapshot,
) -> anyhow::Result<EventEnvelope> {
    let assignment_id = receipt_assignment_id(receipt)?;
    let assignment = snapshot
        .assignments
        .get(assignment_id)
        .context("Task Service transition assignment is absent from provider snapshot")?;
    let attempt_number = match &receipt.result {
        ProviderResult::Attempt(attempt) => Some(attempt.attempt_number.get()),
        _ => assignment.active_attempt.map(|number| number.get()),
    };
    let detail = transition_detail(receipt);
    let fact = TaskAssignmentTransitionFact {
        schema: TASK_SERVICE_ASSIGNMENT_TRANSITION_SCHEMA.to_string(),
        project_id: assignment.project_id.clone(),
        transition,
        action_id: receipt.action_id.as_str().to_string(),
        assignment_id: assignment.assignment_id.as_str().to_string(),
        task_id: assignment.task_id.as_str().to_string(),
        assignee_cutex_session_id: assignment.assignee_cutex_session.as_str().to_string(),
        attempt_number,
        closure_reason: assignment.closure.as_ref().map(|closure| closure.reason),
        detail,
        committed_at: receipt.committed_at.as_str().to_string(),
        journal_sequence: receipt.journal_sequence,
    };
    append_owner_event_once(
        coordinator.as_str(),
        TASK_SERVICE_ASSIGNMENT_TRANSITION_METHOD,
        serde_json::to_value(fact)?,
        EventCorrelation {
            management_request_id: Some(receipt.action_id.as_str().to_string()),
            ..Default::default()
        },
    )
}

/// Appends one raw, safe watchdog fact to the authorized Director's UI-only
/// activity source. The stable fact ID makes restart/replay idempotent; the
/// app-server projector decides presentation without adding model context.
pub fn append_task_watchdog_fact(
    director: &CutexSessionId,
    fact: &crate::task_service::TaskWatchdogFact,
) -> anyhow::Result<EventEnvelope> {
    if fact.schema != crate::task_service::TASK_WATCHDOG_FACT_SCHEMA
        || fact.event_key != fact.stage.event_key()
    {
        anyhow::bail!("invalid Task watchdog presentation fact");
    }
    let method = match fact.stage {
        crate::task_service::TaskWatchdogStage::FirstStale => TASK_WATCHDOG_FIRST_STALE_METHOD,
        crate::task_service::TaskWatchdogStage::DirectorEscalated => {
            TASK_WATCHDOG_DIRECTOR_ESCALATED_METHOD
        }
    };
    append_owner_event_once(
        director.as_str(),
        method,
        serde_json::to_value(fact)?,
        EventCorrelation {
            management_request_id: Some(fact.fact_id.clone()),
            ..Default::default()
        },
    )
}

fn receipt_assignment_id(receipt: &ProviderReceipt) -> anyhow::Result<&AssignmentId> {
    match &receipt.result {
        ProviderResult::Assignment { assignment, .. } => Ok(&assignment.assignment_id),
        ProviderResult::Attempt(attempt) => Ok(&attempt.assignment_id),
        ProviderResult::SendAttempt(send_attempt) => Ok(&send_attempt.assignment_id),
        _ => anyhow::bail!("Task Service transition receipt has no assignment identity"),
    }
}

fn transition_detail(receipt: &ProviderReceipt) -> Option<String> {
    match &receipt.result {
        ProviderResult::Attempt(attempt) => attempt
            .status_receipts
            .last()
            .map(|status| status.summary.clone())
            .or_else(|| {
                attempt
                    .result_receipts
                    .last()
                    .map(|result| result.result_reference.clone())
            }),
        _ => None,
    }
}

pub fn append_agent_bus_message_sent(
    sender: &CutexSessionId,
    message_id: &str,
    params: Value,
) -> anyhow::Result<EventEnvelope> {
    append_owner_event(
        sender.as_str(),
        "cutex/agentBus/messageSent",
        params,
        EventCorrelation {
            agent_bus_message_id: Some(message_id.to_string()),
            ..Default::default()
        },
    )
}

fn append_owner_event(
    owner_cutex_session_id: &str,
    method: &str,
    params: Value,
    correlation: EventCorrelation,
) -> anyhow::Result<EventEnvelope> {
    if owner_cutex_session_id.trim().is_empty() {
        anyhow::bail!("integration event owner must not be empty");
    }
    management_v2_repository()?
        .append(PendingEvent {
            cutex_session_id: owner_cutex_session_id.to_string(),
            host_id: current_host_name(),
            source: EventSource::Cutex,
            schema: None,
            correlation,
            native: None,
            cutex: Some(CutexMessage {
                method: method.to_string(),
                params,
            }),
        })
        .with_context(|| format!("failed to append {method} Management v2 event"))
}

fn append_owner_event_once(
    owner_cutex_session_id: &str,
    method: &str,
    params: Value,
    correlation: EventCorrelation,
) -> anyhow::Result<EventEnvelope> {
    let action_id = correlation
        .management_request_id
        .as_deref()
        .context("idempotent integration event requires a management request ID")?
        .to_string();
    let repository = management_v2_repository()?;
    append_owner_event_once_with_repository(
        repository,
        owner_cutex_session_id,
        method,
        &action_id,
        params,
        correlation,
    )
}

fn append_owner_event_once_with_repository(
    repository: &EventRepository,
    owner_cutex_session_id: &str,
    method: &str,
    action_id: &str,
    params: Value,
    correlation: EventCorrelation,
) -> anyhow::Result<EventEnvelope> {
    let key = format!("{owner_cutex_session_id}\u{0}{method}\u{0}{action_id}");
    let lock_path = repository.root().join(INTEGRATION_DEDUPE_LOCK);
    fs::create_dir_all(repository.root())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock_exclusive()?;
    let result = (|| -> anyhow::Result<EventEnvelope> {
        let state_path = repository.root().join(INTEGRATION_DEDUPE_STATE);
        let mut state = match fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice::<IntegrationEventDedupeState>(&bytes)
                .context("invalid integration event dedupe state")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                IntegrationEventDedupeState::default()
            }
            Err(error) => return Err(error.into()),
        };
        if state.committed.contains(&key) {
            return find_integration_event(repository, owner_cutex_session_id, method, action_id)?
                .context("committed integration event is no longer retained");
        }
        if let Some(existing) =
            find_integration_event(repository, owner_cutex_session_id, method, action_id)?
        {
            state.pending.remove(&key);
            state.committed.insert(key);
            crate::config::atomic::write_private_pretty_json_atomic(
                &state_path,
                &state,
                "integration event dedupe state",
            )?;
            return Ok(existing);
        }
        state.pending.insert(key.clone());
        crate::config::atomic::write_private_pretty_json_atomic(
            &state_path,
            &state,
            "integration event dedupe state",
        )?;
        let event = repository.append(PendingEvent {
            cutex_session_id: owner_cutex_session_id.to_string(),
            host_id: current_host_name(),
            source: EventSource::Cutex,
            schema: None,
            correlation,
            native: None,
            cutex: Some(CutexMessage {
                method: method.to_string(),
                params,
            }),
        })?;
        state.pending.remove(&key);
        state.committed.insert(key);
        crate::config::atomic::write_private_pretty_json_atomic(
            &state_path,
            &state,
            "integration event dedupe state",
        )?;
        Ok(event)
    })();
    let _ = FileExt::unlock(&lock);
    result
}

fn find_integration_event(
    repository: &EventRepository,
    owner_cutex_session_id: &str,
    method: &str,
    action_id: &str,
) -> anyhow::Result<Option<EventEnvelope>> {
    let metadata = repository.stream_metadata()?;
    let mut after = None;
    loop {
        let page = repository
            .page(ReplayQuery {
                stream_id: Some(metadata.stream_id.clone()),
                after,
                limit: 1000,
                cutex_session_id: Some(owner_cutex_session_id.to_string()),
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if let Some(event) = page.events.into_iter().find(|event| {
            event
                .cutex
                .as_ref()
                .is_some_and(|message| message.method == method)
                && event.correlation.management_request_id.as_deref() == Some(action_id)
        }) {
            return Ok(Some(event));
        }
        if !page.has_more {
            return Ok(None);
        }
        after = page.next_cursor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn management_phase_fact_replay_keeps_one_raw_frontend_event() {
        let root = std::env::temp_dir().join(format!(
            "cutex-management-phase-dedupe-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = EventRepository::open(&root, current_host_name()).unwrap();
        let owner = "cutex.director-old";
        let phase_event_id = "agent-management:rotate-1:phase:2";
        let params = serde_json::json!({
            "schema": AGENT_MANAGEMENT_PHASE_TRANSITION_SCHEMA,
            "phase_event_id": phase_event_id,
            "action_id": "rotate-1",
            "project_id": "project-1",
            "operation": "director_rotate",
            "phase": "predecessor_closing",
            "phase_sequence": 2,
            "committed_at": "2026-08-28T00:00:00Z",
            "primary_presentation_target_cutex_session_id": owner,
            "primary_presentation_target_metadata": null,
            "predecessor_cutex_session_id": owner,
            "predecessor_metadata": null,
            "successor_cutex_session_id": null,
            "successor_metadata": null,
            "replace_policy": null,
            "rotation_mode": "close_predecessor_then_create_with_message",
            "authority_epoch": 7
        });
        let correlation = EventCorrelation {
            management_request_id: Some(phase_event_id.to_string()),
            ..Default::default()
        };
        let first = append_owner_event_once_with_repository(
            &repository,
            owner,
            AGENT_MANAGEMENT_PHASE_TRANSITION_METHOD,
            phase_event_id,
            params.clone(),
            correlation.clone(),
        )
        .unwrap();
        let replay = append_owner_event_once_with_repository(
            &repository,
            owner,
            AGENT_MANAGEMENT_PHASE_TRANSITION_METHOD,
            phase_event_id,
            params,
            correlation,
        )
        .unwrap();
        assert_eq!(first.event_id, replay.event_id);
        let page = repository
            .page(ReplayQuery {
                stream_id: Some(first.stream_id.clone()),
                after: None,
                limit: 100,
                cutex_session_id: Some(owner.to_string()),
            })
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(
            page.events[0].cutex.as_ref().unwrap().method,
            AGENT_MANAGEMENT_PHASE_TRANSITION_METHOD
        );
        drop(repository);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transition_fact_dedupe_is_cross_instance_and_crash_pending_safe() {
        let root =
            std::env::temp_dir().join(format!("cutex-integration-dedupe-{}", uuid::Uuid::new_v4()));
        let owner = "cutex.director";
        let method = TASK_SERVICE_ASSIGNMENT_TRANSITION_METHOD;
        let action = "transition-1";
        let params = serde_json::json!({
            "schema": TASK_SERVICE_ASSIGNMENT_TRANSITION_SCHEMA,
            "transition": "review_ready",
            "action_id": action,
            "assignment_id": "assignment-1",
            "task_id": "task-1",
            "assignee_cutex_session_id": "cutex.worker",
            "attempt_number": 1,
            "closure_reason": null,
            "detail": "result.md",
            "committed_at": "2026-08-28T00:00:00Z",
            "journal_sequence": 1
        });
        let correlation = EventCorrelation {
            management_request_id: Some(action.to_string()),
            ..Default::default()
        };
        let pending_key = format!("{owner}\u{0}{method}\u{0}{action}");
        crate::config::atomic::write_private_pretty_json_atomic(
            &root.join(INTEGRATION_DEDUPE_STATE),
            &IntegrationEventDedupeState {
                committed: BTreeSet::new(),
                pending: BTreeSet::from([pending_key]),
            },
            "test integration dedupe state",
        )
        .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut threads = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let barrier = Arc::clone(&barrier);
            let params = params.clone();
            let correlation = correlation.clone();
            threads.push(std::thread::spawn(move || {
                let repository = EventRepository::open(&root, current_host_name()).unwrap();
                barrier.wait();
                append_owner_event_once_with_repository(
                    &repository,
                    owner,
                    method,
                    action,
                    params,
                    correlation,
                )
                .unwrap()
            }));
        }
        let first = threads.remove(0).join().unwrap();
        let second = threads.remove(0).join().unwrap();
        assert_eq!(first.event_id, second.event_id);
        let repository = EventRepository::open(&root, current_host_name()).unwrap();
        let page = repository.page(ReplayQuery::default()).unwrap();
        assert_eq!(
            page.events
                .iter()
                .filter(|event| event
                    .cutex
                    .as_ref()
                    .is_some_and(|value| value.method == method))
                .count(),
            1
        );
        let state: IntegrationEventDedupeState =
            serde_json::from_slice(&fs::read(root.join(INTEGRATION_DEDUPE_STATE)).unwrap())
                .unwrap();
        assert!(state.pending.is_empty());
        assert_eq!(state.committed.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
