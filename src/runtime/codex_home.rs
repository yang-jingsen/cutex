//! Codex home session lookup helpers for runtime resume planning.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};

use crate::config::paths::host_codex_home_dir;
use crate::role_revision::Rfc3339;

pub fn codex_session_exists_in_home(session_id: &str) -> anyhow::Result<bool> {
    let codex_home = host_codex_home_dir()?;
    if codex_session_index_contains(&codex_home, session_id)? {
        return Ok(true);
    }
    Ok(codex_session_rollout_file_exists(
        &codex_home.join("sessions"),
        session_id,
    )?)
}

/// Conservatively checks the complete native Codex session sources for a
/// session created during one historical bootstrap attempt. This is an
/// absence proof: malformed or unreadable evidence is an error, never empty.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeSessionCorrelation {
    ProvenAbsent,
    Present { session_id: String },
    Ambiguous { reason: String },
}

#[derive(Clone, Debug)]
struct NativeSessionCandidate {
    session_id: String,
    cwd: Option<PathBuf>,
}

/// Correlates all native Codex evidence in the attempt window with the exact
/// reserved managed cwd. An explicitly different cwd is unrelated; evidence
/// that cannot be correlated remains ambiguous rather than authorizing retry.
pub fn correlate_codex_session_between(
    started_at: &Rfc3339,
    failed_at: &Rfc3339,
    managed_cwd: &Path,
) -> anyhow::Result<NativeSessionCorrelation> {
    let codex_home = host_codex_home_dir()?;
    correlate_codex_session_between_in_home(&codex_home, started_at, failed_at, managed_cwd)
}

/// Performs the same conservative correlation against an explicitly selected
/// runtime's actual Codex home (for example, the host-mounted Docker home).
pub fn correlate_codex_session_between_in_home(
    codex_home: &Path,
    started_at: &Rfc3339,
    failed_at: &Rfc3339,
    managed_cwd: &Path,
) -> anyhow::Result<NativeSessionCorrelation> {
    let start = DateTime::parse_from_rfc3339(started_at.as_str())?.with_timezone(&Utc);
    let end = DateTime::parse_from_rfc3339(failed_at.as_str())?.with_timezone(&Utc);
    if end < start {
        anyhow::bail!("native bootstrap reconciliation window is reversed");
    }
    // Rollout file names have second precision. Expanding one second on both
    // sides can only produce a safe false-positive fence.
    let start = start - Duration::seconds(1);
    let end = end + Duration::seconds(1);
    let mut candidates = session_index_entries_between(codex_home, start, end)?;
    for rollout in rollout_entries_between(&codex_home.join("sessions"), start, end)? {
        match candidates.get_mut(&rollout.session_id) {
            Some(candidate) => {
                if let (Some(index_cwd), Some(rollout_cwd)) = (&candidate.cwd, &rollout.cwd) {
                    if index_cwd != rollout_cwd {
                        return Ok(NativeSessionCorrelation::Ambiguous {
                            reason: "native index and rollout cwd markers conflict".to_string(),
                        });
                    }
                }
                if candidate.cwd.is_none() {
                    candidate.cwd = rollout.cwd;
                }
            }
            None => {
                candidates.insert(rollout.session_id.clone(), rollout);
            }
        }
    }
    let uncorrelated = candidates
        .values()
        .filter(|candidate| candidate.cwd.is_none())
        .map(|candidate| candidate.session_id.as_str())
        .collect::<Vec<_>>();
    if !uncorrelated.is_empty() {
        return Ok(NativeSessionCorrelation::Ambiguous {
            reason: format!(
                "native session(s) {} have no managed-cwd correlation marker",
                uncorrelated.join(", ")
            ),
        });
    }
    let matching = candidates
        .values()
        .filter(|candidate| candidate.cwd.as_deref() == Some(managed_cwd))
        .map(|candidate| candidate.session_id.clone())
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] => Ok(NativeSessionCorrelation::ProvenAbsent),
        [session_id] => Ok(NativeSessionCorrelation::Present {
            session_id: session_id.clone(),
        }),
        _ => Ok(NativeSessionCorrelation::Ambiguous {
            reason: format!(
                "multiple native sessions match the exact managed cwd: {}",
                matching.join(", ")
            ),
        }),
    }
}

fn session_index_entries_between(
    codex_home: &Path,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<BTreeMap<String, NativeSessionCandidate>> {
    let mut candidates = BTreeMap::new();
    let path = codex_home.join("session_index.jsonl");
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(candidates),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to open session index: {}", path.display()));
        }
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse native session index: {}", path.display()))?;
        let Some(session_id) = value.get("id").and_then(serde_json::Value::as_str) else {
            anyhow::bail!("native session index entry omitted id: {}", path.display());
        };
        if session_id.trim().is_empty() {
            anyhow::bail!(
                "native session index entry has empty id: {}",
                path.display()
            );
        }
        let timestamp = [
            "timestamp",
            "created_at",
            "createdAt",
            "updated_at",
            "updatedAt",
        ]
        .into_iter()
        .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
        .ok_or_else(|| anyhow::anyhow!("native session index entry omitted timestamp"))?;
        let timestamp = DateTime::parse_from_rfc3339(timestamp)
            .with_context(|| format!("native session index timestamp is invalid: {timestamp}"))?
            .with_timezone(&Utc);
        if timestamp >= start && timestamp <= end {
            let cwd = value
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from);
            candidates.insert(
                session_id.to_string(),
                NativeSessionCandidate {
                    session_id: session_id.to_string(),
                    cwd,
                },
            );
        }
    }
    Ok(candidates)
}

fn rollout_entries_between(
    root: &Path,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> anyhow::Result<Vec<NativeSessionCandidate>> {
    let mut candidates = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(candidates),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to read Codex sessions dir: {}", root.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            anyhow::bail!(
                "native session source contains a symlink: {}",
                path.display()
            );
        }
        if file_type.is_dir() {
            candidates.extend(rollout_entries_between(&path, start, end)?);
            continue;
        }
        if !file_type.is_file() {
            anyhow::bail!(
                "native session source has an unsupported entry: {}",
                path.display()
            );
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            anyhow::bail!("native rollout file name is not UTF-8: {}", path.display());
        };
        if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
            continue;
        }
        let wall_clock = file_name
            .get(8..27)
            .ok_or_else(|| anyhow::anyhow!("native rollout file omitted timestamp"))?;
        let wall_clock = NaiveDateTime::parse_from_str(wall_clock, "%Y-%m-%dT%H-%M-%S")
            .with_context(|| {
                format!("native rollout filename timestamp is invalid: {file_name}")
            })?;
        // Codex rollout filenames use local wall-clock time without an offset.
        // Use that value only as a wide discovery bound, never as identity or
        // attempt-time proof. Every civil time-zone offset is inside 24 hours;
        // the authoritative embedded RFC3339 metadata below decides inclusion.
        let wall_clock_as_utc = Utc.from_utc_datetime(&wall_clock);
        if wall_clock_as_utc < start - Duration::hours(24)
            || wall_clock_as_utc > end + Duration::hours(24)
        {
            continue;
        }
        let (created_at, event_timestamp, candidate) = rollout_session_metadata(&path)?;
        let creation_in_window = created_at >= start && created_at <= end;
        let event_in_window = event_timestamp >= start && event_timestamp <= end;
        match (creation_in_window, event_in_window) {
            (true, true) => candidates.push(candidate),
            (false, false) => {}
            _ => anyhow::bail!(
                "native rollout session_meta timestamps straddle the reconciliation window: {}",
                path.display()
            ),
        }
    }
    Ok(candidates)
}

fn rollout_session_metadata(
    path: &Path,
) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>, NativeSessionCandidate)> {
    let file = fs::File::open(path)
        .with_context(|| format!("Failed to open native rollout: {}", path.display()))?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).with_context(|| {
            format!(
                "Failed to parse native rollout metadata: {}",
                path.display()
            )
        })?;
        if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
            continue;
        }
        let event_timestamp = value
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "native rollout session_meta omitted event timestamp: {}",
                    path.display()
                )
            })?;
        let event_timestamp = DateTime::parse_from_rfc3339(event_timestamp)
            .with_context(|| {
                format!(
                    "native rollout session_meta event timestamp is invalid: {}",
                    path.display()
                )
            })?
            .with_timezone(&Utc);
        let payload = value.get("payload").ok_or_else(|| {
            anyhow::anyhow!(
                "native rollout session_meta omitted payload: {}",
                path.display()
            )
        })?;
        let created_at = payload
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "native rollout session_meta omitted creation timestamp: {}",
                    path.display()
                )
            })?;
        let created_at = DateTime::parse_from_rfc3339(created_at)
            .with_context(|| {
                format!(
                    "native rollout session_meta creation timestamp is invalid: {}",
                    path.display()
                )
            })?
            .with_timezone(&Utc);
        let event_delay = event_timestamp.signed_duration_since(created_at);
        if event_delay < Duration::zero() || event_delay > Duration::seconds(5) {
            anyhow::bail!(
                "native rollout session_meta timestamps conflict: {}",
                path.display()
            );
        }
        let session_id = payload
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("native rollout session_meta omitted id: {}", path.display())
            })?;
        if let Some(legacy_session_id) = payload
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            // Modern subagent rollouts use `id` for the child thread while
            // retaining the parent thread in the legacy `session_id` field.
            // That is a parent relationship, not an identity conflict. Keep
            // rejecting all other mismatches so malformed evidence cannot
            // weaken the absence proof used to authorize an exact retry.
            let declared_parent = [
                payload
                    .get("forked_from_id")
                    .and_then(serde_json::Value::as_str),
                payload
                    .get("parent_thread_id")
                    .and_then(serde_json::Value::as_str),
                payload
                    .pointer("/source/subagent/thread_spawn/parent_thread_id")
                    .and_then(serde_json::Value::as_str),
            ]
            .into_iter()
            .flatten()
            .any(|value| value == legacy_session_id);
            let is_subagent_parent = payload
                .get("thread_source")
                .and_then(serde_json::Value::as_str)
                == Some("subagent")
                && declared_parent;
            if legacy_session_id != session_id && !is_subagent_parent {
                anyhow::bail!(
                    "native rollout session_meta session identities conflict: {}",
                    path.display()
                );
            }
        }
        let cwd = payload
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from);
        return Ok((
            created_at,
            event_timestamp,
            NativeSessionCandidate {
                session_id: session_id.to_string(),
                cwd,
            },
        ));
    }
    anyhow::bail!("native rollout omitted session_meta: {}", path.display())
}

fn codex_session_index_contains(codex_home: &Path, session_id: &str) -> anyhow::Result<bool> {
    let path = codex_home.join("session_index.jsonl");
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to open session index: {}", path.display()));
        }
    };
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(serde_json::Value::as_str) == Some(session_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn codex_session_rollout_file_exists(root: &Path, session_id: &str) -> anyhow::Result<bool> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to read Codex sessions dir: {}", root.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            if codex_session_rollout_file_exists(&path, session_id)? {
                return Ok(true);
            }
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if file_name.starts_with("rollout-")
            && file_name.ends_with(".jsonl")
            && file_name.contains(session_id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> (Rfc3339, Rfc3339) {
        (
            Rfc3339::new("2026-08-30T01:02:03Z").unwrap(),
            Rfc3339::new("2026-08-30T01:02:05Z").unwrap(),
        )
    }

    fn root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cutex-native-reconciliation-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn native_reconciliation_correlates_rollout_cwd_and_ignores_unrelated_session() {
        let index_root = root("correlated");
        let rollout_day = index_root
            .join("sessions")
            .join("2026")
            .join("08")
            .join("30");
        fs::create_dir_all(&rollout_day).unwrap();
        fs::write(
            index_root.join("session_index.jsonl"),
            concat!(
                "{\"id\":\"native-matching\",\"timestamp\":\"2026-08-30T01:02:04Z\"}\n",
                "{\"id\":\"native-unrelated\",\"timestamp\":\"2026-08-30T01:02:04Z\"}\n"
            ),
        )
        .unwrap();
        fs::write(
            rollout_day.join("rollout-2026-08-30T01-02-04-native-matching.jsonl"),
            "{\"timestamp\":\"2026-08-30T01:02:04.100Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"native-matching\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/managed/worker\"}}\n",
        )
        .unwrap();
        fs::write(
            rollout_day.join("rollout-2026-08-30T01-02-04-native-unrelated.jsonl"),
            "{\"timestamp\":\"2026-08-30T01:02:04.100Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"native-unrelated\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/other/worker\"}}\n",
        )
        .unwrap();
        let (started_at, failed_at) = window();
        assert_eq!(
            correlate_codex_session_between_in_home(
                &index_root,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::Present {
                session_id: "native-matching".to_string()
            }
        );
        assert_eq!(
            correlate_codex_session_between_in_home(
                &index_root,
                &started_at,
                &failed_at,
                Path::new("/absent/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::ProvenAbsent
        );
        fs::remove_dir_all(index_root).unwrap();
    }

    #[test]
    fn native_reconciliation_reports_uncorrelated_incident_shape_and_rejects_malformed_evidence() {
        let empty_root = root("empty");
        fs::create_dir_all(&empty_root).unwrap();
        let (started_at, failed_at) = window();
        assert_eq!(
            correlate_codex_session_between_in_home(
                &empty_root,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::ProvenAbsent
        );

        let incident_root = root("incident");
        fs::create_dir_all(&incident_root).unwrap();
        fs::write(
            incident_root.join("session_index.jsonl"),
            "{\"id\":\"historical-native\",\"timestamp\":\"2026-08-30T01:02:04Z\"}\n",
        )
        .unwrap();
        assert!(matches!(
            correlate_codex_session_between_in_home(
                &incident_root,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::Ambiguous { reason }
                if reason.contains("no managed-cwd correlation marker")
        ));

        let malformed_root = root("malformed");
        fs::create_dir_all(&malformed_root).unwrap();
        fs::write(malformed_root.join("session_index.jsonl"), "not-json\n").unwrap();
        assert!(correlate_codex_session_between_in_home(
            &malformed_root,
            &started_at,
            &failed_at,
            Path::new("/managed/worker")
        )
        .is_err());

        fs::remove_dir_all(empty_root).unwrap();
        fs::remove_dir_all(incident_root).unwrap();
        fs::remove_dir_all(malformed_root).unwrap();
    }

    #[test]
    fn native_reconciliation_rejects_multiple_exact_cwd_sessions() {
        let codex_home = root("multiple-exact");
        let rollout_day = codex_home
            .join("sessions")
            .join("2026")
            .join("08")
            .join("30");
        fs::create_dir_all(&rollout_day).unwrap();
        for session_id in ["native-one", "native-two"] {
            fs::write(
                rollout_day.join(format!(
                    "rollout-2026-08-30T01-02-04-{session_id}.jsonl"
                )),
                format!(
                    "{{\"timestamp\":\"2026-08-30T01:02:04.100Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/managed/worker\"}}}}\n"
                ),
            )
            .unwrap();
        }
        let (started_at, failed_at) = window();
        assert!(matches!(
            correlate_codex_session_between_in_home(
                &codex_home,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::Ambiguous { reason }
                if reason.contains("multiple native sessions")
        ));
        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn native_reconciliation_uses_embedded_utc_time_not_local_wall_clock_filename() {
        let codex_home = root("local-filename-utc-metadata");
        let rollout_day = codex_home
            .join("sessions")
            .join("2026")
            .join("08")
            .join("30");
        fs::create_dir_all(&rollout_day).unwrap();
        let session_id = "01a05067-fb62-7d12-ad3e-bbd8cfd35e95";
        let managed_cwd = "/home/example/Projects/cutex/agent-home/r23-toolchain-review-glm-r1";
        fs::write(
            rollout_day.join(format!(
                "rollout-2026-08-30T12-03-06-{session_id}.jsonl"
            )),
            format!(
                "{{\"timestamp\":\"2026-08-30T02:03:07.028Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"{session_id}\",\"session_id\":\"{session_id}\",\"timestamp\":\"2026-08-30T02:03:06.981Z\",\"cwd\":\"{managed_cwd}\"}}}}\n"
            ),
        )
        .unwrap();
        let started_at = Rfc3339::new("2026-08-30T02:03:05Z").unwrap();
        let failed_at = Rfc3339::new("2026-08-30T02:03:19Z").unwrap();

        assert_eq!(
            correlate_codex_session_between_in_home(
                &codex_home,
                &started_at,
                &failed_at,
                Path::new(managed_cwd)
            )
            .unwrap(),
            NativeSessionCorrelation::Present {
                session_id: session_id.to_string()
            }
        );
        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn native_reconciliation_accepts_subagent_parent_session_id_without_matching_child() {
        let codex_home = root("subagent-parent-session-id");
        let rollout_day = codex_home
            .join("sessions")
            .join("2026")
            .join("08")
            .join("30");
        fs::create_dir_all(&rollout_day).unwrap();
        fs::write(
            rollout_day.join("rollout-2026-08-30T01-02-04-child-native.jsonl"),
            "{\"timestamp\":\"2026-08-30T01:02:04.100Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"child-native\",\"session_id\":\"parent-native\",\"forked_from_id\":\"parent-native\",\"parent_thread_id\":\"parent-native\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/director\",\"thread_source\":\"subagent\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent-native\"}}}}}\n",
        )
        .unwrap();
        let (started_at, failed_at) = window();

        assert_eq!(
            correlate_codex_session_between_in_home(
                &codex_home,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .unwrap(),
            NativeSessionCorrelation::ProvenAbsent
        );
        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn native_reconciliation_rejects_unrelated_legacy_session_id_mismatch() {
        let codex_home = root("unrelated-session-id-mismatch");
        let rollout_day = codex_home.join("sessions").join("2026");
        fs::create_dir_all(&rollout_day).unwrap();
        fs::write(
            rollout_day.join("rollout-2026-08-30T01-02-04-native.jsonl"),
            "{\"timestamp\":\"2026-08-30T01:02:04.100Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"native\",\"session_id\":\"different-native\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/managed/worker\"}}\n",
        )
        .unwrap();
        let (started_at, failed_at) = window();

        assert!(correlate_codex_session_between_in_home(
            &codex_home,
            &started_at,
            &failed_at,
            Path::new("/managed/worker")
        )
        .is_err());
        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn native_reconciliation_rejects_missing_malformed_and_conflicting_embedded_timestamps() {
        let (started_at, failed_at) = window();
        let cases = [
            (
                "missing-event-time",
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"native\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/managed/worker\"}}\n",
            ),
            (
                "malformed-creation-time",
                "{\"timestamp\":\"2026-08-30T01:02:04Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"native\",\"timestamp\":\"not-rfc3339\",\"cwd\":\"/managed/worker\"}}\n",
            ),
            (
                "conflicting-times",
                "{\"timestamp\":\"2026-08-30T01:02:20Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"native\",\"timestamp\":\"2026-08-30T01:02:04Z\",\"cwd\":\"/managed/worker\"}}\n",
            ),
        ];
        for (label, metadata) in cases {
            let codex_home = root(label);
            let rollout_day = codex_home.join("sessions").join("2026");
            fs::create_dir_all(&rollout_day).unwrap();
            fs::write(
                rollout_day.join("rollout-2026-08-30T11-02-04-native.jsonl"),
                metadata,
            )
            .unwrap();
            assert!(correlate_codex_session_between_in_home(
                &codex_home,
                &started_at,
                &failed_at,
                Path::new("/managed/worker")
            )
            .is_err());
            fs::remove_dir_all(codex_home).unwrap();
        }
    }
}
