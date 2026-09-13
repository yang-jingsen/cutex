use crate::model::{RunId, ServiceId};
use crate::protocol::LogEntry;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileLogConfig {
    pub max_bytes_per_file: u64,
    /// Total files retained per service, including the active file.
    pub max_files_per_service: usize,
}

impl Default for FileLogConfig {
    fn default() -> Self {
        Self {
            max_bytes_per_file: 4 * 1024 * 1024,
            max_files_per_service: 4,
        }
    }
}

pub struct RotatingFileLogs {
    root: PathBuf,
    config: FileLogConfig,
    gate: Mutex<()>,
}

impl RotatingFileLogs {
    pub fn open(root: impl Into<PathBuf>, mut config: FileLogConfig) -> std::io::Result<Self> {
        let root = root.into();
        config.max_bytes_per_file = config.max_bytes_per_file.max(256);
        config.max_files_per_service = config.max_files_per_service.clamp(1, 64);
        prepare_private_directory(&root)?;
        Ok(Self {
            root,
            config,
            gate: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn append(&self, entry: &LogEntry) -> std::io::Result<()> {
        let _gate = self.gate.lock().expect("file log lock poisoned");
        let path = self.active_path(&entry.service_id);
        let mut encoded = encode_bounded(entry, self.config.max_bytes_per_file as usize)?;
        encoded.push(b'\n');
        let current_len = match open_log_file(&path) {
            Ok(file) => file.metadata()?.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        if current_len > 0
            && current_len.saturating_add(encoded.len() as u64) > self.config.max_bytes_per_file
        {
            self.rotate(&path)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
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
        let mut file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "active log path is not a regular file",
            ));
        }
        validate_windows_regular_file(&file)?;
        secure_open_file(&file, &path)?;
        file.write_all(&encoded)?;
        file.flush()?;
        Ok(())
    }

    pub fn read(
        &self,
        service_id: &ServiceId,
        run_id: Option<&RunId>,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> std::io::Result<Vec<LogEntry>> {
        let _gate = self.gate.lock().expect("file log lock poisoned");
        let after = after_sequence.unwrap_or(0);
        let max = usize::try_from(limit.clamp(1, 10_000)).unwrap_or(10_000);
        let mut entries = Vec::new();
        for path in self.paths_oldest_first(service_id) {
            let file = match open_log_file(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for line in BufReader::new(file).lines() {
                let entry: LogEntry = serde_json::from_str(&line?)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                if entry.sequence <= after
                    || run_id.is_some_and(|selected| &entry.run_id != selected)
                {
                    continue;
                }
                entries.push(entry);
                if entries.len() >= max {
                    return Ok(entries);
                }
            }
        }
        Ok(entries)
    }

    pub fn last_sequence(&self, service_id: &ServiceId) -> std::io::Result<u64> {
        let _gate = self.gate.lock().expect("file log lock poisoned");
        let mut last = 0;
        for path in self.paths_oldest_first(service_id) {
            let file = match open_log_file(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for line in BufReader::new(file).lines() {
                let entry: LogEntry = serde_json::from_str(&line?)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                last = last.max(entry.sequence);
            }
        }
        Ok(last)
    }

    fn active_path(&self, service_id: &ServiceId) -> PathBuf {
        self.root.join(format!("{}.jsonl", service_id.as_str()))
    }

    fn rotated_path(&self, active: &Path, generation: usize) -> PathBuf {
        let mut name = active.as_os_str().to_os_string();
        name.push(format!(".{generation}"));
        PathBuf::from(name)
    }

    fn rotate(&self, active: &Path) -> std::io::Result<()> {
        if self.config.max_files_per_service == 1 {
            match fs::remove_file(active) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        let oldest = self.rotated_path(active, self.config.max_files_per_service - 1);
        if let Err(error) = fs::remove_file(&oldest) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error);
            }
        }
        for generation in (2..self.config.max_files_per_service).rev() {
            let source = self.rotated_path(active, generation - 1);
            let destination = self.rotated_path(active, generation);
            match fs::rename(source, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match fs::rename(active, self.rotated_path(active, 1)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn paths_oldest_first(&self, service_id: &ServiceId) -> Vec<PathBuf> {
        let active = self.active_path(service_id);
        let mut paths = (1..self.config.max_files_per_service)
            .rev()
            .map(|generation| self.rotated_path(&active, generation))
            .collect::<Vec<_>>();
        paths.push(active);
        paths
    }
}

fn prepare_private_directory(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "log root must be a real directory, not a symlink",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    secure_directory(path)
}

fn open_log_file(path: &Path) -> std::io::Result<File> {
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
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path is not a regular file",
        ));
    }
    validate_windows_regular_file(&file)?;
    secure_open_file(&file, path)?;
    Ok(file)
}

#[cfg(target_os = "windows")]
fn validate_windows_regular_file(file: &File) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "log path must not be a Windows reparse point",
        ))
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn validate_windows_regular_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}

fn encode_bounded(entry: &LogEntry, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let encoded = serde_json::to_vec(entry).map_err(std::io::Error::other)?;
    if encoded.len() <= max_bytes.saturating_sub(1) {
        return Ok(encoded);
    }
    let mut bounded = entry.clone();
    bounded.truncated = true;
    let mut low = 0;
    let mut high = bounded.data.len();
    let mut accepted = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        let mut boundary = middle;
        while boundary > 0 && !bounded.data.is_char_boundary(boundary) {
            boundary -= 1;
        }
        bounded.data.truncate(boundary);
        let candidate = serde_json::to_vec(&bounded).map_err(std::io::Error::other)?;
        if candidate.len() <= max_bytes.saturating_sub(1) {
            accepted = Some(candidate);
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
        bounded.data = entry.data.clone();
    }
    accepted.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file log bound is too small for entry metadata",
        )
    })
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(target_os = "windows")]
fn secure_directory(path: &Path) -> std::io::Result<()> {
    crate::windows_security::refuse_reparse_point(path, "log directory")?;
    crate::windows_security::secure_path_for_current_user(path, true)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn secure_directory(_path: &Path) -> std::io::Result<()> {
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
