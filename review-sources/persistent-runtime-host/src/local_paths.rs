use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct RuntimePaths {
    pub state_dir: PathBuf,
    pub socket: PathBuf,
    pub registry: PathBuf,
    pub logs: PathBuf,
    pub lock: PathBuf,
}

impl RuntimePaths {
    pub fn new(state_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let state_dir = state_dir.into();
        if !state_dir.is_absolute() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PRH state directory must be absolute",
            ));
        }
        #[cfg(target_os = "windows")]
        let socket = PathBuf::from(crate::windows_support::windows_pipe_name_for_state(
            &state_dir.to_string_lossy(),
        ));
        #[cfg(not(target_os = "windows"))]
        let socket = state_dir.join("prh-v1.sock");
        Ok(Self {
            socket,
            registry: state_dir.join("registry-v1.json"),
            logs: state_dir.join("logs-v1"),
            lock: state_dir.join("host.lock"),
            state_dir,
        })
    }

    pub fn prepare(&self) -> std::io::Result<()> {
        match fs::symlink_metadata(&self.state_dir) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "PRH state path must be a real directory, not a symlink",
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.uid() != unsafe { libc::geteuid() } {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "PRH state directory is owned by another user",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.state_dir)?;
            }
            Err(error) => return Err(error),
        }
        #[cfg(target_os = "windows")]
        {
            crate::windows_security::refuse_reparse_point(&self.state_dir, "PRH state directory")?;
            crate::windows_security::secure_path_for_current_user(&self.state_dir, true)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.state_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

pub fn default_state_dir() -> std::io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("PRH_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        Ok(std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("PersistentRuntimeHost")
            .join("state-v1"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
            return Ok(PathBuf::from(path).join("persistent-runtime-host"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME is unavailable; pass --state-dir explicitly",
            )
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("persistent-runtime-host"))
    }
}

pub struct SingleInstanceGuard {
    file: File,
    path: PathBuf,
}

impl SingleInstanceGuard {
    pub fn acquire(paths: &RuntimePaths) -> std::io::Result<Self> {
        paths.prepare()?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(&paths.lock)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PRH instance lock must be a regular file",
            ));
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "PRH instance lock must not be a reparse point",
                ));
            }
            crate::windows_security::secure_path_for_current_user(&paths.lock, false)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.try_lock_exclusive().map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "another PRH instance holds {}: {error}",
                    paths.lock.display()
                ),
            )
        })?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;
        Ok(Self {
            file,
            path: paths.lock.clone(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
