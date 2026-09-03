use cutex::config::global_settings::ConfigValueUpdate;
use cutex::config::proxy::proxy_config_from_parts;
use cutex::launch::docker::{default_docker_user_name, normalize_docker_user_name};
use cutex::profiles::codex_profile::{
    is_builtin_model_provider, CodexProfileConfigPatch, CodexProfileConfigSnapshot,
    DEFAULT_REQUEST_MAX_RETRIES, DEFAULT_STREAM_IDLE_TIMEOUT_MS, DEFAULT_STREAM_MAX_RETRIES,
    DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS, MAX_PROVIDER_RETRIES,
};
use cutex::profiles::deepseek;
use cutex::profiles::model::{ProxyConfig, RuntimeConfig, SessionConfig};

use super::account_store::ProfileCatalogEntry;
use super::profile_settings::{ProfileApiKeyUpdate, ProfileSettingsPatch};
use super::prompt::{cli_args_label, parse_cli_args_value};
use super::session_tui_settings::{
    SecretSettingsAction, SessionSettingsChoice, SessionSettingsEditorKind,
    SessionTuiSettingCategory, SessionTuiSettingOption,
};

const DEFAULT_DOCKER_IMAGE: &str = "cutex-base";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileSettingsField {
    Name,
    AgentName,
    ApiKey,
    Runtime,
    DockerImage,
    DockerUser,
    ExtraCliArgs,
    ProxyMode,
    ProxyUrl,
    ProxyNoProxy,
    ProxyForceHttpTransport,
    ManagedSessions,
    DeepSeekPreset,
    Model,
    ModelReasoningEffort,
    ModelCatalogJson,
    ForcedLoginMethod,
    ModelProvider,
    ProviderName,
    ProviderBaseUrl,
    ProviderEnvKey,
    ProviderWireApi,
    RequestMaxRetries,
    StreamMaxRetries,
    StreamIdleTimeoutMs,
    WebsocketConnectTimeoutMs,
    ProviderRequiresOpenAiAuth,
    ProviderSupportsWebsockets,
}

impl ProfileSettingsField {
    pub(super) fn editor_kind(self) -> SessionSettingsEditorKind {
        match self {
            Self::ApiKey => SessionSettingsEditorKind::Secret,
            Self::Runtime
            | Self::ProxyMode
            | Self::ProxyForceHttpTransport
            | Self::ManagedSessions
            | Self::DeepSeekPreset
            | Self::ModelReasoningEffort
            | Self::ForcedLoginMethod
            | Self::ProviderWireApi
            | Self::ProviderRequiresOpenAiAuth
            | Self::ProviderSupportsWebsockets => SessionSettingsEditorKind::Choice,
            Self::Name
            | Self::AgentName
            | Self::DockerImage
            | Self::DockerUser
            | Self::ExtraCliArgs
            | Self::ProxyUrl
            | Self::ProxyNoProxy
            | Self::Model
            | Self::ModelCatalogJson
            | Self::ModelProvider
            | Self::ProviderName
            | Self::ProviderBaseUrl
            | Self::ProviderEnvKey
            | Self::RequestMaxRetries
            | Self::StreamMaxRetries
            | Self::StreamIdleTimeoutMs
            | Self::WebsocketConnectTimeoutMs => SessionSettingsEditorKind::Text,
        }
    }
}

const PROFILE_SETTINGS_FIELDS: &[ProfileSettingsField] = &[
    ProfileSettingsField::Name,
    ProfileSettingsField::AgentName,
    ProfileSettingsField::ApiKey,
    ProfileSettingsField::Runtime,
    ProfileSettingsField::DockerImage,
    ProfileSettingsField::DockerUser,
    ProfileSettingsField::ExtraCliArgs,
    ProfileSettingsField::ProxyMode,
    ProfileSettingsField::ProxyUrl,
    ProfileSettingsField::ProxyNoProxy,
    ProfileSettingsField::ProxyForceHttpTransport,
    ProfileSettingsField::ManagedSessions,
    ProfileSettingsField::DeepSeekPreset,
    ProfileSettingsField::Model,
    ProfileSettingsField::ModelReasoningEffort,
    ProfileSettingsField::ModelCatalogJson,
    ProfileSettingsField::ForcedLoginMethod,
    ProfileSettingsField::ModelProvider,
    ProfileSettingsField::ProviderName,
    ProfileSettingsField::ProviderBaseUrl,
    ProfileSettingsField::ProviderEnvKey,
    ProfileSettingsField::ProviderWireApi,
    ProfileSettingsField::RequestMaxRetries,
    ProfileSettingsField::StreamMaxRetries,
    ProfileSettingsField::StreamIdleTimeoutMs,
    ProfileSettingsField::WebsocketConnectTimeoutMs,
    ProfileSettingsField::ProviderRequiresOpenAiAuth,
    ProfileSettingsField::ProviderSupportsWebsockets,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileRuntimeKind {
    Host,
    Docker,
}

impl ProfileRuntimeKind {
    fn value(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Docker => "docker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileProxyMode {
    Inherit,
    Disabled,
    Enabled,
}

impl ProfileProxyMode {
    fn value(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileSessionMode {
    Inherit,
    Enabled,
    Disabled,
}

impl ProfileSessionMode {
    fn value(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProfileSettingsSnapshot {
    name: String,
    active: bool,
    cli_kind: String,
    source: Option<String>,
    plan_type: Option<String>,
    email: Option<String>,
    runtime: RuntimeConfig,
    proxy: Option<ProxyConfig>,
    session: Option<SessionConfig>,
    default_cli_args: Vec<String>,
    agent_name: Option<String>,
    api_key_configured: bool,
    codex_config: Option<CodexProfileConfigSnapshot>,
    codex_config_error: Option<String>,
}

impl ProfileSettingsSnapshot {
    pub(super) fn from_catalog_entry(entry: &ProfileCatalogEntry) -> Self {
        Self {
            name: entry.name.clone(),
            active: entry.active,
            cli_kind: entry.cli_kind.clone(),
            source: entry.source.clone(),
            plan_type: entry.plan_type.clone(),
            email: entry.email.clone(),
            runtime: entry.runtime.clone(),
            proxy: entry.proxy.clone(),
            session: entry.session.clone(),
            default_cli_args: entry.default_cli_args.clone(),
            agent_name: entry.agent_name.clone(),
            api_key_configured: entry.api_key_configured,
            codex_config: entry.codex_config.clone(),
            codex_config_error: entry.codex_config_error.clone(),
        }
    }

    pub(super) fn categories(
        &self,
        draft: &ProfileSettingsDraft,
    ) -> Vec<SessionTuiSettingCategory> {
        let mut launch =
            vec![self.editable_option("Runtime", ProfileSettingsField::Runtime, draft)];
        if draft.effective_runtime_kind(self) == ProfileRuntimeKind::Docker {
            launch.extend([
                self.editable_option("Docker image", ProfileSettingsField::DockerImage, draft),
                self.editable_option("Docker user", ProfileSettingsField::DockerUser, draft),
            ]);
        }
        launch.push(self.editable_option(
            "Extra CLI args",
            ProfileSettingsField::ExtraCliArgs,
            draft,
        ));

        let mut proxy =
            vec![self.editable_option("Override", ProfileSettingsField::ProxyMode, draft)];
        if draft.effective_proxy_mode(self) == ProfileProxyMode::Enabled {
            proxy.extend([
                self.editable_option("URL", ProfileSettingsField::ProxyUrl, draft),
                self.editable_option("NO_PROXY", ProfileSettingsField::ProxyNoProxy, draft),
                self.editable_option(
                    "Force HTTP transport",
                    ProfileSettingsField::ProxyForceHttpTransport,
                    draft,
                ),
            ]);
        }

        let mut categories = vec![
            SessionTuiSettingCategory::profile(
                "Identity",
                vec![
                    self.editable_option("Name", ProfileSettingsField::Name, draft),
                    self.editable_option("Agent name", ProfileSettingsField::AgentName, draft),
                    SessionTuiSettingOption::profile_read_only(
                        "Active home",
                        if self.active { "yes" } else { "no" },
                    ),
                    SessionTuiSettingOption::profile_read_only("CLI", self.cli_kind.clone()),
                ],
            ),
            SessionTuiSettingCategory::profile(
                "Imported metadata",
                vec![
                    SessionTuiSettingOption::profile_read_only(
                        "Source",
                        optional(self.source.as_deref()),
                    ),
                    SessionTuiSettingOption::profile_read_only(
                        "Plan",
                        optional(self.plan_type.as_deref()),
                    ),
                    SessionTuiSettingOption::profile_read_only(
                        "Email",
                        optional(self.email.as_deref()),
                    ),
                ],
            ),
        ];

        if self.cli_kind == "codex" && self.source.as_deref() == Some("api-key") {
            categories.push(SessionTuiSettingCategory::profile(
                "Authentication",
                vec![self.editable_option("API key", ProfileSettingsField::ApiKey, draft)],
            ));
        }

        if self.cli_kind == "codex" {
            match self.codex_config.as_ref() {
                Some(_) => {
                    let deepseek_preset = if self.source.as_deref() == Some("api-key") {
                        self.editable_option(
                            "DeepSeek preset",
                            ProfileSettingsField::DeepSeekPreset,
                            draft,
                        )
                    } else {
                        SessionTuiSettingOption::profile_read_only(
                            "DeepSeek preset",
                            "requires API-key profile",
                        )
                    };
                    categories.push(SessionTuiSettingCategory::profile(
                        "Model",
                        vec![
                            deepseek_preset,
                            self.editable_option("Model", ProfileSettingsField::Model, draft),
                            self.editable_option(
                                "Reasoning effort",
                                ProfileSettingsField::ModelReasoningEffort,
                                draft,
                            ),
                            self.editable_option(
                                "Model catalog",
                                ProfileSettingsField::ModelCatalogJson,
                                draft,
                            ),
                            self.editable_option(
                                "Forced login",
                                ProfileSettingsField::ForcedLoginMethod,
                                draft,
                            ),
                        ],
                    ));

                    let mut provider = vec![self.editable_option(
                        "Provider ID",
                        ProfileSettingsField::ModelProvider,
                        draft,
                    )];
                    match draft.effective_model_provider(self) {
                        Some(provider_id) if !is_builtin_model_provider(&provider_id) => {
                            provider.extend([
                                self.editable_option(
                                    "Display name",
                                    ProfileSettingsField::ProviderName,
                                    draft,
                                ),
                                self.editable_option(
                                    "Base URL",
                                    ProfileSettingsField::ProviderBaseUrl,
                                    draft,
                                ),
                                self.editable_option(
                                    "API key env",
                                    ProfileSettingsField::ProviderEnvKey,
                                    draft,
                                ),
                                self.editable_option(
                                    "Wire API",
                                    ProfileSettingsField::ProviderWireApi,
                                    draft,
                                ),
                                self.editable_option(
                                    "Request retries",
                                    ProfileSettingsField::RequestMaxRetries,
                                    draft,
                                ),
                                self.editable_option(
                                    "Stream retries",
                                    ProfileSettingsField::StreamMaxRetries,
                                    draft,
                                ),
                                self.editable_option(
                                    "Stream idle timeout (ms)",
                                    ProfileSettingsField::StreamIdleTimeoutMs,
                                    draft,
                                ),
                                self.editable_option(
                                    "WebSocket connect timeout (ms)",
                                    ProfileSettingsField::WebsocketConnectTimeoutMs,
                                    draft,
                                ),
                                self.editable_option(
                                    "OpenAI auth",
                                    ProfileSettingsField::ProviderRequiresOpenAiAuth,
                                    draft,
                                ),
                                self.editable_option(
                                    "WebSockets",
                                    ProfileSettingsField::ProviderSupportsWebsockets,
                                    draft,
                                ),
                            ]);
                        }
                        Some(provider_id) => {
                            provider.push(SessionTuiSettingOption::profile_read_only(
                                "Provider options",
                                format!("built in ({provider_id})"),
                            ));
                        }
                        None => {
                            provider.push(SessionTuiSettingOption::profile_read_only(
                                "Provider options",
                                "set a custom Provider ID",
                            ));
                        }
                    }
                    categories.push(SessionTuiSettingCategory::profile("Provider", provider));
                }
                None => categories.push(SessionTuiSettingCategory::profile(
                    "Model",
                    vec![SessionTuiSettingOption::profile_read_only(
                        "Config error",
                        self.codex_config_error.as_deref().unwrap_or("unavailable"),
                    )],
                )),
            }
        }

        categories.extend([
            SessionTuiSettingCategory::profile("Launch", launch),
            SessionTuiSettingCategory::profile("Proxy", proxy),
            SessionTuiSettingCategory::profile(
                "Managed sessions",
                vec![self.editable_option(
                    "Default behavior",
                    ProfileSettingsField::ManagedSessions,
                    draft,
                )],
            ),
        ]);
        categories
    }

    pub(super) fn choices(&self, field: ProfileSettingsField) -> Vec<SessionSettingsChoice> {
        let values: &[(&str, &str)] = match field {
            ProfileSettingsField::Runtime => &[("Host", "host"), ("Docker", "docker")],
            ProfileSettingsField::ProxyMode => &[
                ("Inherit global", "inherit"),
                ("Disabled", "disabled"),
                ("Enabled", "enabled"),
            ],
            ProfileSettingsField::ProxyForceHttpTransport => {
                &[("Enabled", "enabled"), ("Disabled", "disabled")]
            }
            ProfileSettingsField::ManagedSessions => &[
                ("Inherit global", "inherit"),
                ("Enabled", "enabled"),
                ("Disabled", "disabled"),
            ],
            ProfileSettingsField::DeepSeekPreset => &[
                ("Keep current model/provider config", "current"),
                ("Apply DeepSeek defaults", "deepseek"),
            ],
            ProfileSettingsField::ModelReasoningEffort => &[
                ("Codex/model default", "default"),
                ("Minimal", "minimal"),
                ("Low", "low"),
                ("Medium", "medium"),
                ("High", "high"),
                ("Extra high", "xhigh"),
            ],
            ProfileSettingsField::ForcedLoginMethod => &[
                ("Codex default", "default"),
                ("ChatGPT", "chatgpt"),
                ("API key", "api"),
            ],
            ProfileSettingsField::ProviderWireApi => &[
                ("Codex default (responses)", "default"),
                ("Responses", "responses"),
            ],
            ProfileSettingsField::ProviderRequiresOpenAiAuth
            | ProfileSettingsField::ProviderSupportsWebsockets => &[
                ("Codex default (disabled)", "default"),
                ("Enabled", "enabled"),
                ("Disabled", "disabled"),
            ],
            ProfileSettingsField::ApiKey
            | ProfileSettingsField::Name
            | ProfileSettingsField::AgentName
            | ProfileSettingsField::DockerImage
            | ProfileSettingsField::DockerUser
            | ProfileSettingsField::ExtraCliArgs
            | ProfileSettingsField::ProxyUrl
            | ProfileSettingsField::ProxyNoProxy
            | ProfileSettingsField::Model
            | ProfileSettingsField::ModelCatalogJson
            | ProfileSettingsField::ModelProvider
            | ProfileSettingsField::ProviderName
            | ProfileSettingsField::ProviderBaseUrl
            | ProfileSettingsField::ProviderEnvKey
            | ProfileSettingsField::RequestMaxRetries
            | ProfileSettingsField::StreamMaxRetries
            | ProfileSettingsField::StreamIdleTimeoutMs
            | ProfileSettingsField::WebsocketConnectTimeoutMs => &[],
        };
        values
            .iter()
            .map(|(label, value)| SessionSettingsChoice {
                label: (*label).to_string(),
                value: Some((*value).to_string()),
            })
            .collect()
    }

    pub(super) fn editor_value(
        &self,
        draft: &ProfileSettingsDraft,
        field: ProfileSettingsField,
    ) -> String {
        draft.editor_value(self, field)
    }

    fn editable_option(
        &self,
        label: &'static str,
        field: ProfileSettingsField,
        draft: &ProfileSettingsDraft,
    ) -> SessionTuiSettingOption {
        SessionTuiSettingOption::profile_editable(
            label,
            draft.value(self, field),
            field,
            draft.field_is_dirty(field),
        )
    }

    fn runtime_kind(&self) -> ProfileRuntimeKind {
        match self.runtime {
            RuntimeConfig::Host => ProfileRuntimeKind::Host,
            RuntimeConfig::Docker { .. } => ProfileRuntimeKind::Docker,
        }
    }

    fn proxy_mode(&self) -> ProfileProxyMode {
        match self.proxy.as_ref() {
            None => ProfileProxyMode::Inherit,
            Some(proxy) if proxy.enabled => ProfileProxyMode::Enabled,
            Some(_) => ProfileProxyMode::Disabled,
        }
    }

    fn session_mode(&self) -> ProfileSessionMode {
        match self.session.as_ref() {
            None => ProfileSessionMode::Inherit,
            Some(session) if session.enabled => ProfileSessionMode::Enabled,
            Some(_) => ProfileSessionMode::Disabled,
        }
    }

    fn docker_image(&self) -> &str {
        match &self.runtime {
            RuntimeConfig::Host => DEFAULT_DOCKER_IMAGE,
            RuntimeConfig::Docker { image, .. } => image,
        }
    }

    fn docker_user(&self) -> String {
        match &self.runtime {
            RuntimeConfig::Host => default_docker_user_name(),
            RuntimeConfig::Docker { user_name, .. } => {
                user_name.clone().unwrap_or_else(default_docker_user_name)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ProfileSettingsDraft {
    name: Option<String>,
    agent_name: Option<Option<String>>,
    api_key: ProfileApiKeyUpdate,
    runtime_kind: Option<ProfileRuntimeKind>,
    docker_image: Option<String>,
    docker_user: Option<String>,
    default_cli_args: Option<Vec<String>>,
    proxy_mode: Option<ProfileProxyMode>,
    proxy_url: Option<Option<String>>,
    proxy_no_proxy: Option<Option<String>>,
    proxy_force_http_transport: Option<bool>,
    session_mode: Option<ProfileSessionMode>,
    codex_config: CodexProfileConfigPatch,
}

impl ProfileSettingsDraft {
    pub(super) fn stage(
        &mut self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        match field {
            ProfileSettingsField::ApiKey => {
                anyhow::bail!("API key must be changed through the secret editor")
            }
            ProfileSettingsField::Name => {
                let value = required_trimmed(value, "Profile name")?;
                self.name = (value != snapshot.name).then_some(value);
            }
            ProfileSettingsField::AgentName => {
                let value = optional_trimmed(value);
                self.agent_name = (value != snapshot.agent_name).then_some(value);
            }
            ProfileSettingsField::Runtime => {
                let kind = match value.as_deref() {
                    Some("host") => ProfileRuntimeKind::Host,
                    Some("docker") => ProfileRuntimeKind::Docker,
                    Some(other) => anyhow::bail!("unsupported profile runtime: {other}"),
                    None => anyhow::bail!("profile runtime cannot inherit"),
                };
                self.runtime_kind = (kind != snapshot.runtime_kind()).then_some(kind);
                if kind != ProfileRuntimeKind::Docker {
                    self.docker_image = None;
                    self.docker_user = None;
                }
            }
            ProfileSettingsField::DockerImage => {
                if self.effective_runtime_kind(snapshot) != ProfileRuntimeKind::Docker {
                    anyhow::bail!("Set profile runtime to Docker first");
                }
                let value = required_trimmed(value, "Docker image")?;
                self.docker_image = (value != snapshot.docker_image()).then_some(value);
            }
            ProfileSettingsField::DockerUser => {
                if self.effective_runtime_kind(snapshot) != ProfileRuntimeKind::Docker {
                    anyhow::bail!("Set profile runtime to Docker first");
                }
                let value = normalize_docker_user_name(value)?;
                self.docker_user = (value != snapshot.docker_user()).then_some(value);
            }
            ProfileSettingsField::ExtraCliArgs => {
                let args = parse_cli_args_value(value.as_deref().unwrap_or_default())?;
                self.default_cli_args = (args != snapshot.default_cli_args).then_some(args);
            }
            ProfileSettingsField::ProxyMode => {
                let mode = match value.as_deref() {
                    Some("inherit") => ProfileProxyMode::Inherit,
                    Some("disabled") => ProfileProxyMode::Disabled,
                    Some("enabled") => ProfileProxyMode::Enabled,
                    Some(other) => anyhow::bail!("unsupported profile proxy mode: {other}"),
                    None => anyhow::bail!("profile proxy mode cannot be empty"),
                };
                self.proxy_mode = (mode != snapshot.proxy_mode()).then_some(mode);
                if mode != ProfileProxyMode::Enabled {
                    self.proxy_url = None;
                    self.proxy_no_proxy = None;
                    self.proxy_force_http_transport = None;
                }
            }
            ProfileSettingsField::ProxyUrl => {
                self.require_enabled_proxy(snapshot)?;
                let value = optional_trimmed(value);
                self.proxy_url = (value
                    != snapshot.proxy.as_ref().and_then(|proxy| proxy.url.clone()))
                .then_some(value);
            }
            ProfileSettingsField::ProxyNoProxy => {
                self.require_enabled_proxy(snapshot)?;
                let value = optional_trimmed(value);
                self.proxy_no_proxy = (value
                    != snapshot
                        .proxy
                        .as_ref()
                        .and_then(|proxy| proxy.no_proxy.clone()))
                .then_some(value);
            }
            ProfileSettingsField::ProxyForceHttpTransport => {
                self.require_enabled_proxy(snapshot)?;
                let enabled = match value.as_deref() {
                    Some("enabled") => true,
                    Some("disabled") => false,
                    Some(other) => anyhow::bail!("unsupported HTTP transport mode: {other}"),
                    None => anyhow::bail!("HTTP transport mode cannot be empty"),
                };
                let original = snapshot
                    .proxy
                    .as_ref()
                    .map(|proxy| proxy.force_http_transport)
                    .unwrap_or(true);
                self.proxy_force_http_transport = (enabled != original).then_some(enabled);
            }
            ProfileSettingsField::ManagedSessions => {
                let mode = match value.as_deref() {
                    Some("inherit") => ProfileSessionMode::Inherit,
                    Some("enabled") => ProfileSessionMode::Enabled,
                    Some("disabled") => ProfileSessionMode::Disabled,
                    Some(other) => anyhow::bail!("unsupported managed-session mode: {other}"),
                    None => anyhow::bail!("managed-session mode cannot be empty"),
                };
                self.session_mode = (mode != snapshot.session_mode()).then_some(mode);
            }
            ProfileSettingsField::DeepSeekPreset => match value.as_deref() {
                Some("current") => {
                    self.require_codex_config(snapshot)?;
                    self.codex_config = CodexProfileConfigPatch::default();
                }
                Some("deepseek") => {
                    self.require_codex_config(snapshot)?;
                    if snapshot.source.as_deref() != Some("api-key") {
                        anyhow::bail!("DeepSeek preset requires an API-key profile");
                    }
                    self.codex_config = CodexProfileConfigPatch {
                        apply_deepseek_preset: true,
                        ..CodexProfileConfigPatch::default()
                    };
                }
                Some(other) => anyhow::bail!("unsupported model/provider preset: {other}"),
                None => anyhow::bail!("model/provider preset cannot be empty"),
            },
            ProfileSettingsField::Model
            | ProfileSettingsField::ModelReasoningEffort
            | ProfileSettingsField::ModelCatalogJson
            | ProfileSettingsField::ForcedLoginMethod
            | ProfileSettingsField::ModelProvider
            | ProfileSettingsField::ProviderName
            | ProfileSettingsField::ProviderBaseUrl
            | ProfileSettingsField::ProviderEnvKey
            | ProfileSettingsField::ProviderWireApi => {
                self.stage_codex_string(snapshot, field, value)?;
            }
            ProfileSettingsField::RequestMaxRetries
            | ProfileSettingsField::StreamMaxRetries
            | ProfileSettingsField::StreamIdleTimeoutMs
            | ProfileSettingsField::WebsocketConnectTimeoutMs => {
                self.stage_codex_u64(snapshot, field, value)?;
            }
            ProfileSettingsField::ProviderRequiresOpenAiAuth
            | ProfileSettingsField::ProviderSupportsWebsockets => {
                self.stage_codex_bool(snapshot, field, value)?;
            }
        }
        Ok(())
    }

    pub(super) fn stage_secret(
        &mut self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
        action: SecretSettingsAction,
    ) -> anyhow::Result<()> {
        if field != ProfileSettingsField::ApiKey {
            anyhow::bail!("profile field is not a secret setting");
        }
        if snapshot.cli_kind != "codex" || snapshot.source.as_deref() != Some("api-key") {
            anyhow::bail!("API key editing is only available for Codex API-key profiles");
        }
        self.api_key = match action {
            SecretSettingsAction::Keep => ProfileApiKeyUpdate::Unchanged,
            SecretSettingsAction::Clear if !snapshot.api_key_configured => {
                ProfileApiKeyUpdate::Unchanged
            }
            SecretSettingsAction::Clear => ProfileApiKeyUpdate::Clear,
            SecretSettingsAction::Replace(value) => {
                let value = value.trim();
                if value.is_empty() {
                    anyhow::bail!("Replacement API key cannot be empty");
                }
                ProfileApiKeyUpdate::Replace(value.to_string())
            }
        };
        Ok(())
    }

    pub(super) fn value(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> String {
        match field {
            ProfileSettingsField::ApiKey => match self.api_key {
                ProfileApiKeyUpdate::Unchanged if snapshot.api_key_configured => {
                    "(configured)".to_string()
                }
                ProfileApiKeyUpdate::Unchanged => "(missing)".to_string(),
                ProfileApiKeyUpdate::Replace(_) => "(replace staged)".to_string(),
                ProfileApiKeyUpdate::Clear => "(clear staged)".to_string(),
            },
            ProfileSettingsField::Name => self
                .name
                .as_deref()
                .unwrap_or(snapshot.name.as_str())
                .to_string(),
            ProfileSettingsField::AgentName => optional(
                self.agent_name
                    .as_ref()
                    .unwrap_or(&snapshot.agent_name)
                    .as_deref(),
            ),
            ProfileSettingsField::Runtime => {
                self.effective_runtime_kind(snapshot).value().to_string()
            }
            ProfileSettingsField::DockerImage => self
                .docker_image
                .as_deref()
                .unwrap_or_else(|| snapshot.docker_image())
                .to_string(),
            ProfileSettingsField::DockerUser => self
                .docker_user
                .clone()
                .unwrap_or_else(|| snapshot.docker_user()),
            ProfileSettingsField::ExtraCliArgs => cli_args_label(
                self.default_cli_args
                    .as_deref()
                    .unwrap_or(snapshot.default_cli_args.as_slice()),
            ),
            ProfileSettingsField::ProxyMode => {
                self.effective_proxy_mode(snapshot).value().to_string()
            }
            ProfileSettingsField::ProxyUrl => optional(
                self.proxy_url
                    .as_ref()
                    .unwrap_or(&snapshot.proxy.as_ref().and_then(|proxy| proxy.url.clone()))
                    .as_deref(),
            ),
            ProfileSettingsField::ProxyNoProxy => optional(
                self.proxy_no_proxy
                    .as_ref()
                    .unwrap_or(
                        &snapshot
                            .proxy
                            .as_ref()
                            .and_then(|proxy| proxy.no_proxy.clone()),
                    )
                    .as_deref(),
            ),
            ProfileSettingsField::ProxyForceHttpTransport => {
                let enabled = self.proxy_force_http_transport.unwrap_or_else(|| {
                    snapshot
                        .proxy
                        .as_ref()
                        .map(|proxy| proxy.force_http_transport)
                        .unwrap_or(true)
                });
                enabled_label(enabled).to_string()
            }
            ProfileSettingsField::ManagedSessions => {
                self.effective_session_mode(snapshot).value().to_string()
            }
            ProfileSettingsField::DeepSeekPreset => {
                if self.codex_config.apply_deepseek_preset {
                    "staged".to_string()
                } else if snapshot
                    .codex_config
                    .as_ref()
                    .is_some_and(codex_config_matches_deepseek_preset)
                {
                    "configured".to_string()
                } else {
                    "available".to_string()
                }
            }
            ProfileSettingsField::Model
            | ProfileSettingsField::ModelReasoningEffort
            | ProfileSettingsField::ModelCatalogJson
            | ProfileSettingsField::ForcedLoginMethod
            | ProfileSettingsField::ModelProvider
            | ProfileSettingsField::ProviderName
            | ProfileSettingsField::ProviderBaseUrl
            | ProfileSettingsField::ProviderEnvKey => self
                .effective_codex_string(snapshot, field)
                .unwrap_or_else(|| "default".to_string()),
            ProfileSettingsField::ProviderWireApi => self
                .effective_codex_string(snapshot, field)
                .unwrap_or_else(|| "default (responses)".to_string()),
            ProfileSettingsField::RequestMaxRetries => display_optional_u64(
                self.effective_codex_u64(snapshot, field),
                DEFAULT_REQUEST_MAX_RETRIES,
            ),
            ProfileSettingsField::StreamMaxRetries => display_optional_u64(
                self.effective_codex_u64(snapshot, field),
                DEFAULT_STREAM_MAX_RETRIES,
            ),
            ProfileSettingsField::StreamIdleTimeoutMs => display_optional_u64(
                self.effective_codex_u64(snapshot, field),
                DEFAULT_STREAM_IDLE_TIMEOUT_MS,
            ),
            ProfileSettingsField::WebsocketConnectTimeoutMs => display_optional_u64(
                self.effective_codex_u64(snapshot, field),
                DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS,
            ),
            ProfileSettingsField::ProviderRequiresOpenAiAuth
            | ProfileSettingsField::ProviderSupportsWebsockets => self
                .effective_codex_bool(snapshot, field)
                .map(|value| enabled_label(value).to_string())
                .unwrap_or_else(|| "default (disabled)".to_string()),
        }
    }

    pub(super) fn editor_value(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> String {
        match field {
            ProfileSettingsField::ApiKey => String::new(),
            ProfileSettingsField::DeepSeekPreset => if self.codex_config.apply_deepseek_preset {
                "deepseek"
            } else {
                "current"
            }
            .to_string(),
            ProfileSettingsField::ModelReasoningEffort
            | ProfileSettingsField::ForcedLoginMethod
            | ProfileSettingsField::ProviderWireApi => self
                .effective_codex_string(snapshot, field)
                .unwrap_or_else(|| "default".to_string()),
            ProfileSettingsField::ProviderRequiresOpenAiAuth
            | ProfileSettingsField::ProviderSupportsWebsockets => self
                .effective_codex_bool(snapshot, field)
                .map(enabled_label)
                .unwrap_or("default")
                .to_string(),
            ProfileSettingsField::Model
            | ProfileSettingsField::ModelCatalogJson
            | ProfileSettingsField::ModelProvider
            | ProfileSettingsField::ProviderName
            | ProfileSettingsField::ProviderBaseUrl
            | ProfileSettingsField::ProviderEnvKey => self
                .effective_codex_string(snapshot, field)
                .unwrap_or_default(),
            ProfileSettingsField::RequestMaxRetries
            | ProfileSettingsField::StreamMaxRetries
            | ProfileSettingsField::StreamIdleTimeoutMs
            | ProfileSettingsField::WebsocketConnectTimeoutMs => self
                .effective_codex_u64(snapshot, field)
                .map(|value| value.to_string())
                .unwrap_or_default(),
            ProfileSettingsField::AgentName
            | ProfileSettingsField::ExtraCliArgs
            | ProfileSettingsField::ProxyUrl
            | ProfileSettingsField::ProxyNoProxy => {
                let value = self.value(snapshot, field);
                (value != "-").then_some(value).unwrap_or_default()
            }
            ProfileSettingsField::Name
            | ProfileSettingsField::Runtime
            | ProfileSettingsField::DockerImage
            | ProfileSettingsField::DockerUser
            | ProfileSettingsField::ProxyMode
            | ProfileSettingsField::ProxyForceHttpTransport
            | ProfileSettingsField::ManagedSessions => self.value(snapshot, field),
        }
    }

    pub(super) fn choices(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> Vec<SessionSettingsChoice> {
        snapshot.choices(field)
    }

    pub(super) fn field_is_dirty(&self, field: ProfileSettingsField) -> bool {
        match field {
            ProfileSettingsField::ApiKey => !self.api_key.is_unchanged(),
            ProfileSettingsField::Name => self.name.is_some(),
            ProfileSettingsField::AgentName => self.agent_name.is_some(),
            ProfileSettingsField::Runtime => self.runtime_kind.is_some(),
            ProfileSettingsField::DockerImage => self.docker_image.is_some(),
            ProfileSettingsField::DockerUser => self.docker_user.is_some(),
            ProfileSettingsField::ExtraCliArgs => self.default_cli_args.is_some(),
            ProfileSettingsField::ProxyMode => self.proxy_mode.is_some(),
            ProfileSettingsField::ProxyUrl => self.proxy_url.is_some(),
            ProfileSettingsField::ProxyNoProxy => self.proxy_no_proxy.is_some(),
            ProfileSettingsField::ProxyForceHttpTransport => {
                self.proxy_force_http_transport.is_some()
            }
            ProfileSettingsField::ManagedSessions => self.session_mode.is_some(),
            ProfileSettingsField::DeepSeekPreset => self.codex_config.apply_deepseek_preset,
            ProfileSettingsField::Model => {
                !matches!(self.codex_config.model, ConfigValueUpdate::Unchanged)
            }
            ProfileSettingsField::ModelReasoningEffort => !matches!(
                self.codex_config.model_reasoning_effort,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ModelCatalogJson => !matches!(
                self.codex_config.model_catalog_json,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ForcedLoginMethod => !matches!(
                self.codex_config.forced_login_method,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ModelProvider => !matches!(
                self.codex_config.model_provider,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderName => !matches!(
                self.codex_config.provider_name,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderBaseUrl => !matches!(
                self.codex_config.provider_base_url,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderEnvKey => !matches!(
                self.codex_config.provider_env_key,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderWireApi => !matches!(
                self.codex_config.provider_wire_api,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::RequestMaxRetries => !matches!(
                self.codex_config.request_max_retries,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::StreamMaxRetries => !matches!(
                self.codex_config.stream_max_retries,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::StreamIdleTimeoutMs => !matches!(
                self.codex_config.stream_idle_timeout_ms,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::WebsocketConnectTimeoutMs => !matches!(
                self.codex_config.websocket_connect_timeout_ms,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderRequiresOpenAiAuth => !matches!(
                self.codex_config.requires_openai_auth,
                ConfigValueUpdate::Unchanged
            ),
            ProfileSettingsField::ProviderSupportsWebsockets => !matches!(
                self.codex_config.supports_websockets,
                ConfigValueUpdate::Unchanged
            ),
        }
    }

    pub(super) fn dirty_count(&self) -> usize {
        PROFILE_SETTINGS_FIELDS
            .iter()
            .filter(|field| self.field_is_dirty(**field))
            .count()
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty_count() != 0
    }

    pub(super) fn patch(
        &self,
        snapshot: &ProfileSettingsSnapshot,
    ) -> anyhow::Result<ProfileSettingsPatch> {
        let runtime = if self.runtime_kind.is_some()
            || self.docker_image.is_some()
            || self.docker_user.is_some()
        {
            Some(match self.effective_runtime_kind(snapshot) {
                ProfileRuntimeKind::Host => RuntimeConfig::Host,
                ProfileRuntimeKind::Docker => RuntimeConfig::Docker {
                    image: self
                        .docker_image
                        .clone()
                        .unwrap_or_else(|| snapshot.docker_image().to_string()),
                    user_name: Some(normalize_docker_user_name(Some(
                        self.docker_user
                            .clone()
                            .unwrap_or_else(|| snapshot.docker_user()),
                    ))?),
                },
            })
        } else {
            None
        };

        let proxy_dirty = self.proxy_mode.is_some()
            || self.proxy_url.is_some()
            || self.proxy_no_proxy.is_some()
            || self.proxy_force_http_transport.is_some();
        let proxy = if !proxy_dirty {
            ConfigValueUpdate::Unchanged
        } else {
            match self.effective_proxy_mode(snapshot) {
                ProfileProxyMode::Inherit => ConfigValueUpdate::Clear,
                ProfileProxyMode::Disabled => {
                    ConfigValueUpdate::Set(proxy_config_from_parts(false, None, None, true)?)
                }
                ProfileProxyMode::Enabled => ConfigValueUpdate::Set(proxy_config_from_parts(
                    true,
                    self.effective_proxy_url(snapshot),
                    self.effective_proxy_no_proxy(snapshot),
                    self.proxy_force_http_transport.unwrap_or_else(|| {
                        snapshot
                            .proxy
                            .as_ref()
                            .map(|proxy| proxy.force_http_transport)
                            .unwrap_or(true)
                    }),
                )?),
            }
        };

        let session = match self.session_mode {
            None => ConfigValueUpdate::Unchanged,
            Some(ProfileSessionMode::Inherit) => ConfigValueUpdate::Clear,
            Some(ProfileSessionMode::Enabled) => {
                ConfigValueUpdate::Set(SessionConfig { enabled: true })
            }
            Some(ProfileSessionMode::Disabled) => {
                ConfigValueUpdate::Set(SessionConfig { enabled: false })
            }
        };
        let agent_name = match &self.agent_name {
            None => ConfigValueUpdate::Unchanged,
            Some(Some(value)) => ConfigValueUpdate::Set(value.clone()),
            Some(None) => ConfigValueUpdate::Clear,
        };

        Ok(ProfileSettingsPatch {
            name: self.name.clone(),
            runtime,
            proxy,
            session,
            default_cli_args: self.default_cli_args.clone(),
            agent_name,
            api_key: self.api_key.clone(),
            codex_config: self.codex_config.clone(),
            ..ProfileSettingsPatch::default()
        })
    }

    fn stage_codex_string(
        &mut self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        self.require_codex_config(snapshot)?;
        let value = match field {
            ProfileSettingsField::ModelReasoningEffort
            | ProfileSettingsField::ForcedLoginMethod
            | ProfileSettingsField::ProviderWireApi
                if value.as_deref() == Some("default") =>
            {
                None
            }
            _ => optional_trimmed(value),
        };
        match field {
            ProfileSettingsField::ModelReasoningEffort
                if value.as_deref().is_some_and(|value| {
                    !matches!(value, "minimal" | "low" | "medium" | "high" | "xhigh")
                }) =>
            {
                anyhow::bail!("unsupported model reasoning effort")
            }
            ProfileSettingsField::ForcedLoginMethod
                if value
                    .as_deref()
                    .is_some_and(|value| !matches!(value, "chatgpt" | "api")) =>
            {
                anyhow::bail!("forced login must be `chatgpt`, `api`, or default")
            }
            _ => {}
        }
        if field == ProfileSettingsField::ProviderWireApi
            && value.as_deref().is_some_and(|value| value != "responses")
        {
            anyhow::bail!("wire API only supports `responses` in cute-codex 0.144.1");
        }
        if matches!(
            field,
            ProfileSettingsField::ProviderName
                | ProfileSettingsField::ProviderBaseUrl
                | ProfileSettingsField::ProviderEnvKey
                | ProfileSettingsField::ProviderWireApi
        ) {
            self.require_custom_provider(snapshot)?;
        }
        let base = self.codex_baseline(snapshot)?;
        let update = string_update(codex_snapshot_string(&base, field), value);
        match field {
            ProfileSettingsField::Model => self.codex_config.model = update,
            ProfileSettingsField::ModelReasoningEffort => {
                self.codex_config.model_reasoning_effort = update
            }
            ProfileSettingsField::ModelCatalogJson => self.codex_config.model_catalog_json = update,
            ProfileSettingsField::ForcedLoginMethod => {
                self.codex_config.forced_login_method = update
            }
            ProfileSettingsField::ModelProvider => self.codex_config.model_provider = update,
            ProfileSettingsField::ProviderName => self.codex_config.provider_name = update,
            ProfileSettingsField::ProviderBaseUrl => self.codex_config.provider_base_url = update,
            ProfileSettingsField::ProviderEnvKey => self.codex_config.provider_env_key = update,
            ProfileSettingsField::ProviderWireApi => self.codex_config.provider_wire_api = update,
            _ => anyhow::bail!("profile field is not a Codex string setting"),
        }
        Ok(())
    }

    fn stage_codex_u64(
        &mut self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        self.require_codex_config(snapshot)?;
        let value = optional_trimmed(value)
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| anyhow::anyhow!("value must be a non-negative integer"))
            })
            .transpose()?;
        if matches!(
            field,
            ProfileSettingsField::RequestMaxRetries | ProfileSettingsField::StreamMaxRetries
        ) && value.is_some_and(|value| value > MAX_PROVIDER_RETRIES)
        {
            anyhow::bail!("retry count must be between 0 and {MAX_PROVIDER_RETRIES}");
        }
        if value.is_some_and(|value| i64::try_from(value).is_err()) {
            anyhow::bail!("value is too large for a TOML integer");
        }
        self.require_custom_provider(snapshot)?;
        let base = self.codex_baseline(snapshot)?;
        let update = value_update(codex_snapshot_u64(&base, field), value);
        match field {
            ProfileSettingsField::RequestMaxRetries => {
                self.codex_config.request_max_retries = update
            }
            ProfileSettingsField::StreamMaxRetries => self.codex_config.stream_max_retries = update,
            ProfileSettingsField::StreamIdleTimeoutMs => {
                self.codex_config.stream_idle_timeout_ms = update
            }
            ProfileSettingsField::WebsocketConnectTimeoutMs => {
                self.codex_config.websocket_connect_timeout_ms = update
            }
            _ => anyhow::bail!("profile field is not a Codex numeric setting"),
        }
        Ok(())
    }

    fn stage_codex_bool(
        &mut self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        self.require_codex_config(snapshot)?;
        let value = match value.as_deref() {
            Some("default") => None,
            Some("enabled") => Some(true),
            Some("disabled") => Some(false),
            Some(other) => anyhow::bail!("unsupported provider boolean value: {other}"),
            None => anyhow::bail!("provider boolean value cannot be empty"),
        };
        self.require_custom_provider(snapshot)?;
        let base = self.codex_baseline(snapshot)?;
        let update = value_update(codex_snapshot_bool(&base, field), value);
        match field {
            ProfileSettingsField::ProviderRequiresOpenAiAuth => {
                self.codex_config.requires_openai_auth = update
            }
            ProfileSettingsField::ProviderSupportsWebsockets => {
                self.codex_config.supports_websockets = update
            }
            _ => anyhow::bail!("profile field is not a Codex boolean setting"),
        }
        Ok(())
    }

    fn effective_codex_string(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> Option<String> {
        let base = self.codex_baseline(snapshot).ok()?;
        let update = match field {
            ProfileSettingsField::Model => &self.codex_config.model,
            ProfileSettingsField::ModelReasoningEffort => &self.codex_config.model_reasoning_effort,
            ProfileSettingsField::ModelCatalogJson => &self.codex_config.model_catalog_json,
            ProfileSettingsField::ForcedLoginMethod => &self.codex_config.forced_login_method,
            ProfileSettingsField::ModelProvider => &self.codex_config.model_provider,
            ProfileSettingsField::ProviderName => &self.codex_config.provider_name,
            ProfileSettingsField::ProviderBaseUrl => &self.codex_config.provider_base_url,
            ProfileSettingsField::ProviderEnvKey => &self.codex_config.provider_env_key,
            ProfileSettingsField::ProviderWireApi => &self.codex_config.provider_wire_api,
            _ => return None,
        };
        effective_value(codex_snapshot_string(&base, field), update)
    }

    fn effective_codex_u64(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> Option<u64> {
        let base = self.codex_baseline(snapshot).ok()?;
        let update = match field {
            ProfileSettingsField::RequestMaxRetries => &self.codex_config.request_max_retries,
            ProfileSettingsField::StreamMaxRetries => &self.codex_config.stream_max_retries,
            ProfileSettingsField::StreamIdleTimeoutMs => &self.codex_config.stream_idle_timeout_ms,
            ProfileSettingsField::WebsocketConnectTimeoutMs => {
                &self.codex_config.websocket_connect_timeout_ms
            }
            _ => return None,
        };
        effective_value(codex_snapshot_u64(&base, field), update)
    }

    fn effective_codex_bool(
        &self,
        snapshot: &ProfileSettingsSnapshot,
        field: ProfileSettingsField,
    ) -> Option<bool> {
        let base = self.codex_baseline(snapshot).ok()?;
        let update = match field {
            ProfileSettingsField::ProviderRequiresOpenAiAuth => {
                &self.codex_config.requires_openai_auth
            }
            ProfileSettingsField::ProviderSupportsWebsockets => {
                &self.codex_config.supports_websockets
            }
            _ => return None,
        };
        effective_value(codex_snapshot_bool(&base, field), update)
    }

    fn effective_model_provider(&self, snapshot: &ProfileSettingsSnapshot) -> Option<String> {
        self.effective_codex_string(snapshot, ProfileSettingsField::ModelProvider)
    }

    fn codex_baseline(
        &self,
        snapshot: &ProfileSettingsSnapshot,
    ) -> anyhow::Result<CodexProfileConfigSnapshot> {
        self.require_codex_config(snapshot)?;
        Ok(if self.codex_config.apply_deepseek_preset {
            deepseek_config_snapshot()
        } else {
            snapshot.codex_config.clone().unwrap_or_default()
        })
    }

    fn require_codex_config(&self, snapshot: &ProfileSettingsSnapshot) -> anyhow::Result<()> {
        if snapshot.cli_kind != "codex" {
            anyhow::bail!("Codex model settings are unavailable for this profile");
        }
        if let Some(error) = snapshot.codex_config_error.as_deref() {
            anyhow::bail!("Codex profile config is invalid: {error}");
        }
        Ok(())
    }

    fn require_custom_provider(&self, snapshot: &ProfileSettingsSnapshot) -> anyhow::Result<()> {
        let provider = self
            .effective_model_provider(snapshot)
            .ok_or_else(|| anyhow::anyhow!("Set a custom Provider ID first"))?;
        if is_builtin_model_provider(&provider) {
            anyhow::bail!("Provider options cannot override built-in provider `{provider}`");
        }
        Ok(())
    }

    fn effective_runtime_kind(&self, snapshot: &ProfileSettingsSnapshot) -> ProfileRuntimeKind {
        self.runtime_kind.unwrap_or_else(|| snapshot.runtime_kind())
    }

    fn effective_proxy_mode(&self, snapshot: &ProfileSettingsSnapshot) -> ProfileProxyMode {
        self.proxy_mode.unwrap_or_else(|| snapshot.proxy_mode())
    }

    fn effective_session_mode(&self, snapshot: &ProfileSettingsSnapshot) -> ProfileSessionMode {
        self.session_mode.unwrap_or_else(|| snapshot.session_mode())
    }

    fn effective_proxy_url(&self, snapshot: &ProfileSettingsSnapshot) -> Option<String> {
        self.proxy_url
            .clone()
            .unwrap_or_else(|| snapshot.proxy.as_ref().and_then(|proxy| proxy.url.clone()))
    }

    fn effective_proxy_no_proxy(&self, snapshot: &ProfileSettingsSnapshot) -> Option<String> {
        self.proxy_no_proxy.clone().unwrap_or_else(|| {
            snapshot
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.no_proxy.clone())
        })
    }

    fn require_enabled_proxy(&self, snapshot: &ProfileSettingsSnapshot) -> anyhow::Result<()> {
        if self.effective_proxy_mode(snapshot) != ProfileProxyMode::Enabled {
            anyhow::bail!("Enable the profile proxy override first");
        }
        Ok(())
    }
}

fn deepseek_config_snapshot() -> CodexProfileConfigSnapshot {
    CodexProfileConfigSnapshot {
        model: Some(deepseek::DEEPSEEK_DEFAULT_MODEL.to_string()),
        model_provider: Some(deepseek::DEEPSEEK_PROVIDER_ID.to_string()),
        forced_login_method: Some("api".to_string()),
        model_reasoning_effort: Some(deepseek::DEEPSEEK_DEFAULT_REASONING.to_string()),
        model_catalog_json: Some(deepseek::DEEPSEEK_MODEL_CATALOG_FILE.to_string()),
        provider_name: Some(deepseek::DEEPSEEK_PROVIDER_NAME.to_string()),
        provider_base_url: Some(deepseek::DEEPSEEK_BASE_URL.to_string()),
        provider_env_key: Some(deepseek::DEEPSEEK_API_KEY_ENV.to_string()),
        provider_wire_api: Some("responses".to_string()),
        requires_openai_auth: Some(false),
        ..CodexProfileConfigSnapshot::default()
    }
}

fn codex_config_matches_deepseek_preset(config: &CodexProfileConfigSnapshot) -> bool {
    let preset = deepseek_config_snapshot();
    config.model == preset.model
        && config.model_provider == preset.model_provider
        && config.forced_login_method == preset.forced_login_method
        && config.model_reasoning_effort == preset.model_reasoning_effort
        && config.model_catalog_json == preset.model_catalog_json
        && config.provider_name == preset.provider_name
        && config.provider_base_url == preset.provider_base_url
        && config.provider_env_key == preset.provider_env_key
        && config.provider_wire_api == preset.provider_wire_api
        && config.requires_openai_auth == preset.requires_openai_auth
}

fn codex_snapshot_string(
    snapshot: &CodexProfileConfigSnapshot,
    field: ProfileSettingsField,
) -> Option<&String> {
    match field {
        ProfileSettingsField::Model => snapshot.model.as_ref(),
        ProfileSettingsField::ModelReasoningEffort => snapshot.model_reasoning_effort.as_ref(),
        ProfileSettingsField::ModelCatalogJson => snapshot.model_catalog_json.as_ref(),
        ProfileSettingsField::ForcedLoginMethod => snapshot.forced_login_method.as_ref(),
        ProfileSettingsField::ModelProvider => snapshot.model_provider.as_ref(),
        ProfileSettingsField::ProviderName => snapshot.provider_name.as_ref(),
        ProfileSettingsField::ProviderBaseUrl => snapshot.provider_base_url.as_ref(),
        ProfileSettingsField::ProviderEnvKey => snapshot.provider_env_key.as_ref(),
        ProfileSettingsField::ProviderWireApi => snapshot.provider_wire_api.as_ref(),
        _ => None,
    }
}

fn codex_snapshot_u64(
    snapshot: &CodexProfileConfigSnapshot,
    field: ProfileSettingsField,
) -> Option<&u64> {
    match field {
        ProfileSettingsField::RequestMaxRetries => snapshot.request_max_retries.as_ref(),
        ProfileSettingsField::StreamMaxRetries => snapshot.stream_max_retries.as_ref(),
        ProfileSettingsField::StreamIdleTimeoutMs => snapshot.stream_idle_timeout_ms.as_ref(),
        ProfileSettingsField::WebsocketConnectTimeoutMs => {
            snapshot.websocket_connect_timeout_ms.as_ref()
        }
        _ => None,
    }
}

fn codex_snapshot_bool(
    snapshot: &CodexProfileConfigSnapshot,
    field: ProfileSettingsField,
) -> Option<&bool> {
    match field {
        ProfileSettingsField::ProviderRequiresOpenAiAuth => snapshot.requires_openai_auth.as_ref(),
        ProfileSettingsField::ProviderSupportsWebsockets => snapshot.supports_websockets.as_ref(),
        _ => None,
    }
}

fn effective_value<T: Clone>(base: Option<&T>, update: &ConfigValueUpdate<T>) -> Option<T> {
    match update {
        ConfigValueUpdate::Unchanged => base.cloned(),
        ConfigValueUpdate::Set(value) => Some(value.clone()),
        ConfigValueUpdate::Clear => None,
    }
}

fn value_update<T: Eq>(base: Option<&T>, value: Option<T>) -> ConfigValueUpdate<T> {
    if value.as_ref() == base {
        ConfigValueUpdate::Unchanged
    } else {
        match value {
            Some(value) => ConfigValueUpdate::Set(value),
            None => ConfigValueUpdate::Clear,
        }
    }
}

fn string_update(base: Option<&String>, value: Option<String>) -> ConfigValueUpdate<String> {
    value_update(base, value)
}

fn display_optional_u64(value: Option<u64>, default: u64) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| format!("default ({default})"))
}

fn optional(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn optional_trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "-")
}

fn required_trimmed(value: Option<String>, label: &str) -> anyhow::Result<String> {
    let value = optional_trimmed(value);
    value.ok_or_else(|| anyhow::anyhow!("{label} cannot be empty"))
}

fn enabled_label(enabled: bool) -> &'static str {
    if enabled {
        "enabled"
    } else {
        "disabled"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> ProfileCatalogEntry {
        ProfileCatalogEntry {
            id: "profile-id".to_string(),
            name: "alpha".to_string(),
            email: Some("alpha@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            source: Some("chatgpt".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: "codex".to_string(),
            default_cli_args: Vec::new(),
            agent_name: None,
            api_key_configured: false,
            codex_config: Some(CodexProfileConfigSnapshot::default()),
            codex_config_error: None,
            active: true,
        }
    }

    fn option_fields(
        categories: &[SessionTuiSettingCategory],
    ) -> Vec<Option<ProfileSettingsField>> {
        categories
            .iter()
            .flat_map(|category| category.options.iter().map(|option| option.profile_field))
            .collect()
    }

    fn option<'a>(
        categories: &'a [SessionTuiSettingCategory],
        field: ProfileSettingsField,
    ) -> &'a SessionTuiSettingOption {
        categories
            .iter()
            .flat_map(|category| &category.options)
            .find(|option| option.profile_field == Some(field))
            .unwrap_or_else(|| panic!("missing projected field {field:?}"))
    }

    #[test]
    fn expanded_projection_separates_editable_and_imported_profile_data() {
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry());
        let categories = snapshot.categories(&ProfileSettingsDraft::default());

        assert_eq!(
            categories
                .iter()
                .map(|category| category.label)
                .collect::<Vec<_>>(),
            [
                "Identity",
                "Imported metadata",
                "Model",
                "Provider",
                "Launch",
                "Proxy",
                "Managed sessions",
            ]
        );
        assert!(categories[0]
            .options
            .iter()
            .any(|option| option.label == "Active home" && option.value == "yes"));
        assert!(categories[1]
            .options
            .iter()
            .all(|option| option.profile_field.is_none()));
        assert!(!option_fields(&categories).contains(&Some(ProfileSettingsField::DockerImage)));
        assert!(!option_fields(&categories).contains(&Some(ProfileSettingsField::ProxyUrl)));
        assert_eq!(
            option(&categories, ProfileSettingsField::Model).value,
            "default"
        );
        assert_eq!(
            option(&categories, ProfileSettingsField::ModelProvider).value,
            "default"
        );
    }

    #[test]
    fn api_key_profiles_expose_only_redacted_secret_state() {
        let mut entry = entry();
        entry.source = Some("api-key".to_string());
        entry.api_key_configured = true;
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry);
        let mut draft = ProfileSettingsDraft::default();
        let categories = snapshot.categories(&draft);

        assert!(categories
            .iter()
            .any(|category| category.label == "Authentication"));
        assert_eq!(
            option(&categories, ProfileSettingsField::ApiKey).value,
            "(configured)"
        );
        assert!(snapshot
            .editor_value(&draft, ProfileSettingsField::ApiKey)
            .is_empty());

        let test_key = "sk-test-profile-secret";
        draft
            .stage_secret(
                &snapshot,
                ProfileSettingsField::ApiKey,
                SecretSettingsAction::Replace(test_key.to_string()),
            )
            .expect("replacement API key should stage");
        assert_eq!(
            draft.value(&snapshot, ProfileSettingsField::ApiKey),
            "(replace staged)"
        );
        assert_eq!(draft.dirty_count(), 1);
        assert!(!format!("{draft:?}").contains(test_key));

        let patch = draft.patch(&snapshot).expect("API key patch");
        assert!(
            matches!(patch.api_key, ProfileApiKeyUpdate::Replace(ref value) if value == test_key)
        );
        assert!(!format!("{patch:?}").contains(test_key));

        draft
            .stage_secret(
                &snapshot,
                ProfileSettingsField::ApiKey,
                SecretSettingsAction::Keep,
            )
            .expect("keep should clear the staged replacement");
        assert!(!draft.is_dirty());
    }

    #[test]
    fn non_api_key_profiles_do_not_project_an_api_key_editor() {
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry());
        let mut draft = ProfileSettingsDraft::default();
        assert!(!option_fields(&snapshot.categories(&draft))
            .contains(&Some(ProfileSettingsField::ApiKey)));
        assert!(draft
            .stage_secret(
                &snapshot,
                ProfileSettingsField::ApiKey,
                SecretSettingsAction::Replace("sk-test-rejected".to_string()),
            )
            .is_err());
        assert!(!draft.is_dirty());
    }

    #[test]
    fn runtime_and_proxy_choices_reproject_conditional_editor_rows() {
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry());
        let mut draft = ProfileSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                ProfileSettingsField::Runtime,
                Some("docker".to_string()),
            )
            .unwrap();
        draft
            .stage(
                &snapshot,
                ProfileSettingsField::ProxyMode,
                Some("enabled".to_string()),
            )
            .unwrap();
        let categories = snapshot.categories(&draft);
        let fields = option_fields(&categories);
        assert!(fields.contains(&Some(ProfileSettingsField::DockerImage)));
        assert!(fields.contains(&Some(ProfileSettingsField::DockerUser)));
        assert!(fields.contains(&Some(ProfileSettingsField::ProxyUrl)));
        assert_eq!(draft.dirty_count(), 2);

        assert!(draft.patch(&snapshot).is_err());
        draft
            .stage(
                &snapshot,
                ProfileSettingsField::ProxyUrl,
                Some("http://127.0.0.1:8080".to_string()),
            )
            .unwrap();
        let patch = draft.patch(&snapshot).unwrap();
        assert!(matches!(patch.runtime, Some(RuntimeConfig::Docker { .. })));
        assert!(matches!(patch.proxy, ConfigValueUpdate::Set(_)));
    }

    #[test]
    fn staging_original_values_returns_to_a_clean_draft() {
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry());
        let mut draft = ProfileSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                ProfileSettingsField::Name,
                Some("beta".to_string()),
            )
            .unwrap();
        assert!(draft.is_dirty());
        draft
            .stage(
                &snapshot,
                ProfileSettingsField::Name,
                Some("alpha".to_string()),
            )
            .unwrap();
        assert!(!draft.is_dirty());
        assert_eq!(
            draft.patch(&snapshot).unwrap(),
            ProfileSettingsPatch::default()
        );
    }

    #[test]
    fn unset_provider_controls_show_codex_defaults_but_open_empty() {
        let mut entry = entry();
        entry.codex_config = Some(
            cutex::profiles::codex_profile::inspect_codex_profile_config(Some(
                r#"
model_provider = "custom"

[model_providers.custom]
name = "Custom"
"#,
            ))
            .expect("typed custom provider config"),
        );
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry);
        let draft = ProfileSettingsDraft::default();
        let categories = snapshot.categories(&draft);

        assert_eq!(
            option(&categories, ProfileSettingsField::RequestMaxRetries).value,
            "default (4)"
        );
        assert_eq!(
            snapshot.editor_value(&draft, ProfileSettingsField::RequestMaxRetries),
            ""
        );
        assert_eq!(
            snapshot.editor_value(&draft, ProfileSettingsField::ProviderWireApi),
            "default"
        );
        assert_eq!(
            option(
                &categories,
                ProfileSettingsField::ProviderRequiresOpenAiAuth
            )
            .value,
            "default (disabled)"
        );
    }

    #[test]
    fn deepseek_preset_reprojects_provider_and_builds_one_atomic_patch() {
        let mut entry = entry();
        entry.source = Some("api-key".to_string());
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry);
        let mut draft = ProfileSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                ProfileSettingsField::DeepSeekPreset,
                Some("deepseek".to_string()),
            )
            .unwrap();

        let categories = snapshot.categories(&draft);
        assert_eq!(
            option(&categories, ProfileSettingsField::Model).value,
            deepseek::DEEPSEEK_DEFAULT_MODEL
        );
        assert_eq!(
            option(&categories, ProfileSettingsField::ProviderBaseUrl).value,
            deepseek::DEEPSEEK_BASE_URL
        );
        assert_eq!(draft.dirty_count(), 1);
        let patch = draft.patch(&snapshot).unwrap();
        assert!(patch.codex_config.apply_deepseek_preset);
        assert!(matches!(
            patch.codex_config.request_max_retries,
            ConfigValueUpdate::Unchanged
        ));

        draft
            .stage(
                &snapshot,
                ProfileSettingsField::RequestMaxRetries,
                Some("100".to_string()),
            )
            .unwrap();
        assert!(draft
            .stage(
                &snapshot,
                ProfileSettingsField::RequestMaxRetries,
                Some("101".to_string()),
            )
            .is_err());
        let patch = draft.patch(&snapshot).unwrap();
        assert_eq!(
            patch.codex_config.request_max_retries,
            ConfigValueUpdate::Set(100)
        );
    }

    #[test]
    fn deepseek_preset_is_read_only_for_non_api_key_profiles() {
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry());
        let draft = ProfileSettingsDraft::default();
        let categories = snapshot.categories(&draft);
        let preset = categories
            .iter()
            .flat_map(|category| &category.options)
            .find(|option| option.label == "DeepSeek preset")
            .expect("preset row");

        assert!(preset.profile_field.is_none());
        assert_eq!(preset.value, "requires API-key profile");
        let mut draft = ProfileSettingsDraft::default();
        assert!(draft
            .stage(
                &snapshot,
                ProfileSettingsField::DeepSeekPreset,
                Some("deepseek".to_string()),
            )
            .is_err());
    }

    #[test]
    fn empty_retry_editor_clears_override_and_zero_remains_explicit() {
        let mut entry = entry();
        entry.codex_config = Some(
            cutex::profiles::codex_profile::inspect_codex_profile_config(Some(
                r#"
model_provider = "custom"

[model_providers.custom]
name = "Custom"
request_max_retries = 8
"#,
            ))
            .expect("typed custom provider config"),
        );
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry);
        let mut draft = ProfileSettingsDraft::default();

        draft
            .stage(&snapshot, ProfileSettingsField::RequestMaxRetries, None)
            .unwrap();
        assert_eq!(
            draft
                .patch(&snapshot)
                .unwrap()
                .codex_config
                .request_max_retries,
            ConfigValueUpdate::Clear
        );
        assert_eq!(
            draft.value(&snapshot, ProfileSettingsField::RequestMaxRetries),
            "default (4)"
        );
        assert_eq!(
            draft.editor_value(&snapshot, ProfileSettingsField::RequestMaxRetries),
            ""
        );

        draft
            .stage(
                &snapshot,
                ProfileSettingsField::RequestMaxRetries,
                Some("0".to_string()),
            )
            .unwrap();
        assert_eq!(
            draft
                .patch(&snapshot)
                .unwrap()
                .codex_config
                .request_max_retries,
            ConfigValueUpdate::Set(0)
        );
        draft
            .stage(
                &snapshot,
                ProfileSettingsField::RequestMaxRetries,
                Some("8".to_string()),
            )
            .unwrap();
        assert!(!draft.field_is_dirty(ProfileSettingsField::RequestMaxRetries));
    }

    #[test]
    fn non_codex_profiles_do_not_project_model_provider_settings() {
        let mut entry = entry();
        entry.cli_kind = "claude".to_string();
        let snapshot = ProfileSettingsSnapshot::from_catalog_entry(&entry);
        let categories = snapshot.categories(&ProfileSettingsDraft::default());

        assert!(!categories
            .iter()
            .any(|category| matches!(category.label, "Model" | "Provider")));
    }
}
