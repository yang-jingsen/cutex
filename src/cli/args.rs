//! Clap command-line argument shape for cutex.

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

/// cutex - profile launcher for cute-codex
#[derive(Parser, Debug)]
#[command(
    name = "cutex",
    about = "Interactive session manager and profile launcher for cute-codex",
    version,
    subcommand_required = false,
    arg_required_else_help = false,
    after_help = "CLI selection order: CUTEX_CODEX_BIN / CODEZ_CODEX_BIN override, then cute-codex, then cutex-codex, then codex."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CommandKind>,

    /// Use the default profile without interactive selection when
    /// running without an explicit subcommand.
    #[arg(short = 'q', long = "quick")]
    pub quick: bool,

    /// Force this invocation to run the selected CLI on the host, even for Docker profiles.
    #[arg(long = "host")]
    pub host: bool,

    /// Enable cutex inter-agent collaboration for this launch.
    #[arg(long = "agent", visible_alias = "collab")]
    pub agent: bool,

    /// Collaboration group(s) for this agent launch. Implies --agent.
    #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
    pub groups: Vec<String>,

    /// When no subcommand is provided, any remaining arguments are
    /// passed through to the selected CLI invocation.
    #[arg(last = true, value_name = "CLI_ARGS")]
    pub codex_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum CommandKind {
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
        /// Enable cutex inter-agent collaboration for this launch.
        #[arg(long = "agent", visible_alias = "collab")]
        agent: bool,
        /// Collaboration group(s) for this agent launch. Implies --agent.
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
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

    /// Open the agent TUI; list filters retain the compatibility start picker
    Start {
        #[command(flatten)]
        list: SessionListArgs,
    },

    /// Open the continuous agent selector
    Tui,

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

    /// Report objective per-agent token usage observed by Cutex
    Usage {
        /// Time bucket used for report rows
        #[arg(long, value_enum, default_value = "day")]
        period: UsagePeriodArg,
        /// Dimension used within each time bucket
        #[arg(long, value_enum, default_value = "agent")]
        group_by: UsageGroupByArg,
        /// Inclusive lower bound as RFC3339 or YYYY-MM-DD (UTC)
        #[arg(long, value_name = "TIME", conflicts_with = "last")]
        since: Option<String>,
        /// Exclusive upper bound as RFC3339 or YYYY-MM-DD (UTC)
        #[arg(long, value_name = "TIME")]
        until: Option<String>,
        /// Relative range ending at --until or now, for example 24h, 7d, or 8w
        #[arg(long, value_name = "DURATION", conflicts_with = "since")]
        last: Option<String>,
        /// Reset window used when --period reset
        #[arg(long, value_enum, default_value = "primary")]
        reset_window: UsageResetWindowArg,
        /// Emit the exact report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Configure proxy settings (legacy compatibility command)
    #[command(hide = true)]
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },

    /// Manage durable cutex sessions and cute-alden attachments
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

    /// Manage IM/workbench registration for coding sessions
    Im {
        #[command(subcommand)]
        command: ImCommand,
    },

    /// Serve backend-facing coding session management APIs
    Management {
        #[command(subcommand)]
        command: ManagementCommand,
    },

    /// List and message cutex-launched agents
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },

    /// Open the main interactive configuration wizard
    #[command(visible_alias = "config")]
    Wizard,
}

#[derive(Subcommand, Debug)]
pub enum GlobalCommand {
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
        /// Enable or disable the local inter-agent message bus
        #[arg(long = "agent-bus-enable", value_name = "BOOL")]
        agent_bus_enable: Option<bool>,
        /// Fixed local port for the inter-agent message bus
        #[arg(long = "agent-bus-port", value_name = "PORT")]
        agent_bus_port: Option<u16>,
        /// Shared token for the inter-agent message bus; pass '-' to clear
        #[arg(long = "agent-bus-token", value_name = "TOKEN")]
        agent_bus_token: Option<String>,
        /// Prefix template applied to delivered agent messages; use {from}/{to}
        #[arg(long = "agent-message-prefix", value_name = "TEMPLATE")]
        agent_message_prefix: Option<String>,
        /// Suffix template applied to delivered agent messages; use {from}/{to}
        #[arg(long = "agent-message-suffix", value_name = "TEMPLATE")]
        agent_message_suffix: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProxyCommand {
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

#[derive(Args, Debug, Clone)]
pub struct SessionListArgs {
    /// Show every durable session, including historical local-only sessions
    #[arg(long)]
    pub all: bool,

    /// Include only offline durable sessions
    #[arg(long)]
    pub offline: bool,

    /// Include only non-persistent local/ephemeral durable sessions
    #[arg(long = "one-shot")]
    pub one_shot: bool,

    /// Include only host-backed durable sessions
    #[arg(long)]
    pub host: bool,

    /// Only show cute-alden runtime sessions
    #[arg(long)]
    pub alden: bool,

    /// Include only attachable durable sessions
    #[arg(long)]
    pub attachable: bool,

    /// Filter by project/cwd/name/id text
    #[arg(long = "project", value_name = "TEXT", action = ArgAction::Append)]
    pub projects: Vec<String>,

    /// Filter by session group
    #[arg(long = "group", value_name = "GROUP", action = ArgAction::Append)]
    pub groups: Vec<String>,

    /// Sort durable sessions
    #[arg(long, value_enum, default_value = "status")]
    pub sort: SessionListSort,
}

impl Default for SessionListArgs {
    fn default() -> Self {
        Self {
            all: false,
            offline: false,
            one_shot: false,
            host: false,
            alden: false,
            attachable: false,
            projects: Vec::new(),
            groups: Vec::new(),
            sort: SessionListSort::Status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SessionListSort {
    Status,
    Recent,
    Name,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UsagePeriodArg {
    Total,
    Hour,
    Day,
    Week,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UsageGroupByArg {
    Agent,
    Profile,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UsageResetWindowArg {
    Primary,
    Secondary,
}

#[derive(Subcommand, Debug)]
pub enum SessionCommand {
    /// Open the interactive session management wizard
    #[command(visible_alias = "edit")]
    Wizard {
        #[command(flatten)]
        list: SessionListArgs,
    },

    /// List durable cutex sessions and known cute-alden sessions
    List {
        #[command(flatten)]
        list: SessionListArgs,
    },

    /// Show a durable cutex session by cutex_session_id or Codex session id
    Show {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// List retired durable cutex sessions
    Retired {
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Retire a managed session after its runtime is safely offline
    Retire {
        /// cutex_session_id or Codex session id
        id: String,
        /// Optional audit reason
        #[arg(long)]
        reason: Option<String>,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Restore a retired session as active and offline without launching it
    Restore {
        /// cutex_session_id or Codex session id
        id: String,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Adopt a recent/local Codex session into durable cutex management
    Adopt {
        /// cutex_session_id, Codex session id, or unique thread/display name
        id: String,
        /// Display name hint for cutex/IM surfaces
        #[arg(long)]
        name: Option<String>,
        /// Managed launch cwd override
        #[arg(long = "cwd", conflicts_with = "current_cwd")]
        cwd: Option<String>,
        /// Set managed launch cwd to the shell's current directory
        #[arg(long = "current-cwd")]
        current_cwd: bool,
        /// Collaboration group(s) to persist
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
        /// Also expose this managed session to the IM/backend workbench
        #[arg(long = "im")]
        expose_to_im: bool,
        /// Pin this session on the start screen
        #[arg(long)]
        pin: bool,
    },

    /// Expose a Codex session to the management/IM backend
    Expose {
        /// cutex_session_id or Codex session id
        id: String,
        /// Display name hint for frontends
        #[arg(long)]
        name: Option<String>,
        /// Replace groups on the exposed session
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
    },

    /// Hide a session from the management/IM backend without deleting runtime metadata
    Hide {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Remove cutex management metadata without deleting history or killing runtime
    Unmanage {
        /// cutex_session_id, Codex session id, or unique thread/display name
        id: String,
    },

    /// Control whether a session appears on the start quick-action screen
    Quick {
        #[command(subcommand)]
        command: SessionQuickCommand,
    },

    /// Edit durable session groups
    Groups {
        #[command(subcommand)]
        command: SessionGroupsCommand,
    },

    /// Set or clear the durable profile used for future managed launches
    Profile {
        #[command(subcommand)]
        command: SessionProfileCommand,
    },

    /// Edit durable session runtime defaults
    Defaults {
        #[command(subcommand)]
        command: SessionDefaultsCommand,
    },

    /// Edit the cutex-managed launch cwd for a session
    Cwd {
        #[command(subcommand)]
        command: SessionCwdCommand,
    },

    /// Bring a managed session online
    Online {
        /// cutex_session_id or Codex session id
        id: String,
        /// Use a profile for processes launched by this action only
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },

    /// Resume a managed session in this visible terminal
    Foreground {
        /// cutex_session_id or Codex session id
        id: String,
        /// Use a profile for this visible TUI launch only
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
    },

    /// Bring a managed session offline
    Offline {
        /// cutex_session_id or Codex session id
        id: String,
        /// Force stop where supported
        #[arg(long)]
        force: bool,
    },

    /// Close a managed session runtime
    Close {
        /// cutex_session_id or Codex session id
        id: String,
        /// Force close where supported
        #[arg(long)]
        force: bool,
    },

    /// Attach to an existing named cute-alden session
    Attach {
        /// Session name
        #[arg(long)]
        name: String,
        /// Detach any active cute-alden client first, then attach here
        #[arg(long)]
        takeover: bool,
    },

    /// Take over an attachable cute-alden runtime by cutex/Codex session id
    Takeover {
        /// cutex_session_id, Codex session id, or live cute-alden session name
        id: String,
    },

    /// Check whether resuming a Codex session would duplicate a live runtime
    DuplicateCheck {
        /// cutex_session_id or Codex session id
        id: String,
        /// Print machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionQuickCommand {
    /// Always recommend this session on the start screen
    Pin {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Never recommend this session on the start screen
    Hide {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Let cutex recommend this session only from cwd or explicit user history
    Auto {
        /// cutex_session_id or Codex session id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionDefaultsCommand {
    /// Show runtime defaults for one session
    Show {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Update runtime defaults for one session
    Set {
        /// cutex_session_id or Codex session id
        id: String,
        /// Runtime backend: host, native, docker, cute-alden, future
        #[arg(long = "runtime-backend")]
        runtime_backend: Option<String>,
        /// Permission preset such as read-only, workspace, or full-access
        #[arg(long = "permission")]
        permission_defaults: Option<String>,
        /// Approval policy such as on-request or never
        #[arg(long = "approval-policy")]
        approval_policy: Option<String>,
        /// Sandbox mode such as workspace-write or danger-full-access
        #[arg(long = "sandbox")]
        sandbox_mode: Option<String>,
        /// Default model override
        #[arg(long)]
        model: Option<String>,
        /// Default reasoning effort
        #[arg(long)]
        reasoning: Option<String>,
        /// Replace extra cute-codex CLI args; repeat for multiple args
        #[arg(long = "cli-arg", value_name = "ARG", action = ArgAction::Append, allow_hyphen_values = true)]
        cli_args: Vec<String>,
        /// Clear all extra cute-codex CLI args
        #[arg(long = "clear-cli-args")]
        clear_cli_args: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionCwdCommand {
    /// Show session cwd, current cwd, and managed cwd
    Show {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Set the managed launch cwd
    Set {
        /// cutex_session_id or Codex session id
        id: String,
        /// Path to use for future managed launches
        path: String,
    },

    /// Set the managed launch cwd to the shell's current directory
    Current {
        /// cutex_session_id or Codex session id
        id: String,
    },

    /// Clear the managed cwd and fall back to the session cwd
    Clear {
        /// cutex_session_id or Codex session id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionGroupsCommand {
    /// Replace all groups
    Set {
        /// cutex_session_id or Codex session id
        id: String,
        /// Group to set
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
    },

    /// Add groups
    Add {
        /// cutex_session_id or Codex session id
        id: String,
        /// Group to add
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
    },

    /// Remove groups
    Remove {
        /// cutex_session_id or Codex session id
        id: String,
        /// Group to remove
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SessionProfileCommand {
    /// Set the named profile as durable configured intent for a session
    Set {
        /// cutex_session_id or Codex session id
        id: String,
        /// Existing profile name or id on the target session's host
        profile: String,
    },
    /// Clear durable configured intent and follow the global default next launch
    Clear {
        /// cutex_session_id or Codex session id
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// List currently registered cutex agents
    List {
        /// Only show agents in this group
        #[arg(long = "group")]
        group: Vec<String>,
        /// Show all groups even when running inside an agent
        #[arg(long)]
        all_groups: bool,
        /// Also query peer hosts discovered through Bridgeboard
        #[arg(long)]
        all_hosts: bool,
    },

    /// Send a message to another cutex agent
    Send {
        /// Runtime endpoint ID, durable cutex session ID, or unique display/thread name
        target: String,
        /// Message to deliver
        message: String,
        /// Stable external identifier for idempotent message delivery
        #[arg(
            long,
            value_name = "NONEMPTY_ID",
            value_parser = parse_external_message_id
        )]
        external_message_id: Option<String>,
        /// Resolve the target across all registered groups
        #[arg(long)]
        all_groups: bool,
        /// Deliver passively without asking the target to process it
        #[arg(long = "queue-only")]
        queue_only: bool,
        /// Ask the target to process the message as soon as possible
        #[arg(long)]
        soon: bool,
        /// Request an explicit interruption of the target's current work
        #[arg(long)]
        interrupt: bool,
        /// Override the sender label
        #[arg(long)]
        from: Option<String>,
    },

    /// Submit one strict local Task Service action, query, or reconciliation document
    TaskAction {
        /// Private JSON request document, or '-' for stdin
        #[arg(long, value_name = "PATH")]
        request_file: Option<String>,
    },

    /// Request the mechanical Release-only rotation from the current Director runtime
    ReleaseRotation {
        /// Strict Release rotation JSON request, or '-' for stdin
        #[arg(long, value_name = "PATH")]
        request_file: Option<String>,
    },

    /// Call the project-scoped Agent Management service as the current Agent
    Manage {
        #[command(subcommand)]
        command: AgentManagementCliCommand,
    },

    /// Show agent bus config and health
    Status,

    /// Show recent local agent-bus audit events
    Log {
        /// Only show records related to this agent id/name
        #[arg(long)]
        agent: Option<String>,
        /// Maximum records to print
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Print raw JSONL records
        #[arg(long)]
        json: bool,
    },

    /// Change live agent collaboration groups
    Groups {
        #[command(subcommand)]
        command: AgentGroupsCommand,
    },

    /// Legacy: point this machine at another host's bus through Bridgeboard
    RemoteUp {
        /// Bus owner host, such as host-a
        host: String,
        /// Bridgeboard service id to use; defaults to cutex-agent-bus
        #[arg(long)]
        service_id: Option<String>,
        /// Local forwarded port to configure cutex against
        #[arg(long)]
        local_port: Option<u16>,
        /// Remote bus port on the owner host
        #[arg(long)]
        remote_port: Option<u16>,
        /// Shared bus token; omit to keep the current configured token
        #[arg(long)]
        token: Option<String>,
        /// Show fallback SSH command even if Bridgeboard succeeds
        #[arg(long)]
        show_ssh_fallback: bool,
        /// Do not update local cutex config
        #[arg(long)]
        no_config: bool,
    },

    /// Run the shared agent bus in the foreground
    #[command(hide = true)]
    Serve {
        /// Port to bind on 127.0.0.1
        #[arg(long)]
        port: Option<u16>,
        /// Bearer token accepted by the bus
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentManagementCliCommand {
    Create {
        #[arg(long)]
        request_file: String,
    },
    QueryManaged {
        #[arg(long)]
        request_file: String,
    },
    Online {
        #[arg(long)]
        request_file: String,
    },
    Offline {
        #[arg(long)]
        request_file: String,
    },
    Restart {
        #[arg(long)]
        request_file: String,
    },
    Close {
        #[arg(long)]
        request_file: String,
    },
    Replace {
        #[arg(long)]
        request_file: String,
    },
    DirectorRotate {
        #[arg(long)]
        request_file: String,
    },
}

fn parse_external_message_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("external message ID cannot be empty or whitespace-only".to_string());
    }
    Ok(value.to_string())
}

#[derive(Subcommand, Debug)]
pub enum AgentGroupsCommand {
    /// Replace groups for a live agent by id, name, or session id
    Set {
        /// Agent id/name or session id
        target: String,
        /// New collaboration groups
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },

    /// Add groups to a live agent by id, name, or session id
    Add {
        /// Agent id/name or session id
        target: String,
        /// Groups to add
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },

    /// Remove groups from a live agent by id, name, or session id
    Remove {
        /// Agent id/name or session id
        target: String,
        /// Groups to remove
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImCommand {
    /// Register or update a coding session in the IM/workbench registry
    Register {
        /// Codex/cute-codex session id
        session_id: String,
        /// Display name shown by IM/workbench
        #[arg(long)]
        name: Option<String>,
        /// Host where this session belongs
        #[arg(long)]
        host: Option<String>,
        /// Working directory for this session
        #[arg(long)]
        cwd: Option<String>,
        /// cutex profile for this session
        #[arg(long)]
        profile: Option<String>,
        /// Group(s) assigned to this session registration
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
        /// Register as temporary/ephemeral instead of persistent
        #[arg(long)]
        temporary: bool,
    },

    /// Register the current cutex-launched agent session in the IM/workbench registry
    RegisterCurrent {
        /// Display name shown by IM/workbench; defaults to the live agent base name
        #[arg(long)]
        name: Option<String>,
        /// Group(s) assigned to this session registration
        #[arg(long = "group", value_name = "GROUP", num_args = 1.., action = ArgAction::Append)]
        groups: Vec<String>,
        /// Register as temporary/ephemeral instead of persistent
        #[arg(long)]
        temporary: bool,
    },

    /// Hide/unmanage a coding session in the IM/workbench registry
    Unregister {
        /// Codex/cute-codex session id
        session_id: String,
    },

    /// Hide/unmanage the current cutex-launched agent session in the IM/workbench registry
    UnregisterCurrent,

    /// List registered IM/workbench coding sessions
    List,

    /// Show one registered IM/workbench coding session
    Show {
        /// Codex/cute-codex session id
        session_id: String,
    },

    /// Show current live agent/session registration diagnostics
    StatusCurrent,

    /// Change registered session groups
    Groups {
        #[command(subcommand)]
        command: ImGroupsCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ImGroupsCommand {
    /// Replace registered groups for a coding session
    Set {
        /// Codex/cute-codex session id
        session_id: String,
        /// New groups
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },

    /// Add registered groups for a coding session
    Add {
        /// Codex/cute-codex session id
        session_id: String,
        /// Groups to add
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },

    /// Remove registered groups for a coding session
    Remove {
        /// Codex/cute-codex session id
        session_id: String,
        /// Groups to remove
        #[arg(value_name = "GROUP", required = true)]
        groups: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ManagementCommand {
    /// Run the local HTTP management API in the foreground
    Serve {
        /// Port to bind on 127.0.0.1
        #[arg(long)]
        port: Option<u16>,
        /// Address to bind, for example 127.0.0.1 or a Tailscale IPv4 address
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        /// Management bearer; a value distinct from Agent Bus enables seat administration
        #[arg(long)]
        token: Option<String>,
    },

    /// Connect this machine to another host's management API through Bridgeboard
    RemoteUp {
        /// Management API owner host, such as host-b
        host: String,
        /// Bridgeboard service id to use; defaults to cutex-management-api
        #[arg(long)]
        service_id: Option<String>,
        /// Local forwarded port used by this host
        #[arg(long)]
        local_port: Option<u16>,
        /// Remote management API port on the owner host
        #[arg(long)]
        remote_port: Option<u16>,
        /// Management bearer override; omit to use configured credential selection
        #[arg(long)]
        token: Option<String>,
        /// Show fallback SSH command even if Bridgeboard succeeds
        #[arg(long)]
        show_ssh_fallback: bool,
    },

    /// Administer durable Task Service logical-seat occupancy
    Seat {
        #[command(subcommand)]
        command: ManagementSeatCommand,
    },

    /// Configure, query, or explicitly continue mechanical Release rotation
    ReleaseRotation {
        #[command(subcommand)]
        command: ManagementReleaseRotationCommand,
    },

    /// Initialize or explicitly correct one project's authorized Director
    AgentAuthority {
        /// Strict project authority JSON request
        #[arg(long)]
        request_file: String,
        /// Local Management API port
        #[arg(long)]
        port: Option<u16>,
        /// Dedicated Management bearer override
        #[arg(long)]
        token: Option<String>,
    },

    /// Import one missing ownership record for the already-authorized legacy Director
    AgentOwnershipImport {
        /// Strict legacy Director ownership import JSON request
        #[arg(long)]
        request_file: String,
        /// Local Management API port
        #[arg(long)]
        port: Option<u16>,
        /// Dedicated Management bearer override
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ManagementReleaseRotationCommand {
    /// Configure the next strict versioned Release template from JSON
    TemplateSet {
        #[arg(long, value_name = "PATH")]
        request_file: String,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Query the current Release template
    TemplateQuery {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Query durable Release rotation receipts
    Query {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Explicitly continue one blocked exact rotation record
    Retry {
        #[arg(long)]
        action_id: String,
        #[arg(long)]
        expected_request_sha256: String,
        /// Exact completed boundary reported by the durable rotation receipt
        #[arg(long, value_enum)]
        expected_completed_boundary: ManagementReleaseRotationBoundaryArg,
        /// Exact outcome-unknown external step reported after provider restart
        #[arg(long, value_enum)]
        expected_pending_external_step: Option<ManagementReleaseRotationExternalStepArg>,
        /// Exact already-created durable successor for an outcome-unknown create boundary
        #[arg(long)]
        corrected_successor_cutex_session: Option<String>,
        /// Exact already-persisted native thread for an outcome-unknown thread/start boundary
        #[arg(long)]
        corrected_successor_thread_id: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ManagementReleaseRotationBoundaryArg {
    SeatRevoked,
    PredecessorOfflined,
    PredecessorRetired,
    SuccessorSessionCreated,
    SuccessorThreadStarted,
    SuccessorRuntimeOnline,
    SuccessorBound,
    DirectorMessageDelivered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ManagementReleaseRotationExternalStepArg {
    OfflinePredecessor,
    RetirePredecessor,
    CreateSuccessorSession,
    StartSuccessorThread,
    LaunchSuccessorRuntime,
    DeliverDirectorMessage,
}

#[derive(Subcommand, Debug)]
pub enum ManagementSeatCommand {
    /// Bind or rebind one logical seat to a durable Cutex session
    Bind {
        /// Idempotency key for this administrative mutation
        #[arg(long)]
        action_id: String,
        /// Logical seat, normally cutex-director or cutex-release
        #[arg(long)]
        seat_id: String,
        /// Durable Cutex session that will occupy the seat
        #[arg(long)]
        occupant_cutex_session: String,
        /// Local Management API port
        #[arg(long)]
        port: Option<u16>,
        /// Dedicated Management bearer override; omit to use the configured root
        #[arg(long)]
        token: Option<String>,
    },

    /// Query current logical-seat occupancy
    Query {
        /// Local Management API port
        #[arg(long)]
        port: Option<u16>,
        /// Dedicated Management bearer override; omit to use the configured root
        #[arg(long)]
        token: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum NotifyCommand {
    /// Manage the native desktop notification bridge
    Desktop {
        #[command(subcommand)]
        command: DesktopNotifyCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum DesktopNotifyCommand {
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
pub enum ProfileCommand {
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
        /// Name exposed through `cutex agent list`
        #[arg(long = "agent-name", conflicts_with = "clear_agent_name")]
        agent_name: Option<String>,
        /// Clear the stored agent name and use the profile name
        #[arg(long = "clear-agent-name", conflicts_with = "agent_name")]
        clear_agent_name: bool,
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
