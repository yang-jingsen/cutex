//! Host identity helpers.

use std::process::Command;
use std::sync::OnceLock;

static DETECTED_HOST_NAME: OnceLock<String> = OnceLock::new();

pub fn current_host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DETECTED_HOST_NAME.get_or_init(|| {
            Command::new("hostname")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string())
        }).clone())
}
