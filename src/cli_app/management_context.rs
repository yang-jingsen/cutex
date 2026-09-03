use std::sync::Mutex;
use std::sync::OnceLock;

use cutex::im::registry::ImRegistry;
use cutex::management::server::ManagementNativeForwardError;
use cutex::management::server::ManagementRequestContext;
use cutex::platform::process::process_is_running;
use cutex::session::service::coding_registration_from_cutex_session_record;
use cutex::session::store::load_cutex_session_store;

static MANAGEMENT_V2_SESSION_MUTATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn load_management_v2_registry() -> anyhow::Result<ImRegistry> {
    Ok(ImRegistry::default())
}

pub(crate) fn management_request_context() -> ManagementRequestContext {
    ManagementRequestContext {
        load_registry: load_management_v2_registry,
        load_runtime_status: load_app_server_runtime_status,
        forward_native_request: forward_management_native_request,
        respond_native_server_request: respond_management_native_server_request,
        handle_user_input: super::app_server_user_input::submit_v2,
        flush_user_input_queue: flush_management_user_input_queue,
        load_bootstrap_state: load_management_bootstrap_state,
        mutate_session: mutate_management_v2_session,
        retry_release_rotation: super::rotation::handle_release_rotation_retry,
        request_release_rotation: super::rotation::execute_release_rotation,
        bind_project_authority: super::agent_management::bind_project_authority,
        import_legacy_director_ownership: super::agent_management::import_legacy_director_ownership,
    }
}

fn load_management_bootstrap_state(
    cutex_session_id: &str,
    runtime_generation: u64,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::json!({
        "userInputQueue": cutex::management::v2::user_input::user_input_repository()?
            .snapshot(cutex_session_id)?,
        "pendingServerRequests": cutex::management::v2::server_requests::snapshot(
            cutex_session_id,
            runtime_generation,
        )?,
        "agentBusMessages": cutex::management::v2::agent_bus_state::agent_bus_message_repository()?
            .snapshot(cutex_session_id)?,
    }))
}

fn mutate_management_v2_session(
    cutex_session_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, cutex::management::v2::user_input::UserInputExecutionError> {
    use cutex::management::v2::user_input::UserInputExecutionError;
    use cutex::session::model::parse_cutex_session_runtime_backend;
    use cutex::session::service::cutex_session_key_for_user_id;
    use cutex::session::service::persist_cutex_session_store_and_im_record;
    use cutex::session::service::set_cutex_session_profile_by_key_with_expected_revision;

    let _guard = MANAGEMENT_V2_SESSION_MUTATION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| session_mutation_invalid("management v2 session mutation lock poisoned"))?;
    if matches!(
        method,
        "cutex/runtime/online" | "cutex/runtime/offline" | "cutex/runtime/close"
    ) {
        return mutate_management_v2_runtime(cutex_session_id, method, &params);
    }
    if matches!(method, "cutex/session/retire" | "cutex/session/restore") {
        return super::management_archive::mutate_management_v2_archive(
            cutex_session_id,
            method,
            &params,
        );
    }
    let mut store = load_cutex_session_store().map_err(session_mutation_persistence_error)?;
    let key = cutex_session_key_for_user_id(&store, cutex_session_id).ok_or_else(|| {
        UserInputExecutionError {
            stage: "route".to_string(),
            code: "session_not_found".to_string(),
            message: "the durable cutex session does not exist".to_string(),
            retryable: false,
            details: serde_json::json!({ "cutexSessionId": cutex_session_id }),
            outcome_unknown: false,
        }
    })?;
    if matches!(
        method,
        "cutex/session/profile/set" | "cutex/session/profile/clear"
    ) {
        let expected_revision = params
            .get("expectedRevision")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                session_mutation_invalid("expectedRevision must be a JSON-safe integer")
            })?;
        let target = store.sessions.get(&key).ok_or_else(|| {
            session_mutation_invalid("cutex session disappeared during profile mutation")
        })?;
        if !cutex::runtime::lifecycle::cutex_session_host_is_local(
            &target.host_id,
            &cutex::platform::host::current_host_name(),
        ) {
            return Err(session_mutation_invalid(
                "profile mutation must be resolved on the target host",
            ));
        }
        let profile = match method {
            "cutex/session/profile/set" => {
                let requested = params
                    .get("profile")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        session_mutation_invalid("profile must be a non-empty string")
                    })?;
                Some(
                    super::launch::resolve_launch_profile_override(requested)
                        .map_err(session_mutation_invalid_error)?
                        .account
                        .name,
                )
            }
            "cutex/session/profile/clear" => None,
            _ => unreachable!(),
        };
        let outcome = set_cutex_session_profile_by_key_with_expected_revision(
            &mut store,
            &key,
            profile.clone(),
            expected_revision,
        )
        .map_err(|error| {
            let current = store
                .sessions
                .get(&key)
                .map(|record| record.durable_revision());
            if current.is_some_and(|current| current != expected_revision) {
                session_revision_conflict(expected_revision, current.expect("current revision"))
            } else {
                session_mutation_invalid_error(error)
            }
        })?;
        persist_cutex_session_store_and_im_record(&store, &key).map_err(|error| {
            if error
                .downcast_ref::<cutex::session::store::CutexSessionStoreRevisionConflict>()
                .is_some()
            {
                session_mutation_persistence_error(error)
            } else {
                session_mutation_persistence_uncertain(format!("{error:#}"))
            }
        })?;
        return Ok(serde_json::json!({
            "cutexSessionId": outcome.key,
            "revision": store.sessions.get(&key).expect("persisted session").durable_revision(),
            "configuredProfile": profile,
        }));
    }
    let record = store
        .sessions
        .get_mut(&key)
        .ok_or_else(|| UserInputExecutionError {
            stage: "route".to_string(),
            code: "session_not_found".to_string(),
            message: "the durable cutex session disappeared during mutation".to_string(),
            retryable: false,
            details: serde_json::json!({ "cutexSessionId": cutex_session_id }),
            outcome_unknown: false,
        })?;
    let result = match method {
        "cutex/session/defaults/update" => {
            let patch = params
                .get("patch")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| session_mutation_invalid("defaults patch must be an object"))?;
            if let Some(backend) = patch.get("backend").and_then(serde_json::Value::as_str) {
                record.runtime_backend = parse_cutex_session_runtime_backend(backend)
                    .map_err(session_mutation_invalid_error)?;
            }
            apply_optional_string(patch, "managedCwd", &mut record.managed_cwd)?;
            apply_optional_string(patch, "permissions", &mut record.permission_defaults)?;
            apply_optional_string(patch, "approvalPolicy", &mut record.approval_policy)?;
            apply_optional_string(patch, "sandboxMode", &mut record.sandbox_mode)?;
            apply_optional_string(patch, "model", &mut record.model_defaults)?;
            apply_optional_string(patch, "reasoningEffort", &mut record.reasoning_defaults)?;
            if let Some(args) = patch.get("cliArgs") {
                record.default_cli_args = json_string_array(args, "cliArgs")?;
            }
            if let Some(groups) = patch.get("groups") {
                record.agent_groups = normalized_string_array(groups, "groups")?;
            }
            serde_json::json!({})
        }
        "cutex/session/groups/set" | "cutex/session/groups/add" | "cutex/session/groups/remove" => {
            let groups = normalized_string_array(
                params
                    .get("groups")
                    .ok_or_else(|| session_mutation_invalid("groups are required"))?,
                "groups",
            )?;
            match method {
                "cutex/session/groups/set" => record.agent_groups = groups,
                "cutex/session/groups/add" => {
                    record.agent_groups.extend(groups);
                    record.agent_groups.sort();
                    record.agent_groups.dedup();
                }
                "cutex/session/groups/remove" => {
                    record.agent_groups.retain(|group| !groups.contains(group));
                }
                _ => unreachable!(),
            }
            serde_json::json!({ "groups": record.agent_groups })
        }
        "cutex/session/visibility/show" => {
            record.exposed_to_backend = true;
            serde_json::json!({})
        }
        "cutex/session/visibility/hide" => {
            record.exposed_to_backend = false;
            serde_json::json!({})
        }
        _ => {
            return Err(session_mutation_invalid(
                "unsupported session mutation method",
            ))
        }
    };
    record
        .bump_durable_revision()
        .map_err(session_mutation_persistence_error)?;
    record.updated_at = chrono::Utc::now().to_rfc3339();
    persist_cutex_session_store_and_im_record(&store, &key).map_err(|error| {
        if error
            .downcast_ref::<cutex::session::store::CutexSessionStoreRevisionConflict>()
            .is_some()
        {
            session_mutation_persistence_error(error)
        } else {
            session_mutation_persistence_uncertain(format!("{error:#}"))
        }
    })?;
    let revision = store
        .sessions
        .get(&key)
        .map(|record| record.durable_revision())
        .ok_or_else(|| session_mutation_invalid("cutex session disappeared after mutation"))?;
    let mut result = result;
    result
        .as_object_mut()
        .expect("session mutation result object")
        .insert("revision".to_string(), serde_json::json!(revision));
    Ok(result)
}

pub(crate) fn mutate_archive_session(
    cutex_session_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, cutex::management::v2::user_input::UserInputExecutionError> {
    mutate_management_v2_session(cutex_session_id, method, params)
}

fn mutate_management_v2_runtime(
    cutex_session_id: &str,
    method: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, cutex::management::v2::user_input::UserInputExecutionError> {
    use cutex::management::v2::model::MAX_SAFE_SEQUENCE;
    use cutex::management::v2::user_input::UserInputExecutionError;
    use cutex::session::service::cutex_session_key_for_user_id;

    let expected_generation = params
        .get("expectedRuntimeGeneration")
        .and_then(serde_json::Value::as_u64)
        .filter(|generation| *generation <= MAX_SAFE_SEQUENCE)
        .ok_or_else(|| {
            session_mutation_invalid("expectedRuntimeGeneration must be a JSON-safe integer")
        })?;
    let store = load_cutex_session_store().map_err(session_mutation_persistence_error)?;
    let key = cutex_session_key_for_user_id(&store, cutex_session_id).ok_or_else(|| {
        UserInputExecutionError {
            stage: "route".to_string(),
            code: "session_not_found".to_string(),
            message: "the durable cutex session does not exist".to_string(),
            retryable: false,
            details: serde_json::json!({ "cutexSessionId": cutex_session_id }),
            outcome_unknown: false,
        }
    })?;
    let record = store
        .sessions
        .get(&key)
        .cloned()
        .ok_or_else(|| session_mutation_invalid("cutex session disappeared during mutation"))?;
    if record.runtime_generation != expected_generation {
        return Err(UserInputExecutionError {
            stage: "route".to_string(),
            code: "revision_conflict".to_string(),
            message: format!(
                "runtime generation conflict: expected {expected_generation}, current {}",
                record.runtime_generation
            ),
            retryable: true,
            details: serde_json::json!({
                "expectedRuntimeGeneration": expected_generation,
                "currentRuntimeGeneration": record.runtime_generation,
                "resyncRequired": true,
            }),
            outcome_unknown: false,
        });
    }
    let entry = coding_registration_from_cutex_session_record(&record).ok_or_else(|| {
        session_mutation_invalid("cutex session has no bound Codex thread identity")
    })?;
    let config = cutex::config::store::load_codez_config();
    let launch_profile = if method == "cutex/runtime/online" {
        match params.get("launchProfile") {
            Some(serde_json::Value::String(requested)) => {
                let current_host = cutex::platform::host::current_host_name();
                if !cutex::runtime::lifecycle::cutex_session_host_is_local(
                    &record.host_id,
                    &current_host,
                ) {
                    return Err(launch_profile_unavailable(
                        requested,
                        anyhow::anyhow!(
                            "one-launch profile must be resolved on target host {} (current host {})",
                            record.host_id,
                            current_host
                        ),
                    ));
                }
                Some(
                    super::launch::resolve_launch_profile_override(requested)
                        .map_err(|error| launch_profile_unavailable(requested, error))?,
                )
            }
            Some(_) => {
                return Err(session_mutation_invalid(
                    "launchProfile must be a non-empty string",
                ))
            }
            None => None,
        }
    } else {
        None
    };
    let mut profile_application = LaunchProfileApplication::default();

    let status = match method {
        "cutex/runtime/online" => {
            let open_visible_terminal = params
                .get("openVisibleTerminal")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let manager_status = super::app_server_runtime::runtime_manager()
                .status(cutex_session_id)
                .map_err(session_mutation_persistence_error)?;
            if manager_status.as_ref().is_some_and(|status| {
                status.connected && status.runtime_generation != record.runtime_generation
            }) {
                return Err(runtime_cutover_required(
                    &record,
                    "manager_generation_does_not_match_persisted_runtime",
                    None,
                ));
            }
            if manager_status
                .as_ref()
                .is_some_and(|status| status.connected && record.app_server_runtime.is_none())
            {
                return Err(runtime_cutover_required(
                    &record,
                    "manager_runtime_binding_missing",
                    None,
                ));
            }
            let mut manager_connected = manager_status
                .as_ref()
                .is_some_and(|status| status.connected);
            if manager_connected
                && record
                    .app_server_runtime
                    .as_ref()
                    .is_some_and(|binding| !process_is_running(binding.pid))
            {
                // A manager worker can lag the child exit by one poll tick.
                // Do not let that stale in-memory status suppress persisted
                // ownership recovery or a typed cutover decision.
                let _ = super::app_server_runtime::disconnect_runtime(cutex_session_id);
                manager_connected = false;
            }
            if !manager_connected {
                let recovery_action =
                    super::app_server_runtime::classify_local_persisted_runtime_recovery(
                        &record,
                        process_is_running,
                    );
                let launch_new_runtime = match recovery_action {
                    super::app_server_runtime::PersistedRuntimeRecoveryAction::Reconnect {
                        runtime_agent_id,
                    } => {
                        let binding = record
                            .app_server_runtime
                            .as_ref()
                            .expect("reconnect action requires a persisted binding");
                        if let Err(error) = super::app_server_runtime::connect_runtime(
                            &config,
                            &record,
                            binding,
                            &runtime_agent_id,
                        ) {
                            let rollback =
                                super::app_server_runtime::restore_recovery_snapshot_if_owned(
                                    &record,
                                );
                            eprintln!(
                                "same-binding runtime recovery failed for {}: {error:#}",
                                record.cutex_session_id
                            );
                            if let Err(rollback_error) = rollback {
                                eprintln!(
                                    "same-binding runtime recovery rollback failed for {}: {rollback_error:#}",
                                    record.cutex_session_id
                                );
                                return Err(runtime_cutover_required(
                                    &record,
                                    "same_binding_reconnect_rollback_failed",
                                    Some(binding.pid),
                                ));
                            }
                            return Err(runtime_cutover_required(
                                &record,
                                "same_binding_reconnect_failed",
                                Some(binding.pid),
                            ));
                        }
                        match super::app_server_runtime::runtime_manager()
                            .status(&record.cutex_session_id)
                        {
                            Ok(Some(status))
                                if status.connected
                                    && status.runtime_generation == record.runtime_generation =>
                            {
                            }
                            Ok(_) | Err(_) => {
                                let _ = super::app_server_runtime::disconnect_runtime(
                                    &record.cutex_session_id,
                                );
                                return Err(runtime_cutover_required(
                                    &record,
                                    "manager_owner_not_connected_after_recovery",
                                    Some(binding.pid),
                                ));
                            }
                        }
                        match super::app_server_runtime::persisted_runtime_ownership_matches(
                            &record,
                        ) {
                            Ok(true) => {}
                            Ok(false) => {
                                let _ = super::app_server_runtime::disconnect_runtime(
                                    &record.cutex_session_id,
                                );
                                return Err(runtime_cutover_required(
                                    &record,
                                    "runtime_changed_during_same_binding_reconnect",
                                    Some(binding.pid),
                                ));
                            }
                            Err(error) => {
                                let _ = super::app_server_runtime::disconnect_runtime(
                                    &record.cutex_session_id,
                                );
                                eprintln!(
                                    "failed to verify same-binding runtime ownership for {}: {error:#}",
                                    record.cutex_session_id
                                );
                                return Err(runtime_cutover_required(
                                    &record,
                                    "runtime_ownership_verification_failed",
                                    Some(binding.pid),
                                ));
                            }
                        }
                        false
                    }
                    super::app_server_runtime::PersistedRuntimeRecoveryAction::ClearStaleAndLaunch => {
                        if let Err(error) =
                            super::app_server_runtime::clear_stale_persisted_runtime(&record)
                        {
                            eprintln!(
                                "stale runtime cleanup failed for {}: {error:#}",
                                record.cutex_session_id
                            );
                            return Err(runtime_cutover_required(
                                &record,
                                "stale_runtime_cleanup_failed",
                                record
                                    .app_server_runtime
                                    .as_ref()
                                    .map(|binding| binding.pid),
                            ));
                        }
                        true
                    }
                    super::app_server_runtime::PersistedRuntimeRecoveryAction::Launch => true,
                    super::app_server_runtime::PersistedRuntimeRecoveryAction::CutoverRequired {
                        reason,
                        pid,
                    } => return Err(runtime_cutover_required(&record, reason, pid)),
                };
                if launch_new_runtime {
                    let outcome =
                        super::management_lifecycle::start_cutex_session_online_with_profile(
                            &config,
                            &entry,
                            launch_profile.as_ref(),
                        )
                        .map_err(runtime_mutation_error)?;
                    profile_application.runtime |= outcome.runtime_launched;
                    profile_application.tui |= outcome.tui_launched;
                }
            }
            profile_application.tui |=
                super::management_lifecycle::ensure_managed_tui_peer_with_profile(
                    &entry,
                    launch_profile.as_ref(),
                )
                .map_err(runtime_mutation_error)?;
            let foreground_required_reason = if record.runtime_backend
                == cutex::session::model::CutexSessionRuntimeBackend::HostForeground
                && open_visible_terminal
            {
                match cutex::management::host_foreground_actions::try_start_host_foreground_desktop_terminal_with_profile(
                    &record,
                    launch_profile.as_ref().map(|profile| profile.effective_name()),
                ) {
                    Ok(true) => {
                        profile_application.tui = true;
                        None
                    }
                    Ok(false) => {
                        eprintln!(
                            "visible terminal is required for host_foreground session {}",
                            record.cutex_session_id
                        );
                        Some("desktop_launcher_unavailable")
                    }
                    Err(error) => {
                        eprintln!(
                            "failed to launch visible terminal for {}: {error:#}",
                            record.cutex_session_id
                        );
                        Some("desktop_launcher_failed")
                    }
                }
            } else {
                None
            };
            ("online", foreground_required_reason)
        }
        "cutex/runtime/offline" | "cutex/runtime/close" => {
            let force = runtime_stop_force_policy(
                method,
                params
                    .get("force")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                cfg!(windows),
            );
            let live_agents =
                super::management_lifecycle::live_agents_for_management_entry(&config, &entry);
            let stop = super::management_lifecycle::stop_cutex_session_runtime_for_entry(
                &entry,
                &live_agents,
                force,
            )
            .map_err(runtime_mutation_error)?;
            if !stop.stopped {
                ("closing", None)
            } else if method == "cutex/runtime/close" {
                ("closed", None)
            } else {
                ("offline", None)
            }
        }
        _ => {
            return Err(session_mutation_invalid(
                "unsupported runtime mutation method",
            ))
        }
    };
    let store = load_cutex_session_store().map_err(session_mutation_persistence_error)?;
    let record = store
        .sessions
        .get(&key)
        .ok_or_else(|| session_mutation_invalid("cutex session disappeared after mutation"))?;
    let mut result = serde_json::json!({
        "runtimeGeneration": record.runtime_generation,
        "runtimeAgentId": record.current_runtime_agent_id,
        "status": status.0,
    });
    if let Some(reason) = status.1 {
        result
            .as_object_mut()
            .expect("runtime mutation result object")
            .insert(
                "foregroundRequiredReason".to_string(),
                serde_json::Value::String(reason.to_string()),
            );
    }
    let launched_provenance = record.app_server_runtime.as_ref().and_then(|binding| {
        binding.launched_profile.as_deref().map(|selected| {
            let source = binding
                .launch_profile_source
                .as_ref()
                .map(|source| source.as_str())
                .unwrap_or("unknown");
            (selected, source)
        })
    });
    if let Some(profile) = launch_profile.as_ref() {
        let receipt = profile_application
            .receipt(
                &profile.requested,
                profile.effective_name(),
                "one_launch_override",
            )
            .ok_or_else(|| launch_profile_not_applied(profile))?;
        result
            .as_object_mut()
            .expect("runtime mutation result object")
            .insert("launchProfile".to_string(), receipt);
    } else if let Some((selected, source @ ("session_configured" | "global_default" | "unknown"))) =
        launched_provenance
    {
        if let Some(receipt) = profile_application.receipt(selected, selected, source) {
            result
                .as_object_mut()
                .expect("runtime mutation result object")
                .insert("launchProfile".to_string(), receipt);
        }
    }
    Ok(result)
}

fn runtime_stop_force_policy(method: &str, requested_force: bool, windows: bool) -> bool {
    requested_force
        || (windows && matches!(method, "cutex/runtime/offline" | "cutex/runtime/close"))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LaunchProfileApplication {
    runtime: bool,
    tui: bool,
}

impl LaunchProfileApplication {
    fn scope(self) -> Option<&'static str> {
        match (self.runtime, self.tui) {
            (true, true) => Some("runtime_and_tui"),
            (true, false) => Some("runtime"),
            (false, true) => Some("tui"),
            (false, false) => None,
        }
    }

    fn receipt(self, requested: &str, effective: &str, source: &str) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "requested": requested,
            "selected": effective,
            "effective": effective,
            "source": source,
            "applicationScope": self.scope()?,
            "persisted": false,
        }))
    }
}

fn launch_profile_unavailable(
    requested: &str,
    error: anyhow::Error,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "runtime".to_string(),
        code: "launch_profile_unavailable".to_string(),
        message: format!("one-launch profile '{requested}' is unavailable: {error:#}"),
        retryable: false,
        details: serde_json::json!({
            "requested": requested,
            "persisted": false,
        }),
        outcome_unknown: false,
    }
}

fn launch_profile_not_applied(
    profile: &super::launch::ResolvedLaunchProfile,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "runtime".to_string(),
        code: "launch_profile_not_applied".to_string(),
        message:
            "the requested profile was not applied because the action reused existing processes"
                .to_string(),
        retryable: false,
        details: serde_json::json!({
            "requested": profile.requested,
            "effective": profile.effective_name(),
            "applicationScope": "none",
            "persisted": false,
        }),
        outcome_unknown: false,
    }
}

fn runtime_mutation_error(
    error: anyhow::Error,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "runtime".to_string(),
        code: "runtime_mutation_failed".to_string(),
        message: format!("{error:#}"),
        retryable: true,
        details: serde_json::json!({}),
        outcome_unknown: false,
    }
}

fn runtime_cutover_required(
    record: &cutex::session::model::CutexSessionRecord,
    reason: &'static str,
    pid: Option<u32>,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "runtime".to_string(),
        code: "cutover_required".to_string(),
        message:
            "the existing managed runtime could not be safely recovered; explicit offline/cutover is required before launching a new generation"
                .to_string(),
        retryable: false,
        details: serde_json::json!({
            "runtimeGeneration": record.runtime_generation,
            "runtimePid": pid,
            "reason": reason,
            "recoveryAttempts": 1,
            "requiredAction": "cutex/runtime/offline",
        }),
        outcome_unknown: false,
    }
}

fn apply_optional_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    target: &mut Option<String>,
) -> Result<(), cutex::management::v2::user_input::UserInputExecutionError> {
    let Some(value) = object.get(key) else {
        return Ok(());
    };
    *target = match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) => Some(value.clone()),
        _ => {
            return Err(session_mutation_invalid(&format!(
                "{key} must be string or null"
            )))
        }
    };
    Ok(())
}

fn json_string_array(
    value: &serde_json::Value,
    label: &str,
) -> Result<Vec<String>, cutex::management::v2::user_input::UserInputExecutionError> {
    value
        .as_array()
        .ok_or_else(|| session_mutation_invalid(&format!("{label} must be an array")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| session_mutation_invalid(&format!("{label} must contain strings")))
        })
        .collect()
}

fn normalized_string_array(
    value: &serde_json::Value,
    label: &str,
) -> Result<Vec<String>, cutex::management::v2::user_input::UserInputExecutionError> {
    let mut values = json_string_array(value, label)?;
    if values.iter().any(|value| value.is_empty()) {
        return Err(session_mutation_invalid(&format!(
            "{label} must not contain empty strings"
        )));
    }
    values.sort();
    values.dedup();
    Ok(values)
}

fn session_mutation_invalid(
    message: &str,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "route".to_string(),
        code: "invalid_request".to_string(),
        message: message.to_string(),
        retryable: false,
        details: serde_json::json!({}),
        outcome_unknown: false,
    }
}

fn session_mutation_invalid_error(
    error: anyhow::Error,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    session_mutation_invalid(&format!("{error:#}"))
}

fn session_revision_conflict(
    expected_revision: u64,
    current_revision: u64,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "route".to_string(),
        code: "revision_conflict".to_string(),
        message: format!(
            "session revision conflict: expected {expected_revision}, current {current_revision}"
        ),
        retryable: true,
        details: serde_json::json!({
            "expectedRevision": expected_revision,
            "currentRevision": current_revision,
            "resyncRequired": true,
        }),
        outcome_unknown: false,
    }
}

fn session_mutation_persistence_error(
    error: anyhow::Error,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "route".to_string(),
        code: "event_persistence_unavailable".to_string(),
        message: format!("{error:#}"),
        retryable: true,
        details: serde_json::json!({}),
        outcome_unknown: false,
    }
}

fn session_mutation_persistence_uncertain(
    detail: impl Into<String>,
) -> cutex::management::v2::user_input::UserInputExecutionError {
    cutex::management::v2::user_input::UserInputExecutionError {
        stage: "persistence".to_string(),
        code: "persistence_uncertain".to_string(),
        message: "the durable session outcome is uncertain; resync before retrying".to_string(),
        retryable: false,
        details: serde_json::json!({
            "diagnostic": detail.into(),
            "resyncRequired": true,
        }),
        outcome_unknown: true,
    }
}

fn flush_management_user_input_queue(
    cutex_session_id: &str,
    max_items: usize,
) -> anyhow::Result<usize> {
    let mut flushed = 0;
    while flushed < max_items
        && super::app_server_user_input::flush_queued_if_idle(cutex_session_id)?
    {
        flushed += 1;
    }
    Ok(flushed)
}

fn respond_management_native_server_request(
    cutex_session_id: &str,
    expected_runtime_generation: u64,
    message: serde_json::Value,
) -> Result<(), ManagementNativeForwardError> {
    use cutex::app_server::client::AppServerClientError;
    use cutex::app_server::manager::AppServerExactRuntimeHandleError;

    let handle = super::app_server_runtime::runtime_manager()
        .handle_for_generation(cutex_session_id, expected_runtime_generation)
        .map_err(|error| match error {
            AppServerExactRuntimeHandleError::StaleGeneration { expected, actual } => {
                ManagementNativeForwardError::StaleRuntimeGeneration { expected, actual }
            }
            AppServerExactRuntimeHandleError::Unavailable(message) => {
                ManagementNativeForwardError::BeforeForward(message)
            }
        })?;
    handle.respond_raw(message).map_err(|error| match error {
        AppServerClientError::InvalidEndpoint(message)
        | AppServerClientError::InvalidOptions(message)
        | AppServerClientError::Connect(message)
        | AppServerClientError::Protocol(message)
        | AppServerClientError::RequestIdInUse(message) => {
            ManagementNativeForwardError::BeforeForward(message)
        }
        AppServerClientError::Backpressure(queue) => {
            ManagementNativeForwardError::BeforeForward(format!("app-server {queue} queue is full"))
        }
        AppServerClientError::Shutdown => ManagementNativeForwardError::BeforeForward(
            "app-server client is shut down".to_string(),
        ),
        AppServerClientError::Transport(message) | AppServerClientError::Disconnected(message) => {
            ManagementNativeForwardError::OutcomeUnknown(message)
        }
        AppServerClientError::Timeout { method } => ManagementNativeForwardError::OutcomeUnknown(
            format!("timed out waiting for app-server {method}"),
        ),
        AppServerClientError::Rpc(error) => ManagementNativeForwardError::OutcomeUnknown(format!(
            "unexpected native RPC error {}: {}",
            error.code, error.message
        )),
    })
}

fn forward_management_native_request(
    cutex_session_id: &str,
    message: serde_json::Value,
) -> Result<serde_json::Value, ManagementNativeForwardError> {
    use cutex::app_server::client::AppServerClientError;

    let handle = super::app_server_runtime::runtime_manager()
        .handle(cutex_session_id)
        .map_err(|error| ManagementNativeForwardError::BeforeForward(format!("{error:#}")))?;
    handle
        .request_raw_message(message)
        .map_err(|error| match error {
            AppServerClientError::RequestIdInUse(message) => {
                ManagementNativeForwardError::NativeRequestIdInUse(message)
            }
            AppServerClientError::InvalidEndpoint(message)
            | AppServerClientError::InvalidOptions(message)
            | AppServerClientError::Connect(message)
            | AppServerClientError::Protocol(message) => {
                ManagementNativeForwardError::BeforeForward(message)
            }
            AppServerClientError::Backpressure(queue) => {
                ManagementNativeForwardError::BeforeForward(format!(
                    "app-server {queue} queue is full"
                ))
            }
            AppServerClientError::Shutdown => ManagementNativeForwardError::BeforeForward(
                "app-server client is shut down".to_string(),
            ),
            AppServerClientError::Transport(message)
            | AppServerClientError::Disconnected(message) => {
                ManagementNativeForwardError::OutcomeUnknown(message)
            }
            AppServerClientError::Timeout { method } => {
                ManagementNativeForwardError::OutcomeUnknown(format!(
                    "timed out waiting for app-server {method}"
                ))
            }
            AppServerClientError::Rpc(error) => {
                ManagementNativeForwardError::OutcomeUnknown(format!(
                    "unexpected collapsed native RPC error {}: {}",
                    error.code, error.message
                ))
            }
        })
}

pub(crate) fn load_app_server_runtime_status(
    cutex_session_id: &str,
) -> anyhow::Result<Option<cutex::app_server::manager::AppServerManagedRuntimeStatus>> {
    super::app_server_runtime::runtime_manager().status(cutex_session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_app::test_home::IsolatedTestHome;
    use cutex::agent_bus::model::{AgentBusAgent, AgentBusRegisterRequest};
    use cutex::config::paths::config_dir;
    use cutex::config::store::save_codez_config;
    use cutex::im::registry::load_im_registry;
    use cutex::profiles::materialize::materialized_account_files;
    use cutex::profiles::model::{AccountsStore, CliKind, RuntimeConfig, StoredAccount};
    use cutex::profiles::store::save_store;
    use cutex::session::model::{
        CutexSessionRecord, CutexSessionRuntimeBackend, CutexSessionStore,
    };
    use cutex::session::store::save_cutex_session_store;
    use serde_json::json;
    use std::fs;
    use std::io;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    fn profile(name: &str) -> StoredAccount {
        StoredAccount {
            id: format!("{name}-id"),
            name: name.to_string(),
            email: None,
            plan_type: None,
            source: Some("test".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        }
    }

    fn save_resolvable_profile(account: &StoredAccount) {
        save_store(&AccountsStore {
            version: 3,
            accounts: vec![account.clone()],
            active_account_id: Some(account.id.clone()),
        })
        .expect("save profile store");
        let files = materialized_account_files(account).expect("profile files");
        fs::create_dir_all(files.auth_path.parent().expect("profile parent"))
            .expect("create profile directory");
        fs::write(&files.auth_path, "{}\n").expect("write profile auth");
        fs::write(&files.config_path, "model = \"test\"\n").expect("write profile config");
    }

    fn save_local_session(id: &str) {
        let record = CutexSessionRecord::new_at(
            id.to_string(),
            Some("thread-management-profile".to_string()),
            cutex::platform::host::current_host_name(),
            "/tmp/management-profile".to_string(),
            None,
            "2026-08-15T00:00:00Z".to_string(),
        )
        .expect("session record");
        let mut store = CutexSessionStore::default();
        store.sessions.insert(id.to_string(), record);
        save_cutex_session_store(&store).expect("save session store");
    }

    #[cfg(unix)]
    struct ProductionHandlerFixture {
        home: IsolatedTestHome,
        bus_stop: Arc<AtomicBool>,
        bus_thread: Option<thread::JoinHandle<()>>,
        old_cutex_codex_bin: Option<std::ffi::OsString>,
        old_cutex_alden_bin: Option<std::ffi::OsString>,
        old_alden_registry: Option<std::ffi::OsString>,
    }

    #[cfg(unix)]
    impl ProductionHandlerFixture {
        fn new() -> Self {
            use cutex::http::server::{
                read_simple_http_request, write_http_response, write_json_response,
            };
            use std::os::unix::fs::PermissionsExt;

            let home = IsolatedTestHome::new("c20").expect("create isolated fixture HOME");
            let root = home.root().to_path_buf();
            let codex = root.join("fake-codex.py");
            fs::write(
                &codex,
                r##"#!/usr/bin/python3
import base64, hashlib, json, os, socket, struct, sys, time

def recv_exact(conn, count):
    data = b''
    while len(data) < count:
        part = conn.recv(count - len(data))
        if not part: return None
        data += part
    return data

def recv_frame(conn):
    head = recv_exact(conn, 2)
    if not head: return None
    opcode = head[0] & 15
    size = head[1] & 127
    if size == 126: size = struct.unpack('!H', recv_exact(conn, 2))[0]
    elif size == 127: size = struct.unpack('!Q', recv_exact(conn, 8))[0]
    mask = recv_exact(conn, 4) if (head[1] & 128) else None
    data = recv_exact(conn, size) or b''
    if mask: data = bytes(value ^ mask[index % 4] for index, value in enumerate(data))
    return opcode, data

def send_json(conn, value):
    data = json.dumps(value).encode()
    size = len(data)
    if size < 126: header = bytes([129, size])
    elif size < 65536: header = bytes([129, 126]) + struct.pack('!H', size)
    else: header = bytes([129, 127]) + struct.pack('!Q', size)
    conn.sendall(header + data)

if '--listen' not in sys.argv:
    while True: time.sleep(1)
path = sys.argv[sys.argv.index('--listen') + 1].removeprefix('unix://')
try: os.unlink(path)
except FileNotFoundError: pass
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
server.listen(4)
while True:
    conn, _ = server.accept()
    headers = b''
    while b'\r\n\r\n' not in headers: headers += conn.recv(1024)
    key = [line.split(b': ', 1)[1] for line in headers.split(b'\r\n') if line.lower().startswith(b'sec-websocket-key:')][0]
    accept = base64.b64encode(hashlib.sha1(key + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
    conn.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
    while True:
        frame = recv_frame(conn)
        if not frame: break
        opcode, payload = frame
        if opcode == 8: break
        if opcode != 1: continue
        request = json.loads(payload.decode())
        if 'id' not in request: continue
        if request.get('method') == 'initialize': result = {'userAgent': 'fixture'}
        elif request.get('method') == 'thread/resume': result = {'thread': {'id': request['params']['threadId'], 'status': {'type': 'idle'}}}
        else: result = {}
        send_json(conn, {'id': request['id'], 'result': result})
    conn.close()
"##,
            )
            .expect("write fake app-server program");
            let alden = root.join("fake-alden.sh");
            fs::write(
                &alden,
                "#!/bin/sh\nif [ \"$1\" = \"--list\" ]; then\n  if [ -f \"$CUTEX_TEST_ALDEN_REGISTRY\" ]; then\n    while read -r pid name; do\n      if kill -0 \"$pid\" 2>/dev/null; then printf '%s\\t%s\\n' \"$pid\" \"$name\"; fi\n    done < \"$CUTEX_TEST_ALDEN_REGISTRY\"\n  fi\n  exit 0\nfi\nname=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = \"--name\" ]; then name=\"$2\"; shift 2; continue; fi\n  if [ \"$1\" = \"--\" ]; then shift; break; fi\n  shift\ndone\nprintf '%s\\t%s\\n' \"$$\" \"$name\" > \"$CUTEX_TEST_ALDEN_REGISTRY\"\nexec \"$@\"\n",
            )
            .expect("write fake cute-alden program");
            fs::set_permissions(&codex, fs::Permissions::from_mode(0o700))
                .expect("make fake app-server executable");
            fs::set_permissions(&alden, fs::Permissions::from_mode(0o700))
                .expect("make fake cute-alden executable");

            let listener = (24_000..25_000)
                .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
                .expect("bind fixture Agent Bus in the permitted port range");
            listener
                .set_nonblocking(true)
                .expect("make fixture Agent Bus nonblocking");
            let port = listener
                .local_addr()
                .expect("fixture Agent Bus address")
                .port();
            let registered = Arc::new(Mutex::new(None::<AgentBusAgent>));
            let registered_for_thread = Arc::clone(&registered);
            let bus_stop = Arc::new(AtomicBool::new(false));
            let bus_stop_for_thread = Arc::clone(&bus_stop);
            let bus_thread = thread::spawn(move || {
                while !bus_stop_for_thread.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let request = read_simple_http_request(&mut stream)
                                .expect("read fixture Agent Bus request");
                            let path = request.path.split('?').next().unwrap_or(&request.path);
                            match (request.method.as_str(), path) {
                                ("GET", "/") => {
                                    write_http_response(&mut stream, 200, "OK", "text/plain", b"ok")
                                        .expect("write fixture Agent Bus health")
                                }
                                ("POST", "/api/agents/register") => {
                                    let registration: AgentBusRegisterRequest =
                                        serde_json::from_slice(&request.body)
                                            .expect("parse fixture runtime registration");
                                    *registered_for_thread
                                        .lock()
                                        .expect("fixture registration lock") =
                                        Some(AgentBusAgent {
                                            id: registration.id,
                                            // Keep the response's durable fields equal to the
                                            // fixture record. The handler still consumes the actual
                                            // registration endpoint, while this isolates the launch
                                            // occurrence from unrelated Agent Bus reconciliation.
                                            name: "fixture".to_string(),
                                            base_name: None,
                                            thread_name: None,
                                            path_key: None,
                                            session_id: registration.session_id,
                                            cutex_session_id: None,
                                            profile: registration.profile,
                                            cwd: registration.cwd,
                                            pid: registration.pid,
                                            host_id: registration.host_id,
                                            groups: Vec::new(),
                                            registration_class: registration.registration_class,
                                            last_seen_epoch_secs: 1,
                                        });
                                    write_json_response(
                                        &mut stream,
                                        200,
                                        "OK",
                                        &json!({ "ok": true }),
                                    )
                                    .expect("write fixture runtime registration");
                                }
                                ("GET", "/api/agents") => {
                                    let agents = registered_for_thread
                                        .lock()
                                        .expect("fixture registration lock")
                                        .clone()
                                        .into_iter()
                                        .collect::<Vec<_>>();
                                    write_json_response(
                                        &mut stream,
                                        200,
                                        "OK",
                                        &serde_json::to_value(agents)
                                            .expect("serialize fixture agents"),
                                    )
                                    .expect("write fixture agents");
                                }
                                ("GET", "/api/messages/poll") => write_json_response(
                                    &mut stream,
                                    200,
                                    "OK",
                                    &json!({ "messages": [] }),
                                )
                                .expect("write fixture poll"),
                                ("POST", "/api/agents/unregister") => write_json_response(
                                    &mut stream,
                                    200,
                                    "OK",
                                    &json!({ "ok": true, "removed": true }),
                                )
                                .expect("write fixture unregister"),
                                _ => write_json_response(
                                    &mut stream,
                                    200,
                                    "OK",
                                    &json!({ "ok": true }),
                                )
                                .expect("write fixture Agent Bus response"),
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("fixture Agent Bus accept failed: {error}"),
                    }
                }
            });

            let old_cutex_codex_bin = std::env::var_os("CUTEX_CODEX_BIN");
            let old_cutex_alden_bin = std::env::var_os("CUTEX_ALDEN_BIN");
            let old_alden_registry = std::env::var_os("CUTEX_TEST_ALDEN_REGISTRY");
            unsafe {
                std::env::set_var("CUTEX_CODEX_BIN", &codex);
                std::env::set_var("CUTEX_ALDEN_BIN", &alden);
                std::env::set_var("CUTEX_TEST_ALDEN_REGISTRY", root.join("alden-registry"));
            }
            let config = cutex::profiles::model::CodezConfig {
                agent_bus_enabled: true,
                agent_bus_port: Some(port),
                ..Default::default()
            };
            save_codez_config(&config).expect("save fixture config");
            Self {
                home,
                bus_stop,
                bus_thread: Some(bus_thread),
                old_cutex_codex_bin,
                old_cutex_alden_bin,
                old_alden_registry,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for ProductionHandlerFixture {
        fn drop(&mut self) {
            self.bus_stop.store(true, Ordering::Release);
            let _ = std::net::TcpStream::connect(("127.0.0.1", 9));
            if let Some(thread) = self.bus_thread.take() {
                let _ = thread.join();
            }
            unsafe {
                match self.old_cutex_codex_bin.take() {
                    Some(value) => std::env::set_var("CUTEX_CODEX_BIN", value),
                    None => std::env::remove_var("CUTEX_CODEX_BIN"),
                }
                match self.old_cutex_alden_bin.take() {
                    Some(value) => std::env::set_var("CUTEX_ALDEN_BIN", value),
                    None => std::env::remove_var("CUTEX_ALDEN_BIN"),
                }
                match self.old_alden_registry.take() {
                    Some(value) => std::env::set_var("CUTEX_TEST_ALDEN_REGISTRY", value),
                    None => std::env::remove_var("CUTEX_TEST_ALDEN_REGISTRY"),
                }
            }
            // `user_input_repository` is process-global. Keep this disposable
            // fixture root alive for the rest of the test process after the
            // first production launch initializes that repository.
            self.home.retain_root();
        }
    }

    #[cfg(unix)]
    fn save_runtime_session(
        id: &str,
        profile: Option<&str>,
        backend: CutexSessionRuntimeBackend,
    ) -> CutexSessionRecord {
        let cwd = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("fixture HOME is set")
            .join("work");
        fs::create_dir_all(&cwd).expect("create fixture runtime working directory");
        let mut record = CutexSessionRecord::new_at(
            id.to_string(),
            Some(format!("thread-{id}")),
            cutex::platform::host::current_host_name(),
            cwd.display().to_string(),
            profile.map(str::to_string),
            "2026-08-15T00:00:00Z".to_string(),
        )
        .expect("create runtime session");
        record.runtime_backend = backend;
        record.display_name_hint = Some("fixture".to_string());
        record.agent_enabled = true;
        let codex_home =
            cutex::config::paths::host_codex_home_dir().expect("resolve fixture Codex home");
        fs::create_dir_all(&codex_home).expect("create fixture Codex home");
        fs::write(
            codex_home.join("session_index.jsonl"),
            format!(
                "{{\"id\":\"{}\",\"timestamp\":\"2026-08-15T00:00:00Z\"}}\n",
                record
                    .codex_session_id
                    .as_deref()
                    .expect("fixture Codex session id")
            ),
        )
        .expect("write fixture Codex session index");
        let mut store = load_cutex_session_store().expect("load fixture session store");
        store.sessions.clear();
        store.sessions.insert(id.to_string(), record.clone());
        save_cutex_session_store(&store).expect("save runtime session");
        record
    }

    #[cfg(unix)]
    fn assert_production_receipt(
        result: &serde_json::Value,
        selected: &str,
        source: &str,
        scope: &str,
    ) {
        assert_eq!(result["launchProfile"]["selected"], selected);
        assert_eq!(result["launchProfile"]["effective"], selected);
        assert_eq!(result["launchProfile"]["source"], source);
        assert_eq!(result["launchProfile"]["applicationScope"], scope);
        assert_eq!(result["launchProfile"]["persisted"], false);
    }

    #[cfg(unix)]
    fn stop_production_runtime(id: &str, generation: u64) {
        mutate_management_v2_session(
            id,
            "cutex/runtime/offline",
            json!({ "expectedRuntimeGeneration": generation, "force": true }),
        )
        .expect("stop fixture runtime");
    }

    #[test]
    fn one_launch_profile_application_scope_tracks_only_started_processes() {
        assert_eq!(LaunchProfileApplication::default().scope(), None);
        assert_eq!(
            LaunchProfileApplication {
                runtime: true,
                tui: false,
            }
            .scope(),
            Some("runtime")
        );
        assert_eq!(
            LaunchProfileApplication {
                runtime: false,
                tui: true,
            }
            .scope(),
            Some("tui")
        );
        assert_eq!(
            LaunchProfileApplication {
                runtime: true,
                tui: true,
            }
            .scope(),
            Some("runtime_and_tui")
        );
    }

    #[test]
    fn one_launch_profile_receipt_is_explicitly_non_persistent() {
        assert_eq!(
            LaunchProfileApplication {
                runtime: true,
                tui: false,
            }
            .receipt("profile-id", "beta", "one_launch_override"),
            Some(json!({
                "requested": "profile-id",
                "selected": "beta",
                "effective": "beta",
                "source": "one_launch_override",
                "applicationScope": "runtime",
                "persisted": false,
            }))
        );
        assert!(LaunchProfileApplication::default()
            .receipt("beta", "beta", "one_launch_override")
            .is_none());
    }

    #[test]
    fn legacy_tui_only_receipt_keeps_unknown_provenance() {
        assert_eq!(
            LaunchProfileApplication {
                runtime: false,
                tui: true,
            }
            .receipt("legacy-profile", "legacy-profile", "unknown"),
            Some(json!({
                "requested": "legacy-profile",
                "selected": "legacy-profile",
                "effective": "legacy-profile",
                "source": "unknown",
                "applicationScope": "tui",
                "persisted": false,
            }))
        );
    }

    #[cfg(unix)]
    #[test]
    fn production_runtime_handler_constructs_runtime_tui_and_combined_profile_receipts() {
        let _fixture = ProductionHandlerFixture::new();
        let alpha = profile("alpha");
        let beta = profile("beta");
        save_resolvable_profile(&alpha);
        save_resolvable_profile(&beta);
        save_store(&AccountsStore {
            version: 3,
            accounts: vec![alpha, beta],
            active_account_id: Some("beta-id".to_string()),
        })
        .expect("save fixture profile store");
        let mut config = cutex::config::store::load_codez_config();
        config.default_profile = Some("beta".to_string());
        save_codez_config(&config).expect("set fixture global default");

        save_runtime_session(
            "cutex.runtime.session-profile",
            Some("alpha"),
            CutexSessionRuntimeBackend::Host,
        );
        let session_profile = mutate_management_v2_runtime(
            "cutex.runtime.session-profile",
            "cutex/runtime/online",
            &json!({ "expectedRuntimeGeneration": 0, "openVisibleTerminal": false }),
        )
        .expect("start session-configured runtime through production handler");
        assert_production_receipt(&session_profile, "alpha", "session_configured", "runtime");
        stop_production_runtime("cutex.runtime.session-profile", 1);

        save_runtime_session(
            "cutex.runtime.global-profile",
            None,
            CutexSessionRuntimeBackend::Host,
        );
        let global_profile = mutate_management_v2_runtime(
            "cutex.runtime.global-profile",
            "cutex/runtime/online",
            &json!({ "expectedRuntimeGeneration": 0, "openVisibleTerminal": false }),
        )
        .expect("start global-default runtime through production handler");
        assert_production_receipt(&global_profile, "beta", "global_default", "runtime");
        stop_production_runtime("cutex.runtime.global-profile", 1);

        save_runtime_session(
            "cutex.runtime.combined",
            Some("alpha"),
            CutexSessionRuntimeBackend::CuteAlden,
        );
        let combined = mutate_management_v2_session(
            "cutex.runtime.combined",
            "cutex/runtime/online",
            json!({ "expectedRuntimeGeneration": 0, "openVisibleTerminal": false }),
        )
        .expect("start combined runtime and TUI through production session handler");
        assert_production_receipt(&combined, "alpha", "session_configured", "runtime_and_tui");
        stop_production_runtime("cutex.runtime.combined", 1);

        save_runtime_session(
            "cutex.runtime.legacy-tui",
            Some("alpha"),
            CutexSessionRuntimeBackend::Host,
        );
        let runtime = mutate_management_v2_runtime(
            "cutex.runtime.legacy-tui",
            "cutex/runtime/online",
            &json!({ "expectedRuntimeGeneration": 0, "openVisibleTerminal": false }),
        )
        .expect("start fixture runtime before TUI-only restoration");
        assert_production_receipt(&runtime, "alpha", "session_configured", "runtime");
        let mut store = load_cutex_session_store().expect("load fixture runtime binding");
        let record = store
            .sessions
            .get_mut("cutex.runtime.legacy-tui")
            .expect("fixture session exists");
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record
            .app_server_runtime
            .as_mut()
            .expect("fixture runtime binding exists")
            .launch_profile_source = None;
        save_cutex_session_store(&store).expect("save legacy TUI fixture");
        let legacy_tui = mutate_management_v2_session(
            "cutex.runtime.legacy-tui",
            "cutex/runtime/online",
            json!({ "expectedRuntimeGeneration": 1, "openVisibleTerminal": false }),
        )
        .expect("restore legacy TUI through production session handler");
        assert_production_receipt(&legacy_tui, "alpha", "unknown", "tui");
        stop_production_runtime("cutex.runtime.legacy-tui", 1);
    }

    #[cfg(unix)]
    #[test]
    fn production_one_launch_override_leaves_durable_configured_intent_unchanged() {
        let _fixture = ProductionHandlerFixture::new();
        let alpha = profile("alpha");
        let beta = profile("beta");
        save_resolvable_profile(&alpha);
        save_resolvable_profile(&beta);
        save_store(&AccountsStore {
            version: 3,
            accounts: vec![alpha, beta],
            active_account_id: Some("beta-id".to_string()),
        })
        .expect("save fixture profile store");
        save_runtime_session(
            "cutex.runtime.one-launch",
            Some("beta"),
            CutexSessionRuntimeBackend::Host,
        );
        let before = load_cutex_session_store()
            .expect("load configured intent")
            .sessions["cutex.runtime.one-launch"]
            .clone();
        let one_launch = mutate_management_v2_runtime(
            "cutex.runtime.one-launch",
            "cutex/runtime/online",
            &json!({
                "expectedRuntimeGeneration": 0,
                "openVisibleTerminal": false,
                "launchProfile": "alpha"
            }),
        )
        .expect("apply one-launch override through production handler");
        assert_production_receipt(&one_launch, "alpha", "one_launch_override", "runtime");
        let after = load_cutex_session_store()
            .expect("reload durable configured intent")
            .sessions["cutex.runtime.one-launch"]
            .clone();
        assert_eq!(after.profile, before.profile);
        assert_eq!(after.durable_revision(), before.durable_revision());
        stop_production_runtime("cutex.runtime.one-launch", 1);
    }

    #[test]
    fn management_profile_handler_uses_durable_store_and_reports_im_write_uncertainty() {
        let _home = IsolatedTestHome::new("cmp").expect("create isolated HOME");
        let id = "cutex.management-profile";
        let account = profile("alpha");
        save_resolvable_profile(&account);
        save_local_session(id);
        let initial_revision =
            load_cutex_session_store().expect("load session").sessions[id].durable_revision();

        let set = mutate_management_v2_session(
            id,
            "cutex/session/profile/set",
            json!({ "expectedRevision": initial_revision, "profile": "alpha" }),
        )
        .expect("set configured profile");
        assert_eq!(set["configuredProfile"], "alpha");
        let configured = load_cutex_session_store()
            .expect("load configured session")
            .sessions[id]
            .clone();
        assert_eq!(configured.profile.as_deref(), Some("alpha"));
        assert_eq!(
            load_im_registry()
                .expect("load mirrored IM record")
                .sessions["thread-management-profile"]
                .profile
                .as_deref(),
            Some("alpha")
        );

        let stale = mutate_management_v2_session(
            id,
            "cutex/session/profile/clear",
            json!({ "expectedRevision": initial_revision }),
        )
        .expect_err("stale revision must be rejected");
        assert_eq!(stale.code, "revision_conflict");
        assert_eq!(stale.details["resyncRequired"], true);
        assert_eq!(
            load_cutex_session_store()
                .expect("reload after stale request")
                .sessions[id]
                .profile,
            Some("alpha".to_string())
        );

        let im_registry_path = config_dir().expect("config dir").join("im-sessions.json");
        fs::remove_file(&im_registry_path).expect("remove IM registry for failure fixture");
        fs::create_dir(&im_registry_path).expect("replace IM registry with failure fixture");
        let uncertain = mutate_management_v2_session(
            id,
            "cutex/session/profile/clear",
            json!({ "expectedRevision": configured.durable_revision() }),
        )
        .expect_err("IM persistence failure must surface as outcome unknown");
        assert_eq!(uncertain.code, "persistence_uncertain");
        assert!(!uncertain.retryable);
        assert!(uncertain.outcome_unknown);
        assert_eq!(uncertain.details["resyncRequired"], true);
        assert_eq!(
            load_cutex_session_store()
                .expect("durable store persists before IM failure")
                .sessions[id]
                .profile,
            None
        );
        fs::remove_dir(im_registry_path).expect("remove IM failure fixture");
    }

    #[test]
    fn failed_same_binding_recovery_returns_typed_bounded_cutover() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "host-a".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-08-07T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.runtime_generation = 7;

        let error = runtime_cutover_required(&record, "same_binding_reconnect_failed", Some(4242));

        assert_eq!(error.stage, "runtime");
        assert_eq!(error.code, "cutover_required");
        assert!(!error.retryable);
        assert!(!error.outcome_unknown);
        assert_eq!(error.details["runtimeGeneration"], 7);
        assert_eq!(error.details["runtimePid"], 4242);
        assert_eq!(error.details["recoveryAttempts"], 1);
        assert_eq!(error.details["requiredAction"], "cutex/runtime/offline");
    }

    #[test]
    fn windows_terminal_runtime_actions_use_force_termination() {
        assert!(runtime_stop_force_policy(
            "cutex/runtime/offline",
            false,
            true
        ));
        assert!(runtime_stop_force_policy(
            "cutex/runtime/close",
            false,
            true
        ));
        assert!(!runtime_stop_force_policy(
            "cutex/runtime/close",
            false,
            false
        ));
        assert!(runtime_stop_force_policy(
            "cutex/runtime/close",
            true,
            false
        ));
    }
}
