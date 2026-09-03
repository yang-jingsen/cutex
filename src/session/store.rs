//! Durable cutex session store persistence.

use std::fmt;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use fs2::FileExt;

use crate::config::atomic::write_pretty_json_atomic;
use crate::config::paths::config_dir;
use crate::session::model::CutexSessionStore;

const CUTEX_SESSIONS_LOCK_FILE: &str = "cutex-sessions.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutexSessionStoreRevisionConflict {
    pub expected: u64,
    pub actual: u64,
}

impl fmt::Display for CutexSessionStoreRevisionConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cutex session store revision conflict: expected {}, current {}; reload before retrying",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CutexSessionStoreRevisionConflict {}

pub fn cutex_sessions_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("cutex-sessions.json"))
}

pub fn load_cutex_session_store() -> anyhow::Result<CutexSessionStore> {
    let path = cutex_sessions_path()?;
    load_cutex_session_store_from_path(&path)
}

pub fn save_cutex_session_store(store: &CutexSessionStore) -> anyhow::Result<()> {
    let path = cutex_sessions_path()?;
    save_cutex_session_store_to_path(&path, store)
}

pub(crate) fn load_cutex_session_store_from_path(path: &Path) -> anyhow::Result<CutexSessionStore> {
    match fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse cutex session store: {}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(CutexSessionStore::default()),
        Err(error) => Err(error)
            .with_context(|| format!("Failed to read cutex session store: {}", path.display())),
    }
}

pub(crate) fn save_cutex_session_store_to_path(
    path: &Path,
    store: &CutexSessionStore,
) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("cutex session store path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "Failed to create cutex session store directory: {}",
            parent.display()
        )
    })?;
    let lock = open_store_lock(&parent.join(CUTEX_SESSIONS_LOCK_FILE))?;
    FileExt::lock_exclusive(&lock).context("Failed to lock cutex session store")?;

    let expected_revision = store.store_revision.get();
    let result = (|| {
        let current = load_cutex_session_store_from_path(path)?;
        let current_revision = current.store_revision.get();
        if current_revision != expected_revision {
            return Err(CutexSessionStoreRevisionConflict {
                expected: expected_revision,
                actual: current_revision,
            }
            .into());
        }
        let next_revision = current_revision
            .checked_add(1)
            .context("cutex session store revision overflow")?;
        store.store_revision.set(next_revision);
        if let Err(error) = write_pretty_json_atomic(path, store, "cutex session store") {
            store.store_revision.set(expected_revision);
            return Err(error);
        }
        Ok(())
    })();
    let unlock_result = FileExt::unlock(&lock).context("Failed to unlock cutex session store");
    match (result, unlock_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn open_store_lock(path: &Path) -> anyhow::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    options.open(path).with_context(|| {
        format!(
            "Failed to open cutex session store lock: {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::model::CutexSessionRecord;

    fn record(name: &str) -> CutexSessionRecord {
        CutexSessionRecord::new_at(
            format!("cutex.{name}"),
            Some(format!("thread-{name}")),
            "host-a".to_string(),
            format!("/tmp/{name}"),
            None,
            "2026-08-10T00:00:00Z".to_string(),
        )
        .expect("record")
    }

    #[test]
    fn concurrent_target_update_cannot_be_overwritten() {
        let root =
            std::env::temp_dir().join(format!("cutex-session-store-cas-{}", uuid::Uuid::new_v4()));
        let path = root.join("cutex-sessions.json");
        let mut initial = CutexSessionStore::default();
        initial
            .sessions
            .insert("cutex.target".to_string(), record("target"));
        save_cutex_session_store_to_path(&path, &initial).expect("save initial store");
        assert_eq!(initial.store_revision.get(), 1);

        let mut first = load_cutex_session_store_from_path(&path).expect("load first writer");
        let mut stale = load_cutex_session_store_from_path(&path).expect("load stale writer");
        let first_target = first
            .sessions
            .get_mut("cutex.target")
            .expect("first target");
        first_target.display_name_hint = Some("newer-name".to_string());
        first_target
            .bump_durable_revision()
            .expect("bump first target");
        let stale_target = stale
            .sessions
            .get_mut("cutex.target")
            .expect("stale target");
        stale_target.archive_state = crate::session::model::CutexSessionArchiveState::Retired;
        stale_target.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        stale_target
            .bump_durable_revision()
            .expect("bump stale target");

        save_cutex_session_store_to_path(&path, &first).expect("save first writer");
        let error = save_cutex_session_store_to_path(&path, &stale)
            .expect_err("stale writer must conflict");
        let conflict = error
            .downcast_ref::<CutexSessionStoreRevisionConflict>()
            .expect("typed store conflict");
        assert_eq!(
            *conflict,
            CutexSessionStoreRevisionConflict {
                expected: 1,
                actual: 2,
            }
        );
        assert_eq!(stale.store_revision.get(), 1);

        let persisted = load_cutex_session_store_from_path(&path).expect("reload persisted store");
        assert_eq!(persisted.store_revision.get(), 2);
        let target = persisted
            .sessions
            .get("cutex.target")
            .expect("persisted target");
        assert_eq!(target.display_name_hint.as_deref(), Some("newer-name"));
        assert!(target.is_active());
        assert_eq!(target.durable_revision(), 2);
        fs::remove_dir_all(root).expect("remove test store");
    }
}
