//! Management v2 HTTP server loop and app-layer callback boundary.

use std::net::{IpAddr, TcpListener, TcpStream};

use anyhow::Context;

use crate::app_server::manager::AppServerManagedRuntimeStatus;
use crate::http::server::read_simple_http_request_with_body_limit;
use crate::http::server::write_http_response;
use crate::http::server::HttpRequestBodyTooLarge;
use crate::im::registry::ImRegistry;
use crate::management::service::management_health_url;
use crate::management::v2::session::MAX_REQUEST_BYTES;
use crate::management::v2::user_input::UserInputExecutionError;
use crate::management::v2::user_input::UserInputSubmitCommand;
use crate::management::v2::user_input::UserInputSubmitExecution;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub type ManagementHandoffRegistrar = fn(IpAddr, u16);
pub type ManagementRegistryLoader = fn() -> anyhow::Result<ImRegistry>;
pub type ManagementRuntimeStatusLoader =
    fn(&str) -> anyhow::Result<Option<AppServerManagedRuntimeStatus>>;
pub type ManagementNativeRequestHandler =
    fn(&str, serde_json::Value) -> Result<serde_json::Value, ManagementNativeForwardError>;
pub type ManagementNativeServerResponseHandler =
    fn(&str, u64, serde_json::Value) -> Result<(), ManagementNativeForwardError>;
pub type ManagementUserInputHandler =
    fn(UserInputSubmitCommand) -> Result<UserInputSubmitExecution, UserInputExecutionError>;
pub type ManagementUserInputQueueFlusher = fn(&str, usize) -> anyhow::Result<usize>;
pub type ManagementBootstrapStateLoader = fn(&str, u64) -> anyhow::Result<serde_json::Value>;
pub type ManagementSessionMutationHandler =
    fn(&str, &str, serde_json::Value) -> Result<serde_json::Value, UserInputExecutionError>;
pub type ManagementReleaseRotationRetryHandler =
    fn(crate::rotation::RetryReleaseRotationRequest) -> anyhow::Result<serde_json::Value>;
pub type ManagementReleaseRotationRequestHandler = fn(
    crate::rotation::ReleaseRotationInvocation,
    crate::rotation::ReleaseRotationRequest,
) -> anyhow::Result<serde_json::Value>;
pub type ManagementProjectAuthorityHandler =
    fn(crate::agent_management::ProjectAuthorityRequest) -> serde_json::Value;
pub type ManagementLegacyDirectorOwnershipImportHandler =
    fn(crate::agent_management::LegacyDirectorOwnershipImportRequest) -> serde_json::Value;
pub type ManagementAgentReservationReconciliationHandler =
    fn(crate::agent_management::AgentReservationReconciliationRequest) -> serde_json::Value;
pub type HumanManagementProjectCollectionHandler = fn(
    &crate::management::control_plane::HumanManagementPrincipal,
) -> Result<
    crate::management::control_plane::HumanManagementProjectCollection,
    crate::agent_management::AgentManagementError,
>;
pub type HumanManagementProjectReadHandler = fn(
    &crate::management::control_plane::HumanManagementPrincipal,
    &crate::agent_management::ProjectId,
) -> Result<
    crate::management::control_plane::HumanManagementProjectWorkspace,
    crate::agent_management::AgentManagementError,
>;
pub type HumanManagementPresentationUpdateHandler = fn(
    &crate::management::control_plane::HumanManagementPrincipal,
    &crate::management::control_plane::HumanManagementPresentationUpdateRequest,
) -> Result<
    crate::agent_management::ProjectPresentationSettings,
    crate::agent_management::AgentManagementError,
>;
pub type HumanManagementOperatorActionHandler = fn(
    &crate::management::control_plane::HumanManagementPrincipal,
    &crate::management::control_plane::HumanManagementOperatorActionRequest,
) -> Result<
    crate::management::control_plane::HumanManagementOperatorReceipt,
    crate::agent_management::AgentManagementError,
>;
pub type HumanManagementTaskQueryHandler =
    fn(
        &crate::management::control_plane::HumanManagementPrincipal,
        &crate::management::control_plane::HumanManagementTaskQueryRequest,
    ) -> anyhow::Result<crate::management::control_plane::HumanManagementTaskQueryResponse>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementNativeForwardError {
    BeforeForward(String),
    NativeRequestIdInUse(String),
    OutcomeUnknown(String),
    StaleRuntimeGeneration { expected: u64, actual: Option<u64> },
}

#[derive(Clone, Copy)]
pub struct ManagementRequestContext {
    pub load_registry: ManagementRegistryLoader,
    pub load_runtime_status: ManagementRuntimeStatusLoader,
    pub forward_native_request: ManagementNativeRequestHandler,
    pub respond_native_server_request: ManagementNativeServerResponseHandler,
    pub handle_user_input: ManagementUserInputHandler,
    pub flush_user_input_queue: ManagementUserInputQueueFlusher,
    pub load_bootstrap_state: ManagementBootstrapStateLoader,
    pub mutate_session: ManagementSessionMutationHandler,
    pub retry_release_rotation: ManagementReleaseRotationRetryHandler,
    pub request_release_rotation: ManagementReleaseRotationRequestHandler,
    pub bind_project_authority: ManagementProjectAuthorityHandler,
    pub import_legacy_director_ownership: ManagementLegacyDirectorOwnershipImportHandler,
    pub reconcile_agent_reservation: ManagementAgentReservationReconciliationHandler,
    pub list_management_projects: HumanManagementProjectCollectionHandler,
    pub read_management_project: HumanManagementProjectReadHandler,
    pub update_management_project_presentation: HumanManagementPresentationUpdateHandler,
    pub execute_management_operator_action: HumanManagementOperatorActionHandler,
    pub query_management_tasks: HumanManagementTaskQueryHandler,
}

pub fn run_management_server(
    bind_addr: IpAddr,
    port: u16,
    token: Option<String>,
    seat_admin_token: Option<String>,
    agent_management_admin_token: Option<String>,
    owner_task_read_credentials: Vec<crate::task_service::OwnerTaskReadCredential>,
    register_handoff: ManagementHandoffRegistrar,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind((bind_addr, port))
        .with_context(|| format!("Failed to bind cutex management API on {bind_addr}:{port}"))?;
    println!(
        "cutex management API listening on {}",
        management_health_url(bind_addr, port)
    );
    std::thread::spawn(move || register_handoff(bind_addr, port));

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let token = token.clone();
                let seat_admin_token = seat_admin_token.clone();
                let agent_management_admin_token = agent_management_admin_token.clone();
                let owner_task_read_credentials = owner_task_read_credentials.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_management_request(
                        &mut stream,
                        token.as_deref(),
                        seat_admin_token.as_deref(),
                        agent_management_admin_token.as_deref(),
                        &owner_task_read_credentials,
                        context,
                    ) {
                        let _ = write_http_response(
                            &mut stream,
                            500,
                            "Internal Server Error",
                            "text/plain",
                            format!("{error:#}").as_bytes(),
                        );
                    }
                });
            }
            Err(error) => {
                eprintln!("{YELLOW}warning:{RESET} management API accept failed: {error}")
            }
        }
    }
    Ok(())
}

pub fn handle_management_request(
    stream: &mut TcpStream,
    token: Option<&str>,
    seat_admin_token: Option<&str>,
    agent_management_admin_token: Option<&str>,
    owner_task_read_credentials: &[crate::task_service::OwnerTaskReadCredential],
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let request = match read_simple_http_request_with_body_limit(stream, MAX_REQUEST_BYTES) {
        Ok(request) => request,
        Err(error) => {
            if let Some(too_large) = error.downcast_ref::<HttpRequestBodyTooLarge>() {
                if too_large.path == "/v2" || too_large.path.starts_with("/v2/") {
                    return crate::management::v2::server::write_payload_too_large(
                        stream,
                        too_large.content_length,
                        too_large.limit,
                    );
                }
            }
            return Err(error);
        }
    };
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str());
    if path == "/v2" || path.starts_with("/v2/") {
        return crate::management::v2::server::handle_v2_request(
            stream,
            &request,
            token,
            seat_admin_token,
            agent_management_admin_token,
            owner_task_read_credentials,
            context,
        );
    }
    match (request.method.as_str(), path) {
        ("GET", "/") => write_http_response(stream, 200, "OK", "text/plain", b"ok"),
        _ => write_http_response(stream, 404, "Not Found", "text/plain", b"not found"),
    }
}
