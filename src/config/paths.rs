//! Filesystem layout for cutex runtime and profile state.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;

#[cfg(test)]
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

#[cfg(not(test))]
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if std::env::var_os("CUTEX_TEST_PRIVATE_HOME").is_some() {
            return private_test_home_dir();
        }
        #[cfg(windows)]
        if let Some(environment_home) = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            let native_home = dirs::home_dir();
            if native_home.as_ref() != Some(&environment_home) {
                return None;
            }
        }
    }
    dirs::home_dir()
}

#[cfg(all(not(test), debug_assertions))]
fn private_test_home_dir() -> Option<PathBuf> {
    let boundary = std::env::var_os("CUTEX_TEST_PRIVATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)?;
    if !boundary.is_dir() || !boundary.join(".cutex-test-private-home").is_file() {
        return None;
    }
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .or(Some(boundary))
}

pub fn config_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".cutex"))
}

pub fn legacy_config_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".codez-cli"))
}

pub fn runtime_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("runtime"))
}

pub fn host_codex_home_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("codex-home"))
}

pub fn legacy_host_codex_home_dir() -> anyhow::Result<PathBuf> {
    let home = home_dir().context("Could not determine home directory")?;
    Ok(home.join(".codex-codez"))
}

pub fn docker_runtime_home_dir() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("docker-home"))
}

pub fn legacy_docker_runtime_home_dir() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("thirdparty").join("userhome"))
}

pub fn login_runtime_root() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("login"))
}

pub fn migrate_legacy_runtime_layout() -> anyhow::Result<()> {
    migrate_dir_if_needed(&legacy_config_dir()?, &config_dir()?)?;
    migrate_dir_if_needed(&legacy_host_codex_home_dir()?, &host_codex_home_dir()?)?;
    Ok(())
}

fn migrate_dir_if_needed(from: &Path, to: &Path) -> anyhow::Result<()> {
    if !from.exists() || to.exists() {
        return Ok(());
    }

    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create migration parent dir: {}",
                parent.display()
            )
        })?;
    }

    fs::rename(from, to).with_context(|| {
        format!(
            "Failed to migrate legacy directory {} -> {}",
            from.display(),
            to.display()
        )
    })?;
    Ok(())
}

pub fn accounts_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("accounts.json"))
}

pub fn quick_state_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("state.json"))
}

pub fn config_path() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}
