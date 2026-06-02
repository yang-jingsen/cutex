use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use clap::{ArgAction, Parser, Subcommand};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use toml::value::Table;
use url::Url;
use uuid::Uuid;

const STORE_VERSION: u32 = 3;
const CODEZ_BUILD: &str = "2026-06-02-api-key-provider-v57";

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const BLUE: &str = "\x1b[34m";
const MAGENTA: &str = "\x1b[35m";
const CYAN: &str = "\x1b[36m";
const CODEX_CONFIG_FILE_ENV_VAR: &str = "CODEX_CONFIG_FILE";
const CODEX_AUTH_FILE_ENV_VAR: &str = "CODEX_AUTH_FILE";
const CUTEX_CODEX_BIN_ENV_VAR: &str = "CUTEX_CODEX_BIN";
const CODEZ_CODEX_BIN_ENV_VAR: &str = "CODEZ_CODEX_BIN";
const CUTEX_CLAUDE_BIN_ENV_VAR: &str = "CUTEX_CLAUDE_BIN";
const CLAUDE_CONFIG_DIR_ENV_VAR: &str = "CLAUDE_CONFIG_DIR";
const CUTEX_ALDEN_BIN_ENV_VAR: &str = "CUTEX_ALDEN_BIN";
const CUTEX_DOCKER_USE_SUDO_ENV_VAR: &str = "CUTEX_DOCKER_USE_SUDO";
const CODEZ_DOCKER_USE_SUDO_ENV_VAR: &str = "CODEZ_DOCKER_USE_SUDO";
const CODEX_CUSTOM_STATUS_ITEMS_FILE_ENV_VAR: &str = "CODEX_CUSTOM_STATUS_ITEMS_FILE";
const CODEX_INSTALL_DIR_ENV_VAR: &str = "CODEX_INSTALL_DIR";
const CUTE_CODEX_FORCE_HTTP_TRANSPORT_ENV_VAR: &str = "CUTE_CODEX_FORCE_HTTP_TRANSPORT";
const CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR: &str = "CODEX_NOTIFY_IDLE_TIMEOUT";
const CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR: &str = "CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT";
const CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR: &str = "CODEX_NOTIFY_APPROVAL_TIMEOUT";
const CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR: &str = "CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT";
const CODEX_NOTIFY_EVENTS_ENV_VAR: &str = "CODEX_NOTIFY_EVENTS";
const CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR: &str = "CODEX_NOTIFY_USER_MESSAGE_CONTENT";
const CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR: &str =
    "CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS";
const CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR: &str =
    "CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE";
const CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR: &str = "CODEX_RATE_LIMIT_MODEL_NUDGE_MODE";
const DOCKER_PROXY_HOST_ALIAS: &str = "host.docker.internal";
const DEFAULT_NOTIFY_EVENTS: &str =
    "task_completed,thinking_too_long,waiting_approval,connection_error,session_exit,session_started,session_startup_idle,user_message_sent,user_message_dispatched,turn_started,turn_completed,turn_interrupted,turn_failed,approval_requested,approval_resolved,thread_closed,context_compacted,rate_limit_warning,rate_limit_prompt_shown";
const DEFAULT_DESKTOP_NOTIFY_PORT: u16 = 24250;
const DESKTOP_NOTIFY_BRIDGE_ID: &str = "cutex-desktop-notify";

/// cutex - profile launcher for cute-codex
#[derive(Parser, Debug)]
#[command(
    name = "cutex",
    about = "Profile launcher and configuration wizard for cute-codex",
    version,
    subcommand_required = false,
    arg_required_else_help = false,
    after_help = "CLI selection order: CUTEX_CODEX_BIN / CODEZ_CODEX_BIN override, then cute-codex, then cutex-codex, then codex."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandKind>,

    /// Use the default profile without interactive selection when
    /// running without an explicit subcommand.
    #[arg(short = 'q', long = "quick")]
    quick: bool,

    /// Force this invocation to run the selected CLI on the host, even for Docker profiles.
    #[arg(long = "host")]
    host: bool,

    /// When no subcommand is provided, any remaining arguments are
    /// passed through to the selected CLI invocation.
    #[arg(last = true, value_name = "CLI_ARGS")]
    codex_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum CommandKind {
    /// List profiles (legacy alias for `profile list`)
    #[command(hide = true)]
    List,

    /// Show active profile details (legacy alias for `profile show`)
    #[command(hide = true)]
    Current,

    /// Switch active profile (legacy alias for `profile use`)
    #[command(hide = true)]
    Use {
        /// Account name or id
        target: String,
    },

    /// Switch account and then run the selected CLI
    Run {
        /// Account name or id
        profile: String,
        /// Force this invocation to run the selected CLI on the host.
        #[arg(long = "host", conflicts_with = "docker_image")]
        host: bool,
        /// Override the Docker image only for this invocation.
        #[arg(long, value_name = "IMAGE")]
        docker_image: Option<String>,
        /// Override the Docker user name only for this invocation.
        #[arg(long, value_name = "NAME", requires = "docker_image")]
        docker_user_name: Option<String>,
        /// Arguments to pass to the selected CLI
        #[arg(last = true, value_name = "CLI_ARGS")]
        codex_args: Vec<String>,
    },

    /// Add an account from an existing auth file
    #[command(hide = true)]
    Add {
        /// Path to auth.json (codex) or .credentials.json (claude)
        #[arg(long, value_name = "PATH")]
        from_auth: String,
        /// Optional path to config.toml
        #[arg(long, value_name = "PATH")]
        from_config: Option<String>,
        /// Run this profile inside a Docker image
        #[arg(long, value_name = "IMAGE")]
        docker_image: Option<String>,
        /// Logical username used for the Docker home path
        #[arg(long, value_name = "NAME", requires = "docker_image")]
        docker_user_name: Option<String>,
        /// Friendly account name (e.g., "work", "personal")
        #[arg(long)]
        name: String,
        /// CLI type: codex (default) or claude
        #[arg(long, default_value = "codex")]
        cli: String,
    },

    /// Log in and create a new profile (interactive wizard if no arguments)
    Login {
        /// Friendly account name (e.g., "work", "personal")
        #[arg(long)]
        name: Option<String>,
        /// CLI type: codex or claude
        #[arg(long)]
        cli: Option<String>,
        /// API key for third-party provider login (skips OAuth)
        #[arg(long)]
        api_key: Option<String>,
        /// API base URL for third-party provider
        #[arg(long)]
        base_url: Option<String>,
        /// Provider display name (e.g., "deepseek", "anthropic")
        #[arg(long)]
        provider: Option<String>,
    },

    /// Rename an existing account
    #[command(hide = true)]
    Rename {
        /// Existing account name or id
        target: String,
        /// New account name
        #[arg(long)]
        name: String,
    },

    /// Remove an existing account
    #[command(hide = true)]
    Remove {
        /// Existing account name or id
        target: String,
    },

    /// Edit display metadata for a profile (legacy alias for `profile set`)
    #[command(hide = true)]
    Annotate {
        /// Existing account name or id
        target: String,
        /// Override the displayed source/provider label
        #[arg(long, conflicts_with = "clear_source")]
        source: Option<String>,
        /// Clear the displayed source/provider label
        #[arg(long)]
        clear_source: bool,
        /// Override the displayed plan label
        #[arg(long, conflicts_with = "clear_plan")]
        plan: Option<String>,
        /// Clear the displayed plan label
        #[arg(long)]
        clear_plan: bool,
        /// Override the displayed email label
        #[arg(long, conflicts_with = "clear_email")]
        email: Option<String>,
        /// Clear the displayed email label
        #[arg(long)]
        clear_email: bool,
    },

    /// Configure runtime for a profile (legacy alias for `profile set`)
    #[command(hide = true)]
    Runtime {
        /// Account name or id
        target: String,
        /// Run this profile on the host
        #[arg(long, conflicts_with = "docker_image")]
        host: bool,
        /// Run this profile inside a Docker image
        #[arg(long, value_name = "IMAGE")]
        docker_image: Option<String>,
        /// Logical username used for the Docker home path
        #[arg(long, value_name = "NAME", requires = "docker_image")]
        docker_user_name: Option<String>,
    },

    /// Unified profile management commands
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },

    /// Unified global settings (proxy, docker sudo, and other defaults)
    Global {
        #[command(subcommand)]
        command: GlobalCommand,
    },

    /// Configure proxy settings (legacy compatibility command)
    #[command(hide = true)]
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },

    /// Manage detachable cute-alden sessions
    #[command(visible_alias = "ss")]
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },

    /// Manage local notification bridges
    Notify {
        #[command(subcommand)]
        command: NotifyCommand,
    },

    /// Open the main interactive configuration wizard
    #[command(visible_alias = "config")]
    Wizard,
}

#[derive(Subcommand, Debug)]
enum GlobalCommand {
    /// Show effective global settings
    Show,

    /// Interactively edit global settings
    #[command(visible_alias = "wizard")]
    Edit,

    /// Update global settings in one command
    Set {
        /// Use `sudo docker` for Docker runtime launches by default
        #[arg(long = "docker-use-sudo", value_name = "BOOL")]
        docker_use_sudo: Option<bool>,
        /// Enable or disable managed cute-alden sessions by default
        #[arg(long = "session-enable", value_name = "BOOL")]
        session_enable: Option<bool>,
        /// Profile name or id used as the global default fallback
        #[arg(
            long = "default-profile",
            value_name = "PROFILE",
            conflicts_with = "clear_default_profile"
        )]
        default_profile: Option<String>,
        /// Clear the configured global default profile fallback
        #[arg(long = "clear-default-profile", conflicts_with = "default_profile")]
        clear_default_profile: bool,
        /// Start the configured default profile directly when running plain `cutex`
        #[arg(long = "default-profile-direct-launch", value_name = "BOOL")]
        default_profile_direct_launch: Option<bool>,
        /// Set the global proxy URL
        #[arg(long = "proxy-url", value_name = "URL", conflicts_with = "proxy_clear")]
        proxy_url: Option<String>,
        /// Optional NO_PROXY value for --proxy-url
        #[arg(long = "proxy-no-proxy", value_name = "VALUE", requires = "proxy_url")]
        proxy_no_proxy: Option<String>,
        /// Optional force-http value for --proxy-url (true/false)
        #[arg(long = "proxy-force-http", value_name = "BOOL", requires = "proxy_url")]
        proxy_force_http_transport: Option<bool>,
        /// Clear the global proxy fallback
        #[arg(long = "proxy-clear", conflicts_with = "proxy_url")]
        proxy_clear: bool,
        /// Set the short idle notify timeout in seconds
        #[arg(long = "notify-idle-timeout", value_name = "SECS")]
        notify_idle_timeout: Option<u64>,
        /// Set the long composer idle notify timeout in seconds
        #[arg(long = "notify-composer-idle-timeout", value_name = "SECS")]
        notify_composer_idle_timeout: Option<u64>,
        /// Set the approval prompt notify timeout in seconds
        #[arg(long = "notify-approval-timeout", value_name = "SECS")]
        notify_approval_timeout: Option<u64>,
        /// Set the startup idle notify timeout in seconds
        #[arg(long = "notify-startup-idle-timeout", value_name = "SECS")]
        notify_startup_idle_timeout: Option<u64>,
        /// Set notify event allowlist as comma-separated snake_case names
        #[arg(long = "notify-events", value_name = "CSV")]
        notify_events: Option<String>,
        /// Set user message content mode for notify payloads: none, preview, full
        #[arg(long = "notify-user-message-content", value_name = "MODE")]
        notify_user_message_content: Option<String>,
        /// Set user message preview length in chars
        #[arg(long = "notify-user-message-preview-chars", value_name = "CHARS")]
        notify_user_message_preview_chars: Option<u64>,
        /// Set threshold warning reminder mode: off, daily, always
        #[arg(long = "rate-limit-threshold-warning-mode", value_name = "MODE")]
        rate_limit_threshold_warning_mode: Option<String>,
        /// Set model nudge reminder mode: off, daily, always
        #[arg(long = "rate-limit-model-nudge-mode", value_name = "MODE")]
        rate_limit_model_nudge_mode: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ProxyCommand {
    /// Show the global proxy or the effective proxy for one profile
    Show {
        /// Optional account name or id
        profile: Option<String>,
    },

    /// Set the global proxy used when a profile does not override it
    Set {
        /// Proxy URL, for example http://127.0.0.1:7890 or socks5h://127.0.0.1:7890
        url: String,
        /// Optional NO_PROXY value
        #[arg(long = "no-proxy")]
        no_proxy: Option<String>,
        /// Force cute-codex model traffic away from WebSocket transport
        #[arg(long = "force-http", default_value_t = true, action = ArgAction::Set)]
        force_http_transport: bool,
    },

    /// Clear the global proxy
    Clear,

    /// Set a proxy override for one profile
    SetProfile {
        /// Account name or id
        profile: String,
        /// Proxy URL, for example http://127.0.0.1:7890 or socks5h://127.0.0.1:7890
        url: String,
        /// Optional NO_PROXY value
        #[arg(long = "no-proxy")]
        no_proxy: Option<String>,
        /// Force cute-codex model traffic away from WebSocket transport
        #[arg(long = "force-http", default_value_t = true, action = ArgAction::Set)]
        force_http_transport: bool,
    },

    /// Disable proxy inheritance for one profile
    DisableProfile {
        /// Account name or id
        profile: String,
    },

    /// Clear a profile proxy override so it inherits the global proxy again
    ClearProfile {
        /// Account name or id
        profile: String,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCommand {
    /// List named cute-alden sessions
    List,

    /// Attach to an existing named cute-alden session
    Attach {
        /// Session name
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum NotifyCommand {
    /// Manage the native desktop notification bridge
    Desktop {
        #[command(subcommand)]
        command: DesktopNotifyCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DesktopNotifyCommand {
    /// Enable desktop notifications and start the shared bridge if needed
    Enable {
        /// Fixed localhost port for the bridge
        #[arg(long)]
        port: Option<u16>,
    },
    /// Disable desktop notifications without changing the external notify service
    Disable,
    /// Start the shared bridge service if it is not already running
    Start {
        /// Fixed localhost port for the bridge
        #[arg(long)]
        port: Option<u16>,
    },
    /// Show bridge config and health
    Status,
    /// Run the bridge HTTP server in the foreground
    Serve {
        /// Port to bind on 127.0.0.1
        #[arg(long)]
        port: Option<u16>,
        /// Bearer token accepted from cute-codex
        #[arg(long)]
        token: Option<String>,
    },
    /// Send a test desktop notification through notify-send
    Test {
        /// Optional message body
        message: Option<String>,
    },
    /// Install and start an Ubuntu/Kubuntu systemd user service
    InstallUbuntu {
        /// Fixed localhost port for the bridge
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop and remove the Ubuntu/Kubuntu systemd user service
    UninstallUbuntu,
}

#[derive(Subcommand, Debug)]
enum ProfileCommand {
    /// List profiles with full runtime/proxy/provider context
    List,

    /// Show one profile in detail (defaults to active profile)
    Show {
        /// Optional account name or id; defaults to the active profile
        target: Option<String>,
    },

    /// Interactively edit one profile (defaults to active profile)
    #[command(visible_alias = "wizard")]
    Edit {
        /// Optional account name or id; defaults to the active profile
        target: Option<String>,
    },

    /// Switch the active profile
    Use {
        /// Account name or id
        target: String,
    },

    /// Rename a profile
    Rename {
        /// Existing profile name or id
        target: String,
        /// New profile name
        #[arg(long)]
        name: String,
    },

    /// Remove a profile
    Remove {
        /// Existing profile name or id
        target: String,
    },

    /// Copy one profile into a new profile, optionally changing provider settings
    Copy {
        /// Source profile name or id
        source: String,
        /// New profile name
        #[arg(long)]
        name: String,
        /// Optional provider id for the copied profile
        #[arg(long)]
        provider: Option<String>,
        /// Optional base_url override for the copied provider
        #[arg(long = "provider-base-url")]
        provider_base_url: Option<String>,
    },

    /// Copy one profile's status_line to every profile
    CloneStatusLine {
        /// Source profile name or id; defaults to the active profile
        #[arg(long)]
        from: Option<String>,
    },

    /// Move a profile to the top of the list order
    PinTop {
        /// Account name or id
        target: String,
    },

    /// Move a profile to the bottom of the list order
    PinBottom {
        /// Account name or id
        target: String,
    },

    /// Update profile metadata, runtime, and proxy override in one command
    Set {
        /// Account name or id
        target: String,
        /// Rename the profile
        #[arg(long)]
        name: Option<String>,
        /// Override the displayed source/provider label
        #[arg(long, conflicts_with = "clear_source")]
        source: Option<String>,
        /// Clear the displayed source/provider label
        #[arg(long)]
        clear_source: bool,
        /// Override the displayed plan label
        #[arg(long, conflicts_with = "clear_plan")]
        plan: Option<String>,
        /// Clear the displayed plan label
        #[arg(long)]
        clear_plan: bool,
        /// Override the displayed email label
        #[arg(long, conflicts_with = "clear_email")]
        email: Option<String>,
        /// Clear the displayed email label
        #[arg(long)]
        clear_email: bool,
        /// Default CLI args prepended for this profile, parsed like a shell command line
        #[arg(
            long = "default-cli-args",
            value_name = "SHELL",
            conflicts_with = "clear_default_cli_args"
        )]
        default_cli_args: Option<String>,
        /// Clear the stored default CLI args for this profile
        #[arg(long = "clear-default-cli-args", conflicts_with = "default_cli_args")]
        clear_default_cli_args: bool,
        /// Run this profile on the host
        #[arg(long = "host", conflicts_with = "docker_image")]
        host: bool,
        /// Run this profile inside a Docker image
        #[arg(long = "docker-image", value_name = "IMAGE")]
        docker_image: Option<String>,
        /// Logical username used for the Docker home path
        #[arg(
            long = "docker-user-name",
            value_name = "NAME",
            requires = "docker_image"
        )]
        docker_user_name: Option<String>,
        /// Set a profile proxy override URL (enables override)
        #[arg(
            long = "proxy-url",
            value_name = "URL",
            conflicts_with_all = ["proxy_disable", "proxy_inherit"]
        )]
        proxy_url: Option<String>,
        /// Optional NO_PROXY value for --proxy-url
        #[arg(long = "proxy-no-proxy", value_name = "VALUE", requires = "proxy_url")]
        proxy_no_proxy: Option<String>,
        /// Optional force-http value for --proxy-url (true/false)
        #[arg(long = "proxy-force-http", value_name = "BOOL", requires = "proxy_url")]
        proxy_force_http_transport: Option<bool>,
        /// Disable proxy inheritance for this profile
        #[arg(long = "proxy-disable", conflicts_with = "proxy_inherit")]
        proxy_disable: bool,
        /// Clear profile proxy override and inherit global proxy
        #[arg(long = "proxy-inherit")]
        proxy_inherit: bool,
        /// Force managed sessions on for this profile
        #[arg(
            long = "session-enable",
            conflicts_with_all = ["session_disable", "session_inherit"]
        )]
        session_enable: bool,
        /// Disable managed sessions for this profile
        #[arg(long = "session-disable", conflicts_with = "session_inherit")]
        session_disable: bool,
        /// Clear the profile session override so it inherits the global default
        #[arg(long = "session-inherit")]
        session_inherit: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum CliKind {
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
struct AccountsStore {
    version: u32,
    accounts: Vec<StoredAccount>,
    active_account_id: Option<String>,
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
struct StoredAccount {
    id: String,
    name: String,
    email: Option<String>,
    plan_type: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    runtime: RuntimeConfig,
    #[serde(default)]
    proxy: Option<ProxyConfig>,
    #[serde(default)]
    session: Option<SessionConfig>,
    #[serde(default)]
    cli_kind: CliKind,
    #[serde(default)]
    default_cli_args: Vec<String>,
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct LegacyAccountsStoreV2 {
    #[serde(default)]
    accounts: Vec<LegacyStoredAccountV2>,
    active_account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyStoredAccountV2 {
    id: String,
    name: String,
    email: Option<String>,
    plan_type: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    raw_auth_json: Option<String>,
    #[serde(default)]
    raw_config_toml: Option<String>,
    #[serde(default)]
    auth: Option<AuthData>,
    #[serde(default)]
    runtime: RuntimeConfig,
    #[serde(default)]
    proxy: Option<ProxyConfig>,
    #[serde(default)]
    session: Option<SessionConfig>,
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RuntimeConfig {
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum AuthData {
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
struct QuickRunState {
    last_global_profile: Option<String>,
    per_directory: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CodezConfig {
    #[serde(default)]
    docker_use_sudo: bool,
    #[serde(default)]
    custom_status_items: Vec<CustomStatusItemCatalogEntry>,
    #[serde(default)]
    proxy: Option<ProxyConfig>,
    #[serde(default = "default_session_config")]
    session: SessionConfig,
    #[serde(default)]
    default_profile: Option<String>,
    #[serde(default)]
    default_profile_direct_launch: bool,
    #[serde(default)]
    notify_service_url: Option<String>,
    #[serde(default)]
    notify_service_token: Option<String>,
    #[serde(default)]
    notify_service_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    notify_service_composer_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    notify_service_approval_timeout_secs: Option<u64>,
    #[serde(default)]
    notify_service_startup_idle_timeout_secs: Option<u64>,
    #[serde(default)]
    notify_service_events: Option<Vec<String>>,
    #[serde(default)]
    notify_service_user_message_content: Option<String>,
    #[serde(default)]
    notify_service_user_message_preview_chars: Option<u64>,
    #[serde(default)]
    rate_limit_threshold_warning_mode: Option<String>,
    #[serde(default)]
    rate_limit_model_nudge_mode: Option<String>,
    #[serde(default)]
    desktop_notify_enabled: bool,
    #[serde(default)]
    desktop_notify_port: Option<u16>,
    #[serde(default)]
    desktop_notify_token: Option<String>,
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
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct ProxyConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    no_proxy: Option<String>,
    #[serde(default = "default_true")]
    force_http_transport: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct SessionConfig {
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CustomStatusItemsCatalogFile {
    #[serde(default)]
    items: Vec<CustomStatusItemCatalogEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CustomStatusItemCatalogEntry {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    source: CustomStatusItemSource,
    #[serde(default)]
    render: CustomStatusItemRender,
    #[serde(default)]
    style: CustomStatusItemStyle,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CustomStatusItemSource {
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
enum CustomStatusItemRender {
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
struct CustomStatusItemStyle {
    #[serde(default)]
    fg: Option<String>,
    #[serde(default)]
    bg: Option<String>,
    #[serde(default)]
    bold: bool,
    #[serde(default)]
    dim: bool,
    #[serde(default)]
    italic: bool,
    #[serde(default)]
    underlined: bool,
}

fn default_true() -> bool {
    true
}

fn default_session_config() -> SessionConfig {
    SessionConfig { enabled: false }
}

#[derive(Debug)]
struct ImportedSnapshot {
    raw_auth_json: String,
    raw_config_toml: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaunchCommand {
    program: String,
    args: Vec<String>,
    envs: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct MaterializedAccountFiles {
    auth_path: PathBuf,
    config_path: PathBuf,
    custom_status_items_path: PathBuf,
}

impl LaunchCommand {
    fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
        }
    }

    fn arg(mut self, value: impl Into<String>) -> Self {
        self.args.push(value.into());
        self
    }

    fn args<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(values.into_iter().map(Into::into));
        self
    }

    fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    fn to_command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        for (key, value) in &self.envs {
            cmd.env(key, value);
        }
        cmd
    }

    #[cfg(test)]
    fn to_shell_command(&self) -> String {
        let mut parts = Vec::new();
        for (key, value) in &self.envs {
            parts.push(format!("{key}={}", shell_quote(value)));
        }
        parts.push(shell_quote(&self.program));
        parts.extend(self.args.iter().map(|arg| shell_quote(arg)));
        parts.join(" ")
    }
}

const CODEX_LAUNCH_PROFILE_ENV_VAR: &str = "CODEX_LAUNCH_PROFILE";
const CODEX_LAUNCH_RUNTIME_ENV_VAR: &str = "CODEX_LAUNCH_RUNTIME";
const CODEX_LAUNCH_PROFILE_SOURCE_ENV_VAR: &str = "CODEX_LAUNCH_PROFILE_SOURCE";
const CODEX_LAUNCH_PROFILE_TYPE_ENV_VAR: &str = "CODEX_LAUNCH_PROFILE_TYPE";
const CODEX_LAUNCH_PROFILE_EMAIL_ENV_VAR: &str = "CODEX_LAUNCH_PROFILE_EMAIL";

fn main() {
    if let Err(err) = real_main() {
        eprintln!("{RED}error:{RESET} {err:#}");
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    set_codez_codex_home()?;
    let cli = Cli::parse();

    match cli.command {
        Some(CommandKind::List) => cmd_list()?,
        Some(CommandKind::Current) => cmd_current()?,
        Some(CommandKind::Use { target }) => cmd_use(&target)?,
        Some(CommandKind::Run {
            profile,
            host,
            docker_image,
            docker_user_name,
            codex_args,
        }) => {
            print_cutex_build();
            cmd_run(
                &profile,
                codex_args,
                cli.host || host,
                docker_image,
                docker_user_name,
            )?
        }
        Some(CommandKind::Add {
            from_auth,
            from_config,
            docker_image,
            docker_user_name,
            name,
            cli,
        }) => cmd_add(
            &from_auth,
            from_config.as_deref(),
            docker_image,
            docker_user_name,
            &name,
            &cli,
        )?,
        Some(CommandKind::Login {
            name,
            cli,
            api_key,
            base_url,
            provider,
        }) => cmd_login(
            name.as_deref(),
            cli.as_deref(),
            api_key.as_deref(),
            base_url.as_deref(),
            provider.as_deref(),
        )?,
        Some(CommandKind::Rename { target, name }) => cmd_rename(&target, &name)?,
        Some(CommandKind::Remove { target }) => cmd_remove(&target)?,
        Some(CommandKind::Annotate {
            target,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
        }) => cmd_annotate(
            &target,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
        )?,
        Some(CommandKind::Runtime {
            target,
            host,
            docker_image,
            docker_user_name,
        }) => cmd_runtime(&target, host, docker_image, docker_user_name)?,
        Some(CommandKind::Profile { command }) => cmd_profile(command)?,
        Some(CommandKind::Global { command }) => cmd_global(command)?,
        Some(CommandKind::Proxy { command }) => cmd_proxy(command)?,
        Some(CommandKind::Session { command }) => cmd_session(command)?,
        Some(CommandKind::Notify { command }) => cmd_notify(command)?,
        Some(CommandKind::Wizard) => cmd_wizard()?,
        None => {
            print_cutex_build();
            cmd_quick_run(cli.codex_args, cli.quick, cli.host)?
        }
    }

    Ok(())
}

fn print_cutex_build() {
    println!("cutex build: {CODEZ_BUILD}");
}

fn set_codez_codex_home() -> anyhow::Result<()> {
    migrate_legacy_runtime_layout()?;
    let path = host_codex_home_dir()?;
    std::env::set_var("CODEX_HOME", &path);
    Ok(())
}

fn config_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".cutex"))
}

fn legacy_config_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".codez-cli"))
}

fn runtime_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("runtime"))
}

fn host_codex_home_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("codex-home"))
}

fn legacy_host_codex_home_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".codex-codez"))
}

fn docker_runtime_home_dir() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("docker-home"))
}

fn legacy_docker_runtime_home_dir() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("thirdparty").join("userhome"))
}

fn login_runtime_root() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("login"))
}

fn migrate_legacy_runtime_layout() -> anyhow::Result<()> {
    migrate_dir_if_needed(&legacy_config_dir()?, &config_dir()?)?;
    migrate_dir_if_needed(&legacy_host_codex_home_dir()?, &host_codex_home_dir()?)?;
    Ok(())
}

fn migrate_dir_if_needed(from: &Path, to: &Path) -> anyhow::Result<()> {
    if !from.exists() || to.exists() {
        return Ok(());
    }

    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create migration parent dir: {}",
                parent.display()
            )
        })?;
    }

    fs::rename(from, to).with_context(|| {
        format!(
            "Failed to migrate legacy directory {} -> {}",
            from.display(),
            to.display()
        )
    })?;
    Ok(())
}

fn accounts_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("accounts.json"))
}

fn quick_state_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("state.json"))
}

fn config_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

fn load_store() -> anyhow::Result<AccountsStore> {
    let path = accounts_path()?;
    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let data = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;
    let json: Value = serde_json::from_str(&data)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;
    let version = json
        .get("version")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);

    if version < STORE_VERSION {
        let legacy: LegacyAccountsStoreV2 = serde_json::from_value(json).with_context(|| {
            format!(
                "Failed to parse legacy accounts file for migration: {}",
                path.display()
            )
        })?;
        let migrated = migrate_legacy_store_v2_to_v3(legacy, &path)?;
        save_store(&migrated)?;
        eprintln!(
            "{YELLOW}migrated:{RESET} accounts.json upgraded to v{}; backup saved alongside accounts file",
            STORE_VERSION
        );
        return Ok(migrated);
    }

    let store: AccountsStore = serde_json::from_value(json)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;
    Ok(canonicalize_store(store))
}

fn save_store(store: &AccountsStore) -> anyhow::Result<()> {
    let path = accounts_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
    }

    let data = serde_json::to_string_pretty(store)?;
    fs::write(&path, data)
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;
    Ok(())
}

fn canonicalize_store(mut store: AccountsStore) -> AccountsStore {
    if store.version != STORE_VERSION {
        store.version = STORE_VERSION;
    }
    for account in &mut store.accounts {
        if account.source.is_none() {
            account.source = detect_source_label_for_account_files(account);
        }
    }
    let active_exists = store.active_account_id.as_ref().is_some_and(|active_id| {
        store
            .accounts
            .iter()
            .any(|account| account.id.as_str() == active_id.as_str())
    });
    if !active_exists {
        store.active_account_id = store.accounts.first().map(|account| account.id.clone());
    }
    store
}

fn detect_source_label_for_account_files(account: &StoredAccount) -> Option<String> {
    let files = materialized_account_files(account).ok()?;
    let auth = read_optional_text(&files.auth_path).ok().flatten();
    let profile_config = read_optional_text(&files.config_path)
        .ok()
        .flatten()
        .and_then(|config| extract_profile_config_toml(&config).ok().flatten());
    Some(detect_source_label(
        auth.as_deref(),
        profile_config.as_deref(),
    ))
}

fn migrate_legacy_store_v2_to_v3(
    legacy: LegacyAccountsStoreV2,
    accounts_file_path: &Path,
) -> anyhow::Result<AccountsStore> {
    backup_legacy_accounts_file(accounts_file_path)?;
    let global_config = load_codez_config();
    let mut migrated = AccountsStore {
        version: STORE_VERSION,
        accounts: Vec::with_capacity(legacy.accounts.len()),
        active_account_id: legacy.active_account_id,
    };

    for legacy_account in legacy.accounts {
        let legacy_profile_config = legacy_account
            .raw_config_toml
            .as_deref()
            .map(extract_profile_config_toml)
            .transpose()?
            .flatten();
        let legacy_source = legacy_account.source.clone().or_else(|| {
            Some(detect_source_label(
                legacy_account.raw_auth_json.as_deref(),
                legacy_profile_config.as_deref(),
            ))
        });

        let account = StoredAccount {
            id: legacy_account.id,
            name: legacy_account.name,
            email: legacy_account.email,
            plan_type: legacy_account.plan_type,
            source: legacy_source,
            runtime: legacy_account.runtime,
            proxy: legacy_account.proxy,
            session: legacy_account.session,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            last_used_at: legacy_account.last_used_at,
        };

        materialize_migrated_account_files(
            &account,
            legacy_account.raw_auth_json.as_deref(),
            legacy_account.auth.as_ref(),
            legacy_profile_config.as_deref(),
            &global_config,
        )?;
        migrated.accounts.push(account);
    }

    Ok(canonicalize_store(migrated))
}

fn backup_legacy_accounts_file(accounts_file_path: &Path) -> anyhow::Result<()> {
    let backup_path = accounts_file_path.with_file_name("accounts.v2.backup.json");
    if backup_path.exists() {
        return Ok(());
    }
    fs::copy(accounts_file_path, &backup_path).with_context(|| {
        format!(
            "Failed to create legacy accounts backup {} -> {}",
            accounts_file_path.display(),
            backup_path.display()
        )
    })?;
    Ok(())
}

fn materialize_migrated_account_files(
    account: &StoredAccount,
    legacy_raw_auth_json: Option<&str>,
    legacy_auth_data: Option<&AuthData>,
    legacy_profile_config_toml: Option<&str>,
    global_config: &CodezConfig,
) -> anyhow::Result<()> {
    let files = materialized_account_files(account)?;
    if let Some(parent) = files.auth_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
    }

    let existing_auth = read_optional_text(&files.auth_path)?;
    let fallback_auth = if let Some(raw_auth_json) = legacy_raw_auth_json {
        Some(raw_auth_json.to_string())
    } else if let Some(auth_data) = legacy_auth_data {
        Some(legacy_auth_json_from_auth_data(auth_data)?)
    } else {
        None
    };
    let auth_contents = existing_auth.or(fallback_auth).ok_or_else(|| {
        anyhow!(
            "Legacy profile '{}' has no auth payload and no materialized auth.json at {}",
            account.name,
            files.auth_path.display()
        )
    })?;
    write_optional_text_if_changed(&files.auth_path, Some(&auth_contents))?;

    let existing_profile_config = read_optional_text(&files.config_path)?
        .and_then(|existing| match extract_profile_config_toml(&existing) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "{YELLOW}warning:{RESET} ignoring invalid existing config during migration at {}: {err:#}",
                    files.config_path.display()
                );
                None
            }
        });
    let profile_config =
        existing_profile_config.or_else(|| legacy_profile_config_toml.map(str::to_string));
    let profile_config = normalize_profile_config_for_account(account, profile_config)?;
    merge_and_write_config_toml(
        &files.config_path,
        profile_config.as_deref(),
        effective_proxy_config(account, global_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;

    let custom_status_items_json = custom_status_items_catalog_json(global_config)?;
    write_optional_text_if_changed(
        &files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&files)?;
    Ok(())
}

fn load_quick_state() -> QuickRunState {
    let path = match quick_state_path() {
        Ok(p) => p,
        Err(_) => return QuickRunState::default(),
    };
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return QuickRunState::default(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_quick_state(state: &QuickRunState) -> anyhow::Result<()> {
    let path = quick_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
    }

    let data = serde_json::to_string_pretty(state)?;
    fs::write(&path, data)
        .with_context(|| format!("Failed to write state file: {}", path.display()))?;
    Ok(())
}

fn load_codez_config() -> CodezConfig {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return CodezConfig::default(),
    };
    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return CodezConfig::default(),
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_codez_config(config: &CodezConfig) -> anyhow::Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config dir: {}", parent.display()))?;
    }

    let data = serde_json::to_string_pretty(config)?;
    fs::write(&path, data)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;
    Ok(())
}

fn env_var_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| std::env::var(name).ok())
}

fn cmd_wizard() -> anyhow::Result<()> {
    loop {
        println!();
        println!("{BOLD}{CYAN}cutex Wizard{RESET}");
        println!("  1. Start default profile");
        println!("  2. List profiles");
        println!("  3. Show active profile");
        println!("  4. Edit active profile");
        println!("  5. Edit global settings");
        println!("  6. Log in / create profile");

        let Some(choice) = read_wizard_choice(6)? else {
            println!("Done.");
            return Ok(());
        };

        match choice {
            1 => return cmd_quick_run(Vec::new(), true, false),
            2 => cmd_profile_list()?,
            3 => cmd_profile_show(None)?,
            4 => cmd_profile_edit(None)?,
            5 => cmd_global_edit()?,
            6 => cmd_login_interactive()?,
            _ => unreachable!(),
        }
    }
}

fn cmd_list() -> anyhow::Result<()> {
    cmd_profile_list()
}

fn account_proxy_scope_label(account: &StoredAccount, global_config: &CodezConfig) -> String {
    let effective = effective_proxy_config(account, global_config);
    match (account.proxy.as_ref(), effective) {
        (Some(proxy), _) if !proxy.enabled => "off(profile)".to_string(),
        (Some(proxy), Some(_)) if proxy.enabled => "on(profile)".to_string(),
        (None, Some(proxy)) if proxy.enabled => "on(global)".to_string(),
        (None, Some(_)) => "off(global)".to_string(),
        (None, None) => "off".to_string(),
        _ => "off".to_string(),
    }
}

fn effective_session_config<'a>(
    account: &'a StoredAccount,
    global_config: &'a CodezConfig,
) -> &'a SessionConfig {
    account.session.as_ref().unwrap_or(&global_config.session)
}

fn account_session_scope_label(account: &StoredAccount, global_config: &CodezConfig) -> String {
    match account.session.as_ref() {
        Some(config) if config.enabled => "on(profile)".to_string(),
        Some(_) => "off(profile)".to_string(),
        None if global_config.session.enabled => "on(global)".to_string(),
        None => "off(global)".to_string(),
    }
}

fn session_config_label(config: &SessionConfig) -> &'static str {
    if config.enabled {
        "enabled"
    } else {
        "disabled"
    }
}

fn account_model_provider(account: &StoredAccount) -> Option<String> {
    let provider_from_config = account_profile_config_table(account).and_then(|table| {
        table
            .get("model_provider")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    });
    if provider_from_config.is_some() {
        return provider_from_config;
    }

    if is_official_codex_account(account) {
        return Some("openai".to_string());
    }

    if account_uses_openai_auth(account) {
        return Some("openai".to_string());
    }

    None
}

fn account_profile_config_table(account: &StoredAccount) -> Option<Table> {
    let files = materialized_account_files(account).ok()?;
    let raw = read_optional_text(&files.config_path).ok().flatten()?;
    parse_toml_table(&raw).ok()
}

fn account_auth_payload(account: &StoredAccount) -> Option<Value> {
    let files = materialized_account_files(account).ok()?;
    let raw = read_optional_text(&files.auth_path).ok().flatten()?;
    serde_json::from_str::<Value>(&raw).ok()
}

fn account_model_api_base(account: &StoredAccount) -> Option<String> {
    let provider = account_model_provider(account)?;
    let explicit_base_url = account_profile_config_table(account).and_then(|table| {
        table
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get(provider.as_str()))
            .and_then(|value| value.as_table())
            .and_then(|provider_table| provider_table.get("base_url"))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    });
    if explicit_base_url.is_some() {
        return explicit_base_url;
    }

    default_provider_api_base(&provider, account)
}

fn default_provider_api_base(provider: &str, account: &StoredAccount) -> Option<String> {
    if provider.eq_ignore_ascii_case("openai") {
        let uses_chatgpt_auth = account_uses_chatgpt_auth(account);
        return Some(if uses_chatgpt_auth {
            "https://chatgpt.com/backend-api/codex".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        });
    }

    if provider.eq_ignore_ascii_case("ollama") {
        return Some(default_oss_provider_base_url(11434));
    }

    if provider.eq_ignore_ascii_case("lmstudio") {
        return Some(default_oss_provider_base_url(1234));
    }

    None
}

fn default_oss_provider_base_url(default_port: u16) -> String {
    if let Ok(base_url) = std::env::var("CODEX_OSS_BASE_URL") {
        if !base_url.trim().is_empty() {
            return base_url;
        }
    }

    let port = std::env::var("CODEX_OSS_PORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(default_port);
    format!("http://localhost:{port}/v1")
}

fn account_uses_chatgpt_auth(account: &StoredAccount) -> bool {
    if is_official_codex_account(account) {
        return true;
    }

    account_auth_payload(account)
        .and_then(|json| {
            json.get("tokens")
                .and_then(|value| value.as_object())
                .cloned()
        })
        .is_some()
}

fn is_official_codex_account(account: &StoredAccount) -> bool {
    account.cli_kind == CliKind::Codex && account.source.as_deref() == Some("official")
}

fn account_uses_openai_auth(account: &StoredAccount) -> bool {
    account_auth_payload(account).is_some_and(|json| {
        json.get("tokens")
            .and_then(|value| value.as_object())
            .is_some()
            || json.get("OPENAI_API_KEY").is_some()
            || json.get("openai_api_key").is_some()
    })
}

fn account_status_line_len(account: &StoredAccount) -> Option<usize> {
    let table = account_profile_config_table(account)?;
    table
        .get("tui")
        .and_then(|value| value.as_table())
        .and_then(|tui| tui.get("status_line"))
        .and_then(|value| value.as_array())
        .map(Vec::len)
}

fn cmd_profile(command: ProfileCommand) -> anyhow::Result<()> {
    match command {
        ProfileCommand::List => cmd_profile_list(),
        ProfileCommand::Show { target } => cmd_profile_show(target.as_deref()),
        ProfileCommand::Edit { target } => cmd_profile_edit(target.as_deref()),
        ProfileCommand::Use { target } => cmd_use(&target),
        ProfileCommand::Rename { target, name } => cmd_rename(&target, &name),
        ProfileCommand::Remove { target } => cmd_remove(&target),
        ProfileCommand::Copy {
            source,
            name,
            provider,
            provider_base_url,
        } => cmd_profile_copy(&source, &name, provider, provider_base_url),
        ProfileCommand::CloneStatusLine { from } => cmd_profile_clone_status_line(from.as_deref()),
        ProfileCommand::PinTop { target } => cmd_profile_pin(&target, true),
        ProfileCommand::PinBottom { target } => cmd_profile_pin(&target, false),
        ProfileCommand::Set {
            target,
            name,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
            default_cli_args,
            clear_default_cli_args,
            host,
            docker_image,
            docker_user_name,
            proxy_url,
            proxy_no_proxy,
            proxy_force_http_transport,
            proxy_disable,
            proxy_inherit,
            session_enable,
            session_disable,
            session_inherit,
        } => cmd_profile_set(
            &target,
            name,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
            default_cli_args,
            clear_default_cli_args,
            host,
            docker_image,
            docker_user_name,
            proxy_url,
            proxy_no_proxy,
            proxy_force_http_transport,
            proxy_disable,
            proxy_inherit,
            session_enable,
            session_disable,
            session_inherit,
        ),
    }
}

fn cmd_profile_list() -> anyhow::Result<()> {
    let store = load_store()?;
    if store.accounts.is_empty() {
        println!(
            "No accounts configured. Use `cutex login` or `cutex add --from-auth <path> --name <name>` to add one."
        );
        return Ok(());
    }

    struct Row {
        name: String,
        cli: String,
        source: String,
        plan: String,
        runtime: String,
        provider: String,
        email: String,
        active: bool,
    }

    let rows: Vec<Row> = store
        .accounts
        .iter()
        .map(|acc| {
            let active = Some(&acc.id) == store.active_account_id.as_ref();
            let runtime_str = match &acc.runtime {
                RuntimeConfig::Host => "host".to_string(),
                RuntimeConfig::Docker { image, .. } => format!("docker {image}"),
            };
            Row {
                name: acc.name.clone(),
                cli: acc.cli_kind.to_string(),
                source: acc.source.as_deref().unwrap_or("-").to_string(),
                plan: acc.plan_type.as_deref().unwrap_or("-").to_string(),
                runtime: runtime_str,
                provider: account_model_provider(acc).unwrap_or_else(|| "-".to_string()),
                email: acc.email.as_deref().unwrap_or("-").to_string(),
                active,
            }
        })
        .collect();

    let w_name = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let w_cli = rows.iter().map(|r| r.cli.len()).max().unwrap_or(3).max(3);
    let w_src = rows
        .iter()
        .map(|r| r.source.len())
        .max()
        .unwrap_or(6)
        .max(6);
    let w_plan = rows.iter().map(|r| r.plan.len()).max().unwrap_or(4).max(4);
    let w_rt = rows
        .iter()
        .map(|r| r.runtime.len())
        .max()
        .unwrap_or(7)
        .max(7);
    let w_prov = rows
        .iter()
        .map(|r| r.provider.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let w_email = rows.iter().map(|r| r.email.len()).max().unwrap_or(5).max(5);

    println!(
        "{DIM}  #  {:<w_name$}  {:<w_cli$}  {:<w_src$}  {:<w_plan$}  {:<w_rt$}  {:<w_prov$}  {:<w_email$}{RESET}",
        "Name", "CLI", "Source", "Plan", "Runtime", "Provider", "Email"
    );

    for (idx, row) in rows.iter().enumerate() {
        let badge = if row.active {
            format!("  {GREEN}● active{RESET}")
        } else {
            String::new()
        };
        let name_color = if row.active { GREEN } else { CYAN };
        println!(
            "  {BOLD}{}{RESET}  {name_color}{:<w_name$}{RESET}  {:<w_cli$}  {BLUE}{:<w_src$}{RESET}  {MAGENTA}{:<w_plan$}{RESET}  {YELLOW}{:<w_rt$}{RESET}  {:<w_prov$}  {DIM}{:<w_email$}{RESET}{badge}",
            idx + 1,
            row.name,
            row.cli,
            row.source,
            row.plan,
            row.runtime,
            row.provider,
            row.email,
        );
    }

    Ok(())
}

fn cmd_current() -> anyhow::Result<()> {
    cmd_profile_show(None)
}

fn print_profile_details(
    store: &AccountsStore,
    account: &StoredAccount,
    global_config: &CodezConfig,
) -> anyhow::Result<()> {
    let files = materialized_account_files(account).ok();
    let active = store.active_account_id.as_deref() == Some(account.id.as_str());
    let provider = account_model_provider(account).unwrap_or_else(|| "-".to_string());
    let api = account_model_api_base(account).unwrap_or_else(|| "-".to_string());
    let status_line_len = account_status_line_len(account);

    println!("{BOLD}{CYAN}Profile{RESET} {}", account.name);
    println!("{DIM}Active{RESET}  {}", bool_label(active));
    println!("{DIM}Id{RESET}      {}", account.id);
    println!(
        "{DIM}Source{RESET}  {}",
        account.source.as_deref().unwrap_or("unknown")
    );
    println!(
        "{DIM}Plan{RESET}    {}",
        account.plan_type.as_deref().unwrap_or("unknown")
    );
    println!(
        "{DIM}Email{RESET}   {}",
        account.email.as_deref().unwrap_or("-")
    );
    println!(
        "{DIM}DefaultArgs{RESET} {}",
        cli_args_label(&account.default_cli_args)
    );
    println!(
        "{DIM}Runtime{RESET} {}",
        runtime_description(&account.runtime)
    );
    println!("{DIM}Provider{RESET} {}", provider);
    println!("{DIM}ApiBase{RESET} {}", api);
    match status_line_len {
        Some(count) => println!("{DIM}StatusLine{RESET} {} items", count),
        None => println!("{DIM}StatusLine{RESET} -"),
    }
    println!(
        "{DIM}Proxy(profile){RESET}  {}",
        proxy_config_label(account.proxy.as_ref())
    );
    println!(
        "{DIM}Proxy(global){RESET}   {}",
        proxy_config_label(global_config.proxy.as_ref())
    );
    println!(
        "{DIM}Proxy(effective){RESET} {}",
        proxy_config_label(effective_proxy_config(account, global_config))
    );
    println!(
        "{DIM}Proxy(scope){RESET} {}",
        account_proxy_scope_label(account, global_config)
    );
    println!(
        "{DIM}Session(profile){RESET}  {}",
        account
            .session
            .as_ref()
            .map(session_config_label)
            .unwrap_or("inherit")
    );
    println!(
        "{DIM}Session(global){RESET}   {}",
        session_config_label(&global_config.session)
    );
    println!(
        "{DIM}Session(effective){RESET} {}",
        session_config_label(effective_session_config(account, global_config))
    );
    println!(
        "{DIM}Session(scope){RESET} {}",
        account_session_scope_label(account, global_config)
    );
    if let Some(files) = files {
        println!(
            "{DIM}Config File{RESET} {}",
            if files.config_path.exists() {
                "present"
            } else {
                "missing"
            }
        );
        println!(
            "{DIM}Auth File{RESET} {}",
            if files.auth_path.exists() {
                "present"
            } else {
                "missing"
            }
        );
        println!("{DIM}Config{RESET}  {}", files.config_path.display());
        println!("{DIM}Auth{RESET}    {}", files.auth_path.display());
    }

    Ok(())
}

fn cmd_profile_show(target: Option<&str>) -> anyhow::Result<()> {
    let store = load_store()?;
    let global_config = load_codez_config();
    let account = match target {
        Some(target) => {
            find_account(&store, target)?.ok_or_else(|| anyhow!("Account not found: {target}"))?
        }
        None => {
            let Some(active_id) = store.active_account_id.as_ref() else {
                println!("No active account. Use `cutex use <name>` to select one.");
                return Ok(());
            };
            let Some(account) = store
                .accounts
                .iter()
                .find(|candidate| &candidate.id == active_id)
            else {
                println!("No active account. Use `cutex use <name>` to select one.");
                return Ok(());
            };
            account
        }
    };

    print_profile_details(&store, account, &global_config)
}

fn cmd_use(target: &str) -> anyhow::Result<()> {
    let account = activate_account(target)?;

    println!(
        "{GREEN}Switched{RESET} active profile to {BOLD}{}{RESET}",
        account.name
    );
    Ok(())
}

fn cmd_profile_copy(
    source: &str,
    name: &str,
    provider: Option<String>,
    provider_base_url: Option<String>,
) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("Profile name cannot be empty");
    }

    let mut store = load_store()?;
    ensure_unique_name(&store, name, None)?;

    let source_index = store
        .accounts
        .iter()
        .position(|account| account.name == source || account.id == source)
        .ok_or_else(|| anyhow!("Account not found: {source}"))?;
    let source_account = store.accounts[source_index].clone();

    let mut copied_account = source_account.clone();
    copied_account.id = Uuid::new_v4().to_string();
    copied_account.name = name.to_string();
    copied_account.last_used_at = None;

    copy_profile_account_files(
        &source_account,
        &copied_account,
        provider,
        provider_base_url,
    )?;

    if let Some(source_label) = detect_source_label_for_account_files(&copied_account) {
        copied_account.source = Some(source_label);
    }

    let copied_name = copied_account.name.clone();
    store.accounts.insert(source_index + 1, copied_account);
    save_store(&store)?;

    println!(
        "{GREEN}Copied{RESET} profile {BOLD}{}{RESET} -> {BOLD}{}{RESET}",
        source_account.name, copied_name
    );
    cmd_profile_show(Some(copied_name.as_str()))
}

fn copy_profile_account_files(
    source_account: &StoredAccount,
    target_account: &StoredAccount,
    provider: Option<String>,
    provider_base_url: Option<String>,
) -> anyhow::Result<()> {
    let source_files = ensure_materialized_account_files(source_account)?;
    let target_files = materialized_account_files(target_account)?;
    if let Some(parent) = target_files.auth_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
    }

    let auth_contents = read_optional_text(&source_files.auth_path)?.ok_or_else(|| {
        anyhow!(
            "Source profile '{}' is missing auth.json at {}",
            source_account.name,
            source_files.auth_path.display()
        )
    })?;
    write_optional_text_if_changed(&target_files.auth_path, Some(&auth_contents))?;

    let source_config_contents = read_optional_text(&source_files.config_path)?;
    let copied_config = build_copied_profile_config(
        source_account,
        source_config_contents,
        provider,
        provider_base_url,
    )?;
    write_optional_text_if_changed(&target_files.config_path, copied_config.as_deref())?;

    let codez_config = load_codez_config();
    let custom_status_items_json = custom_status_items_catalog_json(&codez_config)?;
    write_optional_text_if_changed(
        &target_files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&target_files)?;
    Ok(())
}

fn build_copied_profile_config(
    source_account: &StoredAccount,
    source_config_contents: Option<String>,
    provider: Option<String>,
    provider_base_url: Option<String>,
) -> anyhow::Result<Option<String>> {
    let provider = provider
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let provider_base_url = provider_base_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if provider.is_none() && provider_base_url.is_none() {
        return Ok(source_config_contents);
    }

    let mut config_table = match source_config_contents.as_deref() {
        Some(contents) => parse_toml_table(contents)?,
        None => Table::new(),
    };

    let target_provider = provider.or_else(|| {
        if provider_base_url.is_some() {
            account_model_provider(source_account)
        } else {
            None
        }
    });
    let Some(target_provider) = target_provider else {
        anyhow::bail!(
            "Unable to determine which provider to update. Pass --provider <ID> or copy from a profile that already has a provider."
        );
    };

    let previous_provider = config_table
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    if previous_provider.as_deref() != Some(target_provider.as_str()) {
        config_table.insert(
            "model_provider".to_string(),
            toml::Value::String(target_provider.clone()),
        );
        if let Some(previous_provider) = previous_provider.as_deref() {
            remove_model_provider_entry(&mut config_table, previous_provider);
        }
    }

    if let Some(base_url) = provider_base_url {
        let providers_value = config_table
            .entry("model_providers".to_string())
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let providers_table = providers_value
            .as_table_mut()
            .ok_or_else(|| anyhow!("config.toml key `model_providers` must be a table"))?;
        let provider_value = providers_table
            .entry(target_provider.clone())
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let provider_table = provider_value.as_table_mut().ok_or_else(|| {
            anyhow!(
                "config.toml key `model_providers.{}` must be a table",
                target_provider
            )
        })?;
        ensure_model_provider_name(provider_table, &target_provider);
        if source_account.source.as_deref() == Some("api-key") {
            ensure_model_provider_uses_api_key_env(provider_table);
        }
        provider_table.insert("base_url".to_string(), toml::Value::String(base_url));
    } else {
        let has_provider_config = config_table
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get(target_provider.as_str()))
            .is_some();
        let has_default_provider_base =
            default_provider_api_base(&target_provider, source_account).is_some();
        if !has_provider_config && !has_default_provider_base {
            anyhow::bail!(
                "Provider `{}` requires --provider-base-url because the source profile does not define it and cutex has no built-in default for it",
                target_provider
            );
        }
    }

    Ok(Some(toml::to_string_pretty(&config_table)?))
}

fn normalize_profile_config_for_account(
    account: &StoredAccount,
    profile_config_toml: Option<String>,
) -> anyhow::Result<Option<String>> {
    if account.cli_kind != CliKind::Codex {
        return Ok(profile_config_toml);
    }

    let mut root = profile_config_toml
        .as_deref()
        .map(parse_toml_table)
        .transpose()?
        .unwrap_or_default();

    ensure_default_status_line(&mut root)?;

    if account.source.as_deref() == Some("api-key") {
        if let Some(provider_name) = root
            .get("model_provider")
            .and_then(|value| value.as_str())
            .map(str::to_string)
        {
            if let Some(provider_table) = root
                .get_mut("model_providers")
                .and_then(|value| value.as_table_mut())
                .and_then(|providers| providers.get_mut(&provider_name))
                .and_then(|value| value.as_table_mut())
            {
                ensure_model_provider_name(provider_table, &provider_name);
                ensure_model_provider_uses_api_key_env(provider_table);
            }
        }
    }

    if root.is_empty() {
        Ok(None)
    } else {
        Ok(Some(toml::to_string_pretty(&root)?))
    }
}

fn ensure_default_status_line(root: &mut Table) -> anyhow::Result<()> {
    let tui = root
        .entry("tui".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    let tui_table = tui
        .as_table_mut()
        .ok_or_else(|| anyhow!("config.toml key `tui` must be a table"))?;

    tui_table
        .entry("status_line".to_string())
        .or_insert_with(default_status_line_value);
    tui_table
        .entry("status_line_use_colors".to_string())
        .or_insert(toml::Value::Boolean(true));

    Ok(())
}

fn default_status_line_value() -> toml::Value {
    toml::Value::Array(
        DEFAULT_CUTEX_STATUS_LINE
            .iter()
            .map(|item| toml::Value::String((*item).to_string()))
            .collect(),
    )
}

fn remove_model_provider_entry(root: &mut Table, provider_name: &str) {
    let remove_section = root
        .get_mut("model_providers")
        .and_then(|value| value.as_table_mut())
        .map(|providers| {
            providers.remove(provider_name);
            providers.is_empty()
        })
        .unwrap_or(false);
    if remove_section {
        root.remove("model_providers");
    }
}

fn ensure_model_provider_name(provider_table: &mut Table, provider_name: &str) {
    let has_non_empty_name = provider_table
        .get("name")
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_non_empty_name {
        provider_table.insert(
            "name".to_string(),
            toml::Value::String(provider_name.to_string()),
        );
    }
}

fn ensure_model_provider_uses_api_key_env(provider_table: &mut Table) {
    provider_table.insert(
        "env_key".to_string(),
        toml::Value::String("OPENAI_API_KEY".to_string()),
    );
    provider_table.insert(
        "requires_openai_auth".to_string(),
        toml::Value::Boolean(false),
    );
}

#[allow(clippy::too_many_arguments)]
fn cmd_profile_set(
    target: &str,
    name: Option<String>,
    source: Option<String>,
    clear_source: bool,
    plan: Option<String>,
    clear_plan: bool,
    email: Option<String>,
    clear_email: bool,
    default_cli_args: Option<String>,
    clear_default_cli_args: bool,
    host: bool,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
    proxy_url: Option<String>,
    proxy_no_proxy: Option<String>,
    proxy_force_http_transport: Option<bool>,
    proxy_disable: bool,
    proxy_inherit: bool,
    session_enable: bool,
    session_disable: bool,
    session_inherit: bool,
) -> anyhow::Result<()> {
    if proxy_no_proxy.is_some() && proxy_url.is_none() {
        anyhow::bail!("--proxy-no-proxy requires --proxy-url");
    }
    if proxy_force_http_transport.is_some() && proxy_url.is_none() {
        anyhow::bail!("--proxy-force-http requires --proxy-url");
    }

    let mut store = load_store()?;
    let account_id = find_account(&store, target)?
        .map(|account| account.id.clone())
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    if let Some(new_name) = name.as_deref() {
        if new_name.trim().is_empty() {
            anyhow::bail!("Profile name cannot be empty");
        }
        ensure_unique_name(&store, new_name, Some(&account_id))?;
    }

    let metadata_requested = source.is_some()
        || clear_source
        || plan.is_some()
        || clear_plan
        || email.is_some()
        || clear_email;
    let default_cli_args_requested = default_cli_args.is_some() || clear_default_cli_args;
    let runtime_requested = host || docker_image.is_some();
    let proxy_requested = proxy_inherit || proxy_disable || proxy_url.is_some();
    let session_requested = session_enable || session_disable || session_inherit;

    if name.is_none()
        && !metadata_requested
        && !default_cli_args_requested
        && !runtime_requested
        && !proxy_requested
        && !session_requested
    {
        anyhow::bail!(
            "No changes requested. Provide at least one of --name, metadata flags, default CLI args, runtime flags, proxy flags, or session flags."
        );
    }

    let mut changed = false;
    let mut renamed: Option<(String, String)> = None;
    let account_name = {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| anyhow!("Account not found after lookup: {target}"))?;

        if let Some(new_name) = name.as_deref() {
            if account.name != new_name {
                let old_name = account.name.clone();
                account.name = new_name.to_string();
                renamed = Some((old_name, account.name.clone()));
                changed = true;
            }
        }

        if metadata_requested {
            let before = (
                account.source.clone(),
                account.plan_type.clone(),
                account.email.clone(),
            );
            apply_annotation(
                account,
                source.clone(),
                clear_source,
                plan.clone(),
                clear_plan,
                email.clone(),
                clear_email,
            );
            let after = (
                account.source.clone(),
                account.plan_type.clone(),
                account.email.clone(),
            );
            if before != after {
                changed = true;
            }
        }

        if clear_default_cli_args {
            if !account.default_cli_args.is_empty() {
                account.default_cli_args.clear();
                changed = true;
            }
        } else if let Some(value) = default_cli_args.as_deref() {
            let next_default_cli_args = parse_cli_args_value(value)?;
            if account.default_cli_args != next_default_cli_args {
                account.default_cli_args = next_default_cli_args;
                changed = true;
            }
        }

        if runtime_requested {
            let next_runtime = if let Some(image) = docker_image.as_ref() {
                RuntimeConfig::Docker {
                    image: image.clone(),
                    user_name: Some(normalize_docker_user_name(docker_user_name.clone())?),
                }
            } else {
                RuntimeConfig::Host
            };
            if account.runtime != next_runtime {
                account.runtime = next_runtime;
                changed = true;
            }
        }

        if proxy_inherit {
            if account.proxy.is_some() {
                account.proxy = None;
                changed = true;
            }
        } else if proxy_disable {
            let next_proxy =
                proxy_config_from_parts(false, None, None, /*force_http_transport*/ true)?;
            if account.proxy.as_ref() != Some(&next_proxy) {
                account.proxy = Some(next_proxy);
                changed = true;
            }
        } else if let Some(url) = proxy_url.as_ref() {
            let next_proxy = proxy_config_from_parts(
                true,
                Some(url.clone()),
                proxy_no_proxy.clone(),
                proxy_force_http_transport.unwrap_or(true),
            )?;
            if account.proxy.as_ref() != Some(&next_proxy) {
                account.proxy = Some(next_proxy);
                changed = true;
            }
        }

        if session_inherit {
            if account.session.is_some() {
                account.session = None;
                changed = true;
            }
        } else if session_enable {
            let next_session = SessionConfig { enabled: true };
            if account.session.as_ref() != Some(&next_session) {
                account.session = Some(next_session);
                changed = true;
            }
        } else if session_disable {
            let next_session = SessionConfig { enabled: false };
            if account.session.as_ref() != Some(&next_session) {
                account.session = Some(next_session);
                changed = true;
            }
        }

        account.name.clone()
    };

    if !changed {
        println!(
            "{YELLOW}No changes{RESET} for profile {BOLD}{}{RESET}",
            account_name
        );
        return Ok(());
    }

    save_store(&store)?;

    if let Some((old_name, new_name)) = renamed {
        let mut state = load_quick_state();
        rename_profile_references(&mut state, &old_name, &new_name);
        let _ = save_quick_state(&state);
    }

    println!(
        "{GREEN}Updated{RESET} profile {BOLD}{}{RESET}",
        account_name
    );
    cmd_profile_show(Some(account_name.as_str()))
}

fn profile_edit_target_id(
    store: &AccountsStore,
    target: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if let Some(target) = target {
        return Ok(Some(
            find_account(store, target)?
                .map(|account| account.id.clone())
                .ok_or_else(|| anyhow!("Account not found: {target}"))?,
        ));
    }

    choose_profile_for_edit(store)
}

fn choose_profile_for_edit(store: &AccountsStore) -> anyhow::Result<Option<String>> {
    if store.accounts.is_empty() {
        anyhow::bail!("No profiles configured. Use `cutex login` to create one.");
    }

    let default_index = store
        .active_account_id
        .as_ref()
        .and_then(|active_id| {
            store
                .accounts
                .iter()
                .position(|account| &account.id == active_id)
        })
        .unwrap_or(0);

    println!();
    println!("{BOLD}{CYAN}Choose Profile{RESET}");
    for (idx, account) in store.accounts.iter().enumerate() {
        let active = store.active_account_id.as_deref() == Some(account.id.as_str());
        let marker = if active {
            format!("{GREEN}●{RESET}")
        } else {
            format!("{DIM}○{RESET}")
        };
        let provider = account_model_provider(account).unwrap_or_else(|| "-".to_string());
        println!(
            "  {BOLD}{:>2}{RESET}. {marker} {CYAN}{}{RESET}  {DIM}{} / {} / {}{RESET}",
            idx + 1,
            account.name,
            account.cli_kind,
            account.source.as_deref().unwrap_or("-"),
            provider,
        );
    }

    loop {
        print!(
            "Select profile number [{BOLD}{}{RESET}], or q to quit: ",
            default_index + 1
        );
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let input = line.trim();
        if input.eq_ignore_ascii_case("q") {
            return Ok(None);
        }
        let choice = if input.is_empty() {
            default_index + 1
        } else {
            input
                .parse::<usize>()
                .with_context(|| format!("Invalid profile selection: {input}"))?
        };
        if choice == 0 || choice > store.accounts.len() {
            eprintln!("{YELLOW}warning:{RESET} profile selection out of range: {choice}");
            continue;
        }
        return Ok(Some(store.accounts[choice - 1].id.clone()));
    }
}

fn session_override_label(session: Option<&SessionConfig>) -> &'static str {
    match session {
        Some(SessionConfig { enabled: true }) => "enabled override",
        Some(SessionConfig { enabled: false }) => "disabled override",
        None => "inherit global",
    }
}

fn cmd_profile_edit(target: Option<&str>) -> anyhow::Result<()> {
    let initial_store = load_store()?;
    let Some(account_id) = profile_edit_target_id(&initial_store, target)? else {
        println!("Done.");
        return Ok(());
    };

    loop {
        let store = load_store()?;
        let global_config = load_codez_config();
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| anyhow!("Profile disappeared while editing"))?
            .clone();
        let is_host = matches!(account.runtime, RuntimeConfig::Host);
        let docker_image = match &account.runtime {
            RuntimeConfig::Docker { image, .. } => Some(image.as_str()),
            RuntimeConfig::Host => None,
        };
        let docker_user_name = match &account.runtime {
            RuntimeConfig::Docker { user_name, .. } => user_name.as_deref(),
            RuntimeConfig::Host => None,
        };
        let proxy_enabled = account.proxy.as_ref().is_some_and(|proxy| proxy.enabled);
        let proxy_disabled = account.proxy.as_ref().is_some_and(|proxy| !proxy.enabled);
        let proxy_inherit = account.proxy.is_none();
        let session_inherit = account.session.is_none();
        let session_enabled = account
            .session
            .as_ref()
            .is_some_and(|session| session.enabled);
        let session_disabled = account
            .session
            .as_ref()
            .is_some_and(|session| !session.enabled);

        println!();
        println!(
            "{BOLD}{CYAN}Profile Wizard{RESET} {BOLD}{}{RESET}",
            account.name
        );
        println!("{DIM}Boolean rows toggle immediately. Text rows prompt for a new value. Use `-` to clear optional values.{RESET}");
        println!(
            "  1.     name                                  {}",
            wizard_value(&account.name)
        );
        println!(
            "  2.     source                                {}",
            wizard_value(account.source.as_deref().unwrap_or("-"))
        );
        println!(
            "  3.     plan                                  {}",
            wizard_value(account.plan_type.as_deref().unwrap_or("-"))
        );
        println!(
            "  4.     email                                 {}",
            wizard_value(account.email.as_deref().unwrap_or("-"))
        );
        println!(
            "  5.     default cli args                      {}",
            wizard_value(cli_args_label(&account.default_cli_args))
        );
        println!(
            "  6. {} runtime host                          {}",
            checkbox(is_host),
            wizard_value(runtime_description(&account.runtime))
        );
        println!(
            "  7.     docker image                          {}",
            wizard_value(docker_image.unwrap_or("-"))
        );
        println!(
            "  8.     docker user name                      {}",
            wizard_value(docker_user_name.unwrap_or("-"))
        );
        println!(
            "  9. {} proxy inherit global                   {}",
            checkbox(proxy_inherit),
            wizard_value(account_proxy_scope_label(&account, &global_config))
        );
        println!(
            " 10. {} proxy disabled override                {}",
            checkbox(proxy_disabled),
            wizard_value(proxy_config_label(account.proxy.as_ref()))
        );
        println!(
            " 11. {} proxy enabled override                 {}",
            checkbox(proxy_enabled),
            wizard_value(proxy_config_label(account.proxy.as_ref()))
        );
        println!(
            " 12.     proxy url                             {}",
            wizard_value(
                account
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.url.as_deref())
                    .unwrap_or("-")
            )
        );
        println!(
            " 13.     proxy no_proxy                        {}",
            wizard_value(
                account
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.no_proxy.as_deref())
                    .unwrap_or("-")
            )
        );
        println!(
            " 14. {} proxy force_http                       {}",
            checkbox(
                account
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.force_http_transport)
            ),
            account
                .proxy
                .as_ref()
                .map(|proxy| bool_label(proxy.force_http_transport))
                .map(wizard_value)
                .unwrap_or_else(|| wizard_value("-"))
        );
        println!(
            " 15. {} session inherit global                 {}",
            checkbox(session_inherit),
            wizard_value(session_override_label(account.session.as_ref()))
        );
        println!(
            " 16. {} session enabled override               {}",
            checkbox(session_enabled),
            wizard_value(session_override_label(account.session.as_ref()))
        );
        println!(
            " 17. {} session disabled override              {}",
            checkbox(session_disabled),
            wizard_value(session_override_label(account.session.as_ref()))
        );
        println!(" 18.     show profile details");

        let Some(choice) = read_wizard_choice(18)? else {
            println!("Done.");
            return Ok(());
        };

        let mut store = load_store()?;
        let account_index = store
            .accounts
            .iter()
            .position(|candidate| candidate.id == account_id)
            .ok_or_else(|| anyhow!("Profile disappeared while editing"))?;
        let mut renamed: Option<(String, String)> = None;

        match choice {
            1 => {
                let current_name = store.accounts[account_index].name.clone();
                let name = prompt_line("Profile name", &current_name)?;
                let name = name.trim();
                if name.is_empty() {
                    anyhow::bail!("Profile name cannot be empty");
                }
                ensure_unique_name(&store, name, Some(&account_id))?;
                if store.accounts[account_index].name != name {
                    renamed = Some((store.accounts[account_index].name.clone(), name.to_string()));
                    store.accounts[account_index].name = name.to_string();
                }
            }
            2 => {
                store.accounts[account_index].source = prompt_optional_string(
                    "Profile source",
                    store.accounts[account_index].source.as_deref(),
                )?;
            }
            3 => {
                store.accounts[account_index].plan_type = prompt_optional_string(
                    "Profile plan",
                    store.accounts[account_index].plan_type.as_deref(),
                )?;
            }
            4 => {
                store.accounts[account_index].email = prompt_optional_string(
                    "Profile email",
                    store.accounts[account_index].email.as_deref(),
                )?;
            }
            5 => {
                let next_args = prompt_cli_args(
                    "Default CLI args",
                    &store.accounts[account_index].default_cli_args,
                )?;
                store.accounts[account_index].default_cli_args = next_args;
            }
            6 => {
                store.accounts[account_index].runtime = RuntimeConfig::Host;
            }
            7 => {
                let current_image = match &store.accounts[account_index].runtime {
                    RuntimeConfig::Docker { image, .. } => image.as_str(),
                    RuntimeConfig::Host => "cutex-base",
                };
                let image = prompt_line("Docker image", current_image)?;
                if image.trim().is_empty() || image.trim() == "-" {
                    store.accounts[account_index].runtime = RuntimeConfig::Host;
                } else {
                    let user_name = match &store.accounts[account_index].runtime {
                        RuntimeConfig::Docker { user_name, .. } => user_name.clone(),
                        RuntimeConfig::Host => None,
                    };
                    store.accounts[account_index].runtime = RuntimeConfig::Docker {
                        image: image.trim().to_string(),
                        user_name: Some(normalize_docker_user_name(user_name)?),
                    };
                }
            }
            8 => {
                let current_user_name = match &store.accounts[account_index].runtime {
                    RuntimeConfig::Docker { user_name, .. } => user_name.as_deref().unwrap_or(""),
                    RuntimeConfig::Host => "",
                };
                let value = prompt_line("Docker user name", current_user_name)?;
                match &mut store.accounts[account_index].runtime {
                    RuntimeConfig::Docker { user_name, .. } => {
                        *user_name = Some(normalize_docker_user_name(Some(value))?);
                    }
                    RuntimeConfig::Host => {
                        println!("{YELLOW}Set Docker image first.{RESET}");
                        continue;
                    }
                }
            }
            9 => {
                store.accounts[account_index].proxy = None;
            }
            10 => {
                store.accounts[account_index].proxy = Some(proxy_config_from_parts(
                    false, None, None, /*force_http_transport*/ true,
                )?);
            }
            11 => {
                let url = store.accounts[account_index]
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.url.clone())
                    .unwrap_or_else(|| "socks5h://127.0.0.1:7890".to_string());
                store.accounts[account_index].proxy = Some(proxy_config_from_parts(
                    true,
                    Some(url),
                    store.accounts[account_index]
                        .proxy
                        .as_ref()
                        .and_then(|proxy| proxy.no_proxy.clone()),
                    store.accounts[account_index]
                        .proxy
                        .as_ref()
                        .map(|proxy| proxy.force_http_transport)
                        .unwrap_or(true),
                )?);
            }
            12 => {
                let current_url = store.accounts[account_index]
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.url.as_deref());
                let url = prompt_optional_string("Profile proxy URL", current_url)?;
                store.accounts[account_index].proxy = url
                    .map(|url| {
                        proxy_config_from_parts(
                            true,
                            Some(url),
                            store.accounts[account_index]
                                .proxy
                                .as_ref()
                                .and_then(|proxy| proxy.no_proxy.clone()),
                            store.accounts[account_index]
                                .proxy
                                .as_ref()
                                .map(|proxy| proxy.force_http_transport)
                                .unwrap_or(true),
                        )
                    })
                    .transpose()?;
            }
            13 => {
                let Some(proxy) = store.accounts[account_index].proxy.as_mut() else {
                    println!("{YELLOW}Enable profile proxy first.{RESET}");
                    continue;
                };
                proxy.no_proxy =
                    prompt_optional_string("Profile proxy NO_PROXY", proxy.no_proxy.as_deref())?;
            }
            14 => {
                let Some(proxy) = store.accounts[account_index].proxy.as_mut() else {
                    println!("{YELLOW}Enable profile proxy first.{RESET}");
                    continue;
                };
                proxy.force_http_transport = !proxy.force_http_transport;
            }
            15 => {
                store.accounts[account_index].session = None;
            }
            16 => {
                store.accounts[account_index].session = Some(SessionConfig { enabled: true });
            }
            17 => {
                store.accounts[account_index].session = Some(SessionConfig { enabled: false });
            }
            18 => {
                print_profile_details(&store, &store.accounts[account_index], &global_config)?;
                continue;
            }
            _ => unreachable!(),
        }

        save_store(&store)?;
        if let Some((old_name, new_name)) = renamed {
            let mut state = load_quick_state();
            rename_profile_references(&mut state, &old_name, &new_name);
            let _ = save_quick_state(&state);
        }
        println!("{GREEN}Saved.{RESET}");
    }
}

fn cmd_profile_pin(target: &str, to_top: bool) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let index = store
        .accounts
        .iter()
        .position(|account| account.name == target || account.id == target)
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    let destination_index = if to_top { 0 } else { store.accounts.len() - 1 };
    if index == destination_index {
        let account = &store.accounts[index];
        println!(
            "{YELLOW}No changes{RESET} profile {BOLD}{}{RESET} is already at the {}",
            account.name,
            if to_top { "top" } else { "bottom" }
        );
        return Ok(());
    }

    let account = store.accounts.remove(index);
    let account_name = account.name.clone();
    if to_top {
        store.accounts.insert(0, account);
    } else {
        store.accounts.push(account);
    }
    save_store(&store)?;
    println!(
        "{GREEN}Moved{RESET} profile {BOLD}{}{RESET} to the {}",
        account_name,
        if to_top { "top" } else { "bottom" }
    );
    Ok(())
}

fn cmd_profile_clone_status_line(from: Option<&str>) -> anyhow::Result<()> {
    let store = load_store()?;
    if store.accounts.is_empty() {
        anyhow::bail!(
            "No accounts configured. Use `cutex add --from-auth <path> --name <name>` to add one."
        );
    }

    let source_account = match from {
        Some(target) => {
            find_account(&store, target)?.ok_or_else(|| anyhow!("Account not found: {target}"))?
        }
        None => {
            let active_id = store
                .active_account_id
                .as_ref()
                .ok_or_else(|| anyhow!("No active profile. Use `cutex use <name>` first."))?;
            store
                .accounts
                .iter()
                .find(|account| account.id.as_str() == active_id.as_str())
                .ok_or_else(|| anyhow!("Active profile not found: {active_id}"))?
        }
    };

    let source_files = materialized_account_files(source_account)?;
    let source_config = read_optional_text(&source_files.config_path)?.ok_or_else(|| {
        anyhow!(
            "Source profile has no config.toml: {}",
            source_files.config_path.display()
        )
    })?;
    let source_table = parse_toml_table(&source_config).with_context(|| {
        format!(
            "Failed to parse source profile config.toml: {}",
            source_files.config_path.display()
        )
    })?;
    let source_status_line = source_table
        .get("tui")
        .and_then(|value| value.as_table())
        .and_then(|tui| tui.get("status_line"))
        .and_then(|value| value.as_array())
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Source profile '{}' has no [tui].status_line in {}",
                source_account.name,
                source_files.config_path.display()
            )
        })?;

    let global_config = load_codez_config();
    for account in &store.accounts {
        let files = materialized_account_files(account)?;
        let mut profile_table = read_profile_specific_config_table(&files.config_path)?;
        let tui_entry = profile_table
            .entry("tui".to_string())
            .or_insert_with(|| toml::Value::Table(Table::new()));
        let tui_table = tui_entry
            .as_table_mut()
            .ok_or_else(|| anyhow!("config.toml key `tui` must be a table"))?;
        tui_table.insert(
            "status_line".to_string(),
            toml::Value::Array(source_status_line.clone()),
        );

        let profile_config_toml = toml::to_string_pretty(&profile_table)?;
        merge_and_write_config_toml(
            &files.config_path,
            Some(profile_config_toml.as_str()),
            effective_proxy_config(account, &global_config)
                .map(|proxy| proxy.enabled)
                .unwrap_or(false),
        )?;
        set_materialized_file_permissions(&files)?;
    }

    println!(
        "{GREEN}Cloned{RESET} [tui].status_line from {BOLD}{}{RESET} to {} profiles",
        source_account.name,
        store.accounts.len()
    );
    Ok(())
}

fn cmd_add(
    auth_path: &str,
    config_path: Option<&str>,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
    name: &str,
    cli: &str,
) -> anyhow::Result<()> {
    let cli_kind: CliKind = cli.parse()?;
    let mut store = load_store()?;
    ensure_unique_name(&store, name, None)?;

    let runtime = runtime_from_option(docker_image, docker_user_name);

    if cli_kind == CliKind::Claude {
        let id = Uuid::new_v4().to_string();
        let account = StoredAccount {
            id,
            name: name.to_string(),
            email: None,
            plan_type: None,
            source: Some("anthropic".to_string()),
            runtime,
            proxy: None,
            session: None,
            cli_kind: CliKind::Claude,
            default_cli_args: Vec::new(),
            last_used_at: Some(Utc::now()),
        };
        ensure_claude_profile_dir(&account, auth_path)?;
        add_account_to_store(&mut store, account)
    } else {
        let snapshot = import_snapshot(auth_path, config_path)?;
        let mut account = StoredAccount::from_import(name.to_string(), &snapshot, runtime);
        account.cli_kind = cli_kind;
        materialize_imported_account_files(&account, &snapshot)?;
        add_account_to_store(&mut store, account)
    }
}

fn cmd_login(
    name: Option<&str>,
    cli: Option<&str>,
    api_key: Option<&str>,
    base_url: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<()> {
    if name.is_none() && api_key.is_none() {
        return cmd_login_interactive();
    }

    let cli_str = cli.unwrap_or("codex");
    let cli_kind: CliKind = cli_str.parse()?;

    if let Some(key) = api_key {
        let profile_name = name.ok_or_else(|| anyhow!("--name is required with --api-key"))?;
        return cmd_login_api_key(
            profile_name,
            &cli_kind,
            key,
            base_url,
            provider.unwrap_or("custom"),
        );
    }

    let profile_name = name.ok_or_else(|| anyhow!("--name is required for official login"))?;
    let mut store = load_store()?;
    ensure_unique_name(&store, profile_name, None)?;

    match cli_kind {
        CliKind::Claude => cmd_login_claude_official(profile_name, &mut store),
        CliKind::Codex => cmd_login_codex_official(profile_name, &mut store),
    }
}

fn prompt_line(label: &str, default: &str) -> anyhow::Result<String> {
    if default.is_empty() {
        print!("{BOLD}{label}{RESET}: ");
    } else {
        print!("{BOLD}{label}{RESET} [{CYAN}{default}{RESET}]: ");
    }
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let input = line.trim();
    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}

fn prompt_choice(label: &str, options: &[(&str, &str)], default: usize) -> anyhow::Result<usize> {
    for (i, (name, desc)) in options.iter().enumerate() {
        let marker = if i + 1 == default {
            format!("{GREEN}▸{RESET}")
        } else {
            format!(" ")
        };
        println!(
            "{marker} {BOLD}[{}]{RESET} {CYAN}{name}{RESET}  {DIM}{desc}{RESET}",
            i + 1
        );
    }
    let answer = prompt_line(label, &default.to_string())?;
    let idx = answer.parse::<usize>().unwrap_or(default);
    if idx < 1 || idx > options.len() {
        Ok(default)
    } else {
        Ok(idx)
    }
}

fn cmd_login_interactive() -> anyhow::Result<()> {
    println!("{BOLD}{CYAN}cutex login{RESET}\n");

    println!("{BOLD}Step 1:{RESET} Choose CLI");
    let cli_choice = prompt_choice(
        "CLI",
        &[
            ("codex", "OpenAI Codex"),
            ("claude", "Anthropic Claude Code"),
        ],
        1,
    )?;
    let cli_kind = if cli_choice == 2 {
        CliKind::Claude
    } else {
        CliKind::Codex
    };
    println!();

    println!("{BOLD}Step 2:{RESET} Choose auth method");
    let auth_choice = prompt_choice(
        "Auth",
        &[
            ("official", "OAuth login"),
            ("api-key", "Third-party API key + base URL"),
        ],
        1,
    )?;
    println!();

    if auth_choice == 2 {
        let default_url = match cli_kind {
            CliKind::Codex => "https://api.openai.com/v1",
            CliKind::Claude => "https://api.anthropic.com",
        };
        let url = prompt_line(&format!("{BOLD}Step 3:{RESET} API base URL"), default_url)?;
        println!();

        let key = prompt_line(&format!("{BOLD}Step 4:{RESET} API key"), "")?;
        if key.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        println!();

        let prov = prompt_line(
            &format!("{BOLD}Step 5:{RESET} Provider name (for display)"),
            "custom",
        )?;
        println!();

        let name = prompt_line(&format!("{BOLD}Step 6:{RESET} Profile name"), "")?;
        if name.is_empty() {
            anyhow::bail!("Profile name cannot be empty");
        }

        return cmd_login_api_key(&name, &cli_kind, &key, Some(url.as_str()), &prov);
    }

    let name = prompt_line(&format!("{BOLD}Step 3:{RESET} Profile name"), "")?;
    if name.is_empty() {
        anyhow::bail!("Profile name cannot be empty");
    }
    println!();

    let mut store = load_store()?;
    ensure_unique_name(&store, &name, None)?;

    match cli_kind {
        CliKind::Claude => cmd_login_claude_official(&name, &mut store),
        CliKind::Codex => cmd_login_codex_official(&name, &mut store),
    }
}

fn cmd_login_api_key(
    name: &str,
    cli_kind: &CliKind,
    api_key: &str,
    base_url: Option<&str>,
    provider: &str,
) -> anyhow::Result<()> {
    let mut store = load_store()?;
    ensure_unique_name(&store, name, None)?;

    let id = Uuid::new_v4().to_string();
    let account = StoredAccount {
        id,
        name: name.to_string(),
        email: None,
        plan_type: Some(provider.to_string()),
        source: Some("api-key".to_string()),
        runtime: RuntimeConfig::Host,
        proxy: None,
        session: None,
        cli_kind: cli_kind.clone(),
        default_cli_args: Vec::new(),
        last_used_at: Some(Utc::now()),
    };

    match cli_kind {
        CliKind::Codex => {
            let auth_json = serde_json::json!({
                "OPENAI_API_KEY": api_key,
                "tokens": null
            });
            let config_toml_str = codex_api_key_config_toml(provider, base_url);
            let snapshot = ImportedSnapshot {
                raw_auth_json: auth_json.to_string(),
                raw_config_toml: Some(config_toml_str),
                email: None,
                plan_type: Some(provider.to_string()),
                source: "api-key".to_string(),
            };
            materialize_imported_account_files(&account, &snapshot)?;
        }
        CliKind::Claude => {
            let profile_dir = materialized_claude_config_dir(&account);
            fs::create_dir_all(&profile_dir)?;

            let api_key_path = profile_dir.join("api_key");
            fs::write(&api_key_path, api_key)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&api_key_path, fs::Permissions::from_mode(0o600))?;
            }

            if let Some(url) = base_url {
                let provider_json = serde_json::json!({
                    "provider": provider,
                    "base_url": url
                });
                fs::write(
                    profile_dir.join("provider.json"),
                    serde_json::to_string_pretty(&provider_json)?,
                )?;
            }
        }
    }

    add_account_to_store(&mut store, account)?;
    println!(
        "{GREEN}Profile {BOLD}{name}{RESET}{GREEN} created ({}, api-key, {provider}).{RESET}",
        cli_kind
    );
    Ok(())
}

fn codex_api_key_config_toml(provider: &str, base_url: Option<&str>) -> String {
    let mut config_lines = vec![format!("model_provider = \"{}\"", provider)];
    if let Some(url) = base_url {
        config_lines.push(format!("\n[model_providers.{}]", provider));
        config_lines.push(format!("name = \"{}\"", provider));
        config_lines.push(format!("base_url = \"{}\"", url));
        config_lines.push("env_key = \"OPENAI_API_KEY\"".to_string());
        config_lines.push("requires_openai_auth = false".to_string());
    }
    config_lines.join("\n") + "\n"
}

fn cmd_login_codex_official(name: &str, store: &mut AccountsStore) -> anyhow::Result<()> {
    let program = codex_program();
    let global_config = load_codez_config();

    let tmp_home = login_runtime_root()?.join(format!("codex-login-{}", Uuid::new_v4()));
    fs::create_dir_all(&tmp_home)
        .with_context(|| format!("Failed to create temp codex home: {}", tmp_home.display()))?;

    let tmp_config = tmp_home.join("config.toml");
    let config_contents = "cli_auth_credentials_store = \"file\"\n";
    fs::write(&tmp_config, config_contents)
        .with_context(|| format!("Failed to write temp config.toml: {}", tmp_config.display()))?;

    println!(
        "Starting `{}` for {BOLD}{}{RESET} using {}",
        format!("{program} login --device-auth"),
        name,
        tmp_home.display()
    );
    let mut command = Command::new(&program);
    scrub_codex_login_env(&mut command);
    command.env("CODEX_HOME", &tmp_home);
    for (key, value) in proxy_envs(global_config.proxy.as_ref(), None) {
        command.env(key, value);
    }
    let status = command
        .arg("login")
        .arg("--device-auth")
        .status()
        .with_context(|| format!("Failed to start {program} login"))?;

    if !status.success() {
        let _ = fs::remove_dir_all(&tmp_home);
        anyhow::bail!("{program} login exited with status {status}");
    }

    let auth_path = tmp_home.join("auth.json");
    if !auth_path.exists() {
        let _ = fs::remove_dir_all(&tmp_home);
        anyhow::bail!(
            "{program} login did not produce auth.json at {}",
            auth_path.display()
        );
    }

    let snapshot = import_snapshot(
        auth_path
            .to_str()
            .ok_or_else(|| anyhow!("Invalid auth.json path"))?,
        Some(
            tmp_config
                .to_str()
                .ok_or_else(|| anyhow!("Invalid config.toml path"))?,
        ),
    )?;
    let mut account = StoredAccount::from_import(name.to_string(), &snapshot, RuntimeConfig::Host);
    account.source = Some("official".to_string());
    materialize_imported_account_files(&account, &snapshot)?;
    add_account_to_store(store, account)?;

    let _ = fs::remove_dir_all(&tmp_home);
    Ok(())
}

fn codex_login_env_override_keys() -> &'static [&'static str] {
    &[
        CODEX_CONFIG_FILE_ENV_VAR,
        CODEX_AUTH_FILE_ENV_VAR,
        CODEX_CUSTOM_STATUS_ITEMS_FILE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_ENV_VAR,
        CODEX_LAUNCH_RUNTIME_ENV_VAR,
        CODEX_LAUNCH_PROFILE_SOURCE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_TYPE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_EMAIL_ENV_VAR,
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
    ]
}

fn scrub_codex_login_env(command: &mut Command) {
    for key in codex_login_env_override_keys() {
        command.env_remove(key);
    }
}

fn cmd_login_claude_official(name: &str, store: &mut AccountsStore) -> anyhow::Result<()> {
    let program = claude_program();

    let tmp_claude_dir = login_runtime_root()?.join(format!("claude-login-{}", Uuid::new_v4()));
    fs::create_dir_all(&tmp_claude_dir).with_context(|| {
        format!(
            "Failed to create temp claude dir: {}",
            tmp_claude_dir.display()
        )
    })?;

    println!("Starting `{program}` for {BOLD}{name}{RESET} — please complete the OAuth login.",);
    println!("Using temp config dir: {}", tmp_claude_dir.display());
    println!("{DIM}Press Ctrl+C after login completes to return to cutex.{RESET}");

    let status = Command::new(&program)
        .env(CLAUDE_CONFIG_DIR_ENV_VAR, &tmp_claude_dir)
        .status()
        .with_context(|| format!("Failed to start {program}"))?;

    // Claude Code exits normally after the session — check if credentials were created
    let credentials_path = tmp_claude_dir.join(".credentials.json");
    if !credentials_path.exists() {
        let _ = fs::remove_dir_all(&tmp_claude_dir);
        if !status.success() {
            anyhow::bail!("{program} exited with status {status} and no credentials were saved");
        }
        anyhow::bail!(
            "No credentials found at {} — login may not have completed.\nNote: if this system uses keychain auth, use `cutex add --cli claude --from-auth <path> --name {name}` instead.",
            credentials_path.display()
        );
    }

    let id = Uuid::new_v4().to_string();
    let account = StoredAccount {
        id: id.clone(),
        name: name.to_string(),
        email: None,
        plan_type: None,
        source: Some("anthropic".to_string()),
        runtime: RuntimeConfig::Host,
        proxy: None,
        session: None,
        cli_kind: CliKind::Claude,
        default_cli_args: Vec::new(),
        last_used_at: Some(Utc::now()),
    };

    let profile_claude_dir = materialized_claude_config_dir(&account);
    fs::create_dir_all(&profile_claude_dir)?;

    // Move credentials from temp dir to profile dir
    let target_credentials = profile_claude_dir.join(".credentials.json");
    fs::copy(&credentials_path, &target_credentials)?;

    // Copy settings.json if it exists
    let settings_path = tmp_claude_dir.join("settings.json");
    if settings_path.exists() {
        fs::copy(&settings_path, profile_claude_dir.join("settings.json"))?;
    }

    let _ = fs::remove_dir_all(&tmp_claude_dir);
    add_account_to_store(store, account)?;
    println!("{GREEN}Claude profile {BOLD}{name}{RESET}{GREEN} created.{RESET}");
    Ok(())
}

fn ensure_claude_profile_dir(account: &StoredAccount, auth_path: &str) -> anyhow::Result<()> {
    let profile_dir = materialized_claude_config_dir(account);
    fs::create_dir_all(&profile_dir).with_context(|| {
        format!(
            "Failed to create Claude profile dir: {}",
            profile_dir.display()
        )
    })?;

    let target = profile_dir.join(".credentials.json");
    fs::copy(auth_path, &target)
        .with_context(|| format!("Failed to copy credentials to {}", target.display()))?;

    Ok(())
}

fn cmd_rename(target: &str, new_name: &str) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let account_id = find_account(&store, target)?
        .map(|account| account.id.clone())
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    ensure_unique_name(&store, new_name, Some(&account_id))?;

    let old_name = {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| anyhow!("Account not found after lookup: {target}"))?;
        let old_name = account.name.clone();
        account.name = new_name.to_string();
        old_name
    };

    save_store(&store)?;

    let mut state = load_quick_state();
    rename_profile_references(&mut state, &old_name, new_name);
    let _ = save_quick_state(&state);

    let mut global_config = load_codez_config();
    if rename_global_profile_references(&mut global_config, &old_name, new_name) {
        save_codez_config(&global_config)?;
    }

    println!(
        "{GREEN}Renamed{RESET} profile {BOLD}{}{RESET} -> {BOLD}{}{RESET}",
        old_name, new_name
    );
    Ok(())
}

fn cmd_remove(target: &str) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let account = find_account(&store, target)?
        .cloned()
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    store
        .accounts
        .retain(|candidate| candidate.id != account.id);

    if store.active_account_id.as_deref() == Some(account.id.as_str()) {
        store.active_account_id = store.accounts.first().map(|next| next.id.clone());
    }
    save_store(&store)?;

    let mut state = load_quick_state();
    remove_profile_references(&mut state, &account.name);
    let _ = save_quick_state(&state);

    let mut global_config = load_codez_config();
    if remove_global_profile_references(&mut global_config, &account.name) {
        save_codez_config(&global_config)?;
    }

    println!(
        "{YELLOW}Removed{RESET} profile {BOLD}{}{RESET}",
        account.name
    );
    if let Some(active_id) = store.active_account_id.as_ref() {
        if let Some(next) = store
            .accounts
            .iter()
            .find(|candidate| &candidate.id == active_id)
        {
            println!("Current active profile: {}", next.name);
        }
    }

    Ok(())
}

fn cmd_annotate(
    target: &str,
    source: Option<String>,
    clear_source: bool,
    plan: Option<String>,
    clear_plan: bool,
    email: Option<String>,
    clear_email: bool,
) -> anyhow::Result<()> {
    if !(source.is_some()
        || clear_source
        || plan.is_some()
        || clear_plan
        || email.is_some()
        || clear_email)
    {
        anyhow::bail!(
            "Specify at least one of --source, --clear-source, --plan, --clear-plan, --email, or --clear-email"
        );
    }

    let mut store = load_store()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.name == target || account.id == target)
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    apply_annotation(
        account,
        source,
        clear_source,
        plan,
        clear_plan,
        email,
        clear_email,
    );

    let name = account.name.clone();
    save_store(&store)?;

    println!("{GREEN}Updated{RESET} metadata for {BOLD}{}{RESET}", name);
    Ok(())
}

fn cmd_runtime(
    target: &str,
    host: bool,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let runtime = if let Some(image) = docker_image {
        RuntimeConfig::Docker {
            image,
            user_name: Some(normalize_docker_user_name(docker_user_name)?),
        }
    } else if host {
        RuntimeConfig::Host
    } else {
        anyhow::bail!("Specify either --host or --docker-image <IMAGE>");
    };

    let account_name = {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.name == target || account.id == target)
            .ok_or_else(|| anyhow!("Account not found: {target}"))?;
        account.runtime = runtime.clone();
        account.name.clone()
    };

    save_store(&store)?;
    println!(
        "{GREEN}Updated{RESET} runtime for {BOLD}{}{RESET} to {}",
        account_name,
        runtime_label(&runtime)
    );
    Ok(())
}

fn print_global_settings(config: &CodezConfig) {
    println!("{BOLD}{CYAN}Global Settings{RESET}");
    println!(
        "{DIM}docker_use_sudo{RESET} {}",
        bool_label(config.docker_use_sudo)
    );
    println!(
        "{DIM}custom_status_items{RESET} {}",
        config.custom_status_items.len()
    );
    println!(
        "{DIM}session{RESET} {}",
        session_config_label(&config.session)
    );
    println!(
        "{DIM}default_profile{RESET} {}",
        config.default_profile.as_deref().unwrap_or("-")
    );
    println!(
        "{DIM}default_profile_direct_launch{RESET} {}",
        bool_label(config.default_profile_direct_launch)
    );
    println!(
        "{DIM}proxy{RESET} {}",
        proxy_config_label(config.proxy.as_ref())
    );
    println!(
        "{DIM}notify_service_url{RESET} {}",
        config.notify_service_url.as_deref().unwrap_or("-")
    );
    println!(
        "{DIM}notify_service_token{RESET} {}",
        if config
            .notify_service_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
        {
            "(set)"
        } else {
            "-"
        }
    );
    println!(
        "{DIM}notify_service_idle_timeout_secs{RESET} {}",
        config
            .notify_service_idle_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}notify_service_composer_idle_timeout_secs{RESET} {}",
        config
            .notify_service_composer_idle_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}notify_service_approval_timeout_secs{RESET} {}",
        config
            .notify_service_approval_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}notify_service_startup_idle_timeout_secs{RESET} {}",
        config
            .notify_service_startup_idle_timeout_secs
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}notify_service_events{RESET} {}",
        config
            .notify_service_events
            .as_ref()
            .map(|events| events.join(","))
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}notify_service_user_message_content{RESET} {}",
        config
            .notify_service_user_message_content
            .as_deref()
            .unwrap_or("-")
    );
    println!(
        "{DIM}notify_service_user_message_preview_chars{RESET} {}",
        config
            .notify_service_user_message_preview_chars
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!(
        "{DIM}rate_limit_threshold_warning_mode{RESET} {}",
        optional_label(config.rate_limit_threshold_warning_mode.as_deref())
    );
    println!(
        "{DIM}rate_limit_model_nudge_mode{RESET} {}",
        optional_label(config.rate_limit_model_nudge_mode.as_deref())
    );
    println!(
        "{DIM}desktop_notify_enabled{RESET} {}",
        bool_label(config.desktop_notify_enabled)
    );
    println!(
        "{DIM}desktop_notify_port{RESET} {}",
        config
            .desktop_notify_port
            .unwrap_or(DEFAULT_DESKTOP_NOTIFY_PORT)
    );
    println!(
        "{DIM}desktop_notify_token{RESET} {}",
        if config
            .desktop_notify_token
            .as_ref()
            .is_some_and(|token| !token.is_empty())
        {
            "(set)"
        } else {
            "-"
        }
    );
}

fn checkbox(value: bool) -> String {
    if value {
        format!("{GREEN}[x]{RESET}")
    } else {
        format!("{DIM}[ ]{RESET}")
    }
}

fn wizard_value(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    if value.is_empty() || value == "-" {
        format!("{DIM}-{RESET}")
    } else {
        format!("{BOLD}{value}{RESET}")
    }
}

fn optional_label(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn optional_u64_label(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn cli_args_label(args: &[String]) -> String {
    if args.is_empty() {
        "-".to_string()
    } else {
        args.iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn parse_cli_args_value(value: &str) -> anyhow::Result<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Ok(Vec::new());
    }

    shlex::split(trimmed).ok_or_else(|| anyhow!("Invalid shell-style CLI args: {value}"))
}

fn read_wizard_choice(max: usize) -> anyhow::Result<Option<usize>> {
    print!("{CYAN}Select item number{RESET}, or q to quit: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let input = line.trim();
    if input.is_empty() || input.eq_ignore_ascii_case("q") {
        return Ok(None);
    }
    let choice = input
        .parse::<usize>()
        .with_context(|| format!("Invalid menu selection: {input}"))?;
    if choice == 0 || choice > max {
        anyhow::bail!("Menu selection out of range: {choice}");
    }
    Ok(Some(choice))
}

fn prompt_optional_string(label: &str, current: Option<&str>) -> anyhow::Result<Option<String>> {
    let current_label = optional_label(current);
    let value = prompt_line(&format!("{label} (`-` clears)"), &current_label)?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn prompt_cli_args(label: &str, current: &[String]) -> anyhow::Result<Vec<String>> {
    let current_label = cli_args_label(current);
    let value = prompt_line(&format!("{label} (`-` clears)"), &current_label)?;
    parse_cli_args_value(&value)
}

fn prompt_optional_u64(label: &str, current: Option<u64>) -> anyhow::Result<Option<u64>> {
    let current_label = optional_u64_label(current);
    let value = prompt_line(&format!("{label} (`-` clears)"), &current_label)?;
    parse_optional_u64(&value)
}

fn prompt_optional_csv(
    label: &str,
    current: Option<&[String]>,
) -> anyhow::Result<Option<Vec<String>>> {
    let current_label = current
        .map(|items| items.join(","))
        .unwrap_or_else(|| "-".to_string());
    let value = prompt_line(&format!("{label} (`-` clears)"), &current_label)?;
    Ok(parse_optional_csv(&value))
}

fn cmd_global_edit() -> anyhow::Result<()> {
    loop {
        let config = load_codez_config();
        println!();
        println!("{BOLD}{CYAN}Global Settings Wizard{RESET}");
        println!("{DIM}Boolean rows toggle immediately. Text rows prompt for a new value. Use `-` to clear optional values.{RESET}");
        println!(
            "  1. {} docker_use_sudo                         {}",
            checkbox(config.docker_use_sudo),
            bool_label(config.docker_use_sudo)
        );
        println!(
            "  2. {} managed sessions                        {}",
            checkbox(config.session.enabled),
            session_config_label(&config.session)
        );
        println!(
            "  3. {} direct launch default profile          {}",
            checkbox(config.default_profile_direct_launch),
            bool_label(config.default_profile_direct_launch)
        );
        println!(
            "  4.     default profile                        {}",
            config.default_profile.as_deref().unwrap_or("-")
        );
        println!(
            "  5. {} global proxy enabled                    {}",
            checkbox(config.proxy.as_ref().is_some_and(|proxy| proxy.enabled)),
            proxy_config_label(config.proxy.as_ref())
        );
        println!(
            "  6.     proxy url                              {}",
            config
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.url.as_deref())
                .unwrap_or("-")
        );
        println!(
            "  7.     proxy no_proxy                         {}",
            config
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.no_proxy.as_deref())
                .unwrap_or("-")
        );
        println!(
            "  8. {} proxy force_http                        {}",
            checkbox(
                config
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.force_http_transport)
            ),
            config
                .proxy
                .as_ref()
                .map(|proxy| bool_label(proxy.force_http_transport))
                .unwrap_or("-")
        );
        println!(
            "  9.     notify service url                     {}",
            config.notify_service_url.as_deref().unwrap_or("-")
        );
        println!(
            " 10.     notify service token                   {}",
            if config
                .notify_service_token
                .as_ref()
                .is_some_and(|token| !token.is_empty())
            {
                "(set)"
            } else {
                "-"
            }
        );
        println!(
            " 11.     notify idle timeout                    {}",
            optional_u64_label(config.notify_service_idle_timeout_secs)
        );
        println!(
            " 12.     notify composer idle timeout           {}",
            optional_u64_label(config.notify_service_composer_idle_timeout_secs)
        );
        println!(
            " 13.     notify approval timeout                {}",
            optional_u64_label(config.notify_service_approval_timeout_secs)
        );
        println!(
            " 14.     notify startup idle timeout            {}",
            optional_u64_label(config.notify_service_startup_idle_timeout_secs)
        );
        println!(
            " 15.     notify events                          {}",
            config
                .notify_service_events
                .as_ref()
                .map(|events| events.join(","))
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            " 16.     notify user message content            {}",
            config
                .notify_service_user_message_content
                .as_deref()
                .unwrap_or("-")
        );
        println!(
            " 17.     notify user message preview chars      {}",
            optional_u64_label(config.notify_service_user_message_preview_chars)
        );
        println!(
            " 18.     rate limit threshold warning mode      {}",
            config
                .rate_limit_threshold_warning_mode
                .as_deref()
                .unwrap_or("-")
        );
        println!(
            " 19.     rate limit model nudge mode            {}",
            config.rate_limit_model_nudge_mode.as_deref().unwrap_or("-")
        );
        println!(" 20.     show current settings");

        let Some(choice) = read_wizard_choice(20)? else {
            println!("Done.");
            return Ok(());
        };

        let mut next = config.clone();
        match choice {
            1 => next.docker_use_sudo = !next.docker_use_sudo,
            2 => next.session.enabled = !next.session.enabled,
            3 => next.default_profile_direct_launch = !next.default_profile_direct_launch,
            4 => {
                let store = load_store()?;
                let value = prompt_optional_string(
                    "Default profile name or id",
                    next.default_profile.as_deref(),
                )?;
                next.default_profile = resolve_configured_default_profile_name(&store, value)?;
            }
            5 => {
                if next.proxy.as_ref().is_some_and(|proxy| proxy.enabled) {
                    next.proxy = None;
                } else {
                    let url = prompt_line("Proxy URL", "socks5h://127.0.0.1:7890")?;
                    next.proxy = Some(proxy_config_from_parts(
                        true,
                        Some(url),
                        None,
                        /*force_http_transport*/ true,
                    )?);
                }
            }
            6 => {
                let url = prompt_optional_string(
                    "Proxy URL",
                    next.proxy.as_ref().and_then(|proxy| proxy.url.as_deref()),
                )?;
                next.proxy = url
                    .map(|url| {
                        proxy_config_from_parts(
                            true,
                            Some(url),
                            next.proxy.as_ref().and_then(|proxy| proxy.no_proxy.clone()),
                            next.proxy
                                .as_ref()
                                .map(|proxy| proxy.force_http_transport)
                                .unwrap_or(true),
                        )
                    })
                    .transpose()?;
            }
            7 => {
                let Some(proxy) = next.proxy.as_mut() else {
                    println!("{YELLOW}Enable proxy first.{RESET}");
                    continue;
                };
                proxy.no_proxy =
                    prompt_optional_string("Proxy NO_PROXY", proxy.no_proxy.as_deref())?;
            }
            8 => {
                let Some(proxy) = next.proxy.as_mut() else {
                    println!("{YELLOW}Enable proxy first.{RESET}");
                    continue;
                };
                proxy.force_http_transport = !proxy.force_http_transport;
            }
            9 => {
                next.notify_service_url = prompt_optional_string(
                    "Notify service URL",
                    next.notify_service_url.as_deref(),
                )?;
            }
            10 => {
                next.notify_service_token = prompt_optional_string(
                    "Notify service token",
                    next.notify_service_token.as_deref(),
                )?;
            }
            11 => {
                next.notify_service_idle_timeout_secs = prompt_optional_u64(
                    "Notify idle timeout seconds",
                    next.notify_service_idle_timeout_secs,
                )?;
            }
            12 => {
                next.notify_service_composer_idle_timeout_secs = prompt_optional_u64(
                    "Notify composer idle timeout seconds",
                    next.notify_service_composer_idle_timeout_secs,
                )?;
            }
            13 => {
                next.notify_service_approval_timeout_secs = prompt_optional_u64(
                    "Notify approval timeout seconds",
                    next.notify_service_approval_timeout_secs,
                )?;
            }
            14 => {
                next.notify_service_startup_idle_timeout_secs = prompt_optional_u64(
                    "Notify startup idle timeout seconds",
                    next.notify_service_startup_idle_timeout_secs,
                )?;
            }
            15 => {
                let current = next.notify_service_events.as_deref();
                let events = prompt_optional_csv("Notify event CSV", current)?;
                next.notify_service_events = events;
                if next.notify_service_events.is_none() {
                    println!(
                        "{DIM}Using cute-codex default events: {DEFAULT_NOTIFY_EVENTS}{RESET}"
                    );
                }
            }
            16 => {
                let current = next
                    .notify_service_user_message_content
                    .as_deref()
                    .unwrap_or("-");
                let value = prompt_line(
                    "Notify user message content: none, preview, full (`-` clears)",
                    current,
                )?;
                next.notify_service_user_message_content =
                    parse_optional_user_message_content(&value)?;
            }
            17 => {
                next.notify_service_user_message_preview_chars = prompt_optional_u64(
                    "Notify user message preview chars",
                    next.notify_service_user_message_preview_chars,
                )?;
            }
            18 => {
                let current = next
                    .rate_limit_threshold_warning_mode
                    .as_deref()
                    .unwrap_or("-");
                let value = prompt_line(
                    "Rate limit threshold warning mode: off, daily, always (`-` clears)",
                    current,
                )?;
                next.rate_limit_threshold_warning_mode = parse_optional_rate_limit_mode(&value)?;
            }
            19 => {
                let current = next.rate_limit_model_nudge_mode.as_deref().unwrap_or("-");
                let value = prompt_line(
                    "Rate limit model nudge mode: off, daily, always (`-` clears)",
                    current,
                )?;
                next.rate_limit_model_nudge_mode = parse_optional_rate_limit_mode(&value)?;
            }
            20 => {
                print_global_settings(&config);
                continue;
            }
            _ => unreachable!(),
        }

        save_codez_config(&next)?;
        println!("{GREEN}Saved.{RESET}");
    }
}

fn cmd_global(command: GlobalCommand) -> anyhow::Result<()> {
    match command {
        GlobalCommand::Show => {
            let config = load_codez_config();
            print_global_settings(&config);
        }
        GlobalCommand::Edit => cmd_global_edit()?,
        GlobalCommand::Set {
            docker_use_sudo,
            session_enable,
            default_profile,
            clear_default_profile,
            default_profile_direct_launch,
            proxy_url,
            proxy_no_proxy,
            proxy_force_http_transport,
            proxy_clear,
            notify_idle_timeout,
            notify_composer_idle_timeout,
            notify_approval_timeout,
            notify_startup_idle_timeout,
            notify_events,
            notify_user_message_content,
            notify_user_message_preview_chars,
            rate_limit_threshold_warning_mode,
            rate_limit_model_nudge_mode,
        } => {
            if proxy_no_proxy.is_some() && proxy_url.is_none() {
                anyhow::bail!("--proxy-no-proxy requires --proxy-url");
            }
            if proxy_force_http_transport.is_some() && proxy_url.is_none() {
                anyhow::bail!("--proxy-force-http requires --proxy-url");
            }

            if docker_use_sudo.is_none()
                && session_enable.is_none()
                && default_profile.is_none()
                && !clear_default_profile
                && default_profile_direct_launch.is_none()
                && proxy_url.is_none()
                && !proxy_clear
                && notify_idle_timeout.is_none()
                && notify_composer_idle_timeout.is_none()
                && notify_approval_timeout.is_none()
                && notify_startup_idle_timeout.is_none()
                && notify_events.is_none()
                && notify_user_message_content.is_none()
                && notify_user_message_preview_chars.is_none()
                && rate_limit_threshold_warning_mode.is_none()
                && rate_limit_model_nudge_mode.is_none()
            {
                anyhow::bail!(
                    "No changes requested. Provide --docker-use-sudo <BOOL>, --session-enable <BOOL>, --default-profile <PROFILE>, --clear-default-profile, --default-profile-direct-launch <BOOL>, --proxy-url <URL>, --proxy-clear, --notify-idle-timeout <SECS>, --notify-composer-idle-timeout <SECS>, --notify-approval-timeout <SECS>, --notify-startup-idle-timeout <SECS>, --notify-events <CSV>, --notify-user-message-content <MODE>, --notify-user-message-preview-chars <CHARS>, --rate-limit-threshold-warning-mode <MODE>, or --rate-limit-model-nudge-mode <MODE>."
                );
            }

            let mut config = load_codez_config();
            let mut changed = false;

            if let Some(next_docker_use_sudo) = docker_use_sudo {
                if config.docker_use_sudo != next_docker_use_sudo {
                    config.docker_use_sudo = next_docker_use_sudo;
                    changed = true;
                }
            }

            if let Some(next_session_enable) = session_enable {
                if config.session.enabled != next_session_enable {
                    config.session.enabled = next_session_enable;
                    changed = true;
                }
            }

            if clear_default_profile {
                if config.default_profile.take().is_some() {
                    changed = true;
                }
            } else if let Some(target) = default_profile {
                let store = load_store()?;
                let next_default = find_account(&store, &target)?
                    .map(|account| account.name.clone())
                    .ok_or_else(|| anyhow!("Account not found: {target}"))?;
                if config.default_profile.as_deref() != Some(next_default.as_str()) {
                    config.default_profile = Some(next_default);
                    changed = true;
                }
            }

            if let Some(next_direct_launch) = default_profile_direct_launch {
                if config.default_profile_direct_launch != next_direct_launch {
                    config.default_profile_direct_launch = next_direct_launch;
                    changed = true;
                }
            }

            if proxy_clear {
                if config.proxy.is_some() {
                    config.proxy = None;
                    changed = true;
                }
            } else if let Some(url) = proxy_url {
                let next_proxy = proxy_config_from_parts(
                    true,
                    Some(url),
                    proxy_no_proxy,
                    proxy_force_http_transport.unwrap_or(true),
                )?;
                if config.proxy.as_ref() != Some(&next_proxy) {
                    config.proxy = Some(next_proxy);
                    changed = true;
                }
            }

            if let Some(next_timeout) = notify_idle_timeout {
                if config.notify_service_idle_timeout_secs != Some(next_timeout) {
                    config.notify_service_idle_timeout_secs = Some(next_timeout);
                    changed = true;
                }
            }

            if let Some(next_timeout) = notify_composer_idle_timeout {
                if config.notify_service_composer_idle_timeout_secs != Some(next_timeout) {
                    config.notify_service_composer_idle_timeout_secs = Some(next_timeout);
                    changed = true;
                }
            }

            if let Some(next_timeout) = notify_approval_timeout {
                if config.notify_service_approval_timeout_secs != Some(next_timeout) {
                    config.notify_service_approval_timeout_secs = Some(next_timeout);
                    changed = true;
                }
            }

            if let Some(next_timeout) = notify_startup_idle_timeout {
                if config.notify_service_startup_idle_timeout_secs != Some(next_timeout) {
                    config.notify_service_startup_idle_timeout_secs = Some(next_timeout);
                    changed = true;
                }
            }

            if let Some(events) = notify_events {
                let next_events = parse_optional_csv(&events);
                if config.notify_service_events != next_events {
                    config.notify_service_events = next_events;
                    changed = true;
                }
            }

            if let Some(content) = notify_user_message_content {
                let next_content = parse_optional_user_message_content(&content)?;
                if config.notify_service_user_message_content != next_content {
                    config.notify_service_user_message_content = next_content;
                    changed = true;
                }
            }

            if let Some(next_chars) = notify_user_message_preview_chars {
                if config.notify_service_user_message_preview_chars != Some(next_chars) {
                    config.notify_service_user_message_preview_chars = Some(next_chars);
                    changed = true;
                }
            }

            if let Some(next_mode) = rate_limit_threshold_warning_mode {
                let next_mode = parse_optional_rate_limit_mode(&next_mode)?;
                if config.rate_limit_threshold_warning_mode != next_mode {
                    config.rate_limit_threshold_warning_mode = next_mode;
                    changed = true;
                }
            }

            if let Some(next_mode) = rate_limit_model_nudge_mode {
                let next_mode = parse_optional_rate_limit_mode(&next_mode)?;
                if config.rate_limit_model_nudge_mode != next_mode {
                    config.rate_limit_model_nudge_mode = next_mode;
                    changed = true;
                }
            }

            if changed {
                save_codez_config(&config)?;
                println!("{GREEN}Updated{RESET} global settings");
            } else {
                println!(
                    "{YELLOW}No changes{RESET} global settings already match requested values"
                );
            }
            print_global_settings(&config);
        }
    }

    Ok(())
}

fn cmd_proxy(command: ProxyCommand) -> anyhow::Result<()> {
    match command {
        ProxyCommand::Show { profile } => {
            let global_config = load_codez_config();
            if let Some(profile) = profile {
                let store = load_store()?;
                let account = find_account(&store, &profile)?
                    .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
                println!("{BOLD}{CYAN}Proxy for profile{RESET} {}", account.name);
                println!(
                    "{DIM}profile{RESET} {}",
                    proxy_config_label(account.proxy.as_ref())
                );
                println!(
                    "{DIM}global{RESET}  {}",
                    proxy_config_label(global_config.proxy.as_ref())
                );
                println!(
                    "{DIM}effective{RESET} {}",
                    proxy_config_label(effective_proxy_config(account, &global_config))
                );
            } else {
                println!("{BOLD}{CYAN}Global Proxy{RESET}");
                println!("{}", proxy_config_label(global_config.proxy.as_ref()));
            }
        }
        ProxyCommand::Set {
            url,
            no_proxy,
            force_http_transport,
        } => {
            let mut config = load_codez_config();
            config.proxy = Some(proxy_config_from_parts(
                true,
                Some(url),
                no_proxy,
                force_http_transport,
            )?);
            save_codez_config(&config)?;
            println!(
                "{GREEN}Updated{RESET} global proxy: {}",
                proxy_config_label(config.proxy.as_ref())
            );
        }
        ProxyCommand::Clear => {
            let mut config = load_codez_config();
            config.proxy = None;
            save_codez_config(&config)?;
            println!("{YELLOW}Cleared{RESET} global proxy");
        }
        ProxyCommand::SetProfile {
            profile,
            url,
            no_proxy,
            force_http_transport,
        } => {
            let mut store = load_store()?;
            let account = store
                .accounts
                .iter_mut()
                .find(|account| account.name == profile || account.id == profile)
                .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
            account.proxy = Some(proxy_config_from_parts(
                true,
                Some(url),
                no_proxy,
                force_http_transport,
            )?);
            let name = account.name.clone();
            let label = proxy_config_label(account.proxy.as_ref());
            save_store(&store)?;
            println!("{GREEN}Updated{RESET} proxy for {BOLD}{name}{RESET}: {label}");
        }
        ProxyCommand::DisableProfile { profile } => {
            let mut store = load_store()?;
            let account = store
                .accounts
                .iter_mut()
                .find(|account| account.name == profile || account.id == profile)
                .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
            account.proxy = Some(proxy_config_from_parts(
                false, None, None, /*force_http_transport*/ true,
            )?);
            let name = account.name.clone();
            save_store(&store)?;
            println!("{YELLOW}Disabled{RESET} proxy for {BOLD}{name}{RESET}");
        }
        ProxyCommand::ClearProfile { profile } => {
            let mut store = load_store()?;
            let account = store
                .accounts
                .iter_mut()
                .find(|account| account.name == profile || account.id == profile)
                .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
            account.proxy = None;
            let name = account.name.clone();
            save_store(&store)?;
            println!("{YELLOW}Cleared{RESET} proxy override for {BOLD}{name}{RESET}");
        }
    }

    Ok(())
}

fn cmd_notify(command: NotifyCommand) -> anyhow::Result<()> {
    match command {
        NotifyCommand::Desktop { command } => cmd_notify_desktop(command),
    }
}

fn cmd_notify_desktop(command: DesktopNotifyCommand) -> anyhow::Result<()> {
    match command {
        DesktopNotifyCommand::Enable { port } => {
            let config = ensure_desktop_notify_config(true, port)?;
            ensure_desktop_notify_bridge_running(&config)?;
            println!(
                "{GREEN}Enabled{RESET} desktop notifications on {}",
                desktop_notify_bridge_url(desktop_notify_port(&config))
            );
            Ok(())
        }
        DesktopNotifyCommand::Disable => {
            let mut config = load_codez_config();
            config.desktop_notify_enabled = false;
            save_codez_config(&config)?;
            println!("{YELLOW}Disabled{RESET} desktop notifications.");
            Ok(())
        }
        DesktopNotifyCommand::Start { port } => {
            let config =
                ensure_desktop_notify_config(load_codez_config().desktop_notify_enabled, port)?;
            ensure_desktop_notify_bridge_running(&config)?;
            println!(
                "{GREEN}Running{RESET} desktop notification bridge on {}",
                desktop_notify_bridge_url(desktop_notify_port(&config))
            );
            Ok(())
        }
        DesktopNotifyCommand::Status => {
            let config = load_codez_config();
            let port = desktop_notify_port(&config);
            let healthy =
                desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref());
            println!("{BOLD}{CYAN}Desktop Notify Bridge{RESET}");
            println!(
                "{DIM}enabled{RESET} {}",
                bool_label(config.desktop_notify_enabled)
            );
            println!("{DIM}port{RESET} {port}");
            println!(
                "{DIM}token{RESET} {}",
                if config
                    .desktop_notify_token
                    .as_ref()
                    .is_some_and(|token| !token.is_empty())
                {
                    "(set)"
                } else {
                    "-"
                }
            );
            println!(
                "{DIM}health{RESET} {}",
                if healthy { "healthy" } else { "not running" }
            );
            println!(
                "{DIM}external_forward{RESET} {}",
                if config
                    .notify_service_url
                    .as_ref()
                    .is_some_and(|url| !url.is_empty())
                {
                    "configured"
                } else {
                    "-"
                }
            );
            Ok(())
        }
        DesktopNotifyCommand::Serve { port, token } => {
            let mut config = load_codez_config();
            if let Some(port) = port {
                config.desktop_notify_port = Some(port);
            }
            if let Some(token) = token {
                config.desktop_notify_token = Some(token);
            }
            run_desktop_notify_bridge(config)
        }
        DesktopNotifyCommand::Test { message } => {
            let message = message.unwrap_or_else(|| "cutex desktop notification test".to_string());
            send_native_desktop_notification("cutex desktop notify", &message)?;
            println!("{GREEN}Sent{RESET} test desktop notification.");
            Ok(())
        }
        DesktopNotifyCommand::InstallUbuntu { port } => install_ubuntu_desktop_notify_service(port),
        DesktopNotifyCommand::UninstallUbuntu => uninstall_ubuntu_desktop_notify_service(),
    }
}

fn ensure_desktop_notify_bridge_for_launch(account: &StoredAccount) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex {
        return Ok(());
    }
    let config = load_codez_config();
    if !config.desktop_notify_enabled {
        return Ok(());
    }
    let config = ensure_desktop_notify_config(true, None)?;
    ensure_desktop_notify_bridge_running(&config)
}

fn ensure_desktop_notify_config(enabled: bool, port: Option<u16>) -> anyhow::Result<CodezConfig> {
    let mut config = load_codez_config();
    config.desktop_notify_enabled = enabled;
    if let Some(port) = port {
        validate_desktop_notify_port(port)?;
        config.desktop_notify_port = Some(port);
    } else if config.desktop_notify_port.is_none() {
        config.desktop_notify_port = Some(DEFAULT_DESKTOP_NOTIFY_PORT);
    }
    if config
        .desktop_notify_token
        .as_ref()
        .is_none_or(|token| token.trim().is_empty())
    {
        config.desktop_notify_token = Some(format!("cutex-{}", Uuid::new_v4()));
    }
    save_codez_config(&config)?;
    Ok(config)
}

fn validate_desktop_notify_port(port: u16) -> anyhow::Result<()> {
    if !(24000..=24999).contains(&port) {
        anyhow::bail!("Desktop notify bridge port must be in the Bridgeboard 24xxx range");
    }
    Ok(())
}

fn desktop_notify_port(config: &CodezConfig) -> u16 {
    config
        .desktop_notify_port
        .unwrap_or(DEFAULT_DESKTOP_NOTIFY_PORT)
}

fn desktop_notify_bridge_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/api/agent-notify/push")
}

fn desktop_notify_health_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

fn ubuntu_desktop_notify_service_path() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home
        .join(".config")
        .join("systemd")
        .join("user")
        .join("cutex-desktop-notify.service"))
}

fn install_ubuntu_desktop_notify_service(port: Option<u16>) -> anyhow::Result<()> {
    let config = ensure_desktop_notify_config(true, port)?;
    let port = desktop_notify_port(&config);
    validate_desktop_notify_port(port)?;
    let service_path = ubuntu_desktop_notify_service_path()?;
    let service_dir = service_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid systemd service path"))?;
    fs::create_dir_all(service_dir)
        .with_context(|| format!("Failed to create {}", service_dir.display()))?;
    let exe = std::env::current_exe().context("Failed to resolve current cutex executable")?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow!("Current cutex path is not valid UTF-8"))?;
    let service = format!(
        r#"[Unit]
Description=cutex desktop notification bridge
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart={exe} notify desktop serve --port {port}
Restart=on-failure
RestartSec=2
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus
Environment=PATH=/home/%u/.local/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
"#
    );
    fs::write(&service_path, service)
        .with_context(|| format!("Failed to write {}", service_path.display()))?;

    run_systemctl_user(&["daemon-reload"])?;
    run_systemctl_user(&["enable", "--now", "cutex-desktop-notify.service"])?;

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
            register_desktop_notify_handoff(port);
            println!(
                "{GREEN}Installed{RESET} Ubuntu desktop notification service on {}",
                desktop_notify_bridge_url(port)
            );
            return Ok(());
        }
    }

    anyhow::bail!(
        "Installed service, but bridge did not become healthy on port {port}. Check `systemctl --user status cutex-desktop-notify.service`."
    )
}

fn uninstall_ubuntu_desktop_notify_service() -> anyhow::Result<()> {
    let _ = run_systemctl_user(&["disable", "--now", "cutex-desktop-notify.service"]);
    let service_path = ubuntu_desktop_notify_service_path()?;
    if service_path.exists() {
        fs::remove_file(&service_path)
            .with_context(|| format!("Failed to remove {}", service_path.display()))?;
    }
    let _ = run_systemctl_user(&["daemon-reload"]);
    let mut config = load_codez_config();
    config.desktop_notify_enabled = false;
    save_codez_config(&config)?;
    println!("{YELLOW}Uninstalled{RESET} Ubuntu desktop notification service.");
    Ok(())
}

fn run_systemctl_user(args: &[&str]) -> anyhow::Result<()> {
    if !command_exists_in_path("systemctl") {
        anyhow::bail!("systemctl is not available in PATH");
    }
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .with_context(|| format!("Failed to run systemctl --user {}", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("systemctl --user {} exited with {status}", args.join(" "));
    }
    Ok(())
}

fn ensure_desktop_notify_bridge_running(config: &CodezConfig) -> anyhow::Result<()> {
    let port = desktop_notify_port(config);
    validate_desktop_notify_port(port)?;
    if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
        register_desktop_notify_handoff(port);
        return Ok(());
    }

    let exe = std::env::current_exe().context("Failed to resolve current cutex executable")?;
    let log_dir = runtime_dir()?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create runtime dir: {}", log_dir.display()))?;
    let log_path = log_dir.join("desktop-notify-bridge.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .context("Failed to clone bridge log file")?;

    let mut child = Command::new(exe);
    child
        .arg("notify")
        .arg("desktop")
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(token) = config.desktop_notify_token.as_ref() {
        child.arg("--token").arg(token);
    }
    #[cfg(unix)]
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    child
        .spawn()
        .with_context(|| format!("Failed to start desktop notify bridge on port {port}"))?;

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
            register_desktop_notify_handoff(port);
            return Ok(());
        }
    }

    anyhow::bail!(
        "Desktop notify bridge did not become healthy on port {port}. See {}",
        log_path.display()
    )
}

fn desktop_notify_bridge_healthy(port: u16, token: Option<&str>) -> bool {
    let Ok(mut stream) = connect_local_port(port, Duration::from_millis(250)) else {
        return false;
    };
    let auth = token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request =
        format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Length: 0\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0_u8; 128];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"),
        Err(_) => false,
    }
}

fn connect_local_port(port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, timeout)
}

fn register_desktop_notify_handoff(port: u16) {
    if !command_exists_in_path("bridgeboard") {
        return;
    }
    let owner_host = std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            Command::new("hostname")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        });
    let health_url = desktop_notify_health_url(port);
    let _ = Command::new("bridgeboard")
        .arg("handoff")
        .arg("--id")
        .arg(DESKTOP_NOTIFY_BRIDGE_ID)
        .arg("--title")
        .arg("cutex desktop notification bridge")
        .arg("--port")
        .arg(port.to_string())
        .arg("--owner-host")
        .arg(owner_host)
        .arg("--pid-from-port")
        .arg("--health-url")
        .arg(health_url)
        .arg("--require-healthy")
        .status();
}

fn run_desktop_notify_bridge(config: CodezConfig) -> anyhow::Result<()> {
    let port = desktop_notify_port(&config);
    validate_desktop_notify_port(port)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("Failed to bind desktop notify bridge on 127.0.0.1:{port}"))?;
    println!(
        "cutex desktop notify bridge listening on {}",
        desktop_notify_health_url(port)
    );
    register_desktop_notify_handoff(port);

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(err) = handle_desktop_notify_request(&mut stream, &config) {
                    let _ = write_http_response(
                        &mut stream,
                        500,
                        "Internal Server Error",
                        "text/plain",
                        format!("{err:#}").as_bytes(),
                    );
                }
            }
            Err(err) => eprintln!("{YELLOW}warning:{RESET} desktop notify accept failed: {err}"),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct SimpleHttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn handle_desktop_notify_request(
    stream: &mut TcpStream,
    config: &CodezConfig,
) -> anyhow::Result<()> {
    let request = read_simple_http_request(stream)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_http_response(stream, 200, "OK", "text/plain", b"ok"),
        ("POST", "/api/agent-notify/push") => {
            require_bridge_token(&request, config.desktop_notify_token.as_deref())?;
            let live_config = load_codez_config();
            if live_config.desktop_notify_enabled {
                handle_native_desktop_notify(&request.body)?;
            }
            forward_to_external_notify_service(&live_config, &request.body);
            write_http_response(stream, 200, "OK", "text/plain", b"ok")
        }
        _ => write_http_response(stream, 404, "Not Found", "text/plain", b"not found"),
    }
}

fn read_simple_http_request(stream: &mut TcpStream) -> anyhow::Result<SimpleHttpRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let n = stream
            .read(&mut chunk)
            .context("Failed to read HTTP request")?;
        if n == 0 {
            anyhow::bail!("Connection closed before HTTP headers");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 1024 * 1024 {
            anyhow::bail!("HTTP request headers are too large");
        }
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let headers_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = headers_text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("Missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("Missing HTTP method"))?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| anyhow!("Missing HTTP path"))?
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > 1024 * 1024 {
        anyhow::bail!("HTTP request body is too large");
    }
    while buf.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let n = stream
            .read(&mut chunk)
            .context("Failed to read HTTP body")?;
        if n == 0 {
            anyhow::bail!("Connection closed before HTTP body");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = buf[header_end..header_end + content_length].to_vec();
    Ok(SimpleHttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn require_bridge_token(request: &SimpleHttpRequest, token: Option<&str>) -> anyhow::Result<()> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return Ok(());
    };
    let expected = format!("Bearer {token}");
    let actual = request
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    if actual != expected {
        anyhow::bail!("Unauthorized desktop notify request");
    }
    Ok(())
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .context("Failed to write HTTP response headers")?;
    stream
        .write_all(body)
        .context("Failed to write HTTP response body")?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

fn handle_native_desktop_notify(body: &[u8]) -> anyhow::Result<()> {
    let payload: Value = serde_json::from_slice(body).context("Failed to parse notify JSON")?;
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("notification");
    let project = payload
        .get("project_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let agent = payload
        .get("agent_name")
        .and_then(Value::as_str)
        .unwrap_or("codex");
    let duration = payload
        .get("duration_seconds")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}s"))
        .unwrap_or_else(|| "-".to_string());
    let idle = payload
        .get("idle_seconds")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}s"))
        .unwrap_or_else(|| "-".to_string());
    let title = format!("{agent}: {status}");
    let body = format!("{project} · duration {duration} · idle {idle}");
    send_native_desktop_notification(&title, &body)
}

fn send_native_desktop_notification(title: &str, body: &str) -> anyhow::Result<()> {
    if !command_exists_in_path("notify-send") {
        anyhow::bail!("notify-send is not available in PATH");
    }
    let status = Command::new("notify-send")
        .arg("-a")
        .arg("cutex")
        .arg("-u")
        .arg("normal")
        .arg(title)
        .arg(body)
        .status()
        .context("Failed to run notify-send")?;
    if !status.success() {
        anyhow::bail!("notify-send exited with status {status}");
    }
    Ok(())
}

fn forward_to_external_notify_service(config: &CodezConfig, body: &[u8]) {
    let Some(url) = config
        .notify_service_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
    else {
        return;
    };
    if is_desktop_bridge_url(config, url) {
        return;
    }
    if let Err(err) = post_http_json(url, config.notify_service_token.as_deref(), body) {
        eprintln!("{YELLOW}warning:{RESET} external notify forward failed: {err:#}");
    }
}

fn is_desktop_bridge_url(config: &CodezConfig, url: &str) -> bool {
    url == desktop_notify_bridge_url(desktop_notify_port(config))
}

fn post_http_json(url: &str, token: Option<&str>, body: &[u8]) -> anyhow::Result<()> {
    let parsed = Url::parse(url).with_context(|| format!("Invalid notify URL: {url}"))?;
    if parsed.scheme() != "http" {
        anyhow::bail!("Only http:// notify forwarding is supported by cutex desktop bridge");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("Notify URL has no host: {url}"))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addr = format!("{host}:{port}");
    let mut stream =
        TcpStream::connect(addr).context("Failed to connect external notify service")?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut path = parsed.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = parsed.query() {
        path.push('?');
        path.push_str(query);
    }
    let auth = token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(body)?;
    let mut response = [0_u8; 64];
    let n = stream.read(&mut response).unwrap_or(0);
    if n > 0 && !String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 2") {
        anyhow::bail!("External notify service returned non-success");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CuteAldenSession {
    pid: u32,
    name: Option<String>,
}

fn cmd_session(command: SessionCommand) -> anyhow::Result<()> {
    match command {
        SessionCommand::List => cmd_session_list(),
        SessionCommand::Attach { name } => cmd_session_attach(&name),
    }
}

fn cmd_session_list() -> anyhow::Result<()> {
    for session in cute_alden_sessions()? {
        println!(
            "{}\t{}",
            session.pid,
            session.name.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn cmd_session_attach(name: &str) -> anyhow::Result<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Session name cannot be empty");
    }

    let program = cute_alden_program()?;
    let exit_code = exit_code_from_status(
        Command::new(&program)
            .arg("--attach")
            .arg(trimmed)
            .status()
            .with_context(|| format!("Failed to start {program} --attach {trimmed}"))?,
    );

    std::process::exit(exit_code);
}

fn cute_alden_sessions() -> anyhow::Result<Vec<CuteAldenSession>> {
    let program = cute_alden_program()?;
    let output = Command::new(&program)
        .arg("--list")
        .output()
        .with_context(|| format!("Failed to start {program} --list"))?;
    if !output.status.success() {
        anyhow::bail!("{program} --list exited with status {}", output.status);
    }

    let stdout =
        String::from_utf8(output.stdout).context("cute-alden --list returned invalid UTF-8")?;
    let mut sessions = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let (pid_text, name_text) = line
            .split_once('\t')
            .ok_or_else(|| anyhow!("Unexpected cute-alden --list output line: {line}"))?;
        let pid = pid_text
            .trim()
            .parse::<u32>()
            .with_context(|| format!("Invalid cute-alden session pid: {pid_text}"))?;
        let name = match name_text.trim() {
            "" | "-" => None,
            value => Some(value.to_string()),
        };
        sessions.push(CuteAldenSession { pid, name });
    }

    Ok(sessions)
}

fn cute_alden_program() -> anyhow::Result<String> {
    if let Some(program) = env_var_first(&[CUTEX_ALDEN_BIN_ENV_VAR])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Ok(program);
    }

    if command_exists_in_path("cute-alden") {
        return Ok("cute-alden".to_string());
    }

    let repo_candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|path| {
            path.join("cute-alden")
                .join("cute-alden-0.2")
                .join("cute-alden")
        })
        .filter(|path| path.is_file());
    if let Some(repo_candidate) = repo_candidate {
        return Ok(repo_candidate.to_string_lossy().to_string());
    }

    anyhow::bail!(
        "cute-alden binary not found. Set {CUTEX_ALDEN_BIN_ENV_VAR} or put `cute-alden` on PATH."
    );
}

fn should_wrap_launch_with_session(account: &StoredAccount, codex_args: &[String]) -> bool {
    if already_inside_cute_alden_session() {
        return false;
    }

    if !codex_args.is_empty() {
        return false;
    }

    let global_config = load_codez_config();
    effective_session_config(account, &global_config).enabled
}

fn already_inside_cute_alden_session() -> bool {
    env_bool_override("CUTE_ALDEN_SESSION_ACTIVE").unwrap_or(false)
        || env_bool_override("ALDEN_SESSION_ACTIVE").unwrap_or(false)
}

fn maybe_wrap_launch_with_session(
    account: &StoredAccount,
    codex_args: &[String],
    launch: LaunchCommand,
) -> anyhow::Result<LaunchCommand> {
    if !should_wrap_launch_with_session(account, codex_args) {
        return Ok(launch);
    }

    let session_name = default_managed_session_name(account)?;
    let alden_program = cute_alden_program()?;
    println!("Session: managed via {BOLD}{}{RESET}", session_name);
    Ok(wrap_launch_with_cute_alden(
        launch,
        &alden_program,
        &session_name,
    ))
}

fn wrap_launch_with_cute_alden(
    launch: LaunchCommand,
    alden_program: &str,
    session_name: &str,
) -> LaunchCommand {
    let LaunchCommand {
        program,
        args,
        envs,
    } = launch;

    let mut wrapped = LaunchCommand::new(alden_program);
    for (key, value) in envs {
        wrapped = wrapped.env(key, value);
    }

    wrapped
        .arg("--name")
        .arg(session_name)
        .arg("--")
        .arg(program)
        .args(args)
}

fn default_managed_session_name(account: &StoredAccount) -> anyhow::Result<String> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let profile = sanitize_session_component(&account.name, 24, "profile");
    let runtime = sanitize_session_component(runtime_label(&account.runtime), 12, "runtime");
    let project = sanitize_session_component(
        cwd.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("root"),
        24,
        "project",
    );
    let hash = fnv1a_hex(format!(
        "{}\0{}\0{}",
        account.id,
        runtime_label(&account.runtime),
        cwd.display()
    ));
    Ok(format!(
        "cutex.{profile}.{runtime}.{project}.{}",
        &hash[..10]
    ))
}

fn sanitize_session_component(input: &str, max_len: usize, fallback: &str) -> String {
    let mut sanitized = String::with_capacity(max_len);
    let mut last_dash = false;

    for ch in input.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else if matches!(ch, '.' | '-' | '_') {
            ch
        } else {
            '-'
        };

        if next == '-' && last_dash {
            continue;
        }

        sanitized.push(next);
        last_dash = next == '-';

        if sanitized.len() >= max_len {
            break;
        }
    }

    let trimmed = sanitized.trim_matches(|ch: char| matches!(ch, '.' | '-' | '_'));
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn fnv1a_hex(input: String) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.into_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn add_account_to_store(store: &mut AccountsStore, account: StoredAccount) -> anyhow::Result<()> {
    let source = account.source.as_deref().unwrap_or("unknown");
    let plan = account.plan_type.as_deref().unwrap_or("unknown");
    let email = account.email.as_deref().unwrap_or("-");
    let runtime = runtime_label(&account.runtime);

    store.accounts.push(account.clone());
    if store.active_account_id.is_none() {
        store.active_account_id = Some(account.id.clone());
    }
    save_store(store)?;

    println!(
        "{GREEN}Added{RESET} profile `{}` ({}, {}, {}, {})",
        account.name, source, plan, runtime, email
    );
    Ok(())
}

fn cmd_run(
    profile: &str,
    codex_args: Vec<String>,
    force_host: bool,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> anyhow::Result<()> {
    let account = activate_account(profile)?;
    let effective_account =
        apply_runtime_override(&account, force_host, docker_image, docker_user_name)?;
    println!(
        "{GREEN}Switched{RESET} active profile to {BOLD}{}{RESET}",
        account.name
    );
    if effective_account.runtime != account.runtime {
        println!(
            "One-off runtime override: {}",
            runtime_description(&effective_account.runtime)
        );
    }
    run_codex_process(&effective_account, codex_args)?;
    Ok(())
}

fn cmd_quick_run(codex_args: Vec<String>, quick: bool, force_host: bool) -> anyhow::Result<()> {
    let store = load_store()?;
    if store.accounts.is_empty() {
        println!(
            "No accounts configured. Use `cutex add --from-auth <path> --name <name>` to add one."
        );
        return Ok(());
    }

    let mut state = load_quick_state();
    let global_config = load_codez_config();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(|text| text.to_string()));

    let default_name = determine_default_profile(&store, &state, &global_config, cwd.as_deref());
    let use_default_without_prompt = quick || global_config.default_profile_direct_launch;

    let chosen = if use_default_without_prompt {
        println!("Using default profile: {BOLD}{}{RESET}", default_name);
        default_name.clone()
    } else {
        let selected = prompt_for_profile(&store, &default_name)?;
        println!("Using profile: {BOLD}{}{RESET}", selected);
        selected
    };

    if let Some(dir) = cwd {
        state.per_directory.insert(dir, chosen.clone());
    }
    state.last_global_profile = Some(chosen.clone());
    let _ = save_quick_state(&state);
    let program = codex_program();
    let chosen_account = store
        .accounts
        .iter()
        .find(|account| account.name == chosen)
        .ok_or_else(|| anyhow!("Profile disappeared after selection: {chosen}"))?;
    let preview_args = combined_profile_cli_args(chosen_account, codex_args.clone());

    if !preview_args.is_empty() {
        let args_preview = cli_args_label(&preview_args);
        if use_default_without_prompt {
            println!(
                "Running profile '{}' with: {} {}",
                chosen, program, args_preview
            );
        } else {
            println!(
                "Will run profile '{}' with: {} {}",
                chosen, program, args_preview
            );
            print!("Proceed? [Y/n] ");
            io::stdout().flush()?;

            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let answer = line.trim();
            if !(answer.is_empty() || matches!(answer, "y" | "Y")) {
                println!("Aborted.");
                return Ok(());
            }
        }
    } else {
        println!("Running profile '{}' with: {}", chosen, program);
    }

    cmd_run(&chosen, codex_args, force_host, None, None)
}

fn determine_default_profile(
    store: &AccountsStore,
    state: &QuickRunState,
    global_config: &CodezConfig,
    cwd: Option<&str>,
) -> String {
    if let Some(dir) = cwd {
        if let Some(name) = state.per_directory.get(dir) {
            if store.accounts.iter().any(|account| account.name == *name) {
                return name.clone();
            }
        }
    }

    if let Some(name) = &global_config.default_profile {
        if store.accounts.iter().any(|account| account.name == *name) {
            return name.clone();
        }
    }

    if let Some(name) = &state.last_global_profile {
        if store.accounts.iter().any(|account| account.name == *name) {
            return name.clone();
        }
    }

    store
        .accounts
        .first()
        .map(|account| account.name.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn prompt_for_profile(store: &AccountsStore, default_name: &str) -> anyhow::Result<String> {
    println!("{BOLD}{CYAN}Choose a profile{RESET}");
    for (idx, acc) in store.accounts.iter().enumerate() {
        let is_active = Some(&acc.id) == store.active_account_id.as_ref();
        let is_default = acc.name == default_name;
        let marker = if is_active {
            format!("{GREEN}●{RESET}")
        } else if is_default {
            format!("{YELLOW}◆{RESET}")
        } else {
            format!("{DIM}○{RESET}")
        };
        let badges = match (is_active, is_default) {
            (true, true) => format!("{GREEN}active{RESET} {YELLOW}default{RESET}"),
            (true, false) => format!("{GREEN}active{RESET}"),
            (false, true) => format!("{YELLOW}default{RESET}"),
            (false, false) => String::new(),
        };
        let runtime = runtime_label(&acc.runtime);
        println!(
            "  {} {BOLD}[{}]{RESET} {CYAN}{}{RESET}  {DIM}{}{RESET}  {YELLOW}{}{RESET}  {}",
            marker,
            idx + 1,
            acc.name,
            acc.source.as_deref().unwrap_or("unknown"),
            runtime,
            badges
        );
    }

    print!("Profile to use [{default_name}]: ");
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let input = line.trim();

    if input.is_empty() {
        return Ok(default_name.to_string());
    }

    if let Some(acc) = store
        .accounts
        .iter()
        .find(|account| account.name == input || account.id == input)
    {
        return Ok(acc.name.clone());
    }

    if let Ok(idx) = input.parse::<usize>() {
        if idx >= 1 && idx <= store.accounts.len() {
            return Ok(store.accounts[idx - 1].name.clone());
        }
    }

    anyhow::bail!("Unknown profile: {input}")
}

fn import_snapshot(auth_path: &str, config_path: Option<&str>) -> anyhow::Result<ImportedSnapshot> {
    let raw_auth_json = fs::read_to_string(auth_path)
        .with_context(|| format!("Failed to read auth.json: {auth_path}"))?;

    let raw_config_toml = match config_path {
        Some(path) => Some(
            fs::read_to_string(path)
                .with_context(|| format!("Failed to read config.toml: {path}"))?,
        ),
        None => infer_config_toml(auth_path)?,
    };
    let raw_config_toml = raw_config_toml
        .map(|text| extract_profile_config_toml(&text))
        .transpose()?
        .flatten();

    let (email, plan_type) = parse_auth_metadata(&raw_auth_json);
    let source = detect_source_label(Some(&raw_auth_json), raw_config_toml.as_deref());

    Ok(ImportedSnapshot {
        raw_auth_json,
        raw_config_toml,
        email,
        plan_type,
        source,
    })
}

fn infer_config_toml(auth_path: &str) -> anyhow::Result<Option<String>> {
    let auth_path = Path::new(auth_path);
    let config_path = auth_path
        .parent()
        .map(|parent| parent.join("config.toml"))
        .ok_or_else(|| anyhow!("Failed to determine auth.json parent directory"))?;

    read_optional_text(&config_path)
}

fn parse_auth_metadata(raw_auth_json: &str) -> (Option<String>, Option<String>) {
    let json: Value = match serde_json::from_str(raw_auth_json) {
        Ok(value) => value,
        Err(_) => return (None, None),
    };

    if let Some(id_token) = json
        .get("tokens")
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(|value| value.as_str())
    {
        return parse_id_token_claims(id_token);
    }

    (None, None)
}

fn parse_id_token_claims(id_token: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return (None, None);
    }

    let payload = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(bytes) => bytes,
        Err(_) => return (None, None),
    };

    let json: Value = match serde_json::from_slice(&payload) {
        Ok(value) => value,
        Err(_) => return (None, None),
    };

    let email = json
        .get("email")
        .and_then(|value| value.as_str())
        .map(String::from);
    let plan_type = json
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_plan_type"))
        .and_then(|value| value.as_str())
        .map(String::from);

    (email, plan_type)
}

fn detect_source_label(raw_auth_json: Option<&str>, raw_config_toml: Option<&str>) -> String {
    if let Some(config) = raw_config_toml {
        let lower = config.to_ascii_lowercase();
        if lower.contains("base_url")
            || lower.contains("model_provider")
            || lower.contains("[model_providers.")
        {
            return "third-party".to_string();
        }
        if lower.contains("cli_auth_credentials_store") {
            return "official".to_string();
        }
    }

    if let Some(auth) = raw_auth_json {
        if let Ok(json) = serde_json::from_str::<Value>(auth) {
            if json
                .get("tokens")
                .and_then(|value| value.as_object())
                .is_some()
            {
                return "official".to_string();
            }
            if json.get("OPENAI_API_KEY").is_some() || json.get("openai_api_key").is_some() {
                return "api-key".to_string();
            }
        }
    }

    "custom".to_string()
}

fn find_account<'a>(
    store: &'a AccountsStore,
    target: &str,
) -> anyhow::Result<Option<&'a StoredAccount>> {
    Ok(store
        .accounts
        .iter()
        .find(|account| account.name == target || account.id == target))
}

fn resolve_configured_default_profile_name(
    store: &AccountsStore,
    target: Option<String>,
) -> anyhow::Result<Option<String>> {
    let Some(target) = target else {
        return Ok(None);
    };
    let target = target.trim();
    if target.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        find_account(store, target)?
            .map(|account| account.name.clone())
            .ok_or_else(|| anyhow!("Account not found: {target}"))?,
    ))
}

fn ensure_unique_name(
    store: &AccountsStore,
    name: &str,
    ignore_account_id: Option<&str>,
) -> anyhow::Result<()> {
    if store.accounts.iter().any(|account| {
        account.name == name && ignore_account_id.map(|id| id != account.id).unwrap_or(true)
    }) {
        anyhow::bail!("An account with name '{}' already exists", name);
    }
    Ok(())
}

fn proxy_config_from_parts(
    enabled: bool,
    url: Option<String>,
    no_proxy: Option<String>,
    force_http_transport: bool,
) -> anyhow::Result<ProxyConfig> {
    let url = url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let no_proxy = no_proxy
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if enabled {
        let url =
            url.ok_or_else(|| anyhow!("Proxy URL must not be empty when proxy is enabled"))?;
        let scheme = url
            .split("://")
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let supported = matches!(
            scheme.as_str(),
            "http" | "https" | "socks5" | "socks5h" | "socks4" | "socks4a"
        );
        if !supported {
            anyhow::bail!(
                "Unsupported proxy scheme `{scheme}`. Expected http, https, socks5, socks5h, socks4, or socks4a"
            );
        }
        Ok(ProxyConfig {
            enabled: true,
            url: Some(url),
            no_proxy,
            force_http_transport,
        })
    } else {
        Ok(ProxyConfig {
            enabled: false,
            url: None,
            no_proxy: None,
            force_http_transport,
        })
    }
}

fn effective_proxy_config<'a>(
    account: &'a StoredAccount,
    global_config: &'a CodezConfig,
) -> Option<&'a ProxyConfig> {
    account.proxy.as_ref().or(global_config.proxy.as_ref())
}

fn proxy_config_label(proxy: Option<&ProxyConfig>) -> String {
    match proxy {
        Some(proxy) if !proxy.enabled => "disabled".to_string(),
        Some(proxy) => {
            let url = proxy.url.as_deref().unwrap_or("<missing-url>");
            let no_proxy = proxy.no_proxy.as_deref().unwrap_or("-");
            format!(
                "enabled url={url} no_proxy={no_proxy} force_http={}",
                bool_label(proxy.force_http_transport)
            )
        }
        None => "inherit/none".to_string(),
    }
}

fn proxy_envs(
    proxy: Option<&ProxyConfig>,
    runtime: Option<&RuntimeConfig>,
) -> Vec<(String, String)> {
    let Some(proxy) = proxy else {
        return Vec::new();
    };
    if !proxy.enabled {
        return vec![
            ("HTTP_PROXY".to_string(), String::new()),
            ("HTTPS_PROXY".to_string(), String::new()),
            ("ALL_PROXY".to_string(), String::new()),
            ("NO_PROXY".to_string(), String::new()),
            ("http_proxy".to_string(), String::new()),
            ("https_proxy".to_string(), String::new()),
            ("all_proxy".to_string(), String::new()),
            ("no_proxy".to_string(), String::new()),
            (
                CUTE_CODEX_FORCE_HTTP_TRANSPORT_ENV_VAR.to_string(),
                "0".to_string(),
            ),
        ];
    };

    let configured_url = proxy.url.clone().unwrap_or_default();
    let url = match runtime {
        Some(RuntimeConfig::Docker { .. }) => {
            rewrite_docker_loopback_proxy_url(&configured_url).unwrap_or(configured_url)
        }
        _ => configured_url,
    };
    let http_proxy_value = url.clone();
    let mut envs = vec![
        ("HTTP_PROXY".to_string(), http_proxy_value.clone()),
        ("HTTPS_PROXY".to_string(), http_proxy_value.clone()),
        ("ALL_PROXY".to_string(), url.clone()),
        ("http_proxy".to_string(), http_proxy_value.clone()),
        ("https_proxy".to_string(), http_proxy_value),
        ("all_proxy".to_string(), url),
        (
            CUTE_CODEX_FORCE_HTTP_TRANSPORT_ENV_VAR.to_string(),
            if proxy.force_http_transport {
                "1".to_string()
            } else {
                "0".to_string()
            },
        ),
    ];

    let no_proxy = proxy.no_proxy.clone().unwrap_or_default();
    envs.push(("NO_PROXY".to_string(), no_proxy.clone()));
    envs.push(("no_proxy".to_string(), no_proxy));

    envs
}

fn host_is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn rewrite_docker_loopback_proxy_url(url: &str) -> Option<String> {
    if url.trim().is_empty() {
        return None;
    }
    let mut parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    if !host_is_loopback(host) {
        return None;
    }
    parsed.set_host(Some(DOCKER_PROXY_HOST_ALIAS)).ok()?;
    Some(parsed.to_string())
}

fn rename_profile_references(state: &mut QuickRunState, old_name: &str, new_name: &str) {
    if state.last_global_profile.as_deref() == Some(old_name) {
        state.last_global_profile = Some(new_name.to_string());
    }

    for value in state.per_directory.values_mut() {
        if value == old_name {
            *value = new_name.to_string();
        }
    }
}

fn rename_global_profile_references(
    config: &mut CodezConfig,
    old_name: &str,
    new_name: &str,
) -> bool {
    if config.default_profile.as_deref() == Some(old_name) {
        config.default_profile = Some(new_name.to_string());
        true
    } else {
        false
    }
}

fn remove_profile_references(state: &mut QuickRunState, removed_name: &str) {
    if state.last_global_profile.as_deref() == Some(removed_name) {
        state.last_global_profile = None;
    }

    state.per_directory.retain(|_, value| value != removed_name);
}

fn remove_global_profile_references(config: &mut CodezConfig, removed_name: &str) -> bool {
    if config.default_profile.as_deref() == Some(removed_name) {
        config.default_profile = None;
        true
    } else {
        false
    }
}

fn read_optional_text(path: &Path) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let data = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;
    Ok(Some(data))
}

fn write_optional_text(path: &Path, contents: Option<&str>) -> anyhow::Result<()> {
    match contents {
        Some(text) => {
            fs::write(path, text)
                .with_context(|| format!("Failed to write file: {}", path.display()))?;
        }
        None => {
            if path.exists() {
                fs::remove_file(path)
                    .with_context(|| format!("Failed to remove file: {}", path.display()))?;
            }
        }
    }

    Ok(())
}

const PROFILE_CONFIG_SCALAR_KEYS: [&str; 4] = [
    "cli_auth_credentials_store",
    "model_provider",
    "model_context_window",
    "model_auto_compact_token_limit",
];
const PROFILE_CONFIG_TABLE_KEYS: [&str; 1] = ["shell_environment_policy"];
const DEFAULT_CUTEX_STATUS_LINE: [&str; 6] = [
    "custom:bon-voyage",
    "custom:profile",
    "model-with-reasoning",
    "current-dir",
    "context-used",
    "weekly-limit",
];
const PROFILE_CONFIG_TUI_KEYS: [&str; 3] = [
    "status_line",
    "status_line_use_colors",
    "session_picker_provider_filter",
];
const TOOL_PROXY_ENV_EXCLUDE_PATTERNS: [&str; 13] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "PIP_PROXY",
    "NPM_CONFIG_PROXY",
    "NPM_CONFIG_HTTP_PROXY",
    "NPM_CONFIG_HTTPS_PROXY",
    "YARN_PROXY",
    "YARN_HTTP_PROXY",
    "YARN_HTTPS_PROXY",
    "BUNDLE_HTTP_PROXY",
    "BUNDLE_HTTPS_PROXY",
];

fn extract_profile_config_toml(config_toml: &str) -> anyhow::Result<Option<String>> {
    let root = parse_toml_table(config_toml)?;
    let mut profile = Table::new();

    for key in PROFILE_CONFIG_SCALAR_KEYS {
        copy_toml_key(&root, &mut profile, key);
    }
    for key in PROFILE_CONFIG_TABLE_KEYS {
        copy_toml_key(&root, &mut profile, key);
    }
    for key in PROFILE_CONFIG_TUI_KEYS {
        copy_toml_nested_key(&root, &mut profile, &["tui", key]);
    }

    if let Some(model_provider) = root.get("model_provider").and_then(|value| value.as_str()) {
        if let Some(provider_value) = root
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get(model_provider))
            .cloned()
        {
            let providers = profile
                .entry("model_providers".to_string())
                .or_insert_with(|| toml::Value::Table(Table::new()));
            if let Some(providers_table) = providers.as_table_mut() {
                providers_table.insert(model_provider.to_string(), provider_value);
            }
        }
    }

    if profile.is_empty() {
        return Ok(None);
    }

    Ok(Some(toml::to_string_pretty(&profile)?))
}

fn merge_and_write_config_toml(
    path: &Path,
    profile_config_toml: Option<&str>,
    exclude_tool_proxy_envs: bool,
) -> anyhow::Result<()> {
    let existing = read_optional_text(path)?;
    let mut merged = existing
        .as_deref()
        .map(parse_toml_table)
        .transpose()?
        .unwrap_or_default();

    strip_profile_config_keys(&mut merged);

    if let Some(profile_config_toml) = profile_config_toml {
        let profile = parse_toml_table(profile_config_toml)?;

        for key in PROFILE_CONFIG_SCALAR_KEYS {
            if let Some(value) = profile.get(key).cloned() {
                merged.insert(key.to_string(), value);
            }
        }
        for key in PROFILE_CONFIG_TABLE_KEYS {
            copy_toml_key(&profile, &mut merged, key);
        }
        for key in PROFILE_CONFIG_TUI_KEYS {
            copy_toml_nested_key(&profile, &mut merged, &["tui", key]);
        }

        if let Some(provider_name) = profile
            .get("model_provider")
            .and_then(|value| value.as_str())
        {
            let provider_value = profile
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get(provider_name))
                .cloned();

            if let Some(provider_value) = provider_value {
                let providers = merged
                    .entry("model_providers".to_string())
                    .or_insert_with(|| toml::Value::Table(Table::new()));
                let providers_table = providers
                    .as_table_mut()
                    .ok_or_else(|| anyhow!("config.toml key `model_providers` must be a table"))?;
                providers_table.insert(provider_name.to_string(), provider_value);
            }
        }
    }

    reconcile_tool_proxy_env_excludes(&mut merged, exclude_tool_proxy_envs)?;

    if merged.is_empty() {
        return write_optional_text(path, None);
    }

    write_optional_text(path, Some(&toml::to_string_pretty(&merged)?))
}

fn parse_toml_table(contents: &str) -> anyhow::Result<Table> {
    let value: toml::Value = toml::from_str(contents).context("Failed to parse config.toml")?;
    value
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow!("config.toml root must be a table"))
}

fn read_profile_specific_config_table(path: &Path) -> anyhow::Result<Table> {
    let Some(existing) = read_optional_text(path)? else {
        return Ok(Table::new());
    };
    let extracted = extract_profile_config_toml(&existing)?.and_then(|value| {
        if value.trim().is_empty() {
            None
        } else {
            Some(value)
        }
    });
    match extracted {
        Some(value) => parse_toml_table(&value),
        None => Ok(Table::new()),
    }
}

fn strip_profile_config_keys(root: &mut Table) {
    let provider_to_remove = root
        .get("model_provider")
        .and_then(|value| value.as_str())
        .map(str::to_string);

    for key in PROFILE_CONFIG_SCALAR_KEYS {
        root.remove(key);
    }
    for key in PROFILE_CONFIG_TUI_KEYS {
        remove_toml_nested_key(root, &["tui", key]);
    }

    if let Some(provider_name) = provider_to_remove {
        if let Some(providers) = root
            .get_mut("model_providers")
            .and_then(|value| value.as_table_mut())
        {
            providers.remove(&provider_name);
            if providers.is_empty() {
                root.remove("model_providers");
            }
        }
    }
}

fn reconcile_tool_proxy_env_excludes(
    root: &mut Table,
    exclude_tool_proxy_envs: bool,
) -> anyhow::Result<()> {
    if exclude_tool_proxy_envs && !root.contains_key("shell_environment_policy") {
        root.insert(
            "shell_environment_policy".to_string(),
            toml::Value::Table(Table::new()),
        );
    }

    let remove_shell_policy = {
        let Some(policy_value) = root.get_mut("shell_environment_policy") else {
            return Ok(());
        };
        let policy = policy_value
            .as_table_mut()
            .ok_or_else(|| anyhow!("config.toml key `shell_environment_policy` must be a table"))?;

        let mut exclude_values = match policy.remove("exclude") {
            Some(toml::Value::Array(values)) => values,
            Some(_) => {
                anyhow::bail!("config.toml key `shell_environment_policy.exclude` must be an array")
            }
            None => Vec::new(),
        };

        exclude_values.retain(|value| {
            value
                .as_str()
                .map(|pattern| !is_managed_tool_proxy_env_exclude_pattern(pattern))
                .unwrap_or(true)
        });

        if exclude_tool_proxy_envs {
            for pattern in TOOL_PROXY_ENV_EXCLUDE_PATTERNS {
                let already_present = exclude_values.iter().any(|value| {
                    value
                        .as_str()
                        .map(|existing| existing.eq_ignore_ascii_case(pattern))
                        .unwrap_or(false)
                });
                if !already_present {
                    exclude_values.push(toml::Value::String(pattern.to_string()));
                }
            }
        }

        if exclude_values.is_empty() {
            policy.remove("exclude");
        } else {
            policy.insert("exclude".to_string(), toml::Value::Array(exclude_values));
        }

        policy.is_empty()
    };

    if remove_shell_policy {
        root.remove("shell_environment_policy");
    }

    Ok(())
}

fn is_managed_tool_proxy_env_exclude_pattern(pattern: &str) -> bool {
    TOOL_PROXY_ENV_EXCLUDE_PATTERNS
        .iter()
        .any(|managed| managed.eq_ignore_ascii_case(pattern))
}

fn copy_toml_key(source: &Table, target: &mut Table, key: &str) {
    if let Some(value) = source.get(key).cloned() {
        target.insert(key.to_string(), value);
    }
}

fn copy_toml_nested_key(source: &Table, target: &mut Table, path: &[&str]) {
    if path.len() != 2 {
        return;
    }

    let [section, key] = [path[0], path[1]];
    let Some(value) = source
        .get(section)
        .and_then(|value| value.as_table())
        .and_then(|table| table.get(key))
        .cloned()
    else {
        return;
    };

    let section_value = target
        .entry(section.to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()));
    if let Some(section_table) = section_value.as_table_mut() {
        section_table.insert(key.to_string(), value);
    }
}

fn remove_toml_nested_key(root: &mut Table, path: &[&str]) {
    if path.len() != 2 {
        return;
    }

    let [section, key] = [path[0], path[1]];
    let remove_section = root
        .get_mut(section)
        .and_then(|value| value.as_table_mut())
        .map(|table| {
            table.remove(key);
            table.is_empty()
        })
        .unwrap_or(false);

    if remove_section {
        root.remove(section);
    }
}

impl StoredAccount {
    fn from_import(name: String, snapshot: &ImportedSnapshot, runtime: RuntimeConfig) -> Self {
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
            last_used_at: None,
        }
    }
}

fn activate_account(target: &str) -> anyhow::Result<StoredAccount> {
    let mut store = load_store()?;
    let account_id = find_account(&store, target)?
        .map(|account| account.id.clone())
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    let account = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| anyhow!("Account not found after sync: {target}"))?;

    switch_to_account(&account)?;

    store.active_account_id = Some(account.id.clone());
    if let Some(acc) = store.accounts.iter_mut().find(|a| a.id == account.id) {
        acc.last_used_at = Some(Utc::now());
    }
    save_store(&store)?;

    Ok(account)
}

fn switch_to_account(account: &StoredAccount) -> anyhow::Result<()> {
    let files = ensure_materialized_account_files(account)?;
    sync_active_codex_home_files(account, &files)
}

fn sync_active_codex_home_files(
    account: &StoredAccount,
    files: &MaterializedAccountFiles,
) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex {
        return Ok(());
    }

    let codex_home = host_codex_home_dir()?;
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create CODEX_HOME: {}", codex_home.display()))?;

    let active_auth_path = codex_home.join("auth.json");
    let auth_json = fs::read_to_string(&files.auth_path)
        .with_context(|| format!("Failed to read auth.json: {}", files.auth_path.display()))?;
    write_optional_text_if_changed(&active_auth_path, Some(&auth_json))?;

    let profile_config = read_optional_text(&files.config_path)?
        .map(|existing| {
            extract_profile_config_toml(&existing).with_context(|| {
                format!(
                    "Failed to parse existing config.toml for profile '{}' at {}",
                    account.name,
                    files.config_path.display()
                )
            })
        })
        .transpose()?
        .flatten();
    let global_config = load_codez_config();
    merge_and_write_config_toml(
        &codex_home.join("config.toml"),
        profile_config.as_deref(),
        effective_proxy_config(account, &global_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;

    let active_custom_status_items_path = codex_home.join("custom-status-items.json");
    let custom_status_items = read_optional_text(&files.custom_status_items_path)?;
    write_optional_text_if_changed(
        &active_custom_status_items_path,
        custom_status_items.as_deref(),
    )?;

    set_active_codex_home_file_permissions(
        &active_auth_path,
        &codex_home.join("config.toml"),
        &active_custom_status_items_path,
    )?;
    Ok(())
}

fn set_active_codex_home_file_permissions(
    _auth_path: &Path,
    _config_path: &Path,
    _custom_status_items_path: &Path,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        for path in [_auth_path, _config_path, _custom_status_items_path] {
            if path.exists() {
                fs::set_permissions(path, perms.clone())?;
            }
        }
    }
    Ok(())
}

fn materialized_profiles_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("profiles"))
}

fn materialized_account_files(account: &StoredAccount) -> anyhow::Result<MaterializedAccountFiles> {
    let dir = materialized_profiles_dir()?.join(&account.id);
    Ok(MaterializedAccountFiles {
        auth_path: dir.join("auth.json"),
        config_path: dir.join("config.toml"),
        custom_status_items_path: dir.join("custom-status-items.json"),
    })
}

fn default_custom_status_items() -> Vec<CustomStatusItemCatalogEntry> {
    vec![
        CustomStatusItemCatalogEntry {
            id: "custom:bon-voyage".to_string(),
            title: "Bon voyage".to_string(),
            description: None,
            source: CustomStatusItemSource::Static {
                value: "Bon voyage !".to_string(),
            },
            render: CustomStatusItemRender::Value,
            style: CustomStatusItemStyle {
                fg: Some("#F6A3C8".to_string()),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underlined: false,
            },
        },
        CustomStatusItemCatalogEntry {
            id: "custom:profile".to_string(),
            title: "Profile".to_string(),
            description: None,
            source: CustomStatusItemSource::LaunchProfile,
            render: CustomStatusItemRender::Value,
            style: CustomStatusItemStyle {
                fg: Some("#FFFFFF".to_string()),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underlined: false,
            },
        },
    ]
}

fn normalize_custom_status_items(
    items: &[CustomStatusItemCatalogEntry],
) -> Vec<CustomStatusItemCatalogEntry> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let defaults = default_custom_status_items();

    for item in items.iter().chain(defaults.iter()) {
        let id = item.id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }

        let title = item.title.trim();
        normalized.push(CustomStatusItemCatalogEntry {
            id: id.to_string(),
            title: if title.is_empty() {
                id.to_string()
            } else {
                title.to_string()
            },
            description: item
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            source: item.source.clone(),
            render: item.render.clone(),
            style: item.style.clone(),
        });
    }

    normalized
}

fn custom_status_items_catalog_json(config: &CodezConfig) -> anyhow::Result<Option<String>> {
    let items = normalize_custom_status_items(&config.custom_status_items);
    if items.is_empty() {
        return Ok(None);
    }

    Ok(Some(serde_json::to_string_pretty(
        &CustomStatusItemsCatalogFile { items },
    )?))
}

fn write_optional_text_if_changed(path: &Path, contents: Option<&str>) -> anyhow::Result<()> {
    let existing = read_optional_text(path)?;
    let next = contents.map(str::to_string);
    if existing == next {
        return Ok(());
    }

    write_optional_text(path, contents)
}

fn ensure_materialized_account_files(
    account: &StoredAccount,
) -> anyhow::Result<MaterializedAccountFiles> {
    let files = materialized_account_files(account)?;
    if let Some(parent) = files.auth_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
    }

    if !files.auth_path.exists() {
        anyhow::bail!(
            "Profile '{}' is missing auth.json at {}. Re-import or restore this profile file.",
            account.name,
            files.auth_path.display()
        );
    }

    let profile_config = read_optional_text(&files.config_path)?
        .map(|existing| {
            extract_profile_config_toml(&existing).with_context(|| {
                format!(
                    "Failed to parse existing config.toml for profile '{}' at {}",
                    account.name,
                    files.config_path.display()
                )
            })
        })
        .transpose()?
        .flatten();
    let profile_config = normalize_profile_config_for_account(account, profile_config)?;
    let codez_config = load_codez_config();
    merge_and_write_config_toml(
        &files.config_path,
        profile_config.as_deref(),
        effective_proxy_config(account, &codez_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;
    let custom_status_items_json = custom_status_items_catalog_json(&codez_config)?;
    write_optional_text_if_changed(
        &files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&files)?;
    Ok(files)
}

fn materialize_imported_account_files(
    account: &StoredAccount,
    snapshot: &ImportedSnapshot,
) -> anyhow::Result<()> {
    let files = materialized_account_files(account)?;
    if let Some(parent) = files.auth_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
    }

    write_optional_text_if_changed(&files.auth_path, Some(&snapshot.raw_auth_json))?;
    let codez_config = load_codez_config();
    let profile_config =
        normalize_profile_config_for_account(account, snapshot.raw_config_toml.clone())?;
    merge_and_write_config_toml(
        &files.config_path,
        profile_config.as_deref(),
        effective_proxy_config(account, &codez_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;
    let custom_status_items_json = custom_status_items_catalog_json(&codez_config)?;
    write_optional_text_if_changed(
        &files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&files)?;
    Ok(())
}

fn set_materialized_file_permissions(_files: &MaterializedAccountFiles) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&_files.auth_path, perms.clone())?;
        if _files.config_path.exists() {
            fs::set_permissions(&_files.config_path, perms.clone())?;
        }
        if _files.custom_status_items_path.exists() {
            fs::set_permissions(&_files.custom_status_items_path, perms)?;
        }
    }
    Ok(())
}

fn legacy_auth_json_from_auth_data(auth: &AuthData) -> anyhow::Result<String> {
    let value = match auth {
        AuthData::ApiKey { key } => serde_json::json!({
            "OPENAI_API_KEY": key,
            "tokens": null,
            "last_refresh": null,
        }),
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id,
            },
            "last_refresh": Utc::now(),
        }),
    };

    Ok(serde_json::to_string_pretty(&value)?)
}

fn sandbox_user_home(_user_name: &str) -> anyhow::Result<PathBuf> {
    let preferred = docker_runtime_home_dir()?;
    if preferred.exists() {
        return Ok(preferred);
    }

    let legacy = legacy_docker_runtime_home_dir()?;
    if legacy.exists() {
        return Ok(legacy);
    }

    Ok(preferred)
}

fn runtime_label(runtime: &RuntimeConfig) -> &'static str {
    match runtime {
        RuntimeConfig::Host => "host",
        RuntimeConfig::Docker { .. } => "docker",
    }
}

fn runtime_from_option(
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> RuntimeConfig {
    match docker_image {
        Some(image) => RuntimeConfig::Docker {
            image,
            user_name: Some(
                normalize_docker_user_name(docker_user_name)
                    .unwrap_or_else(|_| default_docker_user_name()),
            ),
        },
        None => RuntimeConfig::Host,
    }
}

fn runtime_description(runtime: &RuntimeConfig) -> String {
    match runtime {
        RuntimeConfig::Host => "host".to_string(),
        RuntimeConfig::Docker { image, user_name } => format!(
            "docker image={} user={}",
            image,
            docker_user_name(user_name.as_deref()).unwrap_or_else(|_| default_docker_user_name())
        ),
    }
}

fn apply_runtime_override(
    account: &StoredAccount,
    force_host: bool,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> anyhow::Result<StoredAccount> {
    if force_host {
        let mut effective = account.clone();
        effective.runtime = RuntimeConfig::Host;
        return Ok(effective);
    }

    if let Some(image) = docker_image {
        let mut effective = account.clone();
        effective.runtime = RuntimeConfig::Docker {
            image,
            user_name: Some(normalize_docker_user_name(docker_user_name)?),
        };
        return Ok(effective);
    }

    Ok(account.clone())
}

fn parse_optional_u64(value: &str) -> anyhow::Result<Option<u64>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Ok(None);
    }
    let parsed = trimmed
        .parse::<u64>()
        .with_context(|| format!("Unsupported integer value: {value}"))?;
    Ok(Some(parsed))
}

fn parse_optional_csv(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }
    Some(
        trimmed
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| item.replace('-', "_"))
            .collect(),
    )
}

fn parse_optional_user_message_content(value: &str) -> anyhow::Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Ok(None);
    }
    let normalized = trimmed.replace('-', "_");
    match normalized.as_str() {
        "none" | "preview" | "full" => Ok(Some(normalized)),
        _ => anyhow::bail!(
            "Unsupported notify user message content mode: {value}. Use none, preview, or full"
        ),
    }
}

fn parse_optional_rate_limit_mode(value: &str) -> anyhow::Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return Ok(None);
    }
    let normalized = trimmed.replace('-', "_");
    match normalized.as_str() {
        "off" | "daily" | "always" => Ok(Some(normalized)),
        _ => anyhow::bail!(
            "Unsupported rate limit reminder mode: {value}. Use off, daily, or always"
        ),
    }
}

fn bool_label(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn apply_annotation(
    account: &mut StoredAccount,
    source: Option<String>,
    clear_source: bool,
    plan: Option<String>,
    clear_plan: bool,
    email: Option<String>,
    clear_email: bool,
) {
    if clear_source {
        account.source = None;
    } else if let Some(source) = source {
        account.source = Some(source);
    }

    if clear_plan {
        account.plan_type = None;
    } else if let Some(plan) = plan {
        account.plan_type = Some(plan);
    }

    if clear_email {
        account.email = None;
    } else if let Some(email) = email {
        account.email = Some(email);
    }
}

fn run_codex_process(account: &StoredAccount, codex_args: Vec<String>) -> anyhow::Result<()> {
    let program = cli_program(&account.cli_kind);
    let base_codex_args = combined_profile_cli_args(account, codex_args);
    let add_docker_sandbox_bypass = should_add_docker_sandbox_bypass(account, &base_codex_args);
    let effective_codex_args = codex_args_for_runtime(account, base_codex_args);
    println!("CLI binary: {BOLD}{}{RESET}", program);
    print_launch_summary(account);
    if !account.default_cli_args.is_empty() {
        println!(
            "Default CLI args: {}",
            cli_args_label(&account.default_cli_args)
        );
    }
    if !effective_codex_args.is_empty() {
        println!("CLI args: {}", cli_args_label(&effective_codex_args));
    }
    if add_docker_sandbox_bypass {
        println!(
            "docker detected: adding {} --sandbox danger-full-access to avoid bubblewrap/userns failures",
            program
        );
    }
    ensure_desktop_notify_bridge_for_launch(account)?;
    let launch = maybe_wrap_launch_with_session(
        account,
        &effective_codex_args,
        codex_launch_command(account, &effective_codex_args)?,
    )?;
    let exit_code = exit_code_from_status(
        launch
            .to_command()
            .status()
            .with_context(|| format!("Failed to start launch command for {program}"))?,
    );

    std::process::exit(exit_code);
}

fn print_launch_summary(account: &StoredAccount) {
    let global_config = load_codez_config();
    let effective_proxy = effective_proxy_config(account, &global_config);
    let proxy_scope = account_proxy_scope_label(account, &global_config);
    let proxy_label = proxy_config_label(effective_proxy);
    let session_scope = account_session_scope_label(account, &global_config);
    let session_label = session_config_label(effective_session_config(account, &global_config));
    let provider = account_model_provider(account).unwrap_or_else(|| "-".to_string());
    let api = account_model_api_base(account).unwrap_or_else(|| "-".to_string());
    let tool_proxy_mode = if effective_proxy.map(|proxy| proxy.enabled).unwrap_or(false) {
        "direct(excluded)"
    } else {
        "inherit-shell"
    };

    println!(
        "Launch: cli={} profile={} runtime=\"{}\" proxy_scope={} proxy=\"{}\" session_scope={} session={} provider={} api={} tool_proxy={}",
        account.cli_kind,
        account.name,
        runtime_description(&account.runtime),
        proxy_scope,
        proxy_label,
        session_scope,
        session_label,
        provider,
        api,
        tool_proxy_mode
    );
}

fn combined_profile_cli_args(account: &StoredAccount, codex_args: Vec<String>) -> Vec<String> {
    let mut effective_args = account.default_cli_args.clone();
    effective_args.extend(codex_args);
    effective_args
}

fn codex_args_for_runtime(account: &StoredAccount, mut codex_args: Vec<String>) -> Vec<String> {
    if should_add_docker_sandbox_bypass(account, &codex_args) {
        codex_args.insert(0, "danger-full-access".to_string());
        codex_args.insert(0, "--sandbox".to_string());
    }
    codex_args
}

fn program_supports_codex_sandbox_flag() -> bool {
    let program = codex_program();
    let program_name = Path::new(program.as_str())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program.as_str());
    matches!(program_name, "codex" | "cute-codex" | "cutex-codex")
}

fn should_add_docker_sandbox_bypass(account: &StoredAccount, codex_args: &[String]) -> bool {
    account.cli_kind == CliKind::Codex
        && matches!(account.runtime, RuntimeConfig::Docker { .. })
        && program_supports_codex_sandbox_flag()
        && !codex_args
            .iter()
            .any(|arg| arg == "--sandbox" || arg.starts_with("--sandbox="))
        && !codex_args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
}

fn codex_launch_command(
    account: &StoredAccount,
    codex_args: &[String],
) -> anyhow::Result<LaunchCommand> {
    let files = ensure_materialized_account_files(account)?;
    match &account.runtime {
        RuntimeConfig::Host => Ok(apply_codex_launch_envs(
            LaunchCommand::new(cli_program(&account.cli_kind)).args(codex_args.iter().cloned()),
            account,
            files.auth_path.to_string_lossy().as_ref(),
            files.config_path.to_string_lossy().as_ref(),
            files.custom_status_items_path.to_string_lossy().as_ref(),
            Some(&files.auth_path),
        )),
        RuntimeConfig::Docker { image, .. } => {
            docker_codex_launch_command(account, image, codex_args, &files.auth_path)
        }
    }
}

fn docker_codex_launch_command(
    account: &StoredAccount,
    image: &str,
    codex_args: &[String],
    host_auth_path: &Path,
) -> anyhow::Result<LaunchCommand> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let user_name = docker_user_name(match &account.runtime {
        RuntimeConfig::Docker { user_name, .. } => user_name.as_deref(),
        RuntimeConfig::Host => None,
    })?;
    let sandbox_user_home = sandbox_user_home(&user_name)?;

    let container_user_home = format!("/home/{user_name}");
    let container_codex_home = format!("{container_user_home}/.codex");
    let profiles_root = materialized_profiles_dir()?;
    let container_profiles_root = format!("{container_user_home}/.cutex-profiles");
    let container_profile_dir = format!("{container_profiles_root}/{}", account.id);
    let container_auth_path = format!("{container_profile_dir}/auth.json");
    let container_config_path = format!("{container_profile_dir}/config.toml");
    let container_custom_status_items_path =
        format!("{container_profile_dir}/custom-status-items.json");
    let workspace = cwd
        .to_str()
        .ok_or_else(|| anyhow!("Current directory is not valid UTF-8"))?;
    let global_config = load_codez_config();
    let effective_proxy = effective_proxy_config(account, &global_config);
    let add_host_gateway_alias = effective_proxy
        .filter(|proxy| proxy.enabled)
        .and_then(|proxy| proxy.url.as_deref())
        .is_some_and(|url| rewrite_docker_loopback_proxy_url(url).is_some());
    let launch_envs = codex_launch_envs(
        account,
        &container_auth_path,
        &container_config_path,
        &container_custom_status_items_path,
        Some(host_auth_path),
    );
    let mut launch = docker_command();

    launch = launch
        .arg("run")
        .arg("--rm")
        .arg("-it")
        .arg("--user")
        .arg(current_user_spec()?)
        .arg("-e")
        .arg(format!("HOME={container_user_home}"))
        .arg("-e")
        .arg(format!("USER={user_name}"))
        .arg("-e")
        .arg(format!("LOGNAME={user_name}"))
        .arg("-e")
        .arg(format!("CODEX_HOME={container_codex_home}"))
        .arg("-v")
        .arg(format!("{workspace}:{workspace}"))
        .arg("-w")
        .arg(workspace)
        .arg("-v")
        .arg(format!(
            "{}:{}",
            sandbox_user_home.display(),
            container_user_home
        ))
        .arg("-v")
        .arg(format!(
            "{}:{}",
            profiles_root.display(),
            container_profiles_root
        ));
    if add_host_gateway_alias {
        launch = launch
            .arg("--add-host")
            .arg(format!("{DOCKER_PROXY_HOST_ALIAS}:host-gateway"));
    }

    for (key, value) in &launch_envs {
        launch = launch.arg("-e").arg(format!("{key}={value}"));
    }

    launch = launch
        .arg(image)
        .arg(cli_program(&account.cli_kind))
        .args(codex_args.iter().cloned());

    Ok(launch)
}

fn codex_launch_envs(
    account: &StoredAccount,
    auth_path: &str,
    config_path: &str,
    custom_status_items_path: &str,
    api_key_auth_path: Option<&Path>,
) -> Vec<(String, String)> {
    let mut envs = match account.cli_kind {
        CliKind::Claude => claude_launch_envs(account),
        CliKind::Codex => codex_specific_launch_envs(
            account,
            auth_path,
            config_path,
            custom_status_items_path,
            api_key_auth_path,
        ),
    };

    let global_config = load_codez_config();
    if global_config.desktop_notify_enabled {
        let port = desktop_notify_port(&global_config);
        envs.push((
            "CODEX_NOTIFY_SERVICE_URL".to_string(),
            desktop_notify_bridge_url(port),
        ));
        if let Some(token) = &global_config.desktop_notify_token {
            if !token.is_empty() {
                envs.push(("CODEX_NOTIFY_SERVICE_TOKEN".to_string(), token.clone()));
            }
        }
    } else {
        if let Some(url) = &global_config.notify_service_url {
            if !url.is_empty() {
                envs.push(("CODEX_NOTIFY_SERVICE_URL".to_string(), url.clone()));
            }
        }
        if let Some(token) = &global_config.notify_service_token {
            if !token.is_empty() {
                envs.push(("CODEX_NOTIFY_SERVICE_TOKEN".to_string(), token.clone()));
            }
        }
    }
    if std::env::var_os(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR).is_none() {
        if let Some(timeout) = global_config.notify_service_idle_timeout_secs {
            envs.push((
                CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR.to_string(),
                timeout.to_string(),
            ));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR).is_none() {
        if let Some(timeout) = global_config.notify_service_composer_idle_timeout_secs {
            envs.push((
                CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR.to_string(),
                timeout.to_string(),
            ));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR).is_none() {
        if let Some(timeout) = global_config.notify_service_approval_timeout_secs {
            envs.push((
                CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR.to_string(),
                timeout.to_string(),
            ));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR).is_none() {
        if let Some(timeout) = global_config.notify_service_startup_idle_timeout_secs {
            envs.push((
                CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR.to_string(),
                timeout.to_string(),
            ));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_EVENTS_ENV_VAR).is_none() {
        if let Some(events) = &global_config.notify_service_events {
            envs.push((CODEX_NOTIFY_EVENTS_ENV_VAR.to_string(), events.join(",")));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR).is_none() {
        if let Some(content) = &global_config.notify_service_user_message_content {
            envs.push((
                CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR.to_string(),
                content.clone(),
            ));
        }
    }
    if std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR).is_none() {
        if let Some(chars) = global_config.notify_service_user_message_preview_chars {
            envs.push((
                CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR.to_string(),
                chars.to_string(),
            ));
        }
    }
    if std::env::var_os(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR).is_none() {
        if let Some(mode) = &global_config.rate_limit_threshold_warning_mode {
            envs.push((
                CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR.to_string(),
                mode.clone(),
            ));
        }
    }
    if std::env::var_os(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR).is_none() {
        if let Some(mode) = &global_config.rate_limit_model_nudge_mode {
            envs.push((
                CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR.to_string(),
                mode.clone(),
            ));
        }
    }

    envs
}

fn claude_launch_envs(account: &StoredAccount) -> Vec<(String, String)> {
    let global_config = load_codez_config();
    let claude_config_dir = materialized_claude_config_dir(account);
    let mut envs = vec![
        (
            CLAUDE_CONFIG_DIR_ENV_VAR.to_string(),
            claude_config_dir.to_string_lossy().to_string(),
        ),
        (
            CODEX_LAUNCH_PROFILE_ENV_VAR.to_string(),
            account.name.clone(),
        ),
        (
            CODEX_LAUNCH_RUNTIME_ENV_VAR.to_string(),
            runtime_label(&account.runtime).to_string(),
        ),
    ];

    let api_key_path = claude_config_dir.join("api_key");
    if let Ok(key) = fs::read_to_string(&api_key_path) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            envs.push(("ANTHROPIC_API_KEY".to_string(), key));
        }
    }

    let provider_json_path = claude_config_dir.join("provider.json");
    if let Ok(raw) = fs::read_to_string(&provider_json_path) {
        if let Ok(val) = serde_json::from_str::<Value>(&raw) {
            if let Some(url) = val.get("base_url").and_then(|v| v.as_str()) {
                if !url.is_empty() {
                    envs.push(("ANTHROPIC_BASE_URL".to_string(), url.to_string()));
                }
            }
        }
    }

    envs.extend(proxy_envs(
        effective_proxy_config(account, &global_config),
        Some(&account.runtime),
    ));
    envs
}

fn materialized_claude_config_dir(account: &StoredAccount) -> PathBuf {
    let base = config_dir().unwrap_or_else(|_| PathBuf::from("."));
    base.join("profiles").join(&account.id).join("claude")
}

fn codex_specific_launch_envs(
    account: &StoredAccount,
    auth_path: &str,
    config_path: &str,
    custom_status_items_path: &str,
    api_key_auth_path: Option<&Path>,
) -> Vec<(String, String)> {
    let global_config = load_codez_config();
    let mut envs = vec![
        (
            CODEX_LAUNCH_PROFILE_ENV_VAR.to_string(),
            account.name.clone(),
        ),
        (
            CODEX_LAUNCH_RUNTIME_ENV_VAR.to_string(),
            runtime_label(&account.runtime).to_string(),
        ),
        (
            CODEX_LAUNCH_PROFILE_SOURCE_ENV_VAR.to_string(),
            account.source.as_deref().unwrap_or("unknown").to_string(),
        ),
        (
            CODEX_LAUNCH_PROFILE_TYPE_ENV_VAR.to_string(),
            account
                .plan_type
                .as_deref()
                .unwrap_or("unknown")
                .to_string(),
        ),
        (
            CODEX_LAUNCH_PROFILE_EMAIL_ENV_VAR.to_string(),
            account.email.as_deref().unwrap_or("-").to_string(),
        ),
        (CODEX_AUTH_FILE_ENV_VAR.to_string(), auth_path.to_string()),
        (
            CODEX_CONFIG_FILE_ENV_VAR.to_string(),
            config_path.to_string(),
        ),
        (
            CODEX_CUSTOM_STATUS_ITEMS_FILE_ENV_VAR.to_string(),
            custom_status_items_path.to_string(),
        ),
    ];
    if let Some(install_dir) = codex_install_dir_for_host_launch(account) {
        envs.push((CODEX_INSTALL_DIR_ENV_VAR.to_string(), install_dir));
    }
    if account.source.as_deref() == Some("api-key") {
        if let Some(api_key) = api_key_auth_path.and_then(codex_api_key_from_auth_file) {
            envs.push(("OPENAI_API_KEY".to_string(), api_key));
        }
    }
    envs.extend(proxy_envs(
        effective_proxy_config(account, &global_config),
        Some(&account.runtime),
    ));
    envs
}

fn codex_api_key_from_auth_file(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    json.get("OPENAI_API_KEY")
        .or_else(|| json.get("openai_api_key"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn apply_codex_launch_envs(
    mut launch: LaunchCommand,
    account: &StoredAccount,
    auth_path: &str,
    config_path: &str,
    custom_status_items_path: &str,
    api_key_auth_path: Option<&Path>,
) -> LaunchCommand {
    for (key, value) in codex_launch_envs(
        account,
        auth_path,
        config_path,
        custom_status_items_path,
        api_key_auth_path,
    ) {
        launch = launch.env(key, value);
    }
    launch
}

fn codex_install_dir_for_host_launch(account: &StoredAccount) -> Option<String> {
    if !matches!(account.runtime, RuntimeConfig::Host) {
        return None;
    }

    let program = codex_program();
    let program_name = Path::new(program.as_str())
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program.as_str());
    if program_name == "codex" {
        return None;
    }

    match ensure_codex_compat_install_dir(&program) {
        Ok(path) => Some(path.to_string_lossy().to_string()),
        Err(err) => {
            eprintln!(
                "{YELLOW}warning:{RESET} failed to prepare CODEX_INSTALL_DIR for app-server compatibility: {err:#}"
            );
            None
        }
    }
}

fn ensure_codex_compat_install_dir(program: &str) -> anyhow::Result<PathBuf> {
    let install_dir = runtime_dir()?.join("bin");
    fs::create_dir_all(&install_dir).with_context(|| {
        format!(
            "Failed to create Codex compatibility bin dir: {}",
            install_dir.display()
        )
    })?;

    let target = resolve_program_for_wrapper(program);
    let wrapper = install_dir.join("codex");
    let contents = format!("#!/usr/bin/env sh\nexec {} \"$@\"\n", shell_quote(&target));
    write_optional_text_if_changed(&wrapper, Some(&contents))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))?;
    }

    Ok(install_dir)
}

fn resolve_program_for_wrapper(program: &str) -> String {
    let path = Path::new(program);
    if path.is_absolute() || program.contains('/') || program.contains('\\') {
        return program.to_string();
    }

    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(program))
                .find(|candidate| candidate.is_file())
        })
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| program.to_string())
}

fn codex_program() -> String {
    env_var_first(&[CUTEX_CODEX_BIN_ENV_VAR, CODEZ_CODEX_BIN_ENV_VAR])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if command_exists_in_path("cute-codex") {
                "cute-codex".to_string()
            } else if command_exists_in_path("cutex-codex") {
                "cutex-codex".to_string()
            } else {
                "codex".to_string()
            }
        })
}

fn claude_program() -> String {
    env_var_first(&[CUTEX_CLAUDE_BIN_ENV_VAR])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "claude".to_string())
}

fn cli_program(kind: &CliKind) -> String {
    match kind {
        CliKind::Codex => codex_program(),
        CliKind::Claude => claude_program(),
    }
}

#[cfg(unix)]
fn current_user_spec() -> anyhow::Result<String> {
    let uid = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to query current uid")?;
    let gid = std::process::Command::new("id")
        .arg("-g")
        .output()
        .context("Failed to query current gid")?;

    if !uid.status.success() || !gid.status.success() {
        anyhow::bail!("Failed to determine current uid/gid");
    }

    let uid = String::from_utf8(uid.stdout).context("Invalid uid output")?;
    let gid = String::from_utf8(gid.stdout).context("Invalid gid output")?;
    Ok(format!("{}:{}", uid.trim(), gid.trim()))
}

#[cfg(not(unix))]
fn current_user_spec() -> anyhow::Result<String> {
    Ok("0:0".to_string())
}

fn docker_command() -> LaunchCommand {
    if docker_requires_sudo() {
        LaunchCommand::new("sudo").arg("docker")
    } else {
        LaunchCommand::new("docker")
    }
}

fn docker_requires_sudo() -> bool {
    env_bool_override_any(&[CUTEX_DOCKER_USE_SUDO_ENV_VAR, CODEZ_DOCKER_USE_SUDO_ENV_VAR])
        .unwrap_or_else(|| load_codez_config().docker_use_sudo)
}

fn docker_user_name(input: Option<&str>) -> anyhow::Result<String> {
    match input {
        Some(value) => normalize_docker_user_name(Some(value.to_string())),
        None => Ok(default_docker_user_name()),
    }
}

fn default_docker_user_name() -> String {
    std::env::var("USER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value != "."
                && value != ".."
                && !value.starts_with('-')
                && !value.contains('/')
                && !value.contains('\\')
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        })
        .unwrap_or_else(|| "cutex".to_string())
}

fn normalize_docker_user_name(input: Option<String>) -> anyhow::Result<String> {
    let value = input
        .unwrap_or_else(default_docker_user_name)
        .trim()
        .to_string();

    if value.is_empty() {
        anyhow::bail!("Docker user name cannot be empty");
    }

    if value == "." || value == ".." {
        anyhow::bail!("Docker user name cannot be '.' or '..'");
    }

    if value.contains('/') || value.contains('\\') {
        anyhow::bail!("Docker user name cannot contain path separators");
    }

    if value.starts_with('-') {
        anyhow::bail!("Docker user name cannot start with '-'");
    }

    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        anyhow::bail!("Docker user name may only contain ASCII letters, digits, '.', '_' or '-'");
    }

    Ok(value)
}

fn env_bool_override(name: &str) -> Option<bool> {
    parse_bool_env(std::env::var(name).ok().as_deref())
}

fn env_bool_override_any(names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| env_bool_override(name))
}

fn parse_bool_env(value: Option<&str>) -> Option<bool> {
    match value {
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES") | Some("on")
        | Some("ON") => Some(true),
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO") | Some("off")
        | Some("OFF") => Some(false),
        _ => None,
    }
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn command_exists_in_path(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }

    if command.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(command).is_file();
    }

    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    #[cfg(windows)]
    {
        let path_has_extension = Path::new(command).extension().is_some();
        let extensions: Vec<String> = if path_has_extension {
            vec![String::new()]
        } else {
            std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .filter(|values: &Vec<String>| !values.is_empty())
                .unwrap_or_else(|| vec![".EXE".to_string(), ".CMD".to_string(), ".BAT".to_string()])
        };

        std::env::split_paths(&path_var).any(|dir| {
            extensions.iter().any(|ext| {
                let candidate = if ext.is_empty() {
                    dir.join(command)
                } else {
                    dir.join(format!("{command}{ext}"))
                };
                candidate.is_file()
            })
        })
    }

    #[cfg(not(windows))]
    {
        std::env::split_paths(&path_var).any(|dir| dir.join(command).is_file())
    }
}

fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn sample_account(name: &str) -> StoredAccount {
        StoredAccount {
            id: format!("{name}-id"),
            name: name.to_string(),
            email: None,
            plan_type: None,
            source: Some("official".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            last_used_at: None,
        }
    }

    #[test]
    fn config_alias_opens_wizard() {
        let cli = Cli::try_parse_from(["cutex", "config"]).expect("config alias should parse");
        assert!(matches!(cli.command, Some(CommandKind::Wizard)));
    }

    #[test]
    fn ubuntu_desktop_notify_install_command_parses() {
        let cli = Cli::try_parse_from([
            "cutex",
            "notify",
            "desktop",
            "install-ubuntu",
            "--port",
            "24250",
        ])
        .expect("install-ubuntu should parse");
        assert!(matches!(
            cli.command,
            Some(CommandKind::Notify {
                command: NotifyCommand::Desktop {
                    command: DesktopNotifyCommand::InstallUbuntu { port: Some(24250) }
                }
            })
        ));
    }

    #[test]
    fn parse_cli_args_value_supports_shell_quoting() {
        let args =
            parse_cli_args_value("--sandbox danger-full-access --system-prompt 'hello world'")
                .expect("cli args should parse");
        assert_eq!(
            args,
            vec![
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
                "--system-prompt".to_string(),
                "hello world".to_string()
            ]
        );
    }

    fn write_profile_files(
        account: &StoredAccount,
        auth_json: &str,
        config_toml: Option<&str>,
    ) -> anyhow::Result<()> {
        let files = materialized_account_files(account)?;
        if let Some(parent) = files.auth_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
        }
        fs::write(&files.auth_path, auth_json)
            .with_context(|| format!("Failed to write auth.json: {}", files.auth_path.display()))?;
        match config_toml {
            Some(config) => fs::write(&files.config_path, config).with_context(|| {
                format!(
                    "Failed to write config.toml: {}",
                    files.config_path.display()
                )
            })?,
            None => {
                if files.config_path.exists() {
                    fs::remove_file(&files.config_path).with_context(|| {
                        format!(
                            "Failed to remove config.toml: {}",
                            files.config_path.display()
                        )
                    })?;
                }
            }
        }
        Ok(())
    }

    #[test]
    fn extract_profile_config_keeps_only_profile_specific_keys() {
        let config = r#"
cli_auth_credentials_store = "file"
model_provider = "anthropic"
model_context_window = 1000000
model_auto_compact_token_limit = 400000
other_key = "keep-out"

[tui]
status_line = ["launch-profile", "model-with-reasoning", "current-dir"]
session_picker_provider_filter = "all"

[model_providers.anthropic]
base_url = "https://example.test"
env_key = "ANTHROPIC_API_KEY"

[model_providers.openai]
base_url = "https://api.openai.com"
"#;

        let extracted = extract_profile_config_toml(config)
            .expect("extract should succeed")
            .expect("profile config should exist");

        assert!(extracted.contains("cli_auth_credentials_store = \"file\""));
        assert!(extracted.contains("model_provider = \"anthropic\""));
        assert!(extracted.contains("model_context_window = 1000000"));
        assert!(extracted.contains("model_auto_compact_token_limit = 400000"));
        assert!(extracted.contains("[model_providers.anthropic]"));
        assert!(extracted.contains("base_url = \"https://example.test\""));
        assert!(!extracted.contains("other_key"));
        assert!(!extracted.contains("[model_providers.openai]"));

        let extracted_table = parse_toml_table(&extracted).expect("extracted config should parse");
        assert_eq!(
            extracted_table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("status_line"))
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                }),
            Some(vec![
                "launch-profile".to_string(),
                "model-with-reasoning".to_string(),
                "current-dir".to_string(),
            ])
        );
        assert_eq!(
            extracted_table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("session_picker_provider_filter"))
                .and_then(|value| value.as_str()),
            Some("all")
        );
    }

    #[test]
    fn merge_and_write_config_replaces_selected_provider_only() {
        let tempdir = std::env::temp_dir().join(format!("codez-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&tempdir).expect("tempdir should be created");
        let path = tempdir.join("config.toml");

        let existing = r#"
foo = "bar"
model_context_window = 222222
model_auto_compact_token_limit = 111111
model_provider = "openai"

[tui]
status_line = ["model-name"]
session_picker_provider_filter = "current"

[model_providers.openai]
base_url = "https://old.example"

[model_providers.other]
base_url = "https://other.example"
"#;
        fs::write(&path, existing).expect("existing config should be written");

        let profile = r#"
cli_auth_credentials_store = "file"
model_provider = "anthropic"
model_context_window = 1000000
model_auto_compact_token_limit = 400000

[tui]
status_line = ["launch-profile", "model-with-reasoning", "current-dir"]
session_picker_provider_filter = "all"

[model_providers.anthropic]
base_url = "https://new.example"
"#;

        merge_and_write_config_toml(&path, Some(profile), false).expect("merge should succeed");
        let merged = fs::read_to_string(&path).expect("merged config should be readable");

        assert!(merged.contains("foo = \"bar\""));
        assert!(merged.contains("cli_auth_credentials_store = \"file\""));
        assert!(merged.contains("model_provider = \"anthropic\""));
        assert!(merged.contains("model_context_window = 1000000"));
        assert!(merged.contains("model_auto_compact_token_limit = 400000"));
        assert!(merged.contains("[model_providers.anthropic]"));
        assert!(merged.contains("base_url = \"https://new.example\""));
        assert!(merged.contains("[model_providers.other]"));
        assert!(!merged.contains("https://old.example"));
        assert!(!merged.contains("model_context_window = 222222"));
        assert!(!merged.contains("model_auto_compact_token_limit = 111111"));

        let merged_table = parse_toml_table(&merged).expect("merged config should parse");
        assert_eq!(
            merged_table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("status_line"))
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                }),
            Some(vec![
                "launch-profile".to_string(),
                "model-with-reasoning".to_string(),
                "current-dir".to_string(),
            ])
        );
        assert_eq!(
            merged_table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("session_picker_provider_filter"))
                .and_then(|value| value.as_str()),
            Some("all")
        );

        let _ = fs::remove_dir_all(&tempdir);
    }

    #[test]
    fn normalize_profile_config_adds_default_cutex_status_line() {
        let account = StoredAccount {
            id: "demo-id".to_string(),
            name: "demo".to_string(),
            email: None,
            plan_type: None,
            source: Some("official".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            last_used_at: None,
        };

        let normalized = normalize_profile_config_for_account(&account, None)
            .expect("normalize should succeed")
            .expect("default config should be materialized");
        let table = parse_toml_table(&normalized).expect("normalized config should parse");
        let tui = table
            .get("tui")
            .and_then(|value| value.as_table())
            .expect("tui table should exist");

        let status_line = tui
            .get("status_line")
            .and_then(|value| value.as_array())
            .expect("status_line should exist")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(status_line, DEFAULT_CUTEX_STATUS_LINE);
        assert_eq!(
            tui.get("status_line_use_colors")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn normalize_profile_config_preserves_explicit_status_line() {
        let account = StoredAccount {
            id: "demo-id".to_string(),
            name: "demo".to_string(),
            email: None,
            plan_type: None,
            source: Some("official".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            last_used_at: None,
        };
        let existing = r#"
[tui]
status_line = ["current-dir"]
status_line_use_colors = false
"#;

        let normalized = normalize_profile_config_for_account(&account, Some(existing.to_string()))
            .expect("normalize should succeed")
            .expect("config should remain materialized");
        let table = parse_toml_table(&normalized).expect("normalized config should parse");
        let tui = table
            .get("tui")
            .and_then(|value| value.as_table())
            .expect("tui table should exist");

        let status_line = tui
            .get("status_line")
            .and_then(|value| value.as_array())
            .expect("status_line should exist")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(status_line, vec!["current-dir".to_string()]);
        assert_eq!(
            tui.get("status_line_use_colors")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn custom_status_items_catalog_includes_cutex_defaults() {
        let json = custom_status_items_catalog_json(&CodezConfig::default())
            .expect("catalog json should serialize")
            .expect("default catalog should be materialized");
        let catalog: CustomStatusItemsCatalogFile =
            serde_json::from_str(&json).expect("catalog should parse");
        let ids = catalog
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"custom:bon-voyage"));
        assert!(ids.contains(&"custom:profile"));
    }

    #[test]
    fn merge_and_write_config_adds_managed_proxy_excludes_when_enabled() {
        let tempdir = std::env::temp_dir().join(format!("codez-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&tempdir).expect("tempdir should be created");
        let path = tempdir.join("config.toml");

        let existing = r#"
[shell_environment_policy]
exclude = ["PATH", "http_proxy"]
"#;
        fs::write(&path, existing).expect("existing config should be written");

        merge_and_write_config_toml(&path, None, true).expect("merge should succeed");
        let merged = fs::read_to_string(&path).expect("merged config should be readable");
        let merged_table = parse_toml_table(&merged).expect("merged config should parse");
        let excludes = merged_table
            .get("shell_environment_policy")
            .and_then(|value| value.as_table())
            .and_then(|policy| policy.get("exclude"))
            .and_then(|value| value.as_array())
            .expect("exclude list should exist");
        let excludes_upper = excludes
            .iter()
            .filter_map(|value| value.as_str().map(|entry| entry.to_ascii_uppercase()))
            .collect::<Vec<_>>();

        assert!(excludes_upper.iter().any(|entry| entry == "PATH"));
        for managed in TOOL_PROXY_ENV_EXCLUDE_PATTERNS {
            assert!(
                excludes_upper.iter().any(|entry| entry == managed),
                "missing managed exclude `{managed}`"
            );
            assert_eq!(
                excludes_upper
                    .iter()
                    .filter(|entry| entry.as_str() == managed)
                    .count(),
                1,
                "managed exclude `{managed}` should not be duplicated"
            );
        }

        let _ = fs::remove_dir_all(&tempdir);
    }

    #[test]
    fn merge_and_write_config_removes_managed_proxy_excludes_when_disabled() {
        let tempdir = std::env::temp_dir().join(format!("codez-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&tempdir).expect("tempdir should be created");
        let path = tempdir.join("config.toml");

        let existing = r#"
foo = "bar"

[shell_environment_policy]
exclude = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "PATH"]
"#;
        fs::write(&path, existing).expect("existing config should be written");

        merge_and_write_config_toml(&path, None, false).expect("merge should succeed");
        let merged = fs::read_to_string(&path).expect("merged config should be readable");
        let merged_table = parse_toml_table(&merged).expect("merged config should parse");

        assert_eq!(
            merged_table.get("foo").and_then(|value| value.as_str()),
            Some("bar")
        );
        let excludes = merged_table
            .get("shell_environment_policy")
            .and_then(|value| value.as_table())
            .and_then(|policy| policy.get("exclude"))
            .and_then(|value| value.as_array())
            .expect("exclude list should exist");
        let excludes_upper = excludes
            .iter()
            .filter_map(|value| value.as_str().map(|entry| entry.to_ascii_uppercase()))
            .collect::<Vec<_>>();

        assert!(excludes_upper.iter().any(|entry| entry == "PATH"));
        for managed in TOOL_PROXY_ENV_EXCLUDE_PATTERNS {
            assert!(
                !excludes_upper.iter().any(|entry| entry == managed),
                "managed exclude `{managed}` should be removed when disabled"
            );
        }

        let _ = fs::remove_dir_all(&tempdir);
    }

    #[test]
    fn ensure_materialized_account_files_adds_managed_proxy_excludes_for_enabled_proxy() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut global = CodezConfig::default();
        global.proxy = Some(
            proxy_config_from_parts(
                true,
                Some("socks5h://127.0.0.1:7890".to_string()),
                Some("localhost,127.0.0.1".to_string()),
                true,
            )
            .expect("proxy config should be valid"),
        );
        save_codez_config(&global).expect("config should save");

        let account = sample_account("proxy-materialize");
        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some(
                r#"
model_provider = "openai"

[tui]
status_line = ["launch-profile", "current-dir"]
"#,
            ),
        )
        .expect("profile files should be written");

        let files =
            ensure_materialized_account_files(&account).expect("account files should materialize");
        let merged = fs::read_to_string(&files.config_path).expect("config should be readable");
        let merged_table = parse_toml_table(&merged).expect("merged config should parse");
        let excludes = merged_table
            .get("shell_environment_policy")
            .and_then(|value| value.as_table())
            .and_then(|policy| policy.get("exclude"))
            .and_then(|value| value.as_array())
            .expect("exclude list should exist");
        let excludes_upper = excludes
            .iter()
            .filter_map(|value| value.as_str().map(|entry| entry.to_ascii_uppercase()))
            .collect::<Vec<_>>();

        for managed in TOOL_PROXY_ENV_EXCLUDE_PATTERNS {
            assert!(
                excludes_upper.iter().any(|entry| entry == managed),
                "missing managed exclude `{managed}`"
            );
        }

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn determine_default_profile_prefers_directory_mapping_then_config_then_state_then_first() {
        let store = AccountsStore {
            version: STORE_VERSION,
            accounts: vec![sample_account("alpha"), sample_account("beta")],
            active_account_id: None,
        };

        let mut state = QuickRunState::default();
        state.last_global_profile = Some("beta".to_string());
        state
            .per_directory
            .insert("/workspace/project".to_string(), "alpha".to_string());
        let global_config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };

        assert_eq!(
            determine_default_profile(&store, &state, &global_config, Some("/workspace/project")),
            "alpha"
        );
        assert_eq!(
            determine_default_profile(&store, &state, &global_config, Some("/workspace/other")),
            "alpha"
        );
        assert_eq!(
            determine_default_profile(
                &store,
                &state,
                &CodezConfig::default(),
                Some("/workspace/other")
            ),
            "beta"
        );
        assert_eq!(
            determine_default_profile(
                &store,
                &QuickRunState::default(),
                &CodezConfig::default(),
                None
            ),
            "alpha"
        );
    }

    #[test]
    fn normalize_docker_user_name_rejects_invalid_values() {
        assert!(normalize_docker_user_name(Some("valid.user-1".to_string())).is_ok());
        assert!(normalize_docker_user_name(Some("".to_string())).is_err());
        assert!(normalize_docker_user_name(Some("../bad".to_string())).is_err());
        assert!(normalize_docker_user_name(Some("-bad".to_string())).is_err());
        assert!(normalize_docker_user_name(Some("bad name".to_string())).is_err());
    }

    #[test]
    fn docker_command_defaults_to_plain_docker() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let old_cutex = std::env::var_os(CUTEX_DOCKER_USE_SUDO_ENV_VAR);
        let old_codez = std::env::var_os(CODEZ_DOCKER_USE_SUDO_ENV_VAR);
        unsafe {
            std::env::set_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR, "0");
            std::env::remove_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR);
        }

        let launch = docker_command();
        assert_eq!(launch.program, "docker");
        assert!(launch.args.is_empty());

        match old_cutex {
            Some(value) => unsafe { std::env::set_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR) },
        }
        match old_codez {
            Some(value) => unsafe { std::env::set_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR) },
        }
    }

    #[test]
    fn docker_command_can_be_prefixed_with_sudo() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let old_cutex = std::env::var_os(CUTEX_DOCKER_USE_SUDO_ENV_VAR);
        let old_codez = std::env::var_os(CODEZ_DOCKER_USE_SUDO_ENV_VAR);
        unsafe {
            std::env::set_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR, "1");
            std::env::remove_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR);
        }

        let launch = docker_command();
        assert_eq!(launch.program, "sudo");
        assert_eq!(launch.args, vec!["docker".to_string()]);

        match old_cutex {
            Some(value) => unsafe { std::env::set_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_DOCKER_USE_SUDO_ENV_VAR) },
        }
        match old_codez {
            Some(value) => unsafe { std::env::set_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_DOCKER_USE_SUDO_ENV_VAR) },
        }
    }

    #[test]
    fn codez_config_defaults_match_expected_runtime_behavior() {
        let config = CodezConfig::default();

        assert!(!config.docker_use_sudo);
        assert!(config.custom_status_items.is_empty());
        assert!(config.proxy.is_none());
        assert!(!config.session.enabled);
        assert!(config.default_profile.is_none());
        assert!(!config.default_profile_direct_launch);
    }

    #[test]
    fn rename_and_remove_global_default_profile_references_follow_profile_changes() {
        let mut config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };

        assert!(rename_global_profile_references(
            &mut config,
            "alpha",
            "beta"
        ));
        assert_eq!(config.default_profile.as_deref(), Some("beta"));

        assert!(remove_global_profile_references(&mut config, "beta"));
        assert!(config.default_profile.is_none());
    }

    #[test]
    fn set_codez_codex_home_uses_codez_cli_subdir_and_migrates_legacy_home() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_codex_home = std::env::var_os("CODEX_HOME");
        let legacy_home = temp_home.join(".codex-codez");
        let new_home = temp_home.join(".cutex").join("codex-home");

        fs::create_dir_all(&legacy_home).expect("legacy codex home should be created");
        fs::write(legacy_home.join("marker.txt"), "demo").expect("legacy marker should be written");

        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        set_codez_codex_home().expect("codex home should be set");

        assert_eq!(
            std::env::var_os("CODEX_HOME"),
            Some(new_home.clone().into_os_string())
        );
        assert!(!legacy_home.exists());
        assert_eq!(
            fs::read_to_string(new_home.join("marker.txt")).expect("migrated marker should exist"),
            "demo"
        );

        match old_codex_home {
            Some(value) => unsafe { std::env::set_var("CODEX_HOME", value) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn set_codez_codex_home_migrates_legacy_codez_cli_root_to_cutex() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_codex_home = std::env::var_os("CODEX_HOME");
        let legacy_root = temp_home.join(".codez-cli");
        let new_root = temp_home.join(".cutex");

        fs::create_dir_all(&legacy_root).expect("legacy root should be created");
        fs::write(legacy_root.join("config.json"), "{\"demo\":true}\n")
            .expect("legacy config should be written");

        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        set_codez_codex_home().expect("codex home should be set");

        assert!(!legacy_root.exists());
        assert_eq!(
            fs::read_to_string(new_root.join("config.json")).expect("config should migrate"),
            "{\"demo\":true}\n"
        );

        match old_codex_home {
            Some(value) => unsafe { std::env::set_var("CODEX_HOME", value) },
            None => unsafe { std::env::remove_var("CODEX_HOME") },
        }
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn official_codex_login_scrubs_profile_env_overrides() {
        let mut command = Command::new("cute-codex");
        command.env("CODEX_HOME", "/tmp/cutex-login");
        for key in codex_login_env_override_keys() {
            command.env(key, "/tmp/profile-value");
        }

        scrub_codex_login_env(&mut command);

        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.iter().any(|(key, value)| {
            *key == std::ffi::OsStr::new("CODEX_HOME")
                && value == &Some(std::ffi::OsStr::new("/tmp/cutex-login"))
        }));
        for expected_key in codex_login_env_override_keys() {
            assert!(
                envs.iter().any(|(key, value)| {
                    *key == std::ffi::OsStr::new(expected_key) && value.is_none()
                }),
                "{expected_key} should be explicitly removed for login"
            );
        }
    }

    #[test]
    fn sandbox_user_home_falls_back_to_legacy_runtime_home_when_new_path_missing() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let legacy_runtime_home = temp_home
            .join(".cutex")
            .join("runtime")
            .join("thirdparty")
            .join("userhome");
        let new_runtime_home = temp_home.join(".cutex").join("runtime").join("docker-home");

        fs::create_dir_all(&legacy_runtime_home).expect("legacy runtime home should be created");
        fs::write(legacy_runtime_home.join(".write-test"), "demo")
            .expect("legacy runtime marker should be written");

        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let resolved = sandbox_user_home("demo").expect("runtime home should resolve");

        assert_eq!(resolved, legacy_runtime_home);
        assert!(legacy_runtime_home.exists());
        assert!(!new_runtime_home.exists());

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn materialized_account_files_live_under_codez_profiles_dir() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let account = sample_account("demo");
        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");

        let files =
            ensure_materialized_account_files(&account).expect("account files should materialize");

        assert_eq!(
            files.auth_path,
            temp_home
                .join(".cutex")
                .join("profiles")
                .join("demo-id")
                .join("auth.json")
        );
        assert_eq!(
            files.config_path,
            temp_home
                .join(".cutex")
                .join("profiles")
                .join("demo-id")
                .join("config.toml")
        );
        assert_eq!(
            fs::read_to_string(&files.auth_path).expect("auth should be readable"),
            "{\"demo\":true}\n"
        );
        let config = fs::read_to_string(&files.config_path).expect("config should be readable");
        let config_table = parse_toml_table(&config).expect("config should parse");
        assert_eq!(
            config_table
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("openai")
        );
        assert_eq!(
            config_table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|table| table.get("status_line"))
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                }),
            Some(DEFAULT_CUTEX_STATUS_LINE.map(str::to_string).to_vec())
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn activate_account_preserves_existing_materialized_config() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut store = AccountsStore::default();
        let account = sample_account("demo");
        store.accounts.push(account.clone());
        save_store(&store).expect("store should save");

        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");
        let files =
            ensure_materialized_account_files(&account).expect("account files should materialize");
        let edited = r#"
model_provider = "anthropic"
model_context_window = 1000000
model_auto_compact_token_limit = 400000

[tui]
status_line = ["launch-profile", "current-dir"]
status_line_use_colors = true
"#;
        fs::write(&files.config_path, edited).expect("edited config should be written");

        let activated = activate_account("demo").expect("account should activate");
        let persisted = fs::read_to_string(&files.config_path).expect("config should remain");
        let reloaded = load_store().expect("store should reload");

        let persisted_table =
            parse_toml_table(&persisted).expect("persisted config should parse as TOML");
        let edited_table = parse_toml_table(edited).expect("edited config should parse as TOML");
        assert_eq!(persisted_table, edited_table);
        assert_eq!(
            account_model_provider(&activated).as_deref(),
            Some("anthropic")
        );

        let active_auth = fs::read_to_string(
            host_codex_home_dir()
                .expect("host codex home should resolve")
                .join("auth.json"),
        )
        .expect("active auth should be synced");
        assert_eq!(active_auth, "{\"demo\":true}\n");
        assert!(reloaded
            .accounts
            .iter()
            .any(|candidate| candidate.id == account.id));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn activate_account_syncs_active_codex_home_for_app_server() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut store = AccountsStore::default();
        let account = sample_account("demo");
        store.accounts.push(account.clone());
        save_store(&store).expect("store should save");

        let codex_home = host_codex_home_dir().expect("host codex home should resolve");
        fs::create_dir_all(&codex_home).expect("codex home should be created");
        fs::write(
            codex_home.join("config.toml"),
            r#"
approval_policy = "never"
model_provider = "old"

[model_providers.old]
base_url = "https://old.example.test"
"#,
        )
        .expect("existing shared config should be written");

        write_profile_files(
            &account,
            "{\"profile\":true}\n",
            Some(
                r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://custom.example.test/v1"

[tui]
status_line = ["launch-profile", "current-dir"]
"#,
            ),
        )
        .expect("profile files should be written");

        activate_account("demo").expect("account should activate");

        let active_auth =
            fs::read_to_string(codex_home.join("auth.json")).expect("active auth should sync");
        assert_eq!(active_auth, "{\"profile\":true}\n");

        let active_config =
            fs::read_to_string(codex_home.join("config.toml")).expect("active config should sync");
        let table = parse_toml_table(&active_config).expect("active config should parse");
        assert_eq!(
            table
                .get("approval_policy")
                .and_then(|value| value.as_str()),
            Some("never")
        );
        assert_eq!(
            table.get("model_provider").and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(
            table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get("old"))
                .is_none(),
            "previous active provider should be removed"
        );
        assert!(
            table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get("custom"))
                .is_some(),
            "selected profile provider should be present"
        );
        assert_eq!(
            table
                .get("tui")
                .and_then(|value| value.as_table())
                .and_then(|tui| tui.get("status_line"))
                .and_then(|value| value.as_array())
                .map(|items| items.len()),
            Some(2)
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn save_store_round_trips_profile_default_cli_args() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut store = AccountsStore::default();
        let mut account = sample_account("work");
        account.default_cli_args = vec!["--sandbox".to_string(), "danger-full-access".to_string()];
        store.accounts.push(account);
        save_store(&store).expect("store should save");

        let reloaded = load_store().expect("store should reload");
        assert_eq!(
            reloaded.accounts[0].default_cli_args,
            vec!["--sandbox".to_string(), "danger-full-access".to_string(),]
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_sets_codex_install_dir_for_cute_codex_app_server() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join("bin")).expect("temp bin should be created");
        let cute_codex = temp_home.join("bin").join("cute-codex");
        fs::write(&cute_codex, "#!/usr/bin/env sh\nexit 0\n")
            .expect("fake cute-codex should write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cute_codex, fs::Permissions::from_mode(0o755))
                .expect("fake cute-codex should be executable");
        }
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var("PATH", temp_home.join("bin"));
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "cute-codex");
        }

        let account = sample_account("demo");
        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");

        let launch = codex_launch_command(&account, &[]).expect("launch should build");
        let install_dir = launch
            .envs
            .iter()
            .find_map(|(key, value)| (key == CODEX_INSTALL_DIR_ENV_VAR).then_some(value.clone()))
            .expect("CODEX_INSTALL_DIR should be set");
        let wrapper = PathBuf::from(&install_dir).join("codex");
        let wrapper_contents = fs::read_to_string(&wrapper).expect("codex wrapper should exist");
        assert!(wrapper_contents.contains(cute_codex.to_string_lossy().as_ref()));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_path {
            Some(value) => unsafe { std::env::set_var("PATH", value) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_command_exports_account_file_envs() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/custom-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let account = sample_account("demo");
        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");

        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert_eq!(launch.program, "/tmp/custom-codex");
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == CODEX_AUTH_FILE_ENV_VAR
                && value.ends_with("/.cutex/profiles/demo-id/auth.json")));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == CODEX_CONFIG_FILE_ENV_VAR
                && value.ends_with("/.cutex/profiles/demo-id/config.toml")));

        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_command_includes_global_notify_timeouts() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        let old_notify_idle = std::env::var_os(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_composer = std::env::var_os(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_approval = std::env::var_os(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR);
        let old_notify_startup_idle = std::env::var_os(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_events = std::env::var_os(CODEX_NOTIFY_EVENTS_ENV_VAR);
        let old_notify_content = std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR);
        let old_notify_preview = std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR);
        let old_threshold_warning_mode =
            std::env::var_os(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR);
        let old_model_nudge_mode = std::env::var_os(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_EVENTS_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR);
            std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR);
            std::env::remove_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR);
            std::env::remove_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR);
        }

        let mut config = CodezConfig::default();
        config.notify_service_url = Some("http://127.0.0.1:38765/notify".to_string());
        config.notify_service_token = Some("test-token".to_string());
        config.notify_service_idle_timeout_secs = Some(20);
        config.notify_service_composer_idle_timeout_secs = Some(5);
        config.notify_service_approval_timeout_secs = Some(30);
        config.notify_service_startup_idle_timeout_secs = Some(180);
        config.notify_service_events = Some(vec![
            "task_completed".to_string(),
            "user_message_sent".to_string(),
        ]);
        config.notify_service_user_message_content = Some("preview".to_string());
        config.notify_service_user_message_preview_chars = Some(80);
        config.rate_limit_threshold_warning_mode = Some("daily".to_string());
        config.rate_limit_model_nudge_mode = Some("off".to_string());
        save_codez_config(&config).expect("config should be saved");

        let account = sample_account("notify-timeouts");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");

        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert!(launch.envs.iter().any(|(key, value)| {
            key == "CODEX_NOTIFY_SERVICE_URL" && value == "http://127.0.0.1:38765/notify"
        }));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| { key == "CODEX_NOTIFY_SERVICE_TOKEN" && value == "test-token" }));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| { key == CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR && value == "20" }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR && value == "5"
        }));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| { key == CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR && value == "30" }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR && value == "180"
        }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_NOTIFY_EVENTS_ENV_VAR && value == "task_completed,user_message_sent"
        }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR && value == "preview"
        }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR && value == "80"
        }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR && value == "daily"
        }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR && value == "off"
        }));

        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        match old_notify_idle {
            Some(value) => unsafe { std::env::set_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_composer {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_approval {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR) },
        }
        match old_notify_startup_idle {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_events {
            Some(value) => unsafe { std::env::set_var(CODEX_NOTIFY_EVENTS_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_EVENTS_ENV_VAR) },
        }
        match old_notify_content {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR) },
        }
        match old_notify_preview {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR, value)
            },
            None => unsafe {
                std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR)
            },
        }
        match old_threshold_warning_mode {
            Some(value) => unsafe {
                std::env::set_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR, value)
            },
            None => unsafe {
                std::env::remove_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR)
            },
        }
        match old_model_nudge_mode {
            Some(value) => unsafe {
                std::env::set_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR) },
        }
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_command_keeps_explicit_notify_timeout_env_over_global_config() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        let old_notify_idle = std::env::var_os(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_composer = std::env::var_os(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_approval = std::env::var_os(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR);
        let old_notify_startup_idle = std::env::var_os(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR);
        let old_notify_events = std::env::var_os(CODEX_NOTIFY_EVENTS_ENV_VAR);
        let old_notify_content = std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR);
        let old_notify_preview = std::env::var_os(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR);
        let old_threshold_warning_mode =
            std::env::var_os(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR);
        let old_model_nudge_mode = std::env::var_os(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
            std::env::set_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR, "20");
            std::env::set_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR, "5");
            std::env::set_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR, "30");
            std::env::set_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR, "180");
            std::env::set_var(CODEX_NOTIFY_EVENTS_ENV_VAR, "task_completed");
            std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR, "none");
            std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR, "40");
            std::env::set_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR, "always");
            std::env::set_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR, "daily");
        }

        let mut config = CodezConfig::default();
        config.notify_service_idle_timeout_secs = Some(60);
        config.notify_service_composer_idle_timeout_secs = Some(600);
        config.notify_service_approval_timeout_secs = Some(90);
        config.notify_service_startup_idle_timeout_secs = Some(240);
        config.notify_service_events = Some(vec!["user_message_sent".to_string()]);
        config.notify_service_user_message_content = Some("full".to_string());
        config.notify_service_user_message_preview_chars = Some(200);
        config.rate_limit_threshold_warning_mode = Some("off".to_string());
        config.rate_limit_model_nudge_mode = Some("off".to_string());
        save_codez_config(&config).expect("config should be saved");

        let account = sample_account("notify-timeout-env");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");

        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_EVENTS_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR));
        assert!(!launch
            .envs
            .iter()
            .any(|(key, _)| key == CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR));

        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        match old_notify_idle {
            Some(value) => unsafe { std::env::set_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_composer {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_approval {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR) },
        }
        match old_notify_startup_idle {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR) },
        }
        match old_notify_events {
            Some(value) => unsafe { std::env::set_var(CODEX_NOTIFY_EVENTS_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_EVENTS_ENV_VAR) },
        }
        match old_notify_content {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR) },
        }
        match old_notify_preview {
            Some(value) => unsafe {
                std::env::set_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR, value)
            },
            None => unsafe {
                std::env::remove_var(CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR)
            },
        }
        match old_threshold_warning_mode {
            Some(value) => unsafe {
                std::env::set_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR, value)
            },
            None => unsafe {
                std::env::remove_var(CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR)
            },
        }
        match old_model_nudge_mode {
            Some(value) => unsafe {
                std::env::set_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR, value)
            },
            None => unsafe { std::env::remove_var(CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR) },
        }
        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn docker_runtime_adds_sandbox_bypass_by_default() {
        let mut account = sample_account("docker");
        account.runtime = RuntimeConfig::Docker {
            image: "img".to_string(),
            user_name: Some("user".to_string()),
        };

        let args = codex_args_for_runtime(&account, vec!["resume".to_string()]);

        assert_eq!(
            args,
            vec![
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
                "resume".to_string()
            ]
        );
    }

    #[test]
    fn profile_default_cli_args_are_prepended_before_user_args() {
        let mut account = sample_account("work");
        account.default_cli_args = vec!["--sandbox".to_string(), "danger-full-access".to_string()];

        let args = combined_profile_cli_args(&account, vec!["resume".to_string()]);

        assert_eq!(
            args,
            vec![
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
                "resume".to_string()
            ]
        );
    }

    #[test]
    fn docker_runtime_keeps_profile_default_sandbox_choice() {
        let mut account = sample_account("docker");
        account.runtime = RuntimeConfig::Docker {
            image: "img".to_string(),
            user_name: Some("user".to_string()),
        };
        account.default_cli_args = vec!["--sandbox".to_string(), "danger-full-access".to_string()];

        let args = codex_args_for_runtime(&account, account.default_cli_args.clone());

        assert_eq!(
            args,
            vec!["--sandbox".to_string(), "danger-full-access".to_string()]
        );
    }

    #[test]
    fn docker_runtime_skips_sandbox_bypass_for_non_codex_binary() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        unsafe {
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/sh");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let mut account = sample_account("docker");
        account.runtime = RuntimeConfig::Docker {
            image: "img".to_string(),
            user_name: Some("user".to_string()),
        };

        let args = codex_args_for_runtime(&account, vec!["-lc".to_string(), "env".to_string()]);
        assert_eq!(args, vec!["-lc".to_string(), "env".to_string()]);

        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
    }

    #[test]
    fn docker_runtime_preserves_explicit_sandbox_choice() {
        let mut account = sample_account("docker");
        account.runtime = RuntimeConfig::Docker {
            image: "img".to_string(),
            user_name: Some("user".to_string()),
        };

        let args = codex_args_for_runtime(
            &account,
            vec![
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                "resume".to_string(),
            ],
        );

        assert_eq!(
            args,
            vec![
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                "resume".to_string()
            ]
        );
    }

    #[test]
    fn host_launch_command_includes_profile_and_runtime_envs() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let mut account = sample_account("test-profile");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");
        account.plan_type = Some("plus".to_string());
        account.email = Some("test-profile@example.test".to_string());

        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert_eq!(launch.program, "/tmp/cute-codex");
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_LAUNCH_PROFILE_ENV_VAR && value == "test-profile"
        }));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| { key == CODEX_LAUNCH_RUNTIME_ENV_VAR && value == "host" }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_LAUNCH_PROFILE_SOURCE_ENV_VAR && value == "official"
        }));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| { key == CODEX_LAUNCH_PROFILE_TYPE_ENV_VAR && value == "plus" }));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CODEX_LAUNCH_PROFILE_EMAIL_ENV_VAR && value == "test-profile@example.test"
        }));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_api_key_launch_exports_openai_api_key_from_profile_auth() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let mut account = sample_account("api-key-host");
        account.source = Some("api-key".to_string());
        write_profile_files(
            &account,
            r#"{ "openai_api_key": "sk-host-test", "tokens": null }"#,
            Some(
                r#"
model_provider = "codexapis"

[model_providers.codexapis]
base_url = "https://www.codexapis.com/v1"
env_key = "OPENAI_API_KEY"
requires_openai_auth = false
"#,
            ),
        )
        .expect("profile files should be written");

        let launch = codex_launch_command(&account, &[]).expect("launch should build");

        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "OPENAI_API_KEY" && value == "sk-host-test"));
        let files = materialized_account_files(&account).expect("account files should resolve");
        let config =
            fs::read_to_string(&files.config_path).expect("profile config should be readable");
        let table = parse_toml_table(&config).expect("profile config should parse");
        let provider = table
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get("codexapis"))
            .and_then(|value| value.as_table())
            .expect("codexapis provider should exist");
        assert_eq!(
            provider.get("env_key").and_then(|value| value.as_str()),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            provider
                .get("requires_openai_auth")
                .and_then(|value| value.as_bool()),
            Some(false)
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn docker_api_key_launch_exports_openai_api_key_from_profile_auth() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let mut account = sample_account("api-key-docker");
        account.source = Some("api-key".to_string());
        account.runtime = RuntimeConfig::Docker {
            image: "cutex-dev-v2".to_string(),
            user_name: Some("cutex".to_string()),
        };
        write_profile_files(
            &account,
            r#"{ "OPENAI_API_KEY": "sk-docker-test", "tokens": null }"#,
            Some(
                r#"
model_provider = "codexapis"

[model_providers.codexapis]
base_url = "https://www.codexapis.com/v1"
env_key = "OPENAI_API_KEY"
requires_openai_auth = false
"#,
            ),
        )
        .expect("profile files should be written");

        let launch = codex_launch_command(&account, &[]).expect("launch should build");

        assert!(launch
            .args
            .windows(2)
            .any(|args| { args[0] == "-e" && args[1] == "OPENAI_API_KEY=sk-docker-test" }));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn managed_session_wraps_default_host_launch() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        let old_cutex_alden_bin = std::env::var_os(CUTEX_ALDEN_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
            std::env::set_var(CUTEX_ALDEN_BIN_ENV_VAR, "/tmp/cute-alden");
        }

        let account = sample_account("demo session");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");
        let mut global = CodezConfig::default();
        global.session.enabled = true;
        save_codez_config(&global).expect("global config should be saved");

        let direct = codex_launch_command(&account, &[]).expect("launch should build");
        let wrapped = maybe_wrap_launch_with_session(&account, &[], direct)
            .expect("session wrapping should work");
        let shell_command = wrapped.to_shell_command();

        assert_eq!(wrapped.program, "/tmp/cute-alden");
        assert!(shell_command.contains("'--name'"));
        assert!(shell_command.contains("'--'"));
        assert!(shell_command.contains("'/tmp/cute-codex'"));
        assert!(shell_command.contains("cutex.demo-session.host"));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        match old_cutex_alden_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_ALDEN_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_ALDEN_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn managed_session_skips_launch_when_profile_disables_it() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        let old_cutex_alden_bin = std::env::var_os(CUTEX_ALDEN_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
            std::env::set_var(CUTEX_ALDEN_BIN_ENV_VAR, "/tmp/cute-alden");
        }

        let mut account = sample_account("no-session");
        account.session = Some(SessionConfig { enabled: false });
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");

        let direct = codex_launch_command(&account, &[]).expect("launch should build");
        let wrapped = maybe_wrap_launch_with_session(&account, &[], direct.clone())
            .expect("launch should build");

        assert_eq!(wrapped.program, direct.program);
        assert_eq!(wrapped.args, direct.args);

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        match old_cutex_alden_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_ALDEN_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_ALDEN_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn launch_shell_command_serializes_profile_and_runtime_envs() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        let old_codez_codex_bin = std::env::var_os(CODEZ_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
            std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR);
        }

        let mut account = sample_account("test-profile");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");
        account.plan_type = Some("plus".to_string());
        account.email = Some("test-profile@example.test".to_string());

        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");
        let shell_command = launch.to_shell_command();

        assert!(shell_command.contains("CODEX_LAUNCH_PROFILE='test-profile'"));
        assert!(shell_command.contains("CODEX_LAUNCH_RUNTIME='host'"));
        assert!(shell_command.contains("CODEX_LAUNCH_PROFILE_SOURCE='official'"));
        assert!(shell_command.contains("CODEX_LAUNCH_PROFILE_TYPE='plus'"));
        assert!(shell_command.contains("CODEX_LAUNCH_PROFILE_EMAIL='test-profile@example.test'"));
        assert!(shell_command.contains("'/tmp/cute-codex' 'resume'"));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        match old_codez_codex_bin {
            Some(value) => unsafe { std::env::set_var(CODEZ_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CODEZ_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_command_includes_http_proxy_envs() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
        }
        let mut config = CodezConfig::default();
        config.proxy = Some(
            proxy_config_from_parts(
                true,
                Some("http://127.0.0.1:7890".to_string()),
                Some("localhost,127.0.0.1".to_string()),
                true,
            )
            .expect("proxy config should be valid"),
        );
        save_codez_config(&config).expect("config should be saved");

        let account = sample_account("proxy-http");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");
        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "HTTP_PROXY" && value == "http://127.0.0.1:7890"));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "ALL_PROXY" && value == "http://127.0.0.1:7890"));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "NO_PROXY" && value == "localhost,127.0.0.1"));
        assert!(launch.envs.iter().any(|(key, value)| {
            key == CUTE_CODEX_FORCE_HTTP_TRANSPORT_ENV_VAR && value == "1"
        }));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn host_launch_command_sets_http_and_all_proxy_for_socks_proxy() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        let old_cutex_codex_bin = std::env::var_os(CUTEX_CODEX_BIN_ENV_VAR);
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
            std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, "/tmp/cute-codex");
        }
        let mut config = CodezConfig::default();
        config.proxy = Some(
            proxy_config_from_parts(
                true,
                Some("socks5h://127.0.0.1:7890".to_string()),
                None,
                true,
            )
            .expect("proxy config should be valid"),
        );
        save_codez_config(&config).expect("config should be saved");

        let account = sample_account("proxy-socks");
        write_profile_files(&account, "{\"demo\":true}\n", None)
            .expect("profile files should be written");
        let launch =
            codex_launch_command(&account, &["resume".to_string()]).expect("launch should build");

        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "ALL_PROXY" && value == "socks5h://127.0.0.1:7890"));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "HTTP_PROXY" && value == "socks5h://127.0.0.1:7890"));
        assert!(launch
            .envs
            .iter()
            .any(|(key, value)| key == "HTTPS_PROXY" && value == "socks5h://127.0.0.1:7890"));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        match old_cutex_codex_bin {
            Some(value) => unsafe { std::env::set_var(CUTEX_CODEX_BIN_ENV_VAR, value) },
            None => unsafe { std::env::remove_var(CUTEX_CODEX_BIN_ENV_VAR) },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn docker_runtime_rewrites_loopback_proxy_to_host_alias() {
        let proxy = proxy_config_from_parts(
            true,
            Some("socks5h://127.0.0.1:7891".to_string()),
            Some("localhost,127.0.0.1,::1".to_string()),
            true,
        )
        .expect("proxy config should be valid");
        let envs = proxy_envs(
            Some(&proxy),
            Some(&RuntimeConfig::Docker {
                image: "cutex-dev-v2".to_string(),
                user_name: Some("cutex".to_string()),
            }),
        );

        assert!(envs.iter().any(|(key, value)| {
            key == "ALL_PROXY" && value.starts_with("socks5h://host.docker.internal:7891")
        }));
        assert!(envs.iter().any(|(key, value)| {
            key == "HTTP_PROXY" && value.starts_with("socks5h://host.docker.internal:7891")
        }));
        assert!(envs.iter().any(|(key, value)| {
            key == "HTTPS_PROXY" && value.starts_with("socks5h://host.docker.internal:7891")
        }));
    }

    #[test]
    fn account_proxy_scope_label_reports_profile_vs_global_state() {
        let global = CodezConfig {
            proxy: Some(
                proxy_config_from_parts(
                    true,
                    Some("socks5h://127.0.0.1:7891".to_string()),
                    None,
                    true,
                )
                .expect("global proxy should be valid"),
            ),
            ..CodezConfig::default()
        };

        let mut account = sample_account("scope");
        assert_eq!(account_proxy_scope_label(&account, &global), "on(global)");

        account.proxy = Some(
            proxy_config_from_parts(false, None, None, /*force_http_transport*/ true)
                .expect("disabled proxy should be valid"),
        );
        assert_eq!(account_proxy_scope_label(&account, &global), "off(profile)");

        account.proxy = Some(
            proxy_config_from_parts(true, Some("http://127.0.0.1:8080".to_string()), None, true)
                .expect("profile proxy should be valid"),
        );
        assert_eq!(account_proxy_scope_label(&account, &global), "on(profile)");
    }

    #[test]
    fn account_model_provider_reads_model_provider_from_profile_config() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let account = sample_account("provider");
        write_profile_files(
            &account,
            "{\"demo\":true}\n",
            Some(
                r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://example.test/v1"
"#,
            ),
        )
        .expect("profile files should be written");

        assert_eq!(account_model_provider(&account).as_deref(), Some("custom"));
        assert_eq!(
            account_model_api_base(&account).as_deref(),
            Some("https://example.test/v1")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn api_key_config_toml_includes_named_model_provider() {
        let config = codex_api_key_config_toml("custom", Some("https://api.example.test/v1"));
        let table = parse_toml_table(&config).expect("config should parse");
        assert_eq!(
            table.get("model_provider").and_then(|value| value.as_str()),
            Some("custom")
        );
        let provider = table
            .get("model_providers")
            .and_then(|value| value.as_table())
            .and_then(|providers| providers.get("custom"))
            .and_then(|value| value.as_table())
            .expect("custom provider should exist");
        assert_eq!(
            provider.get("name").and_then(|value| value.as_str()),
            Some("custom")
        );
        assert_eq!(
            provider.get("base_url").and_then(|value| value.as_str()),
            Some("https://api.example.test/v1")
        );
        assert_eq!(
            provider.get("env_key").and_then(|value| value.as_str()),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            provider
                .get("requires_openai_auth")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn account_model_provider_falls_back_to_openai_for_official_auth() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let chatgpt = sample_account("openai-chatgpt-fallback");
        write_profile_files(
            &chatgpt,
            r#"{
  "openai_api_key": null,
  "tokens": { "id_token": "x", "access_token": "y", "refresh_token": "z" }
}"#,
            None,
        )
        .expect("profile files should be written");
        assert_eq!(account_model_provider(&chatgpt).as_deref(), Some("openai"));
        assert_eq!(
            account_model_api_base(&chatgpt).as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn official_openai_base_uses_chatgpt_even_if_materialized_auth_is_stale_api_key() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let account = sample_account("official-stale-auth");
        write_profile_files(
            &account,
            r#"{ "openai_api_key": "sk-stale", "tokens": null }"#,
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");

        assert_eq!(account_model_provider(&account).as_deref(), Some("openai"));
        assert_eq!(
            account_model_api_base(&account).as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn account_model_api_base_falls_back_for_openai_by_auth_mode() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let chatgpt = sample_account("openai-chatgpt");
        write_profile_files(
            &chatgpt,
            r#"{
  "openai_api_key": null,
  "tokens": { "id_token": "x", "access_token": "y", "refresh_token": "z" }
}"#,
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");
        assert_eq!(
            account_model_api_base(&chatgpt).as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );

        let mut api_key = sample_account("openai-api-key");
        api_key.source = Some("api-key".to_string());
        write_profile_files(
            &api_key,
            r#"{ "openai_api_key": "sk-test", "tokens": null }"#,
            Some("model_provider = \"openai\"\n"),
        )
        .expect("profile files should be written");
        assert_eq!(
            account_model_api_base(&api_key).as_deref(),
            Some("https://api.openai.com/v1")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn account_model_api_base_reads_oss_defaults() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("codez-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(&temp_home).expect("temp home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let ollama = sample_account("ollama");
        write_profile_files(
            &ollama,
            "{\"demo\":true}\n",
            Some("model_provider = \"ollama\"\n"),
        )
        .expect("profile files should be written");
        assert_eq!(
            account_model_api_base(&ollama).as_deref(),
            Some("http://localhost:11434/v1")
        );

        let lmstudio = sample_account("lmstudio");
        write_profile_files(
            &lmstudio,
            "{\"demo\":true}\n",
            Some("model_provider = \"lmstudio\"\n"),
        )
        .expect("profile files should be written");
        assert_eq!(
            account_model_api_base(&lmstudio).as_deref(),
            Some("http://localhost:1234/v1")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn load_store_migrates_v2_store_and_materializes_profile_files() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let accounts_path = temp_home.join(".cutex").join("accounts.json");
        let legacy = r#"
{
  "version": 2,
  "accounts": [
    {
      "id": "legacy-id",
      "name": "legacy",
      "email": "legacy@example.test",
      "plan_type": "plus",
      "source": null,
      "raw_auth_json": "{\"openai_api_key\":\"sk-test\",\"tokens\":null}",
      "raw_config_toml": "model_provider = \"openai\"\n",
      "runtime": { "kind": "host" },
      "proxy": null,
      "last_used_at": null
    }
  ],
  "active_account_id": "legacy-id"
}
"#;
        fs::write(&accounts_path, legacy).expect("legacy accounts.json should be written");

        let store = load_store().expect("store should migrate");
        assert_eq!(store.version, STORE_VERSION);
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].name, "legacy");
        assert_eq!(
            temp_home
                .join(".cutex")
                .join("accounts.v2.backup.json")
                .exists(),
            true
        );

        let files = materialized_account_files(&store.accounts[0]).expect("paths should resolve");
        let auth = fs::read_to_string(&files.auth_path).expect("auth should be materialized");
        let config = fs::read_to_string(&files.config_path).expect("config should be materialized");
        let auth_json: serde_json::Value =
            serde_json::from_str(&auth).expect("auth should parse as JSON");
        assert!(
            auth_json.get("OPENAI_API_KEY").is_some() || auth_json.get("openai_api_key").is_some()
        );
        assert!(config.contains("model_provider = \"openai\""));

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn profile_pin_top_and_bottom_reorders_accounts() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut store = AccountsStore::default();
        let alpha = sample_account("alpha");
        let beta = sample_account("beta");
        let gamma = sample_account("gamma");
        store.accounts = vec![alpha.clone(), beta.clone(), gamma.clone()];
        store.active_account_id = Some(beta.id.clone());
        save_store(&store).expect("store should save");

        cmd_profile_pin("gamma", true).expect("pin top should succeed");
        let reloaded = load_store().expect("store should reload");
        assert_eq!(
            reloaded
                .accounts
                .iter()
                .map(|account| account.name.clone())
                .collect::<Vec<_>>(),
            vec!["gamma".to_string(), "alpha".to_string(), "beta".to_string()]
        );

        cmd_profile_pin("gamma", false).expect("pin bottom should succeed");
        let reloaded = load_store().expect("store should reload");
        assert_eq!(
            reloaded
                .accounts
                .iter()
                .map(|account| account.name.clone())
                .collect::<Vec<_>>(),
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn profile_clone_status_line_copies_active_profile_to_all_profiles() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let alpha = sample_account("alpha");
        let beta = sample_account("beta");
        let mut store = AccountsStore::default();
        store.accounts = vec![alpha.clone(), beta.clone()];
        store.active_account_id = Some(alpha.id.clone());
        save_store(&store).expect("store should save");

        write_profile_files(
            &alpha,
            "{\"demo\":true}\n",
            Some(
                r#"
model_provider = "openai"

[tui]
status_line = ["launch-profile", "model-name", "current-dir"]
"#,
            ),
        )
        .expect("alpha files should be written");
        write_profile_files(
            &beta,
            "{\"demo\":true}\n",
            Some(
                r#"
model_provider = "openai"

[tui]
status_line = ["current-dir"]
"#,
            ),
        )
        .expect("beta files should be written");

        cmd_profile_clone_status_line(None).expect("clone should succeed");

        let beta_files = materialized_account_files(&beta).expect("beta paths should resolve");
        let beta_config =
            fs::read_to_string(&beta_files.config_path).expect("beta config should be readable");
        let beta_table = parse_toml_table(&beta_config).expect("beta config should parse");
        let status_line = beta_table
            .get("tui")
            .and_then(|value| value.as_table())
            .and_then(|tui| tui.get("status_line"))
            .and_then(|value| value.as_array())
            .expect("beta status_line should exist")
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert_eq!(
            status_line,
            vec![
                "launch-profile".to_string(),
                "model-name".to_string(),
                "current-dir".to_string()
            ]
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn profile_copy_duplicates_profile_metadata_and_files() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let mut source = sample_account("source");
        source.email = Some("source@example.test".to_string());
        source.plan_type = Some("pro".to_string());
        source.runtime = RuntimeConfig::Docker {
            image: "cutex-dev-v2".to_string(),
            user_name: Some("devuser".to_string()),
        };
        source.proxy = Some(
            proxy_config_from_parts(
                true,
                Some("socks5h://127.0.0.1:7891".to_string()),
                Some("localhost,127.0.0.1,::1".to_string()),
                true,
            )
            .expect("proxy config should be valid"),
        );

        let mut store = AccountsStore::default();
        store.accounts.push(source.clone());
        store.active_account_id = Some(source.id.clone());
        save_store(&store).expect("store should save");

        let source_config = r#"
model_provider = "custom"

[tui]
status_line = ["launch-profile", "current-dir"]

[model_providers.custom]
base_url = "https://old.example/v1"
"#;
        write_profile_files(&source, "{\"demo\":true}\n", Some(source_config))
            .expect("source files should be written");

        cmd_profile_copy("source", "copied", None, None).expect("copy should succeed");

        let reloaded = load_store().expect("store should reload");
        assert_eq!(reloaded.accounts.len(), 2);
        assert_eq!(reloaded.accounts[1].name, "copied");
        assert_eq!(
            reloaded.accounts[1].email.as_deref(),
            Some("source@example.test")
        );
        assert_eq!(reloaded.accounts[1].plan_type.as_deref(), Some("pro"));
        assert_eq!(reloaded.accounts[1].runtime, source.runtime);
        assert_eq!(reloaded.accounts[1].proxy, source.proxy);

        let source_files =
            materialized_account_files(&source).expect("source files should resolve");
        let copied_files =
            materialized_account_files(&reloaded.accounts[1]).expect("copied files should resolve");
        assert_eq!(
            fs::read_to_string(&copied_files.auth_path).expect("copied auth should be readable"),
            fs::read_to_string(&source_files.auth_path).expect("source auth should be readable")
        );
        assert_eq!(
            fs::read_to_string(&copied_files.config_path)
                .expect("copied config should be readable"),
            fs::read_to_string(&source_files.config_path)
                .expect("source config should be readable")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn profile_copy_can_override_provider_base_url_for_same_provider() {
        let _guard = env_lock().lock().expect("env lock should not be poisoned");
        let temp_home = std::env::temp_dir().join(format!("cutex-home-{}", Uuid::new_v4()));
        let old_home = std::env::var_os("HOME");
        fs::create_dir_all(temp_home.join(".cutex")).expect("temp cutex home should be created");
        unsafe {
            std::env::set_var("HOME", &temp_home);
        }

        let source = sample_account("source");
        let mut store = AccountsStore::default();
        store.accounts.push(source.clone());
        save_store(&store).expect("store should save");

        let source_config = r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://old.example/v1"
"#;
        write_profile_files(&source, "{\"demo\":true}\n", Some(source_config))
            .expect("source files should be written");

        cmd_profile_copy(
            "source",
            "copied",
            None,
            Some("https://new.example/v1".to_string()),
        )
        .expect("copy should succeed");

        let reloaded = load_store().expect("store should reload");
        let copied = reloaded
            .accounts
            .iter()
            .find(|account| account.name == "copied")
            .expect("copied profile should exist");
        let copied_files = materialized_account_files(copied).expect("copied files should resolve");
        let copied_config = fs::read_to_string(&copied_files.config_path)
            .expect("copied config should be readable");
        let copied_table = parse_toml_table(&copied_config).expect("copied config should parse");
        assert_eq!(
            copied_table
                .get("model_provider")
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert_eq!(
            copied_table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get("custom"))
                .and_then(|value| value.as_table())
                .and_then(|provider| provider.get("name"))
                .and_then(|value| value.as_str()),
            Some("custom")
        );
        assert_eq!(
            copied_table
                .get("model_providers")
                .and_then(|value| value.as_table())
                .and_then(|providers| providers.get("custom"))
                .and_then(|value| value.as_table())
                .and_then(|provider| provider.get("base_url"))
                .and_then(|value| value.as_str()),
            Some("https://new.example/v1")
        );

        match old_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(temp_home);
    }

    #[test]
    fn apply_annotation_updates_and_clears_display_fields() {
        let mut account = sample_account("annotated");
        account.plan_type = Some("unknown".to_string());
        account.email = Some("-".to_string());

        apply_annotation(
            &mut account,
            Some("api".to_string()),
            false,
            Some("target.example".to_string()),
            false,
            Some("portal".to_string()),
            false,
        );

        assert_eq!(account.source.as_deref(), Some("api"));
        assert_eq!(account.plan_type.as_deref(), Some("target.example"));
        assert_eq!(account.email.as_deref(), Some("portal"));

        apply_annotation(&mut account, None, true, None, true, None, true);

        assert!(account.source.is_none());
        assert!(account.plan_type.is_none());
        assert!(account.email.is_none());
    }
}
