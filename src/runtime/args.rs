//! Runtime default and override argument policy for Codex session resumes.

use std::collections::HashSet;

use crate::session::model::{normalize_runtime_token, CutexSessionRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CodexCliOverrideKey {
    Model,
    Sandbox,
    ApprovalPolicy,
    ReasoningEffort,
    Cwd,
}

pub fn append_codex_cli_args_with_overrides(
    base: Vec<String>,
    overrides: Vec<String>,
) -> Vec<String> {
    let override_keys = codex_cli_override_keys(&overrides);
    if override_keys.is_empty() {
        let mut merged = base;
        merged.extend(overrides);
        return merged;
    }
    let mut merged = strip_codex_cli_args_for_override_keys(base, &override_keys);
    merged.extend(overrides);
    merged
}

fn codex_cli_override_keys(args: &[String]) -> HashSet<CodexCliOverrideKey> {
    let mut keys = HashSet::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--model" | "-m" => {
                keys.insert(CodexCliOverrideKey::Model);
                index += 2;
                continue;
            }
            "--sandbox" | "-s" => {
                keys.insert(CodexCliOverrideKey::Sandbox);
                index += 2;
                continue;
            }
            "--ask-for-approval" | "-a" => {
                keys.insert(CodexCliOverrideKey::ApprovalPolicy);
                index += 2;
                continue;
            }
            "--cd" | "-C" => {
                keys.insert(CodexCliOverrideKey::Cwd);
                index += 2;
                continue;
            }
            "-c" | "--config"
                if args
                    .get(index + 1)
                    .is_some_and(|value| is_reasoning_effort_config_arg(value)) =>
            {
                keys.insert(CodexCliOverrideKey::ReasoningEffort);
                index += 2;
                continue;
            }
            _ => {}
        }

        if arg
            .strip_prefix("--model=")
            .or_else(|| arg.strip_prefix("-m="))
            .is_some()
        {
            keys.insert(CodexCliOverrideKey::Model);
        } else if arg
            .strip_prefix("--sandbox=")
            .or_else(|| arg.strip_prefix("-s="))
            .is_some()
        {
            keys.insert(CodexCliOverrideKey::Sandbox);
        } else if arg
            .strip_prefix("--ask-for-approval=")
            .or_else(|| arg.strip_prefix("-a="))
            .is_some()
        {
            keys.insert(CodexCliOverrideKey::ApprovalPolicy);
        } else if arg
            .strip_prefix("--cd=")
            .or_else(|| arg.strip_prefix("-C="))
            .is_some()
        {
            keys.insert(CodexCliOverrideKey::Cwd);
        } else if arg
            .strip_prefix("--config=")
            .is_some_and(is_reasoning_effort_config_arg)
        {
            keys.insert(CodexCliOverrideKey::ReasoningEffort);
        }
        index += 1;
    }
    keys
}

fn strip_codex_cli_args_for_override_keys(
    args: Vec<String>,
    keys: &HashSet<CodexCliOverrideKey>,
) -> Vec<String> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let pair_key = match arg {
            "--model" | "-m" => Some(CodexCliOverrideKey::Model),
            "--sandbox" | "-s" => Some(CodexCliOverrideKey::Sandbox),
            "--ask-for-approval" | "-a" => Some(CodexCliOverrideKey::ApprovalPolicy),
            "--cd" | "-C" => Some(CodexCliOverrideKey::Cwd),
            "-c" | "--config"
                if args
                    .get(index + 1)
                    .is_some_and(|value| is_reasoning_effort_config_arg(value)) =>
            {
                Some(CodexCliOverrideKey::ReasoningEffort)
            }
            _ => None,
        };
        if pair_key.is_some_and(|key| keys.contains(&key)) {
            index += 2;
            continue;
        }

        let inline_key = if arg.starts_with("--model=") || arg.starts_with("-m=") {
            Some(CodexCliOverrideKey::Model)
        } else if arg.starts_with("--sandbox=") || arg.starts_with("-s=") {
            Some(CodexCliOverrideKey::Sandbox)
        } else if arg.starts_with("--ask-for-approval=") || arg.starts_with("-a=") {
            Some(CodexCliOverrideKey::ApprovalPolicy)
        } else if arg.starts_with("--cd=") || arg.starts_with("-C=") {
            Some(CodexCliOverrideKey::Cwd)
        } else if arg
            .strip_prefix("--config=")
            .is_some_and(is_reasoning_effort_config_arg)
        {
            Some(CodexCliOverrideKey::ReasoningEffort)
        } else {
            None
        };
        if inline_key.is_some_and(|key| keys.contains(&key)) {
            index += 1;
            continue;
        }

        filtered.push(args[index].clone());
        index += 1;
    }
    filtered
}

fn is_reasoning_effort_config_arg(value: &str) -> bool {
    value
        .trim()
        .strip_prefix("model_reasoning_effort")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

pub fn cutex_session_runtime_default_cli_args(record: &CutexSessionRecord) -> Vec<String> {
    let (sandbox_mode, approval_policy) = effective_runtime_permission_defaults(record);
    let mut args = Vec::new();
    if let Some(model) = record
        .model_defaults
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(sandbox) = sandbox_mode {
        args.push("--sandbox".to_string());
        args.push(sandbox);
    }
    if let Some(approval) = approval_policy {
        args.push("--ask-for-approval".to_string());
        args.push(approval);
    }
    if let Some(reasoning) = record
        .reasoning_defaults
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push("-c".to_string());
        args.push(format!("model_reasoning_effort={reasoning}"));
    }
    args
}

pub fn effective_runtime_permission_defaults(
    record: &CutexSessionRecord,
) -> (Option<String>, Option<String>) {
    let mut sandbox_mode = record.sandbox_mode.clone();
    let mut approval_policy = record.approval_policy.clone();
    if let Some(permission) = record.permission_defaults.as_deref() {
        match normalize_runtime_token(permission).as_str() {
            "full-access" | "danger-full-access" | ":danger-full-access" | "danger" => {
                if sandbox_mode.is_none() {
                    sandbox_mode = Some("danger-full-access".to_string());
                }
                if approval_policy.is_none() {
                    approval_policy = Some("never".to_string());
                }
            }
            "workspace" | "workspace-write" | ":workspace" | "ask-for-approval" => {
                if sandbox_mode.is_none() {
                    sandbox_mode = Some("workspace-write".to_string());
                }
            }
            "read-only" | ":read-only" | "readonly" | "read" => {
                if sandbox_mode.is_none() {
                    sandbox_mode = Some("read-only".to_string());
                }
            }
            _ => {}
        }
    }
    (sandbox_mode, approval_policy)
}
