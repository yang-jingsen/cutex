#![cfg(target_os = "linux")]

use persistent_runtime_host::linux_process::{LinuxProcessBackend, LinuxProcessConfig};
use persistent_runtime_host::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(12);

struct HostProcess {
    child: Option<Child>,
    stderr_path: PathBuf,
}

impl HostProcess {
    fn id(&self) -> u32 {
        self.child.as_ref().unwrap().id()
    }

    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        self.child.take().unwrap().wait().unwrap()
    }

    fn diagnostics(&self) -> String {
        std::fs::read_to_string(&self.stderr_path).unwrap_or_default()
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if std::thread::panicking() {
            eprintln!("prh-host diagnostics:\n{}", self.diagnostics());
        }
    }
}

#[test]
fn explicit_host_refuses_to_replace_a_non_socket_endpoint_collision() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    std::fs::create_dir(&state_dir).unwrap();
    let collision = state_dir.join("prh-v1.sock");
    std::fs::write(&collision, b"do-not-replace").unwrap();
    let output = Command::new(host_binary())
        .arg("--state-dir")
        .arg(&state_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refusing to replace non-socket"));
    assert_eq!(std::fs::read(&collision).unwrap(), b"do-not-replace");
}

#[test]
fn real_hostctl_lifecycle_rotates_logs_and_cleans_multi_service_groups() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let absent = hostctl(&state_dir, &["info"]);
    assert!(!absent.status.success());
    assert!(String::from_utf8_lossy(&absent.stderr).contains("never starts PRH implicitly"));
    assert!(!state_dir.exists());
    let mut host = spawn_host(directory.path(), &state_dir, 1_024, 3);
    assert_endpoint_boundaries(&state_dir);

    let second = Command::new(host_binary())
        .arg("--state-dir")
        .arg(&state_dir)
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("another PRH instance"));

    match successful(hostctl(&state_dir, &["info"])) {
        Response::GetHostInfo(info) => {
            assert_eq!(info.product, "persistent-runtime-host");
            assert_eq!(info.backend, "linux_process_group_sentinel");
        }
        response => panic!("unexpected info response: {response:?}"),
    }

    let dependency_port = unused_tcp_port();
    let mut app_port = unused_tcp_port();
    while app_port == dependency_port {
        app_port = unused_tcp_port();
    }
    let dependency_pid = directory.path().join("dependency.pid");
    let dependency_child_pid = directory.path().join("dependency-child.pid");
    let app_pid = directory.path().join("app.pid");
    let app_child_pid = directory.path().join("app-child.pid");
    let event_client = LocalClient::new(state_dir.join("prh-v1.sock"));
    let (subscription_response, mut event_reader) = event_client
        .subscribe(&RequestEnvelope::v1(
            "stage2-event-stream",
            Request::SubscribeEvents(SubscribeEventsParams {
                after_sequence: None,
                replay_limit: 100,
            }),
        ))
        .unwrap();
    let subscription_id = match subscription_response.outcome {
        ResponseOutcome::Ok { response } => match *response {
            Response::SubscribeEvents(result) => result.subscription_id,
            response => panic!("unexpected subscription response: {response:?}"),
        },
        ResponseOutcome::Error { error } => panic!("subscription failed: {error:?}"),
    };
    event_reader
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    register_definition(
        &state_dir,
        directory.path(),
        fixture_definition(
            "dependency",
            dependency_port,
            &dependency_pid,
            &dependency_child_pid,
            &[],
            false,
        ),
    );
    let event = event_reader.next_event().unwrap().unwrap();
    assert_eq!(event.subscription_id, subscription_id);
    assert_eq!(event.protocol, ProtocolVersion::V1);
    drop(event_reader);
    let mut app_definition = fixture_definition(
        "app",
        app_port,
        &app_pid,
        &app_child_pid,
        &["dependency"],
        false,
    );
    app_definition.readiness_probe = Some(ReadinessProbe::Http {
        url: format!("http://127.0.0.1:{app_port}/ready"),
        interval_ms: 50,
        timeout_ms: 25,
        expected_status_min: 200,
        expected_status_max: 299,
    });
    register_definition(&state_dir, directory.path(), app_definition);

    let app_started = match successful(hostctl(&state_dir, &["start", "app"])) {
        Response::EnsureRunning(result) => result,
        response => panic!("unexpected start response: {response:?}"),
    };
    let first_app_run = app_started.service.runtime.run_id.clone().unwrap();
    assert!(app_started.service.runtime.started_at_ms.unwrap() > 1_500_000_000_000);
    let first_app_leader = wait_pid_file(&app_pid, None);
    let first_app_child = wait_pid_file(&app_child_pid, None);
    let dependency_leader = wait_pid_file(&dependency_pid, None);
    let dependency_child = wait_pid_file(&dependency_child_pid, None);
    let first_app_leader_marker = process_marker(first_app_leader);
    let first_app_child_marker = process_marker(first_app_child);
    let dependency_leader_marker = process_marker(dependency_leader);
    let dependency_child_marker = process_marker(dependency_child);
    assert_direct_fixture(first_app_leader);
    assert_direct_fixture(dependency_leader);
    assert_eq!(process_group(first_app_leader), first_app_leader);
    assert_eq!(process_group(first_app_child), first_app_leader);
    assert_eq!(process_group(dependency_leader), dependency_leader);
    assert_eq!(process_group(dependency_child), dependency_leader);
    assert_eq!(sentinel_pids(host.id()).len(), 2);

    match successful(hostctl(&state_dir, &["get", "dependency"])) {
        Response::GetService(service) => {
            assert_eq!(service.runtime.observed_state, ObservedState::Running)
        }
        response => panic!("unexpected get response: {response:?}"),
    }

    let rejected = hostctl(&state_dir, &["stop", "dependency"]);
    assert!(!rejected.status.success());
    let rejected: ResponseEnvelope = serde_json::from_slice(&rejected.stdout).unwrap();
    assert!(matches!(
        rejected.outcome,
        ResponseOutcome::Error {
            error: ApiError {
                code: ErrorCode::DependencyInUse,
                ..
            }
        }
    ));

    wait_until(WAIT, || {
        match successful(hostctl(&state_dir, &["logs", "app", "--limit", "10000"])) {
            Response::ReadLogs(page) => {
                page.entries
                    .iter()
                    .any(|entry| entry.stream == LogStream::Stdout)
                    && page
                        .entries
                        .iter()
                        .any(|entry| entry.stream == LogStream::Stderr)
            }
            _ => false,
        }
    });
    assert_log_files_bounded(&state_dir, "app", 1_024, 3);

    let restarted = match successful(hostctl(&state_dir, &["restart", "app"])) {
        Response::Restart(result) => result,
        response => panic!("unexpected restart response: {response:?}"),
    };
    assert_ne!(
        restarted.service.runtime.run_id.as_ref(),
        Some(&first_app_run)
    );
    wait_identity_gone(first_app_leader, first_app_leader_marker);
    wait_identity_gone(first_app_child, first_app_child_marker);
    let second_app_leader = wait_pid_file(&app_pid, Some(first_app_leader));
    let second_app_child = wait_pid_file(&app_child_pid, Some(first_app_child));
    let second_app_leader_marker = process_marker(second_app_leader);
    let second_app_child_marker = process_marker(second_app_child);
    assert_eq!(process_group(second_app_child), second_app_leader);
    wait_until(WAIT, || sentinel_pids(host.id()).len() == 2);

    match successful(hostctl(&state_dir, &["stop", "app"])) {
        Response::EnsureStopped(result) => {
            assert_eq!(
                result.service.runtime.last_stop_outcome,
                Some(StopOutcome::Graceful)
            );
        }
        response => panic!("unexpected stop response: {response:?}"),
    }
    wait_identity_gone(second_app_leader, second_app_leader_marker);
    wait_identity_gone(second_app_child, second_app_child_marker);
    wait_until(WAIT, || sentinel_pids(host.id()).len() == 1);

    match successful(hostctl(&state_dir, &["stop", "dependency"])) {
        Response::EnsureStopped(result) => assert_eq!(
            result.service.runtime.last_stop_outcome,
            Some(StopOutcome::Graceful)
        ),
        response => panic!("unexpected dependency stop response: {response:?}"),
    }
    wait_identity_gone(dependency_leader, dependency_leader_marker);
    wait_identity_gone(dependency_child, dependency_child_marker);
    wait_until(WAIT, || sentinel_pids(host.id()).is_empty());

    let stubborn_pid = directory.path().join("stubborn.pid");
    let stubborn_child_pid = directory.path().join("stubborn-child.pid");
    register_definition(
        &state_dir,
        directory.path(),
        fixture_definition(
            "stubborn",
            unused_tcp_port(),
            &stubborn_pid,
            &stubborn_child_pid,
            &[],
            true,
        ),
    );
    successful(hostctl(&state_dir, &["start", "stubborn"]));
    let stubborn_leader = wait_pid_file(&stubborn_pid, None);
    let stubborn_child = wait_pid_file(&stubborn_child_pid, None);
    let stubborn_leader_marker = process_marker(stubborn_leader);
    let stubborn_child_marker = process_marker(stubborn_child);
    match successful(hostctl(&state_dir, &["stop", "stubborn"])) {
        Response::EnsureStopped(result) => assert_eq!(
            result.service.runtime.last_stop_outcome,
            Some(StopOutcome::Forced)
        ),
        response => panic!("unexpected forced stop response: {response:?}"),
    }
    wait_identity_gone(stubborn_leader, stubborn_leader_marker);
    wait_identity_gone(stubborn_child, stubborn_child_marker);
    wait_until(WAIT, || sentinel_pids(host.id()).is_empty());

    successful(hostctl(&state_dir, &["shutdown"]));
    let status = host.wait_for_exit();
    assert!(status.success(), "host diagnostics: {}", host.diagnostics());
    assert!(!state_dir.join("prh-v1.sock").exists());
}

#[test]
fn host_sigkill_is_detected_by_eof_and_leaves_no_contained_process_or_sentinel() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let mut host = spawn_host(directory.path(), &state_dir, 4_096, 2);
    let pid_file = directory.path().join("crash.pid");
    let child_pid_file = directory.path().join("crash-child.pid");
    register_definition(
        &state_dir,
        directory.path(),
        fixture_definition(
            "crash",
            unused_tcp_port(),
            &pid_file,
            &child_pid_file,
            &[],
            true,
        ),
    );
    successful(hostctl(&state_dir, &["start", "crash"]));
    let leader = wait_pid_file(&pid_file, None);
    let descendant = wait_pid_file(&child_pid_file, None);
    let leader_marker = process_marker(leader);
    let descendant_marker = process_marker(descendant);
    let sentinel = wait_single_sentinel(host.id());
    let sentinel_marker = process_marker(sentinel);

    send_signal(host.id(), libc::SIGKILL);
    let status = host.wait_for_exit();
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    wait_identity_gone(leader, leader_marker);
    wait_identity_gone(descendant, descendant_marker);
    wait_identity_gone(sentinel, sentinel_marker);

    // A later explicit bootstrap may recover the stale socket left by SIGKILL.
    let mut recovered = spawn_host(directory.path(), &state_dir, 4_096, 2);
    match successful(hostctl(&state_dir, &["get", "crash"])) {
        Response::GetService(service) => {
            assert_eq!(service.runtime.observed_state, ObservedState::Stopped);
            assert_eq!(service.runtime.definition_revision, 1);
        }
        response => panic!("unexpected recovered service response: {response:?}"),
    }
    successful(hostctl(&state_dir, &["shutdown"]));
    assert!(recovered.wait_for_exit().success());
}

#[test]
fn sentinel_loss_fails_closed_reports_failure_and_leaves_no_occurrence() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let mut host = spawn_host(directory.path(), &state_dir, 4_096, 2);
    let pid_file = directory.path().join("sentinel-loss.pid");
    let child_pid_file = directory.path().join("sentinel-loss-child.pid");
    register_definition(
        &state_dir,
        directory.path(),
        fixture_definition(
            "sentinel-loss",
            unused_tcp_port(),
            &pid_file,
            &child_pid_file,
            &[],
            true,
        ),
    );
    successful(hostctl(&state_dir, &["start", "sentinel-loss"]));
    let leader = wait_pid_file(&pid_file, None);
    let descendant = wait_pid_file(&child_pid_file, None);
    let leader_marker = process_marker(leader);
    let descendant_marker = process_marker(descendant);
    let sentinel = wait_single_sentinel(host.id());
    let sentinel_marker = process_marker(sentinel);

    send_signal(sentinel, libc::SIGKILL);
    wait_identity_gone(sentinel, sentinel_marker);
    wait_identity_gone(leader, leader_marker);
    wait_identity_gone(descendant, descendant_marker);
    wait_until(WAIT, || {
        match successful(hostctl(&state_dir, &["get", "sentinel-loss"])) {
            Response::GetService(service) => {
                service.runtime.run_id.is_none()
                    && service.runtime.observed_state == ObservedState::Failed
                    && service
                        .runtime
                        .last_exit
                        .is_some_and(|exit| exit.unexpected)
            }
            _ => false,
        }
    });
    wait_until(WAIT, || {
        match successful(hostctl(
            &state_dir,
            &["logs", "sentinel-loss", "--limit", "100"],
        )) {
            Response::ReadLogs(page) => page
                .diagnostics
                .last_error
                .is_some_and(|message| message.contains("containment sentinel disappeared")),
            _ => false,
        }
    });
    assert!(sentinel_pids(host.id()).is_empty());

    successful(hostctl(&state_dir, &["shutdown"]));
    assert!(host.wait_for_exit().success());
}

#[test]
fn saturated_process_output_queue_keeps_draining_and_reports_loss() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("bounded.pid");
    let child_pid_file = directory.path().join("bounded-child.pid");
    let mut definition = fixture_definition(
        "bounded-output",
        unused_tcp_port(),
        &pid_file,
        &child_pid_file,
        &[],
        false,
    );
    definition.readiness_probe = None;
    definition.shutdown_policy.graceful_timeout_ms = 2_000;
    let mut config = LinuxProcessConfig::new(sentinel_binary());
    config.event_queue_capacity = 1;
    let (backend, events) = LinuxProcessBackend::new(config).unwrap();
    let run_id = RunId::from("bounded-run");
    let identity = backend.start(&definition, &run_id).unwrap();
    let leader = wait_pid_file(&pid_file, None);
    let descendant = wait_pid_file(&child_pid_file, None);
    assert_eq!(identity.pid, leader);
    let leader_marker = process_marker(leader);
    let descendant_marker = process_marker(descendant);
    let sentinel = wait_sentinel_for_group(leader);
    let sentinel_marker = process_marker(sentinel);

    // No receiver consumes the capacity-one queue during this interval. The
    // fixture emits far more than one pipe chunk, yet remains stoppable.
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        backend
            .stop(&definition.id, &run_id, &definition.shutdown_policy)
            .unwrap(),
        StopOutcome::Graceful
    );
    wait_identity_gone(leader, leader_marker);
    wait_identity_gone(descendant, descendant_marker);
    wait_identity_gone(sentinel, sentinel_marker);

    let deadline = Instant::now() + WAIT;
    let mut observed_loss = false;
    while Instant::now() < deadline {
        match events.recv_timeout(Duration::from_millis(100)) {
            Ok(BackendEvent::LogFailure { message, .. })
                if message.contains("bounded process-output queue dropped") =>
            {
                observed_loss = true;
                break;
            }
            Ok(_) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(observed_loss, "bounded queue loss was not reported");
}

#[test]
fn readiness_timeout_fails_start_and_cleans_the_unreported_occurrence() {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("not-ready.pid");
    let child_pid_file = directory.path().join("not-ready-child.pid");
    let mut definition = fixture_definition(
        "not-ready",
        unused_tcp_port(),
        &pid_file,
        &child_pid_file,
        &[],
        true,
    );
    // Keep the probe but do not give the fixture a listener.
    definition.arguments.drain(0..2);
    let mut config = LinuxProcessConfig::new(sentinel_binary());
    config.readiness_deadline = Duration::from_millis(150);
    let (backend, _events) = LinuxProcessBackend::new(config).unwrap();
    let error = backend
        .start(&definition, &RunId::from("not-ready-run"))
        .unwrap_err();
    assert!(error.message.contains("readiness did not succeed"));
    let leader = wait_pid_value(&pid_file);
    let descendant = wait_pid_value(&child_pid_file);
    wait_until(WAIT, || process_marker_if_alive(leader).is_none());
    wait_until(WAIT, || process_marker_if_alive(descendant).is_none());
    wait_until(WAIT, || sentinel_for_group(leader).is_none());
}

#[test]
fn real_unexpected_exit_honors_backoff_and_stops_at_the_window_budget() {
    let directory = tempfile::tempdir().unwrap();
    let state_dir = directory.path().join("state");
    let mut host = spawn_host(directory.path(), &state_dir, 4_096, 2);
    let pid_file = directory.path().join("restart.pid");
    let port = unused_tcp_port();
    let mut definition = fixture_definition(
        "restart-budget",
        port,
        &pid_file,
        &directory.path().join("unused-child.pid"),
        &[],
        false,
    );
    definition.arguments = vec![
        "--tcp-port".to_owned(),
        port.to_string(),
        "--heartbeat-ms".to_owned(),
        "5".to_owned(),
        "--pid-file".to_owned(),
        pid_file.display().to_string(),
        "--exit-after-ms".to_owned(),
        "80".to_owned(),
        "--exit-code".to_owned(),
        "23".to_owned(),
    ];
    definition.restart_policy = RestartPolicy::BoundedOnFailure {
        max_restarts: 2,
        window_ms: 2_000,
        backoff_ms: 120,
    };
    register_definition(&state_dir, directory.path(), definition);

    let started_at = Instant::now();
    let initial_run = match successful(hostctl(&state_dir, &["start", "restart-budget"])) {
        Response::EnsureRunning(result) => result.service.runtime.run_id.unwrap(),
        response => panic!("unexpected restart-budget start response: {response:?}"),
    };
    wait_until(WAIT, || {
        match successful(hostctl(&state_dir, &["get", "restart-budget"])) {
            Response::GetService(service) => {
                service.runtime.restart_count >= 1
                    && service
                        .runtime
                        .run_id
                        .as_ref()
                        .is_some_and(|run| run != &initial_run)
            }
            _ => false,
        }
    });
    assert!(started_at.elapsed() >= Duration::from_millis(180));

    wait_until(WAIT, || {
        match successful(hostctl(&state_dir, &["get", "restart-budget"])) {
            Response::GetService(service) => {
                service.runtime.restart_count == 2
                    && service.runtime.run_id.is_none()
                    && service.runtime.observed_state == ObservedState::Failed
                    && service
                        .runtime
                        .last_exit
                        .is_some_and(|exit| exit.exit_code == Some(23) && exit.unexpected)
            }
            _ => false,
        }
    });
    wait_until(WAIT, || sentinel_pids(host.id()).is_empty());
    successful(hostctl(&state_dir, &["shutdown"]));
    assert!(host.wait_for_exit().success());
}

fn spawn_host(root: &Path, state_dir: &Path, log_bytes: u64, log_files: usize) -> HostProcess {
    let stderr_path = root.join(format!("host-{}.stderr", uuid::Uuid::new_v4()));
    let stderr = std::fs::File::create(&stderr_path).unwrap();
    let child = Command::new(host_binary())
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--log-max-bytes")
        .arg(log_bytes.to_string())
        .arg("--log-files")
        .arg(log_files.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let mut host = HostProcess {
        child: Some(child),
        stderr_path,
    };
    wait_until(WAIT, || {
        if let Some(status) = host.child.as_mut().unwrap().try_wait().unwrap() {
            panic!(
                "host exited before readiness ({status}); diagnostics: {}",
                host.diagnostics()
            );
        }
        hostctl(state_dir, &["info"]).status.success()
    });
    host
}

fn register_definition(state_dir: &Path, root: &Path, definition: ServiceDefinition) {
    let path = root.join(format!("{}.json", definition.id));
    std::fs::write(&path, serde_json::to_vec_pretty(&definition).unwrap()).unwrap();
    let output = hostctl(
        state_dir,
        &["register", path.to_str().expect("UTF-8 definition path")],
    );
    successful(output);
}

fn assert_endpoint_boundaries(state_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;

    let socket = state_dir.join("prh-v1.sock");
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let mut stream = UnixStream::connect(&socket).unwrap();
    stream.write_all(b"{malformed-json}\n").unwrap();
    let response: ResponseEnvelope =
        persistent_runtime_host::transport::read_json_frame(&mut std::io::BufReader::new(stream))
            .unwrap()
            .unwrap();
    assert!(matches!(
        response.outcome,
        ResponseOutcome::Error {
            error: ApiError {
                code: ErrorCode::InvalidRequest,
                ..
            }
        }
    ));
}

fn fixture_definition(
    id: &str,
    port: u16,
    pid_file: &Path,
    child_pid_file: &Path,
    dependencies: &[&str],
    ignore_term: bool,
) -> ServiceDefinition {
    let mut arguments = vec![
        "--tcp-port".to_owned(),
        port.to_string(),
        "--heartbeat-ms".to_owned(),
        "2".to_owned(),
        "--payload-bytes".to_owned(),
        "512".to_owned(),
        "--pid-file".to_owned(),
        pid_file.display().to_string(),
        "--child-pid-file".to_owned(),
        child_pid_file.display().to_string(),
    ];
    if ignore_term {
        arguments.push("--ignore-term".to_owned());
    }
    ServiceDefinition {
        id: ServiceId::from(id),
        display_name: format!("Stage 2 fixture {id}"),
        description: Some("isolated executable fixture".to_owned()),
        executable: fixture_binary().display().to_string(),
        arguments,
        working_directory: pid_file.parent().unwrap().display().to_string(),
        environment: BTreeMap::from([("PRH_STAGE2_FIXTURE".to_owned(), id.to_owned())]),
        start_policy: StartPolicy::Manual,
        restart_policy: RestartPolicy::Never,
        dependencies: dependencies.iter().copied().map(ServiceId::from).collect(),
        readiness_probe: Some(ReadinessProbe::Tcp {
            host: "127.0.0.1".to_owned(),
            port,
            interval_ms: 20,
            timeout_ms: 10,
        }),
        shutdown_policy: ShutdownPolicy {
            graceful_timeout_ms: 250,
            force_kill_timeout_ms: 2_000,
        },
        metadata: BTreeMap::new(),
    }
}

fn hostctl(state_dir: &Path, arguments: &[&str]) -> Output {
    Command::new(hostctl_binary())
        .arg("--state-dir")
        .arg(state_dir)
        .args(arguments)
        .output()
        .unwrap()
}

fn successful(output: Output) -> Response {
    assert!(
        output.status.success(),
        "hostctl failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: ResponseEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    match envelope.outcome {
        ResponseOutcome::Ok { response } => *response,
        ResponseOutcome::Error { error } => panic!("host returned API error: {error:?}"),
    }
}

fn unused_tcp_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_pid_file(path: &Path, different_from: Option<u32>) -> u32 {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        let selected = std::fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|pid| Some(*pid) != different_from && process_marker_if_alive(*pid).is_some());
        if let Some(pid) = selected {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "PID file {} did not identify a live process; final contents: {:?}",
        path.display(),
        std::fs::read_to_string(path)
    );
}

fn wait_pid_value(path: &Path) -> u32 {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("PID file {} was not written", path.display());
}

fn wait_single_sentinel(host_pid: u32) -> u32 {
    let mut selected = None;
    wait_until(WAIT, || {
        let sentinels = sentinel_pids(host_pid);
        if sentinels.len() == 1 {
            selected = sentinels.first().copied();
            true
        } else {
            false
        }
    });
    selected.unwrap()
}

fn wait_sentinel_for_group(process_group: u32) -> u32 {
    let mut selected = None;
    wait_until(WAIT, || {
        selected = sentinel_for_group(process_group);
        selected.is_some()
    });
    selected.unwrap()
}

fn sentinel_for_group(process_group: u32) -> Option<u32> {
    let expected = process_group.to_string();
    std::fs::read_dir("/proc")
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            let arguments = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let arguments = arguments
                .split(|byte| *byte == 0)
                .filter_map(|argument| std::str::from_utf8(argument).ok())
                .collect::<Vec<_>>();
            arguments.first().is_some_and(|executable| {
                Path::new(executable)
                    .file_name()
                    .is_some_and(|name| name == "prh-linux-sentinel")
            }) && arguments
                .windows(2)
                .any(|pair| pair == ["--process-group", expected.as_str()])
        })
}

fn sentinel_pids(host_pid: u32) -> Vec<u32> {
    direct_children(host_pid)
        .into_iter()
        .filter(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe"))
                .ok()
                .and_then(|path| path.file_name().map(|name| name.to_owned()))
                .is_some_and(|name| name == "prh-linux-sentinel")
        })
        .collect()
}

fn direct_children(pid: u32) -> Vec<u32> {
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .collect()
}

fn assert_direct_fixture(pid: u32) {
    assert_eq!(
        std::fs::canonicalize(format!("/proc/{pid}/exe")).unwrap(),
        std::fs::canonicalize(fixture_binary()).unwrap()
    );
}

fn process_group(pid: u32) -> u32 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let close = stat.rfind(')').unwrap();
    stat[close + 1..]
        .split_whitespace()
        .nth(2)
        .unwrap()
        .parse()
        .unwrap()
}

fn process_marker(pid: u32) -> String {
    process_marker_if_alive(pid).unwrap_or_else(|| panic!("process {pid} is not alive"))
}

fn process_marker_if_alive(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

fn wait_identity_gone(pid: u32, marker: String) {
    wait_until(WAIT, || {
        process_marker_if_alive(pid).is_none_or(|current| current != marker)
    });
}

fn send_signal(pid: u32, signal: i32) {
    assert!(pid > 1);
    // SAFETY: tests signal only exact PIDs they just discovered and validate.
    assert_eq!(unsafe { libc::kill(pid as i32, signal) }, 0);
}

fn assert_log_files_bounded(state_dir: &Path, service_id: &str, bytes: u64, files: usize) {
    let prefix = format!("{service_id}.jsonl");
    let entries = std::fs::read_dir(state_dir.join("logs-v1"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .collect::<Vec<_>>();
    assert!(!entries.is_empty());
    assert!(entries.len() <= files);
    assert!(entries.iter().all(|entry| match entry.metadata() {
        Ok(metadata) => metadata.len() <= bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => panic!("failed to inspect rotated log: {error}"),
    }));
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        condition(),
        "condition did not become true within {timeout:?}"
    );
}

fn host_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_prh-host"))
}

fn hostctl_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_hostctl"))
}

fn fixture_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_prh-fixture-service"))
}

fn sentinel_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_prh-linux-sentinel"))
}
