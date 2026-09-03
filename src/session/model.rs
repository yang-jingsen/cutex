//! Durable cutex session value types.

use serde::Deserialize;
use serde::Serialize;

use std::cell::Cell;
use std::collections::HashMap;

use chrono::Utc;

use crate::agent_bus::model::AgentRegistrationClass;
use crate::platform::host::current_host_name;
use crate::session::identity::default_cutex_session_id_for_codex_session;
use crate::session::identity::normalize_codex_session_id;
use crate::session::identity::normalize_cutex_session_id;

/// Largest revision that can be represented exactly by every JSON consumer.
pub const MAX_DURABLE_SESSION_REVISION: u64 = 9_007_199_254_740_991;

fn default_durable_session_revision() -> u64 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CutexSessionArchiveState {
    #[default]
    Active,
    Retired,
}

impl CutexSessionArchiveState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CutexSessionQuickActionMode {
    #[default]
    Auto,
    Pinned,
    Hidden,
}

impl CutexSessionQuickActionMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pinned => "pinned",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutexSessionUserAction {
    Attach,
    Takeover,
    Online,
    ResumeAttach,
    ResumeHere,
    ResumeManaged,
    KillAndResume,
}

impl CutexSessionUserAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Takeover => "takeover",
            Self::Online => "online",
            Self::ResumeAttach => "resume-attach",
            Self::ResumeHere => "resume-here",
            Self::ResumeManaged => "resume-managed",
            Self::KillAndResume => "kill-and-resume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CutexSessionRuntimeBackend {
    #[default]
    Host,
    HostForeground,
    Docker,
    CuteAlden,
    Future,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutexAppServerTransport {
    UnixSocket,
    LoopbackWebSocket,
}

/// Immutable provenance for the profile selected for one managed runtime
/// occurrence.  This is deliberately distinct from `CutexSessionRecord::profile`,
/// which is durable configured intent for future launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchProfileSource {
    OneLaunchOverride,
    SessionConfigured,
    GlobalDefault,
    Unknown,
}

impl LaunchProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OneLaunchOverride => "one_launch_override",
            Self::SessionConfigured => "session_configured",
            Self::GlobalDefault => "global_default",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutexAppServerRuntimeBinding {
    pub transport: CutexAppServerTransport,
    pub endpoint: String,
    pub pid: u32,
    pub runtime_dir: String,
    /// Profile actually used to launch this app-server occurrence. `None` is
    /// retained for legacy bindings whose launch evidence predates this field;
    /// it must never be inferred from the current global default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched_profile: Option<String>,
    /// Source captured at occurrence creation. A missing value is legacy
    /// evidence and projects as `unknown`; it is never inferred later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_profile_source: Option<LaunchProfileSource>,
    #[serde(default)]
    pub auth_token_path: Option<String>,
    pub diagnostic_journal_path: String,
    pub schema_version: String,
    pub schema_sha256: String,
    pub started_at: String,
}

pub fn normalize_runtime_token(value: &str) -> String {
    value.trim().replace(['_', ' '], "-").to_ascii_lowercase()
}

pub fn parse_cutex_session_runtime_backend(
    value: &str,
) -> anyhow::Result<CutexSessionRuntimeBackend> {
    match normalize_runtime_token(value).as_str() {
        "host" | "local" => Ok(CutexSessionRuntimeBackend::Host),
        "host-foreground" | "host_foreground" | "foreground" | "native" | "windows-native"
        | "windows_native" | "host-fg" | "host_fg" => {
            Ok(CutexSessionRuntimeBackend::HostForeground)
        }
        "docker" => Ok(CutexSessionRuntimeBackend::Docker),
        "cute-alden" | "alden" | "background" | "attachable" => {
            Ok(CutexSessionRuntimeBackend::CuteAlden)
        }
        "future" => Ok(CutexSessionRuntimeBackend::Future),
        other => anyhow::bail!("unsupported runtime_backend: {other}"),
    }
}

pub fn parse_cutex_session_quick_action_mode(
    value: &str,
) -> anyhow::Result<CutexSessionQuickActionMode> {
    match normalize_runtime_token(value).as_str() {
        "auto" | "automatic" => Ok(CutexSessionQuickActionMode::Auto),
        "pin" | "pinned" => Ok(CutexSessionQuickActionMode::Pinned),
        "hide" | "hidden" => Ok(CutexSessionQuickActionMode::Hidden),
        other => anyhow::bail!("unsupported quick action mode: {other}"),
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CutexSessionStore {
    /// Internal compare-and-swap generation for cross-process writers.
    #[serde(default, rename = "storeRevision")]
    pub store_revision: Cell<u64>,
    #[serde(default)]
    pub sessions: HashMap<String, CutexSessionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CutexSessionRecord {
    pub cutex_session_id: String,
    /// Durable specification/lifecycle revision.  Runtime occurrence changes
    /// are fenced separately by `runtime_generation`.
    #[serde(
        default = "default_durable_session_revision",
        alias = "durable_revision",
        alias = "durableRevision"
    )]
    pub revision: u64,
    #[serde(default, rename = "lifecycle", alias = "archive_state")]
    pub archive_state: CutexSessionArchiveState,
    #[serde(default, rename = "retiredAt", alias = "retired_at")]
    pub retired_at: Option<String>,
    #[serde(default)]
    pub codex_session_id: Option<String>,
    #[serde(default)]
    // Native launch correlation reported by historical heartbeat flows.
    pub pending_launch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // Crash fence owned only by a Management app-server launch attempt.
    pub app_server_launch_claim_id: Option<String>,
    #[serde(default)]
    pub thread_name: Option<String>,
    #[serde(default)]
    pub display_name_hint: Option<String>,
    pub host_id: String,
    pub cwd: String,
    #[serde(default)]
    pub managed_cwd: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub runtime_backend: CutexSessionRuntimeBackend,
    #[serde(default)]
    pub agent_enabled: bool,
    #[serde(default)]
    pub agent_groups: Vec<String>,
    #[serde(default)]
    pub registration_class: AgentRegistrationClass,
    #[serde(default)]
    pub exposed_to_backend: bool,
    #[serde(default)]
    pub quick_action: CutexSessionQuickActionMode,
    #[serde(default)]
    pub default_cli_args: Vec<String>,
    #[serde(default)]
    pub permission_defaults: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<String>,
    #[serde(default)]
    pub sandbox_mode: Option<String>,
    #[serde(default)]
    pub model_defaults: Option<String>,
    #[serde(default)]
    pub reasoning_defaults: Option<String>,
    #[serde(default)]
    pub alden_session_name: Option<String>,
    #[serde(default)]
    pub alden_pid: Option<u32>,
    #[serde(default)]
    pub runtime_pid: Option<u32>,
    #[serde(default)]
    pub app_server_runtime: Option<CutexAppServerRuntimeBinding>,
    #[serde(default)]
    pub current_runtime_agent_id: Option<String>,
    #[serde(default)]
    pub runtime_generation: u64,
    #[serde(default)]
    pub last_runtime_agent_id: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub last_user_selected_at: Option<String>,
    #[serde(default)]
    pub last_user_action: Option<CutexSessionUserAction>,
    pub created_at: String,
    pub updated_at: String,
}

impl CutexSessionRecord {
    #[allow(dead_code)]
    pub fn new(
        cutex_session_id: String,
        codex_session_id: Option<String>,
        host_id: String,
        cwd: String,
        profile: Option<String>,
    ) -> anyhow::Result<Self> {
        let now = Utc::now().to_rfc3339();
        Self::new_at(
            cutex_session_id,
            codex_session_id,
            host_id,
            cwd,
            profile,
            now,
        )
    }

    pub fn new_at(
        cutex_session_id: String,
        codex_session_id: Option<String>,
        host_id: String,
        cwd: String,
        profile: Option<String>,
        timestamp: String,
    ) -> anyhow::Result<Self> {
        let cutex_session_id = normalize_cutex_session_id(&cutex_session_id)?;
        let codex_session_id = codex_session_id
            .map(|value| normalize_codex_session_id(&value))
            .transpose()?;
        Ok(Self {
            cutex_session_id,
            revision: default_durable_session_revision(),
            archive_state: CutexSessionArchiveState::Active,
            retired_at: None,
            codex_session_id,
            pending_launch_id: None,
            app_server_launch_claim_id: None,
            thread_name: None,
            display_name_hint: None,
            host_id,
            cwd,
            managed_cwd: None,
            profile,
            runtime_backend: CutexSessionRuntimeBackend::Host,
            agent_enabled: false,
            agent_groups: Vec::new(),
            registration_class: AgentRegistrationClass::LocalOnly,
            exposed_to_backend: false,
            quick_action: CutexSessionQuickActionMode::Auto,
            default_cli_args: Vec::new(),
            permission_defaults: None,
            approval_policy: None,
            sandbox_mode: None,
            model_defaults: None,
            reasoning_defaults: None,
            alden_session_name: None,
            alden_pid: None,
            runtime_pid: None,
            app_server_runtime: None,
            current_runtime_agent_id: None,
            runtime_generation: 0,
            last_runtime_agent_id: None,
            last_seen_at: None,
            last_user_selected_at: None,
            last_user_action: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        })
    }

    #[allow(dead_code)]
    pub fn from_codex_session_id(codex_session_id: &str) -> anyhow::Result<Self> {
        let codex_session_id = normalize_codex_session_id(codex_session_id)?;
        let cutex_session_id = default_cutex_session_id_for_codex_session(&codex_session_id);
        let cwd = std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self::new(
            cutex_session_id,
            Some(codex_session_id),
            current_host_name(),
            cwd,
            None,
        )
    }

    pub fn is_retired(&self) -> bool {
        self.archive_state == CutexSessionArchiveState::Retired
    }

    pub fn is_active(&self) -> bool {
        !self.is_retired()
    }

    pub fn durable_revision(&self) -> u64 {
        if self.revision == 0 {
            default_durable_session_revision()
        } else {
            self.revision
        }
    }

    pub fn bump_durable_revision(&mut self) -> anyhow::Result<u64> {
        let current = self.durable_revision();
        self.revision = current
            .checked_add(1)
            .filter(|revision| *revision <= MAX_DURABLE_SESSION_REVISION)
            .ok_or_else(|| anyhow::anyhow!("cutex session durable revision exhausted"))?;
        Ok(self.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cutex_session_runtime_backend_accepts_common_aliases() {
        assert_eq!(
            parse_cutex_session_runtime_backend("host").expect("host should parse"),
            CutexSessionRuntimeBackend::Host
        );
        assert_eq!(
            parse_cutex_session_runtime_backend("host_foreground")
                .expect("host_foreground should parse"),
            CutexSessionRuntimeBackend::HostForeground
        );
        assert_eq!(
            parse_cutex_session_runtime_backend("native").expect("native should parse"),
            CutexSessionRuntimeBackend::HostForeground
        );
        assert_eq!(
            parse_cutex_session_runtime_backend("cute-alden").expect("cute-alden should parse"),
            CutexSessionRuntimeBackend::CuteAlden
        );
    }

    #[test]
    fn parse_cutex_session_quick_action_mode_accepts_common_aliases() {
        assert_eq!(
            parse_cutex_session_quick_action_mode("auto").expect("auto should parse"),
            CutexSessionQuickActionMode::Auto
        );
        assert_eq!(
            parse_cutex_session_quick_action_mode("pinned").expect("pinned should parse"),
            CutexSessionQuickActionMode::Pinned
        );
        assert_eq!(
            parse_cutex_session_quick_action_mode("hide").expect("hide should parse"),
            CutexSessionQuickActionMode::Hidden
        );
    }

    #[test]
    fn legacy_store_defaults_generation_and_writes_current_name() {
        let store: CutexSessionStore = serde_json::from_value(serde_json::json!({
            "sessions": {}
        }))
        .expect("deserialize legacy store");
        assert_eq!(store.store_revision.get(), 0);

        let current = serde_json::to_value(store).expect("serialize current store");
        assert_eq!(current["storeRevision"], 0);
        assert!(current.get("store_revision").is_none());
    }

    #[test]
    fn session_record_without_app_server_binding_still_deserializes() {
        let record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            None,
            "host-1".to_string(),
            "/tmp/worktree".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("create session record");
        let mut value = serde_json::to_value(record).expect("serialize session record");
        value
            .as_object_mut()
            .expect("session record should be an object")
            .remove("app_server_runtime");

        let restored: CutexSessionRecord =
            serde_json::from_value(value).expect("deserialize legacy session record");
        assert!(restored.app_server_runtime.is_none());
    }

    #[test]
    fn pre_archive_session_record_defaults_to_active_revision_one() {
        let record = CutexSessionRecord::new_at(
            "cutex-legacy".to_string(),
            None,
            "host-1".to_string(),
            "/tmp/worktree".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("create session record");
        let mut value = serde_json::to_value(record).expect("serialize session record");
        let object = value
            .as_object_mut()
            .expect("session record should be an object");
        object.remove("revision");
        object.remove("lifecycle");
        object.remove("retiredAt");

        let restored: CutexSessionRecord =
            serde_json::from_value(value).expect("deserialize legacy session record");

        assert_eq!(restored.durable_revision(), 1);
        assert_eq!(restored.archive_state, CutexSessionArchiveState::Active);
        assert_eq!(restored.retired_at, None);
    }

    #[test]
    fn archive_fields_read_legacy_aliases_and_write_exact_current_names() {
        let record = CutexSessionRecord::new_at(
            "cutex-archive-alias".to_string(),
            None,
            "host-1".to_string(),
            "/tmp/worktree".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("create session record");
        let mut value = serde_json::to_value(record).expect("serialize session record");
        let object = value
            .as_object_mut()
            .expect("session record should be an object");
        object.remove("revision");
        object.remove("lifecycle");
        object.remove("retiredAt");
        object.insert("durable_revision".to_string(), serde_json::json!(7));
        object.insert("archive_state".to_string(), serde_json::json!("retired"));
        object.insert(
            "retired_at".to_string(),
            serde_json::json!("2026-07-10T00:01:00Z"),
        );

        let restored: CutexSessionRecord =
            serde_json::from_value(value).expect("deserialize aliased session record");
        assert_eq!(restored.durable_revision(), 7);
        assert_eq!(restored.archive_state, CutexSessionArchiveState::Retired);
        assert_eq!(restored.retired_at.as_deref(), Some("2026-07-10T00:01:00Z"));

        let current = serde_json::to_value(restored).expect("serialize current session record");
        let current = current.as_object().expect("current record object");
        assert_eq!(current.get("revision"), Some(&serde_json::json!(7)));
        assert_eq!(
            current.get("lifecycle"),
            Some(&serde_json::json!("retired"))
        );
        assert_eq!(
            current.get("retiredAt"),
            Some(&serde_json::json!("2026-07-10T00:01:00Z"))
        );
        assert!(!current.contains_key("durable_revision"));
        assert!(!current.contains_key("archive_state"));
        assert!(!current.contains_key("retired_at"));
    }

    #[test]
    fn active_session_serializes_nullable_retired_at() {
        let record = CutexSessionRecord::new_at(
            "cutex-active".to_string(),
            None,
            "host-1".to_string(),
            "/tmp/worktree".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("create session record");
        let value = serde_json::to_value(record).expect("serialize session record");

        assert_eq!(value["revision"], 1);
        assert_eq!(value["lifecycle"], "active");
        assert!(value
            .get("retiredAt")
            .is_some_and(serde_json::Value::is_null));
    }

    #[test]
    fn legacy_pending_launch_id_does_not_materialize_an_app_server_claim() {
        let record = CutexSessionRecord::new_at(
            "cutex-1".to_string(),
            None,
            "host-1".to_string(),
            "/tmp/worktree".to_string(),
            None,
            "2026-07-10T00:00:00Z".to_string(),
        )
        .expect("create session record");
        let mut value = serde_json::to_value(record).expect("serialize session record");
        let object = value
            .as_object_mut()
            .expect("session record should be an object");
        object.remove("app_server_launch_claim_id");
        object.insert(
            "pending_launch_id".to_string(),
            serde_json::Value::String("legacy-heartbeat-launch".to_string()),
        );

        let restored: CutexSessionRecord =
            serde_json::from_value(value).expect("deserialize legacy session record");

        assert_eq!(
            restored.pending_launch_id.as_deref(),
            Some("legacy-heartbeat-launch")
        );
        assert!(restored.app_server_launch_claim_id.is_none());
    }

    #[test]
    fn legacy_runtime_binding_without_launched_profile_is_unknown() {
        let binding = serde_json::json!({
            "transport": "unix_socket",
            "endpoint": "unix:///tmp/runtime/app.sock",
            "pid": 4242,
            "runtime_dir": "/tmp/runtime",
            "diagnostic_journal_path": "/tmp/runtime/events.jsonl",
            "schema_version": "test",
            "schema_sha256": "hash",
            "started_at": "2026-07-10T00:00:00Z"
        });
        let restored: CutexAppServerRuntimeBinding =
            serde_json::from_value(binding).expect("legacy binding should deserialize");
        assert_eq!(restored.launched_profile, None);
        assert_eq!(restored.launch_profile_source, None);
        let serialized = serde_json::to_value(&restored).expect("legacy binding should serialize");
        assert!(serialized.get("launched_profile").is_none());
        assert!(serialized.get("launch_profile_source").is_none());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutexSessionReconcileOutcome {
    pub cutex_session_id: String,
    pub codex_session_id: String,
    pub events: Vec<CutexSessionReconcileEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutexSessionReconcileEvent {
    pub event_type: &'static str,
    pub summary: String,
    pub previous_runtime_agent_id: Option<String>,
    pub runtime_agent_id: Option<String>,
    pub previous_cutex_session_id: Option<String>,
}
