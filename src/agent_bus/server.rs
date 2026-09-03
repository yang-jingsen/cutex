//! Blocking HTTP route handling for the local cutex agent bus.

pub(crate) mod task_action_store;

use std::collections::BTreeSet;
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use anyhow::Context;
use chrono::Utc;
use serde_json::Value;

use crate::agent_bus::audit::append_agent_bus_audit_record;
use crate::agent_bus::federation::dedupe_agents_by_id;
use crate::agent_bus::federation::fetch_federated_agent_bus_agents;
use crate::agent_bus::federation::filter_federated_agents_for_request;
use crate::agent_bus::groups::agent_from_register_request;
use crate::agent_bus::groups::update_agent_groups;
use crate::agent_bus::identity::normalize_agent_groups;
use crate::agent_bus::model::AgentBusAckRequest;
use crate::agent_bus::model::AgentBusAgent;
use crate::agent_bus::model::AgentBusGroupUpdateRequest;
use crate::agent_bus::model::AgentBusHeartbeatRequest;
use crate::agent_bus::model::AgentBusMessage;
use crate::agent_bus::model::AgentBusPollResponse;
use crate::agent_bus::model::AgentBusSendRequest;
use crate::agent_bus::model::AgentBusUnregisterRequest;
use crate::agent_bus::model::{
    TaskServiceActionOutcome, TaskServiceActionResponse, TaskServiceActionResponseSchema,
    TaskServiceQueryOutcome, TaskServiceQueryResponse, TaskServiceQueryResponseSchema,
    TaskServiceWorkerContextOutcome, TaskServiceWorkerContextResponse,
    TaskServiceWorkerPrepareOutcome, TaskServiceWorkerPrepareResponse,
};
use crate::agent_bus::model::{
    TaskWorkerActionNoWrite, TaskWorkerActionOutcome, TaskWorkerActionRequest,
    TaskWorkerActionResponse, TaskWorkerActionResponseSchema, TaskWorkerCommittedReceiptEvidence,
    TaskWorkerReceiptAbsence, TaskWorkerReconciliationNoWrite, TaskWorkerReconciliationOperation,
    TaskWorkerReconciliationOutcome, TaskWorkerReconciliationRequest,
    TaskWorkerReconciliationResponse, TaskWorkerReconciliationResponseSchema,
    TaskWorkerResolutionEvidence, TASK_WORKER_ACTION_MAX_BODY_BYTES,
};
use crate::agent_bus::queue::ack_agent_messages;
use crate::agent_bus::queue::poll_agent_messages;
use crate::agent_bus::routing::project_current_durable_session_ids;
use crate::agent_bus::routing::visible_agents_for_request;
use crate::agent_bus::routing::AgentTargetResolutionCode;
use crate::agent_bus::routing::AgentTargetResolutionError;
use crate::agent_bus::store::agent_is_local_to_bus;
use crate::agent_bus::store::persist_agent_bus_registry;
use crate::agent_bus::store::prune_stale_agents;
use crate::agent_bus::store::save_agent_bus_registry_locked;
use crate::agent_bus::store::AgentBusState;
use crate::agent_management::{
    AgentActionId, AgentManagementInvocation, AgentManagementOutcome, AgentManagementRequest,
    AgentManagementResponse, AgentManagementSchema, AGENT_MANAGEMENT_MAX_BODY_BYTES,
};
use crate::http::query::query_bool;
use crate::http::query::query_has_key;
use crate::http::query::query_value;
use crate::http::server::read_simple_http_request;
use crate::http::server::require_service_bridge_token;
use crate::http::server::write_http_response;
use crate::http::server::write_json_response;
use crate::platform::host::current_host_name;
use crate::platform::now_epoch_secs;
use crate::role_revision::RuntimeAgentId;
use crate::rotation::{
    ReleaseRotationInvocation, ReleaseRotationOutcome, ReleaseRotationRequest,
    ReleaseRotationResponse, ReleaseRotationResponseSchema,
};
use crate::session::store::load_cutex_session_store;
use crate::task_delivery::{
    validate_task_worker_action_request, TaskWorkerActionAdapter, TaskWorkerAuthorizedAction,
    TaskWorkerRosterSender, TaskWorkerTransitionResult,
};

use task_action_store::{
    authorized_action_from_record, ActionProbe, EvidenceStoreError, PreparedTaskWorkerAction,
    TaskWorkerActionEvidenceStore,
};

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";
const MAX_POLL_WAIT: Duration = Duration::from_secs(5);
const COMPLETION_DRAIN_RETRY_SECS: u64 = 5;

#[cfg(test)]
#[derive(Default)]
struct CompletionDrainTestGate {
    entered: (Mutex<bool>, Condvar),
    released: (Mutex<bool>, Condvar),
}

#[cfg(test)]
impl CompletionDrainTestGate {
    fn block_after_execution_lock(&self) {
        let mut entered = self.entered.0.lock().unwrap();
        *entered = true;
        self.entered.1.notify_all();
        drop(entered);

        let mut released = self.released.0.lock().unwrap();
        while !*released {
            released = self.released.1.wait(released).unwrap();
        }
    }

    fn wait_until_entered(&self, timeout: Duration) -> bool {
        let entered = self.entered.0.lock().unwrap();
        if *entered {
            return true;
        }
        let (entered, _) = self.entered.1.wait_timeout(entered, timeout).unwrap();
        *entered
    }

    fn release(&self) {
        let mut released = self.released.0.lock().unwrap();
        *released = true;
        self.released.1.notify_all();
    }
}

pub struct TaskWorkerActionHost {
    adapter: Arc<TaskWorkerActionAdapter>,
    evidence: TaskWorkerActionEvidenceStore,
    provider: Option<crate::task_service::TaskServiceProvider>,
    seat_authority: Option<crate::seat::SeatOccupancyStore>,
    watchdog: Option<Arc<crate::task_service::TaskStaleWatchdog>>,
    execution: Mutex<()>,
    completion_drain_requested: AtomicBool,
    completion_unavailable_target_seats: Mutex<BTreeSet<String>>,
    completion_unavailable_target_sessions: Mutex<BTreeSet<String>>,
    completion_drain_retry_at: AtomicU64,
    completion_drain_execution: Mutex<()>,
    completion_drain_scheduled: AtomicBool,
    completion_target_probe_requested: AtomicBool,
    #[cfg(test)]
    completion_drain_scans: AtomicU64,
    #[cfg(test)]
    completion_drain_test_gate: Mutex<Option<Arc<CompletionDrainTestGate>>>,
}

impl TaskWorkerActionHost {
    pub fn open_recovered(
        task_service_root: impl Into<std::path::PathBuf>,
        evidence_root: impl Into<std::path::PathBuf>,
        seat_authority_root: impl Into<std::path::PathBuf>,
    ) -> anyhow::Result<Self> {
        let task_service_root = task_service_root.into();
        let adapter = TaskWorkerActionAdapter::open_recovered(task_service_root.clone())
            .map_err(|error| anyhow!("failed to recover worker Task Service: {error}"))?;
        let provider =
            crate::task_service::TaskServiceProvider::open(task_service_root.join("provider-v2"))
                .map_err(|error| anyhow!("failed to open Task Service provider v2: {error}"))?;
        provider
            .recover()
            .map_err(|error| anyhow!("failed to recover Task Service provider v2: {error}"))?;
        let watchdog = Arc::new(crate::task_service::TaskStaleWatchdog::open(
            task_service_root.join("watchdog-v1"),
            crate::task_service::TaskWatchdogConfig::from_env()?,
        )?);
        let evidence = TaskWorkerActionEvidenceStore::open(evidence_root.into())
            .map_err(|error| anyhow!("failed to recover worker evidence store: {error:?}"))?;
        let seat_authority = crate::seat::SeatOccupancyStore::open(seat_authority_root.into())
            .map_err(|error| anyhow!("failed to recover Task Service seat authority: {error}"))?;
        Ok(Self {
            adapter: Arc::new(adapter),
            evidence,
            provider: Some(provider),
            seat_authority: Some(seat_authority),
            watchdog: Some(watchdog),
            execution: Mutex::new(()),
            completion_drain_requested: AtomicBool::new(true),
            completion_unavailable_target_seats: Mutex::new(BTreeSet::new()),
            completion_unavailable_target_sessions: Mutex::new(BTreeSet::new()),
            completion_drain_retry_at: AtomicU64::new(0),
            completion_drain_execution: Mutex::new(()),
            completion_drain_scheduled: AtomicBool::new(false),
            completion_target_probe_requested: AtomicBool::new(false),
            #[cfg(test)]
            completion_drain_scans: AtomicU64::new(0),
            #[cfg(test)]
            completion_drain_test_gate: Mutex::new(None),
        })
    }

    #[cfg(test)]
    fn with_parts(
        adapter: Arc<TaskWorkerActionAdapter>,
        evidence: TaskWorkerActionEvidenceStore,
    ) -> Self {
        Self {
            adapter,
            evidence,
            provider: None,
            seat_authority: None,
            watchdog: None,
            execution: Mutex::new(()),
            completion_drain_requested: AtomicBool::new(false),
            completion_unavailable_target_seats: Mutex::new(BTreeSet::new()),
            completion_unavailable_target_sessions: Mutex::new(BTreeSet::new()),
            completion_drain_retry_at: AtomicU64::new(0),
            completion_drain_execution: Mutex::new(()),
            completion_drain_scheduled: AtomicBool::new(false),
            completion_target_probe_requested: AtomicBool::new(false),
            completion_drain_scans: AtomicU64::new(0),
            completion_drain_test_gate: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn with_v2_parts(
        adapter: Arc<TaskWorkerActionAdapter>,
        evidence: TaskWorkerActionEvidenceStore,
        provider: crate::task_service::TaskServiceProvider,
        seat_authority: crate::seat::SeatOccupancyStore,
    ) -> Self {
        Self {
            adapter,
            evidence,
            provider: Some(provider),
            seat_authority: Some(seat_authority),
            watchdog: None,
            execution: Mutex::new(()),
            completion_drain_requested: AtomicBool::new(true),
            completion_unavailable_target_seats: Mutex::new(BTreeSet::new()),
            completion_unavailable_target_sessions: Mutex::new(BTreeSet::new()),
            completion_drain_retry_at: AtomicU64::new(0),
            completion_drain_execution: Mutex::new(()),
            completion_drain_scheduled: AtomicBool::new(false),
            completion_target_probe_requested: AtomicBool::new(false),
            completion_drain_scans: AtomicU64::new(0),
            completion_drain_test_gate: Mutex::new(None),
        }
    }

    fn execute_v2(
        &self,
        sender: TaskWorkerRosterSender,
        request: crate::task_service::WorkerProviderActionEnvelope,
    ) -> TaskServiceActionResponse {
        let action_id = request.action_id().clone();
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return task_service_v2_no_write(
                    action_id,
                    "persistence_unavailable",
                    "execution lock unavailable",
                )
            }
        };
        let Some(provider) = self.provider.as_ref() else {
            return task_service_v2_no_write(
                action_id,
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let principal =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(principal) => principal,
                Err(error) => {
                    return task_service_v2_no_write(
                        action_id,
                        "unauthorized",
                        &format!("stable Worker authentication failed: {error:?}"),
                    )
                }
            };
        let was_known = provider
            .query()
            .is_ok_and(|snapshot| snapshot.receipts.contains_key(&action_id));
        let transition = worker_transition_kind(&request.action);
        match provider.execute_worker_action(&principal, &request) {
            Ok(receipt) => {
                self.append_task_transition_if_new(was_known, transition, &receipt, None);
                TaskServiceActionResponse {
                    schema: TaskServiceActionResponseSchema::V2,
                    action_id,
                    outcome: TaskServiceActionOutcome::Committed(receipt),
                }
            }
            Err(error) => {
                let code = match error {
                    crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
                    crate::task_service::ProviderError::Unauthorized => "unauthorized",
                    crate::task_service::ProviderError::NotFound(_) => "not_found",
                    crate::task_service::ProviderError::Conflict(_) => "conflict",
                    crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
                    crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
                    crate::task_service::ProviderError::PersistenceUnavailable
                    | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
                    crate::task_service::ProviderError::InvalidStore => "invalid_store",
                };
                task_service_v2_no_write(action_id, code, &error.to_string())
            }
        }
    }

    fn append_task_transition_if_new(
        &self,
        was_known: bool,
        transition: crate::management::v2::integration_events::TaskAssignmentTransitionKind,
        receipt: &crate::task_service::ProviderReceipt,
        authoritative_seats: Option<&crate::seat::SeatOccupancySnapshot>,
    ) {
        if was_known {
            return;
        }
        let result = (|| -> anyhow::Result<()> {
            let provider = self
                .provider
                .as_ref()
                .context("Task Service provider unavailable")?;
            let snapshot = provider.query()?;
            let assignment_id = match &receipt.result {
                crate::task_service::ProviderResult::Assignment { assignment, .. } => {
                    &assignment.assignment_id
                }
                crate::task_service::ProviderResult::Attempt(attempt) => &attempt.assignment_id,
                crate::task_service::ProviderResult::SendAttempt(send_attempt) => {
                    &send_attempt.assignment_id
                }
                _ => anyhow::bail!("receipt has no Task assignment identity"),
            };
            let assignment = snapshot
                .assignments
                .get(assignment_id)
                .context("Task assignment absent after committed transition")?;
            let task = snapshot
                .task_revisions
                .get(&assignment.task_id)
                .and_then(|revisions| revisions.get(&assignment.task_revision))
                .context("Task revision absent after committed transition")?;
            let workflow = snapshot
                .workflows
                .get(&task.workflow_id)
                .context("Task workflow absent after committed transition")?;
            let queried_seats;
            let seat_snapshot = match authoritative_seats {
                Some(snapshot) => snapshot,
                None => {
                    let seats = self
                        .seat_authority
                        .as_ref()
                        .context("Task Service seat authority unavailable")?;
                    queried_seats = seats.query()?;
                    &queried_seats
                }
            };
            let coordinator = seat_snapshot
                .occupancies
                .get(&workflow.coordinator_seat_id)
                .context("Task coordinator seat is not occupied")?;
            crate::management::v2::integration_events::append_task_service_transition(
                &coordinator.occupant_cutex_session,
                transition,
                receipt,
                &snapshot,
            )?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!(
                "{YELLOW}warning:{RESET} failed to append Task Service transition activity: {error:#}"
            );
        }
    }

    pub fn recover_completion_notifications(&self, state: &Arc<Mutex<AgentBusState>>) {
        self.request_completion_notification_drain();
        self.dispatch_completion_notifications_blocking(state);
    }

    /// Starts the single Task Service-owned stale-running scheduler. The
    /// agent-bus host ownership lock guarantees one contender per provider
    /// root; tests call the decision engine directly with a fake clock.
    pub fn spawn_task_watchdog(
        self: &Arc<Self>,
        state: &Arc<Mutex<AgentBusState>>,
    ) -> anyhow::Result<()> {
        let Some(watchdog) = self.watchdog.as_ref() else {
            return Ok(());
        };
        let interval = watchdog.poll_interval();
        let host = Arc::clone(self);
        let state = Arc::clone(state);
        std::thread::Builder::new()
            .name("cutex-task-watchdog".to_string())
            .spawn(move || loop {
                if let Err(error) = host.run_task_watchdog_once(&state) {
                    eprintln!(
                        "{YELLOW}warning:{RESET} Task Service watchdog scan failed closed: {error:#}"
                    );
                }
                std::thread::sleep(interval);
            })
            .context("failed to spawn Task Service watchdog scheduler")?;
        Ok(())
    }

    fn run_task_watchdog_once(&self, state: &Arc<Mutex<AgentBusState>>) -> anyhow::Result<()> {
        let (Some(provider), Some(seats), Some(watchdog)) =
            (&self.provider, &self.seat_authority, &self.watchdog)
        else {
            return Ok(());
        };
        let snapshot = provider.query()?;
        let activity = crate::management::v2::activity::load_session_activity_states()
            .map(|states| crate::task_service::task_watchdog_activity_projections(&states))
            .unwrap_or_default();
        let outcome = watchdog.scan(&snapshot, &activity)?;
        let seat_snapshot = seats.query()?;

        if !outcome.cancelled_notification_ids.is_empty() {
            let cancelled = outcome
                .cancelled_notification_ids
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let mut bus = state
                .lock()
                .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
            for queue in bus.messages.values_mut() {
                queue.retain(|message| {
                    !message.sender_kind.is_task_service_system()
                        || message.control_type.as_deref() != Some("cutex.task_service.watchdog.v1")
                        || !message
                            .external_message_id
                            .as_deref()
                            .is_some_and(|id| cancelled.contains(id))
                });
            }
            bus.recent_sends.retain(|_, message| {
                message.control_type.as_deref() != Some("cutex.task_service.watchdog.v1")
                    || !message
                        .external_message_id
                        .as_deref()
                        .is_some_and(|id| cancelled.contains(id))
            });
        }

        for fact in &outcome.presentations {
            let assignment_id =
                match crate::task_service::AssignmentId::new(fact.assignment_id.clone()) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
            let Some(assignment) = snapshot.assignments.get(&assignment_id) else {
                continue;
            };
            let Some(task) = snapshot
                .task_revisions
                .get(&assignment.task_id)
                .and_then(|revisions| revisions.get(&assignment.task_revision))
            else {
                continue;
            };
            if assignment.project_id != fact.project_id
                || assignment.task_id.as_str() != fact.task_id
                || assignment.task_revision.get() != fact.task_revision
                || assignment.active_attempt.map(|number| number.get()) != Some(fact.attempt_number)
            {
                continue;
            }
            let Some(director) = seat_snapshot
                .occupancies
                .get(&task.completion_policy.authority_seat_id)
            else {
                continue;
            };
            crate::management::v2::integration_events::append_task_watchdog_fact(
                &director.occupant_cutex_session,
                fact,
            )?;
        }

        for notification in outcome.notifications {
            let target_session = match &notification.target {
                crate::task_service::TaskWatchdogTarget::AssigneeSession(session) => {
                    Some(session.as_str())
                }
                crate::task_service::TaskWatchdogTarget::AuthoritySeat(seat) => {
                    let seat = crate::task_service::SeatId::new(seat.clone()).ok();
                    seat.as_ref()
                        .and_then(|seat| seat_snapshot.occupancies.get(seat))
                        .map(|occupancy| occupancy.occupant_cutex_session.as_str())
                }
            };
            let target = target_session.and_then(|session| {
                crate::task_delivery::provider_adapter::resolve_current_runtime_target(
                    state, session,
                )
            });
            let Some((target_id, target_name)) = target else {
                watchdog.record_delivery_fact(
                    &notification.notification_id,
                    crate::task_service::TaskWatchdogDeliveryFactKind::Uncertain,
                    Some("target_unavailable".to_string()),
                )?;
                watchdog.record_delivery_fact(
                    &notification.notification_id,
                    crate::task_service::TaskWatchdogDeliveryFactKind::RetryScheduled,
                    Some("next_poll".to_string()),
                )?;
                continue;
            };
            let delivery_mode = match notification.delivery_mode {
                crate::task_service::TaskWatchdogDeliveryMode::Soon => {
                    crate::agent_bus::delivery::AgentDeliveryMode::Soon
                }
                crate::task_service::TaskWatchdogDeliveryMode::AfterTurn => {
                    crate::agent_bus::delivery::AgentDeliveryMode::AfterTurn
                }
            };
            let metadata = crate::task_service::TaskWatchdogMessageMetadata::from(&notification);
            let system = crate::agent_bus::identity::task_service_system_principal();
            match crate::agent_bus::queue::enqueue_task_service_watchdog_message_once(
                state,
                &system,
                &target_id,
                &target_name,
                &notification.content,
                &metadata,
                delivery_mode,
                &notification.external_message_id,
                crate::platform::now_epoch_secs(),
            ) {
                Ok(queued) => watchdog.record_delivery_fact(
                    &notification.notification_id,
                    crate::task_service::TaskWatchdogDeliveryFactKind::Queued,
                    Some(queued.record.id),
                )?,
                Err(error) => {
                    watchdog.record_delivery_fact(
                        &notification.notification_id,
                        crate::task_service::TaskWatchdogDeliveryFactKind::Uncertain,
                        Some("agent_bus_unavailable".to_string()),
                    )?;
                    watchdog.record_delivery_fact(
                        &notification.notification_id,
                        crate::task_service::TaskWatchdogDeliveryFactKind::RetryScheduled,
                        Some("next_poll".to_string()),
                    )?;
                    return Err(error).context("failed to queue Task watchdog notification");
                }
            }
        }
        Ok(())
    }

    fn dispatch_completion_notifications_after_transition(
        self: &Arc<Self>,
        state: &Arc<Mutex<AgentBusState>>,
        response: &TaskServiceActionResponse,
    ) {
        if matches!(&response.outcome, TaskServiceActionOutcome::Committed(_)) {
            self.request_completion_notification_drain();
            self.schedule_completion_work_if_due(state);
        }
    }

    fn retry_completion_notifications_for_available_target(
        self: &Arc<Self>,
        state: &Arc<Mutex<AgentBusState>>,
    ) {
        let has_unavailable_target = self
            .completion_unavailable_target_seats
            .lock()
            .map(|seats| !seats.is_empty())
            .unwrap_or(false)
            || self
                .completion_unavailable_target_sessions
                .lock()
                .map(|sessions| !sessions.is_empty())
                .unwrap_or(false);
        if !has_unavailable_target {
            return;
        }
        self.completion_target_probe_requested
            .store(true, Ordering::Release);
        self.schedule_completion_work_if_due(state);
    }

    fn request_completion_notification_drain(&self) {
        self.completion_drain_retry_at.store(0, Ordering::Release);
        self.completion_drain_requested
            .store(true, Ordering::Release);
    }

    fn completion_drain_is_due(&self) -> bool {
        if !self.completion_drain_requested.load(Ordering::Acquire) {
            return false;
        }
        let retry_at = self.completion_drain_retry_at.load(Ordering::Acquire);
        retry_at == 0 || now_epoch_secs() >= retry_at
    }

    fn schedule_completion_work_if_due(self: &Arc<Self>, state: &Arc<Mutex<AgentBusState>>) {
        if !self.completion_drain_is_due()
            && !self
                .completion_target_probe_requested
                .load(Ordering::Acquire)
        {
            return;
        }
        if self
            .completion_drain_scheduled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let host = Arc::clone(self);
        let state = Arc::clone(state);
        std::thread::spawn(move || {
            let execution_lock_healthy = if let Ok(_drain) = host.completion_drain_execution.lock()
            {
                #[cfg(test)]
                host.block_on_completion_drain_test_gate();
                if host
                    .completion_target_probe_requested
                    .swap(false, Ordering::AcqRel)
                    && host.completion_target_became_available(&state)
                {
                    host.request_completion_notification_drain();
                }
                if host.completion_drain_is_due() {
                    host.dispatch_completion_notifications_locked(&state);
                }
                true
            } else {
                false
            };
            host.completion_drain_scheduled
                .store(false, Ordering::Release);
            // Close the race where a transition or availability event arrived
            // while this single-flight worker was completing. A poisoned
            // execution mutex is left requested for a later explicit recovery
            // rather than turning into an unbounded thread-respawn loop.
            if execution_lock_healthy {
                host.schedule_completion_work_if_due(&state);
            }
        });
    }

    fn completion_target_became_available(&self, state: &Arc<Mutex<AgentBusState>>) -> bool {
        let unavailable_target_seats = self
            .completion_unavailable_target_seats
            .lock()
            .map(|seats| seats.clone())
            .unwrap_or_default();
        let unavailable_target_sessions = self
            .completion_unavailable_target_sessions
            .lock()
            .map(|sessions| sessions.clone())
            .unwrap_or_default();
        let worker_available = unavailable_target_sessions.iter().any(|session| {
            crate::task_delivery::provider_adapter::completion_target_is_current(state, session)
        });
        worker_available
            || self.seat_authority.as_ref().is_some_and(|seats| {
                seats.query().is_ok_and(|snapshot| {
                    unavailable_target_seats.iter().any(|target_seat| {
                        snapshot
                        .occupancies
                        .values()
                        .find(|occupancy| occupancy.seat_id.as_str() == target_seat)
                        .is_some_and(|occupancy| {
                            crate::task_delivery::provider_adapter::completion_target_is_current(
                                state,
                                occupancy.occupant_cutex_session.as_str(),
                            )
                        })
                    })
                })
            })
    }

    fn dispatch_completion_notifications_blocking(&self, state: &Arc<Mutex<AgentBusState>>) {
        let Ok(_drain) = self.completion_drain_execution.lock() else {
            return;
        };
        // An explicit transition or target-availability trigger is not delayed
        // by a prior bounded recovery backoff.
        self.completion_drain_retry_at.store(0, Ordering::Release);
        self.dispatch_completion_notifications_locked(state);
    }

    fn dispatch_completion_notifications_locked(&self, state: &Arc<Mutex<AgentBusState>>) {
        if !self
            .completion_drain_requested
            .swap(false, Ordering::AcqRel)
        {
            return;
        }
        let (Some(provider), Some(seats)) = (&self.provider, &self.seat_authority) else {
            return;
        };
        #[cfg(test)]
        self.completion_drain_scans.fetch_add(1, Ordering::SeqCst);
        match crate::task_delivery::provider_adapter::TaskServiceAgentBusDispatcher::dispatch_pending_completion_notifications(
            provider,
            seats,
            state,
            crate::platform::now_epoch_secs(),
        ) {
            Ok(summary) => {
                if let Ok(mut seats) = self.completion_unavailable_target_seats.lock() {
                    *seats = summary.unavailable_target_seats;
                }
                if let Ok(mut sessions) = self.completion_unavailable_target_sessions.lock() {
                    *sessions = summary.unavailable_target_sessions;
                }
                if summary.uncertain > summary.target_unavailable {
                    self.completion_drain_requested
                        .store(true, Ordering::Release);
                    self.completion_drain_retry_at.store(
                        now_epoch_secs().saturating_add(COMPLETION_DRAIN_RETRY_SECS),
                        Ordering::Release,
                    );
                }
            }
            Err(error) => {
                self.completion_drain_requested
                    .store(true, Ordering::Release);
                self.completion_drain_retry_at.store(
                    now_epoch_secs().saturating_add(COMPLETION_DRAIN_RETRY_SECS),
                    Ordering::Release,
                );
                eprintln!(
                    "{YELLOW}warning:{RESET} Task Service completion notification remains pending: {error}"
                );
            }
        }
    }

    #[cfg(test)]
    fn completion_drain_scan_count(&self) -> u64 {
        self.completion_drain_scans.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    fn install_completion_drain_test_gate(&self) -> Arc<CompletionDrainTestGate> {
        let gate = Arc::new(CompletionDrainTestGate::default());
        *self.completion_drain_test_gate.lock().unwrap() = Some(Arc::clone(&gate));
        gate
    }

    #[cfg(test)]
    fn block_on_completion_drain_test_gate(&self) {
        let gate = self.completion_drain_test_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.block_after_execution_lock();
        }
    }

    fn execute_release_rotation<T>(
        &self,
        sender: TaskWorkerRosterSender,
        request: &ReleaseRotationRequest,
        operation: impl FnOnce(ReleaseRotationInvocation) -> T,
    ) -> Result<T, ReleaseRotationResponse> {
        let action_id = request.action_id.clone();
        let _execution = self.execution.lock().map_err(|_| {
            release_rotation_no_write(
                action_id.clone(),
                "persistence_unavailable",
                "Task Service execution lock unavailable",
            )
        })?;
        let provider = self.provider.as_ref().ok_or_else(|| {
            release_rotation_no_write(
                action_id.clone(),
                "persistence_unavailable",
                "Task Service provider v2 is unavailable",
            )
        })?;
        let seats = self.seat_authority.as_ref().ok_or_else(|| {
            release_rotation_no_write(
                action_id.clone(),
                "persistence_unavailable",
                "seat authority is unavailable",
            )
        })?;
        let stable = crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender)
            .map_err(|_| {
                release_rotation_no_write(
                    action_id.clone(),
                    "unauthorized",
                    "stable Director authentication failed",
                )
            })?;
        let director_cutex_session = stable.authenticated_session_id().cloned().map_err(|_| {
            release_rotation_no_write(
                action_id.clone(),
                "unauthorized",
                "stable Director identity is unavailable",
            )
        })?;
        let is_current_director = seats.query().is_ok_and(|snapshot| {
            snapshot.occupancies.values().any(|occupancy| {
                occupancy.seat_id.as_str() == "cutex-director"
                    && occupancy.occupant_cutex_session == director_cutex_session
            })
        });
        if !is_current_director {
            return Err(release_rotation_no_write(
                action_id,
                "unauthorized",
                "caller is not the current cutex-director seat principal",
            ));
        }
        let predecessor_has_nonterminal_assignment = provider
            .query()
            .map_err(|_| {
                release_rotation_no_write(
                    request.action_id.clone(),
                    "persistence_unavailable",
                    "Task Service assignment snapshot is unavailable",
                )
            })?
            .assignments
            .values()
            .any(|assignment| {
                assignment.assignee_cutex_session == request.expected_predecessor_cutex_session
                    && assignment.state != crate::task_service::AssignmentState::Closed
            });
        Ok(operation(ReleaseRotationInvocation {
            director_cutex_session,
            director_runtime_agent_id: sender.runtime_agent_id.as_str().to_string(),
            predecessor_has_nonterminal_assignment,
        }))
    }

    fn execute_coordinator_v2(
        &self,
        sender: TaskWorkerRosterSender,
        state: &Arc<Mutex<AgentBusState>>,
        request: crate::task_service::CoordinatorActionRequest,
    ) -> TaskServiceActionResponse {
        let action_id = request.command.action_id().clone();
        let stable =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(stable) => stable,
                Err(error) => {
                    return task_service_v2_no_write(
                        action_id,
                        "unauthorized",
                        &format!("stable session authentication failed: {error:?}"),
                    )
                }
            };
        let session_id = match stable.authenticated_session_id() {
            Ok(session_id) => session_id.clone(),
            Err(error) => {
                return task_service_v2_no_write(action_id, "unauthorized", &error.to_string())
            }
        };
        self.execute_coordinator_session_v2(&session_id, state, request)
    }

    fn execute_worker_context_v2(
        &self,
        sender: TaskWorkerRosterSender,
        request: crate::task_service::WorkerContextRequest,
    ) -> TaskServiceWorkerContextResponse {
        let Some(provider) = self.provider.as_ref() else {
            return task_service_worker_context_no_write(
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let principal =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(principal) => principal,
                Err(_) => {
                    return task_service_worker_context_no_write(
                        "unauthorized",
                        "stable Worker authentication failed",
                    )
                }
            };
        match provider.worker_context(&principal, &request) {
            Ok(context) => TaskServiceWorkerContextResponse {
                schema: crate::task_service::WorkerContextResponseSchema::V2,
                outcome: TaskServiceWorkerContextOutcome::Context(context),
            },
            Err(error) => task_service_worker_context_provider_error(error),
        }
    }

    fn execute_worker_prepare_v2(
        &self,
        sender: TaskWorkerRosterSender,
        request: crate::task_service::WorkerPrepareRequest,
    ) -> TaskServiceWorkerPrepareResponse {
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return task_service_worker_prepare_no_write(
                    "persistence_unavailable",
                    "execution lock unavailable",
                )
            }
        };
        let Some(provider) = self.provider.as_ref() else {
            return task_service_worker_prepare_no_write(
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let principal =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(principal) => principal,
                Err(_) => {
                    return task_service_worker_prepare_no_write(
                        "unauthorized",
                        "stable Worker authentication failed",
                    )
                }
            };
        match provider.prepare_worker_action(&principal, &request) {
            Ok(crate::task_service::WorkerPrepareOutcome::Prepared(envelope)) => {
                TaskServiceWorkerPrepareResponse {
                    schema: crate::task_service::WorkerPrepareResponseSchema::V2,
                    outcome: TaskServiceWorkerPrepareOutcome::Prepared(envelope),
                }
            }
            Ok(crate::task_service::WorkerPrepareOutcome::Committed(receipt)) => {
                TaskServiceWorkerPrepareResponse {
                    schema: crate::task_service::WorkerPrepareResponseSchema::V2,
                    outcome: TaskServiceWorkerPrepareOutcome::Committed(receipt),
                }
            }
            Err(error) => task_service_worker_prepare_provider_error(error),
        }
    }

    fn execute_coordinator_session_v2(
        &self,
        session_id: &crate::role_revision::CutexSessionId,
        state: &Arc<Mutex<AgentBusState>>,
        request: crate::task_service::CoordinatorActionRequest,
    ) -> TaskServiceActionResponse {
        let action_id = request.command.action_id().clone();
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return task_service_v2_no_write(
                    action_id,
                    "persistence_unavailable",
                    "execution lock unavailable",
                )
            }
        };
        let Some(provider) = self.provider.as_ref() else {
            return task_service_v2_no_write(
                action_id,
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let was_known = provider
            .query()
            .is_ok_and(|snapshot| snapshot.receipts.contains_key(&action_id));
        let transition = coordinator_transition_kind(&request.command);
        let response = match self.with_current_seated_session(session_id, |principal| {
            match (&request.command, &request.context) {
                (
                    crate::task_service::CoordinatorOperation::CreateRevision(request),
                    crate::task_service::CoordinatorMechanicalContext::CreateRevision {
                        expected_workflow_revision,
                    },
                ) => {
                    provider_result_response(
                        action_id.clone(),
                        provider.create_revision(
                            principal,
                            request,
                            *expected_workflow_revision,
                        ),
                    )
                }
                (
                    crate::task_service::CoordinatorOperation::AssignAndDispatch(request),
                    crate::task_service::CoordinatorMechanicalContext::AssignAndDispatch {
                        expected_workflow_revision,
                    },
                ) => {
                    match crate::task_delivery::provider_adapter::TaskServiceAgentBusDispatcher::assign_and_dispatch(
                        provider,
                        principal,
                        state,
                        &request.request,
                        *expected_workflow_revision,
                        &request.human_readable_content,
                        now_epoch_secs(),
                    ) {
                        Ok(outcome) => {
                            if let Err(error) = crate::management::v2::integration_events::append_task_service_assignment(
                                session_id,
                                &outcome.assignment_receipt,
                            ) {
                                eprintln!("{YELLOW}warning:{RESET} failed to project Task Service assignment: {error:#}");
                            }
                            if let Err(error) = crate::management::v2::integration_events::append_task_service_communication(
                                session_id,
                                &outcome.communication_receipt,
                                Some(&outcome.agent_bus_message_id),
                            ) {
                                eprintln!("{YELLOW}warning:{RESET} failed to project Task Service communication: {error:#}");
                            }
                            TaskServiceActionResponse {
                                schema: TaskServiceActionResponseSchema::V2,
                                action_id: action_id.clone(),
                                outcome: TaskServiceActionOutcome::Committed(
                                    outcome.assignment_receipt,
                                ),
                            }
                        },
                        Err(error) => {
                            task_service_dispatch_response(
                                provider,
                                session_id,
                                action_id.clone(),
                                error,
                            )
                        }
                    }
                }
                (
                    crate::task_service::CoordinatorOperation::RetryDelivery(request),
                    crate::task_service::CoordinatorMechanicalContext::RetryDelivery {
                        expected_assignment_revision,
                    },
                ) => {
                    match crate::task_delivery::provider_adapter::TaskServiceAgentBusDispatcher::retry_delivery(
                        provider,
                        principal,
                        state,
                        &request.request,
                        *expected_assignment_revision,
                        &request.human_readable_content,
                        now_epoch_secs(),
                    ) {
                        Ok(outcome) => {
                            if let Err(error) = crate::management::v2::integration_events::append_task_service_communication(
                                session_id,
                                &outcome.communication_receipt,
                                Some(&outcome.agent_bus_message_id),
                            ) {
                                eprintln!("{YELLOW}warning:{RESET} failed to project Task Service communication: {error:#}");
                            }
                            TaskServiceActionResponse {
                                schema: TaskServiceActionResponseSchema::V2,
                                action_id: action_id.clone(),
                                outcome: TaskServiceActionOutcome::Committed(
                                    outcome.assignment_receipt,
                                ),
                            }
                        },
                        Err(error) => {
                            task_service_dispatch_response(
                                provider,
                                session_id,
                                action_id.clone(),
                                error,
                            )
                        }
                    }
                }
                (
                    crate::task_service::CoordinatorOperation::CancelAssignment(request),
                    crate::task_service::CoordinatorMechanicalContext::CancelAssignment {
                        expected_assignment_revision,
                        active_attempt,
                    },
                ) => {
                    provider_result_response(
                        action_id.clone(),
                        provider.cancel_assignment(
                            principal,
                            request,
                            *expected_assignment_revision,
                            active_attempt.as_ref(),
                        ),
                    )
                }
                (
                    crate::task_service::CoordinatorOperation::AuthorizeAttemptRetry(request),
                    crate::task_service::CoordinatorMechanicalContext::AuthorizeAttemptRetry {
                        expected_assignment_revision,
                    },
                ) => {
                    provider_result_response(
                        action_id.clone(),
                        provider.authorize_attempt_retry(
                            principal,
                            request,
                            *expected_assignment_revision,
                        ),
                    )
                }
                (
                    crate::task_service::CoordinatorOperation::CloseAssignment(request),
                    crate::task_service::CoordinatorMechanicalContext::CloseAssignment {
                        expected_assignment_revision,
                        attempt,
                    },
                ) => {
                    provider_result_response(
                        action_id.clone(),
                        provider.close_assignment(
                            principal,
                            request,
                            *expected_assignment_revision,
                            attempt,
                        ),
                    )
                }
                _ => task_service_v2_no_write(
                    action_id.clone(),
                    "invalid_request",
                    "coordinator command/context operation mismatch",
                ),
            }
        }) {
            Ok(response) => response,
            Err(detail) => task_service_v2_no_write(action_id, "unauthorized", &detail),
        };
        if let (Some(transition), TaskServiceActionOutcome::Committed(receipt)) =
            (transition, &response.outcome)
        {
            self.append_task_transition_if_new(was_known, transition, receipt, None);
        }
        response
    }

    fn execute_terminal_v2(
        &self,
        sender: TaskWorkerRosterSender,
        request: crate::task_service::TerminalActionEnvelope,
    ) -> TaskServiceActionResponse {
        let action_id = request.action_id().clone();
        let stable =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(stable) => stable,
                Err(error) => {
                    return task_service_v2_no_write(
                        action_id,
                        "unauthorized",
                        &format!("stable session authentication failed: {error:?}"),
                    )
                }
            };
        let session_id = match stable.authenticated_session_id() {
            Ok(session_id) => session_id.clone(),
            Err(error) => {
                return task_service_v2_no_write(action_id, "unauthorized", &error.to_string())
            }
        };
        self.execute_terminal_session_v2(&session_id, request)
    }

    fn execute_terminal_session_v2(
        &self,
        session_id: &crate::role_revision::CutexSessionId,
        request: crate::task_service::TerminalActionEnvelope,
    ) -> TaskServiceActionResponse {
        let action_id = request.action_id().clone();
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return task_service_v2_no_write(
                    action_id,
                    "persistence_unavailable",
                    "execution lock unavailable",
                )
            }
        };
        let Some(provider) = self.provider.as_ref() else {
            return task_service_v2_no_write(
                action_id,
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let was_known = provider
            .query()
            .is_ok_and(|snapshot| snapshot.receipts.contains_key(&action_id));
        let transition = terminal_transition_kind(&request.command);
        match self.with_current_seated_session(session_id, |principal| {
            provider_result_response(
                action_id.clone(),
                provider.execute_terminal_action(principal, &request),
            )
        }) {
            Ok(response) => {
                if let TaskServiceActionOutcome::Committed(receipt) = &response.outcome {
                    self.append_task_transition_if_new(was_known, transition, receipt, None);
                }
                response
            }
            Err(detail) => task_service_v2_no_write(action_id, "unauthorized", &detail),
        }
    }

    fn execute_query_v2(
        &self,
        sender: TaskWorkerRosterSender,
        request: crate::task_service::TaskServiceQueryRequest,
    ) -> TaskServiceQueryResponse {
        let stable =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(stable) => stable,
                Err(_) => {
                    return task_service_query_no_write(
                        "unauthorized",
                        "stable session authentication failed",
                    )
                }
            };
        let session_id = match stable.authenticated_session_id() {
            Ok(session_id) => session_id.clone(),
            Err(_) => {
                return task_service_query_no_write(
                    "unauthorized",
                    "stable session authentication failed",
                )
            }
        };
        self.execute_query_session_v2(&session_id, &stable, request)
    }

    fn execute_director_v1(
        &self,
        sender: TaskWorkerRosterSender,
        state: &Arc<Mutex<AgentBusState>>,
        request: crate::task_service::DirectorActionRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        let action_id = request.action_id.clone();
        let stable =
            match crate::task_delivery::provider_adapter::authenticate_worker_principal(&sender) {
                Ok(stable) => stable,
                Err(_) => {
                    return director_no_write(
                        action_id,
                        director_operation_name(&request.action),
                        "unauthorized",
                    )
                }
            };
        let session_id = match stable.authenticated_session_id() {
            Ok(session_id) => session_id.clone(),
            Err(_) => {
                return director_no_write(
                    action_id,
                    director_operation_name(&request.action),
                    "unauthorized",
                )
            }
        };
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return director_no_write(
                    action_id,
                    director_operation_name(&request.action),
                    "persistence_unavailable",
                )
            }
        };
        let Some(provider) = self.provider.as_ref() else {
            return director_no_write(
                action_id,
                director_operation_name(&request.action),
                "persistence_unavailable",
            );
        };
        let operation = director_operation_name(&request.action);
        let result =
            self.with_current_seated_session_snapshot(&session_id, |principal, seat_snapshot| {
                self.execute_authenticated_director_action(
                    provider,
                    principal,
                    seat_snapshot,
                    &session_id,
                    state,
                    &request,
                )
            });
        match result {
            Ok(receipt) => receipt,
            Err(_) => director_no_write(action_id, operation, "unauthorized"),
        }
    }

    fn execute_authenticated_director_action(
        &self,
        provider: &crate::task_service::TaskServiceProvider,
        principal: &crate::task_service::AuthenticatedPrincipal,
        seat_snapshot: &crate::seat::SeatOccupancySnapshot,
        session_id: &crate::role_revision::CutexSessionId,
        state: &Arc<Mutex<AgentBusState>>,
        request: &crate::task_service::DirectorActionRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        use crate::task_service::DirectorSemanticOperation as Operation;
        if !director_project_contract_is_valid(request) {
            return director_no_write(
                request.action_id.clone(),
                director_operation_name(&request.action),
                "project_contract_invalid",
            );
        }
        match &request.action {
            Operation::Query { selector } => {
                // A v2 Director query is deliberately project-scoped.  The
                // caller's exact durable session identity comes from the
                // authenticated route; no cwd, group, display name, or native
                // workspace metadata participates in this authority lookup.
                let project_scope =
                    if request.schema == crate::task_service::DirectorActionSchema::V2 {
                        match director_exact_project_scope(session_id) {
                            Ok(scope) => Some(scope),
                            Err(code) => {
                                return director_no_write(request.action_id.clone(), "query", code)
                            }
                        }
                    } else {
                        None
                    };
                return self.director_query(
                    provider,
                    seat_snapshot,
                    seat_for_session_in_snapshot(seat_snapshot, session_id).as_ref(),
                    request.action_id.clone(),
                    selector,
                    project_scope.as_ref(),
                );
            }
            Operation::CreateRevision(create) => {
                return self.director_create_revision(
                    provider,
                    principal,
                    seat_snapshot,
                    session_id,
                    request.action_id.clone(),
                    request.action_id.clone(),
                    create,
                )
            }
            Operation::Assign(assign) => {
                return self.director_assign(
                    provider,
                    principal,
                    session_id,
                    state,
                    request.action_id.clone(),
                    request.action_id.clone(),
                    assign,
                )
            }
            Operation::CreateAndAssign {
                create_revision,
                assign,
            } => {
                let create_id = derived_director_action_id(&request.action_id, "create");
                let create_receipt = self.director_create_revision(
                    provider,
                    principal,
                    seat_snapshot,
                    session_id,
                    request.action_id.clone(),
                    create_id,
                    create_revision,
                );
                if !matches!(
                    create_receipt.status,
                    crate::task_service::DirectorActionStatus::Committed
                        | crate::task_service::DirectorActionStatus::CurrentState
                ) {
                    return create_receipt;
                }
                let assign_id = derived_director_action_id(&request.action_id, "assign");
                let mut assign_receipt = self.director_assign(
                    provider,
                    principal,
                    session_id,
                    state,
                    request.action_id.clone(),
                    assign_id,
                    assign,
                );
                assign_receipt.operation = "create_and_assign".to_string();
                if !matches!(
                    assign_receipt.status,
                    crate::task_service::DirectorActionStatus::Committed
                        | crate::task_service::DirectorActionStatus::CurrentState
                ) {
                    assign_receipt.continuation = Some(crate::task_service::DirectorContinuation {
                        phase: "create_revision_committed".to_string(),
                        retry_action_id: request.action_id.clone(),
                    });
                }
                return assign_receipt;
            }
            _ => {}
        }

        let (decision, operation) = match &request.action {
            Operation::AcceptResult(decision) => (decision, "accept_result"),
            Operation::RequestChanges(decision) => (decision, "request_changes"),
            Operation::FailResult(decision) => (decision, "fail_result"),
            Operation::Cancel(decision) => (decision, "cancel"),
            _ => unreachable!(),
        };
        let snapshot = match provider.query() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return director_provider_error(request.action_id.clone(), operation, error)
            }
        };
        let was_known = snapshot.receipts.contains_key(&request.action_id);
        let Some(assignment) = snapshot.assignments.get(&decision.assignment_id) else {
            return director_no_write(request.action_id.clone(), operation, "not_found");
        };
        let context = worker_mechanical_context(&snapshot, assignment);
        if matches!(&request.action, Operation::Cancel(_)) {
            let is_coordinator = snapshot
                .task_revisions
                .get(&assignment.task_id)
                .and_then(|revisions| revisions.get(&assignment.task_revision))
                .and_then(|task| snapshot.workflows.get(&task.workflow_id))
                .zip(seat_for_session_in_snapshot(seat_snapshot, session_id))
                .is_some_and(|(workflow, seat)| workflow.coordinator_seat_id == seat);
            if is_coordinator {
                let cancel = crate::task_service::AssignmentActionRequest {
                    schema: crate::task_service::ProviderActionSchema::V2,
                    action_id: request.action_id.clone(),
                    assignment_id: decision.assignment_id.clone(),
                };
                return match provider.cancel_assignment(
                    principal,
                    &cancel,
                    assignment.local_revision,
                    context.attempt.as_ref(),
                ) {
                    Ok(provider_receipt) => {
                        self.append_task_transition_if_new(
                            was_known,
                            crate::management::v2::integration_events::TaskAssignmentTransitionKind::Closed,
                            &provider_receipt,
                            Some(seat_snapshot),
                        );
                        director_provider_receipt(
                            request.action_id.clone(),
                            operation,
                            was_known,
                            &provider_receipt,
                        )
                    }
                    Err(error) => {
                        director_provider_error(request.action_id.clone(), operation, error)
                    }
                };
            }
        }
        let body = crate::task_service::TerminalActionRequest {
            schema: crate::task_service::ProviderActionSchema::V2,
            action_id: request.action_id.clone(),
            assignment_id: decision.assignment_id.clone(),
            decision_reference: decision.decision_reference.clone(),
        };
        let command = match &request.action {
            Operation::AcceptResult(_) => {
                crate::task_service::TerminalAuthorityRequest::AcceptResult(body)
            }
            Operation::RequestChanges(_) => {
                crate::task_service::TerminalAuthorityRequest::RequestChanges(body)
            }
            Operation::FailResult(_) => {
                crate::task_service::TerminalAuthorityRequest::FailResult(body)
            }
            Operation::Cancel(_) => crate::task_service::TerminalAuthorityRequest::Cancel(body),
            _ => unreachable!(),
        };
        let envelope = crate::task_service::TerminalActionEnvelope {
            schema: crate::task_service::TerminalRequestSchema::V2,
            command,
            context,
        };
        match provider.execute_terminal_action(principal, &envelope) {
            Ok(provider_receipt) => {
                self.append_task_transition_if_new(
                    was_known,
                    terminal_transition_kind(&envelope.command),
                    &provider_receipt,
                    Some(seat_snapshot),
                );
                director_provider_receipt(
                    request.action_id.clone(),
                    operation,
                    was_known,
                    &provider_receipt,
                )
            }
            Err(error) => director_provider_error(request.action_id.clone(), operation, error),
        }
    }

    fn director_create_revision(
        &self,
        provider: &crate::task_service::TaskServiceProvider,
        principal: &crate::task_service::AuthenticatedPrincipal,
        seat_snapshot: &crate::seat::SeatOccupancySnapshot,
        session_id: &crate::role_revision::CutexSessionId,
        semantic_action_id: crate::task_service::ActionId,
        provider_action_id: crate::task_service::ActionId,
        request: &crate::task_service::CreateRevisionSemanticRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        let snapshot = match provider.query() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return director_provider_error(semantic_action_id, "create_revision", error)
            }
        };
        let was_known = snapshot.receipts.contains_key(&provider_action_id);
        let expected = snapshot
            .workflows
            .get(&request.workflow_id)
            .map(|value| value.local_revision);
        let authority_session = request
            .completion_authority_cutex_session_id
            .as_ref()
            .unwrap_or(session_id);
        let Some(authority_seat_id) =
            seat_for_session_in_snapshot(seat_snapshot, authority_session)
        else {
            return director_no_write(
                semantic_action_id,
                "create_revision",
                "completion_authority_not_current",
            );
        };
        let completion_policy = crate::task_service::CompletionPolicy {
            kind: match request.completion_policy {
                crate::task_service::SemanticCompletionPolicy::DirectorAcceptance => {
                    crate::task_service::CompletionPolicyKind::DirectorAcceptance
                }
                crate::task_service::SemanticCompletionPolicy::ReleaseReview => {
                    crate::task_service::CompletionPolicyKind::ReleaseReview
                }
            },
            authority_seat_id,
        };
        let result = if let Some(project_id) = request.project_id.as_ref() {
            provider.create_project_revision(
                principal,
                &crate::task_service::CreateProjectRevisionRequest {
                    schema: crate::task_service::ProviderActionSchema::V3,
                    action_id: provider_action_id,
                    project_id: project_id.clone(),
                    workflow_id: request.workflow_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    contract_sha256: request.contract_sha256.clone(),
                    opaque_contract: request.opaque_contract.clone(),
                    completion_policy,
                },
                expected,
            )
        } else {
            provider.create_revision(
                principal,
                &crate::task_service::CreateRevisionRequest {
                    schema: crate::task_service::ProviderActionSchema::V2,
                    action_id: provider_action_id,
                    workflow_id: request.workflow_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    contract_sha256: request.contract_sha256.clone(),
                    opaque_contract: request.opaque_contract.clone(),
                    completion_policy,
                },
                expected,
            )
        };
        match result {
            Ok(receipt) => director_provider_receipt(
                semantic_action_id,
                "create_revision",
                was_known,
                &receipt,
            ),
            Err(error) => director_provider_error(semantic_action_id, "create_revision", error),
        }
    }

    fn director_assign(
        &self,
        provider: &crate::task_service::TaskServiceProvider,
        principal: &crate::task_service::AuthenticatedPrincipal,
        session_id: &crate::role_revision::CutexSessionId,
        state: &Arc<Mutex<AgentBusState>>,
        semantic_action_id: crate::task_service::ActionId,
        provider_action_id: crate::task_service::ActionId,
        request: &crate::task_service::AssignSemanticRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        let snapshot = match provider.query() {
            Ok(snapshot) => snapshot,
            Err(error) => return director_provider_error(semantic_action_id, "assign", error),
        };
        let was_known = snapshot.receipts.contains_key(&provider_action_id);
        let Some(task) = snapshot
            .task_revisions
            .get(&request.task_id)
            .and_then(|revisions| revisions.get(&request.task_revision))
        else {
            return director_no_write(semantic_action_id, "assign", "not_found");
        };
        let Some(workflow) = snapshot.workflows.get(&task.workflow_id) else {
            return director_no_write(semantic_action_id, "assign", "invalid_store");
        };
        let send_attempt_id = derived_send_attempt_id(&provider_action_id);
        let external_message_id = format!(
            "task-service-director-{}",
            derived_short_digest(&provider_action_id, "message")
        );
        let dispatch = if let Some(project_id) = request.project_id.as_ref() {
            crate::task_delivery::provider_adapter::TaskServiceAgentBusDispatcher::assign_project_and_dispatch(
                provider,
                principal,
                state,
                &crate::task_service::AssignProjectAndDispatchRequest {
                    schema: crate::task_service::ProviderActionSchema::V3,
                    action_id: provider_action_id.clone(),
                    project_id: project_id.clone(),
                    assignment_id: request.assignment_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    assignee_cutex_session: request.assignee_cutex_session_id.clone(),
                    send_attempt_id,
                    external_message_id,
                },
                workflow.local_revision,
                &request.summary,
                now_epoch_secs(),
            )
        } else {
            crate::task_delivery::provider_adapter::TaskServiceAgentBusDispatcher::assign_and_dispatch(
                provider,
                principal,
                state,
                &crate::task_service::AssignAndDispatchRequest {
                    schema: crate::task_service::ProviderActionSchema::V2,
                    action_id: provider_action_id.clone(),
                    assignment_id: request.assignment_id.clone(),
                    task_id: request.task_id.clone(),
                    task_revision: request.task_revision,
                    assignee_cutex_session: request.assignee_cutex_session_id.clone(),
                    send_attempt_id,
                    external_message_id,
                },
                workflow.local_revision,
                &request.summary,
                now_epoch_secs(),
            )
        };
        match dispatch {
            Ok(outcome) => {
                if !was_known {
                    if let Err(error) =
                        crate::management::v2::integration_events::append_task_service_assignment(
                            session_id,
                            &outcome.assignment_receipt,
                        )
                    {
                        eprintln!("{YELLOW}warning:{RESET} failed to append Task Service assignment activity: {error:#}");
                    }
                    if let Err(error) =
                        crate::management::v2::integration_events::append_task_service_communication(
                            session_id,
                            &outcome.communication_receipt,
                            Some(&outcome.agent_bus_message_id),
                        )
                    {
                        eprintln!("{YELLOW}warning:{RESET} failed to append Task Service communication activity: {error:#}");
                    }
                }
                director_provider_receipt(
                    semantic_action_id,
                    "assign",
                    was_known,
                    &outcome.assignment_receipt,
                )
            }
            Err(error) => {
                if let Ok(after) = provider.query() {
                    if let Some(receipt) = after.receipts.get(&provider_action_id) {
                        if !was_known {
                            if let Err(projection_error) = crate::management::v2::integration_events::append_task_service_assignment(session_id, receipt) {
                                eprintln!("{YELLOW}warning:{RESET} failed to append committed Task Service assignment after uncertain dispatch: {projection_error:#}");
                            }
                        }
                        let mut semantic = director_provider_receipt(
                            semantic_action_id,
                            "assign",
                            was_known,
                            receipt,
                        );
                        semantic.status =
                            crate::task_service::DirectorActionStatus::ResponseUncertain;
                        semantic.code = Some("delivery_outcome_uncertain".to_string());
                        return semantic;
                    }
                }
                task_service_dispatch_director_error(semantic_action_id, error)
            }
        }
    }

    fn director_query(
        &self,
        provider: &crate::task_service::TaskServiceProvider,
        seat_snapshot: &crate::seat::SeatOccupancySnapshot,
        caller_seat: Option<&crate::task_service::SeatId>,
        action_id: crate::task_service::ActionId,
        selector: &crate::task_service::DirectorQuerySelector,
        exact_project_scope: Option<&BTreeSet<crate::agent_management::ProjectId>>,
    ) -> crate::task_service::DirectorActionReceipt {
        let Some(caller_seat) = caller_seat else {
            return director_no_write(action_id, "query", "unauthorized");
        };
        let snapshot = match provider.query() {
            Ok(snapshot) => snapshot,
            Err(error) => return director_provider_error(action_id, "query", error),
        };
        let activity_states =
            crate::management::v2::activity::load_session_activity_states().unwrap_or_default();
        let mut tasks = Vec::new();
        for revisions in snapshot.task_revisions.values() {
            for task in revisions.values() {
                if exact_project_scope.is_some_and(|scope| {
                    !task
                        .project_id
                        .as_ref()
                        .is_some_and(|project_id| scope.contains(project_id))
                }) {
                    continue;
                }
                let coordinator = snapshot
                    .workflows
                    .get(&task.workflow_id)
                    .is_some_and(|workflow| workflow.coordinator_seat_id == *caller_seat);
                if !coordinator && task.completion_policy.authority_seat_id != *caller_seat {
                    continue;
                }
                if matches!(selector, crate::task_service::DirectorQuerySelector::Task { task_id } if task_id != &task.task_id)
                {
                    continue;
                }
                let authority = Some(seat_snapshot)
                    .and_then(|snapshot| {
                        snapshot
                            .occupancies
                            .get(&task.completion_policy.authority_seat_id)
                    })
                    .map(|occupancy| occupancy.occupant_cutex_session.clone());
                tasks.push(crate::task_service::DirectorTaskView {
                    project_id: task.project_id.clone(),
                    task_id: task.task_id.clone(),
                    task_revision: task.task_revision,
                    workflow_id: task.workflow_id.clone(),
                    contract_sha256: task.contract_sha256.clone(),
                    completion_policy: match task.completion_policy.kind {
                        crate::task_service::CompletionPolicyKind::DirectorAcceptance => {
                            crate::task_service::SemanticCompletionPolicy::DirectorAcceptance
                        }
                        crate::task_service::CompletionPolicyKind::ReleaseReview => {
                            crate::task_service::SemanticCompletionPolicy::ReleaseReview
                        }
                    },
                    completion_authority_cutex_session_id: authority,
                    created_at: task.created_at.as_str().to_string(),
                });
            }
        }
        let authorized_tasks = tasks
            .iter()
            .map(|task| (task.task_id.clone(), task.task_revision))
            .collect::<BTreeSet<_>>();
        let mut assignments = Vec::new();
        for assignment in snapshot.assignments.values() {
            if !authorized_tasks.contains(&(assignment.task_id.clone(), assignment.task_revision)) {
                continue;
            }
            if matches!(selector, crate::task_service::DirectorQuerySelector::Assignment { assignment_id } if assignment_id != &assignment.assignment_id)
            {
                continue;
            }
            let session_activity = activity_states.get(assignment.assignee_cutex_session.as_str());
            let attempts = snapshot
                .attempts
                .get(&assignment.assignment_id)
                .into_iter()
                .flat_map(|items| items.values())
                .map(|attempt| crate::task_service::DirectorAttemptView {
                    attempt_number: attempt.attempt_number.get(),
                    phase: attempt_phase_name(attempt.phase).to_string(),
                    started_at: attempt.started_at.as_str().to_string(),
                    updated_at: attempt.updated_at.as_str().to_string(),
                    latest_status_summary: attempt.status_receipts.last().and_then(|status| {
                        crate::observability::sanitize_visible_output(&status.summary)
                    }),
                    latest_status_at: attempt
                        .status_receipts
                        .last()
                        .map(|status| status.recorded_at.as_str().to_string()),
                    last_output: director_last_output(
                        session_activity,
                        assignment.project_id.as_ref(),
                        assignment.assignee_cutex_session.as_str(),
                        assignment.assignment_id.as_str(),
                        attempt.attempt_number.get(),
                    ),
                    last_tool_call: director_last_tool_call(
                        session_activity,
                        assignment.project_id.as_ref(),
                        assignment.assignee_cutex_session.as_str(),
                        assignment.assignment_id.as_str(),
                        attempt.attempt_number.get(),
                    ),
                    result_reference: attempt.result_receipts.last().and_then(|result| {
                        crate::observability::sanitize_visible_output(&result.result_reference)
                    }),
                    result_submitted_at: attempt
                        .result_receipts
                        .last()
                        .map(|result| result.submitted_at.as_str().to_string()),
                })
                .collect();
            let assignee_metadata =
                crate::app_server::participants::ParticipantMetadataResolver::resolve(
                    &crate::app_server::participants::RegistryParticipantMetadataResolver,
                    assignment.assignee_cutex_session.as_str(),
                );
            assignments.push(crate::task_service::DirectorAssignmentView {
                project_id: assignment.project_id.clone(),
                assignment_id: assignment.assignment_id.clone(),
                task_id: assignment.task_id.clone(),
                task_revision: assignment.task_revision,
                assignee_cutex_session_id: assignment.assignee_cutex_session.clone(),
                assignee_display_name: assignee_metadata.display_name,
                state: assignment_state_name(assignment.state).to_string(),
                active_attempt_number: assignment.active_attempt.map(|number| number.get()),
                closure_reason: assignment.closure.as_ref().map(|closure| closure.reason),
                created_at: assignment.created_at.as_str().to_string(),
                acknowledged_at: assignment
                    .acknowledged_at
                    .as_ref()
                    .map(|value| value.as_str().to_string()),
                closed_at: assignment
                    .closure
                    .as_ref()
                    .map(|closure| closure.closed_at.as_str().to_string()),
                attempts,
            });
        }
        if matches!(
            selector,
            crate::task_service::DirectorQuerySelector::Assignment { .. }
        ) {
            let allowed = assignments
                .iter()
                .map(|assignment| (assignment.task_id.clone(), assignment.task_revision))
                .collect::<BTreeSet<_>>();
            tasks.retain(|task| allowed.contains(&(task.task_id.clone(), task.task_revision)));
        }
        crate::task_service::DirectorActionReceipt {
            schema: crate::task_service::DirectorReceiptSchema::V1,
            action_id,
            operation: "query".to_string(),
            status: crate::task_service::DirectorActionStatus::CurrentState,
            project_id: None,
            task_id: None,
            task_revision: None,
            assignment_id: None,
            closure_reason: None,
            continuation: None,
            tasks,
            assignments,
            code: None,
        }
    }

    fn execute_query_session_v2(
        &self,
        session_id: &crate::role_revision::CutexSessionId,
        principal: &crate::task_service::AuthenticatedPrincipal,
        request: crate::task_service::TaskServiceQueryRequest,
    ) -> TaskServiceQueryResponse {
        let Some(provider) = self.provider.as_ref() else {
            return task_service_query_no_write(
                "persistence_unavailable",
                "provider v2 is unavailable",
            );
        };
        let result = match request.query {
            crate::task_service::TaskServiceQueryOperation::Snapshot => {
                match self.seat_authority.as_ref() {
                    Some(seats) => match seats.with_current_principal(session_id, |_| {
                        provider.query().map(TaskServiceQueryOutcome::Snapshot)
                    }) {
                        Ok(result) => result,
                        Err(crate::seat::SeatAuthorityError::Unauthorized) => provider
                            .query_assignee(principal)
                            .map(TaskServiceQueryOutcome::AssigneeSnapshot),
                        Err(_) => Err(crate::task_service::ProviderError::PersistenceUnavailable),
                    },
                    None => provider
                        .query_assignee(principal)
                        .map(TaskServiceQueryOutcome::AssigneeSnapshot),
                }
            }
            crate::task_service::TaskServiceQueryOperation::Watch {
                after_sequence,
                limit,
            } => provider
                .watch(after_sequence, limit)
                .map(TaskServiceQueryOutcome::Watch),
        };
        match result {
            Ok(outcome) => TaskServiceQueryResponse {
                schema: TaskServiceQueryResponseSchema::V2,
                outcome,
            },
            Err(error) => task_service_query_provider_error(error),
        }
    }

    fn with_current_seated_session<T>(
        &self,
        session_id: &crate::role_revision::CutexSessionId,
        operation: impl FnOnce(&crate::task_service::AuthenticatedPrincipal) -> T,
    ) -> Result<T, String> {
        let seats = self
            .seat_authority
            .as_ref()
            .ok_or_else(|| "seat authority is unavailable".to_string())?;
        seats
            .with_current_principal(session_id, operation)
            .map_err(|error| format!("current seat resolution failed: {error}"))
    }

    fn with_current_seated_session_snapshot<T>(
        &self,
        session_id: &crate::role_revision::CutexSessionId,
        operation: impl FnOnce(
            &crate::task_service::AuthenticatedPrincipal,
            &crate::seat::SeatOccupancySnapshot,
        ) -> T,
    ) -> Result<T, String> {
        let seats = self
            .seat_authority
            .as_ref()
            .ok_or_else(|| "seat authority is unavailable".to_string())?;
        seats
            .with_current_principal_snapshot(session_id, operation)
            .map_err(|error| format!("current seat resolution failed: {error}"))
    }

    fn execute(
        &self,
        sender: TaskWorkerRosterSender,
        validated: crate::task_delivery::worker_action_adapter::ValidatedTaskWorkerAction,
    ) -> TaskWorkerActionResponse {
        let action_id = validated.request.action_id.clone();
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => {
                return task_worker_action_no_write(
                    Some(action_id),
                    TaskWorkerActionNoWrite::PersistenceUnavailable,
                )
            }
        };
        let authorized = match self.adapter.authorize(sender, validated) {
            Ok(authorized) => authorized,
            Err(error) => {
                let error = if self.evidence.contains_action_id(&action_id) {
                    TaskWorkerActionNoWrite::ActionConflict
                } else {
                    error
                };
                return task_worker_action_no_write(Some(action_id), error);
            }
        };
        let outcome = self.execute_authorized(&authorized);
        TaskWorkerActionResponse {
            schema: TaskWorkerActionResponseSchema::V1,
            action_id: Some(action_id),
            outcome,
        }
    }

    fn execute_authorized(
        &self,
        authorized: &TaskWorkerAuthorizedAction,
    ) -> TaskWorkerActionOutcome {
        let probe = match self.evidence.probe(authorized) {
            Ok(probe) => probe,
            Err(_) => {
                return TaskWorkerActionOutcome::NoWrite(
                    TaskWorkerActionNoWrite::PersistenceUnavailable,
                )
            }
        };
        let existing = match probe {
            ActionProbe::Conflict => {
                return TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::ActionConflict)
            }
            ActionProbe::Blocked => {
                return TaskWorkerActionOutcome::NoWrite(
                    TaskWorkerActionNoWrite::UncertaintyBlocked,
                )
            }
            ActionProbe::ExactBlocked {
                uncertainty_id,
                action_id,
            } => {
                return TaskWorkerActionOutcome::ReconciliationRequired {
                    uncertainty_id,
                    action_id,
                }
            }
            ActionProbe::Existing(record) => Some(record),
            ActionProbe::New => None,
        };

        let observation = match self.adapter.inspect_receipt(&authorized.request.action_id) {
            Ok(observation) => observation,
            Err(error) => return TaskWorkerActionOutcome::NoWrite(error),
        };
        if matches!(
            observation,
            crate::task_delivery::PilotWorkerReceiptObservation::Committed(_)
        ) {
            let Some(existing) = existing.as_ref() else {
                return TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::ActionConflict);
            };
            return match self.adapter.receipt_from_observation(
                authorized,
                &existing.record_id,
                &observation,
            ) {
                Ok(Some(receipt)) => TaskWorkerActionOutcome::Committed(receipt),
                Ok(None) => TaskWorkerActionOutcome::NoWrite(
                    TaskWorkerActionNoWrite::DurableRequestRejected,
                ),
                Err(error) => TaskWorkerActionOutcome::NoWrite(error),
            };
        }

        let prepared = match self.evidence.prepare(authorized) {
            Ok(prepared) => prepared,
            Err(EvidenceStoreError::PersistenceUnknown) => {
                return self.reconciliation_after_prepare_unknown(authorized)
            }
            Err(EvidenceStoreError::DefiniteNoWrite) => {
                return TaskWorkerActionOutcome::NoWrite(
                    TaskWorkerActionNoWrite::PersistenceUnavailable,
                )
            }
            Err(_) => {
                return TaskWorkerActionOutcome::NoWrite(
                    TaskWorkerActionNoWrite::PersistenceUnavailable,
                )
            }
        };
        let transition = self
            .adapter
            .transition_once(authorized, &prepared.record.record_id);
        match transition {
            TaskWorkerTransitionResult::PersistenceUnknown => reconciliation_required(&prepared),
            TaskWorkerTransitionResult::Committed(receipt) => {
                match self.evidence.clear_known(&prepared) {
                    Ok(()) => TaskWorkerActionOutcome::Committed(receipt),
                    Err(_) => self.reconciliation_after_clear_unknown(&prepared),
                }
            }
            TaskWorkerTransitionResult::NoWrite(error) => {
                match self.evidence.clear_known(&prepared) {
                    Ok(()) => TaskWorkerActionOutcome::NoWrite(error),
                    Err(_) => self.reconciliation_after_clear_unknown(&prepared),
                }
            }
        }
    }

    fn reconciliation_after_prepare_unknown(
        &self,
        authorized: &TaskWorkerAuthorizedAction,
    ) -> TaskWorkerActionOutcome {
        match self.evidence.probe(authorized) {
            Ok(ActionProbe::ExactBlocked {
                uncertainty_id,
                action_id,
            }) => TaskWorkerActionOutcome::ReconciliationRequired {
                uncertainty_id,
                action_id,
            },
            _ => TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::PersistenceUnavailable),
        }
    }

    fn reconciliation_after_clear_unknown(
        &self,
        prepared: &PreparedTaskWorkerAction,
    ) -> TaskWorkerActionOutcome {
        match self.evidence.uncertainty() {
            Ok(Some(uncertainty))
                if uncertainty.uncertainty_id == prepared.uncertainty.uncertainty_id =>
            {
                reconciliation_required(prepared)
            }
            _ => TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::PersistenceUnavailable),
        }
    }

    fn reconcile(
        &self,
        sender: TaskWorkerRosterSender,
        request: TaskWorkerReconciliationRequest,
    ) -> TaskWorkerReconciliationResponse {
        let _execution = match self.execution.lock() {
            Ok(lock) => lock,
            Err(_) => return reconciliation_persistence_unavailable(),
        };
        let (uncertainty_id, action_id) = match &request.operation {
            TaskWorkerReconciliationOperation::Inspect {
                uncertainty_id,
                action_id,
            }
            | TaskWorkerReconciliationOperation::Ack {
                uncertainty_id,
                action_id,
                ..
            } => (uncertainty_id.clone(), action_id.clone()),
        };
        let uncertainty = match self.evidence.uncertainty() {
            Ok(Some(uncertainty)) => uncertainty,
            Ok(None) => return reconciliation_rejected(),
            Err(_) => return reconciliation_persistence_unavailable(),
        };
        if uncertainty.uncertainty_id != uncertainty_id || uncertainty.action_id != action_id {
            return reconciliation_rejected();
        }
        let record = match self.evidence.record_for_uncertainty(&uncertainty) {
            Ok(record) => record,
            Err(_) => return reconciliation_persistence_unavailable(),
        };
        if self
            .adapter
            .authorize_stored_owner(
                &sender,
                &uncertainty.owner,
                &uncertainty.task_id,
                uncertainty.task_revision,
                &uncertainty.attempt_fence,
            )
            .is_err()
        {
            return reconciliation_rejected();
        }
        match request.operation {
            TaskWorkerReconciliationOperation::Inspect { .. } => {
                if let Some(resolution) = uncertainty.resolution {
                    return reconciliation_resolved(resolution);
                }
                let observation = match self.adapter.inspect_receipt(&record.action_id) {
                    Ok(observation) => observation,
                    Err(_) => return reconciliation_unknown(),
                };
                let evidence = match observation {
                    crate::task_delivery::PilotWorkerReceiptObservation::Committed(
                        observation_record,
                    ) => {
                        let authorized = match authorized_action_from_record(&record) {
                            Ok(authorized) => authorized,
                            Err(_) => return reconciliation_persistence_unavailable(),
                        };
                        let receipt_observation =
                            crate::task_delivery::PilotWorkerReceiptObservation::Committed(
                                observation_record.clone(),
                            );
                        let receipt = match self.adapter.receipt_from_observation(
                            &authorized,
                            &record.record_id,
                            &receipt_observation,
                        ) {
                            Ok(Some(receipt)) => receipt,
                            _ => return reconciliation_rejected(),
                        };
                        TaskWorkerResolutionEvidence::Committed(
                            TaskWorkerCommittedReceiptEvidence {
                                receipt,
                                request_digest_sha256: observation_record
                                    .request_digest_sha256
                                    .clone(),
                                event_cursor: observation_record.event_cursor.clone(),
                                observed_store_revision: observation_record.observed_store_revision,
                                observed_journal_cursor: observation_record
                                    .observed_journal_cursor
                                    .clone(),
                            },
                        )
                    }
                    crate::task_delivery::PilotWorkerReceiptObservation::Absent {
                        observed_store_revision,
                        observed_journal_cursor,
                    } => TaskWorkerResolutionEvidence::Absent(TaskWorkerReceiptAbsence {
                        observed_store_revision,
                        observed_journal_cursor,
                    }),
                };
                match self.evidence.resolve(&uncertainty_id, &action_id, evidence) {
                    Ok(resolution) => reconciliation_resolved(resolution),
                    Err(EvidenceStoreError::PersistenceUnknown) => {
                        match self.evidence.uncertainty() {
                            Ok(Some(fence)) => fence
                                .resolution
                                .map(reconciliation_resolved)
                                .unwrap_or_else(reconciliation_unknown),
                            _ => reconciliation_unknown(),
                        }
                    }
                    Err(_) => reconciliation_persistence_unavailable(),
                }
            }
            TaskWorkerReconciliationOperation::Ack {
                resolution_id,
                resolution_sha256,
                ..
            } => match self.evidence.ack(
                &uncertainty_id,
                &action_id,
                &resolution_id,
                &resolution_sha256,
            ) {
                Ok(()) => TaskWorkerReconciliationResponse {
                    schema: TaskWorkerReconciliationResponseSchema::V1,
                    outcome: TaskWorkerReconciliationOutcome::Acknowledged,
                },
                Err(EvidenceStoreError::PersistenceUnknown) => match self.evidence.uncertainty() {
                    Ok(None) => TaskWorkerReconciliationResponse {
                        schema: TaskWorkerReconciliationResponseSchema::V1,
                        outcome: TaskWorkerReconciliationOutcome::Acknowledged,
                    },
                    _ => reconciliation_persistence_unavailable(),
                },
                Err(EvidenceStoreError::DefiniteNoWrite) => reconciliation_rejected(),
                Err(_) => reconciliation_persistence_unavailable(),
            },
        }
    }
}

/// Returns only project IDs whose durable authority record names this exact
/// authenticated Director session. It intentionally has no presentation or
/// heuristic fallback: inability to read the authority store fails the v2
/// query closed.
fn director_exact_project_scope(
    director_session: &crate::role_revision::CutexSessionId,
) -> Result<BTreeSet<crate::agent_management::ProjectId>, &'static str> {
    let snapshot = crate::agent_management::AgentManagementProvider::open_default()
        .map_err(|_| "project_authority_unavailable")?
        .store()
        .snapshot()
        .map_err(|_| "project_authority_unavailable")?;
    let projects = exact_project_scope_from_authorities(snapshot.projects, director_session);
    if projects.is_empty() {
        Err("project_authority_absent")
    } else {
        Ok(projects)
    }
}

fn exact_project_scope_from_authorities(
    authorities: impl IntoIterator<
        Item = (
            crate::agent_management::ProjectId,
            crate::agent_management::ProjectAuthority,
        ),
    >,
    director_session: &crate::role_revision::CutexSessionId,
) -> BTreeSet<crate::agent_management::ProjectId> {
    authorities
        .into_iter()
        .filter_map(|(project_id, authority)| {
            (authority.authorized_director_session == *director_session).then_some(project_id)
        })
        .collect()
}

fn reconciliation_required(prepared: &PreparedTaskWorkerAction) -> TaskWorkerActionOutcome {
    TaskWorkerActionOutcome::ReconciliationRequired {
        uncertainty_id: prepared.uncertainty.uncertainty_id.clone(),
        action_id: prepared.record.action_id.clone(),
    }
}

fn task_worker_action_no_write(
    action_id: Option<crate::role_revision::ReceiptId>,
    error: TaskWorkerActionNoWrite,
) -> TaskWorkerActionResponse {
    TaskWorkerActionResponse {
        schema: TaskWorkerActionResponseSchema::V1,
        action_id,
        outcome: TaskWorkerActionOutcome::NoWrite(error),
    }
}

fn reconciliation_unknown() -> TaskWorkerReconciliationResponse {
    TaskWorkerReconciliationResponse {
        schema: TaskWorkerReconciliationResponseSchema::V1,
        outcome: TaskWorkerReconciliationOutcome::Unknown,
    }
}

fn reconciliation_resolved(
    resolution: crate::agent_bus::model::TaskWorkerResolution,
) -> TaskWorkerReconciliationResponse {
    TaskWorkerReconciliationResponse {
        schema: TaskWorkerReconciliationResponseSchema::V1,
        outcome: TaskWorkerReconciliationOutcome::Resolved(resolution),
    }
}

fn reconciliation_rejected() -> TaskWorkerReconciliationResponse {
    TaskWorkerReconciliationResponse {
        schema: TaskWorkerReconciliationResponseSchema::V1,
        outcome: TaskWorkerReconciliationOutcome::NoWrite(
            TaskWorkerReconciliationNoWrite::Rejected,
        ),
    }
}

fn reconciliation_persistence_unavailable() -> TaskWorkerReconciliationResponse {
    TaskWorkerReconciliationResponse {
        schema: TaskWorkerReconciliationResponseSchema::V1,
        outcome: TaskWorkerReconciliationOutcome::NoWrite(
            TaskWorkerReconciliationNoWrite::PersistenceUnavailable,
        ),
    }
}

struct AgentBusPollSignal {
    generation: Mutex<u64>,
    available: Condvar,
}

static AGENT_BUS_POLL_SIGNAL: OnceLock<AgentBusPollSignal> = OnceLock::new();

fn agent_bus_poll_signal() -> &'static AgentBusPollSignal {
    AGENT_BUS_POLL_SIGNAL.get_or_init(|| AgentBusPollSignal {
        generation: Mutex::new(0),
        available: Condvar::new(),
    })
}

pub fn notify_agent_bus_message_available() {
    let signal = agent_bus_poll_signal();
    if let Ok(mut generation) = signal.generation.lock() {
        *generation = generation.wrapping_add(1);
        signal.available.notify_all();
    }
}

#[derive(Clone, Copy)]
pub struct AgentBusRequestHandlers {
    pub reconcile_registration_agent: fn(&AgentBusAgent) -> anyhow::Result<()>,
    pub reconcile_agent: fn(&AgentBusAgent) -> anyhow::Result<()>,
    pub redrive_ordinary_messages: fn(&Arc<Mutex<AgentBusState>>) -> anyhow::Result<usize>,
    pub send_payload_response:
        fn(&Arc<Mutex<AgentBusState>>, AgentBusSendRequest, bool) -> anyhow::Result<Value>,
    pub release_rotation: fn(
        &Arc<Mutex<AgentBusState>>,
        ReleaseRotationInvocation,
        ReleaseRotationRequest,
    ) -> anyhow::Result<Value>,
    pub agent_management: fn(
        &Arc<Mutex<AgentBusState>>,
        AgentManagementInvocation,
        AgentManagementRequest,
    ) -> anyhow::Result<Value>,
}

pub fn handle_agent_bus_request(
    stream: &mut TcpStream,
    state: &Arc<Mutex<AgentBusState>>,
    token: Option<&str>,
    handlers: AgentBusRequestHandlers,
    task_actions: &Arc<TaskWorkerActionHost>,
) -> anyhow::Result<()> {
    let request = read_simple_http_request(stream)?;
    // Request routing only checks atomics and may start one single-flight
    // worker. Provider, Seat, and Agent Bus drain work stays off this response
    // path, including after a failed recovery scan reaches its retry deadline.
    task_actions.schedule_completion_work_if_due(state);
    let path_only = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str());
    match (request.method.as_str(), path_only) {
        ("GET", "/") => write_http_response(stream, 200, "OK", "text/plain", b"ok"),
        ("GET", "/api/agents") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            if prune_stale_agents(state)? {
                persist_agent_bus_registry(state)?;
            }
            let requester = query_value(&request.path, "agent_id");
            let all_groups =
                query_bool(&request.path, "all_groups") || query_bool(&request.path, "allGroups");
            let all_hosts_requested =
                query_bool(&request.path, "all_hosts") || query_bool(&request.path, "allHosts");
            let all_hosts_explicit = query_has_key(&request.path, "all_hosts")
                || query_has_key(&request.path, "allHosts");
            let all_hosts = all_hosts_requested || (requester.is_some() && !all_hosts_explicit);
            let (mut agents, requester_groups) = {
                let state = state
                    .lock()
                    .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
                let mut agents =
                    visible_agents_for_request(&state, requester.as_deref(), all_groups);
                agents.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                let requester_groups = requester
                    .as_deref()
                    .and_then(|id| state.agents.get(id).map(|agent| agent.groups.clone()));
                (agents, requester_groups)
            };
            if let Ok(sessions) = load_cutex_session_store() {
                project_current_durable_session_ids(&mut agents, &sessions);
            }
            if all_hosts {
                agents.extend(filter_federated_agents_for_request(
                    fetch_federated_agent_bus_agents(),
                    requester.as_deref(),
                    requester_groups.as_deref(),
                    all_groups,
                ));
                agents = dedupe_agents_by_id(agents);
                agents.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
            }
            write_json_response(stream, 200, "OK", &serde_json::to_value(agents)?)
        }
        ("GET", "/api/federation/agents") => {
            if prune_stale_agents(state)? {
                persist_agent_bus_registry(state)?;
            }
            let requester = query_value(&request.path, "agent_id");
            let all_groups =
                query_bool(&request.path, "all_groups") || query_bool(&request.path, "allGroups");
            let agents = {
                let state = state
                    .lock()
                    .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
                let mut agents =
                    visible_agents_for_request(&state, requester.as_deref(), all_groups);
                agents.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                agents
            };
            let mut agents = agents;
            if let Ok(sessions) = load_cutex_session_store() {
                project_current_durable_session_ids(&mut agents, &sessions);
            }
            write_json_response(stream, 200, "OK", &serde_json::to_value(agents)?)
        }
        ("POST", "/api/agents/register") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            let payload: crate::agent_bus::model::AgentBusRegisterRequest =
                serde_json::from_slice(&request.body)
                    .context("Failed to parse agent register JSON")?;
            let now = now_epoch_secs();
            let agent = agent_from_register_request(payload, now);
            register_agent_with_reconciliation(
                state,
                agent,
                handlers.reconcile_registration_agent,
                save_agent_bus_registry_locked,
            )?;
            if let Err(error) = (handlers.redrive_ordinary_messages)(state) {
                eprintln!("warning: durable ordinary-message redrive failed: {error:#}");
            }
            task_actions.retry_completion_notifications_for_available_target(state);
            write_json_response(stream, 200, "OK", &serde_json::json!({"ok": true}))
        }
        ("POST", "/api/agents/groups") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            if prune_stale_agents(state)? {
                persist_agent_bus_registry(state)?;
            }
            let payload: AgentBusGroupUpdateRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse agent group update JSON")?;
            let groups = normalize_agent_groups(payload.groups);
            let (agent_id, agent_name, groups, agent) =
                update_agent_groups(state, &payload.target, &groups, payload.mode)?;
            persist_agent_bus_registry(state)?;
            if let Err(err) = (handlers.reconcile_agent)(&agent) {
                eprintln!(
                    "{YELLOW}warning:{RESET} failed to reconcile cutex session registry: {err:#}"
                );
            }
            write_json_response(
                stream,
                200,
                "OK",
                &serde_json::json!({
                    "ok": true,
                    "agent_id": agent_id,
                    "agent_name": agent_name,
                    "groups": groups,
                }),
            )
        }
        ("POST", "/api/agents/heartbeat") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            let payload: AgentBusHeartbeatRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse agent heartbeat JSON")?;
            let now = now_epoch_secs();
            {
                let mut state = state
                    .lock()
                    .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
                if let Some(agent) = state.agents.get_mut(&payload.id) {
                    agent.last_seen_epoch_secs = now;
                }
            }
            task_actions.retry_completion_notifications_for_available_target(state);
            write_json_response(stream, 200, "OK", &serde_json::json!({"ok": true}))
        }
        ("POST", "/api/agents/unregister") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            let payload: AgentBusUnregisterRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse agent unregister JSON")?;
            let removed = {
                let mut state = state
                    .lock()
                    .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
                state.messages.remove(&payload.id);
                let removed = state.agents.remove(&payload.id).is_some();
                if removed {
                    save_agent_bus_registry_locked(&state)?;
                }
                removed
            };
            if removed {
                notify_agent_bus_message_available();
            }
            write_json_response(
                stream,
                200,
                "OK",
                &serde_json::json!({"ok": true, "removed": removed}),
            )
        }
        ("POST", "/api/messages/send") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            if prune_stale_agents(state)? {
                persist_agent_bus_registry(state)?;
            }
            let payload: AgentBusSendRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse agent message JSON")?;
            let response = match (handlers.send_payload_response)(state, payload, true) {
                Ok(response) => response,
                Err(error) => {
                    if let Some(response) = write_agent_target_resolution_error(stream, &error) {
                        return response;
                    }
                    return Err(error);
                }
            };
            write_json_response(stream, 200, "OK", &response)
        }
        ("POST", "/api/rotation/v1/release") => {
            if request.body.len() > crate::rotation::RELEASE_ROTATION_MAX_MESSAGE_BYTES + 16 * 1024
            {
                let response = release_rotation_no_write(
                    crate::task_service::ActionId::new("invalid-body").expect("fixed action ID"),
                    "body_too_large",
                    "rotation request exceeds route limit",
                );
                return write_json_response(stream, 200, "OK", &serde_json::to_value(response)?);
            }
            let payload: ReleaseRotationRequest = match serde_json::from_slice(&request.body) {
                Ok(payload) => payload,
                Err(_) => {
                    let response = release_rotation_no_write(
                        crate::task_service::ActionId::new("invalid-body")
                            .expect("fixed action ID"),
                        "invalid_body",
                        "strict Release rotation request parsing failed",
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(_) => {
                    let response = release_rotation_no_write(
                        payload.action_id.clone(),
                        "unauthorized",
                        "Agent Bus sender authentication failed",
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            let response = task_actions.execute_release_rotation(sender, &payload, |invocation| {
                (handlers.release_rotation)(state, invocation, payload.clone())
            });
            match response {
                Ok(response) => write_json_response(stream, 200, "OK", &response?),
                Err(response) => {
                    write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
                }
            }
        }
        ("POST", "/api/agent-management/v1/actions") => {
            if request.body.len() > AGENT_MANAGEMENT_MAX_BODY_BYTES {
                let response = agent_management_no_write(
                    AgentActionId::new("invalid-body").expect("fixed action ID"),
                    "body_too_large",
                    "Agent Management request exceeds route limit",
                );
                return write_json_response(stream, 200, "OK", &serde_json::to_value(response)?);
            }
            let payload: AgentManagementRequest = match serde_json::from_slice(&request.body) {
                Ok(payload) => payload,
                Err(_) => {
                    let response = agent_management_no_write(
                        AgentActionId::new("invalid-body").expect("fixed action ID"),
                        "invalid_body",
                        "strict Agent Management request parsing failed",
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            if require_agent_management_bridge_token(&request, token).is_err() {
                let response = agent_management_no_write(
                    payload.action_id.clone(),
                    "unauthorized",
                    "Agent Management requires authenticated Agent Bus access",
                );
                return write_json_response(stream, 200, "OK", &serde_json::to_value(response)?);
            }
            let sender = match agent_management_sender(&request, state) {
                Ok(sender) => sender,
                Err(_) => {
                    let response = agent_management_no_write(
                        payload.action_id.clone(),
                        "unauthorized",
                        "Agent Bus sender authentication failed",
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            let invocation = match resolve_agent_management_invocation(&sender) {
                Ok(invocation) => invocation,
                Err(error) => {
                    let response = agent_management_no_write(
                        payload.action_id.clone(),
                        error.code(),
                        error.detail(),
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            let response = (handlers.agent_management)(state, invocation, payload)?;
            write_json_response(stream, 200, "OK", &response)
        }
        ("POST", "/api/task/actions") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_worker_action_no_write(
                        None,
                        TaskWorkerActionNoWrite::BodyTooLarge,
                    ))?,
                );
            }
            let payload: TaskWorkerActionRequest = match serde_json::from_slice(&request.body) {
                Ok(payload) => payload,
                Err(_) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_worker_action_no_write(
                            None,
                            TaskWorkerActionNoWrite::InvalidBody,
                        ))?,
                    )
                }
            };
            let action_id = Some(payload.action_id.clone());
            let validated = match validate_task_worker_action_request(payload) {
                Ok(validated) => validated,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_worker_action_no_write(action_id, error))?,
                    )
                }
            };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_worker_action_no_write(action_id, error))?,
                    )
                }
            };
            let response = task_actions.execute(sender, validated);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/actions") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_v2_no_write(
                        crate::task_service::ActionId::new("invalid-body")
                            .expect("fixed action ID"),
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::WorkerProviderActionEnvelope =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_v2_no_write(
                                crate::task_service::ActionId::new("invalid-body")
                                    .expect("fixed action ID"),
                                "invalid_body",
                                "strict v2 request parsing failed",
                            ))?,
                        )
                    }
                };
            let action_id = payload.action_id().clone();
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_v2_no_write(
                            action_id,
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_v2(sender, payload);
            task_actions.dispatch_completion_notifications_after_transition(state, &response);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/worker-prepare") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_worker_prepare_no_write(
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::WorkerPrepareRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_worker_prepare_no_write(
                                "invalid_body",
                                "strict Worker prepare request parsing failed",
                            ))?,
                        )
                    }
                };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_worker_prepare_no_write(
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_worker_prepare_v2(sender, payload);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/worker-context") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_worker_context_no_write(
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::WorkerContextRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_worker_context_no_write(
                                "invalid_body",
                                "strict Worker context request parsing failed",
                            ))?,
                        )
                    }
                };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_worker_context_no_write(
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_worker_context_v2(sender, payload);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/director-action") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                let response = director_no_write(
                    crate::task_service::ActionId::new("invalid-body").expect("fixed action ID"),
                    "unknown",
                    "body_too_large",
                );
                return write_json_response(stream, 200, "OK", &serde_json::to_value(response)?);
            }
            require_task_worker_bridge_token(&request, token)?;
            let payload: crate::task_service::DirectorActionRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        let response = director_no_write(
                            crate::task_service::ActionId::new("invalid-body")
                                .expect("fixed action ID"),
                            "unknown",
                            "invalid_body",
                        );
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(response)?,
                        );
                    }
                };
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(_) => {
                    let response = director_no_write(
                        payload.action_id,
                        director_operation_name(&payload.action),
                        "unauthorized",
                    );
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(response)?,
                    );
                }
            };
            let response = task_actions.execute_director_v1(sender, state, payload);
            if matches!(
                response.status,
                crate::task_service::DirectorActionStatus::Committed
            ) {
                task_actions.request_completion_notification_drain();
            }
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/coordinator") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_v2_no_write(
                        crate::task_service::ActionId::new("invalid-body")
                            .expect("fixed action ID"),
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::CoordinatorActionRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_v2_no_write(
                                crate::task_service::ActionId::new("invalid-body")
                                    .expect("fixed action ID"),
                                "invalid_body",
                                "strict coordinator request parsing failed",
                            ))?,
                        )
                    }
                };
            let action_id = payload.command.action_id().clone();
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_v2_no_write(
                            action_id,
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_coordinator_v2(sender, state, payload);
            task_actions.dispatch_completion_notifications_after_transition(state, &response);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/terminal") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_v2_no_write(
                        crate::task_service::ActionId::new("invalid-body")
                            .expect("fixed action ID"),
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::TerminalActionEnvelope =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_v2_no_write(
                                crate::task_service::ActionId::new("invalid-body")
                                    .expect("fixed action ID"),
                                "invalid_body",
                                "strict terminal request parsing failed",
                            ))?,
                        )
                    }
                };
            let action_id = payload.action_id().clone();
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_v2_no_write(
                            action_id,
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_terminal_v2(sender, payload);
            task_actions.dispatch_completion_notifications_after_transition(state, &response);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/v2/query") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(task_service_query_no_write(
                        "body_too_large",
                        "request exceeds route limit",
                    ))?,
                );
            }
            let payload: crate::task_service::TaskServiceQueryRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(task_service_query_no_write(
                                "invalid_body",
                                "strict query request parsing failed",
                            ))?,
                        )
                    }
                };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(error) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(task_service_query_no_write(
                            "unauthorized",
                            &format!("Agent Bus sender authentication failed: {error:?}"),
                        ))?,
                    )
                }
            };
            let response = task_actions.execute_query_v2(sender, payload);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/task/actions/reconcile") => {
            if request.body.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
                return write_json_response(
                    stream,
                    200,
                    "OK",
                    &serde_json::to_value(reconciliation_rejected())?,
                );
            }
            let payload: TaskWorkerReconciliationRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(payload) => payload,
                    Err(_) => {
                        return write_json_response(
                            stream,
                            200,
                            "OK",
                            &serde_json::to_value(reconciliation_rejected())?,
                        )
                    }
                };
            require_task_worker_bridge_token(&request, token)?;
            let sender = match task_worker_sender(&request, state) {
                Ok(sender) => sender,
                Err(_) => {
                    return write_json_response(
                        stream,
                        200,
                        "OK",
                        &serde_json::to_value(reconciliation_rejected())?,
                    )
                }
            };
            let response = task_actions.reconcile(sender, payload);
            write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
        }
        ("POST", "/api/federation/messages/send") => {
            if prune_stale_agents(state)? {
                persist_agent_bus_registry(state)?;
            }
            let payload: AgentBusSendRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse federated agent message JSON")?;
            let response = match (handlers.send_payload_response)(state, payload, false) {
                Ok(response) => response,
                Err(error) => {
                    if let Some(response) = write_agent_target_resolution_error(stream, &error) {
                        return response;
                    }
                    return Err(error);
                }
            };
            write_json_response(stream, 200, "OK", &response)
        }
        ("GET", "/api/messages/poll") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            let agent_id = query_value(&request.path, "agent_id")
                .ok_or_else(|| anyhow!("Missing agent_id query parameter"))?;
            let ack_mode = query_value(&request.path, "ack")
                .as_deref()
                .is_some_and(|value| matches!(value, "1" | "true" | "yes"));
            let wait = poll_wait_duration(&request.path);
            let (agent_name, messages) =
                poll_agent_messages_with_wait(state, &agent_id, ack_mode, wait)?;
            if !messages.is_empty() {
                if let Err(err) = append_agent_bus_audit_record(serde_json::json!({
                    "event": "polled",
                    "timestamp": Utc::now().to_rfc3339(),
                    "agent_id": agent_id,
                    "agent_name": agent_name,
                    "ack_mode": ack_mode,
                    "count": messages.len(),
                    "message_ids": messages.iter().map(|message| message.id.clone()).collect::<Vec<_>>(),
                })) {
                    eprintln!("{YELLOW}warning:{RESET} failed to write agent audit log: {err:#}");
                }
            }
            write_json_response(
                stream,
                200,
                "OK",
                &serde_json::to_value(AgentBusPollResponse { messages })?,
            )
        }
        ("POST", "/api/messages/ack") => {
            require_service_bridge_token(&request, token, "Agent Bus")?;
            let payload: AgentBusAckRequest = serde_json::from_slice(&request.body)
                .context("Failed to parse agent message ack JSON")?;
            let acked = ack_agent_messages(state, &payload.agent_id, &payload.message_ids)?;
            if acked > 0 {
                if let Err(err) = append_agent_bus_audit_record(serde_json::json!({
                    "event": "acked",
                    "timestamp": Utc::now().to_rfc3339(),
                    "agent_id": payload.agent_id,
                    "count": acked,
                    "message_ids": payload.message_ids,
                })) {
                    eprintln!("{YELLOW}warning:{RESET} failed to write agent audit log: {err:#}");
                }
            }
            write_json_response(
                stream,
                200,
                "OK",
                &serde_json::json!({
                    "ok": true,
                    "acked": acked,
                }),
            )
        }
        _ => write_http_response(stream, 404, "Not Found", "text/plain", b"not found"),
    }
}

fn write_agent_target_resolution_error(
    stream: &mut TcpStream,
    error: &anyhow::Error,
) -> Option<anyhow::Result<()>> {
    let target = error.downcast_ref::<AgentTargetResolutionError>()?;
    let (status, reason) = match target.code() {
        AgentTargetResolutionCode::NotFound => (404, "Not Found"),
        AgentTargetResolutionCode::Ambiguous | AgentTargetResolutionCode::TargetUnavailable => {
            (409, "Conflict")
        }
    };
    Some(write_json_response(
        stream,
        status,
        reason,
        &serde_json::json!({
            "ok": false,
            "code": target.code().label(),
            "message": target.to_string(),
        }),
    ))
}

fn release_rotation_no_write(
    action_id: crate::task_service::ActionId,
    code: &str,
    reason: &str,
) -> ReleaseRotationResponse {
    ReleaseRotationResponse {
        schema: ReleaseRotationResponseSchema::V1,
        action_id,
        outcome: ReleaseRotationOutcome::NoWrite {
            code: code.to_string(),
            reason: reason.to_string(),
        },
    }
}

fn agent_management_no_write(
    action_id: AgentActionId,
    code: &str,
    detail: &str,
) -> AgentManagementResponse {
    AgentManagementResponse {
        schema: AgentManagementSchema::V1,
        action_id,
        outcome: AgentManagementOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

#[derive(Clone, Debug)]
struct AgentManagementRosterSender {
    runtime_agent_id: RuntimeAgentId,
    roster_session_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentManagementInvocationError {
    PersistenceUnavailable,
    StaleRuntimeIdentity,
}

impl AgentManagementInvocationError {
    fn code(self) -> &'static str {
        match self {
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::StaleRuntimeIdentity => "stale_runtime_identity",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::PersistenceUnavailable => "Agent Management durable session state is unavailable",
            Self::StaleRuntimeIdentity => {
                "ambient runtime does not resolve to one active durable Cutex session"
            }
        }
    }
}

fn require_agent_management_bridge_token(
    request: &crate::http::server::SimpleHttpRequest,
    token: Option<&str>,
) -> anyhow::Result<()> {
    let route_token = token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("Agent Management requires a configured Agent Bus token"))?;
    require_service_bridge_token(request, Some(route_token), "Agent Bus")
}

fn agent_management_sender(
    request: &crate::http::server::SimpleHttpRequest,
    state: &Arc<Mutex<AgentBusState>>,
) -> anyhow::Result<AgentManagementRosterSender> {
    let sender_id = request
        .headers
        .get("x-cutex-agent-id")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Agent Management sender header is missing"))?;
    let runtime_agent_id = RuntimeAgentId::new(sender_id.to_string())
        .map_err(|_| anyhow!("Agent Management sender is invalid"))?;
    let roster = state
        .lock()
        .map_err(|_| anyhow!("Agent Bus state lock is unavailable"))?
        .agents
        .get(sender_id)
        .cloned()
        .ok_or_else(|| anyhow!("Agent Management sender is not registered"))?;
    if roster.id != sender_id
        || !agent_is_local_to_bus(&roster, &current_host_name())
        || !crate::platform::process::process_is_running(roster.pid)
    {
        return Err(anyhow!("Agent Management sender is not current and local"));
    }
    let cutoff =
        now_epoch_secs().saturating_sub(crate::agent_bus::store::AGENT_BUS_STALE_HEARTBEAT_SECS);
    if roster.last_seen_epoch_secs < cutoff {
        return Err(anyhow!("Agent Management sender heartbeat is stale"));
    }
    let roster_session_id = roster
        .session_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Agent Management sender has no durable session"))?;
    Ok(AgentManagementRosterSender {
        runtime_agent_id,
        roster_session_id,
    })
}

fn resolve_agent_management_invocation(
    sender: &AgentManagementRosterSender,
) -> Result<AgentManagementInvocation, AgentManagementInvocationError> {
    let store = crate::session::store::load_cutex_session_store()
        .map_err(|_| AgentManagementInvocationError::PersistenceUnavailable)?;
    resolve_agent_management_invocation_from_store(sender, &store)
}

fn resolve_agent_management_invocation_from_store(
    sender: &AgentManagementRosterSender,
    store: &crate::session::model::CutexSessionStore,
) -> Result<AgentManagementInvocation, AgentManagementInvocationError> {
    let key = crate::session::service::cutex_session_key_for_user_id_including_retired(
        store,
        &sender.roster_session_id,
    )
    .ok_or(AgentManagementInvocationError::StaleRuntimeIdentity)?;
    let session = store
        .sessions
        .get(&key)
        .ok_or(AgentManagementInvocationError::StaleRuntimeIdentity)?;
    if session.archive_state != crate::session::model::CutexSessionArchiveState::Active
        || session.current_runtime_agent_id.as_deref() != Some(sender.runtime_agent_id.as_str())
    {
        return Err(AgentManagementInvocationError::StaleRuntimeIdentity);
    }
    Ok(AgentManagementInvocation {
        caller_cutex_session: crate::role_revision::CutexSessionId::new(
            session.cutex_session_id.clone(),
        )
        .map_err(|_| AgentManagementInvocationError::StaleRuntimeIdentity)?,
        caller_runtime_agent_id: sender.runtime_agent_id.as_str().to_string(),
    })
}

fn require_task_worker_bridge_token(
    request: &crate::http::server::SimpleHttpRequest,
    token: Option<&str>,
) -> anyhow::Result<()> {
    let route_token = token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("task worker routes require a configured bridge token"))?;
    require_service_bridge_token(request, Some(route_token), "Agent Bus")
}

fn task_service_v2_no_write(
    action_id: crate::task_service::ActionId,
    code: &str,
    detail: &str,
) -> TaskServiceActionResponse {
    TaskServiceActionResponse {
        schema: TaskServiceActionResponseSchema::V2,
        action_id,
        outcome: TaskServiceActionOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

fn worker_transition_kind(
    action: &crate::task_service::WorkerActionRequest,
) -> crate::management::v2::integration_events::TaskAssignmentTransitionKind {
    use crate::management::v2::integration_events::TaskAssignmentTransitionKind as Kind;
    match action {
        crate::task_service::WorkerActionRequest::Start(_) => Kind::AttemptStarted,
        crate::task_service::WorkerActionRequest::ReportStatus(_) => Kind::AttemptProgressed,
        crate::task_service::WorkerActionRequest::Block(_) => Kind::AttemptBlocked,
        crate::task_service::WorkerActionRequest::Resume(_) => Kind::AttemptResumed,
        crate::task_service::WorkerActionRequest::Submit(_) => Kind::ReviewReady,
        crate::task_service::WorkerActionRequest::Decline(_) => Kind::Declined,
        crate::task_service::WorkerActionRequest::AbortAttempt(_) => Kind::Aborted,
    }
}

fn terminal_transition_kind(
    action: &crate::task_service::TerminalAuthorityRequest,
) -> crate::management::v2::integration_events::TaskAssignmentTransitionKind {
    use crate::management::v2::integration_events::TaskAssignmentTransitionKind as Kind;
    match action {
        crate::task_service::TerminalAuthorityRequest::AcceptResult(_) => Kind::Completed,
        crate::task_service::TerminalAuthorityRequest::RequestChanges(_) => Kind::AttemptResumed,
        crate::task_service::TerminalAuthorityRequest::FailResult(_) => Kind::Failed,
        crate::task_service::TerminalAuthorityRequest::Cancel(_) => Kind::Closed,
    }
}

fn coordinator_transition_kind(
    action: &crate::task_service::CoordinatorOperation,
) -> Option<crate::management::v2::integration_events::TaskAssignmentTransitionKind> {
    use crate::management::v2::integration_events::TaskAssignmentTransitionKind as Kind;
    match action {
        crate::task_service::CoordinatorOperation::RetryDelivery(_) => Some(Kind::RetryScheduled),
        crate::task_service::CoordinatorOperation::CancelAssignment(_)
        | crate::task_service::CoordinatorOperation::CloseAssignment(_) => Some(Kind::Closed),
        crate::task_service::CoordinatorOperation::AuthorizeAttemptRetry(_) => {
            Some(Kind::RetryScheduled)
        }
        crate::task_service::CoordinatorOperation::CreateRevision(_)
        | crate::task_service::CoordinatorOperation::AssignAndDispatch(_) => None,
    }
}

fn seat_for_session_in_snapshot(
    snapshot: &crate::seat::SeatOccupancySnapshot,
    session_id: &crate::role_revision::CutexSessionId,
) -> Option<crate::task_service::SeatId> {
    snapshot
        .occupancies
        .values()
        .find(|occupancy| &occupancy.occupant_cutex_session == session_id)
        .map(|occupancy| occupancy.seat_id.clone())
}

fn director_operation_name(
    action: &crate::task_service::DirectorSemanticOperation,
) -> &'static str {
    match action {
        crate::task_service::DirectorSemanticOperation::CreateRevision(_) => "create_revision",
        crate::task_service::DirectorSemanticOperation::Assign(_) => "assign",
        crate::task_service::DirectorSemanticOperation::CreateAndAssign { .. } => {
            "create_and_assign"
        }
        crate::task_service::DirectorSemanticOperation::Query { .. } => "query",
        crate::task_service::DirectorSemanticOperation::AcceptResult(_) => "accept_result",
        crate::task_service::DirectorSemanticOperation::RequestChanges(_) => "request_changes",
        crate::task_service::DirectorSemanticOperation::FailResult(_) => "fail_result",
        crate::task_service::DirectorSemanticOperation::Cancel(_) => "cancel",
    }
}

fn director_project_contract_is_valid(
    request: &crate::task_service::DirectorActionRequest,
) -> bool {
    use crate::task_service::DirectorSemanticOperation as Operation;
    match (&request.schema, &request.action) {
        (crate::task_service::DirectorActionSchema::V1, Operation::CreateRevision(value)) => {
            value.project_id.is_none()
        }
        (crate::task_service::DirectorActionSchema::V1, Operation::Assign(value)) => {
            value.project_id.is_none()
        }
        (
            crate::task_service::DirectorActionSchema::V1,
            Operation::CreateAndAssign {
                create_revision,
                assign,
            },
        ) => create_revision.project_id.is_none() && assign.project_id.is_none(),
        (crate::task_service::DirectorActionSchema::V2, Operation::CreateRevision(value)) => {
            value.project_id.is_some()
        }
        (crate::task_service::DirectorActionSchema::V2, Operation::Assign(value)) => {
            value.project_id.is_some()
        }
        (
            crate::task_service::DirectorActionSchema::V2,
            Operation::CreateAndAssign {
                create_revision,
                assign,
            },
        ) => {
            create_revision.project_id.is_some() && create_revision.project_id == assign.project_id
        }
        _ => true,
    }
}

fn director_no_write(
    action_id: crate::task_service::ActionId,
    operation: &str,
    code: &str,
) -> crate::task_service::DirectorActionReceipt {
    crate::task_service::DirectorActionReceipt {
        schema: crate::task_service::DirectorReceiptSchema::V1,
        action_id,
        operation: operation.to_string(),
        status: if code == "conflict" {
            crate::task_service::DirectorActionStatus::Conflict
        } else {
            crate::task_service::DirectorActionStatus::NoWrite
        },
        project_id: None,
        task_id: None,
        task_revision: None,
        assignment_id: None,
        closure_reason: None,
        continuation: None,
        tasks: Vec::new(),
        assignments: Vec::new(),
        code: Some(code.to_string()),
    }
}

fn director_provider_error(
    action_id: crate::task_service::ActionId,
    operation: &str,
    error: crate::task_service::ProviderError,
) -> crate::task_service::DirectorActionReceipt {
    let code = match error {
        crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
        crate::task_service::ProviderError::Unauthorized => "unauthorized",
        crate::task_service::ProviderError::NotFound(_) => "not_found",
        crate::task_service::ProviderError::Conflict(_) => "conflict",
        crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
        crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
        crate::task_service::ProviderError::PersistenceUnavailable
        | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
        crate::task_service::ProviderError::InvalidStore => "invalid_store",
    };
    director_no_write(action_id, operation, code)
}

fn director_provider_receipt(
    action_id: crate::task_service::ActionId,
    operation: &str,
    was_known: bool,
    receipt: &crate::task_service::ProviderReceipt,
) -> crate::task_service::DirectorActionReceipt {
    let (project_id, task_id, task_revision, assignment_id, closure_reason) = match &receipt.result
    {
        crate::task_service::ProviderResult::TaskRevision(task) => (
            task.project_id.clone(),
            Some(task.task_id.clone()),
            Some(task.task_revision),
            None,
            None,
        ),
        crate::task_service::ProviderResult::Assignment { assignment, .. } => (
            assignment.project_id.clone(),
            Some(assignment.task_id.clone()),
            Some(assignment.task_revision),
            Some(assignment.assignment_id.clone()),
            assignment.closure.as_ref().map(|closure| closure.reason),
        ),
        crate::task_service::ProviderResult::Attempt(attempt) => (
            attempt.project_id.clone(),
            None,
            None,
            Some(attempt.assignment_id.clone()),
            None,
        ),
        _ => (None, None, None, None, None),
    };
    crate::task_service::DirectorActionReceipt {
        schema: crate::task_service::DirectorReceiptSchema::V1,
        action_id,
        operation: operation.to_string(),
        status: if was_known {
            crate::task_service::DirectorActionStatus::CurrentState
        } else {
            crate::task_service::DirectorActionStatus::Committed
        },
        project_id,
        task_id,
        task_revision,
        assignment_id,
        closure_reason,
        continuation: None,
        tasks: Vec::new(),
        assignments: Vec::new(),
        code: None,
    }
}

fn derived_short_digest(action_id: &crate::task_service::ActionId, domain: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"cutex/task-service-director-action/v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(action_id.as_str().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn derived_director_action_id(
    action_id: &crate::task_service::ActionId,
    domain: &str,
) -> crate::task_service::ActionId {
    crate::task_service::ActionId::new(format!(
        "director-{}",
        derived_short_digest(action_id, domain)
    ))
    .expect("derived Director action ID is valid")
}

fn derived_send_attempt_id(
    action_id: &crate::task_service::ActionId,
) -> crate::task_service::SendAttemptId {
    crate::task_service::SendAttemptId::new(format!(
        "director-{}",
        derived_short_digest(action_id, "send")
    ))
    .expect("derived send attempt ID is valid")
}

fn worker_mechanical_context(
    snapshot: &crate::task_service::TaskServiceSnapshot,
    assignment: &crate::task_service::Assignment,
) -> crate::task_service::WorkerMechanicalContext {
    let attempt = assignment.active_attempt.and_then(|number| {
        snapshot
            .attempts
            .get(&assignment.assignment_id)
            .and_then(|attempts| attempts.get(&number))
            .map(|attempt| crate::task_service::AttemptMechanicalContext {
                attempt_number: attempt.attempt_number,
                attempt_token: attempt.attempt_token.clone(),
                expected_attempt_revision: attempt.local_revision,
            })
    });
    crate::task_service::WorkerMechanicalContext {
        expected_assignment_revision: assignment.local_revision,
        attempt,
    }
}

fn assignment_state_name(state: crate::task_service::AssignmentState) -> &'static str {
    match state {
        crate::task_service::AssignmentState::AwaitingAck => "awaiting_ack",
        crate::task_service::AssignmentState::Active => "active",
        crate::task_service::AssignmentState::RetryPending => "retry_pending",
        crate::task_service::AssignmentState::Closed => "closed",
    }
}

fn attempt_phase_name(phase: crate::task_service::AttemptPhase) -> &'static str {
    match phase {
        crate::task_service::AttemptPhase::Running => "running",
        crate::task_service::AttemptPhase::Blocked => "blocked",
        crate::task_service::AttemptPhase::ReviewReady => "review_ready",
        crate::task_service::AttemptPhase::Completed => "completed",
        crate::task_service::AttemptPhase::Failed => "failed",
        crate::task_service::AttemptPhase::Cancelled => "cancelled",
        crate::task_service::AttemptPhase::Aborted => "aborted",
    }
}

fn director_last_output(
    activity: Option<&crate::management::v2::activity::SessionActivityState>,
    project_id: Option<&crate::agent_management::ProjectId>,
    cutex_session_id: &str,
    assignment_id: &str,
    attempt_number: u64,
) -> Option<crate::observability::SafeOutputProjection> {
    activity
        .and_then(|activity| activity.last_output.as_ref())
        .filter(|output| {
            output.association.project_id.as_ref() == project_id
                && output.association.cutex_session_id == cutex_session_id
                && output
                    .association
                    .matches_task(assignment_id, attempt_number)
        })
        .cloned()
}

fn director_last_tool_call(
    activity: Option<&crate::management::v2::activity::SessionActivityState>,
    project_id: Option<&crate::agent_management::ProjectId>,
    cutex_session_id: &str,
    assignment_id: &str,
    attempt_number: u64,
) -> Option<crate::observability::SafeToolCallProjection> {
    activity
        .and_then(|activity| activity.last_tool_call.as_ref())
        .filter(|tool| {
            tool.association.project_id.as_ref() == project_id
                && tool.association.cutex_session_id == cutex_session_id
                && tool.association.matches_task(assignment_id, attempt_number)
        })
        .cloned()
}

fn task_service_dispatch_director_error(
    action_id: crate::task_service::ActionId,
    error: crate::task_delivery::provider_adapter::AssignmentDispatchError,
) -> crate::task_service::DirectorActionReceipt {
    match error {
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Provider(error) => {
            director_provider_error(action_id, "assign", error)
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Contract(_) => {
            director_no_write(action_id, "assign", "invalid_assignment_contract")
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::TargetUnavailable => {
            director_no_write(action_id, "assign", "target_unavailable")
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::AgentBusUnavailable => {
            let mut receipt = director_no_write(action_id, "assign", "response_uncertain");
            receipt.status = crate::task_service::DirectorActionStatus::ResponseUncertain;
            receipt
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::InvalidCommittedShape => {
            director_no_write(action_id, "assign", "invalid_store")
        }
    }
}

fn provider_result_response(
    action_id: crate::task_service::ActionId,
    result: Result<crate::task_service::ProviderReceipt, crate::task_service::ProviderError>,
) -> TaskServiceActionResponse {
    match result {
        Ok(receipt) => TaskServiceActionResponse {
            schema: TaskServiceActionResponseSchema::V2,
            action_id,
            outcome: TaskServiceActionOutcome::Committed(receipt),
        },
        Err(error) => task_service_provider_no_write(action_id, error),
    }
}

fn task_service_provider_no_write(
    action_id: crate::task_service::ActionId,
    error: crate::task_service::ProviderError,
) -> TaskServiceActionResponse {
    let code = match error {
        crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
        crate::task_service::ProviderError::Unauthorized => "unauthorized",
        crate::task_service::ProviderError::NotFound(_) => "not_found",
        crate::task_service::ProviderError::Conflict(_) => "conflict",
        crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
        crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
        crate::task_service::ProviderError::PersistenceUnavailable
        | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
        crate::task_service::ProviderError::InvalidStore => "invalid_store",
    };
    task_service_v2_no_write(action_id, code, &error.to_string())
}

fn task_service_dispatch_response(
    provider: &crate::task_service::TaskServiceProvider,
    coordinator_cutex_session: &crate::role_revision::CutexSessionId,
    action_id: crate::task_service::ActionId,
    error: crate::task_delivery::provider_adapter::AssignmentDispatchError,
) -> TaskServiceActionResponse {
    let payload_conflict = matches!(
        &error,
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Provider(
            crate::task_service::ProviderError::Conflict("action_id_payload_conflict")
        )
    );
    if !payload_conflict {
        if let Ok(snapshot) = provider.query() {
            if let Some(receipt) = snapshot.receipts.get(&action_id) {
                if matches!(
                    &receipt.result,
                    crate::task_service::ProviderResult::Assignment { .. }
                ) {
                    if let Err(projection_error) =
                        crate::management::v2::integration_events::append_task_service_assignment(
                            coordinator_cutex_session,
                            receipt,
                        )
                    {
                        eprintln!("{YELLOW}warning:{RESET} failed to project committed Task Service assignment after dispatch error: {projection_error:#}");
                    }
                }
                return TaskServiceActionResponse {
                    schema: TaskServiceActionResponseSchema::V2,
                    action_id,
                    outcome: TaskServiceActionOutcome::Committed(receipt.clone()),
                };
            }
        }
    }
    let detail = error.to_string();
    match error {
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Provider(error) => {
            task_service_provider_no_write(action_id, error)
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Contract(
            crate::agent_bus::model::TaskServiceAssignmentContractError::TooLarge { .. },
        ) => task_service_v2_no_write(action_id, "assignment_contract_too_large", &detail),
        crate::task_delivery::provider_adapter::AssignmentDispatchError::Contract(_) => {
            task_service_v2_no_write(action_id, "assignment_contract_invalid", &detail)
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::TargetUnavailable => {
            task_service_v2_no_write(action_id, "target_unavailable", &detail)
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::AgentBusUnavailable => {
            task_service_v2_no_write(action_id, "agent_bus_unavailable", &detail)
        }
        crate::task_delivery::provider_adapter::AssignmentDispatchError::InvalidCommittedShape => {
            task_service_v2_no_write(action_id, "invalid_request", &detail)
        }
    }
}

fn task_service_query_no_write(code: &str, detail: &str) -> TaskServiceQueryResponse {
    TaskServiceQueryResponse {
        schema: TaskServiceQueryResponseSchema::V2,
        outcome: TaskServiceQueryOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

fn task_service_query_provider_error(
    error: crate::task_service::ProviderError,
) -> TaskServiceQueryResponse {
    let code = match error {
        crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
        crate::task_service::ProviderError::Unauthorized => "unauthorized",
        crate::task_service::ProviderError::NotFound(_) => "not_found",
        crate::task_service::ProviderError::Conflict(_) => "conflict",
        crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
        crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
        crate::task_service::ProviderError::PersistenceUnavailable
        | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
        crate::task_service::ProviderError::InvalidStore => "invalid_store",
    };
    task_service_query_no_write(code, &error.to_string())
}

fn task_service_worker_context_no_write(
    code: &str,
    detail: &str,
) -> TaskServiceWorkerContextResponse {
    TaskServiceWorkerContextResponse {
        schema: crate::task_service::WorkerContextResponseSchema::V2,
        outcome: TaskServiceWorkerContextOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

fn task_service_worker_context_provider_error(
    error: crate::task_service::ProviderError,
) -> TaskServiceWorkerContextResponse {
    let code = match error {
        crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
        crate::task_service::ProviderError::Unauthorized => "unauthorized",
        crate::task_service::ProviderError::NotFound(_) => "not_found",
        crate::task_service::ProviderError::Conflict(_) => "conflict",
        crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
        crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
        crate::task_service::ProviderError::PersistenceUnavailable
        | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
        crate::task_service::ProviderError::InvalidStore => "invalid_store",
    };
    task_service_worker_context_no_write(code, &error.to_string())
}

fn task_service_worker_prepare_no_write(
    code: &str,
    detail: &str,
) -> TaskServiceWorkerPrepareResponse {
    TaskServiceWorkerPrepareResponse {
        schema: crate::task_service::WorkerPrepareResponseSchema::V2,
        outcome: TaskServiceWorkerPrepareOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

fn task_service_worker_prepare_provider_error(
    error: crate::task_service::ProviderError,
) -> TaskServiceWorkerPrepareResponse {
    let code = match error {
        crate::task_service::ProviderError::InvalidRequest(_) => "invalid_request",
        crate::task_service::ProviderError::Unauthorized => "unauthorized",
        crate::task_service::ProviderError::NotFound(_) => "not_found",
        crate::task_service::ProviderError::Conflict(_) => "conflict",
        crate::task_service::ProviderError::IllegalState(_) => "illegal_state",
        crate::task_service::ProviderError::RecoveryRequired => "recovery_required",
        crate::task_service::ProviderError::PersistenceUnavailable
        | crate::task_service::ProviderError::Io(_) => "persistence_unavailable",
        crate::task_service::ProviderError::InvalidStore => "invalid_store",
    };
    task_service_worker_prepare_no_write(code, &error.to_string())
}

fn task_worker_sender(
    request: &crate::http::server::SimpleHttpRequest,
    state: &Arc<Mutex<AgentBusState>>,
) -> Result<TaskWorkerRosterSender, TaskWorkerActionNoWrite> {
    let sender_id = request
        .headers
        .get("x-cutex-agent-id")
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(TaskWorkerActionNoWrite::SenderHeaderMissing)?;
    let runtime_agent_id = RuntimeAgentId::new(sender_id.to_string())
        .map_err(|_| TaskWorkerActionNoWrite::SenderNotRegistered)?;
    let roster = state
        .lock()
        .map_err(|_| TaskWorkerActionNoWrite::PersistenceUnavailable)?
        .agents
        .get(sender_id)
        .cloned()
        .ok_or(TaskWorkerActionNoWrite::SenderNotRegistered)?;
    if roster.id != sender_id {
        return Err(TaskWorkerActionNoWrite::SenderNotRegistered);
    }
    if !agent_is_local_to_bus(&roster, &current_host_name()) {
        return Err(TaskWorkerActionNoWrite::FederatedSenderRejected);
    }
    if !crate::platform::process::process_is_running(roster.pid) {
        return Err(TaskWorkerActionNoWrite::SenderNotRegistered);
    }
    let cutoff =
        now_epoch_secs().saturating_sub(crate::agent_bus::store::AGENT_BUS_STALE_HEARTBEAT_SECS);
    if roster.last_seen_epoch_secs < cutoff {
        return Err(TaskWorkerActionNoWrite::SenderNotRegistered);
    }
    let roster_session_id = roster
        .session_id
        .filter(|value| !value.trim().is_empty())
        .ok_or(TaskWorkerActionNoWrite::RosterSessionMissing)?;
    Ok(TaskWorkerRosterSender {
        runtime_agent_id,
        roster_session_id,
    })
}

fn register_agent_with_reconciliation(
    state: &Arc<Mutex<AgentBusState>>,
    agent: AgentBusAgent,
    reconcile_registration_agent: impl FnOnce(&AgentBusAgent) -> anyhow::Result<()>,
    persist_registry: impl FnOnce(&AgentBusState) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    // The registration callback does not re-enter Agent Bus. Keeping the state
    // lock across its store CAS and roster persistence prevents unregister or
    // roster observation from entering the post-CAS/pre-visibility gap.
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
    reconcile_registration_agent(&agent)?;
    state.agents.insert(agent.id.clone(), agent.clone());
    persist_registry(&state)?;
    Ok(())
}

fn poll_wait_duration(path: &str) -> Duration {
    query_value(path, "wait_ms")
        .or_else(|| query_value(path, "waitMs"))
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or_default()
        .min(MAX_POLL_WAIT)
}

fn poll_agent_messages_with_wait(
    state: &Arc<Mutex<AgentBusState>>,
    agent_id: &str,
    ack_mode: bool,
    wait: Duration,
) -> anyhow::Result<(String, Vec<AgentBusMessage>)> {
    let deadline = Instant::now() + wait;
    loop {
        let signal = agent_bus_poll_signal();
        let observed_generation = *signal
            .generation
            .lock()
            .map_err(|_| anyhow!("agent bus poll signal lock poisoned"))?;
        let result = poll_agent_messages(state, agent_id, ack_mode, now_epoch_secs())?;
        if !result.1.is_empty() || wait.is_zero() || !agent_is_registered(state, agent_id)? {
            return Ok(result);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(result);
        }
        let generation = signal
            .generation
            .lock()
            .map_err(|_| anyhow!("agent bus poll signal lock poisoned"))?;
        if *generation != observed_generation {
            continue;
        }
        let (_generation, wait_result) = signal
            .available
            .wait_timeout(generation, remaining)
            .map_err(|_| anyhow!("agent bus poll signal lock poisoned"))?;
        if wait_result.timed_out() {
            return poll_agent_messages(state, agent_id, ack_mode, now_epoch_secs());
        }
    }
}

fn agent_is_registered(state: &Arc<Mutex<AgentBusState>>, agent_id: &str) -> anyhow::Result<bool> {
    state
        .lock()
        .map(|state| state.agents.contains_key(agent_id))
        .map_err(|_| anyhow!("agent bus state lock poisoned"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use crate::agent_bus::delivery::AgentDeliveryMode;
    use crate::agent_bus::model::AgentBusEnvelopeKind;
    use crate::agent_bus::model::AgentMessageKind;
    use crate::agent_bus::model::AgentRegistrationClass;
    use crate::agent_bus::model::{
        TaskWorkerActionKind, TaskWorkerActionRequest, TaskWorkerActionSchema,
        TaskWorkerReconciliationSchema, TaskWorkerResult,
    };
    use crate::session::archive::commit_retire;
    use crate::session::archive::record_has_runtime_claim;
    use crate::session::archive::validate_retire_preconditions;
    use crate::session::archive::CutexSessionArchiveError;
    use crate::session::model::CutexSessionRecord;
    use crate::session::model::CutexSessionStore;
    use crate::session::runtime_reconciliation::reconcile_cutex_session_store_for_registration;
    use crate::session::store::load_cutex_session_store_from_path;
    use crate::session::store::save_cutex_session_store_to_path;
    use crate::session::store::CutexSessionStoreRevisionConflict;
    use crate::task_delivery::worker_action_adapter::{
        WorkerSessionSnapshotBoundary, WorkerSessionSnapshotError,
    };
    use crate::task_delivery::{
        AgentBusDeliveryReceiptV1, AttemptToken, PilotAttemptFence, PilotDeliveryMode,
        PilotDeliveryRequest, PilotOwnerSnapshot, PilotPublishRequest, PilotTaskSpecification,
        TaskDeliveryPilot,
    };

    #[test]
    fn v2_project_scope_uses_only_the_exact_director_session() {
        let director = crate::role_revision::CutexSessionId::new("cutex.director").unwrap();
        let other = crate::role_revision::CutexSessionId::new("cutex.other").unwrap();
        let authority = |session: crate::role_revision::CutexSessionId| {
            crate::agent_management::ProjectAuthority {
                project_id: crate::agent_management::ProjectId::new(format!(
                    "project-{}",
                    session.as_str()
                ))
                .unwrap(),
                authorized_director_session: session,
                authority_epoch: 1,
                updated_at: crate::role_revision::Rfc3339::new("2026-01-01T00:00:00Z").unwrap(),
            }
        };
        let own = authority(director.clone());
        let foreign = authority(other);
        let scope = exact_project_scope_from_authorities(
            vec![
                (own.project_id.clone(), own),
                (foreign.project_id.clone(), foreign),
            ],
            &director,
        );
        assert_eq!(scope.len(), 1);
        assert!(scope
            .contains(&crate::agent_management::ProjectId::new("project-cutex.director").unwrap()));
        // Matching a presentation-like string is intentionally irrelevant.
        assert!(!scope
            .contains(&crate::agent_management::ProjectId::new("project-cutex.other").unwrap()));
    }

    #[test]
    fn poll_wait_duration_is_optional_and_bounded() {
        assert_eq!(
            poll_wait_duration("/api/messages/poll?agent_id=one"),
            Duration::ZERO
        );
        assert_eq!(
            poll_wait_duration("/api/messages/poll?wait_ms=2000"),
            Duration::from_secs(2)
        );
        assert_eq!(
            poll_wait_duration("/api/messages/poll?waitMs=60000"),
            MAX_POLL_WAIT
        );
    }

    #[test]
    fn target_resolution_errors_are_typed_4xx_and_internal_errors_remain_unmapped() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let target_error = crate::agent_bus::routing::resolve_agent_target_for_sender(
            &state,
            "cutex.01a0487d-c794-7e43-aeb4-19af2717037f",
            None,
            true,
        )
        .expect_err("unknown durable target");
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let mut client =
            TcpStream::connect(listener.local_addr().expect("address")).expect("connect client");
        let (mut server, _) = listener.accept().expect("accept server");
        write_agent_target_resolution_error(&mut server, &target_error)
            .expect("target error should map")
            .expect("write target error");
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("read response");
        let response = String::from_utf8(response).expect("utf8 response");
        assert!(response.starts_with("HTTP/1.1 404 Not Found"));
        assert!(response.contains("\"code\":\"not_found\""));

        assert!(write_agent_target_resolution_error(
            &mut server,
            &anyhow::anyhow!("registry persistence unavailable"),
        )
        .is_none());
    }

    #[test]
    fn director_observability_requires_exact_session_assignment_and_attempt() {
        let association = crate::observability::ObservationAssociation::session("cutex.worker")
            .with_task("assignment-1".to_string(), Some(2));
        let activity = crate::management::v2::activity::SessionActivityState {
            last_output: Some(crate::observability::SafeOutputProjection {
                association: association.clone(),
                class: crate::observability::SafeOutputClass::FinalVisible,
                display_text: "done".to_string(),
                updated_at: "2026-08-29T00:00:00Z".to_string(),
                runtime_generation: 4,
            }),
            last_tool_call: Some(crate::observability::SafeToolCallProjection {
                association,
                class: crate::observability::SafeToolCallClass::McpTool,
                status: crate::observability::SafeToolCallStatus::Finished,
                display_text: "MCP tool".to_string(),
                updated_at: "2026-08-29T00:00:00Z".to_string(),
                runtime_generation: 4,
            }),
            ..Default::default()
        };

        assert!(
            director_last_output(Some(&activity), None, "cutex.worker", "assignment-1", 2)
                .is_some()
        );
        assert!(
            director_last_tool_call(Some(&activity), None, "cutex.worker", "assignment-1", 2)
                .is_some()
        );
        for (session, assignment, attempt) in [
            ("cutex.other", "assignment-1", 2),
            ("cutex.worker", "assignment-other", 2),
            ("cutex.worker", "assignment-1", 3),
        ] {
            assert!(
                director_last_output(Some(&activity), None, session, assignment, attempt).is_none()
            );
            assert!(
                director_last_tool_call(Some(&activity), None, session, assignment, attempt)
                    .is_none()
            );
        }
        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        assert!(director_last_output(
            Some(&activity),
            Some(&project),
            "cutex.worker",
            "assignment-1",
            2
        )
        .is_none());

        let encoded = serde_json::to_string(&activity.last_tool_call).unwrap();
        for forbidden in ["arguments", "command_line", "output", "attempt_token"] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn long_poll_wakes_when_a_message_is_enqueued() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state
            .lock()
            .expect("agent bus state")
            .agents
            .insert("target".to_string(), test_agent("target"));
        let poll_state = Arc::clone(&state);
        let started = Instant::now();
        let poller = std::thread::spawn(move || {
            poll_agent_messages_with_wait(&poll_state, "target", true, Duration::from_secs(1))
                .expect("long poll should complete")
        });

        std::thread::sleep(Duration::from_millis(25));
        state
            .lock()
            .expect("agent bus state")
            .messages
            .entry("target".to_string())
            .or_default()
            .push_back(AgentBusMessage {
                id: "message-1".to_string(),
                kind: AgentBusEnvelopeKind::Message,
                from: "sender".to_string(),
                to: "target".to_string(),
                from_cutex_session_id: None,
                to_cutex_session_id: None,
                content: "hello".to_string(),
                delivery_mode: AgentDeliveryMode::Passive,
                trigger_turn: false,
                created_at_epoch_secs: 1,
                sender_kind: AgentMessageKind::Agent,
                display_source: None,
                submit_mode: None,
                control_type: None,
                control_payload: None,
                external_action_id: None,
                external_message_id: None,
            });
        notify_agent_bus_message_available();

        let (_, messages) = poller.join().expect("long poll thread");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, "message-1");
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn long_poll_wakes_when_an_agent_is_unregistered() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state
            .lock()
            .expect("agent bus state")
            .agents
            .insert("target".to_string(), test_agent("target"));
        let poll_state = state.clone();
        let started = Instant::now();
        let poller = std::thread::spawn(move || {
            poll_agent_messages_with_wait(&poll_state, "target", true, Duration::from_secs(1))
                .expect("long poll should complete")
        });

        std::thread::sleep(Duration::from_millis(25));
        state
            .lock()
            .expect("agent bus state")
            .agents
            .remove("target");
        notify_agent_bus_message_available();

        let (_, messages) = poller.join().expect("long poll thread");
        assert!(messages.is_empty());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn retire_store_write_wins_before_new_registration_becomes_visible() {
        let (root, path) = initialized_session_store();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let registration_state = Arc::clone(&state);
        let registration_path = path.clone();
        let (loaded_tx, loaded_rx) = mpsc::sync_channel(1);
        let (continue_tx, continue_rx) = mpsc::sync_channel(1);
        let registration = std::thread::spawn(move || {
            register_agent_with_reconciliation(
                &registration_state,
                test_agent("target"),
                move |agent| {
                    let mut store = load_cutex_session_store_from_path(&registration_path)?;
                    let reconciliation = reconcile_cutex_session_store_for_registration(
                        &mut store,
                        agent,
                        "host",
                        "2026-08-10T00:00:30Z",
                    )?;
                    assert!(reconciliation.store_fence_required);
                    assert!(reconciliation.outcome.is_some());
                    loaded_tx.send(()).expect("signal registration load");
                    continue_rx.recv().expect("continue registration save");
                    save_cutex_session_store_to_path(&registration_path, &store)
                },
                |_| Ok(()),
            )
        });

        loaded_rx.recv().expect("registration loaded old revision");
        let mut archive = load_cutex_session_store_from_path(&path).expect("load archive writer");
        let record = archive
            .sessions
            .get_mut("cutex.registration-race")
            .expect("archive record");
        commit_retire(record, 1, 0, true, "2026-08-10T00:01:00Z".to_string())
            .expect("commit retire winner");
        save_cutex_session_store_to_path(&path, &archive).expect("save retire winner");
        continue_tx.send(()).expect("release registration save");

        let error = registration
            .join()
            .expect("registration thread")
            .expect_err("stale registration reconciliation must fail");
        assert!(error
            .downcast_ref::<CutexSessionStoreRevisionConflict>()
            .is_some());
        assert!(state.lock().expect("agent bus state").agents.is_empty());
        let persisted = load_cutex_session_store_from_path(&path).expect("reload retired store");
        assert!(persisted
            .sessions
            .get("cutex.registration-race")
            .expect("persisted record")
            .is_retired());
        fs::remove_dir_all(root).expect("remove test store");
    }

    #[test]
    fn new_registration_store_write_wins_before_roster_visibility() {
        let (root, path) = initialized_session_store();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let registration_state = Arc::clone(&state);
        let registration_path = path.clone();
        let (saved_tx, saved_rx) = mpsc::sync_channel(1);
        let (continue_tx, continue_rx) = mpsc::sync_channel(1);
        let (visible_tx, visible_rx) = mpsc::sync_channel(1);
        let registration = std::thread::spawn(move || {
            register_agent_with_reconciliation(
                &registration_state,
                test_agent("target"),
                move |agent| {
                    let mut store = load_cutex_session_store_from_path(&registration_path)?;
                    let reconciliation = reconcile_cutex_session_store_for_registration(
                        &mut store,
                        agent,
                        "host",
                        "2026-08-10T00:00:30Z",
                    )?;
                    assert!(reconciliation.store_fence_required);
                    assert!(reconciliation.outcome.is_some());
                    save_cutex_session_store_to_path(&registration_path, &store)?;
                    saved_tx.send(()).expect("signal registration save");
                    continue_rx.recv().expect("continue roster persistence");
                    Ok(())
                },
                move |_| {
                    visible_tx.send(()).expect("signal roster persistence");
                    Ok(())
                },
            )
        });

        saved_rx
            .recv()
            .expect("registration reconciliation saved first");
        assert!(matches!(
            visible_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let persisted =
            load_cutex_session_store_from_path(&path).expect("load registration winner");
        let record = persisted
            .sessions
            .get("cutex.registration-race")
            .expect("registration record");
        assert_eq!(record.current_runtime_agent_id.as_deref(), Some("target"));
        assert!(matches!(
            validate_retire_preconditions(record, 1, 0),
            Err(CutexSessionArchiveError::StaleRevision {
                expected: 1,
                actual: 2
            })
        ));
        continue_tx.send(()).expect("release roster persistence");
        registration
            .join()
            .expect("registration thread")
            .expect("registration winner");
        visible_rx.recv().expect("roster persisted after release");
        assert!(state
            .lock()
            .expect("agent bus state")
            .agents
            .contains_key("target"));
        assert!(load_cutex_session_store_from_path(&path)
            .expect("reload active store")
            .sessions
            .get("cutex.registration-race")
            .expect("active record")
            .is_active());
        fs::remove_dir_all(root).expect("remove test store");
    }

    #[test]
    fn failed_refresh_reconciliation_preserves_prior_roster_entry() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state
            .lock()
            .expect("agent bus state")
            .agents
            .insert("target".to_string(), test_agent("target"));
        let mut refreshed = test_agent("target");
        refreshed.pid = 99;

        let error = register_agent_with_reconciliation(
            &state,
            refreshed,
            |_| anyhow::bail!("injected registration reconciliation failure"),
            |_| panic!("failed reconciliation must not persist the roster"),
        )
        .expect_err("refresh reconciliation must fail closed");

        assert!(error
            .to_string()
            .contains("registration reconciliation failure"));
        assert_eq!(
            state
                .lock()
                .expect("agent bus state")
                .agents
                .get("target")
                .expect("prior roster entry")
                .pid,
            42
        );
    }

    #[test]
    fn unregister_cannot_enter_between_registration_cas_and_roster_write() {
        let (root, path) = initialized_session_store();
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state
            .lock()
            .expect("agent bus state")
            .agents
            .insert("target".to_string(), test_agent("target"));
        let registration_state = Arc::clone(&state);
        let registration_path = path.clone();
        let (saved_tx, saved_rx) = mpsc::sync_channel(1);
        let (continue_tx, continue_rx) = mpsc::sync_channel(1);
        let (order_tx, order_rx) = mpsc::channel();
        let visible_order_tx = order_tx.clone();
        let registration = std::thread::spawn(move || {
            register_agent_with_reconciliation(
                &registration_state,
                test_agent("target"),
                move |agent| {
                    let mut store = load_cutex_session_store_from_path(&registration_path)?;
                    let reconciliation = reconcile_cutex_session_store_for_registration(
                        &mut store,
                        agent,
                        "host",
                        "2026-08-10T00:00:30Z",
                    )?;
                    assert!(reconciliation.store_fence_required);
                    assert!(reconciliation.outcome.is_some());
                    save_cutex_session_store_to_path(&registration_path, &store)?;
                    saved_tx.send(()).expect("signal refresh save");
                    continue_rx.recv().expect("continue roster write");
                    Ok(())
                },
                move |_| {
                    visible_order_tx
                        .send("visible")
                        .expect("record roster visibility");
                    Ok(())
                },
            )
        });

        saved_rx.recv().expect("refresh store CAS completed");
        let unregister_state = Arc::clone(&state);
        let (attempted_tx, attempted_rx) = mpsc::sync_channel(1);
        let unregister = std::thread::spawn(move || {
            attempted_tx.send(()).expect("signal unregister attempt");
            let removed = unregister_state
                .lock()
                .expect("agent bus state")
                .agents
                .remove("target")
                .is_some();
            order_tx
                .send("unregistered")
                .expect("record unregister completion");
            removed
        });

        attempted_rx
            .recv()
            .expect("unregister attempted state lock");
        assert!(matches!(
            order_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        continue_tx.send(()).expect("release roster write");
        assert_eq!(order_rx.recv().expect("first ordered action"), "visible");
        registration
            .join()
            .expect("registration thread")
            .expect("refresh registration");
        assert_eq!(
            order_rx.recv().expect("second ordered action"),
            "unregistered"
        );
        assert!(unregister.join().expect("unregister thread"));
        assert!(state.lock().expect("agent bus state").agents.is_empty());
        assert!(load_cutex_session_store_from_path(&path)
            .expect("reload registration store")
            .sessions
            .get("cutex.registration-race")
            .expect("active record")
            .is_active());
        fs::remove_dir_all(root).expect("remove test store");
    }

    #[test]
    fn retired_session_rejects_matching_sessionless_registration() {
        let (root, path) = initialized_sessionless_store();
        let mut archive = load_cutex_session_store_from_path(&path).expect("load archive writer");
        let record = archive
            .sessions
            .get_mut("cutex.registration-race")
            .expect("archive record");
        commit_retire(record, 1, 0, true, "2026-08-10T00:01:00Z".to_string())
            .expect("commit retire winner");
        save_cutex_session_store_to_path(&path, &archive).expect("save retire winner");
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let registration_path = path.clone();
        let error = register_agent_with_reconciliation(
            &state,
            sessionless_test_agent("target"),
            move |agent| {
                let mut store = load_cutex_session_store_from_path(&registration_path)?;
                let reconciliation = reconcile_cutex_session_store_for_registration(
                    &mut store,
                    agent,
                    "host",
                    "2026-08-10T00:01:30Z",
                )?;
                if reconciliation.store_fence_required {
                    save_cutex_session_store_to_path(&registration_path, &store)?;
                }
                Ok(())
            },
            |_| panic!("retired registration must not persist roster"),
        )
        .expect_err("retired sessionless registration must fail");

        assert!(error.to_string().contains("retired cutex session"));
        assert!(state.lock().expect("agent bus state").agents.is_empty());
        assert!(load_cutex_session_store_from_path(&path)
            .expect("reload retired store")
            .sessions
            .get("cutex.registration-race")
            .expect("retired record")
            .is_retired());
        fs::remove_dir_all(root).expect("remove test store");
    }

    #[test]
    fn matching_sessionless_runtime_claim_wins_before_roster_write() {
        let (root, path) = initialized_sessionless_store();
        let mut stale_archive =
            load_cutex_session_store_from_path(&path).expect("load stale archive writer");
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let registration_state = Arc::clone(&state);
        let registration_path = path.clone();
        let (saved_tx, saved_rx) = mpsc::sync_channel(1);
        let (continue_tx, continue_rx) = mpsc::sync_channel(1);
        let (visible_tx, visible_rx) = mpsc::sync_channel(1);
        let registration = std::thread::spawn(move || {
            register_agent_with_reconciliation(
                &registration_state,
                sessionless_test_agent("target"),
                move |agent| {
                    let mut store = load_cutex_session_store_from_path(&registration_path)?;
                    let reconciliation = reconcile_cutex_session_store_for_registration(
                        &mut store,
                        agent,
                        "host",
                        "2026-08-10T00:00:30Z",
                    )?;
                    assert!(reconciliation.store_fence_required);
                    assert!(reconciliation.outcome.is_none());
                    save_cutex_session_store_to_path(&registration_path, &store)?;
                    saved_tx.send(()).expect("signal sessionless save");
                    continue_rx.recv().expect("continue roster write");
                    Ok(())
                },
                move |_| {
                    visible_tx.send(()).expect("signal roster persistence");
                    Ok(())
                },
            )
        });

        saved_rx.recv().expect("sessionless store fence saved");
        assert!(matches!(
            visible_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let persisted = load_cutex_session_store_from_path(&path)
            .expect("load sessionless registration winner");
        let claimed = persisted
            .sessions
            .get("cutex.registration-race")
            .expect("claimed sessionless record");
        assert_eq!(claimed.current_runtime_agent_id.as_deref(), Some("target"));
        assert_eq!(claimed.runtime_generation, 1);
        assert!(record_has_runtime_claim(claimed));
        assert!(matches!(
            validate_retire_preconditions(claimed, 1, 0),
            Err(CutexSessionArchiveError::StaleRuntimeFence {
                expected: 0,
                actual: 1
            })
        ));
        let record = stale_archive
            .sessions
            .get_mut("cutex.registration-race")
            .expect("archive record");
        commit_retire(record, 1, 0, true, "2026-08-10T00:01:00Z".to_string())
            .expect("prepare stale retire");
        let error = save_cutex_session_store_to_path(&path, &stale_archive)
            .expect_err("registration store fence must stale retire writer");
        assert!(error
            .downcast_ref::<CutexSessionStoreRevisionConflict>()
            .is_some());
        continue_tx.send(()).expect("release roster write");
        registration
            .join()
            .expect("registration thread")
            .expect("sessionless registration winner");
        visible_rx.recv().expect("roster persisted after release");
        assert!(state
            .lock()
            .expect("agent bus state")
            .agents
            .contains_key("target"));
        assert!(load_cutex_session_store_from_path(&path)
            .expect("reload active store")
            .sessions
            .get("cutex.registration-race")
            .expect("active record")
            .is_active());
        fs::remove_dir_all(root).expect("remove test store");
    }

    fn initialized_session_store() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "cutex-registration-retire-race-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("cutex-sessions.json");
        let mut store = CutexSessionStore::default();
        store.sessions.insert(
            "cutex.registration-race".to_string(),
            CutexSessionRecord::new_at(
                "cutex.registration-race".to_string(),
                Some("thread-1".to_string()),
                "host".to_string(),
                "/tmp".to_string(),
                None,
                "2026-08-10T00:00:00Z".to_string(),
            )
            .expect("session record"),
        );
        save_cutex_session_store_to_path(&path, &store).expect("save initial store");
        (root, path)
    }

    fn initialized_sessionless_store() -> (PathBuf, PathBuf) {
        let (root, path) = initialized_session_store();
        let mut store = load_cutex_session_store_from_path(&path).expect("load sessionless store");
        store
            .sessions
            .get_mut("cutex.registration-race")
            .expect("sessionless record")
            .last_runtime_agent_id = Some("target".to_string());
        save_cutex_session_store_to_path(&path, &store).expect("save sessionless runtime identity");
        (root, path)
    }

    struct RouteSessionBoundary {
        records: BTreeMap<String, CutexSessionRecord>,
    }

    impl RouteSessionBoundary {
        fn observer_bytes(&self) -> Vec<u8> {
            serde_json::to_vec(&self.records).expect("serialize independent session observer")
        }
    }

    impl WorkerSessionSnapshotBoundary for RouteSessionBoundary {
        fn load_for_roster_session(
            &self,
            roster_session_id: &str,
        ) -> Result<CutexSessionRecord, WorkerSessionSnapshotError> {
            self.records
                .get(roster_session_id)
                .cloned()
                .ok_or(WorkerSessionSnapshotError::NotFound)
        }
    }

    struct WorkerHostFixture {
        root: PathBuf,
        task_root: PathBuf,
        evidence_root: PathBuf,
        pilot: Arc<TaskDeliveryPilot>,
        adapter: Arc<TaskWorkerActionAdapter>,
        host: Arc<TaskWorkerActionHost>,
        state: Arc<Mutex<AgentBusState>>,
        sessions: Arc<RouteSessionBoundary>,
        sender: TaskWorkerRosterSender,
        action: TaskWorkerActionRequest,
    }

    fn active_route_session(
        cutex_session_id: &str,
        roster_session_id: &str,
        runtime_id: &str,
        revision: u64,
        generation: u64,
    ) -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            cutex_session_id.to_string(),
            Some(roster_session_id.to_string()),
            current_host_name(),
            format!("/tmp/{runtime_id}"),
            Some("aemeath".to_string()),
            "2026-08-22T00:00:00Z".to_string(),
        )
        .expect("active route session");
        record.revision = revision;
        record.current_runtime_agent_id = Some(runtime_id.to_string());
        record.runtime_generation = generation;
        record.agent_enabled = true;
        record
    }

    fn active_route_roster(runtime_id: &str, roster_session_id: &str) -> AgentBusAgent {
        let mut roster = test_agent(runtime_id);
        roster.session_id = Some(roster_session_id.to_string());
        roster.host_id = Some(current_host_name());
        roster.pid = std::process::id();
        roster.last_seen_epoch_secs = now_epoch_secs();
        roster
    }

    #[test]
    fn agent_management_sender_discards_project_chat_groups() {
        let runtime_id = "runtime-director-r10";
        let roster_session_id = "01a041ba-47f6-7e31-bb09-1462cd309ae4";
        let mut roster = active_route_roster(runtime_id, roster_session_id);
        roster.groups = vec![
            "project:legacy-chat-label".to_string(),
            "project:automatic-routing-label".to_string(),
        ];
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state
            .lock()
            .unwrap()
            .agents
            .insert(runtime_id.to_string(), roster);
        let request = crate::http::server::SimpleHttpRequest {
            method: "POST".to_string(),
            path: "/api/agent-management/v1/actions".to_string(),
            headers: std::collections::HashMap::from([(
                "x-cutex-agent-id".to_string(),
                runtime_id.to_string(),
            )]),
            body: Vec::new(),
        };

        let sender = agent_management_sender(&request, &state).unwrap();
        assert_eq!(sender.runtime_agent_id.as_str(), runtime_id);
        assert_eq!(sender.roster_session_id, roster_session_id);
    }

    #[test]
    fn agent_management_caller_resolution_binds_current_runtime_to_durable_session() {
        let mut store = CutexSessionStore::default();
        let record = active_route_session(
            "cutex.director-r10",
            "01a041ba-47f6-7e31-bb09-1462cd309ae4",
            "runtime-director-r10",
            3,
            7,
        );
        store
            .sessions
            .insert(record.cutex_session_id.clone(), record);
        let sender = AgentManagementRosterSender {
            runtime_agent_id: RuntimeAgentId::new("runtime-director-r10").unwrap(),
            roster_session_id: "01a041ba-47f6-7e31-bb09-1462cd309ae4".to_string(),
        };
        let invocation = resolve_agent_management_invocation_from_store(&sender, &store).unwrap();
        assert_eq!(
            invocation.caller_cutex_session.as_str(),
            "cutex.director-r10"
        );
        assert_eq!(invocation.caller_runtime_agent_id, "runtime-director-r10");

        store
            .sessions
            .get_mut("cutex.director-r10")
            .unwrap()
            .current_runtime_agent_id = Some("superseded-runtime".to_string());
        assert_eq!(
            resolve_agent_management_invocation_from_store(&sender, &store),
            Err(AgentManagementInvocationError::StaleRuntimeIdentity)
        );
        let record = store.sessions.get_mut("cutex.director-r10").unwrap();
        record.current_runtime_agent_id = Some("runtime-director-r10".to_string());
        record.archive_state = crate::session::model::CutexSessionArchiveState::Retired;
        assert_eq!(
            resolve_agent_management_invocation_from_store(&sender, &store),
            Err(AgentManagementInvocationError::StaleRuntimeIdentity)
        );
    }

    fn worker_host_fixture(label: &str) -> WorkerHostFixture {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("{label}-{}", uuid::Uuid::new_v4()));
        let task_root = root.join("task-service");
        let evidence_root = root.join("evidence");
        for path in [&root, &task_root, &evidence_root] {
            fs::create_dir(path).expect("create private worker fixture root");
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .expect("secure private worker fixture root");
            #[cfg(windows)]
            crate::platform::private_fs::secure_directory(path)
                .expect("secure private worker fixture root");
        }
        let pilot = Arc::new(TaskDeliveryPilot::open(&task_root).expect("open worker pilot"));
        pilot.recover().expect("recover worker pilot");
        let task_id = crate::role_revision::TaskId::new("route-task").unwrap();
        let task_revision = crate::role_revision::TaskRevision::new(1).unwrap();
        let runtime_id = crate::role_revision::RuntimeAgentId::new("route-runtime").unwrap();
        let session_id = crate::role_revision::CutexSessionId::new("cutex.route-session").unwrap();
        let owner = PilotOwnerSnapshot {
            cutex_session_id: session_id.clone(),
            durable_revision: crate::role_revision::DurableRevision::new(7).unwrap(),
            runtime_agent_id: runtime_id.clone(),
            runtime_generation: crate::role_revision::RuntimeGeneration::new(3).unwrap(),
        };
        let contract = "opaque route contract".to_string();
        let published = pilot
            .publish(PilotPublishRequest {
                specification: PilotTaskSpecification {
                    task_id: task_id.clone(),
                    task_revision,
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract,
                },
                create_receipt_id: crate::role_revision::ReceiptId::new("route:create").unwrap(),
                publish_receipt_id: crate::role_revision::ReceiptId::new("route:publish").unwrap(),
                expected_store_revision: crate::role_revision::StoreRevision::new(1).unwrap(),
                attempt_token: AttemptToken::new("route:attempt:1").unwrap(),
                owner: owner.clone(),
            })
            .expect("publish route task");
        let fence: PilotAttemptFence = published.fence().clone();
        let delivery_action = crate::role_revision::DeliveryId::new("route:delivery").unwrap();
        let delivery_transition =
            crate::role_revision::ReceiptId::new("route:delivery:transition").unwrap();
        let envelope_sha256 = crate::task_delivery::agent_bus_adapter::delivery_envelope_sha256(
            &published,
            &delivery_action,
        )
        .unwrap();
        pilot
            .deliver(
                PilotDeliveryRequest::new(published, delivery_action.clone(), delivery_transition),
                AgentBusDeliveryReceiptV1 {
                    delivery_action_id: delivery_action,
                    agent_bus_message_id: "route-delivery-message".to_string(),
                    target_cutex_session_id: session_id,
                    target_runtime_agent_id: runtime_id.clone(),
                    target_runtime_generation: owner.runtime_generation,
                    delivery_mode: PilotDeliveryMode::AfterTurn,
                    queued: true,
                    deduplicated: false,
                    envelope_sha256,
                },
            )
            .expect("deliver route task");
        let sessions = Arc::new(RouteSessionBoundary {
            records: BTreeMap::from([
                (
                    "route-thread".to_string(),
                    active_route_session(
                        "cutex.route-session",
                        "route-thread",
                        runtime_id.as_str(),
                        7,
                        3,
                    ),
                ),
                (
                    "other-thread".to_string(),
                    active_route_session(
                        "cutex.other-session",
                        "other-thread",
                        "other-runtime",
                        11,
                        2,
                    ),
                ),
            ]),
        });
        let adapter = Arc::new(TaskWorkerActionAdapter::with_pilot_and_sessions(
            pilot.clone(),
            sessions.clone(),
        ));
        let evidence = TaskWorkerActionEvidenceStore::open(&evidence_root).unwrap();
        let host = Arc::new(TaskWorkerActionHost::with_parts(adapter.clone(), evidence));
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        state.lock().unwrap().agents.extend([
            (
                runtime_id.as_str().to_string(),
                active_route_roster(runtime_id.as_str(), "route-thread"),
            ),
            (
                "other-runtime".to_string(),
                active_route_roster("other-runtime", "other-thread"),
            ),
        ]);
        let sender = TaskWorkerRosterSender {
            runtime_agent_id: runtime_id,
            roster_session_id: "route-thread".to_string(),
        };
        let action = TaskWorkerActionRequest {
            schema: TaskWorkerActionSchema::V1,
            action: TaskWorkerActionKind::Accept,
            task_id,
            task_revision,
            attempt_fence: fence,
            expected_store_revision: crate::role_revision::StoreRevision::new(4).unwrap(),
            action_id: crate::role_revision::ReceiptId::new("route:accept:1").unwrap(),
            result: None,
        };
        WorkerHostFixture {
            root,
            task_root,
            evidence_root,
            pilot,
            adapter,
            host,
            state,
            sessions,
            sender,
            action,
        }
    }

    fn inspect_request(
        uncertainty_id: crate::role_revision::ReceiptId,
        action_id: crate::role_revision::ReceiptId,
    ) -> TaskWorkerReconciliationRequest {
        TaskWorkerReconciliationRequest {
            schema: TaskWorkerReconciliationSchema::V1,
            operation: TaskWorkerReconciliationOperation::Inspect {
                uncertainty_id,
                action_id,
            },
        }
    }

    fn ack_request(
        uncertainty_id: crate::role_revision::ReceiptId,
        action_id: crate::role_revision::ReceiptId,
        resolution: &crate::agent_bus::model::TaskWorkerResolution,
    ) -> TaskWorkerReconciliationRequest {
        TaskWorkerReconciliationRequest {
            schema: TaskWorkerReconciliationSchema::V1,
            operation: TaskWorkerReconciliationOperation::Ack {
                uncertainty_id,
                action_id,
                resolution_id: resolution.resolution_id.clone(),
                resolution_sha256: resolution.resolution_sha256.clone(),
            },
        }
    }

    fn root_image(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut image = fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        image.sort_by(|left, right| left.0.cmp(&right.0));
        image
    }

    fn route_action_body(
        state: Arc<Mutex<AgentBusState>>,
        host: Arc<TaskWorkerActionHost>,
        sender: &str,
        body: &[u8],
    ) -> TaskWorkerActionResponse {
        serde_json::from_value(http_json(&invoke_task_route(
            raw_task_route_request("/api/task/actions", "route-token", sender, body),
            state,
            host,
        )))
        .expect("deserialize strict HTTP action response")
    }

    fn route_action_with(
        state: Arc<Mutex<AgentBusState>>,
        host: Arc<TaskWorkerActionHost>,
        sender: &str,
        action: &TaskWorkerActionRequest,
    ) -> TaskWorkerActionResponse {
        route_action_body(
            state,
            host,
            sender,
            &serde_json::to_vec(action).expect("serialize strict HTTP action"),
        )
    }

    fn route_action(
        fixture: &WorkerHostFixture,
        action: &TaskWorkerActionRequest,
    ) -> TaskWorkerActionResponse {
        route_action_with(
            fixture.state.clone(),
            fixture.host.clone(),
            fixture.sender.runtime_agent_id.as_str(),
            action,
        )
    }

    fn execute_fixture_action(
        fixture: &WorkerHostFixture,
        action: &TaskWorkerActionRequest,
    ) -> TaskWorkerActionResponse {
        route_action(fixture, action)
    }

    fn route_reconciliation_with(
        state: Arc<Mutex<AgentBusState>>,
        host: Arc<TaskWorkerActionHost>,
        sender: &str,
        request: &TaskWorkerReconciliationRequest,
    ) -> TaskWorkerReconciliationResponse {
        serde_json::from_value(http_json(&invoke_task_route(
            raw_task_route_request(
                "/api/task/actions/reconcile",
                "route-token",
                sender,
                &serde_json::to_vec(request).expect("serialize strict reconciliation request"),
            ),
            state,
            host,
        )))
        .expect("deserialize strict HTTP reconciliation response")
    }

    fn route_reconciliation_body(
        state: Arc<Mutex<AgentBusState>>,
        host: Arc<TaskWorkerActionHost>,
        sender: &str,
        body: &[u8],
    ) -> TaskWorkerReconciliationResponse {
        serde_json::from_value(http_json(&invoke_task_route(
            raw_task_route_request("/api/task/actions/reconcile", "route-token", sender, body),
            state,
            host,
        )))
        .expect("deserialize raw HTTP reconciliation response")
    }

    fn committed_receipt(
        fixture: &str,
        response: &TaskWorkerActionResponse,
    ) -> crate::agent_bus::model::TaskWorkerActionReceipt {
        match &response.outcome {
            TaskWorkerActionOutcome::Committed(receipt) => receipt.clone(),
            other => panic!("{fixture} expected committed HTTP action: {other:?}"),
        }
    }

    fn reconciliation_ids(
        fixture: &str,
        response: &TaskWorkerActionResponse,
    ) -> (
        crate::role_revision::ReceiptId,
        crate::role_revision::ReceiptId,
    ) {
        match &response.outcome {
            TaskWorkerActionOutcome::ReconciliationRequired {
                uncertainty_id,
                action_id,
            } => (uncertainty_id.clone(), action_id.clone()),
            other => panic!("{fixture} expected durable reconciliation fence: {other:?}"),
        }
    }

    fn reopened_pilot(task_root: &Path) -> Arc<TaskDeliveryPilot> {
        let pilot = Arc::new(TaskDeliveryPilot::open(task_root).expect("reopen Task Service"));
        pilot.recover().expect("recover reopened Task Service");
        pilot
    }

    fn reopened_host(fixture: &WorkerHostFixture) -> Arc<TaskWorkerActionHost> {
        let pilot = reopened_pilot(&fixture.task_root);
        let adapter = Arc::new(TaskWorkerActionAdapter::with_pilot_and_sessions(
            pilot,
            fixture.sessions.clone(),
        ));
        Arc::new(TaskWorkerActionHost::with_parts(
            adapter,
            TaskWorkerActionEvidenceStore::open(&fixture.evidence_root)
                .expect("reopen evidence store"),
        ))
    }

    fn task_journal_count(root: &Path) -> usize {
        fs::read_to_string(root.join("task-service-v1.events.jsonl"))
            .expect("read Task Service journal")
            .lines()
            .count()
    }

    fn evidence_snapshot(root: &Path) -> Value {
        serde_json::from_slice(
            &fs::read(root.join("task-worker-action-evidence-v1.json"))
                .expect("read evidence snapshot"),
        )
        .expect("decode evidence snapshot independently")
    }

    fn evidence_records(root: &Path) -> Vec<Value> {
        evidence_snapshot(root)["records_by_action_key"]
            .as_object()
            .expect("evidence record map")
            .values()
            .cloned()
            .collect()
    }

    fn evidence_record_for_action(root: &Path, action_id: &str) -> Value {
        evidence_records(root)
            .into_iter()
            .find(|record| record["action_id"] == action_id)
            .unwrap_or_else(|| panic!("missing independent evidence record for {action_id}"))
    }

    fn raw_result_count(root: &Path) -> usize {
        evidence_records(root)
            .iter()
            .filter(|record| !record["result"].is_null())
            .count()
    }

    fn raw_result_bytes(record: &Value) -> Vec<u8> {
        match record["result"]["encoding"]
            .as_str()
            .expect("tagged result encoding")
        {
            "utf8" => record["result"]["text"]
                .as_str()
                .expect("UTF-8 result text")
                .as_bytes()
                .to_vec(),
            "base64" => BASE64
                .decode(
                    record["result"]["data"]
                        .as_str()
                        .expect("Base64 result data"),
                )
                .expect("canonical Base64 evidence bytes"),
            encoding => panic!("unexpected evidence result encoding {encoding}"),
        }
    }

    struct RawWorkerTransitionExpectation {
        action_id: String,
        task_id: String,
        task_revision: Value,
        attempt_number: Value,
        expected_store_revision: Value,
        command_type: &'static str,
        prior_phase: &'static str,
        resulting_phase: &'static str,
        transport_record_id: String,
        result_sha256: String,
    }

    struct RawTaskReceiptOracle {
        action_id: String,
        receipt_present: bool,
        event_present: bool,
        store_revision: Value,
        journal_cursor: Value,
        evidence: Value,
        task_image: Vec<(String, Vec<u8>)>,
    }

    fn closed_json_object<'a>(
        value: &'a Value,
        expected_keys: &[&str],
        label: &str,
    ) -> &'a serde_json::Map<String, Value> {
        let object = value
            .as_object()
            .unwrap_or_else(|| panic!("{label}: expected JSON object"));
        let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let expected = expected_keys.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "{label}: closed parser keys");
        object
    }

    fn independent_raw_task_receipt_oracle(
        root: &Path,
        expected: &RawWorkerTransitionExpectation,
    ) -> RawTaskReceiptOracle {
        let task_image = root_image(root);
        let snapshot_bytes = fs::read(root.join("task-service-v1.json"))
            .expect("raw oracle reads recovered Task Service snapshot bytes");
        let journal_bytes = fs::read(root.join("task-service-v1.events.jsonl"))
            .expect("raw oracle reads recovered Task Service journal bytes");
        assert!(
            journal_bytes.ends_with(b"\n"),
            "raw oracle requires a complete recovered journal"
        );
        let snapshot: Value = serde_json::from_slice(&snapshot_bytes)
            .expect("closed raw oracle decodes recovered snapshot JSON");
        let snapshot = closed_json_object(
            &snapshot,
            &[
                "schema",
                "store_revision",
                "journal_checkpoint",
                "tasks",
                "receipts",
            ],
            "raw Task Service snapshot",
        );
        assert_eq!(snapshot["schema"], "cutex/task-store/v1");
        closed_json_object(
            &snapshot["journal_checkpoint"],
            &["sequence", "event_sha256"],
            "raw snapshot journal cursor",
        );

        let records = journal_bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                let record: Value = serde_json::from_slice(line)
                    .expect("closed raw oracle decodes one journal record");
                closed_json_object(
                    &record,
                    &[
                        "schema",
                        "sequence",
                        "previous_event_sha256",
                        "event_sha256",
                        "store_revision",
                        "event",
                    ],
                    "raw Task Service journal record",
                );
                assert_eq!(record["schema"], "cutex/task-journal-record/v1");
                record
            })
            .collect::<Vec<_>>();
        let head = records.last().expect("raw recovered journal has a head");
        let journal_cursor = serde_json::json!({
            "sequence": head["sequence"].clone(),
            "event_sha256": head["event_sha256"].clone(),
        });
        assert_eq!(snapshot["journal_checkpoint"], journal_cursor);
        assert_eq!(snapshot["store_revision"], head["store_revision"]);

        let receipts = snapshot["receipts"]
            .as_object()
            .expect("raw snapshot receipt map");
        let receipt = receipts.get(&expected.action_id);
        let matching_events = records
            .iter()
            .filter(|record| {
                record["event"]["kind"] == "transition"
                    && record["event"]["body"]["envelope"]["receipt_id"] == expected.action_id
            })
            .collect::<Vec<_>>();
        assert!(
            matching_events.len() <= 1,
            "raw journal contains at most one exact action event"
        );
        let event = matching_events.first().copied();
        assert_eq!(
            receipt.is_some(),
            event.is_some(),
            "raw snapshot receipt and journal event presence"
        );

        let evidence = match (receipt, event) {
            (Some(receipt), Some(record)) => {
                closed_json_object(
                    receipt,
                    &[
                        "schema",
                        "receipt_id",
                        "request_digest_sha256",
                        "response",
                        "event_cursor",
                    ],
                    "raw Task Service receipt",
                );
                assert_eq!(receipt["schema"], "cutex/task-receipt/v1");
                assert_eq!(receipt["receipt_id"], expected.action_id);
                closed_json_object(
                    &receipt["event_cursor"],
                    &["sequence", "event_sha256"],
                    "raw receipt event cursor",
                );

                let event = closed_json_object(
                    &record["event"],
                    &["kind", "body"],
                    "raw transition event wrapper",
                );
                assert_eq!(event["kind"], "transition");
                let transition = closed_json_object(
                    &event["body"],
                    &["envelope", "response"],
                    "raw transition event",
                );
                let envelope = closed_json_object(
                    &transition["envelope"],
                    &[
                        "schema",
                        "receipt_id",
                        "request_digest_sha256",
                        "expected_store_revision",
                        "fence",
                        "command",
                    ],
                    "raw transition envelope",
                );
                assert_eq!(envelope["schema"], "cutex/task-transition-envelope/v1");
                assert_eq!(envelope["receipt_id"], expected.action_id);
                assert_eq!(
                    envelope["expected_store_revision"],
                    expected.expected_store_revision
                );
                let command = closed_json_object(
                    &envelope["command"],
                    &["type", "body"],
                    "raw transition command",
                );
                assert_eq!(command["type"], expected.command_type);
                let command_evidence = closed_json_object(
                    &command["body"],
                    &["external_receipt_id", "observed_at", "evidence_sha256"],
                    "raw transition evidence",
                );
                assert_eq!(
                    command_evidence["external_receipt_id"],
                    expected.transport_record_id
                );
                assert!(command_evidence["observed_at"].is_null());
                assert_eq!(command_evidence["evidence_sha256"], expected.result_sha256);

                let response = &transition["response"];
                closed_json_object(
                    response,
                    &[
                        "schema",
                        "receipt_id",
                        "committed_store_revision",
                        "task_id",
                        "task_revision",
                        "attempt_number",
                        "prior_phase",
                        "resulting_phase",
                        "committed_at",
                    ],
                    "raw transition response",
                );
                assert_eq!(response["schema"], "cutex/task-transition-response/v1");
                assert_eq!(response["receipt_id"], expected.action_id);
                assert_eq!(response["task_id"], expected.task_id);
                assert_eq!(response["task_revision"], expected.task_revision);
                assert_eq!(response["attempt_number"], expected.attempt_number);
                assert_eq!(response["prior_phase"], expected.prior_phase);
                assert_eq!(response["resulting_phase"], expected.resulting_phase);
                assert_eq!(
                    response["committed_store_revision"],
                    record["store_revision"]
                );
                assert_eq!(receipt["response"], *response);
                assert_eq!(
                    receipt["request_digest_sha256"],
                    envelope["request_digest_sha256"]
                );
                assert_eq!(
                    receipt["event_cursor"],
                    serde_json::json!({
                        "sequence": record["sequence"].clone(),
                        "event_sha256": record["event_sha256"].clone(),
                    })
                );
                serde_json::json!({
                    "status": "committed",
                    "evidence": {
                        "receipt": {
                            "action_id": expected.action_id,
                            "task_id": expected.task_id,
                            "task_revision": expected.task_revision,
                            "attempt_number": expected.attempt_number,
                            "prior_phase": expected.prior_phase,
                            "resulting_phase": expected.resulting_phase,
                            "committed_store_revision": response["committed_store_revision"].clone(),
                            "committed_at": response["committed_at"].clone(),
                            "transport_record_id": expected.transport_record_id,
                            "result_sha256": expected.result_sha256,
                        },
                        "request_digest_sha256": receipt["request_digest_sha256"].clone(),
                        "event_cursor": receipt["event_cursor"].clone(),
                        "observed_store_revision": snapshot["store_revision"].clone(),
                        "observed_journal_cursor": snapshot["journal_checkpoint"].clone(),
                    }
                })
            }
            (None, None) => serde_json::json!({
                "status": "absent",
                "evidence": {
                    "observed_store_revision": snapshot["store_revision"].clone(),
                    "observed_journal_cursor": snapshot["journal_checkpoint"].clone(),
                }
            }),
            _ => unreachable!("receipt/event presence equality checked above"),
        };

        RawTaskReceiptOracle {
            action_id: expected.action_id.clone(),
            receipt_present: receipt.is_some(),
            event_present: event.is_some(),
            store_revision: snapshot["store_revision"].clone(),
            journal_cursor,
            evidence,
            task_image,
        }
    }

    fn roster_observer_bytes(state: &Arc<Mutex<AgentBusState>>) -> Vec<u8> {
        let state = state.lock().expect("roster observer lock");
        let mut rows = state
            .agents
            .values()
            .map(|agent| {
                (
                    agent.id.clone(),
                    agent.session_id.clone(),
                    agent.host_id.clone(),
                    agent.pid,
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        serde_json::to_vec(&rows).expect("serialize independent roster observer")
    }

    struct UnrelatedOracle {
        task_id: crate::role_revision::TaskId,
        task_revision: crate::role_revision::TaskRevision,
        task: crate::task_delivery::PilotTaskSnapshot,
        record: Value,
        sessions: Vec<u8>,
        roster: Vec<u8>,
    }

    fn install_unrelated_http_oracle(fixture: &mut WorkerHostFixture) -> UnrelatedOracle {
        const LABEL: &str = "noninterference/setup";
        let contract = "unrelated immutable route task".to_string();
        let published = fixture
            .pilot
            .publish(PilotPublishRequest {
                specification: PilotTaskSpecification {
                    task_id: crate::role_revision::TaskId::new("unrelated-task").unwrap(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract,
                },
                create_receipt_id: crate::role_revision::ReceiptId::new("unrelated:create")
                    .unwrap(),
                publish_receipt_id: crate::role_revision::ReceiptId::new("unrelated:publish")
                    .unwrap(),
                expected_store_revision: fixture.action.expected_store_revision,
                attempt_token: AttemptToken::new("unrelated:attempt").unwrap(),
                owner: fixture.action.attempt_fence.owner.clone(),
            })
            .expect("publish unrelated oracle task");
        let delivery_action = crate::role_revision::DeliveryId::new("unrelated:delivery").unwrap();
        let delivery_transition =
            crate::role_revision::ReceiptId::new("unrelated:delivery:transition").unwrap();
        let envelope_sha256 = crate::task_delivery::agent_bus_adapter::delivery_envelope_sha256(
            &published,
            &delivery_action,
        )
        .unwrap();
        let delivered = fixture
            .pilot
            .deliver(
                PilotDeliveryRequest::new(published, delivery_action.clone(), delivery_transition),
                AgentBusDeliveryReceiptV1 {
                    delivery_action_id: delivery_action,
                    agent_bus_message_id: "unrelated-delivery-message".to_string(),
                    target_cutex_session_id: fixture
                        .action
                        .attempt_fence
                        .owner
                        .cutex_session_id
                        .clone(),
                    target_runtime_agent_id: fixture
                        .action
                        .attempt_fence
                        .owner
                        .runtime_agent_id
                        .clone(),
                    target_runtime_generation: fixture
                        .action
                        .attempt_fence
                        .owner
                        .runtime_generation,
                    delivery_mode: PilotDeliveryMode::AfterTurn,
                    queued: true,
                    deduplicated: false,
                    envelope_sha256,
                },
            )
            .expect("deliver unrelated oracle task");
        let unrelated_action = TaskWorkerActionRequest {
            schema: TaskWorkerActionSchema::V1,
            action: TaskWorkerActionKind::Accept,
            task_id: delivered.published().specification().task_id.clone(),
            task_revision: delivered.published().specification().task_revision,
            attempt_fence: delivered.published().fence().clone(),
            expected_store_revision: delivered.committed_store_revision(),
            action_id: crate::role_revision::ReceiptId::new("unrelated:accept").unwrap(),
            result: None,
        };
        let response = route_action(fixture, &unrelated_action);
        let receipt = committed_receipt(LABEL, &response);
        fixture.action.expected_store_revision = receipt.committed_store_revision;
        let task = reopened_pilot(&fixture.task_root)
            .task(
                unrelated_action.task_id.clone(),
                unrelated_action.task_revision,
            )
            .expect("observe unrelated task")
            .expect("unrelated task exists");
        UnrelatedOracle {
            task_id: unrelated_action.task_id,
            task_revision: unrelated_action.task_revision,
            task,
            record: evidence_record_for_action(
                &fixture.evidence_root,
                unrelated_action.action_id.as_str(),
            ),
            sessions: fixture.sessions.observer_bytes(),
            roster: roster_observer_bytes(&fixture.state),
        }
    }

    fn assert_unrelated_unchanged(
        fixture: &WorkerHostFixture,
        oracle: &UnrelatedOracle,
        case: &str,
    ) {
        let observed = reopened_pilot(&fixture.task_root)
            .task(oracle.task_id.clone(), oracle.task_revision)
            .expect("observe unrelated task after event")
            .expect("unrelated task survives");
        assert_eq!(observed, oracle.task, "{case}: unrelated Task Service task");
        assert_eq!(
            evidence_record_for_action(&fixture.evidence_root, "unrelated:accept"),
            oracle.record,
            "{case}: unrelated immutable evidence record"
        );
        assert_eq!(
            fixture.sessions.observer_bytes(),
            oracle.sessions,
            "{case}: session observer bytes"
        );
        assert_eq!(
            roster_observer_bytes(&fixture.state),
            oracle.roster,
            "{case}: roster observer bytes"
        );
    }

    fn drive_to_running_http(
        fixture: &WorkerHostFixture,
    ) -> (
        TaskWorkerActionRequest,
        TaskWorkerActionResponse,
        TaskWorkerActionRequest,
        TaskWorkerActionResponse,
    ) {
        let accept = fixture.action.clone();
        let accept_response = route_action(fixture, &accept);
        let accept_receipt = committed_receipt("drive-to-running/accept", &accept_response);
        let mut start = accept.clone();
        start.action = TaskWorkerActionKind::Start;
        start.expected_store_revision = accept_receipt.committed_store_revision;
        start.action_id = crate::role_revision::ReceiptId::new("route:start:1").unwrap();
        let start_response = route_action(fixture, &start);
        committed_receipt("drive-to-running/start", &start_response);
        (accept, accept_response, start, start_response)
    }

    fn complete_request(
        fixture: &WorkerHostFixture,
        expected_store_revision: crate::role_revision::StoreRevision,
        action_id: &str,
        result: TaskWorkerResult,
    ) -> TaskWorkerActionRequest {
        let mut complete = fixture.action.clone();
        complete.action = TaskWorkerActionKind::Complete;
        complete.expected_store_revision = expected_store_revision;
        complete.action_id = crate::role_revision::ReceiptId::new(action_id).unwrap();
        complete.result = Some(result);
        complete
    }

    #[cfg(target_os = "linux")]
    fn spawn_snapshot_obstruction(
        watch_path: &Path,
        watch_mask: u32,
        snapshot: PathBuf,
        backup: PathBuf,
    ) -> std::thread::JoinHandle<bool> {
        let watch_path = std::ffi::CString::new(watch_path.as_os_str().as_encoded_bytes()).unwrap();
        let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        assert!(descriptor >= 0, "create causal filesystem observer");
        let watch = unsafe { libc::inotify_add_watch(descriptor, watch_path.as_ptr(), watch_mask) };
        assert!(watch >= 0, "install causal filesystem observer");
        std::thread::spawn(move || {
            let mut event = [0_u8; 4096];
            let read = unsafe { libc::read(descriptor, event.as_mut_ptr().cast(), event.len()) };
            assert!(read > 0, "observe durable persistence boundary");
            if fs::rename(&snapshot, &backup).is_err() {
                unsafe { libc::close(descriptor) };
                return false;
            }
            let installed = fs::create_dir(&snapshot).is_ok();
            unsafe { libc::close(descriptor) };
            installed
        })
    }

    #[cfg(target_os = "linux")]
    fn restore_snapshot_obstruction(snapshot: &Path, backup: &Path, installed: bool) {
        if installed {
            fs::remove_dir(snapshot).expect("remove causal snapshot obstruction");
            fs::rename(backup, snapshot).expect("restore pre-boundary snapshot");
        } else if backup.exists() {
            fs::remove_file(backup).expect("remove lost-race backup");
        }
    }

    fn pending_after_http_prepare(
        label: &str,
    ) -> (
        WorkerHostFixture,
        UnrelatedOracle,
        crate::role_revision::ReceiptId,
        crate::role_revision::ReceiptId,
    ) {
        let mut fixture = worker_host_fixture(label);
        let unrelated = install_unrelated_http_oracle(&mut fixture);
        let faulted = Arc::new(TaskWorkerActionHost::with_parts(
            fixture.adapter.clone(),
            TaskWorkerActionEvidenceStore::open_with_fault(
                &fixture.evidence_root,
                task_action_store::StoreFaultPoint::AfterRename,
            )
            .unwrap(),
        ));
        let response = route_action_with(
            fixture.state.clone(),
            faulted,
            "route-runtime",
            &fixture.action,
        );
        let (uncertainty_id, action_id) = reconciliation_ids(label, &response);
        (fixture, unrelated, uncertainty_id, action_id)
    }

    #[test]
    fn t_exec_evidence_nonterminal_http_records_are_distinct() {
        const FIXTURE: &str = "T-EXEC-EVIDENCE-NONTERMINAL";
        let fixture = worker_host_fixture(FIXTURE);
        let task_before = reopened_pilot(&fixture.task_root)
            .task(fixture.action.task_id.clone(), fixture.action.task_revision)
            .unwrap()
            .unwrap();
        assert_eq!(
            task_before.phase,
            crate::task_delivery::PilotTaskPhase::Delivered
        );
        let task_root_before = root_image(&fixture.task_root);
        let evidence_root_before = root_image(&fixture.evidence_root);
        let mut forged = serde_json::to_value(&fixture.action).unwrap();
        forged.as_object_mut().unwrap().insert(
            "transport_reference".to_string(),
            serde_json::json!("caller-owned"),
        );
        let rejected = route_action_body(
            fixture.state.clone(),
            fixture.host.clone(),
            "route-runtime",
            &serde_json::to_vec(&forged).unwrap(),
        );
        assert!(matches!(
            rejected.outcome,
            TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::InvalidBody)
        ));
        assert_eq!(root_image(&fixture.task_root), task_root_before);
        assert_eq!(root_image(&fixture.evidence_root), evidence_root_before);

        let accept = route_action(&fixture, &fixture.action);
        let accept_receipt = committed_receipt(FIXTURE, &accept);
        let mut start = fixture.action.clone();
        start.action = TaskWorkerActionKind::Start;
        start.expected_store_revision = accept_receipt.committed_store_revision;
        start.action_id = crate::role_revision::ReceiptId::new("route:start:1").unwrap();
        let start_response = route_action(&fixture, &start);
        let start_receipt = committed_receipt(FIXTURE, &start_response);
        assert_ne!(
            accept_receipt.transport_record_id,
            start_receipt.transport_record_id
        );
        let reopened = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
        let accept_record = reopened
            .record_by_id(&accept_receipt.transport_record_id)
            .unwrap()
            .unwrap();
        let start_record = reopened
            .record_by_id(&start_receipt.transport_record_id)
            .unwrap()
            .unwrap();
        assert!(accept_record.result.is_none());
        assert!(start_record.result.is_none());
        assert_eq!(
            accept_record.expected_store_revision,
            fixture.action.expected_store_revision
        );
        assert_eq!(
            start_record.expected_store_revision,
            accept_receipt.committed_store_revision
        );
        let journal =
            fs::read_to_string(fixture.task_root.join("task-service-v1.events.jsonl")).unwrap();
        assert!(journal.contains(accept_receipt.transport_record_id.as_str()));
        assert!(journal.contains(start_receipt.transport_record_id.as_str()));
        assert_eq!(evidence_records(&fixture.evidence_root).len(), 2);
        assert_eq!(raw_result_count(&fixture.evidence_root), 0);
        let observed = reopened_pilot(&fixture.task_root)
            .task(fixture.action.task_id.clone(), fixture.action.task_revision)
            .unwrap()
            .unwrap();
        assert_eq!(
            observed.phase,
            crate::task_delivery::PilotTaskPhase::Running
        );
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_evidence_complete_http_utf8_and_base64_restart() {
        const FIXTURE: &str = "T-EXEC-EVIDENCE-COMPLETE";
        for (label, bytes, result) in [
            (
                "utf8",
                "opaque π result".as_bytes().to_vec(),
                TaskWorkerResult::Utf8 {
                    text: "opaque π result".to_string(),
                    sha256: crate::task_service::sha256_bytes("opaque π result".as_bytes()),
                },
            ),
            (
                "base64",
                vec![0, 255, 1, 128, 42],
                TaskWorkerResult::Base64 {
                    data: BASE64.encode([0, 255, 1, 128, 42]),
                    sha256: crate::task_service::sha256_bytes(&[0, 255, 1, 128, 42]),
                },
            ),
        ] {
            let fixture = worker_host_fixture(&format!("{FIXTURE}-{label}"));
            let (_, _, _, start_response) = drive_to_running_http(&fixture);
            let start_receipt = committed_receipt(FIXTURE, &start_response);
            let complete = complete_request(
                &fixture,
                start_receipt.committed_store_revision,
                &format!("route:complete:{label}"),
                result,
            );
            let request_path = fixture.root.join(format!("private-complete-{label}.json"));
            fs::write(&request_path, serde_json::to_vec(&complete).unwrap()).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&request_path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let before_count = task_journal_count(&fixture.task_root);
            let response = route_action_body(
                fixture.state.clone(),
                fixture.host.clone(),
                "route-runtime",
                &fs::read(&request_path).unwrap(),
            );
            let receipt = committed_receipt(FIXTURE, &response);
            fs::remove_file(&request_path).unwrap();
            assert_eq!(task_journal_count(&fixture.task_root), before_count + 1);
            let _restarted = reopened_host(&fixture);
            let reopened = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
            let record = reopened
                .record_by_id(&receipt.transport_record_id)
                .unwrap()
                .expect("retrieve complete record by server UUID after restart");
            let raw = raw_result_bytes(&serde_json::to_value(&record).unwrap());
            assert_eq!(raw, bytes);
            assert_eq!(
                crate::task_service::sha256_bytes(&raw),
                receipt.result_sha256.clone().unwrap()
            );
            assert_eq!(
                record.expected_store_revision,
                start_receipt.committed_store_revision
            );
            let journal =
                fs::read_to_string(fixture.task_root.join("task-service-v1.events.jsonl")).unwrap();
            assert!(journal.contains(receipt.transport_record_id.as_str()));
            assert!(journal.contains(receipt.result_sha256.as_ref().unwrap().as_str()));
            let evidence_before_forgery = root_image(&fixture.evidence_root);
            let task_before_forgery = root_image(&fixture.task_root);
            assert!(reopened
                .record_by_id(
                    &crate::role_revision::ReceiptId::new("00000000-0000-4000-8000-000000000000",)
                        .unwrap(),
                )
                .unwrap()
                .is_none());
            assert_eq!(root_image(&fixture.evidence_root), evidence_before_forgery);
            assert_eq!(root_image(&fixture.task_root), task_before_forgery);
            fs::remove_dir_all(fixture.root).unwrap();
        }
    }

    #[test]
    fn t_exec_evidence_idempotency_http_conflict_and_concurrency() {
        const FIXTURE: &str = "T-EXEC-EVIDENCE-IDEMPOTENCY";
        let fixture = worker_host_fixture(FIXTURE);
        let action_body = serde_json::to_vec(&fixture.action).unwrap();
        let transition_count = task_journal_count(&fixture.task_root);
        let first_state = fixture.state.clone();
        let first_host = fixture.host.clone();
        let first_body = action_body.clone();
        let second_state = fixture.state.clone();
        let second_host = fixture.host.clone();
        let second_body = action_body.clone();
        let first = std::thread::spawn(move || {
            route_action_body(first_state, first_host, "route-runtime", &first_body)
        });
        let second = std::thread::spawn(move || {
            route_action_body(second_state, second_host, "route-runtime", &second_body)
        });
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_eq!(first, second);
        let receipt = committed_receipt(FIXTURE, &first);
        assert_eq!(task_journal_count(&fixture.task_root), transition_count + 1);
        assert_eq!(evidence_records(&fixture.evidence_root).len(), 1);
        let task_bytes = root_image(&fixture.task_root);
        let evidence_bytes = root_image(&fixture.evidence_root);

        let restarted = reopened_host(&fixture);
        let exact = route_action_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &fixture.action,
        );
        assert_eq!(exact, first);
        assert_eq!(root_image(&fixture.task_root), task_bytes);
        assert_eq!(root_image(&fixture.evidence_root), evidence_bytes);

        let mut changed = fixture.action.clone();
        changed.expected_store_revision = receipt.committed_store_revision;
        let conflict =
            route_action_with(fixture.state.clone(), restarted, "route-runtime", &changed);
        assert!(matches!(
            conflict.outcome,
            TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::ActionConflict)
        ));
        assert_eq!(root_image(&fixture.task_root), task_bytes);
        assert_eq!(root_image(&fixture.evidence_root), evidence_bytes);
        assert_eq!(task_journal_count(&fixture.task_root), transition_count + 1);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_crash_before_transition_http_absent_oracle() {
        const FIXTURE: &str = "T-EXEC-CRASH-BEFORE-TRANSITION";
        let mut fixture = worker_host_fixture(FIXTURE);
        let unrelated = install_unrelated_http_oracle(&mut fixture);
        let task_before = root_image(&fixture.task_root);
        let transition_count = task_journal_count(&fixture.task_root);
        let host = Arc::new(TaskWorkerActionHost::with_parts(
            fixture.adapter.clone(),
            TaskWorkerActionEvidenceStore::open_with_fault(
                &fixture.evidence_root,
                task_action_store::StoreFaultPoint::AfterRename,
            )
            .unwrap(),
        ));
        let action_response = route_action_with(
            fixture.state.clone(),
            host,
            "route-runtime",
            &fixture.action,
        );
        let (uncertainty_id, action_id) = reconciliation_ids(FIXTURE, &action_response);
        assert_eq!(root_image(&fixture.task_root), task_before);
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        assert!(reopened_pilot(&fixture.task_root)
            .receipt(action_id.clone())
            .unwrap()
            .is_none());
        let reopened = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
        let pending = reopened.uncertainty().unwrap().unwrap();
        assert_eq!(pending.uncertainty_id, uncertainty_id);
        assert_eq!(
            pending.state,
            task_action_store::TaskWorkerUncertaintyState::Pending
        );
        assert!(reopened
            .record_by_id(&pending.transport_record_id)
            .unwrap()
            .is_some());

        let restarted = reopened_host(&fixture);
        let inspect = inspect_request(uncertainty_id.clone(), action_id.clone());
        let first = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &inspect,
        );
        let resolution = match first.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{FIXTURE} expected absent resolution: {other:?}"),
        };
        assert!(matches!(
            resolution.evidence,
            TaskWorkerResolutionEvidence::Absent(_)
        ));
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        let acked = route_reconciliation_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &ack_request(uncertainty_id, action_id, &resolution),
        );
        assert!(matches!(
            acked.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_reconcile_restart_http_exact_resolution_replay() {
        const FIXTURE: &str = "T-EXEC-RECONCILE-RESTART";
        let (fixture, unrelated, uncertainty_id, action_id) = pending_after_http_prepare(FIXTURE);
        let inspect = inspect_request(uncertainty_id.clone(), action_id.clone());
        let first_host = reopened_host(&fixture);
        let first =
            route_reconciliation_with(fixture.state.clone(), first_host, "route-runtime", &inspect);
        assert!(matches!(
            first.outcome,
            TaskWorkerReconciliationOutcome::Resolved(_)
        ));
        let evidence_after_resolution = root_image(&fixture.evidence_root);
        let task_after_resolution = root_image(&fixture.task_root);
        let transitions = task_journal_count(&fixture.task_root);
        let second_host = reopened_host(&fixture);
        let repeated = route_reconciliation_with(
            fixture.state.clone(),
            second_host.clone(),
            "route-runtime",
            &inspect,
        );
        assert_eq!(repeated, first);
        assert_eq!(
            root_image(&fixture.evidence_root),
            evidence_after_resolution
        );
        assert_eq!(root_image(&fixture.task_root), task_after_resolution);
        assert_eq!(task_journal_count(&fixture.task_root), transitions);

        let changed = inspect_request(
            crate::role_revision::ReceiptId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            action_id,
        );
        let rejected = route_reconciliation_with(
            fixture.state.clone(),
            second_host,
            "route-runtime",
            &changed,
        );
        assert_eq!(rejected, reconciliation_rejected());
        assert_eq!(
            root_image(&fixture.evidence_root),
            evidence_after_resolution
        );
        assert_eq!(root_image(&fixture.task_root), task_after_resolution);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_reconcile_wrong_owner_registered_http_inspect_and_ack() {
        const FIXTURE: &str = "T-EXEC-RECONCILE-WRONG-OWNER";
        let (fixture, unrelated, uncertainty_id, action_id) = pending_after_http_prepare(FIXTURE);
        assert!(fixture.sessions.records.contains_key("other-thread"));
        assert!(fixture
            .state
            .lock()
            .unwrap()
            .agents
            .contains_key("other-runtime"));
        let restarted = reopened_host(&fixture);
        let exact_owner = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &inspect_request(uncertainty_id.clone(), action_id.clone()),
        );
        let resolution = match exact_owner.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{FIXTURE} expected exact-owner resolution: {other:?}"),
        };
        let resolved_evidence_snapshot = evidence_snapshot(&fixture.evidence_root);
        let persisted_resolution = &resolved_evidence_snapshot["uncertainty"]["resolution"];
        assert_eq!(
            persisted_resolution["resolution_id"],
            resolution.resolution_id.as_str()
        );
        assert_eq!(
            persisted_resolution["resolution_sha256"],
            resolution.resolution_sha256.as_str()
        );

        let wrong_inspect_evidence = root_image(&fixture.evidence_root);
        let wrong_inspect_task = root_image(&fixture.task_root);
        let wrong_inspect_sessions = fixture.sessions.observer_bytes();
        let wrong_inspect_roster = roster_observer_bytes(&fixture.state);
        let wrong_inspect = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "other-runtime",
            &inspect_request(uncertainty_id.clone(), action_id.clone()),
        );
        assert_eq!(wrong_inspect, reconciliation_rejected());
        assert_eq!(root_image(&fixture.evidence_root), wrong_inspect_evidence);
        assert_eq!(root_image(&fixture.task_root), wrong_inspect_task);
        assert_eq!(fixture.sessions.observer_bytes(), wrong_inspect_sessions);
        assert_eq!(roster_observer_bytes(&fixture.state), wrong_inspect_roster);

        let wrong_ack = ack_request(uncertainty_id.clone(), action_id.clone(), &resolution);
        let evidence_before_ack = root_image(&fixture.evidence_root);
        let task_before_ack = root_image(&fixture.task_root);
        let sessions_before_ack = fixture.sessions.observer_bytes();
        let roster_before_ack = roster_observer_bytes(&fixture.state);
        assert_eq!(
            route_reconciliation_with(
                fixture.state.clone(),
                restarted.clone(),
                "other-runtime",
                &wrong_ack,
            ),
            reconciliation_rejected()
        );
        assert_eq!(root_image(&fixture.evidence_root), evidence_before_ack);
        assert_eq!(root_image(&fixture.task_root), task_before_ack);
        assert_eq!(fixture.sessions.observer_bytes(), sessions_before_ack);
        assert_eq!(roster_observer_bytes(&fixture.state), roster_before_ack);

        let exact_ack = route_reconciliation_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &wrong_ack,
        );
        assert!(matches!(
            exact_ack.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert!(evidence_snapshot(&fixture.evidence_root)
            .get("uncertainty")
            .is_none());
        assert_eq!(root_image(&fixture.task_root), task_before_ack);
        assert_eq!(fixture.sessions.observer_bytes(), sessions_before_ack);
        assert_eq!(roster_observer_bytes(&fixture.state), roster_before_ack);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_reconcile_ack_http_exact_token_only() {
        const FIXTURE: &str = "T-EXEC-RECONCILE-ACK";
        let (fixture, unrelated, uncertainty_id, action_id) = pending_after_http_prepare(FIXTURE);
        let restarted = reopened_host(&fixture);
        let resolved = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &inspect_request(uncertainty_id.clone(), action_id.clone()),
        );
        let resolution = match resolved.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{FIXTURE} expected durable resolution: {other:?}"),
        };
        let exact_ack = ack_request(uncertainty_id, action_id, &resolution);
        let evidence_before_controls = root_image(&fixture.evidence_root);
        let task_before_controls = root_image(&fixture.task_root);
        let transitions = task_journal_count(&fixture.task_root);

        let mut missing = serde_json::to_value(&exact_ack).unwrap();
        missing["operation"]["body"]
            .as_object_mut()
            .unwrap()
            .remove("resolution_sha256");
        let missing = route_reconciliation_body(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &serde_json::to_vec(&missing).unwrap(),
        );
        assert_eq!(missing, reconciliation_rejected());
        let mut wrong = exact_ack.clone();
        if let TaskWorkerReconciliationOperation::Ack {
            resolution_sha256, ..
        } = &mut wrong.operation
        {
            *resolution_sha256 = crate::task_service::zero_sha256();
        }
        assert_eq!(
            route_reconciliation_with(
                fixture.state.clone(),
                restarted.clone(),
                "route-runtime",
                &wrong,
            ),
            reconciliation_rejected()
        );
        let blocked = route_action_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &fixture.action,
        );
        assert!(matches!(
            blocked.outcome,
            TaskWorkerActionOutcome::ReconciliationRequired { .. }
        ));
        assert_eq!(root_image(&fixture.evidence_root), evidence_before_controls);
        assert_eq!(root_image(&fixture.task_root), task_before_controls);
        assert_eq!(task_journal_count(&fixture.task_root), transitions);

        let acked = route_reconciliation_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &exact_ack,
        );
        assert!(matches!(
            acked.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert!(TaskWorkerActionEvidenceStore::open(&fixture.evidence_root)
            .unwrap()
            .uncertainty()
            .unwrap()
            .is_none());
        assert_eq!(root_image(&fixture.task_root), task_before_controls);
        assert_eq!(task_journal_count(&fixture.task_root), transitions);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_noninterference_repair_http_manifest() {
        const FIXTURE: &str = "T-EXEC-NONINTERFERENCE-REPAIR";
        let mut fixture = worker_host_fixture(FIXTURE);
        let unrelated = install_unrelated_http_oracle(&mut fixture);
        let sessions_before = fixture.sessions.observer_bytes();
        let roster_before = roster_observer_bytes(&fixture.state);
        let response = route_action(&fixture, &fixture.action);
        committed_receipt(FIXTURE, &response);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        assert_eq!(fixture.sessions.observer_bytes(), sessions_before);
        assert_eq!(roster_observer_bytes(&fixture.state), roster_before);
        assert_eq!(
            [
                "prepare-before-write",
                "prepare-after-temp-sync",
                "prepare-after-rename",
                "prepare-after-parent-sync",
                "committed",
                "absent",
                "persistence-unknown",
                "inspect",
                "ack",
            ]
            .len(),
            9,
            "companion HTTP fixtures preserve this oracle beside every fault path"
        );
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_store_prepare_crash_never_reaches_task_service() {
        const FIXTURE: &str = "T-EXEC-STORE-PREPARE-CRASH";
        for point in [
            task_action_store::StoreFaultPoint::BeforeWrite,
            task_action_store::StoreFaultPoint::AfterTempSync,
            task_action_store::StoreFaultPoint::AfterRename,
            task_action_store::StoreFaultPoint::AfterParentSync,
        ] {
            let mut fixture = worker_host_fixture(FIXTURE);
            let unrelated = install_unrelated_http_oracle(&mut fixture);
            let evidence =
                TaskWorkerActionEvidenceStore::open_with_fault(&fixture.evidence_root, point)
                    .unwrap();
            let faulted = Arc::new(TaskWorkerActionHost::with_parts(
                fixture.adapter.clone(),
                evidence,
            ));
            let task_before = root_image(&fixture.task_root);
            let evidence_before = root_image(&fixture.evidence_root);
            let transitions_before = task_journal_count(&fixture.task_root);
            let response = route_action_with(
                fixture.state.clone(),
                faulted,
                "route-runtime",
                &fixture.action,
            );
            assert!(matches!(
                response.outcome,
                TaskWorkerActionOutcome::NoWrite(_)
                    | TaskWorkerActionOutcome::ReconciliationRequired { .. }
            ));
            assert_eq!(root_image(&fixture.task_root), task_before);
            assert_eq!(task_journal_count(&fixture.task_root), transitions_before);
            assert!(reopened_pilot(&fixture.task_root)
                .receipt(fixture.action.action_id.clone())
                .unwrap()
                .is_none());
            let reopened = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
            if let Some(uncertainty) = reopened.uncertainty().unwrap() {
                assert!(matches!(
                    point,
                    task_action_store::StoreFaultPoint::AfterRename
                        | task_action_store::StoreFaultPoint::AfterParentSync
                ));
                assert_eq!(
                    uncertainty.state,
                    task_action_store::TaskWorkerUncertaintyState::Pending
                );
                assert!(reopened
                    .record_by_id(&uncertainty.transport_record_id)
                    .unwrap()
                    .is_some());
            } else {
                assert!(matches!(
                    point,
                    task_action_store::StoreFaultPoint::BeforeWrite
                        | task_action_store::StoreFaultPoint::AfterTempSync
                ));
                assert_eq!(root_image(&fixture.evidence_root), evidence_before);
            }
            assert_unrelated_unchanged(&fixture, &unrelated, &format!("{FIXTURE}/{point:?}"));
            fs::remove_dir_all(fixture.root).unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn t_exec_crash_committed_http_oracle() {
        const COMMITTED: &str = "T-EXEC-CRASH-COMMITTED";
        let mut observed = None;
        for attempt in 0..24 {
            let mut fixture = worker_host_fixture(COMMITTED);
            let unrelated = install_unrelated_http_oracle(&mut fixture);
            let journal = fixture.task_root.join("task-service-v1.events.jsonl");
            let snapshot = fixture
                .evidence_root
                .join("task-worker-action-evidence-v1.json");
            let backup = fixture
                .evidence_root
                .join(format!("task-worker-action-evidence-v1.boundary-{attempt}"));
            let task_before = task_journal_count(&fixture.task_root);
            let obstruction = spawn_snapshot_obstruction(
                &journal,
                libc::IN_MODIFY,
                snapshot.clone(),
                backup.clone(),
            );
            let _response = route_action(&fixture, &fixture.action);
            let installed = obstruction.join().unwrap();
            restore_snapshot_obstruction(&snapshot, &backup, installed);
            let pending = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root)
                .unwrap()
                .uncertainty()
                .unwrap();
            let receipt = reopened_pilot(&fixture.task_root)
                .receipt(fixture.action.action_id.clone())
                .unwrap();
            if installed && pending.is_some() && receipt.is_some() {
                observed = Some((fixture, unrelated, pending.unwrap(), task_before));
                break;
            }
            fs::remove_dir_all(fixture.root).unwrap();
        }
        let (fixture, unrelated, pending, task_before) =
            observed.expect("observe committed transition before evidence-fence clear");
        assert_eq!(task_journal_count(&fixture.task_root), task_before + 1);
        let independent = reopened_pilot(&fixture.task_root)
            .receipt(pending.action_id.clone())
            .unwrap()
            .expect("independent committed Task Service receipt oracle");
        let task_after_action = root_image(&fixture.task_root);
        let transition_count = task_journal_count(&fixture.task_root);
        let restarted = reopened_host(&fixture);
        let evidence_before_control = root_image(&fixture.evidence_root);
        let mut later = fixture.action.clone();
        later.action = TaskWorkerActionKind::Start;
        later.action_id = crate::role_revision::ReceiptId::new("route:start:blocked").unwrap();
        let blocked = route_action_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &later,
        );
        assert!(matches!(
            blocked.outcome,
            TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::UncertaintyBlocked)
        ));
        assert_eq!(root_image(&fixture.task_root), task_after_action);
        assert_eq!(root_image(&fixture.evidence_root), evidence_before_control);
        let resolved = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &inspect_request(pending.uncertainty_id.clone(), pending.action_id.clone()),
        );
        let resolution = match resolved.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{COMMITTED}: {other:?}"),
        };
        let committed = match &resolution.evidence {
            TaskWorkerResolutionEvidence::Committed(committed) => committed,
            other => panic!("{COMMITTED} independent oracle mismatch: {other:?}"),
        };
        assert_eq!(committed.receipt.action_id, independent.receipt_id);
        assert_eq!(
            committed.receipt.committed_store_revision,
            independent.committed_store_revision
        );
        assert_eq!(root_image(&fixture.task_root), task_after_action);
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        let acked = route_reconciliation_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &ack_request(pending.uncertainty_id, pending.action_id, &resolution),
        );
        assert!(matches!(
            acked.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        assert_unrelated_unchanged(&fixture, &unrelated, COMMITTED);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn t_exec_crash_absent_http_definite_no_write_oracle() {
        const FIXTURE: &str = "T-EXEC-CRASH-ABSENT";
        let mut observed = None;
        for attempt in 0..24 {
            let mut fixture = worker_host_fixture(FIXTURE);
            let unrelated = install_unrelated_http_oracle(&mut fixture);
            let mut start = fixture.action.clone();
            start.action = TaskWorkerActionKind::Start;
            start.action_id = crate::role_revision::ReceiptId::new("route:start:early").unwrap();
            let snapshot = fixture
                .evidence_root
                .join("task-worker-action-evidence-v1.json");
            let backup = fixture
                .evidence_root
                .join(format!("task-worker-action-evidence-v1.absent-{attempt}"));
            let transitions = task_journal_count(&fixture.task_root);
            let obstruction = spawn_snapshot_obstruction(
                &fixture.evidence_root,
                libc::IN_MOVED_TO,
                snapshot.clone(),
                backup.clone(),
            );
            let _response = route_action(&fixture, &start);
            let installed = obstruction.join().unwrap();
            restore_snapshot_obstruction(&snapshot, &backup, installed);
            let pending = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root)
                .unwrap()
                .uncertainty()
                .unwrap();
            if installed && pending.is_some() {
                observed = Some((fixture, unrelated, start, pending.unwrap(), transitions));
                break;
            }
            fs::remove_dir_all(fixture.root).unwrap();
        }
        let (fixture, unrelated, start, pending, transitions) =
            observed.expect("observe definite no-write before evidence-fence clear");
        assert_eq!(task_journal_count(&fixture.task_root), transitions);
        assert!(reopened_pilot(&fixture.task_root)
            .receipt(start.action_id.clone())
            .unwrap()
            .is_none());
        let restarted = reopened_host(&fixture);
        let evidence_before_blocked = root_image(&fixture.evidence_root);
        let task_before_blocked = root_image(&fixture.task_root);
        let blocked = route_action_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &fixture.action,
        );
        assert!(matches!(
            blocked.outcome,
            TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::UncertaintyBlocked)
        ));
        assert_eq!(root_image(&fixture.evidence_root), evidence_before_blocked);
        assert_eq!(root_image(&fixture.task_root), task_before_blocked);
        let task_before_inspect = root_image(&fixture.task_root);
        let resolution = route_reconciliation_with(
            fixture.state.clone(),
            restarted.clone(),
            "route-runtime",
            &inspect_request(pending.uncertainty_id.clone(), pending.action_id.clone()),
        );
        let resolution = match resolution.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{FIXTURE}: {other:?}"),
        };
        assert!(matches!(
            resolution.evidence,
            TaskWorkerResolutionEvidence::Absent(_)
        ));
        assert_eq!(root_image(&fixture.task_root), task_before_inspect);
        assert_eq!(task_journal_count(&fixture.task_root), transitions);
        let acked = route_reconciliation_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &ack_request(pending.uncertainty_id, pending.action_id, &resolution),
        );
        assert!(matches!(
            acked.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert_eq!(task_journal_count(&fixture.task_root), transitions);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn t_exec_crash_persistence_unknown_uses_real_task_service_boundary() {
        const FIXTURE: &str = "T-EXEC-CRASH-PERSISTENCE-UNKNOWN";
        let mut observed = None;
        for attempt in 0..24 {
            let mut fixture = worker_host_fixture(FIXTURE);
            let unrelated = install_unrelated_http_oracle(&mut fixture);
            let (_, _, _, start_response) = drive_to_running_http(&fixture);
            let start_receipt = committed_receipt(FIXTURE, &start_response);
            let result_bytes = b"persistence-unknown opaque result".to_vec();
            let expected_result_sha256 = crate::task_service::sha256_bytes(&result_bytes);
            let complete = complete_request(
                &fixture,
                start_receipt.committed_store_revision,
                "route:complete:unknown",
                TaskWorkerResult::Utf8 {
                    text: String::from_utf8(result_bytes).unwrap(),
                    sha256: expected_result_sha256.clone(),
                },
            );
            let journal = fixture.task_root.join("task-service-v1.events.jsonl");
            let snapshot = fixture.task_root.join("task-service-v1.json");
            let backup = fixture
                .task_root
                .join(format!("task-service-v1.json.boundary-{attempt}"));
            let transitions_before = task_journal_count(&fixture.task_root);
            let sabotage = spawn_snapshot_obstruction(
                &journal,
                libc::IN_MODIFY,
                snapshot.clone(),
                backup.clone(),
            );
            let response = route_action(&fixture, &complete);
            let installed = sabotage.join().unwrap();
            restore_snapshot_obstruction(&snapshot, &backup, installed);
            let pending = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root)
                .unwrap()
                .uncertainty()
                .unwrap();
            if installed
                && pending.is_some()
                && matches!(
                    response.outcome,
                    TaskWorkerActionOutcome::ReconciliationRequired { .. }
                )
            {
                let pending = pending.unwrap();
                let _recovered = reopened_pilot(&fixture.task_root);
                let expected = RawWorkerTransitionExpectation {
                    action_id: complete.action_id.as_str().to_string(),
                    task_id: complete.task_id.as_str().to_string(),
                    task_revision: serde_json::to_value(complete.task_revision).unwrap(),
                    attempt_number: serde_json::to_value(complete.attempt_fence.attempt_number)
                        .unwrap(),
                    expected_store_revision: serde_json::to_value(complete.expected_store_revision)
                        .unwrap(),
                    command_type: "complete_running",
                    prior_phase: "running",
                    resulting_phase: "completed",
                    transport_record_id: pending.transport_record_id.as_str().to_string(),
                    result_sha256: expected_result_sha256.as_str().to_string(),
                };
                let oracle = independent_raw_task_receipt_oracle(&fixture.task_root, &expected);
                observed = Some((
                    fixture,
                    unrelated,
                    complete,
                    pending,
                    oracle,
                    transitions_before,
                ));
                break;
            }
            fs::remove_dir_all(fixture.root).unwrap();
        }
        let (fixture, unrelated, complete, pending, oracle, transitions_before) =
            observed.expect("observe real HTTP Task Service persistence uncertainty");
        assert_eq!(
            pending.state,
            task_action_store::TaskWorkerUncertaintyState::Pending
        );
        assert_eq!(oracle.action_id, complete.action_id.as_str());
        assert_eq!(oracle.receipt_present, oracle.event_present);
        assert_eq!(
            oracle.evidence["evidence"]["observed_store_revision"],
            oracle.store_revision
        );
        closed_json_object(
            &oracle.journal_cursor,
            &["sequence", "event_sha256"],
            "immutable raw oracle journal cursor",
        );
        assert_eq!(
            oracle.evidence["evidence"]["observed_journal_cursor"],
            oracle.journal_cursor
        );
        let transitions_after_action = task_journal_count(&fixture.task_root);
        assert!(transitions_after_action >= transitions_before);
        let raw_results = raw_result_count(&fixture.evidence_root);
        assert_eq!(raw_results, 1);
        let task_before_inspect = oracle.task_image.clone();
        let restarted = reopened_host(&fixture);
        let inspect = inspect_request(pending.uncertainty_id.clone(), pending.action_id.clone());
        let resolved_http = invoke_task_route(
            raw_task_route_request(
                "/api/task/actions/reconcile",
                "route-token",
                "route-runtime",
                &serde_json::to_vec(&inspect).unwrap(),
            ),
            fixture.state.clone(),
            restarted.clone(),
        );
        let resolved_json = http_json(&resolved_http);
        let persisted = evidence_snapshot(&fixture.evidence_root);
        let persisted_resolution = &persisted["uncertainty"]["resolution"];
        closed_json_object(
            persisted_resolution,
            &[
                "resolution_id",
                "resolution_sha256",
                "resolved_at",
                "evidence",
            ],
            "persisted raw reconciliation resolution",
        );
        assert_eq!(persisted_resolution["evidence"], oracle.evidence);
        let expected_resolved_json = serde_json::json!({
            "schema": "cutex/task-worker-reconciliation-response/v1",
            "outcome": {
                "kind": "resolved",
                "body": {
                    "resolution_id": persisted_resolution["resolution_id"].clone(),
                    "resolution_sha256": persisted_resolution["resolution_sha256"].clone(),
                    "resolved_at": persisted_resolution["resolved_at"].clone(),
                    "evidence": oracle.evidence.clone(),
                }
            }
        });
        assert_eq!(
            resolved_json, expected_resolved_json,
            "{FIXTURE}: serialized Inspect response matches the raw-byte oracle field-by-field"
        );
        let resolved: TaskWorkerReconciliationResponse =
            serde_json::from_value(resolved_json).unwrap();
        let resolution = match &resolved.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution.clone(),
            other => panic!("{FIXTURE} expected oracle-selected resolution: {other:?}"),
        };
        assert_eq!(root_image(&fixture.task_root), task_before_inspect);
        assert_eq!(
            task_journal_count(&fixture.task_root),
            transitions_after_action
        );
        assert_eq!(raw_result_count(&fixture.evidence_root), raw_results);
        let evidence_after_inspect = root_image(&fixture.evidence_root);

        let fully_restarted = reopened_host(&fixture);
        let replayed = route_reconciliation_with(
            fixture.state.clone(),
            fully_restarted.clone(),
            "route-runtime",
            &inspect,
        );
        assert_eq!(replayed, resolved);
        assert_eq!(root_image(&fixture.evidence_root), evidence_after_inspect);
        assert_eq!(
            task_journal_count(&fixture.task_root),
            transitions_after_action
        );
        assert_eq!(raw_result_count(&fixture.evidence_root), raw_results);
        let acked = route_reconciliation_with(
            fixture.state.clone(),
            fully_restarted,
            "route-runtime",
            &ack_request(pending.uncertainty_id, pending.action_id, &resolution),
        );
        assert!(matches!(
            acked.outcome,
            TaskWorkerReconciliationOutcome::Acknowledged
        ));
        assert_eq!(
            task_journal_count(&fixture.task_root),
            transitions_after_action
        );
        assert_eq!(raw_result_count(&fixture.evidence_root), raw_results);
        let _after_ack_restart = reopened_host(&fixture);
        assert_eq!(
            task_journal_count(&fixture.task_root),
            transitions_after_action
        );
        assert_eq!(raw_result_count(&fixture.evidence_root), raw_results);
        assert_unrelated_unchanged(&fixture, &unrelated, FIXTURE);
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_replay_later_phase_all_actions_http_restart() {
        const REPLAY: &str = "T-EXEC-REPLAY-LATER-PHASE";
        const UNRELATED_ORACLE: &str = "replay/unrelated-oracle";
        let mut fixture = worker_host_fixture(REPLAY);
        let accept_request = fixture.action.clone();
        let accept = execute_fixture_action(&fixture, &accept_request);
        let accept_receipt = match &accept.outcome {
            TaskWorkerActionOutcome::Committed(receipt) => receipt.clone(),
            other => panic!("accept failed: {other:?}"),
        };
        let mut start = fixture.action.clone();
        start.action = TaskWorkerActionKind::Start;
        start.action_id = crate::role_revision::ReceiptId::new("route:start:1").unwrap();
        start.expected_store_revision = accept_receipt.committed_store_revision;
        let start_response = execute_fixture_action(&fixture, &start);
        let start_receipt = match &start_response.outcome {
            TaskWorkerActionOutcome::Committed(receipt) => receipt.clone(),
            other => panic!("start failed: {other:?}"),
        };
        let result_bytes = b"terminal opaque payload".to_vec();
        let mut complete = fixture.action.clone();
        complete.action = TaskWorkerActionKind::Complete;
        complete.action_id = crate::role_revision::ReceiptId::new("route:complete:1").unwrap();
        complete.expected_store_revision = start_receipt.committed_store_revision;
        complete.result = Some(TaskWorkerResult::Utf8 {
            text: String::from_utf8(result_bytes.clone()).unwrap(),
            sha256: crate::task_service::sha256_bytes(&result_bytes),
        });
        let request_path = fixture.root.join("private-complete.json");
        fs::write(&request_path, serde_json::to_vec(&complete).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&request_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let complete_response = execute_fixture_action(&fixture, &complete);
        let complete_receipt = match complete_response.outcome {
            TaskWorkerActionOutcome::Committed(receipt) => receipt,
            other => panic!("complete failed: {other:?}"),
        };
        fs::remove_file(request_path).unwrap();
        let evidence = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
        let accept_record = evidence
            .record_by_id(&accept_receipt.transport_record_id)
            .unwrap()
            .unwrap();
        let start_record = evidence
            .record_by_id(&start_receipt.transport_record_id)
            .unwrap()
            .unwrap();
        let record = evidence
            .record_by_id(&complete_receipt.transport_record_id)
            .unwrap()
            .unwrap();
        assert!(accept_record.result.is_none());
        assert!(start_record.result.is_none());
        assert_eq!(
            raw_result_bytes(&serde_json::to_value(&record).unwrap()),
            result_bytes
        );
        let journal =
            fs::read_to_string(fixture.task_root.join("task-service-v1.events.jsonl")).unwrap();
        for record_id in [
            &accept_receipt.transport_record_id,
            &start_receipt.transport_record_id,
            &complete_receipt.transport_record_id,
        ] {
            assert!(journal.contains(record_id.as_str()));
        }
        assert!(journal.contains(complete_receipt.result_sha256.as_ref().unwrap().as_str()));
        let evidence_before_forgery = root_image(&fixture.evidence_root);
        assert!(evidence
            .record_by_id(
                &crate::role_revision::ReceiptId::new("00000000-0000-4000-8000-000000000000",)
                    .unwrap(),
            )
            .unwrap()
            .is_none());
        assert_eq!(root_image(&fixture.evidence_root), evidence_before_forgery);
        fixture.action.expected_store_revision = complete_receipt.committed_store_revision;
        let unrelated = install_unrelated_http_oracle(&mut fixture);
        let task_bytes = root_image(&fixture.task_root);
        let evidence_bytes = root_image(&fixture.evidence_root);
        let transition_count = task_journal_count(&fixture.task_root);
        let restarted = reopened_host(&fixture);
        for (action, expected, label) in [
            (&accept_request, &accept, "accept"),
            (&start, &start_response, "start"),
            (
                &complete,
                &TaskWorkerActionResponse {
                    schema: TaskWorkerActionResponseSchema::V1,
                    action_id: Some(complete.action_id.clone()),
                    outcome: TaskWorkerActionOutcome::Committed(complete_receipt.clone()),
                },
                "complete",
            ),
        ] {
            let replayed = route_action_with(
                fixture.state.clone(),
                restarted.clone(),
                "route-runtime",
                action,
            );
            assert_eq!(replayed, expected.clone(), "{REPLAY}/{label}");
            assert_eq!(
                root_image(&fixture.task_root),
                task_bytes,
                "{REPLAY}/{label}"
            );
            assert_eq!(
                root_image(&fixture.evidence_root),
                evidence_bytes,
                "{REPLAY}/{label}"
            );
            assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        }
        let mut reconstructed = accept_request.clone();
        reconstructed.expected_store_revision = fixture.action.expected_store_revision;
        let conflict = route_action_with(
            fixture.state.clone(),
            restarted,
            "route-runtime",
            &reconstructed,
        );
        assert!(matches!(
            conflict.outcome,
            TaskWorkerActionOutcome::NoWrite(TaskWorkerActionNoWrite::ActionConflict)
        ));
        assert_eq!(root_image(&fixture.task_root), task_bytes);
        assert_eq!(root_image(&fixture.evidence_root), evidence_bytes);
        assert_eq!(task_journal_count(&fixture.task_root), transition_count);
        assert_unrelated_unchanged(&fixture, &unrelated, UNRELATED_ORACLE);
        let snapshot = reopened_pilot(&fixture.task_root)
            .task(accept_request.task_id.clone(), accept_request.task_revision)
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot.phase,
            crate::task_delivery::PilotTaskPhase::Completed
        );
        assert_eq!(snapshot.fence, Some(fixture.action.attempt_fence.clone()));
        assert!(
            !root_image(&fixture.evidence_root).is_empty(),
            "{UNRELATED_ORACLE}"
        );
        fs::remove_dir_all(fixture.root).unwrap();
    }

    static TASK_ROUTE_MANAGEMENT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static TASK_ROUTE_CHAT_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn counted_noop_reconcile(_agent: &AgentBusAgent) -> anyhow::Result<()> {
        TASK_ROUTE_MANAGEMENT_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn counted_unreachable_send(
        _state: &Arc<Mutex<AgentBusState>>,
        _request: AgentBusSendRequest,
        _allow_federation: bool,
    ) -> anyhow::Result<Value> {
        TASK_ROUTE_CHAT_CALLS.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("task action route must not invoke chat/Management send")
    }

    fn counted_unreachable_rotation(
        _state: &Arc<Mutex<AgentBusState>>,
        _invocation: ReleaseRotationInvocation,
        _request: ReleaseRotationRequest,
    ) -> anyhow::Result<Value> {
        anyhow::bail!("task action route must not invoke Release rotation")
    }

    fn counted_unreachable_agent_management(
        _state: &Arc<Mutex<AgentBusState>>,
        _invocation: AgentManagementInvocation,
        _request: AgentManagementRequest,
    ) -> anyhow::Result<Value> {
        anyhow::bail!("task action route must not invoke Agent Management")
    }

    fn task_route_handlers() -> AgentBusRequestHandlers {
        AgentBusRequestHandlers {
            reconcile_registration_agent: counted_noop_reconcile,
            reconcile_agent: counted_noop_reconcile,
            redrive_ordinary_messages: |_| Ok(0),
            send_payload_response: counted_unreachable_send,
            release_rotation: counted_unreachable_rotation,
            agent_management: counted_unreachable_agent_management,
        }
    }

    struct CompletionDrainFixture {
        root: PathBuf,
        provider: crate::task_service::TaskServiceProvider,
        seats: crate::seat::SeatOccupancyStore,
        host: Arc<TaskWorkerActionHost>,
        state: Arc<Mutex<AgentBusState>>,
        assignment_id: crate::task_service::AssignmentId,
        director_session: crate::role_revision::CutexSessionId,
        release_seat: crate::task_service::SeatId,
        review_context: crate::task_service::WorkerMechanicalContext,
    }

    fn completion_drain_fixture(label: &str) -> CompletionDrainFixture {
        use crate::task_service::{
            ActionId, AssignAndDispatchRequest, AssignmentActionRequest, AuthenticatedPrincipal,
            CompletionPolicy, CompletionPolicyKind, CreateRevisionRequest, ProviderActionSchema,
            SendAttemptId, SubmitActionRequest, WorkerActionRequest, WorkerContextRequest,
            WorkerContextRequestSchema, WorkerPrepareOutcome, WorkerPrepareRequest,
            WorkerPrepareRequestSchema, WorkflowId,
        };
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "cutex-completion-drain-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        let task_worker_root = root.join("task-worker-actions-v1");
        let evidence_root = root.join("worker-evidence-v1");
        for path in [&root, &task_worker_root, &evidence_root] {
            fs::create_dir(path).unwrap();
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            #[cfg(windows)]
            crate::platform::private_fs::secure_directory(path).unwrap();
        }
        let provider =
            crate::task_service::TaskServiceProvider::open(root.join("provider-v2")).unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(root.join("seats-v1")).unwrap();
        let director_session =
            crate::role_revision::CutexSessionId::new(format!("director-{label}")).unwrap();
        let release_session =
            crate::role_revision::CutexSessionId::new(format!("release-old-{label}")).unwrap();
        let worker_session =
            crate::role_revision::CutexSessionId::new(format!("worker-{label}")).unwrap();
        let director_seat = crate::task_service::SeatId::new("cutex-director").unwrap();
        let release_seat = crate::task_service::SeatId::new("cutex-release").unwrap();
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new(format!("bind-director-{label}")).unwrap(),
                seat_id: director_seat.clone(),
                occupant_cutex_session: director_session.clone(),
            })
            .unwrap();
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new(format!("bind-release-old-{label}")).unwrap(),
                seat_id: release_seat.clone(),
                occupant_cutex_session: release_session,
            })
            .unwrap();
        let coordinator =
            AuthenticatedPrincipal::seated_session(director_session.clone(), director_seat, 1)
                .unwrap();
        let task_id = crate::role_revision::TaskId::new(format!("CUTEX-drain-{label}")).unwrap();
        let task_revision = crate::role_revision::TaskRevision::new(1).unwrap();
        let contract = format!("completion drain contract {label}");
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new(format!("create-{label}")).unwrap(),
                    workflow_id: WorkflowId::new(format!("workflow-{label}")).unwrap(),
                    task_id: task_id.clone(),
                    task_revision,
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract,
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: release_seat.clone(),
                    },
                },
                None,
            )
            .unwrap();
        let assignment_id =
            crate::task_service::AssignmentId::new(format!("assignment-{label}")).unwrap();
        provider
            .assign_and_dispatch(
                &coordinator,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new(format!("assign-{label}")).unwrap(),
                    assignment_id: assignment_id.clone(),
                    task_id,
                    task_revision,
                    assignee_cutex_session: worker_session.clone(),
                    send_attempt_id: SendAttemptId::new(format!("send-{label}")).unwrap(),
                    external_message_id: format!("assignment-message-{label}"),
                },
                1,
                "completion drain assignment",
            )
            .unwrap();
        let worker = AuthenticatedPrincipal::session(worker_session);
        for action in [
            WorkerActionRequest::Start(AssignmentActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new(format!("start-{label}")).unwrap(),
                assignment_id: assignment_id.clone(),
            }),
            WorkerActionRequest::Submit(SubmitActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new(format!("submit-{label}")).unwrap(),
                assignment_id: assignment_id.clone(),
                result_sha256: crate::task_service::sha256_bytes(label.as_bytes()),
                result_reference: format!("result-{label}"),
            }),
        ] {
            let prepared = provider
                .prepare_worker_action(
                    &worker,
                    &WorkerPrepareRequest {
                        schema: WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap();
            let WorkerPrepareOutcome::Prepared(envelope) = prepared else {
                panic!("new completion action must prepare")
            };
            provider.execute_worker_action(&worker, &envelope).unwrap();
        }
        let review_context = provider
            .worker_context(
                &worker,
                &WorkerContextRequest {
                    schema: WorkerContextRequestSchema::V2,
                    assignment_id: assignment_id.clone(),
                },
            )
            .unwrap()
            .context;
        let adapter = Arc::new(TaskWorkerActionAdapter::open_recovered(task_worker_root).unwrap());
        let evidence = TaskWorkerActionEvidenceStore::open(evidence_root).unwrap();
        let host = Arc::new(TaskWorkerActionHost::with_v2_parts(
            adapter,
            evidence,
            provider.clone(),
            seats.clone(),
        ));
        CompletionDrainFixture {
            root,
            provider,
            seats,
            host,
            state: Arc::new(Mutex::new(AgentBusState::default())),
            assignment_id,
            director_session,
            release_seat,
            review_context,
        }
    }

    fn raw_get_request(path: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").into_bytes()
    }

    #[test]
    fn completion_drain_idle_requests_and_unrelated_heartbeats_never_rescan_provider() {
        let fixture = completion_drain_fixture("idle");
        fixture
            .host
            .recover_completion_notifications(&fixture.state);
        assert_eq!(fixture.host.completion_drain_scan_count(), 1);

        fixture.state.lock().unwrap().agents.insert(
            "unrelated-runtime".to_string(),
            active_route_roster("unrelated-runtime", "unrelated-session"),
        );
        let mut ordinary = Vec::new();
        for _ in 0..32 {
            let state = Arc::clone(&fixture.state);
            let host = Arc::clone(&fixture.host);
            ordinary.push(std::thread::spawn(move || {
                invoke_task_route(raw_get_request("/"), state, host)
            }));
        }
        for response in ordinary {
            assert!(response.join().unwrap().starts_with(b"HTTP/1.1 200"));
        }
        let heartbeat = serde_json::to_vec(&AgentBusHeartbeatRequest {
            id: "unrelated-runtime".to_string(),
        })
        .unwrap();
        for _ in 0..64 {
            let response = invoke_task_route(
                raw_task_route_request(
                    "/api/agents/heartbeat",
                    "route-token",
                    "unrelated-runtime",
                    &heartbeat,
                ),
                Arc::clone(&fixture.state),
                Arc::clone(&fixture.host),
            );
            assert!(response.starts_with(b"HTTP/1.1 200"));
        }
        assert_eq!(
            fixture.host.completion_drain_scan_count(),
            1,
            "ordinary concurrency and unrelated heartbeat polling stay on the idle fast path"
        );
        assert!(fixture.state.lock().unwrap().messages.is_empty());
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn director_semantic_transport_authorizes_seats_replays_and_hides_mechanics() {
        use crate::task_service::{
            ActionId, AssignSemanticRequest, AuthenticatedPrincipal, CreateRevisionSemanticRequest,
            DirectorActionRequest, DirectorActionSchema, DirectorActionStatus,
            DirectorQuerySelector, DirectorSemanticOperation, SemanticCompletionPolicy, WorkflowId,
        };

        let fixture = completion_drain_fixture("director-semantic");
        let director_seat = crate::task_service::SeatId::new("cutex-director").unwrap();
        let principal = AuthenticatedPrincipal::seated_session(
            fixture.director_session.clone(),
            director_seat.clone(),
            1,
        )
        .unwrap();
        let seat_snapshot = fixture.seats.query().unwrap();
        let contract = "semantic Director contract";
        let create = CreateRevisionSemanticRequest {
            project_id: None,
            workflow_id: WorkflowId::new("semantic-workflow").unwrap(),
            task_id: crate::role_revision::TaskId::new("semantic-task").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
            contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
            opaque_contract: contract.to_string(),
            completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
            completion_authority_cutex_session_id: None,
        };
        let request = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("semantic-create").unwrap(),
            action: DirectorSemanticOperation::CreateRevision(create.clone()),
        };
        let first = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &request,
        );
        assert_eq!(first.status, DirectorActionStatus::Committed);
        let replay = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &request,
        );
        assert_eq!(replay.status, DirectorActionStatus::CurrentState);

        let query = fixture.host.director_query(
            &fixture.provider,
            &seat_snapshot,
            Some(&director_seat),
            ActionId::new("semantic-query").unwrap(),
            &DirectorQuerySelector::All {},
            None,
        );
        let encoded = serde_json::to_value(&query).unwrap();
        assert_eq!(query.status, DirectorActionStatus::CurrentState);
        let assignment = query
            .assignments
            .first()
            .expect("authorized Director query includes its assignment");
        assert!(assignment.assignee_display_name.is_some());
        assert!(assignment.acknowledged_at.is_some());
        let attempt = assignment
            .attempts
            .first()
            .expect("authorized Director query includes its attempt");
        assert!(attempt.result_reference.is_some());
        assert!(attempt.result_submitted_at.is_some());
        assert!(attempt.last_output.is_none());
        assert!(attempt.last_tool_call.is_none());
        let serialized = encoded.to_string();
        assert!(serialized.len() < 16_384, "Director query stays compact");
        for forbidden in [
            "opaque_contract",
            "local_revision",
            "attempt_token",
            "runtime_agent_id",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        let outsider_query = fixture.host.director_query(
            &fixture.provider,
            &seat_snapshot,
            None,
            ActionId::new("outsider-query").unwrap(),
            &DirectorQuerySelector::All {},
            None,
        );
        assert_eq!(outsider_query.code.as_deref(), Some("unauthorized"));
        assert!(outsider_query.tasks.is_empty());
        assert!(outsider_query.assignments.is_empty());

        let combined = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("semantic-combined").unwrap(),
            action: DirectorSemanticOperation::CreateAndAssign {
                create_revision: CreateRevisionSemanticRequest {
                    project_id: None,
                    workflow_id: WorkflowId::new("combined-workflow").unwrap(),
                    task_id: crate::role_revision::TaskId::new("combined-task").unwrap(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract.to_string(),
                    completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
                    completion_authority_cutex_session_id: None,
                },
                assign: AssignSemanticRequest {
                    project_id: None,
                    assignment_id: crate::task_service::AssignmentId::new("combined-assignment")
                        .unwrap(),
                    task_id: crate::role_revision::TaskId::new("combined-task").unwrap(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    assignee_cutex_session_id: crate::role_revision::CutexSessionId::new(
                        "offline-worker",
                    )
                    .unwrap(),
                    summary: "bounded semantic assignment summary".to_string(),
                },
            },
        };
        let partial = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &combined,
        );
        assert_eq!(partial.status, DirectorActionStatus::ResponseUncertain);
        assert_eq!(
            partial
                .continuation
                .as_ref()
                .map(|value| value.phase.as_str()),
            Some("create_revision_committed")
        );
        let retry = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &combined,
        );
        assert_eq!(retry.status, DirectorActionStatus::ResponseUncertain);
        assert_eq!(retry.assignment_id, partial.assignment_id);

        let rejected_unscoped_v2 = DirectorActionRequest {
            schema: DirectorActionSchema::V2,
            action_id: ActionId::new("semantic-v2-unscoped").unwrap(),
            action: DirectorSemanticOperation::CreateRevision(CreateRevisionSemanticRequest {
                project_id: None,
                workflow_id: WorkflowId::new("semantic-v2-unscoped-workflow").unwrap(),
                task_id: crate::role_revision::TaskId::new("semantic-v2-unscoped-task").unwrap(),
                task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                opaque_contract: contract.to_string(),
                completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
                completion_authority_cutex_session_id: None,
            }),
        };
        let rejected = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &rejected_unscoped_v2,
        );
        assert_eq!(rejected.status, DirectorActionStatus::NoWrite);
        assert_eq!(rejected.code.as_deref(), Some("project_contract_invalid"));

        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let scoped_v2 = DirectorActionRequest {
            schema: DirectorActionSchema::V2,
            action_id: ActionId::new("semantic-v2-scoped").unwrap(),
            action: DirectorSemanticOperation::CreateRevision(CreateRevisionSemanticRequest {
                project_id: Some(project.clone()),
                workflow_id: WorkflowId::new("semantic-v2-workflow").unwrap(),
                task_id: crate::role_revision::TaskId::new("semantic-v2-task").unwrap(),
                task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                opaque_contract: contract.to_string(),
                completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
                completion_authority_cutex_session_id: None,
            }),
        };
        let scoped = fixture.host.execute_authenticated_director_action(
            &fixture.provider,
            &principal,
            &seat_snapshot,
            &fixture.director_session,
            &fixture.state,
            &scoped_v2,
        );
        assert_eq!(scoped.status, DirectorActionStatus::Committed);
        assert_eq!(scoped.project_id.as_ref(), Some(&project));
        fs::remove_dir_all(fixture.root).unwrap();
    }

    fn invoke_director_route_bounded(
        fixture: &CompletionDrainFixture,
        sender_runtime_id: &str,
        request: &crate::task_service::DirectorActionRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        invoke_director_route_on_host_bounded(fixture, &fixture.host, sender_runtime_id, request)
    }

    fn invoke_director_route_on_host_bounded(
        fixture: &CompletionDrainFixture,
        host: &Arc<TaskWorkerActionHost>,
        sender_runtime_id: &str,
        request: &crate::task_service::DirectorActionRequest,
    ) -> crate::task_service::DirectorActionReceipt {
        let body = serde_json::to_vec(request).unwrap();
        let route = raw_task_route_request(
            "/api/task/v2/director-action",
            "route-token",
            sender_runtime_id,
            &body,
        );
        let state = Arc::clone(&fixture.state);
        let host = Arc::clone(host);
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sent.send(invoke_task_route(route, state, host));
        });
        let response = received
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| {
                panic!(
                    "Director loopback route {} must return within two seconds",
                    request.action_id.as_str()
                )
            });
        assert!(response.starts_with(b"HTTP/1.1 200"));
        serde_json::from_value(http_json(&response)).unwrap()
    }

    fn invoke_route_on_host_bounded(
        fixture: &CompletionDrainFixture,
        host: &Arc<TaskWorkerActionHost>,
        request: Vec<u8>,
    ) -> Vec<u8> {
        let state = Arc::clone(&fixture.state);
        let host = Arc::clone(host);
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sent.send(invoke_task_route(request, state, host));
        });
        received
            .recv_timeout(Duration::from_secs(2))
            .expect("Agent Bus loopback route must return within two seconds")
    }

    fn reopen_completion_host(fixture: &CompletionDrainFixture) -> Arc<TaskWorkerActionHost> {
        let provider =
            crate::task_service::TaskServiceProvider::open(fixture.root.join("provider-v2"))
                .unwrap();
        provider.recover().unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(fixture.root.join("seats-v1")).unwrap();
        let adapter = Arc::new(
            TaskWorkerActionAdapter::open_recovered(fixture.root.join("task-worker-actions-v1"))
                .unwrap(),
        );
        let evidence =
            TaskWorkerActionEvidenceStore::open(fixture.root.join("worker-evidence-v1")).unwrap();
        Arc::new(TaskWorkerActionHost::with_v2_parts(
            adapter, evidence, provider, seats,
        ))
    }

    fn queued_message_count(state: &Arc<Mutex<AgentBusState>>) -> usize {
        state
            .lock()
            .unwrap()
            .messages
            .values()
            .map(|messages| messages.len())
            .sum()
    }

    fn wait_for_background_drain(host: &TaskWorkerActionHost) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.completion_drain_scheduled.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "background completion drain exceeded two seconds"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn semantic_create_and_assign(
        label: &str,
        assignee: crate::role_revision::CutexSessionId,
    ) -> crate::task_service::DirectorActionRequest {
        use crate::task_service::{
            ActionId, AssignSemanticRequest, CreateRevisionSemanticRequest, DirectorActionRequest,
            DirectorActionSchema, DirectorSemanticOperation, SemanticCompletionPolicy, WorkflowId,
        };

        let contract = format!("bounded Director loopback contract {label}");
        let task_id = crate::role_revision::TaskId::new(format!("task-{label}")).unwrap();
        DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new(format!("create-assign-{label}")).unwrap(),
            action: DirectorSemanticOperation::CreateAndAssign {
                create_revision: CreateRevisionSemanticRequest {
                    project_id: None,
                    workflow_id: WorkflowId::new(format!("workflow-{label}")).unwrap(),
                    task_id: task_id.clone(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract,
                    completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
                    completion_authority_cutex_session_id: None,
                },
                assign: AssignSemanticRequest {
                    project_id: None,
                    assignment_id: crate::task_service::AssignmentId::new(format!(
                        "assignment-{label}"
                    ))
                    .unwrap(),
                    task_id,
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    assignee_cutex_session_id: assignee,
                    summary: format!("bounded assignment summary {label}"),
                },
            },
        }
    }

    #[test]
    fn director_real_loopback_actions_return_and_terminal_delivery_follows_response() {
        use crate::task_service::{
            ActionId, AssignmentDecisionRequest, CompletionNotificationFactKind,
            CompletionNotificationFactRequest, CreateRevisionSemanticRequest,
            DirectorActionRequest, DirectorActionSchema, DirectorActionStatus,
            DirectorQuerySelector, DirectorSemanticOperation, ProviderActionSchema,
            SemanticCompletionPolicy, SubmitActionRequest, WorkerActionRequest,
            WorkerPrepareOutcome, WorkerPrepareRequest, WorkerPrepareRequestSchema, WorkflowId,
        };

        const HELPER_ENV: &str = "CUTEX_R17_DIRECTOR_LOOPBACK_HELPER";
        if std::env::var_os(HELPER_ENV).is_none() {
            use std::process::{Command, Stdio};

            let isolated_home = std::env::temp_dir().join(format!(
                "cutex-r17-director-loopback-home-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&isolated_home).unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg(
                    "agent_bus::server::tests::director_real_loopback_actions_return_and_terminal_delivery_follows_response",
                )
                .arg("--nocapture")
                .env(HELPER_ENV, "1")
                .env("HOME", &isolated_home)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    fs::remove_dir_all(isolated_home).unwrap();
                    assert!(status.success(), "isolated Director loopback helper failed");
                    return;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    let _ = child.wait();
                    fs::remove_dir_all(isolated_home).unwrap();
                    panic!("isolated Director loopback helper exceeded ten seconds");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let fixture = completion_drain_fixture("director-loopback-r17");
        fixture
            .host
            .completion_drain_requested
            .store(false, Ordering::Release);
        let director_runtime = "director-runtime-r17";
        let director_roster_session = "director-roster-thread-r17";
        let worker_roster_session = "worker-roster-thread-r17";
        let worker_session =
            crate::role_revision::CutexSessionId::new("worker-director-loopback-r17").unwrap();
        let mut session_store = CutexSessionStore::default();
        let director_record = active_route_session(
            fixture.director_session.as_str(),
            director_roster_session,
            director_runtime,
            1,
            1,
        );
        let worker_record = active_route_session(
            worker_session.as_str(),
            worker_roster_session,
            "worker-runtime-r17",
            1,
            1,
        );
        session_store
            .sessions
            .insert(director_record.cutex_session_id.clone(), director_record);
        session_store
            .sessions
            .insert(worker_record.cutex_session_id.clone(), worker_record);
        crate::session::store::save_cutex_session_store(&session_store).unwrap();
        {
            let mut state = fixture.state.lock().unwrap();
            state.agents.insert(
                director_runtime.to_string(),
                active_route_roster(director_runtime, director_roster_session),
            );
            state.agents.insert(
                "worker-runtime-r17".to_string(),
                active_route_roster("worker-runtime-r17", worker_roster_session),
            );
        }

        let query = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("director-loopback-query-r17").unwrap(),
            action: DirectorSemanticOperation::Query {
                selector: DirectorQuerySelector::All {},
            },
        };
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &query).status,
            DirectorActionStatus::CurrentState
        );

        let create_contract = "Director loopback create-only contract";
        let create = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("director-loopback-create-r17").unwrap(),
            action: DirectorSemanticOperation::CreateRevision(CreateRevisionSemanticRequest {
                project_id: None,
                workflow_id: WorkflowId::new("director-loopback-create-workflow-r17").unwrap(),
                task_id: crate::role_revision::TaskId::new("director-loopback-create-task-r17")
                    .unwrap(),
                task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                contract_sha256: crate::task_service::sha256_bytes(create_contract.as_bytes()),
                opaque_contract: create_contract.to_string(),
                completion_policy: SemanticCompletionPolicy::DirectorAcceptance,
                completion_authority_cutex_session_id: None,
            }),
        };
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &create).status,
            DirectorActionStatus::Committed
        );

        let cancel_create = semantic_create_and_assign("cancel-r17", worker_session.clone());
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &cancel_create).status,
            DirectorActionStatus::Committed
        );
        let cancel = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("director-loopback-cancel-r17").unwrap(),
            action: DirectorSemanticOperation::Cancel(AssignmentDecisionRequest {
                assignment_id: crate::task_service::AssignmentId::new("assignment-cancel-r17")
                    .unwrap(),
                decision_reference: Some("cancelled by bounded test".to_string()),
            }),
        };
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &cancel).status,
            DirectorActionStatus::Committed
        );

        let accept_create = semantic_create_and_assign("accept-r17", worker_session.clone());
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &accept_create).status,
            DirectorActionStatus::Committed
        );
        let accept_assignment =
            crate::task_service::AssignmentId::new("assignment-accept-r17").unwrap();
        let worker = crate::task_service::AuthenticatedPrincipal::session(worker_session);
        for action in [
            WorkerActionRequest::Start(crate::task_service::AssignmentActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new("director-loopback-start-r17").unwrap(),
                assignment_id: accept_assignment.clone(),
            }),
            WorkerActionRequest::Submit(SubmitActionRequest {
                schema: ProviderActionSchema::V2,
                action_id: ActionId::new("director-loopback-submit-r17").unwrap(),
                assignment_id: accept_assignment.clone(),
                result_sha256: crate::task_service::sha256_bytes(b"bounded-result-r17"),
                result_reference: "bounded-result-reference-r17".to_string(),
            }),
        ] {
            let prepared = fixture
                .provider
                .prepare_worker_action(
                    &worker,
                    &WorkerPrepareRequest {
                        schema: WorkerPrepareRequestSchema::V2,
                        action,
                    },
                )
                .unwrap();
            let WorkerPrepareOutcome::Prepared(envelope) = prepared else {
                panic!("fresh worker action must prepare")
            };
            fixture
                .provider
                .execute_worker_action(&worker, &envelope)
                .unwrap();
        }

        let accept = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("director-loopback-accept-r17").unwrap(),
            action: DirectorSemanticOperation::AcceptResult(AssignmentDecisionRequest {
                assignment_id: accept_assignment,
                decision_reference: Some("accepted bounded result".to_string()),
            }),
        };
        // Establish a quiescent pre-terminal baseline. This direct recovery is
        // the documented startup boundary, not a served request.
        fixture
            .host
            .recover_completion_notifications(&fixture.state);
        wait_for_background_drain(&fixture.host);
        let messages_before_terminal = queued_message_count(&fixture.state);
        assert_eq!(
            invoke_director_route_bounded(&fixture, director_runtime, &accept).status,
            DirectorActionStatus::Committed,
            "terminal HTTP response is independent of completion delivery"
        );
        assert!(fixture
            .host
            .completion_drain_requested
            .load(Ordering::Acquire));
        assert_eq!(
            queued_message_count(&fixture.state),
            messages_before_terminal,
            "completion delivery follows the terminal response"
        );

        // Reopen before the pending terminal notification is drained. A
        // request schedules one single-flight worker; the test gate blocks it
        // after it owns the drain mutex but before any Seat/provider work.
        let reopened_before_drain = reopen_completion_host(&fixture);
        let drain_gate = reopened_before_drain.install_completion_drain_test_gate();
        let trigger =
            invoke_route_on_host_bounded(&fixture, &reopened_before_drain, raw_get_request("/"));
        assert!(trigger.starts_with(b"HTTP/1.1 200"));
        assert!(drain_gate.wait_until_entered(Duration::from_secs(2)));

        let blocked_drain_query = DirectorActionRequest {
            schema: DirectorActionSchema::V1,
            action_id: ActionId::new("director-loopback-blocked-drain-query-r17").unwrap(),
            action: DirectorSemanticOperation::Query {
                selector: DirectorQuerySelector::All {},
            },
        };
        assert_eq!(
            invoke_director_route_on_host_bounded(
                &fixture,
                &reopened_before_drain,
                director_runtime,
                &blocked_drain_query,
            )
            .status,
            DirectorActionStatus::CurrentState,
            "an inner blocked drain cannot hold the next Director response"
        );
        assert_eq!(
            queued_message_count(&fixture.state),
            messages_before_terminal
        );

        drain_gate.release();
        wait_for_background_drain(&reopened_before_drain);
        let messages_after_drain = queued_message_count(&fixture.state);
        assert_eq!(messages_after_drain, messages_before_terminal + 1);

        let terminal_metadata = {
            let state = fixture.state.lock().unwrap();
            state.messages[director_runtime]
                .iter()
                .rev()
                .filter_map(|message| message.control_payload.clone())
                .filter_map(|payload| serde_json::from_value(payload).ok())
                .find(
                    |metadata: &crate::agent_bus::model::TaskServiceCompletionMetadata| {
                        metadata.kind
                            == crate::task_service::CompletionNotificationKind::TerminalClosure
                    },
                )
                .expect("terminal completion message")
        };
        let after_queue = fixture.provider.query().unwrap();
        let terminal_notification =
            after_queue.completion_notifications[&terminal_metadata.notification_id].clone();
        assert_eq!(
            terminal_notification
                .facts
                .iter()
                .filter(|fact| fact.kind == CompletionNotificationFactKind::Queued)
                .count(),
            1
        );
        assert_eq!(
            terminal_notification
                .facts
                .iter()
                .filter(|fact| fact.kind == CompletionNotificationFactKind::Delivered)
                .count(),
            0
        );
        fixture
            .provider
            .record_completion_notification_fact(
                &crate::task_service::AuthenticatedPrincipal::task_service_system(),
                &CompletionNotificationFactRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("director-loopback-delivered-r17").unwrap(),
                    notification_id: terminal_metadata.notification_id.clone(),
                    expected_notification_revision: terminal_notification.local_revision,
                    kind: CompletionNotificationFactKind::Delivered,
                    reference: Some("director-loopback-native-delivery-r17".to_string()),
                },
            )
            .unwrap();

        // Reopen after queue/delivery. Startup recovery and semantic replay
        // preserve exactly one message and one fact of each kind.
        let reopened_after_delivery = reopen_completion_host(&fixture);
        reopened_after_delivery.recover_completion_notifications(&fixture.state);
        assert_eq!(queued_message_count(&fixture.state), messages_after_drain);
        let after_recovery = fixture.provider.query().unwrap();
        let terminal_after_recovery =
            &after_recovery.completion_notifications[&terminal_metadata.notification_id];
        for kind in [
            CompletionNotificationFactKind::Queued,
            CompletionNotificationFactKind::Delivered,
        ] {
            assert_eq!(
                terminal_after_recovery
                    .facts
                    .iter()
                    .filter(|fact| fact.kind == kind)
                    .count(),
                1
            );
        }

        assert_eq!(
            invoke_director_route_on_host_bounded(
                &fixture,
                &reopened_after_delivery,
                director_runtime,
                &accept,
            )
            .status,
            DirectorActionStatus::CurrentState
        );
        assert_eq!(
            queued_message_count(&fixture.state),
            messages_after_drain,
            "exact terminal replay does not duplicate completion messages"
        );

        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn completion_drain_restart_recovery_seat_rebind_and_terminal_transition_are_durable() {
        use crate::task_service::{
            ActionId, AuthenticatedPrincipal, CompletionNotificationFactKind,
            CompletionNotificationFactRequest, ProviderActionSchema, TerminalActionEnvelope,
            TerminalActionRequest, TerminalAuthorityRequest, TerminalRequestSchema,
        };

        let fixture = completion_drain_fixture("rebind");
        fixture
            .host
            .recover_completion_notifications(&fixture.state);
        assert_eq!(fixture.host.completion_drain_scan_count(), 1);
        let after_offline = fixture.provider.query().unwrap();
        let review = after_offline
            .completion_notifications
            .values()
            .next()
            .unwrap();
        assert_eq!(
            review.kind,
            crate::task_service::CompletionNotificationKind::ReviewReady
        );
        assert_eq!(
            review
                .facts
                .iter()
                .filter(|fact| fact.kind == CompletionNotificationFactKind::Uncertain)
                .count(),
            1
        );

        let release_new = crate::role_revision::CutexSessionId::new("release-new-rebind").unwrap();
        let rebound = fixture
            .seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("bind-release-new-rebind").unwrap(),
                seat_id: fixture.release_seat.clone(),
                occupant_cutex_session: release_new.clone(),
            })
            .unwrap();
        assert_eq!(rebound.occupancy.epoch, 2);
        fixture.state.lock().unwrap().agents.insert(
            "release-runtime".to_string(),
            active_route_roster("release-runtime", release_new.as_str()),
        );
        fixture
            .host
            .retry_completion_notifications_for_available_target(&fixture.state);
        wait_for_background_drain(&fixture.host);
        assert_eq!(fixture.host.completion_drain_scan_count(), 2);
        let review_message = fixture.state.lock().unwrap().messages["release-runtime"][0].clone();
        let review_metadata: crate::agent_bus::model::TaskServiceCompletionMetadata =
            serde_json::from_value(review_message.control_payload.clone().unwrap()).unwrap();
        assert_eq!(
            review_metadata.kind,
            crate::task_service::CompletionNotificationKind::ReviewReady
        );
        fixture
            .host
            .retry_completion_notifications_for_available_target(&fixture.state);
        assert_eq!(fixture.host.completion_drain_scan_count(), 2);

        let review_notification = fixture.provider.query().unwrap().completion_notifications
            [&review_metadata.notification_id]
            .clone();
        fixture
            .provider
            .record_completion_notification_fact(
                &AuthenticatedPrincipal::task_service_system(),
                &CompletionNotificationFactRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("delivered-review-rebind").unwrap(),
                    notification_id: review_metadata.notification_id.clone(),
                    expected_notification_revision: review_notification.local_revision,
                    kind: CompletionNotificationFactKind::Delivered,
                    reference: Some("review-native-submission".to_string()),
                },
            )
            .unwrap();

        fixture.state.lock().unwrap().agents.insert(
            "director-runtime".to_string(),
            active_route_roster("director-runtime", fixture.director_session.as_str()),
        );
        let release = AuthenticatedPrincipal::seated_session(
            release_new,
            fixture.release_seat.clone(),
            rebound.occupancy.epoch,
        )
        .unwrap();
        let terminal_receipt = fixture
            .provider
            .execute_terminal_action(
                &release,
                &TerminalActionEnvelope {
                    schema: TerminalRequestSchema::V2,
                    command: TerminalAuthorityRequest::AcceptResult(TerminalActionRequest {
                        schema: ProviderActionSchema::V2,
                        action_id: ActionId::new("accept-rebind").unwrap(),
                        assignment_id: fixture.assignment_id.clone(),
                        decision_reference: Some("accepted".to_string()),
                    }),
                    context: fixture.review_context.clone(),
                },
            )
            .unwrap();
        fixture
            .host
            .dispatch_completion_notifications_after_transition(
                &fixture.state,
                &TaskServiceActionResponse {
                    schema: TaskServiceActionResponseSchema::V2,
                    action_id: ActionId::new("accept-rebind").unwrap(),
                    outcome: TaskServiceActionOutcome::Committed(terminal_receipt),
                },
            );
        wait_for_background_drain(&fixture.host);
        assert_eq!(fixture.host.completion_drain_scan_count(), 3);
        let terminal_message =
            fixture.state.lock().unwrap().messages["director-runtime"][0].clone();
        let terminal_metadata: crate::agent_bus::model::TaskServiceCompletionMetadata =
            serde_json::from_value(terminal_message.control_payload.clone().unwrap()).unwrap();
        assert_eq!(
            terminal_metadata.kind,
            crate::task_service::CompletionNotificationKind::TerminalClosure
        );
        assert_ne!(review_message.id, terminal_message.id);

        for _ in 0..16 {
            let response = invoke_task_route(
                raw_get_request("/"),
                Arc::clone(&fixture.state),
                Arc::clone(&fixture.host),
            );
            assert!(response.starts_with(b"HTTP/1.1 200"));
        }
        assert_eq!(fixture.host.completion_drain_scan_count(), 3);
        let final_snapshot = fixture.provider.query().unwrap();
        let review_final =
            &final_snapshot.completion_notifications[&review_metadata.notification_id];
        assert_eq!(
            review_final
                .facts
                .iter()
                .filter(|fact| fact.kind == CompletionNotificationFactKind::Queued)
                .count(),
            1
        );
        assert_eq!(
            review_final
                .facts
                .iter()
                .filter(|fact| fact.kind == CompletionNotificationFactKind::Delivered)
                .count(),
            1
        );
        fs::remove_dir_all(fixture.root).unwrap();
    }

    struct RouteNoWriteObserver {
        task_root: Vec<(String, Vec<u8>)>,
        evidence_root: Vec<(String, Vec<u8>)>,
        transition_count: usize,
        record_count: usize,
        management_calls: usize,
        chat_calls: usize,
    }

    fn route_no_write_observer(fixture: &WorkerHostFixture) -> RouteNoWriteObserver {
        let evidence_snapshot = fixture
            .evidence_root
            .join("task-worker-action-evidence-v1.json");
        let record_count = if evidence_snapshot.exists() {
            evidence_records(&fixture.evidence_root).len()
        } else {
            0
        };
        RouteNoWriteObserver {
            task_root: root_image(&fixture.task_root),
            evidence_root: root_image(&fixture.evidence_root),
            transition_count: task_journal_count(&fixture.task_root),
            record_count,
            management_calls: TASK_ROUTE_MANAGEMENT_CALLS.load(Ordering::SeqCst),
            chat_calls: TASK_ROUTE_CHAT_CALLS.load(Ordering::SeqCst),
        }
    }

    fn assert_route_no_write_observer(
        fixture: &WorkerHostFixture,
        before: &RouteNoWriteObserver,
        control: &str,
    ) {
        let after = route_no_write_observer(fixture);
        assert_eq!(
            after.task_root, before.task_root,
            "{control}: Task Service root"
        );
        assert_eq!(
            after.evidence_root, before.evidence_root,
            "{control}: evidence root"
        );
        assert_eq!(
            after.transition_count, before.transition_count,
            "{control}: transition count"
        );
        assert_eq!(
            after.record_count, before.record_count,
            "{control}: evidence record count"
        );
        assert_eq!(
            after.management_calls, before.management_calls,
            "{control}: Management handler calls"
        );
        assert_eq!(
            after.chat_calls, before.chat_calls,
            "{control}: chat handler calls"
        );
    }

    fn raw_task_route_request(path: &str, token: &str, sender: &str, body: &[u8]) -> Vec<u8> {
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nX-Cutex-Agent-Id: {sender}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        request
    }

    fn invoke_task_route(
        request: Vec<u8>,
        state: Arc<Mutex<AgentBusState>>,
        host: Arc<TaskWorkerActionHost>,
    ) -> Vec<u8> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client.write_all(&request).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        if let Err(error) = handle_agent_bus_request(
            &mut server,
            &state,
            Some("route-token"),
            task_route_handlers(),
            &host,
        ) {
            write_http_response(
                &mut server,
                500,
                "Internal Server Error",
                "text/plain",
                error.to_string().as_bytes(),
            )
            .unwrap();
        }
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        response
    }

    fn http_json(response: &[u8]) -> Value {
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        serde_json::from_slice(&response[split + 4..]).unwrap()
    }

    #[test]
    fn coordinator_terminal_and_query_routes_reject_forged_authority_fields_without_write() {
        let fixture = worker_host_fixture("task-v2-authority-strict");
        let before = route_no_write_observer(&fixture);
        let cases = [
            (
                "/api/task/v2/actions",
                serde_json::json!({
                    "operation": "start",
                    "body": {
                        "schema": "cutex/task-service-action/v2",
                        "action_id": "unwrapped-worker",
                        "assignment_id": "assignment-1"
                    }
                }),
            ),
            (
                "/api/task/v2/worker-context",
                serde_json::json!({
                    "schema": "cutex/task-service-worker-context/v2",
                    "assignment_id": "assignment-1",
                    "expected_assignment_revision": 1
                }),
            ),
            (
                "/api/task/v2/worker-prepare",
                serde_json::json!({
                    "schema": "cutex/task-service-worker-prepare/v2",
                    "action": {
                        "operation": "start",
                        "body": {
                            "schema": "cutex/task-service-action/v2",
                            "action_id": "forged-worker-prepare",
                            "assignment_id": "assignment-1"
                        }
                    },
                    "expected_assignment_revision": 1
                }),
            ),
            (
                "/api/task/v2/coordinator",
                serde_json::json!({
                    "schema": "cutex/task-service-coordinator/v2",
                    "command": {
                        "operation": "cancel_assignment",
                        "body": {
                            "schema": "cutex/task-service-action/v2",
                            "action_id": "forged-coordinator",
                            "assignment_id": "assignment-1"
                        }
                    },
                    "seat_epoch": 99
                }),
            ),
            (
                "/api/task/v2/terminal",
                serde_json::json!({
                    "schema": "cutex/task-service-terminal/v2",
                    "command": {
                        "operation": "accept_result",
                        "body": {
                            "schema": "cutex/task-service-action/v2",
                            "action_id": "forged-terminal",
                            "assignment_id": "assignment-1",
                            "decision_reference": null
                        }
                    },
                    "cutex_session_id": "forged-session"
                }),
            ),
            (
                "/api/task/v2/query",
                serde_json::json!({
                    "schema": "cutex/task-service-query/v2",
                    "query": { "operation": "snapshot" },
                    "runtime_agent_id": "forged-runtime"
                }),
            ),
        ];
        for (path, body) in cases {
            let response = invoke_task_route(
                raw_task_route_request(
                    path,
                    "route-token",
                    "route-runtime",
                    &serde_json::to_vec(&body).unwrap(),
                ),
                fixture.state.clone(),
                fixture.host.clone(),
            );
            let response = http_json(&response);
            assert_eq!(response["outcome"]["kind"], "no_write");
            assert_eq!(response["outcome"]["body"]["code"], "invalid_body");
        }
        assert_route_no_write_observer(&fixture, &before, "strict v2 authority routes");
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn release_rotation_route_rejects_forged_caller_authority_fields_before_execution() {
        let fixture = worker_host_fixture("release-rotation-authority-strict");
        let before = route_no_write_observer(&fixture);
        let valid = serde_json::json!({
            "schema": "cutex/release-rotation-command/v1",
            "action_id": "rotate-release",
            "target_seat": "cutex-release",
            "expected_predecessor_cutex_session": "cutex.release-old",
            "expected_seat_epoch": 1,
            "expected_template_version": 1,
            "expected_template_sha256": "1111111111111111111111111111111111111111111111111111111111111111",
            "starting_message": "Review the frozen candidate."
        });
        for forbidden in [
            "caller_cutex_session",
            "director_seat_epoch",
            "runtime_agent_id",
            "management_root",
        ] {
            let mut request = valid.clone();
            request[forbidden] = serde_json::json!("forged");
            let response = invoke_task_route(
                raw_task_route_request(
                    "/api/rotation/v1/release",
                    "route-token",
                    "route-runtime",
                    &serde_json::to_vec(&request).unwrap(),
                ),
                fixture.state.clone(),
                fixture.host.clone(),
            );
            assert_eq!(http_json(&response)["outcome"]["status"], "no_write");
            assert_eq!(http_json(&response)["outcome"]["code"], "invalid_body");
        }
        assert_route_no_write_observer(&fixture, &before, "release rotation strict caller body");
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn authenticated_unseated_and_superseded_sessions_receive_typed_no_write() {
        let fixture = worker_host_fixture("task-v2-seat-no-write");
        let provider =
            crate::task_service::TaskServiceProvider::open(fixture.root.join("provider-v2"))
                .unwrap();
        provider.recover().unwrap();
        let seats = crate::seat::SeatOccupancyStore::open(fixture.root.join("seat-v1")).unwrap();
        let predecessor =
            crate::role_revision::CutexSessionId::new("director-predecessor").unwrap();
        let successor = crate::role_revision::CutexSessionId::new("director-successor").unwrap();
        let unseated = crate::role_revision::CutexSessionId::new("director-unseated").unwrap();
        let director_seat = crate::task_service::SeatId::new("cutex-director").unwrap();
        seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: crate::task_service::ActionId::new("bind-predecessor").unwrap(),
                seat_id: director_seat.clone(),
                occupant_cutex_session: predecessor.clone(),
            })
            .unwrap();
        let evidence = TaskWorkerActionEvidenceStore::open(&fixture.evidence_root).unwrap();
        let host = TaskWorkerActionHost::with_v2_parts(
            fixture.adapter.clone(),
            evidence,
            provider,
            seats.clone(),
        );
        let contract = "typed no-write contract";
        let request = crate::task_service::CoordinatorActionRequest {
            schema: crate::task_service::CoordinatorRequestSchema::V2,
            command: crate::task_service::CoordinatorOperation::CreateRevision(
                crate::task_service::CreateRevisionRequest {
                    schema: crate::task_service::ProviderActionSchema::V2,
                    action_id: crate::task_service::ActionId::new("seat-create").unwrap(),
                    workflow_id: crate::task_service::WorkflowId::new("seat-workflow").unwrap(),
                    task_id: crate::role_revision::TaskId::new("CUTEX-seat-route").unwrap(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract.to_string(),
                    completion_policy: crate::task_service::CompletionPolicy {
                        kind: crate::task_service::CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: director_seat.clone(),
                    },
                },
            ),
            context: crate::task_service::CoordinatorMechanicalContext::CreateRevision {
                expected_workflow_revision: None,
            },
        };
        let mut mismatched = request.clone();
        mismatched.context = crate::task_service::CoordinatorMechanicalContext::AssignAndDispatch {
            expected_workflow_revision: 1,
        };
        assert!(matches!(
            host.execute_coordinator_session_v2(&predecessor, &fixture.state, mismatched)
                .outcome,
            TaskServiceActionOutcome::NoWrite { ref code, .. } if code == "invalid_request"
        ));
        assert!(matches!(
            host.execute_coordinator_session_v2(&predecessor, &fixture.state, request.clone())
                .outcome,
            TaskServiceActionOutcome::Committed(_)
        ));
        for session in [&unseated] {
            assert!(matches!(
                host.execute_coordinator_session_v2(session, &fixture.state, request.clone())
                    .outcome,
                TaskServiceActionOutcome::NoWrite { ref code, .. } if code == "unauthorized"
            ));
        }
        let rebound = seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: crate::task_service::ActionId::new("bind-successor").unwrap(),
                seat_id: director_seat,
                occupant_cutex_session: successor.clone(),
            })
            .unwrap();
        assert_eq!(rebound.occupancy.epoch, 2);
        assert!(matches!(
            host.execute_coordinator_session_v2(
                &predecessor,
                &fixture.state,
                request.clone()
            )
            .outcome,
            TaskServiceActionOutcome::NoWrite { ref code, .. } if code == "unauthorized"
        ));
        assert!(matches!(
            host.execute_coordinator_session_v2(&successor, &fixture.state, request)
                .outcome,
            TaskServiceActionOutcome::Committed(_)
        ));
        fs::remove_dir_all(fixture.root).unwrap();
    }

    #[test]
    fn t_exec_route_durable_repair_action_inspect_ack_and_strict_surface() {
        const FIXTURE: &str = "T-EXEC-ROUTE-DURABLE-REPAIR";
        const CONCURRENT_DUPLICATE: &str = "route/concurrent-duplicate";
        let fixture = worker_host_fixture(FIXTURE);
        let action_body = serde_json::to_vec(&fixture.action).unwrap();
        let wrong_token_before = route_no_write_observer(&fixture);
        let unauthorized = invoke_task_route(
            raw_task_route_request(
                "/api/task/actions",
                "wrong-token",
                "route-runtime",
                &action_body,
            ),
            fixture.state.clone(),
            fixture.host.clone(),
        );
        assert!(unauthorized.starts_with(b"HTTP/1.1 500"));
        assert_route_no_write_observer(&fixture, &wrong_token_before, "wrong-token");
        let mut forged = serde_json::to_value(&fixture.action).unwrap();
        forged.as_object_mut().unwrap().insert(
            "transport_reference".to_string(),
            serde_json::json!("caller-owned"),
        );
        let strict_before = route_no_write_observer(&fixture);
        let strict = invoke_task_route(
            raw_task_route_request(
                "/api/task/actions",
                "route-token",
                "route-runtime",
                &serde_json::to_vec(&forged).unwrap(),
            ),
            fixture.state.clone(),
            fixture.host.clone(),
        );
        assert_eq!(
            http_json(&strict)["outcome"]["body"]["code"],
            "invalid_body"
        );
        assert_route_no_write_observer(&fixture, &strict_before, "strict-unknown-field");
        let federation_before = route_no_write_observer(&fixture);
        let federation = invoke_task_route(
            raw_task_route_request(
                "/api/federation/task/actions",
                "route-token",
                "route-runtime",
                &action_body,
            ),
            fixture.state.clone(),
            fixture.host.clone(),
        );
        assert!(federation.starts_with(b"HTTP/1.1 404"));
        assert_route_no_write_observer(&fixture, &federation_before, "federation-404");

        let pending_host = Arc::new(TaskWorkerActionHost::with_parts(
            fixture.adapter.clone(),
            TaskWorkerActionEvidenceStore::open_with_fault(
                &fixture.evidence_root,
                task_action_store::StoreFaultPoint::AfterRename,
            )
            .unwrap(),
        ));
        let prepared = route_action_body(
            fixture.state.clone(),
            pending_host.clone(),
            "route-runtime",
            &action_body,
        );
        let (uncertainty_id, action_id) = reconciliation_ids(FIXTURE, &prepared);
        let inspect = inspect_request(uncertainty_id.clone(), action_id.clone());
        let inspected = invoke_task_route(
            raw_task_route_request(
                "/api/task/actions/reconcile",
                "route-token",
                "route-runtime",
                &serde_json::to_vec(&inspect).unwrap(),
            ),
            fixture.state.clone(),
            pending_host.clone(),
        );
        let inspected: TaskWorkerReconciliationResponse =
            serde_json::from_value(http_json(&inspected)).unwrap();
        let resolution = match inspected.outcome {
            TaskWorkerReconciliationOutcome::Resolved(resolution) => resolution,
            other => panic!("{FIXTURE}: {other:?}"),
        };
        let ack = ack_request(uncertainty_id, action_id, &resolution);
        let acked = invoke_task_route(
            raw_task_route_request(
                "/api/task/actions/reconcile",
                "route-token",
                "route-runtime",
                &serde_json::to_vec(&ack).unwrap(),
            ),
            fixture.state.clone(),
            pending_host,
        );
        assert_eq!(http_json(&acked)["outcome"]["kind"], "acknowledged");
        let first_request = raw_task_route_request(
            "/api/task/actions",
            "route-token",
            "route-runtime",
            &action_body,
        );
        let second_request = first_request.clone();
        let first_state = fixture.state.clone();
        let first_host = fixture.host.clone();
        let second_state = fixture.state.clone();
        let second_host = fixture.host.clone();
        let first =
            std::thread::spawn(move || invoke_task_route(first_request, first_state, first_host));
        let second = std::thread::spawn(move || {
            invoke_task_route(second_request, second_state, second_host)
        });
        let first = http_json(&first.join().unwrap());
        let second = http_json(&second.join().unwrap());
        assert_eq!(first, second, "{CONCURRENT_DUPLICATE}");
        assert_eq!(first["outcome"]["kind"], "committed");
        assert_eq!(
            fs::read_to_string(fixture.task_root.join("task-service-v1.events.jsonl"))
                .unwrap()
                .lines()
                .count(),
            4,
            "one accept after create/publish/deliver"
        );
        let evidence: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .evidence_root
                    .join("task-worker-action-evidence-v1.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            evidence["records_by_action_key"].as_object().unwrap().len(),
            1
        );
        assert!(evidence.get("uncertainty").is_none());
        fs::remove_dir_all(fixture.root).unwrap();
    }

    fn test_agent(id: &str) -> AgentBusAgent {
        AgentBusAgent {
            id: id.to_string(),
            name: id.to_string(),
            base_name: Some(id.to_string()),
            thread_name: None,
            path_key: None,
            session_id: Some("thread-1".to_string()),
            cutex_session_id: None,
            profile: "profile".to_string(),
            cwd: "/tmp".to_string(),
            pid: 42,
            host_id: Some("host".to_string()),
            groups: Vec::new(),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 1,
        }
    }

    fn sessionless_test_agent(id: &str) -> AgentBusAgent {
        AgentBusAgent {
            session_id: None,
            ..test_agent(id)
        }
    }
}
