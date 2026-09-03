//! Launch-time notification environment construction.

use crate::config::env::{
    CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR, CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR,
    CODEX_NOTIFY_EVENTS_ENV_VAR, CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR,
    CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR, CODEX_NOTIFY_USER_MESSAGE_CONTENT_ENV_VAR,
    CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR, CODEX_RATE_LIMIT_MODEL_NUDGE_MODE_ENV_VAR,
    CODEX_RATE_LIMIT_THRESHOLD_WARNING_MODE_ENV_VAR,
};
use crate::profiles::model::CodezConfig;

pub fn launch_notify_envs(
    global_config: &CodezConfig,
    desktop_notify_url: Option<String>,
) -> Vec<(String, String)> {
    let mut envs = Vec::new();

    if global_config.desktop_notify_enabled {
        if let Some(url) = desktop_notify_url {
            envs.push(("CODEX_NOTIFY_SERVICE_URL".to_string(), url));
        }
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

    push_optional_env_if_unset(
        &mut envs,
        CODEX_NOTIFY_IDLE_TIMEOUT_ENV_VAR,
        global_config.notify_service_idle_timeout_secs,
    );
    push_optional_env_if_unset(
        &mut envs,
        CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT_ENV_VAR,
        global_config.notify_service_composer_idle_timeout_secs,
    );
    push_optional_env_if_unset(
        &mut envs,
        CODEX_NOTIFY_APPROVAL_TIMEOUT_ENV_VAR,
        global_config.notify_service_approval_timeout_secs,
    );
    push_optional_env_if_unset(
        &mut envs,
        CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT_ENV_VAR,
        global_config.notify_service_startup_idle_timeout_secs,
    );
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
    push_optional_env_if_unset(
        &mut envs,
        CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS_ENV_VAR,
        global_config.notify_service_user_message_preview_chars,
    );
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

fn push_optional_env_if_unset(envs: &mut Vec<(String, String)>, key: &str, value: Option<u64>) {
    if std::env::var_os(key).is_none() {
        if let Some(value) = value {
            envs.push((key.to_string(), value.to_string()));
        }
    }
}
