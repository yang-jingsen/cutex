use std::io::{self, Write};

use anyhow::anyhow;
use chrono::Utc;

use cutex::config::store::{load_codez_config, load_quick_state, save_quick_state};
use cutex::launch::args::combined_profile_cli_args;
use cutex::launch::program::codex_program;
use cutex::launch::runtime::{apply_runtime_override, runtime_description};
use cutex::profiles::lookup::find_account;
use cutex::profiles::materialize::ensure_materialized_account_files;
use cutex::profiles::materialize::validate_materialized_account_files;
use cutex::profiles::model::{
    runtime_label, AccountsStore, CodezConfig, MaterializedAccountFiles, QuickRunState,
    StoredAccount,
};
use cutex::profiles::store::save_store;

use super::account_store::{load_store, load_store_read_only};
use super::launch_output::LaunchOutput;
use super::launch_process;
use super::prompt::cli_args_label;
use super::root_wizard;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

pub(crate) fn print_build(output: LaunchOutput) {
    root_wizard::print_cutex_build(output);
}

pub(crate) fn wizard() -> anyhow::Result<()> {
    root_wizard::cmd_wizard()
}

pub(crate) fn run_profile(
    profile: &str,
    codex_args: Vec<String>,
    output: LaunchOutput,
    force_host: bool,
    agent_mode: bool,
    agent_groups: Vec<String>,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> anyhow::Result<()> {
    let account = prepare_account_for_launch(profile)?;
    let effective_account =
        apply_runtime_override(&account, force_host, docker_image, docker_user_name)?;
    output.line(format_args!(
        "{GREEN}Running{RESET} profile {BOLD}{}{RESET} without changing active profile",
        account.name
    ));
    if effective_account.runtime != account.runtime {
        output.line(format_args!(
            "One-off runtime override: {}",
            runtime_description(&effective_account.runtime)
        ));
    }
    launch_process::run_codex_process(
        &effective_account,
        codex_args,
        output,
        agent_mode,
        agent_groups,
    )?;
    Ok(())
}

pub(crate) fn quick_run(
    codex_args: Vec<String>,
    output: LaunchOutput,
    quick: bool,
    force_host: bool,
    agent_mode: bool,
    agent_groups: Vec<String>,
) -> anyhow::Result<()> {
    let store = load_store()?;
    if store.accounts.is_empty() {
        output.line(format_args!(
            "No accounts configured. Use `cutex add --from-auth <path> --name <name>` to add one."
        ));
        return Ok(());
    }

    let mut state = load_quick_state();
    let global_config = load_codez_config();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| path.to_str().map(|text| text.to_string()));

    let default_name = determine_default_profile(&store, &state, &global_config, cwd.as_deref());
    let use_default_without_prompt =
        quick || output.is_machine_readable() || global_config.default_profile_direct_launch;

    let chosen = if use_default_without_prompt {
        output.line(format_args!(
            "Using default profile: {BOLD}{}{RESET}",
            default_name
        ));
        default_name.clone()
    } else {
        let selected = prompt_for_profile(&store, &default_name)?;
        output.line(format_args!("Using profile: {BOLD}{}{RESET}", selected));
        selected
    };

    if let Some(dir) = cwd {
        state.per_directory.insert(dir, chosen.clone());
    }
    state.last_global_profile = Some(chosen.clone());
    let _ = save_quick_state(&state);
    let program = codex_program();
    let chosen_account = store
        .accounts
        .iter()
        .find(|account| account.name == chosen)
        .ok_or_else(|| anyhow!("Profile disappeared after selection: {chosen}"))?;
    let preview_args = combined_profile_cli_args(chosen_account, codex_args.clone());

    if !preview_args.is_empty() {
        let args_preview = cli_args_label(&preview_args);
        if use_default_without_prompt {
            output.line(format_args!(
                "Running profile '{}' with: {} {}",
                chosen, program, args_preview
            ));
        } else {
            println!(
                "Will run profile '{}' with: {} {}",
                chosen, program, args_preview
            );
            print!("Proceed? [Y/n] ");
            io::stdout().flush()?;

            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let answer = line.trim();
            if !(answer.is_empty() || matches!(answer, "y" | "Y")) {
                println!("Aborted.");
                return Ok(());
            }
        }
    } else {
        output.line(format_args!(
            "Running profile '{}' with: {}",
            chosen, program
        ));
    }

    run_profile(
        &chosen,
        codex_args,
        output,
        force_host,
        agent_mode,
        agent_groups,
        None,
        None,
    )
}

pub(crate) fn determine_default_profile(
    store: &AccountsStore,
    state: &QuickRunState,
    global_config: &CodezConfig,
    cwd: Option<&str>,
) -> String {
    if let Some(dir) = cwd {
        if let Some(name) = state.per_directory.get(dir) {
            if store.accounts.iter().any(|account| account.name == *name) {
                return name.clone();
            }
        }
    }

    if let Some(name) = &global_config.default_profile {
        if store.accounts.iter().any(|account| account.name == *name) {
            return name.clone();
        }
    }

    if let Some(name) = &state.last_global_profile {
        if store.accounts.iter().any(|account| account.name == *name) {
            return name.clone();
        }
    }

    store
        .accounts
        .first()
        .map(|account| account.name.clone())
        .unwrap_or_else(|| "default".to_string())
}

fn prompt_for_profile(store: &AccountsStore, default_name: &str) -> anyhow::Result<String> {
    println!("{BOLD}{CYAN}Choose a profile{RESET}");
    for (idx, acc) in store.accounts.iter().enumerate() {
        let is_active = Some(&acc.id) == store.active_account_id.as_ref();
        let is_default = acc.name == default_name;
        let marker = if is_active {
            format!("{GREEN}●{RESET}")
        } else if is_default {
            format!("{YELLOW}◆{RESET}")
        } else {
            format!("{DIM}○{RESET}")
        };
        let badges = match (is_active, is_default) {
            (true, true) => format!("{GREEN}active{RESET} {YELLOW}default{RESET}"),
            (true, false) => format!("{GREEN}active{RESET}"),
            (false, true) => format!("{YELLOW}default{RESET}"),
            (false, false) => String::new(),
        };
        let runtime = runtime_label(&acc.runtime);
        println!(
            "  {} {BOLD}[{}]{RESET} {CYAN}{}{RESET}  {DIM}{}{RESET}  {YELLOW}{}{RESET}  {}",
            marker,
            idx + 1,
            acc.name,
            acc.source.as_deref().unwrap_or("unknown"),
            runtime,
            badges
        );
    }

    print!("Profile to use [{default_name}]: ");
    io::stdout().flush()?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let input = line.trim();

    if input.is_empty() {
        return Ok(default_name.to_string());
    }

    if let Some(acc) = store
        .accounts
        .iter()
        .find(|account| account.name == input || account.id == input)
    {
        return Ok(acc.name.clone());
    }

    if let Ok(idx) = input.parse::<usize>() {
        if idx >= 1 && idx <= store.accounts.len() {
            return Ok(store.accounts[idx - 1].name.clone());
        }
    }

    anyhow::bail!("Unknown profile: {input}")
}

pub(crate) fn prepare_account_for_launch(target: &str) -> anyhow::Result<StoredAccount> {
    let mut store = load_store()?;
    let account_id = find_account(&store, target)?
        .map(|account| account.id.clone())
        .ok_or_else(|| anyhow!("Account not found: {target}"))?;

    let account = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| anyhow!("Account not found after sync: {target}"))?;

    ensure_materialized_account_files(&account)?;

    if let Some(acc) = store.accounts.iter_mut().find(|a| a.id == account.id) {
        acc.last_used_at = Some(Utc::now());
    }
    save_store(&store)?;

    Ok(account)
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedLaunchProfile {
    pub(crate) requested: String,
    pub(crate) account: StoredAccount,
    pub(crate) files: MaterializedAccountFiles,
}

impl ResolvedLaunchProfile {
    pub(crate) fn effective_name(&self) -> &str {
        &self.account.name
    }
}

pub(crate) fn resolve_launch_profile_override(
    target: &str,
) -> anyhow::Result<ResolvedLaunchProfile> {
    let requested = target.trim();
    if requested.is_empty() {
        anyhow::bail!("One-launch profile override cannot be empty");
    }

    let store = load_store_read_only()?;
    let account = find_account(&store, requested)?
        .cloned()
        .ok_or_else(|| anyhow!("Account not found: {requested}"))?;
    let files = validate_materialized_account_files(&account)?;

    Ok(ResolvedLaunchProfile {
        requested: requested.to_string(),
        account,
        files,
    })
}
