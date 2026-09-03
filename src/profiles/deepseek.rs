//! DeepSeek Codex profile defaults published for cute-codex 0.144.x.

use toml::value::Table;

use crate::profiles::profile_config::remove_model_provider_entry;
use crate::profiles::profile_config::PROFILE_LOCAL_MODEL_CATALOG_FILE;

pub const DEEPSEEK_PROVIDER_ID: &str = "deepseek";
pub const DEEPSEEK_PROVIDER_NAME: &str = "DeepSeek";
pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/";
pub const DEEPSEEK_DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const DEEPSEEK_DEFAULT_REASONING: &str = "high";
pub const DEEPSEEK_MODEL_CATALOG_FILE: &str = PROFILE_LOCAL_MODEL_CATALOG_FILE;
pub const DEEPSEEK_API_KEY_ENV: &str = "OPENAI_API_KEY";

// Vendored from DeepSeek's official Codex setup script on 2026-08-07. Keeping
// the catalog local makes profile launches deterministic and offline-capable.
const DEEPSEEK_MODELS_JSON: &str = include_str!("deepseek_models_0_144_1.json");

const DEEPSEEK_CONFLICTING_KEYS: &[&str] = &[
    "base_instructions",
    "compact_prompt",
    "experimental_compact_prompt_file",
    "experimental_use_unified_exec_tool",
    "model_auto_compact_token_limit",
    "model_auto_compact_token_limit_scope",
    "model_context_window",
    "model_instructions_file",
    "model_reasoning_summary",
    "model_verbosity",
    "openai_base_url",
    "oss_provider",
    "plan_mode_reasoning_effort",
    "profile",
    "service_tier",
];

pub fn is_deepseek_provider(provider: &str) -> bool {
    provider.trim().eq_ignore_ascii_case(DEEPSEEK_PROVIDER_ID)
}

pub fn model_catalog_json() -> &'static str {
    DEEPSEEK_MODELS_JSON
}

pub fn apply_profile_preset(root: &mut Table, base_url: Option<&str>) -> anyhow::Result<()> {
    let previous_provider = root
        .get("model_provider")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    if let Some(previous_provider) = previous_provider.as_deref() {
        if previous_provider != DEEPSEEK_PROVIDER_ID {
            remove_model_provider_entry(root, previous_provider);
        }
    }

    for key in DEEPSEEK_CONFLICTING_KEYS {
        root.remove(*key);
    }
    root.remove("preferred_auth_method");
    root.insert(
        "model".to_string(),
        toml::Value::String(DEEPSEEK_DEFAULT_MODEL.to_string()),
    );
    root.insert(
        "model_provider".to_string(),
        toml::Value::String(DEEPSEEK_PROVIDER_ID.to_string()),
    );
    root.insert(
        "forced_login_method".to_string(),
        toml::Value::String("api".to_string()),
    );
    root.insert(
        "model_reasoning_effort".to_string(),
        toml::Value::String(DEEPSEEK_DEFAULT_REASONING.to_string()),
    );
    root.insert(
        "model_catalog_json".to_string(),
        toml::Value::String(DEEPSEEK_MODEL_CATALOG_FILE.to_string()),
    );

    let providers = root
        .entry("model_providers".to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config.toml key `model_providers` must be a table"))?;
    let provider = providers
        .entry(DEEPSEEK_PROVIDER_ID.to_string())
        .or_insert_with(|| toml::Value::Table(Table::new()))
        .as_table_mut()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "config.toml key `model_providers.{DEEPSEEK_PROVIDER_ID}` must be a table"
            )
        })?;

    for conflicting in ["auth", "aws", "experimental_bearer_token"] {
        provider.remove(conflicting);
    }
    provider.insert(
        "name".to_string(),
        toml::Value::String(DEEPSEEK_PROVIDER_NAME.to_string()),
    );
    provider.insert(
        "base_url".to_string(),
        toml::Value::String(base_url.unwrap_or(DEEPSEEK_BASE_URL).to_string()),
    );
    provider.insert(
        "env_key".to_string(),
        toml::Value::String(DEEPSEEK_API_KEY_ENV.to_string()),
    );
    provider.insert(
        "wire_api".to_string(),
        toml::Value::String("responses".to_string()),
    );
    provider.insert(
        "requires_openai_auth".to_string(),
        toml::Value::Boolean(false),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_catalog_is_valid_and_contains_flash_and_future_pro_entries() {
        let catalog: serde_json::Value =
            serde_json::from_str(model_catalog_json()).expect("catalog should be valid JSON");
        let models = catalog
            .get("models")
            .and_then(serde_json::Value::as_array)
            .expect("catalog should contain models");
        let slugs = models
            .iter()
            .filter_map(|model| model.get("slug").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>();
        assert!(slugs.contains(&"deepseek-v4-flash"));
        assert!(slugs.contains(&"deepseek-v4-pro"));
    }

    #[test]
    fn preset_uses_env_auth_and_leaves_retry_controls_at_codex_defaults() {
        let mut root = Table::new();
        apply_profile_preset(&mut root, None).expect("preset should apply");

        assert_eq!(
            root.get("model").and_then(toml::Value::as_str),
            Some(DEEPSEEK_DEFAULT_MODEL)
        );
        assert_eq!(
            root.get("model_catalog_json").and_then(toml::Value::as_str),
            Some(DEEPSEEK_MODEL_CATALOG_FILE)
        );
        assert!(!root.contains_key("preferred_auth_method"));
        let provider = root
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(DEEPSEEK_PROVIDER_ID))
            .and_then(toml::Value::as_table)
            .expect("DeepSeek provider should exist");
        assert_eq!(
            provider.get("env_key").and_then(toml::Value::as_str),
            Some(DEEPSEEK_API_KEY_ENV)
        );
        assert!(!provider.contains_key("experimental_bearer_token"));
        assert!(!provider.contains_key("request_max_retries"));
        assert!(!provider.contains_key("stream_max_retries"));
        assert!(!provider.contains_key("stream_idle_timeout_ms"));
        assert!(!provider.contains_key("websocket_connect_timeout_ms"));
    }

    #[test]
    fn preset_preserves_unknown_provider_fields_while_removing_secret_conflicts() {
        let mut root = toml::from_str::<toml::Value>(
            r#"
model_provider = "deepseek"
service_tier = "fast"

[model_providers.deepseek]
experimental_bearer_token = "must-not-survive"
future_provider_key = "keep"
request_max_retries = 12
"#,
        )
        .expect("config should parse")
        .as_table()
        .expect("config root should be a table")
        .clone();

        apply_profile_preset(&mut root, Some("https://deepseek.example/")).expect("preset");

        assert!(!root.contains_key("service_tier"));
        let provider = root
            .get("model_providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(DEEPSEEK_PROVIDER_ID))
            .and_then(toml::Value::as_table)
            .expect("provider should exist");
        assert_eq!(
            provider
                .get("future_provider_key")
                .and_then(toml::Value::as_str),
            Some("keep")
        );
        assert_eq!(
            provider
                .get("request_max_retries")
                .and_then(toml::Value::as_integer),
            Some(12)
        );
        assert!(!provider.contains_key("experimental_bearer_token"));
    }
}
