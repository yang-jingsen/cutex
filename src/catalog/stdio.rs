use std::ffi::OsString;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;

use super::client::bounded_text;
use super::client::CatalogEndpoint;
use super::client::CatalogError;
use crate::app_server::protocol::classify_inbound;
use crate::app_server::protocol::error_response_message;
use crate::app_server::protocol::notification_message;
use crate::app_server::protocol::request_message;
use crate::app_server::protocol::InboundMessage;
use crate::app_server::protocol::RpcResponseOutcome;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
const DEFAULT_MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_STDERR_BYTES: usize = 16 * 1024;
const INBOUND_CAPACITY: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioAppServerOptions {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub codex_home: PathBuf,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub max_message_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl StdioAppServerOptions {
    pub fn new(program: impl Into<OsString>, codex_home: PathBuf) -> Self {
        Self {
            program: program.into(),
            args: vec![OsString::from("app-server"), OsString::from("--stdio")],
            codex_home,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
        }
    }
}

#[derive(Debug)]
enum ReaderEvent {
    Message(Value),
    Failure(String),
    Eof,
}

#[derive(Debug, Default)]
struct BoundedStderr {
    bytes: Vec<u8>,
    truncated: bool,
}

impl BoundedStderr {
    fn push(&mut self, chunk: &[u8], limit: usize) {
        let remaining = limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        self.truncated |= chunk.len() > remaining;
    }

    fn display(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim().to_string();
        if self.truncated {
            text.push_str("…[truncated]");
        }
        bounded_text(&text)
    }
}

/// A Cutex-owned stdio app-server process. Requests are deliberately
/// serialized because a catalog endpoint has one consumer for one TUI lifetime.
pub struct OwnedStdioEndpoint {
    child: Child,
    stdin: Option<ChildStdin>,
    inbound: mpsc::Receiver<ReaderEvent>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr: Arc<Mutex<BoundedStderr>>,
    next_id: u64,
    request_timeout: Duration,
    shutdown_timeout: Duration,
    failed: bool,
}

impl OwnedStdioEndpoint {
    pub fn spawn(options: StdioAppServerOptions) -> Result<Self, CatalogError> {
        validate_options(&options)?;
        let mut command = Command::new(&options.program);
        command
            .args(&options.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The child must never inherit another session's selected home.
            .env("CODEX_HOME", &options.codex_home);
        configure_no_window(&mut command);
        let mut child = command.spawn().map_err(|error| {
            CatalogError::Launch(format!(
                "failed to start app-server program {}: {}",
                PathBuf::from(&options.program).display(),
                bounded_text(&error.to_string())
            ))
        })?;
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => return Err(cleanup_incomplete_child(&mut child, "stdin")),
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => return Err(cleanup_incomplete_child(&mut child, "stdout")),
        };
        let stderr_pipe = match child.stderr.take() {
            Some(stderr) => stderr,
            None => return Err(cleanup_incomplete_child(&mut child, "stderr")),
        };

        let (sender, inbound) = mpsc::sync_channel(INBOUND_CAPACITY);
        let max_message_bytes = options.max_message_bytes;
        let stdout_thread = std::thread::spawn(move || {
            read_stdout(stdout, max_message_bytes, sender);
        });
        let stderr = Arc::new(Mutex::new(BoundedStderr::default()));
        let stderr_for_thread = Arc::clone(&stderr);
        let max_stderr_bytes = options.max_stderr_bytes;
        let stderr_thread = std::thread::spawn(move || {
            let mut reader = stderr_pipe;
            let mut buffer = [0_u8; 4_096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if let Ok(mut stderr) = stderr_for_thread.lock() {
                            stderr.push(&buffer[..count], max_stderr_bytes);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            stdin: Some(stdin),
            inbound,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            stderr,
            next_id: 1,
            request_timeout: options.request_timeout,
            shutdown_timeout: options.shutdown_timeout,
            failed: false,
        })
    }

    pub fn child_id(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, message: &Value) -> Result<(), CatalogError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| {
            CatalogError::Transport("app-server child stdin is closed".to_string())
        })?;
        serde_json::to_writer(&mut *stdin, message).map_err(|error| {
            CatalogError::Transport(format!(
                "failed to encode app-server request: {}",
                bounded_text(&error.to_string())
            ))
        })?;
        stdin.write_all(b"\n").map_err(|error| {
            CatalogError::Transport(format!(
                "failed to write app-server request: {}",
                bounded_text(&error.to_string())
            ))
        })?;
        stdin.flush().map_err(|error| {
            CatalogError::Transport(format!(
                "failed to flush app-server request: {}",
                bounded_text(&error.to_string())
            ))
        })
    }

    fn receive_response(&mut self, id: u64, method: &str) -> Result<Value, CatalogError> {
        let started = Instant::now();
        loop {
            let Some(remaining) = self.request_timeout.checked_sub(started.elapsed()) else {
                self.failed = true;
                self.stop();
                return Err(CatalogError::Timeout {
                    method: method.to_string(),
                });
            };
            let event = match self.inbound.recv_timeout(remaining) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.failed = true;
                    self.stop();
                    return Err(CatalogError::Timeout {
                        method: method.to_string(),
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.child_failure("app-server stdout reader stopped"));
                }
            };
            match event {
                ReaderEvent::Message(raw) => match classify_inbound(raw) {
                    Ok(InboundMessage::Response(response)) => {
                        if response.id.as_u64() != Some(id) {
                            self.failed = true;
                            let error = CatalogError::Protocol(format!(
                                "app-server returned response id {} while waiting for {id}",
                                bounded_text(&response.id.to_string())
                            ));
                            self.stop();
                            return Err(error);
                        }
                        return match response.outcome {
                            RpcResponseOutcome::Result(result) => Ok(result),
                            RpcResponseOutcome::Error(error) => Err(CatalogError::Rpc {
                                method: method.to_string(),
                                error,
                            }),
                        };
                    }
                    Ok(InboundMessage::ServerRequest(request)) => {
                        if let Err(error) = self.send(&error_response_message(
                            request.id,
                            -32601,
                            "Cutex catalog transport does not handle server requests",
                            None,
                        )) {
                            self.failed = true;
                            self.stop();
                            return Err(error);
                        }
                    }
                    Ok(InboundMessage::Notification(_)) => {}
                    Err(error) => {
                        self.failed = true;
                        let error = CatalogError::Protocol(format!(
                            "invalid app-server message: {}",
                            bounded_text(&error.to_string())
                        ));
                        self.stop();
                        return Err(error);
                    }
                },
                ReaderEvent::Failure(message) => {
                    self.failed = true;
                    let error = CatalogError::Protocol(message);
                    self.stop();
                    return Err(error);
                }
                ReaderEvent::Eof => return Err(self.child_failure("app-server closed stdout")),
            }
        }
    }

    fn child_failure(&mut self, reason: &str) -> CatalogError {
        self.failed = true;
        self.stop();
        let status = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown status".to_string());
        let stderr = self
            .stderr
            .lock()
            .map(|stderr| stderr.display())
            .unwrap_or_default();
        let suffix = if stderr.is_empty() {
            String::new()
        } else {
            format!("; stderr: {stderr}")
        };
        CatalogError::Transport(format!("{reason} ({status}){suffix}"))
    }

    fn stop(&mut self) {
        self.stdin.take();
        let started = Instant::now();
        let mut exited = false;
        while started.elapsed() < self.shutdown_timeout {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        if !exited {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}

impl CatalogEndpoint for OwnedStdioEndpoint {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, CatalogError> {
        if self.failed {
            return Err(CatalogError::Transport(
                "app-server stdio transport cannot be reused after failure".to_string(),
            ));
        }
        if method.trim().is_empty() {
            return Err(CatalogError::Protocol(
                "app-server method must not be empty".to_string(),
            ));
        }
        if self.next_id > i64::MAX as u64 {
            self.failed = true;
            self.stop();
            return Err(CatalogError::Transport(
                "app-server request id space is exhausted".to_string(),
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            CatalogError::Transport("app-server request id space is exhausted".to_string())
        })?;
        if let Err(error) = self.send(&request_message(id, method, params)) {
            self.failed = true;
            self.stop();
            return Err(error);
        }
        self.receive_response(id, method)
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), CatalogError> {
        if self.failed {
            return Err(CatalogError::Transport(
                "app-server stdio transport cannot be reused after failure".to_string(),
            ));
        }
        if method.trim().is_empty() {
            return Err(CatalogError::Protocol(
                "app-server method must not be empty".to_string(),
            ));
        }
        if let Err(error) = self.send(&notification_message(method, params)) {
            self.failed = true;
            self.stop();
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for OwnedStdioEndpoint {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_options(options: &StdioAppServerOptions) -> Result<(), CatalogError> {
    if options.program.is_empty() {
        return Err(CatalogError::Launch(
            "app-server program must not be empty".to_string(),
        ));
    }
    if !options.codex_home.is_absolute() {
        return Err(CatalogError::Launch(format!(
            "Cutex host Codex home must be absolute: {}",
            options.codex_home.display()
        )));
    }
    if options.request_timeout.is_zero() || options.shutdown_timeout.is_zero() {
        return Err(CatalogError::Launch(
            "app-server request and shutdown timeouts must be positive".to_string(),
        ));
    }
    if options.max_message_bytes == 0 || options.max_stderr_bytes == 0 {
        return Err(CatalogError::Launch(
            "app-server output bounds must be positive".to_string(),
        ));
    }
    Ok(())
}

fn cleanup_incomplete_child(child: &mut Child, stream: &str) -> CatalogError {
    let _ = child.kill();
    let _ = child.wait();
    CatalogError::Launch(format!("app-server child {stream} was not piped"))
}

#[cfg(windows)]
fn configure_no_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_no_window(_command: &mut Command) {}

fn read_stdout(stdout: impl Read, max_message_bytes: usize, sender: mpsc::SyncSender<ReaderEvent>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let line = match read_bounded_line(&mut reader, max_message_bytes) {
            Ok(Some(line)) => line,
            Ok(None) => {
                let _ = sender.try_send(ReaderEvent::Eof);
                break;
            }
            Err(error) => {
                let _ = sender.try_send(ReaderEvent::Failure(format!(
                    "failed to read app-server stdout: {}",
                    bounded_text(&error.to_string())
                )));
                break;
            }
        };
        let line = match line {
            BoundedLine::Data(line) => line,
            BoundedLine::TooLong => {
                let _ = sender.try_send(ReaderEvent::Failure(format!(
                    "app-server message exceeded {max_message_bytes} bytes"
                )));
                break;
            }
        };
        let value: Value = match serde_json::from_slice(&line) {
            Ok(value) => value,
            Err(error) => {
                let _ = sender.try_send(ReaderEvent::Failure(format!(
                    "app-server emitted invalid JSON: {}",
                    bounded_text(&error.to_string())
                )));
                break;
            }
        };
        // Catalog clients do not consume notifications. Dropping them here
        // prevents idle provider activity from creating an unbounded queue.
        let is_notification = value
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|method| !method.is_empty())
            && value.get("id").is_none_or(Value::is_null);
        if is_notification {
            continue;
        }
        if sender.try_send(ReaderEvent::Message(value)).is_err() {
            let _ = sender.try_send(ReaderEvent::Failure(
                "app-server response queue exceeded its bounded capacity".to_string(),
            ));
            break;
        }
    }
}

enum BoundedLine {
    Data(Vec<u8>),
    TooLong,
}

fn read_bounded_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Option<BoundedLine>> {
    let mut output = Vec::new();
    let mut too_long = false;
    let mut observed = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if observed {
                Ok(Some(if too_long {
                    BoundedLine::TooLong
                } else {
                    BoundedLine::Data(output)
                }))
            } else {
                Ok(None)
            };
        }
        observed = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let content_len = newline.unwrap_or(available.len());
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&available[..content_len.min(remaining)]);
        too_long |= content_len > remaining;
        reader.consume(consumed);
        if newline.is_some() {
            if output.last() == Some(&b'\r') {
                output.pop();
            }
            return Ok(Some(if too_long {
                BoundedLine::TooLong
            } else {
                BoundedLine::Data(output)
            }));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::ffi::OsStrExt;

    use serde_json::json;

    use super::*;
    use crate::platform::process::process_is_running;

    fn shell_options(script: &str) -> StdioAppServerOptions {
        let mut options = StdioAppServerOptions::new("/bin/sh", PathBuf::from("/tmp/cutex-home"));
        options.args = vec![OsString::from("-c"), OsString::from(script)];
        options.request_timeout = Duration::from_millis(500);
        options.shutdown_timeout = Duration::from_millis(50);
        options
    }

    #[test]
    fn sets_codex_home_and_reaps_child_on_drop() {
        let script = r#"
read request
printf '{"id":1,"result":{"codexHome":"%s"}}\n' "$CODEX_HOME"
read until_eof
"#;
        let mut endpoint = OwnedStdioEndpoint::spawn(shell_options(script)).expect("spawn child");
        let pid = endpoint.child_id();
        let result = endpoint
            .request("probe", json!({}))
            .expect("probe response");
        assert_eq!(
            result.get("codexHome").and_then(Value::as_str),
            Some("/tmp/cutex-home")
        );
        assert!(process_is_running(pid));
        drop(endpoint);
        assert!(!process_is_running(pid));
    }

    #[test]
    fn reports_bounded_child_stderr_on_early_exit() {
        let script = r#"
read request
printf 'provider-start-failed:' >&2
i=0
while [ "$i" -lt 200 ]; do printf 'x' >&2; i=$((i + 1)); done
exit 9
"#;
        let mut options = shell_options(script);
        options.max_stderr_bytes = 32;
        let mut endpoint = OwnedStdioEndpoint::spawn(options).expect("spawn child");
        let error = endpoint
            .request("probe", json!({}))
            .expect_err("child exits");
        let message = error.to_string();
        assert!(message.contains("provider-start-failed"));
        assert!(message.contains("[truncated]"));
        assert!(message.len() < 256);
    }

    #[test]
    fn timeout_terminates_and_reaps_child_and_poison_transport() {
        let script = "read request\nexec sleep 30\n";
        let mut options = shell_options(script);
        options.request_timeout = Duration::from_millis(30);
        let mut endpoint = OwnedStdioEndpoint::spawn(options).expect("spawn child");
        let pid = endpoint.child_id();
        let error = endpoint
            .request("probe", json!({}))
            .expect_err("request timeout");
        assert!(matches!(error, CatalogError::Timeout { .. }));
        assert!(!process_is_running(pid));
        let error = endpoint
            .request("again", json!({}))
            .expect_err("poisoned transport");
        assert!(error.to_string().contains("cannot be reused"));
    }

    #[test]
    fn bounded_line_reader_rejects_oversized_lines_without_retaining_them() {
        let input = b"123456789\n{}\n";
        let mut reader = BufReader::new(&input[..]);
        assert!(matches!(
            read_bounded_line(&mut reader, 4).expect("read"),
            Some(BoundedLine::TooLong)
        ));
        assert!(matches!(
            read_bounded_line(&mut reader, 4).expect("read"),
            Some(BoundedLine::Data(value)) if value == b"{}"
        ));
    }

    #[test]
    fn options_accept_non_utf8_program_names_without_lossy_execution() {
        let name = std::ffi::OsStr::from_bytes(b"/tmp/non-utf8-\xff");
        let options = StdioAppServerOptions::new(name, PathBuf::from("/tmp/cutex-home"));
        assert_eq!(options.program.as_os_str().as_bytes(), name.as_bytes());
    }
}
