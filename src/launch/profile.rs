//! Profile-aware launch command construction.

use crate::launch::command::LaunchCommand;
use crate::launch::env::{
    apply_profile_launch_envs, ApplyProfileLaunchEnvOptions, LaunchEnvContext,
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
        RuntimeConfig::Docker { .. } => anyhow::bail!("Cutex Docker integration was retired; select a Host profile"),
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
