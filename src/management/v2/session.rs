use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Context;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;

use crate::agent_bus::model::AgentRegistrationClass;
use crate::app_server::manager::AppServerManagedRuntimeStatus;
use crate::config::atomic::write_private_pretty_json_atomic;
use crate::config::paths::runtime_dir;
use crate::im::registry::ImRegistry;
use crate::management::v2::repository::EventRepository;
use crate::platform::process::process_is_running;
use crate::session::model::CutexSessionArchiveState;
use crate::session::model::CutexSessionRecord;
use crate::session::model::CutexSessionRuntimeBackend;
use crate::session::model::LaunchProfileSource;
use crate::session::service::cutex_session_display_name;
use crate::session::store::load_cutex_session_store;

use super::activity::load_session_activity_states;
use super::activity::SessionActivityState;

pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const CUTEX_METHOD_REGISTRY_VERSION: u8 = 3;
pub const NATIVE_REQUEST_POLICY_SHA256: &str =
    "e98d81bd297098500503ac5d5e10e0f22fda1c3a56a8ceb5282813d31ce87a6e";
pub const NATIVE_REQUEST_ALLOW_RULES_SHA256: &str =
    "3e2bee2386e40f1878a159d9efc4bfa0ddb143f13177bba818caf83a6b26c206";
pub const CUTEX_METHOD_REGISTRY_INDEX_SHA256: &str =
    "5c97fce39614e2ecc0c751ea0a7b289e5086bb0c6d4043ad1b5ea71347be6896";
pub const CUTEX_METHOD_REGISTRY_SCHEMA_SHA256: &str =
    "6efab871e39598c776a5439d938f76d08fa3c1078c98e834a6ae434978c04780";

pub const CUTEX_METHOD_REGISTRY_INDEX: &str = include_str!("schema/cutex-method-registry-v3.json");

pub const CUTEX_METHOD_REGISTRY_METHODS: &[&str] = &[
    "cutex/session/get",
    "cutex/session/retire",
    "cutex/session/restore",
    "cutex/session/defaults/update",
    "cutex/session/profile/set",
    "cutex/session/profile/clear",
    "cutex/session/groups/get",
    "cutex/session/groups/set",
    "cutex/session/groups/add",
    "cutex/session/groups/remove",
    "cutex/session/visibility/show",
    "cutex/session/visibility/hide",
    "cutex/runtime/online",
    "cutex/runtime/offline",
    "cutex/runtime/close",
    "cutex/focus/get",
    "cutex/focus/set",
    "cutex/focus/clear",
    "cutex/userInput/submit",
    "cutex/userInput/queue/list",
    "cutex/userInput/queue/update",
    "cutex/userInput/queue/remove",
    "cutex/userInput/queue/flush",
];

pub fn cutex_method_is_registered(method: &str) -> bool {
    CUTEX_METHOD_REGISTRY_METHODS.contains(&method)
}

const SESSION_PROJECTION_FILE: &str = "session-projection-state.json";
const SESSION_PROJECTION_LOCK_FILE: &str = "session-projection-state.lock";

static SESSION_PROJECTION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static ACTIVITY_LOAD_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

pub type RuntimeStatusLoader = fn(&str) -> anyhow::Result<Option<AppServerManagedRuntimeStatus>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FocusState {
    revision: u64,
    owner: String,
    mobile_muted: bool,
    source: Option<String>,
    updated_at: Option<String>,
}

impl Default for FocusState {
    fn default() -> Self {
        Self {
            revision: 1,
            owner: "none".to_string(),
            mobile_muted: false,
            source: None,
            updated_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionProjectionEntry {
    revision: u64,
    fingerprint: String,
    focus: FocusState,
}

impl Default for SessionProjectionEntry {
    fn default() -> Self {
        Self {
            revision: 1,
            fingerprint: String::new(),
            focus: FocusState::default(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionProjectionStore {
    #[serde(default = "projection_store_version")]
    version: u8,
    #[serde(default)]
    sessions: HashMap<String, SessionProjectionEntry>,
}

fn projection_store_version() -> u8 {
    1
}

pub fn session_list_resource(
    registry: &ImRegistry,
    load_runtime_status: RuntimeStatusLoader,
    repository: &EventRepository,
) -> anyhow::Result<Value> {
    let store = load_cutex_session_store()?;
    let host_id = crate::platform::host::current_host_name();
    let management = management_identity(repository)?;
    let activity_states = load_activity_states_best_effort();
    let mut seen = HashSet::new();
    let mut records = store
        .sessions
        .values()
        .filter(|record| record.host_id == host_id)
        .filter(|record| record.is_active())
        .filter(|record| is_durable_management_session(record))
        .filter(|record| seen.insert(record.cutex_session_id.clone()))
        .collect::<Vec<_>>();
    records.sort_by(|left, right| left.cutex_session_id.cmp(&right.cutex_session_id));
    let projection_inputs = records
        .into_iter()
        .filter_map(|record| {
            let visible = session_is_visible(registry, record);
            visible.then_some((record, visible))
        })
        .map(|(record, visible)| {
            Ok((
                record,
                visible,
                load_runtime_status(&record.cutex_session_id)?,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let sessions = with_projection_store(|store| {
        projection_inputs
            .into_iter()
            .map(|(record, visible, runtime_status)| {
                let entry = store
                    .sessions
                    .entry(record.cutex_session_id.clone())
                    .or_default();
                project_session_entry(
                    record,
                    visible,
                    runtime_status,
                    activity_states.get(&record.cutex_session_id),
                    &management,
                    entry,
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })?;
    Ok(json!({
        "contractVersion": 2,
        "hostId": host_id,
        "management": management,
        "sessions": sessions,
    }))
}

pub fn session_resource(
    cutex_session_id: &str,
    registry: &ImRegistry,
    load_runtime_status: RuntimeStatusLoader,
    repository: &EventRepository,
) -> anyhow::Result<Option<Value>> {
    session_resource_with_visibility(
        cutex_session_id,
        registry,
        load_runtime_status,
        repository,
        true,
    )
}

pub fn session_resource_including_hidden(
    cutex_session_id: &str,
    registry: &ImRegistry,
    load_runtime_status: RuntimeStatusLoader,
    repository: &EventRepository,
) -> anyhow::Result<Option<Value>> {
    session_resource_with_visibility(
        cutex_session_id,
        registry,
        load_runtime_status,
        repository,
        false,
    )
}

pub fn session_resource_including_archive(
    cutex_session_id: &str,
    registry: &ImRegistry,
    load_runtime_status: RuntimeStatusLoader,
    repository: &EventRepository,
) -> anyhow::Result<Option<Value>> {
    let store = load_cutex_session_store()?;
    let Some(record) = store
        .sessions
        .values()
        .find(|record| record.cutex_session_id == cutex_session_id)
    else {
        return Ok(None);
    };
    if record.host_id != crate::platform::host::current_host_name()
        || !is_durable_management_session(record)
    {
        return Ok(None);
    }
    if record.is_retired() {
        return retired_session_resource(cutex_session_id);
    }
    let visible = session_is_visible(registry, record);
    let activity_states = load_activity_states_best_effort();
    Ok(Some(project_session(
        record,
        visible,
        load_runtime_status(&record.cutex_session_id)?,
        activity_states.get(&record.cutex_session_id),
        &management_identity(repository)?,
    )?))
}

fn session_resource_with_visibility(
    cutex_session_id: &str,
    registry: &ImRegistry,
    load_runtime_status: RuntimeStatusLoader,
    repository: &EventRepository,
    require_visible: bool,
) -> anyhow::Result<Option<Value>> {
    let store = load_cutex_session_store()?;
    let Some(record) = store
        .sessions
        .values()
        .find(|record| record.cutex_session_id == cutex_session_id)
    else {
        return Ok(None);
    };
    let visible = session_is_visible(registry, record);
    if record.host_id != crate::platform::host::current_host_name()
        || record.is_retired()
        || !is_durable_management_session(record)
        || (require_visible && !visible)
    {
        return Ok(None);
    }
    let activity_states = load_activity_states_best_effort();
    Ok(Some(project_session(
        record,
        visible,
        load_runtime_status(&record.cutex_session_id)?,
        activity_states.get(&record.cutex_session_id),
        &management_identity(repository)?,
    )?))
}

pub fn management_identity(repository: &EventRepository) -> anyhow::Result<Value> {
    let metadata = repository.stream_metadata()?;
    Ok(json!({
        "eventStream": metadata,
        "nativeRequestPolicy": {
            "version": 2,
            "sha256": NATIVE_REQUEST_POLICY_SHA256,
            "allowRulesSha256": NATIVE_REQUEST_ALLOW_RULES_SHA256,
        },
        "cutexMethodRegistry": {
            "version": CUTEX_METHOD_REGISTRY_VERSION,
            "indexSha256": CUTEX_METHOD_REGISTRY_INDEX_SHA256,
            "schemaSha256": CUTEX_METHOD_REGISTRY_SCHEMA_SHA256,
            "capabilities": {
                "sessionArchive": {
                    "version": 1,
                    "states": ["active", "retired"],
                    "retireMethod": "cutex/session/retire",
                    "restoreMethod": "cutex/session/restore",
                    "archiveQuery": "GET /v2/sessions?lifecycle=retired",
                    "archiveView": "GET /v2/sessions/{cutexSessionId}?lifecycle=retired",
                    "retireRequiresRuntimeFence": true,
                    "restoreStartsRuntime": false,
                },
                "sessionActivity": {
                    "version": 2,
                    "source": "native_app_server_events",
                    "fields": [
                        "runtimeGeneration",
                        "lastOutputAt",
                        "lastOutputCompletedAt",
                        "lastTurnCompletedAt",
                        "lastFileChangeAt",
                        "lastOutput",
                        "lastToolCall"
                    ],
                    "changesSessionRevision": false,
                }
            },
        },
        "maxRequestBytes": MAX_REQUEST_BYTES,
    }))
}

pub fn retired_session_list_resource() -> anyhow::Result<Value> {
    let store = load_cutex_session_store()?;
    let host_id = crate::platform::host::current_host_name();
    let activity_states = load_activity_states_best_effort();
    let mut sessions = store
        .sessions
        .values()
        .filter(|record| record.host_id == host_id)
        .filter(|record| record.is_retired())
        .filter(|record| is_durable_management_session(record))
        .map(|record| {
            retired_session_resource_value(record, activity_states.get(&record.cutex_session_id))
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        left.get("cutexSessionId")
            .and_then(Value::as_str)
            .cmp(&right.get("cutexSessionId").and_then(Value::as_str))
    });
    Ok(json!({
        "contractVersion": 2,
        "hostId": host_id,
        "lifecycle": CutexSessionArchiveState::Retired.label(),
        "sessions": sessions,
    }))
}

pub fn retired_session_resource(cutex_session_id: &str) -> anyhow::Result<Option<Value>> {
    let store = load_cutex_session_store()?;
    let activity_states = load_activity_states_best_effort();
    let record = store.sessions.values().find(|record| {
        record.cutex_session_id == cutex_session_id
            && record.host_id == crate::platform::host::current_host_name()
            && record.is_retired()
            && is_durable_management_session(record)
    });
    Ok(record.map(|record| {
        retired_session_resource_value(record, activity_states.get(&record.cutex_session_id))
    }))
}

fn retired_session_resource_value(
    record: &CutexSessionRecord,
    activity: Option<&SessionActivityState>,
) -> Value {
    json!({
        "contractVersion": 2,
        "cutexSessionId": record.cutex_session_id,
        "revision": record.durable_revision(),
        "lifecycle": record.archive_state.label(),
        "retiredAt": record.retired_at,
        "hostId": record.host_id,
        "displayName": cutex_session_display_name(record),
        "threadName": record.thread_name,
        "cwd": record.cwd,
        "managedCwd": record.managed_cwd,
        "profile": record.profile,
        "groups": normalized_groups(&record.agent_groups),
        "registrationClass": if record.registration_class == AgentRegistrationClass::Persistent {
            "persistent"
        } else {
            "local_only"
        },
        "native": { "threadId": record.codex_session_id },
        "runtime": {
            "backend": runtime_backend_name(record.runtime_backend),
            "status": "offline",
            "runtimeGeneration": record.runtime_generation,
            "runtimeAgentId": Value::Null,
        },
        "runtimeDefaults": runtime_defaults_resource(record),
        "activity": activity_resource(activity),
        "createdAt": record.created_at,
        "updatedAt": record.updated_at,
    })
}

pub fn focus_resource(cutex_session_id: &str) -> anyhow::Result<Value> {
    with_projection_entry(cutex_session_id, |entry| {
        ensure_focus_timestamp(&mut entry.focus);
        serde_json::to_value(&entry.focus).context("Failed to serialize management v2 focus")
    })
}

pub fn runtime_defaults_resource(record: &CutexSessionRecord) -> Value {
    json!({
        "backend": runtime_backend_name(record.runtime_backend),
        "managedCwd": record.managed_cwd,
        "permissions": record.permission_defaults,
        "approvalPolicy": record.approval_policy,
        "sandboxMode": record.sandbox_mode,
        "model": record.model_defaults,
        "reasoningEffort": record.reasoning_defaults,
        "cliArgs": record.default_cli_args,
        "groups": normalized_groups(&record.agent_groups),
    })
}

fn activity_resource(activity: Option<&SessionActivityState>) -> Value {
    let activity = activity.cloned().unwrap_or_default();
    let mut resource = json!({
        "revision": activity.revision,
        "runtimeGeneration": activity.runtime_generation,
        "lastOutputAt": activity.last_output_at,
        "lastOutputCompletedAt": activity.last_output_completed_at,
        "lastTurnCompletedAt": activity.last_turn_completed_at,
        "lastFileChangeAt": activity.last_file_change_at,
    });
    let object = resource
        .as_object_mut()
        .expect("session activity resource is an object");
    if let Some(output) = activity.last_output.as_ref() {
        object.insert("lastOutput".to_string(), output_resource(output));
    }
    if let Some(tool) = activity.last_tool_call.as_ref() {
        object.insert("lastToolCall".to_string(), tool_resource(tool));
    }
    resource
}

fn output_resource(output: &crate::observability::SafeOutputProjection) -> Value {
    let mut value = json!({
        "cutexSessionId": output.association.cutex_session_id,
        "class": output.class,
        "displayText": output.display_text,
        "updatedAt": output.updated_at,
        "runtimeGeneration": output.runtime_generation,
    });
    insert_task_association(&mut value, &output.association);
    value
}

fn tool_resource(tool: &crate::observability::SafeToolCallProjection) -> Value {
    let mut value = json!({
        "cutexSessionId": tool.association.cutex_session_id,
        "class": tool.class,
        "status": tool.status,
        "displayText": tool.display_text,
        "updatedAt": tool.updated_at,
        "runtimeGeneration": tool.runtime_generation,
    });
    insert_task_association(&mut value, &tool.association);
    value
}

fn insert_task_association(
    value: &mut Value,
    association: &crate::observability::ObservationAssociation,
) {
    let object = value
        .as_object_mut()
        .expect("observability resource is an object");
    if let Some(project_id) = association.project_id.as_ref() {
        object.insert("projectId".to_string(), json!(project_id));
    }
    if let Some(assignment_id) = association.assignment_id.as_ref() {
        object.insert("assignmentId".to_string(), json!(assignment_id));
    }
    if let Some(attempt_number) = association.attempt_number {
        object.insert("attemptNumber".to_string(), json!(attempt_number));
    }
}

fn load_activity_states_best_effort() -> HashMap<String, SessionActivityState> {
    match load_session_activity_states() {
        Ok(states) => {
            ACTIVITY_LOAD_WARNING_EMITTED.store(false, Ordering::Relaxed);
            states
        }
        Err(error) => {
            if !ACTIVITY_LOAD_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "warning: session activity projection is unavailable; returning null activity: {error:#}"
                );
            }
            HashMap::new()
        }
    }
}

fn tui_is_known_attached(record: &CutexSessionRecord, app_server_connected: bool) -> bool {
    app_server_connected
        && record.runtime_backend == CutexSessionRuntimeBackend::CuteAlden
        && record.alden_pid.is_some_and(process_is_running)
}

pub fn set_focus(
    cutex_session_id: &str,
    expected_revision: u64,
    owner: &str,
    mobile_muted: bool,
    source: Option<String>,
) -> anyhow::Result<Value> {
    if !matches!(owner, "pc" | "mobile" | "backend" | "none") {
        anyhow::bail!("focus owner is outside the v2 contract");
    }
    with_projection_entry(cutex_session_id, |entry| {
        if entry.focus.revision != expected_revision {
            anyhow::bail!(
                "focus revision conflict: expected {expected_revision}, current {}",
                entry.focus.revision
            );
        }
        if entry.focus.revision >= crate::management::v2::model::MAX_SAFE_SEQUENCE {
            anyhow::bail!("focus revision exhausted the JSON-safe integer range");
        }
        entry.focus.revision += 1;
        entry.focus.owner = owner.to_string();
        entry.focus.mobile_muted = mobile_muted;
        entry.focus.source = source;
        entry.focus.updated_at = Some(Utc::now().to_rfc3339());
        serde_json::to_value(&entry.focus).context("Failed to serialize management v2 focus")
    })
}

pub fn clear_focus(cutex_session_id: &str, expected_revision: u64) -> anyhow::Result<Value> {
    set_focus(cutex_session_id, expected_revision, "none", false, None)
}

fn project_session(
    record: &CutexSessionRecord,
    visible: bool,
    runtime_status: Option<AppServerManagedRuntimeStatus>,
    activity: Option<&SessionActivityState>,
    management: &Value,
) -> anyhow::Result<Value> {
    let cutex_session_id = record.cutex_session_id.clone();
    with_projection_entry(&cutex_session_id, |entry| {
        project_session_entry(record, visible, runtime_status, activity, management, entry)
    })
}

fn project_session_entry(
    record: &CutexSessionRecord,
    visible: bool,
    runtime_status: Option<AppServerManagedRuntimeStatus>,
    activity: Option<&SessionActivityState>,
    management: &Value,
    entry: &mut SessionProjectionEntry,
) -> anyhow::Result<Value> {
    ensure_focus_timestamp(&mut entry.focus);
    let backend = runtime_backend_name(record.runtime_backend);
    let app_server_connected = runtime_status
        .as_ref()
        .is_some_and(|status| status.connected);
    let runtime_state = if app_server_connected {
        "online"
    } else if runtime_status
        .as_ref()
        .and_then(|status| status.last_error.as_ref())
        .is_some()
    {
        "error"
    } else {
        "offline"
    };
    let schema = record.app_server_runtime.as_ref().map(|binding| {
        json!({
            "protocol": "codex-app-server",
            "majorVersion": 2,
            "version": binding.schema_version,
            "sha256": binding.schema_sha256,
            "channel": "experimental",
            "capabilities": { "experimentalApi": true },
            "extensions": ["cutex-inter-agent-v2"],
        })
    });
    let effective_next_launch = effective_next_launch_resource(record);
    let launched_profile = record
        .app_server_runtime
        .as_ref()
        .and_then(|binding| binding.launched_profile.clone());
    let launch_profile_source = record.app_server_runtime.as_ref().and_then(|binding| {
        binding.launched_profile.as_ref().map(|_| {
            binding
                .launch_profile_source
                .as_ref()
                .map(|source| source.as_str())
                .unwrap_or(LaunchProfileSource::Unknown.as_str())
        })
    });
    let mut value = json!({
        "contractVersion": 2,
        "cutexSessionId": record.cutex_session_id,
        "revision": record.durable_revision(),
        "lifecycle": record.archive_state.label(),
        "retiredAt": record.retired_at,
        "hostId": record.host_id,
        "displayName": cutex_session_display_name(record),
        "threadName": record.thread_name,
        "cwd": record.cwd,
        "profile": record.profile,
        "configuredProfile": record.profile,
        "effectiveNextLaunch": effective_next_launch,
        "launchedProfile": launched_profile,
        "launchProfileSource": launch_profile_source,
        "groups": normalized_groups(&record.agent_groups),
        "registrationClass": registration_class_value(record)?,
        "visible": visible,
        "native": {
            "threadId": record.codex_session_id,
            "schema": schema,
        },
        "runtime": {
            "backend": backend,
            "status": runtime_state,
            "appServerConnected": app_server_connected,
            "runtimeGeneration": record.runtime_generation,
            "runtimeAgentId": record.current_runtime_agent_id,
            "activeTurnId": runtime_status.as_ref().and_then(|status| status.active_turn_id.clone()),
            "activeTurnObservedAt": runtime_status.as_ref().and_then(|status| status.active_turn_observed_at.clone()),
            "threadStatus": runtime_status.as_ref().and_then(|status| status.thread_status.clone()),
            "threadStatusObservedAt": runtime_status.as_ref().and_then(|status| status.thread_status_observed_at.clone()),
            "threadSettings": runtime_status.as_ref().and_then(|status| status.thread_settings.clone()),
            "threadSettingsSource": runtime_status.as_ref().and_then(|status| status.thread_settings_source.clone()),
            "threadSettingsComplete": runtime_status.as_ref().is_some_and(|status| status.thread_settings_complete),
            "threadSettingsObservedAt": runtime_status.as_ref().and_then(|status| status.thread_settings_observed_at.clone()),
            "runtimeWorkspaceRoots": runtime_status.as_ref().and_then(|status| status.runtime_workspace_roots.clone()),
            "instructionSources": runtime_status.as_ref().and_then(|status| status.instruction_sources.clone()),
            "resumeSnapshotObservedAt": runtime_status.as_ref().and_then(|status| status.resume_snapshot_observed_at.clone()),
            "requiresVisibleTerminal": record.runtime_backend == CutexSessionRuntimeBackend::HostForeground,
            "tuiAttached": tui_is_known_attached(record, app_server_connected),
            "lastEventAt": runtime_status.as_ref().and_then(|status| status.last_event_at.clone()),
            "lastError": runtime_status.as_ref().and_then(|status| status.last_error.clone()),
        },
        "runtimeDefaults": runtime_defaults_resource(record),
        "activity": activity_resource(activity),
        "focus": entry.focus,
        "management": management,
        "createdAt": record.created_at,
        "updatedAt": record.updated_at,
    });
    let mut fingerprint_value = value.clone();
    let fingerprint_object = fingerprint_value
        .as_object_mut()
        .expect("session resource object");
    fingerprint_object.remove("revision");
    fingerprint_object.remove("activity");
    let fingerprint = serde_json::to_string(&fingerprint_value)?;
    if !entry.fingerprint.is_empty() && entry.fingerprint != fingerprint {
        entry.revision = entry
            .revision
            .checked_add(1)
            .filter(|revision| *revision <= super::model::MAX_SAFE_SEQUENCE)
            .context("management v2 session revision exhausted")?;
    }
    entry.fingerprint = fingerprint;
    value["revision"] = Value::from(record.durable_revision());
    Ok(value)
}

fn effective_next_launch_resource(record: &CutexSessionRecord) -> Value {
    if let Some(profile) = record
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        return json!({ "profile": profile, "source": "session_configured" });
    }
    let global = crate::config::store::load_codez_config();
    if let Some(profile) = global
        .default_profile
        .as_deref()
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        return json!({ "profile": profile, "source": "global_default" });
    }
    json!({ "profile": Value::Null, "source": "unavailable" })
}

fn ensure_focus_timestamp(focus: &mut FocusState) {
    if focus.updated_at.is_none() {
        focus.updated_at = Some(Utc::now().to_rfc3339());
    }
}

fn session_is_visible(registry: &ImRegistry, record: &CutexSessionRecord) -> bool {
    record.exposed_to_backend
        || registry.sessions.values().any(|entry| {
            entry.visible
                && (entry.session_id == record.cutex_session_id
                    || record.codex_session_id.as_deref() == Some(entry.session_id.as_str()))
        })
}

fn is_durable_management_session(record: &CutexSessionRecord) -> bool {
    matches!(
        record.registration_class,
        AgentRegistrationClass::Persistent | AgentRegistrationClass::LocalOnly
    )
}

fn normalized_groups(groups: &[String]) -> Vec<String> {
    let mut groups = groups
        .iter()
        .map(|group| group.trim())
        .filter(|group| !group.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    groups.sort();
    groups.dedup();
    groups
}

fn runtime_backend_name(backend: CutexSessionRuntimeBackend) -> &'static str {
    match backend {
        CutexSessionRuntimeBackend::Host => "host",
        CutexSessionRuntimeBackend::HostForeground => "host_foreground",
        CutexSessionRuntimeBackend::Docker => "docker",
        CutexSessionRuntimeBackend::CuteAlden => "cute_alden",
        CutexSessionRuntimeBackend::Future => "future",
    }
}

fn registration_class_value(record: &CutexSessionRecord) -> anyhow::Result<Value> {
    match record.registration_class {
        AgentRegistrationClass::Persistent => Ok(json!("persistent")),
        AgentRegistrationClass::LocalOnly => Ok(json!("local_only")),
        AgentRegistrationClass::Ephemeral => {
            anyhow::bail!("ephemeral agents are not durable management v2 sessions")
        }
    }
}

fn with_projection_entry<T>(
    cutex_session_id: &str,
    action: impl FnOnce(&mut SessionProjectionEntry) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    with_projection_store(|store| {
        let entry = store
            .sessions
            .entry(cutex_session_id.to_string())
            .or_default();
        action(entry)
    })
}

fn with_projection_store<T>(
    action: impl FnOnce(&mut SessionProjectionStore) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _process_guard = SESSION_PROJECTION_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("management v2 session projection lock was poisoned"))?;
    let root = runtime_dir()?.join("management-v2");
    fs::create_dir_all(&root)?;
    secure_directory(&root)?;
    let lock_path = root.join(SESSION_PROJECTION_LOCK_FILE);
    let lock_file = open_private_lock(&lock_path)?;
    lock_file.lock()?;
    let result = (|| {
        let path = root.join(SESSION_PROJECTION_FILE);
        let mut store = load_projection_store(&path)?;
        let result = action(&mut store)?;
        write_private_pretty_json_atomic(&path, &store, "management v2 session projections")?;
        Ok(result)
    })();
    let unlock = lock_file.unlock();
    if result.is_ok() {
        unlock?;
    }
    result
}

fn load_projection_store(path: &Path) -> anyhow::Result<SessionProjectionStore> {
    match fs::read(path) {
        Ok(bytes) => {
            let store: SessionProjectionStore =
                serde_json::from_slice(&bytes).with_context(|| {
                    format!(
                        "Failed to parse management v2 session projections: {}",
                        path.display()
                    )
                })?;
            if store.version != 1 {
                anyhow::bail!("unsupported management v2 session projection version");
            }
            Ok(store)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SessionProjectionStore {
            version: 1,
            sessions: HashMap::new(),
        }),
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to read management v2 session projections: {}",
                path.display()
            )
        }),
    }
}

fn open_private_lock(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let file = options.open(path)?;
    secure_file(path)?;
    Ok(file)
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use uuid::Uuid;

    use super::*;

    static TEST_HOME_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    struct IsolatedTestHome {
        root: PathBuf,
        previous_home: Option<OsString>,
        _environment_guard: MutexGuard<'static, ()>,
    }

    impl IsolatedTestHome {
        fn new(prefix: &str) -> std::io::Result<Self> {
            let environment_guard = TEST_HOME_MUTEX
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let root = std::env::temp_dir().join(format!(
                "{prefix}-{}",
                &Uuid::new_v4().simple().to_string()[..8]
            ));
            create_owner_only_test_dir(&root)?;
            let previous_home = std::env::var_os("HOME");
            unsafe {
                std::env::set_var("HOME", &root);
            }
            Ok(Self {
                root,
                previous_home,
                _environment_guard: environment_guard,
            })
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for IsolatedTestHome {
        fn drop(&mut self) {
            unsafe {
                match self.previous_home.take() {
                    Some(value) => std::env::set_var("HOME", value),
                    None => std::env::remove_var("HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    fn create_owner_only_test_dir(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(path)
    }

    #[cfg(not(unix))]
    fn create_owner_only_test_dir(path: &Path) -> std::io::Result<()> {
        fs::create_dir(path)
    }

    #[test]
    fn backend_names_match_frozen_contract() {
        assert_eq!(
            runtime_backend_name(CutexSessionRuntimeBackend::Host),
            "host"
        );
        assert_eq!(
            runtime_backend_name(CutexSessionRuntimeBackend::HostForeground),
            "host_foreground"
        );
        assert_eq!(
            runtime_backend_name(CutexSessionRuntimeBackend::CuteAlden),
            "cute_alden"
        );
    }

    #[test]
    fn management_hashes_are_exact_candidate_inputs() {
        use sha2::Digest;

        assert_eq!(NATIVE_REQUEST_POLICY_SHA256.len(), 64);
        assert_eq!(NATIVE_REQUEST_ALLOW_RULES_SHA256.len(), 64);
        assert_eq!(CUTEX_METHOD_REGISTRY_INDEX_SHA256.len(), 64);
        assert_eq!(CUTEX_METHOD_REGISTRY_SCHEMA_SHA256.len(), 64);
        assert_eq!(MAX_REQUEST_BYTES, 16_777_216);
        assert_eq!(
            format!(
                "{:x}",
                sha2::Sha256::digest(CUTEX_METHOD_REGISTRY_INDEX.as_bytes())
            ),
            CUTEX_METHOD_REGISTRY_INDEX_SHA256
        );
    }

    #[test]
    fn runtime_defaults_use_the_frozen_v2_field_names() {
        let mut record = CutexSessionRecord::from_codex_session_id("thread-defaults")
            .expect("durable test session");
        record.runtime_backend = CutexSessionRuntimeBackend::HostForeground;
        record.permission_defaults = Some("full-access".to_string());
        record.reasoning_defaults = Some("xhigh".to_string());
        record.agent_groups = vec!["waveline".to_string()];

        assert_eq!(
            runtime_defaults_resource(&record),
            json!({
                "backend": "host_foreground",
                "managedCwd": null,
                "permissions": "full-access",
                "approvalPolicy": null,
                "sandboxMode": null,
                "model": null,
                "reasoningEffort": "xhigh",
                "cliArgs": [],
                "groups": ["waveline"],
            })
        );
    }

    #[test]
    fn session_resource_exposes_manager_observed_native_runtime_state() {
        let mut record = CutexSessionRecord::from_codex_session_id("thread-native-state")
            .expect("durable test session");
        record.cutex_session_id = "cutex-native-state".to_string();
        record.runtime_generation = 7;
        let runtime_status = AppServerManagedRuntimeStatus {
            cutex_session_id: record.cutex_session_id.clone(),
            thread_id: "thread-native-state".to_string(),
            runtime_generation: 7,
            connected: true,
            active_turn_id: Some("turn-active".to_string()),
            active_turn_observed_at: Some("2026-07-24T00:00:03Z".to_string()),
            thread_status: Some(json!({
                "type": "active",
                "activeFlags": ["waitingOnApproval"]
            })),
            thread_status_observed_at: Some("2026-07-24T00:00:02Z".to_string()),
            thread_settings: Some(json!({
                "model": "gpt-test",
                "activePermissionProfile": { "id": ":workspace" },
                "collaborationMode": {
                    "mode": "plan",
                    "settings": {
                        "model": "",
                        "reasoning_effort": "medium",
                        "developer_instructions": null
                    }
                }
            })),
            thread_settings_source: Some("thread/settings/updated".to_string()),
            thread_settings_complete: true,
            thread_settings_observed_at: Some("2026-07-24T00:00:01Z".to_string()),
            runtime_workspace_roots: Some(json!(["/workspace", "/shared"])),
            instruction_sources: Some(json!(["/workspace/AGENTS.md"])),
            resume_snapshot_observed_at: Some("2026-07-24T00:00:00Z".to_string()),
            event_method_counts: HashMap::new(),
            initialized_user_agent: Some("test-client".to_string()),
            last_event_at: Some("2026-07-24T00:00:00Z".to_string()),
            last_error: None,
        };
        let mut entry = SessionProjectionEntry::default();
        let activity = SessionActivityState {
            revision: 4,
            runtime_generation: Some(7),
            last_output_at: Some("2026-07-24T00:00:04Z".to_string()),
            last_output_completed_at: Some("2026-07-24T00:00:05Z".to_string()),
            last_turn_completed_at: Some("2026-07-24T00:00:06Z".to_string()),
            last_file_change_at: Some("2026-07-24T00:00:03Z".to_string()),
            last_output: Some(crate::observability::SafeOutputProjection {
                association: crate::observability::ObservationAssociation::session(
                    "cutex-test-session",
                )
                .with_task("assignment-1".to_string(), Some(2)),
                class: crate::observability::SafeOutputClass::FinalVisible,
                display_text: "visible result".to_string(),
                updated_at: "2026-07-24T00:00:05Z".to_string(),
                runtime_generation: 7,
            }),
            last_tool_call: Some(crate::observability::SafeToolCallProjection {
                association: crate::observability::ObservationAssociation::session(
                    "cutex-test-session",
                )
                .with_task("assignment-1".to_string(), Some(2)),
                class: crate::observability::SafeToolCallClass::Command,
                status: crate::observability::SafeToolCallStatus::Finished,
                display_text: "Command".to_string(),
                updated_at: "2026-07-24T00:00:04Z".to_string(),
                runtime_generation: 7,
            }),
            ..SessionActivityState::default()
        };

        let resource = project_session_entry(
            &record,
            true,
            Some(runtime_status.clone()),
            Some(&activity),
            &json!({}),
            &mut entry,
        )
        .expect("session resource");

        assert_eq!(
            resource.pointer("/runtime/activeTurnId"),
            Some(&json!("turn-active"))
        );
        assert_eq!(
            resource.pointer("/runtime/activeTurnObservedAt"),
            Some(&json!("2026-07-24T00:00:03Z"))
        );
        assert_eq!(
            resource.pointer("/runtime/threadStatus"),
            Some(&json!({
                "type": "active",
                "activeFlags": ["waitingOnApproval"]
            }))
        );
        assert_eq!(
            resource.pointer("/runtime/threadStatusObservedAt"),
            Some(&json!("2026-07-24T00:00:02Z"))
        );
        assert_eq!(
            resource.pointer("/runtime/threadSettings"),
            Some(&json!({
                "model": "gpt-test",
                "activePermissionProfile": { "id": ":workspace" },
                "collaborationMode": {
                    "mode": "plan",
                    "settings": {
                        "model": "",
                        "reasoning_effort": "medium",
                        "developer_instructions": null
                    }
                }
            }))
        );
        assert_eq!(
            resource.pointer("/runtime/threadSettingsSource"),
            Some(&json!("thread/settings/updated"))
        );
        assert_eq!(
            resource.pointer("/runtime/threadSettingsComplete"),
            Some(&json!(true))
        );
        assert_eq!(
            resource.pointer("/runtime/threadSettingsObservedAt"),
            Some(&json!("2026-07-24T00:00:01Z"))
        );
        assert_eq!(
            resource.pointer("/runtime/runtimeWorkspaceRoots"),
            Some(&json!(["/workspace", "/shared"]))
        );
        assert_eq!(
            resource.pointer("/runtime/instructionSources"),
            Some(&json!(["/workspace/AGENTS.md"]))
        );
        assert_eq!(
            resource.pointer("/runtime/resumeSnapshotObservedAt"),
            Some(&json!("2026-07-24T00:00:00Z"))
        );
        assert_eq!(
            resource.get("activity"),
            Some(&json!({
                "revision": 4,
                "runtimeGeneration": 7,
                "lastOutputAt": "2026-07-24T00:00:04Z",
                "lastOutputCompletedAt": "2026-07-24T00:00:05Z",
                "lastTurnCompletedAt": "2026-07-24T00:00:06Z",
                "lastFileChangeAt": "2026-07-24T00:00:03Z",
                "lastOutput": {
                    "cutexSessionId": "cutex-test-session",
                    "assignmentId": "assignment-1",
                    "attemptNumber": 2,
                    "class": "final_visible",
                    "displayText": "visible result",
                    "updatedAt": "2026-07-24T00:00:05Z",
                    "runtimeGeneration": 7
                },
                "lastToolCall": {
                    "cutexSessionId": "cutex-test-session",
                    "assignmentId": "assignment-1",
                    "attemptNumber": 2,
                    "class": "command",
                    "status": "finished",
                    "displayText": "Command",
                    "updatedAt": "2026-07-24T00:00:04Z",
                    "runtimeGeneration": 7
                }
            }))
        );

        let mut later_activity = activity;
        later_activity.revision = 5;
        later_activity.last_output_at = Some("2026-07-24T00:00:07Z".to_string());
        let later_resource = project_session_entry(
            &record,
            true,
            Some(runtime_status),
            Some(&later_activity),
            &json!({}),
            &mut entry,
        )
        .expect("session resource with later activity");
        assert_eq!(entry.revision, 1);
        assert_eq!(later_resource["revision"], record.durable_revision());
        assert_eq!(
            later_resource.pointer("/activity/lastOutputAt"),
            Some(&json!("2026-07-24T00:00:07Z"))
        );
    }

    #[test]
    fn tui_attachment_is_not_inferred_from_the_app_server_process() {
        let mut record =
            CutexSessionRecord::from_codex_session_id("thread-tui").expect("durable test session");
        record.runtime_pid = Some(std::process::id());
        assert!(!tui_is_known_attached(&record, true));

        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.alden_pid = Some(std::process::id());
        assert!(tui_is_known_attached(&record, true));
        assert!(!tui_is_known_attached(&record, false));
    }

    #[test]
    fn legacy_known_launch_projects_unknown_source_without_relabeling() {
        let mut record = CutexSessionRecord::from_codex_session_id("thread-legacy-profile")
            .expect("durable test session");
        record.cutex_session_id = "cutex-legacy-profile".to_string();
        record.profile = Some("configured-profile".to_string());
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.app_server_runtime = Some(crate::session::model::CutexAppServerRuntimeBinding {
            transport: crate::session::model::CutexAppServerTransport::UnixSocket,
            endpoint: "unix:///tmp/legacy-profile.sock".to_string(),
            pid: 4242,
            runtime_dir: "/tmp/legacy-profile".to_string(),
            launched_profile: Some("legacy-profile".to_string()),
            launch_profile_source: None,
            auth_token_path: None,
            diagnostic_journal_path: "/tmp/legacy-profile.jsonl".to_string(),
            schema_version: "test".to_string(),
            schema_sha256: "hash".to_string(),
            started_at: "2026-08-15T00:00:00Z".to_string(),
        });
        let mut entry = SessionProjectionEntry::default();
        let resource = project_session_entry(&record, true, None, None, &json!({}), &mut entry)
            .expect("legacy projection");
        assert_eq!(resource["launchedProfile"], "legacy-profile");
        assert_eq!(resource["launchProfileSource"], "unknown");
        assert_eq!(resource["configuredProfile"], "configured-profile");
    }

    #[test]
    fn profile_projection_distinguishes_durable_intent_global_fallback_and_unavailable_launch() {
        let previous_home = std::env::var_os("HOME");
        let isolated_home = IsolatedTestHome::new("cutex-profile-projection-test")
            .expect("create isolated test HOME");
        let mut record = CutexSessionRecord::from_codex_session_id("thread-profile-projection")
            .expect("durable test session");
        record.cutex_session_id = "cutex-profile-projection".to_string();
        let mut entry = SessionProjectionEntry::default();

        crate::config::store::save_codez_config(&crate::profiles::model::CodezConfig::default())
            .expect("clear global profile default");
        record.profile = Some("configured".to_string());
        let configured = project_session_entry(&record, true, None, None, &json!({}), &mut entry)
            .expect("configured projection");
        assert_eq!(configured["profile"], "configured");
        assert_eq!(configured["configuredProfile"], "configured");
        assert_eq!(
            configured["effectiveNextLaunch"],
            json!({ "profile": "configured", "source": "session_configured" })
        );

        record.profile = None;
        let global = crate::profiles::model::CodezConfig {
            default_profile: Some("global".to_string()),
            ..Default::default()
        };
        crate::config::store::save_codez_config(&global).expect("save global profile default");
        let fallback = project_session_entry(&record, true, None, None, &json!({}), &mut entry)
            .expect("global fallback projection");
        assert_eq!(fallback["profile"], Value::Null);
        assert_eq!(
            fallback["effectiveNextLaunch"],
            json!({ "profile": "global", "source": "global_default" })
        );

        crate::config::store::save_codez_config(&crate::profiles::model::CodezConfig::default())
            .expect("clear global profile default again");
        let unavailable = project_session_entry(&record, true, None, None, &json!({}), &mut entry)
            .expect("unavailable projection");
        assert_eq!(
            unavailable["effectiveNextLaunch"],
            json!({ "profile": Value::Null, "source": "unavailable" })
        );
        assert!(
            isolated_home.root().join(".cutex/config.json").is_file(),
            "the test config must stay inside the isolated HOME"
        );
        drop(isolated_home);
        assert_eq!(
            std::env::var_os("HOME"),
            previous_home,
            "the isolated HOME guard must restore the process environment"
        );
    }

    #[test]
    fn hidden_durable_sessions_remain_addressable_but_ephemeral_agents_do_not() {
        let registry = ImRegistry::default();
        let mut durable = CutexSessionRecord::from_codex_session_id("thread-hidden")
            .expect("durable test session");
        durable.registration_class = AgentRegistrationClass::Persistent;
        durable.exposed_to_backend = false;

        assert!(!session_is_visible(&registry, &durable));
        assert!(is_durable_management_session(&durable));

        durable.registration_class = AgentRegistrationClass::Ephemeral;
        assert!(!is_durable_management_session(&durable));
    }

    #[test]
    fn retired_projection_is_archive_only_and_forces_offline_runtime_identity() {
        let mut record = CutexSessionRecord::from_codex_session_id("thread-retired")
            .expect("durable test session");
        record.cutex_session_id = "cutex-retired".to_string();
        record.registration_class = AgentRegistrationClass::Persistent;
        record.archive_state = CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        record.revision = 8;
        record.runtime_generation = 4;
        record.current_runtime_agent_id = Some("stale-runtime".to_string());

        let resource = retired_session_resource_value(&record, None);

        assert_eq!(resource["lifecycle"], "retired");
        assert_eq!(resource["retiredAt"], "2026-08-10T00:01:00Z");
        assert_eq!(resource["revision"], 8);
        assert_eq!(resource["runtime"]["status"], "offline");
        assert!(resource["runtime"]["runtimeAgentId"].is_null());
        assert!(resource.get("visible").is_none());
        assert!(resource.get("management").is_none());
        assert_eq!(
            resource.get("activity"),
            Some(&json!({
                "revision": 0,
                "runtimeGeneration": null,
                "lastOutputAt": null,
                "lastOutputCompletedAt": null,
                "lastTurnCompletedAt": null,
                "lastFileChangeAt": null
            }))
        );
    }
}
