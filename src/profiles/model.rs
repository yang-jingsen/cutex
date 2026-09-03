//! Profile, account, auth, and global cutex config data models.

use std::collections::HashMap;

use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

pub const STORE_VERSION: u32 = 3;
pub const DEFAULT_AGENT_MESSAGE_PREFIX_TEMPLATE: &str = "[message from {from}] ";

/// A persisted Management API root credential whose diagnostic form never
/// reveals the credential bytes.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct ManagementApiToken(String);

impl ManagementApiToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ManagementApiToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ManagementApiToken([REDACTED])")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CliKind {
    Codex,
    Claude,
}

impl Default for CliKind {
    fn default() -> Self {
        CliKind::Codex
    }
}

impl std::fmt::Display for CliKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliKind::Codex => write!(f, "codex"),
            CliKind::Claude => write!(f, "claude"),
        }
    }
}

impl std::str::FromStr for CliKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "codex" => Ok(CliKind::Codex),
            "claude" => Ok(CliKind::Claude),
            _ => Err(anyhow!(
                "unknown CLI kind: {s} (expected 'codex' or 'claude')"
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AccountsStore {
    pub version: u32,
    pub accounts: Vec<StoredAccount>,
    pub active_account_id: Option<String>,
}

impl Default for AccountsStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            accounts: Vec::new(),
            active_account_id: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StoredAccount {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
    #[serde(default)]
    pub cli_kind: CliKind,
    #[serde(default)]
    pub default_cli_args: Vec<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl StoredAccount {
    pub fn from_import(name: String, snapshot: &ImportedSnapshot, runtime: RuntimeConfig) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            email: snapshot.email.clone(),
            plan_type: snapshot.plan_type.clone(),
            source: Some(snapshot.source.clone()),
            runtime,
            proxy: None,
            session: None,
            cli_kind: CliKind::default(),
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LegacyAccountsStoreV2 {
    #[serde(default)]
    pub accounts: Vec<LegacyStoredAccountV2>,
    pub active_account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LegacyStoredAccountV2 {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub raw_auth_json: Option<String>,
    #[serde(default)]
    pub raw_config_toml: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthData>,
    #[serde(default)]
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub session: Option<SessionConfig>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RuntimeConfig {
    Host,
    Docker {
        image: String,
        #[serde(default)]
        user_name: Option<String>,
    },
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::Host
    }
}

pub fn runtime_label(runtime: &RuntimeConfig) -> &'static str {
    match runtime {
        RuntimeConfig::Host => "host",
        RuntimeConfig::Docker { .. } => "docker",
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum AuthData {
    ApiKey {
        key: String,
    },
    ChatGPT {
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct QuickRunState {
    pub last_global_profile: Option<String>,
    pub per_directory: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodezConfig {
    #[serde(default)]
    pub docker_use_sudo: bool,
    #[serde(default)]
    pub custom_status_items: Vec<CustomStatusItemCatalogEntry>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default = "default_session_config")]
    pub session: SessionConfig,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub default_profile_direct_launch: bool,
    #[serde(default)]
    pub notify_service_url: Option<String>,
    #[serde(default)]
    pub notify_service_token: Option<String>,
    #[serde(default)]
    pub notify_service_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub notify_service_composer_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub notify_service_approval_timeout_secs: Option<u64>,
    #[serde(default)]
    pub notify_service_startup_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub notify_service_events: Option<Vec<String>>,
    #[serde(default)]
    pub notify_service_user_message_content: Option<String>,
    #[serde(default)]
    pub notify_service_user_message_preview_chars: Option<u64>,
    #[serde(default)]
    pub rate_limit_threshold_warning_mode: Option<String>,
    #[serde(default)]
    pub rate_limit_model_nudge_mode: Option<String>,
    #[serde(default)]
    pub desktop_notify_enabled: bool,
    #[serde(default)]
    pub desktop_notify_port: Option<u16>,
    #[serde(default)]
    pub desktop_notify_token: Option<String>,
    #[serde(default = "default_true")]
    pub agent_bus_enabled: bool,
    #[serde(default)]
    pub agent_bus_port: Option<u16>,
    #[serde(default)]
    pub agent_bus_token: Option<String>,
    #[serde(default)]
    pub management_api_token: Option<ManagementApiToken>,
    /// Dedicated project-scoped read credentials. They never authorize
    /// ordinary Management, Agent Bus, or Task mutation routes.
    #[serde(default)]
    pub owner_task_read_credentials: Vec<crate::task_service::OwnerTaskReadCredential>,
    #[serde(default = "default_agent_message_prefix_template")]
    pub agent_message_prefix_template: Option<String>,
    #[serde(default)]
    pub agent_message_suffix_template: Option<String>,
}

impl Default for CodezConfig {
    fn default() -> Self {
        Self {
            docker_use_sudo: false,
            custom_status_items: Vec::new(),
            proxy: None,
            session: default_session_config(),
            default_profile: None,
            default_profile_direct_launch: false,
            notify_service_url: None,
            notify_service_token: None,
            notify_service_idle_timeout_secs: None,
            notify_service_composer_idle_timeout_secs: None,
            notify_service_approval_timeout_secs: None,
            notify_service_startup_idle_timeout_secs: None,
            notify_service_events: None,
            notify_service_user_message_content: None,
            notify_service_user_message_preview_chars: None,
            rate_limit_threshold_warning_mode: None,
            rate_limit_model_nudge_mode: None,
            desktop_notify_enabled: false,
            desktop_notify_port: None,
            desktop_notify_token: None,
            agent_bus_enabled: true,
            agent_bus_port: None,
            agent_bus_token: None,
            management_api_token: None,
            owner_task_read_credentials: Vec::new(),
            agent_message_prefix_template: default_agent_message_prefix_template(),
            agent_message_suffix_template: None,
        }
    }
}

fn default_agent_message_prefix_template() -> Option<String> {
    Some(DEFAULT_AGENT_MESSAGE_PREFIX_TEMPLATE.to_string())
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub no_proxy: Option<String>,
    #[serde(default = "default_true")]
    pub force_http_transport: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomStatusItemsCatalogFile {
    #[serde(default)]
    pub items: Vec<CustomStatusItemCatalogEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomStatusItemCatalogEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source: CustomStatusItemSource,
    #[serde(default)]
    pub render: CustomStatusItemRender,
    #[serde(default)]
    pub style: CustomStatusItemStyle,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CustomStatusItemSource {
    Static {
        value: String,
    },
    Env {
        key: String,
        #[serde(default = "default_true")]
        trim: bool,
    },
    FileText {
        path: String,
        #[serde(default = "default_true")]
        trim: bool,
    },
    LaunchProfile,
    LaunchRuntime,
    LaunchProfileSource,
    LaunchProfileType,
    LaunchProfileEmail,
    CurrentDir,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CustomStatusItemRender {
    Value,
    LabelValue { label: String },
    Template { template: String },
}

impl Default for CustomStatusItemRender {
    fn default() -> Self {
        Self::Value
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CustomStatusItemStyle {
    #[serde(default)]
    pub fg: Option<String>,
    #[serde(default)]
    pub bg: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underlined: bool,
}

fn default_true() -> bool {
    true
}

fn default_session_config() -> SessionConfig {
    SessionConfig { enabled: false }
}

#[derive(Debug)]
pub struct ImportedSnapshot {
    pub raw_auth_json: String,
    pub raw_config_toml: Option<String>,
    pub raw_model_catalog_json: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct MaterializedAccountFiles {
    pub auth_path: PathBuf,
    pub config_path: PathBuf,
    pub model_catalog_path: PathBuf,
    pub custom_status_items_path: PathBuf,
}
