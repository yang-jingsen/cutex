//! Runtime lifecycle planning for durable `cutex_session` records.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::process::{Child, Stdio};

use anyhow::Context;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::agent_bus::identity::default_agent_group_for;
use crate::config::env::CUTEX_HEADLESS_AGENT_RUNTIME_ENV_VAR;
use crate::launch::args::codex_args_for_runtime;
use crate::launch::command::LaunchCommand;
use crate::profiles::model::{CliKind, RuntimeConfig, StoredAccount};
use crate::runtime::alden::{cute_alden_program, wrap_launch_with_cute_alden_server_only};
pub use crate::runtime::args::{
    append_codex_cli_args_with_overrides, cutex_session_runtime_default_cli_args,
    effective_runtime_permission_defaults,
};
pub use crate::runtime::codex_home::codex_session_exists_in_home;
pub use crate::runtime::session_online::{
    default_cutex_alden_session_name, session_online_agent_id, session_online_agent_identity_env,
    session_online_agent_identity_env_with_id, session_online_log_path, session_online_log_tail,
    session_online_terminal_color_env,
};
pub use crate::runtime::stop::{session_runtime_stop_target, SessionRuntimeStopTarget};
use crate::session::model::{CutexSessionRecord, CutexSessionRuntimeBackend};
use crate::session::service::cutex_session_launch_cwd;

#[derive(Debug, Clone)]
pub struct SessionOnlineResumePlan {
    pub effective_args: Vec<String>,
    pub groups: Vec<String>,
    pub codex_session_id: String,
    pub launch_cwd: String,
}

#[derive(Debug, Clone)]
pub struct LiveRemoteTuiAttachPlan {
    pub effective_args: Vec<String>,
    pub codex_session_id: String,
    pub launch_cwd: String,
}

#[derive(Debug, Clone)]
pub struct SessionOnlineLaunch {
    pub launch: LaunchCommand,
    pub backend: CutexSessionRuntimeBackend,
    pub alden_session_name: Option<String>,
    pub cwd: String,
}

pub fn cutex_session_host_is_local(session_host_id: &str, current_host: &str) -> bool {
    let host = session_host_id.trim();
    host.is_empty()
        || host.eq_ignore_ascii_case(current_host)
        || (host.eq_ignore_ascii_case("localhost") && current_host != "unknown")
}

pub fn session_online_resume_plan(
    record: &CutexSessionRecord,
    account: &StoredAccount,
) -> anyhow::Result<SessionOnlineResumePlan> {
    let mut codex_args = session_online_base_codex_args(record, account)?;
    let codex_session_id = record
        .codex_session_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cutex session has no Codex session id"))?;
    codex_args.push("resume".to_string());
    codex_args.push("--cwd-policy".to_string());
    codex_args.push("current".to_string());
    codex_args.push(codex_session_id.to_string());
    let effective_args = codex_args_for_runtime(account, codex_args);
    let groups = session_online_agent_groups(record);

    Ok(SessionOnlineResumePlan {
        effective_args,
        groups,
        codex_session_id: codex_session_id.to_string(),
        launch_cwd: cutex_session_launch_cwd(record).to_string(),
    })
}

pub fn session_online_base_codex_args(
    record: &CutexSessionRecord,
    account: &StoredAccount,
) -> anyhow::Result<Vec<String>> {
    let codex_args = session_runtime_base_codex_args(record, account)?;
    let codex_session_id = required_codex_session_id(record)?;
    if !codex_session_exists_in_home(codex_session_id)? {
        anyhow::bail!("codex_session_not_found: {codex_session_id}");
    }
    Ok(codex_args)
}

pub fn live_remote_tui_attach_plan(
    record: &CutexSessionRecord,
    account: &StoredAccount,
) -> anyhow::Result<LiveRemoteTuiAttachPlan> {
    let mut codex_args = session_runtime_base_codex_args(record, account)?;
    let codex_session_id = required_codex_session_id(record)?;
    codex_args.push("resume".to_string());
    codex_args.push("--cwd-policy".to_string());
    codex_args.push("current".to_string());
    codex_args.push(codex_session_id.to_string());
    Ok(LiveRemoteTuiAttachPlan {
        effective_args: codex_args_for_runtime(account, codex_args),
        codex_session_id: codex_session_id.to_string(),
        launch_cwd: cutex_session_launch_cwd(record).to_string(),
    })
}

fn required_codex_session_id(record: &CutexSessionRecord) -> anyhow::Result<&str> {
    record
        .codex_session_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("cutex session has no Codex session id"))
}

fn session_runtime_base_codex_args(
    record: &CutexSessionRecord,
    account: &StoredAccount,
) -> anyhow::Result<Vec<String>> {
    if account.cli_kind != CliKind::Codex {
        anyhow::bail!("session.online currently supports Codex profiles only");
    }
    if !matches!(account.runtime, RuntimeConfig::Host) {
        anyhow::bail!("session.online currently supports host runtime only");
    }
    let launch_cwd = cutex_session_launch_cwd(record).to_string();
    let mut codex_args = account.default_cli_args.clone();
    codex_args = append_codex_cli_args_with_overrides(
        codex_args,
        cutex_session_runtime_default_cli_args(record),
    );
    codex_args = append_codex_cli_args_with_overrides(codex_args, record.default_cli_args.clone());
    codex_args = append_codex_cli_args_with_overrides(
        codex_args,
        vec!["--cd".to_string(), launch_cwd.clone()],
    );
    Ok(codex_args)
}

/// Base app-server launch arguments for a durable Cutex session whose native
/// thread will be created through `thread/start` after the process is ready.
pub fn session_new_thread_base_codex_args(
    record: &CutexSessionRecord,
    account: &StoredAccount,
) -> anyhow::Result<Vec<String>> {
    if account.cli_kind != CliKind::Codex {
        anyhow::bail!("new-thread launch currently supports Codex profiles only");
    }
    if !matches!(account.runtime, RuntimeConfig::Host) {
        anyhow::bail!("new-thread launch currently supports host runtime only");
    }
    if record.codex_session_id.is_some() {
        anyhow::bail!("new-thread launch record already has a Codex session id");
    }
    let launch_cwd = cutex_session_launch_cwd(record).to_string();
    let mut codex_args = account.default_cli_args.clone();
    codex_args = append_codex_cli_args_with_overrides(
        codex_args,
        cutex_session_runtime_default_cli_args(record),
    );
    codex_args = append_codex_cli_args_with_overrides(codex_args, record.default_cli_args.clone());
    reject_conversation_targeting_args(&codex_args)?;
    codex_args =
        append_codex_cli_args_with_overrides(codex_args, vec!["--cd".to_string(), launch_cwd]);
    Ok(codex_args)
}

fn reject_conversation_targeting_args(args: &[String]) -> anyhow::Result<()> {
    if args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "resume" | "fork" | "--thread-id" | "--session-id" | "--conversation-id"
        ) || arg.starts_with("--thread-id=")
            || arg.starts_with("--session-id=")
            || arg.starts_with("--conversation-id=")
    }) {
        anyhow::bail!("new-thread launch defaults may not target an existing conversation");
    }
    Ok(())
}

pub fn session_online_agent_groups(record: &CutexSessionRecord) -> Vec<String> {
    if record.agent_groups.is_empty() {
        vec![default_agent_group_for(
            None,
            cutex_session_launch_cwd(record),
        )]
    } else {
        record.agent_groups.clone()
    }
}

pub fn finalize_session_online_launch(
    record: &CutexSessionRecord,
    account: &StoredAccount,
    base_launch: LaunchCommand,
    resume_plan: &SessionOnlineResumePlan,
) -> anyhow::Result<SessionOnlineLaunch> {
    let launch = session_online_agent_identity_env(
        session_online_terminal_color_env(base_launch),
        account,
        record,
        &resume_plan.groups,
    );
    match record.runtime_backend {
        CutexSessionRuntimeBackend::Host => Ok(SessionOnlineLaunch {
            launch: launch.env(CUTEX_HEADLESS_AGENT_RUNTIME_ENV_VAR, "1"),
            backend: CutexSessionRuntimeBackend::Host,
            alden_session_name: None,
            cwd: resume_plan.launch_cwd.clone(),
        }),
        CutexSessionRuntimeBackend::CuteAlden => {
            let alden_program = cute_alden_program()?;
            let alden_session_name = record
                .alden_session_name
                .clone()
                .unwrap_or_else(|| default_cutex_alden_session_name(record));
            Ok(SessionOnlineLaunch {
                launch: wrap_launch_with_cute_alden_server_only(
                    launch,
                    &alden_program,
                    &alden_session_name,
                    &resume_plan.launch_cwd,
                ),
                backend: CutexSessionRuntimeBackend::CuteAlden,
                alden_session_name: Some(alden_session_name),
                cwd: resume_plan.launch_cwd.clone(),
            })
        }
        CutexSessionRuntimeBackend::HostForeground => {
            anyhow::bail!(
                "session.online requires a visible terminal for runtime_backend=host_foreground; run `cutex session foreground {}` from the local terminal",
                resume_plan.codex_session_id
            )
        }
        CutexSessionRuntimeBackend::Docker | CutexSessionRuntimeBackend::Future => {
            anyhow::bail!(
                "session.online does not support runtime_backend={:?}",
                record.runtime_backend
            )
        }
    }
}

pub fn spawn_detached_session_launch(
    launch: &LaunchCommand,
    cwd: &str,
    log_path: &Path,
) -> anyhow::Result<Child> {
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("Failed to open session runtime log: {}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .context("Failed to clone session runtime log")?;
    let mut command = launch.to_command();
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("Failed to start session runtime: {}", launch.program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn host_codex_account() -> StoredAccount {
        StoredAccount {
            id: "account-live-remote-tui".to_string(),
            name: "live-remote-tui".to_string(),
            email: None,
            plan_type: None,
            source: None,
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: vec!["--model".to_string(), "profile-model".to_string()],
            agent_name: None,
            last_used_at: None,
        }
    }

    #[test]
    fn fresh_live_remote_attach_skips_rollout_but_offline_resume_still_requires_it() {
        let thread_id = format!("fresh-live-no-rollout-{}", Uuid::new_v4());
        assert!(!codex_session_exists_in_home(&thread_id).expect("rollout lookup"));
        let mut record = CutexSessionRecord::new_at(
            "cutex.fresh-live".to_string(),
            Some(thread_id.clone()),
            "host".to_string(),
            "/tmp/fresh-live".to_string(),
            Some("live-remote-tui".to_string()),
            "2026-08-27T00:00:00Z".to_string(),
        )
        .expect("session record");
        record.default_cli_args = vec!["--no-alt-screen".to_string()];
        let account = host_codex_account();

        let plan = live_remote_tui_attach_plan(&record, &account)
            .expect("live remote attach does not need a rollout");
        assert_eq!(plan.codex_session_id, thread_id);
        assert_eq!(plan.launch_cwd, "/tmp/fresh-live");
        assert!(plan
            .effective_args
            .windows(2)
            .any(|args| args == ["--model", "profile-model"]));
        assert!(plan
            .effective_args
            .windows(2)
            .any(|args| args == ["--cd", "/tmp/fresh-live"]));
        assert!(plan
            .effective_args
            .iter()
            .any(|arg| arg == "--no-alt-screen"));
        assert!(plan.effective_args.windows(4).any(|args| args
            == [
                "resume",
                "--cwd-policy",
                "current",
                plan.codex_session_id.as_str()
            ]));

        let error = session_online_resume_plan(&record, &account)
            .expect_err("ordinary offline resume must still require a rollout");
        assert_eq!(
            error.to_string(),
            format!("codex_session_not_found: {}", plan.codex_session_id)
        );
    }

    #[test]
    fn new_thread_defaults_reject_existing_conversation_selectors() {
        for selector in [
            "resume",
            "fork",
            "--thread-id",
            "--session-id=thread-old",
            "--conversation-id=thread-old",
        ] {
            assert!(reject_conversation_targeting_args(&[selector.to_string()]).is_err());
        }
        assert!(reject_conversation_targeting_args(&[
            "--model".to_string(),
            "gpt-test".to_string(),
            "--no-alt-screen".to_string(),
        ])
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn detached_session_launch_uses_cwd_and_appends_output_to_log() {
        let temp_dir =
            std::env::temp_dir().join(format!("cutex-detached-launch-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let log_path = temp_dir.join("runtime.log");
        let launch = LaunchCommand::new("sh").args([
            "-c",
            "printf '%s\\n' \"$PWD\"; printf '%s\\n' runtime-stderr >&2",
        ]);

        let mut child = spawn_detached_session_launch(
            &launch,
            temp_dir.to_str().expect("temp path should be utf-8"),
            &log_path,
        )
        .expect("launch should spawn");
        let status = child.wait().expect("child should be waitable");

        assert!(status.success(), "child exited with {status}");
        let log = fs::read_to_string(&log_path).expect("runtime log should exist");
        assert!(log.contains(temp_dir.to_string_lossy().as_ref()));
        assert!(log.contains("runtime-stderr"));
        fs::remove_dir_all(&temp_dir).expect("temp dir should be removed");
    }
}
