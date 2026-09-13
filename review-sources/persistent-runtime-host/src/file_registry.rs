use crate::model::ServiceId;
use crate::registry::{RegistrySnapshot, RegistryStore, RegistryStoreError, StoredDefinition};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;
static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

pub struct FileRegistry {
    path: PathBuf,
    snapshot: Mutex<RegistrySnapshot>,
}

impl FileRegistry {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, RegistryStoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            prepare_private_directory(parent).map_err(io_error)?;
        }
        let snapshot = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(io_error(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "registry path must be a regular file, not a symlink",
                    )));
                }
                #[cfg(target_os = "windows")]
                crate::windows_security::refuse_reparse_point(&path, "registry path")
                    .map_err(io_error)?;
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW);
                }
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
                    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
                }
                let file = options.open(&path).map_err(io_error)?;
                validate_owned_file(&file).map_err(io_error)?;
                secure_open_file(&file, &path).map_err(io_error)?;
                if file.metadata().map_err(io_error)?.len() > MAX_REGISTRY_BYTES {
                    return Err(RegistryStoreError::Unavailable(format!(
                        "registry exceeds the {} byte safety limit",
                        MAX_REGISTRY_BYTES
                    )));
                }
                let mut bytes = Vec::new();
                file.take(MAX_REGISTRY_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(io_error)?;
                serde_json::from_slice(&bytes).map_err(|error| {
                    RegistryStoreError::Unavailable(format!(
                        "registry {} is not valid v1 JSON: {error}",
                        path.display()
                    ))
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                RegistrySnapshot::default()
            }
            Err(error) => return Err(io_error(error)),
        };
        Ok(Self {
            path,
            snapshot: Mutex::new(snapshot),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn persist(&self, snapshot: &RegistrySnapshot) -> Result<(), RegistryStoreError> {
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| {
            RegistryStoreError::Unavailable(format!("failed to encode registry: {error}"))
        })?;
        if (bytes.len() as u64).saturating_add(1) > MAX_REGISTRY_BYTES {
            return Err(RegistryStoreError::Unavailable(format!(
                "registry exceeds the {} byte safety limit",
                MAX_REGISTRY_BYTES
            )));
        }
        let parent = self.path.parent().ok_or_else(|| {
            RegistryStoreError::Unavailable("registry path has no parent directory".to_owned())
        })?;
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                RegistryStoreError::Unavailable("registry file name is not UTF-8".to_owned())
            })?;
        let serial = TEMP_SERIAL.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{file_name}.tmp-{}-{serial}", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let write_result = (|| -> std::io::Result<()> {
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            replace_file_atomically(&temporary, &self.path)?;
            secure_file(&self.path)?;
            sync_parent_directory(parent)?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(io_error(error));
        }
        Ok(())
    }
}

impl RegistryStore for FileRegistry {
    fn load(&self) -> Result<RegistrySnapshot, RegistryStoreError> {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| RegistryStoreError::Unavailable("registry lock was poisoned".to_owned()))
    }

    fn replace(
        &self,
        expected_registry_revision: u64,
        definitions: BTreeMap<ServiceId, StoredDefinition>,
    ) -> Result<RegistrySnapshot, RegistryStoreError> {
        let mut current = self.snapshot.lock().map_err(|_| {
            RegistryStoreError::Unavailable("registry lock was poisoned".to_owned())
        })?;
        if current.revision != expected_registry_revision {
            return Err(RegistryStoreError::Conflict {
                expected: expected_registry_revision,
                actual: current.revision,
            });
        }
        let next = RegistrySnapshot {
            revision: current.revision.checked_add(1).ok_or_else(|| {
                RegistryStoreError::Unavailable("registry revision overflow".to_owned())
            })?,
            definitions,
        };
        self.persist(&next)?;
        *current = next.clone();
        Ok(next)
    }
}

fn io_error(error: std::io::Error) -> RegistryStoreError {
    RegistryStoreError::Unavailable(error.to_string())
}

#[cfg(not(target_os = "windows"))]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(not(target_os = "windows"))]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(target_os = "windows")]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    // MoveFileExW above uses MOVEFILE_WRITE_THROUGH. Opening a directory for
    // sync requires backup-semantics flags and adds no stronger guarantee here.
    Ok(())
}

#[cfg(target_os = "windows")]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn prepare_private_directory(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "registry parent must be a real directory, not a symlink",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    secure_directory(path)
}

#[cfg(unix)]
fn validate_owned_file(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    if file.metadata()?.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "registry file is owned by another user",
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn validate_owned_file(file: &File) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "registry file must not be a reparse point",
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn validate_owned_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_open_file(file: &File, _path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(target_os = "windows")]
fn secure_open_file(_file: &File, path: &Path) -> std::io::Result<()> {
    crate::windows_security::secure_path_for_current_user(path, false)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn secure_open_file(_file: &File, _path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(target_os = "windows")]
fn secure_directory(path: &Path) -> std::io::Result<()> {
    crate::windows_security::refuse_reparse_point(path, "registry directory")?;
    crate::windows_security::secure_path_for_current_user(path, true)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn secure_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(target_os = "windows")]
fn secure_file(path: &Path) -> std::io::Result<()> {
    crate::windows_security::refuse_reparse_point(path, "registry file")?;
    crate::windows_security::secure_path_for_current_user(path, false)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn secure_file(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
