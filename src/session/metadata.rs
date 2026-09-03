//! Read-only metadata and lookup helpers for durable `cutex_session` records.

use std::path::Path;

use crate::agent_bus::model::AgentRegistrationClass;
use crate::session::identity::default_cutex_session_id_for_codex_session;
use crate::session::model::CutexSessionRecord;
use crate::session::model::CutexSessionStore;

pub fn cutex_session_launch_cwd(record: &CutexSessionRecord) -> &str {
    record
        .managed_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(record.cwd.as_str())
}

pub fn normalize_cutex_session_managed_cwd_path(path: &str) -> anyhow::Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "-" {
        anyhow::bail!("managed cwd path cannot be empty");
    }
    if trimmed == "~" {
        return user_home_dir_string()
            .ok_or_else(|| anyhow::anyhow!("HOME/USERPROFILE is not set"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home =
            user_home_dir_string().ok_or_else(|| anyhow::anyhow!("HOME/USERPROFILE is not set"))?;
        return Ok(Path::new(&home).join(rest).display().to_string());
    }
    Ok(trimmed.to_string())
}

fn user_home_dir_string() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

pub fn cutex_session_display_name(record: &CutexSessionRecord) -> String {
    record
        .display_name_hint
        .clone()
        .or_else(|| record.thread_name.clone())
        .or_else(|| record.codex_session_id.clone())
        .unwrap_or_else(|| record.cutex_session_id.clone())
}

pub fn cutex_session_is_managed(record: &CutexSessionRecord) -> bool {
    record.registration_class == AgentRegistrationClass::Persistent
}

pub fn cutex_session_key_for_codex_session(
    store: &CutexSessionStore,
    codex_session_id: &str,
) -> String {
    store
        .sessions
        .iter()
        .find_map(|(key, record)| {
            (record.is_active() && record.codex_session_id.as_deref() == Some(codex_session_id))
                .then(|| key.clone())
        })
        .unwrap_or_else(|| default_cutex_session_id_for_codex_session(codex_session_id))
}

pub fn cutex_session_key_for_user_id(store: &CutexSessionStore, id: &str) -> Option<String> {
    cutex_session_key_for_user_id_matching(store, id, CutexSessionRecord::is_active)
}

pub fn cutex_session_key_for_user_id_including_retired(
    store: &CutexSessionStore,
    id: &str,
) -> Option<String> {
    cutex_session_key_for_user_id_matching(store, id, |_| true)
}

pub fn cutex_session_key_for_codex_session_including_retired(
    store: &CutexSessionStore,
    codex_session_id: &str,
) -> Option<String> {
    store.sessions.iter().find_map(|(key, record)| {
        (record.codex_session_id.as_deref() == Some(codex_session_id)).then(|| key.clone())
    })
}

fn cutex_session_key_for_user_id_matching(
    store: &CutexSessionStore,
    id: &str,
    include: impl Fn(&CutexSessionRecord) -> bool + Copy,
) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    if store.sessions.get(id).is_some_and(include) {
        return Some(id.to_string());
    }
    store
        .sessions
        .iter()
        .find_map(|(key, record)| {
            (include(record) && record.codex_session_id.as_deref() == Some(id)).then(|| key.clone())
        })
        .or_else(|| unique_cutex_session_key_by_name(store, id, include))
}

fn unique_cutex_session_key_by_name(
    store: &CutexSessionStore,
    name: &str,
    include: impl Fn(&CutexSessionRecord) -> bool + Copy,
) -> Option<String> {
    let needle = name.trim();
    if needle.is_empty() {
        return None;
    }
    let mut matches = store.sessions.iter().filter_map(|(key, record)| {
        (include(record)
            && [
                record.display_name_hint.as_deref(),
                record.thread_name.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|candidate| candidate == needle))
        .then(|| key.clone())
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_cwd_path_normalization_rejects_empty_values() {
        assert!(normalize_cutex_session_managed_cwd_path("").is_err());
        assert!(normalize_cutex_session_managed_cwd_path("   ").is_err());
        assert!(normalize_cutex_session_managed_cwd_path("-").is_err());
    }

    #[test]
    fn managed_cwd_path_normalization_trims_literal_path() {
        let normalized =
            normalize_cutex_session_managed_cwd_path("  /tmp/cutex-session  ").expect("path");
        assert_eq!(normalized, "/tmp/cutex-session");
    }
}
