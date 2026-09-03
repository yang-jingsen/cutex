use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use super::{IoStage, PersistencePhase, RepositoryError, RoleSeatStore};
use crate::role_revision::{validate_store, StoreRevision, StoreSchema};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FaultPoint {
    BeforeReplace,
    AfterReplace,
}

#[derive(Default)]
pub(super) struct FaultController {
    #[cfg(test)]
    point: std::sync::Mutex<Option<FaultPoint>>,
}

impl FaultController {
    #[cfg(test)]
    pub(super) fn new(point: FaultPoint) -> Self {
        Self {
            point: std::sync::Mutex::new(Some(point)),
        }
    }

    fn hit(&self, point: FaultPoint) -> bool {
        #[cfg(test)]
        {
            let mut configured = self.point.lock().expect("fault mutex");
            if configured.as_ref() == Some(&point) {
                *configured = None;
                return true;
            }
        }
        #[cfg(not(test))]
        let _ = point;
        false
    }
}

pub(super) enum PersistFailure {
    Definite(RepositoryError),
    Unknown(PersistencePhase),
}

pub(super) fn validate_root(root: &Path) -> Result<(), RepositoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = fs::symlink_metadata(root).map_err(|error| RepositoryError::Io {
            stage: IoStage::InspectRoot,
            kind: error.kind(),
        })?;
        if !metadata.file_type().is_dir() {
            return Err(RepositoryError::RootNotDirectory);
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(RepositoryError::RootOwnerMismatch);
        }
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err(RepositoryError::RootModeMismatch);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Err(RepositoryError::UnsupportedPlatform)
    }
}

pub(super) fn open_lock(path: &Path, create: bool) -> Result<Option<File>, RepositoryError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path) {
        Ok(file) => {
            validate_private_file(&file)?;
            Ok(Some(file))
        }
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RepositoryError::Io {
            stage: IoStage::OpenLock,
            kind: error.kind(),
        }),
    }
}

pub(super) fn load_store(path: &Path) -> Result<RoleSeatStore, RepositoryError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return empty_store(),
        Err(error) => {
            return Err(RepositoryError::Io {
                stage: IoStage::OpenStore,
                kind: error.kind(),
            });
        }
    };
    validate_private_file(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| RepositoryError::Io {
            stage: IoStage::ReadStore,
            kind: error.kind(),
        })?;
    let store: RoleSeatStore =
        serde_json::from_slice(&bytes).map_err(|_| RepositoryError::InvalidJson)?;
    validate_store(&store).map_err(|error| RepositoryError::InvalidStore { code: error.code })?;
    Ok(store)
}

pub(super) fn replace_store(
    root: &Path,
    target: &Path,
    bytes: &[u8],
    faults: &FaultController,
) -> Result<(), PersistFailure> {
    let temp = root.join(format!(
        ".{STORE_FILE_STEM}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = replace_store_inner(root, target, &temp, bytes, faults);
    if matches!(&result, Err(PersistFailure::Definite(_))) {
        let _ = fs::remove_file(&temp);
    }
    result
}

const STORE_FILE_STEM: &str = "role-seat-core-v1";

fn replace_store_inner(
    root: &Path,
    target: &Path,
    temp: &Path,
    bytes: &[u8],
    faults: &FaultController,
) -> Result<(), PersistFailure> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(temp).map_err(|error| {
        PersistFailure::Definite(RepositoryError::Io {
            stage: IoStage::CreateTemp,
            kind: error.kind(),
        })
    })?;
    validate_private_file(&file).map_err(PersistFailure::Definite)?;
    file.write_all(bytes).map_err(|error| {
        PersistFailure::Definite(RepositoryError::Io {
            stage: IoStage::WriteTemp,
            kind: error.kind(),
        })
    })?;
    file.sync_all().map_err(|error| {
        PersistFailure::Definite(RepositoryError::Io {
            stage: IoStage::SyncTemp,
            kind: error.kind(),
        })
    })?;
    drop(file);
    if faults.hit(FaultPoint::BeforeReplace) {
        return Err(PersistFailure::Definite(
            RepositoryError::InjectedPreReplaceFailure,
        ));
    }
    fs::rename(temp, target).map_err(|error| {
        PersistFailure::Definite(RepositoryError::Io {
            stage: IoStage::Replace,
            kind: error.kind(),
        })
    })?;
    if faults.hit(FaultPoint::AfterReplace) {
        return Err(PersistFailure::Unknown(PersistencePhase::AfterReplace));
    }
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| PersistFailure::Unknown(PersistencePhase::ParentDirectorySync))?;
    Ok(())
}

fn empty_store() -> Result<RoleSeatStore, RepositoryError> {
    let store = RoleSeatStore {
        schema: StoreSchema::V1,
        store_revision: StoreRevision::new(1).expect("revision one is valid"),
        family: None,
        idempotency: Default::default(),
    };
    validate_store(&store).map_err(|error| RepositoryError::InvalidStore { code: error.code })?;
    Ok(store)
}

fn validate_private_file(file: &File) -> Result<(), RepositoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = file.metadata().map_err(|error| RepositoryError::Io {
            stage: IoStage::InspectRoot,
            kind: error.kind(),
        })?;
        if !metadata.file_type().is_file() {
            return Err(RepositoryError::PrivateFileNotRegular);
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(RepositoryError::PrivateFileOwnerMismatch);
        }
        if metadata.permissions().mode() & 0o7777 != 0o600 {
            return Err(RepositoryError::PrivateFileModeMismatch);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Err(RepositoryError::UnsupportedPlatform)
    }
}
