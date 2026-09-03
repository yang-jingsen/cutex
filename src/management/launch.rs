//! Management v2 service launch.

use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use crate::config::paths::runtime_dir;
use crate::management::remote::management_api_healthy;
use crate::management::service::{management_api_token, validate_management_port};
use crate::profiles::model::CodezConfig;
use anyhow::Context;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub fn ensure_management_api_running(config: &CodezConfig, port: u16) -> anyhow::Result<()> {
    validate_management_port(port)?;
    let token = management_api_token(config, None);
    if management_api_healthy(port, token) {
        return Ok(());
    }

    let exe = std::env::current_exe().context("Failed to resolve current cutex executable")?;
    let log_dir = runtime_dir()?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create runtime dir: {}", log_dir.display()))?;
    let log_path = log_dir.join("management-api.log");
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .context("Failed to clone management log file")?;

    let mut child = management_launch_command(&exe, port);
    child
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    unsafe {
        child.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    child
        .spawn()
        .with_context(|| format!("Failed to start cutex management API on port {port}"))?;

    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if management_api_healthy(port, token) {
            return Ok(());
        }
    }

    anyhow::bail!(
        "cutex management API did not become healthy on port {port}. See {}",
        log_path.display()
    )
}

fn management_launch_command(exe: &Path, port: u16) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("management")
        .arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--bind")
        .arg("127.0.0.1");
    command
}

pub fn warn_management_api_unavailable(err: &anyhow::Error) {
    eprintln!("{YELLOW}warning:{RESET} cutex management v2 service unavailable: {err:#}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_management_launch_has_no_bearer_argument() {
        let management = "fixture-management-root-not-for-argv";
        let agent_bus = "fixture-agent-bus-root-not-for-argv";
        let command = management_launch_command(Path::new("/tmp/cutex"), 24270);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            [
                "management",
                "serve",
                "--port",
                "24270",
                "--bind",
                "127.0.0.1"
            ]
        );
        assert!(!args.iter().any(|arg| arg == "--token"));
        assert!(!args.iter().any(|arg| arg == management || arg == agent_bus));
    }
}
