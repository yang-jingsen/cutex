use std::sync::{Arc, Mutex, OnceLock};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::role_revision::{CutexSessionId, Sha256};
use crate::seat::{DirectorSeatTransferRequest, SeatAuthorityError, SeatOccupancyStore};
use crate::task_service::ActionId;
use sha2::{Digest as _, Sha256 as Sha256Digest};

use super::store::{now, request_sha256, AgentManagementSnapshot};
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleFailure {
    pub code: String,
    pub detail: String,
    pub outcome_unknown: bool,
    pub known_native_session_id: Option<String>,
}

impl LifecycleFailure {
    pub fn definite(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            outcome_unknown: false,
            known_native_session_id: None,
        }
    }

    pub fn outcome_unknown(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            outcome_unknown: true,
            known_native_session_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeRecoveryOutcome {
    NoClaim,
    RecoveredExact,
    ClearedDeadClaim,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeBootstrapReconciliation {
    /// Every provider-owned native and runtime source was readable and none
    /// contained a session/runtime created by the historical bootstrap.
    ProvenAbsent { reason: String },
    /// Exact correlated runtime/session evidence exists.
    Present { reason: String },
    /// Evidence exists but cannot be correlated conclusively.
    Ambiguous { reason: String },
    /// A provider-owned evidence source could not be read or validated.
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeBootstrapIdentityReconciliation {
    /// Exactly one native thread was correlated to the reserved identity.
    Exact {
        native_session_id: String,
        reason: String,
    },
    /// All provider-owned evidence sources were readable and no matching
    /// native thread exists. This does not authorize a relaunch for an
    /// ambiguous historical attempt.
    Absent { reason: String },
    /// Evidence is present but not uniquely identity-correlated.
    Ambiguous { reason: String },
    /// A provider-owned source could not be read or validated.
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoricalRuntimeOccurrenceReconciliation {
    /// Every provider-owned source was readable and proves that no runtime,
    /// endpoint, process, or launch claim exists for the durable target.
    ProvenAbsent {
        fence: RuntimeOccurrenceFence,
        reason: String,
    },
    /// A live or durably claimed occurrence exists. Legacy actions without an
    /// exact occurrence identity must never act on it.
    Present { reason: String },
    /// Evidence exists but is incomplete or conflicts across sources.
    Ambiguous { reason: String },
    /// An authoritative source could not be read or validated.
    Unavailable { reason: String },
}

pub trait AgentLifecycle: Send + Sync {
    fn prepare_private_cwd(&self, spec: &ManagedAgentSpec) -> Result<(), LifecycleFailure>;
    fn bootstrap_native(&self, spec: &ManagedAgentSpec) -> Result<String, LifecycleFailure>;
    fn reconcile_pre_sid_bootstrap(
        &self,
        _spec: &ManagedAgentSpec,
        _started_at: &crate::role_revision::Rfc3339,
        _failed_at: &crate::role_revision::Rfc3339,
    ) -> Result<NativeBootstrapReconciliation, LifecycleFailure> {
        Err(LifecycleFailure::outcome_unknown(
            "native_bootstrap_reconciliation_unavailable",
            "lifecycle provider cannot prove the historical native bootstrap absent",
        ))
    }
    fn reconcile_ambiguous_native_bootstrap(
        &self,
        _spec: &ManagedAgentSpec,
        _started_at: &crate::role_revision::Rfc3339,
        _failed_at: &crate::role_revision::Rfc3339,
    ) -> Result<NativeBootstrapIdentityReconciliation, LifecycleFailure> {
        Err(LifecycleFailure::outcome_unknown(
            "native_bootstrap_reconciliation_unavailable",
            "lifecycle provider cannot identify the historical native bootstrap",
        ))
    }
    fn reconcile_historical_runtime_occurrence(
        &self,
        _cutex_session_id: &CutexSessionId,
    ) -> Result<HistoricalRuntimeOccurrenceReconciliation, LifecycleFailure> {
        Err(LifecycleFailure::outcome_unknown(
            "runtime_occurrence_reconciliation_unavailable",
            "lifecycle provider cannot fence the historical runtime occurrence",
        ))
    }
    fn adopt_native(
        &self,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<CutexSessionId, LifecycleFailure>;
    fn configure(
        &self,
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<(), LifecycleFailure>;
    fn recover_runtime(
        &self,
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<RuntimeRecoveryOutcome, LifecycleFailure>;
    fn online(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure>;
    fn offline(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure>;
    fn offline_if_occurrence(
        &self,
        cutex_session_id: &CutexSessionId,
        expected: &RuntimeOccurrenceFence,
    ) -> Result<(), LifecycleFailure> {
        let observed = self.observe(cutex_session_id)?;
        if observed.runtime_generation != expected.runtime_generation
            || observed.runtime_agent_ids
                != expected
                    .current_runtime_agent_id
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            || observed.agent_bus_endpoint_ids != expected.agent_bus_endpoint_ids
            || observed.app_server_runtime != expected.app_server_connected
        {
            return Err(LifecycleFailure::outcome_unknown(
                "runtime_occurrence_changed",
                "runtime occurrence changed before the fenced offline effect",
            ));
        }
        self.offline(cutex_session_id)
    }
    fn restart_if_occurrence(
        &self,
        _cutex_session_id: &CutexSessionId,
        _expected: &RuntimeOccurrenceFence,
    ) -> Result<(AgentRuntimeObservation, AgentRuntimeObservation), LifecycleFailure> {
        Err(LifecycleFailure::outcome_unknown(
            "fenced_restart_unavailable",
            "lifecycle provider cannot atomically fence a historical restart",
        ))
    }
    fn retire(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure>;
    fn observe(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AgentRuntimeObservation, LifecycleFailure>;
    fn send_message(
        &self,
        system: &crate::agent_bus::identity::AgentManagementSystemPrincipal,
        metadata: &AgentManagementMessageMetadata,
        to_agent: &CutexSessionId,
        exact_message: &str,
        external_message_id: &str,
    ) -> Result<String, LifecycleFailure>;
}

pub trait AgentManagementPhaseObserver: Send + Sync {
    fn phase_committed(&self, event: &AgentManagementPhaseEvent);
}

impl<F> AgentManagementPhaseObserver for F
where
    F: Fn(&AgentManagementPhaseEvent) + Send + Sync,
{
    fn phase_committed(&self, event: &AgentManagementPhaseEvent) {
        self(event);
    }
}

#[derive(Default)]
struct NoopPhaseObserver;

impl AgentManagementPhaseObserver for NoopPhaseObserver {
    fn phase_committed(&self, _event: &AgentManagementPhaseEvent) {}
}

pub struct AgentManagementProvider {
    store: AgentManagementStore,
    director_seats: SeatOccupancyStore,
    phase_observer: Arc<dyn AgentManagementPhaseObserver>,
    #[cfg(test)]
    fail_after_predecessor_close_once: Arc<AtomicBool>,
    #[cfg(test)]
    fail_after_director_seat_transfer_once: Arc<AtomicBool>,
}

fn provider_execution_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl AgentManagementProvider {
    pub fn open(root: impl Into<std::path::PathBuf>) -> Result<Self, AgentManagementError> {
        let root = root.into();
        Ok(Self {
            store: AgentManagementStore::open(&root)?,
            director_seats: SeatOccupancyStore::open(root.join("task-service-seat-authority-v1"))
                .map_err(seat_authority_error)?,
            phase_observer: Arc::new(NoopPhaseObserver),
            #[cfg(test)]
            fail_after_predecessor_close_once: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_after_director_seat_transfer_once: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn open_default() -> anyhow::Result<Self> {
        Ok(Self {
            store: AgentManagementStore::open_default()?,
            director_seats: SeatOccupancyStore::open_default()?,
            phase_observer: Arc::new(NoopPhaseObserver),
            #[cfg(test)]
            fail_after_predecessor_close_once: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_after_director_seat_transfer_once: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn with_phase_observer(mut self, observer: Arc<dyn AgentManagementPhaseObserver>) -> Self {
        self.phase_observer = observer;
        self
    }

    #[cfg(test)]
    fn with_fail_after_predecessor_close_once(self) -> Self {
        self.fail_after_predecessor_close_once
            .store(true, Ordering::SeqCst);
        self
    }

    #[cfg(test)]
    fn with_fail_after_director_seat_transfer_once(self) -> Self {
        self.fail_after_director_seat_transfer_once
            .store(true, Ordering::SeqCst);
        self
    }

    #[cfg(test)]
    fn inject_process_loss_after_predecessor_close(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Option<AgentManagementResponse> {
        self.fail_after_predecessor_close_once
            .swap(false, Ordering::SeqCst)
            .then(|| {
                no_write(
                    &request.action_id,
                    "injected_process_loss",
                    "test-only process loss after predecessor close",
                )
            })
    }

    #[cfg(not(test))]
    fn inject_process_loss_after_predecessor_close(
        &self,
        _request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Option<AgentManagementResponse> {
        None
    }

    #[cfg(test)]
    fn inject_process_loss_after_director_seat_transfer(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Option<AgentManagementResponse> {
        self.fail_after_director_seat_transfer_once
            .swap(false, Ordering::SeqCst)
            .then(|| {
                no_write(
                    &request.action_id,
                    "injected_process_loss",
                    "test-only process loss after Task Service Director seat transfer",
                )
            })
    }

    #[cfg(not(test))]
    fn inject_process_loss_after_director_seat_transfer(
        &self,
        _request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Option<AgentManagementResponse> {
        None
    }

    pub fn store(&self) -> &AgentManagementStore {
        &self.store
    }

    fn notify_phase(&self, event: &AgentManagementPhaseEvent) {
        self.phase_observer.phase_committed(event);
    }

    pub fn bind_project_authority(
        &self,
        request: &ProjectAuthorityRequest,
    ) -> Result<ProjectAuthorityReceipt, AgentManagementError> {
        let _execution = provider_execution_lock()
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        if request.expected_authority_epoch == Some(0)
            || request.expected_authorized_director_session.is_some()
                != request.expected_authority_epoch.is_some()
        {
            return Err(AgentManagementError::InvalidRequest(
                "authority_expectations_must_be_complete",
            ));
        }
        let digest = request_sha256(request)?;
        self.store.with_state(true, |mut state| {
            if state.actions.contains_key(&request.action_id)
                || state
                    .legacy_director_ownership_import_receipts
                    .contains_key(&request.action_id)
            {
                return Err(AgentManagementError::Conflict("action_id_domain_conflict"));
            }
            if let Some(receipt) = state.authority_receipts.get(&request.action_id).cloned() {
                return if receipt.request_sha256 == digest {
                    Ok((state, receipt, false))
                } else {
                    Err(AgentManagementError::Conflict("action_id_payload_conflict"))
                };
            }
            let epoch = match state.projects.get(&request.project_id) {
                None => {
                    if request.expected_authorized_director_session.is_some() {
                        return Err(AgentManagementError::Conflict(
                            "project_authority_not_initialized",
                        ));
                    }
                    1
                }
                Some(current) => {
                    if request.expected_authorized_director_session.as_ref()
                        != Some(&current.authorized_director_session)
                        || request.expected_authority_epoch != Some(current.authority_epoch)
                    {
                        return Err(AgentManagementError::Conflict("stale_project_authority"));
                    }
                    current
                        .authority_epoch
                        .checked_add(1)
                        .filter(|epoch| *epoch <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
                        .ok_or(AgentManagementError::Conflict("authority_epoch_overflow"))?
                }
            };
            let authority = ProjectAuthority {
                project_id: request.project_id.clone(),
                authorized_director_session: request.authorized_director_session.clone(),
                authority_epoch: epoch,
                updated_at: now(),
            };
            let receipt = ProjectAuthorityReceipt {
                schema: AgentManagementReceiptSchema::V1,
                action_id: request.action_id.clone(),
                request_sha256: digest.clone(),
                authority: authority.clone(),
            };
            state.projects.insert(request.project_id.clone(), authority);
            state
                .authority_receipts
                .insert(request.action_id.clone(), receipt.clone());
            Ok((state, receipt, true))
        })
    }

    /// Atomically imports the one missing ownership record for an exact legacy
    /// Director. This is called only by the root administration adapter; it is
    /// deliberately not part of `AgentOperation` or the ambient Agent tool.
    pub fn import_legacy_director_ownership<F>(
        &self,
        request: &LegacyDirectorOwnershipImportRequest,
        load_evidence: F,
    ) -> Result<(LegacyDirectorOwnershipImportReceipt, bool), AgentManagementError>
    where
        F: FnOnce() -> Result<LegacyDirectorOwnershipEvidence, AgentManagementError>,
    {
        let _execution = provider_execution_lock()
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        if request.expected_authority_epoch == 0
            || request.expected_authority_epoch > crate::role_revision::MAX_JSON_SAFE_INTEGER
        {
            return Err(AgentManagementError::InvalidRequest(
                "invalid_authority_epoch",
            ));
        }
        if request.expected_authorized_director_session != request.director_cutex_session_id {
            return Err(AgentManagementError::InvalidRequest(
                "expected_authority_must_name_imported_director",
            ));
        }
        let digest = request_sha256(request)?;
        self.store.with_state(true, |mut state| {
            if state.actions.contains_key(&request.action_id)
                || state.authority_receipts.contains_key(&request.action_id)
            {
                return Err(AgentManagementError::Conflict("action_id_domain_conflict"));
            }
            if let Some(receipt) = state
                .legacy_director_ownership_import_receipts
                .get(&request.action_id)
                .cloned()
            {
                return if receipt.request_sha256 == digest {
                    Ok((state, (receipt, true), false))
                } else {
                    Err(AgentManagementError::Conflict("action_id_payload_conflict"))
                };
            }

            let authority = state.projects.get(&request.project_id).cloned().ok_or(
                AgentManagementError::Conflict("project_authority_not_initialized"),
            )?;
            if authority.authorized_director_session != request.director_cutex_session_id
                || authority.authorized_director_session
                    != request.expected_authorized_director_session
                || authority.authority_epoch != request.expected_authority_epoch
            {
                return Err(AgentManagementError::Conflict("stale_project_authority"));
            }
            if state.projects.iter().any(|(project_id, candidate)| {
                project_id != &request.project_id
                    && candidate.authorized_director_session == request.director_cutex_session_id
            }) {
                return Err(AgentManagementError::Conflict(
                    "director_authorized_for_multiple_projects",
                ));
            }
            if let Some(existing) = state.agents.get(&request.director_cutex_session_id) {
                let reason = if existing.project_id != request.project_id {
                    "director_owned_by_another_project"
                } else if existing.retired_at.is_some() {
                    "director_ownership_record_retired"
                } else {
                    "director_ownership_record_exists"
                };
                return Err(AgentManagementError::Conflict(reason));
            }

            let evidence = load_evidence()?;
            if evidence.director_cutex_session_id != request.director_cutex_session_id {
                return Err(AgentManagementError::Conflict(
                    "durable_session_identity_mismatch",
                ));
            }
            if evidence.native_session_id.trim().is_empty()
                || evidence.native_session_id.trim() != evidence.native_session_id
            {
                return Err(AgentManagementError::InvalidRequest(
                    "invalid_native_session_id",
                ));
            }
            if evidence.durable_session_revision == 0
                || evidence.durable_session_revision > crate::role_revision::MAX_JSON_SAFE_INTEGER
                || evidence.runtime_generation > crate::role_revision::MAX_JSON_SAFE_INTEGER
            {
                return Err(AgentManagementError::Conflict(
                    "invalid_durable_lifecycle_evidence",
                ));
            }
            evidence.spec.validate()?;
            if state.agents.values().any(|agent| {
                agent.native_session_id == evidence.native_session_id
                    && agent.cutex_session_id != request.director_cutex_session_id
            }) {
                return Err(AgentManagementError::Conflict(
                    "native_session_owned_by_another_agent",
                ));
            }

            let agent = ManagedAgentRecord {
                project_id: request.project_id.clone(),
                created_by_director_session: request.director_cutex_session_id.clone(),
                cutex_session_id: request.director_cutex_session_id.clone(),
                native_session_id: evidence.native_session_id,
                spec: evidence.spec,
                created_at: now(),
                retired_at: None,
            };
            let receipt = LegacyDirectorOwnershipImportReceipt {
                schema: LegacyDirectorOwnershipImportReceiptSchema::V1,
                action_id: request.action_id.clone(),
                request_sha256: digest.clone(),
                authority: authority.clone(),
                agent: agent.clone(),
                durable_session_revision: evidence.durable_session_revision,
                runtime_generation: evidence.runtime_generation,
            };
            state
                .agents
                .insert(request.director_cutex_session_id.clone(), agent);
            state
                .legacy_director_ownership_import_receipts
                .insert(request.action_id.clone(), receipt.clone());
            Ok((state, (receipt, false), true))
        })
    }

    pub fn execute(
        &self,
        invocation: &AgentManagementInvocation,
        request: &AgentManagementRequest,
        lifecycle: &dyn AgentLifecycle,
    ) -> AgentManagementResponse {
        let _execution = match provider_execution_lock().lock() {
            Ok(execution) => execution,
            Err(_) => {
                return no_write(
                    &request.action_id,
                    "persistence_unavailable",
                    "Agent Management execution lock is unavailable",
                )
            }
        };
        if let Err(error) = request.validate() {
            return error_response(&request.action_id, error);
        }
        let digest = match request_sha256(request) {
            Ok(digest) => digest,
            Err(error) => return error_response(&request.action_id, error),
        };
        let request = match self.authorize_request(invocation, request, &digest) {
            Ok(request) => request,
            Err(error) => return error_response(&request.action_id, error),
        };
        let historical_continuation =
            match self.authorize_historical_bootstrap_continuation(&request, lifecycle) {
                Ok(continuation) => continuation,
                Err(response) => return response,
            };
        let action = match self.begin_action(invocation, &request, &digest, historical_continuation)
        {
            Ok(BeginAction::Replay(response)) => {
                if let Err(error) = self.reconcile_completed_rotation(&request, &response) {
                    return error_response(&request.action_id, error);
                }
                return response;
            }
            Ok(BeginAction::Execute(action)) => action,
            Err(error) => return error_response(&request.action_id, error),
        };
        match self.execute_started(invocation, &request, &digest, action, lifecycle) {
            Ok(response) => response,
            Err(error) => {
                if matches!(error, AgentManagementError::OwnerActionRequired(_)) {
                    self.owner_action_required(&request, &error.to_string())
                        .unwrap_or_else(|store_error| {
                            error_response(&request.action_id, store_error)
                        })
                } else {
                    self.terminalize_pre_effect_no_write(&request, error)
                }
            }
        }
    }

    #[allow(clippy::result_large_err)] // The typed fence response is returned directly, never copied.
    fn authorize_historical_bootstrap_continuation(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<HistoricalBootstrapContinuation, AgentManagementResponse> {
        let snapshot = self
            .store
            .snapshot()
            .map_err(|error| error_response(&request.action_id, error))?;
        let Some(action) = snapshot.actions.get(&request.action_id) else {
            return Ok(HistoricalBootstrapContinuation::None);
        };
        if legacy_offline_revision_conflict_candidate(action) {
            let cutex_session_id = match &request.operation {
                AgentOperation::Offline { cutex_session_id }
                | AgentOperation::Restart { cutex_session_id }
                | AgentOperation::Close { cutex_session_id } => cutex_session_id,
                _ => return Ok(HistoricalBootstrapContinuation::None),
            };
            let agent = self
                .active_agent(&request.project_id, cutex_session_id)
                .map_err(|error| {
                    reconciliation_fence_response(action, "ambiguous", &error.to_string())
                })?;
            let before = lifecycle.observe(cutex_session_id).map_err(|error| {
                reconciliation_fence_response(
                    action,
                    "unavailable",
                    &format!("{}: {}", error.code, error.detail),
                )
            })?;
            validate_managed_observation_identity(&agent, &before).map_err(|error| {
                reconciliation_fence_response(action, "ambiguous", &error.to_string())
            })?;
            let reconciliation = lifecycle
                .reconcile_historical_runtime_occurrence(cutex_session_id)
                .unwrap_or_else(
                    |error| HistoricalRuntimeOccurrenceReconciliation::Unavailable {
                        reason: format!("{}: {}", error.code, error.detail),
                    },
                );
            return match reconciliation {
                HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent { fence, .. }
                    if fence.is_proven_absent()
                        && before.runtime_generation == fence.runtime_generation
                        && before.runtime_agent_ids.is_empty()
                        && before.agent_bus_endpoint_ids.is_empty()
                        && !before.app_server_runtime =>
                {
                    Ok(HistoricalBootstrapContinuation::RetryLifecycleAfterOffline(
                        fence,
                    ))
                }
                HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent { .. } => {
                    Err(reconciliation_fence_response(
                        action,
                        "ambiguous",
                        "provider absence proof conflicts with the durable runtime observation",
                    ))
                }
                HistoricalRuntimeOccurrenceReconciliation::Present { reason } => {
                    Err(reconciliation_fence_response(action, "present", &reason))
                }
                HistoricalRuntimeOccurrenceReconciliation::Ambiguous { reason } => {
                    Err(reconciliation_fence_response(action, "ambiguous", &reason))
                }
                HistoricalRuntimeOccurrenceReconciliation::Unavailable { reason } => Err(
                    reconciliation_fence_response(action, "unavailable", &reason),
                ),
            };
        }
        let AgentOperation::Create { spec, .. } = &request.operation else {
            return Ok(HistoricalBootstrapContinuation::None);
        };
        if legacy_pre_sid_retry_candidate(action) {
            let reconciliation = lifecycle
                .reconcile_pre_sid_bootstrap(spec, &action.created_at, &action.updated_at)
                .unwrap_or_else(|error| NativeBootstrapReconciliation::Unavailable {
                    reason: format!("{}: {}", error.code, error.detail),
                });
            return match reconciliation {
                NativeBootstrapReconciliation::ProvenAbsent { .. } => {
                    Ok(HistoricalBootstrapContinuation::RetryProvenAbsent)
                }
                NativeBootstrapReconciliation::Present { reason } => {
                    Err(reconciliation_fence_response(action, "present", &reason))
                }
                NativeBootstrapReconciliation::Ambiguous { reason } => {
                    Err(reconciliation_fence_response(action, "ambiguous", &reason))
                }
                NativeBootstrapReconciliation::Unavailable { reason } => Err(
                    reconciliation_fence_response(action, "unavailable", &reason),
                ),
            };
        }
        if !legacy_ambiguous_sid_recovery_candidate(action) {
            return Ok(HistoricalBootstrapContinuation::None);
        }
        let Some((started_at, failed_at)) = latest_native_bootstrap_window(&snapshot, action)
        else {
            return Err(reconciliation_fence_response(
                action,
                "unavailable",
                "the most recent native bootstrap attempt window is unavailable",
            ));
        };
        let reconciliation = lifecycle
            .reconcile_ambiguous_native_bootstrap(spec, &started_at, &failed_at)
            .unwrap_or_else(|error| NativeBootstrapIdentityReconciliation::Unavailable {
                reason: format!("{}: {}", error.code, error.detail),
            });
        match reconciliation {
            NativeBootstrapIdentityReconciliation::Exact {
                native_session_id,
                reason,
            } => match uuid::Uuid::parse_str(&native_session_id) {
                Ok(native_session_id) => Ok(HistoricalBootstrapContinuation::CaptureExactSid(
                    native_session_id.to_string(),
                )),
                Err(_) => Err(reconciliation_fence_response(
                    action,
                    "ambiguous",
                    &format!("reconciled native session identity is invalid: {reason}"),
                )),
            },
            NativeBootstrapIdentityReconciliation::Absent { reason } => {
                Err(reconciliation_fence_response(action, "absent", &reason))
            }
            NativeBootstrapIdentityReconciliation::Ambiguous { reason } => {
                Err(reconciliation_fence_response(action, "ambiguous", &reason))
            }
            NativeBootstrapIdentityReconciliation::Unavailable { reason } => Err(
                reconciliation_fence_response(action, "unavailable", &reason),
            ),
        }
    }

    fn authorize_request<'a>(
        &self,
        invocation: &AgentManagementInvocation,
        request: &'a AgentManagementRequest,
        digest: &Sha256,
    ) -> Result<AuthorizedAgentManagementRequest<'a>, AgentManagementError> {
        let snapshot = self.store.snapshot()?;
        if let Some(existing) = snapshot.actions.get(&request.action_id) {
            if existing.caller_cutex_session != invocation.caller_cutex_session {
                return Err(AgentManagementError::NotAuthorizedDirector);
            }
            if &existing.request_sha256 != digest {
                return Err(AgentManagementError::Conflict("action_id_payload_conflict"));
            }
            // An exact immutable receipt remains replayable by its original
            // authenticated caller after a Director rotation transfers live
            // project authority. This grants no new action authority.
            if existing.response.is_none()
                || legacy_pre_sid_retry_candidate(existing)
                || legacy_ambiguous_sid_recovery_candidate(existing)
                || legacy_offline_revision_conflict_candidate(existing)
            {
                let authority = snapshot
                    .projects
                    .get(&existing.project_id)
                    .ok_or(AgentManagementError::NotAuthorizedDirector)?;
                if authority.authorized_director_session != invocation.caller_cutex_session {
                    return Err(AgentManagementError::NotAuthorizedDirector);
                }
            }
            return Ok(AuthorizedAgentManagementRequest {
                project_id: existing.project_id.clone(),
                request,
            });
        }
        let authorized_projects = snapshot
            .projects
            .values()
            .filter(|authority| {
                authority.authorized_director_session == invocation.caller_cutex_session
            })
            .map(|authority| authority.project_id.clone())
            .collect::<Vec<_>>();
        let project_id = match request.project_id.as_ref() {
            Some(selector) if authorized_projects.contains(selector) => selector.clone(),
            Some(_) if authorized_projects.is_empty() => {
                return Err(AgentManagementError::NotAuthorizedDirector)
            }
            Some(_) => return Err(AgentManagementError::ProjectNotAuthorized),
            None => match authorized_projects.as_slice() {
                [project_id] => project_id.clone(),
                [] => return Err(AgentManagementError::NotAuthorizedDirector),
                _ => return Err(AgentManagementError::ProjectSelectionRequired),
            },
        };
        Ok(AuthorizedAgentManagementRequest {
            project_id,
            request,
        })
    }

    fn begin_action(
        &self,
        invocation: &AgentManagementInvocation,
        request: &AuthorizedAgentManagementRequest<'_>,
        digest: &Sha256,
        historical_continuation: HistoricalBootstrapContinuation,
    ) -> Result<BeginAction, AgentManagementError> {
        self.store
            .with_state(true, |mut state| {
                if state.authority_receipts.contains_key(&request.action_id)
                    || state
                        .legacy_director_ownership_import_receipts
                        .contains_key(&request.action_id)
                {
                    return Err(AgentManagementError::Conflict("action_id_domain_conflict"));
                }
                if let Some(existing) = state.actions.get(&request.action_id).cloned() {
                    if &existing.request_sha256 != digest {
                        return Err(AgentManagementError::Conflict("action_id_payload_conflict"));
                    }
                    if existing.caller_cutex_session != invocation.caller_cutex_session
                        || existing.project_id != request.project_id
                    {
                        return Err(AgentManagementError::Conflict("action_identity_conflict"));
                    }
                    if historical_continuation == HistoricalBootstrapContinuation::RetryProvenAbsent
                        && legacy_pre_sid_retry_candidate(&existing)
                    {
                        let action = state
                            .actions
                            .get_mut(&request.action_id)
                            .ok_or(AgentManagementError::InvalidStore)?;
                        action.response = None;
                        action.native_bootstrap_retryable = true;
                        let (action, event) = Self::record_phase_event(
                            &mut state,
                            request,
                            AgentActionPhase::NativeBootstrapPending,
                            true,
                        )?;
                        return Ok((state, (BeginAction::Execute(action), Some(event)), true));
                    }
                    if let HistoricalBootstrapContinuation::RetryLifecycleAfterOffline(fence) =
                        &historical_continuation
                    {
                        if !legacy_offline_revision_conflict_candidate(&existing) {
                            return Ok(match existing.response {
                                Some(response) => {
                                    (state, (BeginAction::Replay(response), None), false)
                                }
                                None => (state, (BeginAction::Execute(existing), None), false),
                            });
                        }
                        let action = state
                            .actions
                            .get_mut(&request.action_id)
                            .ok_or(AgentManagementError::InvalidStore)?;
                        action.response = None;
                        action.historical_runtime_occurrence_fence = Some(fence.clone());
                        let (action, event) = Self::record_phase_event(
                            &mut state,
                            request,
                            AgentActionPhase::Prepared,
                            true,
                        )?;
                        return Ok((state, (BeginAction::Execute(action), Some(event)), true));
                    }
                    if let HistoricalBootstrapContinuation::CaptureExactSid(native_session_id) =
                        &historical_continuation
                    {
                        if legacy_ambiguous_sid_recovery_candidate(&existing) {
                            let action = state
                                .actions
                                .get_mut(&request.action_id)
                                .ok_or(AgentManagementError::InvalidStore)?;
                            action.response = None;
                            action.native_bootstrap_retryable = false;
                            action.known_native_session_id = Some(native_session_id.clone());
                            let (action, event) = Self::record_phase_event(
                                &mut state,
                                request,
                                AgentActionPhase::NativeSessionCaptured,
                                true,
                            )?;
                            return Ok((state, (BeginAction::Execute(action), Some(event)), true));
                        }
                    }
                    return Ok(match existing.response {
                        Some(response) => (state, (BeginAction::Replay(response), None), false),
                        None => (state, (BeginAction::Execute(existing), None), false),
                    });
                }
                let authority = state
                    .projects
                    .get(&request.project_id)
                    .ok_or(AgentManagementError::Unauthorized)?;
                if authority.authorized_director_session != invocation.caller_cutex_session {
                    return Err(AgentManagementError::Unauthorized);
                }
                let reservation = operation_reservation(request);
                if let Some(spec) = reservation {
                    let collides = state.actions.values().any(|action| {
                        !matches!(
                            action.phase,
                            AgentActionPhase::Complete | AgentActionPhase::NoWrite
                        ) && (action.reserved_agent_name.as_deref() == Some(spec.name.as_str())
                            || action.reserved_agent_cwd.as_deref() == Some(spec.cwd.as_str()))
                    });
                    if collides {
                        return Err(AgentManagementError::Conflict(
                            "unresolved_agent_reservation",
                        ));
                    }
                }
                let timestamp = now();
                let external_message_id = operation_message(request)
                    .map(|_| format!("agent-management:{}:start", request.action_id.as_str()));
                let action = AgentActionRecord {
                    action_id: request.action_id.clone(),
                    request_sha256: digest.clone(),
                    operation: request.operation.kind(),
                    project_id: request.project_id.clone(),
                    caller_cutex_session: invocation.caller_cutex_session.clone(),
                    phase: AgentActionPhase::Prepared,
                    phase_sequence: 0,
                    reserved_agent_name: reservation.map(|spec| spec.name.clone()),
                    reserved_agent_cwd: reservation.map(|spec| spec.cwd.clone()),
                    known_successor_cutex_session: None,
                    known_native_session_id: None,
                    native_bootstrap_retryable: false,
                    historical_runtime_occurrence_fence: None,
                    external_message_id,
                    response: None,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                };
                state
                    .actions
                    .insert(request.action_id.clone(), action.clone());
                let (action, event) = Self::record_phase_event(
                    &mut state,
                    request,
                    AgentActionPhase::Prepared,
                    true,
                )?;
                Ok((state, (BeginAction::Execute(action), Some(event)), true))
            })
            .map(|(begin, event)| {
                if let Some(event) = event.as_ref() {
                    self.notify_phase(event);
                }
                begin
            })
    }

    fn execute_started(
        &self,
        invocation: &AgentManagementInvocation,
        request: &AuthorizedAgentManagementRequest<'_>,
        digest: &Sha256,
        action: AgentActionRecord,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<AgentManagementResponse, AgentManagementError> {
        match &request.operation {
            AgentOperation::Create {
                spec,
                start_mode,
                frozen_message,
            } => {
                let created = self.create_steps(
                    invocation,
                    request,
                    action,
                    spec,
                    *start_mode,
                    frozen_message.as_deref(),
                    lifecycle,
                )?;
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Created {
                        agent: created.agent,
                        observation: created.observation,
                        message_id: created.message_id,
                    },
                )
            }
            AgentOperation::QueryManaged => {
                let snapshot = self.store.snapshot()?;
                let authority = snapshot
                    .projects
                    .get(&request.project_id)
                    .cloned()
                    .ok_or(AgentManagementError::Unauthorized)?;
                let agents = snapshot
                    .agents
                    .values()
                    .filter(|agent| {
                        agent.project_id == request.project_id && agent.retired_at.is_none()
                    })
                    .cloned()
                    .collect();
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::QueryManaged { authority, agents },
                )
            }
            AgentOperation::Online { cutex_session_id } => {
                let (agent, observation) =
                    self.online_existing(request, cutex_session_id, lifecycle)?;
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Lifecycle { agent, observation },
                )
            }
            AgentOperation::Offline { cutex_session_id } => {
                let (agent, observation) = self.offline_existing(
                    request,
                    cutex_session_id,
                    action.historical_runtime_occurrence_fence.as_ref(),
                    lifecycle,
                )?;
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Lifecycle { agent, observation },
                )
            }
            AgentOperation::Restart { cutex_session_id } => {
                let agent = self.active_agent(&request.project_id, cutex_session_id)?;
                let (before, after) = match action.historical_runtime_occurrence_fence.as_ref() {
                    Some(fence) => lifecycle
                        .restart_if_occurrence(cutex_session_id, fence)
                        .map_err(lifecycle_error)?,
                    None => {
                        recover_runtime_for_agent(&agent, lifecycle)?;
                        let before = lifecycle
                            .observe(cutex_session_id)
                            .map_err(lifecycle_error)?;
                        lifecycle
                            .offline(cutex_session_id)
                            .map_err(lifecycle_error)?;
                        lifecycle
                            .online(cutex_session_id)
                            .map_err(lifecycle_error)?;
                        let after = lifecycle
                            .observe(cutex_session_id)
                            .map_err(lifecycle_error)?;
                        (before, after)
                    }
                };
                validate_ready(&agent, &after)?;
                validate_identity_preserved(&before, &after)?;
                if after.runtime_generation <= before.runtime_generation {
                    return Err(AgentManagementError::OwnerActionRequired(
                        "restart did not advance the runtime occurrence".to_string(),
                    ));
                }
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Lifecycle {
                        agent,
                        observation: after,
                    },
                )
            }
            AgentOperation::Close { cutex_session_id } => {
                let (agent, observation) = self.close_existing(
                    request,
                    cutex_session_id,
                    action.historical_runtime_occurrence_fence.as_ref(),
                    lifecycle,
                )?;
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Lifecycle { agent, observation },
                )
            }
            AgentOperation::Replace {
                predecessor_cutex_session_id,
                policy,
                successor,
                start_mode,
                frozen_message,
            } => {
                let mut action = action;
                if action.phase == AgentActionPhase::Prepared {
                    self.active_agent(&request.project_id, predecessor_cutex_session_id)?;
                    self.reject_active_collision_except(
                        successor,
                        (*policy == AgentReplacePolicy::CloseBeforeCreate)
                            .then_some(predecessor_cutex_session_id),
                    )?;
                }
                if *policy == AgentReplacePolicy::CloseBeforeCreate
                    && matches!(
                        action.phase,
                        AgentActionPhase::Prepared | AgentActionPhase::PredecessorClosing
                    )
                {
                    if action.phase == AgentActionPhase::Prepared {
                        self.set_phase(request, AgentActionPhase::PredecessorClosing)?;
                    }
                    if let Some(response) = self.close_or_reconcile_expected_predecessor(
                        request,
                        predecessor_cutex_session_id,
                        lifecycle,
                    )? {
                        return Ok(response);
                    }
                    action = self.set_phase(request, AgentActionPhase::PredecessorClosed)?;
                }
                let created = if *policy == AgentReplacePolicy::CloseAfterReady
                    && matches!(
                        action.phase,
                        AgentActionPhase::PredecessorClosing | AgentActionPhase::PredecessorClosed
                    ) {
                    self.recover_created_agent(request, &action, *start_mode, lifecycle)?
                } else {
                    self.create_steps(
                        invocation,
                        request,
                        action.clone(),
                        successor,
                        *start_mode,
                        frozen_message.as_deref(),
                        lifecycle,
                    )?
                };
                if *policy == AgentReplacePolicy::CloseAfterReady {
                    if action.phase != AgentActionPhase::PredecessorClosed {
                        if action.phase != AgentActionPhase::PredecessorClosing {
                            self.set_phase(request, AgentActionPhase::PredecessorClosing)?;
                        }
                        if let Some(response) = self.close_or_reconcile_expected_predecessor(
                            request,
                            predecessor_cutex_session_id,
                            lifecycle,
                        )? {
                            return Ok(response);
                        }
                        self.set_phase(request, AgentActionPhase::PredecessorClosed)?;
                    }
                }
                self.complete(
                    request,
                    digest,
                    AgentManagementResult::Replaced {
                        predecessor_cutex_session_id: predecessor_cutex_session_id.clone(),
                        successor: created.agent,
                        observation: created.observation,
                        message_id: created.message_id,
                    },
                )
            }
            AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session,
                expected_authority_epoch,
                mode,
                successor,
                frozen_message,
            } => {
                if &invocation.caller_cutex_session != expected_predecessor_cutex_session {
                    return Err(AgentManagementError::Conflict("stale_director_predecessor"));
                }
                self.require_authority(
                    &request.project_id,
                    expected_predecessor_cutex_session,
                    *expected_authority_epoch,
                )?;
                let mut action = action;
                let transfer_action_id = director_seat_transfer_action_id(&request.action_id)?;
                let replay_transfer =
                    action
                        .known_successor_cutex_session
                        .as_ref()
                        .map(|successor| DirectorSeatTransferRequest {
                            action_id: transfer_action_id.clone(),
                            expected_predecessor_cutex_session: expected_predecessor_cutex_session
                                .clone(),
                            successor_cutex_session: successor.clone(),
                        });
                self.director_seats
                    .preflight_director_transfer(
                        &transfer_action_id,
                        expected_predecessor_cutex_session,
                        replay_transfer.as_ref(),
                    )
                    .map_err(seat_authority_error)?;
                if action.phase == AgentActionPhase::Prepared {
                    self.active_agent(&request.project_id, expected_predecessor_cutex_session)?;
                    self.reject_active_collision_except(
                        successor,
                        (*mode == DirectorRotateMode::ClosePredecessorThenCreateWithMessage)
                            .then_some(expected_predecessor_cutex_session),
                    )?;
                }
                if *mode == DirectorRotateMode::ClosePredecessorThenCreateWithMessage
                    && matches!(
                        action.phase,
                        AgentActionPhase::Prepared | AgentActionPhase::PredecessorClosing
                    )
                {
                    if action.phase == AgentActionPhase::Prepared {
                        self.set_phase(request, AgentActionPhase::PredecessorClosing)?;
                    }
                    if let Some(response) = self.close_or_reconcile_expected_predecessor(
                        request,
                        expected_predecessor_cutex_session,
                        lifecycle,
                    )? {
                        return Ok(response);
                    }
                    action = self.set_phase(request, AgentActionPhase::PredecessorClosed)?;
                }
                let start_mode = match mode {
                    DirectorRotateMode::RetainPredecessorBootstrapOnly => {
                        AgentStartMode::BootstrapOnly
                    }
                    DirectorRotateMode::ClosePredecessorThenCreateWithMessage
                    | DirectorRotateMode::RetainPredecessorWithMessage => {
                        AgentStartMode::CustomMessage
                    }
                };
                if action.phase == AgentActionPhase::AuthorityTransferPending {
                    let created =
                        self.recover_created_agent(request, &action, start_mode, lifecycle)?;
                    return self.complete_rotation(
                        request,
                        digest,
                        expected_predecessor_cutex_session,
                        *expected_authority_epoch,
                        created,
                    );
                }
                let created = self.create_steps(
                    invocation,
                    request,
                    action,
                    successor,
                    start_mode,
                    frozen_message.as_deref(),
                    lifecycle,
                )?;
                self.set_phase(request, AgentActionPhase::AuthorityTransferPending)?;
                self.complete_rotation(
                    request,
                    digest,
                    expected_predecessor_cutex_session,
                    *expected_authority_epoch,
                    created,
                )
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_steps(
        &self,
        invocation: &AgentManagementInvocation,
        request: &AuthorizedAgentManagementRequest<'_>,
        mut action: AgentActionRecord,
        spec: &ManagedAgentSpec,
        start_mode: AgentStartMode,
        message: Option<&str>,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<CreatedAgent, AgentManagementError> {
        if action.known_successor_cutex_session.is_none()
            && action.known_native_session_id.is_none()
            && matches!(
                action.phase,
                AgentActionPhase::Prepared | AgentActionPhase::PredecessorClosed
            )
        {
            self.reject_active_collision(spec)?;
            lifecycle
                .prepare_private_cwd(spec)
                .map_err(lifecycle_error)?;
            action = self.set_phase(request, AgentActionPhase::PrivateCwdReady)?;
        }
        if action.known_native_session_id.is_none()
            && matches!(
                action.phase,
                AgentActionPhase::PrivateCwdReady | AgentActionPhase::NativeBootstrapPending
            )
            && (action.phase == AgentActionPhase::PrivateCwdReady
                || action.native_bootstrap_retryable)
        {
            if action.phase == AgentActionPhase::PrivateCwdReady {
                self.set_phase(request, AgentActionPhase::NativeBootstrapPending)?;
            } else {
                self.consume_native_bootstrap_retry(request)?;
            }
            match lifecycle.bootstrap_native(spec) {
                Ok(native_session_id) => {
                    action = self.capture_native_session(request, &native_session_id)?;
                }
                Err(error) => {
                    if let Some(native_session_id) = error.known_native_session_id.as_deref() {
                        self.capture_native_session(request, native_session_id)?;
                    } else if !error.outcome_unknown {
                        self.mark_native_bootstrap_retryable(request)?;
                    }
                    return Err(lifecycle_error(error));
                }
            }
        } else if action.known_native_session_id.is_none()
            && action.phase == AgentActionPhase::NativeBootstrapPending
        {
            return Err(AgentManagementError::OwnerActionRequired(
                "native bootstrap outcome is unknown; no second Agent was created".to_string(),
            ));
        }
        let native_session_id = action.known_native_session_id.clone().ok_or_else(|| {
            AgentManagementError::OwnerActionRequired(
                "captured native session identity is unavailable".to_string(),
            )
        })?;
        if action.known_successor_cutex_session.is_none() {
            let cutex_session_id = lifecycle
                .adopt_native(&native_session_id, spec)
                .map_err(lifecycle_error)?;
            action = self.capture_adopted_agent(
                invocation,
                request,
                &cutex_session_id,
                &native_session_id,
                spec,
            )?;
        }
        let cutex_session_id = action
            .known_successor_cutex_session
            .clone()
            .ok_or(AgentManagementError::InvalidStore)?;
        if matches!(action.phase, AgentActionPhase::Adopted) {
            lifecycle
                .configure(&cutex_session_id, &native_session_id, spec)
                .map_err(lifecycle_error)?;
            action = self.set_phase(request, AgentActionPhase::Configured)?;
        }
        if matches!(action.phase, AgentActionPhase::Configured) {
            let agent = self.active_agent(&request.project_id, &cutex_session_id)?;
            recover_runtime_for_agent(&agent, lifecycle)?;
            lifecycle
                .online(&cutex_session_id)
                .map_err(lifecycle_error)?;
            action = self.set_phase(request, AgentActionPhase::Online)?;
        } else if matches!(action.phase, AgentActionPhase::Online) {
            let agent = self.active_agent(&request.project_id, &cutex_session_id)?;
            recover_runtime_for_agent(&agent, lifecycle)?;
            lifecycle
                .online(&cutex_session_id)
                .map_err(lifecycle_error)?;
        }
        let observation = lifecycle
            .observe(&cutex_session_id)
            .map_err(lifecycle_error)?;
        let agent = self.active_agent(&request.project_id, &cutex_session_id)?;
        validate_ready(&agent, &observation)?;
        if matches!(action.phase, AgentActionPhase::Online) {
            action = self.set_phase(request, AgentActionPhase::Ready)?;
        }
        let message_id = if start_mode == AgentStartMode::CustomMessage {
            let exact_message = message.ok_or(AgentManagementError::InvalidRequest(
                "custom_message_requires_frozen_message",
            ))?;
            let external_message_id = action
                .external_message_id
                .clone()
                .ok_or(AgentManagementError::InvalidStore)?;
            if action.phase == AgentActionPhase::Ready {
                action = self.set_phase(request, AgentActionPhase::MessagePending)?;
            }
            if action.phase == AgentActionPhase::MessagePending {
                let metadata = AgentManagementMessageMetadata {
                    schema: AgentManagementSchema::V1,
                    requested_by_director: invocation.caller_cutex_session.clone(),
                };
                let system = crate::agent_bus::identity::agent_management_system_principal();
                let message_id = lifecycle
                    .send_message(
                        &system,
                        &metadata,
                        &cutex_session_id,
                        exact_message,
                        &external_message_id,
                    )
                    .map_err(lifecycle_error)?;
                self.set_phase(request, AgentActionPhase::MessageQueued)?;
                Some(message_id)
            } else if action.phase == AgentActionPhase::MessageQueued {
                Some(external_message_id)
            } else {
                return Err(AgentManagementError::InvalidStore);
            }
        } else {
            None
        };
        Ok(CreatedAgent {
            agent,
            observation,
            message_id,
        })
    }

    fn recover_created_agent(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        action: &AgentActionRecord,
        start_mode: AgentStartMode,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<CreatedAgent, AgentManagementError> {
        let successor = action
            .known_successor_cutex_session
            .as_ref()
            .ok_or(AgentManagementError::InvalidStore)?;
        let agent = self.active_agent(&request.project_id, successor)?;
        let observation = lifecycle.observe(successor).map_err(lifecycle_error)?;
        validate_ready(&agent, &observation)?;
        Ok(CreatedAgent {
            agent,
            observation,
            message_id: (start_mode == AgentStartMode::CustomMessage)
                .then(|| action.external_message_id.clone())
                .flatten(),
        })
    }

    fn active_agent(
        &self,
        project_id: &ProjectId,
        cutex_session_id: &CutexSessionId,
    ) -> Result<ManagedAgentRecord, AgentManagementError> {
        let snapshot = self.store.snapshot()?;
        let agent = snapshot
            .agents
            .get(cutex_session_id)
            .cloned()
            .ok_or_else(|| {
                AgentManagementError::OwnerActionRequired(
                    "Agent has no explicit Agent Management ownership record".to_string(),
                )
            })?;
        if &agent.project_id != project_id {
            return Err(AgentManagementError::Unauthorized);
        }
        if agent.retired_at.is_some() {
            return Err(AgentManagementError::OwnerActionRequired(
                "retired Agent history is immutable and outside ordinary lifecycle".to_string(),
            ));
        }
        Ok(agent)
    }

    fn online_existing(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        cutex_session_id: &CutexSessionId,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<(ManagedAgentRecord, AgentRuntimeObservation), AgentManagementError> {
        let agent = self.active_agent(&request.project_id, cutex_session_id)?;
        recover_runtime_for_agent(&agent, lifecycle)?;
        let before = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        lifecycle
            .online(cutex_session_id)
            .map_err(lifecycle_error)?;
        let after = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        validate_identity_preserved(&before, &after)?;
        validate_ready(&agent, &after)?;
        Ok((agent, after))
    }

    fn offline_existing(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        cutex_session_id: &CutexSessionId,
        occurrence_fence: Option<&RuntimeOccurrenceFence>,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<(ManagedAgentRecord, AgentRuntimeObservation), AgentManagementError> {
        let agent = self.active_agent(&request.project_id, cutex_session_id)?;
        let before = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        match occurrence_fence {
            Some(fence) => lifecycle
                .offline_if_occurrence(cutex_session_id, fence)
                .map_err(lifecycle_error)?,
            None => lifecycle
                .offline(cutex_session_id)
                .map_err(lifecycle_error)?,
        }
        let after = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        validate_identity_preserved(&before, &after)?;
        if after.app_server_runtime
            || !after.runtime_agent_ids.is_empty()
            || !after.agent_bus_endpoint_ids.is_empty()
        {
            return Err(AgentManagementError::OwnerActionRequired(
                "offline Agent still exposes a runtime endpoint".to_string(),
            ));
        }
        Ok((agent, after))
    }

    fn close_existing(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        cutex_session_id: &CutexSessionId,
        occurrence_fence: Option<&RuntimeOccurrenceFence>,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<(ManagedAgentRecord, AgentRuntimeObservation), AgentManagementError> {
        let (agent, _) =
            self.offline_existing(request, cutex_session_id, occurrence_fence, lifecycle)?;
        lifecycle
            .retire(cutex_session_id)
            .map_err(lifecycle_error)?;
        let observation = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        if observation.active
            || observation.app_server_runtime
            || !observation.runtime_agent_ids.is_empty()
            || !observation.agent_bus_endpoint_ids.is_empty()
        {
            return Err(AgentManagementError::OwnerActionRequired(
                "close did not retire the Agent and remove its endpoint".to_string(),
            ));
        }
        let retired = self.store.with_state(true, |mut state| {
            let record = state.agents.get_mut(cutex_session_id).ok_or(
                AgentManagementError::OwnerActionRequired(
                    "managed Agent ownership disappeared during close".to_string(),
                ),
            )?;
            if record.retired_at.is_none() {
                record.retired_at = Some(now());
            }
            let retired = record.clone();
            Ok((state, retired, true))
        })?;
        debug_assert_eq!(retired.project_id, agent.project_id);
        Ok((retired, observation))
    }

    /// Completes one predecessor close owned by the exact durable action. The
    /// `PredecessorClosing` intent is committed before this seam is entered.
    /// On replay, an exact already-retired predecessor may be adopted as the
    /// outcome of that intent only after its complete managed identity and
    /// endpoint-free retired state are proven.
    fn close_or_reconcile_expected_predecessor(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        cutex_session_id: &CutexSessionId,
        lifecycle: &dyn AgentLifecycle,
    ) -> Result<Option<AgentManagementResponse>, AgentManagementError> {
        if !operation_closes_expected_predecessor(request, cutex_session_id) {
            return Err(AgentManagementError::InvalidStore);
        }
        let snapshot = self.store.snapshot()?;
        let action = snapshot
            .actions
            .get(&request.action_id)
            .ok_or(AgentManagementError::InvalidStore)?;
        if action.phase != AgentActionPhase::PredecessorClosing
            || action.project_id != request.project_id
        {
            return Err(AgentManagementError::InvalidStore);
        }
        let expected = snapshot.agents.get(cutex_session_id).cloned().ok_or(
            AgentManagementError::OwnerActionRequired(
                "expected predecessor ownership record is unavailable".to_string(),
            ),
        )?;
        if expected.project_id != request.project_id {
            return Err(AgentManagementError::Unauthorized);
        }
        let observation = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        validate_managed_observation_identity(&expected, &observation)?;
        let externally_closed = !observation.active
            && !observation.app_server_runtime
            && observation.runtime_agent_ids.is_empty()
            && observation.agent_bus_endpoint_ids.is_empty();

        if expected.retired_at.is_some() {
            return if externally_closed {
                Ok(None)
            } else {
                Err(AgentManagementError::OwnerActionRequired(
                    "retired predecessor still exposes active lifecycle state".to_string(),
                ))
            };
        }
        if externally_closed {
            self.mark_expected_predecessor_retired(request, &expected)?;
            return Ok(None);
        }
        if !observation.active {
            return Err(AgentManagementError::OwnerActionRequired(
                "predecessor close outcome is partial or ambiguous".to_string(),
            ));
        }
        let (agent, _) = self.offline_existing(request, cutex_session_id, None, lifecycle)?;
        lifecycle
            .retire(cutex_session_id)
            .map_err(lifecycle_error)?;
        let after = lifecycle
            .observe(cutex_session_id)
            .map_err(lifecycle_error)?;
        validate_managed_observation_identity(&agent, &after)?;
        if after.active
            || after.app_server_runtime
            || !after.runtime_agent_ids.is_empty()
            || !after.agent_bus_endpoint_ids.is_empty()
        {
            return Err(AgentManagementError::OwnerActionRequired(
                "close did not retire the expected predecessor and remove its endpoint".to_string(),
            ));
        }
        if let Some(response) = self.inject_process_loss_after_predecessor_close(request) {
            return Ok(Some(response));
        }
        self.mark_expected_predecessor_retired(request, &expected)?;
        Ok(None)
    }

    fn mark_expected_predecessor_retired(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        expected: &ManagedAgentRecord,
    ) -> Result<(), AgentManagementError> {
        self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if action.phase != AgentActionPhase::PredecessorClosing
                || action.project_id != request.project_id
                || !operation_closes_expected_predecessor(request, &expected.cutex_session_id)
            {
                return Err(AgentManagementError::InvalidStore);
            }
            let record = state
                .agents
                .get_mut(&expected.cutex_session_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if record.project_id != request.project_id {
                return Err(AgentManagementError::Unauthorized);
            }
            if record != expected || record.retired_at.is_some() {
                return Err(AgentManagementError::OwnerActionRequired(
                    "predecessor ownership changed during close reconciliation".to_string(),
                ));
            }
            record.retired_at = Some(now());
            Ok((state, (), true))
        })
    }

    fn reject_active_collision(&self, spec: &ManagedAgentSpec) -> Result<(), AgentManagementError> {
        self.reject_active_collision_except(spec, None)
    }

    fn reject_active_collision_except(
        &self,
        spec: &ManagedAgentSpec,
        allowed_predecessor: Option<&CutexSessionId>,
    ) -> Result<(), AgentManagementError> {
        let snapshot = self.store.snapshot()?;
        if snapshot.agents.values().any(|agent| {
            agent.retired_at.is_none()
                && allowed_predecessor != Some(&agent.cutex_session_id)
                && (agent.spec.name == spec.name || agent.spec.cwd == spec.cwd)
        }) {
            return Err(AgentManagementError::Conflict("active_agent_collision"));
        }
        Ok(())
    }

    fn terminalize_pre_effect_no_write(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        error: AgentManagementError,
    ) -> AgentManagementResponse {
        let deterministic = matches!(
            &error,
            AgentManagementError::InvalidRequest(_)
                | AgentManagementError::Unauthorized
                | AgentManagementError::NotFound(_)
                | AgentManagementError::Conflict(_)
        );
        let response = error_response(&request.action_id, error);
        let recorded = self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if deterministic {
                if action.phase != AgentActionPhase::Prepared
                    || action.known_native_session_id.is_some()
                    || action.known_successor_cutex_session.is_some()
                {
                    return Ok((state, (response.clone(), None), false));
                }
                action.response = Some(response.clone());
            }
            let (_, event) = Self::record_phase_event(
                &mut state,
                request,
                if deterministic {
                    AgentActionPhase::NoWrite
                } else {
                    AgentActionPhase::Failure
                },
                deterministic,
            )?;
            Ok((state, (response.clone(), Some(event)), true))
        });
        match recorded {
            Ok((response, event)) => {
                if let Some(event) = event.as_ref() {
                    self.notify_phase(event);
                }
                response
            }
            Err(_) => response,
        }
    }

    fn require_authority(
        &self,
        project_id: &ProjectId,
        expected_session: &CutexSessionId,
        expected_epoch: u64,
    ) -> Result<(), AgentManagementError> {
        let snapshot = self.store.snapshot()?;
        let current = snapshot
            .projects
            .get(project_id)
            .ok_or(AgentManagementError::Unauthorized)?;
        if &current.authorized_director_session != expected_session
            || current.authority_epoch != expected_epoch
        {
            return Err(AgentManagementError::Conflict("stale_project_authority"));
        }
        Ok(())
    }

    fn capture_native_session(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        native_session_id: &str,
    ) -> Result<AgentActionRecord, AgentManagementError> {
        if native_session_id.trim().is_empty() {
            return Err(AgentManagementError::OwnerActionRequired(
                "native bootstrap returned an empty session identity".to_string(),
            ));
        }
        self.update_action(request, AgentActionPhase::NativeSessionCaptured, |action| {
            if let Some(existing) = action.known_native_session_id.as_deref() {
                if existing != native_session_id {
                    return Err(AgentManagementError::OwnerActionRequired(
                        "conflicting native session identities were observed".to_string(),
                    ));
                }
            }
            action.known_native_session_id = Some(native_session_id.to_string());
            Ok(())
        })
    }

    fn mark_native_bootstrap_retryable(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Result<(), AgentManagementError> {
        self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if action.phase != AgentActionPhase::NativeBootstrapPending
                || action.known_native_session_id.is_some()
                || action.known_successor_cutex_session.is_some()
            {
                return Err(AgentManagementError::InvalidStore);
            }
            action.native_bootstrap_retryable = true;
            action.updated_at = now();
            Ok((state, (), true))
        })
    }

    fn consume_native_bootstrap_retry(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
    ) -> Result<AgentActionRecord, AgentManagementError> {
        self.update_action(
            request,
            AgentActionPhase::NativeBootstrapPending,
            |action| {
                if !action.native_bootstrap_retryable
                    || action.known_native_session_id.is_some()
                    || action.known_successor_cutex_session.is_some()
                {
                    return Err(AgentManagementError::InvalidStore);
                }
                action.native_bootstrap_retryable = false;
                Ok(())
            },
        )
    }

    fn capture_adopted_agent(
        &self,
        invocation: &AgentManagementInvocation,
        request: &AuthorizedAgentManagementRequest<'_>,
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<AgentActionRecord, AgentManagementError> {
        let (action, event) = self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if action.known_native_session_id.as_deref() != Some(native_session_id) {
                return Err(AgentManagementError::InvalidStore);
            }
            if let Some(existing) = action.known_successor_cutex_session.as_ref() {
                if existing != cutex_session_id {
                    return Err(AgentManagementError::OwnerActionRequired(
                        "native session resolved to conflicting durable Agents".to_string(),
                    ));
                }
            }
            if let Some(existing) = state.agents.get(cutex_session_id) {
                if existing.project_id != request.project_id
                    || existing.native_session_id != native_session_id
                    || existing.spec != *spec
                {
                    return Err(AgentManagementError::OwnerActionRequired(
                        "durable Agent has conflicting explicit ownership".to_string(),
                    ));
                }
            } else {
                state.agents.insert(
                    cutex_session_id.clone(),
                    ManagedAgentRecord {
                        project_id: request.project_id.clone(),
                        created_by_director_session: invocation.caller_cutex_session.clone(),
                        cutex_session_id: cutex_session_id.clone(),
                        native_session_id: native_session_id.to_string(),
                        spec: spec.clone(),
                        created_at: now(),
                        retired_at: None,
                    },
                );
            }
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            action.known_successor_cutex_session = Some(cutex_session_id.clone());
            let (action, event) =
                Self::record_phase_event(&mut state, request, AgentActionPhase::Adopted, true)?;
            Ok((state, (action, event), true))
        })?;
        self.notify_phase(&event);
        Ok(action)
    }

    fn set_phase(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        phase: AgentActionPhase,
    ) -> Result<AgentActionRecord, AgentManagementError> {
        self.update_action(request, phase, |_| Ok(()))
    }

    fn update_action(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        phase: AgentActionPhase,
        update: impl FnOnce(&mut AgentActionRecord) -> Result<(), AgentManagementError>,
    ) -> Result<AgentActionRecord, AgentManagementError> {
        let (action, event) = self.store.with_state(true, |mut state| {
            update(
                state
                    .actions
                    .get_mut(&request.action_id)
                    .ok_or(AgentManagementError::InvalidStore)?,
            )?;
            let (action, event) = Self::record_phase_event(&mut state, request, phase, true)?;
            Ok((state, (action, event), true))
        })?;
        self.notify_phase(&event);
        Ok(action)
    }

    fn record_phase_event(
        state: &mut AgentManagementSnapshot,
        request: &AuthorizedAgentManagementRequest<'_>,
        phase: AgentActionPhase,
        update_current_phase: bool,
    ) -> Result<(AgentActionRecord, AgentManagementPhaseEvent), AgentManagementError> {
        let committed_at = now();
        let action = state
            .actions
            .get_mut(&request.action_id)
            .ok_or(AgentManagementError::InvalidStore)?;
        action.phase_sequence = action
            .phase_sequence
            .checked_add(1)
            .filter(|sequence| *sequence <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
            .ok_or(AgentManagementError::Conflict("phase_sequence_overflow"))?;
        if update_current_phase {
            action.phase = phase;
        }
        action.updated_at = committed_at.clone();
        let action = action.clone();

        let (predecessor, replace_policy, rotation_mode) = match &request.operation {
            AgentOperation::Replace {
                predecessor_cutex_session_id,
                policy,
                ..
            } => (
                Some(predecessor_cutex_session_id.clone()),
                Some(*policy),
                None,
            ),
            AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session,
                mode,
                ..
            } => (
                Some(expected_predecessor_cutex_session.clone()),
                None,
                Some(*mode),
            ),
            _ => (None, None, None),
        };
        let successor = action.known_successor_cutex_session.clone();
        let predecessor_phase = matches!(
            phase,
            AgentActionPhase::PredecessorClosing | AgentActionPhase::PredecessorClosed
        );
        let (subject_cutex_session_id, subject_agent_name) = match &request.operation {
            AgentOperation::Create { spec, .. } => (successor.clone(), Some(spec.name.clone())),
            AgentOperation::QueryManaged => (None, None),
            AgentOperation::Online { cutex_session_id }
            | AgentOperation::Offline { cutex_session_id }
            | AgentOperation::Restart { cutex_session_id }
            | AgentOperation::Close { cutex_session_id } => (Some(cutex_session_id.clone()), None),
            AgentOperation::Replace {
                predecessor_cutex_session_id,
                successor: successor_spec,
                ..
            } => {
                if predecessor_phase {
                    (Some(predecessor_cutex_session_id.clone()), None)
                } else {
                    (successor.clone(), Some(successor_spec.name.clone()))
                }
            }
            AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session,
                successor: successor_spec,
                ..
            } => {
                if predecessor_phase {
                    (Some(expected_predecessor_cutex_session.clone()), None)
                } else {
                    (successor.clone(), Some(successor_spec.name.clone()))
                }
            }
        };
        let transferred_rotation_phase = request.operation.kind()
            == AgentOperationKind::DirectorRotate
            && matches!(
                phase,
                AgentActionPhase::AuthorityTransferred
                    | AgentActionPhase::SuccessorReady
                    | AgentActionPhase::Complete
            );
        let presentation_owner_cutex_session_id = if transferred_rotation_phase {
            successor
                .clone()
                .ok_or(AgentManagementError::InvalidStore)?
        } else if request.operation.kind() == AgentOperationKind::DirectorRotate {
            predecessor
                .clone()
                .ok_or(AgentManagementError::InvalidStore)?
        } else {
            action.caller_cutex_session.clone()
        };
        let authority_epoch = (request.operation.kind() == AgentOperationKind::DirectorRotate)
            .then(|| {
                state
                    .projects
                    .get(&request.project_id)
                    .map(|authority| authority.authority_epoch)
                    .ok_or(AgentManagementError::InvalidStore)
            })
            .transpose()?;
        let event_id = format!(
            "agent-management:{}:phase:{}",
            request.action_id.as_str(),
            action.phase_sequence
        );
        let event = AgentManagementPhaseEvent {
            event_id: event_id.clone(),
            action_id: request.action_id.clone(),
            project_id: request.project_id.clone(),
            operation: request.operation.kind(),
            phase,
            phase_sequence: action.phase_sequence,
            committed_at,
            presentation_owner_cutex_session_id,
            subject_cutex_session_id,
            subject_agent_name,
            predecessor_cutex_session_id: predecessor,
            successor_cutex_session_id: successor,
            replace_policy,
            rotation_mode,
            authority_epoch,
        };
        if state.phase_events.insert(event_id, event.clone()).is_some() {
            return Err(AgentManagementError::InvalidStore);
        }
        Ok((action, event))
    }

    fn complete(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        digest: &Sha256,
        result: AgentManagementResult,
    ) -> Result<AgentManagementResponse, AgentManagementError> {
        let receipt = AgentManagementReceipt {
            schema: AgentManagementReceiptSchema::V1,
            action_id: request.action_id.clone(),
            request_sha256: digest.clone(),
            operation: request.operation.kind(),
            project_id: request.project_id.clone(),
            completed_at: now(),
            result,
        };
        let response = AgentManagementResponse {
            schema: AgentManagementSchema::V1,
            action_id: request.action_id.clone(),
            outcome: AgentManagementOutcome::Complete { receipt },
        };
        let (response, event) = self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            action.response = Some(response.clone());
            let (_, event) =
                Self::record_phase_event(&mut state, request, AgentActionPhase::Complete, true)?;
            Ok((state, (response.clone(), event), true))
        })?;
        self.notify_phase(&event);
        Ok(response)
    }

    fn complete_rotation(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        digest: &Sha256,
        predecessor: &CutexSessionId,
        expected_epoch: u64,
        created: CreatedAgent,
    ) -> Result<AgentManagementResponse, AgentManagementError> {
        let seat_transfer = DirectorSeatTransferRequest {
            action_id: director_seat_transfer_action_id(&request.action_id)?,
            expected_predecessor_cutex_session: predecessor.clone(),
            successor_cutex_session: created.agent.cutex_session_id.clone(),
        };
        self.director_seats
            .transfer_director(&seat_transfer)
            .map_err(seat_authority_error)?;
        if let Some(response) = self.inject_process_loss_after_director_seat_transfer(request) {
            return Ok(response);
        }
        let (response, events) = self.store.with_state(true, |mut state| {
            let current = state
                .projects
                .get(&request.project_id)
                .cloned()
                .ok_or(AgentManagementError::Unauthorized)?;
            if &current.authorized_director_session != predecessor
                || current.authority_epoch != expected_epoch
            {
                return Err(AgentManagementError::Conflict("stale_project_authority"));
            }
            let next_epoch = expected_epoch
                .checked_add(1)
                .filter(|epoch| *epoch <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
                .ok_or(AgentManagementError::Conflict("authority_epoch_overflow"))?;
            let authority = ProjectAuthority {
                project_id: request.project_id.clone(),
                authorized_director_session: created.agent.cutex_session_id.clone(),
                authority_epoch: next_epoch,
                updated_at: now(),
            };
            let receipt = AgentManagementReceipt {
                schema: AgentManagementReceiptSchema::V1,
                action_id: request.action_id.clone(),
                request_sha256: digest.clone(),
                operation: AgentOperationKind::DirectorRotate,
                project_id: request.project_id.clone(),
                completed_at: now(),
                result: AgentManagementResult::DirectorRotated {
                    predecessor_cutex_session_id: predecessor.clone(),
                    successor: created.agent,
                    observation: created.observation,
                    authority: authority.clone(),
                    message_id: created.message_id,
                },
            };
            let response = AgentManagementResponse {
                schema: AgentManagementSchema::V1,
                action_id: request.action_id.clone(),
                outcome: AgentManagementOutcome::Complete { receipt },
            };
            state.projects.insert(request.project_id.clone(), authority);
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            action.response = Some(response.clone());
            let events = vec![
                Self::record_phase_event(
                    &mut state,
                    request,
                    AgentActionPhase::AuthorityTransferred,
                    true,
                )?
                .1,
                Self::record_phase_event(
                    &mut state,
                    request,
                    AgentActionPhase::SuccessorReady,
                    true,
                )?
                .1,
                Self::record_phase_event(&mut state, request, AgentActionPhase::Complete, true)?.1,
            ];
            Ok((state, (response, events), true))
        })?;
        for event in &events {
            self.notify_phase(event);
        }
        self.director_seats
            .finish_director_transfer(&seat_transfer)
            .map_err(seat_authority_error)?;
        self.director_seats
            .verify_director_transfer(&seat_transfer)
            .map_err(seat_authority_error)?;
        Ok(response)
    }

    fn reconcile_completed_rotation(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        response: &AgentManagementResponse,
    ) -> Result<(), AgentManagementError> {
        let (
            AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session,
                ..
            },
            AgentManagementOutcome::Complete { receipt },
        ) = (&request.operation, &response.outcome)
        else {
            return Ok(());
        };
        let AgentManagementResult::DirectorRotated {
            successor,
            authority,
            ..
        } = &receipt.result
        else {
            return Err(AgentManagementError::InvalidStore);
        };
        let project_authority = self
            .store
            .snapshot()?
            .projects
            .get(&request.project_id)
            .cloned()
            .ok_or(AgentManagementError::InvalidStore)?;
        if &project_authority != authority
            || authority.authorized_director_session != successor.cutex_session_id
        {
            return Err(AgentManagementError::Conflict(
                "director_authority_surfaces_disagree",
            ));
        }
        let transfer = DirectorSeatTransferRequest {
            action_id: director_seat_transfer_action_id(&request.action_id)?,
            expected_predecessor_cutex_session: expected_predecessor_cutex_session.clone(),
            successor_cutex_session: successor.cutex_session_id.clone(),
        };
        self.director_seats
            .finish_director_transfer(&transfer)
            .map_err(seat_authority_error)?;
        self.director_seats
            .verify_director_transfer(&transfer)
            .map_err(seat_authority_error)?;
        Ok(())
    }

    fn owner_action_required(
        &self,
        request: &AuthorizedAgentManagementRequest<'_>,
        detail: &str,
    ) -> Result<AgentManagementResponse, AgentManagementError> {
        let (response, phase_event) = self.store.with_state(true, |mut state| {
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if let Some(response) = action.response.clone() {
                return Ok((state, (response, None), false));
            }
            let resumable_continuation =
                action.known_native_session_id.is_some() || action.native_bootstrap_retryable;
            let event_id = format!("agent-management:{}:failure", request.action_id.as_str());
            let authority = state.projects.get(&request.project_id).cloned();
            let route_to_director_session = authority
                .as_ref()
                .map(|authority| authority.authorized_director_session.clone());
            let target_cutex_session_id = match &request.operation {
                AgentOperation::Online { cutex_session_id }
                | AgentOperation::Offline { cutex_session_id }
                | AgentOperation::Restart { cutex_session_id }
                | AgentOperation::Close { cutex_session_id } => Some(cutex_session_id.clone()),
                AgentOperation::Create { .. }
                | AgentOperation::QueryManaged
                | AgentOperation::Replace { .. }
                | AgentOperation::DirectorRotate { .. } => None,
            };
            let failure = AgentManagementFailureEvent {
                schema: AgentManagementFailureSchema::V1,
                event_id: event_id.clone(),
                action_id: request.action_id.clone(),
                project_id: request.project_id.clone(),
                operation: request.operation.kind(),
                code: "owner_action_required".to_string(),
                detail: detail.to_string(),
                routing_status: if route_to_director_session.is_some() {
                    FailureRoutingStatus::Routable
                } else {
                    FailureRoutingStatus::Unrouted
                },
                route_to_director_session,
                target_cutex_session_id,
                created_at: now(),
            };
            let response = AgentManagementResponse {
                schema: AgentManagementSchema::V1,
                action_id: request.action_id.clone(),
                outcome: AgentManagementOutcome::OwnerActionRequired {
                    failure: failure.clone(),
                },
            };
            state.failure_events.insert(event_id, failure);
            let action = state
                .actions
                .get_mut(&request.action_id)
                .ok_or(AgentManagementError::InvalidStore)?;
            if !resumable_continuation {
                action.response = Some(response.clone());
            }
            let (_, event) = Self::record_phase_event(
                &mut state,
                request,
                AgentActionPhase::OwnerActionRequired,
                !resumable_continuation,
            )?;
            Ok((state, (response, Some(event)), true))
        })?;
        if let Some(event) = phase_event.as_ref() {
            self.notify_phase(event);
        }
        Ok(response)
    }
}

struct AuthorizedAgentManagementRequest<'a> {
    project_id: ProjectId,
    request: &'a AgentManagementRequest,
}

impl std::ops::Deref for AuthorizedAgentManagementRequest<'_> {
    type Target = AgentManagementRequest;

    fn deref(&self) -> &Self::Target {
        self.request
    }
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)] // Internal state-machine values avoid allocation on every phase.
enum BeginAction {
    Replay(AgentManagementResponse),
    Execute(AgentActionRecord),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HistoricalBootstrapContinuation {
    None,
    RetryProvenAbsent,
    CaptureExactSid(String),
    RetryLifecycleAfterOffline(RuntimeOccurrenceFence),
}

fn legacy_offline_revision_conflict_candidate(action: &AgentActionRecord) -> bool {
    matches!(
        action.operation,
        AgentOperationKind::Offline | AgentOperationKind::Restart | AgentOperationKind::Close
    ) && action.phase == AgentActionPhase::OwnerActionRequired
        && matches!(
            action.response.as_ref().map(|response| &response.outcome),
            Some(AgentManagementOutcome::OwnerActionRequired { failure })
                if offline_revision_conflict_detail_matches(&failure.detail)
        )
}

fn offline_revision_conflict_detail_matches(detail: &str) -> bool {
    let detail = detail
        .strip_prefix("owner_action_required: ")
        .unwrap_or(detail);
    let Some(detail) = detail
        .strip_prefix("session_offline_failed: cutex session store revision conflict: expected ")
    else {
        return false;
    };
    let Some(detail) = detail.strip_suffix(" (external outcome unknown)") else {
        return false;
    };
    let Some((expected, current)) = detail.split_once(", current ") else {
        return false;
    };
    let Some(current) = current.strip_suffix("; reload before retrying") else {
        return false;
    };
    expected.parse::<u64>().is_ok() && current.parse::<u64>().is_ok()
}

struct CreatedAgent {
    agent: ManagedAgentRecord,
    observation: AgentRuntimeObservation,
    message_id: Option<String>,
}

fn director_seat_transfer_action_id(
    action_id: &AgentActionId,
) -> Result<ActionId, AgentManagementError> {
    let digest = Sha256Digest::digest(action_id.as_str().as_bytes());
    ActionId::new(format!("agent-management/director-rotate-seat/{digest:x}"))
        .map_err(|_| AgentManagementError::InvalidStore)
}

fn seat_authority_error(error: SeatAuthorityError) -> AgentManagementError {
    match error {
        SeatAuthorityError::InvalidRequest(reason) => AgentManagementError::InvalidRequest(reason),
        SeatAuthorityError::Conflict(reason) => AgentManagementError::Conflict(reason),
        SeatAuthorityError::Unauthorized => {
            AgentManagementError::Conflict("task_service_director_seat_unauthorized")
        }
        SeatAuthorityError::PersistenceUnavailable | SeatAuthorityError::Io(_) => {
            AgentManagementError::PersistenceUnavailable
        }
        SeatAuthorityError::InvalidStore => AgentManagementError::InvalidStore,
    }
}

/// Identifies the narrow historical receipt that is eligible for authoritative
/// reconciliation. The receipt text does not itself authorize a retry: the
/// lifecycle provider must separately prove native/runtime absence before the
/// cached response is reopened. New attempts persist their retry classification
/// directly.
fn legacy_pre_sid_retry_candidate(action: &AgentActionRecord) -> bool {
    action.operation == AgentOperationKind::Create
        && action.phase == AgentActionPhase::OwnerActionRequired
        && action.known_native_session_id.is_none()
        && action.known_successor_cutex_session.is_none()
        && !action.native_bootstrap_retryable
        && matches!(
            action.response.as_ref().map(|response| &response.outcome),
            Some(AgentManagementOutcome::OwnerActionRequired { failure })
                if legacy_pre_sid_failure_detail_matches(&failure.detail)
        )
}

fn legacy_pre_sid_failure_detail_matches(detail: &str) -> bool {
    // One historical provider path wrapped the lifecycle detail once before
    // persisting it. Normalize only that exact known prefix; the remaining
    // text is still merely a reconciliation trigger, never absence proof.
    let detail = detail
        .strip_prefix("owner_action_required: ")
        .unwrap_or(detail);
    let detail = detail
        .strip_suffix(" (external outcome unknown)")
        .unwrap_or(detail);
    let native_session_unknown = detail
        .strip_prefix("native_session_unknown: native exec exited ")
        .and_then(|detail| detail.split_once(" without one captured SID"))
        .is_some_and(|(status, diagnostic)| {
            !status.is_empty()
                && (diagnostic.is_empty() || diagnostic.starts_with("; diagnostic: "))
        });
    native_session_unknown
        || detail
            == "native_bootstrap_output_malformed: native exec JSONL stdout line 1 is not valid JSON"
}

/// Identifies only the one historical receipt class produced after the native
/// bootstrap had already run but legacy UUID scraping could not select its SID.
/// Receipt text is a trigger for evidence reconciliation, never identity proof.
fn legacy_ambiguous_sid_recovery_candidate(action: &AgentActionRecord) -> bool {
    action.operation == AgentOperationKind::Create
        && action.phase == AgentActionPhase::OwnerActionRequired
        && action.known_native_session_id.is_none()
        && action.known_successor_cutex_session.is_none()
        && !action.native_bootstrap_retryable
        && matches!(
            action.response.as_ref().map(|response| &response.outcome),
            Some(AgentManagementOutcome::OwnerActionRequired { failure })
                if legacy_ambiguous_sid_failure_detail_matches(&failure.detail)
        )
}

fn legacy_ambiguous_sid_failure_detail_matches(detail: &str) -> bool {
    let detail = detail
        .strip_prefix("owner_action_required: ")
        .unwrap_or(detail);
    matches!(
        detail,
        "native_session_ambiguous: native exec exposed multiple possible session identities"
            | "native_session_ambiguous: native exec exposed multiple possible session identities (external outcome unknown)"
    )
}

fn latest_native_bootstrap_window(
    snapshot: &AgentManagementSnapshot,
    action: &AgentActionRecord,
) -> Option<(crate::role_revision::Rfc3339, crate::role_revision::Rfc3339)> {
    let started_at = snapshot
        .phase_events
        .values()
        .filter(|event| {
            event.action_id == action.action_id
                && event.phase == AgentActionPhase::NativeBootstrapPending
        })
        .max_by_key(|event| event.phase_sequence)?
        .committed_at
        .clone();
    (started_at.as_str() <= action.updated_at.as_str())
        .then(|| (started_at, action.updated_at.clone()))
}

/// Returns an invocation-only projection. The historical action, response, and
/// failure event remain byte-for-byte unchanged in the durable store.
fn reconciliation_fence_response(
    action: &AgentActionRecord,
    status: &str,
    reason: &str,
) -> AgentManagementResponse {
    let Some(mut response) = action.response.clone() else {
        return no_write(
            &action.action_id,
            "invalid_store",
            "native bootstrap reconciliation did not find the immutable receipt",
        );
    };
    if let AgentManagementOutcome::OwnerActionRequired { failure } = &mut response.outcome {
        if legacy_offline_revision_conflict_candidate(action) {
            failure.event_id = format!(
                "agent-management:{}:runtime-occurrence-reconciliation:{status}",
                action.action_id
            );
            failure.code = format!("runtime_occurrence_reconciliation_{status}");
            failure.detail = format!(
                "historical lifecycle replay remains fenced ({status}): {reason}; immutable original receipt remains unchanged"
            );
            return response;
        }
        failure.event_id = format!(
            "agent-management:{}:native-bootstrap-reconciliation:{status}",
            action.action_id
        );
        failure.code = format!("native_bootstrap_reconciliation_{status}");
        failure.detail = format!(
            "historical create retry remains fenced ({status}): {reason}; immutable original receipt remains unchanged"
        );
    }
    response
}

fn operation_message<'a>(request: &'a AuthorizedAgentManagementRequest<'a>) -> Option<&'a str> {
    match &request.operation {
        AgentOperation::Create { frozen_message, .. }
        | AgentOperation::Replace { frozen_message, .. }
        | AgentOperation::DirectorRotate { frozen_message, .. } => frozen_message.as_deref(),
        _ => None,
    }
}

fn operation_reservation<'a>(
    request: &'a AuthorizedAgentManagementRequest<'a>,
) -> Option<&'a ManagedAgentSpec> {
    match &request.operation {
        AgentOperation::Create { spec, .. } => Some(spec),
        AgentOperation::Replace { successor, .. }
        | AgentOperation::DirectorRotate { successor, .. } => Some(successor),
        AgentOperation::QueryManaged
        | AgentOperation::Online { .. }
        | AgentOperation::Offline { .. }
        | AgentOperation::Restart { .. }
        | AgentOperation::Close { .. } => None,
    }
}

fn operation_closes_expected_predecessor(
    request: &AuthorizedAgentManagementRequest<'_>,
    cutex_session_id: &CutexSessionId,
) -> bool {
    match &request.operation {
        AgentOperation::Replace {
            predecessor_cutex_session_id,
            policy: AgentReplacePolicy::CloseBeforeCreate | AgentReplacePolicy::CloseAfterReady,
            ..
        } => predecessor_cutex_session_id == cutex_session_id,
        AgentOperation::DirectorRotate {
            expected_predecessor_cutex_session,
            mode: DirectorRotateMode::ClosePredecessorThenCreateWithMessage,
            ..
        } => expected_predecessor_cutex_session == cutex_session_id,
        _ => false,
    }
}

fn lifecycle_error(error: LifecycleFailure) -> AgentManagementError {
    let ambiguity = if error.outcome_unknown {
        " (external outcome unknown)"
    } else {
        ""
    };
    AgentManagementError::OwnerActionRequired(format!(
        "{}: {}{}",
        error.code, error.detail, ambiguity
    ))
}

fn recover_runtime_for_agent(
    agent: &ManagedAgentRecord,
    lifecycle: &dyn AgentLifecycle,
) -> Result<RuntimeRecoveryOutcome, AgentManagementError> {
    let before = lifecycle
        .observe(&agent.cutex_session_id)
        .map_err(lifecycle_error)?;
    validate_recovery_spec(agent, &before)?;
    let outcome = lifecycle
        .recover_runtime(
            &agent.cutex_session_id,
            &agent.native_session_id,
            &agent.spec,
        )
        .map_err(lifecycle_error)?;
    let after = lifecycle
        .observe(&agent.cutex_session_id)
        .map_err(lifecycle_error)?;
    validate_recovery_spec(agent, &after)?;
    validate_identity_preserved(&before, &after)?;
    if after.runtime_generation != before.runtime_generation {
        return Err(AgentManagementError::OwnerActionRequired(
            "runtime recovery changed the claimed generation".to_string(),
        ));
    }
    match outcome {
        RuntimeRecoveryOutcome::NoClaim if before != after => {
            Err(AgentManagementError::OwnerActionRequired(
                "no-claim recovery mutated runtime state".to_string(),
            ))
        }
        RuntimeRecoveryOutcome::RecoveredExact => {
            if before.runtime_generation == 0 || before.runtime_agent_ids != after.runtime_agent_ids
            {
                return Err(AgentManagementError::OwnerActionRequired(
                    "recovered runtime does not preserve its claimed identity".to_string(),
                ));
            }
            validate_ready(agent, &after)?;
            Ok(outcome)
        }
        RuntimeRecoveryOutcome::ClearedDeadClaim => {
            if after.app_server_runtime
                || !after.runtime_agent_ids.is_empty()
                || !after.agent_bus_endpoint_ids.is_empty()
            {
                return Err(AgentManagementError::OwnerActionRequired(
                    "dead runtime recovery did not clear the ownership claim".to_string(),
                ));
            }
            Ok(outcome)
        }
        RuntimeRecoveryOutcome::NoClaim => Ok(outcome),
    }
}

fn validate_recovery_spec(
    agent: &ManagedAgentRecord,
    observation: &AgentRuntimeObservation,
) -> Result<(), AgentManagementError> {
    let spec = &agent.spec;
    let groups_match = observation.groups == spec.groups
        || observation.groups == expected_runtime_groups(&spec.cwd, &spec.groups);
    if observation.active
        && observation.cutex_session_id == agent.cutex_session_id
        && observation.native_session_id == agent.native_session_id
        && observation.cwd == spec.cwd
        && observation.profile == spec.profile
        && observation.runtime_backend == spec.runtime_backend
        && observation.model == spec.model
        && observation.reasoning == spec.reasoning
        && observation.permissions == spec.permissions
        && observation.approval_policy == spec.approval_policy
        && observation.sandbox_mode == spec.sandbox_mode
        && groups_match
    {
        Ok(())
    } else {
        Err(AgentManagementError::OwnerActionRequired(
            "runtime recovery durable/native identity or managed spec mismatch".to_string(),
        ))
    }
}

fn validate_managed_observation_identity(
    agent: &ManagedAgentRecord,
    observation: &AgentRuntimeObservation,
) -> Result<(), AgentManagementError> {
    let spec = &agent.spec;
    let groups_match = observation.groups == spec.groups
        || observation.groups == expected_runtime_groups(&spec.cwd, &spec.groups);
    if observation.cutex_session_id == agent.cutex_session_id
        && observation.native_session_id == agent.native_session_id
        && observation.cwd == spec.cwd
        && observation.profile == spec.profile
        && observation.runtime_backend == spec.runtime_backend
        && observation.model == spec.model
        && observation.reasoning == spec.reasoning
        && observation.permissions == spec.permissions
        && observation.approval_policy == spec.approval_policy
        && observation.sandbox_mode == spec.sandbox_mode
        && groups_match
        && observation.runtime_generation > 0
    {
        Ok(())
    } else {
        Err(AgentManagementError::OwnerActionRequired(
            "predecessor durable/native identity or managed spec mismatch".to_string(),
        ))
    }
}

fn validate_ready(
    agent: &ManagedAgentRecord,
    observation: &AgentRuntimeObservation,
) -> Result<(), AgentManagementError> {
    let spec = &agent.spec;
    let one_runtime = observation.runtime_agent_ids.as_slice();
    let one_endpoint = observation.agent_bus_endpoint_ids.as_slice();
    let exact = observation.active
        && observation.cutex_session_id == agent.cutex_session_id
        && observation.native_session_id == agent.native_session_id
        && observation.cwd == spec.cwd
        && observation.profile == spec.profile
        && observation.runtime_backend == spec.runtime_backend
        && observation.model == spec.model
        && observation.reasoning == spec.reasoning
        && observation.permissions == spec.permissions
        && observation.approval_policy == spec.approval_policy
        && observation.sandbox_mode == spec.sandbox_mode
        && observation.groups == expected_runtime_groups(&spec.cwd, &spec.groups)
        && observation.runtime_generation > 0
        && observation.app_server_runtime
        && matches!((one_runtime, one_endpoint), ([runtime], [endpoint]) if runtime == endpoint);
    if exact {
        Ok(())
    } else {
        Err(AgentManagementError::OwnerActionRequired(
            "Agent readiness does not match the exact durable specification and sole endpoint"
                .to_string(),
        ))
    }
}

fn expected_runtime_groups(cwd: &str, groups: &[String]) -> Vec<String> {
    let cwd_hash = crate::agent_bus::identity::fnv1a_hex(cwd);
    crate::agent_bus::groups::normalize_registered_agent_groups(
        groups.to_vec(),
        Some(&cwd_hash[..7]),
        cwd,
    )
}

fn validate_identity_preserved(
    before: &AgentRuntimeObservation,
    after: &AgentRuntimeObservation,
) -> Result<(), AgentManagementError> {
    if before.cutex_session_id != after.cutex_session_id
        || before.native_session_id != after.native_session_id
    {
        Err(AgentManagementError::OwnerActionRequired(
            "lifecycle operation changed durable Agent or native thread identity".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn error_response(
    action_id: &AgentActionId,
    error: AgentManagementError,
) -> AgentManagementResponse {
    no_write(action_id, error.code(), &error.to_string())
}

fn no_write(action_id: &AgentActionId, code: &str, detail: &str) -> AgentManagementResponse {
    AgentManagementResponse {
        schema: AgentManagementSchema::V1,
        action_id: action_id.clone(),
        outcome: AgentManagementOutcome::NoWrite {
            code: code.to_string(),
            detail: detail.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::*;

    const REAL_LEGACY_NO_SID_DETAIL: &str =
        "owner_action_required: native_session_unknown: native exec exited exit status: 1 without one captured SID";
    const REAL_CURRENT_NO_SID_DETAIL: &str =
        "owner_action_required: native_session_unknown: native exec exited exit status: 1 without one captured SID (external outcome unknown)";
    const REAL_LEGACY_AMBIGUOUS_SID_DETAIL: &str =
        "owner_action_required: native_session_ambiguous: native exec exposed multiple possible session identities (external outcome unknown)";
    const REAL_MALFORMED_JSONL_DETAIL: &str =
        "native_bootstrap_output_malformed: native exec JSONL stdout line 1 is not valid JSON (external outcome unknown)";
    const REAL_OFFLINE_REVISION_CONFLICT_DETAIL: &str = "owner_action_required: session_offline_failed: cutex session store revision conflict: expected 1163254, current 1163259; reload before retrying (external outcome unknown)";
    const RECOVERED_NATIVE_SID: &str = "01a041ba-47f6-7e31-bb09-1462cd309ae4";

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    enum FakeRecovery {
        #[default]
        NoClaim,
        ExactLive,
        DeadClaim,
        MismatchedGeneration,
        Ambiguous,
    }

    #[derive(Default)]
    struct FakeState {
        log: Vec<String>,
        next_agent: u64,
        known_sid_bootstrap_failure: Option<String>,
        definite_pre_sid_bootstrap_failures: usize,
        uncertain_pre_sid_bootstrap_failures: usize,
        reconciliation_override: Option<NativeBootstrapReconciliation>,
        identity_reconciliation_override: Option<NativeBootstrapIdentityReconciliation>,
        occurrence_reconciliation_override: Option<HistoricalRuntimeOccurrenceReconciliation>,
        advance_before_fenced_restart_check: bool,
        advance_after_fenced_restart_check: bool,
        extra_online_groups: Vec<String>,
        recovery: FakeRecovery,
        launch_count: usize,
        agents: BTreeMap<CutexSessionId, AgentRuntimeObservation>,
        messages: HashMap<String, String>,
    }

    #[derive(Default)]
    struct FakeLifecycle {
        state: Mutex<FakeState>,
        ordering_log: Option<Arc<Mutex<Vec<String>>>>,
    }

    impl FakeLifecycle {
        fn fail_bootstrap_once_with_known_sid(native_session_id: &str) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    known_sid_bootstrap_failure: Some(native_session_id.to_string()),
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn fail_bootstrap_once_before_sid() -> Self {
            Self {
                state: Mutex::new(FakeState {
                    definite_pre_sid_bootstrap_failures: 1,
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn fail_bootstrap_once_with_uncertain_sid() -> Self {
            Self {
                state: Mutex::new(FakeState {
                    uncertain_pre_sid_bootstrap_failures: 1,
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn with_extra_online_group(group: &str) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    extra_online_groups: vec![group.to_string()],
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn with_reconciliation(outcome: NativeBootstrapReconciliation) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    reconciliation_override: Some(outcome),
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn with_identity_reconciliation(outcome: NativeBootstrapIdentityReconciliation) -> Self {
            Self {
                state: Mutex::new(FakeState {
                    identity_reconciliation_override: Some(outcome),
                    ..FakeState::default()
                }),
                ordering_log: None,
            }
        }

        fn with_ordering_log(ordering_log: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                state: Mutex::new(FakeState::default()),
                ordering_log: Some(ordering_log),
            }
        }

        fn log(&self) -> Vec<String> {
            self.state.lock().unwrap().log.clone()
        }

        fn bootstrap_count(&self) -> usize {
            self.log()
                .iter()
                .filter(|entry| entry.as_str() == "bootstrap:Hi.")
                .count()
        }

        fn message_count(&self) -> usize {
            self.log()
                .iter()
                .filter(|entry| entry.starts_with("message:"))
                .count()
        }

        fn retire_count(&self, cutex_session_id: &CutexSessionId) -> usize {
            let expected = format!("retire:{}", cutex_session_id.as_str());
            self.log()
                .iter()
                .filter(|entry| entry.as_str() == expected)
                .count()
        }

        fn corrupt_native_session_id(&self, cutex_session_id: &CutexSessionId) {
            self.state
                .lock()
                .unwrap()
                .agents
                .get_mut(cutex_session_id)
                .unwrap()
                .native_session_id = "native-conflict".to_string();
        }

        fn advance_runtime_occurrence(&self, cutex_session_id: &CutexSessionId) {
            let mut state = self.state.lock().unwrap();
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            agent.runtime_generation += 1;
            let runtime_id = format!(
                "runtime:{}:{}",
                cutex_session_id.as_str(),
                agent.runtime_generation
            );
            agent.active = true;
            agent.app_server_runtime = true;
            agent.runtime_agent_ids = vec![runtime_id.clone()];
            agent.agent_bus_endpoint_ids = vec![runtime_id];
        }

        fn set_occurrence_reconciliation(
            &self,
            outcome: HistoricalRuntimeOccurrenceReconciliation,
        ) {
            self.state
                .lock()
                .unwrap()
                .occurrence_reconciliation_override = Some(outcome);
        }

        fn advance_before_fenced_restart_check(&self) {
            self.state
                .lock()
                .unwrap()
                .advance_before_fenced_restart_check = true;
        }

        fn advance_after_fenced_restart_check(&self) {
            self.state
                .lock()
                .unwrap()
                .advance_after_fenced_restart_check = true;
        }

        fn offline_count(&self, cutex_session_id: &CutexSessionId) -> usize {
            let expected = format!("offline:{}", cutex_session_id.as_str());
            self.log()
                .iter()
                .filter(|entry| entry.as_str() == expected)
                .count()
        }

        fn online_count(&self, cutex_session_id: &CutexSessionId) -> usize {
            let expected = format!("online:{}", cutex_session_id.as_str());
            self.log()
                .iter()
                .filter(|entry| entry.as_str() == expected)
                .count()
        }

        fn set_groups(&self, cutex_session_id: &CutexSessionId, groups: Vec<String>) {
            let mut state = self.state.lock().unwrap();
            state.extra_online_groups.clear();
            state.agents.get_mut(cutex_session_id).unwrap().groups = groups;
        }

        fn disconnect_with_claim(&self, cutex_session_id: &CutexSessionId, recovery: FakeRecovery) {
            let mut state = self.state.lock().unwrap();
            state.recovery = recovery;
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            assert_eq!(agent.runtime_agent_ids.len(), 1);
            agent.app_server_runtime = false;
            agent.agent_bus_endpoint_ids.clear();
        }

        fn launch_count(&self) -> usize {
            self.state.lock().unwrap().launch_count
        }

        fn recovery_count(&self) -> usize {
            self.log()
                .iter()
                .filter(|entry| entry.starts_with("recover:"))
                .count()
        }

        fn insert_agent(
            &self,
            cutex_session_id: &CutexSessionId,
            native_session_id: &str,
            spec: &ManagedAgentSpec,
        ) {
            let mut observed = observation(cutex_session_id, native_session_id, spec, true, 1);
            let runtime_id = format!("runtime:{}:1", cutex_session_id.as_str());
            observed.runtime_agent_ids = vec![runtime_id.clone()];
            observed.agent_bus_endpoint_ids = vec![runtime_id];
            observed.app_server_runtime = true;
            self.state
                .lock()
                .unwrap()
                .agents
                .insert(cutex_session_id.clone(), observed);
        }
    }

    impl AgentLifecycle for FakeLifecycle {
        fn prepare_private_cwd(&self, spec: &ManagedAgentSpec) -> Result<(), LifecycleFailure> {
            self.state
                .lock()
                .unwrap()
                .log
                .push(format!("prepare:{}", spec.name));
            Ok(())
        }

        fn bootstrap_native(&self, _spec: &ManagedAgentSpec) -> Result<String, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.next_agent += 1;
            state.log.push("bootstrap:Hi.".to_string());
            if let Some(native_session_id) = state.known_sid_bootstrap_failure.take() {
                return Err(LifecycleFailure {
                    code: "native_bootstrap_failed".to_string(),
                    detail: "native exec failed after exposing one exact SID".to_string(),
                    outcome_unknown: true,
                    known_native_session_id: Some(native_session_id),
                });
            }
            if state.definite_pre_sid_bootstrap_failures > 0 {
                state.definite_pre_sid_bootstrap_failures -= 1;
                return Err(LifecycleFailure::definite(
                    "native_bootstrap_pre_sid_failed",
                    "native wrapper exited before publishing a SID",
                ));
            }
            if state.uncertain_pre_sid_bootstrap_failures > 0 {
                state.uncertain_pre_sid_bootstrap_failures -= 1;
                return Err(LifecycleFailure::outcome_unknown(
                    "native_session_unknown",
                    "native wrapper outcome may include an unpublished SID",
                ));
            }
            Ok(format!("native-{}", state.next_agent))
        }

        fn reconcile_pre_sid_bootstrap(
            &self,
            spec: &ManagedAgentSpec,
            _started_at: &crate::role_revision::Rfc3339,
            _failed_at: &crate::role_revision::Rfc3339,
        ) -> Result<NativeBootstrapReconciliation, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.log.push("reconcile-pre-sid".to_string());
            if let Some(outcome) = state.reconciliation_override.clone() {
                return Ok(outcome);
            }
            if state.agents.values().any(|agent| agent.cwd == spec.cwd) {
                Ok(NativeBootstrapReconciliation::Present {
                    reason: "fake exact managed runtime is present".to_string(),
                })
            } else {
                Ok(NativeBootstrapReconciliation::ProvenAbsent {
                    reason: "fake provider sources are empty".to_string(),
                })
            }
        }

        fn reconcile_ambiguous_native_bootstrap(
            &self,
            _spec: &ManagedAgentSpec,
            _started_at: &crate::role_revision::Rfc3339,
            _failed_at: &crate::role_revision::Rfc3339,
        ) -> Result<NativeBootstrapIdentityReconciliation, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.log.push("reconcile-ambiguous-sid".to_string());
            Ok(state.identity_reconciliation_override.clone().unwrap_or(
                NativeBootstrapIdentityReconciliation::Absent {
                    reason: "fake selected-profile sources are empty".to_string(),
                },
            ))
        }

        fn reconcile_historical_runtime_occurrence(
            &self,
            cutex_session_id: &CutexSessionId,
        ) -> Result<HistoricalRuntimeOccurrenceReconciliation, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.log.push(format!(
                "reconcile-occurrence:{}",
                cutex_session_id.as_str()
            ));
            if let Some(outcome) = state.occurrence_reconciliation_override.clone() {
                return Ok(outcome);
            }
            let observed = state
                .agents
                .get(cutex_session_id)
                .cloned()
                .ok_or_else(|| LifecycleFailure::definite("not_found", "missing fake Agent"))?;
            if observed.app_server_runtime
                || !observed.runtime_agent_ids.is_empty()
                || !observed.agent_bus_endpoint_ids.is_empty()
            {
                return Ok(HistoricalRuntimeOccurrenceReconciliation::Present {
                    reason: "fake provider has an active runtime occurrence".to_string(),
                });
            }
            Ok(HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent {
                fence: RuntimeOccurrenceFence {
                    runtime_generation: observed.runtime_generation,
                    current_runtime_agent_id: None,
                    agent_bus_endpoint_ids: Vec::new(),
                    pending_launch_id: None,
                    app_server_launch_claim_id: None,
                    alden_session_name: None,
                    alden_pid: None,
                    runtime_pid: None,
                    app_server_pid: None,
                    app_server_endpoint: None,
                    app_server_connected: false,
                },
                reason: "fake provider sources prove the occurrence absent".to_string(),
            })
        }

        fn adopt_native(
            &self,
            native_session_id: &str,
            spec: &ManagedAgentSpec,
        ) -> Result<CutexSessionId, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.log.push(format!("adopt:{native_session_id}"));
            if let Some(existing) = state
                .agents
                .values()
                .find(|agent| agent.native_session_id == native_session_id)
            {
                return Ok(existing.cutex_session_id.clone());
            }
            let suffix = native_session_id
                .strip_prefix("native-")
                .unwrap_or(native_session_id);
            let cutex_session_id = CutexSessionId::new(format!("cutex.agent-{suffix}")).unwrap();
            state.agents.insert(
                cutex_session_id.clone(),
                observation(&cutex_session_id, native_session_id, spec, true, 0),
            );
            Ok(cutex_session_id)
        }

        fn configure(
            &self,
            cutex_session_id: &CutexSessionId,
            _native_session_id: &str,
            _spec: &ManagedAgentSpec,
        ) -> Result<(), LifecycleFailure> {
            self.state
                .lock()
                .unwrap()
                .log
                .push(format!("configure:{}", cutex_session_id.as_str()));
            Ok(())
        }

        fn recover_runtime(
            &self,
            cutex_session_id: &CutexSessionId,
            native_session_id: &str,
            spec: &ManagedAgentSpec,
        ) -> Result<RuntimeRecoveryOutcome, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state
                .log
                .push(format!("recover:{}", cutex_session_id.as_str()));
            let recovery = state.recovery;
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            if agent.native_session_id != native_session_id
                || agent.cwd != spec.cwd
                || agent.profile != spec.profile
                || agent.runtime_backend != spec.runtime_backend
                || agent.model != spec.model
                || agent.reasoning != spec.reasoning
                || agent.permissions != spec.permissions
                || agent.approval_policy != spec.approval_policy
                || agent.sandbox_mode != spec.sandbox_mode
            {
                return Err(LifecycleFailure::definite(
                    "runtime_recovery_spec_mismatch",
                    "managed runtime recovery identity/spec mismatch",
                ));
            }
            match recovery {
                FakeRecovery::NoClaim => Ok(RuntimeRecoveryOutcome::NoClaim),
                FakeRecovery::ExactLive => {
                    let [runtime_id] = agent.runtime_agent_ids.as_slice() else {
                        return Err(LifecycleFailure::definite(
                            "runtime_recovery_identity_missing",
                            "exact live claim omitted its runtime identity",
                        ));
                    };
                    agent.app_server_runtime = true;
                    agent.agent_bus_endpoint_ids = vec![runtime_id.clone()];
                    state.recovery = FakeRecovery::NoClaim;
                    Ok(RuntimeRecoveryOutcome::RecoveredExact)
                }
                FakeRecovery::DeadClaim => {
                    agent.app_server_runtime = false;
                    agent.runtime_agent_ids.clear();
                    agent.agent_bus_endpoint_ids.clear();
                    state.recovery = FakeRecovery::NoClaim;
                    Ok(RuntimeRecoveryOutcome::ClearedDeadClaim)
                }
                FakeRecovery::MismatchedGeneration => {
                    agent.runtime_generation += 1;
                    let runtime_id = format!(
                        "runtime:{}:{}",
                        cutex_session_id.as_str(),
                        agent.runtime_generation
                    );
                    agent.app_server_runtime = true;
                    agent.runtime_agent_ids = vec![runtime_id.clone()];
                    agent.agent_bus_endpoint_ids = vec![runtime_id];
                    Ok(RuntimeRecoveryOutcome::RecoveredExact)
                }
                FakeRecovery::Ambiguous => Err(LifecycleFailure::outcome_unknown(
                    "runtime_recovery_ambiguous",
                    "runtime ownership cannot be proven",
                )),
            }
        }

        fn online(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state
                .log
                .push(format!("online:{}", cutex_session_id.as_str()));
            let extra_online_groups = state.extra_online_groups.clone();
            let launches = !state
                .agents
                .get(cutex_session_id)
                .unwrap()
                .app_server_runtime;
            if launches {
                state.launch_count += 1;
            }
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            if !agent.app_server_runtime {
                agent.runtime_generation += 1;
            }
            agent.active = true;
            agent.app_server_runtime = true;
            let runtime = format!(
                "runtime:{}:{}",
                cutex_session_id.as_str(),
                agent.runtime_generation
            );
            agent.runtime_agent_ids = vec![runtime.clone()];
            agent.agent_bus_endpoint_ids = vec![runtime];
            agent.groups = expected_runtime_groups(&agent.cwd, &agent.groups);
            agent.groups.extend(extra_online_groups);
            Ok(())
        }

        fn offline(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
            if let Some(log) = self.ordering_log.as_ref() {
                log.lock()
                    .unwrap()
                    .push(format!("lifecycle:offline:{}", cutex_session_id.as_str()));
            }
            let mut state = self.state.lock().unwrap();
            state
                .log
                .push(format!("offline:{}", cutex_session_id.as_str()));
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            agent.active = true;
            agent.app_server_runtime = false;
            agent.runtime_agent_ids.clear();
            agent.agent_bus_endpoint_ids.clear();
            Ok(())
        }

        fn offline_if_occurrence(
            &self,
            cutex_session_id: &CutexSessionId,
            expected: &RuntimeOccurrenceFence,
        ) -> Result<(), LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            let observed = state.agents.get(cutex_session_id).unwrap();
            if observed.runtime_generation != expected.runtime_generation
                || observed.runtime_agent_ids
                    != expected
                        .current_runtime_agent_id
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                || observed.agent_bus_endpoint_ids != expected.agent_bus_endpoint_ids
                || observed.app_server_runtime != expected.app_server_connected
            {
                state.log.push(format!(
                    "fenced-offline-rejected:{}",
                    cutex_session_id.as_str()
                ));
                return Err(LifecycleFailure::outcome_unknown(
                    "runtime_occurrence_changed",
                    "runtime occurrence changed before the fenced offline effect",
                ));
            }
            drop(state);
            self.offline(cutex_session_id)
        }

        fn restart_if_occurrence(
            &self,
            cutex_session_id: &CutexSessionId,
            expected: &RuntimeOccurrenceFence,
        ) -> Result<(AgentRuntimeObservation, AgentRuntimeObservation), LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            let advance = |state: &mut FakeState| {
                let agent = state.agents.get_mut(cutex_session_id).unwrap();
                agent.runtime_generation += 1;
                let runtime_id = format!(
                    "runtime:{}:{}",
                    cutex_session_id.as_str(),
                    agent.runtime_generation
                );
                agent.app_server_runtime = true;
                agent.runtime_agent_ids = vec![runtime_id.clone()];
                agent.agent_bus_endpoint_ids = vec![runtime_id];
            };
            if std::mem::take(&mut state.advance_before_fenced_restart_check) {
                advance(&mut state);
            }
            let matches = |observed: &AgentRuntimeObservation| {
                observed.runtime_generation == expected.runtime_generation
                    && observed.runtime_agent_ids
                        == expected
                            .current_runtime_agent_id
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    && observed.agent_bus_endpoint_ids == expected.agent_bus_endpoint_ids
                    && observed.app_server_runtime == expected.app_server_connected
            };
            if !matches(state.agents.get(cutex_session_id).unwrap()) {
                state.log.push(format!(
                    "fenced-restart-rejected:{}",
                    cutex_session_id.as_str()
                ));
                return Err(LifecycleFailure::outcome_unknown(
                    "runtime_occurrence_changed",
                    "runtime occurrence changed before the fenced restart effect",
                ));
            }
            if std::mem::take(&mut state.advance_after_fenced_restart_check) {
                advance(&mut state);
            }
            if !matches(state.agents.get(cutex_session_id).unwrap()) {
                state.log.push(format!(
                    "fenced-restart-rejected:{}",
                    cutex_session_id.as_str()
                ));
                return Err(LifecycleFailure::outcome_unknown(
                    "runtime_occurrence_changed",
                    "runtime occurrence changed before the fenced restart claim",
                ));
            }
            let before = state.agents.get(cutex_session_id).unwrap().clone();
            state
                .log
                .push(format!("online:{}", cutex_session_id.as_str()));
            state.launch_count += 1;
            let agent = state.agents.get_mut(cutex_session_id).unwrap();
            agent.runtime_generation += 1;
            agent.active = true;
            agent.app_server_runtime = true;
            let runtime = format!(
                "runtime:{}:{}",
                cutex_session_id.as_str(),
                agent.runtime_generation
            );
            agent.runtime_agent_ids = vec![runtime.clone()];
            agent.agent_bus_endpoint_ids = vec![runtime];
            Ok((before, agent.clone()))
        }

        fn retire(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state
                .log
                .push(format!("retire:{}", cutex_session_id.as_str()));
            state.agents.get_mut(cutex_session_id).unwrap().active = false;
            Ok(())
        }

        fn observe(
            &self,
            cutex_session_id: &CutexSessionId,
        ) -> Result<AgentRuntimeObservation, LifecycleFailure> {
            self.state
                .lock()
                .unwrap()
                .agents
                .get(cutex_session_id)
                .cloned()
                .ok_or_else(|| LifecycleFailure::definite("not_found", "missing fake Agent"))
        }

        fn send_message(
            &self,
            _system: &crate::agent_bus::identity::AgentManagementSystemPrincipal,
            metadata: &AgentManagementMessageMetadata,
            _to_agent: &CutexSessionId,
            exact_message: &str,
            external_message_id: &str,
        ) -> Result<String, LifecycleFailure> {
            let mut state = self.state.lock().unwrap();
            state.log.push(format!(
                "message:{external_message_id}:requested_by={}:{exact_message}",
                metadata.requested_by_director.as_str()
            ));
            let next = state.messages.len() + 1;
            Ok(state
                .messages
                .entry(external_message_id.to_string())
                .or_insert_with(|| format!("message-{next}"))
                .clone())
        }
    }

    fn root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cutex-agent-management-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn project() -> ProjectId {
        ProjectId::new("cutex-project").unwrap()
    }

    fn session(value: &str) -> CutexSessionId {
        CutexSessionId::new(value).unwrap()
    }

    fn action(value: &str) -> AgentActionId {
        AgentActionId::new(value).unwrap()
    }

    fn test_agent_cwd(name: &str) -> String {
        std::env::temp_dir()
            .join("cutex-agent-home")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    fn spec(name: &str) -> ManagedAgentSpec {
        ManagedAgentSpec {
            name: name.to_string(),
            cwd: test_agent_cwd(name),
            profile: "aemeath".to_string(),
            runtime_backend: "cute_alden".to_string(),
            model: "gpt-5.6-sol".to_string(),
            reasoning: "high".to_string(),
            permissions: "danger-full-access".to_string(),
            approval_policy: "never".to_string(),
            sandbox_mode: "danger-full-access".to_string(),
            groups: vec!["cutex".to_string(), format!("agent:{name}")],
            expose_to_im: true,
            pin: false,
        }
    }

    fn observation(
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
        active: bool,
        generation: u64,
    ) -> AgentRuntimeObservation {
        AgentRuntimeObservation {
            cutex_session_id: cutex_session_id.clone(),
            native_session_id: native_session_id.to_string(),
            active,
            cwd: spec.cwd.clone(),
            profile: spec.profile.clone(),
            runtime_backend: spec.runtime_backend.clone(),
            model: spec.model.clone(),
            reasoning: spec.reasoning.clone(),
            permissions: spec.permissions.clone(),
            approval_policy: spec.approval_policy.clone(),
            sandbox_mode: spec.sandbox_mode.clone(),
            groups: spec.groups.clone(),
            runtime_generation: generation,
            runtime_agent_ids: Vec::new(),
            app_server_runtime: false,
            agent_bus_endpoint_ids: Vec::new(),
        }
    }

    fn bind(
        provider: &AgentManagementProvider,
        action_id: &str,
        director: &str,
        expected: Option<(&str, u64)>,
    ) -> ProjectAuthorityReceipt {
        bind_project(provider, action_id, project().as_str(), director, expected)
    }

    fn bind_project(
        provider: &AgentManagementProvider,
        action_id: &str,
        project_id: &str,
        director: &str,
        expected: Option<(&str, u64)>,
    ) -> ProjectAuthorityReceipt {
        let receipt = bind_project_only(provider, action_id, project_id, director, expected);
        let fixture_digest = Sha256Digest::digest(action_id.as_bytes());
        provider
            .director_seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new(format!("test-director-seat/{fixture_digest:x}")).unwrap(),
                seat_id: crate::task_service::SeatId::new("cutex-director").unwrap(),
                occupant_cutex_session: session(director),
            })
            .unwrap();
        receipt
    }

    fn bind_project_only(
        provider: &AgentManagementProvider,
        action_id: &str,
        project_id: &str,
        director: &str,
        expected: Option<(&str, u64)>,
    ) -> ProjectAuthorityReceipt {
        provider
            .bind_project_authority(&ProjectAuthorityRequest {
                schema: AgentManagementSchema::V1,
                action_id: action(action_id),
                project_id: ProjectId::new(project_id).unwrap(),
                authorized_director_session: session(director),
                expected_authorized_director_session: expected.map(|value| session(value.0)),
                expected_authority_epoch: expected.map(|value| value.1),
            })
            .unwrap()
    }

    fn invocation(director: &str) -> AgentManagementInvocation {
        AgentManagementInvocation {
            caller_cutex_session: session(director),
            caller_runtime_agent_id: format!("runtime:{director}"),
        }
    }

    fn create_request(
        action_id: &str,
        name: &str,
        start_mode: AgentStartMode,
    ) -> AgentManagementRequest {
        AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action(action_id),
            project_id: None,
            operation: AgentOperation::Create {
                spec: spec(name),
                start_mode,
                frozen_message: (start_mode == AgentStartMode::CustomMessage)
                    .then(|| "Frozen assignment.".to_string()),
            },
        }
    }

    fn seed_legacy_no_sid_receipt(
        provider: &AgentManagementProvider,
        request: &AgentManagementRequest,
    ) -> AgentManagementResponse {
        seed_owner_action_receipt(provider, request, REAL_LEGACY_NO_SID_DETAIL)
    }

    fn seed_malformed_jsonl_receipt(
        provider: &AgentManagementProvider,
        request: &AgentManagementRequest,
    ) -> AgentManagementResponse {
        seed_owner_action_receipt(provider, request, REAL_MALFORMED_JSONL_DETAIL)
    }

    fn seed_legacy_ambiguous_sid_receipt(
        provider: &AgentManagementProvider,
        request: &AgentManagementRequest,
    ) -> AgentManagementResponse {
        let response =
            seed_owner_action_receipt(provider, request, REAL_LEGACY_AMBIGUOUS_SID_DETAIL);
        let snapshot = provider.store().snapshot().unwrap();
        let action = snapshot.actions[&request.action_id].clone();
        let event_id = format!(
            "agent-management:{}:phase:{}",
            request.action_id.as_str(),
            3
        );
        provider
            .store()
            .with_state(true, |mut state| {
                state.phase_events.insert(
                    event_id.clone(),
                    AgentManagementPhaseEvent {
                        event_id,
                        action_id: request.action_id.clone(),
                        project_id: project(),
                        operation: AgentOperationKind::Create,
                        phase: AgentActionPhase::NativeBootstrapPending,
                        phase_sequence: 3,
                        committed_at: action.created_at,
                        presentation_owner_cutex_session_id: session("cutex.director"),
                        subject_cutex_session_id: None,
                        subject_agent_name: action.reserved_agent_name.clone(),
                        predecessor_cutex_session_id: None,
                        successor_cutex_session_id: None,
                        replace_policy: None,
                        rotation_mode: None,
                        authority_epoch: None,
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        response
    }

    fn seed_owner_action_receipt(
        provider: &AgentManagementProvider,
        request: &AgentManagementRequest,
        detail: &str,
    ) -> AgentManagementResponse {
        let digest = request_sha256(request).unwrap();
        let timestamp = now();
        let failure = AgentManagementFailureEvent {
            schema: AgentManagementFailureSchema::V1,
            event_id: format!("agent-management:{}:failure", request.action_id.as_str()),
            action_id: request.action_id.clone(),
            project_id: project(),
            operation: AgentOperationKind::Create,
            code: "owner_action_required".to_string(),
            detail: detail.to_string(),
            routing_status: FailureRoutingStatus::Routable,
            route_to_director_session: Some(session("cutex.director")),
            target_cutex_session_id: None,
            created_at: timestamp.clone(),
        };
        let response = AgentManagementResponse {
            schema: AgentManagementSchema::V1,
            action_id: request.action_id.clone(),
            outcome: AgentManagementOutcome::OwnerActionRequired {
                failure: failure.clone(),
            },
        };
        let AgentOperation::Create { spec, .. } = &request.operation else {
            panic!("legacy no-SID fixture requires create")
        };
        let external_message_id = matches!(
            &request.operation,
            AgentOperation::Create {
                frozen_message: Some(_),
                ..
            }
        )
        .then(|| format!("agent-management:{}:start", request.action_id.as_str()));
        provider
            .store()
            .with_state(true, |mut state| {
                state
                    .failure_events
                    .insert(failure.event_id.clone(), failure);
                state.actions.insert(
                    request.action_id.clone(),
                    AgentActionRecord {
                        action_id: request.action_id.clone(),
                        request_sha256: digest,
                        operation: AgentOperationKind::Create,
                        project_id: project(),
                        caller_cutex_session: session("cutex.director"),
                        phase: AgentActionPhase::OwnerActionRequired,
                        phase_sequence: 4,
                        reserved_agent_name: Some(spec.name.clone()),
                        reserved_agent_cwd: Some(spec.cwd.clone()),
                        known_successor_cutex_session: None,
                        known_native_session_id: None,
                        native_bootstrap_retryable: false,
                        historical_runtime_occurrence_fence: None,
                        external_message_id,
                        response: Some(response.clone()),
                        created_at: timestamp.clone(),
                        updated_at: timestamp,
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        response
    }

    fn seed_lifecycle_revision_conflict_receipt(
        provider: &AgentManagementProvider,
        request: &AgentManagementRequest,
        target: &CutexSessionId,
    ) -> AgentManagementResponse {
        let digest = request_sha256(request).unwrap();
        let timestamp = now();
        let failure = AgentManagementFailureEvent {
            schema: AgentManagementFailureSchema::V1,
            event_id: format!("agent-management:{}:failure", request.action_id.as_str()),
            action_id: request.action_id.clone(),
            project_id: project(),
            operation: request.operation.kind(),
            code: "owner_action_required".to_string(),
            detail: REAL_OFFLINE_REVISION_CONFLICT_DETAIL.to_string(),
            routing_status: FailureRoutingStatus::Routable,
            route_to_director_session: Some(session("cutex.director")),
            target_cutex_session_id: Some(target.clone()),
            created_at: timestamp.clone(),
        };
        let response = AgentManagementResponse {
            schema: AgentManagementSchema::V1,
            action_id: request.action_id.clone(),
            outcome: AgentManagementOutcome::OwnerActionRequired {
                failure: failure.clone(),
            },
        };
        provider
            .store()
            .with_state(true, |mut state| {
                state
                    .failure_events
                    .insert(failure.event_id.clone(), failure);
                state.actions.insert(
                    request.action_id.clone(),
                    AgentActionRecord {
                        action_id: request.action_id.clone(),
                        request_sha256: digest,
                        operation: request.operation.kind(),
                        project_id: project(),
                        caller_cutex_session: session("cutex.director"),
                        phase: AgentActionPhase::OwnerActionRequired,
                        phase_sequence: 2,
                        reserved_agent_name: None,
                        reserved_agent_cwd: None,
                        known_successor_cutex_session: None,
                        known_native_session_id: None,
                        native_bootstrap_retryable: false,
                        historical_runtime_occurrence_fence: None,
                        external_message_id: None,
                        response: Some(response.clone()),
                        created_at: timestamp.clone(),
                        updated_at: timestamp,
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        response
    }

    fn legacy_import_request(
        action_id: &str,
        project_id: &str,
        director: &str,
        epoch: u64,
    ) -> LegacyDirectorOwnershipImportRequest {
        LegacyDirectorOwnershipImportRequest {
            schema: LegacyDirectorOwnershipImportSchema::V1,
            action_id: action(action_id),
            project_id: ProjectId::new(project_id).unwrap(),
            director_cutex_session_id: session(director),
            expected_authorized_director_session: session(director),
            expected_authority_epoch: epoch,
        }
    }

    fn legacy_import_evidence(
        director: &str,
        native_session_id: &str,
    ) -> LegacyDirectorOwnershipEvidence {
        LegacyDirectorOwnershipEvidence {
            director_cutex_session_id: session(director),
            native_session_id: native_session_id.to_string(),
            durable_session_revision: 7,
            runtime_generation: 3,
            spec: spec("legacy-director"),
        }
    }

    fn completed(response: AgentManagementResponse) -> AgentManagementReceipt {
        match response.outcome {
            AgentManagementOutcome::Complete { receipt } => receipt,
            other => panic!("expected completed response, got {other:?}"),
        }
    }

    fn created_agent(receipt: &AgentManagementReceipt) -> ManagedAgentRecord {
        match &receipt.result {
            AgentManagementResult::Created { agent, .. } => agent.clone(),
            other => panic!("expected created result, got {other:?}"),
        }
    }

    fn ready_observation(agent: &ManagedAgentRecord) -> AgentRuntimeObservation {
        let runtime_id = "runtime:ready:1".to_string();
        let mut observation = observation(
            &agent.cutex_session_id,
            &agent.native_session_id,
            &agent.spec,
            true,
            1,
        );
        observation.groups = expected_runtime_groups(&agent.spec.cwd, &agent.spec.groups);
        observation.runtime_agent_ids = vec![runtime_id.clone()];
        observation.agent_bus_endpoint_ids = vec![runtime_id];
        observation.app_server_runtime = true;
        observation
    }

    #[test]
    fn legacy_director_import_enables_query_and_ordinary_rotation() {
        let provider = AgentManagementProvider::open(root("legacy-import-rotate")).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let request = legacy_import_request("import", project().as_str(), "cutex.director", 1);
        let evidence = legacy_import_evidence("cutex.director", "native-director");
        let (receipt, replayed) = provider
            .import_legacy_director_ownership(&request, || Ok(evidence.clone()))
            .unwrap();
        assert!(!replayed);
        assert_eq!(receipt.authority.authority_epoch, 1);
        assert_eq!(receipt.agent.cutex_session_id, session("cutex.director"));
        assert_eq!(
            receipt.agent.created_by_director_session,
            session("cutex.director")
        );

        let lifecycle = FakeLifecycle::default();
        lifecycle.insert_agent(
            &receipt.agent.cutex_session_id,
            &receipt.agent.native_session_id,
            &receipt.agent.spec,
        );
        let query = completed(provider.execute(
            &invocation("cutex.director"),
            &AgentManagementRequest {
                schema: AgentManagementSchema::V1,
                action_id: action("query-after-import"),
                project_id: Some(project()),
                operation: AgentOperation::QueryManaged,
            },
            &lifecycle,
        ));
        let AgentManagementResult::QueryManaged { authority, agents } = query.result else {
            panic!("expected managed query result")
        };
        assert_eq!(authority.authority_epoch, 1);
        assert_eq!(agents, vec![receipt.agent.clone()]);

        let rotated = completed(provider.execute(
            &invocation("cutex.director"),
            &AgentManagementRequest {
                schema: AgentManagementSchema::V1,
                action_id: action("rotate-after-import"),
                project_id: Some(project()),
                operation: AgentOperation::DirectorRotate {
                    expected_predecessor_cutex_session: session("cutex.director"),
                    expected_authority_epoch: 1,
                    mode: DirectorRotateMode::RetainPredecessorBootstrapOnly,
                    successor: spec("successor-director"),
                    frozen_message: None,
                },
            },
            &lifecycle,
        ));
        let AgentManagementResult::DirectorRotated {
            predecessor_cutex_session_id,
            authority,
            ..
        } = rotated.result
        else {
            panic!("expected Director rotation result")
        };
        assert_eq!(predecessor_cutex_session_id, session("cutex.director"));
        assert_eq!(authority.authority_epoch, 2);
        assert_eq!(
            provider
                .store()
                .snapshot()
                .unwrap()
                .projects
                .get(&project())
                .unwrap()
                .authorized_director_session,
            authority.authorized_director_session
        );
    }

    #[test]
    fn legacy_director_import_replay_conflict_and_restart_are_durable() {
        let root = root("legacy-import-replay");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let request = legacy_import_request("import", project().as_str(), "cutex.director", 1);
        let evidence = legacy_import_evidence("cutex.director", "native-director");
        let (first, replayed) = provider
            .import_legacy_director_ownership(&request, || Ok(evidence))
            .unwrap();
        assert!(!replayed);
        drop(provider);

        let reopened = AgentManagementProvider::open(&root).unwrap();
        let (replay, replayed) = reopened
            .import_legacy_director_ownership(&request, || {
                panic!("exact replay must not reload durable session evidence")
            })
            .unwrap();
        assert!(replayed);
        assert_eq!(replay, first);
        assert_eq!(
            reopened.store().snapshot().unwrap().agents.len(),
            1,
            "restart replay must not duplicate ownership"
        );

        let mut changed = request.clone();
        changed.expected_authority_epoch = 2;
        assert_eq!(
            reopened
                .import_legacy_director_ownership(&changed, || {
                    panic!("payload conflict must precede evidence loading")
                })
                .unwrap_err(),
            AgentManagementError::Conflict("action_id_payload_conflict")
        );
    }

    #[test]
    fn legacy_director_import_fails_closed_on_authority_project_and_evidence_mismatch() {
        let provider = AgentManagementProvider::open(root("legacy-import-denials")).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let evidence = legacy_import_evidence("cutex.director", "native-director");

        let stale_epoch =
            legacy_import_request("stale-epoch", project().as_str(), "cutex.director", 2);
        assert_eq!(
            provider
                .import_legacy_director_ownership(&stale_epoch, || Ok(evidence.clone()))
                .unwrap_err(),
            AgentManagementError::Conflict("stale_project_authority")
        );
        let stale_session = legacy_import_request(
            "stale-session",
            project().as_str(),
            "cutex.other-director",
            1,
        );
        assert_eq!(
            provider
                .import_legacy_director_ownership(&stale_session, || Ok(evidence.clone()))
                .unwrap_err(),
            AgentManagementError::Conflict("stale_project_authority")
        );
        let wrong_project =
            legacy_import_request("wrong-project", "other-project", "cutex.director", 1);
        assert_eq!(
            provider
                .import_legacy_director_ownership(&wrong_project, || Ok(evidence.clone()))
                .unwrap_err(),
            AgentManagementError::Conflict("project_authority_not_initialized")
        );
        bind_project(
            &provider,
            "bind-other",
            "other-project",
            "cutex.director",
            None,
        );
        let cross_project =
            legacy_import_request("cross-project", project().as_str(), "cutex.director", 1);
        assert_eq!(
            provider
                .import_legacy_director_ownership(&cross_project, || Ok(evidence.clone()))
                .unwrap_err(),
            AgentManagementError::Conflict("director_authorized_for_multiple_projects")
        );

        let separate = AgentManagementProvider::open(root("legacy-import-evidence")).unwrap();
        bind(&separate, "bind", "cutex.director", None);
        let request = legacy_import_request("mismatch", project().as_str(), "cutex.director", 1);
        let mut mismatched = evidence;
        mismatched.director_cutex_session_id = session("cutex.other-director");
        assert_eq!(
            separate
                .import_legacy_director_ownership(&request, || Ok(mismatched))
                .unwrap_err(),
            AgentManagementError::Conflict("durable_session_identity_mismatch")
        );
        assert_eq!(
            separate
                .import_legacy_director_ownership(&request, || {
                    Err(AgentManagementError::NotFound(
                        "durable_director_session_not_found",
                    ))
                })
                .unwrap_err(),
            AgentManagementError::NotFound("durable_director_session_not_found")
        );
        assert!(separate.store().snapshot().unwrap().agents.is_empty());
    }

    #[test]
    fn legacy_director_import_rejects_existing_and_retired_ownership_records() {
        for (label, owned_project, retired, expected) in [
            (
                "existing",
                "cutex-project",
                false,
                AgentManagementError::Conflict("director_ownership_record_exists"),
            ),
            (
                "retired",
                "cutex-project",
                true,
                AgentManagementError::Conflict("director_ownership_record_retired"),
            ),
            (
                "cross-project-record",
                "other-project",
                false,
                AgentManagementError::Conflict("director_owned_by_another_project"),
            ),
        ] {
            let provider = AgentManagementProvider::open(root(label)).unwrap();
            bind(&provider, "bind", "cutex.director", None);
            let evidence = legacy_import_evidence("cutex.director", "native-director");
            provider
                .store()
                .with_state(true, |mut state| {
                    state.agents.insert(
                        session("cutex.director"),
                        ManagedAgentRecord {
                            project_id: ProjectId::new(owned_project).unwrap(),
                            created_by_director_session: session("cutex.director"),
                            cutex_session_id: session("cutex.director"),
                            native_session_id: evidence.native_session_id.clone(),
                            spec: evidence.spec.clone(),
                            created_at: now(),
                            retired_at: retired.then(now),
                        },
                    );
                    Ok((state, (), true))
                })
                .unwrap();
            let request = legacy_import_request("import", project().as_str(), "cutex.director", 1);
            assert_eq!(
                provider
                    .import_legacy_director_ownership(&request, || Ok(evidence))
                    .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn readiness_accepts_only_the_cwd_derived_project_group_delta() {
        let agent = ManagedAgentRecord {
            project_id: project(),
            created_by_director_session: session("cutex.director"),
            cutex_session_id: session("cutex.worker"),
            native_session_id: "native-worker".to_string(),
            spec: spec("worker"),
            created_at: now(),
            retired_at: None,
        };
        let ready = ready_observation(&agent);
        validate_ready(&agent, &ready).expect("cwd-derived project group is system-owned");

        let mut extra = ready.clone();
        extra.groups.push("unexpected-extra".to_string());
        assert!(validate_ready(&agent, &extra).is_err());

        let mut missing = ready;
        missing.groups.retain(|group| group != "cutex");
        assert!(validate_ready(&agent, &missing).is_err());
    }

    #[test]
    fn readiness_keeps_identity_defaults_and_sole_endpoint_strict() {
        let agent = ManagedAgentRecord {
            project_id: project(),
            created_by_director_session: session("cutex.director"),
            cutex_session_id: session("cutex.worker"),
            native_session_id: "native-worker".to_string(),
            spec: spec("worker"),
            created_at: now(),
            retired_at: None,
        };
        let ready = ready_observation(&agent);

        macro_rules! reject_string_change {
            ($field:ident, $value:expr) => {{
                let mut changed = ready.clone();
                changed.$field = $value.to_string();
                assert!(
                    validate_ready(&agent, &changed).is_err(),
                    stringify!($field)
                );
            }};
        }

        reject_string_change!(native_session_id, "other-native");
        reject_string_change!(cwd, test_agent_cwd("other-cwd"));
        reject_string_change!(profile, "other-profile");
        reject_string_change!(runtime_backend, "host_foreground");
        reject_string_change!(model, "other-model");
        reject_string_change!(reasoning, "other-reasoning");
        reject_string_change!(permissions, "read-only");
        reject_string_change!(approval_policy, "on-request");
        reject_string_change!(sandbox_mode, "workspace-write");

        let mut other_session = ready.clone();
        other_session.cutex_session_id = session("cutex.other-worker");
        assert!(validate_ready(&agent, &other_session).is_err());

        let mut extra_endpoint = ready;
        extra_endpoint
            .agent_bus_endpoint_ids
            .push("runtime:unexpected:2".to_string());
        assert!(validate_ready(&agent, &extra_endpoint).is_err());
    }

    #[test]
    fn readiness_owner_action_replay_queues_custom_message_once_after_group_repair() {
        let root = root("readiness-group-replay");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::with_extra_online_group("unexpected-extra");
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            first.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        let snapshot = provider.store().snapshot().unwrap();
        let action = &snapshot.actions[&request.action_id];
        assert_eq!(action.phase, AgentActionPhase::Online);
        assert_eq!(lifecycle.bootstrap_count(), 1);
        assert_eq!(lifecycle.message_count(), 0);
        let cutex_session_id = action.known_successor_cutex_session.clone().unwrap();
        let agent = snapshot.agents[&cutex_session_id].clone();
        lifecycle.disconnect_with_claim(&cutex_session_id, FakeRecovery::ExactLive);
        lifecycle.set_groups(
            &cutex_session_id,
            expected_runtime_groups(&agent.spec.cwd, &agent.spec.groups),
        );

        let completed_response =
            provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(completed_response, replay);
        assert!(matches!(
            completed_response.outcome,
            AgentManagementOutcome::Complete { .. }
        ));
        assert_eq!(lifecycle.bootstrap_count(), 1);
        assert_eq!(lifecycle.message_count(), 1);
        assert_eq!(lifecycle.launch_count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn create_follows_normal_order_supports_modes_and_exact_replay() {
        let root = root("create-order");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();

        let bootstrap = create_request(
            "create-bootstrap",
            "worker-a",
            AgentStartMode::BootstrapOnly,
        );
        let bootstrap_receipt =
            completed(provider.execute(&invocation("cutex.director"), &bootstrap, &lifecycle));
        assert!(matches!(
            bootstrap_receipt.result,
            AgentManagementResult::Created {
                message_id: None,
                ..
            }
        ));
        assert_eq!(
            lifecycle.log()[..6],
            [
                "prepare:worker-a",
                "bootstrap:Hi.",
                "adopt:native-1",
                "configure:cutex.agent-1",
                "recover:cutex.agent-1",
                "online:cutex.agent-1"
            ]
        );
        assert_eq!(
            lifecycle.message_count(),
            0,
            "bootstrap-only creation must not construct a custom message"
        );

        let custom = create_request("create-custom", "worker-b", AgentStartMode::CustomMessage);
        let first = provider.execute(&invocation("cutex.director"), &custom, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &custom, &lifecycle);
        assert_eq!(first, replay);
        assert_eq!(lifecycle.bootstrap_count(), 2);
        assert_eq!(lifecycle.message_count(), 1);
        let receipt = completed(first);
        match receipt.result {
            AgentManagementResult::Created {
                message_id: Some(message_id),
                ..
            } => assert_eq!(message_id, "message-1"),
            other => panic!("unexpected result: {other:?}"),
        }
        assert!(lifecycle.log().iter().any(
            |entry| entry
                == "message:agent-management:create-custom:start:requested_by=cutex.director:Frozen assignment."
        ));

        let mut changed = custom.clone();
        if let AgentOperation::Create { spec, .. } = &mut changed.operation {
            spec.model = "gpt-5.6-terra".to_string();
        }
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &changed, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unique_project_is_selected_implicitly_and_exact_request_replays() {
        let root = root("implicit-project");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind_project(&provider, "bind-alpha", "alpha", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("implicit-query"),
            project_id: None,
            operation: AgentOperation::QueryManaged,
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        bind_project(&provider, "bind-beta", "beta", "cutex.director", None);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        let receipt = completed(first);
        assert_eq!(receipt.project_id, ProjectId::new("alpha").unwrap());
        assert_eq!(receipt.request_sha256, request_sha256(&request).unwrap());
        assert_eq!(provider.store().snapshot().unwrap().actions.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_project_selection_and_denials_are_typed_and_no_write() {
        let root = root("multi-project-selection");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind_project(&provider, "bind-alpha", "alpha", "cutex.director", None);
        bind_project(&provider, "bind-beta", "beta", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let mut request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("query"),
            project_id: None,
            operation: AgentOperation::QueryManaged,
        };

        let ambiguous = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            ambiguous.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. }
                if code == "project_selection_required"
        ));
        assert!(provider.store().snapshot().unwrap().actions.is_empty());

        request.project_id = Some(ProjectId::new("forged").unwrap());
        let forged = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            forged.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. }
                if code == "project_not_authorized"
        ));
        assert!(provider.store().snapshot().unwrap().actions.is_empty());

        request.project_id = Some(ProjectId::new("alpha").unwrap());
        let worker = provider.execute(&invocation("cutex.worker"), &request, &lifecycle);
        assert!(matches!(
            worker.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. }
                if code == "not_authorized_director"
        ));
        assert!(provider.store().snapshot().unwrap().actions.is_empty());

        let selected =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));
        assert_eq!(selected.project_id, ProjectId::new("alpha").unwrap());
        assert_eq!(selected.request_sha256, request_sha256(&request).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_and_automatic_project_chat_groups_do_not_select_authority() {
        let root = root("chat-groups-no-authority");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind_project(
            &provider,
            "bind-authoritative",
            "authoritative",
            "cutex.director",
            None,
        );
        let lifecycle = FakeLifecycle::default();
        let mut request = create_request(
            "create-grouped-worker",
            "grouped-worker",
            AgentStartMode::BootstrapOnly,
        );
        let AgentOperation::Create { spec, .. } = &mut request.operation else {
            unreachable!()
        };
        spec.groups = crate::agent_bus::identity::normalize_agent_groups(vec![
            "cutex".to_string(),
            "agent:grouped-worker".to_string(),
            "project:legacy-chat-label".to_string(),
            "project:automatic-routing-label".to_string(),
            "project:sc-polya-v2".to_string(),
            "project:umi-tools-scpolya-lab".to_string(),
        ]);

        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));
        assert_eq!(receipt.project_id, ProjectId::new("authoritative").unwrap());
        let AgentManagementResult::Created { agent, .. } = receipt.result else {
            panic!("expected created Agent")
        };
        assert!(agent
            .spec
            .groups
            .iter()
            .any(|group| group == "project:legacy-chat-label"));
        assert!(agent
            .spec
            .groups
            .iter()
            .any(|group| group == "project:automatic-routing-label"));
        let snapshot = provider.store().snapshot().unwrap();
        assert!(snapshot
            .projects
            .contains_key(&ProjectId::new("authoritative").unwrap()));
        assert!(!snapshot
            .projects
            .contains_key(&ProjectId::new("sc-polya-v2").unwrap()));
        assert!(!snapshot
            .projects
            .contains_key(&ProjectId::new("umi-tools-scpolya-lab").unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authority_and_query_are_project_scoped_and_reject_old_director() {
        let root = root("authority-query");
        let provider = AgentManagementProvider::open(&root).unwrap();
        let first = bind(&provider, "bind-1", "cutex.director-old", None);
        assert_eq!(first.authority.authority_epoch, 1);
        let lifecycle = FakeLifecycle::default();
        let create = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        provider.execute(&invocation("cutex.director-old"), &create, &lifecycle);
        let rebound = bind(
            &provider,
            "bind-2",
            "cutex.director-new",
            Some(("cutex.director-old", 1)),
        );
        assert_eq!(rebound.authority.authority_epoch, 2);
        let query = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("query"),
            project_id: Some(project()),
            operation: AgentOperation::QueryManaged,
        };
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director-old"), &query, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "not_authorized_director"
        ));
        let unauthorized_custom = create_request(
            "unauthorized-custom",
            "worker-unauthorized",
            AgentStartMode::CustomMessage,
        );
        assert!(matches!(
            provider
                .execute(
                    &invocation("cutex.director-old"),
                    &unauthorized_custom,
                    &lifecycle
                )
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "not_authorized_director"
        ));
        assert_eq!(
            lifecycle.message_count(),
            0,
            "a caller that is no longer the project Director cannot reach the system projection"
        );
        let receipt =
            completed(provider.execute(&invocation("cutex.director-new"), &query, &lifecycle));
        match receipt.result {
            AgentManagementResult::QueryManaged { authority, agents } => {
                assert_eq!(authority.authority_epoch, 2);
                assert_eq!(agents.len(), 1);
                assert_eq!(agents[0].project_id, project());
            }
            other => panic!("unexpected query: {other:?}"),
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lifecycle_preserves_durable_and_native_identity_and_close_retires() {
        let root = root("lifecycle");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let restart = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        let restarted =
            completed(provider.execute(&invocation("cutex.director"), &restart, &lifecycle));
        match restarted.result {
            AgentManagementResult::Lifecycle { agent, observation } => {
                assert_eq!(agent.cutex_session_id, created.cutex_session_id);
                assert_eq!(observation.native_session_id, created.native_session_id);
                assert_eq!(observation.runtime_generation, 2);
            }
            other => panic!("unexpected restart: {other:?}"),
        }
        let close = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("close"),
            project_id: Some(project()),
            operation: AgentOperation::Close {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        let closed = completed(provider.execute(&invocation("cutex.director"), &close, &lifecycle));
        match closed.result {
            AgentManagementResult::Lifecycle { agent, observation } => {
                assert!(agent.retired_at.is_some());
                assert!(!observation.active);
                assert!(observation.agent_bus_endpoint_ids.is_empty());
            }
            other => panic!("unexpected close: {other:?}"),
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_restart_replay_reconciles_historical_offline_revision_conflict_once() {
        let root = root("restart-revision-conflict-replay");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = Arc::new(FakeLifecycle::default());
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            lifecycle.as_ref(),
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        lifecycle.offline(&created.cutex_session_id).unwrap();
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        let launches = lifecycle.launch_count();
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        drop(provider);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let request = request.clone();
                let lifecycle = Arc::clone(&lifecycle);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let provider = AgentManagementProvider::open(root).unwrap();
                    barrier.wait();
                    provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
                })
            })
            .collect::<Vec<_>>();
        let responses = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses[0], responses[1]);
        let receipt = completed(responses[0].clone());
        let AgentManagementResult::Lifecycle { observation, .. } = receipt.result else {
            panic!("expected lifecycle receipt")
        };
        assert_eq!(observation.runtime_generation, 2);
        assert_eq!(lifecycle.launch_count(), launches + 1);
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects,
            "a proven-absent historical occurrence does not need an offline effect"
        );
        let effects = lifecycle.log();
        assert_eq!(
            effects
                .iter()
                .filter(|entry| entry.starts_with("online:"))
                .count(),
            2,
            "create plus one recovered restart launch"
        );

        let reopened = AgentManagementProvider::open(&root).unwrap();
        let replay = reopened.execute(&invocation("cutex.director"), &request, lifecycle.as_ref());
        assert!(matches!(
            replay.outcome,
            AgentManagementOutcome::Complete { .. }
        ));
        assert_eq!(lifecycle.launch_count(), launches + 1);
        let action = reopened
            .store()
            .snapshot()
            .unwrap()
            .actions
            .get(&request.action_id)
            .cloned()
            .unwrap();
        assert!(
            action
                .historical_runtime_occurrence_fence
                .as_ref()
                .is_some_and(RuntimeOccurrenceFence::is_proven_absent),
            "the exact absence occurrence is durable with the reopened action"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_restart_fences_newer_same_identity_occurrence_before_lifecycle_effect() {
        let root = root("restart-revision-conflict-newer-occurrence");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        lifecycle.advance_runtime_occurrence(&created.cutex_session_id);
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        let launches = lifecycle.launch_count();

        let fenced = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let AgentManagementOutcome::OwnerActionRequired { failure } = fenced.outcome else {
            panic!("newer occurrence must remain fenced")
        };
        assert_eq!(failure.code, "runtime_occurrence_reconciliation_present");
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects,
            "a newer occurrence must not receive an offline call"
        );
        assert_eq!(lifecycle.launch_count(), launches);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_restart_rechecks_committed_fence_at_atomic_restart_entry() {
        let root = root("restart-revision-conflict-occurrence-race");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        lifecycle.offline(&created.cutex_session_id).unwrap();
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        lifecycle.advance_before_fenced_restart_check();

        let fenced = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            fenced.outcome,
            AgentManagementOutcome::OwnerActionRequired { ref failure }
                if failure.detail.contains("runtime occurrence changed before the fenced restart effect")
        ));
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects,
            "a generation change after fence commit must still prevent the offline call"
        );
        assert_eq!(
            lifecycle
                .log()
                .iter()
                .filter(|entry| entry.starts_with("fenced-restart-rejected:"))
                .count(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_restart_rechecks_fence_at_atomic_restart_claim_boundary() {
        let root = root("restart-revision-conflict-claim-race");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        lifecycle.offline(&created.cutex_session_id).unwrap();
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        let recoveries = lifecycle.recovery_count();
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        let online_effects = lifecycle.online_count(&created.cutex_session_id);
        let launches = lifecycle.launch_count();
        lifecycle.advance_after_fenced_restart_check();

        let fenced = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            fenced.outcome,
            AgentManagementOutcome::OwnerActionRequired { ref failure }
                if failure.detail.contains("runtime occurrence changed before the fenced restart claim")
        ));
        assert_eq!(lifecycle.recovery_count(), recoveries);
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects
        );
        assert_eq!(
            lifecycle.online_count(&created.cutex_session_id),
            online_effects
        );
        assert_eq!(lifecycle.launch_count(), launches);
        let newer = lifecycle.observe(&created.cutex_session_id).unwrap();
        assert!(newer.app_server_runtime);
        assert_eq!(newer.runtime_generation, 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_restart_rechecks_fence_before_any_runtime_recovery_effect() {
        let root = root("restart-revision-conflict-pre-recovery-race");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = Arc::new(FakeLifecycle::default());
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            lifecycle.as_ref(),
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        lifecycle.offline(&created.cutex_session_id).unwrap();
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        assert!(provider
            .store()
            .snapshot()
            .unwrap()
            .actions
            .get(&request.action_id)
            .unwrap()
            .historical_runtime_occurrence_fence
            .is_none());
        drop(provider);

        let target = created.cutex_session_id.clone();
        let barrier_action = request.action_id.clone();
        let barrier_lifecycle = Arc::clone(&lifecycle);
        let installed_occurrence = Arc::new(Mutex::new(None));
        let barrier_observation = Arc::clone(&installed_occurrence);
        let provider = AgentManagementProvider::open(&root)
            .unwrap()
            .with_phase_observer(Arc::new(move |event: &AgentManagementPhaseEvent| {
                if event.action_id == barrier_action && event.phase == AgentActionPhase::Prepared {
                    barrier_lifecycle.advance_runtime_occurrence(&target);
                    *barrier_observation.lock().unwrap() =
                        Some(barrier_lifecycle.observe(&target).unwrap());
                }
            }));
        let recoveries = lifecycle.recovery_count();
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        let online_effects = lifecycle.online_count(&created.cutex_session_id);
        let launches = lifecycle.launch_count();

        let fenced = provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref());
        assert!(matches!(
            fenced.outcome,
            AgentManagementOutcome::OwnerActionRequired { ref failure }
                if failure.detail.contains("runtime occurrence changed before the fenced restart effect")
        ));
        assert_eq!(
            lifecycle.recovery_count(),
            recoveries,
            "recovery must not run"
        );
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects,
            "the newer occurrence must not receive an offline call"
        );
        assert_eq!(
            lifecycle.online_count(&created.cutex_session_id),
            online_effects,
            "the newer occurrence must not receive an online call"
        );
        assert_eq!(lifecycle.launch_count(), launches, "no child may launch");
        assert_eq!(
            lifecycle.observe(&created.cutex_session_id).unwrap(),
            installed_occurrence.lock().unwrap().clone().unwrap(),
            "the newer occurrence and its endpoint/claim evidence must remain unchanged"
        );

        drop(provider);
        let reopened = AgentManagementProvider::open(&root).unwrap();
        let replay = reopened.execute(&invocation("cutex.director"), &request, lifecycle.as_ref());
        assert_eq!(
            replay, fenced,
            "the owner-action receipt must replay exactly"
        );
        assert_eq!(lifecycle.recovery_count(), recoveries);
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects
        );
        assert_eq!(
            lifecycle.online_count(&created.cutex_session_id),
            online_effects
        );
        assert_eq!(lifecycle.launch_count(), launches);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_restart_fences_ambiguous_and_unavailable_occurrence_before_effect() {
        let root = root("restart-revision-conflict-ambiguous-occurrence");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        seed_lifecycle_revision_conflict_receipt(&provider, &request, &created.cutex_session_id);
        let offline_effects = lifecycle.offline_count(&created.cutex_session_id);
        lifecycle.set_occurrence_reconciliation(
            HistoricalRuntimeOccurrenceReconciliation::Ambiguous {
                reason: "fake occurrence evidence conflicts".to_string(),
            },
        );
        let ambiguous = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            ambiguous.outcome,
            AgentManagementOutcome::OwnerActionRequired { ref failure }
                if failure.code == "runtime_occurrence_reconciliation_ambiguous"
        ));
        lifecycle.set_occurrence_reconciliation(
            HistoricalRuntimeOccurrenceReconciliation::Unavailable {
                reason: "fake process registry unavailable".to_string(),
            },
        );
        let unavailable = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            unavailable.outcome,
            AgentManagementOutcome::OwnerActionRequired { ref failure }
                if failure.code == "runtime_occurrence_reconciliation_unavailable"
        ));
        assert_eq!(
            lifecycle.offline_count(&created.cutex_session_id),
            offline_effects
        );
        assert_eq!(lifecycle.launch_count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_revision_conflict_replay_fences_authority_and_identity_drift() {
        let root = root("restart-revision-conflict-fences");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-revision-conflict"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };
        let original = seed_lifecycle_revision_conflict_receipt(
            &provider,
            &request,
            &created.cutex_session_id,
        );
        lifecycle.corrupt_native_session_id(&created.cutex_session_id);
        let fenced = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            fenced.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        assert_eq!(lifecycle.launch_count(), 1);

        bind(
            &provider,
            "rebind",
            "cutex.director-new",
            Some(("cutex.director", 1)),
        );
        let unauthorized = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            unauthorized.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. }
                if code == "not_authorized_director"
        ));
        assert_ne!(
            fenced, original,
            "typed reconciliation reason is observable"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn online_recovers_exact_live_claim_without_launching_a_child() {
        let root = root("online-recover-live");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let generation = lifecycle
            .observe(&created.cutex_session_id)
            .unwrap()
            .runtime_generation;
        let launches = lifecycle.launch_count();
        let recoveries = lifecycle.recovery_count();
        let authority = provider.store().snapshot().unwrap().projects;
        lifecycle.disconnect_with_claim(&created.cutex_session_id, FakeRecovery::ExactLive);
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("online-recover-live"),
            project_id: Some(project()),
            operation: AgentOperation::Online {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        let receipt = completed(first);
        let AgentManagementResult::Lifecycle { observation, .. } = receipt.result else {
            panic!("expected lifecycle result")
        };
        assert_eq!(observation.runtime_generation, generation);
        assert_eq!(lifecycle.launch_count(), launches);
        assert_eq!(lifecycle.recovery_count(), recoveries + 1);
        assert_eq!(lifecycle.message_count(), 0);
        assert_eq!(provider.store().snapshot().unwrap().projects, authority);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn online_fences_dead_claim_before_one_replacement_launch() {
        let root = root("online-recover-dead");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let generation = lifecycle
            .observe(&created.cutex_session_id)
            .unwrap()
            .runtime_generation;
        let launches = lifecycle.launch_count();
        let recoveries = lifecycle.recovery_count();
        lifecycle.disconnect_with_claim(&created.cutex_session_id, FakeRecovery::DeadClaim);
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("online-recover-dead"),
            project_id: Some(project()),
            operation: AgentOperation::Online {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        let receipt = completed(first);
        let AgentManagementResult::Lifecycle { observation, .. } = receipt.result else {
            panic!("expected lifecycle result")
        };
        assert_eq!(observation.runtime_generation, generation + 1);
        assert_eq!(lifecycle.launch_count(), launches + 1);
        assert_eq!(lifecycle.recovery_count(), recoveries + 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ambiguous_recovery_fails_closed_and_exact_replay_is_immutable() {
        let root = root("online-recover-ambiguous");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let launches = lifecycle.launch_count();
        let recoveries = lifecycle.recovery_count();
        lifecycle.disconnect_with_claim(&created.cutex_session_id, FakeRecovery::Ambiguous);
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("online-recover-ambiguous"),
            project_id: Some(project()),
            operation: AgentOperation::Online {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        assert!(matches!(
            first.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        assert_eq!(lifecycle.recovery_count(), recoveries + 1);
        assert_eq!(lifecycle.launch_count(), launches);
        assert_eq!(lifecycle.message_count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovered_runtime_with_a_different_generation_fails_closed() {
        let root = root("online-recover-generation-mismatch");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let launches = lifecycle.launch_count();
        let recoveries = lifecycle.recovery_count();
        lifecycle.disconnect_with_claim(
            &created.cutex_session_id,
            FakeRecovery::MismatchedGeneration,
        );
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("online-recover-generation-mismatch"),
            project_id: Some(project()),
            operation: AgentOperation::Online {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        assert!(matches!(
            first.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        assert_eq!(lifecycle.recovery_count(), recoveries + 1);
        assert_eq!(lifecycle.launch_count(), launches);
        assert_eq!(lifecycle.message_count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_adopts_exact_claim_before_launching_one_new_generation() {
        let root = root("restart-recover-live");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let created = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let generation = lifecycle
            .observe(&created.cutex_session_id)
            .unwrap()
            .runtime_generation;
        let launches = lifecycle.launch_count();
        let recoveries = lifecycle.recovery_count();
        lifecycle.disconnect_with_claim(&created.cutex_session_id, FakeRecovery::ExactLive);
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("restart-recover-live"),
            project_id: Some(project()),
            operation: AgentOperation::Restart {
                cutex_session_id: created.cutex_session_id.clone(),
            },
        };

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert_eq!(first, replay);
        let receipt = completed(first);
        let AgentManagementResult::Lifecycle { observation, .. } = receipt.result else {
            panic!("expected lifecycle result")
        };
        assert_eq!(observation.runtime_generation, generation + 1);
        assert_eq!(lifecycle.launch_count(), launches + 1);
        assert_eq!(lifecycle.recovery_count(), recoveries + 1);
        assert_eq!(lifecycle.message_count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replace_is_policy_ordered_composition_without_seat_state() {
        let root = root("replace");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let predecessor = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request("create", "worker-old", AgentStartMode::BootstrapOnly),
            &lifecycle,
        )));
        let before = lifecycle.log().len();
        let replace = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("replace"),
            project_id: Some(project()),
            operation: AgentOperation::Replace {
                predecessor_cutex_session_id: predecessor.cutex_session_id.clone(),
                policy: AgentReplacePolicy::CloseBeforeCreate,
                successor: spec("worker-new"),
                start_mode: AgentStartMode::BootstrapOnly,
                frozen_message: None,
            },
        };
        completed(provider.execute(&invocation("cutex.director"), &replace, &lifecycle));
        let log = lifecycle.log();
        let steps = &log[before..];
        assert!(steps[0].starts_with("offline:"));
        assert!(steps[1].starts_with("retire:"));
        assert_eq!(steps[2], "prepare:worker-new");
        let snapshot = provider.store().snapshot().unwrap();
        assert_eq!(
            snapshot.projects.get(&project()).unwrap().authority_epoch,
            1
        );
        let replace_phases = snapshot
            .phase_events
            .values()
            .filter(|event| event.action_id == replace.action_id)
            .collect::<Vec<_>>();
        assert!(replace_phases
            .iter()
            .any(|event| event.phase == AgentActionPhase::PredecessorClosed));
        assert!(replace_phases.iter().all(|event| {
            event.predecessor_cutex_session_id.as_ref() == Some(&predecessor.cutex_session_id)
                && event.replace_policy == Some(AgentReplacePolicy::CloseBeforeCreate)
                && event.rotation_mode.is_none()
        }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replace_close_policies_recover_after_process_loss_without_duplicate_successors_or_messages()
    {
        for (label, policy) in [
            (
                "replace-close-before-crash",
                AgentReplacePolicy::CloseBeforeCreate,
            ),
            (
                "replace-close-after-crash",
                AgentReplacePolicy::CloseAfterReady,
            ),
        ] {
            let root = root(label);
            let provider = AgentManagementProvider::open(&root)
                .unwrap()
                .with_fail_after_predecessor_close_once();
            bind(&provider, "bind", "cutex.director", None);
            let lifecycle = Arc::new(FakeLifecycle::default());
            let predecessor = created_agent(&completed(provider.execute(
                &invocation("cutex.director"),
                &create_request("create-old", "worker-old", AgentStartMode::BootstrapOnly),
                lifecycle.as_ref(),
            )));
            let request = AgentManagementRequest {
                schema: AgentManagementSchema::V1,
                action_id: action("replace-crash"),
                project_id: Some(project()),
                operation: AgentOperation::Replace {
                    predecessor_cutex_session_id: predecessor.cutex_session_id.clone(),
                    policy,
                    successor: spec("worker-new"),
                    start_mode: AgentStartMode::CustomMessage,
                    frozen_message: Some("Exact replacement handoff.".to_string()),
                },
            };

            let interrupted =
                provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref());
            assert!(matches!(
                interrupted.outcome,
                AgentManagementOutcome::NoWrite { ref code, .. }
                    if code == "injected_process_loss"
            ));
            let interrupted_state = provider.store().snapshot().unwrap();
            assert_eq!(
                interrupted_state.actions[&request.action_id].phase,
                AgentActionPhase::PredecessorClosing
            );
            assert!(interrupted_state.agents[&predecessor.cutex_session_id]
                .retired_at
                .is_none());
            let externally_retired = lifecycle.observe(&predecessor.cutex_session_id).unwrap();
            assert!(!externally_retired.active);
            assert!(externally_retired.agent_bus_endpoint_ids.is_empty());
            assert_eq!(lifecycle.retire_count(&predecessor.cutex_session_id), 1);

            let bypass =
                create_request("fresh-bypass", "worker-new", AgentStartMode::BootstrapOnly);
            assert!(matches!(
                provider
                    .execute(&invocation("cutex.director"), &bypass, lifecycle.as_ref())
                    .outcome,
                AgentManagementOutcome::NoWrite { ref code, ref detail }
                    if code == "conflict" && detail.contains("unresolved_agent_reservation")
            ));

            drop(provider);
            let reopened = Arc::new(AgentManagementProvider::open(&root).unwrap());
            let mut threads = Vec::new();
            for _ in 0..2 {
                let provider = Arc::clone(&reopened);
                let lifecycle = Arc::clone(&lifecycle);
                let request = request.clone();
                threads.push(std::thread::spawn(move || {
                    provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
                }));
            }
            let first = threads.remove(0).join().unwrap();
            let second = threads.remove(0).join().unwrap();
            assert_eq!(first, second);
            let receipt = completed(first);
            let AgentManagementResult::Replaced { successor, .. } = receipt.result else {
                panic!("expected replacement receipt")
            };
            assert_eq!(successor.spec, spec("worker-new"));
            assert_eq!(lifecycle.bootstrap_count(), 2);
            assert_eq!(lifecycle.message_count(), 1);
            assert_eq!(lifecycle.retire_count(&predecessor.cutex_session_id), 1);
            let final_state = reopened.store().snapshot().unwrap();
            assert_eq!(
                final_state.actions[&request.action_id].phase,
                AgentActionPhase::Complete
            );
            assert!(final_state.agents[&predecessor.cutex_session_id]
                .retired_at
                .is_some());
            assert_eq!(
                final_state
                    .agents
                    .values()
                    .filter(|agent| agent.retired_at.is_none())
                    .count(),
                1
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn close_predecessor_rotation_recovers_once_and_transfers_authority_once() {
        let root = root("rotate-close-crash");
        let provider = AgentManagementProvider::open(&root)
            .unwrap()
            .with_fail_after_predecessor_close_once();
        bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
        let lifecycle = Arc::new(FakeLifecycle::default());
        let predecessor = created_agent(&completed(provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            lifecycle.as_ref(),
        )));
        bind(
            &provider,
            "bind-director",
            predecessor.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );
        let request = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("rotate-crash"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: predecessor.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::ClosePredecessorThenCreateWithMessage,
                successor: spec("director-r2"),
                frozen_message: Some("Exact Director handoff.".to_string()),
            },
        };
        let interrupted = provider.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &request,
            lifecycle.as_ref(),
        );
        assert!(matches!(
            interrupted.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "injected_process_loss"
        ));
        let interrupted_state = provider.store().snapshot().unwrap();
        assert_eq!(
            interrupted_state.actions[&request.action_id].phase,
            AgentActionPhase::PredecessorClosing
        );
        assert_eq!(
            interrupted_state.projects[&project()].authorized_director_session,
            predecessor.cutex_session_id
        );
        assert!(interrupted_state.agents[&predecessor.cutex_session_id]
            .retired_at
            .is_none());
        assert!(
            !lifecycle
                .observe(&predecessor.cutex_session_id)
                .unwrap()
                .active
        );
        assert_eq!(lifecycle.bootstrap_count(), 1);

        drop(provider);
        let reopened = Arc::new(AgentManagementProvider::open(&root).unwrap());
        let mut threads = Vec::new();
        for _ in 0..2 {
            let provider = Arc::clone(&reopened);
            let lifecycle = Arc::clone(&lifecycle);
            let request = request.clone();
            let predecessor = predecessor.cutex_session_id.clone();
            threads.push(std::thread::spawn(move || {
                provider.execute(
                    &invocation(predecessor.as_str()),
                    &request,
                    lifecycle.as_ref(),
                )
            }));
        }
        let first = threads.remove(0).join().unwrap();
        let second = threads.remove(0).join().unwrap();
        assert_eq!(first, second);
        let receipt = completed(first);
        let AgentManagementResult::DirectorRotated {
            successor,
            authority,
            ..
        } = receipt.result
        else {
            panic!("expected Director rotation receipt")
        };
        assert_eq!(authority.authority_epoch, 3);
        assert_eq!(
            authority.authorized_director_session,
            successor.cutex_session_id
        );
        assert_eq!(lifecycle.bootstrap_count(), 2);
        assert_eq!(lifecycle.message_count(), 1);
        assert_eq!(lifecycle.retire_count(&predecessor.cutex_session_id), 1);
        assert_eq!(
            reopened.store().snapshot().unwrap().projects[&project()],
            authority
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn predecessor_close_reconciliation_fences_identity_and_authority_changes() {
        let mismatch_root = root("replace-close-mismatch");
        let mismatch_provider = AgentManagementProvider::open(&mismatch_root)
            .unwrap()
            .with_fail_after_predecessor_close_once();
        bind(&mismatch_provider, "bind", "cutex.director", None);
        let mismatch_lifecycle = FakeLifecycle::default();
        let predecessor = created_agent(&completed(mismatch_provider.execute(
            &invocation("cutex.director"),
            &create_request("create-old", "worker-old", AgentStartMode::BootstrapOnly),
            &mismatch_lifecycle,
        )));
        let replace = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("replace-mismatch"),
            project_id: Some(project()),
            operation: AgentOperation::Replace {
                predecessor_cutex_session_id: predecessor.cutex_session_id.clone(),
                policy: AgentReplacePolicy::CloseBeforeCreate,
                successor: spec("worker-new"),
                start_mode: AgentStartMode::BootstrapOnly,
                frozen_message: None,
            },
        };
        mismatch_provider.execute(&invocation("cutex.director"), &replace, &mismatch_lifecycle);
        mismatch_lifecycle.corrupt_native_session_id(&predecessor.cutex_session_id);
        assert!(matches!(
            AgentManagementProvider::open(&mismatch_root)
                .unwrap()
                .execute(&invocation("cutex.director"), &replace, &mismatch_lifecycle)
                .outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        assert_eq!(mismatch_lifecycle.bootstrap_count(), 1);
        std::fs::remove_dir_all(mismatch_root).unwrap();

        let authority_root = root("rotate-close-authority-change");
        let authority_provider = AgentManagementProvider::open(&authority_root)
            .unwrap()
            .with_fail_after_predecessor_close_once();
        bind(
            &authority_provider,
            "bind-bootstrap",
            "cutex.bootstrap",
            None,
        );
        let authority_lifecycle = FakeLifecycle::default();
        let director = created_agent(&completed(authority_provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            &authority_lifecycle,
        )));
        bind(
            &authority_provider,
            "bind-director",
            director.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );
        let rotate = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("rotate-authority-change"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: director.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::ClosePredecessorThenCreateWithMessage,
                successor: spec("director-r2"),
                frozen_message: Some("Exact Director handoff.".to_string()),
            },
        };
        authority_provider.execute(
            &invocation(director.cutex_session_id.as_str()),
            &rotate,
            &authority_lifecycle,
        );
        bind(
            &authority_provider,
            "external-authority-change",
            "cutex.other-director",
            Some((director.cutex_session_id.as_str(), 2)),
        );
        assert!(matches!(
            AgentManagementProvider::open(&authority_root)
                .unwrap()
                .execute(
                    &invocation(director.cutex_session_id.as_str()),
                    &rotate,
                    &authority_lifecycle
                )
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. }
                if code == "not_authorized_director"
        ));
        assert_eq!(authority_lifecycle.bootstrap_count(), 1);
        assert_eq!(authority_lifecycle.message_count(), 0);
        std::fs::remove_dir_all(authority_root).unwrap();
    }

    #[test]
    fn director_rotation_cas_changes_only_project_pointer_and_excludes_retired() {
        let root = root("director-rotate");
        let ordering_log = Arc::new(Mutex::new(Vec::new()));
        let observer_log = Arc::clone(&ordering_log);
        let provider = AgentManagementProvider::open(&root)
            .unwrap()
            .with_phase_observer(Arc::new(move |event: &AgentManagementPhaseEvent| {
                observer_log
                    .lock()
                    .unwrap()
                    .push(format!("phase:{:?}", event.phase));
            }));
        bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
        let lifecycle = FakeLifecycle::with_ordering_log(Arc::clone(&ordering_log));
        let director = created_agent(&completed(provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        )));
        bind(
            &provider,
            "bind-director",
            director.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );
        let rotate = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("rotate"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: director.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::ClosePredecessorThenCreateWithMessage,
                successor: spec("director-r2"),
                frozen_message: Some("Frozen director handoff.".to_string()),
            },
        };
        ordering_log.lock().unwrap().clear();
        let receipt = completed(provider.execute(
            &invocation(director.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        ));
        let successor = match receipt.result {
            AgentManagementResult::DirectorRotated {
                successor,
                authority,
                ..
            } => {
                assert_eq!(authority.authority_epoch, 3);
                assert_eq!(
                    authority.authorized_director_session,
                    successor.cutex_session_id
                );
                successor
            }
            other => panic!("unexpected rotation: {other:?}"),
        };
        let snapshot = provider.store().snapshot().unwrap();
        let mut rotate_phases = snapshot
            .phase_events
            .values()
            .filter(|event| event.action_id == rotate.action_id)
            .cloned()
            .collect::<Vec<_>>();
        rotate_phases.sort_by_key(|event| event.phase_sequence);
        assert_eq!(
            rotate_phases
                .iter()
                .map(|event| event.phase)
                .collect::<Vec<_>>(),
            vec![
                AgentActionPhase::Prepared,
                AgentActionPhase::PredecessorClosing,
                AgentActionPhase::PredecessorClosed,
                AgentActionPhase::PrivateCwdReady,
                AgentActionPhase::NativeBootstrapPending,
                AgentActionPhase::NativeSessionCaptured,
                AgentActionPhase::Adopted,
                AgentActionPhase::Configured,
                AgentActionPhase::Online,
                AgentActionPhase::Ready,
                AgentActionPhase::MessagePending,
                AgentActionPhase::MessageQueued,
                AgentActionPhase::AuthorityTransferPending,
                AgentActionPhase::AuthorityTransferred,
                AgentActionPhase::SuccessorReady,
                AgentActionPhase::Complete,
            ]
        );
        for event in rotate_phases.iter().take(13) {
            assert_eq!(
                event.presentation_owner_cutex_session_id,
                director.cutex_session_id
            );
            assert_eq!(event.authority_epoch, Some(2));
        }
        for event in rotate_phases.iter().skip(13) {
            assert_eq!(
                event.presentation_owner_cutex_session_id,
                successor.cutex_session_id
            );
            assert_eq!(event.authority_epoch, Some(3));
        }
        let ordered = ordering_log.lock().unwrap().clone();
        let closing = ordered
            .iter()
            .position(|entry| entry == "phase:PredecessorClosing")
            .unwrap();
        let offline = ordered
            .iter()
            .position(|entry| entry.starts_with("lifecycle:offline:"))
            .unwrap();
        assert!(
            closing < offline,
            "phase must be observed before lifecycle close"
        );
        let phase_count = snapshot.phase_events.len();
        let replay = provider.execute(
            &invocation(director.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        );
        assert!(matches!(
            replay.outcome,
            AgentManagementOutcome::Complete { .. }
        ));
        assert_eq!(
            provider.store().snapshot().unwrap().phase_events.len(),
            phase_count
        );
        assert_eq!(
            snapshot.agents[&director.cutex_session_id].project_id,
            project()
        );
        assert!(snapshot.agents[&director.cutex_session_id]
            .retired_at
            .is_some());
        assert_eq!(
            snapshot.agents[&successor.cutex_session_id].project_id,
            project()
        );
        let query = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("query-after-rotate"),
            project_id: Some(project()),
            operation: AgentOperation::QueryManaged,
        };
        assert!(matches!(
            provider
                .execute(
                    &invocation(director.cutex_session_id.as_str()),
                    &query,
                    &lifecycle
                )
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "not_authorized_director"
        ));
        let queried = completed(provider.execute(
            &invocation(successor.cutex_session_id.as_str()),
            &query,
            &lifecycle,
        ));
        match queried.result {
            AgentManagementResult::QueryManaged { agents, .. } => {
                assert_eq!(agents.len(), 1);
                assert_eq!(agents[0].cutex_session_id, successor.cutex_session_id);
            }
            other => panic!("unexpected query: {other:?}"),
        }
        let seat_snapshot = provider.director_seats.query().unwrap();
        assert_eq!(
            seat_snapshot
                .occupancies
                .get(&crate::task_service::SeatId::new("cutex-director").unwrap())
                .unwrap()
                .occupant_cutex_session,
            successor.cutex_session_id
        );
        assert!(seat_snapshot.active_director_transfer.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn director_rotation_preflight_rejects_missing_or_stale_task_service_seat_before_launch() {
        for (label, stale_occupant, expected_reason) in [
            ("missing", None, "director_seat_not_bound"),
            (
                "stale",
                Some("cutex.unrelated-director"),
                "stale_director_seat_occupancy",
            ),
        ] {
            let root = root(&format!("director-seat-preflight-{label}"));
            let provider = AgentManagementProvider::open(&root).unwrap();
            bind_project_only(
                &provider,
                "bind-bootstrap",
                project().as_str(),
                "cutex.bootstrap",
                None,
            );
            let lifecycle = FakeLifecycle::default();
            let predecessor = created_agent(&completed(provider.execute(
                &invocation("cutex.bootstrap"),
                &create_request(
                    "create-director",
                    "director-r1",
                    AgentStartMode::BootstrapOnly,
                ),
                &lifecycle,
            )));
            bind_project_only(
                &provider,
                "bind-director",
                project().as_str(),
                predecessor.cutex_session_id.as_str(),
                Some(("cutex.bootstrap", 1)),
            );
            if let Some(occupant) = stale_occupant {
                provider
                    .director_seats
                    .bind(&crate::seat::SeatOccupancyBindRequest {
                        schema: crate::seat::SeatOccupancyCommandSchema::V1,
                        action_id: ActionId::new("stale-seat-fixture").unwrap(),
                        seat_id: crate::task_service::SeatId::new("cutex-director").unwrap(),
                        occupant_cutex_session: session(occupant),
                    })
                    .unwrap();
            }
            let rotate = AgentManagementRequest {
                schema: AgentManagementSchema::V1,
                action_id: action("rotate-preflight"),
                project_id: Some(project()),
                operation: AgentOperation::DirectorRotate {
                    expected_predecessor_cutex_session: predecessor.cutex_session_id.clone(),
                    expected_authority_epoch: 2,
                    mode: DirectorRotateMode::ClosePredecessorThenCreateWithMessage,
                    successor: spec("director-r2"),
                    frozen_message: Some("Do not deliver.".to_string()),
                },
            };
            let response = provider.execute(
                &invocation(predecessor.cutex_session_id.as_str()),
                &rotate,
                &lifecycle,
            );
            assert!(matches!(
                response.outcome,
                AgentManagementOutcome::NoWrite { ref detail, .. }
                    if detail.contains(expected_reason)
            ));
            assert_eq!(lifecycle.bootstrap_count(), 1);
            assert_eq!(lifecycle.message_count(), 0);
            assert!(
                lifecycle
                    .observe(&predecessor.cutex_session_id)
                    .unwrap()
                    .active
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn director_rotation_recovers_process_loss_between_seat_and_project_authority() {
        let root = root("director-seat-project-boundary-loss");
        let provider = AgentManagementProvider::open(&root)
            .unwrap()
            .with_fail_after_director_seat_transfer_once();
        bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
        let lifecycle = FakeLifecycle::default();
        let predecessor = created_agent(&completed(provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        )));
        bind(
            &provider,
            "bind-director",
            predecessor.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );
        let rotate = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("rotate-boundary-loss"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: predecessor.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::RetainPredecessorBootstrapOnly,
                successor: spec("director-r2"),
                frozen_message: None,
            },
        };
        let interrupted = provider.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        );
        assert!(matches!(
            interrupted.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "injected_process_loss"
        ));
        let management = provider.store().snapshot().unwrap();
        let successor = management.actions[&rotate.action_id]
            .known_successor_cutex_session
            .clone()
            .unwrap();
        assert_eq!(
            management.projects[&project()].authorized_director_session,
            predecessor.cutex_session_id
        );
        assert_eq!(
            management.actions[&rotate.action_id].phase,
            AgentActionPhase::AuthorityTransferPending
        );
        let seats = provider.director_seats.query().unwrap();
        assert_eq!(
            seats.occupancies[&crate::task_service::SeatId::new("cutex-director").unwrap()]
                .occupant_cutex_session,
            successor
        );
        assert_eq!(
            seats.active_director_transfer,
            Some(director_seat_transfer_action_id(&rotate.action_id).unwrap())
        );
        assert_eq!(lifecycle.bootstrap_count(), 2);

        drop(provider);
        let reopened = AgentManagementProvider::open(&root).unwrap();
        let completed_response = reopened.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        );
        let replay = reopened.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        );
        assert_eq!(completed_response, replay);
        let receipt = completed(completed_response);
        let AgentManagementResult::DirectorRotated {
            successor: completed_successor,
            authority,
            ..
        } = receipt.result
        else {
            panic!("expected completed Director rotation")
        };
        assert_eq!(completed_successor.cutex_session_id, successor);
        assert_eq!(authority.authorized_director_session, successor);
        assert_eq!(
            reopened.store().snapshot().unwrap().projects[&project()],
            authority
        );
        let seats = reopened.director_seats.query().unwrap();
        assert_eq!(
            seats.occupancies[&crate::task_service::SeatId::new("cutex-director").unwrap()]
                .occupant_cutex_session,
            successor
        );
        assert!(seats.active_director_transfer.is_none());
        assert_eq!(lifecycle.bootstrap_count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_director_rotation_replay_fails_closed_after_unrelated_seat_rebind() {
        let root = root("director-complete-seat-divergence");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
        let lifecycle = FakeLifecycle::default();
        let predecessor = created_agent(&completed(provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        )));
        bind(
            &provider,
            "bind-director",
            predecessor.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );
        let rotate = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("rotate-then-diverge"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: predecessor.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::RetainPredecessorBootstrapOnly,
                successor: spec("director-r2"),
                frozen_message: None,
            },
        };
        completed(provider.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        ));
        provider
            .director_seats
            .bind(&crate::seat::SeatOccupancyBindRequest {
                schema: crate::seat::SeatOccupancyCommandSchema::V1,
                action_id: ActionId::new("unrelated-admin-rebind").unwrap(),
                seat_id: crate::task_service::SeatId::new("cutex-director").unwrap(),
                occupant_cutex_session: session("cutex.unrelated-director"),
            })
            .unwrap();
        let replay = provider.execute(
            &invocation(predecessor.cutex_session_id.as_str()),
            &rotate,
            &lifecycle,
        );
        assert!(matches!(
            replay.outcome,
            AgentManagementOutcome::NoWrite { ref detail, .. }
                if detail.contains("director_seat_changed_after_transfer")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_rotation_modes_emit_only_committed_close_and_message_subsets() {
        for (label, mode, frozen_message, expects_message) in [
            (
                "retain-message",
                DirectorRotateMode::RetainPredecessorWithMessage,
                Some("Frozen retained handoff.".to_string()),
                true,
            ),
            (
                "retain-bootstrap",
                DirectorRotateMode::RetainPredecessorBootstrapOnly,
                None,
                false,
            ),
        ] {
            let root = root(label);
            let provider = AgentManagementProvider::open(&root).unwrap();
            bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
            let lifecycle = FakeLifecycle::default();
            let predecessor = created_agent(&completed(provider.execute(
                &invocation("cutex.bootstrap"),
                &create_request(
                    "create-director",
                    "director-r1",
                    AgentStartMode::BootstrapOnly,
                ),
                &lifecycle,
            )));
            bind(
                &provider,
                "bind-director",
                predecessor.cutex_session_id.as_str(),
                Some(("cutex.bootstrap", 1)),
            );
            let rotate = AgentManagementRequest {
                schema: AgentManagementSchema::V1,
                action_id: action("rotate"),
                project_id: Some(project()),
                operation: AgentOperation::DirectorRotate {
                    expected_predecessor_cutex_session: predecessor.cutex_session_id.clone(),
                    expected_authority_epoch: 2,
                    mode,
                    successor: spec("director-r2"),
                    frozen_message,
                },
            };
            let receipt = completed(provider.execute(
                &invocation(predecessor.cutex_session_id.as_str()),
                &rotate,
                &lifecycle,
            ));
            let AgentManagementResult::DirectorRotated { successor, .. } = receipt.result else {
                panic!("expected Director rotation")
            };
            let snapshot = provider.store().snapshot().unwrap();
            assert!(snapshot.agents[&predecessor.cutex_session_id]
                .retired_at
                .is_none());
            let phases = snapshot
                .phase_events
                .values()
                .filter(|event| event.action_id == rotate.action_id)
                .map(|event| event.phase)
                .collect::<Vec<_>>();
            assert!(!phases.contains(&AgentActionPhase::PredecessorClosing));
            assert!(!phases.contains(&AgentActionPhase::PredecessorClosed));
            assert_eq!(
                phases.contains(&AgentActionPhase::MessagePending),
                expects_message
            );
            assert_eq!(
                phases.contains(&AgentActionPhase::MessageQueued),
                expects_message
            );
            for required in [
                AgentActionPhase::AuthorityTransferPending,
                AgentActionPhase::AuthorityTransferred,
                AgentActionPhase::SuccessorReady,
                AgentActionPhase::Complete,
            ] {
                assert!(phases.contains(&required));
            }
            let successor_ready = snapshot
                .phase_events
                .values()
                .find(|event| {
                    event.action_id == rotate.action_id
                        && event.phase == AgentActionPhase::SuccessorReady
                })
                .unwrap();
            assert_eq!(
                successor_ready.presentation_owner_cutex_session_id,
                successor.cutex_session_id
            );
            assert_eq!(successor_ready.authority_epoch, Some(3));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn deterministic_pre_effect_no_write_releases_only_the_cross_action_reservation() {
        let root = root("pre-effect-no-write");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind-bootstrap", "cutex.bootstrap", None);
        let lifecycle = FakeLifecycle::default();
        let director = created_agent(&completed(provider.execute(
            &invocation("cutex.bootstrap"),
            &create_request(
                "create-director",
                "director-r1",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        )));
        bind(
            &provider,
            "bind-director",
            director.cutex_session_id.as_str(),
            Some(("cutex.bootstrap", 1)),
        );

        let stale = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("stale-rotate"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: session("cutex.mistyped-director"),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::RetainPredecessorBootstrapOnly,
                successor: spec("director-r2"),
                frozen_message: None,
            },
        };
        let first = provider.execute(
            &invocation(director.cutex_session_id.as_str()),
            &stale,
            &lifecycle,
        );
        assert!(matches!(
            first.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        let snapshot = provider.store().snapshot().unwrap();
        let stale_action = &snapshot.actions[&stale.action_id];
        assert_eq!(stale_action.phase, AgentActionPhase::NoWrite);
        assert!(stale_action.response.is_some());
        assert_eq!(
            provider.execute(
                &invocation(director.cutex_session_id.as_str()),
                &stale,
                &lifecycle,
            ),
            first,
            "exact replay returns the original terminal NoWrite"
        );

        let mut changed_same_action = stale.clone();
        if let AgentOperation::DirectorRotate {
            expected_predecessor_cutex_session,
            ..
        } = &mut changed_same_action.operation
        {
            *expected_predecessor_cutex_session = director.cutex_session_id.clone();
        }
        assert!(matches!(
            provider
                .execute(
                    &invocation(director.cutex_session_id.as_str()),
                    &changed_same_action,
                    &lifecycle,
                )
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));

        let corrected = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("corrected-rotate"),
            project_id: Some(project()),
            operation: AgentOperation::DirectorRotate {
                expected_predecessor_cutex_session: director.cutex_session_id.clone(),
                expected_authority_epoch: 2,
                mode: DirectorRotateMode::RetainPredecessorBootstrapOnly,
                successor: spec("director-r2"),
                frozen_message: None,
            },
        };
        let corrected_receipt = completed(provider.execute(
            &invocation(director.cutex_session_id.as_str()),
            &corrected,
            &lifecycle,
        ));
        assert!(matches!(
            corrected_receipt.result,
            AgentManagementResult::DirectorRotated { .. }
        ));
        assert_eq!(lifecycle.bootstrap_count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn definite_active_collision_releases_reservation_for_a_corrected_fresh_action() {
        let root = root("active-collision-no-write");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let taken = created_agent(&completed(provider.execute(
            &invocation("cutex.director"),
            &create_request(
                "create-taken",
                "worker-taken",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        )));

        let collision = create_request(
            "create-collision",
            "worker-taken",
            AgentStartMode::BootstrapOnly,
        );
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &collision, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        assert_eq!(
            provider.store().snapshot().unwrap().actions[&collision.action_id].phase,
            AgentActionPhase::NoWrite
        );
        let close = AgentManagementRequest {
            schema: AgentManagementSchema::V1,
            action_id: action("close-taken"),
            project_id: Some(project()),
            operation: AgentOperation::Close {
                cutex_session_id: taken.cutex_session_id,
            },
        };
        completed(provider.execute(&invocation("cutex.director"), &close, &lifecycle));
        completed(provider.execute(
            &invocation("cutex.director"),
            &create_request(
                "create-corrected",
                "worker-taken",
                AgentStartMode::BootstrapOnly,
            ),
            &lifecycle,
        ));
        assert_eq!(lifecycle.bootstrap_count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn definite_pre_sid_failure_retries_exact_create_after_external_repair() {
        let root = root("pre-sid-retry");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::fail_bootstrap_once_before_sid();
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);

        let failed = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            failed.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        let snapshot = provider.store().snapshot().unwrap();
        let pending = &snapshot.actions[&request.action_id];
        assert_eq!(pending.phase, AgentActionPhase::NativeBootstrapPending);
        assert!(pending.native_bootstrap_retryable);
        assert!(pending.response.is_none());
        assert!(pending.known_native_session_id.is_none());
        assert!(pending.known_successor_cutex_session.is_none());

        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));
        let agent = created_agent(&receipt);
        assert_eq!(agent.spec, spec("worker"));
        assert_eq!(lifecycle.bootstrap_count(), 2);
        assert_eq!(lifecycle.message_count(), 1);
        assert_eq!(provider.store().snapshot().unwrap().agents.len(), 1);
        let complete = provider.store().snapshot().unwrap().actions[&request.action_id].clone();
        assert_eq!(complete.phase, AgentActionPhase::Complete);
        assert!(!complete.native_bootstrap_retryable);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_sid_retry_survives_service_restart() {
        let root = root("pre-sid-restart");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::fail_bootstrap_once_before_sid();
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &request, &lifecycle)
                .outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        drop(provider);

        let reopened = AgentManagementProvider::open(&root).unwrap();
        let receipt =
            completed(reopened.execute(&invocation("cutex.director"), &request, &lifecycle));
        assert_eq!(created_agent(&receipt).spec, spec("worker"));
        assert_eq!(lifecycle.bootstrap_count(), 2);
        assert_eq!(reopened.store().snapshot().unwrap().agents.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_exact_pre_sid_retries_launch_only_one_replacement_attempt() {
        let root = root("pre-sid-concurrent");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = Arc::new(FakeLifecycle::fail_bootstrap_once_before_sid());
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
                .outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        drop(provider);

        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let request = request.clone();
            let lifecycle = lifecycle.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let provider = AgentManagementProvider::open(root).unwrap();
                barrier.wait();
                provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
            }));
        }
        barrier.wait();
        let responses = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(responses[0], responses[1]);
        assert!(responses
            .iter()
            .all(|response| matches!(response.outcome, AgentManagementOutcome::Complete { .. })));
        assert_eq!(lifecycle.bootstrap_count(), 2);
        assert_eq!(lifecycle.message_count(), 1);
        assert_eq!(
            AgentManagementProvider::open(&root)
                .unwrap()
                .store()
                .snapshot()
                .unwrap()
                .agents
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uncertain_pre_sid_outcome_remains_fenced_and_never_relaunches() {
        let root = root("pre-sid-uncertain");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::fail_bootstrap_once_with_uncertain_sid();
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);

        let first = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            first.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        let fenced = provider.store().snapshot().unwrap().actions[&request.action_id].clone();
        assert_eq!(fenced.phase, AgentActionPhase::OwnerActionRequired);
        assert!(!fenced.native_bootstrap_retryable);
        assert!(fenced.response.is_some());
        let mut accepted_snapshot =
            serde_json::to_value(provider.store().snapshot().unwrap()).unwrap();
        accepted_snapshot["actions"][request.action_id.as_str()]
            .as_object_mut()
            .unwrap()
            .remove("native_bootstrap_retryable");
        let decoded: AgentManagementSnapshot = serde_json::from_value(accepted_snapshot)
            .expect("accepted action without the R23 retry field remains readable");
        assert!(!decoded.actions[&request.action_id].native_bootstrap_retryable);
        assert_eq!(
            provider.execute(&invocation("cutex.director"), &request, &lifecycle),
            first
        );
        assert_eq!(lifecycle.bootstrap_count(), 1);

        let bypass = create_request("bypass", "worker", AgentStartMode::BootstrapOnly);
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &bypass, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        assert_eq!(lifecycle.bootstrap_count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_legacy_native_session_unknown_receipt_upgrades_to_current_authority_retry() {
        let root = root("legacy-pre-sid-retry");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        seed_legacy_no_sid_receipt(&provider, &request);

        bind(
            &provider,
            "rotate-away",
            "cutex.other-director",
            Some(("cutex.director", 1)),
        );
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &request, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "not_authorized_director"
        ));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert!(
            provider.store().snapshot().unwrap().actions[&request.action_id]
                .response
                .is_some()
        );
        bind(
            &provider,
            "rotate-back",
            "cutex.director",
            Some(("cutex.other-director", 2)),
        );

        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));
        assert_eq!(created_agent(&receipt).spec, spec("worker"));
        assert_eq!(
            lifecycle
                .log()
                .iter()
                .filter(|entry| entry.as_str() == "reconcile-pre-sid")
                .count(),
            1,
            "the byte-exact live stored detail must enter reconciliation"
        );
        assert_eq!(lifecycle.bootstrap_count(), 1);
        assert_eq!(provider.store().snapshot().unwrap().agents.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_current_native_session_unknown_receipt_reconciles_and_retries_once() {
        let root = root("current-pre-sid-retry");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);
        seed_owner_action_receipt(&provider, &request, REAL_CURRENT_NO_SID_DETAIL);

        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));

        assert_eq!(created_agent(&receipt).spec, spec("worker"));
        assert_eq!(lifecycle.bootstrap_count(), 1);
        assert_eq!(lifecycle.message_count(), 1);
        assert_eq!(
            lifecycle
                .log()
                .iter()
                .filter(|entry| entry.as_str() == "reconcile-pre-sid")
                .count(),
            1
        );
        assert_eq!(
            provider.store().snapshot().unwrap().actions[&request.action_id].phase,
            AgentActionPhase::Complete
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_live_malformed_jsonl_receipt_reconciles_proven_absence_once_after_restart() {
        let root = root("malformed-jsonl-proven-absent-retry");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);
        seed_malformed_jsonl_receipt(&provider, &request);
        drop(provider);

        let lifecycle = Arc::new(FakeLifecycle::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let request = request.clone();
                let lifecycle = Arc::clone(&lifecycle);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let provider = AgentManagementProvider::open(root).unwrap();
                    barrier.wait();
                    provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
                })
            })
            .collect::<Vec<_>>();
        let receipts = handles
            .into_iter()
            .map(|handle| completed(handle.join().unwrap()))
            .collect::<Vec<_>>();

        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(lifecycle.bootstrap_count(), 1);
        assert_eq!(lifecycle.message_count(), 1);
        let reconciliation_count = lifecycle
            .log()
            .iter()
            .filter(|entry| entry.as_str() == "reconcile-pre-sid")
            .count();
        assert!((1..=2).contains(&reconciliation_count));
        let snapshot = AgentManagementProvider::open(&root)
            .unwrap()
            .store()
            .snapshot()
            .unwrap();
        assert_eq!(snapshot.agents.len(), 1);
        assert_eq!(
            snapshot.actions[&request.action_id].phase,
            AgentActionPhase::Complete
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_jsonl_receipt_present_ambiguous_or_unavailable_never_relaunches() {
        let cases = [
            (
                "present",
                NativeBootstrapReconciliation::Present {
                    reason: "exact managed runtime is present".to_string(),
                },
                "native_bootstrap_reconciliation_present",
            ),
            (
                "ambiguous",
                NativeBootstrapReconciliation::Ambiguous {
                    reason: "native evidence cannot be correlated exactly".to_string(),
                },
                "native_bootstrap_reconciliation_ambiguous",
            ),
            (
                "unavailable",
                NativeBootstrapReconciliation::Unavailable {
                    reason: "selected profile registry is unavailable".to_string(),
                },
                "native_bootstrap_reconciliation_unavailable",
            ),
        ];

        for (label, reconciliation, expected_code) in cases {
            let root = root(&format!("malformed-jsonl-{label}"));
            let provider = AgentManagementProvider::open(&root).unwrap();
            bind(&provider, "bind", "cutex.director", None);
            let lifecycle = FakeLifecycle::with_reconciliation(reconciliation);
            let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
            let original = seed_malformed_jsonl_receipt(&provider, &request);

            let response = provider.execute(&invocation("cutex.director"), &request, &lifecycle);

            assert!(matches!(
                &response.outcome,
                AgentManagementOutcome::OwnerActionRequired { failure }
                    if failure.code == expected_code
            ));
            assert_eq!(lifecycle.bootstrap_count(), 0);
            let snapshot = provider.store().snapshot().unwrap();
            assert_eq!(
                snapshot.actions[&request.action_id].response.as_ref(),
                Some(&original)
            );
            assert!(snapshot.agents.is_empty());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn unrelated_owner_action_detail_replays_immutably_without_reconciliation() {
        assert!(legacy_pre_sid_failure_detail_matches(
            REAL_LEGACY_NO_SID_DETAIL
        ));
        assert!(legacy_pre_sid_failure_detail_matches(
            "native_session_unknown: native exec exited exit status: 1 without one captured SID"
        ));
        assert!(legacy_pre_sid_failure_detail_matches(
            "owner_action_required: native_session_unknown: native exec exited exit status: 1 without one captured SID (external outcome unknown)"
        ));
        assert!(legacy_pre_sid_failure_detail_matches(
            "owner_action_required: native_session_unknown: native exec exited exit status: 1 without one captured SID; diagnostic: stderr: provider unavailable (external outcome unknown)"
        ));
        assert!(legacy_pre_sid_failure_detail_matches(
            REAL_MALFORMED_JSONL_DETAIL
        ));
        assert!(legacy_pre_sid_failure_detail_matches(&format!(
            "owner_action_required: {REAL_MALFORMED_JSONL_DETAIL}"
        )));
        assert!(!legacy_pre_sid_failure_detail_matches(
            "native_bootstrap_output_malformed: native exec JSONL stdout line 2 is not valid JSON (external outcome unknown)"
        ));
        assert!(!legacy_pre_sid_failure_detail_matches(
            "owner_action_required: owner_action_required: native_session_unknown: native exec exited exit status: 1 without one captured SID"
        ));
        assert!(!legacy_pre_sid_failure_detail_matches(
            "owner_action_required: native_session_unknown: unrelated failure without one captured SID (external outcome unknown)"
        ));
        assert!(!legacy_pre_sid_failure_detail_matches(
            "owner_action_required: unrelated startup failure"
        ));
        assert!(legacy_ambiguous_sid_failure_detail_matches(
            REAL_LEGACY_AMBIGUOUS_SID_DETAIL
        ));
        assert!(legacy_ambiguous_sid_failure_detail_matches(
            "native_session_ambiguous: native exec exposed multiple possible session identities"
        ));
        assert!(!legacy_ambiguous_sid_failure_detail_matches(
            "owner_action_required: native_session_ambiguous: unrelated ambiguity"
        ));
        assert!(offline_revision_conflict_detail_matches(
            REAL_OFFLINE_REVISION_CONFLICT_DETAIL
        ));
        assert!(!offline_revision_conflict_detail_matches(
            "owner_action_required: session_offline_failed: unrelated persistence failure (external outcome unknown)"
        ));
        assert!(!offline_revision_conflict_detail_matches(
            "owner_action_required: session_online_failed: cutex session store revision conflict: expected 1, current 2; reload before retrying (external outcome unknown)"
        ));
        let root = root("legacy-pre-sid-unrelated-detail");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        let original = seed_owner_action_receipt(
            &provider,
            &request,
            "owner_action_required: unrelated startup failure",
        );

        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);

        assert_eq!(replay, original);
        assert!(!lifecycle
            .log()
            .iter()
            .any(|entry| entry == "reconcile-pre-sid"));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert_eq!(
            provider.store().snapshot().unwrap().actions[&request.action_id]
                .response
                .as_ref(),
            Some(&original)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_no_output_receipt_with_existing_runtime_stays_fenced_without_bootstrap() {
        let root = root("legacy-pre-sid-existing-runtime");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        let original = seed_legacy_no_sid_receipt(&provider, &request);
        lifecycle.insert_agent(
            &session("cutex.existing-native-runtime"),
            "native-existing",
            &spec("worker"),
        );

        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);

        assert!(matches!(
            &replay.outcome,
            AgentManagementOutcome::OwnerActionRequired { failure }
                if failure.code == "native_bootstrap_reconciliation_present"
                    && failure.detail.contains("exact managed runtime is present")
                    && failure.detail.contains("immutable original receipt remains unchanged")
        ));
        assert_ne!(replay, original);
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert_eq!(
            lifecycle
                .log()
                .iter()
                .filter(|entry| entry.as_str() == "reconcile-pre-sid")
                .count(),
            1
        );
        let action = &provider.store().snapshot().unwrap().actions[&request.action_id];
        assert_eq!(action.phase, AgentActionPhase::OwnerActionRequired);
        assert!(!action.native_bootstrap_retryable);
        assert_eq!(action.response.as_ref(), Some(&original));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_no_output_receipt_projects_unavailable_reason_without_mutating_receipt() {
        let root = root("legacy-pre-sid-unavailable");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle =
            FakeLifecycle::with_reconciliation(NativeBootstrapReconciliation::Unavailable {
                reason: "native rollout evidence is malformed".to_string(),
            });
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        let original = seed_legacy_no_sid_receipt(&provider, &request);

        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);

        assert!(matches!(
            &replay.outcome,
            AgentManagementOutcome::OwnerActionRequired { failure }
                if failure.code == "native_bootstrap_reconciliation_unavailable"
                    && failure.detail.contains("native rollout evidence is malformed")
        ));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        let action = &provider.store().snapshot().unwrap().actions[&request.action_id];
        assert_eq!(action.response.as_ref(), Some(&original));
        assert_eq!(action.phase, AgentActionPhase::OwnerActionRequired);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_no_output_receipt_projects_ambiguous_reason_without_bootstrap() {
        let root = root("legacy-pre-sid-ambiguous");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle =
            FakeLifecycle::with_reconciliation(NativeBootstrapReconciliation::Ambiguous {
                reason: "native session lacks a managed-cwd marker".to_string(),
            });
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        let original = seed_legacy_no_sid_receipt(&provider, &request);

        let replay = provider.execute(&invocation("cutex.director"), &request, &lifecycle);

        assert!(matches!(
            &replay.outcome,
            AgentManagementOutcome::OwnerActionRequired { failure }
                if failure.code == "native_bootstrap_reconciliation_ambiguous"
                    && failure.detail.contains("lacks a managed-cwd marker")
        ));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert_eq!(
            provider.store().snapshot().unwrap().actions[&request.action_id]
                .response
                .as_ref(),
            Some(&original)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_ambiguous_attempt_recovery_survives_restart_and_concurrent_replay_without_bootstrap() {
        let root = root("ambiguous-sid-exact-recovery");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let request = create_request("create", "worker", AgentStartMode::CustomMessage);
        seed_legacy_ambiguous_sid_receipt(&provider, &request);
        drop(provider);

        let lifecycle = Arc::new(FakeLifecycle::with_identity_reconciliation(
            NativeBootstrapIdentityReconciliation::Exact {
                native_session_id: RECOVERED_NATIVE_SID.to_string(),
                reason: "one exact selected-profile rollout matches the attempt".to_string(),
            },
        ));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let root = root.clone();
                let request = request.clone();
                let lifecycle = Arc::clone(&lifecycle);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let provider = AgentManagementProvider::open(root).unwrap();
                    barrier.wait();
                    provider.execute(&invocation("cutex.director"), &request, lifecycle.as_ref())
                })
            })
            .collect::<Vec<_>>();
        let responses = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let receipts = responses.into_iter().map(completed).collect::<Vec<_>>();
        assert_eq!(receipts[0], receipts[1]);
        assert_eq!(
            created_agent(&receipts[0]).native_session_id,
            RECOVERED_NATIVE_SID
        );
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert_eq!(lifecycle.message_count(), 1);
        let snapshot = AgentManagementProvider::open(&root)
            .unwrap()
            .store()
            .snapshot()
            .unwrap();
        assert_eq!(snapshot.agents.len(), 1);
        assert_eq!(
            snapshot.failure_events
                [&format!("agent-management:{}:failure", request.action_id.as_str())]
                .detail,
            REAL_LEGACY_AMBIGUOUS_SID_DETAIL
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ambiguous_attempt_absent_multiple_and_unavailable_evidence_never_relaunch() {
        let cases = [
            (
                "absent",
                NativeBootstrapIdentityReconciliation::Absent {
                    reason: "no selected-profile thread matches".to_string(),
                },
                "native_bootstrap_reconciliation_absent",
            ),
            (
                "multiple",
                NativeBootstrapIdentityReconciliation::Ambiguous {
                    reason: "two exact-cwd native rollouts match".to_string(),
                },
                "native_bootstrap_reconciliation_ambiguous",
            ),
            (
                "unavailable",
                NativeBootstrapIdentityReconciliation::Unavailable {
                    reason: "selected-profile session index is malformed".to_string(),
                },
                "native_bootstrap_reconciliation_unavailable",
            ),
        ];
        for (label, reconciliation, expected_code) in cases {
            let root = root(&format!("ambiguous-sid-{label}"));
            let provider = AgentManagementProvider::open(&root).unwrap();
            bind(&provider, "bind", "cutex.director", None);
            let lifecycle = FakeLifecycle::with_identity_reconciliation(reconciliation);
            let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
            let original = seed_legacy_ambiguous_sid_receipt(&provider, &request);

            let response = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
            assert!(matches!(
                &response.outcome,
                AgentManagementOutcome::OwnerActionRequired { failure }
                    if failure.code == expected_code
            ));
            assert_eq!(lifecycle.bootstrap_count(), 0);
            let stored = provider.store().snapshot().unwrap();
            assert_eq!(
                stored.actions[&request.action_id].response.as_ref(),
                Some(&original)
            );
            assert!(stored.agents.is_empty());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn ambiguous_attempt_recovery_requires_current_director_authority() {
        let root = root("ambiguous-sid-authority");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let request = create_request("create", "worker", AgentStartMode::BootstrapOnly);
        seed_legacy_ambiguous_sid_receipt(&provider, &request);
        bind(
            &provider,
            "rotate-authority",
            "cutex.new-director",
            Some(("cutex.director", 1)),
        );
        let lifecycle = FakeLifecycle::with_identity_reconciliation(
            NativeBootstrapIdentityReconciliation::Exact {
                native_session_id: RECOVERED_NATIVE_SID.to_string(),
                reason: "exact evidence".to_string(),
            },
        );

        let response = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            response.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "not_authorized_director"
        ));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert!(provider.store().snapshot().unwrap().agents.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn real_known_sid_failure_is_resumable_and_reservations_block_bypass() {
        let root = root("known-sid-resume");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::fail_bootstrap_once_with_known_sid("native-7");
        let request = create_request("captured", "worker", AgentStartMode::BootstrapOnly);

        let failed = provider.execute(&invocation("cutex.director"), &request, &lifecycle);
        assert!(matches!(
            failed.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        let snapshot = provider.store().snapshot().unwrap();
        let action = &snapshot.actions[&request.action_id];
        assert_eq!(
            action.phase,
            AgentActionPhase::NativeSessionCaptured,
            "known SID failure must retain its exact continuation phase"
        );
        assert_eq!(action.known_native_session_id.as_deref(), Some("native-7"));
        assert!(action.response.is_none(), "known SID failure is resumable");

        let mut changed_same_action = request.clone();
        if let AgentOperation::Create {
            spec: candidate_spec,
            ..
        } = &mut changed_same_action.operation
        {
            candidate_spec.name = "changed-name".to_string();
        }
        assert!(matches!(
            provider
                .execute(
                    &invocation("cutex.director"),
                    &changed_same_action,
                    &lifecycle
                )
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));

        let mut changed_name = create_request(
            "fresh-same-cwd",
            "changed-name",
            AgentStartMode::BootstrapOnly,
        );
        if let AgentOperation::Create {
            spec: candidate_spec,
            ..
        } = &mut changed_name.operation
        {
            candidate_spec.cwd = spec("worker").cwd;
        }
        let changed_name_response =
            provider.execute(&invocation("cutex.director"), &changed_name, &lifecycle);
        assert!(matches!(
            changed_name_response.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));

        let mut changed_cwd =
            create_request("fresh-same-name", "worker", AgentStartMode::BootstrapOnly);
        if let AgentOperation::Create {
            spec: candidate_spec,
            ..
        } = &mut changed_cwd.operation
        {
            candidate_spec.cwd = test_agent_cwd("changed-cwd");
        }
        let changed_cwd_response =
            provider.execute(&invocation("cutex.director"), &changed_cwd, &lifecycle);
        assert!(matches!(
            changed_cwd_response.outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        assert_eq!(lifecycle.bootstrap_count(), 1);

        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &request, &lifecycle));
        assert_eq!(created_agent(&receipt).native_session_id, "native-7");
        assert_eq!(lifecycle.bootstrap_count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn captured_sid_continues_exact_agent_and_unknown_bootstrap_never_creates_second() {
        let root = root("captured-sid");
        let provider = AgentManagementProvider::open(&root).unwrap();
        bind(&provider, "bind", "cutex.director", None);
        let lifecycle = FakeLifecycle::default();
        let captured = create_request("captured", "worker", AgentStartMode::BootstrapOnly);
        let digest = request_sha256(&captured).unwrap();
        let timestamp = now();
        provider
            .store()
            .with_state(true, |mut state| {
                state.actions.insert(
                    captured.action_id.clone(),
                    AgentActionRecord {
                        action_id: captured.action_id.clone(),
                        request_sha256: digest,
                        operation: AgentOperationKind::Create,
                        project_id: project(),
                        caller_cutex_session: session("cutex.director"),
                        phase: AgentActionPhase::NativeSessionCaptured,
                        phase_sequence: 0,
                        reserved_agent_name: Some("worker".to_string()),
                        reserved_agent_cwd: Some(spec("worker").cwd),
                        known_successor_cutex_session: None,
                        known_native_session_id: Some("native-7".to_string()),
                        native_bootstrap_retryable: false,
                        historical_runtime_occurrence_fence: None,
                        external_message_id: None,
                        response: None,
                        created_at: timestamp.clone(),
                        updated_at: timestamp,
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        let receipt =
            completed(provider.execute(&invocation("cutex.director"), &captured, &lifecycle));
        assert_eq!(created_agent(&receipt).native_session_id, "native-7");
        assert_eq!(lifecycle.bootstrap_count(), 0);

        let unknown = create_request("unknown", "worker-unknown", AgentStartMode::BootstrapOnly);
        let digest = request_sha256(&unknown).unwrap();
        let timestamp = now();
        provider
            .store()
            .with_state(true, |mut state| {
                state.actions.insert(
                    unknown.action_id.clone(),
                    AgentActionRecord {
                        action_id: unknown.action_id.clone(),
                        request_sha256: digest,
                        operation: AgentOperationKind::Create,
                        project_id: project(),
                        caller_cutex_session: session("cutex.director"),
                        phase: AgentActionPhase::NativeBootstrapPending,
                        phase_sequence: 0,
                        reserved_agent_name: Some("worker-unknown".to_string()),
                        reserved_agent_cwd: Some(spec("worker-unknown").cwd),
                        known_successor_cutex_session: None,
                        known_native_session_id: None,
                        native_bootstrap_retryable: false,
                        historical_runtime_occurrence_fence: None,
                        external_message_id: None,
                        response: None,
                        created_at: timestamp.clone(),
                        updated_at: timestamp,
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        let response = provider.execute(&invocation("cutex.director"), &unknown, &lifecycle);
        assert!(matches!(
            response.outcome,
            AgentManagementOutcome::OwnerActionRequired { .. }
        ));
        assert_eq!(
            provider.store().snapshot().unwrap().actions[&unknown.action_id].phase,
            AgentActionPhase::OwnerActionRequired,
            "an outcome-unknown pre-SID action must retain its fence"
        );
        assert_eq!(lifecycle.bootstrap_count(), 0);
        assert_eq!(provider.store().snapshot().unwrap().agents.len(), 1);

        let mut bypass = create_request(
            "unknown-bypass",
            "changed-unknown-name",
            AgentStartMode::BootstrapOnly,
        );
        if let AgentOperation::Create {
            spec: candidate_spec,
            ..
        } = &mut bypass.operation
        {
            candidate_spec.cwd = spec("worker-unknown").cwd;
        }
        assert!(matches!(
            provider
                .execute(&invocation("cutex.director"), &bypass, &lifecycle)
                .outcome,
            AgentManagementOutcome::NoWrite { ref code, .. } if code == "conflict"
        ));
        assert_eq!(lifecycle.bootstrap_count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }
}
