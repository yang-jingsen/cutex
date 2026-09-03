//! Docker launch helper policy.

use std::process::Command;

use anyhow::Context;

use crate::config::env::{
    env_bool_override_any, CODEZ_DOCKER_USE_SUDO_ENV_VAR, CUTEX_DOCKER_USE_SUDO_ENV_VAR,
};
use crate::config::paths::{docker_runtime_home_dir, legacy_docker_runtime_home_dir};
use crate::config::store::load_codez_config;
use crate::launch::command::LaunchCommand;
use crate::profiles::materialize::materialized_profiles_dir;

pub struct DockerLaunchPaths {
    pub container_user_home: String,
    pub container_codex_home: String,
    pub host_user_home: std::path::PathBuf,
    pub host_profiles_root: std::path::PathBuf,
    pub container_profiles_root: String,
    pub container_profile_dir: String,
    pub container_auth_path: String,
    pub container_config_path: String,
    pub container_custom_status_items_path: String,
}

pub struct DockerRunCommandSpec<'a> {
    pub image: &'a str,
    pub user_name: &'a str,
    pub user_spec: String,
    pub workspace: &'a str,
    pub paths: &'a DockerLaunchPaths,
    pub add_host_gateway_alias: bool,
    pub host_gateway_alias: &'a str,
    pub launch_envs: &'a [(String, String)],
    pub cli_program: String,
    pub cli_args: &'a [String],
}

impl DockerLaunchPaths {
    pub fn new(user_name: &str, account_id: &str) -> anyhow::Result<Self> {
        let container_user_home = format!("/home/{user_name}");
        let container_codex_home = format!("{container_user_home}/.codex");
        let host_user_home = sandbox_user_home(user_name)?;
        let host_profiles_root = materialized_profiles_dir()?;
        let container_profiles_root = format!("{container_user_home}/.cutex-profiles");
        let container_profile_dir = format!("{container_profiles_root}/{account_id}");
        let container_auth_path = format!("{container_profile_dir}/auth.json");
        let container_config_path = format!("{container_profile_dir}/config.toml");
        let container_custom_status_items_path =
            format!("{container_profile_dir}/custom-status-items.json");

        Ok(Self {
            container_user_home,
            container_codex_home,
            host_user_home,
            host_profiles_root,
            container_profiles_root,
            container_profile_dir,
            container_auth_path,
            container_config_path,
            container_custom_status_items_path,
        })
    }
}

pub fn build_docker_run_command(spec: DockerRunCommandSpec<'_>) -> LaunchCommand {
    let mut launch = docker_command()
        .arg("run")
        .arg("--rm")
        .arg("-it")
        .arg("--user")
        .arg(spec.user_spec)
        .arg("-e")
        .arg(format!("HOME={}", spec.paths.container_user_home))
        .arg("-e")
        .arg(format!("USER={}", spec.user_name))
        .arg("-e")
        .arg(format!("LOGNAME={}", spec.user_name))
        .arg("-e")
        .arg(format!("CODEX_HOME={}", spec.paths.container_codex_home))
        .arg("-v")
        .arg(format!("{0}:{0}", spec.workspace))
        .arg("-w")
        .arg(spec.workspace)
        .arg("-v")
        .arg(format!(
            "{}:{}",
            spec.paths.host_user_home.display(),
            spec.paths.container_user_home
        ))
        .arg("-v")
        .arg(format!(
            "{}:{}",
            spec.paths.host_profiles_root.display(),
            spec.paths.container_profiles_root
        ));
    if spec.add_host_gateway_alias {
        launch = launch
            .arg("--add-host")
            .arg(format!("{}:host-gateway", spec.host_gateway_alias));
    }

    for (key, value) in spec.launch_envs {
        launch = launch.arg("-e").arg(format!("{key}={value}"));
    }

    launch
        .arg(spec.image)
        .arg(spec.cli_program)
        .args(spec.cli_args.iter().cloned())
}

pub fn docker_command() -> LaunchCommand {
    if docker_requires_sudo() {
        LaunchCommand::new("sudo").arg("docker")
    } else {
        LaunchCommand::new("docker")
    }
}

fn docker_requires_sudo() -> bool {
    env_bool_override_any(&[CUTEX_DOCKER_USE_SUDO_ENV_VAR, CODEZ_DOCKER_USE_SUDO_ENV_VAR])
        .unwrap_or_else(|| load_codez_config().docker_use_sudo)
}

pub fn docker_user_name(input: Option<&str>) -> anyhow::Result<String> {
    match input {
        Some(value) => normalize_docker_user_name(Some(value.to_string())),
        None => Ok(default_docker_user_name()),
    }
}

pub fn default_docker_user_name() -> String {
    std::env::var("USER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value != "."
                && value != ".."
                && !value.starts_with('-')
                && !value.contains('/')
                && !value.contains('\\')
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        })
        .unwrap_or_else(|| "cutex".to_string())
}

pub fn normalize_docker_user_name(input: Option<String>) -> anyhow::Result<String> {
    let value = input
        .unwrap_or_else(default_docker_user_name)
        .trim()
        .to_string();

    if value.is_empty() {
        anyhow::bail!("Docker user name cannot be empty");
    }

    if value == "." || value == ".." {
        anyhow::bail!("Docker user name cannot be '.' or '..'");
    }

    if value.contains('/') || value.contains('\\') {
        anyhow::bail!("Docker user name cannot contain path separators");
    }

    if value.starts_with('-') {
        anyhow::bail!("Docker user name cannot start with '-'");
    }

    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        anyhow::bail!("Docker user name may only contain ASCII letters, digits, '.', '_' or '-'");
    }

    Ok(value)
}

pub fn sandbox_user_home(_user_name: &str) -> anyhow::Result<std::path::PathBuf> {
    let preferred = docker_runtime_home_dir()?;
    if preferred.exists() {
        return Ok(preferred);
    }

    let legacy = legacy_docker_runtime_home_dir()?;
    if legacy.exists() {
        return Ok(legacy);
    }

    Ok(preferred)
}

#[cfg(unix)]
pub fn current_user_spec() -> anyhow::Result<String> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .context("Failed to query current uid")?;
    let gid = Command::new("id")
        .arg("-g")
        .output()
        .context("Failed to query current gid")?;

    if !uid.status.success() || !gid.status.success() {
        anyhow::bail!("Failed to determine current uid/gid");
    }

    let uid = String::from_utf8(uid.stdout).context("Invalid uid output")?;
    let gid = String::from_utf8(gid.stdout).context("Invalid gid output")?;
    Ok(format!("{}:{}", uid.trim(), gid.trim()))
}

#[cfg(not(unix))]
pub fn current_user_spec() -> anyhow::Result<String> {
    Ok("0:0".to_string())
}
