use cutex_job_service::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const API: &[u8] = b"api-token-for-js3b-tests-32-bytes-minimum";
const GRANT: &[u8] = b"grant-key-for-js3b-tests-32-bytes-minimum";
const COMPLETION: &[u8] = b"completion-token-for-js3b-tests-minimum";

fn runner() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_cutex-job-service"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn request(cwd: &std::path::Path, action: &str) -> JobRequest {
    JobRequest {
        action_id: action.into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "printf complete".into()],
        cwd: cwd.display().to_string(),
        environment: BTreeMap::new(),
        subscriber_cutex_session_id: "cutex.11111111-1111-4111-8111-111111111111".into(),
        origin: ExecutionOrigin {
            runtime_agent_id: "runtime-js3b".into(),
            native_thread_id: "thread-js3b".into(),
            permission_profile_type: "disabled".into(),
        },
    }
}

fn grant(request: &JobRequest, launcher: &str) -> ExecutionGrant {
    GrantIssuer::new(GRANT)
        .unwrap()
        .issue(
            request,
            TrustedSandboxContext {
                subject_cutex_session_id: request.subscriber_cutex_session_id.clone(),
                cwd: request.cwd.clone(),
                sandbox_state: json!({
                    "permissionProfile":{"type":"disabled"},
                    "codexLinuxSandboxExe":null,
                    "sandboxCwd":format!("file://{}", request.cwd),
                    "useLegacyLandlock":false
                }),
                launcher_path: launcher.into(),
                launcher_sha256: file_sha256(std::path::Path::new(launcher)).unwrap(),
                operating_system_uid: unsafe { libc::geteuid() },
            },
            now(),
            60,
        )
        .unwrap()
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn core_call(socket: &std::path::Path, method: &str, params: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    serde_json::to_writer(
        &mut stream,
        &json!({"token":hex::encode(API),"method":method,"params":params}),
    )
    .unwrap();
    stream.write_all(b"\n").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

fn await_job_state(state_root: &std::path::Path, job_id: &str, wanted: &str) -> Value {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(bytes) = std::fs::read(state_root.join("state.json"))
            && let Ok(state) = serde_json::from_slice::<Value>(&bytes)
        {
            let job = &state["jobs"][job_id];
            if job["completionDelivery"]["state"] == wanted {
                return state;
            }
        }
        assert!(Instant::now() < until, "job did not reach {wanted}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn spawn_daemon(
    state: &std::path::Path,
    socket: &std::path::Path,
    api: &std::path::Path,
    grant_key: &std::path::Path,
    endpoint: &str,
    token: &std::path::Path,
    launcher: &str,
) -> Child {
    let child = Command::new(runner())
        .args(["serve", state.to_str().unwrap(), socket.to_str().unwrap()])
        .args([api, grant_key])
        .arg(launcher)
        .args(["--completion", endpoint, token.to_str().unwrap()])
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(120);
    while !socket.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(socket.exists());
    child
}

struct Fixture {
    endpoint: String,
    stop: Arc<AtomicBool>,
    submits: Arc<Mutex<Vec<Vec<u8>>>>,
    requests: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn lost_then_pending_then_delivered() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let submits = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(AtomicUsize::new(0));
        let thread_stop = Arc::clone(&stop);
        let thread_submits = Arc::clone(&submits);
        let thread_requests = Arc::clone(&requests);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (path, body, token) = read_http(&mut stream);
                        assert_eq!(token, String::from_utf8_lossy(COMPLETION));
                        let number = thread_requests.fetch_add(1, Ordering::AcqRel);
                        if path == "/api/job-service/v1/completions" {
                            thread_submits.lock().unwrap().push(body.clone());
                        }
                        if number == 0 {
                            continue;
                        }
                        let request: Value = serde_json::from_slice(&body).unwrap();
                        let event = request["eventId"].as_str().unwrap().to_string();
                        let schema = request["schema"].as_str().unwrap();
                        let receipt = if number == 1 {
                            receipt_with_schema(schema, &event, "pending", None)
                        } else {
                            receipt_with_schema(
                                schema,
                                &event,
                                "delivered",
                                Some(json!({"receiptId":"a4-test"})),
                            )
                        };
                        write_http(&mut stream, 200, &receipt);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self {
            endpoint,
            stop,
            submits,
            requests,
            thread: Some(thread),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn read_http(stream: &mut TcpStream) -> (String, Vec<u8>, String) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let path = request_line.split_whitespace().nth(1).unwrap().to_string();
    let mut length = 0usize;
    let mut token = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
        if let Some(value) = line.strip_prefix("Authorization: Bearer ") {
            token = value.trim().to_string();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    (path, body, token)
}

fn receipt(event: &str, disposition: &str, a4: Option<Value>) -> Value {
    receipt_with_schema(COMPLETION_CONTRACT, event, disposition, a4)
}

fn receipt_with_schema(schema: &str, event: &str, disposition: &str, a4: Option<Value>) -> Value {
    use sha2::{Digest, Sha256};
    json!({
        "schema":schema,
        "status":"committed",
        "eventId":event,
        "messageId":format!("jsc_{:x}", Sha256::digest(event.as_bytes())),
        "disposition":disposition,
        "deduplicated":false,
        "a4Receipt":a4
    })
}

#[test]
fn v2_outbox_freezes_facts_and_retries_identical_bytes_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let state_root = temp.path().join("state");
    let launcher_file = temp.path().join("launcher");
    std::fs::write(
        &launcher_file,
        b"#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher_file, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launcher_file = std::fs::canonicalize(launcher_file).unwrap();
    let token_file = temp.path().join("completion.key");
    write_private(&token_file, COMPLETION);
    let make_config = |completion_wire_version| ServiceConfig {
        completion_enabled: true,
        state_root: state_root.clone(),
        grant_key: GRANT.into(),
        api_token: API.into(),
        runner_executable: runner(),
        per_stream_output_bytes: 4096,
        max_read_bytes: 1024,
        cancel_grace: Duration::from_millis(100),
        max_jobs: 8,
        max_active_jobs: 2,
        allowed_launchers: [(
            launcher_file.display().to_string(),
            file_sha256(&launcher_file).unwrap(),
        )]
        .into_iter()
        .collect(),
        completion_wire_version,
    };
    let fixture = Fixture::lost_then_pending_then_delivered();
    let service = JobService::open(make_config(CompletionWireVersion::V2)).unwrap();
    let request = request(cwd.path(), "v2-frozen-restart");
    let submitted = service
        .submit(
            API,
            request.clone(),
            grant(&request, launcher_file.to_str().unwrap()),
        )
        .unwrap();
    while !service
        .query(API, &submitted.job.job_id)
        .unwrap()
        .state
        .terminal()
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let worker = CompletionDeliveryWorker::start(
        service.clone(),
        CompletionDeliveryConfig {
            endpoint: fixture.endpoint.clone(),
            token_file: token_file.clone(),
            request_timeout: Duration::from_millis(100),
            minimum_backoff: Duration::from_millis(20),
            maximum_backoff: Duration::from_millis(100),
            idle_interval: Duration::from_millis(10),
        },
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while service
        .query(API, &submitted.job.job_id)
        .unwrap()
        .completion_delivery
        .state
        != CompletionDeliveryState::AcceptedPending
    {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.shutdown();
    drop(service);

    let restarted = JobService::open(make_config(CompletionWireVersion::V1)).unwrap();
    let worker = CompletionDeliveryWorker::start(
        restarted.clone(),
        CompletionDeliveryConfig {
            endpoint: fixture.endpoint.clone(),
            token_file,
            request_timeout: Duration::from_millis(100),
            minimum_backoff: Duration::from_millis(20),
            maximum_backoff: Duration::from_millis(100),
            idle_interval: Duration::from_millis(10),
        },
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while restarted
        .query(API, &submitted.job.job_id)
        .unwrap()
        .completion_delivery
        .state
        != CompletionDeliveryState::Delivered
    {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.shutdown();
    let submits = fixture.submits.lock().unwrap();
    assert_eq!(submits.len(), 2);
    assert_eq!(submits[0], submits[1], "v2 lost-reply retry bytes changed");
    let body: Value = serde_json::from_slice(&submits[0]).unwrap();
    assert_eq!(body["schema"], COMPLETION_CONTRACT_V2);
    assert_eq!(body["facts"]["actionId"], "v2-frozen-restart");
    assert!(body["facts"]["execution"]["observedRunDurationMillis"].is_u64());
    assert!(body.get("summary").is_none());
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(state_root.join("state.json")).unwrap()).unwrap();
    assert_eq!(persisted["version"], 2);
    assert_eq!(
        persisted["outbox"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()["frozenRequestV2"],
        body
    );
}

fn write_http(stream: &mut TcpStream, status: u16, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .unwrap();
    stream.write_all(&bytes).unwrap();
}

#[test]
fn standalone_daemon_retries_lost_submit_across_restart_then_observes_a4() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let socket = temp.path().join("run/job.sock");
    let api_file = temp.path().join("api.key");
    let grant_file = temp.path().join("grant.key");
    let completion_file = temp.path().join("completion.key");
    let launcher_file = temp.path().join("test-sandbox-launcher");
    write_private(&api_file, API);
    write_private(&grant_file, GRANT);
    write_private(&completion_file, COMPLETION);
    std::fs::write(
        &launcher_file,
        b"#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher_file, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launcher_file = std::fs::canonicalize(launcher_file).unwrap();
    let fixture = Fixture::lost_then_pending_then_delivered();
    let mut daemon = spawn_daemon(
        &state,
        &socket,
        &api_file,
        &grant_file,
        &fixture.endpoint,
        &completion_file,
        launcher_file.to_str().unwrap(),
    );
    let request = request(cwd.path(), "js3b-daemon");
    let result = core_call(
        &socket,
        "submit",
        json!({"request":request,"grant":grant(&request, launcher_file.to_str().unwrap())}),
    );
    let job_id = result["job"]["jobId"].as_str().unwrap().to_string();
    await_job_state(&state, &job_id, "accepted_pending");
    unsafe { libc::kill(daemon.id() as i32, libc::SIGTERM) };
    assert!(daemon.wait().unwrap().success());
    std::fs::remove_file(&socket).ok();
    let mut daemon = spawn_daemon(
        &state,
        &socket,
        &api_file,
        &grant_file,
        &fixture.endpoint,
        &completion_file,
        launcher_file.to_str().unwrap(),
    );
    let persisted = await_job_state(&state, &job_id, "delivered");
    let outbox = persisted["outbox"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    assert_eq!(outbox["acknowledged"], true);
    assert_eq!(outbox["receipt"]["a4Receipt"]["receiptId"], "a4-test");
    unsafe { libc::kill(daemon.id() as i32, libc::SIGTERM) };
    assert!(daemon.wait().unwrap().success());
    let submits = fixture.submits.lock().unwrap();
    assert_eq!(submits.len(), 2);
    assert_eq!(
        submits[0], submits[1],
        "lost reply retry changed semantic body"
    );
    assert!(fixture.requests.load(Ordering::Acquire) >= 3);
}

#[derive(Clone)]
enum ScriptedReply {
    Http(u16),
    Receipt {
        status: &'static str,
        disposition: &'static str,
        error: Option<&'static str>,
        wrong_event: bool,
        a4: bool,
    },
}

struct ScriptedFixture {
    endpoint: String,
    stop: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ScriptedFixture {
    fn start(replies: Vec<ScriptedReply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let thread_stop = Arc::clone(&stop);
        let thread_paths = Arc::clone(&paths);
        let thread_queue = Arc::clone(&queue);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let (path, body, _) = read_http(&mut stream);
                        thread_paths.lock().unwrap().push(path);
                        let event = serde_json::from_slice::<Value>(&body).unwrap()["eventId"]
                            .as_str()
                            .unwrap()
                            .to_string();
                        let reply = thread_queue
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("unexpected extra completion request");
                        match reply {
                            ScriptedReply::Http(status) => {
                                write_http(&mut stream, status, &json!({"error":"fixture"}));
                            }
                            ScriptedReply::Receipt {
                                status,
                                disposition,
                                error,
                                wrong_event,
                                a4,
                            } => {
                                let mut value = receipt(
                                    if wrong_event { "wrong-event" } else { &event },
                                    disposition,
                                    a4.then(|| json!({"receiptId":"a4-scripted"})),
                                );
                                value["status"] = json!(status);
                                if let Some(error) = error {
                                    value["errorCode"] = json!(error);
                                }
                                write_http(&mut stream, 200, &value);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self {
            endpoint,
            stop,
            paths,
            thread: Some(thread),
        }
    }
}

impl Drop for ScriptedFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn run_scripted_case(
    label: &str,
    replies: Vec<ScriptedReply>,
    expected: CompletionDeliveryState,
) -> (Vec<String>, usize) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let launcher_file = temp.path().join("launcher");
    std::fs::write(
        &launcher_file,
        b"#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher_file, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launcher_file = std::fs::canonicalize(launcher_file).unwrap();
    let token_file = temp.path().join("completion.key");
    write_private(&token_file, COMPLETION);
    let service = JobService::open(ServiceConfig {
        completion_enabled: true,
        state_root: temp.path().join("state"),
        grant_key: GRANT.into(),
        api_token: API.into(),
        runner_executable: runner(),
        per_stream_output_bytes: 4096,
        max_read_bytes: 1024,
        cancel_grace: Duration::from_millis(100),
        max_jobs: 8,
        max_active_jobs: 2,
        allowed_launchers: [(
            launcher_file.display().to_string(),
            file_sha256(&launcher_file).unwrap(),
        )]
        .into_iter()
        .collect(),
        completion_wire_version: CompletionWireVersion::V1,
    })
    .unwrap();
    let request = request(cwd.path(), label);
    let submitted = service
        .submit(
            API,
            request.clone(),
            grant(&request, launcher_file.to_str().unwrap()),
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !service
        .query(API, &submitted.job.job_id)
        .unwrap()
        .state
        .terminal()
    {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let fixture = ScriptedFixture::start(replies);
    let worker = CompletionDeliveryWorker::start(
        service.clone(),
        CompletionDeliveryConfig {
            endpoint: fixture.endpoint.clone(),
            token_file,
            request_timeout: Duration::from_millis(250),
            minimum_backoff: Duration::from_millis(20),
            maximum_backoff: Duration::from_millis(100),
            idle_interval: Duration::from_millis(10),
        },
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let job = service.query(API, &submitted.job.job_id).unwrap();
        if job.completion_delivery.state == expected {
            break;
        }
        assert!(Instant::now() < until, "{label} did not reach {expected:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(150));
    worker.shutdown();
    let paths = fixture.paths.lock().unwrap().clone();
    let pending = service.pending_outbox(API).unwrap().len();
    (paths, pending)
}

#[test]
fn delivery_states_bound_retry_and_preserve_archive_orphan_conflict_semantics() {
    let (paths, pending) = run_scripted_case(
        "authorization",
        vec![ScriptedReply::Http(401)],
        CompletionDeliveryState::AuthorizationFailed,
    );
    assert_eq!(paths.len(), 1, "authorization failure must not busy-retry");
    assert_eq!(pending, 1);

    let (paths, pending) = run_scripted_case(
        "conflict",
        vec![ScriptedReply::Receipt {
            status: "no_write",
            disposition: "pending",
            error: Some("event_conflict"),
            wrong_event: false,
            a4: false,
        }],
        CompletionDeliveryState::Conflict,
    );
    assert_eq!(paths.len(), 1, "conflict must require operator action");
    assert_eq!(pending, 1);

    let (paths, pending) = run_scripted_case(
        "wrong-receipt",
        vec![ScriptedReply::Receipt {
            status: "committed",
            disposition: "pending",
            error: None,
            wrong_event: true,
            a4: false,
        }],
        CompletionDeliveryState::OperatorRequired,
    );
    assert_eq!(
        paths.len(),
        1,
        "foreign receipt must not be retried blindly"
    );
    assert_eq!(pending, 1);

    let (paths, pending) = run_scripted_case(
        "archive-restore",
        vec![
            ScriptedReply::Receipt {
                status: "committed",
                disposition: "archived",
                error: None,
                wrong_event: false,
                a4: false,
            },
            ScriptedReply::Receipt {
                status: "committed",
                disposition: "delivered",
                error: None,
                wrong_event: false,
                a4: true,
            },
        ],
        CompletionDeliveryState::Delivered,
    );
    assert_eq!(
        paths,
        vec![
            "/api/job-service/v1/completions",
            "/api/job-service/v1/completions/query"
        ]
    );
    assert_eq!(pending, 0);

    let (paths, pending) = run_scripted_case(
        "permanent-orphan",
        vec![ScriptedReply::Receipt {
            status: "committed",
            disposition: "orphaned",
            error: None,
            wrong_event: false,
            a4: false,
        }],
        CompletionDeliveryState::Orphaned,
    );
    assert_eq!(paths.len(), 1);
    assert_eq!(
        pending, 0,
        "terminal orphan must not leak in the retry queue"
    );

    let (paths, pending) = run_scripted_case(
        "unavailable-retry",
        vec![
            ScriptedReply::Receipt {
                status: "no_write",
                disposition: "not_found",
                error: Some("target_classification_unavailable"),
                wrong_event: false,
                a4: false,
            },
            ScriptedReply::Receipt {
                status: "committed",
                disposition: "delivered",
                error: None,
                wrong_event: false,
                a4: true,
            },
        ],
        CompletionDeliveryState::Delivered,
    );
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().all(|path| path.ends_with("/completions")));
    assert_eq!(pending, 0);
}

#[test]
fn accepted_cutex_route_persists_real_pending_receipt_when_fixture_is_supplied() {
    let Some(cutex_test_binary) = std::env::var_os("CUTEX_JS3A_TEST_BINARY") else {
        eprintln!("CUTEX_JS3A_TEST_BINARY not supplied; accepted-Cutex route probe omitted");
        return;
    };
    let cutex_home = tempfile::tempdir().unwrap();
    std::fs::set_permissions(cutex_home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        cutex_home.path().join(".cutex-test-private-home"),
        b"js3b isolated accepted Cutex route\n",
    )
    .unwrap();
    let port = (24000..=24999)
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok())
        .unwrap();
    let exact = "cli_app::agent_bus_server::job_service_completion_lane_tests::actual_http_submit_query_replay_and_conflict_use_private_durable_repository";
    let mut cutex = Command::new(cutex_test_binary)
        .args(["--exact", exact, "--nocapture"])
        .env("HOME", cutex_home.path())
        .env("CUTEX_TEST_PRIVATE_HOME", cutex_home.path())
        .env("CUTEX_JS3A_HTTP_CHILD", "1")
        .env("CUTEX_JS3A_HTTP_PORT", port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let token_file = cutex_home
        .path()
        .join("service/job-service-completion.token");
    let until = Instant::now() + Duration::from_secs(10);
    while (!token_file.is_file() || TcpStream::connect(("127.0.0.1", port)).is_err())
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(token_file.is_file(), "accepted Cutex route did not start");

    let job_home = tempfile::tempdir().unwrap();
    std::fs::set_permissions(job_home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let launcher_file = job_home.path().join("launcher");
    std::fs::write(
        &launcher_file,
        b"#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher_file, std::fs::Permissions::from_mode(0o700)).unwrap();
    let launcher_file = std::fs::canonicalize(launcher_file).unwrap();
    let service = JobService::open(ServiceConfig {
        completion_enabled: true,
        state_root: job_home.path().join("state"),
        grant_key: GRANT.into(),
        api_token: API.into(),
        runner_executable: runner(),
        per_stream_output_bytes: 4096,
        max_read_bytes: 1024,
        cancel_grace: Duration::from_millis(100),
        max_jobs: 8,
        max_active_jobs: 2,
        allowed_launchers: [(
            launcher_file.display().to_string(),
            file_sha256(&launcher_file).unwrap(),
        )]
        .into_iter()
        .collect(),
        completion_wire_version: CompletionWireVersion::V1,
    })
    .unwrap();
    let request = request(cwd.path(), "accepted-cutex-route");
    let submitted = service
        .submit(
            API,
            request.clone(),
            grant(&request, launcher_file.to_str().unwrap()),
        )
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !service
        .query(API, &submitted.job.job_id)
        .unwrap()
        .state
        .terminal()
    {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
    let worker = CompletionDeliveryWorker::start(
        service.clone(),
        CompletionDeliveryConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            token_file,
            request_timeout: Duration::from_millis(500),
            minimum_backoff: Duration::from_secs(2),
            maximum_backoff: Duration::from_secs(5),
            idle_interval: Duration::from_millis(10),
        },
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let job = service.query(API, &submitted.job.job_id).unwrap();
        if job.completion_delivery.state == CompletionDeliveryState::AcceptedPending {
            break;
        }
        assert!(
            Instant::now() < until,
            "accepted Cutex route did not persist pending"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.shutdown();
    let cutex_state = cutex_home
        .path()
        .join(".cutex/runtime/management-v2/agent-bus-message-state.json");
    let persisted: Value = serde_json::from_slice(&std::fs::read(cutex_state).unwrap()).unwrap();
    assert_eq!(persisted["messages"].as_object().unwrap().len(), 1);
    cutex.kill().unwrap();
    cutex.wait().unwrap();
}
