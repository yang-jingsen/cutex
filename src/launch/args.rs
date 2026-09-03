//! Launch-time CLI argument policy.

use crate::launch::program::{codex_program, program_name};
use crate::profiles::model::{CliKind, RuntimeConfig, StoredAccount};

pub fn combined_profile_cli_args(account: &StoredAccount, codex_args: Vec<String>) -> Vec<String> {
    let mut effective_args = account.default_cli_args.clone();
    effective_args.extend(codex_args);
    effective_args
}

pub fn codex_args_for_runtime(account: &StoredAccount, mut codex_args: Vec<String>) -> Vec<String> {
    if should_add_docker_sandbox_bypass(account, &codex_args) {
        codex_args.insert(0, "danger-full-access".to_string());
        codex_args.insert(0, "--sandbox".to_string());
    }
    codex_args
}

fn program_supports_codex_sandbox_flag() -> bool {
    let program = codex_program();
    matches!(
        program_name(&program),
        "codex" | "cute-codex" | "cutex-codex"
    )
}

pub fn should_add_docker_sandbox_bypass(account: &StoredAccount, codex_args: &[String]) -> bool {
    account.cli_kind == CliKind::Codex
        && matches!(account.runtime, RuntimeConfig::Docker { .. })
        && program_supports_codex_sandbox_flag()
        && !codex_args
            .iter()
            .any(|arg| arg == "--sandbox" || arg.starts_with("--sandbox="))
        && !codex_args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
}
