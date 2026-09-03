use anyhow::Context;

use cutex::launch::args::{
    codex_args_for_runtime, combined_profile_cli_args, should_add_docker_sandbox_bypass,
};
use cutex::launch::program::cli_program;
use cutex::management::service::DEFAULT_MANAGEMENT_PORT;
use cutex::notify::desktop::ensure_desktop_notify_bridge_for_launch;
use cutex::profiles::model::{CliKind, RuntimeConfig, StoredAccount};

use super::agent_bus_config;
use super::agent_bus_runtime;
use super::launch_command;
use super::launch_output::LaunchOutput;
use super::launch_presenter;
use super::launch_session;
use super::prompt::cli_args_label;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";

pub(crate) fn run_codex_process(
    account: &StoredAccount,
    codex_args: Vec<String>,
    output: LaunchOutput,
    agent_mode: bool,
    agent_groups: Vec<String>,
) -> anyhow::Result<()> {
    let program = cli_program(&account.cli_kind);
    let base_codex_args = combined_profile_cli_args(account, codex_args);
    let add_docker_sandbox_bypass = should_add_docker_sandbox_bypass(account, &base_codex_args);
    let effective_codex_args = codex_args_for_runtime(account, base_codex_args);
    let output = output.including_child_args(&effective_codex_args);
    output.line(format_args!("CLI binary: {BOLD}{}{RESET}", program));
    launch_presenter::print_launch_summary(account, agent_mode, &agent_groups, output);
    if !account.default_cli_args.is_empty() {
        output.line(format_args!(
            "Default CLI args: {}",
            cli_args_label(&account.default_cli_args)
        ));
    }
    if !effective_codex_args.is_empty() {
        output.line(format_args!(
            "CLI args: {}",
            cli_args_label(&effective_codex_args)
        ));
    }
    if add_docker_sandbox_bypass {
        output.line(format_args!(
            "docker detected: adding {} --sandbox danger-full-access to avoid bubblewrap/userns failures",
            program
        ));
    }
    launch_session::warn_if_resume_target_is_already_running(&effective_codex_args, output)?;
    ensure_desktop_notify_bridge_for_launch(account)?;
    ensure_management_api_for_launch(account)?;
    if agent_mode {
        ensure_agent_bus_for_launch(account)?;
    }
    let launch = launch_session::maybe_wrap_launch_with_session(
        account,
        &effective_codex_args,
        output,
        launch_command::codex_launch_command_with_agent_mode(
            account,
            &effective_codex_args,
            agent_mode,
            &agent_groups,
        )?,
    )?;
    let exit_code = exit_code_from_status(
        launch
            .to_command()
            .status()
            .with_context(|| format!("Failed to start launch command for {program}"))?,
    );

    std::process::exit(exit_code);
}

pub(crate) fn ensure_management_api_for_launch(account: &StoredAccount) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex || !matches!(account.runtime, RuntimeConfig::Host) {
        return Ok(());
    }
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    if let Err(err) =
        cutex::management::launch::ensure_management_api_running(&config, DEFAULT_MANAGEMENT_PORT)
    {
        cutex::management::launch::warn_management_api_unavailable(&err);
    }
    Ok(())
}

pub(crate) fn ensure_agent_bus_for_launch(account: &StoredAccount) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex || !matches!(account.runtime, RuntimeConfig::Host) {
        return Ok(());
    }
    let config = cutex::config::store::load_codez_config();
    if !config.agent_bus_enabled {
        return Ok(());
    }
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, true)
}

fn exit_code_from_status(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}
