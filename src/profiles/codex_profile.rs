//! Typed inspection and mutation for profile-scoped Codex model settings.

use toml::value::Table;

use crate::config::global_settings::ConfigValueUpdate;
use crate::profiles::deepseek;
use crate::profiles::profile_config::parse_toml_table;

pub const DEFAULT_REQUEST_MAX_RETRIES: u64 = 4;
pub const DEFAULT_STREAM_MAX_RETRIES: u64 = 5;
pub const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
pub const MAX_PROVIDER_RETRIES: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexProfileConfigSnapshot {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub forced_login_method: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_catalog_json: Option<String>,
    pub provider_name: Option<String>,
    pub provider_base_url: Option<String>,
    pub provider_env_key: Option<String>,
    pub provider_wire_api: Option<String>,
    pub request_max_retries: Option<u64>,
    pub stream_max_retries: Option<u64>,
    pub stream_idle_timeout_ms: Option<u64>,
    pub websocket_connect_timeout_ms: Option<u64>,
    pub requires_openai_auth: Option<bool>,
    pub supports_websockets: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexProfileConfigPatch {
    pub apply_deepseek_preset: bool,
    pub model: ConfigValueUpdate<String>,
    pub model_provider: ConfigValueUpdate<String>,
    pub forced_login_method: ConfigValueUpdate<String>,
    pub model_reasoning_effort: ConfigValueUpdate<String>,
    pub model_catalog_json: ConfigValueUpdate<String>,
    pub provider_name: ConfigValueUpdate<String>,
    pub provider_base_url: ConfigValueUpdate<String>,
    pub provider_env_key: ConfigValueUpdate<String>,
    pub provider_wire_api: ConfigValueUpdate<String>,
    pub request_max_retries: ConfigValueUpdate<u64>,
    pub stream_max_retries: ConfigValueUpdate<u64>,
    pub stream_idle_timeout_ms: ConfigValueUpdate<u64>,
    pub websocket_connect_timeout_ms: ConfigValueUpdate<u64>,
    pub requires_openai_auth: ConfigValueUpdate<bool>,
    pub supports_websockets: ConfigValueUpdate<bool>,
}

impl CodexProfileConfigPatch {
    pub fn is_empty(&self) -> bool {
        !self.apply_deepseek_preset
            && matches!(self.model, ConfigValueUpdate::Unchanged)
            && matches!(self.model_provider, ConfigValueUpdate::Unchanged)
            && matches!(self.forced_login_method, ConfigValueUpdate::Unchanged)
            && matches!(self.model_reasoning_effort, ConfigValueUpdate::Unchanged)
            && matches!(self.model_catalog_json, ConfigValueUpdate::Unchanged)
            && matches!(self.provider_name, ConfigValueUpdate::Unchanged)
            && matches!(self.provider_base_url, ConfigValueUpdate::Unchanged)
            && matches!(self.provider_env_key, ConfigValueUpdate::Unchanged)
            && matches!(self.provider_wire_api, ConfigValueUpdate::Unchanged)
            && matches!(self.request_max_retries, ConfigValueUpdate::Unchanged)
            && matches!(self.stream_max_retries, ConfigValueUpdate::Unchanged)
            && matches!(self.stream_idle_timeout_ms, ConfigValueUpdate::Unchanged)
            && matches!(
                self.websocket_connect_timeout_ms,
                ConfigValueUpdate::Unchanged
            )
            && matches!(self.requires_openai_auth, ConfigValueUpdate::Unchanged)
            && matches!(self.supports_websockets, ConfigValueUpdate::Unchanged)
    }
}

pub fn inspect_codex_profile_config(
    config_toml: Option<&str>,
) -> anyhow::Result<CodexProfileConfigSnapshot> {
    let root = config_toml
        .map(parse_toml_table)
        .transpose()?
        .unwrap_or_default();
    let model_provider = optional_string(&root, "model_provider")?;
    let provider = model_provider
        .as_deref()
        .and_then(|provider_id| selected_provider_table(&root, provider_id));

    Ok(CodexProfileConfigSnapshot {
        model: optional_string(&root, "model")?,
        model_provider,
        forced_login_method: optional_string(&root, "forced_login_method")?,
        model_reasoning_effort: optional_string(&root, "model_reasoning_effort")?,
        model_catalog_json: optional_string(&root, "model_catalog_json")?,
        provider_name: optional_provider_string(provider, "name")?,
        provider_base_url: optional_provider_string(provider, "base_url")?,
        provider_env_key: optional_provider_string(provider, "env_key")?,
        provider_wire_api: optional_provider_string(provider, "wire_api")?,
        request_max_retries: optional_provider_u64(provider, "request_max_retries")?,
        stream_max_retries: optional_provider_u64(provider, "stream_max_retries")?,
        stream_idle_timeout_ms: optional_provider_u64(provider, "stream_idle_timeout_ms")?,
        websocket_connect_timeout_ms: optional_provider_u64(
            provider,
            "websocket_connect_timeout_ms",
        )?,
        requires_openai_auth: optional_provider_bool(provider, "requires_openai_auth")?,
        supports_websockets: optional_provider_bool(provider, "supports_websockets")?,
    })
}

pub fn apply_codex_profile_config_patch(
    existing_config_toml: Option<&str>,
    patch: &CodexProfileConfigPatch,
) -> anyhow::Result<(Option<String>, bool)> {
    if patch.is_empty() {
        return Ok((existing_config_toml.map(str::to_string), false));
    }

    let mut root = existing_config_toml
        .map(parse_toml_table)
        .transpose()?
        .unwrap_or_default();
    let before = root.clone();

    if patch.apply_deepseek_preset {
        deepseek::apply_profile_preset(&mut root, None)?;
    }

    let previous_provider = optional_string(&root, "model_provider")?;
    apply_optional_string(&mut root, "model", &patch.model, false)?;
    apply_optional_string(
        &mut root,
        "forced_login_method",
        &patch.forced_login_method,
        false,
    )?;
    apply_optional_string(
        &mut root,
        "model_reasoning_effort",
        &patch.model_reasoning_effort,
        false,
    )?;
    apply_optional_string(
        &mut root,
        "model_catalog_json",
        &patch.model_catalog_json,
        false,
    )?;
    apply_model_provider_update(
        &mut root,
        previous_provider.as_deref(),
        &patch.model_provider,
    )?;

    let effective_provider = optional_string(&root, "model_provider")?;
    let provider_patch_requested = provider_patch_requested(patch);
    if provider_patch_requested {
        let provider_id = effective_provider.as_deref().ok_or_else(|| {
            anyhow::anyhow!("Set a model provider before editing provider options")
        })?;
        if is_builtin_model_provider(provider_id) {
            anyhow::bail!(
                "Provider options cannot override built-in provider `{provider_id}`; set a custom provider ID first"
            );
        }
        let provider = ensure_selected_provider_table(&mut root, provider_id)?;
        apply_optional_string(provider, "name", &patch.provider_name, true)?;
        apply_optional_string(provider, "base_url", &patch.provider_base_url, false)?;
        apply_optional_string(provider, "env_key", &patch.provider_env_key, false)?;
        validate_wire_api_update(&patch.provider_wire_api)?;
        apply_optional_string(provider, "wire_api", &patch.provider_wire_api, false)?;
        apply_retry_update(provider, "request_max_retries", &patch.request_max_retries)?;
        apply_retry_update(provider, "stream_max_retries", &patch.stream_max_retries)?;
        apply_optional_u64(
            provider,
            "stream_idle_timeout_ms",
            &patch.stream_idle_timeout_ms,
        )?;
        apply_optional_u64(
            provider,
            "websocket_connect_timeout_ms",
            &patch.websocket_connect_timeout_ms,
        )?;
        apply_optional_bool(
            provider,
            "requires_openai_auth",
            &patch.requires_openai_auth,
        );
        apply_optional_bool(provider, "supports_websockets", &patch.supports_websockets);
    }

    let changed = root != before;
    let rendered = (!root.is_empty())
        .then(|| toml::to_string_pretty(&root))
        .transpose()?;
    Ok((rendered, changed))
}

pub fn is_builtin_model_provider(provider_id: &str) -> bool {
    matches!(
        provider_id.trim().to_ascii_lowercase().as_str(),
        "openai" | "ollama" | "lmstudio" | "amazon-bedrock"
    )
}

fn provider_patch_requested(patch: &CodexProfileConfigPatch) -> bool {
    !matches!(patch.provider_name, ConfigValueUpdate::Unchanged)
        || !matches!(patch.provider_base_url, ConfigValueUpdate::Unchanged)
        || !matches!(patch.provider_env_key, ConfigValueUpdate::Unchanged)
        || !matches!(patch.provider_wire_api, ConfigValueUpdate::Unchanged)
        || !matches!(patch.request_max_retries, ConfigValueUpdate::Unchanged)
        || !matches!(patch.stream_max_retries, ConfigValueUpdate::Unchanged)
        || !matches!(patch.stream_idle_timeout_ms, ConfigValueUpdate::Unchanged)
        || !matches!(
            patch.websocket_connect_timeout_ms,
            ConfigValueUpdate::Unchanged
        )
        || !matches!(patch.requires_openai_auth, ConfigValueUpdate::Unchanged)
        || !matches!(patch.supports_websockets, ConfigValueUpdate::Unchanged)
}

fn apply_model_provider_update(
    root: &mut Table,
    previous_provider: Option<&str>,
    update: &ConfigValueUpdate<String>,
) -> anyhow::Result<()> {
    let next_provider = match update {
        ConfigValueUpdate::Unchanged => return Ok(()),
        ConfigValueUpdate::Set(value) => Some(required_string(value, "Model provider")?),
        ConfigValueUpdate::Clear => None,
    };
    if previous_provider == next_provider.as_deref() {
        return Ok(());
    }

    let previous_table = previous_provider.and_then(|provider| take_provider_table(root, provider));
    match next_provider {
        Some(provider) => {
            root.insert(
                "model_provider".to_string(),
                toml::Value::String(provider.clone()),
            );
            if !is_builtin_model_provider(&provider) {
                let mut moved = previous_table
                    .and_then(|value| value.as_table().cloned())
                    .unwrap_or_default();
                if let Some(existing) =
                    take_provider_table(root, &provider).and_then(|value| value.as_table().cloned())
                {
                    moved.extend(existing);
                }
                if !moved.is_empty() {
                    insert_provider_table(root, &provider, moved)?;
                }
            }
        }
        None => {
            root.remove("model_provider");
        }
    }
    remove_empty_provider_section(root);
    Ok(())
}

fn apply_optional_string(
    table: &mut Table,
    key: &str,
    update: &ConfigValueUpdate<String>,
    required_when_set: bool,
) -> anyhow::Result<()> {
    match update {
        ConfigValueUpdate::Unchanged => {}
        ConfigValueUpdate::Clear => {
            table.remove(key);
        }
        ConfigValueUpdate::Set(value) => {
            let value = if required_when_set {
                required_string(value, key)?
            } else {
                optional_string_value(value)
                    .ok_or_else(|| anyhow::anyhow!("{key} cannot be empty when set"))?
            };
            table.insert(key.to_string(), toml::Value::String(value));
        }
    }
    Ok(())
}

fn apply_retry_update(
    table: &mut Table,
    key: &str,
    update: &ConfigValueUpdate<u64>,
) -> anyhow::Result<()> {
    if let ConfigValueUpdate::Set(value) = update {
        if *value > MAX_PROVIDER_RETRIES {
            anyhow::bail!("{key} must be between 0 and {MAX_PROVIDER_RETRIES}");
        }
    }
    apply_optional_u64(table, key, update)?;
    Ok(())
}

fn apply_optional_u64(
    table: &mut Table,
    key: &str,
    update: &ConfigValueUpdate<u64>,
) -> anyhow::Result<()> {
    match update {
        ConfigValueUpdate::Unchanged => {}
        ConfigValueUpdate::Clear => {
            table.remove(key);
        }
        ConfigValueUpdate::Set(value) => {
            let value = i64::try_from(*value)
                .map_err(|_| anyhow::anyhow!("{key} is too large for a TOML integer"))?;
            table.insert(key.to_string(), toml::Value::Integer(value));
        }
    }
    Ok(())
}

fn apply_optional_bool(table: &mut Table, key: &str, update: &ConfigValueUpdate<bool>) {
    match update {
        ConfigValueUpdate::Unchanged => {}
        ConfigValueUpdate::Clear => {
            table.remove(key);
        }
        ConfigValueUpdate::Set(value) => {
            table.insert(key.to_string(), toml::Value::Boolean(*value));
        }
    }
}

fn validate_wire_api_update(update: &ConfigValueUpdate<String>) -> anyhow::Result<()> {
    if let ConfigValueUpdate::Set(value) = update {
        if value.trim() != "responses" {
            anyhow::bail!("wire_api only supports `responses` in cute-codex 0.144.1");
        }
    }
    Ok(())
}

fn optional_string(table: &Table, key: &str) -> anyhow::Result<Option<String>> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("config.toml key `{key}` must be a string"))?;
    Ok(optional_string_value(value))
}

fn optional_provider_string(provider: Option<&Table>, key: &str) -> anyhow::Result<Option<String>> {
    provider
        .map(|provider| optional_string(provider, key))
        .transpose()
        .map(Option::flatten)
}

fn optional_provider_u64(provider: Option<&Table>, key: &str) -> anyhow::Result<Option<u64>> {
    let Some(value) = provider.and_then(|provider| provider.get(key)) else {
        return Ok(None);
    };
    let value = value
        .as_integer()
        .ok_or_else(|| anyhow::anyhow!("provider key `{key}` must be an integer"))?;
    u64::try_from(value)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("provider key `{key}` cannot be negative"))
}

fn optional_provider_bool(provider: Option<&Table>, key: &str) -> anyhow::Result<Option<bool>> {
    let Some(value) = provider.and_then(|provider| provider.get(key)) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("provider key `{key}` must be a boolean"))
}

fn selected_provider_table<'a>(root: &'a Table, provider_id: &str) -> Option<&'a Table> {
    root.get("model_providers")
        .and_then(toml::Value::as_table)
        .and_then(|providers| providers.get(provider_id))
        .and_then(toml::Value::as_table)
}

fn ensure_selected_provider_table<'a>(
    root: &'a mut Table,
    provider_id: &str,
) -> anyhow::Result<&'a mut Table> {
    let providers = root
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml key `model_providers` must be a table"))?;
    providers
        .entry(provider_id.to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("config.toml key `model_providers.{provider_id}` must be a table")
        })
}

fn take_provider_table(root: &mut Table, provider_id: &str) -> Option<toml::Value> {
    let providers = root
        .get_mut("model_providers")
        .and_then(toml::Value::as_table_mut)?;
    providers.remove(provider_id)
}

fn insert_provider_table(
    root: &mut Table,
    provider_id: &str,
    provider: Table,
) -> anyhow::Result<()> {
    let providers = root
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml key `model_providers` must be a table"))?;
    providers.insert(provider_id.to_string(), toml::Value::Table(provider));
    Ok(())
}

fn remove_empty_provider_section(root: &mut Table) {
    let remove = root
        .get("model_providers")
        .and_then(toml::Value::as_table)
        .is_some_and(Table::is_empty);
    if remove {
        root.remove("model_providers");
    }
}

fn optional_string_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != "-").then(|| value.to_string())
}

fn required_string(value: &str, label: &str) -> anyhow::Result<String> {
    optional_string_value(value).ok_or_else(|| anyhow::anyhow!("{label} cannot be empty"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_reads_all_supported_provider_scalars() {
        let config = r#"
model = "deepseek-v4-flash"
model_provider = "deepseek"
forced_login_method = "api"
model_reasoning_effort = "high"
model_catalog_json = "models.json"

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
request_max_retries = 8
stream_max_retries = 9
stream_idle_timeout_ms = 400000
websocket_connect_timeout_ms = 20000
requires_openai_auth = false
supports_websockets = false
future_provider_key = "preserved"
"#;

        let snapshot = inspect_codex_profile_config(Some(config)).expect("inspect config");

        assert_eq!(snapshot.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(snapshot.request_max_retries, Some(8));
        assert_eq!(snapshot.stream_max_retries, Some(9));
        assert_eq!(snapshot.stream_idle_timeout_ms, Some(400000));
        assert_eq!(snapshot.websocket_connect_timeout_ms, Some(20000));
        assert_eq!(snapshot.supports_websockets, Some(false));
    }

    #[test]
    fn patch_updates_and_clears_scalars_without_losing_unknown_provider_fields() {
        let existing = r#"
model = "old"
model_provider = "custom"

[model_providers.custom]
name = "Custom"
request_max_retries = 7
future_provider_key = "preserved"
"#;
        let patch = CodexProfileConfigPatch {
            model: ConfigValueUpdate::Set("new-model".to_string()),
            request_max_retries: ConfigValueUpdate::Clear,
            stream_max_retries: ConfigValueUpdate::Set(12),
            supports_websockets: ConfigValueUpdate::Set(true),
            ..CodexProfileConfigPatch::default()
        };

        let (updated, changed) = apply_codex_profile_config_patch(Some(existing), &patch)
            .expect("profile patch should apply");
        assert!(changed);
        let updated = updated.expect("updated config");
        let table = parse_toml_table(&updated).expect("updated config should parse");
        let provider = selected_provider_table(&table, "custom").expect("custom provider");
        assert!(!provider.contains_key("request_max_retries"));
        assert_eq!(
            provider
                .get("stream_max_retries")
                .and_then(toml::Value::as_integer),
            Some(12)
        );
        assert_eq!(
            provider
                .get("future_provider_key")
                .and_then(toml::Value::as_str),
            Some("preserved")
        );
    }

    #[test]
    fn provider_rename_moves_unknown_fields_and_builtin_switch_removes_custom_table() {
        let existing = r#"
model_provider = "before"

[model_providers.before]
name = "Before"
future_provider_key = "preserved"
"#;
        let rename = CodexProfileConfigPatch {
            model_provider: ConfigValueUpdate::Set("after".to_string()),
            provider_name: ConfigValueUpdate::Set("After".to_string()),
            ..CodexProfileConfigPatch::default()
        };
        let (renamed, _) =
            apply_codex_profile_config_patch(Some(existing), &rename).expect("rename provider");
        let renamed = renamed.expect("renamed config");
        let renamed_table = parse_toml_table(&renamed).expect("renamed config should parse");
        let provider = selected_provider_table(&renamed_table, "after").expect("renamed provider");
        assert_eq!(
            provider
                .get("future_provider_key")
                .and_then(toml::Value::as_str),
            Some("preserved")
        );
        assert!(selected_provider_table(&renamed_table, "before").is_none());

        let builtin = CodexProfileConfigPatch {
            model_provider: ConfigValueUpdate::Set("openai".to_string()),
            ..CodexProfileConfigPatch::default()
        };
        let (builtin, _) = apply_codex_profile_config_patch(Some(&renamed), &builtin)
            .expect("switch to built-in provider");
        let builtin = parse_toml_table(&builtin.expect("builtin config")).expect("parse builtin");
        assert!(builtin.get("model_providers").is_none());
    }

    #[test]
    fn retry_limits_and_wire_api_are_validated() {
        let existing = r#"
model_provider = "custom"
[model_providers.custom]
name = "Custom"
"#;
        let retries = CodexProfileConfigPatch {
            request_max_retries: ConfigValueUpdate::Set(101),
            ..CodexProfileConfigPatch::default()
        };
        assert!(apply_codex_profile_config_patch(Some(existing), &retries).is_err());

        let wire = CodexProfileConfigPatch {
            provider_wire_api: ConfigValueUpdate::Set("chat".to_string()),
            ..CodexProfileConfigPatch::default()
        };
        assert!(apply_codex_profile_config_patch(Some(existing), &wire).is_err());
    }

    #[test]
    fn deepseek_preset_is_atomic_and_leaves_retry_defaults_unset() {
        let patch = CodexProfileConfigPatch {
            apply_deepseek_preset: true,
            ..CodexProfileConfigPatch::default()
        };

        let (updated, changed) =
            apply_codex_profile_config_patch(None, &patch).expect("apply DeepSeek preset");
        assert!(changed);
        let snapshot = inspect_codex_profile_config(updated.as_deref()).expect("inspect preset");
        assert_eq!(snapshot.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(snapshot.model_provider.as_deref(), Some("deepseek"));
        assert_eq!(snapshot.request_max_retries, None);
        assert_eq!(snapshot.stream_max_retries, None);
    }
}
