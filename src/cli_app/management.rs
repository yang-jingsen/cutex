use std::fs;
use std::net::IpAddr;
use std::process::Command;
use std::process::Stdio;

use anyhow::Context;

use cutex::cli::args::{
    ManagementCommand, ManagementReleaseRotationBoundaryArg, ManagementReleaseRotationCommand,
    ManagementReleaseRotationExternalStepArg, ManagementSeatCommand,
};
use cutex::config::store::load_codez_config;
use cutex::management::remote::{
    ensure_management_remote_tunnel, management_http_json, raw_management_ssh_tunnel_command,
};
use cutex::management::service::{
    management_api_token, management_base_url, management_health_local_url, management_health_url,
    management_root_credential, task_service_seat_credential, validate_management_port,
    DEFAULT_MANAGEMENT_PORT, DEFAULT_MANAGEMENT_REMOTE_TUNNEL_PORT, MANAGEMENT_BRIDGE_ID,
};
use cutex::platform::command::command_exists_in_path;
use cutex::platform::host::current_host_name;

use super::agent_bus_config;
use super::agent_bus_runtime;
use super::app_server_runtime;
use super::management_context;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[2m";

pub(crate) fn run_command(command: ManagementCommand) -> anyhow::Result<()> {
    match command {
        ManagementCommand::Serve { port, bind, token } => cmd_management_serve(port, &bind, token),
        ManagementCommand::RemoteUp {
            host,
            service_id,
            local_port,
            remote_port,
            token,
            show_ssh_fallback,
        } => cmd_management_remote_up(
            &host,
            service_id.as_deref(),
            local_port,
            remote_port,
            token.as_deref(),
            show_ssh_fallback,
        ),
        ManagementCommand::Seat { command } => cmd_management_seat(command),
        ManagementCommand::ReleaseRotation { command } => cmd_management_release_rotation(command),
        ManagementCommand::AgentAuthority {
            request_file,
            port,
            token,
        } => cmd_management_agent_authority(&request_file, port, token.as_deref()),
        ManagementCommand::AgentOwnershipImport {
            request_file,
            port,
            token,
        } => cmd_management_agent_ownership_import(&request_file, port, token.as_deref()),
        ManagementCommand::AgentReservationReconcile {
            request_file,
            port,
            token,
        } => cmd_management_agent_reservation_reconcile(&request_file, port, token.as_deref()),
    }
}

fn cmd_management_agent_reservation_reconcile(
    request_file: &str,
    port: Option<u16>,
    explicit_root_token: Option<&str>,
) -> anyhow::Result<()> {
    let bytes = fs::read(request_file).with_context(|| {
        format!("Failed to read Agent reservation reconciliation request: {request_file}")
    })?;
    let request: cutex::agent_management::AgentReservationReconciliationRequest =
        serde_json::from_slice(&bytes)
            .context("Failed to parse strict Agent reservation reconciliation request")?;
    let body = serde_json::to_vec(&request)?;
    let config = load_codez_config();
    let port = port.unwrap_or(DEFAULT_MANAGEMENT_PORT);
    validate_management_port(port)?;
    let token = management_root_credential(&config, explicit_root_token)?;
    let response = management_http_json(
        &management_base_url(port),
        "POST",
        "/v2/agent-management/reservation-reconciliation",
        Some(token),
        Some(&body),
    )?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_management_agent_ownership_import(
    request_file: &str,
    port: Option<u16>,
    explicit_root_token: Option<&str>,
) -> anyhow::Result<()> {
    let bytes = fs::read(request_file).with_context(|| {
        format!("Failed to read legacy Director ownership import request: {request_file}")
    })?;
    let request: cutex::agent_management::LegacyDirectorOwnershipImportRequest =
        serde_json::from_slice(&bytes)
            .context("Failed to parse strict legacy Director ownership import request")?;
    let body = serde_json::to_vec(&request)?;
    let config = load_codez_config();
    let port = port.unwrap_or(DEFAULT_MANAGEMENT_PORT);
    validate_management_port(port)?;
    let token = management_root_credential(&config, explicit_root_token)?;
    let response = management_http_json(
        &management_base_url(port),
        "POST",
        "/v2/agent-management/legacy-director-ownership-import",
        Some(token),
        Some(&body),
    )?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_management_agent_authority(
    request_file: &str,
    port: Option<u16>,
    explicit_root_token: Option<&str>,
) -> anyhow::Result<()> {
    let bytes = fs::read(request_file)
        .with_context(|| format!("Failed to read project authority request: {request_file}"))?;
    let request: cutex::agent_management::ProjectAuthorityRequest = serde_json::from_slice(&bytes)
        .context("Failed to parse strict project authority request")?;
    let body = serde_json::to_vec(&request)?;
    let config = load_codez_config();
    let port = port.unwrap_or(DEFAULT_MANAGEMENT_PORT);
    validate_management_port(port)?;
    let token = management_root_credential(&config, explicit_root_token)?;
    let response = management_http_json(
        &management_base_url(port),
        "POST",
        "/v2/agent-management/authority",
        Some(token),
        Some(&body),
    )?;
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_management_release_rotation(
    command: ManagementReleaseRotationCommand,
) -> anyhow::Result<()> {
    let config = load_codez_config();
    let (port, explicit_root_token) = match &command {
        ManagementReleaseRotationCommand::TemplateSet { port, token, .. }
        | ManagementReleaseRotationCommand::TemplateQuery { port, token }
        | ManagementReleaseRotationCommand::Query { port, token }
        | ManagementReleaseRotationCommand::Retry { port, token, .. } => {
            (port.unwrap_or(DEFAULT_MANAGEMENT_PORT), token.as_deref())
        }
    };
    validate_management_port(port)?;
    let token = task_service_seat_credential(&config, explicit_root_token)?;
    let base_url = management_base_url(port);
    let response = match command {
        ManagementReleaseRotationCommand::TemplateSet { request_file, .. } => {
            let body = fs::read(&request_file).with_context(|| {
                format!("Failed to read Release template request: {request_file}")
            })?;
            let request: cutex::rotation::ConfigureReleaseTemplateRequest =
                serde_json::from_slice(&body)
                    .context("Failed to parse strict Release template request")?;
            let body = serde_json::to_vec(&request)?;
            management_http_json(
                &base_url,
                "POST",
                "/v2/release-rotation/template",
                Some(&token),
                Some(&body),
            )?
        }
        ManagementReleaseRotationCommand::TemplateQuery { .. } => management_http_json(
            &base_url,
            "GET",
            "/v2/release-rotation/template",
            Some(&token),
            None,
        )?,
        ManagementReleaseRotationCommand::Query { .. } => {
            management_http_json(&base_url, "GET", "/v2/release-rotation", Some(&token), None)?
        }
        ManagementReleaseRotationCommand::Retry {
            action_id,
            expected_request_sha256,
            expected_completed_boundary,
            expected_pending_external_step,
            corrected_successor_cutex_session,
            corrected_successor_thread_id,
            ..
        } => {
            let request = cutex::rotation::RetryReleaseRotationRequest {
                schema: cutex::rotation::ReleaseRotationCommandSchema::V1,
                action_id: cutex::task_service::ActionId::new(action_id)?,
                expected_request_sha256: cutex::role_revision::Sha256::new(expected_request_sha256)
                    .map_err(|_| anyhow::anyhow!("invalid expected request SHA-256"))?,
                expected_completed_boundary: match expected_completed_boundary {
                    ManagementReleaseRotationBoundaryArg::SeatRevoked => {
                        cutex::rotation::ReleaseRotationBoundary::SeatRevoked
                    }
                    ManagementReleaseRotationBoundaryArg::PredecessorOfflined => {
                        cutex::rotation::ReleaseRotationBoundary::PredecessorOfflined
                    }
                    ManagementReleaseRotationBoundaryArg::PredecessorRetired => {
                        cutex::rotation::ReleaseRotationBoundary::PredecessorRetired
                    }
                    ManagementReleaseRotationBoundaryArg::SuccessorSessionCreated => {
                        cutex::rotation::ReleaseRotationBoundary::SuccessorSessionCreated
                    }
                    ManagementReleaseRotationBoundaryArg::SuccessorThreadStarted => {
                        cutex::rotation::ReleaseRotationBoundary::SuccessorThreadStarted
                    }
                    ManagementReleaseRotationBoundaryArg::SuccessorRuntimeOnline => {
                        cutex::rotation::ReleaseRotationBoundary::SuccessorRuntimeOnline
                    }
                    ManagementReleaseRotationBoundaryArg::SuccessorBound => {
                        cutex::rotation::ReleaseRotationBoundary::SuccessorBound
                    }
                    ManagementReleaseRotationBoundaryArg::DirectorMessageDelivered => {
                        cutex::rotation::ReleaseRotationBoundary::DirectorMessageDelivered
                    }
                },
                expected_pending_external_step: expected_pending_external_step.map(
                    |step| match step {
                        ManagementReleaseRotationExternalStepArg::OfflinePredecessor => {
                            cutex::rotation::ReleaseRotationExternalStep::OfflinePredecessor
                        }
                        ManagementReleaseRotationExternalStepArg::RetirePredecessor => {
                            cutex::rotation::ReleaseRotationExternalStep::RetirePredecessor
                        }
                        ManagementReleaseRotationExternalStepArg::CreateSuccessorSession => {
                            cutex::rotation::ReleaseRotationExternalStep::CreateSuccessorSession
                        }
                        ManagementReleaseRotationExternalStepArg::StartSuccessorThread => {
                            cutex::rotation::ReleaseRotationExternalStep::StartSuccessorThread
                        }
                        ManagementReleaseRotationExternalStepArg::LaunchSuccessorRuntime => {
                            cutex::rotation::ReleaseRotationExternalStep::LaunchSuccessorRuntime
                        }
                        ManagementReleaseRotationExternalStepArg::DeliverDirectorMessage => {
                            cutex::rotation::ReleaseRotationExternalStep::DeliverDirectorMessage
                        }
                    },
                ),
                corrected_successor_cutex_session: corrected_successor_cutex_session
                    .map(cutex::role_revision::CutexSessionId::new)
                    .transpose()
                    .map_err(|_| anyhow::anyhow!("invalid corrected successor session"))?,
                corrected_successor_thread_id,
            };
            let body = serde_json::to_vec(&request)?;
            management_http_json(
                &base_url,
                "POST",
                "/v2/release-rotation/retry",
                Some(&token),
                Some(&body),
            )?
        }
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

fn cmd_management_seat(command: ManagementSeatCommand) -> anyhow::Result<()> {
    let config = load_codez_config();
    let (port, explicit_root_token) = match &command {
        ManagementSeatCommand::Bind { port, token, .. }
        | ManagementSeatCommand::Query { port, token } => {
            (port.unwrap_or(DEFAULT_MANAGEMENT_PORT), token.as_deref())
        }
    };
    validate_management_port(port)?;
    let token = task_service_seat_credential(&config, explicit_root_token)?;
    let base_url = management_base_url(port);
    let response = match command {
        ManagementSeatCommand::Bind {
            action_id,
            seat_id,
            occupant_cutex_session,
            ..
        } => {
            let request = cutex::seat::SeatOccupancyBindRequest {
                schema: cutex::seat::SeatOccupancyCommandSchema::V1,
                action_id: cutex::task_service::ActionId::new(action_id)?,
                seat_id: cutex::task_service::SeatId::new(seat_id)?,
                occupant_cutex_session: cutex::role_revision::CutexSessionId::new(
                    occupant_cutex_session,
                )
                .map_err(|_| anyhow::anyhow!("invalid durable Cutex session ID"))?,
            };
            let body = serde_json::to_vec(&request)?;
            management_http_json(
                &base_url,
                "POST",
                "/v2/task-service/seats/bind",
                Some(&token),
                Some(&body),
            )?
        }
        ManagementSeatCommand::Query { .. } => management_http_json(
            &base_url,
            "GET",
            "/v2/task-service/seats",
            Some(&token),
            None,
        )?,
    };
    println!("{}", serde_json::to_string(&response)?);
    Ok(())
}

pub(crate) fn cmd_management_serve(
    port: Option<u16>,
    bind: &str,
    token: Option<String>,
) -> anyhow::Result<()> {
    let configured = load_codez_config();
    let config = agent_bus_config::ensure_agent_bus_config(true, configured.agent_bus_port)?;
    agent_bus_runtime::ensure_agent_bus_running(&config, true)?;
    let adoption = app_server_runtime::adopt_persisted_runtimes(&config, &current_host_name())?;
    if adoption.adopted > 0
        || adoption.cleared_stale > 0
        || adoption.skipped > 0
        || !adoption.failures.is_empty()
    {
        println!(
            "app-server runtime adoption: adopted={} cleared_stale={} skipped={} failed={}",
            adoption.adopted,
            adoption.cleared_stale,
            adoption.skipped,
            adoption.failures.len()
        );
        for failure in adoption.failures.iter().take(3) {
            eprintln!("app-server adoption warning: {failure}");
        }
    }
    // Classify any process-loss window before the Management API begins
    // serving query or retry traffic. Subsequent provider opens in this
    // process do not reclassify an actively executing rotation.
    let _ = cutex::rotation::ReleaseRotationProvider::open_default()?;
    let port = port.unwrap_or(DEFAULT_MANAGEMENT_PORT);
    validate_management_port(port)?;
    let bind_addr = bind
        .parse::<IpAddr>()
        .with_context(|| format!("Invalid management bind address: {bind}"))?;
    let seat_admin_token = task_service_seat_credential(&config, token.as_deref()).ok();
    let agent_management_admin_token = management_root_credential(&config, token.as_deref())
        .ok()
        .map(str::to_string);
    let token = management_api_token(&config, token.as_deref()).map(str::to_string);
    let owner_task_read_credentials = config.owner_task_read_credentials.clone();
    run_management_server(
        bind_addr,
        port,
        token,
        seat_admin_token,
        agent_management_admin_token,
        owner_task_read_credentials,
    )
}

fn run_management_server(
    bind_addr: IpAddr,
    port: u16,
    token: Option<String>,
    seat_admin_token: Option<String>,
    agent_management_admin_token: Option<String>,
    owner_task_read_credentials: Vec<cutex::task_service::OwnerTaskReadCredential>,
) -> anyhow::Result<()> {
    cutex::management::server::run_management_server(
        bind_addr,
        port,
        token,
        seat_admin_token,
        agent_management_admin_token,
        owner_task_read_credentials,
        register_management_handoff,
        management_context::management_request_context(),
    )
}

fn register_management_handoff(bind_addr: IpAddr, port: u16) {
    if !command_exists_in_path("bridgeboard") {
        return;
    }
    let owner_host = current_host_name().to_ascii_lowercase();
    let url = management_health_url(bind_addr, port);
    let _ = Command::new("bridgeboard")
        .arg("handoff")
        .arg("--id")
        .arg(MANAGEMENT_BRIDGE_ID)
        .arg("--title")
        .arg("cutex management API")
        .arg("--port")
        .arg(port.to_string())
        .arg("--owner-host")
        .arg(owner_host)
        .arg("--pid-from-port")
        .arg("--local-url")
        .arg(&url)
        .arg("--open-url")
        .arg(&url)
        .arg("--health-url")
        .arg(&url)
        .arg("--tunnel-mode")
        .arg("local_forward")
        .arg("--require-healthy")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) fn cmd_management_remote_up(
    host: &str,
    service_id: Option<&str>,
    local_port: Option<u16>,
    remote_port: Option<u16>,
    token: Option<&str>,
    show_ssh_fallback: bool,
) -> anyhow::Result<()> {
    let config = load_codez_config();
    let local_port = local_port.unwrap_or(DEFAULT_MANAGEMENT_REMOTE_TUNNEL_PORT);
    let remote_port = remote_port.unwrap_or(DEFAULT_MANAGEMENT_PORT);
    let service_id = service_id.unwrap_or(MANAGEMENT_BRIDGE_ID);
    let token = management_api_token(&config, token);
    ensure_management_remote_tunnel(host, service_id, local_port, remote_port, token)?;
    println!(
        "{GREEN}management tunnel ready{RESET}: service={BOLD}{service_id}{RESET} host={BOLD}{host}{RESET} local_url={BOLD}{}{RESET}",
        management_health_local_url(local_port)
    );
    if show_ssh_fallback {
        println!(
            "{DIM}SSH fallback:{RESET} {}",
            raw_management_ssh_tunnel_command(host, local_port, remote_port)
        );
    }
    Ok(())
}

#[cfg(test)]
mod seat_cli_tests {
    use clap::Parser;

    use super::*;
    use cutex::cli::args::{Cli, CommandKind};

    #[test]
    fn seat_cli_exposes_only_typed_administration_fields() {
        let parsed = Cli::try_parse_from([
            "cutex",
            "management",
            "seat",
            "bind",
            "--action-id",
            "bind-director-1",
            "--seat-id",
            "cutex-director",
            "--occupant-cutex-session",
            "cutex-session-director",
            "--token",
            "dedicated-management-secret",
        ])
        .expect("parse typed seat bind");
        assert!(matches!(
            parsed.command,
            Some(CommandKind::Management {
                command: ManagementCommand::Seat {
                    command: ManagementSeatCommand::Bind {
                        action_id,
                        seat_id,
                        occupant_cutex_session,
                        port: None,
                        token,
                    }
                }
            }) if action_id == "bind-director-1"
                && seat_id == "cutex-director"
                && occupant_cutex_session == "cutex-session-director"
                && token.as_deref() == Some("dedicated-management-secret")
        ));
        assert!(matches!(
            Cli::try_parse_from(["cutex", "management", "seat", "query"])
                .expect("configured Management credential should need no CLI token")
                .command,
            Some(CommandKind::Management {
                command: ManagementCommand::Seat {
                    command: ManagementSeatCommand::Query {
                        port: None,
                        token: None,
                    }
                }
            })
        ));
        for forbidden in ["--runtime-agent-id", "--seat-epoch", "--attempt-token"] {
            assert!(Cli::try_parse_from([
                "cutex",
                "management",
                "seat",
                "query",
                "--token",
                "dedicated-management-secret",
                forbidden,
                "forged",
            ])
            .is_err());
        }
    }

    #[test]
    fn release_rotation_retry_cli_binds_exact_boundary_and_pending_step() {
        let parsed = Cli::try_parse_from([
            "cutex",
            "management",
            "release-rotation",
            "retry",
            "--action-id",
            "rotate-release-1",
            "--expected-request-sha256",
            &"1".repeat(64),
            "--expected-completed-boundary",
            "predecessor-retired",
            "--expected-pending-external-step",
            "create-successor-session",
            "--corrected-successor-cutex-session",
            "cutex.release-new",
        ])
        .expect("parse exact restart recovery retry");
        assert!(matches!(
            parsed.command,
            Some(CommandKind::Management {
                command: ManagementCommand::ReleaseRotation {
                    command: ManagementReleaseRotationCommand::Retry {
                        action_id,
                        expected_completed_boundary:
                            ManagementReleaseRotationBoundaryArg::PredecessorRetired,
                        expected_pending_external_step:
                            Some(ManagementReleaseRotationExternalStepArg::CreateSuccessorSession),
                        corrected_successor_cutex_session: Some(successor),
                        ..
                    }
                }
            }) if action_id == "rotate-release-1" && successor == "cutex.release-new"
        ));
        assert!(Cli::try_parse_from([
            "cutex",
            "management",
            "release-rotation",
            "retry",
            "--action-id",
            "rotate-release-1",
            "--expected-request-sha256",
            &"1".repeat(64),
            "--expected-completed-boundary",
            "predecessor-retired",
            "--expected-pending-external-step",
            "guessed-step",
        ])
        .is_err());
        assert!(Cli::try_parse_from([
            "cutex",
            "management",
            "release-rotation",
            "retry",
            "--action-id",
            "rotate-release-1",
            "--expected-request-sha256",
            &"1".repeat(64),
        ])
        .is_err());
    }
}
