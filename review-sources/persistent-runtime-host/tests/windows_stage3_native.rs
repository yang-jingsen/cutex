#![cfg(target_os = "windows")]

use persistent_runtime_host::{
    EnsureRunningParams, EnsureStoppedParams, GetHostInfoParams, LocalClient, MutationOptions,
    Request, RequestEnvelope, Response, ResponseOutcome, RestartPolicy, RuntimePaths,
    ServiceDefinition, ServiceId, ShutdownHostParams, ShutdownPolicy, StartPolicy, StopOutcome,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

const NATIVE_WAIT: Duration = Duration::from_secs(10);

#[test]
fn missing_windows_pipe_never_bootstraps_a_host_or_state_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join("absent-state");
    let paths = RuntimePaths::new(&state_directory).unwrap();
    let result = LocalClient::new(paths.socket).call(&RequestEnvelope::v1(
        request_id("missing"),
        Request::GetHostInfo(GetHostInfoParams {}),
    ));
    assert!(result.is_err());
    assert!(!state_directory.exists());
}

#[test]
#[ignore = "requires an explicitly approved isolated Windows acceptance environment"]
fn forced_stop_cleans_the_real_target_and_job_descendant() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join("state");
    let parent_pid_file = temporary.path().join("target.pid");
    let child_pid_file = temporary.path().join("descendant.pid");
    let mut host = NativeHost::start(&state_directory);
    let client = wait_for_host(&mut host, &state_directory);

    register_service(
        &client,
        service_definition(
            "forced-cleanup",
            temporary.path(),
            &parent_pid_file,
            &child_pid_file,
        ),
    );
    let response = call_ok(
        &client,
        Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::new("forced-cleanup"),
            mutation: mutation("start"),
        }),
    );
    let reported_pid = match response {
        Response::EnsureRunning(result) => {
            result
                .service
                .runtime
                .process
                .expect("running occurrence has process identity")
                .pid
        }
        other => panic!("unexpected EnsureRunning response: {other:?}"),
    };
    let parent_pid = wait_for_pid_file(&parent_pid_file);
    let child_pid = wait_for_pid_file(&child_pid_file);
    assert_eq!(reported_pid, parent_pid);
    assert!(process_is_alive(parent_pid));
    assert!(process_is_alive(child_pid));

    let response = call_ok(
        &client,
        Request::EnsureStopped(EnsureStoppedParams {
            service_id: ServiceId::new("forced-cleanup"),
            cascade: false,
            mutation: mutation("stop"),
        }),
    );
    match response {
        Response::EnsureStopped(result) => assert_eq!(
            result.service.runtime.last_stop_outcome,
            Some(StopOutcome::Forced)
        ),
        other => panic!("unexpected EnsureStopped response: {other:?}"),
    }
    wait_for_process_exit(parent_pid);
    wait_for_process_exit(child_pid);
    shutdown_host(&client);
    host.wait_for_exit();
}

#[test]
#[ignore = "requires an explicitly approved isolated Windows acceptance environment"]
fn abrupt_host_exit_activates_job_kill_on_close_for_target_and_descendant() {
    let temporary = tempfile::tempdir().unwrap();
    let state_directory = temporary.path().join("state");
    let parent_pid_file = temporary.path().join("target.pid");
    let child_pid_file = temporary.path().join("descendant.pid");
    let mut host = NativeHost::start(&state_directory);
    let client = wait_for_host(&mut host, &state_directory);

    register_service(
        &client,
        service_definition(
            "host-crash-cleanup",
            temporary.path(),
            &parent_pid_file,
            &child_pid_file,
        ),
    );
    let _ = call_ok(
        &client,
        Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::new("host-crash-cleanup"),
            mutation: mutation("start"),
        }),
    );
    let parent_pid = wait_for_pid_file(&parent_pid_file);
    let child_pid = wait_for_pid_file(&child_pid_file);
    assert!(process_is_alive(parent_pid));
    assert!(process_is_alive(child_pid));

    host.terminate_abruptly();
    wait_for_process_exit(parent_pid);
    wait_for_process_exit(child_pid);
}

fn service_definition(
    id: &str,
    working_directory: &Path,
    parent_pid_file: &Path,
    child_pid_file: &Path,
) -> ServiceDefinition {
    ServiceDefinition {
        id: ServiceId::new(id),
        display_name: format!("Windows Stage 3 fixture {id}"),
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

fn register_service(client: &LocalClient, definition: ServiceDefinition) {
    use persistent_runtime_host::RegisterServiceParams;
    let response = call_ok(
        client,
        Request::RegisterService(RegisterServiceParams {
            definition,
            mutation: mutation("register"),
        }),
    );
    assert!(matches!(response, Response::RegisterService(_)));
}

fn wait_for_host(host: &mut NativeHost, state_directory: &Path) -> LocalClient {
    let paths = RuntimePaths::new(state_directory).unwrap();
    let client = LocalClient::new(paths.socket);
    let deadline = Instant::now() + NATIVE_WAIT;
    loop {
        if client
            .call(&RequestEnvelope::v1(
                request_id("ready"),
                Request::GetHostInfo(GetHostInfoParams {}),
            ))
            .is_ok()
        {
            return client;
        }
        if let Some(status) = host.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("Windows PRH host exited before becoming ready: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "Windows PRH host did not become ready"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn call_ok(client: &LocalClient, request: Request) -> Response {
    let envelope = client
        .call(&RequestEnvelope::v1(request_id("call"), request))
        .unwrap();
    match envelope.outcome {
        ResponseOutcome::Ok { response } => *response,
        ResponseOutcome::Error { error } => panic!("PRH request failed: {error}"),
    }
}

fn shutdown_host(client: &LocalClient) {
    let response = call_ok(
        client,
        Request::ShutdownHost(ShutdownHostParams {
            mutation: mutation("shutdown"),
        }),
    );
    assert!(matches!(response, Response::ShutdownHost(_)));
}

fn mutation(label: &str) -> MutationOptions {
    MutationOptions::new(format!("native-{label}-{}", uuid::Uuid::new_v4()))
}

fn request_id(label: &str) -> String {
    format!("native-{label}-{}", uuid::Uuid::new_v4())
}

fn wait_for_pid_file(path: &Path) -> u32 {
    let deadline = Instant::now() + NATIVE_WAIT;
    loop {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        assert!(
            Instant::now() < deadline,
            "PID file did not appear: {path:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + NATIVE_WAIT;
    while process_is_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "Windows process {pid} survived its containment cleanup"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn process_is_alive(pid: u32) -> bool {
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return false;
    }
    let wait = unsafe { WaitForSingleObject(process, 0) };
    unsafe {
        let _ = CloseHandle(process);
    }
    wait == WAIT_TIMEOUT
}

fn host_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_prh-host"))
}

fn fixture_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_prh-fixture-service"))
}

struct NativeHost {
    child: Option<Child>,
}

impl NativeHost {
    fn start(state_directory: &Path) -> Self {
        let child = Command::new(host_executable())
            .arg("--state-dir")
            .arg(state_directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self { child: Some(child) }
    }

    fn terminate_abruptly(&mut self) {
        let mut child = self.child.take().expect("host already reaped");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + NATIVE_WAIT;
        let child = self.child.as_mut().expect("host already reaped");
        loop {
            if child.try_wait().unwrap().is_some() {
                self.child = None;
                return;
            }
            assert!(Instant::now() < deadline, "Windows PRH host did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
