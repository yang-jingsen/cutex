#![cfg(target_os = "windows")]

use crate::protocol::{GetHostInfoParams, Request, RequestEnvelope, ResponseOutcome};
use crate::windows_service::{
    delete_service, ensure_service, query_windows_service, service_binary_path,
    start_windows_service, stop_windows_service, validate_service_name, ServiceConfiguration,
};
use crate::{
    configure_operator_sid, current_user_sid_string, LocalClient, RuntimePaths,
    DEFAULT_WINDOWS_INSTALL_ROOT, DEFAULT_WINDOWS_RUN_VALUE_NAME,
    DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME, DEFAULT_WINDOWS_SERVICE_NAME, DEFAULT_WINDOWS_STATE_DIR,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE,
};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegFlushKey, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

const INSTALLATION_SCHEMA: u32 = 1;
const RELEASE_SCHEMA: u32 = 1;
const INSTALLATION_MANIFEST: &str = "installation-v1.json";
const RELEASE_MANIFEST: &str = "release-manifest-v1.json";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const PRODUCT_BINARIES: [&str; 3] = ["prh-host.exe", "hostctl.exe", "prh-tray.exe"];
const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const STOP_TIMEOUT: Duration = Duration::from_secs(180);
const START_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub struct WindowsInstallOptions {
    pub source_dir: PathBuf,
    pub install_root: PathBuf,
    pub state_dir: PathBuf,
    pub service_name: String,
    pub display_name: String,
    pub run_value_name: String,
    pub release_id: String,
    pub source_revision: String,
    pub operator_sid: String,
}

impl WindowsInstallOptions {
    pub fn production(source_dir: PathBuf, release_id: String) -> io::Result<Self> {
        Ok(Self {
            source_dir,
            install_root: PathBuf::from(DEFAULT_WINDOWS_INSTALL_ROOT),
            state_dir: PathBuf::from(DEFAULT_WINDOWS_STATE_DIR),
            service_name: DEFAULT_WINDOWS_SERVICE_NAME.to_owned(),
            display_name: DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME.to_owned(),
            run_value_name: DEFAULT_WINDOWS_RUN_VALUE_NAME.to_owned(),
            source_revision: release_id.clone(),
            release_id,
            operator_sid: current_user_sid_string()?,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BinaryManifest {
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstalledRelease {
    pub schema_version: u32,
    pub release_id: String,
    pub source_revision: String,
    pub directory: PathBuf,
    pub binaries: BTreeMap<String, BinaryManifest>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WindowsInstallationManifest {
    pub schema_version: u32,
    pub service_name: String,
    pub display_name: String,
    pub run_value_name: String,
    pub operator_sid: String,
    pub state_dir: PathBuf,
    pub install_root: PathBuf,
    pub active: InstalledRelease,
    pub rollback: Option<InstalledRelease>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WindowsUninstallReceipt {
    pub service_removed: bool,
    pub tray_run_value_removed: bool,
    pub state_preserved: PathBuf,
    pub install_root_preserved: PathBuf,
}

pub fn install_or_upgrade_windows(
    options: &WindowsInstallOptions,
) -> io::Result<WindowsInstallationManifest> {
    validate_options(options)?;
    configure_operator_sid(&options.operator_sid)?;
    prepare_private_directory(&options.install_root)?;
    RuntimePaths::new(&options.state_dir)?.prepare()?;
    let candidate = stage_release(options)?;
    let prior = read_installation_manifest(&options.install_root)?;
    if let Some(prior) = &prior {
        validate_installation_identity(prior, options)?;
        verify_owned_registration(prior)?;
    } else {
        refuse_unowned_registration(options, &candidate)?;
    }

    let rollback = prior.as_ref().and_then(|manifest| {
        if manifest.active.release_id == candidate.release_id {
            manifest.rollback.clone()
        } else {
            Some(manifest.active.clone())
        }
    });
    let next = WindowsInstallationManifest {
        schema_version: INSTALLATION_SCHEMA,
        service_name: options.service_name.clone(),
        display_name: options.display_name.clone(),
        run_value_name: options.run_value_name.clone(),
        operator_sid: options.operator_sid.clone(),
        state_dir: options.state_dir.clone(),
        install_root: options.install_root.clone(),
        active: candidate,
        rollback,
    };

    if query_windows_service(&options.service_name)?.is_some() {
        stop_windows_service(&options.service_name, STOP_TIMEOUT)?;
    }
    if let Err(primary) = activate(&next) {
        return finish_failed_transition(primary, prior.as_ref(), &next);
    }
    if let Err(primary) = write_installation_manifest(&next) {
        return finish_failed_transition(primary, prior.as_ref(), &next);
    }
    Ok(next)
}

pub fn rollback_windows_installation(
    install_root: &Path,
) -> io::Result<WindowsInstallationManifest> {
    let current = read_installation_manifest(install_root)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no PRH installation manifest exists under {}",
                install_root.display()
            ),
        )
    })?;
    let target = current.rollback.clone().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no PRH rollback release is recorded",
        )
    })?;
    configure_operator_sid(&current.operator_sid)?;
    verify_release(&target)?;
    verify_owned_registration(&current)?;
    stop_windows_service(&current.service_name, STOP_TIMEOUT)?;
    let next = WindowsInstallationManifest {
        active: target,
        rollback: Some(current.active.clone()),
        ..current.clone()
    };
    if let Err(primary) = activate(&next) {
        return finish_failed_transition(primary, Some(&current), &next);
    }
    if let Err(primary) = write_installation_manifest(&next) {
        return finish_failed_transition(primary, Some(&current), &next);
    }
    Ok(next)
}

pub fn uninstall_windows(install_root: &Path) -> io::Result<WindowsUninstallReceipt> {
    let manifest = match read_installation_manifest(install_root)? {
        Some(manifest) => manifest,
        None => {
            return Ok(WindowsUninstallReceipt {
                service_removed: false,
                tray_run_value_removed: false,
                state_preserved: PathBuf::from(DEFAULT_WINDOWS_STATE_DIR),
                install_root_preserved: install_root.to_path_buf(),
            })
        }
    };
    configure_operator_sid(&manifest.operator_sid)?;
    verify_owned_registration(&manifest)?;
    let service_removed = query_windows_service(&manifest.service_name)?.is_some();
    if service_removed {
        delete_service(&manifest.service_name, STOP_TIMEOUT)?;
    }
    let expected_run = tray_command(
        &manifest.active,
        &manifest.state_dir,
        &manifest.service_name,
    )?;
    let tray_run_value_removed =
        delete_run_value_if_equal(&manifest.run_value_name, &expected_run)?;
    Ok(WindowsUninstallReceipt {
        service_removed,
        tray_run_value_removed,
        state_preserved: manifest.state_dir,
        install_root_preserved: manifest.install_root,
    })
}

pub fn read_windows_installation_manifest(
    install_root: &Path,
) -> io::Result<Option<WindowsInstallationManifest>> {
    read_installation_manifest(install_root)
}

pub fn query_windows_tray_run_value(value_name: &str) -> io::Result<Option<String>> {
    validate_value_name(value_name)?;
    query_run_value(value_name)
}

fn activate(manifest: &WindowsInstallationManifest) -> io::Result<()> {
    verify_release(&manifest.active)?;
    RuntimePaths::new(&manifest.state_dir)?.prepare()?;
    let binary_path = service_command(
        &manifest.active,
        &manifest.state_dir,
        &manifest.service_name,
        &manifest.operator_sid,
    )?;
    ensure_service(&ServiceConfiguration {
        service_name: &manifest.service_name,
        display_name: &manifest.display_name,
        binary_path: &binary_path,
        operator_sid: &manifest.operator_sid,
    })?;
    let tray = tray_command(
        &manifest.active,
        &manifest.state_dir,
        &manifest.service_name,
    )?;
    set_run_value(&manifest.run_value_name, &tray)?;
    start_windows_service(&manifest.service_name, START_TIMEOUT)?;
    verify_host_api(&manifest.state_dir)
}

fn finish_failed_transition<T>(
    primary: io::Error,
    prior: Option<&WindowsInstallationManifest>,
    attempted: &WindowsInstallationManifest,
) -> io::Result<T> {
    let rollback_result = match prior {
        Some(prior) => {
            let _ = stop_windows_service(&attempted.service_name, STOP_TIMEOUT);
            activate(prior)
        }
        None => {
            let _ = delete_service(&attempted.service_name, STOP_TIMEOUT);
            let expected = tray_command(
                &attempted.active,
                &attempted.state_dir,
                &attempted.service_name,
            );
            expected.and_then(|value| {
                delete_run_value_if_equal(&attempted.run_value_name, &value).map(|_| ())
            })
        }
    };
    match rollback_result {
        Ok(()) => Err(io::Error::other(format!(
            "PRH activation failed and the prior registration was restored: {primary}"
        ))),
        Err(rollback) => Err(io::Error::other(format!(
            "PRH activation failed ({primary}); rollback also failed ({rollback})"
        ))),
    }
}

fn verify_host_api(state_dir: &Path) -> io::Result<()> {
    let paths = RuntimePaths::new(state_dir)?;
    let client = LocalClient::new(paths.socket);
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let last_error = match client.call(&RequestEnvelope::v1(
            format!("windows-install-verify-{}", uuid::Uuid::new_v4()),
            Request::GetHostInfo(GetHostInfoParams {}),
        )) {
            Ok(response) => match response.outcome {
                ResponseOutcome::Ok { .. } => return Ok(()),
                ResponseOutcome::Error { error } => {
                    format!("installed PRH API rejected verification: {error}")
                }
            },
            Err(error) => format!("installed PRH API is unreachable: {error}"),
        };
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, last_error));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn stage_release(options: &WindowsInstallOptions) -> io::Result<InstalledRelease> {
    let mut binaries = BTreeMap::new();
    for file_name in PRODUCT_BINARIES {
        let source = options.source_dir.join(file_name);
        validate_regular_file(&source, "release source binary")?;
        let (sha256, bytes) = hash_file(&source)?;
        binaries.insert(
            file_name.to_owned(),
            BinaryManifest {
                file_name: file_name.to_owned(),
                sha256,
                bytes,
            },
        );
    }
    let release = InstalledRelease {
        schema_version: RELEASE_SCHEMA,
        release_id: options.release_id.clone(),
        source_revision: options.source_revision.clone(),
        directory: options.install_root.join(&options.release_id),
        binaries,
    };
    if release.directory.exists() {
        verify_release(&release)?;
        return Ok(release);
    }

    let staging = options.install_root.join(format!(
        ".{}.staging-{}-{}",
        options.release_id,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&staging)?;
    if let Err(error) = stage_into(&staging, &release, &options.source_dir) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, &release.directory) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    crate::windows_security::secure_path_for_current_user(&release.directory, true)?;
    verify_release(&release)?;
    Ok(release)
}

fn stage_into(staging: &Path, release: &InstalledRelease, source_dir: &Path) -> io::Result<()> {
    crate::windows_security::secure_path_for_current_user(staging, true)?;
    for artifact in release.binaries.values() {
        let source = source_dir.join(&artifact.file_name);
        let destination = staging.join(&artifact.file_name);
        fs::copy(&source, &destination)?;
        validate_regular_file(&destination, "staged release binary")?;
        crate::windows_security::secure_path_for_current_user(&destination, false)?;
        let (hash, bytes) = hash_file(&destination)?;
        if hash != artifact.sha256 || bytes != artifact.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "staged binary verification failed for {}",
                    artifact.file_name
                ),
            ));
        }
    }
    let path = staging.join(RELEASE_MANIFEST);
    write_new_json(&path, release)?;
    Ok(())
}

fn verify_release(release: &InstalledRelease) -> io::Result<()> {
    if release.schema_version != RELEASE_SCHEMA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported PRH release manifest schema",
        ));
    }
    validate_real_directory(&release.directory, "installed release directory")?;
    let stored_path = release.directory.join(RELEASE_MANIFEST);
    validate_regular_file(&stored_path, "release manifest")?;
    let stored: InstalledRelease = read_json_limited(&stored_path, 1024 * 1024)?;
    if &stored != release {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "release manifest mismatch under {}",
                release.directory.display()
            ),
        ));
    }
    if release.binaries.len() != PRODUCT_BINARIES.len()
        || PRODUCT_BINARIES
            .iter()
            .any(|name| !release.binaries.contains_key(*name))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release manifest does not contain the exact PRH product binary set",
        ));
    }
    for artifact in release.binaries.values() {
        let path = release.directory.join(&artifact.file_name);
        validate_regular_file(&path, "installed release binary")?;
        let (hash, bytes) = hash_file(&path)?;
        if hash != artifact.sha256 || bytes != artifact.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("installed release hash mismatch for {}", artifact.file_name),
            ));
        }
    }
    Ok(())
}

fn validate_options(options: &WindowsInstallOptions) -> io::Result<()> {
    validate_service_name(&options.service_name)?;
    validate_value_name(&options.run_value_name)?;
    validate_release_id(&options.release_id)?;
    if options.source_revision.is_empty()
        || options.source_revision.len() > 256
        || options.source_revision.contains('\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source revision must be 1..=256 characters without NUL",
        ));
    }
    if options.display_name.is_empty() || options.display_name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "service display name must be non-empty and contain no NUL",
        ));
    }
    for (label, path) in [
        ("release source", &options.source_dir),
        ("install root", &options.install_root),
        ("state directory", &options.state_dir),
    ] {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} must be an absolute Windows path"),
            ));
        }
        if path
            .components()
            .all(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{label} must not be a filesystem root"),
            ));
        }
    }
    if options.install_root == options.state_dir
        || options.install_root.starts_with(&options.state_dir)
        || options.state_dir.starts_with(&options.install_root)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "install root and durable state directory must be separate, non-overlapping paths",
        ));
    }
    validate_real_directory(&options.source_dir, "release source directory")?;
    Ok(())
}

fn validate_installation_identity(
    manifest: &WindowsInstallationManifest,
    options: &WindowsInstallOptions,
) -> io::Result<()> {
    if manifest.schema_version != INSTALLATION_SCHEMA
        || manifest.service_name != options.service_name
        || manifest.display_name != options.display_name
        || manifest.run_value_name != options.run_value_name
        || manifest.operator_sid != options.operator_sid
        || manifest.state_dir != options.state_dir
        || manifest.install_root != options.install_root
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "upgrade options do not match the owned PRH installation identity",
        ));
    }
    verify_release(&manifest.active)?;
    if let Some(rollback) = &manifest.rollback {
        verify_release(rollback)?;
    }
    Ok(())
}

fn verify_owned_registration(manifest: &WindowsInstallationManifest) -> io::Result<()> {
    if let Some(actual) = service_binary_path(&manifest.service_name)? {
        let expected = service_command(
            &manifest.active,
            &manifest.state_dir,
            &manifest.service_name,
            &manifest.operator_sid,
        )?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to modify service {} because its image path is not owned by this manifest",
                    manifest.service_name
                ),
            ));
        }
    }
    if let Some(actual) = query_run_value(&manifest.run_value_name)? {
        let expected = tray_command(
            &manifest.active,
            &manifest.state_dir,
            &manifest.service_name,
        )?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to modify Run value {} because it is not owned by this manifest",
                    manifest.run_value_name
                ),
            ));
        }
    }
    Ok(())
}

fn refuse_unowned_registration(
    options: &WindowsInstallOptions,
    candidate: &InstalledRelease,
) -> io::Result<()> {
    if service_binary_path(&options.service_name)?.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "service {} already exists without this PRH installation manifest",
                options.service_name
            ),
        ));
    }
    if let Some(actual) = query_run_value(&options.run_value_name)? {
        let expected = tray_command(candidate, &options.state_dir, &options.service_name)?;
        if actual != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "Run value {} already exists and is not this PRH tray",
                    options.run_value_name
                ),
            ));
        }
    }
    Ok(())
}

fn service_command(
    release: &InstalledRelease,
    state_dir: &Path,
    service_name: &str,
    operator_sid: &str,
) -> io::Result<String> {
    let executable = release.directory.join("prh-host.exe");
    let values = [
        executable.to_string_lossy().into_owned(),
        "--service".to_owned(),
        "--service-name".to_owned(),
        service_name.to_owned(),
        "--state-dir".to_owned(),
        state_dir.to_string_lossy().into_owned(),
        "--operator-sid".to_owned(),
        operator_sid.to_owned(),
    ];
    Ok(values
        .iter()
        .map(|value| crate::quote_windows_argument(value))
        .collect::<Vec<_>>()
        .join(" "))
}

fn tray_command(
    release: &InstalledRelease,
    state_dir: &Path,
    service_name: &str,
) -> io::Result<String> {
    let executable = release.directory.join("prh-tray.exe");
    let values = [
        executable.to_string_lossy().into_owned(),
        "--state-dir".to_owned(),
        state_dir.to_string_lossy().into_owned(),
        "--service-name".to_owned(),
        service_name.to_owned(),
    ];
    let command = values
        .iter()
        .map(|value| crate::quote_windows_argument(value))
        .collect::<Vec<_>>()
        .join(" ");
    if command.encode_utf16().count() > 259 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "tray Run command exceeds the Windows Run-key 260-character limit",
        ));
    }
    Ok(command)
}

fn read_installation_manifest(
    install_root: &Path,
) -> io::Result<Option<WindowsInstallationManifest>> {
    let path = install_root.join(INSTALLATION_MANIFEST);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_regular_file(&path, "installation manifest")?;
            let manifest: WindowsInstallationManifest = read_json_limited(&path, 1024 * 1024)?;
            if manifest.schema_version != INSTALLATION_SCHEMA {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported PRH installation manifest schema",
                ));
            }
            Ok(Some(manifest))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_installation_manifest(manifest: &WindowsInstallationManifest) -> io::Result<()> {
    let final_path = manifest.install_root.join(INSTALLATION_MANIFEST);
    let temporary = manifest.install_root.join(format!(
        ".{INSTALLATION_MANIFEST}.tmp-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    write_new_json(&temporary, manifest)?;
    let source = wide_path(&temporary);
    let destination = wide_path(&final_path);
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    crate::windows_security::secure_path_for_current_user(&final_path, false)
}

fn write_new_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    crate::windows_security::secure_path_for_current_user(path, false)
}

fn read_json_limited<T: for<'de> Deserialize<'de>>(path: &Path, limit: u64) -> io::Result<T> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("JSON file exceeds safety limit: {}", path.display()),
        ));
    }
    serde_json::from_reader(file.take(limit + 1))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn prepare_private_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_real_directory(path, "install root")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    crate::windows_security::refuse_reparse_point(path, "install root")?;
    crate::windows_security::secure_path_for_current_user(path, true)
}

fn validate_real_directory(path: &Path, label: &str) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be a real directory: {}", path.display()),
        ));
    }
    crate::windows_security::refuse_reparse_point(path, label)
}

fn validate_regular_file(path: &Path, label: &str) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be a regular file: {}", path.display()),
        ));
    }
    crate::windows_security::refuse_reparse_point(path, label)
}

fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let bytes = file.metadata()?.len();
    if bytes > MAX_BINARY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("binary exceeds safety limit: {}", path.display()),
        ));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let digest = hash.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok((encoded, bytes))
}

fn validate_release_id(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 96
        || matches!(value, "." | "..")
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "release ID must contain only ASCII letters, digits, dot, underscore, or hyphen",
        ))
    } else {
        Ok(())
    }
}

fn validate_value_name(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 192
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Run value name must contain only ASCII letters, digits, dot, underscore, or hyphen",
        ))
    } else {
        Ok(())
    }
}

fn query_run_value(value_name: &str) -> io::Result<Option<String>> {
    let key_path = wide(RUN_KEY);
    let mut key = null_mut();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_READ, &mut key) };
    if matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
        return Ok(None);
    }
    check_registry(status, "open current-user Run key")?;
    let key = RegistryKey(key);
    let name = wide(value_name);
    let mut value_type = 0;
    let mut bytes = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut value_type,
            null_mut(),
            &mut bytes,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check_registry(status, "size current-user Run value")?;
    if value_type != REG_SZ {
        return Err(io::Error::from_raw_os_error(ERROR_UNSUPPORTED_TYPE as i32));
    }
    if bytes > 4096 || bytes % 2 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "current-user Run value has an invalid length",
        ));
    }
    let mut data = vec![0_u16; (bytes as usize).div_ceil(2)];
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut value_type,
            data.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    check_registry(status, "read current-user Run value")?;
    while data.last() == Some(&0) {
        data.pop();
    }
    String::from_utf16(&data)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn set_run_value(value_name: &str, value: &str) -> io::Result<()> {
    validate_value_name(value_name)?;
    let key_path = wide(RUN_KEY);
    let mut key = null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    check_registry(status, "open or create current-user Run key")?;
    let key = RegistryKey(key);
    let name = wide(value_name);
    let encoded = wide(value);
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            encoded.as_ptr().cast(),
            (encoded.len() * std::mem::size_of::<u16>()) as u32,
        )
    };
    check_registry(status, "write current-user Run value")?;
    check_registry(unsafe { RegFlushKey(key.0) }, "flush current-user Run key")
}

fn delete_run_value_if_equal(value_name: &str, expected: &str) -> io::Result<bool> {
    match query_run_value(value_name)? {
        None => Ok(false),
        Some(actual) if actual != expected => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to remove non-owned Run value {value_name}"),
        )),
        Some(_) => {
            let key_path = wide(RUN_KEY);
            let mut key = null_mut();
            check_registry(
                unsafe {
                    RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_WRITE, &mut key)
                },
                "open current-user Run key for deletion",
            )?;
            let key = RegistryKey(key);
            let name = wide(value_name);
            let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if status != ERROR_FILE_NOT_FOUND {
                check_registry(status, "delete owned current-user Run value")?;
            }
            check_registry(unsafe { RegFlushKey(key.0) }, "flush current-user Run key")?;
            Ok(true)
        }
    }
}

fn check_registry(status: u32, action: &str) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        let error = io::Error::from_raw_os_error(status as i32);
        Err(io::Error::new(
            error.kind(),
            format!("failed to {action}: {error}"),
        ))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
}
