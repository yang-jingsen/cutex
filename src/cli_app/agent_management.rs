use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::Context;

use cutex::agent_bus::model::AgentBusSendResponse;
use cutex::agent_bus::model::AgentRegistrationClass;
use cutex::agent_management::{
    AgentActionPhase, AgentLifecycle, AgentManagementInvocation, AgentManagementMessageMetadata,
    AgentManagementOutcome, AgentManagementPhaseEvent, AgentManagementPhaseObserver,
    AgentManagementProvider, AgentManagementRequest, AgentManagementResponse,
    AgentManagementSchema, AgentOperationKind, AgentRuntimeObservation,
    HistoricalRuntimeOccurrenceReconciliation, LegacyDirectorOwnershipEvidence,
    LegacyDirectorOwnershipImportOutcome, LegacyDirectorOwnershipImportRequest,
    LegacyDirectorOwnershipImportResponse, LifecycleFailure, ManagedAgentSpec,
    NativeBootstrapIdentityReconciliation, NativeBootstrapReconciliation, ProjectAuthorityOutcome,
    ProjectAuthorityRequest, ProjectAuthorityResponse, RuntimeOccurrenceFence,
    RuntimeRecoveryOutcome,
};
use cutex::management::v2::integration_events::{
    append_agent_management_outcome, append_agent_management_phase,
};
use cutex::role_revision::{CutexSessionId, Rfc3339};
use cutex::runtime::alden::cute_alden_sessions;
use cutex::runtime::lifecycle::default_cutex_alden_session_name;
use cutex::session::model::{
    parse_cutex_session_runtime_backend, CutexSessionQuickActionMode, CutexSessionRecord,
    CutexSessionRuntimeBackend,
};
use cutex::session::service::{
    adopt_cutex_session, coding_registration_from_cutex_session_record,
    cutex_session_key_for_user_id_including_retired, cutex_session_launch_cwd,
    persist_cutex_session_store_and_im_record, CutexSessionAdoptOptions, CutexSessionEnsureSeed,
};
use cutex::session::store::load_cutex_session_store;

use super::{management_archive, management_lifecycle};

pub(crate) fn handle_agent_management(
    state: &std::sync::Arc<std::sync::Mutex<cutex::agent_bus::store::AgentBusState>>,
    invocation: AgentManagementInvocation,
    request: AgentManagementRequest,
) -> anyhow::Result<serde_json::Value> {
    Ok(execute_agent_management(state, invocation, request))
}

fn execute_agent_management(
    state: &std::sync::Arc<std::sync::Mutex<cutex::agent_bus::store::AgentBusState>>,
    invocation: AgentManagementInvocation,
    request: AgentManagementRequest,
) -> serde_json::Value {
    let action_id = request.action_id.clone();
    let caller_cutex_session = invocation.caller_cutex_session.clone();
    let response = match AgentManagementProvider::open_default() {
        Ok(provider) => {
            let publisher = std::sync::Arc::new(ManagementPhasePublisher);
            let provider = provider.with_phase_observer(publisher.clone());
            let response = provider.execute(
                &invocation,
                &request,
                &CutexAgentLifecycle {
                    state: std::sync::Arc::clone(state),
                },
            );
            // Exact replay and process restart may not invoke a fresh phase
            // callback. Re-drain the durable per-action journal; the
            // Management v2 append is idempotent by phase-event identity.
            if let Ok(snapshot) = provider.store().snapshot() {
                for phase in snapshot
                    .phase_events
                    .values()
                    .filter(|phase| phase.action_id == request.action_id)
                {
                    publisher.phase_committed(phase);
                }
            }
            response
        }
        Err(error) => AgentManagementResponse {
            schema: AgentManagementSchema::V1,
            action_id,
            outcome: AgentManagementOutcome::NoWrite {
                code: "persistence_unavailable".to_string(),
                detail: format!("Agent Management provider is unavailable: {error:#}"),
            },
        },
    };
    if let Err(error) = append_agent_management_outcome(&caller_cutex_session, &response) {
        eprintln!(
            "warning: failed to project Agent Management outcome into Management v2: {error:#}"
        );
    }
    serde_json::to_value(response).expect("Agent Management response is serializable")
}

struct ManagementPhasePublisher;

impl AgentManagementPhaseObserver for ManagementPhasePublisher {
    fn phase_committed(&self, phase: &AgentManagementPhaseEvent) {
        match append_agent_management_phase(phase) {
            Ok(event) => {
                if director_rotation_phase_needs_immediate_projection(phase) {
                    if let Err(error) =
                        cutex::app_server::activity_bridge::project_activity_event_immediately(
                            super::app_server_runtime::runtime_manager(),
                            &event,
                        )
                    {
                        eprintln!(
                            "warning: Director rotation {:?} live presentation remains pending: {error:#}",
                            phase.phase
                        );
                    }
                }
            }
            Err(error) => eprintln!(
                "warning: failed to publish Agent Management phase into Management v2: {error:#}"
            ),
        }
    }
}

fn director_rotation_phase_needs_immediate_projection(phase: &AgentManagementPhaseEvent) -> bool {
    phase.operation == AgentOperationKind::DirectorRotate
        && matches!(
            phase.phase,
            AgentActionPhase::PredecessorClosing
                | AgentActionPhase::AuthorityTransferred
                | AgentActionPhase::SuccessorReady
                | AgentActionPhase::Complete
        )
}

pub(crate) fn bind_project_authority(request: ProjectAuthorityRequest) -> serde_json::Value {
    let action_id = request.action_id.clone();
    let outcome = match AgentManagementProvider::open_default() {
        Ok(provider) => match provider.bind_project_authority(&request) {
            Ok(receipt) => ProjectAuthorityOutcome::Complete { receipt },
            Err(error) => ProjectAuthorityOutcome::NoWrite {
                code: error.code().to_string(),
                detail: error.to_string(),
            },
        },
        Err(error) => ProjectAuthorityOutcome::NoWrite {
            code: "persistence_unavailable".to_string(),
            detail: format!("Agent Management provider is unavailable: {error:#}"),
        },
    };
    serde_json::to_value(ProjectAuthorityResponse {
        schema: AgentManagementSchema::V1,
        action_id,
        outcome,
    })
    .expect("project authority response is serializable")
}

pub(crate) fn import_legacy_director_ownership(
    request: LegacyDirectorOwnershipImportRequest,
) -> serde_json::Value {
    let action_id = request.action_id.clone();
    let outcome = match AgentManagementProvider::open_default() {
        Ok(provider) => match provider.import_legacy_director_ownership(&request, || {
            load_legacy_director_ownership_evidence(&request)
        }) {
            Ok((receipt, replayed)) => {
                LegacyDirectorOwnershipImportOutcome::Complete { receipt, replayed }
            }
            Err(error) => LegacyDirectorOwnershipImportOutcome::NoWrite {
                code: error.code().to_string(),
                detail: error.to_string(),
            },
        },
        Err(error) => LegacyDirectorOwnershipImportOutcome::NoWrite {
            code: "persistence_unavailable".to_string(),
            detail: format!("Agent Management provider is unavailable: {error:#}"),
        },
    };
    serde_json::to_value(LegacyDirectorOwnershipImportResponse {
        schema: request.schema,
        action_id,
        outcome,
    })
    .expect("legacy Director ownership import response is serializable")
}

fn load_legacy_director_ownership_evidence(
    request: &LegacyDirectorOwnershipImportRequest,
) -> Result<LegacyDirectorOwnershipEvidence, cutex::agent_management::AgentManagementError> {
    let store = load_cutex_session_store()
        .map_err(|_| cutex::agent_management::AgentManagementError::PersistenceUnavailable)?;
    let record = store
        .sessions
        .get(request.director_cutex_session_id.as_str())
        .ok_or(cutex::agent_management::AgentManagementError::NotFound(
            "durable_director_session_not_found",
        ))?;
    legacy_director_ownership_evidence_from_record(&request.director_cutex_session_id, record)
}

fn legacy_director_ownership_evidence_from_record(
    director_cutex_session_id: &CutexSessionId,
    record: &CutexSessionRecord,
) -> Result<LegacyDirectorOwnershipEvidence, cutex::agent_management::AgentManagementError> {
    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::agent_management::AgentManagementError;
    use cutex::session::model::CutexSessionQuickActionMode;

    if record.cutex_session_id != director_cutex_session_id.as_str() {
        return Err(AgentManagementError::Conflict(
            "durable_session_identity_mismatch",
        ));
    }
    if !record.is_active() || record.retired_at.is_some() {
        return Err(AgentManagementError::Conflict(
            "durable_director_session_retired",
        ));
    }
    if !cutex::runtime::lifecycle::cutex_session_host_is_local(
        &record.host_id,
        &cutex::platform::host::current_host_name(),
    ) {
        return Err(AgentManagementError::Conflict(
            "durable_director_session_not_local",
        ));
    }
    if !record.agent_enabled || record.registration_class != AgentRegistrationClass::Persistent {
        return Err(AgentManagementError::Conflict(
            "durable_director_session_not_managed",
        ));
    }
    if record.app_server_launch_claim_id.is_some() {
        return Err(AgentManagementError::OwnerActionRequired(
            "durable Director has an unresolved app-server launch claim".to_string(),
        ));
    }
    let native_session_id = required_record_field(
        record.codex_session_id.as_deref(),
        "durable_director_native_session_missing",
    )?;
    cutex::session::identity::normalize_codex_session_id(native_session_id)
        .map_err(|_| AgentManagementError::Conflict("invalid_durable_native_session"))?;
    let name = required_record_field(
        record.thread_name.as_deref(),
        "durable_director_name_missing",
    )?;
    if record.display_name_hint.as_deref() != Some(name) {
        return Err(AgentManagementError::Conflict(
            "durable_director_name_ambiguous",
        ));
    }
    let cwd = required_record_field(
        record.managed_cwd.as_deref(),
        "durable_director_managed_cwd_missing",
    )?;
    if cutex_session_launch_cwd(record) != cwd {
        return Err(AgentManagementError::Conflict(
            "durable_director_cwd_ambiguous",
        ));
    }
    let groups = cutex::agent_bus::identity::normalize_agent_groups(record.agent_groups.clone());
    if groups.is_empty() || groups != record.agent_groups {
        return Err(AgentManagementError::Conflict(
            "durable_director_groups_not_canonical",
        ));
    }
    let pin = match record.quick_action {
        CutexSessionQuickActionMode::Auto => false,
        CutexSessionQuickActionMode::Pinned => true,
        CutexSessionQuickActionMode::Hidden => {
            return Err(AgentManagementError::Conflict(
                "durable_director_quick_action_not_representable",
            ))
        }
    };
    let runtime_backend = serde_json::to_value(record.runtime_backend)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or(AgentManagementError::Conflict(
            "durable_director_runtime_backend_invalid",
        ))?;
    let spec = ManagedAgentSpec {
        name: name.to_string(),
        cwd: cwd.to_string(),
        profile: required_record_field(
            record.profile.as_deref(),
            "durable_director_profile_missing",
        )?
        .to_string(),
        runtime_backend,
        model: required_record_field(
            record.model_defaults.as_deref(),
            "durable_director_model_missing",
        )?
        .to_string(),
        reasoning: required_record_field(
            record.reasoning_defaults.as_deref(),
            "durable_director_reasoning_missing",
        )?
        .to_string(),
        permissions: required_record_field(
            record.permission_defaults.as_deref(),
            "durable_director_permissions_missing",
        )?
        .to_string(),
        approval_policy: required_record_field(
            record.approval_policy.as_deref(),
            "durable_director_approval_policy_missing",
        )?
        .to_string(),
        sandbox_mode: required_record_field(
            record.sandbox_mode.as_deref(),
            "durable_director_sandbox_mode_missing",
        )?
        .to_string(),
        groups,
        expose_to_im: record.exposed_to_backend,
        pin,
    };
    spec.validate()?;
    if record.revision == 0
        || record.revision > cutex::role_revision::MAX_JSON_SAFE_INTEGER
        || record.runtime_generation > cutex::role_revision::MAX_JSON_SAFE_INTEGER
    {
        return Err(AgentManagementError::Conflict(
            "invalid_durable_lifecycle_evidence",
        ));
    }
    Ok(LegacyDirectorOwnershipEvidence {
        director_cutex_session_id: director_cutex_session_id.clone(),
        native_session_id: native_session_id.to_string(),
        durable_session_revision: record.revision,
        runtime_generation: record.runtime_generation,
        spec,
    })
}

fn required_record_field<'a>(
    value: Option<&'a str>,
    reason: &'static str,
) -> Result<&'a str, cutex::agent_management::AgentManagementError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(cutex::agent_management::AgentManagementError::Conflict(
            reason,
        ))
}

struct CutexAgentLifecycle {
    state: std::sync::Arc<std::sync::Mutex<cutex::agent_bus::store::AgentBusState>>,
}

impl CutexAgentLifecycle {
    fn historical_runtime_occurrence(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<HistoricalRuntimeOccurrenceReconciliation, LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        self.historical_runtime_occurrence_for_record(&record)
    }

    fn historical_runtime_occurrence_for_record(
        &self,
        record: &CutexSessionRecord,
    ) -> Result<HistoricalRuntimeOccurrenceReconciliation, LifecycleFailure> {
        let bus_ids = if let Some(entry) = coding_registration_from_cutex_session_record(record) {
            let config = cutex::config::store::load_codez_config();
            management_lifecycle::try_live_agents_for_management_entry(&config, &entry)
                .map_err(unknown("runtime_occurrence_bus_unavailable"))?
                .into_iter()
                .map(|agent| agent.id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let manager_status = super::app_server_runtime::runtime_manager()
            .status(&record.cutex_session_id)
            .map_err(unknown("runtime_occurrence_app_server_unavailable"))?;

        let alden_name = record.alden_session_name.clone().or_else(|| {
            (record.runtime_backend == CutexSessionRuntimeBackend::CuteAlden)
                .then(|| default_cutex_alden_session_name(record))
        });
        let alden_session = if let Some(name) = alden_name.as_deref() {
            cute_alden_sessions()
                .map_err(unknown("runtime_occurrence_process_unavailable"))?
                .into_iter()
                .find(|session| session.name.as_deref() == Some(name))
        } else {
            None
        };

        let app_binding = record.app_server_runtime.as_ref();
        let fence = RuntimeOccurrenceFence {
            runtime_generation: record.runtime_generation,
            current_runtime_agent_id: record.current_runtime_agent_id.clone(),
            agent_bus_endpoint_ids: bus_ids,
            pending_launch_id: record.pending_launch_id.clone(),
            app_server_launch_claim_id: record.app_server_launch_claim_id.clone(),
            alden_session_name: alden_name,
            alden_pid: record
                .alden_pid
                .or_else(|| alden_session.as_ref().map(|session| session.pid)),
            runtime_pid: record.runtime_pid,
            app_server_pid: app_binding.map(|binding| binding.pid),
            app_server_endpoint: app_binding.map(|binding| binding.endpoint.clone()),
            app_server_connected: manager_status
                .as_ref()
                .is_some_and(|status| status.connected),
        };
        if fence.is_proven_absent() && manager_status.is_none() && alden_session.is_none() {
            return Ok(
                HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent {
                    fence,
                    reason: "durable session, Agent Bus, app-server manager, process registry, and launch claims prove the runtime occurrence absent".to_string(),
                },
            );
        }
        if manager_status
            .as_ref()
            .is_some_and(|status| !status.connected)
            && fence.current_runtime_agent_id.is_none()
            && fence.agent_bus_endpoint_ids.is_empty()
            && fence.pending_launch_id.is_none()
            && fence.app_server_launch_claim_id.is_none()
            && fence.alden_pid.is_none()
            && fence.runtime_pid.is_none()
            && fence.app_server_pid.is_none()
        {
            return Ok(HistoricalRuntimeOccurrenceReconciliation::Ambiguous {
                reason: "app-server manager retains a disconnected occurrence without a durable ownership claim".to_string(),
            });
        }
        Ok(HistoricalRuntimeOccurrenceReconciliation::Present {
            reason: format!(
                "runtime occurrence generation {} retains an endpoint, process, app-server binding, or launch claim",
                fence.runtime_generation
            ),
        })
    }
}

impl AgentLifecycle for CutexAgentLifecycle {
    fn prepare_private_cwd(&self, spec: &ManagedAgentSpec) -> Result<(), LifecycleFailure> {
        prepare_private_managed_cwd(Path::new(&spec.cwd)).map_err(definite("private_cwd_failed"))
    }

    fn bootstrap_native(&self, spec: &ManagedAgentSpec) -> Result<String, LifecycleFailure> {
        // Validate the selected profile and its materialized credential/config
        // files in this process. A failure here is provably before Command::output
        // can spawn the native runtime and is therefore safe for exact retry.
        super::launch::resolve_launch_profile_override(&spec.profile)
            .map_err(definite("native_bootstrap_preflight_failed"))?;
        let plan = native_bootstrap_plan(spec).map_err(definite("bootstrap_plan_failed"))?;
        let output = plan
            .command()
            .output()
            .map_err(unknown("native_bootstrap_spawn_failed"))?;
        captured_native_session_id(&output)
    }

    fn reconcile_pre_sid_bootstrap(
        &self,
        spec: &ManagedAgentSpec,
        started_at: &Rfc3339,
        failed_at: &Rfc3339,
    ) -> Result<NativeBootstrapReconciliation, LifecycleFailure> {
        let resolved = super::launch::resolve_launch_profile_override(&spec.profile)
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        let effective_profile = resolved.effective_name();
        let expected_agent_name = cutex::agent_bus::identity::account_agent_name(&resolved.account);
        let expected_alden_name =
            cutex::runtime::managed_launch::default_managed_session_name_for_cwd(
                &resolved.account,
                Path::new(&spec.cwd),
            );
        // The cwd is a correlation marker only. Exact identity additionally
        // requires the launch profile/name/groups; no authority is inferred.
        let state = self.state.lock().map_err(|_| {
            LifecycleFailure::outcome_unknown(
                "native_bootstrap_reconciliation_unavailable",
                "Agent Bus state lock is poisoned",
            )
        })?;
        let matching_agents = state
            .agents
            .values()
            .filter(|agent| agent.cwd == spec.cwd)
            .collect::<Vec<_>>();
        if !matching_agents.is_empty() {
            let exact = matching_agents.len() == 1
                && matching_agents[0].profile == effective_profile
                && (matching_agents[0].name == expected_agent_name
                    || matching_agents[0].base_name.as_deref()
                        == Some(expected_agent_name.as_str()))
                && spec
                    .groups
                    .iter()
                    .all(|group| matching_agents[0].groups.contains(group));
            return Ok(if exact {
                NativeBootstrapReconciliation::Present {
                    reason: "exact Agent Bus profile/name/cwd/groups identity is registered"
                        .to_string(),
                }
            } else {
                NativeBootstrapReconciliation::Ambiguous {
                    reason: "Agent Bus runtime occupies the reserved cwd with mismatched identity"
                        .to_string(),
                }
            });
        }
        drop(state);

        let durable = load_cutex_session_store()
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        let matching_records = durable
            .sessions
            .values()
            .filter(|record| {
                record.cwd == spec.cwd || record.managed_cwd.as_deref() == Some(spec.cwd.as_str())
            })
            .collect::<Vec<_>>();
        if !matching_records.is_empty() {
            let exact = matching_records.len() == 1
                && matching_records[0].profile.as_deref() == Some(effective_profile)
                && (matching_records[0].thread_name.as_deref() == Some(spec.name.as_str())
                    || matching_records[0].display_name_hint.as_deref()
                        == Some(spec.name.as_str()))
                && spec
                    .groups
                    .iter()
                    .all(|group| matching_records[0].agent_groups.contains(group));
            return Ok(if exact {
                NativeBootstrapReconciliation::Present {
                    reason: "exact durable session profile/name/cwd/groups identity exists"
                        .to_string(),
                }
            } else {
                NativeBootstrapReconciliation::Ambiguous {
                    reason: "durable session occupies the reserved cwd with mismatched identity"
                        .to_string(),
                }
            });
        }

        let alden_sessions = cutex::runtime::alden::cute_alden_sessions()
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        if let Some(session) = alden_sessions
            .iter()
            .find(|session| session.name.as_deref() == Some(expected_alden_name.as_str()))
        {
            return Ok(
                if cutex::platform::process::process_is_running(session.pid) {
                    NativeBootstrapReconciliation::Present {
                        reason: "exact deterministic cute-alden session name has a live PID"
                            .to_string(),
                    }
                } else {
                    NativeBootstrapReconciliation::Ambiguous {
                        reason: "exact deterministic cute-alden session name has a stale PID"
                            .to_string(),
                    }
                },
            );
        }

        let codex_home = selected_profile_codex_home(&resolved)
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        match cutex::runtime::codex_home::correlate_codex_session_between_in_home(
            &codex_home,
            started_at,
            failed_at,
            Path::new(&spec.cwd),
        )
        .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?
        {
            cutex::runtime::codex_home::NativeSessionCorrelation::Present { .. } => {
                Ok(NativeBootstrapReconciliation::Present {
                    reason: "native rollout/index evidence matches the exact managed cwd"
                        .to_string(),
                })
            }
            cutex::runtime::codex_home::NativeSessionCorrelation::Ambiguous { reason } => {
                Ok(NativeBootstrapReconciliation::Ambiguous { reason })
            }
            cutex::runtime::codex_home::NativeSessionCorrelation::ProvenAbsent => {
                Ok(NativeBootstrapReconciliation::ProvenAbsent {
                    reason: "Agent Bus, durable session, cute-alden, and correlated native sources are absent"
                        .to_string(),
                })
            }
        }
    }

    fn reconcile_ambiguous_native_bootstrap(
        &self,
        spec: &ManagedAgentSpec,
        started_at: &Rfc3339,
        failed_at: &Rfc3339,
    ) -> Result<NativeBootstrapIdentityReconciliation, LifecycleFailure> {
        let resolved = super::launch::resolve_launch_profile_override(&spec.profile)
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        let effective_profile = resolved.effective_name();
        let expected_agent_name = cutex::agent_bus::identity::account_agent_name(&resolved.account);
        let expected_alden_name =
            cutex::runtime::managed_launch::default_managed_session_name_for_cwd(
                &resolved.account,
                Path::new(&spec.cwd),
            );
        let cwd_hash = cutex::agent_bus::identity::fnv1a_hex(&spec.cwd);
        let expected_runtime_groups = cutex::agent_bus::groups::normalize_registered_agent_groups(
            spec.groups.clone(),
            Some(&cwd_hash[..7]),
            &spec.cwd,
        );
        let mut candidates = BTreeSet::new();
        let mut identity_present_without_sid = false;
        let mut attempt_window_native_sid = None;

        let state = self.state.lock().map_err(|_| {
            LifecycleFailure::outcome_unknown(
                "native_bootstrap_reconciliation_unavailable",
                "Agent Bus state lock is poisoned",
            )
        })?;
        let matching_agents = state
            .agents
            .values()
            .filter(|agent| agent.cwd == spec.cwd)
            .collect::<Vec<_>>();
        if !matching_agents.is_empty() {
            let exact = matching_agents.len() == 1
                && matching_agents[0].profile == effective_profile
                && (matching_agents[0].name == expected_agent_name
                    || matching_agents[0].base_name.as_deref()
                        == Some(expected_agent_name.as_str()))
                && (matching_agents[0].groups == spec.groups
                    || matching_agents[0].groups == expected_runtime_groups)
                && cutex::platform::process::process_is_running(matching_agents[0].pid);
            if !exact {
                return Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                    reason: "Agent Bus runtime occupies the reserved cwd with mismatched identity"
                        .to_string(),
                });
            }
            match validated_reconciled_sid(
                matching_agents[0].session_id.as_deref(),
                "exact Agent Bus runtime",
            ) {
                Ok(session_id) => {
                    candidates.insert(session_id);
                    identity_present_without_sid = true;
                }
                Err(reason) => {
                    return Ok(NativeBootstrapIdentityReconciliation::Ambiguous { reason });
                }
            }
        }
        drop(state);

        let durable = load_cutex_session_store()
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        let matching_records = durable
            .sessions
            .values()
            .filter(|record| {
                record.cwd == spec.cwd || record.managed_cwd.as_deref() == Some(spec.cwd.as_str())
            })
            .collect::<Vec<_>>();
        if !matching_records.is_empty() {
            let exact = matching_records.len() == 1
                && matching_records[0].is_active()
                && matching_records[0].retired_at.is_none()
                && cutex::runtime::lifecycle::cutex_session_host_is_local(
                    &matching_records[0].host_id,
                    &cutex::platform::host::current_host_name(),
                )
                && matching_records[0].profile.as_deref() == Some(effective_profile)
                && (matching_records[0].thread_name.as_deref() == Some(spec.name.as_str())
                    || matching_records[0].display_name_hint.as_deref()
                        == Some(spec.name.as_str()))
                && (matching_records[0].agent_groups == spec.groups
                    || matching_records[0].agent_groups == expected_runtime_groups);
            if !exact {
                return Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                    reason: "durable session occupies the reserved cwd with mismatched identity"
                        .to_string(),
                });
            }
            match validated_reconciled_sid(
                matching_records[0].codex_session_id.as_deref(),
                "exact durable session",
            ) {
                Ok(session_id) => {
                    candidates.insert(session_id);
                    identity_present_without_sid = true;
                }
                Err(reason) => {
                    return Ok(NativeBootstrapIdentityReconciliation::Ambiguous { reason });
                }
            }
        }

        let alden_sessions = cutex::runtime::alden::cute_alden_sessions()
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        let matching_alden = alden_sessions
            .iter()
            .filter(|session| session.name.as_deref() == Some(expected_alden_name.as_str()))
            .collect::<Vec<_>>();
        if matching_alden.len() > 1 {
            return Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                reason: "multiple cute-alden sessions have the deterministic managed name"
                    .to_string(),
            });
        }
        if let Some(session) = matching_alden.first() {
            if !cutex::platform::process::process_is_running(session.pid) {
                return Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                    reason: "deterministic cute-alden session has a stale PID".to_string(),
                });
            }
            identity_present_without_sid = true;
        }

        let codex_home = selected_profile_codex_home(&resolved)
            .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?;
        match cutex::runtime::codex_home::correlate_codex_session_between_in_home(
            &codex_home,
            started_at,
            failed_at,
            Path::new(&spec.cwd),
        )
        .map_err(unknown("native_bootstrap_reconciliation_unavailable"))?
        {
            cutex::runtime::codex_home::NativeSessionCorrelation::Present { session_id } => {
                match validated_reconciled_sid(
                    Some(&session_id),
                    "selected-profile native rollout/index evidence",
                ) {
                    Ok(session_id) => {
                        candidates.insert(session_id.clone());
                        attempt_window_native_sid = Some(session_id);
                    }
                    Err(reason) => {
                        return Ok(NativeBootstrapIdentityReconciliation::Ambiguous { reason });
                    }
                }
            }
            cutex::runtime::codex_home::NativeSessionCorrelation::Ambiguous { reason } => {
                return Ok(NativeBootstrapIdentityReconciliation::Ambiguous { reason });
            }
            cutex::runtime::codex_home::NativeSessionCorrelation::ProvenAbsent => {}
        }

        let candidates = candidates.into_iter().collect::<Vec<_>>();
        match (attempt_window_native_sid.as_deref(), candidates.as_slice()) {
            (Some(window_sid), [native_session_id]) if window_sid == native_session_id => {
                Ok(NativeBootstrapIdentityReconciliation::Exact {
                native_session_id: native_session_id.clone(),
                reason:
                    "one native SID matches the selected profile/runtime and exact managed identity"
                        .to_string(),
                })
            }
            (None, []) if identity_present_without_sid => {
                Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                    reason:
                        "exact runtime identity is present but no unique native SID is available"
                            .to_string(),
                })
            }
            (None, []) => Ok(NativeBootstrapIdentityReconciliation::Absent {
                reason:
                    "selected-profile native, Agent Bus, durable, and cute-alden evidence is absent"
                        .to_string(),
            }),
            (None, _) => Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                reason: "identity sources expose a native SID but no exact attempt-window rollout/index record"
                    .to_string(),
            }),
            (Some(_), candidates) => Ok(NativeBootstrapIdentityReconciliation::Ambiguous {
                reason: format!(
                    "provider-owned sources expose conflicting native SIDs: {}",
                    candidates.join(", ")
                ),
            }),
        }
    }

    fn reconcile_historical_runtime_occurrence(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<HistoricalRuntimeOccurrenceReconciliation, LifecycleFailure> {
        self.historical_runtime_occurrence(cutex_session_id)
    }

    fn adopt_native(
        &self,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<CutexSessionId, LifecycleFailure> {
        let mut store =
            load_cutex_session_store().map_err(definite("session_store_unavailable"))?;
        let outcome = adopt_cutex_session(
            &mut store,
            native_session_id,
            CutexSessionEnsureSeed {
                host_id: cutex::platform::host::current_host_name(),
                cwd: spec.cwd.clone(),
                profile: Some(spec.profile.clone()),
            },
            CutexSessionAdoptOptions {
                display_name: Some(&spec.name),
                managed_cwd: Some(spec.cwd.clone()),
                groups: spec.groups.clone(),
                expose_to_im: spec.expose_to_im,
                pin: spec.pin,
            },
        )
        .map_err(definite("session_adopt_failed"))?;
        persist_cutex_session_store_and_im_record(&store, &outcome.key)
            .map_err(unknown("session_adopt_persistence_unknown"))?;
        CutexSessionId::new(outcome.key)
            .map_err(|_| LifecycleFailure::definite("invalid_durable_session", "adopted ID"))
    }

    fn configure(
        &self,
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<(), LifecycleFailure> {
        let mut store =
            load_cutex_session_store().map_err(definite("session_store_unavailable"))?;
        let key =
            cutex_session_key_for_user_id_including_retired(&store, cutex_session_id.as_str())
                .ok_or_else(|| {
                    LifecycleFailure::definite("session_not_found", "adopted Agent missing")
                })?;
        let record = store.sessions.get_mut(&key).ok_or_else(|| {
            LifecycleFailure::definite("session_not_found", "adopted Agent disappeared")
        })?;
        if record.is_retired() || record.codex_session_id.as_deref() != Some(native_session_id) {
            return Err(LifecycleFailure::definite(
                "session_identity_conflict",
                "adopted Agent does not bind the captured native session",
            ));
        }
        record.thread_name = Some(spec.name.clone());
        record.display_name_hint = Some(spec.name.clone());
        record.managed_cwd = Some(spec.cwd.clone());
        record.profile = Some(spec.profile.clone());
        record.runtime_backend = parse_cutex_session_runtime_backend(&spec.runtime_backend)
            .map_err(definite("invalid_runtime_backend"))?;
        record.agent_enabled = true;
        record.agent_groups = spec.groups.clone();
        record.registration_class = AgentRegistrationClass::Persistent;
        record.exposed_to_backend = spec.expose_to_im;
        record.quick_action = if spec.pin {
            CutexSessionQuickActionMode::Pinned
        } else {
            CutexSessionQuickActionMode::Auto
        };
        record.permission_defaults = Some(spec.permissions.clone());
        record.approval_policy = Some(spec.approval_policy.clone());
        record.sandbox_mode = Some(spec.sandbox_mode.clone());
        record.model_defaults = Some(spec.model.clone());
        record.reasoning_defaults = Some(spec.reasoning.clone());
        record
            .bump_durable_revision()
            .map_err(definite("session_revision_failed"))?;
        record.updated_at = chrono::Utc::now().to_rfc3339();
        persist_cutex_session_store_and_im_record(&store, &key)
            .map_err(unknown("session_configuration_persistence_unknown"))
    }

    fn recover_runtime(
        &self,
        cutex_session_id: &CutexSessionId,
        native_session_id: &str,
        spec: &ManagedAgentSpec,
    ) -> Result<RuntimeRecoveryOutcome, LifecycleFailure> {
        let before = load_record(cutex_session_id)?;
        validate_managed_recovery_record(&before, cutex_session_id, native_session_id, spec)?;
        let generation = before.runtime_generation;
        let config = cutex::config::store::load_codez_config();
        let outcome =
            super::app_server_runtime::recover_persisted_runtime_for_lifecycle(&config, &before)
                .map_err(unknown("runtime_recovery_failed"))?;
        let after = load_record(cutex_session_id)?;
        validate_managed_recovery_record(&after, cutex_session_id, native_session_id, spec)?;
        if after.runtime_generation != generation {
            return Err(LifecycleFailure::outcome_unknown(
                "runtime_recovery_generation_changed",
                "runtime recovery changed the claimed generation",
            ));
        }
        match outcome {
            super::app_server_runtime::ManagedRuntimeRecoveryOutcome::NoClaim => {
                Ok(RuntimeRecoveryOutcome::NoClaim)
            }
            super::app_server_runtime::ManagedRuntimeRecoveryOutcome::RecoveredExact => {
                if after.app_server_runtime != before.app_server_runtime
                    || after.current_runtime_agent_id != before.current_runtime_agent_id
                    || after.runtime_pid != before.runtime_pid
                {
                    return Err(LifecycleFailure::outcome_unknown(
                        "runtime_recovery_claim_changed",
                        "exact runtime recovery changed durable ownership",
                    ));
                }
                Ok(RuntimeRecoveryOutcome::RecoveredExact)
            }
            super::app_server_runtime::ManagedRuntimeRecoveryOutcome::ClearedDeadClaim => {
                if after.app_server_runtime.is_some()
                    || after.current_runtime_agent_id.is_some()
                    || after.runtime_pid.is_some()
                    || after.alden_pid.is_some()
                    || after.app_server_launch_claim_id.is_some()
                {
                    return Err(LifecycleFailure::outcome_unknown(
                        "runtime_recovery_cleanup_incomplete",
                        "dead runtime ownership claim was not fully fenced",
                    ));
                }
                Ok(RuntimeRecoveryOutcome::ClearedDeadClaim)
            }
        }
    }

    fn online(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        let entry = coding_registration_from_cutex_session_record(&record).ok_or_else(|| {
            LifecycleFailure::definite(
                "native_session_missing",
                "managed Agent has no bound native session",
            )
        })?;
        if record.app_server_runtime.is_some()
            && record.current_runtime_agent_id.is_some()
            && super::app_server_runtime::runtime_manager()
                .status(cutex_session_id.as_str())
                .ok()
                .flatten()
                .is_some_and(|status| status.connected)
        {
            return Ok(());
        }
        let config = cutex::config::store::load_codez_config();
        management_lifecycle::start_cutex_session_online_with_profile(&config, &entry, None)
            .map(|_| ())
            .map_err(unknown("session_online_failed"))
    }

    fn offline(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        let Some(entry) = coding_registration_from_cutex_session_record(&record) else {
            if record.app_server_runtime.is_none()
                && record.runtime_pid.is_none()
                && record.current_runtime_agent_id.is_none()
            {
                return Ok(());
            }
            return Err(LifecycleFailure::definite(
                "runtime_identity_unknown",
                "managed Agent cannot project an exact runtime identity",
            ));
        };
        let config = cutex::config::store::load_codez_config();
        let live = management_lifecycle::try_live_agents_for_management_entry(&config, &entry)
            .map_err(unknown("agent_bus_observation_failed"))?;
        let result =
            management_lifecycle::stop_cutex_session_runtime_for_entry(&entry, &live, false)
                .map_err(unknown("session_offline_failed"))?;
        if result.stopped {
            Ok(())
        } else {
            Err(LifecycleFailure::outcome_unknown(
                "session_offline_failed",
                result.detail,
            ))
        }
    }

    fn offline_if_occurrence(
        &self,
        cutex_session_id: &CutexSessionId,
        expected: &RuntimeOccurrenceFence,
    ) -> Result<(), LifecycleFailure> {
        match self.historical_runtime_occurrence(cutex_session_id)? {
            HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent { fence, .. }
                if &fence == expected =>
            {
                self.offline(cutex_session_id)
            }
            HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent { .. } => {
                Err(LifecycleFailure::outcome_unknown(
                    "runtime_occurrence_changed",
                    "runtime absence fence changed before the offline effect",
                ))
            }
            HistoricalRuntimeOccurrenceReconciliation::Present { reason }
            | HistoricalRuntimeOccurrenceReconciliation::Ambiguous { reason }
            | HistoricalRuntimeOccurrenceReconciliation::Unavailable { reason } => Err(
                LifecycleFailure::outcome_unknown("runtime_occurrence_changed", reason),
            ),
        }
    }

    fn restart_if_occurrence(
        &self,
        cutex_session_id: &CutexSessionId,
        expected: &RuntimeOccurrenceFence,
    ) -> Result<(AgentRuntimeObservation, AgentRuntimeObservation), LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        let entry = coding_registration_from_cutex_session_record(&record).ok_or_else(|| {
            LifecycleFailure::definite(
                "native_session_missing",
                "managed Agent has no bound native session",
            )
        })?;
        let before = self.observe(cutex_session_id)?;
        let validate =
            |current: &CutexSessionRecord| -> anyhow::Result<()> {
                match self.historical_runtime_occurrence_for_record(current) {
                    Ok(HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent {
                        fence, ..
                    }) if &fence == expected => Ok(()),
                    Ok(HistoricalRuntimeOccurrenceReconciliation::ProvenAbsent { .. }) => {
                        anyhow::bail!("runtime absence fence changed before the fenced restart")
                    }
                    Ok(HistoricalRuntimeOccurrenceReconciliation::Present { reason })
                    | Ok(HistoricalRuntimeOccurrenceReconciliation::Ambiguous { reason })
                    | Ok(HistoricalRuntimeOccurrenceReconciliation::Unavailable { reason }) => {
                        anyhow::bail!(reason)
                    }
                    Err(error) => anyhow::bail!(error.detail),
                }
            };
        let config = cutex::config::store::load_codez_config();
        management_lifecycle::start_cutex_session_online_with_profile_if(
            &config, &entry, None, &validate,
        )
        .map_err(unknown("fenced_restart_failed"))?;
        let after = self.observe(cutex_session_id)?;
        Ok((before, after))
    }

    fn retire(&self, cutex_session_id: &CutexSessionId) -> Result<(), LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        if record.is_retired() {
            return Ok(());
        }
        management_archive::mutate_management_v2_archive(
            cutex_session_id.as_str(),
            "cutex/session/retire",
            &serde_json::json!({
                "expectedRevision": record.revision,
                "expectedRuntimeGeneration": record.runtime_generation,
            }),
        )
        .map(|_| ())
        .map_err(|error| {
            LifecycleFailure::outcome_unknown(
                "session_retire_failed",
                format!("typed retirement failed: {error:?}"),
            )
        })
    }

    fn observe(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AgentRuntimeObservation, LifecycleFailure> {
        let record = load_record(cutex_session_id)?;
        let native_session_id = record.codex_session_id.clone().ok_or_else(|| {
            LifecycleFailure::definite(
                "native_session_missing",
                "managed Agent has no bound native session",
            )
        })?;
        let runtime_agent_ids = record
            .current_runtime_agent_id
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let bus_ids = if let Some(entry) = coding_registration_from_cutex_session_record(&record) {
            let config = cutex::config::store::load_codez_config();
            management_lifecycle::try_live_agents_for_management_entry(&config, &entry)
                .map_err(unknown("agent_bus_observation_failed"))?
                .into_iter()
                .map(|agent| agent.id)
                .collect()
        } else {
            Vec::new()
        };
        let app_server_runtime = record.app_server_runtime.is_some()
            && super::app_server_runtime::runtime_manager()
                .status(cutex_session_id.as_str())
                .map_err(unknown("app_server_observation_failed"))?
                .is_some_and(|status| status.connected);
        Ok(AgentRuntimeObservation {
            cutex_session_id: cutex_session_id.clone(),
            native_session_id,
            active: record.is_active(),
            cwd: cutex_session_launch_cwd(&record).to_string(),
            profile: record.profile.unwrap_or_default(),
            runtime_backend: serde_json::to_value(record.runtime_backend)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            model: record.model_defaults.unwrap_or_default(),
            reasoning: record.reasoning_defaults.unwrap_or_default(),
            permissions: record.permission_defaults.unwrap_or_default(),
            approval_policy: record.approval_policy.unwrap_or_default(),
            sandbox_mode: record.sandbox_mode.unwrap_or_default(),
            groups: record.agent_groups,
            runtime_generation: record.runtime_generation,
            runtime_agent_ids,
            app_server_runtime,
            agent_bus_endpoint_ids: bus_ids,
        })
    }

    fn send_message(
        &self,
        system: &cutex::agent_bus::identity::AgentManagementSystemPrincipal,
        metadata: &AgentManagementMessageMetadata,
        to_agent: &CutexSessionId,
        exact_message: &str,
        external_message_id: &str,
    ) -> Result<String, LifecycleFailure> {
        let target = load_record(to_agent)?;
        let target_runtime = target.current_runtime_agent_id.clone().ok_or_else(|| {
            LifecycleFailure::definite("target_offline", "target Agent has no runtime endpoint")
        })?;
        let source = load_record(&metadata.requested_by_director)?;
        let (from_agent_id, from_session_id) = agent_message_source_binding(
            &metadata.requested_by_director,
            source.current_runtime_agent_id,
        );
        let response = super::agent_bus_server::send_agent_management_system_message_response(
            &self.state,
            system,
            super::agent_bus_server::AgentManagementSystemMessage {
                metadata,
                from_agent_id,
                from_session_id,
                target_runtime_agent_id: &target_runtime,
                target_cutex_session_id: to_agent,
                exact_message,
                external_message_id,
            },
        )
        .map_err(unknown("agent_message_outcome_unknown"))?;
        let response: AgentBusSendResponse =
            serde_json::from_value(response).map_err(definite("invalid_agent_message_receipt"))?;
        if response.to_runtime_agent_id.as_deref() != Some(target_runtime.as_str())
            || response.external_message_id.as_deref() != Some(external_message_id)
        {
            return Err(LifecycleFailure::outcome_unknown(
                "agent_message_receipt_conflict",
                "Agent Bus receipt did not preserve the exact target and external message ID",
            ));
        }
        Ok(response.id)
    }
}

fn agent_message_source_binding(
    from_director: &CutexSessionId,
    current_runtime_agent_id: Option<String>,
) -> (Option<String>, Option<String>) {
    let from_session_id = current_runtime_agent_id
        .as_ref()
        .map(|_| from_director.as_str().to_string());
    (current_runtime_agent_id, from_session_id)
}

fn load_record(
    cutex_session_id: &CutexSessionId,
) -> Result<cutex::session::model::CutexSessionRecord, LifecycleFailure> {
    let store = load_cutex_session_store().map_err(definite("session_store_unavailable"))?;
    let key = cutex_session_key_for_user_id_including_retired(&store, cutex_session_id.as_str())
        .ok_or_else(|| LifecycleFailure::definite("session_not_found", "managed Agent missing"))?;
    store
        .sessions
        .get(&key)
        .cloned()
        .ok_or_else(|| LifecycleFailure::definite("session_not_found", "managed Agent disappeared"))
}

fn validate_managed_recovery_record(
    record: &CutexSessionRecord,
    cutex_session_id: &CutexSessionId,
    native_session_id: &str,
    spec: &ManagedAgentSpec,
) -> Result<(), LifecycleFailure> {
    let backend = parse_cutex_session_runtime_backend(&spec.runtime_backend)
        .map_err(definite("invalid_runtime_backend"))?;
    let cwd_hash = cutex::agent_bus::identity::fnv1a_hex(&spec.cwd);
    let effective_groups = cutex::agent_bus::groups::normalize_registered_agent_groups(
        spec.groups.clone(),
        Some(&cwd_hash[..7]),
        &spec.cwd,
    );
    let groups_match =
        record.agent_groups == spec.groups || record.agent_groups == effective_groups;
    let quick_action = if spec.pin {
        CutexSessionQuickActionMode::Pinned
    } else {
        CutexSessionQuickActionMode::Auto
    };
    let exact = record.is_active()
        && record.cutex_session_id == cutex_session_id.as_str()
        && cutex::runtime::lifecycle::cutex_session_host_is_local(
            &record.host_id,
            &cutex::platform::host::current_host_name(),
        )
        && record.codex_session_id.as_deref() == Some(native_session_id)
        && record.managed_cwd.as_deref() == Some(spec.cwd.as_str())
        && cutex_session_launch_cwd(record) == spec.cwd
        && record.profile.as_deref() == Some(spec.profile.as_str())
        && record.runtime_backend == backend
        && record.thread_name.as_deref() == Some(spec.name.as_str())
        && record.display_name_hint.as_deref() == Some(spec.name.as_str())
        && record.agent_enabled
        && groups_match
        && record.registration_class == AgentRegistrationClass::Persistent
        && record.exposed_to_backend == spec.expose_to_im
        && record.quick_action == quick_action
        && record.permission_defaults.as_deref() == Some(spec.permissions.as_str())
        && record.approval_policy.as_deref() == Some(spec.approval_policy.as_str())
        && record.sandbox_mode.as_deref() == Some(spec.sandbox_mode.as_str())
        && record.model_defaults.as_deref() == Some(spec.model.as_str())
        && record.reasoning_defaults.as_deref() == Some(spec.reasoning.as_str());
    if exact {
        Ok(())
    } else {
        Err(LifecycleFailure::definite(
            "runtime_recovery_spec_mismatch",
            "durable session/native identity or managed runtime spec does not match",
        ))
    }
}

fn prepare_private_managed_cwd(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() || path.parent().is_none() {
        anyhow::bail!("managed Agent cwd must be an absolute non-root path");
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            anyhow::bail!("managed Agent cwd must be a real directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).with_context(|| {
                format!("failed to create managed Agent cwd: {}", path.display())
            })?;
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct NativeBootstrapPlan {
    executable: PathBuf,
    cwd: PathBuf,
    args: Vec<String>,
}

fn validated_reconciled_sid(value: Option<&str>, source: &str) -> Result<String, String> {
    let value = value
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .ok_or_else(|| format!("{source} does not expose one valid native SID"))?;
    Ok(value.to_string())
}

fn selected_profile_codex_home(
    resolved: &super::launch::ResolvedLaunchProfile,
) -> anyhow::Result<PathBuf> {
    match &resolved.account.runtime {
        cutex::profiles::model::RuntimeConfig::Host => cutex::config::paths::host_codex_home_dir(),
        cutex::profiles::model::RuntimeConfig::Docker { user_name, .. } => {
            let user_name = cutex::launch::docker::docker_user_name(user_name.as_deref())?;
            Ok(
                cutex::launch::docker::DockerLaunchPaths::new(&user_name, &resolved.account.id)?
                    .host_user_home
                    .join(".codex"),
            )
        }
    }
}

impl NativeBootstrapPlan {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.current_dir(&self.cwd).args(&self.args);
        command
    }
}

fn native_bootstrap_plan(spec: &ManagedAgentSpec) -> anyhow::Result<NativeBootstrapPlan> {
    let executable = std::env::current_exe().context("failed to resolve Cutex executable")?;
    let mut args = vec![
        "run".to_string(),
        spec.profile.clone(),
        "--agent".to_string(),
    ];
    for group in &spec.groups {
        args.push("--group".to_string());
        args.push(group.clone());
    }
    args.extend([
        "--".to_string(),
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "Hi.".to_string(),
    ]);
    Ok(NativeBootstrapPlan {
        executable,
        cwd: PathBuf::from(&spec.cwd),
        args,
    })
}

fn captured_native_session_id(output: &Output) -> Result<String, LifecycleFailure> {
    let mut candidates = BTreeSet::new();
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        LifecycleFailure::outcome_unknown(
            "native_bootstrap_output_malformed",
            "native exec JSONL stdout is not valid UTF-8",
        )
    })?;
    for (line_index, line) in stdout.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(line).map_err(|_| {
            LifecycleFailure::outcome_unknown(
                "native_bootstrap_output_malformed",
                format!(
                    "native exec JSONL stdout line {} is not valid JSON",
                    line_index + 1
                ),
            )
        })?;
        let event = event.as_object().ok_or_else(|| {
            LifecycleFailure::outcome_unknown(
                "native_bootstrap_output_malformed",
                format!(
                    "native exec JSONL stdout line {} is not an event object",
                    line_index + 1
                ),
            )
        })?;
        let event_type = event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                LifecycleFailure::outcome_unknown(
                    "native_bootstrap_output_malformed",
                    format!(
                        "native exec JSONL stdout line {} omitted an event type",
                        line_index + 1
                    ),
                )
            })?;
        if event_type != "thread.started" {
            continue;
        }
        let thread_id = event
            .get("thread_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                LifecycleFailure::outcome_unknown(
                    "native_bootstrap_output_malformed",
                    format!(
                        "native exec thread.started event on line {} omitted a valid thread_id",
                        line_index + 1
                    ),
                )
            })?;
        let thread_id = uuid::Uuid::parse_str(thread_id).map_err(|_| {
            LifecycleFailure::outcome_unknown(
                "native_bootstrap_output_malformed",
                format!(
                    "native exec thread.started event on line {} omitted a valid thread_id",
                    line_index + 1
                ),
            )
        })?;
        candidates.insert(thread_id.to_string());
    }
    match candidates.into_iter().collect::<Vec<_>>().as_slice() {
        [session_id] if output.status.success() => Ok(session_id.clone()),
        [session_id] => Err(LifecycleFailure {
            code: "native_bootstrap_failed".to_string(),
            detail: format!(
                "native exec exited {} after creating a thread",
                output.status
            ),
            outcome_unknown: true,
            known_native_session_id: Some(session_id.clone()),
        }),
        [] => {
            let diagnostic = native_bootstrap_diagnostic(output)
                .map(|diagnostic| format!("; diagnostic: {diagnostic}"))
                .unwrap_or_default();
            Err(LifecycleFailure::outcome_unknown(
                "native_session_unknown",
                format!(
                    "native exec exited {} without one captured SID{diagnostic}",
                    output.status
                ),
            ))
        }
        _ => Err(LifecycleFailure::outcome_unknown(
            "native_session_ambiguous",
            "native exec exposed multiple possible session identities",
        )),
    }
}

fn native_bootstrap_diagnostic(output: &Output) -> Option<String> {
    let mut diagnostics = Vec::new();
    for (source, bytes) in [
        ("stdout", output.stdout.as_slice()),
        ("stderr", output.stderr.as_slice()),
    ] {
        let value = String::from_utf8_lossy(bytes);
        if let Some(value) = sanitized_output_tail(&value) {
            diagnostics.push(format!("{source}: {value}"));
        }
    }
    (!diagnostics.is_empty()).then(|| diagnostics.join("; "))
}

fn sanitized_output_tail(value: &str) -> Option<String> {
    let safe = cutex::observability::sanitize_visible_output(value)?;
    if safe == "[redacted sensitive output]" {
        return Some(safe);
    }
    // Startup wrappers commonly write several informational lines before the
    // actionable failure. Preserve the bounded tail while still running the
    // sensitive-data scan above against the complete raw stream.
    let tail = value
        .lines()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let tail = tail
        .chars()
        .rev()
        .take(cutex::observability::OBSERVABILITY_TEXT_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    cutex::observability::sanitize_visible_output(&tail)
}

fn definite<E: std::fmt::Display>(code: &'static str) -> impl FnOnce(E) -> LifecycleFailure {
    move |error| LifecycleFailure::definite(code, error.to_string())
}

fn unknown<E: std::fmt::Display>(code: &'static str) -> impl FnOnce(E) -> LifecycleFailure {
    move |error| LifecycleFailure::outcome_unknown(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_immediate_projection_covers_both_live_seats() {
        for phase in [
            AgentActionPhase::PredecessorClosing,
            AgentActionPhase::AuthorityTransferred,
            AgentActionPhase::SuccessorReady,
            AgentActionPhase::Complete,
        ] {
            assert!(director_rotation_phase_needs_immediate_projection(
                &AgentManagementPhaseEvent {
                    event_id: "phase-event".to_string(),
                    action_id: cutex::agent_management::AgentActionId::new("rotate").unwrap(),
                    project_id: cutex::agent_management::ProjectId::new("project").unwrap(),
                    operation: AgentOperationKind::DirectorRotate,
                    phase,
                    phase_sequence: 1,
                    committed_at: Rfc3339::new("2026-09-01T00:00:00Z").unwrap(),
                    presentation_owner_cutex_session_id: CutexSessionId::new("cutex.director")
                        .unwrap(),
                    subject_cutex_session_id: None,
                    subject_agent_name: Some("director-r2".to_string()),
                    predecessor_cutex_session_id: None,
                    successor_cutex_session_id: None,
                    replace_policy: None,
                    rotation_mode: None,
                    authority_epoch: Some(2),
                }
            ));
        }

        let mut unrelated = AgentManagementPhaseEvent {
            event_id: "phase-event".to_string(),
            action_id: cutex::agent_management::AgentActionId::new("create").unwrap(),
            project_id: cutex::agent_management::ProjectId::new("project").unwrap(),
            operation: AgentOperationKind::Create,
            phase: AgentActionPhase::Complete,
            phase_sequence: 1,
            committed_at: Rfc3339::new("2026-09-01T00:00:00Z").unwrap(),
            presentation_owner_cutex_session_id: CutexSessionId::new("cutex.director").unwrap(),
            subject_cutex_session_id: None,
            subject_agent_name: Some("worker".to_string()),
            predecessor_cutex_session_id: None,
            successor_cutex_session_id: None,
            replace_policy: None,
            rotation_mode: None,
            authority_epoch: None,
        };
        assert!(!director_rotation_phase_needs_immediate_projection(
            &unrelated
        ));
        unrelated.operation = AgentOperationKind::DirectorRotate;
        unrelated.phase = AgentActionPhase::Prepared;
        assert!(!director_rotation_phase_needs_immediate_projection(
            &unrelated
        ));
    }

    fn resolved_profile(
        runtime: cutex::profiles::model::RuntimeConfig,
    ) -> super::super::launch::ResolvedLaunchProfile {
        let account = cutex::profiles::model::StoredAccount {
            id: "account-id".to_string(),
            name: "profile".to_string(),
            email: None,
            plan_type: None,
            source: None,
            runtime,
            proxy: None,
            session: None,
            cli_kind: cutex::profiles::model::CliKind::Codex,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        };
        let files = cutex::profiles::materialize::materialized_account_files(&account).unwrap();
        super::super::launch::ResolvedLaunchProfile {
            requested: account.name.clone(),
            account,
            files,
        }
    }

    fn spec() -> ManagedAgentSpec {
        ManagedAgentSpec {
            name: "worker-r1".to_string(),
            cwd: "/tmp/project/agent-home/worker-r1".to_string(),
            profile: "aemeath".to_string(),
            runtime_backend: "cute_alden".to_string(),
            model: "gpt-5.6-sol".to_string(),
            reasoning: "high".to_string(),
            permissions: "danger-full-access".to_string(),
            approval_policy: "never".to_string(),
            sandbox_mode: "danger-full-access".to_string(),
            groups: vec!["cutex".to_string(), "task:cutex-205".to_string()],
            expose_to_im: true,
            pin: false,
        }
    }

    fn managed_recovery_record(spec: &ManagedAgentSpec) -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            "cutex.worker-r1".to_string(),
            Some("01a041ba-47f6-7e31-bb09-1462cd309ae4".to_string()),
            cutex::platform::host::current_host_name(),
            spec.cwd.clone(),
            Some(spec.profile.clone()),
            "2026-08-29T00:00:00Z".to_string(),
        )
        .expect("managed recovery record");
        record.managed_cwd = Some(spec.cwd.clone());
        record.runtime_backend =
            parse_cutex_session_runtime_backend(&spec.runtime_backend).unwrap();
        record.thread_name = Some(spec.name.clone());
        record.display_name_hint = Some(spec.name.clone());
        record.agent_enabled = true;
        record.agent_groups = spec.groups.clone();
        record.registration_class = AgentRegistrationClass::Persistent;
        record.exposed_to_backend = spec.expose_to_im;
        record.quick_action = CutexSessionQuickActionMode::Auto;
        record.permission_defaults = Some(spec.permissions.clone());
        record.approval_policy = Some(spec.approval_policy.clone());
        record.sandbox_mode = Some(spec.sandbox_mode.clone());
        record.model_defaults = Some(spec.model.clone());
        record.reasoning_defaults = Some(spec.reasoning.clone());
        record
    }

    #[test]
    fn legacy_director_import_evidence_uses_exact_authoritative_session_fields() {
        let expected = spec();
        let director = CutexSessionId::new("cutex.worker-r1").unwrap();
        let mut record = managed_recovery_record(&expected);
        record.revision = 9;
        record.runtime_generation = 4;
        let evidence = legacy_director_ownership_evidence_from_record(&director, &record)
            .expect("exact legacy Director session evidence");
        assert_eq!(evidence.director_cutex_session_id, director);
        assert_eq!(
            evidence.native_session_id,
            "01a041ba-47f6-7e31-bb09-1462cd309ae4"
        );
        assert_eq!(evidence.durable_session_revision, 9);
        assert_eq!(evidence.runtime_generation, 4);
        assert_eq!(evidence.spec, expected);
    }

    #[test]
    fn legacy_director_import_evidence_rejects_missing_mismatched_and_ambiguous_sessions() {
        let expected = spec();
        let director = CutexSessionId::new("cutex.worker-r1").unwrap();
        let record = managed_recovery_record(&expected);

        macro_rules! reject_record_change {
            ($label:literal, $change:expr) => {{
                let mut changed = record.clone();
                $change(&mut changed);
                assert!(
                    legacy_director_ownership_evidence_from_record(&director, &changed).is_err(),
                    "accepted {} state",
                    $label
                );
            }};
        }

        reject_record_change!(
            "durable identity mismatch",
            |value: &mut CutexSessionRecord| {
                value.cutex_session_id = "cutex.other-director".to_string()
            }
        );
        reject_record_change!("retired", |value: &mut CutexSessionRecord| {
            value.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
            value.retired_at = Some("2026-08-29T01:00:00Z".to_string())
        });
        reject_record_change!(
            "native identity missing",
            |value: &mut CutexSessionRecord| { value.codex_session_id = None }
        );
        reject_record_change!("remote host", |value: &mut CutexSessionRecord| {
            value.host_id = "definitely-remote-host".to_string()
        });
        reject_record_change!("non-managed", |value: &mut CutexSessionRecord| {
            value.agent_enabled = false
        });
        reject_record_change!(
            "unresolved launch claim",
            |value: &mut CutexSessionRecord| {
                value.app_server_launch_claim_id = Some("ambiguous-claim".to_string())
            }
        );
        reject_record_change!("name mismatch", |value: &mut CutexSessionRecord| {
            value.display_name_hint = Some("other-name".to_string())
        });
        reject_record_change!("managed cwd missing", |value: &mut CutexSessionRecord| {
            value.managed_cwd = None
        });
        reject_record_change!("hidden quick action", |value: &mut CutexSessionRecord| {
            value.quick_action = CutexSessionQuickActionMode::Hidden
        });
        reject_record_change!("profile missing", |value: &mut CutexSessionRecord| {
            value.profile = None
        });
        reject_record_change!("noncanonical groups", |value: &mut CutexSessionRecord| {
            value.agent_groups.push("cutex".to_string())
        });
        reject_record_change!("zero durable revision", |value: &mut CutexSessionRecord| {
            value.revision = 0
        });
    }

    #[test]
    fn managed_runtime_recovery_requires_exact_durable_native_and_spec_identity() {
        let spec = spec();
        let cutex_session_id = CutexSessionId::new("cutex.worker-r1").unwrap();
        let native_session_id = "01a041ba-47f6-7e31-bb09-1462cd309ae4";
        let record = managed_recovery_record(&spec);
        validate_managed_recovery_record(&record, &cutex_session_id, native_session_id, &spec)
            .expect("exact managed record is adoptable");

        macro_rules! reject_record_change {
            ($label:literal, $change:expr) => {{
                let mut changed = record.clone();
                $change(&mut changed);
                assert!(
                    validate_managed_recovery_record(
                        &changed,
                        &cutex_session_id,
                        native_session_id,
                        &spec,
                    )
                    .is_err(),
                    "accepted {} mismatch",
                    $label
                );
            }};
        }

        reject_record_change!("durable session", |value: &mut CutexSessionRecord| {
            value.cutex_session_id = "cutex.other-worker".to_string()
        });
        reject_record_change!("native session", |value: &mut CutexSessionRecord| {
            value.codex_session_id = Some("01a041ba-47f6-7e31-bb09-1462cd309ae5".to_string())
        });
        reject_record_change!("host", |value: &mut CutexSessionRecord| {
            value.host_id = "definitely-remote-host".to_string()
        });
        reject_record_change!("managed cwd", |value: &mut CutexSessionRecord| {
            value.managed_cwd = Some("/tmp/other-managed-cwd".to_string())
        });
        reject_record_change!("profile", |value: &mut CutexSessionRecord| {
            value.profile = Some("other-profile".to_string())
        });
        reject_record_change!("name", |value: &mut CutexSessionRecord| {
            value.thread_name = Some("other-worker".to_string())
        });
        reject_record_change!("groups", |value: &mut CutexSessionRecord| {
            value.agent_groups.push("unexpected-authority".to_string())
        });
        reject_record_change!("permissions", |value: &mut CutexSessionRecord| {
            value.permission_defaults = Some("read-only".to_string())
        });
        reject_record_change!("approval policy", |value: &mut CutexSessionRecord| {
            value.approval_policy = Some("on-request".to_string())
        });
        reject_record_change!("sandbox", |value: &mut CutexSessionRecord| {
            value.sandbox_mode = Some("workspace-write".to_string())
        });
        reject_record_change!("model", |value: &mut CutexSessionRecord| {
            value.model_defaults = Some("other-model".to_string())
        });
        reject_record_change!("reasoning", |value: &mut CutexSessionRecord| {
            value.reasoning_defaults = Some("low".to_string())
        });
    }

    #[test]
    fn managed_runtime_recovery_accepts_only_deterministic_system_groups() {
        let spec = spec();
        let cutex_session_id = CutexSessionId::new("cutex.worker-r1").unwrap();
        let native_session_id = "01a041ba-47f6-7e31-bb09-1462cd309ae4";
        let mut record = managed_recovery_record(&spec);
        let cwd_hash = cutex::agent_bus::identity::fnv1a_hex(&spec.cwd);
        record.agent_groups = cutex::agent_bus::groups::normalize_registered_agent_groups(
            spec.groups.clone(),
            Some(&cwd_hash[..7]),
            &spec.cwd,
        );

        validate_managed_recovery_record(&record, &cutex_session_id, native_session_id, &spec)
            .expect("deterministic system group expansion is allowed");
    }

    #[test]
    fn bootstrap_plan_uses_normal_cutex_exec_hi_lifecycle() {
        let plan = native_bootstrap_plan(&spec()).unwrap();
        assert_eq!(plan.cwd, PathBuf::from("/tmp/project/agent-home/worker-r1"));
        assert_eq!(
            plan.args,
            [
                "run",
                "aemeath",
                "--agent",
                "--group",
                "cutex",
                "--group",
                "task:cutex-205",
                "--",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "Hi."
            ]
        );
    }

    #[test]
    fn reconciliation_selects_the_profiles_actual_host_or_docker_codex_home() {
        let home = crate::cli_app::test_home::IsolatedTestHome::new(
            "cutex-agent-management-profile-codex-home",
        )
        .unwrap();
        let host = resolved_profile(cutex::profiles::model::RuntimeConfig::Host);
        assert_eq!(
            selected_profile_codex_home(&host).unwrap(),
            home.root().join(".cutex").join("codex-home")
        );

        let docker = resolved_profile(cutex::profiles::model::RuntimeConfig::Docker {
            image: "cutex-dev".to_string(),
            user_name: Some("worker".to_string()),
        });
        assert_eq!(
            selected_profile_codex_home(&docker).unwrap(),
            home.root()
                .join(".cutex")
                .join("runtime")
                .join("docker-home")
                .join(".codex")
        );
    }

    #[cfg(unix)]
    fn output(status: i32, stdout: &str, stderr: &str) -> Output {
        use std::os::unix::process::ExitStatusExt;
        Output {
            status: std::process::ExitStatus::from_raw(status << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn sid_capture_accepts_only_structured_thread_started_identity() {
        let sid = "01a041ba-47f6-7e31-bb09-1462cd309ae4";
        let unrelated = "01a041ba-47f6-7e31-bb09-1462cd309ae9";
        assert_eq!(
            captured_native_session_id(&output(
                0,
                &format!(
                    "{{\"type\":\"profile.loaded\",\"account_id\":\"{unrelated}\"}}\n\
                     {{\"type\":\"thread.started\",\"thread_id\":\"{sid}\"}}\n\
                     {{\"type\":\"action.completed\",\"action_id\":\"{unrelated}\"}}\n"
                ),
                &format!("profile id: {unrelated}\n"),
            ))
            .unwrap(),
            sid
        );
    }

    #[cfg(unix)]
    #[test]
    fn sid_capture_rejects_conflicting_or_malformed_structured_events() {
        let sid = "01a041ba-47f6-7e31-bb09-1462cd309ae4";
        let other_sid = "01a041ba-47f6-7e31-bb09-1462cd309ae5";
        let error = captured_native_session_id(&output(
            0,
            &format!(
                "{{\"type\":\"thread.started\",\"thread_id\":\"{sid}\"}}\n\
                 {{\"type\":\"thread.started\",\"thread_id\":\"{other_sid}\"}}\n"
            ),
            "",
        ))
        .unwrap_err();
        assert_eq!(error.code, "native_session_ambiguous");
        assert!(error.outcome_unknown);

        for malformed in [
            "not-json\n",
            "Running profile jsonl without changing active profile\n",
            "[]\n",
            "{}\n",
            "{\"type\":\"thread.started\"}\n",
            "{\"type\":\"thread.started\",\"thread_id\":\"not-a-uuid\"}\n",
        ] {
            let error = captured_native_session_id(&output(0, malformed, ""))
                .expect_err("malformed structured output must remain fail-closed");
            assert_eq!(error.code, "native_bootstrap_output_malformed");
            assert!(error.outcome_unknown);
        }

        for output in [
            output(
                1,
                "",
                "profile auth failed with UUID 01a041ba-47f6-7e31-bb09-1462cd309ae9\n",
            ),
            output(0, "", ""),
        ] {
            let missing = captured_native_session_id(&output)
                .expect_err("native output without a SID remains ambiguous");
            assert_eq!(missing.code, "native_session_unknown");
            assert!(missing.outcome_unknown);
            assert!(missing.known_native_session_id.is_none());
        }

        let diagnostic =
            captured_native_session_id(&output(1, "", "provider startup failed\nsecond line\n"))
                .expect_err("failed bootstrap without a SID must retain a safe diagnostic");
        assert!(diagnostic
            .detail
            .contains("diagnostic: stderr: provider startup failed\nsecond line"));

        let redacted =
            captured_native_session_id(&output(1, "", "Authorization: Bearer secret-value\n"))
                .expect_err("credential-bearing diagnostics remain fail-closed");
        assert!(redacted
            .detail
            .contains("diagnostic: stderr: [redacted sensitive output]"));
        assert!(!redacted.detail.contains("secret-value"));

        let both_streams = captured_native_session_id(&output(
            1,
            "{\"type\":\"error\",\"message\":\"provider rejected request\"}\n",
            "wrapper line 1\nwrapper line 2\nwrapper line 3\nwrapper line 4\nwrapper line 5\nwrapper line 6\nactionable stderr tail\n",
        ))
        .expect_err("both diagnostic streams remain available");
        assert!(both_streams.detail.contains("stdout: {\"type\":\"error\""));
        assert!(both_streams.detail.contains("actionable stderr tail"));
        assert!(!both_streams.detail.contains("wrapper line 1"));

        let non_utf8 = Output {
            status: output(0, "", "").status,
            stdout: vec![0xff, b'\n'],
            stderr: Vec::new(),
        };
        let malformed = captured_native_session_id(&non_utf8)
            .expect_err("non-UTF-8 protocol output must remain fail-closed");
        assert_eq!(malformed.code, "native_bootstrap_output_malformed");
        assert!(malformed.detail.contains("not valid UTF-8"));
        assert!(malformed.outcome_unknown);
    }

    #[test]
    fn agent_message_source_binding_never_claims_a_retired_runtime() {
        let director = CutexSessionId::new("cutex.director-r10").unwrap();
        assert_eq!(
            agent_message_source_binding(&director, Some("runtime-r10".to_string())),
            (
                Some("runtime-r10".to_string()),
                Some("cutex.director-r10".to_string())
            )
        );
        assert_eq!(agent_message_source_binding(&director, None), (None, None));
    }
}
