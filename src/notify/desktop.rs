//! Desktop notification bridge service runtime and native notification helpers.

use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use uuid::Uuid;

use crate::config::paths::home_dir;
use crate::config::paths::runtime_dir;
use crate::config::store::load_codez_config;
use crate::config::store::save_codez_config;
use crate::http::client::http_local_root_status_ok;
use crate::http::client::http_post_json_expect_success;
use crate::http::client::HttpPostStatusRequest;
use crate::http::server::read_simple_http_request;
use crate::http::server::require_bridge_token;
use crate::http::server::write_http_response;
use crate::notify::service::desktop_notify_bridge_url;
use crate::notify::service::desktop_notify_health_url;
use crate::notify::service::desktop_notify_port;
use crate::notify::service::validate_desktop_notify_port;
use crate::notify::service::DEFAULT_DESKTOP_NOTIFY_PORT;
use crate::notify::service::DESKTOP_NOTIFY_BRIDGE_ID;
use crate::platform::command::command_exists_in_path;
use crate::platform::host::current_host_name;
use crate::profiles::model::CliKind;
use crate::profiles::model::CodezConfig;
use crate::profiles::model::StoredAccount;

const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

pub struct UbuntuDesktopNotifyInstall {
    pub bridge_url: String,
}

pub fn ensure_desktop_notify_bridge_for_launch(account: &StoredAccount) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex {
        return Ok(());
    }
    let config = load_codez_config();
    if !config.desktop_notify_enabled {
        return Ok(());
    }
    let config = ensure_desktop_notify_config(true, None)?;
    ensure_desktop_notify_bridge_running(&config)
}

pub fn ensure_desktop_notify_config(
    enabled: bool,
    port: Option<u16>,
) -> anyhow::Result<CodezConfig> {
    let mut config = load_codez_config();
    config.desktop_notify_enabled = enabled;
    if let Some(port) = port {
        validate_desktop_notify_port(port)?;
        config.desktop_notify_port = Some(port);
    } else if config.desktop_notify_port.is_none() {
        config.desktop_notify_port = Some(DEFAULT_DESKTOP_NOTIFY_PORT);
    }
    if config
        .desktop_notify_token
        .as_ref()
        .is_none_or(|token| token.trim().is_empty())
    {
        config.desktop_notify_token = Some(format!("cutex-{}", Uuid::new_v4()));
    }
    save_codez_config(&config)?;
    Ok(config)
}

pub fn disable_desktop_notify_config() -> anyhow::Result<CodezConfig> {
    let mut config = load_codez_config();
    config.desktop_notify_enabled = false;
    save_codez_config(&config)?;
    Ok(config)
}

pub fn install_ubuntu_desktop_notify_service(
    port: Option<u16>,
) -> anyhow::Result<UbuntuDesktopNotifyInstall> {
    let config = ensure_desktop_notify_config(true, port)?;
    let port = desktop_notify_port(&config);
    validate_desktop_notify_port(port)?;
    let service_path = ubuntu_desktop_notify_service_path()?;
    let service_dir = service_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid systemd service path"))?;
    fs::create_dir_all(service_dir)
        .with_context(|| format!("Failed to create {}", service_dir.display()))?;
    let exe = std::env::current_exe().context("Failed to resolve current cutex executable")?;
    let exe = exe
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Current cutex path is not valid UTF-8"))?;
    let service = format!(
        r#"[Unit]
Description=cutex desktop notification bridge
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart={exe} notify desktop serve --port {port}
Restart=on-failure
RestartSec=2
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus
Environment=PATH=/home/%u/.local/bin:/usr/local/bin:/usr/bin:/bin

[Install]
WantedBy=default.target
"#
    );
    fs::write(&service_path, service)
        .with_context(|| format!("Failed to write {}", service_path.display()))?;

    run_systemctl_user(&["daemon-reload"])?;
    run_systemctl_user(&["enable", "--now", "cutex-desktop-notify.service"])?;

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
            register_desktop_notify_handoff(port);
            return Ok(UbuntuDesktopNotifyInstall {
                bridge_url: desktop_notify_bridge_url(port),
            });
        }
    }

    anyhow::bail!(
        "Installed service, but bridge did not become healthy on port {port}. Check `systemctl --user status cutex-desktop-notify.service`."
    )
}

pub fn uninstall_ubuntu_desktop_notify_service() -> anyhow::Result<()> {
    let _ = run_systemctl_user(&["disable", "--now", "cutex-desktop-notify.service"]);
    let service_path = ubuntu_desktop_notify_service_path()?;
    if service_path.exists() {
        fs::remove_file(&service_path)
            .with_context(|| format!("Failed to remove {}", service_path.display()))?;
    }
    let _ = run_systemctl_user(&["daemon-reload"]);
    disable_desktop_notify_config()?;
    Ok(())
}

fn ubuntu_desktop_notify_service_path() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home
        .join(".config")
        .join("systemd")
        .join("user")
        .join("cutex-desktop-notify.service"))
}

fn run_systemctl_user(args: &[&str]) -> anyhow::Result<()> {
    if !command_exists_in_path("systemctl") {
        anyhow::bail!("systemctl is not available in PATH");
    }
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .with_context(|| format!("Failed to run systemctl --user {}", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("systemctl --user {} exited with {status}", args.join(" "));
    }
    Ok(())
}

pub fn ensure_desktop_notify_bridge_running(config: &CodezConfig) -> anyhow::Result<()> {
    let port = desktop_notify_port(config);
    validate_desktop_notify_port(port)?;
    if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
        register_desktop_notify_handoff(port);
        return Ok(());
    }

    let exe = std::env::current_exe().context("Failed to resolve current cutex executable")?;
    let log_dir = runtime_dir()?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create runtime dir: {}", log_dir.display()))?;
    let log_path = log_dir.join("desktop-notify-bridge.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .context("Failed to clone bridge log file")?;

    let mut child = Command::new(exe);
    child
        .arg("notify")
        .arg("desktop")
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(token) = config.desktop_notify_token.as_ref() {
        child.arg("--token").arg(token);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;

        child.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    child
        .spawn()
        .with_context(|| format!("Failed to start desktop notify bridge on port {port}"))?;

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref()) {
            register_desktop_notify_handoff(port);
            return Ok(());
        }
    }

    anyhow::bail!(
        "Desktop notify bridge did not become healthy on port {port}. See {}",
        log_path.display()
    )
}

pub fn desktop_notify_bridge_healthy(port: u16, token: Option<&str>) -> bool {
    http_local_root_status_ok(port, token, Duration::from_millis(250))
}

fn register_desktop_notify_handoff(port: u16) {
    if !command_exists_in_path("bridgeboard") {
        return;
    }
    let owner_host = current_host_name().to_ascii_lowercase();
    let health_url = desktop_notify_health_url(port);
    let _ = Command::new("bridgeboard")
        .arg("handoff")
        .arg("--id")
        .arg(DESKTOP_NOTIFY_BRIDGE_ID)
        .arg("--title")
        .arg("cutex desktop notification bridge")
        .arg("--port")
        .arg(port.to_string())
        .arg("--owner-host")
        .arg(owner_host)
        .arg("--pid-from-port")
        .arg("--health-url")
        .arg(health_url)
        .arg("--require-healthy")
        .status();
}

pub fn run_desktop_notify_bridge(config: CodezConfig) -> anyhow::Result<()> {
    let port = desktop_notify_port(&config);
    validate_desktop_notify_port(port)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("Failed to bind desktop notify bridge on 127.0.0.1:{port}"))?;
    println!(
        "cutex desktop notify bridge listening on {}",
        desktop_notify_health_url(port)
    );
    register_desktop_notify_handoff(port);

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(err) = handle_desktop_notify_request(&mut stream, &config) {
                    let _ = write_http_response(
                        &mut stream,
                        500,
                        "Internal Server Error",
                        "text/plain",
                        format!("{err:#}").as_bytes(),
                    );
                }
            }
            Err(err) => eprintln!("{YELLOW}warning:{RESET} desktop notify accept failed: {err}"),
        }
    }
    Ok(())
}

fn handle_desktop_notify_request(
    stream: &mut TcpStream,
    config: &CodezConfig,
) -> anyhow::Result<()> {
    let request = read_simple_http_request(stream)?;
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => write_http_response(stream, 200, "OK", "text/plain", b"ok"),
        ("POST", "/api/agent-notify/push") => {
            require_bridge_token(&request, config.desktop_notify_token.as_deref())?;
            let live_config = load_codez_config();
            if live_config.desktop_notify_enabled {
                handle_native_desktop_notify(&request.body)?;
            }
            forward_to_external_notify_service(&live_config, &request.body);
            write_http_response(stream, 200, "OK", "text/plain", b"ok")
        }
        _ => write_http_response(stream, 404, "Not Found", "text/plain", b"not found"),
    }
}

fn handle_native_desktop_notify(body: &[u8]) -> anyhow::Result<()> {
    let (title, body) = native_desktop_notification_from_payload(body)?;
    send_native_desktop_notification(&title, &body)
}

fn native_desktop_notification_from_payload(body: &[u8]) -> anyhow::Result<(String, String)> {
    let payload: Value = serde_json::from_slice(body).context("Failed to parse notify JSON")?;
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("notification");
    let project = payload
        .get("project_name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let agent = payload
        .get("agent_name")
        .and_then(Value::as_str)
        .unwrap_or("codex");
    let duration = payload
        .get("duration_seconds")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}s"))
        .unwrap_or_else(|| "-".to_string());
    let idle = payload
        .get("idle_seconds")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}s"))
        .unwrap_or_else(|| "-".to_string());
    Ok((
        format!("{agent}: {status}"),
        format!("{project} · duration {duration} · idle {idle}"),
    ))
}

pub fn send_native_desktop_notification(title: &str, body: &str) -> anyhow::Result<()> {
    if !command_exists_in_path("notify-send") {
        anyhow::bail!("notify-send is not available in PATH");
    }
    let status = Command::new("notify-send")
        .arg("-a")
        .arg("cutex")
        .arg("-u")
        .arg("normal")
        .arg(title)
        .arg(body)
        .status()
        .context("Failed to run notify-send")?;
    if !status.success() {
        anyhow::bail!("notify-send exited with status {status}");
    }
    Ok(())
}

fn forward_to_external_notify_service(config: &CodezConfig, body: &[u8]) {
    let Some(url) = config
        .notify_service_url
        .as_deref()
        .filter(|url| !url.trim().is_empty())
    else {
        return;
    };
    if is_desktop_bridge_url(config, url) {
        return;
    }
    if let Err(err) = post_http_json(url, config.notify_service_token.as_deref(), body) {
        eprintln!("{YELLOW}warning:{RESET} external notify forward failed: {err:#}");
    }
}

fn is_desktop_bridge_url(config: &CodezConfig, url: &str) -> bool {
    url == desktop_notify_bridge_url(desktop_notify_port(config))
}

fn post_http_json(url: &str, token: Option<&str>, body: &[u8]) -> anyhow::Result<()> {
    http_post_json_expect_success(HttpPostStatusRequest {
        url,
        token,
        body,
        timeout: Duration::from_secs(5),
        invalid_url_context: &format!("Invalid notify URL: {url}"),
        only_http_message: "Only http:// notify forwarding is supported by cutex desktop bridge",
        missing_host_message: &format!("Notify URL has no host: {url}"),
        connect_context: "Failed to connect external notify service",
        non_success_message: "External notify service returned non-success",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_desktop_notification_payload_uses_defaults() {
        let (title, body) =
            native_desktop_notification_from_payload(br#"{"status":"done"}"#).unwrap();

        assert_eq!(title, "codex: done");
        assert_eq!(body, "unknown · duration - · idle -");
    }

    #[test]
    fn native_desktop_notification_payload_includes_agent_project_and_timing() {
        let (title, body) = native_desktop_notification_from_payload(
            br#"{"status":"waiting","agent_name":"agent-a","project_name":"cutex","duration_seconds":12,"idle_seconds":3}"#,
        )
        .unwrap();

        assert_eq!(title, "agent-a: waiting");
        assert_eq!(body, "cutex · duration 12s · idle 3s");
    }
}
