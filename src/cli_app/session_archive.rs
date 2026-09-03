//! Thin CLI/TUI adapter for the typed Retire/Restore provider transaction.

use std::fmt;
#[cfg(feature = "archive-conflict-test-hook")]
use std::fs;
#[cfg(feature = "archive-conflict-test-hook")]
use std::path::PathBuf;
#[cfg(feature = "archive-conflict-test-hook")]
use std::thread;
#[cfg(feature = "archive-conflict-test-hook")]
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use cutex::session::model::CutexSessionRecord;
use cutex::session::service::{
    cutex_session_display_name, cutex_session_is_managed,
    cutex_session_key_for_user_id_including_retired,
};
use cutex::session::store::load_cutex_session_store;
use cutex::ui::format::compact_home_path;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveSessionView {
    pub cutex_session_id: String,
    pub revision: u64,
    pub lifecycle: String,
    pub runtime_generation: u64,
    pub status: &'static str,
    pub timestamp: Option<String>,
    pub agent: String,
    pub profile: Option<String>,
    pub managed_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveTransitionReceipt {
    pub cutex_session_id: String,
    pub revision: u64,
    pub lifecycle: String,
    pub runtime_generation: u64,
    pub status: String,
    pub timestamp: Option<String>,
}

#[derive(Debug)]
struct ArchiveCommandError {
    stage: String,
    code: String,
    message: String,
    retryable: bool,
    details: serde_json::Value,
    outcome_unknown: bool,
}

impl ArchiveCommandError {
    fn from_provider(error: cutex::management::v2::user_input::UserInputExecutionError) -> Self {
        Self {
            stage: error.stage,
            code: error.code,
            message: error.message,
            retryable: error.retryable,
            details: error.details,
            outcome_unknown: error.outcome_unknown,
        }
    }

    fn route_error(code: &str, message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            stage: "route".to_string(),
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            details,
            outcome_unknown: false,
        }
    }

    fn provider_receipt_error(message: impl Into<String>) -> Self {
        Self {
            stage: "receipt".to_string(),
            code: "invalid_provider_receipt".to_string(),
            message: message.into(),
            retryable: true,
            details: json!({}),
            outcome_unknown: false,
        }
    }

    fn json_envelope(&self) -> serde_json::Value {
        json!({
            "stage": self.stage,
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable,
            "details": self.details,
            "outcomeUnknown": self.outcome_unknown,
        })
    }

    fn human_message(&self) -> String {
        format_archive_error_fields(
            &self.message,
            &self.code,
            &self.details,
            self.outcome_unknown,
        )
    }
}

impl fmt::Display for ArchiveCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.human_message())
    }
}

impl std::error::Error for ArchiveCommandError {}

#[derive(Debug)]
struct ArchiveJsonProcessError(ArchiveCommandError);

impl fmt::Display for ArchiveJsonProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for ArchiveJsonProcessError {}

pub(crate) fn json_process_error(error: &anyhow::Error) -> Option<serde_json::Value> {
    error
        .downcast_ref::<ArchiveJsonProcessError>()
        .map(|error| error.0.json_envelope())
}

pub(crate) fn cmd_session_retired(json_output: bool) -> anyhow::Result<()> {
    let sessions = retired_sessions()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("No retired durable cutex sessions are known.");
        return Ok(());
    }
    for session in sessions {
        println!(
            "{}  profile={}  path={}  retired={}  revision={}",
            session.agent,
            session.profile.as_deref().unwrap_or("-"),
            session.managed_path.as_deref().unwrap_or("-"),
            session.timestamp.as_deref().unwrap_or("-"),
            session.revision,
        );
    }
    Ok(())
}

pub(crate) fn cmd_session_retire(
    id: &str,
    reason: Option<&str>,
    json_output: bool,
) -> anyhow::Result<()> {
    let receipt = retire_for_command(id, reason).map_err(|error| {
        if json_output {
            anyhow::Error::new(ArchiveJsonProcessError(error))
        } else {
            anyhow::Error::new(error)
        }
    })?;
    present_receipt("Retired", &receipt, json_output)
}

pub(crate) fn cmd_session_restore(id: &str, json_output: bool) -> anyhow::Result<()> {
    let receipt = restore_for_command(id).map_err(|error| {
        if json_output {
            anyhow::Error::new(ArchiveJsonProcessError(error))
        } else {
            anyhow::Error::new(error)
        }
    })?;
    present_receipt("Restored", &receipt, json_output)
}

pub(crate) fn retired_sessions() -> anyhow::Result<Vec<ArchiveSessionView>> {
    let store = load_cutex_session_store()?;
    let mut sessions = store
        .sessions
        .values()
        .filter(|record| record.is_retired())
        .filter(|record| cutex_session_is_managed(record))
        .map(archive_session_view)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.cutex_session_id.cmp(&right.cutex_session_id));
    Ok(sessions)
}

pub(crate) fn retire(id: &str, reason: Option<&str>) -> anyhow::Result<ArchiveTransitionReceipt> {
    retire_for_command(id, reason).map_err(anyhow::Error::new)
}

fn retire_for_command(
    id: &str,
    reason: Option<&str>,
) -> Result<ArchiveTransitionReceipt, ArchiveCommandError> {
    let record = load_including_retired_for_command(id)?;
    if !cutex_session_is_managed(&record) {
        return Err(ArchiveCommandError::route_error(
            "session_not_managed",
            format!("Retire is only available for managed Cutex sessions: {id}"),
            json!({ "cutexSessionId": id }),
        ));
    }
    let mut params = json!({
        "expectedRevision": record.durable_revision(),
        "expectedRuntimeGeneration": record.runtime_generation,
    });
    if let Some(reason) = reason.filter(|reason| !reason.trim().is_empty()) {
        params["reason"] = json!(reason);
    }
    #[cfg(feature = "archive-conflict-test-hook")]
    await_archive_conflict_test_hook()?;
    mutate(&record.cutex_session_id, "cutex/session/retire", params)
}

pub(crate) fn restore(id: &str) -> anyhow::Result<ArchiveTransitionReceipt> {
    restore_for_command(id).map_err(anyhow::Error::new)
}

fn restore_for_command(id: &str) -> Result<ArchiveTransitionReceipt, ArchiveCommandError> {
    let record = load_including_retired_for_command(id)?;
    #[cfg(feature = "archive-conflict-test-hook")]
    await_archive_conflict_test_hook()?;
    mutate(
        &record.cutex_session_id,
        "cutex/session/restore",
        json!({ "expectedRevision": record.durable_revision() }),
    )
}

#[cfg(feature = "archive-conflict-test-hook")]
fn await_archive_conflict_test_hook() -> Result<(), ArchiveCommandError> {
    const TOKEN_ENV: &str = "CUTEX_ARCHIVE_CONFLICT_TEST_TOKEN";
    const MARKER_DIR: &str = "archive-conflict-test-hook";
    const TIMEOUT: Duration = Duration::from_secs(3);

    let Some(token) = std::env::var_os(TOKEN_ENV) else {
        return Ok(());
    };
    let token = token.to_str().ok_or_else(|| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook token must be valid UTF-8",
            json!({}),
        )
    })?;
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook token must contain only ASCII letters, digits, '-' or '_'",
            json!({}),
        ));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook requires an explicit HOME",
            json!({}),
        )
    })?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook HOME must be absolute",
            json!({}),
        ));
    }
    let canonical_home = fs::canonicalize(&home).map_err(|error| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            format!("archive conflict test hook cannot resolve HOME: {error}"),
            json!({}),
        )
    })?;
    let marker_dir = home.join(".cutex").join(MARKER_DIR);
    fs::create_dir_all(&marker_dir).map_err(|error| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            format!("archive conflict test hook cannot create marker directory: {error}"),
            json!({}),
        )
    })?;
    let marker_dir = fs::canonicalize(&marker_dir).map_err(|error| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            format!("archive conflict test hook cannot resolve marker directory: {error}"),
            json!({}),
        )
    })?;
    if !marker_dir.is_absolute() || !marker_dir.starts_with(&canonical_home) {
        return Err(ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook marker directory must remain inside HOME",
            json!({}),
        ));
    }
    let ready = marker_dir.join(format!("{token}.ready"));
    let release = marker_dir.join(format!("{token}.release"));
    if ready.exists() || release.exists() {
        return Err(ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            "archive conflict test hook found stale coordination markers",
            json!({}),
        ));
    }
    fs::write(&ready, token).map_err(|error| {
        ArchiveCommandError::route_error(
            "archive_conflict_test_hook_failed",
            format!("archive conflict test hook cannot write ready marker: {error}"),
            json!({}),
        )
    })?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if release.exists() {
            let release_token = fs::read_to_string(&release).map_err(|error| {
                ArchiveCommandError::route_error(
                    "archive_conflict_test_hook_failed",
                    format!("archive conflict test hook cannot read release marker: {error}"),
                    json!({}),
                )
            })?;
            if release_token == token {
                let _ = fs::remove_file(&ready);
                let _ = fs::remove_file(&release);
                return Ok(());
            }
            return Err(ArchiveCommandError::route_error(
                "archive_conflict_test_hook_failed",
                "archive conflict test hook release marker token did not match",
                json!({}),
            ));
        }
        if Instant::now() >= deadline {
            let _ = fs::remove_file(&ready);
            return Err(ArchiveCommandError::route_error(
                "archive_conflict_test_hook_failed",
                "archive conflict test hook timed out waiting for release marker",
                json!({}),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn load_including_retired_for_command(id: &str) -> Result<CutexSessionRecord, ArchiveCommandError> {
    let store = load_cutex_session_store().map_err(|error| {
        ArchiveCommandError::route_error(
            "session_lookup_failed",
            format!("failed to load durable Cutex sessions: {error:#}"),
            json!({ "cutexSessionId": id }),
        )
    })?;
    let key = cutex_session_key_for_user_id_including_retired(&store, id).ok_or_else(|| {
        ArchiveCommandError::route_error(
            "session_not_found",
            format!("cutex session is not known: {id}"),
            json!({ "cutexSessionId": id }),
        )
    })?;
    store.sessions.get(&key).cloned().ok_or_else(|| {
        ArchiveCommandError::route_error(
            "session_not_found",
            format!("cutex session disappeared while reloading: {key}"),
            json!({ "cutexSessionId": id }),
        )
    })
}

fn mutate(
    cutex_session_id: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<ArchiveTransitionReceipt, ArchiveCommandError> {
    let result =
        super::management_context::mutate_archive_session(cutex_session_id, method, params)
            .map_err(ArchiveCommandError::from_provider)?;
    Ok(ArchiveTransitionReceipt {
        cutex_session_id: required_string(&result, "cutexSessionId")?,
        revision: required_u64(&result, "revision")?,
        lifecycle: required_string(&result, "lifecycle")?,
        runtime_generation: required_u64(&result, "runtimeGeneration")?,
        status: required_string(&result, "status")?,
        timestamp: result
            .get("retiredAt")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

fn archive_session_view(record: &CutexSessionRecord) -> ArchiveSessionView {
    ArchiveSessionView {
        cutex_session_id: record.cutex_session_id.clone(),
        revision: record.durable_revision(),
        lifecycle: record.archive_state.label().to_string(),
        runtime_generation: record.runtime_generation,
        status: "offline",
        timestamp: record.retired_at.clone(),
        agent: cutex_session_display_name(record),
        profile: record.profile.clone(),
        managed_path: record.managed_cwd.as_deref().map(compact_home_path),
    }
}

fn present_receipt(
    verb: &str,
    receipt: &ArchiveTransitionReceipt,
    json_output: bool,
) -> anyhow::Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(receipt)?);
    } else {
        println!(
            "{verb} {} (revision {}, {}, runtime generation {})",
            receipt.cutex_session_id,
            receipt.revision,
            receipt.lifecycle,
            receipt.runtime_generation
        );
    }
    Ok(())
}

fn required_string(value: &serde_json::Value, field: &str) -> Result<String, ArchiveCommandError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            ArchiveCommandError::provider_receipt_error(format!(
                "archive provider returned no {field}"
            ))
        })
}

fn required_u64(value: &serde_json::Value, field: &str) -> Result<u64, ArchiveCommandError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ArchiveCommandError::provider_receipt_error(format!(
                "archive provider returned no {field}"
            ))
        })
}

#[cfg(test)]
fn format_archive_error(
    error: &cutex::management::v2::user_input::UserInputExecutionError,
) -> String {
    format_archive_error_fields(
        &error.message,
        &error.code,
        &error.details,
        error.outcome_unknown,
    )
}

fn format_archive_error_fields(
    message: &str,
    code: &str,
    details: &serde_json::Value,
    outcome_unknown: bool,
) -> String {
    let resync = outcome_unknown || details["resyncRequired"] == true;
    let suffix = resync
        .then_some("; resync required before retry")
        .unwrap_or("");
    format!("{message} ({code}){suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::session::model::CutexSessionArchiveState;

    #[test]
    fn archive_view_is_secret_free_and_offline() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.archive".to_string(),
            Some("thread".to_string()),
            "host".to_string(),
            "/tmp/project".to_string(),
            Some("profile".to_string()),
            "2026-08-14T00:00:00Z".to_string(),
        )
        .expect("record");
        record.archive_state = CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-14T00:01:00Z".to_string());
        record.runtime_generation = 7;

        let view = archive_session_view(&record);

        assert_eq!(view.lifecycle, "retired");
        assert_eq!(view.status, "offline");
        assert_eq!(view.runtime_generation, 7);
        let json = serde_json::to_value(view).expect("view json");
        assert!(json.get("authToken").is_none());
        assert!(json.get("defaultCliArgs").is_none());
    }

    #[test]
    fn outcome_unknown_error_requires_resync() {
        let error = cutex::management::v2::user_input::UserInputExecutionError {
            stage: "persist".to_string(),
            code: "persistence_uncertain".to_string(),
            message: "unknown".to_string(),
            retryable: false,
            details: json!({ "resyncRequired": true }),
            outcome_unknown: true,
        };
        assert_eq!(
            format_archive_error(&error),
            "unknown (persistence_uncertain); resync required before retry"
        );
    }

    #[test]
    fn json_process_error_preserves_typed_provider_fields() {
        let provider_error = cutex::management::v2::user_input::UserInputExecutionError {
            stage: "runtime".to_string(),
            code: "revision_conflict".to_string(),
            message: "runtime generation changed".to_string(),
            retryable: true,
            details: json!({
                "expectedRuntimeGeneration": 2,
                "currentRuntimeGeneration": 3,
                "resyncRequired": true,
            }),
            outcome_unknown: false,
        };
        let error = anyhow::Error::new(ArchiveJsonProcessError(
            ArchiveCommandError::from_provider(provider_error),
        ));

        assert_eq!(
            json_process_error(&error),
            Some(json!({
                "stage": "runtime",
                "code": "revision_conflict",
                "message": "runtime generation changed",
                "retryable": true,
                "details": {
                    "expectedRuntimeGeneration": 2,
                    "currentRuntimeGeneration": 3,
                    "resyncRequired": true,
                },
                "outcomeUnknown": false,
            }))
        );
    }
}
