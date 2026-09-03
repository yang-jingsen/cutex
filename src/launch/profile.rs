//! Profile-aware launch command construction.

use std::path::Path;

use anyhow::{anyhow, Context};

use crate::config::proxy::{effective_proxy_config, rewrite_docker_loopback_proxy_url};
use crate::launch::command::LaunchCommand;
use crate::launch::docker::{
    build_docker_run_command, current_user_spec, docker_user_name, DockerLaunchPaths,
    DockerRunCommandSpec,
};
use crate::launch::env::{
    apply_profile_launch_envs, profile_launch_envs, ApplyProfileLaunchEnvOptions, LaunchEnvContext,
};
use crate::launch::program::cli_program;
use crate::profiles::model::{MaterializedAccountFiles, RuntimeConfig, StoredAccount};

pub fn profile_launch_command(
    account: &StoredAccount,
    codex_args: &[String],
    files: &MaterializedAccountFiles,
    install_dir: Option<String>,
    agent_mode: bool,
    agent_groups: &[String],
    context: &LaunchEnvContext<'_>,
) -> anyhow::Result<LaunchCommand> {
    match &account.runtime {
        RuntimeConfig::Host => Ok(host_profile_launch_command(
            account,
            codex_args,
            files,
            install_dir,
            agent_mode,
            agent_groups,
            context,
        )),
        RuntimeConfig::Docker { image, .. } => docker_profile_launch_command(
            account,
            image,
            codex_args,
            &files.auth_path,
            agent_mode,
            agent_groups,
            context,
        ),
    }
}

pub fn host_profile_launch_command(
    account: &StoredAccount,
    codex_args: &[String],
    files: &MaterializedAccountFiles,
    install_dir: Option<String>,
    agent_mode: bool,
    agent_groups: &[String],
    context: &LaunchEnvContext<'_>,
) -> LaunchCommand {
    let auth_path = files.auth_path.to_string_lossy();
    let config_path = files.config_path.to_string_lossy();
    let custom_status_items_path = files.custom_status_items_path.to_string_lossy();

    apply_profile_launch_envs(
        LaunchCommand::new(cli_program(&account.cli_kind)).args(codex_args.iter().cloned()),
        ApplyProfileLaunchEnvOptions {
            account,
            auth_path: auth_path.as_ref(),
            config_path: config_path.as_ref(),
            custom_status_items_path: custom_status_items_path.as_ref(),
            install_dir,
            api_key_auth_path: Some(&files.auth_path),
            agent_mode,
            agent_groups,
            context,
        },
    )
}

pub fn docker_profile_launch_command(
    account: &StoredAccount,
    image: &str,
    codex_args: &[String],
    host_auth_path: &Path,
    agent_mode: bool,
    agent_groups: &[String],
    context: &LaunchEnvContext<'_>,
) -> anyhow::Result<LaunchCommand> {
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;
    let user_name = docker_user_name(match &account.runtime {
        RuntimeConfig::Docker { user_name, .. } => user_name.as_deref(),
        RuntimeConfig::Host => None,
    })?;
    let paths = DockerLaunchPaths::new(&user_name, &account.id)?;
    let workspace = cwd
        .to_str()
        .ok_or_else(|| anyhow!("Current directory is not valid UTF-8"))?;
    let add_host_gateway_alias = effective_proxy_config(account, context.global_config)
        .filter(|proxy| proxy.enabled)
        .and_then(|proxy| proxy.url.as_deref())
        .is_some_and(|url| rewrite_docker_loopback_proxy_url(url).is_some());
    let launch_envs = profile_launch_envs(
        account,
        &paths.container_auth_path,
        &paths.container_config_path,
        &paths.container_custom_status_items_path,
        None,
        Some(host_auth_path),
        agent_mode,
        agent_groups,
        context,
    );

    Ok(build_docker_run_command(DockerRunCommandSpec {
        image,
        user_name: &user_name,
        user_spec: current_user_spec()?,
        workspace,
        paths: &paths,
        add_host_gateway_alias,
        host_gateway_alias: crate::config::proxy::DOCKER_PROXY_HOST_ALIAS,
        launch_envs: &launch_envs,
        cli_program: cli_program(&account.cli_kind),
        cli_args: codex_args,
    }))
}
