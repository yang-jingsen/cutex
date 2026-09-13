use crate::fake::RestartDirective;
use crate::file_logs::RotatingFileLogs;
use crate::model::{HostPhase, ServiceId};
use crate::process_backend::BackendEvent;
#[cfg(any(unix, target_os = "windows"))]
use crate::protocol::{
    host_capabilities, ApiError, ErrorCode, EventEnvelope, ProtocolVersion, ReadLogsParams,
    Request, RequestEnvelope, Response, ResponseEnvelope, ResponseOutcome, ShutdownHostParams,
};
#[cfg(any(unix, target_os = "windows"))]
use crate::transport::{read_json_frame, write_json_frame};
use crate::HostController;
#[cfg(any(unix, target_os = "windows"))]
use crate::MutationOptions;
use std::collections::BTreeMap;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(any(unix, target_os = "windows"))]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use crate::windows_pipe::{WindowsPipeListener, WindowsPipeStream};
#[cfg(any(unix, target_os = "windows"))]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

pub fn seed_file_log_sequences(
    host: &HostController,
    file_logs: &RotatingFileLogs,
) -> std::io::Result<()> {
    for service_id in host.service_ids() {
        host.seed_log_sequence(&service_id, file_logs.last_sequence(&service_id)?);
    }
    Ok(())
}

pub fn spawn_backend_event_pump(
    host: Arc<HostController>,
    receiver: Receiver<BackendEvent>,
    file_logs: Arc<RotatingFileLogs>,
) -> JoinHandle<()> {
    let scheduler_host = host.clone();
    let (restart_sender, restart_receiver) = mpsc::sync_channel(1_024);
    let scheduler = thread::spawn(move || {
        run_restart_scheduler(scheduler_host, restart_receiver);
    });
    thread::spawn(move || {
        loop {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(BackendEvent::Output {
                    service_id,
                    run_id,
                    stream,
                    bytes,
                }) => match host.emit_log(&service_id, &run_id, stream, &bytes) {
                    Ok(entry) => {
                        if let Err(error) = file_logs.append(&entry) {
                            let _ = host.report_log_failure(
                                &service_id,
                                &run_id,
                                format!("rotating file log write failed: {error}"),
                            );
                        }
                    }
                    Err(error) => eprintln!(
                        "prh-host: discarded output for {service_id}/{run_id}: {}",
                        error.message
                    ),
                },
                Ok(BackendEvent::LogFailure {
                    service_id,
                    run_id,
                    message,
                }) => {
                    let _ = host.report_log_failure(&service_id, &run_id, message);
                }
                Ok(BackendEvent::ContainmentFailure {
                    service_id,
                    run_id,
                    message,
                }) => {
                    let _ = host.report_containment_failure(&service_id, &run_id, message);
                }
                Ok(BackendEvent::Health {
                    service_id,
                    run_id,
                    health,
                }) => {
                    let _ = host.set_health(&service_id, &run_id, health);
                }
                Ok(BackendEvent::Exit {
                    service_id,
                    run_id,
                    exit_code,
                }) => {
                    match host.observe_backend_exit(&service_id, &run_id, exit_code) {
                        Ok(observation) => {
                            if let Some(restart) = observation.restart {
                                if restart_sender.send(restart).is_err() {
                                    eprintln!(
                                        "prh-host: restart scheduler stopped before {service_id}/{run_id} could be queued"
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!(
                                "prh-host: exit reconciliation failed for {service_id}/{run_id}: {}",
                                error.message
                            );
                        }
                    }
                    host.complete_shutdown_if_quiescent();
                }
                Err(RecvTimeoutError::Timeout) if host.host_phase() == HostPhase::Stopped => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(restart_sender);
        let _ = scheduler.join();
    })
}

fn run_restart_scheduler(host: Arc<HostController>, receiver: Receiver<RestartDirective>) {
    let mut pending: BTreeMap<ServiceId, (Instant, RestartDirective)> = BTreeMap::new();
    let mut disconnected = false;
    loop {
        if host.host_phase() == HostPhase::Stopped {
            return;
        }
        if !disconnected {
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(directive) => {
                    let due = Instant::now() + Duration::from_millis(directive.backoff_ms);
                    pending.insert(directive.service_id.clone(), (due, directive));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => disconnected = true,
            }
            loop {
                match receiver.try_recv() {
                    Ok(directive) => {
                        let due = Instant::now() + Duration::from_millis(directive.backoff_ms);
                        pending.insert(directive.service_id.clone(), (due, directive));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let now = Instant::now();
        let due = pending
            .iter()
            .filter(|(_, (deadline, _))| *deadline <= now)
            .map(|(service_id, _)| service_id.clone())
            .collect::<Vec<_>>();
        for service_id in due {
            let (_, directive) = pending
                .remove(&service_id)
                .expect("due restart remained pending");
            if let Err(error) = host.execute_restart_directive(&directive) {
                eprintln!(
                    "prh-host: scheduled restart failed for {}/{}: {}",
                    directive.service_id, directive.failed_run_id, error.message
                );
            }
        }
        if disconnected && pending.is_empty() {
            return;
        }
    }
}

#[cfg(unix)]
pub fn run_local_server(
    socket_path: &Path,
    host: Arc<HostController>,
    file_logs: Arc<RotatingFileLogs>,
    terminate: Arc<AtomicBool>,
) -> std::io::Result<()> {
    validate_socket_path(socket_path)?;
    prepare_socket_path(socket_path)?;
    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    let _socket_guard = SocketGuard(socket_path.to_owned());
    listener.set_nonblocking(true)?;
    let mut workers = Vec::new();

    loop {
        if terminate.load(Ordering::Relaxed) && host.host_phase() == HostPhase::Running {
            request_shutdown(&host, "signal");
        }
        if host.host_phase() == HostPhase::Stopped {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let host = host.clone();
                let file_logs = file_logs.clone();
                workers.push(thread::spawn(move || {
                    if let Err(error) = handle_connection(stream, host, file_logs) {
                        eprintln!("prh-host: local client connection failed: {error}");
                    }
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error),
        }
        reap_finished_workers(&mut workers);
    }

    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn run_local_server(
    pipe_path: &Path,
    host: Arc<HostController>,
    file_logs: Arc<RotatingFileLogs>,
    terminate: Arc<AtomicBool>,
) -> std::io::Result<()> {
    let listener = WindowsPipeListener::bind(pipe_path)?;
    let mut workers = Vec::new();
    loop {
        if terminate.load(Ordering::Relaxed) && host.host_phase() == HostPhase::Running {
            request_shutdown(&host, "signal");
        }
        if host.host_phase() == HostPhase::Stopped {
            break;
        }
        let accepted = listener.accept_while(|| {
            if terminate.load(Ordering::Relaxed) && host.host_phase() == HostPhase::Running {
                request_shutdown(&host, "signal");
            }
            host.host_phase() != HostPhase::Stopped
        })?;
        let Some(stream) = accepted else {
            break;
        };
        let worker_host = host.clone();
        let worker_logs = file_logs.clone();
        workers.push(thread::spawn(move || {
            if let Err(error) = handle_connection(stream, worker_host, worker_logs) {
                eprintln!("prh-host: local client connection failed: {error}");
            }
        }));
        reap_finished_workers(&mut workers);
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
pub fn run_local_server(
    _socket_path: &Path,
    _host: Arc<HostController>,
    _file_logs: Arc<RotatingFileLogs>,
    _terminate: Arc<AtomicBool>,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the PRH local server is available only on Linux and Windows",
    ))
}

#[cfg(any(unix, target_os = "windows"))]
fn handle_connection<S: LocalConnection>(
    mut stream: S,
    host: Arc<HostController>,
    file_logs: Arc<RotatingFileLogs>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = std::io::BufReader::new(stream.try_clone_connection()?);
    let request: RequestEnvelope = match read_json_frame(&mut reader) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            write_json_frame(&mut stream, &invalid_request_response(error.to_string()))?;
            return Ok(());
        }
    };
    let read_logs = match &request.request {
        Request::ReadLogs(params) => Some(params.clone()),
        _ => None,
    };
    let registered_service = match &request.request {
        Request::RegisterService(params) => Some(params.definition.id.clone()),
        _ => None,
    };
    let mut response = host.handle(request);
    if let Some(service_id) = registered_service {
        if matches!(&response.outcome, ResponseOutcome::Ok { .. }) {
            host.seed_log_sequence(&service_id, file_logs.last_sequence(&service_id)?);
        }
    }
    if let Some(params) = read_logs {
        replace_with_file_logs(&mut response, &file_logs, &params);
    }
    let subscription_id = match &response.outcome {
        ResponseOutcome::Ok { response } => match response.as_ref() {
            Response::SubscribeEvents(result) => Some(result.subscription_id.clone()),
            _ => None,
        },
        ResponseOutcome::Error { .. } => None,
    };
    if let Err(error) = write_json_frame(&mut stream, &response) {
        if let Some(subscription_id) = &subscription_id {
            let _ = host.take_event_stream(subscription_id);
        }
        return Err(error);
    }

    if let Some(subscription_id) = subscription_id {
        let event_stream = host.take_event_stream(&subscription_id).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "accepted event stream was not available",
            )
        })?;
        loop {
            match event_stream.recv_timeout(Duration::from_millis(250)) {
                Ok(event) => write_json_frame(
                    &mut stream,
                    &EventEnvelope {
                        protocol: ProtocolVersion::V1,
                        subscription_id: subscription_id.clone(),
                        event,
                    },
                )?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    if host.host_phase() == HostPhase::Stopped =>
                {
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    Ok(())
}

#[cfg(any(unix, target_os = "windows"))]
fn replace_with_file_logs(
    envelope: &mut ResponseEnvelope,
    file_logs: &RotatingFileLogs,
    params: &ReadLogsParams,
) {
    let result = file_logs.read(
        &params.service_id,
        params.run_id.as_ref(),
        params.after_sequence,
        params.limit,
    );
    match result {
        Ok(entries) => {
            if let ResponseOutcome::Ok { response } = &mut envelope.outcome {
                if let Response::ReadLogs(page) = response.as_mut() {
                    page.next_sequence = entries
                        .last()
                        .map(|entry| entry.sequence)
                        .or(params.after_sequence);
                    page.entries = entries;
                }
            }
        }
        Err(error) => {
            envelope.outcome = ResponseOutcome::Error {
                error: ApiError::new(
                    ErrorCode::LogUnavailable,
                    format!("failed to read rotating logs: {error}"),
                )
                .retryable(),
            };
        }
    }
}

#[cfg(any(unix, target_os = "windows"))]
fn request_shutdown(host: &HostController, source: &str) {
    let _ = host.handle(RequestEnvelope::v1(
        format!("host-shutdown-{source}"),
        Request::ShutdownHost(ShutdownHostParams {
            mutation: MutationOptions::new(format!("host-shutdown-{source}")),
        }),
    ));
}

#[cfg(any(unix, target_os = "windows"))]
fn invalid_request_response(message: String) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol: ProtocolVersion::V1,
        capabilities: host_capabilities(),
        request_id: "unparseable-request".to_owned(),
        outcome: ResponseOutcome::Error {
            error: ApiError::new(ErrorCode::InvalidRequest, message),
        },
    }
}

#[cfg(unix)]
fn validate_socket_path(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    if path.as_os_str().as_bytes().len() > 100 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unix socket path exceeds the conservative 100-byte limit",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_socket_path(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("refusing to replace non-socket path {}", path.display()),
        ));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("a local host is already listening at {}", path.display()),
        ));
    }
    std::fs::remove_file(path)
}

#[cfg(any(unix, target_os = "windows"))]
fn reap_finished_workers(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            let _ = worker.join();
        } else {
            index += 1;
        }
    }
}

#[cfg(unix)]
struct SocketGuard(PathBuf);

#[cfg(unix)]
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(any(unix, target_os = "windows"))]
trait LocalConnection: Read + Write + Send + Sized + 'static {
    fn try_clone_connection(&self) -> std::io::Result<Self>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

#[cfg(unix)]
impl LocalConnection for UnixStream {
    fn try_clone_connection(&self) -> std::io::Result<Self> {
        self.try_clone()
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        UnixStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        UnixStream::set_write_timeout(self, timeout)
    }
}

#[cfg(target_os = "windows")]
impl LocalConnection for WindowsPipeStream {
    fn try_clone_connection(&self) -> std::io::Result<Self> {
        self.try_clone()
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        WindowsPipeStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        WindowsPipeStream::set_write_timeout(self, timeout)
    }
}
