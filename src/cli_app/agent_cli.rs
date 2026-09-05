use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::Command;

use anyhow::Context;
use serde_json::Value;

use cutex::agent_bus::audit::{
    agent_audit_record_matches, agent_bus_audit_log_path, print_agent_audit_record,
};
use cutex::agent_bus::client::{
    agent_bus_fetch_agents_scoped_with_hosts, agent_bus_fetch_task_service_worker_context,
    agent_bus_healthy, agent_bus_prepare_task_service_worker_action, agent_bus_send_agent_message,
    agent_bus_submit_agent_management, agent_bus_submit_release_rotation,
    agent_bus_submit_task_service_coordinator_action,
    agent_bus_submit_task_service_director_action, agent_bus_submit_task_service_query,
    agent_bus_submit_task_service_terminal_action, agent_bus_submit_task_service_worker_action,
    agent_bus_submit_task_worker_action, agent_bus_submit_task_worker_reconciliation,
    agent_bus_update_agent_groups,
};
use cutex::agent_bus::delivery::{
    agent_delivery_mode_from_flags, legacy_delivery_mode_label, AgentDeliveryMode,
};
use cutex::agent_bus::identity::normalize_agent_groups;
use cutex::agent_bus::model::AgentGroupUpdateMode;
use cutex::agent_bus::model::{
    TaskWorkerActionRequest, TaskWorkerReconciliationRequest, TASK_WORKER_ACTION_MAX_BODY_BYTES,
};
use cutex::agent_bus::routing::groups_overlap;
use cutex::agent_bus::service::{
    agent_bus_health_url, agent_bus_port, validate_agent_bus_port, AGENT_BUS_BRIDGE_ID,
    DEFAULT_AGENT_BUS_PORT,
};
use cutex::agent_management::{
    AgentManagementRequest, AgentOperationKind, AGENT_MANAGEMENT_MAX_BODY_BYTES,
};
use cutex::cli::args::{AgentCommand, AgentGroupsCommand, AgentManagementCliCommand};
use cutex::config::env::CUTEX_AGENT_ID_ENV_VAR;
#[cfg(windows)]
use cutex::config::env::{env_bool_override, CUTEX_WINDOWS_DESKTOP_LAUNCHER_ENV_VAR};
use cutex::config::store::load_codez_config;
use cutex::platform::command::command_exists_in_path;
use cutex::platform::now_epoch_secs;
use cutex::ui::format::bool_label;

use super::agent_bus_config;
use super::agent_bus_runtime;
use super::agent_bus_server;

pub(crate) fn run_command(command: AgentCommand) -> anyhow::Result<()> {
    match command {
        AgentCommand::List {
            group,
            all_groups,
            all_hosts,
        } => cmd_agent_list(group, all_groups, all_hosts),
        AgentCommand::Send {
            target,
            message,
            external_message_id,
            all_groups,
            queue_only,
            soon,
            interrupt,
            from,
        } => {
            let delivery_mode = agent_delivery_mode_from_flags(queue_only, soon, interrupt)?;
            cmd_agent_send(
                &target,
                &message,
                delivery_mode,
                all_groups,
                from.as_deref(),
                external_message_id.as_deref(),
            )
        }
        AgentCommand::TaskAction { request_file } => cmd_agent_task_action(request_file.as_deref()),
        AgentCommand::ReleaseRotation { request_file } => {
            cmd_agent_release_rotation(request_file.as_deref())
        }
        AgentCommand::Manage { command } => cmd_agent_management(command),
        AgentCommand::Status => cmd_agent_status(),
        AgentCommand::Log { agent, limit, json } => cmd_agent_log(agent.as_deref(), limit, json),
        AgentCommand::Groups { command } => cmd_agent_groups(command),
        AgentCommand::RemoteUp {
            host,
            service_id,
            local_port,
            remote_port,
            token,
            show_ssh_fallback,
            no_config,
        } => cmd_agent_remote_up(
            &host,
            service_id.as_deref(),
            local_port,
            remote_port,
            token.as_deref(),
            show_ssh_fallback,
            no_config,
        ),
        AgentCommand::Serve { port, token } => {
            arm_windows_hosted_launcher_guard()?;
            agent_bus_server::cmd_agent_serve(port, token, agent_bus_server::request_handlers())
        }
    }
}

fn arm_windows_hosted_launcher_guard() -> anyhow::Result<()> {
    #[cfg(windows)]
    if env_bool_override(CUTEX_WINDOWS_DESKTOP_LAUNCHER_ENV_VAR) == Some(true) {
        cutex::platform::windows_parent_guard::arm_launcher_exit_guard()?;
    }
    Ok(())
}

fn cmd_agent_management(command: AgentManagementCliCommand) -> anyhow::Result<()> {
    let (expected, request_file) = match command {
        AgentManagementCliCommand::Create { request_file } => {
            (AgentOperationKind::Create, request_file)
        }
        AgentManagementCliCommand::QueryManaged { request_file } => {
            (AgentOperationKind::QueryManaged, request_file)
        }
        AgentManagementCliCommand::Online { request_file } => {
            (AgentOperationKind::Online, request_file)
        }
        AgentManagementCliCommand::Offline { request_file } => {
            (AgentOperationKind::Offline, request_file)
        }
        AgentManagementCliCommand::Restart { request_file } => {
            (AgentOperationKind::Restart, request_file)
        }
        AgentManagementCliCommand::Close { request_file } => {
            (AgentOperationKind::Close, request_file)
        }
        AgentManagementCliCommand::Replace { request_file } => {
            (AgentOperationKind::Replace, request_file)
        }
        AgentManagementCliCommand::GrantOperator { request_file } => {
            (AgentOperationKind::GrantOperator, request_file)
        }
        AgentManagementCliCommand::RevokeOperator { request_file } => {
            (AgentOperationKind::RevokeOperator, request_file)
        }
        AgentManagementCliCommand::DirectorRotate { request_file } => {
            (AgentOperationKind::DirectorRotate, request_file)
        }
    };
    require_private_action_file(Path::new(&request_file), "Agent Management")?;
    let bytes = fs::read(&request_file)
        .with_context(|| format!("Failed to read Agent Management request: {request_file}"))?;
    if bytes.len() > AGENT_MANAGEMENT_MAX_BODY_BYTES {
        anyhow::bail!("Agent Management request exceeds the local route size limit");
    }
    let request: AgentManagementRequest = serde_json::from_slice(&bytes)
        .context("Failed to parse strict Agent Management request")?;
    if request.operation.kind() != expected {
        anyhow::bail!(
            "Agent Management request operation does not match the selected CLI subcommand"
        );
    }
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, false)?;
    let response = agent_bus_submit_agent_management(&config, &request)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_agent_task_action(request_file: Option<&str>) -> anyhow::Result<()> {
    let bytes = match request_file {
        None | Some("-") => read_bounded_task_action(std::io::stdin().lock())?,
        Some(path) => {
            require_private_action_file(Path::new(path), "task-action")?;
            let file = fs::File::open(path)
                .with_context(|| format!("Failed to open task-action request file: {path}"))?;
            read_bounded_task_action(file)?
        }
    };
    let value: Value = serde_json::from_slice(&bytes)
        .context("Failed to parse strict task-action request JSON")?;
    let schema = task_action_document_schema(&value)
        .context("task-action document requires a typed schema")?;
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, false)?;
    let response = match schema {
        "cutex/task-worker-action/v1" => {
            let request: TaskWorkerActionRequest = serde_json::from_value(value)
                .context("Failed to parse strict worker action document")?;
            serde_json::to_value(agent_bus_submit_task_worker_action(&config, &request)?)?
        }
        "cutex/task-worker-reconciliation/v1" => {
            let request: TaskWorkerReconciliationRequest = serde_json::from_value(value)
                .context("Failed to parse strict worker reconciliation document")?;
            serde_json::to_value(agent_bus_submit_task_worker_reconciliation(
                &config, &request,
            )?)?
        }
        "cutex/task-service-worker-provider/v2" => {
            let request: cutex::task_service::WorkerProviderActionEnvelope =
                serde_json::from_value(value)
                    .context("Failed to parse strict Task Service v2 Worker provider envelope")?;
            serde_json::to_value(agent_bus_submit_task_service_worker_action(
                &config, &request,
            )?)?
        }
        "cutex/task-service-worker-context/v2" => {
            let request: cutex::task_service::WorkerContextRequest = serde_json::from_value(value)
                .context("Failed to parse strict Task Service Worker context request")?;
            serde_json::to_value(agent_bus_fetch_task_service_worker_context(
                &config, &request,
            )?)?
        }
        "cutex/task-service-worker-prepare/v2" => {
            let request: cutex::task_service::WorkerPrepareRequest = serde_json::from_value(value)
                .context("Failed to parse strict Task Service Worker prepare request")?;
            serde_json::to_value(agent_bus_prepare_task_service_worker_action(
                &config, &request,
            )?)?
        }
        "cutex/task-service-coordinator/v2" => {
            let request: cutex::task_service::CoordinatorActionRequest =
                serde_json::from_value(value)
                    .context("Failed to parse strict Task Service coordinator request")?;
            serde_json::to_value(agent_bus_submit_task_service_coordinator_action(
                &config, &request,
            )?)?
        }
        "cutex/task-service-terminal/v2" => {
            let request: cutex::task_service::TerminalActionEnvelope =
                serde_json::from_value(value)
                    .context("Failed to parse strict Task Service terminal request")?;
            serde_json::to_value(agent_bus_submit_task_service_terminal_action(
                &config, &request,
            )?)?
        }
        "cutex/task-service-query/v2" => {
            let request: cutex::task_service::TaskServiceQueryRequest =
                serde_json::from_value(value)
                    .context("Failed to parse strict Task Service query request")?;
            serde_json::to_value(agent_bus_submit_task_service_query(&config, &request)?)?
        }
        "cutex/task-service-director-action/v1" | "cutex/task-service-director-action/v2" => {
            let request: cutex::task_service::DirectorActionRequest = serde_json::from_value(value)
                .context("Failed to parse strict Task Service Director action request")?;
            serde_json::to_value(agent_bus_submit_task_service_director_action(
                &config, &request,
            )?)?
        }
        _ => anyhow::bail!("unsupported task-action document schema"),
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_agent_release_rotation(request_file: Option<&str>) -> anyhow::Result<()> {
    let bytes = match request_file {
        None | Some("-") => read_bounded_release_rotation(std::io::stdin().lock())?,
        Some(path) => {
            require_private_action_file(Path::new(path), "Release rotation")?;
            let file = fs::File::open(path)
                .with_context(|| format!("Failed to open Release rotation request: {path}"))?;
            read_bounded_release_rotation(file)?
        }
    };
    let request: cutex::rotation::ReleaseRotationRequest = serde_json::from_slice(&bytes)
        .context("Failed to parse strict Release rotation request JSON")?;
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, false)?;
    let response = agent_bus_submit_release_rotation(&config, &request)?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn read_bounded_release_rotation(mut reader: impl Read) -> anyhow::Result<Vec<u8>> {
    let limit = cutex::rotation::RELEASE_ROTATION_MAX_MESSAGE_BYTES + 16 * 1024;
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        anyhow::bail!("Release rotation request exceeds the local route size limit");
    }
    Ok(bytes)
}

fn task_action_document_schema(value: &Value) -> Option<&str> {
    value
        .get("schema")
        .or_else(|| value.get("body").and_then(|body| body.get("schema")))
        .and_then(Value::as_str)
}

fn read_bounded_task_action(mut reader: impl Read) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take((TASK_WORKER_ACTION_MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > TASK_WORKER_ACTION_MAX_BODY_BYTES {
        anyhow::bail!("task-action request exceeds the local route size limit");
    }
    Ok(bytes)
}

#[cfg(unix)]
fn require_private_action_file(path: &Path, label: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect {label} request file: {}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        anyhow::bail!("{label} request file must be a private owner regular file");
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private_action_file(path: &Path, label: &str) -> anyhow::Result<()> {
    if !path.is_file() {
        anyhow::bail!("{label} request path must be a regular file");
    }
    Ok(())
}

fn cmd_agent_status() -> anyhow::Result<()> {
    let config = load_codez_config();
    let port = agent_bus_port(&config);
    let healthy = agent_bus_healthy(port, config.agent_bus_token.as_deref());
    if healthy {
        agent_bus_runtime::register_agent_bus_handoff(port);
    }
    println!("\x1b[1m\x1b[36mcutex agent bus\x1b[0m");
    println!(
        "\x1b[2menabled\x1b[0m {}",
        bool_label(config.agent_bus_enabled)
    );
    println!("\x1b[2mport\x1b[0m {port}");
    println!(
        "\x1b[2mtoken\x1b[0m {}",
        if config
            .agent_bus_token
            .as_ref()
            .is_some_and(|token| !token.is_empty())
        {
            "(set)"
        } else {
            "-"
        }
    );
    println!(
        "\x1b[2mhealth\x1b[0m {}",
        if healthy { "healthy" } else { "not running" }
    );
    Ok(())
}

fn cmd_agent_remote_up(
    host: &str,
    service_id: Option<&str>,
    local_port: Option<u16>,
    remote_port: Option<u16>,
    token: Option<&str>,
    show_ssh_fallback: bool,
    no_config: bool,
) -> anyhow::Result<()> {
    let host = host.trim();
    if host.is_empty() {
        anyhow::bail!("Remote host cannot be empty");
    }
    let remote_port = remote_port.unwrap_or(DEFAULT_AGENT_BUS_PORT);
    validate_agent_bus_port(remote_port)?;
    let Some(local_port) = local_port else {
        anyhow::bail!(
            "agent remote-up is a legacy tunnel command and now requires explicit --local-port; normal federated agent bus routing keeps this host on port {DEFAULT_AGENT_BUS_PORT}"
        );
    };
    validate_agent_bus_port(local_port)?;
    let service_id = service_id
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| AGENT_BUS_BRIDGE_ID.to_string());

    let fallback = raw_agent_bus_ssh_tunnel_command(host, local_port, remote_port);
    if command_exists_in_path("bridgeboard") {
        let status = Command::new("bridgeboard")
            .arg("up")
            .arg("--peer")
            .arg(host)
            .arg("--local-port")
            .arg(local_port.to_string())
            .arg(&service_id)
            .status()
            .with_context(|| {
                format!(
                    "Failed to run bridgeboard up --peer {host} --local-port {local_port} {service_id}"
                )
            })?;
        if status.success() {
            println!(
                "\x1b[32mBridgeboard tunnel requested\x1b[0m: service=\x1b[1m{service_id}\x1b[0m host=\x1b[1m{host}\x1b[0m local_port=\x1b[1m{local_port}\x1b[0m"
            );
        } else {
            eprintln!(
                "\x1b[33mwarning:\x1b[0m bridgeboard up --peer {host} --local-port {local_port} {service_id} exited with {status}"
            );
            println!("\x1b[2mSSH fallback:\x1b[0m {fallback}");
            anyhow::bail!("Bridgeboard could not bring up the agent bus tunnel");
        }
    } else {
        println!("\x1b[33mbridgeboard not found; use SSH fallback:\x1b[0m {fallback}");
        if !no_config {
            println!(
                "\x1b[2mConfig can still be updated, but the tunnel must be started manually first.\x1b[0m"
            );
        }
    }

    if !no_config {
        let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
        if token.is_some() {
            eprintln!(
                "\x1b[33mwarning:\x1b[0m --token is ignored by federated remote-up; each host keeps its own local bus token"
            );
        }
        println!(
            "\x1b[32mKept local cutex agent bus config\x1b[0m: port=\x1b[1m{}\x1b[0m token={}",
            agent_bus_port(&config),
            if config
                .agent_bus_token
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                "(set)"
            } else {
                "(not set)"
            }
        );
        if agent_bus_healthy(agent_bus_port(&config), config.agent_bus_token.as_deref()) {
            println!(
                "\x1b[32mhealth ok\x1b[0m: {}",
                agent_bus_health_url(agent_bus_port(&config))
            );
        } else {
            eprintln!(
                "\x1b[33mwarning:\x1b[0m local forwarded bus is not healthy yet: {}",
                agent_bus_health_url(agent_bus_port(&config))
            );
        }
    }

    if show_ssh_fallback {
        println!("\x1b[2mSSH fallback:\x1b[0m {fallback}");
    }

    Ok(())
}

fn raw_agent_bus_ssh_tunnel_command(host: &str, local_port: u16, remote_port: u16) -> String {
    format!("ssh -N -L {local_port}:127.0.0.1:{remote_port} {host}")
}

fn cmd_agent_list(group: Vec<String>, all_groups: bool, all_hosts: bool) -> anyhow::Result<()> {
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, true)?;
    let current_agent_id = std::env::var(CUTEX_AGENT_ID_ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let scoped_agent_id = (!all_groups).then(|| current_agent_id.clone()).flatten();
    let effective_all_hosts = all_hosts || current_agent_id.is_some();
    let group_filter = normalize_agent_groups(group);
    let mut agents = agent_bus_fetch_agents_scoped_with_hosts(
        &config,
        scoped_agent_id.as_deref(),
        all_groups,
        effective_all_hosts,
    )?;
    if !group_filter.is_empty() {
        agents.retain(|agent| groups_overlap(&agent.groups, &group_filter));
    }
    if agents.is_empty() {
        println!("\x1b[2mNo cutex agents are registered.\x1b[0m");
        return Ok(());
    }
    println!("\x1b[1m\x1b[36mcutex agents\x1b[0m");
    if let Some(current_agent_id) = current_agent_id.as_deref() {
        if let Some(agent) = agents.iter().find(|agent| agent.id == current_agent_id) {
            println!(
                "\x1b[32mthis agent\x1b[0m {}  \x1b[2mname={} groups={} cwd={}\x1b[0m",
                agent.id,
                agent.name,
                agent.groups.join(","),
                agent.cwd
            );
        } else {
            println!(
                "\x1b[33mthis agent\x1b[0m {}  \x1b[2mnot registered yet\x1b[0m",
                current_agent_id
            );
        }
    }
    for agent in agents {
        let base = agent.base_name.as_deref().unwrap_or("-");
        let path_key = agent.path_key.as_deref().unwrap_or("-");
        let session = agent.session_id.as_deref().unwrap_or("-");
        let groups = if agent.groups.is_empty() {
            "-".to_string()
        } else {
            agent.groups.join(",")
        };
        let this_marker = if current_agent_id.as_deref() == Some(agent.id.as_str()) {
            "  this"
        } else {
            ""
        };
        println!(
            "\x1b[1m{}\x1b[0m\x1b[32m{}\x1b[0m  \x1b[2mbase={} path={} session={} groups={} class={} id={} host={} profile={} pid={} cwd={} last_seen={}s ago\x1b[0m",
            agent.name,
            this_marker,
            base,
            path_key,
            session,
            groups,
            agent.registration_class.label(),
            agent.id,
            agent.host_id.as_deref().unwrap_or("-"),
            agent.profile,
            agent.pid,
            agent.cwd,
            now_epoch_secs().saturating_sub(agent.last_seen_epoch_secs)
        );
    }
    Ok(())
}

fn cmd_agent_send(
    target: &str,
    message: &str,
    delivery_mode: AgentDeliveryMode,
    all_groups: bool,
    from: Option<&str>,
    external_message_id: Option<&str>,
) -> anyhow::Result<()> {
    if message.trim().is_empty() {
        anyhow::bail!("Agent message cannot be empty");
    }
    if external_message_id.is_some_and(|value| value.trim().is_empty()) {
        anyhow::bail!("External message ID cannot be empty or whitespace-only");
    }
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, false)?;
    let response = agent_bus_send_agent_message(
        &config,
        target,
        message,
        delivery_mode,
        all_groups,
        from,
        external_message_id,
    )?;
    let delivered_to = response.to_name.as_deref().unwrap_or(response.to.as_str());
    let delivered_from = response.from.as_deref().unwrap_or("cutex");
    let delivery_mode = response
        .delivery_mode
        .as_ref()
        .map(AgentDeliveryMode::label)
        .unwrap_or_else(|| legacy_delivery_mode_label(response.trigger_turn));
    println!(
        "\x1b[32mSent\x1b[0m message \x1b[1m{}\x1b[0m from \x1b[1m{}\x1b[0m to \x1b[1m{}\x1b[0m ({delivery_mode}, scope={}, queued={}, deduplicated={})",
        response.id,
        delivered_from,
        delivered_to,
        if all_groups {
            "all-groups"
        } else {
            "visible-groups"
        },
        response.queued,
        response.deduplicated
    );
    Ok(())
}

fn cmd_agent_log(agent: Option<&str>, limit: usize, json: bool) -> anyhow::Result<()> {
    let path = agent_bus_audit_log_path()?;
    if !path.exists() {
        println!(
            "\x1b[2mNo cutex agent audit log exists yet: {}\x1b[0m",
            path.display()
        );
        return Ok(());
    }
    let file = fs::File::open(&path)
        .with_context(|| format!("Failed to open agent audit log: {}", path.display()))?;
    let mut records = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(agent) = agent {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if !agent_audit_record_matches(&value, agent) {
                continue;
            }
        }
        records.push(line);
    }
    let start = records.len().saturating_sub(limit.max(1));
    for line in records.into_iter().skip(start) {
        if json {
            println!("{line}");
        } else if let Ok(value) = serde_json::from_str::<Value>(&line) {
            print_agent_audit_record(&value);
        }
    }
    Ok(())
}

fn cmd_agent_groups(command: AgentGroupsCommand) -> anyhow::Result<()> {
    match command {
        AgentGroupsCommand::Set { target, groups } => {
            cmd_agent_groups_update(&target, groups, AgentGroupUpdateMode::Set)
        }
        AgentGroupsCommand::Add { target, groups } => {
            cmd_agent_groups_update(&target, groups, AgentGroupUpdateMode::Add)
        }
        AgentGroupsCommand::Remove { target, groups } => {
            cmd_agent_groups_update(&target, groups, AgentGroupUpdateMode::Remove)
        }
    }
}

fn cmd_agent_groups_update(
    target: &str,
    groups: Vec<String>,
    mode: AgentGroupUpdateMode,
) -> anyhow::Result<()> {
    let groups = normalize_agent_groups(groups);
    if groups.is_empty() {
        anyhow::bail!("At least one non-empty group is required");
    }
    let config = agent_bus_config::ensure_agent_bus_config(true, None)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, false)?;
    let response = agent_bus_update_agent_groups(&config, target, &groups, mode)?;
    let agent_name = response.agent_name.as_deref().unwrap_or(target);
    let groups = if response.groups.is_empty() {
        "-".to_string()
    } else {
        response.groups.join(",")
    };
    println!("\x1b[32mUpdated\x1b[0m groups for \x1b[1m{agent_name}\x1b[0m: {groups}");
    Ok(())
}

#[cfg(test)]
mod task_action_tests {
    use clap::Parser;

    use super::*;
    use cutex::cli::args::{Cli, CommandKind};

    #[test]
    fn task_action_cli_has_only_private_document_or_stdin_surface() {
        let parsed = Cli::try_parse_from([
            "cutex",
            "agent",
            "task-action",
            "--request-file",
            "/tmp/private-action.json",
        ])
        .expect("parse task-action request-file client");
        assert!(matches!(
            parsed.command,
            Some(CommandKind::Agent {
                command: AgentCommand::TaskAction {
                    request_file: Some(path)
                }
            }) if path == "/tmp/private-action.json"
        ));
        let stdin = Cli::try_parse_from(["cutex", "agent", "task-action"])
            .expect("parse task-action stdin client");
        assert!(matches!(
            stdin.command,
            Some(CommandKind::Agent {
                command: AgentCommand::TaskAction { request_file: None }
            })
        ));
        for forbidden in [
            "--from",
            "--sender",
            "--session",
            "--runtime-agent-id",
            "--transport-reference",
        ] {
            assert!(
                Cli::try_parse_from(["cutex", "agent", "task-action", forbidden, "override"])
                    .is_err()
            );
        }
    }

    #[test]
    fn task_action_reader_enforces_route_bound_before_json_or_transport() {
        let exact = vec![b'x'; TASK_WORKER_ACTION_MAX_BODY_BYTES];
        assert_eq!(
            read_bounded_task_action(exact.as_slice()).unwrap().len(),
            TASK_WORKER_ACTION_MAX_BODY_BYTES
        );
        let oversized = vec![b'x'; TASK_WORKER_ACTION_MAX_BODY_BYTES + 1];
        assert!(read_bounded_task_action(oversized.as_slice()).is_err());
    }

    #[test]
    fn release_rotation_cli_is_strict_and_accepts_the_rotation_route_bound() {
        let parsed = Cli::try_parse_from([
            "cutex",
            "agent",
            "release-rotation",
            "--request-file",
            "/tmp/private-rotation.json",
        ])
        .expect("parse Release rotation request-file client");
        assert!(matches!(
            parsed.command,
            Some(CommandKind::Agent {
                command: AgentCommand::ReleaseRotation {
                    request_file: Some(path)
                }
            }) if path == "/tmp/private-rotation.json"
        ));
        assert!(Cli::try_parse_from([
            "cutex",
            "agent",
            "release-rotation",
            "--runtime-agent-id",
            "forged",
        ])
        .is_err());

        let limit = cutex::rotation::RELEASE_ROTATION_MAX_MESSAGE_BYTES + 16 * 1024;
        assert_eq!(
            read_bounded_release_rotation(vec![b'x'; limit].as_slice())
                .expect("exact route bound")
                .len(),
            limit
        );
        assert!(read_bounded_release_rotation(vec![b'x'; limit + 1].as_slice()).is_err());
    }

    #[test]
    fn semantic_worker_schema_remains_mechanical_field_free() {
        let value = serde_json::json!({
            "operation": "start",
            "body": {
                "schema": "cutex/task-service-action/v2",
                "action_id": "start-1",
                "assignment_id": "assignment-1"
            }
        });
        assert_eq!(
            task_action_document_schema(&value),
            Some("cutex/task-service-action/v2")
        );
        let request: cutex::task_service::WorkerActionRequest =
            serde_json::from_value(value).expect("strict v2 Worker action");
        let encoded = serde_json::to_string(&request).unwrap();
        for forbidden in [
            "expected_store_revision",
            "runtime_agent_id",
            "runtime_generation",
            "attempt_token",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn task_action_cli_routes_coordinator_terminal_and_query_envelopes_by_schema() {
        for schema in [
            "cutex/task-service-worker-provider/v2",
            "cutex/task-service-worker-prepare/v2",
            "cutex/task-service-worker-context/v2",
            "cutex/task-service-coordinator/v2",
            "cutex/task-service-terminal/v2",
            "cutex/task-service-query/v2",
            "cutex/task-service-director-action/v1",
            "cutex/task-service-director-action/v2",
        ] {
            let value = serde_json::json!({ "schema": schema });
            assert_eq!(task_action_document_schema(&value), Some(schema));
        }
    }
}
