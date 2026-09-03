use std::collections::BTreeMap;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Digest};

use crate::config::atomic::write_private_pretty_json_atomic;
use crate::role_revision::{Rfc3339, Sha256};

use super::{
    AgentActionId, AgentActionRecord, AgentManagementError, AgentManagementFailureEvent,
    AgentManagementPhaseEvent, AgentManagementStoreSchema, LegacyDirectorOwnershipImportReceipt,
    ManagedAgentRecord, ProjectAuthority, ProjectAuthorityReceipt, ProjectId,
};

const STORE_FILE: &str = "agent-management-v1.json";
const LOCK_FILE: &str = "agent-management-v1.lock";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentManagementSnapshot {
    pub schema: AgentManagementStoreSchema,
    pub store_revision: u64,
    pub projects: BTreeMap<ProjectId, ProjectAuthority>,
    /// Non-authoritative UI metadata. Its map key is always the canonical
    /// provider-owned project identity; none of these values participate in
    /// authorization.
    #[serde(default)]
    pub project_presentations: BTreeMap<ProjectId, super::ProjectPresentationSettings>,
    pub agents: BTreeMap<crate::role_revision::CutexSessionId, ManagedAgentRecord>,
    pub actions: BTreeMap<AgentActionId, AgentActionRecord>,
    #[serde(default)]
    pub phase_events: BTreeMap<String, AgentManagementPhaseEvent>,
    pub authority_receipts: BTreeMap<AgentActionId, ProjectAuthorityReceipt>,
    #[serde(default)]
    pub legacy_director_ownership_import_receipts:
        BTreeMap<AgentActionId, LegacyDirectorOwnershipImportReceipt>,
    pub failure_events: BTreeMap<String, AgentManagementFailureEvent>,
    /// Preserve additive provider fields across presentation-only writes so a
    /// newer store is not silently downgraded by this reader.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl AgentManagementSnapshot {
    fn empty() -> Self {
        Self {
            schema: AgentManagementStoreSchema::V1,
            store_revision: 0,
            projects: BTreeMap::new(),
            project_presentations: BTreeMap::new(),
            agents: BTreeMap::new(),
            actions: BTreeMap::new(),
            phase_events: BTreeMap::new(),
            authority_receipts: BTreeMap::new(),
            legacy_director_ownership_import_receipts: BTreeMap::new(),
            failure_events: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct AgentManagementStore {
    root: Arc<PathBuf>,
    process_lock: Arc<Mutex<()>>,
}

impl AgentManagementStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, AgentManagementError> {
        let root = root.into();
        prepare_private_root(&root)?;
        Ok(Self {
            root: Arc::new(root),
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn open_default() -> anyhow::Result<Self> {
        Self::open(
            crate::config::paths::runtime_dir()?
                .join("agent-management")
                .join("v1"),
        )
        .map_err(anyhow::Error::new)
    }

    pub fn snapshot(&self) -> Result<AgentManagementSnapshot, AgentManagementError> {
        self.with_state(false, |state| Ok((state.clone(), state, false)))
    }

    pub(crate) fn with_state<T>(
        &self,
        create: bool,
        operation: impl FnOnce(
            AgentManagementSnapshot,
        )
            -> Result<(AgentManagementSnapshot, T, bool), AgentManagementError>,
    ) -> Result<T, AgentManagementError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let lock_path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create);
        set_private_open_options(&mut options);
        let lock = match options.open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                let (state, value, write) = operation(AgentManagementSnapshot::empty())?;
                debug_assert!(!write);
                let _ = state;
                return Ok(value);
            }
            Err(_) => return Err(AgentManagementError::PersistenceUnavailable),
        };
        FileExt::lock_exclusive(&lock).map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let state = read_snapshot(&self.root)?;
        let (mut state, value, write) = operation(state)?;
        if write {
            state.store_revision = state
                .store_revision
                .checked_add(1)
                .filter(|revision| *revision <= crate::role_revision::MAX_JSON_SAFE_INTEGER)
                .ok_or(AgentManagementError::Conflict("store_revision_overflow"))?;
            write_snapshot(&self.root, &state)?;
        }
        Ok(value)
    }
}

pub(crate) fn request_sha256<T: Serialize>(value: &T) -> Result<Sha256, AgentManagementError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| AgentManagementError::InvalidRequest("request_not_serializable"))?;
    Sha256::new(format!("{:x}", Sha256Digest::digest(bytes)))
        .map_err(|_| AgentManagementError::InvalidStore)
}

pub(crate) fn now() -> Rfc3339 {
    canonical_timestamp(chrono::Utc::now())
}

fn canonical_timestamp(instant: chrono::DateTime<chrono::Utc>) -> Rfc3339 {
    Rfc3339::new(instant.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .expect("UTC timestamp is RFC3339")
}

fn prepare_private_root(root: &Path) -> Result<(), AgentManagementError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(AgentManagementError::InvalidStore)
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|_| AgentManagementError::PersistenceUnavailable)?
        }
        Err(_) => return Err(AgentManagementError::PersistenceUnavailable),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
    }
    Ok(())
}

fn read_snapshot(root: &Path) -> Result<AgentManagementSnapshot, AgentManagementError> {
    let path = root.join(STORE_FILE);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| AgentManagementError::InvalidStore),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(AgentManagementSnapshot::empty())
        }
        Err(_) => Err(AgentManagementError::PersistenceUnavailable),
    }
}

fn write_snapshot(
    root: &Path,
    snapshot: &AgentManagementSnapshot,
) -> Result<(), AgentManagementError> {
    write_private_pretty_json_atomic(&root.join(STORE_FILE), snapshot, "agent management store")
        .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
    #[cfg(unix)]
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
    Ok(())
}

fn set_private_open_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_second_timestamp_uses_canonical_zero_fraction_format() {
        let instant = chrono::DateTime::parse_from_rfc3339("2026-08-27T12:34:56.000Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            canonical_timestamp(instant).as_str(),
            "2026-08-27T12:34:56Z"
        );
    }

    #[test]
    fn empty_store_reopens_with_private_versioned_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "cutex-agent-management-store-{}",
            uuid::Uuid::new_v4()
        ));
        let store = AgentManagementStore::open(&root).expect("open");
        assert_eq!(store.snapshot().expect("snapshot").store_revision, 0);
        store
            .with_state(true, |mut state| {
                state.store_revision = 0;
                Ok((state, (), true))
            })
            .expect("write");
        drop(store);
        let reopened = AgentManagementStore::open(&root).expect("reopen");
        assert_eq!(reopened.snapshot().expect("snapshot").store_revision, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.join(STORE_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn accepted_v1_snapshot_without_import_receipts_remains_readable() {
        let mut value = serde_json::to_value(AgentManagementSnapshot::empty()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("legacy_director_ownership_import_receipts");

        let snapshot: AgentManagementSnapshot =
            serde_json::from_value(value).expect("accepted snapshot remains compatible");
        assert!(snapshot
            .legacy_director_ownership_import_receipts
            .is_empty());
    }
}
