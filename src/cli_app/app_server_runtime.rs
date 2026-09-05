use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::{fs, path::Path};

use anyhow::Context;

use cutex::agent_bus::client::AgentBusHttpClient;
use cutex::agent_bus::groups::normalize_registered_agent_groups;
use cutex::agent_bus::identity::fnv1a_hex;
use cutex::agent_bus::identity::sanitize_session_component;
use cutex::agent_bus::model::AgentBusRegisterRequest;
#[cfg(not(test))]
use cutex::app_server::activity_bridge::spawn_activity_projector;
use cutex::app_server::bus_bridge::AppServerAgentBusBridgeOptions;
use cutex::app_server::client::AppServerEndpoint;
use cutex::app_server::manager::AppServerRuntimeConnectResult;
use cutex::app_server::manager::AppServerRuntimeEventContext;
use cutex::app_server::manager::AppServerRuntimeManager;
use cutex::app_server::manager::AppServerRuntimeStartResult;
use cutex::app_server::runtime::cleanup_runtime_binding_files;
use cutex::app_server::runtime::thread_resume_params_for_session_with_model_provider;
use cutex::app_server::runtime::thread_start_params_for_session;
use cutex::app_server::runtime::AppServerRuntimeLayout;
use cutex::management::v2::activity::record_activity_event;
use cutex::management::v2::model::AppServerSchema;
use cutex::management::v2::model::AppServerSchemaChannel;
use cutex::management::v2::native_events::pending_event_from_app_server;
use cutex::management::v2::native_events::NativeEventContext;
use cutex::management::v2::native_events::NativeEventDisposition;
use cutex::management::v2::repository::management_v2_repository;
use cutex::management::v2::usage::record_usage_event;
use cutex::platform::host::current_host_name;
use cutex::platform::process::{process_is_running, process_started_at};
use cutex::profiles::model::CodezConfig;
use cutex::runtime::alden::find_live_cute_alden_session_by_name;
use cutex::runtime::lifecycle::cutex_session_host_is_local;
use cutex::runtime::process_scope::terminate_managed_agent_scope;
use cutex::session::model::CutexAppServerRuntimeBinding;
use cutex::session::model::CutexAppServerTransport;
use cutex::session::model::CutexSessionRecord;
use cutex::session::service::clear_cutex_session_runtime_record;
use cutex::session::service::cutex_session_display_name;
use cutex::session::service::cutex_session_key_for_user_id;
use cutex::session::service::cutex_session_launch_cwd;
use cutex::session::service::persist_cutex_session_store_and_im_record;
use cutex::session::store::load_cutex_session_store;

use super::app_server_state_sync;
use super::app_server_user_input;
use cutex::management::v2::server_requests as app_server_pending_requests;

static APP_SERVER_RUNTIME_MANAGER: OnceLock<AppServerRuntimeManager> = OnceLock::new();
static APP_SERVER_BACKGROUND_DIAGNOSTICS: AtomicBool = AtomicBool::new(true);

pub(crate) fn suppress_background_diagnostics() {
    APP_SERVER_BACKGROUND_DIAGNOSTICS.store(false, Ordering::Release);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeEventSinkDisposition {
    Published,
    IgnoredForeignThreadNotification,
}

fn handle_runtime_native_event(
    context: &AppServerRuntimeEventContext,
    event: &cutex::app_server::client::AppServerEvent,
    publish: impl FnOnce(cutex::management::v2::model::PendingEvent) -> anyhow::Result<()>,
) -> anyhow::Result<RuntimeEventSinkDisposition> {
    let disposition = pending_event_from_app_server(
        &NativeEventContext {
            cutex_session_id: context.cutex_session_id.clone(),
            thread_id: context.thread_id.clone(),
            host_id: current_host_name(),
            runtime_generation: context.runtime_generation,
            runtime_backend: context.runtime_backend.clone(),
            schema: AppServerSchema {
                protocol: "codex-app-server".to_string(),
                major_version: 2,
                version: context.schema.version.clone(),
                sha256: context.schema.sha256.clone(),
                channel: AppServerSchemaChannel::Experimental,
                capabilities: serde_json::json!({ "experimentalApi": true }),
                extensions: vec!["cutex-inter-agent-v2".to_string()],
            },
        },
        event,
    )?;
    match disposition {
        NativeEventDisposition::Publish(pending) => {
            publish(pending)?;
            Ok(RuntimeEventSinkDisposition::Published)
        }
        NativeEventDisposition::IgnoreForeignThreadNotification => {
            Ok(RuntimeEventSinkDisposition::IgnoredForeignThreadNotification)
        }
    }
}

pub(crate) fn runtime_manager() -> &'static AppServerRuntimeManager {
    APP_SERVER_RUNTIME_MANAGER.get_or_init(|| {
        let manager = AppServerRuntimeManager::new(Arc::new(
            |context: &AppServerRuntimeEventContext,
             event: &cutex::app_server::client::AppServerEvent| {
                handle_runtime_native_event(context, event, |pending| {
                    let synchronize_before_publish = matches!(
                        event,
                        cutex::app_server::client::AppServerEvent::ServerRequest(_)
                    ) || matches!(
                        event,
                        cutex::app_server::client::AppServerEvent::Notification(notification)
                            if notification.method == "serverRequest/resolved"
                    );
                    if synchronize_before_publish {
                        app_server_state_sync::handle_runtime_event(context, event)?;
                    }
                    let envelope = management_v2_repository()?.append(pending)?;
                    if let Err(err) = record_activity_event(&envelope) {
                        eprintln!(
                            "\x1b[33mwarning:\x1b[0m failed to update session activity: {err:#}"
                        );
                    }
                    if let Err(err) =
                        record_usage_event(&envelope, context.launched_profile.as_deref())
                    {
                        eprintln!(
                            "\x1b[33mwarning:\x1b[0m failed to update session usage: {err:#}"
                        );
                    }
                    if !synchronize_before_publish {
                        app_server_state_sync::handle_runtime_event(context, event)?;
                    }
                    app_server_user_input::handle_runtime_event(&context.cutex_session_id, event)?;
                    Ok(())
                })?;
                Ok(())
            },
        ));
        #[cfg(not(test))]
        {
            let emit_background_diagnostics =
                APP_SERVER_BACKGROUND_DIAGNOSTICS.load(Ordering::Acquire);
            if let Err(error) =
                spawn_activity_projector(manager.clone(), emit_background_diagnostics)
            {
                if emit_background_diagnostics {
                    eprintln!(
                        "\x1b[33mwarning:\x1b[0m failed to start Cutex activity projector: {error:#}"
                    );
                }
            }
        }
        manager
    })
}

pub(crate) fn connect_runtime(
    config: &CodezConfig,
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    runtime_agent_id: &str,
) -> anyhow::Result<AppServerRuntimeConnectResult> {
    connect_runtime_with_model_provider(config, record, binding, runtime_agent_id, None)
}

pub(crate) fn connect_runtime_with_model_provider(
    config: &CodezConfig,
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    runtime_agent_id: &str,
    model_provider: Option<&str>,
) -> anyhow::Result<AppServerRuntimeConnectResult> {
    if record.is_retired() {
        anyhow::bail!(
            "cannot connect a runtime for retired cutex session {}",
            record.cutex_session_id
        );
    }
    let manager = runtime_manager();
    let resume = thread_resume_params_for_session_with_model_provider(record, model_provider)?;
    let runtime_backend = serde_json::to_value(record.runtime_backend)?
        .as_str()
        .context("runtime backend did not serialize as a string")?
        .to_string();
    let connected = manager.connect_binding(
        &record.cutex_session_id,
        binding,
        resume,
        record.runtime_generation,
        &runtime_backend,
    )?;
    let registration = runtime_agent_registration(record, binding, runtime_agent_id)?;
    if let Err(error) = manager.start_agent_bus_bridge(
        &record.cutex_session_id,
        Arc::new(AgentBusHttpClient::from_config(config)),
        AppServerAgentBusBridgeOptions::new(
            registration,
            record.codex_session_id.clone().unwrap_or_default(),
        )
        .with_cutex_session_id(record.cutex_session_id.clone()),
    ) {
        let _ = manager.disconnect(&record.cutex_session_id);
        return Err(error);
    }
    if let Err(error) = app_server_user_input::flush_queued_if_idle(&record.cutex_session_id) {
        let _ = manager.disconnect(&record.cutex_session_id);
        return Err(error.context("failed to flush queued app-server user input"));
    }
    Ok(connected)
}

pub(crate) fn connect_new_thread_runtime(
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    developer_instructions: Option<String>,
) -> anyhow::Result<AppServerRuntimeStartResult> {
    if record.is_retired() {
        anyhow::bail!(
            "cannot connect a runtime for retired cutex session {}",
            record.cutex_session_id
        );
    }
    let start = thread_start_params_for_session(record, developer_instructions)?;
    let runtime_backend = serde_json::to_value(record.runtime_backend)?
        .as_str()
        .context("runtime backend did not serialize as a string")?
        .to_string();
    runtime_manager().connect_new_thread_binding(
        &record.cutex_session_id,
        binding,
        start,
        record.runtime_generation,
        &runtime_backend,
    )
}

pub(crate) fn start_connected_runtime_bridge(
    config: &CodezConfig,
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    runtime_agent_id: &str,
) -> anyhow::Result<()> {
    let thread_id = record
        .codex_session_id
        .clone()
        .context("new runtime record has no persisted thread id")?;
    let registration = runtime_agent_registration(record, binding, runtime_agent_id)?;
    runtime_manager().start_agent_bus_bridge(
        &record.cutex_session_id,
        Arc::new(AgentBusHttpClient::from_config(config)),
        AppServerAgentBusBridgeOptions::new(registration, thread_id)
            .with_cutex_session_id(record.cutex_session_id.clone()),
    )?;
    Ok(())
}

pub(crate) fn disconnect_runtime(cutex_session_id: &str) -> anyhow::Result<bool> {
    let disconnected = runtime_manager().disconnect(cutex_session_id)?;
    app_server_pending_requests::clear_session(cutex_session_id)?;
    Ok(disconnected)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct AppServerAdoptionSummary {
    pub(crate) adopted: usize,
    pub(crate) cleared_stale: usize,
    pub(crate) skipped: usize,
    pub(crate) failures: Vec<String>,
}

/// Decision made before `cutex/runtime/online` is allowed to create a new
/// manager-owned app-server occurrence.  A persisted binding is an ownership
/// claim, not disposable error state: a live claim must either reconnect or
/// stop with an explicit cutover requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistedRuntimeRecoveryAction {
    Launch,
    Reconnect {
        runtime_agent_id: String,
    },
    ClearStaleAndLaunch,
    CutoverRequired {
        reason: &'static str,
        pid: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedRuntimeRecoveryOutcome {
    NoClaim,
    RecoveredExact,
    ClearedDeadClaim,
}

const MAX_PROCESS_BINDING_START_SKEW: chrono::Duration = chrono::Duration::seconds(10);

/// Provider lifecycle recovery for one exact durable ownership claim. This
/// function never launches a child: it either reconnects the proven existing
/// occurrence, CAS-clears a conclusively dead claim, or fails closed.
pub(crate) fn recover_persisted_runtime_for_lifecycle(
    config: &CodezConfig,
    expected: &CutexSessionRecord,
) -> anyhow::Result<ManagedRuntimeRecoveryOutcome> {
    match classify_local_persisted_runtime_recovery(expected, process_is_running) {
        PersistedRuntimeRecoveryAction::Launch => Ok(ManagedRuntimeRecoveryOutcome::NoClaim),
        PersistedRuntimeRecoveryAction::ClearStaleAndLaunch => {
            let scope_stop = terminate_managed_agent_scope(&expected.cutex_session_id, true)
                .context("failed to clean a stale managed Agent process scope")?;
            if !scope_stop.stopped {
                anyhow::bail!(
                    "stale managed Agent process scope remained active: {}",
                    scope_stop.detail
                );
            }
            clear_stale_persisted_runtime(expected)?;
            Ok(ManagedRuntimeRecoveryOutcome::ClearedDeadClaim)
        }
        PersistedRuntimeRecoveryAction::CutoverRequired { reason, pid } => {
            anyhow::bail!(
                "runtime recovery requires owner action ({reason}, pid={})",
                pid.map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )
        }
        PersistedRuntimeRecoveryAction::Reconnect { runtime_agent_id } => {
            let binding = expected
                .app_server_runtime
                .as_ref()
                .context("runtime reconnect claim omitted its app-server binding")?;
            verify_exact_live_runtime_claim(expected, binding)?;
            let already_connected = runtime_manager()
                .status(&expected.cutex_session_id)?
                .is_some_and(|status| status.connected);
            if already_connected {
                if runtime_manager()
                    .agent_bus_bridge_status(&expected.cutex_session_id)?
                    .is_none()
                {
                    start_connected_runtime_bridge(config, expected, binding, &runtime_agent_id)?;
                }
            } else if let Err(error) = connect_runtime(config, expected, binding, &runtime_agent_id)
            {
                return Err(error.context("failed to reconnect exact claimed runtime"));
            }
            let verification = verify_recovered_runtime_state(expected, binding, &runtime_agent_id);
            if let Err(error) = verification {
                if already_connected {
                    return Err(error);
                }
                let disconnect = disconnect_runtime(&expected.cutex_session_id);
                return Err(match disconnect {
                    Ok(_) => error,
                    Err(disconnect_error) => error.context(format!(
                        "failed to disconnect rejected recovered runtime: {disconnect_error:#}"
                    )),
                });
            }
            Ok(ManagedRuntimeRecoveryOutcome::RecoveredExact)
        }
    }
}

fn verify_recovered_runtime_state(
    expected: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    runtime_agent_id: &str,
) -> anyhow::Result<()> {
    if !persisted_runtime_ownership_matches(expected)? {
        anyhow::bail!("durable runtime ownership changed during reconnect");
    }
    verify_exact_live_runtime_claim(expected, binding)?;
    let status = runtime_manager()
        .status(&expected.cutex_session_id)?
        .context("reconnected runtime is absent from the manager")?;
    if !status.connected
        || status.runtime_generation != expected.runtime_generation
        || expected.codex_session_id.as_deref() != Some(status.thread_id.as_str())
    {
        anyhow::bail!("reconnected runtime identity/generation does not match the durable claim");
    }
    let registration = runtime_manager()
        .refresh_agent_bus_registration(&expected.cutex_session_id)?
        .context("reconnected runtime has no Agent Bus bridge")?;
    if !registration.registered || registration.runtime_agent_id != runtime_agent_id {
        anyhow::bail!("reconnected runtime Agent Bus identity is not exact");
    }
    Ok(())
}

fn verify_exact_live_runtime_claim(
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
) -> anyhow::Result<()> {
    let process_started_at = process_started_at(binding.pid)?;
    let endpoint_owned = binding_endpoint_is_owned_by_process(binding, binding.pid)?;
    validate_exact_live_runtime_claim_evidence(record, binding, process_started_at, endpoint_owned)
}

fn validate_exact_live_runtime_claim_evidence(
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    process_started_at: chrono::DateTime<chrono::Utc>,
    endpoint_owned: bool,
) -> anyhow::Result<()> {
    if record.runtime_generation == 0 {
        anyhow::bail!("live runtime claim has no generation");
    }
    if record.app_server_launch_claim_id.is_some() {
        anyhow::bail!("live runtime claim overlaps an unresolved launch claim");
    }
    if record.runtime_pid != Some(binding.pid) || binding.pid == 0 {
        anyhow::bail!("live runtime PID does not match the durable binding");
    }
    if !record
        .current_runtime_agent_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        anyhow::bail!("live runtime claim has no exact runtime endpoint identity");
    }
    let binding_started_at = chrono::DateTime::parse_from_rfc3339(&binding.started_at)
        .context("runtime binding started_at is invalid")?
        .with_timezone(&chrono::Utc);
    let start_skew = binding_started_at.signed_duration_since(process_started_at);
    if start_skew < chrono::Duration::zero() || start_skew > MAX_PROCESS_BINDING_START_SKEW {
        anyhow::bail!("runtime PID creation time does not match the durable binding");
    }
    if !endpoint_owned {
        anyhow::bail!("runtime endpoint is not live and owned by the claimed PID");
    }
    Ok(())
}

fn binding_endpoint_is_owned_by_process(
    binding: &CutexAppServerRuntimeBinding,
    pid: u32,
) -> anyhow::Result<bool> {
    let layout = AppServerRuntimeLayout::from_binding(binding)?;
    if !layout.endpoint_ready() {
        return Ok(false);
    }
    match binding.transport {
        CutexAppServerTransport::UnixSocket => {
            #[cfg(target_os = "linux")]
            {
                let socket_path = binding
                    .endpoint
                    .strip_prefix("unix://")
                    .context("runtime Unix endpoint is invalid")?;
                return linux_unix_socket_is_owned_by_process(Path::new(socket_path), pid);
            }
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("Unix socket PID ownership verification is unsupported on this platform")
        }
        CutexAppServerTransport::LoopbackWebSocket => {
            #[cfg(windows)]
            {
                return windows_loopback_listener_is_owned_by_process(&binding.endpoint, pid);
            }
            #[cfg(not(windows))]
            anyhow::bail!(
                "loopback WebSocket PID ownership verification is unsupported on this platform"
            )
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_unix_socket_is_owned_by_process(path: &Path, pid: u32) -> anyhow::Result<bool> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect runtime socket {}", path.display()))?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Ok(false);
    }
    let socket_path = path.to_string_lossy();
    let unix_sockets = fs::read_to_string("/proc/net/unix")?;
    // `/proc/net/unix` may repeat the bound pathname on accepted stream
    // sockets. Only the stream row in the unconnected/listening state owns
    // the endpoint; connected rows must not participate in listener identity.
    let mut listener_inodes = unix_sockets
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let flags = u32::from_str_radix(fields.get(3)?, 16).ok()?;
            (fields.get(7).copied() == Some(socket_path.as_ref())
                && fields.get(4).copied() == Some("0001")
                && fields.get(5).copied() == Some("01")
                && flags & 0x0001_0000 != 0)
                .then(|| fields.get(6).copied())
                .flatten()
        })
        .collect::<Vec<_>>();
    listener_inodes.sort_unstable();
    listener_inodes.dedup();
    let [inode] = listener_inodes.as_slice() else {
        return Ok(false);
    };
    let expected_link = format!("socket:[{inode}]");
    let fd_dir = format!("/proc/{pid}/fd");
    for entry in fs::read_dir(&fd_dir)
        .with_context(|| format!("failed to inspect runtime process descriptors {fd_dir}"))?
    {
        let Ok(target) = fs::read_link(entry?.path()) else {
            continue;
        };
        if target.to_string_lossy() == expected_link {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(windows)]
fn windows_loopback_listener_is_owned_by_process(url: &str, pid: u32) -> anyhow::Result<bool> {
    use std::process::{Command, Stdio};

    let url = url::Url::parse(url).context("runtime loopback endpoint is invalid")?;
    let host = url
        .host_str()
        .context("runtime endpoint omitted its host")?;
    if !matches!(host, "127.0.0.1" | "::1" | "localhost") {
        anyhow::bail!("runtime endpoint is not loopback");
    }
    let port = url.port().context("runtime endpoint omitted its port")?;
    let script = format!(
        "Get-NetTCPConnection -State Listen -LocalPort {port} -ErrorAction Stop | Select-Object -ExpandProperty OwningProcess"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .stdin(Stdio::null())
        .output()
        .context("failed to query loopback listener ownership")?;
    if !output.status.success() {
        anyhow::bail!("loopback listener ownership query failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .any(|owner| owner == pid))
}

pub(crate) fn classify_persisted_runtime_recovery(
    record: &CutexSessionRecord,
    process_is_live: impl Fn(u32) -> bool,
) -> PersistedRuntimeRecoveryAction {
    let binding_pid = record
        .app_server_runtime
        .as_ref()
        .map(|binding| binding.pid);
    let binding_is_live = binding_pid.is_some_and(&process_is_live);
    let runtime_pid_is_live = record.runtime_pid.is_some_and(&process_is_live);
    let alden_pid_is_live = record.alden_pid.is_some_and(&process_is_live);
    let live_child_pid = binding_pid
        .filter(|_| binding_is_live)
        .or(record.runtime_pid.filter(|_| runtime_pid_is_live))
        .or(record.alden_pid.filter(|_| alden_pid_is_live));

    if record
        .app_server_launch_claim_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return PersistedRuntimeRecoveryAction::CutoverRequired {
            reason: "pending_launch_ownership_unknown",
            pid: live_child_pid,
        };
    }

    if record.app_server_runtime.is_some() {
        if binding_is_live {
            let runtime_agent_id = record
                .current_runtime_agent_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            return match runtime_agent_id {
                Some(runtime_agent_id) => PersistedRuntimeRecoveryAction::Reconnect {
                    runtime_agent_id: runtime_agent_id.to_string(),
                },
                None => PersistedRuntimeRecoveryAction::CutoverRequired {
                    reason: "live_binding_missing_runtime_agent_id",
                    pid: live_child_pid,
                },
            };
        }
        if runtime_pid_is_live || alden_pid_is_live {
            return PersistedRuntimeRecoveryAction::CutoverRequired {
                reason: "stale_binding_has_live_child",
                pid: live_child_pid,
            };
        }
        return PersistedRuntimeRecoveryAction::ClearStaleAndLaunch;
    }

    if record
        .current_runtime_agent_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return PersistedRuntimeRecoveryAction::CutoverRequired {
            reason: "binding_missing_runtime_agent_id_present",
            pid: live_child_pid.or(record.runtime_pid).or(record.alden_pid),
        };
    }
    if runtime_pid_is_live || alden_pid_is_live {
        return PersistedRuntimeRecoveryAction::CutoverRequired {
            reason: "binding_missing_live_child",
            pid: live_child_pid,
        };
    }
    if record.runtime_pid.is_some() || record.alden_pid.is_some() {
        return PersistedRuntimeRecoveryAction::ClearStaleAndLaunch;
    }
    PersistedRuntimeRecoveryAction::Launch
}

pub(crate) fn classify_local_persisted_runtime_recovery(
    record: &CutexSessionRecord,
    process_is_live: impl Fn(u32) -> bool,
) -> PersistedRuntimeRecoveryAction {
    let action = classify_persisted_runtime_recovery(record, process_is_live);
    if matches!(
        action,
        PersistedRuntimeRecoveryAction::Launch
            | PersistedRuntimeRecoveryAction::ClearStaleAndLaunch
    ) {
        if let Some(pid) = record
            .alden_session_name
            .as_deref()
            .and_then(find_live_cute_alden_session_by_name)
            .map(|session| session.pid)
        {
            return PersistedRuntimeRecoveryAction::CutoverRequired {
                reason: "stale_binding_has_live_tui",
                pid: Some(pid),
            };
        }
    }
    action
}

/// Remove only a binding that was classified stale from the same durable
/// snapshot that was inspected.  A changed generation or ownership field is a
/// concurrent lifecycle action and must block a new launch.
pub(crate) fn clear_stale_persisted_runtime(expected: &CutexSessionRecord) -> anyhow::Result<()> {
    clear_stale_persisted_runtime_if_dead(expected, process_is_running)
}

pub(crate) fn persisted_runtime_ownership_matches(
    expected: &CutexSessionRecord,
) -> anyhow::Result<bool> {
    let store = load_cutex_session_store()?;
    let Some(key) = cutex_session_key_for_user_id(&store, &expected.cutex_session_id) else {
        return Ok(false);
    };
    let Some(current) = store.sessions.get(&key) else {
        return Ok(false);
    };
    Ok(runtime_ownership_snapshot_matches(current, expected))
}

pub(crate) fn restore_recovery_snapshot_if_owned(
    expected: &CutexSessionRecord,
) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let key =
        cutex_session_key_for_user_id(&store, &expected.cutex_session_id).ok_or_else(|| {
            anyhow::anyhow!("cutex session disappeared: {}", expected.cutex_session_id)
        })?;
    let current = store
        .sessions
        .get(&key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("cutex session disappeared: {key}"))?;
    if !runtime_ownership_snapshot_matches(&current, expected) {
        anyhow::bail!("runtime ownership changed concurrently; reconnect rollback was not applied");
    }
    if current == *expected {
        return Ok(());
    }
    store.sessions.insert(key.clone(), expected.clone());
    persist_cutex_session_store_and_im_record(&store, &key)
}

fn clear_stale_persisted_runtime_if_dead(
    expected: &CutexSessionRecord,
    process_is_live: impl Fn(u32) -> bool,
) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let key =
        cutex_session_key_for_user_id(&store, &expected.cutex_session_id).ok_or_else(|| {
            anyhow::anyhow!("cutex session disappeared: {}", expected.cutex_session_id)
        })?;
    let current = store
        .sessions
        .get(&key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("cutex session disappeared: {key}"))?;
    if !runtime_ownership_snapshot_matches(&current, expected) {
        anyhow::bail!("runtime changed concurrently; stale cleanup was not applied");
    }
    if let Some(pid) = runtime_occurrence_pids(&current)
        .into_iter()
        .find(|pid| process_is_live(*pid))
    {
        anyhow::bail!("runtime process became live during stale cleanup: {pid}");
    }
    if let Some(binding) = current.app_server_runtime.as_ref() {
        let layout = AppServerRuntimeLayout::from_binding(binding)
            .context("failed to reconstruct stale app-server endpoint")?;
        if binding_endpoint_is_reachable(&layout)? {
            anyhow::bail!(
                "stale app-server endpoint remains reachable during cleanup: {}",
                binding.endpoint
            );
        }
        cleanup_runtime_binding_files(binding)?;
    }
    clear_cutex_session_runtime_record(&mut store, &key, true)?;
    persist_cutex_session_store_and_im_record(&store, &key)
}

fn binding_endpoint_is_reachable(layout: &AppServerRuntimeLayout) -> anyhow::Result<bool> {
    match layout.endpoint() {
        #[cfg(unix)]
        AppServerEndpoint::UnixSocket { socket_path } => {
            Ok(std::os::unix::net::UnixStream::connect(socket_path).is_ok())
        }
        AppServerEndpoint::LoopbackWebSocket { .. } => Ok(layout.endpoint_ready()),
    }
}

fn runtime_ownership_snapshot_matches(
    current: &CutexSessionRecord,
    expected: &CutexSessionRecord,
) -> bool {
    current.host_id == expected.host_id
        && current.runtime_backend == expected.runtime_backend
        && current.profile == expected.profile
        && current.pending_launch_id == expected.pending_launch_id
        && current.app_server_launch_claim_id == expected.app_server_launch_claim_id
        && current.runtime_generation == expected.runtime_generation
        && current.app_server_runtime == expected.app_server_runtime
        && current.current_runtime_agent_id == expected.current_runtime_agent_id
        && current.runtime_pid == expected.runtime_pid
        && current.alden_pid == expected.alden_pid
        && current.alden_session_name == expected.alden_session_name
}

fn runtime_occurrence_pids(record: &CutexSessionRecord) -> Vec<u32> {
    let mut occurrence_pids = record
        .app_server_runtime
        .as_ref()
        .map(|binding| binding.pid)
        .into_iter()
        .chain(record.runtime_pid)
        .chain(record.alden_pid)
        .filter(|pid| *pid != 0)
        .collect::<Vec<_>>();
    occurrence_pids.sort_unstable();
    occurrence_pids.dedup();
    occurrence_pids
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PersistedRuntimeAdoptionAction {
    Retired,
    Remote,
    Recover(PersistedRuntimeRecoveryAction),
}

fn classify_persisted_runtime_adoption(
    record: &CutexSessionRecord,
    current_host: &str,
    process_is_live: impl Fn(u32) -> bool,
) -> PersistedRuntimeAdoptionAction {
    if record.is_retired() {
        return PersistedRuntimeAdoptionAction::Retired;
    }
    if !cutex_session_host_is_local(&record.host_id, current_host) {
        return PersistedRuntimeAdoptionAction::Remote;
    }
    PersistedRuntimeAdoptionAction::Recover(classify_local_persisted_runtime_recovery(
        record,
        process_is_live,
    ))
}

pub(crate) fn adopt_persisted_runtimes(
    config: &CodezConfig,
    current_host: &str,
) -> anyhow::Result<AppServerAdoptionSummary> {
    let store = load_cutex_session_store()?;
    let mut summary = AppServerAdoptionSummary::default();
    let session_keys = store.sessions.keys().cloned().collect::<Vec<_>>();
    for key in session_keys {
        let Some(record) = store.sessions.get(&key).cloned() else {
            continue;
        };
        match classify_persisted_runtime_adoption(&record, current_host, process_is_running) {
            PersistedRuntimeAdoptionAction::Retired | PersistedRuntimeAdoptionAction::Remote => {
                summary.skipped = summary.skipped.saturating_add(1);
                continue;
            }
            PersistedRuntimeAdoptionAction::Recover(
                PersistedRuntimeRecoveryAction::ClearStaleAndLaunch,
            ) => {
                match clear_stale_persisted_runtime(&record) {
                    Ok(()) => {
                        summary.cleared_stale = summary.cleared_stale.saturating_add(1);
                    }
                    Err(error) => summary.failures.push(format!(
                        "{}: failed to clear stale app-server binding: {error:#}",
                        record.cutex_session_id
                    )),
                }
                continue;
            }
            PersistedRuntimeAdoptionAction::Recover(
                PersistedRuntimeRecoveryAction::CutoverRequired { reason, pid },
            ) => {
                summary.failures.push(format!(
                    "{}: cutover required before adoption ({reason}, pid={})",
                    record.cutex_session_id,
                    pid.map(|pid| pid.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ));
                continue;
            }
            PersistedRuntimeAdoptionAction::Recover(PersistedRuntimeRecoveryAction::Launch) => {
                // A clean offline record has nothing to adopt.  Keeping this
                // branch explicit makes ownership classification total while
                // avoiding noisy startup output for ordinary offline sessions.
                continue;
            }
            PersistedRuntimeAdoptionAction::Recover(
                PersistedRuntimeRecoveryAction::Reconnect { runtime_agent_id },
            ) => {
                let Some(binding) = record.app_server_runtime.as_ref() else {
                    summary.failures.push(format!(
                        "{}: reconnect claim omitted its app-server binding",
                        record.cutex_session_id
                    ));
                    continue;
                };
                match connect_runtime(config, &record, binding, &runtime_agent_id) {
                    Ok(_) => summary.adopted = summary.adopted.saturating_add(1),
                    Err(error) => summary
                        .failures
                        .push(format!("{}: {error:#}", record.cutex_session_id)),
                }
            }
        }
    }
    Ok(summary)
}

fn runtime_agent_registration(
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
    runtime_agent_id: &str,
) -> anyhow::Result<AgentBusRegisterRequest> {
    let runtime_agent_id = runtime_agent_id.trim();
    if runtime_agent_id.is_empty() {
        anyhow::bail!("runtime_agent_id must not be empty");
    }
    let thread_id = record
        .codex_session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("cutex session has no Codex session id"))?;
    let cwd = cutex_session_launch_cwd(record);
    let base_name = sanitize_session_component(&cutex_session_display_name(record), 48, "agent");
    let cwd_hash = fnv1a_hex(cwd);
    let path_key = cwd_hash[..7].to_string();
    let groups =
        normalize_registered_agent_groups(record.agent_groups.clone(), Some(&path_key), cwd);
    Ok(AgentBusRegisterRequest {
        id: runtime_agent_id.to_string(),
        name: format!("{base_name}.{path_key}"),
        base_name: Some(base_name),
        thread_name: record.thread_name.clone(),
        path_key: Some(path_key),
        session_id: Some(thread_id.to_string()),
        // A live registration describes the launched occurrence, not the
        // current durable/global profile selection. Legacy bindings retain an
        // explicit unknown marker rather than being relabeled.
        profile: binding
            .launched_profile
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("-")
            .to_string(),
        cwd: cwd.to_string(),
        pid: binding.pid,
        host_id: Some(current_host_name()),
        groups,
        registration_class: record.registration_class,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::app_server::client::AppServerEvent;
    use cutex::app_server::journal::AppServerSchemaIdentity;
    use cutex::app_server::protocol::RpcNotification;
    use cutex::session::model::CutexAppServerTransport;

    fn event_context() -> AppServerRuntimeEventContext {
        AppServerRuntimeEventContext {
            cutex_session_id: "cutex-1".to_string(),
            thread_id: "root-thread".to_string(),
            runtime_generation: 1,
            runtime_backend: "cute_alden".to_string(),
            launched_profile: None,
            schema: AppServerSchemaIdentity {
                version: "test-schema".to_string(),
                sha256: "a".repeat(64),
            },
        }
    }

    fn notification(thread_id: &str) -> AppServerEvent {
        let raw = serde_json::json!({
            "method": "item/completed",
            "params": {
                "threadId": thread_id,
                "item": { "type": "agentMessage", "id": "item-1" }
            }
        });
        AppServerEvent::Notification(RpcNotification {
            method: "item/completed".to_string(),
            params: raw.get("params").cloned(),
            raw,
        })
    }

    #[test]
    fn runtime_sink_publishes_root_events_before_and_after_ignored_foreign_notifications() {
        let context = event_context();
        let mut published = Vec::new();

        assert_eq!(
            handle_runtime_native_event(&context, &notification("root-thread"), |pending| {
                published.push(pending);
                Ok(())
            })
            .expect("publish root notification"),
            RuntimeEventSinkDisposition::Published
        );
        assert_eq!(
            handle_runtime_native_event(&context, &notification("foreign-thread"), |pending| {
                published.push(pending);
                Ok(())
            })
            .expect("ignore foreign notification"),
            RuntimeEventSinkDisposition::IgnoredForeignThreadNotification
        );
        assert_eq!(published.len(), 1);
        assert_eq!(
            handle_runtime_native_event(&context, &notification("root-thread"), |pending| {
                published.push(pending);
                Ok(())
            })
            .expect("publish later root notification"),
            RuntimeEventSinkDisposition::Published
        );
        assert_eq!(published.len(), 2);
    }

    fn runtime_binding(pid: u32) -> CutexAppServerRuntimeBinding {
        CutexAppServerRuntimeBinding {
            transport: CutexAppServerTransport::UnixSocket,
            endpoint: "unix:///tmp/runtime/app.sock".to_string(),
            pid,
            runtime_dir: "/tmp/runtime".to_string(),
            launched_profile: Some("profile".to_string()),
            launch_profile_source: None,
            auth_token_path: None,
            diagnostic_journal_path: "/tmp/runtime/events.jsonl".to_string(),
            schema_version: "test".to_string(),
            schema_sha256: "hash".to_string(),
            started_at: "2026-07-10T00:00:00Z".to_string(),
        }
    }

    fn exact_claimed_record(pid: u32) -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            Some("profile".to_string()),
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_runtime = Some(runtime_binding(pid));
        record.current_runtime_agent_id = Some("runtime-1".to_string());
        record.runtime_pid = Some(pid);
        record.runtime_generation = 7;
        record
    }

    #[test]
    fn exact_live_claim_requires_pid_start_and_endpoint_ownership_evidence() {
        let record = exact_claimed_record(4242);
        let binding = record.app_server_runtime.as_ref().unwrap();
        let process_started_at = chrono::DateTime::parse_from_rfc3339("2026-07-09T23:59:55Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        validate_exact_live_runtime_claim_evidence(&record, binding, process_started_at, true)
            .expect("exact claim evidence");

        let reused_pid_started_at = chrono::DateTime::parse_from_rfc3339("2026-07-10T00:00:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(validate_exact_live_runtime_claim_evidence(
            &record,
            binding,
            reused_pid_started_at,
            true
        )
        .is_err());
        assert!(validate_exact_live_runtime_claim_evidence(
            &record,
            binding,
            process_started_at,
            false
        )
        .is_err());
    }

    #[test]
    fn exact_live_claim_rejects_generation_pid_and_partial_claim_mismatch() {
        let mut record = exact_claimed_record(4242);
        let binding = record.app_server_runtime.clone().unwrap();
        let process_started_at = chrono::DateTime::parse_from_rfc3339("2026-07-09T23:59:55Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        record.runtime_generation = 0;
        assert!(validate_exact_live_runtime_claim_evidence(
            &record,
            &binding,
            process_started_at,
            true
        )
        .is_err());
        record.runtime_generation = 7;
        record.runtime_pid = Some(4343);
        assert!(validate_exact_live_runtime_claim_evidence(
            &record,
            &binding,
            process_started_at,
            true
        )
        .is_err());
        record.runtime_pid = Some(4242);
        record.app_server_launch_claim_id = Some("partial-launch".to_string());
        assert!(validate_exact_live_runtime_claim_evidence(
            &record,
            &binding,
            process_started_at,
            true
        )
        .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unix_socket_claim_requires_the_claimed_process_to_own_the_live_inode() {
        let root = std::env::temp_dir().join(format!(
            "cutex-runtime-ownership-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir(&root).expect("runtime ownership fixture directory");
        let socket = root.join("app.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .expect("bind runtime ownership fixture socket");

        assert!(
            linux_unix_socket_is_owned_by_process(&socket, std::process::id())
                .expect("inspect exact socket owner")
        );
        assert!(linux_unix_socket_is_owned_by_process(&socket, u32::MAX).is_err());

        drop(listener);
        assert!(
            !linux_unix_socket_is_owned_by_process(&socket, std::process::id())
                .expect("closed socket inode is stale")
        );
        fs::remove_file(&socket).expect("remove fixture socket");
        fs::remove_dir(&root).expect("remove runtime ownership fixture directory");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unix_socket_claim_tolerates_connected_rows_for_the_listener_path() {
        let root = std::env::temp_dir().join(format!(
            "cutex-runtime-listener-ownership-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir(&root).expect("runtime listener fixture directory");
        let socket = root.join("app.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .expect("bind runtime listener fixture socket");
        let client_one =
            std::os::unix::net::UnixStream::connect(&socket).expect("connect first client");
        let (server_one, _) = listener.accept().expect("accept first client");
        let client_two =
            std::os::unix::net::UnixStream::connect(&socket).expect("connect second client");
        let (server_two, _) = listener.accept().expect("accept second client");

        let socket_path = socket.to_string_lossy();
        let matching_rows = fs::read_to_string("/proc/net/unix")
            .expect("read Unix socket table")
            .lines()
            .skip(1)
            .filter(|line| line.split_whitespace().nth(7) == Some(socket_path.as_ref()))
            .count();
        assert!(
            matching_rows >= 3,
            "fixture did not expose listener plus connected pathname rows"
        );
        assert!(
            linux_unix_socket_is_owned_by_process(&socket, std::process::id())
                .expect("inspect listener ownership with connected clients")
        );

        drop((client_one, server_one, client_two, server_two, listener));
        fs::remove_file(&socket).expect("remove fixture socket");
        fs::remove_dir(&root).expect("remove runtime listener fixture directory");
    }

    #[test]
    fn runtime_registration_uses_app_server_pid_and_real_thread_identity() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            Some("profile".to_string()),
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.display_name_hint = Some("Runtime Agent".to_string());
        record.agent_groups = vec!["waveline".to_string()];
        record.registration_class = AgentRegistrationClass::Persistent;
        let binding = runtime_binding(4242);

        let registration =
            runtime_agent_registration(&record, &binding, "runtime-1").expect("registration");
        assert_eq!(registration.id, "runtime-1");
        assert_eq!(registration.pid, 4242);
        assert_eq!(
            registration.session_id.as_deref(),
            Some("019f0000-0000-7000-8000-000000000001")
        );
        assert_eq!(registration.base_name.as_deref(), Some("runtime-agent"));
        assert!(registration.name.starts_with("runtime-agent."));
        assert!(registration.groups.iter().any(|group| group == "waveline"));
        assert!(registration
            .groups
            .iter()
            .any(|group| group.starts_with("project:")));
        assert_eq!(
            registration.registration_class,
            AgentRegistrationClass::Persistent
        );
        assert_eq!(registration.profile, "profile");
    }

    #[test]
    fn legacy_runtime_registration_reports_unknown_profile() {
        let record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-08-07T00:00:00Z".to_string(),
        )
        .expect("session record");
        let mut binding = runtime_binding(4242);
        binding.launched_profile = None;

        let registration =
            runtime_agent_registration(&record, &binding, "runtime-1").expect("registration");

        assert_eq!(registration.profile, "-");
    }

    #[test]
    fn running_occurrence_profile_wins_over_durable_inheritance() {
        let record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-08-07T00:00:00Z".to_string(),
        )
        .expect("session record");
        let mut binding = runtime_binding(4242);
        binding.launched_profile = Some("profile-a".to_string());

        let registration =
            runtime_agent_registration(&record, &binding, "runtime-1").expect("registration");
        assert_eq!(registration.profile, "profile-a");
    }

    #[test]
    fn adoption_recovers_live_and_clears_only_fully_dead_local_bindings() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_runtime = Some(runtime_binding(4242));
        record.current_runtime_agent_id = Some("runtime-1".to_string());
        record.runtime_pid = Some(4242);

        assert_eq!(
            classify_persisted_runtime_adoption(&record, "tethys", |_| true),
            PersistedRuntimeAdoptionAction::Recover(PersistedRuntimeRecoveryAction::Reconnect {
                runtime_agent_id: "runtime-1".to_string(),
            })
        );
        assert_eq!(
            classify_persisted_runtime_adoption(&record, "tethys", |_| false),
            PersistedRuntimeAdoptionAction::Recover(
                PersistedRuntimeRecoveryAction::ClearStaleAndLaunch
            )
        );

        record.alden_pid = Some(5151);
        assert_eq!(
            classify_persisted_runtime_adoption(&record, "tethys", |pid| pid == 5151),
            PersistedRuntimeAdoptionAction::Recover(
                PersistedRuntimeRecoveryAction::CutoverRequired {
                    reason: "stale_binding_has_live_child",
                    pid: Some(5151),
                }
            )
        );

        record.host_id = "eva-02".to_string();
        assert_eq!(
            classify_persisted_runtime_adoption(&record, "tethys", |_| {
                panic!("remote PIDs must not be probed locally")
            }),
            PersistedRuntimeAdoptionAction::Remote
        );
    }

    #[test]
    fn retired_runtime_is_skipped_before_host_or_pid_probes() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-retired".to_string(),
            Some("019f0000-0000-7000-8000-000000000099".to_string()),
            "eva-02".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        record.app_server_runtime = Some(runtime_binding(4242));
        record.runtime_pid = Some(4242);

        assert_eq!(
            classify_persisted_runtime_adoption(&record, "tethys", |_| {
                panic!("retired PIDs must not be probed")
            }),
            PersistedRuntimeAdoptionAction::Retired
        );
    }

    #[test]
    fn live_binding_recovery_keeps_the_persisted_runtime_agent_identity() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_runtime = Some(runtime_binding(4242));
        record.current_runtime_agent_id = Some("runtime-1".to_string());
        record.runtime_pid = Some(4242);
        record.runtime_generation = 7;

        assert_eq!(
            classify_persisted_runtime_recovery(&record, |pid| pid == 4242),
            PersistedRuntimeRecoveryAction::Reconnect {
                runtime_agent_id: "runtime-1".to_string()
            }
        );
        assert_eq!(record.runtime_generation, 7);
        assert_eq!(record.profile, None);
    }

    #[test]
    fn live_binding_without_identity_requires_cutover_instead_of_launch() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_runtime = Some(runtime_binding(4242));
        record.runtime_generation = 7;

        assert_eq!(
            classify_persisted_runtime_recovery(&record, |pid| pid == 4242),
            PersistedRuntimeRecoveryAction::CutoverRequired {
                reason: "live_binding_missing_runtime_agent_id",
                pid: Some(4242),
            }
        );
    }

    #[test]
    fn dead_binding_is_the_only_persisted_state_allowed_to_clear_and_launch() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_runtime = Some(runtime_binding(4242));
        record.runtime_pid = Some(4242);
        assert_eq!(
            classify_persisted_runtime_recovery(&record, |_| false),
            PersistedRuntimeRecoveryAction::ClearStaleAndLaunch
        );

        record.app_server_runtime = None;
        record.current_runtime_agent_id = None;
        record.alden_pid = None;
        assert_eq!(
            classify_persisted_runtime_recovery(&record, |_| false),
            PersistedRuntimeRecoveryAction::ClearStaleAndLaunch
        );

        record.runtime_pid = None;
        assert_eq!(
            classify_persisted_runtime_recovery(&record, |_| false),
            PersistedRuntimeRecoveryAction::Launch
        );
    }

    #[test]
    fn missing_binding_with_live_process_requires_cutover() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.runtime_pid = Some(4242);

        assert_eq!(
            classify_persisted_runtime_recovery(&record, |pid| pid == 4242),
            PersistedRuntimeRecoveryAction::CutoverRequired {
                reason: "binding_missing_live_child",
                pid: Some(4242),
            }
        );
    }

    #[test]
    fn stale_cleanup_snapshot_rejects_a_concurrent_owner_change() {
        let mut expected = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("session record");
        expected.app_server_runtime = Some(runtime_binding(4242));
        expected.current_runtime_agent_id = Some("runtime-1".to_string());
        expected.runtime_pid = Some(4242);
        expected.alden_pid = Some(5151);
        expected.runtime_generation = 7;

        assert!(runtime_ownership_snapshot_matches(&expected, &expected));
        assert_eq!(runtime_occurrence_pids(&expected), vec![4242, 5151]);

        let mut concurrent = expected.clone();
        concurrent.runtime_generation = 8;
        concurrent.current_runtime_agent_id = Some("runtime-2".to_string());
        assert!(!runtime_ownership_snapshot_matches(&concurrent, &expected));

        let mut profile_changed = expected.clone();
        profile_changed.profile = Some("profile-b".to_string());
        assert!(!runtime_ownership_snapshot_matches(
            &profile_changed,
            &expected
        ));

        let mut heartbeat_changed = expected.clone();
        heartbeat_changed.pending_launch_id = Some("new-heartbeat-launch".to_string());
        assert!(!runtime_ownership_snapshot_matches(
            &heartbeat_changed,
            &expected
        ));

        let mut launch_claim_changed = expected.clone();
        launch_claim_changed.app_server_launch_claim_id = Some("launch-2".to_string());
        assert!(!runtime_ownership_snapshot_matches(
            &launch_claim_changed,
            &expected
        ));
    }

    #[test]
    fn legacy_pending_launch_id_does_not_block_a_clean_offline_record() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-08-07T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.pending_launch_id = Some("legacy-heartbeat-launch".to_string());

        assert_eq!(
            classify_persisted_runtime_recovery(&record, |_| false),
            PersistedRuntimeRecoveryAction::Launch
        );
    }

    #[test]
    fn app_server_launch_claim_blocks_an_unfenced_generation() {
        let mut record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            Some("019f0000-0000-7000-8000-000000000001".to_string()),
            "tethys".to_string(),
            "/tmp/project".to_string(),
            None,
            "2026-08-07T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.app_server_launch_claim_id = Some("app-server-launch-1".to_string());

        assert_eq!(
            classify_persisted_runtime_recovery(&record, |_| false),
            PersistedRuntimeRecoveryAction::CutoverRequired {
                reason: "pending_launch_ownership_unknown",
                pid: None,
            }
        );
    }
}
