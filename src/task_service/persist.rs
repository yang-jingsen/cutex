#[cfg(unix)]
use std::ffi::CString;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

use super::model::{
    validate_store, IoStage, PersistencePhase, RecoveryIntent, RecoveryPhase, TaskServiceError,
    TaskStore,
};

pub(super) const LOCK_FILE: &str = "task-service-v1.lock";
pub(super) const JOURNAL_FILE: &str = "task-service-v1.events.jsonl";
pub(super) const SNAPSHOT_FILE: &str = "task-service-v1.json";
pub(super) const RECOVERY_FILE: &str = "task-service-v1.recovery";

pub(super) struct RootHandle {
    pub(super) path: PathBuf,
    directory: File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    identity: crate::platform::private_fs::FileIdentity,
}

impl RootHandle {
    pub(super) fn validate_binding(&self) -> Result<(), TaskServiceError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            let rebound = match open_validated_root(&self.path) {
                Ok(directory) => directory,
                Err(_) => return Err(TaskServiceError::RootBindingChanged),
            };
            let metadata = rebound
                .metadata()
                .map_err(|_| TaskServiceError::RootBindingChanged)?;
            if metadata.dev() != self.device || metadata.ino() != self.inode {
                return Err(TaskServiceError::RootBindingChanged);
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            crate::platform::private_fs::validate_binding(&self.path, self.identity)
                .map_err(|_| TaskServiceError::RootBindingChanged)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FaultPoint {
    BeforeJournalAppend,
    PartialJournalWrite,
    AfterJournalWrite,
    AfterJournalSync,
    BeforeSnapshotRename,
    AfterSnapshotRename,
    AfterSnapshotParentSync,
    AfterRecoveryIntentRename,
    AfterRecoveryIntentParentSync,
    AfterRecoveryTruncate,
    PartialRecoveryRecordWrite,
    AfterRecoveryRecordSync,
    AfterRecoverySnapshotRename,
    AfterRecoveryIntentRemove,
}

#[derive(Default)]
pub(super) struct FaultController {
    configured: Mutex<Option<FaultPoint>>,
}

impl FaultController {
    #[cfg(test)]
    pub(super) fn new(point: FaultPoint) -> Self {
        Self {
            configured: Mutex::new(Some(point)),
        }
    }

    pub(super) fn hit(&self, point: FaultPoint) -> bool {
        let mut configured = self.configured.lock().expect("fault mutex");
        if configured.as_ref() == Some(&point) {
            *configured = None;
            true
        } else {
            false
        }
    }
}

pub(super) enum AppendFailure {
    Unknown(PersistencePhase),
}

pub(super) fn validate_root(root: &Path) -> Result<RootHandle, TaskServiceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let directory = open_validated_root(root)?;
        let metadata = directory.metadata().map_err(|error| TaskServiceError::Io {
            stage: IoStage::InspectRoot,
            kind: error.kind(),
        })?;
        Ok(RootHandle {
            path: root.to_path_buf(),
            directory,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let (directory, identity) =
            crate::platform::private_fs::secure_directory(root).map_err(map_root_error)?;
        Ok(RootHandle {
            path: root.to_path_buf(),
            directory,
            identity,
        })
    }
}

pub(super) fn open_lock(root: &RootHandle, create: bool) -> Result<Option<File>, TaskServiceError> {
    root.validate_binding()?;
    let flags = if create {
        libc::O_RDWR | libc::O_CREAT
    } else {
        libc::O_RDONLY
    };
    match open_child(root, LOCK_FILE, flags, 0o600, IoStage::OpenLock) {
        Ok(file) => {
            validate_private_file(&file)?;
            Ok(Some(file))
        }
        Err(TaskServiceError::Io {
            kind: io::ErrorKind::NotFound,
            ..
        }) if !create => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn load_snapshot(root: &RootHandle) -> Result<Option<TaskStore>, TaskServiceError> {
    let Some(bytes) = read_private(
        root,
        SNAPSHOT_FILE,
        IoStage::OpenSnapshot,
        IoStage::ReadSnapshot,
    )?
    else {
        return Ok(None);
    };
    let store: TaskStore =
        serde_json::from_slice(&bytes).map_err(|_| TaskServiceError::InvalidJson)?;
    validate_store(&store).map_err(|code| TaskServiceError::InvalidStore { code })?;
    Ok(Some(store))
}

pub(super) fn read_journal(root: &RootHandle) -> Result<Option<Vec<u8>>, TaskServiceError> {
    read_private(
        root,
        JOURNAL_FILE,
        IoStage::OpenJournal,
        IoStage::ReadJournal,
    )
}

pub(super) fn load_recovery(root: &RootHandle) -> Result<Option<RecoveryIntent>, TaskServiceError> {
    let Some(bytes) = read_private(
        root,
        RECOVERY_FILE,
        IoStage::OpenRecoveryIntent,
        IoStage::ReadRecoveryIntent,
    )?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| TaskServiceError::InvalidRecoveryIntent {
            code: super::model::ValidationCode::InvalidRecoveryIntent,
        })
}

pub(super) fn append_transition(
    root: &RootHandle,
    file: &mut File,
    line: &[u8],
    faults: &FaultController,
) -> Result<(), AppendFailure> {
    root.validate_binding()
        .map_err(|_| AppendFailure::Unknown(PersistencePhase::JournalWrite))?;
    if faults.hit(FaultPoint::PartialJournalWrite) {
        let count = (line.len() / 2).max(1).min(line.len().saturating_sub(1));
        file.write_all(&line[..count])
            .map_err(|_| AppendFailure::Unknown(PersistencePhase::JournalWrite))?;
        return Err(AppendFailure::Unknown(PersistencePhase::JournalWrite));
    }
    file.write_all(line)
        .map_err(|_| AppendFailure::Unknown(PersistencePhase::JournalWrite))?;
    if faults.hit(FaultPoint::AfterJournalWrite) {
        return Err(AppendFailure::Unknown(PersistencePhase::JournalWrite));
    }
    file.sync_all()
        .map_err(|_| AppendFailure::Unknown(PersistencePhase::JournalSync))?;
    if faults.hit(FaultPoint::AfterJournalSync) {
        return Err(AppendFailure::Unknown(PersistencePhase::JournalSync));
    }
    Ok(())
}

pub(super) fn prepare_journal_append(root: &RootHandle) -> Result<File, TaskServiceError> {
    open_journal_append(root)
}

pub(super) fn replace_snapshot_after_transition(
    root: &RootHandle,
    bytes: &[u8],
    faults: &FaultController,
) -> Result<(), PersistencePhase> {
    if atomic_replace(
        root,
        SNAPSHOT_FILE,
        bytes,
        Some((faults, FaultPoint::BeforeSnapshotRename)),
        Some((faults, FaultPoint::AfterSnapshotRename)),
        IoStage::ReplaceSnapshot,
    )
    .is_err()
    {
        return Err(PersistencePhase::SnapshotReplace);
    }
    if faults.hit(FaultPoint::AfterSnapshotParentSync) {
        return Err(PersistencePhase::SnapshotParentSync);
    }
    Ok(())
}

pub(super) fn persist_replayed_snapshot(
    root: &RootHandle,
    bytes: &[u8],
) -> Result<(), TaskServiceError> {
    atomic_replace(
        root,
        SNAPSHOT_FILE,
        bytes,
        None,
        None,
        IoStage::ReplaceSnapshot,
    )
}

pub(super) fn persist_recovery_intent(
    root: &RootHandle,
    intent: &RecoveryIntent,
    faults: &FaultController,
) -> Result<(), TaskServiceError> {
    if private_exists(root, RECOVERY_FILE, IoStage::OpenRecoveryIntent)? {
        return Err(TaskServiceError::InvalidRecoveryIntent {
            code: super::model::ValidationCode::InvalidRecoveryIntent,
        });
    }
    let bytes = serde_json::to_vec(intent).map_err(|_| TaskServiceError::Serialization)?;
    atomic_replace_without_root_sync(root, RECOVERY_FILE, &bytes, IoStage::ReplaceRecoveryIntent)?;
    if faults.hit(FaultPoint::AfterRecoveryIntentRename) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::IntentParentSync,
        });
    }
    sync_root(root)?;
    if faults.hit(FaultPoint::AfterRecoveryIntentParentSync) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::IntentParentSync,
        });
    }
    Ok(())
}

pub(super) fn truncate_for_recovery(
    root: &RootHandle,
    length: u64,
    faults: &FaultController,
) -> Result<(), TaskServiceError> {
    root.validate_binding()?;
    let file = open_child(
        root,
        JOURNAL_FILE,
        libc::O_RDWR,
        0,
        IoStage::TruncateJournal,
    )?;
    validate_private_file(&file)?;
    root.validate_binding()?;
    file.set_len(length)
        .and_then(|_| file.sync_all())
        .map_err(|error| TaskServiceError::Io {
            stage: IoStage::TruncateJournal,
            kind: error.kind(),
        })?;
    if faults.hit(FaultPoint::AfterRecoveryTruncate) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::JournalTruncate,
        });
    }
    Ok(())
}

pub(super) fn append_recovery_record(
    root: &RootHandle,
    line: &[u8],
    faults: &FaultController,
) -> Result<(), TaskServiceError> {
    let mut file = open_journal_append(root)?;
    root.validate_binding()?;
    if faults.hit(FaultPoint::PartialRecoveryRecordWrite) {
        let count = (line.len() / 2).max(1).min(line.len().saturating_sub(1));
        file.write_all(&line[..count])
            .map_err(|error| TaskServiceError::Io {
                stage: IoStage::AppendJournal,
                kind: error.kind(),
            })?;
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::RecoveryRecordWrite,
        });
    }
    file.write_all(line)
        .and_then(|_| file.sync_all())
        .map_err(|error| TaskServiceError::Io {
            stage: IoStage::SyncJournal,
            kind: error.kind(),
        })?;
    if faults.hit(FaultPoint::AfterRecoveryRecordSync) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::RecoveryRecordSync,
        });
    }
    Ok(())
}

pub(super) fn persist_recovery_snapshot(
    root: &RootHandle,
    bytes: &[u8],
    faults: &FaultController,
) -> Result<(), TaskServiceError> {
    atomic_replace_without_root_sync(root, SNAPSHOT_FILE, bytes, IoStage::ReplaceSnapshot)?;
    if faults.hit(FaultPoint::AfterRecoverySnapshotRename) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::SnapshotReplace,
        });
    }
    sync_root(root)?;
    Ok(())
}

pub(super) fn remove_recovery_intent(
    root: &RootHandle,
    faults: &FaultController,
) -> Result<(), TaskServiceError> {
    root.validate_binding()?;
    unlink_child(root, RECOVERY_FILE, IoStage::RemoveRecoveryIntent)?;
    if faults.hit(FaultPoint::AfterRecoveryIntentRemove) {
        return Err(TaskServiceError::RecoveryStopped {
            phase: RecoveryPhase::IntentRemovalParentSync,
        });
    }
    sync_root(root)
}

fn read_private(
    root: &RootHandle,
    name: &str,
    open_stage: IoStage,
    read_stage: IoStage,
) -> Result<Option<Vec<u8>>, TaskServiceError> {
    root.validate_binding()?;
    let mut file = match open_child(root, name, libc::O_RDONLY, 0, open_stage) {
        Ok(file) => file,
        Err(TaskServiceError::Io {
            kind: io::ErrorKind::NotFound,
            ..
        }) => return Ok(None),
        Err(error) => return Err(error),
    };
    validate_private_file(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| TaskServiceError::Io {
            stage: read_stage,
            kind: error.kind(),
        })?;
    Ok(Some(bytes))
}

fn open_journal_append(root: &RootHandle) -> Result<File, TaskServiceError> {
    root.validate_binding()?;
    let mut file = open_child(
        root,
        JOURNAL_FILE,
        libc::O_RDWR | libc::O_APPEND | libc::O_CREAT,
        0o600,
        IoStage::OpenJournal,
    )?;
    validate_private_file(&file)?;
    file.seek(SeekFrom::End(0))
        .map_err(|error| TaskServiceError::Io {
            stage: IoStage::OpenJournal,
            kind: error.kind(),
        })?;
    Ok(file)
}

fn atomic_replace(
    root: &RootHandle,
    target_name: &str,
    bytes: &[u8],
    before_rename: Option<(&FaultController, FaultPoint)>,
    after_rename: Option<(&FaultController, FaultPoint)>,
    replace_stage: IoStage,
) -> Result<(), TaskServiceError> {
    let result = atomic_replace_without_root_sync_with_faults(
        root,
        target_name,
        bytes,
        before_rename,
        after_rename,
        replace_stage,
    );
    result?;
    sync_root(root)
}

fn atomic_replace_without_root_sync(
    root: &RootHandle,
    target_name: &str,
    bytes: &[u8],
    replace_stage: IoStage,
) -> Result<(), TaskServiceError> {
    atomic_replace_without_root_sync_with_faults(
        root,
        target_name,
        bytes,
        None,
        None,
        replace_stage,
    )
}

fn atomic_replace_without_root_sync_with_faults(
    root: &RootHandle,
    target_name: &str,
    bytes: &[u8],
    before_rename: Option<(&FaultController, FaultPoint)>,
    after_rename: Option<(&FaultController, FaultPoint)>,
    replace_stage: IoStage,
) -> Result<(), TaskServiceError> {
    let temp = format!(
        ".task-service-v1.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let result = (|| {
        root.validate_binding()?;
        let mut file = open_child(
            root,
            &temp,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
            IoStage::CreateTemp,
        )?;
        validate_private_file(&file)?;
        file.write_all(bytes)
            .map_err(|error| TaskServiceError::Io {
                stage: IoStage::WriteTemp,
                kind: error.kind(),
            })?;
        file.sync_all().map_err(|error| TaskServiceError::Io {
            stage: IoStage::SyncTemp,
            kind: error.kind(),
        })?;
        drop(file);
        if before_rename
            .as_ref()
            .is_some_and(|(faults, point)| faults.hit(*point))
        {
            return Err(TaskServiceError::InjectedDefiniteNoWrite);
        }
        root.validate_binding()?;
        rename_child(root, &temp, target_name, replace_stage)?;
        if after_rename
            .as_ref()
            .is_some_and(|(faults, point)| faults.hit(*point))
        {
            return Err(TaskServiceError::InjectedDefiniteNoWrite);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = unlink_child(root, &temp, IoStage::RemoveRecoveryIntent);
    }
    result
}

fn sync_root(root: &RootHandle) -> Result<(), TaskServiceError> {
    root.validate_binding()?;
    #[cfg(unix)]
    {
        root.directory
            .sync_all()
            .map_err(|error| TaskServiceError::Io {
                stage: IoStage::SyncRoot,
                kind: error.kind(),
            })
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::sync_directory(&root.directory).map_err(|error| {
            TaskServiceError::Io {
                stage: IoStage::SyncRoot,
                kind: error.io_kind(),
            }
        })
    }
}

fn validate_private_file(file: &File) -> Result<(), TaskServiceError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = file.metadata().map_err(|error| TaskServiceError::Io {
            stage: IoStage::InspectRoot,
            kind: error.kind(),
        })?;
        if !metadata.file_type().is_file() {
            return Err(TaskServiceError::PrivateFileNotRegular);
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(TaskServiceError::PrivateFileOwnerMismatch);
        }
        if metadata.permissions().mode() & 0o7777 != 0o600 {
            return Err(TaskServiceError::PrivateFileModeMismatch);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::validate_private_file(file).map_err(|error| match error {
            crate::platform::private_fs::PrivateFsError::WrongType
            | crate::platform::private_fs::PrivateFsError::ReparsePoint => {
                TaskServiceError::PrivateFileNotRegular
            }
            crate::platform::private_fs::PrivateFsError::OwnerMismatch => {
                TaskServiceError::PrivateFileOwnerMismatch
            }
            crate::platform::private_fs::PrivateFsError::DaclNotPrivate => {
                TaskServiceError::PrivateFileModeMismatch
            }
            error => TaskServiceError::Io {
                stage: IoStage::InspectRoot,
                kind: error.io_kind(),
            },
        })
    }
}

fn private_exists(root: &RootHandle, name: &str, stage: IoStage) -> Result<bool, TaskServiceError> {
    root.validate_binding()?;
    match open_child(root, name, libc::O_RDONLY, 0, stage) {
        Ok(file) => {
            validate_private_file(&file)?;
            Ok(true)
        }
        Err(TaskServiceError::Io {
            kind: io::ErrorKind::NotFound,
            ..
        }) => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn open_validated_root(root: &Path) -> Result<File, TaskServiceError> {
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let directory = options.open(root).map_err(|error| {
            if error
                .raw_os_error()
                .is_some_and(|code| code == libc::ELOOP || code == libc::ENOTDIR)
            {
                TaskServiceError::RootNotDirectory
            } else {
                TaskServiceError::Io {
                    stage: IoStage::InspectRoot,
                    kind: error.kind(),
                }
            }
        })?;
        let metadata = directory.metadata().map_err(|error| TaskServiceError::Io {
            stage: IoStage::InspectRoot,
            kind: error.kind(),
        })?;
        if !metadata.file_type().is_dir() {
            return Err(TaskServiceError::RootNotDirectory);
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(TaskServiceError::RootOwnerMismatch);
        }
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err(TaskServiceError::RootModeMismatch);
        }
        Ok(directory)
    }
}

#[cfg(unix)]
type PlatformMode = libc::mode_t;
#[cfg(not(unix))]
type PlatformMode = u32;

fn open_child(
    root: &RootHandle,
    name: &str,
    flags: libc::c_int,
    mode: PlatformMode,
    stage: IoStage,
) -> Result<File, TaskServiceError> {
    #[cfg(unix)]
    {
        let name = CString::new(name).map_err(|_| TaskServiceError::Io {
            stage,
            kind: io::ErrorKind::InvalidInput,
        })?;
        let descriptor = unsafe {
            libc::openat(
                root.directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode,
            )
        };
        if descriptor < 0 {
            return Err(TaskServiceError::Io {
                stage,
                kind: io::Error::last_os_error().kind(),
            });
        }
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
    #[cfg(windows)]
    {
        let _ = mode;
        crate::platform::private_fs::open_child(&root.path, root.identity, name, flags, true)
            .map_err(|error| TaskServiceError::Io {
                stage,
                kind: error.io_kind(),
            })
    }
}

fn rename_child(
    root: &RootHandle,
    source: &str,
    target: &str,
    stage: IoStage,
) -> Result<(), TaskServiceError> {
    #[cfg(unix)]
    {
        let source = CString::new(source).map_err(|_| TaskServiceError::Io {
            stage,
            kind: io::ErrorKind::InvalidInput,
        })?;
        let target = CString::new(target).map_err(|_| TaskServiceError::Io {
            stage,
            kind: io::ErrorKind::InvalidInput,
        })?;
        let result = unsafe {
            libc::renameat(
                root.directory.as_raw_fd(),
                source.as_ptr(),
                root.directory.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if result != 0 {
            return Err(TaskServiceError::Io {
                stage,
                kind: io::Error::last_os_error().kind(),
            });
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::replace_child(&root.path, root.identity, source, target)
            .map_err(|error| TaskServiceError::Io {
                stage,
                kind: error.io_kind(),
            })
    }
}

fn unlink_child(root: &RootHandle, name: &str, stage: IoStage) -> Result<(), TaskServiceError> {
    #[cfg(unix)]
    {
        let name = CString::new(name).map_err(|_| TaskServiceError::Io {
            stage,
            kind: io::ErrorKind::InvalidInput,
        })?;
        let result = unsafe { libc::unlinkat(root.directory.as_raw_fd(), name.as_ptr(), 0) };
        if result != 0 {
            return Err(TaskServiceError::Io {
                stage,
                kind: io::Error::last_os_error().kind(),
            });
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        crate::platform::private_fs::unlink_child(&root.path, root.identity, name).map_err(
            |error| TaskServiceError::Io {
                stage,
                kind: error.io_kind(),
            },
        )
    }
}

#[cfg(windows)]
fn map_root_error(error: crate::platform::private_fs::PrivateFsError) -> TaskServiceError {
    use crate::platform::private_fs::PrivateFsError;

    match error {
        PrivateFsError::WrongType | PrivateFsError::ReparsePoint => {
            TaskServiceError::RootNotDirectory
        }
        PrivateFsError::OwnerMismatch => TaskServiceError::RootOwnerMismatch,
        PrivateFsError::DaclNotPrivate => TaskServiceError::RootModeMismatch,
        error => TaskServiceError::Io {
            stage: IoStage::InspectRoot,
            kind: error.io_kind(),
        },
    }
}
