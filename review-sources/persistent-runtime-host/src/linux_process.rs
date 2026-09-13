#![cfg(target_os = "linux")]

use crate::model::{
    HealthState, ProcessIdentity, ReadinessProbe, RunId, ServiceDefinition, ServiceId,
    ShutdownPolicy, StopOutcome,
};
use crate::process_backend::{BackendEvent, ProcessBackend, ProcessBackendError};
use crate::protocol::LogStream;
use std::collections::BTreeMap;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct LinuxProcessConfig {
    pub sentinel_executable: PathBuf,
    pub sentinel_ready_timeout: Duration,
    pub readiness_deadline: Duration,
    pub descendant_cleanup_timeout: Duration,
    pub event_queue_capacity: usize,
}

impl LinuxProcessConfig {
    pub fn new(sentinel_executable: impl Into<PathBuf>) -> Self {
        Self {
            sentinel_executable: sentinel_executable.into(),
            sentinel_ready_timeout: Duration::from_secs(3),
            readiness_deadline: Duration::from_secs(15),
            descendant_cleanup_timeout: Duration::from_secs(3),
            event_queue_capacity: 1_024,
        }
    }
}

pub struct LinuxProcessBackend {
    config: LinuxProcessConfig,
    shared: Arc<Shared>,
    launcher: SyncSender<LaunchRequest>,
}

struct LaunchRequest {
    definition: ServiceDefinition,
    reply: SyncSender<Result<Child, ProcessBackendError>>,
}

struct Shared {
    event_sender: SyncSender<BackendEvent>,
    processes: Mutex<BTreeMap<(ServiceId, RunId), Arc<ProcessRecord>>>,
}

struct ProcessRecord {
    process_group: i32,
    // Closing the write side on host death produces EOF in exactly this
    // occurrence's sentinel, without a reusable PID identity.
    _sentinel_liveness: UnixStream,
    containment_failure: Mutex<Option<String>>,
    exit: Mutex<Option<ExitRecord>>,
    exit_changed: Condvar,
}

#[derive(Clone, Copy)]
struct ExitRecord {
    process_group_clean: bool,
    forced_descendant_cleanup: bool,
}

impl LinuxProcessBackend {
    pub fn new(
        config: LinuxProcessConfig,
    ) -> Result<(Self, Receiver<BackendEvent>), ProcessBackendError> {
        if !config.sentinel_executable.is_absolute() {
            return Err(ProcessBackendError::new(
                "sentinel executable must be an absolute path",
            ));
        }
        let launcher = spawn_direct_launcher()?;
        let (event_sender, event_receiver) = mpsc::sync_channel(config.event_queue_capacity.max(1));
        Ok((
            Self {
                config,
                launcher,
                shared: Arc::new(Shared {
                    event_sender,
                    processes: Mutex::new(BTreeMap::new()),
                }),
            },
            event_receiver,
        ))
    }

    fn active_process_groups(&self) -> Vec<i32> {
        self.shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .values()
            .map(|record| record.process_group)
            .collect()
    }

    fn launch_sentinel(
        &self,
        process_group: i32,
        liveness_reader: UnixStream,
    ) -> Result<Child, ProcessBackendError> {
        let mut command = Command::new(&self.config.sentinel_executable);
        command
            .arg("--process-group")
            .arg(process_group.to_string())
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(liveness_reader)))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut sentinel = command.spawn().map_err(|error| {
            ProcessBackendError::new(format!("failed to start containment sentinel: {error}"))
        })?;
        let stdout = match sentinel.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = sentinel.kill();
                let _ = sentinel.wait();
                return Err(ProcessBackendError::new(
                    "containment sentinel has no readiness pipe",
                ));
            }
        };
        match read_sentinel_readiness(stdout, self.config.sentinel_ready_timeout) {
            Ok(line) if line.trim() == "READY" => Ok(sentinel),
            Ok(line) => {
                let _ = sentinel.kill();
                let _ = sentinel.wait();
                Err(ProcessBackendError::new(format!(
                    "containment sentinel returned unexpected readiness: {line:?}"
                )))
            }
            Err(error) => {
                let _ = sentinel.kill();
                let _ = sentinel.wait();
                Err(error)
            }
        }
    }

    fn wait_until_ready(
        &self,
        definition: &ServiceDefinition,
        record: &ProcessRecord,
    ) -> Result<(), ProcessBackendError> {
        if let Some(message) = record
            .containment_failure
            .lock()
            .expect("containment failure lock poisoned")
            .clone()
        {
            return Err(ProcessBackendError::new(message));
        }
        let Some(probe) = &definition.readiness_probe else {
            return Ok(());
        };
        let deadline = Instant::now() + self.config.readiness_deadline;
        loop {
            if let Some(message) = record
                .containment_failure
                .lock()
                .expect("containment failure lock poisoned")
                .clone()
            {
                return Err(ProcessBackendError::new(message));
            }
            if record
                .exit
                .lock()
                .expect("process exit lock poisoned")
                .is_some()
            {
                return Err(ProcessBackendError::new(
                    "service exited before its readiness probe succeeded",
                ));
            }
            if probe_once(probe) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ProcessBackendError::new(format!(
                    "service readiness did not succeed within {} milliseconds",
                    self.config.readiness_deadline.as_millis()
                )));
            }
            let interval = match probe {
                ReadinessProbe::Tcp { interval_ms, .. }
                | ReadinessProbe::Http { interval_ms, .. } => *interval_ms,
            };
            thread::sleep(Duration::from_millis(interval).min(Duration::from_millis(250)));
        }
    }

    fn start_health_monitor(
        &self,
        definition: &ServiceDefinition,
        service_id: ServiceId,
        run_id: RunId,
        record: Arc<ProcessRecord>,
    ) -> Result<(), ProcessBackendError> {
        let Some(probe) = definition.readiness_probe.clone() else {
            return Ok(());
        };
        let sender = self.shared.event_sender.clone();
        thread::Builder::new()
            .name("prh-health".to_owned())
            .spawn(move || loop {
                let interval = match &probe {
                    ReadinessProbe::Tcp { interval_ms, .. }
                    | ReadinessProbe::Http { interval_ms, .. } => *interval_ms,
                };
                thread::sleep(Duration::from_millis(interval));
                if record
                    .exit
                    .lock()
                    .expect("process exit lock poisoned")
                    .is_some()
                {
                    break;
                }
                let health = if probe_once(&probe) {
                    HealthState::Healthy
                } else {
                    HealthState::Unhealthy
                };
                match sender.try_send(BackendEvent::Health {
                    service_id: service_id.clone(),
                    run_id: run_id.clone(),
                    health,
                }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
            })
            .map(|_| ())
            .map_err(|error| {
                ProcessBackendError::new(format!("failed to start health monitor: {error}"))
            })
    }

    fn terminate_failed_start(&self, process_group: i32, record: &ProcessRecord) {
        let _ = signal_process_group(process_group, libc::SIGKILL);
        let _ = wait_for_exit(record, self.config.descendant_cleanup_timeout);
    }

    fn start_sentinel_monitor(
        &self,
        mut sentinel: Child,
        service_id: ServiceId,
        run_id: RunId,
        record: Arc<ProcessRecord>,
    ) -> Result<(), ProcessBackendError> {
        let sentinel_pid = sentinel.id();
        let sender = self.shared.event_sender.clone();
        let cleanup_record = record.clone();
        thread::Builder::new()
            .name("prh-sentinel".to_owned())
            .spawn(move || {
                let status = sentinel.wait();
                if record
                    .exit
                    .lock()
                    .expect("process exit lock poisoned")
                    .is_some()
                    || !process_group_exists(record.process_group)
                {
                    return;
                }
                let status = match status {
                    Ok(status) => status.to_string(),
                    Err(error) => format!("wait failed: {error}"),
                };
                let message = format!(
                    "containment sentinel disappeared while the occurrence was running ({status}); terminating the process group fail-closed"
                );
                *record
                    .containment_failure
                    .lock()
                    .expect("containment failure lock poisoned") = Some(message.clone());
                let _ = signal_process_group(record.process_group, libc::SIGKILL);
                let _ = sender.send(BackendEvent::ContainmentFailure {
                    service_id,
                    run_id,
                    message,
                });
            })
            .map(|_| ())
            .map_err(|error| {
                terminate_exact_child(sentinel_pid);
                let message = format!(
                    "failed to start containment sentinel monitor: {error}; terminating the occurrence fail-closed"
                );
                *cleanup_record
                    .containment_failure
                    .lock()
                    .expect("containment failure lock poisoned") = Some(message.clone());
                let _ = signal_process_group(cleanup_record.process_group, libc::SIGKILL);
                ProcessBackendError::new(message)
            })
    }
}

impl ProcessBackend for LinuxProcessBackend {
    fn name(&self) -> &'static str {
        "linux_process_group_sentinel"
    }

    fn start(
        &self,
        definition: &ServiceDefinition,
        run_id: &RunId,
    ) -> Result<ProcessIdentity, ProcessBackendError> {
        if self
            .shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .keys()
            .any(|(service_id, _)| service_id == &definition.id)
        {
            return Err(ProcessBackendError::new(
                "a prior occurrence is still tracked or awaiting contained process-group cleanup",
            )
            .retryable());
        }
        let (reply, result) = mpsc::sync_channel(1);
        self.launcher
            .send(LaunchRequest {
                definition: definition.clone(),
                reply,
            })
            .map_err(|_| {
                ProcessBackendError::new(
                    "dedicated direct-launch thread is unavailable; refusing a naked fallback",
                )
            })?;
        let mut child = result.recv().map_err(|_| {
            ProcessBackendError::new(
                "dedicated direct-launch thread exited before returning a child",
            )
        })??;
        let pid = child.id();
        let process_group = match i32::try_from(pid) {
            Ok(process_group) => process_group,
            Err(_) => {
                terminate_untracked_child(&mut child, None);
                return Err(ProcessBackendError::new(
                    "child PID does not fit Linux pid_t",
                ));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_untracked_child(&mut child, Some(process_group));
                return Err(ProcessBackendError::new(
                    "child stdout pipe was not created",
                ));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_untracked_child(&mut child, Some(process_group));
                return Err(ProcessBackendError::new(
                    "child stderr pipe was not created",
                ));
            }
        };
        let birth_marker = match process_birth_marker(pid) {
            Ok(marker) => marker,
            Err(error) => {
                terminate_untracked_child(&mut child, Some(process_group));
                return Err(ProcessBackendError::new(format!(
                    "failed to identify child process: {error}"
                )));
            }
        };
        let (sentinel_reader, sentinel_writer) = UnixStream::pair().map_err(|error| {
            terminate_untracked_child(&mut child, Some(process_group));
            ProcessBackendError::new(format!(
                "failed to create containment liveness channel: {error}"
            ))
        })?;
        let key = (definition.id.clone(), run_id.clone());
        let record = Arc::new(ProcessRecord {
            process_group,
            _sentinel_liveness: sentinel_writer,
            containment_failure: Mutex::new(None),
            exit: Mutex::new(None),
            exit_changed: Condvar::new(),
        });
        self.shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .insert(key.clone(), record.clone());

        spawn_reaper(
            child,
            self.shared.clone(),
            key,
            record.clone(),
            self.config.descendant_cleanup_timeout,
        )?;

        if let Err(error) = spawn_output_drain(
            stdout,
            self.shared.event_sender.clone(),
            definition.id.clone(),
            run_id.clone(),
            LogStream::Stdout,
        ) {
            self.terminate_failed_start(process_group, &record);
            return Err(error);
        }
        if let Err(error) = spawn_output_drain(
            stderr,
            self.shared.event_sender.clone(),
            definition.id.clone(),
            run_id.clone(),
            LogStream::Stderr,
        ) {
            self.terminate_failed_start(process_group, &record);
            return Err(error);
        }

        let sentinel = match self.launch_sentinel(process_group, sentinel_reader) {
            Ok(sentinel) => sentinel,
            Err(error) => {
                self.terminate_failed_start(process_group, &record);
                return Err(error);
            }
        };
        if let Err(error) = self.start_sentinel_monitor(
            sentinel,
            definition.id.clone(),
            run_id.clone(),
            record.clone(),
        ) {
            self.terminate_failed_start(process_group, &record);
            return Err(error);
        }

        if let Err(error) = self.wait_until_ready(definition, &record) {
            self.terminate_failed_start(process_group, &record);
            return Err(error);
        }
        if let Err(error) =
            self.start_health_monitor(definition, definition.id.clone(), run_id.clone(), record)
        {
            let tracked = self
                .shared
                .processes
                .lock()
                .expect("process map lock poisoned")
                .get(&(definition.id.clone(), run_id.clone()))
                .cloned();
            if let Some(record) = tracked {
                self.terminate_failed_start(process_group, &record);
            }
            return Err(error);
        }
        Ok(ProcessIdentity { pid, birth_marker })
    }

    fn stop(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        shutdown_policy: &ShutdownPolicy,
    ) -> Result<StopOutcome, ProcessBackendError> {
        let record = self
            .shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .get(&(service_id.clone(), run_id.clone()))
            .cloned();
        let Some(record) = record else {
            return Ok(StopOutcome::Graceful);
        };
        signal_process_group(record.process_group, libc::SIGTERM).map_err(|error| {
            ProcessBackendError::new(format!("failed to signal service process group: {error}"))
        })?;
        if let Some(exit) = wait_for_exit(
            &record,
            Duration::from_millis(shutdown_policy.graceful_timeout_ms),
        ) {
            if exit.process_group_clean {
                return Ok(if exit.forced_descendant_cleanup {
                    StopOutcome::Forced
                } else {
                    StopOutcome::Graceful
                });
            }
        }
        signal_process_group(record.process_group, libc::SIGKILL).map_err(|error| {
            ProcessBackendError::new(format!("failed to kill service process group: {error}"))
        })?;
        if let Some(exit) = wait_for_clean_exit(
            &record,
            Duration::from_millis(shutdown_policy.force_kill_timeout_ms),
        ) {
            debug_assert!(exit.process_group_clean);
            Ok(StopOutcome::Forced)
        } else {
            Ok(StopOutcome::TimedOut)
        }
    }
}

impl Drop for LinuxProcessBackend {
    fn drop(&mut self) {
        let groups = self.active_process_groups();
        for process_group in groups {
            let _ = signal_process_group(process_group, libc::SIGKILL);
        }
    }
}

fn spawn_output_drain(
    mut reader: impl Read + Send + 'static,
    sender: SyncSender<BackendEvent>,
    service_id: ServiceId,
    run_id: RunId,
    stream: LogStream,
) -> Result<(), ProcessBackendError> {
    thread::Builder::new()
        .name("prh-output".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 8 * 1024];
            let mut dropped_bytes = 0_u64;
            let mut dropped_chunks = 0_u64;
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        if dropped_chunks > 0 {
                            let _ = sender.send(dropped_output_event(
                                &service_id,
                                &run_id,
                                dropped_chunks,
                                dropped_bytes,
                            ));
                        }
                        break;
                    }
                    Ok(length) => {
                        if dropped_chunks > 0 {
                            match sender.try_send(dropped_output_event(
                                &service_id,
                                &run_id,
                                dropped_chunks,
                                dropped_bytes,
                            )) {
                                Ok(()) => {
                                    dropped_chunks = 0;
                                    dropped_bytes = 0;
                                }
                                Err(TrySendError::Full(_)) => {}
                                Err(TrySendError::Disconnected(_)) => break,
                            }
                        }
                        let event = BackendEvent::Output {
                            service_id: service_id.clone(),
                            run_id: run_id.clone(),
                            stream,
                            bytes: buffer[..length].to_vec(),
                        };
                        match sender.try_send(event) {
                            Ok(()) => {}
                            Err(TrySendError::Full(BackendEvent::Output { bytes, .. })) => {
                                dropped_chunks = dropped_chunks.saturating_add(1);
                                dropped_bytes = dropped_bytes.saturating_add(bytes.len() as u64);
                            }
                            Err(TrySendError::Full(_)) => unreachable!("sent output event"),
                            Err(TrySendError::Disconnected(_)) => {
                                break;
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        if sender
                            .send(BackendEvent::LogFailure {
                                service_id,
                                run_id,
                                message: error.to_string(),
                            })
                            .is_err()
                        {
                            break;
                        }
                        break;
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|error| {
            ProcessBackendError::new(format!("failed to start process-output drain: {error}"))
        })
}

fn read_sentinel_readiness(
    mut stdout: ChildStdout,
    timeout: Duration,
) -> Result<String, ProcessBackendError> {
    let descriptor = stdout.as_raw_fd();
    // SAFETY: fcntl operates on the owned sentinel stdout descriptor and does
    // not dereference memory.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags == -1
        || unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
    {
        return Err(ProcessBackendError::new(format!(
            "failed to configure containment sentinel readiness pipe: {}",
            std::io::Error::last_os_error()
        )));
    }
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::with_capacity(64);
    let mut chunk = [0_u8; 64];
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) => {
                return Err(ProcessBackendError::new(
                    "containment sentinel closed before readiness",
                ));
            }
            Ok(length) => {
                bytes.extend_from_slice(&chunk[..length]);
                if bytes.len() > 128 {
                    return Err(ProcessBackendError::new(
                        "containment sentinel readiness exceeded 128 bytes",
                    ));
                }
                if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
                    bytes.truncate(newline);
                    return String::from_utf8(bytes).map_err(|error| {
                        ProcessBackendError::new(format!(
                            "containment sentinel readiness was not UTF-8: {error}"
                        ))
                    });
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(ProcessBackendError::new(format!(
                    "containment sentinel readiness failed: {error}"
                )));
            }
        }
        if Instant::now() >= deadline {
            return Err(ProcessBackendError::new(
                "containment sentinel readiness timed out",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn dropped_output_event(
    service_id: &ServiceId,
    run_id: &RunId,
    chunks: u64,
    bytes: u64,
) -> BackendEvent {
    BackendEvent::LogFailure {
        service_id: service_id.clone(),
        run_id: run_id.clone(),
        message: format!(
            "bounded process-output queue dropped {chunks} chunks ({bytes} bytes); service draining continued"
        ),
    }
}

fn spawn_direct_launcher() -> Result<SyncSender<LaunchRequest>, ProcessBackendError> {
    let (sender, receiver) = mpsc::sync_channel::<LaunchRequest>(1);
    thread::Builder::new()
        .name("prh-direct-launch".to_owned())
        .spawn(move || {
            while let Ok(request) = receiver.recv() {
                let result = direct_launch(&request.definition);
                if let Err(undelivered) = request.reply.send(result) {
                    if let Ok(mut child) = undelivered.0 {
                        let process_group = i32::try_from(child.id()).ok();
                        terminate_untracked_child(&mut child, process_group);
                    }
                }
            }
        })
        .map_err(|error| {
            ProcessBackendError::new(format!(
                "failed to start dedicated direct-launch thread: {error}"
            ))
        })?;
    Ok(sender)
}

fn direct_launch(definition: &ServiceDefinition) -> Result<Child, ProcessBackendError> {
    let parent_pid = std::process::id() as libc::pid_t;
    let mut command = Command::new(&definition.executable);
    command
        .args(&definition.arguments)
        .current_dir(&definition.working_directory)
        .env_clear()
        .envs(controlled_base_environment())
        .envs(&definition.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // SAFETY: this closure uses only async-signal-safe libc calls and
    // constructs an io::Error from the captured OS error. No allocation or
    // lock is performed in the successful child path. The spawning thread is
    // dedicated and stays alive for the backend lifetime because Linux binds
    // PR_SET_PDEATHSIG to the parent thread rather than its thread group.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent_pid {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "PRH parent changed before exec",
                ));
            }
            Ok(())
        });
    }
    command.spawn().map_err(|error| {
        ProcessBackendError::new(format!(
            "failed to directly launch {}: {error}",
            definition.executable
        ))
    })
}

fn controlled_base_environment() -> Vec<(String, std::ffi::OsString)> {
    [
        "PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name.to_owned(), value)))
    .collect()
}

fn spawn_reaper(
    mut child: Child,
    shared: Arc<Shared>,
    key: (ServiceId, RunId),
    record: Arc<ProcessRecord>,
    cleanup_timeout: Duration,
) -> Result<(), ProcessBackendError> {
    let child_pid = child.id();
    let cleanup_shared = shared.clone();
    let cleanup_key = key.clone();
    let cleanup_record = record.clone();
    let spawned = thread::Builder::new()
        .name("prh-reaper".to_owned())
        .spawn(move || {
            let exit_code = child.wait().ok().and_then(|status| status.code());
            // The direct child may leave descendants. Sweep the occurrence's
            // process group before publishing its exit to the state machine.
            let forced_descendant_cleanup = process_group_exists(record.process_group);
            let deadline = Instant::now() + cleanup_timeout;
            while process_group_exists(record.process_group) && Instant::now() < deadline {
                let _ = signal_process_group(record.process_group, libc::SIGKILL);
                thread::sleep(Duration::from_millis(10));
            }
            let process_group_clean = !process_group_exists(record.process_group);
            {
                let mut exit = record.exit.lock().expect("process exit lock poisoned");
                *exit = Some(ExitRecord {
                    process_group_clean,
                    forced_descendant_cleanup,
                });
                record.exit_changed.notify_all();
            }
            if !process_group_clean {
                let _ = shared.event_sender.send(BackendEvent::ContainmentFailure {
                    service_id: key.0.clone(),
                    run_id: key.1.clone(),
                    message: format!(
                        "contained process group {} survived the bounded cleanup deadline",
                        record.process_group
                    ),
                });
                while process_group_exists(record.process_group) {
                    let _ = signal_process_group(record.process_group, libc::SIGKILL);
                    thread::sleep(Duration::from_millis(100));
                }
                let mut exit = record.exit.lock().expect("process exit lock poisoned");
                *exit = Some(ExitRecord {
                    process_group_clean: true,
                    forced_descendant_cleanup,
                });
                record.exit_changed.notify_all();
            }
            shared
                .processes
                .lock()
                .expect("process map lock poisoned")
                .remove(&key);
            let _ = shared.event_sender.send(BackendEvent::Exit {
                service_id: key.0,
                run_id: key.1,
                exit_code,
            });
        });
    match spawned {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = signal_process_group(cleanup_record.process_group, libc::SIGKILL);
            terminate_exact_child(child_pid);
            let deadline = Instant::now() + cleanup_timeout;
            while process_group_exists(cleanup_record.process_group) && Instant::now() < deadline {
                let _ = signal_process_group(cleanup_record.process_group, libc::SIGKILL);
                thread::sleep(Duration::from_millis(10));
            }
            let clean = !process_group_exists(cleanup_record.process_group);
            *cleanup_record
                .exit
                .lock()
                .expect("process exit lock poisoned") = Some(ExitRecord {
                process_group_clean: clean,
                forced_descendant_cleanup: true,
            });
            cleanup_record.exit_changed.notify_all();
            if clean {
                cleanup_shared
                    .processes
                    .lock()
                    .expect("process map lock poisoned")
                    .remove(&cleanup_key);
            }
            Err(ProcessBackendError::new(format!(
                "failed to start required child reaper: {error}; occurrence was terminated fail-closed"
            )))
        }
    }
}

fn wait_for_exit(record: &ProcessRecord, timeout: Duration) -> Option<ExitRecord> {
    let exit = record.exit.lock().expect("process exit lock poisoned");
    if exit.is_some() {
        return *exit;
    }
    let (exit, _) = record
        .exit_changed
        .wait_timeout_while(exit, timeout, |exit| exit.is_none())
        .expect("process exit lock poisoned while waiting");
    *exit
}

fn wait_for_clean_exit(record: &ProcessRecord, timeout: Duration) -> Option<ExitRecord> {
    let exit = record.exit.lock().expect("process exit lock poisoned");
    let (exit, _) = record
        .exit_changed
        .wait_timeout_while(exit, timeout, |exit| {
            exit.is_none_or(|exit| !exit.process_group_clean)
        })
        .expect("process exit lock poisoned while waiting for group cleanup");
    exit.filter(|exit| exit.process_group_clean)
}

fn terminate_untracked_child(child: &mut Child, process_group: Option<i32>) {
    if let Some(process_group) = process_group {
        let _ = signal_process_group(process_group, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_exact_child(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    if pid <= 1 {
        return;
    }
    // SAFETY: the PID belongs to an exact child just created by this process.
    let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
    loop {
        // SAFETY: waitpid targets only that exact child and writes to a local
        // status integer.
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result >= 0 {
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
}

fn probe_once(probe: &ReadinessProbe) -> bool {
    match probe {
        ReadinessProbe::Tcp {
            host,
            port,
            timeout_ms,
            ..
        } => (host.as_str(), *port)
            .to_socket_addrs()
            .ok()
            .into_iter()
            .flatten()
            .any(|address| {
                TcpStream::connect_timeout(&address, Duration::from_millis(*timeout_ms)).is_ok()
            }),
        ReadinessProbe::Http {
            url,
            timeout_ms,
            expected_status_min,
            expected_status_max,
            ..
        } => minreq::get(url)
            .with_timeout(timeout_ms.div_ceil(1_000).max(1))
            .send()
            .is_ok_and(|response| {
                let status = response.status_code;
                status >= i32::from(*expected_status_min)
                    && status <= i32::from(*expected_status_max)
            }),
    }
}

fn process_birth_marker(pid: u32) -> std::io::Result<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The comm field may contain spaces and parentheses. Fields following its
    // final ')' begin at field 3; starttime is field 22, hence index 19 here.
    let close = stat.rfind(')').ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid /proc stat")
    })?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing process start marker",
            )
        })
}

fn signal_process_group(process_group: i32, signal: i32) -> std::io::Result<()> {
    // SAFETY: kill is called with a validated positive process-group ID and a
    // constant signal number. It does not dereference memory.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn process_group_exists(process_group: i32) -> bool {
    // SAFETY: signal zero performs an existence/permission check only.
    let result = unsafe { libc::kill(-process_group, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
