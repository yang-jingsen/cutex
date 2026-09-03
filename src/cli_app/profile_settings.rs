use std::fmt;

use cutex::config::global_settings::ConfigValueUpdate;
use cutex::config::proxy::proxy_config_from_parts;
use cutex::launch::docker::normalize_docker_user_name;
use cutex::profiles::codex_profile::CodexProfileConfigPatch;
use cutex::profiles::model::{ProxyConfig, RuntimeConfig, SessionConfig, StoredAccount};

#[derive(Clone, PartialEq, Eq, Default)]
pub(super) enum ProfileApiKeyUpdate {
    #[default]
    Unchanged,
    Replace(String),
    Clear,
}

impl ProfileApiKeyUpdate {
    pub(super) fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
}

impl fmt::Debug for ProfileApiKeyUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unchanged => formatter.write_str("Unchanged"),
            Self::Replace(_) => formatter.write_str("Replace(<redacted>)"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct ProfileSettingsPatch {
    pub(super) name: Option<String>,
    pub(super) source: ConfigValueUpdate<String>,
    pub(super) plan_type: ConfigValueUpdate<String>,
    pub(super) email: ConfigValueUpdate<String>,
    pub(super) runtime: Option<RuntimeConfig>,
    pub(super) proxy: ConfigValueUpdate<ProxyConfig>,
    pub(super) session: ConfigValueUpdate<SessionConfig>,
    pub(super) default_cli_args: Option<Vec<String>>,
    pub(super) agent_name: ConfigValueUpdate<String>,
    pub(super) api_key: ProfileApiKeyUpdate,
    pub(super) codex_config: CodexProfileConfigPatch,
}

pub(super) fn apply_profile_settings_patch(
    account: &mut StoredAccount,
    patch: &ProfileSettingsPatch,
) -> anyhow::Result<bool> {
    let mut next = account.clone();
    let changed = apply_profile_settings_patch_inner(&mut next, patch)?;
    if changed {
        *account = next;
    }
    Ok(changed)
}

fn apply_profile_settings_patch_inner(
    account: &mut StoredAccount,
    patch: &ProfileSettingsPatch,
) -> anyhow::Result<bool> {
    let mut changed = false;

    if let Some(name) = patch.name.as_deref() {
        if name.trim().is_empty() {
            anyhow::bail!("Profile name cannot be empty");
        }
        if account.name != name {
            account.name = name.to_string();
            changed = true;
        }
    }

    changed |= apply_optional_update(&mut account.source, &patch.source);
    changed |= apply_optional_update(&mut account.plan_type, &patch.plan_type);
    changed |= apply_optional_update(&mut account.email, &patch.email);

    if let Some(runtime) = patch.runtime.as_ref() {
        let normalized = normalize_runtime(runtime)?;
        if account.runtime != normalized {
            account.runtime = normalized;
            changed = true;
        }
    }

    let proxy = normalize_proxy_update(&patch.proxy)?;
    changed |= apply_optional_update(&mut account.proxy, &proxy);
    changed |= apply_optional_update(&mut account.session, &patch.session);

    if let Some(default_cli_args) = patch.default_cli_args.as_ref() {
        if account.default_cli_args != *default_cli_args {
            account.default_cli_args = default_cli_args.clone();
            changed = true;
        }
    }

    let agent_name = match &patch.agent_name {
        ConfigValueUpdate::Set(value) => {
            let value = value.trim();
            if value.is_empty() {
                anyhow::bail!("Agent name cannot be empty");
            }
            ConfigValueUpdate::Set(value.to_string())
        }
        ConfigValueUpdate::Clear => ConfigValueUpdate::Clear,
        ConfigValueUpdate::Unchanged => ConfigValueUpdate::Unchanged,
    };
    changed |= apply_optional_update(&mut account.agent_name, &agent_name);

    Ok(changed)
}

fn normalize_runtime(runtime: &RuntimeConfig) -> anyhow::Result<RuntimeConfig> {
    match runtime {
        RuntimeConfig::Host => Ok(RuntimeConfig::Host),
        RuntimeConfig::Docker { image, user_name } => {
            let image = image.trim();
            if image.is_empty() || image == "-" {
                anyhow::bail!("Docker image cannot be empty");
            }
            Ok(RuntimeConfig::Docker {
                image: image.to_string(),
                user_name: Some(normalize_docker_user_name(user_name.clone())?),
            })
        }
    }
}

fn normalize_proxy_update(
    update: &ConfigValueUpdate<ProxyConfig>,
) -> anyhow::Result<ConfigValueUpdate<ProxyConfig>> {
    match update {
        ConfigValueUpdate::Unchanged => Ok(ConfigValueUpdate::Unchanged),
        ConfigValueUpdate::Clear => Ok(ConfigValueUpdate::Clear),
        ConfigValueUpdate::Set(proxy) => Ok(ConfigValueUpdate::Set(proxy_config_from_parts(
            proxy.enabled,
            proxy.url.clone(),
            proxy.no_proxy.clone(),
            proxy.force_http_transport,
        )?)),
    }
}

fn apply_optional_update<T: Clone + PartialEq>(
    current: &mut Option<T>,
    update: &ConfigValueUpdate<T>,
) -> bool {
    let next = match update {
        ConfigValueUpdate::Unchanged => return false,
        ConfigValueUpdate::Set(value) => Some(value.clone()),
        ConfigValueUpdate::Clear => None,
    };
    if *current == next {
        return false;
    }
    *current = next;
    true
}

#[cfg(test)]
mod tests {
    use cutex::profiles::model::CliKind;

    use super::*;

    fn account() -> StoredAccount {
        StoredAccount {
            id: "profile-id".to_string(),
            name: "alpha".to_string(),
            email: Some("alpha@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            source: Some("chatgpt".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        }
    }

    #[test]
    fn patch_applies_supported_fields_without_touching_imported_metadata() {
        let mut account = account();
        let patch = ProfileSettingsPatch {
            name: Some("beta".to_string()),
            runtime: Some(RuntimeConfig::Docker {
                image: " cutex-base ".to_string(),
                user_name: Some("runner".to_string()),
            }),
            proxy: ConfigValueUpdate::Set(ProxyConfig {
                enabled: true,
                url: Some("http://127.0.0.1:8080".to_string()),
                no_proxy: Some("localhost".to_string()),
                force_http_transport: false,
            }),
            session: ConfigValueUpdate::Set(SessionConfig { enabled: true }),
            default_cli_args: Some(vec!["--model".to_string(), "gpt-5".to_string()]),
            agent_name: ConfigValueUpdate::Set(" build ".to_string()),
            ..ProfileSettingsPatch::default()
        };

        assert!(apply_profile_settings_patch(&mut account, &patch).unwrap());
        assert_eq!(account.name, "beta");
        assert_eq!(account.source.as_deref(), Some("chatgpt"));
        assert_eq!(account.plan_type.as_deref(), Some("pro"));
        assert_eq!(account.email.as_deref(), Some("alpha@example.com"));
        assert_eq!(
            account.runtime,
            RuntimeConfig::Docker {
                image: "cutex-base".to_string(),
                user_name: Some("runner".to_string()),
            }
        );
        assert_eq!(account.agent_name.as_deref(), Some("build"));
    }

    #[test]
    fn invalid_runtime_and_proxy_are_rejected_before_mutation() {
        let mut account = account();
        let invalid_runtime = ProfileSettingsPatch {
            runtime: Some(RuntimeConfig::Docker {
                image: " ".to_string(),
                user_name: None,
            }),
            ..ProfileSettingsPatch::default()
        };
        assert!(apply_profile_settings_patch(&mut account, &invalid_runtime).is_err());
        assert_eq!(account.runtime, RuntimeConfig::Host);

        let invalid_proxy = ProfileSettingsPatch {
            name: Some("must-not-stick".to_string()),
            proxy: ConfigValueUpdate::Set(ProxyConfig {
                enabled: true,
                url: None,
                no_proxy: None,
                force_http_transport: true,
            }),
            ..ProfileSettingsPatch::default()
        };
        assert!(apply_profile_settings_patch(&mut account, &invalid_proxy).is_err());
        assert_eq!(account.name, "alpha");
        assert!(account.proxy.is_none());
    }
}
