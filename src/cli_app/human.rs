//! Human commands use existing local Management credentials, with no login ceremony.
use super::management_control_plane::ManagementControlClient;
use anyhow::{ensure, Context};
use cutex::agent_management::AgentActionId;
use cutex::cli::args::HumanCommand;
use cutex::role_revision::CutexSessionId;

pub(super) fn resolve_id(selector: &str) -> anyhow::Result<String> {
    let store = cutex::session::store::load_cutex_session_store()?;
    if store.sessions.contains_key(selector) {
        return Ok(selector.into());
    }
    let matches: Vec<_> = store
        .sessions
        .values()
        .filter(|r| {
            r.codex_session_id.as_deref() == Some(selector)
                || r.formal_agent_name.as_deref() == Some(selector)
                || r.display_name_hint.as_deref() == Some(selector)
        })
        .collect();
    ensure!(
        matches.len() == 1,
        "agent name/ID is missing or ambiguous; use its full cutex session ID"
    );
    Ok(matches[0].cutex_session_id.clone())
}

pub(super) fn recover(
    id: &str,
    action_id: Option<String>,
) -> anyhow::Result<cutex::agent_management::HumanRuntimeRecovery> {
    let id =
        CutexSessionId::new(resolve_id(id)?).map_err(|_| anyhow::anyhow!("invalid agent ID"))?;
    let action = AgentActionId::new(
        action_id.unwrap_or_else(|| format!("human-recovery-{}", uuid::Uuid::new_v4())),
    )?;
    ManagementControlClient::connect()?.recover_runtime(id, action)
}

pub(super) fn run_command(command: HumanCommand) -> anyhow::Result<()> {
    match command {
        HumanCommand::Action { action_id } => {
            let result = ManagementControlClient::connect()?
                .runtime_action_status(AgentActionId::new(action_id)?)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        HumanCommand::Recover { id, action_id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&recover(&id, action_id)?)?
            );
        }
        HumanCommand::Attach { id } => super::stock_lifecycle::attach(&resolve_id(&id)?)?,
        HumanCommand::Start { id } => {
            let id = resolve_id(&id)?;
            let store = cutex::session::store::load_cutex_session_store()?;
            let record = store.sessions.get(&id).context("agent disappeared")?;
            ensure!(
                record.explicit_launch.is_some(),
                "this agent uses the legacy launcher; use session online until upgraded"
            );
            let action = super::stock_lifecycle::ReviewedStockRuntimeAction::review(&id, false)?;
            eprintln!("Action: {}", action.action_id.as_str());
            let receipt = action.execute()?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        HumanCommand::Session { command } => super::session::run_command(command)?,
        HumanCommand::Management { command } => super::management::run_command(command)?,
    }
    Ok(())
}
