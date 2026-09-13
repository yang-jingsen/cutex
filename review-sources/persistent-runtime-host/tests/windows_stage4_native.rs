#![cfg(target_os = "windows")]

use persistent_runtime_host::{
    current_user_sid_string, install_or_upgrade_windows, query_windows_service,
    query_windows_tray_run_value, read_windows_installation_manifest, start_windows_service,
    stop_windows_service, uninstall_windows, EnsureRunningParams, GetHostInfoParams, LocalClient,
    MutationOptions, Request, RequestEnvelope, Response, ResponseOutcome, RestartPolicy,
    RuntimePaths, ServiceDefinition, ServiceId, ShutdownPolicy, StartPolicy, WindowsInstallOptions,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, WAIT_TIMEOUT};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

const WAIT: Duration = Duration::from_secs(40);
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[test]
#[ignore = "requires an explicitly approved elevated isolated Windows SCM environment"]
fn stage4_scm_install_upgrade_rollback_recovery_and_uninstall_are_real_and_scoped() {
    let suffix = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
    let temporary = tempfile::Builder::new()
        .prefix("prh-s4-")
        .tempdir_in(std::env::temp_dir())
        .unwrap();
    let install_root = temporary.path().join("programs");
    let state_dir = temporary.path().join("state");
    let service_name = format!("PRHStage4-{suffix}");
    let run_value_name = format!("PRHStage4Tray-{suffix}");
    let unrelated_run_value = format!("PRHStage4Unrelated-{suffix}");
    let source_dir = host_executable().parent().unwrap().to_path_buf();
    let operator_sid = current_user_sid_string().unwrap();
    let first_release = format!("test-a-{suffix}");
    let second_release = format!("test-b-{suffix}");
    let mut options = WindowsInstallOptions {
        source_dir,
        install_root: install_root.clone(),
        state_dir: state_dir.clone(),
        service_name: service_name.clone(),
        display_name: format!("PRH Stage 4 disposable {suffix}"),
        run_value_name: run_value_name.clone(),
        release_id: first_release.clone(),
        source_revision: format!("native-test:{suffix}:a"),
        operator_sid,
    };
    let cleanup = InstallationCleanup::new(install_root.clone(), unrelated_run_value.clone());

    let install_output = Command::new(hostctl_executable())
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("windows-install")
        .arg("--source-dir")
        .arg(&options.source_dir)
        .arg("--release-id")
        .arg(&options.release_id)
        .arg("--source-revision")
        .arg(&options.source_revision)
        .arg("--install-root")
        .arg(&install_root)
        .arg("--service-name")
        .arg(&service_name)
        .arg("--display-name")
        .arg(&options.display_name)
        .arg("--run-value-name")
        .arg(&run_value_name)
        .arg("--operator-sid")
        .arg(&options.operator_sid)
        .output()
        .unwrap();
    assert!(
        install_output.status.success(),
        "hostctl windows-install failed: {}",
        String::from_utf8_lossy(&install_output.stderr)
    );
    let installed = read_windows_installation_manifest(&install_root)
        .unwrap()
        .expect("hostctl wrote an installation manifest");
    assert_eq!(installed.active.release_id, first_release);
    assert!(installed.rollback.is_none());
    assert_eq!(installed.active.binaries.len(), 3);
    assert!(installed
        .active
        .binaries
        .values()
        .all(|artifact| artifact.sha256.len() == 64 && artifact.bytes > 0));
    assert!(query_windows_tray_run_value(&run_value_name)
        .unwrap()
        .unwrap()
        .contains("prh-tray.exe"));

    let first_status = query_windows_service(&service_name)
        .unwrap()
        .expect("disposable service was installed");
    assert!(first_status.is_running());
    assert_ne!(first_status.process_id, 0);
    let mut session_id = u32::MAX;
    assert_ne!(
        unsafe { ProcessIdToSessionId(first_status.process_id, &mut session_id) },
        0
    );
    assert_eq!(
        session_id, 0,
        "SCM host must run in non-interactive session 0"
    );
    assert_host_reachable(&state_dir);
    assert!(Command::new(installed.active.directory.join("hostctl.exe"))
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("status")
        .output()
        .unwrap()
        .status
        .success());

    let parent_pid_file = temporary.path().join("target.pid");
    let child_pid_file = temporary.path().join("descendant.pid");
    let client = local_client(&state_dir);
    register_service(
        &client,
        service_definition(
            "stage4-cleanup",
            temporary.path(),
            &parent_pid_file,
            &child_pid_file,
        ),
    );
    start_registered_service(&client, "stage4-cleanup");
    let parent_pid = wait_for_pid_file(&parent_pid_file);
    let child_pid = wait_for_pid_file(&child_pid_file);
    assert!(process_is_alive(parent_pid));
    assert!(process_is_alive(child_pid));
    stop_windows_service(&service_name, WAIT).unwrap();
    wait_for_process_exit(parent_pid);
    wait_for_process_exit(child_pid);

    start_windows_service(&service_name, WAIT).unwrap();
    assert_host_reachable(&state_dir);
    fs::remove_file(&parent_pid_file).unwrap();
    fs::remove_file(&child_pid_file).unwrap();
    let client = local_client(&state_dir);
    start_registered_service(&client, "stage4-cleanup");
    let crash_parent_pid = wait_for_pid_file(&parent_pid_file);
    let crash_child_pid = wait_for_pid_file(&child_pid_file);
    let before_crash = query_windows_service(&service_name).unwrap().unwrap();
    terminate_process(before_crash.process_id);
    wait_for_process_exit(crash_parent_pid);
    wait_for_process_exit(crash_child_pid);
    let recovered = wait_for_recovery(&service_name, before_crash.process_id);
    assert!(recovered.is_running());
    assert_host_reachable(&state_dir);

    let repeated = install_or_upgrade_windows(&options).unwrap();
    assert_eq!(repeated.active.release_id, first_release);
    assert!(repeated.rollback.is_none());
    assert_host_reachable(&state_dir);

    options.release_id = second_release.clone();
    options.source_revision = format!("native-test:{suffix}:b");
    let upgraded = install_or_upgrade_windows(&options).unwrap();
    assert_eq!(upgraded.active.release_id, second_release);
    assert_eq!(
        upgraded
            .rollback
            .as_ref()
            .map(|release| release.release_id.as_str()),
        Some(first_release.as_str())
    );
    assert_host_reachable(&state_dir);
    let repeated_upgrade = install_or_upgrade_windows(&options).unwrap();
    assert_eq!(repeated_upgrade, upgraded);
    assert_host_reachable(&state_dir);

    let rollback_output = Command::new(upgraded.active.directory.join("hostctl.exe"))
        .arg("windows-rollback")
        .arg("--install-root")
        .arg(&install_root)
        .output()
        .unwrap();
    assert!(
        rollback_output.status.success(),
        "hostctl windows-rollback failed: {}",
        String::from_utf8_lossy(&rollback_output.stderr)
    );
    let rolled_back: persistent_runtime_host::WindowsInstallationManifest =
        serde_json::from_slice(&rollback_output.stdout).unwrap();
    assert_eq!(rolled_back.active.release_id, first_release);
    assert_eq!(
        rolled_back
            .rollback
            .as_ref()
            .map(|release| release.release_id.as_str()),
        Some(second_release.as_str())
    );
    assert_host_reachable(&state_dir);

    let state_marker = state_dir.join("uninstall-preservation.marker");
    fs::write(&state_marker, b"preserve").unwrap();
    set_run_value(&unrelated_run_value, "cmd.exe /c exit 0");
    let uninstall_output = Command::new(rolled_back.active.directory.join("hostctl.exe"))
        .arg("windows-uninstall")
        .arg("--install-root")
        .arg(&install_root)
        .output()
        .unwrap();
    assert!(
        uninstall_output.status.success(),
        "hostctl windows-uninstall failed: {}",
        String::from_utf8_lossy(&uninstall_output.stderr)
    );
    let receipt: persistent_runtime_host::WindowsUninstallReceipt =
        serde_json::from_slice(&uninstall_output.stdout).unwrap();
    assert!(receipt.service_removed);
    assert!(receipt.tray_run_value_removed);
    assert!(query_windows_service(&service_name).unwrap().is_none());
    assert!(query_windows_tray_run_value(&run_value_name)
        .unwrap()
        .is_none());
    assert_eq!(
        query_run_value(&unrelated_run_value).as_deref(),
        Some("cmd.exe /c exit 0")
    );
    assert!(
        state_marker.is_file(),
        "uninstall must preserve state and logs"
    );
    assert!(install_root.join("installation-v1.json").is_file());
    let repeated_uninstall = uninstall_windows(&install_root).unwrap();
    assert!(!repeated_uninstall.service_removed);
    assert!(!repeated_uninstall.tray_run_value_removed);
    cleanup.finish();
}

fn assert_host_reachable(state_dir: &Path) {
    let response = local_client(state_dir)
        .call(&RequestEnvelope::v1(
            format!("stage4-reachable-{}", uuid::Uuid::new_v4()),
            Request::GetHostInfo(GetHostInfoParams {}),
        ))
        .unwrap();
    assert!(matches!(response.outcome, ResponseOutcome::Ok { .. }));
}

fn local_client(state_dir: &Path) -> LocalClient {
    LocalClient::new(RuntimePaths::new(state_dir).unwrap().socket)
}

fn register_service(client: &LocalClient, definition: ServiceDefinition) {
    let response = client
        .call(&RequestEnvelope::v1(
            format!("stage4-register-{}", uuid::Uuid::new_v4()),
            Request::RegisterService(persistent_runtime_host::RegisterServiceParams {
                definition,
                mutation: mutation("register"),
            }),
        ))
        .unwrap();
    assert!(matches!(
        response.outcome,
        ResponseOutcome::Ok { response }
            if matches!(*response, Response::RegisterService(_))
    ));
}

fn start_registered_service(client: &LocalClient, service_id: &str) {
    let response = client
        .call(&RequestEnvelope::v1(
            format!("stage4-start-{}", uuid::Uuid::new_v4()),
            Request::EnsureRunning(EnsureRunningParams {
                service_id: ServiceId::new(service_id),
                mutation: mutation("start"),
            }),
        ))
        .unwrap();
    assert!(matches!(response.outcome, ResponseOutcome::Ok { .. }));
}

fn service_definition(
    id: &str,
    working_directory: &Path,
    parent_pid_file: &Path,
    child_pid_file: &Path,
) -> ServiceDefinition {
    ServiceDefinition {
        id: ServiceId::new(id),
        display_name: "Stage 4 native cleanup fixture".to_owned(),
        description: None,
        executable: fixture_executable().to_string_lossy().into_owned(),
        arguments: vec![
            "--pid-file".to_owned(),
            parent_pid_file.to_string_lossy().into_owned(),
            "--child-pid-file".to_owned(),
            child_pid_file.to_string_lossy().into_owned(),
            "--heartbeat-ms".to_owned(),
            "10".to_owned(),
            "--ignore-term".to_owned(),
        ],
        working_directory: working_directory.to_string_lossy().into_owned(),
        environment: BTreeMap::new(),
        start_policy: StartPolicy::Manual,
        restart_policy: RestartPolicy::Never,
        dependencies: Vec::new(),
        readiness_probe: None,
        shutdown_policy: ShutdownPolicy {
            graceful_timeout_ms: 100,
            force_kill_timeout_ms: 3_000,
        },
        metadata: BTreeMap::new(),
    }
}

fn mutation(label: &str) -> MutationOptions {
    MutationOptions::new(format!("stage4-{label}-{}", uuid::Uuid::new_v4()))
}

fn wait_for_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "PID file did not appear: {path:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_recovery(
    service_name: &str,
    prior_pid: u32,
) -> persistent_runtime_host::WindowsServiceStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(status) = query_windows_service(service_name).unwrap() {
            if status.is_running() && status.process_id != 0 && status.process_id != prior_pid {
                return status;
            }
        }
        assert!(
            Instant::now() < deadline,
            "SCM did not recover the crashed PRH host"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn terminate_process(pid: u32) {
    let process = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!process.is_null(), "could not open service process {pid}");
    assert_ne!(unsafe { TerminateProcess(process, 197) }, 0);
    assert_ne!(
        unsafe { WaitForSingleObject(process, 10_000) },
        WAIT_TIMEOUT
    );
    unsafe {
        let _ = CloseHandle(process);
    }
}

fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + WAIT;
    while process_is_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "process {pid} survived containment cleanup"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn process_is_alive(pid: u32) -> bool {
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return false;
    }
    let result = unsafe { WaitForSingleObject(process, 0) } == WAIT_TIMEOUT;
    unsafe {
        let _ = CloseHandle(process);
    }
    result
}

fn set_run_value(name: &str, value: &str) {
    let key_path = wide(RUN_KEY);
    let mut key = null_mut();
    assert_eq!(
        unsafe {
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
        },
        ERROR_SUCCESS
    );
    let key = RegistryKey(key);
    let name = wide(name);
    let value = wide(value);
    assert_eq!(
        unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        },
        ERROR_SUCCESS
    );
}

fn query_run_value(name: &str) -> Option<String> {
    let key_path = wide(RUN_KEY);
    let mut key = null_mut();
    assert_eq!(
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_READ, &mut key) },
        ERROR_SUCCESS
    );
    let key = RegistryKey(key);
    let name = wide(name);
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
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut value = vec![0_u16; (bytes as usize).div_ceil(2)];
    assert_eq!(
        unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                null(),
                &mut value_type,
                value.as_mut_ptr().cast(),
                &mut bytes,
            )
        },
        ERROR_SUCCESS
    );
    while value.last() == Some(&0) {
        value.pop();
    }
    Some(String::from_utf16(&value).unwrap())
}

fn delete_run_value(name: &str) {
    let key_path = wide(RUN_KEY);
    let mut key = null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key_path.as_ptr(), 0, KEY_WRITE, &mut key) }
        != ERROR_SUCCESS
    {
        return;
    }
    let key = RegistryKey(key);
    let name = wide(name);
    unsafe {
        let _ = RegDeleteValueW(key.0, name.as_ptr());
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn host_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_prh-host"))
}

fn hostctl_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hostctl"))
}

fn fixture_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_prh-fixture-service"))
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

struct InstallationCleanup {
    install_root: PathBuf,
    unrelated_run_value: String,
    finished: std::cell::Cell<bool>,
}

impl InstallationCleanup {
    fn new(install_root: PathBuf, unrelated_run_value: String) -> Self {
        Self {
            install_root,
            unrelated_run_value,
            finished: std::cell::Cell::new(false),
        }
    }

    fn finish(&self) {
        delete_run_value(&self.unrelated_run_value);
        self.finished.set(true);
    }
}

impl Drop for InstallationCleanup {
    fn drop(&mut self) {
        if !self.finished.get() {
            let _ = uninstall_windows(&self.install_root);
            delete_run_value(&self.unrelated_run_value);
        }
    }
}
