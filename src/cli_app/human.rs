//! Human commands use existing local Management credentials, with no login ceremony.
use super::management_control_plane::ManagementControlClient;
use anyhow::{ensure, Context};
use cutex::agent_management::AgentActionId;
use cutex::cli::args::{HumanCommand, HumanConfigCommand, HumanTaskCommand};
use cutex::management::control_plane::HumanTaskRecoveryRequest;
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
        HumanCommand::Hosts { command } => {
            use cutex::cli::args::HostsCommand;
            use cutex::management::connections::Hosts;
            match command {
                HostsCommand::Sessions{id,query,cursor} => println!("{}",serde_json::to_string_pretty(&super::remote_sessions::list(&super::remote_sessions::connection(&id)?,&query,cursor.as_deref())?)?),
                HostsCommand::Online{id,session} => println!("{}",serde_json::to_string_pretty(&super::remote_sessions::lifecycle(&super::remote_sessions::connection(&id)?,&session,false)?)?),
                HostsCommand::Close{id,session} => println!("{}",serde_json::to_string_pretty(&super::remote_sessions::lifecycle(&super::remote_sessions::connection(&id)?,&session,true)?)?),
                HostsCommand::Foreground{id,session} => {super::remote_sessions::foreground(&super::remote_sessions::connection(&id)?,&session)?;},
                HostsCommand::List => println!("{}", serde_json::to_string_pretty(&Hosts::load()?)?),
                HostsCommand::Configure { file } => {
                    let hosts: Hosts = serde_json::from_slice(&std::fs::read(file)?)?;
                    hosts.save()?; println!("Saved host connections");
                },
                HostsCommand::Test { id } => {
                    let hosts=Hosts::load()?;
                    let c=hosts.connections.iter().find(|c|c.id==id).context("Connection ID not found")?;
                    c.verified_endpoint()?;println!("{}: connected; host identity {} verified",c.name,c.host_id);
                }
            }
        },
        HumanCommand::Doctor { id } => super::human_diagnostics::run(id.as_deref())?,
        HumanCommand::New { name, cwd } => {
            let cwd = cwd.unwrap_or(std::env::current_dir()?);
            let result = super::light_new::create(&name, &cwd.to_string_lossy())?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        HumanCommand::InstallRuntime {
            bundle_manifest,
            source_home,
            job_descriptor,
        } => {
            use cutex::launch::local_deployment::LocalDeployment;
            let prior = LocalDeployment::selected()?;
            let deployment = LocalDeployment {
                bundle_manifest: bundle_manifest.canonicalize()?,
                native_home: source_home
                    .unwrap_or(cutex::config::paths::host_codex_home_dir()?)
                    .canonicalize()?,
                job_mcp: match job_descriptor {
                    Some(path) => Some(serde_json::from_slice(&std::fs::read(path)?)?),
                    None => prior.and_then(|p| p.job_mcp),
                },
            };
            deployment.install()?;
            println!("{}", serde_json::to_string_pretty(&deployment)?);
        }
        HumanCommand::Config { command } => run_config(command)?,
        HumanCommand::Action { action_id, resume } => {
            let client = ManagementControlClient::connect()?;
            let action_id = AgentActionId::new(action_id)?;
            let result = client.runtime_action_status(action_id.clone())?;
            if resume {
                let receipt = result
                    .receipt
                    .context("no runtime start receipt to resume")?;
                print_runtime_result(client.run_stock_runtime(action_id, receipt.review)?)?;
            } else {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        HumanCommand::Recover { id, action_id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&recover(&id, action_id)?)?
            );
        }
        HumanCommand::Tasks { command } => run_tasks(command)?,
        HumanCommand::Stop {
            id,
            cancel_tasks,
            force,
        } => {
            stop(&resolve_id(&id)?, cancel_tasks, force)?;
        }
        HumanCommand::Restart {
            id,
            cancel_tasks,
            force,
        } => {
            let id = resolve_id(&id)?;
            if cancel_tasks || force {
                stop(&id, cancel_tasks, force)?;
                run_command(HumanCommand::Start { id })?;
            } else {
                print_runtime_result(super::stock_lifecycle::online(&id, true)?)?;
            }
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
            print_runtime_result(super::stock_lifecycle::online(&id, false)?)?;
        }
        HumanCommand::Session { command } => super::session::run_command(command)?,
        HumanCommand::Management { command } => super::management::run_command(command)?,
    }
    Ok(())
}

fn run_tasks(command: HumanTaskCommand) -> anyhow::Result<()> {
    use cutex::task_service::{ActionId, AssignmentId};
    let action = |id: Option<String>| {
        ActionId::new(id.unwrap_or_else(|| format!("human-task-{}", uuid::Uuid::new_v4())))
    };
    let request = match command {
        HumanTaskCommand::List { assignee } => HumanTaskRecoveryRequest::Query {
            assignee: assignee
                .map(|id| {
                    CutexSessionId::new(resolve_id(&id)?)
                        .map_err(|_| anyhow::anyhow!("invalid assignee ID"))
                })
                .transpose()?,
        },
        HumanTaskCommand::Cancel {
            assignment,
            action_id,
        } => HumanTaskRecoveryRequest::Cancel {
            action_id: action(action_id)?,
            assignment_id: AssignmentId::new(assignment)?,
        },
        HumanTaskCommand::Reassign {
            assignment,
            assignee,
            action_id,
        } => {
            let action_id = action(action_id)?;
            // Derive replacement identity from the action so a retry is identical.
            let new_assignment_id =
                AssignmentId::new(format!("{}-assignment", action_id.as_str()))?;
            HumanTaskRecoveryRequest::Reassign {
                action_id,
                assignment_id: AssignmentId::new(assignment)?,
                new_assignment_id,
                assignee: CutexSessionId::new(resolve_id(&assignee)?)
                    .map_err(|_| anyhow::anyhow!("invalid assignee ID"))?,
            }
        }
    };
    match &request {
        HumanTaskRecoveryRequest::Cancel { action_id, .. }
        | HumanTaskRecoveryRequest::Reassign { action_id, .. } => {
            eprintln!("Action: {}", action_id.as_str())
        }
        HumanTaskRecoveryRequest::Query { .. } => {}
    }
    let result = ManagementControlClient::connect()?.human_tasks(&request)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn stop(id: &str, cancel_tasks: bool, force: bool) -> anyhow::Result<()> {
    let client = ManagementControlClient::connect()?;
    let tasks = if cancel_tasks {
        let result = client.human_tasks(&HumanTaskRecoveryRequest::Query {
            assignee: Some(
                CutexSessionId::new(id.to_string())
                    .map_err(|_| anyhow::anyhow!("invalid agent ID"))?,
            ),
        })?;
        result["assignments"]
            .as_array()
            .context("task query omitted assignments")?
            .iter()
            .filter(|a| a["state"] != "closed")
            .map(|a| {
                a["assignment_id"]
                    .as_str()
                    .context("assignment ID missing")
                    .map(str::to_string)
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    if force {
        super::session_runtime::cmd_session_lifecycle_action(id, "session.close", true)?;
    }
    super::session_runtime::cmd_session_close_and_wait_quiet(id)?;
    for assignment in tasks {
        let request = HumanTaskRecoveryRequest::Cancel {
            action_id: cutex::task_service::ActionId::new(format!(
                "human-stop-{}",
                uuid::Uuid::new_v4()
            ))?,
            assignment_id: cutex::task_service::AssignmentId::new(assignment)?,
        };
        client.human_tasks(&request).context("runtime stopped, but task cancellation failed; use cutex human tasks list to inspect remaining work")?;
    }
    println!(
        "{}",
        serde_json::json!({"cutex_session_id":id,"state":"stopped","cancel_tasks":cancel_tasks})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use cutex::cli::args::{Cli, CommandKind};
    #[test]
    fn human_stop_and_task_transfer_parse_without_agent_identity_flags() {
        assert!(matches!(
            Cli::try_parse_from(["cutex", "human", "action", "existing-action", "--resume"])
                .unwrap()
                .command,
            Some(CommandKind::Human {
                command: HumanCommand::Action { resume: true, .. }
            })
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "cutex",
                "human",
                "restart",
                "worker",
                "--cancel-tasks",
                "--force"
            ])
            .unwrap()
            .command,
            Some(CommandKind::Human {
                command: HumanCommand::Restart {
                    cancel_tasks: true,
                    force: true,
                    ..
                }
            })
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "cutex",
                "human",
                "tasks",
                "reassign",
                "old-assignment",
                "new-worker",
                "--action-id",
                "reassign-1"
            ])
            .unwrap()
            .command,
            Some(CommandKind::Human {
                command: HumanCommand::Tasks {
                    command: HumanTaskCommand::Reassign {
                        action_id: Some(_),
                        ..
                    }
                }
            })
        ));
    }
}

fn run_config(command: HumanConfigCommand) -> anyhow::Result<()> {
    use cutex::agent_management::HumanConfigRequest;
    let id = |value: &str| {
        CutexSessionId::new(resolve_id(value)?).map_err(|_| anyhow::anyhow!("invalid agent ID"))
    };
    let action = |value: Option<String>| {
        AgentActionId::new(
            value.unwrap_or_else(|| format!("human-config-{}", uuid::Uuid::new_v4())),
        )
    };
    let request = match command {
        HumanConfigCommand::Show { id: selector } => HumanConfigRequest::Show {
            cutex_session_id: id(&selector)?,
        },
        HumanConfigCommand::Set {
            id: selector,
            fields,
            action_id,
        } => {
            let mut patch = std::collections::BTreeMap::new();
            for field in fields {
                let (key, value) = field
                    .split_once('=')
                    .context("configuration must be KEY=VALUE or KEY=null")?;
                let value = if value == "null" {
                    None
                } else if matches!(key, "cwd" | "bundle_manifest") {
                    Some(
                        std::path::Path::new(value)
                            .canonicalize()?
                            .to_string_lossy()
                            .into_owned(),
                    )
                } else {
                    Some(value.to_string())
                };
                ensure!(
                    patch.insert(key.to_string(), value).is_none(),
                    "duplicate configuration field: {key}"
                );
            }
            HumanConfigRequest::Set {
                cutex_session_id: id(&selector)?,
                action_id: action(action_id)?,
                patch,
            }
        }
        HumanConfigCommand::Undo {
            id: selector,
            original_action_id,
            action_id,
        } => HumanConfigRequest::Undo {
            cutex_session_id: id(&selector)?,
            action_id: action(action_id)?,
            original_action_id: AgentActionId::new(original_action_id)?,
        },
    };
    match &request {
        HumanConfigRequest::Set { action_id, .. } | HumanConfigRequest::Undo { action_id, .. } => {
            eprintln!("Action: {}", action_id.as_str())
        }
        _ => {}
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&ManagementControlClient::connect()?.human_config(&request)?)?
    );
    Ok(())
}

fn print_runtime_result(
    receipt: cutex::agent_management::StockRuntimeReceipt,
) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    ensure!(receipt.stage == cutex::agent_management::StockRuntimeStage::Ready && receipt.error.is_none(),
        "runtime readiness incomplete; inspect with cutex human action {}; after fixing the cause, resume the same action with cutex human action {} --resume",
        receipt.action_id.as_str(),receipt.action_id.as_str());
    Ok(())
}
