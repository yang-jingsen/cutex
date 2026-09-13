#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const API: &[u8] = b"api-token-for-js2-tests-32-bytes-minimum";
const GRANT: &[u8] = b"grant-key-for-js2-tests-32-bytes-minimum";
const BUS_TOKEN: &str = "isolated-agent-bus-token";
const RUNTIME_ID: &str = "runtime-js2-probe";
const DURABLE_ID: &str = "cutex.js2-real-path";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_cutex-job-service")
}

fn launcher() -> &'static str {
    "/home/senxiu/Resources/Shortcuts/cute-codex"
}

#[test]
fn installed_codex_configured_mcp_binds_real_full_access_origin() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let socket = temp.path().join("run/job.sock");
    let api_file = temp.path().join("api.key");
    let grant_file = temp.path().join("grant.key");
    write_secret(&api_file, API);
    write_secret(&grant_file, GRANT);

    let mut daemon = Command::new(binary())
        .args([
            "serve",
            state.to_str().unwrap(),
            socket.to_str().unwrap(),
            api_file.to_str().unwrap(),
            grant_file.to_str().unwrap(),
            launcher(),
        ])
        .spawn()
        .unwrap();
    wait_for_path(&socket, Duration::from_secs(120));

    let observed_thread = Arc::new(Mutex::new(None));
    let (bus_url, bus_stop, bus_worker) = fake_agent_bus(Arc::clone(&observed_thread));
    let (model_url, model_stop, model_worker) =
        fake_responses_server(temp.path(), Arc::clone(&observed_thread));
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&codex_home).unwrap();
    let config = format!(
        r#"model = "gpt-5.4"
model_provider = "js2-mock"

[model_providers.js2-mock]
name = "js2-mock"
base_url = {model_url}
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false

[mcp_servers.job_service]
command = {binary}
args = ["mcp-stdio", {socket}, {api}, {grant}, {launcher}]
env_vars = ["CUTEX_AGENT_ID", "CUTEX_AGENT_BUS_URL", "CUTEX_AGENT_BUS_TOKEN"]
startup_timeout_sec = 120
default_tools_approval_mode = "approve"

[code_mode]
direct_only_tool_namespaces = ["mcp__job_service"]
"#,
        model_url = serde_json::to_string(&format!("{model_url}/v1")).unwrap(),
        binary = serde_json::to_string(binary()).unwrap(),
        socket = serde_json::to_string(socket.to_str().unwrap()).unwrap(),
        api = serde_json::to_string(api_file.to_str().unwrap()).unwrap(),
        grant = serde_json::to_string(grant_file.to_str().unwrap()).unwrap(),
        launcher = serde_json::to_string(launcher()).unwrap(),
    );
    std::fs::write(codex_home.join("config.toml"), config).unwrap();

    let mcp_list = Command::new(launcher())
        .args(["mcp", "list"])
        .env("CODEX_HOME", &codex_home)
        .output()
        .unwrap();
    assert!(
        mcp_list.status.success()
            && String::from_utf8_lossy(&mcp_list.stdout).contains("job_service"),
        "configured MCP server is absent: stdout={} stderr={}",
        String::from_utf8_lossy(&mcp_list.stdout),
        String::from_utf8_lossy(&mcp_list.stderr)
    );

    let output = Command::new(launcher())
        .args([
            "exec",
            "--skip-git-repo-check",
            "--ephemeral",
            "--json",
            "--sandbox",
            "danger-full-access",
            "submit the requested isolated test job",
        ])
        .current_dir(temp.path())
        .env("CODEX_HOME", &codex_home)
        .env("CUTEX_AGENT_ID", RUNTIME_ID)
        .env("CUTEX_AGENT_BUS_URL", &bus_url)
        .env("CUTEX_AGENT_BUS_TOKEN", BUS_TOKEN)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "installed Codex failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    bus_stop.send(()).unwrap();
    model_stop.send(()).unwrap();
    let bus_requests = bus_worker.join().unwrap();
    let model_requests = model_worker.join().unwrap();
    assert_eq!(
        bus_requests.len(),
        1,
        "expected one authenticated Agent Bus lookup; model requests={:?}; codex stdout={} stderr={}",
        model_requests
            .iter()
            .map(|request| summarize_request(request))
            .collect::<Vec<_>>(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(model_requests.len(), 3, "unexpected Responses API requests");

    let store: Value =
        serde_json::from_slice(&std::fs::read(state.join("state.json")).unwrap()).unwrap();
    let jobs = store["jobs"].as_object().unwrap();
    assert_eq!(jobs.len(), 1);
    let job = jobs.values().next().unwrap();
    assert_eq!(job["request"]["subscriberCutexSessionId"], DURABLE_ID);
    assert_eq!(job["request"]["cwd"], temp.path().to_str().unwrap());
    assert_eq!(job["request"]["origin"]["runtimeAgentId"], RUNTIME_ID);
    assert_eq!(
        job["request"]["origin"]["nativeThreadId"],
        observed_thread.lock().unwrap().clone().unwrap()
    );
    assert_eq!(
        job["request"]["origin"]["permissionProfileType"],
        "disabled"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !job_output_contains(&state, jobs.keys().next().unwrap(), b"real-path")
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(job_output_contains(
        &state,
        jobs.keys().next().unwrap(),
        b"real-path"
    ));

    let _ = daemon.kill();
    let _ = daemon.wait();
}

fn fake_agent_bus(
    observed_thread: Arc<Mutex<Option<String>>>,
) -> (String, Sender<()>, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        loop {
            if stop_rx.try_recv().is_ok() {
                return requests;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("Agent Bus accept failed: {error}"),
            };
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("GET /api/agents?agent_id="));
            assert!(request.contains("&all_hosts=false HTTP/1.1"));
            assert!(request.contains(&format!("Authorization: Bearer {BUS_TOKEN}\r\n")));
            let thread_id = observed_thread
                .lock()
                .unwrap()
                .clone()
                .expect("Responses request must establish the originating thread first");
            let body = serde_json::to_vec(&json!([{
                "id":RUNTIME_ID,
                "session_id":thread_id,
                "cutex_session_id":DURABLE_ID
            }]))
            .unwrap();
            write_http(&mut stream, "application/json", &body);
            requests.push(request);
        }
    });
    (format!("http://{address}"), stop_tx, worker)
}

fn fake_responses_server(
    cwd: &std::path::Path,
    observed_thread: Arc<Mutex<Option<String>>>,
) -> (String, Sender<()>, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let cwd = cwd.to_str().unwrap().to_string();
    let (stop_tx, stop_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut requests = Vec::new();
        loop {
            if stop_rx.try_recv().is_ok() {
                return requests;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("Responses API accept failed: {error}"),
            };
            let request = read_http_request(&mut stream);
            assert!(request.starts_with("POST /v1/responses HTTP/1.1"));
            let thread_id = request
                .lines()
                .find_map(|line| line.strip_prefix("thread-id: "))
                .expect("Codex request must carry a thread-id")
                .to_string();
            *observed_thread.lock().unwrap() = Some(thread_id);
            let index = requests.len();
            // The production app-server is long lived. This isolated fresh process must
            // finish hashing the pinned launcher before deferred MCP discovery.
            if index == 0 {
                std::thread::sleep(Duration::from_secs(40));
            }
            let events = if index == 0 {
                vec![
                    json!({"type":"response.created","response":{"id":"resp-js2-search"}}),
                    json!({
                        "type":"response.output_item.done",
                        "item":{
                            "type":"tool_search_call",
                            "call_id":"call-js2-search",
                            "execution":"client",
                            "arguments":json!({
                                "query":"Job Service submit asynchronous command",
                                "limit":4
                            })
                        }
                    }),
                    completed("resp-js2-search"),
                ]
            } else if index == 1 {
                vec![
                    json!({"type":"response.created","response":{"id":"resp-js2-1"}}),
                    json!({
                        "type":"response.output_item.done",
                        "item":{
                            "type":"function_call",
                            "call_id":"call-js2-submit",
                            "namespace":"mcp__job_service",
                            "name":"submit",
                            "arguments":serde_json::to_string(&json!({
                                "actionId":"js2-real-path-action",
                                "argv":["/bin/sh","-c","printf real-path"],
                                "cwd":cwd
                            })).unwrap()
                        }
                    }),
                    completed("resp-js2-1"),
                ]
            } else {
                vec![
                    json!({"type":"response.output_item.done","item":{"type":"message","role":"assistant","id":"msg-js2","content":[{"type":"output_text","text":"submitted"}]}}),
                    completed("resp-js2-2"),
                ]
            };
            let mut body = String::new();
            for event in events {
                let kind = event["type"].as_str().unwrap();
                body.push_str(&format!("event: {kind}\ndata: {event}\n\n"));
            }
            write_http(&mut stream, "text/event-stream", body.as_bytes());
            requests.push(request);
        }
    });
    (format!("http://{address}"), stop_tx, worker)
}

fn completed(id: &str) -> Value {
    json!({"type":"response.completed","response":{"id":id,"usage":{"input_tokens":0,"input_tokens_details":null,"output_tokens":0,"output_tokens_details":null,"total_tokens":0}}})
}

fn summarize_request(request: &str) -> Value {
    let body = request.split_once("\r\n\r\n").unwrap().1;
    let value: Value = serde_json::from_str(body).unwrap();
    json!({
        "tools": value["tools"].as_array().unwrap().iter().filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            (name.contains("job") || name == "tool_search").then_some(name)
        }).collect::<Vec<_>>(),
        "inputTail": value["input"].as_array().unwrap().iter().rev().take(2).cloned().collect::<Vec<_>>()
    })
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        let Some(split) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&bytes[..split]);
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .unwrap_or(0);
        if bytes.len() >= split + 4 + length {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

fn write_http(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
}

fn write_secret(path: &std::path::Path, value: &[u8]) {
    std::fs::write(path, value).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn wait_for_path(path: &std::path::Path, timeout: Duration) {
    let until = Instant::now() + timeout;
    while !path.exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(path.exists(), "path did not appear: {}", path.display());
}

fn job_output_contains(state: &std::path::Path, job_id: &str, expected: &[u8]) -> bool {
    std::fs::read(state.join("output").join(format!("{job_id}.stdout"))).is_ok_and(|bytes| {
        bytes
            .windows(expected.len())
            .any(|window| window == expected)
    })
}
