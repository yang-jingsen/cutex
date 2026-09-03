use std::collections::HashMap;
use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context;
use serde_json::json;
use serde_json::Value;

use crate::http::server::require_service_bridge_token;
use crate::http::server::write_json_response;
use crate::http::server::SimpleHttpRequest;
use crate::management::server::ManagementNativeForwardError;
use crate::management::server::ManagementNativeRequestHandler;
use crate::management::server::ManagementRequestContext;
use crate::management::server::ManagementSessionMutationHandler;
use crate::management::server::ManagementUserInputHandler;
use crate::management::server::ManagementUserInputQueueFlusher;
pub use crate::management::service::task_service_seat_management_token;

use super::model::EventEnvelope;
use super::model::MAX_SAFE_SEQUENCE;
use super::native_requests::request_idempotency_repository;
use super::native_requests::validate_native_request;
use super::native_requests::BeginRequest;
use super::native_requests::NativeRequestValidationError;
use super::native_requests::RequestClaim;
use super::native_requests::RequestIdempotencyRepository;
use super::native_requests::NATIVE_REQUEST_POLICY_VERSION;
use super::repository::management_v2_repository;
use super::repository::EventRepository;
use super::repository::ReplayError;
use super::repository::ReplayQuery;
use super::server_requests;
use super::server_requests::ServerResponseClaimDecision;
use super::session::clear_focus;
use super::session::cutex_method_is_registered;
use super::session::focus_resource;
use super::session::session_list_resource;
use super::session::session_resource;
use super::session::session_resource_including_archive;
use super::session::session_resource_including_hidden;
use super::session::set_focus;
use super::user_input::parse_user_input_submit_params;
use super::user_input::user_input_repository;
use super::user_input::validate_user_input;
use super::user_input::UserInputDisposition;
use super::user_input::UserInputSubmitCommand;

const SSE_EVENT_NAME: &str = "cutex_management_event_v2";
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const SSE_SUBSCRIBER_CAPACITY: usize = 1024;

static CUTEX_REQUEST_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

pub fn handle_v2_request(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    token: Option<&str>,
    seat_admin_token: Option<&str>,
    agent_management_admin_token: Option<&str>,
    owner_task_read_credentials: &[crate::task_service::OwnerTaskReadCredential],
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str());
    if path.starts_with("/v2/projects/") {
        let project_id = match owner_task_project_from_path(path) {
            Some(project_id) => project_id,
            None => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_project",
                    "an exact project task path is required",
                    false,
                    json!({}),
                );
            }
        };
        let principal = match crate::task_service::OwnerTaskReadCredential::authenticate(
            owner_task_read_credentials,
            request.headers.get("authorization").map(String::as_str),
            &project_id,
            chrono::Utc::now(),
        ) {
            Ok(principal) => principal,
            Err(crate::task_service::OwnerTaskReadError::ProjectDenied) => {
                return write_v2_error(
                    stream,
                    403,
                    "Forbidden",
                    "project_denied",
                    "the authenticated principal cannot read this project",
                    false,
                    json!({}),
                );
            }
            Err(_) => {
                return write_v2_error(
                    stream,
                    401,
                    "Unauthorized",
                    "unauthorized",
                    "a valid Owner Task read credential is required",
                    false,
                    json!({}),
                );
            }
        };
        let repository = management_v2_repository()?;
        return handle_v2_request_with_repository(
            stream,
            request,
            repository,
            context,
            Some((&principal, &project_id)),
        );
    }
    let required_token =
        v2_required_token(path, token, seat_admin_token, agent_management_admin_token);
    let legacy_seat_scoped =
        path.starts_with("/v2/task-service/seats") || path.starts_with("/v2/release-rotation");
    let agent_management_admin_scoped = agent_management_admin_path(path);
    let agent_management_scoped =
        path.starts_with("/v2/agent-management") && !agent_management_admin_scoped;
    let required_credential_missing = (legacy_seat_scoped && seat_admin_token.is_none())
        || (agent_management_scoped && token.is_none())
        || (agent_management_admin_scoped && agent_management_admin_token.is_none());
    if require_service_bridge_token(request, required_token, "Management v2").is_err()
        || required_credential_missing
    {
        return write_v2_error(
            stream,
            401,
            "Unauthorized",
            "unauthorized",
            "valid cutex management authentication is required",
            false,
            json!({}),
        );
    }
    let repository = management_v2_repository()?;
    materialize_active_stream_reset(repository, context)?;
    handle_v2_request_with_repository(stream, request, repository, context, None)
}

fn v2_required_token<'a>(
    path: &str,
    management_token: Option<&'a str>,
    seat_admin_token: Option<&'a str>,
    agent_management_admin_token: Option<&'a str>,
) -> Option<&'a str> {
    if path.starts_with("/v2/task-service/seats") || path.starts_with("/v2/release-rotation") {
        seat_admin_token
    } else if agent_management_admin_path(path) {
        agent_management_admin_token
    } else {
        management_token
    }
}

fn agent_management_admin_path(path: &str) -> bool {
    matches!(
        path,
        "/v2/agent-management/authority" | "/v2/agent-management/legacy-director-ownership-import"
    )
}

fn materialize_active_stream_reset(
    repository: &EventRepository,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    if repository.recovery_reset()?.is_none() {
        return Ok(());
    }
    let registry = (context.load_registry)()?;
    let sessions = session_list_resource(&registry, context.load_runtime_status, repository)?;
    let active_session_ids = sessions
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|session| {
            session.pointer("/runtime/status").and_then(Value::as_str) == Some("online")
        })
        .filter_map(|session| session.get("cutexSessionId").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    repository.materialize_recovery_reset(&active_session_ids, true)?;
    Ok(())
}

fn handle_v2_request_with_repository(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    repository: &EventRepository,
    context: ManagementRequestContext,
    owner: Option<(
        &crate::task_service::OwnerTaskReadPrincipal,
        &crate::agent_management::ProjectId,
    )>,
) -> anyhow::Result<()> {
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str());
    match (request.method.as_str(), path) {
        ("GET", path) if owner_task_project_from_path(path).is_some() => {
            let (principal, project_id) =
                owner.expect("Owner route is authenticated before dispatch");
            handle_owner_task_read(stream, request, principal, project_id)
        }
        ("POST", "/v2/task-service/seats/bind") => handle_seat_bind(stream, request),
        ("GET", "/v2/task-service/seats") => handle_seat_query(stream),
        ("POST", "/v2/release-rotation/template") => {
            handle_release_template_configure(stream, request)
        }
        ("GET", "/v2/release-rotation/template") => handle_release_template_query(stream),
        ("GET", "/v2/release-rotation") => handle_release_rotation_query(stream),
        ("POST", "/v2/release-rotation/retry") => {
            handle_release_rotation_retry(stream, request, context)
        }
        ("POST", "/v2/release-rotation/director-request") => {
            handle_release_rotation_director_request(stream, request, context)
        }
        ("POST", "/v2/agent-management/actions") => handle_agent_management_action(stream, request),
        ("POST", "/v2/agent-management/authority") => {
            handle_project_authority(stream, request, context)
        }
        ("POST", "/v2/agent-management/legacy-director-ownership-import") => {
            handle_legacy_director_ownership_import(stream, request, context)
        }
        ("GET", "/v2/sessions") => {
            super::archive::handle_session_collection_get(stream, request, context, repository)
        }
        ("POST", path) if native_request_session_id_from_path(path).is_some() => {
            let cutex_session_id =
                native_request_session_id_from_path(path).expect("matched native request path");
            handle_native_request(stream, request, context, repository, &cutex_session_id)
        }
        ("POST", path) if server_response_session_id_from_path(path).is_some() => {
            let cutex_session_id =
                server_response_session_id_from_path(path).expect("matched server response path");
            handle_native_server_response(stream, request, context, repository, &cutex_session_id)
        }
        ("POST", path) if cutex_request_session_id_from_path(path).is_some() => {
            let cutex_session_id =
                cutex_request_session_id_from_path(path).expect("matched cutex request path");
            handle_cutex_request(stream, request, context, repository, &cutex_session_id)
        }
        ("GET", path) if bootstrap_session_id_from_path(path).is_some() => {
            let cutex_session_id =
                bootstrap_session_id_from_path(path).expect("matched bootstrap path");
            handle_bootstrap(stream, context, repository, &cutex_session_id)
        }
        ("GET", path) if session_id_from_path(path).is_some() => {
            let cutex_session_id = session_id_from_path(path).expect("matched session path");
            let lifecycle = match super::archive::session_lifecycle_query(&request.path) {
                Ok(lifecycle) => lifecycle,
                Err(error) => {
                    return write_v2_error(
                        stream,
                        400,
                        "Bad Request",
                        "invalid_request",
                        &error.to_string(),
                        false,
                        json!({}),
                    )
                }
            };
            match lifecycle {
                super::archive::SessionLifecycleQuery::Retired => {
                    match super::session::retired_session_resource(&cutex_session_id)? {
                        Some(resource) => write_json_response(stream, 200, "OK", &resource),
                        None => write_v2_error(
                            stream,
                            404,
                            "Not Found",
                            "session_not_found",
                            "the retired cutex session does not exist on this host",
                            false,
                            json!({ "cutexSessionId": cutex_session_id, "lifecycle": "retired" }),
                        ),
                    }
                }
                super::archive::SessionLifecycleQuery::Active => {
                    let registry = (context.load_registry)()?;
                    match session_resource(
                        &cutex_session_id,
                        &registry,
                        context.load_runtime_status,
                        repository,
                    )? {
                        Some(resource) => write_json_response(stream, 200, "OK", &resource),
                        None => write_v2_error(
                            stream,
                            404,
                            "Not Found",
                            "session_not_found",
                            "the owner-visible cutex session does not exist on this host",
                            false,
                            json!({ "cutexSessionId": cutex_session_id }),
                        ),
                    }
                }
            }
        }
        ("GET", "/v2/events") => {
            let query = match replay_query(request, false) {
                Ok(query) => query,
                Err(error) => return write_replay_error(stream, error),
            };
            match repository.page(query) {
                Ok(page) => write_json_response(stream, 200, "OK", &serde_json::to_value(page)?),
                Err(error) => write_replay_error(stream, error),
            }
        }
        ("GET", "/v2/events/stream") => {
            let query = match replay_query(request, true) {
                Ok(query) => query,
                Err(error) => return write_replay_error(stream, error),
            };
            let subscription =
                match repository.page_and_subscribe(query.clone(), SSE_SUBSCRIBER_CAPACITY) {
                    Ok(subscription) => subscription,
                    Err(error) => return write_replay_error(stream, error),
                };
            let subscribed_stream_id = subscription.page.stream_id.clone();
            write_sse_headers(stream)?;
            for event in subscription.page.events {
                write_sse_event(stream, &event)?;
            }
            loop {
                match subscription.receiver.recv_timeout(SSE_KEEPALIVE_INTERVAL) {
                    Ok(event) => {
                        if event.stream_id != subscribed_stream_id {
                            write_sse_stream_changed(
                                stream,
                                &event.host_id,
                                &subscribed_stream_id,
                                &event.stream_id,
                            )?;
                            return Ok(());
                        }
                        if query
                            .cutex_session_id
                            .as_deref()
                            .is_none_or(|session_id| event.cutex_session_id == session_id)
                        {
                            write_sse_event(stream, &event)?;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        stream.write_all(b": keepalive\n\n")?;
                        stream.flush()?;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
        }
        _ => write_v2_error(
            stream,
            404,
            "Not Found",
            "route_not_found",
            "management v2 route not found",
            false,
            json!({ "path": path }),
        ),
    }
}

fn handle_agent_management_action(
    stream: &mut TcpStream,
    _request: &SimpleHttpRequest,
) -> anyhow::Result<()> {
    write_v2_error(
        stream,
        403,
        "Forbidden",
        "ambient_agent_bus_required",
        "Agent Management lifecycle actions require a current authenticated Agent Bus occurrence",
        false,
        json!({}),
    )
}

fn handle_project_authority(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let payload: crate::agent_management::ProjectAuthorityRequest =
        match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_request",
                    &format!("strict project authority request parsing failed: {error}"),
                    false,
                    json!({}),
                )
            }
        };
    let response = (context.bind_project_authority)(payload);
    write_json_response(stream, 200, "OK", &response)
}

fn handle_legacy_director_ownership_import(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let payload: crate::agent_management::LegacyDirectorOwnershipImportRequest =
        match serde_json::from_slice(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_request",
                    &format!(
                        "strict legacy Director ownership import request parsing failed: {error}"
                    ),
                    false,
                    json!({}),
                )
            }
        };
    let response = (context.import_legacy_director_ownership)(payload);
    write_json_response(stream, 200, "OK", &response)
}

fn owner_task_project_from_path(path: &str) -> Option<crate::agent_management::ProjectId> {
    let value = path.strip_prefix("/v2/projects/")?.strip_suffix("/tasks")?;
    if value.is_empty() || value.contains('/') || value.contains('%') {
        return None;
    }
    crate::agent_management::ProjectId::new(value.to_string()).ok()
}

fn handle_owner_task_read(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    principal: &crate::task_service::OwnerTaskReadPrincipal,
    project_id: &crate::agent_management::ProjectId,
) -> anyhow::Result<()> {
    let provider = match crate::task_service::TaskServiceProvider::open(
        crate::task_delivery::provider_adapter::default_task_service_provider_root()
            .map_err(|_| anyhow::anyhow!("Task Service root unavailable"))?,
    ) {
        Ok(provider) => provider,
        Err(_) => {
            return write_v2_error(
                stream,
                503,
                "Service Unavailable",
                "task_store_unavailable",
                "the canonical Task store is unavailable",
                true,
                json!({}),
            );
        }
    };
    handle_owner_task_read_with_provider(stream, request, principal, project_id, &provider)
}

fn handle_owner_task_read_with_provider(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    principal: &crate::task_service::OwnerTaskReadPrincipal,
    project_id: &crate::agent_management::ProjectId,
    provider: &crate::task_service::TaskServiceProvider,
) -> anyhow::Result<()> {
    let snapshot = match provider.query_cancellable(|| http_client_disconnected(stream)) {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return write_v2_error(
                stream,
                503,
                "Service Unavailable",
                "task_store_unavailable",
                "the canonical Task store is unavailable",
                true,
                json!({}),
            );
        }
    };
    let activity =
        crate::management::v2::activity::load_session_activity_states().unwrap_or_default();
    write_owner_task_snapshot(
        stream,
        request,
        principal,
        project_id,
        &snapshot,
        &activity,
        chrono::Utc::now(),
    )
}

fn http_client_disconnected(stream: &TcpStream) -> bool {
    match stream.take_error() {
        Ok(Some(_)) | Err(_) => return true,
        Ok(None) => {}
    }
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut byte = [0_u8; 1];
    let result = stream.peek(&mut byte);
    let _ = stream.set_nonblocking(false);
    match result {
        // Receive EOF proves only that the requester closed its sending half.
        // A valid HTTP/1.1 client may still be waiting on the response half.
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    }
}

fn write_owner_task_snapshot(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    principal: &crate::task_service::OwnerTaskReadPrincipal,
    project_id: &crate::agent_management::ProjectId,
    snapshot: &crate::task_service::TaskServiceSnapshot,
    activity: &HashMap<String, crate::management::v2::activity::SessionActivityState>,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    let filter = match owner_task_filter(&request.path) {
        Ok(filter) => filter,
        Err(message) => {
            return write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_query",
                &message,
                false,
                json!({}),
            );
        }
    };
    let response = match crate::task_service::project_owner_tasks(
        snapshot, activity, principal, project_id, &filter, now,
    ) {
        Ok(response) => response,
        Err(crate::task_service::OwnerTaskReadError::ProjectDenied) => {
            return write_v2_error(
                stream,
                403,
                "Forbidden",
                "project_denied",
                "the authenticated principal cannot read this project",
                false,
                json!({}),
            );
        }
        Err(crate::task_service::OwnerTaskReadError::InvalidCursor) => {
            return write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_cursor",
                "the cursor is invalid, expired, or bound to another query",
                false,
                json!({}),
            );
        }
        Err(crate::task_service::OwnerTaskReadError::InvalidQuery(_)) => {
            return write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_query",
                "the Owner Task query is invalid",
                false,
                json!({}),
            );
        }
        Err(_) => {
            return write_v2_error(
                stream,
                503,
                "Service Unavailable",
                "projection_unavailable",
                "the safe Task projection is unavailable",
                true,
                json!({}),
            );
        }
    };
    if filter.task_id.is_some() && response.items.is_empty() {
        return write_v2_error(
            stream,
            404,
            "Not Found",
            "task_not_found",
            "the requested Task does not exist in the selected project",
            false,
            json!({}),
        );
    }
    let bytes = serde_json::to_vec(&response)?;
    if bytes.len() > super::session::MAX_REQUEST_BYTES {
        return write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "response_bound_exceeded",
            "the bounded Task response could not be produced",
            false,
            json!({}),
        );
    }
    write_json_response(stream, 200, "OK", &serde_json::to_value(response)?)
}

fn owner_task_filter(path: &str) -> Result<crate::task_service::OwnerTaskReadFilter, String> {
    let mut filter = crate::task_service::OwnerTaskReadFilter::default();
    let Some((_, query)) = path.split_once('?') else {
        return Ok(filter);
    };
    let mut seen = std::collections::BTreeSet::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if !seen.insert(key.to_string()) {
            return Err(format!("query parameter {key} must not be repeated"));
        }
        match key.as_ref() {
            "state" => {
                for state in value.split(',') {
                    if !matches!(
                        state,
                        "awaiting_ack" | "active" | "retry_pending" | "closed"
                    ) {
                        return Err("state contains an unknown value".to_string());
                    }
                    filter.states.insert(state.to_string());
                }
            }
            "assignee" => {
                filter.assignee = Some(
                    crate::role_revision::CutexSessionId::new(value.into_owned())
                        .map_err(|_| "assignee is invalid".to_string())?,
                );
            }
            "updated_since" => filter.updated_since = Some(value.into_owned()),
            "task_id" => {
                filter.task_id = Some(
                    crate::role_revision::TaskId::new(value.into_owned())
                        .map_err(|_| "task_id is invalid".to_string())?,
                );
            }
            "limit" => {
                filter.limit = value
                    .parse::<usize>()
                    .map_err(|_| "limit must be an integer".to_string())?;
                if filter.limit == 0 || filter.limit > crate::task_service::OWNER_TASK_MAX_LIMIT {
                    return Err("limit is outside 1..=100".to_string());
                }
            }
            "cursor" => filter.cursor = Some(value.into_owned()),
            _ => return Err(format!("unknown query parameter {key}")),
        }
    }
    Ok(filter)
}

fn handle_seat_bind(stream: &mut TcpStream, request: &SimpleHttpRequest) -> anyhow::Result<()> {
    let request: crate::seat::SeatOccupancyBindRequest = match serde_json::from_slice(&request.body)
    {
        Ok(request) => request,
        Err(error) => {
            return write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_request",
                "strict seat-occupancy request parsing failed",
                false,
                json!({ "diagnostic": error.to_string() }),
            )
        }
    };
    let store = crate::seat::SeatOccupancyStore::open_default()?;
    match store.bind(&request) {
        Ok(receipt) => write_json_response(stream, 200, "OK", &serde_json::to_value(receipt)?),
        Err(crate::seat::SeatAuthorityError::Conflict(reason)) => write_v2_error(
            stream,
            409,
            "Conflict",
            "seat_occupancy_conflict",
            reason,
            false,
            json!({}),
        ),
        Err(crate::seat::SeatAuthorityError::InvalidRequest(reason)) => write_v2_error(
            stream,
            400,
            "Bad Request",
            "invalid_request",
            reason,
            false,
            json!({}),
        ),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "seat_occupancy_unavailable",
            "durable seat occupancy could not be updated",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_seat_query(stream: &mut TcpStream) -> anyhow::Result<()> {
    let store = crate::seat::SeatOccupancyStore::open_default()?;
    match store.query() {
        Ok(snapshot) => write_json_response(stream, 200, "OK", &serde_json::to_value(snapshot)?),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "seat_occupancy_unavailable",
            "durable seat occupancy could not be read",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_release_template_configure(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
) -> anyhow::Result<()> {
    let request: crate::rotation::ConfigureReleaseTemplateRequest =
        match serde_json::from_slice(&request.body) {
            Ok(request) => request,
            Err(error) => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_request",
                    "strict Release template request parsing failed",
                    false,
                    json!({ "diagnostic": error.to_string() }),
                )
            }
        };
    let store = crate::rotation::ReleaseTemplateStore::open_default()?;
    match store.configure(&request) {
        Ok(receipt) => write_json_response(stream, 200, "OK", &serde_json::to_value(receipt)?),
        Err(crate::rotation::ReleaseTemplateError::InvalidRequest(reason)) => write_v2_error(
            stream,
            400,
            "Bad Request",
            "invalid_request",
            reason,
            false,
            json!({}),
        ),
        Err(crate::rotation::ReleaseTemplateError::Conflict(reason)) => write_v2_error(
            stream,
            409,
            "Conflict",
            "release_template_conflict",
            reason,
            false,
            json!({}),
        ),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "release_template_unavailable",
            "durable Release template could not be updated",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_release_template_query(stream: &mut TcpStream) -> anyhow::Result<()> {
    let store = crate::rotation::ReleaseTemplateStore::open_default()?;
    match store.query() {
        Ok(snapshot) => write_json_response(stream, 200, "OK", &serde_json::to_value(snapshot)?),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "release_template_unavailable",
            "durable Release template could not be read",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_release_rotation_query(stream: &mut TcpStream) -> anyhow::Result<()> {
    let provider = crate::rotation::ReleaseRotationProvider::open_default()?;
    match provider.query(None) {
        Ok(receipts) => write_json_response(
            stream,
            200,
            "OK",
            &json!({ "schema": "cutex/release-rotation-query/v1", "rotations": receipts }),
        ),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "release_rotation_unavailable",
            "durable Release rotation state could not be read",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_release_rotation_retry(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let request: crate::rotation::RetryReleaseRotationRequest =
        match serde_json::from_slice(&request.body) {
            Ok(request) => request,
            Err(error) => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_request",
                    "strict Release rotation retry request parsing failed",
                    false,
                    json!({ "diagnostic": error.to_string() }),
                )
            }
        };
    match (context.retry_release_rotation)(request) {
        Ok(response) => write_json_response(stream, 200, "OK", &response),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "release_rotation_retry_unavailable",
            "Release rotation retry could not be executed",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_release_rotation_director_request(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let request: crate::rotation::ManagementReleaseRotationRequest =
        match serde_json::from_slice(&request.body) {
            Ok(request) => request,
            Err(error) => {
                return write_v2_error(
                    stream,
                    400,
                    "Bad Request",
                    "invalid_request",
                    "strict authenticated Release rotation envelope parsing failed",
                    false,
                    json!({ "diagnostic": error.to_string() }),
                )
            }
        };
    match (context.request_release_rotation)(request.invocation, request.request) {
        Ok(response) => write_json_response(stream, 200, "OK", &response),
        Err(error) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "release_rotation_unavailable",
            "Release rotation could not be executed by the runtime owner",
            true,
            json!({ "diagnostic": error.to_string() }),
        ),
    }
}

fn handle_bootstrap(
    stream: &mut TcpStream,
    context: ManagementRequestContext,
    repository: &EventRepository,
    cutex_session_id: &str,
) -> anyhow::Result<()> {
    let registry = (context.load_registry)()?;
    let Some(session) = session_resource(
        cutex_session_id,
        &registry,
        context.load_runtime_status,
        repository,
    )?
    else {
        return write_v2_error(
            stream,
            404,
            "Not Found",
            "session_not_found",
            "the owner-visible cutex session does not exist on this host",
            false,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    };
    match build_bootstrap_resource(
        &session,
        repository,
        |message| {
            (context.forward_native_request)(cutex_session_id, message)
                .map_err(|error| format!("{error:?}"))
        },
        |runtime_generation| (context.load_bootstrap_state)(cutex_session_id, runtime_generation),
    ) {
        Ok(response) => write_json_response(stream, 200, "OK", &response),
        Err(BootstrapBuildError::Native { message, details }) => write_v2_error(
            stream,
            502,
            "Bad Gateway",
            "app_server_unavailable",
            &message,
            false,
            details,
        ),
        Err(BootstrapBuildError::State(error)) => write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "event_persistence_unavailable",
            "the bootstrap checkpoint or cutex state could not be read",
            true,
            json!({ "diagnostic": format!("{error:#}") }),
        ),
    }
}

#[derive(Debug)]
enum BootstrapBuildError {
    Native { message: String, details: Value },
    State(anyhow::Error),
}

fn build_bootstrap_resource(
    session: &Value,
    repository: &EventRepository,
    forward_native: impl FnOnce(Value) -> Result<Value, String>,
    load_state: impl FnOnce(u64) -> anyhow::Result<Value>,
) -> Result<Value, BootstrapBuildError> {
    let cutex_session_id = session
        .get("cutexSessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thread_id = session
        .pointer("/native/threadId")
        .and_then(Value::as_str)
        .filter(|thread_id| !thread_id.is_empty());
    let checkpoint = repository
        .checkpoint()
        .map_err(BootstrapBuildError::State)?;
    let app_server_connected = session
        .pointer("/runtime/appServerConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let native = if app_server_connected {
        let Some(thread_id) = thread_id else {
            return Err(BootstrapBuildError::Native {
                message: "the connected app-server session has no bound native thread".to_string(),
                details: json!({ "cutexSessionId": cutex_session_id }),
            });
        };
        let native_response = forward_native(json!({
            "id": format!("cutex-bootstrap-{}", uuid::Uuid::new_v4()),
            "method": "thread/read",
            "params": { "threadId": thread_id, "includeTurns": true },
        }))
        .map_err(|diagnostic| BootstrapBuildError::Native {
            message: "the native thread snapshot could not be read".to_string(),
            details: json!({ "diagnostic": diagnostic }),
        })?;
        let Some(native_result) = native_response.get("result").cloned() else {
            return Err(BootstrapBuildError::Native {
                message: "thread/read returned a native error".to_string(),
                details: json!({ "native": { "message": native_response } }),
            });
        };
        json!({
            "method": "thread/read",
            "result": native_result,
        })
    } else {
        Value::Null
    };
    let runtime_generation = session
        .pointer("/runtime/runtimeGeneration")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let collections = load_state(runtime_generation).map_err(BootstrapBuildError::State)?;
    let user_input_queue = required_bootstrap_array(&collections, "userInputQueue")
        .map_err(BootstrapBuildError::State)?;
    let pending_server_requests = required_bootstrap_array(&collections, "pendingServerRequests")
        .map_err(BootstrapBuildError::State)?;
    let agent_bus_messages = required_bootstrap_array(&collections, "agentBusMessages")
        .map_err(BootstrapBuildError::State)?;
    Ok(json!({
        "contractVersion": 2,
        "cutexSessionId": cutex_session_id,
        "hostId": session.get("hostId").cloned().unwrap_or(Value::Null),
        "checkpoint": checkpoint,
        "schema": session.pointer("/native/schema").cloned().unwrap_or(Value::Null),
        "management": session.get("management").cloned().unwrap_or(Value::Null),
        "native": native,
        "cutexState": {
            "sessionRevision": session.get("revision").cloned().unwrap_or(Value::Null),
            "runtime": {
                "status": session.pointer("/runtime/status").cloned().unwrap_or(Value::Null),
                "runtimeGeneration": runtime_generation,
            },
            "focus": session.get("focus").cloned().unwrap_or(Value::Null),
            "userInputQueue": user_input_queue,
            "pendingServerRequests": pending_server_requests,
            "agentBusMessages": agent_bus_messages,
        }
    }))
}

fn required_bootstrap_array(value: &Value, key: &str) -> anyhow::Result<Value> {
    value
        .get(key)
        .filter(|value| value.is_array())
        .cloned()
        .with_context(|| format!("bootstrap state omitted array {key}"))
}

fn handle_native_server_response(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
    event_repository: &EventRepository,
    cutex_session_id: &str,
) -> anyhow::Result<()> {
    let response_id_hint = response_id_hint(&request.body);
    let registry = (context.load_registry)()?;
    let Some(session) = session_resource(
        cutex_session_id,
        &registry,
        context.load_runtime_status,
        event_repository,
    )?
    else {
        return write_v2_server_response_error(
            stream,
            404,
            "Not Found",
            response_id_hint.as_deref(),
            "session_not_found",
            "the owner-visible cutex session does not exist on this host",
            false,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    };
    let response = match validate_server_response_body(&request.body) {
        Ok(response) => response,
        Err(message) => {
            return write_v2_server_response_error(
                stream,
                400,
                "Bad Request",
                response_id_hint.as_deref(),
                "invalid_request",
                &message,
                false,
                json!({}),
            );
        }
    };
    let expected_runtime_generation = match request.headers.get("if-match") {
        Some(value) => match parse_runtime_generation_precondition(value) {
            Ok(generation) => generation,
            Err(message) => {
                return write_v2_server_response_error(
                    stream,
                    400,
                    "Bad Request",
                    Some(&response.response_id),
                    "invalid_precondition",
                    &message,
                    false,
                    json!({ "header": "If-Match" }),
                );
            }
        },
        None => {
            return write_v2_server_response_error(
                stream,
                428,
                "Precondition Required",
                Some(&response.response_id),
                "precondition_required",
                "If-Match must identify the expected Cutex runtime generation",
                false,
                json!({ "header": "If-Match" }),
            );
        }
    };
    let current_runtime_generation = session
        .pointer("/runtime/runtimeGeneration")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if current_runtime_generation != expected_runtime_generation {
        return write_stale_runtime_generation(
            stream,
            &response.response_id,
            expected_runtime_generation,
            Some(current_runtime_generation).filter(|generation| *generation > 0),
        );
    }
    if !session
        .pointer("/runtime/appServerConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return write_v2_server_response_error(
            stream,
            409,
            "Conflict",
            Some(&response.response_id),
            "session_offline",
            "the cutex session has no connected app-server",
            true,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    }
    let canonical_body: Value = serde_json::from_slice(&request.body)
        .expect("validated server response body must remain valid JSON");
    let canonical_claim = json!({
        "expectedRuntimeGeneration": expected_runtime_generation,
        "body": canonical_body,
    });
    let native_request_id = response.native_message["id"].clone();
    match server_requests::claim_response(
        cutex_session_id,
        expected_runtime_generation,
        &response.response_id,
        &canonical_claim,
        &response.native_message,
    )? {
        ServerResponseClaimDecision::Submit => {
            match (context.respond_native_server_request)(
                cutex_session_id,
                expected_runtime_generation,
                response.native_message.clone(),
            ) {
                Ok(()) => {
                    server_requests::mark_response_submitted(
                        cutex_session_id,
                        expected_runtime_generation,
                        &response.response_id,
                    )?;
                    write_server_response_result(
                        stream,
                        cutex_session_id,
                        &response.response_id,
                        "submitted",
                        response.native_message,
                    )
                }
                Err(ManagementNativeForwardError::BeforeForward(diagnostic))
                | Err(ManagementNativeForwardError::NativeRequestIdInUse(diagnostic)) => {
                    server_requests::release_definitely_failed_response(
                        cutex_session_id,
                        expected_runtime_generation,
                        &response.response_id,
                    )?;
                    write_v2_server_response_error(
                        stream,
                        502,
                        "Bad Gateway",
                        Some(&response.response_id),
                        "server_request_delivery_failed",
                        "the response was not written to the current app-server connection",
                        true,
                        json!({
                            "nativeRequestId": native_request_id,
                            "runtimeGeneration": expected_runtime_generation,
                            "claimReleased": true,
                            "diagnostic": diagnostic,
                        }),
                    )
                }
                Err(ManagementNativeForwardError::StaleRuntimeGeneration { expected, actual }) => {
                    server_requests::release_definitely_failed_response(
                        cutex_session_id,
                        expected_runtime_generation,
                        &response.response_id,
                    )?;
                    write_stale_runtime_generation(stream, &response.response_id, expected, actual)
                }
                Err(ManagementNativeForwardError::OutcomeUnknown(diagnostic)) => {
                    server_requests::mark_response_indeterminate(
                        cutex_session_id,
                        expected_runtime_generation,
                        &response.response_id,
                    )?;
                    write_v2_server_response_error(
                        stream,
                        409,
                        "Conflict",
                        Some(&response.response_id),
                        "server_request_resolved",
                        "the native server request response write has an indeterminate outcome",
                        false,
                        json!({
                            "nativeRequestId": native_request_id,
                            "runtimeGeneration": expected_runtime_generation,
                            "deliveryOutcome": "unknown",
                            "diagnostic": diagnostic,
                        }),
                    )
                }
            }
        }
        ServerResponseClaimDecision::InProgress => write_v2_server_response_error(
            stream,
            202,
            "Accepted",
            Some(&response.response_id),
            "request_in_progress",
            "an identical request is still in progress",
            true,
            json!({ "retryAfterMs": 250 }),
        ),
        ServerResponseClaimDecision::Deduplicated => write_server_response_result(
            stream,
            cutex_session_id,
            &response.response_id,
            "deduplicated",
            response.native_message,
        ),
        ServerResponseClaimDecision::IdempotencyConflict => write_v2_server_response_error(
            stream,
            409,
            "Conflict",
            Some(&response.response_id),
            "idempotency_conflict",
            "responseId was already used with a different response body",
            false,
            json!({ "cutexSessionId": cutex_session_id }),
        ),
        ServerResponseClaimDecision::Resolved => write_server_request_resolved(
            stream,
            &response.response_id,
            native_request_id,
            expected_runtime_generation,
        ),
        ServerResponseClaimDecision::NotFound => write_v2_server_response_error(
            stream,
            404,
            "Not Found",
            Some(&response.response_id),
            "server_request_not_found",
            "the exact native server request is not pending for this runtime generation",
            false,
            json!({
                "nativeRequestId": native_request_id,
                "runtimeGeneration": expected_runtime_generation,
            }),
        ),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ValidatedServerResponse {
    response_id: String,
    native_message: Value,
}

fn parse_runtime_generation_precondition(value: &str) -> Result<u64, String> {
    let generation = value
        .strip_prefix("\"cutex-runtime-generation:")
        .and_then(|value| value.strip_suffix('"'))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "If-Match must be a single quoted cutex-runtime-generation token".to_string()
        })?
        .parse::<u64>()
        .map_err(|_| "If-Match runtime generation must be an integer".to_string())?;
    if generation == 0 || generation > MAX_SAFE_SEQUENCE {
        return Err("If-Match runtime generation must be a positive JSON-safe integer".to_string());
    }
    Ok(generation)
}

fn validate_server_response_body(body: &[u8]) -> Result<ValidatedServerResponse, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("invalid JSON request body: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "request body must be a JSON object".to_string())?;
    if object.len() != 2 || !object.contains_key("responseId") || !object.contains_key("native") {
        return Err("request body must contain exactly responseId and native".to_string());
    }
    let response_id = object
        .get("responseId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 256)
        .ok_or_else(|| {
            "responseId must be a non-empty string of at most 256 characters".to_string()
        })?
        .to_string();
    let native = object
        .get("native")
        .and_then(Value::as_object)
        .ok_or_else(|| "native must be an object".to_string())?;
    if native.len() != 1 || !native.contains_key("message") {
        return Err("native must contain exactly message".to_string());
    }
    let native_message = native
        .get("message")
        .cloned()
        .ok_or_else(|| "native.message is required".to_string())?;
    let message = native_message
        .as_object()
        .ok_or_else(|| "native.message must be an object".to_string())?;
    let id = message
        .get("id")
        .filter(|id| !id.is_null())
        .ok_or_else(|| "native.message.id is required".to_string())?;
    if !id.is_string() && id.as_i64().is_none() {
        return Err("native.message.id must be a string or signed 64-bit integer".to_string());
    }
    let has_result = message.contains_key("result");
    let has_error = message.contains_key("error");
    if has_result == has_error {
        return Err("native.message must contain exactly one of result or error".to_string());
    }
    if let Some(error) = message.get("error") {
        let error = error
            .as_object()
            .ok_or_else(|| "native.message.error must be an object".to_string())?;
        if error.get("code").and_then(Value::as_i64).is_none()
            || error.get("message").and_then(Value::as_str).is_none()
        {
            return Err(
                "native.message.error requires integer code and string message".to_string(),
            );
        }
    }
    Ok(ValidatedServerResponse {
        response_id,
        native_message,
    })
}

fn write_server_response_result(
    stream: &mut TcpStream,
    cutex_session_id: &str,
    response_id: &str,
    disposition: &str,
    native_message: Value,
) -> anyhow::Result<()> {
    write_json_response(
        stream,
        200,
        "OK",
        &json!({
            "contractVersion": 2,
            "responseId": response_id,
            "cutexSessionId": cutex_session_id,
            "disposition": disposition,
            "native": { "message": native_message },
        }),
    )
}

fn write_stale_runtime_generation(
    stream: &mut TcpStream,
    response_id: &str,
    expected_runtime_generation: u64,
    actual_runtime_generation: Option<u64>,
) -> anyhow::Result<()> {
    write_v2_server_response_error(
        stream,
        412,
        "Precondition Failed",
        Some(response_id),
        "stale_runtime_generation",
        "the app-server runtime generation no longer matches this request occurrence",
        false,
        json!({
            "expectedRuntimeGeneration": expected_runtime_generation,
            "actualRuntimeGeneration": actual_runtime_generation,
            "claimCreated": false,
            "nativeResponseForwarded": false,
        }),
    )
}

fn write_server_request_resolved(
    stream: &mut TcpStream,
    response_id: &str,
    native_request_id: Value,
    runtime_generation: u64,
) -> anyhow::Result<()> {
    write_v2_server_response_error(
        stream,
        409,
        "Conflict",
        Some(response_id),
        "server_request_resolved",
        "the native server request was already resolved",
        false,
        json!({
            "nativeRequestId": native_request_id,
            "runtimeGeneration": runtime_generation,
        }),
    )
}

fn handle_native_request(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
    event_repository: &EventRepository,
    cutex_session_id: &str,
) -> anyhow::Result<()> {
    let request_id_hint = request_id_hint(&request.body);
    let registry = (context.load_registry)()?;
    let Some(session) = session_resource(
        cutex_session_id,
        &registry,
        context.load_runtime_status,
        event_repository,
    )?
    else {
        return write_v2_request_error(
            stream,
            404,
            "Not Found",
            request_id_hint.as_deref(),
            "session_not_found",
            "the owner-visible cutex session does not exist on this host",
            false,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    };
    let idempotency = request_idempotency_repository()?;
    handle_native_request_for_session(
        stream,
        request,
        context.forward_native_request,
        cutex_session_id,
        &session,
        idempotency,
    )
}

fn handle_native_request_for_session(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    forward_native_request: ManagementNativeRequestHandler,
    cutex_session_id: &str,
    session: &Value,
    idempotency: &RequestIdempotencyRepository,
) -> anyhow::Result<()> {
    let request_id_hint = request_id_hint(&request.body);
    let Some(bound_thread_id) = session
        .pointer("/native/threadId")
        .and_then(Value::as_str)
        .filter(|thread_id| !thread_id.is_empty())
    else {
        return write_v2_request_error(
            stream,
            409,
            "Conflict",
            request_id_hint.as_deref(),
            "session_offline",
            "the cutex session has no bound app-server thread",
            true,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    };
    let native_request = match validate_native_request(&request.body, bound_thread_id) {
        Ok(request) => request,
        Err(NativeRequestValidationError::Invalid(message)) => {
            return write_v2_request_error(
                stream,
                400,
                "Bad Request",
                request_id_hint.as_deref(),
                "invalid_request",
                &message,
                false,
                json!({}),
            );
        }
        Err(NativeRequestValidationError::MethodDenied(method)) => {
            return write_v2_request_error(
                stream,
                403,
                "Forbidden",
                request_id_hint.as_deref(),
                "native_method_denied",
                "the native request method is not authorized by policy version 2",
                false,
                json!({
                    "method": method,
                    "policyVersion": NATIVE_REQUEST_POLICY_VERSION,
                }),
            );
        }
        Err(NativeRequestValidationError::MissingThreadId) => {
            return write_v2_request_error(
                stream,
                400,
                "Bad Request",
                request_id_hint.as_deref(),
                "missing_thread_id",
                "the native request requires params.threadId",
                false,
                json!({}),
            );
        }
        Err(NativeRequestValidationError::ForeignThread { requested, bound }) => {
            return write_v2_request_error(
                stream,
                403,
                "Forbidden",
                request_id_hint.as_deref(),
                "foreign_thread",
                "the native request thread does not match the bound cutex session thread",
                false,
                json!({ "requestedThreadId": requested, "boundThreadId": bound }),
            );
        }
    };
    if !session
        .pointer("/runtime/appServerConnected")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return write_v2_request_error(
            stream,
            409,
            "Conflict",
            Some(&native_request.request_id),
            "session_offline",
            "the cutex session has no connected app-server",
            true,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    }

    let canonical_body: Value = serde_json::from_slice(&request.body)
        .expect("validated native request body must remain valid JSON");
    let claim = match idempotency.begin(
        cutex_session_id,
        &native_request.request_id,
        &canonical_body,
    ) {
        Ok(BeginRequest::Forward(claim)) => claim,
        Ok(BeginRequest::InProgress) => {
            return write_v2_request_error(
                stream,
                202,
                "Accepted",
                Some(&native_request.request_id),
                "request_in_progress",
                "an identical request is still in progress",
                true,
                json!({ "retryAfterMs": 250 }),
            );
        }
        Ok(BeginRequest::Completed(response)) => {
            return write_json_response(stream, response.status, &response.reason, &response.body);
        }
        Ok(BeginRequest::Conflict) => {
            return write_v2_request_error(
                stream,
                409,
                "Conflict",
                Some(&native_request.request_id),
                "idempotency_conflict",
                "requestId was already used with a different request body",
                false,
                json!({ "cutexSessionId": cutex_session_id }),
            );
        }
        Ok(BeginRequest::OutcomeUnknown) => {
            return write_request_outcome_unknown(
                stream,
                &native_request.request_id,
                json!({ "resyncRequired": true }),
            );
        }
        Err(error) => {
            return write_v2_request_error(
                stream,
                503,
                "Service Unavailable",
                Some(&native_request.request_id),
                "event_persistence_unavailable",
                "durable management request state is unavailable",
                true,
                json!({ "diagnostic": format!("{error:#}") }),
            );
        }
    };

    match forward_native_request(cutex_session_id, native_request.native_message) {
        Ok(native_response) => {
            let response = json!({
                "contractVersion": 2,
                "requestId": native_request.request_id,
                "cutexSessionId": cutex_session_id,
                "native": { "message": native_response },
            });
            if let Err(error) = idempotency.complete(&claim, 200, "OK", response.clone()) {
                return write_v2_request_error(
                    stream,
                    503,
                    "Service Unavailable",
                    response.get("requestId").and_then(Value::as_str),
                    "event_persistence_unavailable",
                    "the native response could not be durably recorded",
                    false,
                    json!({
                        "diagnostic": format!("{error:#}"),
                        "resyncRequired": true,
                    }),
                );
            }
            write_json_response(stream, 200, "OK", &response)
        }
        Err(ManagementNativeForwardError::BeforeForward(diagnostic)) => {
            idempotency.release_before_forward(&claim)?;
            write_v2_request_error(
                stream,
                502,
                "Bad Gateway",
                Some(&native_request.request_id),
                "app_server_unavailable",
                "the current app-server connection failed before forwarding the request",
                true,
                json!({ "diagnostic": diagnostic }),
            )
        }
        Err(ManagementNativeForwardError::NativeRequestIdInUse(diagnostic)) => {
            idempotency.release_before_forward(&claim)?;
            write_v2_request_error(
                stream,
                409,
                "Conflict",
                Some(&native_request.request_id),
                "native_request_id_in_use",
                "the exact native request id is already pending on this app-server connection",
                true,
                json!({ "diagnostic": diagnostic }),
            )
        }
        Err(ManagementNativeForwardError::StaleRuntimeGeneration { expected, actual }) => {
            idempotency.release_before_forward(&claim)?;
            write_v2_request_error(
                stream,
                409,
                "Conflict",
                Some(&native_request.request_id),
                "session_runtime_changed",
                "the app-server runtime changed before forwarding the native request",
                true,
                json!({
                    "expectedRuntimeGeneration": expected,
                    "actualRuntimeGeneration": actual,
                    "resyncRequired": true,
                }),
            )
        }
        Err(ManagementNativeForwardError::OutcomeUnknown(diagnostic)) => {
            idempotency.mark_outcome_unknown(&claim)?;
            write_request_outcome_unknown(
                stream,
                &native_request.request_id,
                json!({ "resyncRequired": true, "diagnostic": diagnostic }),
            )
        }
    }
}

fn handle_cutex_request(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
    event_repository: &EventRepository,
    cutex_session_id: &str,
) -> anyhow::Result<()> {
    let request_lock = cutex_request_lock(cutex_session_id)?;
    let _request_guard = request_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("management v2 cutex request lock was poisoned"))?;
    let request_id_hint = request_id_hint(&request.body);
    let cutex_request = match validate_cutex_request_body(&request.body) {
        Ok(request) => request,
        Err(error) => {
            return write_cutex_request_validation_error(stream, request_id_hint.as_deref(), error);
        }
    };
    let registry = (context.load_registry)()?;
    let session = if matches!(
        cutex_request.method.as_str(),
        "cutex/session/retire" | "cutex/session/restore"
    ) {
        session_resource_including_archive(
            cutex_session_id,
            &registry,
            context.load_runtime_status,
            event_repository,
        )?
    } else if request_may_resolve_hidden_session(&cutex_request.method) {
        session_resource_including_hidden(
            cutex_session_id,
            &registry,
            context.load_runtime_status,
            event_repository,
        )?
    } else {
        session_resource(
            cutex_session_id,
            &registry,
            context.load_runtime_status,
            event_repository,
        )?
    };
    let Some(session) = session else {
        return write_v2_request_error(
            stream,
            404,
            "Not Found",
            request_id_hint.as_deref(),
            "session_not_found",
            "the owner-visible cutex session does not exist on this host",
            false,
            json!({ "cutexSessionId": cutex_session_id }),
        );
    };
    let canonical_body: Value = serde_json::from_slice(&request.body)
        .expect("validated cutex request body must remain valid JSON");
    let idempotency = request_idempotency_repository()?;
    let claim =
        match idempotency.begin(cutex_session_id, &cutex_request.request_id, &canonical_body) {
            Ok(BeginRequest::Forward(claim)) => claim,
            Ok(BeginRequest::InProgress) => {
                return write_v2_request_error(
                    stream,
                    202,
                    "Accepted",
                    Some(&cutex_request.request_id),
                    "request_in_progress",
                    "an identical request is still in progress",
                    true,
                    json!({ "retryAfterMs": 250 }),
                );
            }
            Ok(BeginRequest::Completed(response)) => {
                return write_json_response(
                    stream,
                    response.status,
                    &response.reason,
                    &response.body,
                );
            }
            Ok(BeginRequest::Conflict) => {
                return write_v2_request_error(
                    stream,
                    409,
                    "Conflict",
                    Some(&cutex_request.request_id),
                    "idempotency_conflict",
                    "requestId was already used with a different request body",
                    false,
                    json!({ "cutexSessionId": cutex_session_id }),
                );
            }
            Ok(BeginRequest::OutcomeUnknown) => {
                return write_request_outcome_unknown(
                    stream,
                    &cutex_request.request_id,
                    json!({ "resyncRequired": true }),
                );
            }
            Err(error) => {
                return write_v2_request_error(
                    stream,
                    503,
                    "Service Unavailable",
                    Some(&cutex_request.request_id),
                    "event_persistence_unavailable",
                    "durable management request state is unavailable",
                    true,
                    json!({ "diagnostic": format!("{error:#}") }),
                );
            }
        };

    let operation = dispatch_cutex_request(
        event_repository,
        context.handle_user_input,
        context.flush_user_input_queue,
        context.mutate_session,
        cutex_session_id,
        &session,
        &cutex_request,
    );
    match operation {
        Ok(result) => {
            let response = json!({
                "contractVersion": 2,
                "requestId": cutex_request.request_id,
                "cutexSessionId": cutex_session_id,
                "cutex": {
                    "method": cutex_request.method,
                    "result": result,
                }
            });
            if let Err(error) = idempotency.complete(&claim, 200, "OK", response.clone()) {
                return write_post_cutex_operation_outcome_unknown(
                    stream,
                    idempotency,
                    &claim,
                    &cutex_request.request_id,
                    error,
                );
            }
            write_json_response(stream, 200, "OK", &response)
        }
        Err(error) => write_cutex_operation_error(
            stream,
            idempotency,
            &claim,
            &cutex_request.request_id,
            error,
        ),
    }
}

fn write_cutex_request_validation_error(
    stream: &mut TcpStream,
    request_id: Option<&str>,
    error: CutexRequestValidationError,
) -> anyhow::Result<()> {
    match error {
        CutexRequestValidationError::Invalid(message) => write_v2_request_error(
            stream,
            400,
            "Bad Request",
            request_id,
            "invalid_request",
            &message,
            false,
            json!({}),
        ),
        CutexRequestValidationError::UnsupportedMethod(method) => write_v2_request_error(
            stream,
            403,
            "Forbidden",
            request_id,
            "cutex_method_denied",
            "the cutex request method is not present in the advertised registry",
            false,
            json!({
                "method": method,
                "registryVersion": super::session::CUTEX_METHOD_REGISTRY_VERSION,
            }),
        ),
    }
}

fn write_cutex_operation_error(
    stream: &mut TcpStream,
    idempotency: &RequestIdempotencyRepository,
    claim: &RequestClaim,
    request_id: &str,
    error: super::user_input::UserInputExecutionError,
) -> anyhow::Result<()> {
    let status = cutex_operation_error_status(&error.code, error.outcome_unknown);
    let reason = http_reason(status);
    let outcome_unknown = error.outcome_unknown;
    let response_code = if error.outcome_unknown {
        "request_outcome_unknown".to_string()
    } else {
        error.code.clone()
    };
    let response_message = if error.outcome_unknown {
        "the cutex operation completed far enough that its durable outcome is unknown".to_string()
    } else {
        error.message.clone()
    };
    let mut body = json!({
        "contractVersion": 2,
        "requestId": request_id,
        "error": {
            "source": "cutex",
            "code": response_code,
            "message": response_message,
            "retryable": if outcome_unknown { false } else { error.retryable },
            "details": if outcome_unknown {
                json!({
                    "resyncRequired": true,
                    "originalCode": error.code,
                    "originalDetails": error.details,
                })
            } else {
                error.details
            },
        }
    });
    if outcome_unknown {
        if let Err(mark_error) = idempotency.mark_outcome_unknown(claim) {
            body.pointer_mut("/error/details")
                .and_then(Value::as_object_mut)
                .expect("outcome-unknown details are an object")
                .insert(
                    "idempotencyDiagnostic".to_string(),
                    Value::String(format!("{mark_error:#}")),
                );
        }
    } else {
        idempotency.complete(claim, status, reason, body.clone())?;
    }
    write_json_response(stream, status, reason, &body)
}

fn write_post_cutex_operation_outcome_unknown(
    stream: &mut TcpStream,
    idempotency: &RequestIdempotencyRepository,
    claim: &RequestClaim,
    request_id: &str,
    persistence_error: anyhow::Error,
) -> anyhow::Result<()> {
    let mut details = json!({
        "resyncRequired": true,
        "originalCode": "response_persistence_failed",
        "diagnostic": format!("{persistence_error:#}"),
    });
    if let Err(mark_error) = idempotency.mark_outcome_unknown(claim) {
        details
            .as_object_mut()
            .expect("outcome-unknown details are an object")
            .insert(
                "idempotencyDiagnostic".to_string(),
                Value::String(format!("{mark_error:#}")),
            );
    }
    write_v2_request_error(
        stream,
        409,
        "Conflict",
        Some(request_id),
        "request_outcome_unknown",
        "the cutex operation completed but its response could not be durably recorded",
        false,
        details,
    )
}

fn request_may_resolve_hidden_session(method: &str) -> bool {
    method == "cutex/session/visibility/show"
}

fn cutex_request_lock(cutex_session_id: &str) -> anyhow::Result<Arc<Mutex<()>>> {
    let mut locks = CUTEX_REQUEST_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("management v2 cutex request lock registry was poisoned"))?;
    Ok(locks
        .entry(cutex_session_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ValidatedCutexRequest {
    pub(super) request_id: String,
    pub(super) method: String,
    pub(super) params: Value,
}

#[derive(Debug, Clone, PartialEq)]
enum CutexRequestValidationError {
    Invalid(String),
    UnsupportedMethod(String),
}

fn validate_cutex_request_body(
    body: &[u8],
) -> Result<ValidatedCutexRequest, CutexRequestValidationError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        CutexRequestValidationError::Invalid(format!("invalid JSON request body: {error}"))
    })?;
    let object = value.as_object().ok_or_else(|| {
        CutexRequestValidationError::Invalid("request body must be a JSON object".to_string())
    })?;
    if object.len() != 3
        || !object.contains_key("requestId")
        || !object.contains_key("method")
        || !object.contains_key("params")
    {
        return Err(CutexRequestValidationError::Invalid(
            "request body must contain exactly requestId, method, and params".to_string(),
        ));
    }
    let request_id = object
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.chars().count() <= 256)
        .ok_or_else(|| {
            CutexRequestValidationError::Invalid(
                "requestId must be a non-empty string of at most 256 characters".to_string(),
            )
        })?
        .to_string();
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| method.starts_with("cutex/") && method.matches('/').count() >= 2)
        .ok_or_else(|| {
            CutexRequestValidationError::Invalid(
                "method must be a namespaced cutex method".to_string(),
            )
        })?
        .to_string();
    if !cutex_method_is_registered(&method) {
        return Err(CutexRequestValidationError::UnsupportedMethod(method));
    }
    let params = object
        .get("params")
        .filter(|params| params.is_object())
        .cloned()
        .ok_or_else(|| {
            CutexRequestValidationError::Invalid("params must be an object".to_string())
        })?;
    super::contract_validation::validate_cutex_request(&value)
        .map_err(CutexRequestValidationError::Invalid)?;
    Ok(ValidatedCutexRequest {
        request_id,
        method,
        params,
    })
}

fn dispatch_cutex_request(
    event_repository: &EventRepository,
    handle_user_input: ManagementUserInputHandler,
    flush_user_input_queue: ManagementUserInputQueueFlusher,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    match request.method.as_str() {
        "cutex/session/get" => {
            require_empty_params(&request.params)?;
            Ok(session.clone())
        }
        "cutex/session/retire" => super::archive::dispatch_session_retire(
            event_repository,
            mutate_session,
            cutex_session_id,
            session,
            request,
        ),
        "cutex/session/restore" => super::archive::dispatch_session_restore(
            event_repository,
            mutate_session,
            cutex_session_id,
            session,
            request,
        ),
        "cutex/session/groups/get" => {
            require_empty_params(&request.params)?;
            Ok(json!({
                "revision": session.get("revision").cloned().unwrap_or(json!(1)),
                "groups": session.pointer("/runtimeDefaults/groups").cloned().unwrap_or(json!([])),
            }))
        }
        "cutex/session/defaults/update" => dispatch_session_defaults_update(
            event_repository,
            mutate_session,
            cutex_session_id,
            session,
            request,
        ),
        "cutex/session/profile/set" | "cutex/session/profile/clear" => {
            dispatch_session_profile_mutation(
                event_repository,
                mutate_session,
                cutex_session_id,
                session,
                request,
            )
        }
        "cutex/session/groups/set" | "cutex/session/groups/add" | "cutex/session/groups/remove" => {
            dispatch_session_groups_mutation(
                event_repository,
                mutate_session,
                cutex_session_id,
                session,
                request,
            )
        }
        "cutex/session/visibility/show" | "cutex/session/visibility/hide" => {
            dispatch_session_visibility_mutation(
                event_repository,
                mutate_session,
                cutex_session_id,
                session,
                request,
            )
        }
        "cutex/runtime/online" | "cutex/runtime/offline" | "cutex/runtime/close" => {
            dispatch_runtime_mutation(
                event_repository,
                mutate_session,
                cutex_session_id,
                session,
                request,
            )
        }
        "cutex/focus/get" => {
            require_empty_params(&request.params)?;
            focus_resource(cutex_session_id).map_err(persistence_user_input_error)
        }
        "cutex/focus/set" => dispatch_focus_set(event_repository, cutex_session_id, request),
        "cutex/focus/clear" => dispatch_focus_clear(event_repository, cutex_session_id, request),
        "cutex/userInput/submit" => dispatch_user_input_submit(
            event_repository,
            handle_user_input,
            cutex_session_id,
            session,
            request,
        ),
        "cutex/userInput/queue/list" => {
            require_empty_params(&request.params)?;
            let (revision, items) = user_input_repository()
                .and_then(|repository| repository.list(cutex_session_id))
                .map_err(persistence_user_input_error)?;
            Ok(json!({ "revision": revision, "items": items }))
        }
        "cutex/userInput/queue/update" => {
            dispatch_queue_update(event_repository, cutex_session_id, request)
        }
        "cutex/userInput/queue/remove" => {
            dispatch_queue_remove(event_repository, cutex_session_id, request)
        }
        "cutex/userInput/queue/flush" => {
            dispatch_queue_flush(cutex_session_id, request, flush_user_input_queue)
        }
        _ => Err(super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "cutex_method_denied".to_string(),
            message: "the cutex request method is not present in the advertised registry"
                .to_string(),
            retryable: false,
            details: json!({
                "method": request.method,
                "registryVersion": super::session::CUTEX_METHOD_REGISTRY_VERSION,
            }),
            outcome_unknown: false,
        }),
    }
}

fn dispatch_runtime_mutation(
    event_repository: &EventRepository,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = request
        .params
        .as_object()
        .ok_or_else(|| invalid_user_input_error("params must be an object"))?;
    let online = request.method == "cutex/runtime/online";
    let max_fields = if online { 4 } else { 3 };
    if object.len() > max_fields
        || !object.contains_key("expectedRuntimeGeneration")
        || object.keys().any(|key| {
            !matches!(key.as_str(), "expectedRuntimeGeneration" | "reason")
                && !if online {
                    matches!(key.as_str(), "openVisibleTerminal" | "launchProfile")
                } else {
                    key == "force"
                }
        })
    {
        return Err(invalid_user_input_error(
            "runtime params contain a field outside the method's v2 schema",
        ));
    }
    let expected_generation = required_safe_integer_param(object, "expectedRuntimeGeneration")?;
    let current_generation = session
        .pointer("/runtime/runtimeGeneration")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if expected_generation != current_generation {
        return Err(super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "revision_conflict".to_string(),
            message: format!(
                "runtime generation conflict: expected {expected_generation}, current {current_generation}"
            ),
            retryable: true,
            details: json!({
                "expectedRuntimeGeneration": expected_generation,
                "currentRuntimeGeneration": current_generation,
                "resyncRequired": true,
            }),
            outcome_unknown: false,
        });
    }
    let reason = match object.get("reason") {
        Some(Value::String(reason)) => Some(reason.clone()),
        Some(_) => return Err(invalid_user_input_error("reason must be a string")),
        None => None,
    };
    let optional_boolean_key = if online {
        "openVisibleTerminal"
    } else {
        "force"
    };
    if object
        .get(optional_boolean_key)
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(invalid_user_input_error(&format!(
            "{optional_boolean_key} must be a boolean"
        )));
    }
    let launch_profile = match object.get("launchProfile") {
        Some(Value::String(profile)) if !profile.trim().is_empty() => {
            Some(profile.trim().to_string())
        }
        Some(_) => {
            return Err(invalid_user_input_error(
                "launchProfile must be a non-empty string",
            ))
        }
        None => None,
    };
    let target_generation = current_generation;
    let backend = session
        .pointer("/runtime/backend")
        .and_then(Value::as_str)
        .unwrap_or("host");
    let runtime_agent_id = session.pointer("/runtime/runtimeAgentId").cloned();
    let (requested_method, requested_status, terminal_method) = match request.method.as_str() {
        "cutex/runtime/online" => (
            "cutex/runtime/onlineRequested",
            "starting",
            "cutex/runtime/online",
        ),
        "cutex/runtime/offline" => (
            "cutex/runtime/offlineRequested",
            "closing",
            "cutex/runtime/offline",
        ),
        "cutex/runtime/close" => ("cutex/runtime/closing", "closing", "cutex/runtime/closed"),
        _ => unreachable!(),
    };
    let correlation = super::model::EventCorrelation {
        thread_id: session
            .pointer("/native/threadId")
            .and_then(Value::as_str)
            .map(str::to_string),
        management_request_id: Some(request.request_id.clone()),
        ..Default::default()
    };
    append_cutex_event(
        event_repository,
        cutex_session_id,
        correlation.clone(),
        requested_method,
        runtime_event_params(
            target_generation,
            backend,
            requested_status,
            runtime_agent_id.clone(),
            reason.clone(),
        ),
    )
    .map_err(persistence_user_input_error)?;

    let mut mutation_params = request.params.clone();
    if let Some(profile) = launch_profile.as_deref() {
        mutation_params
            .as_object_mut()
            .expect("validated runtime params object")
            .insert("launchProfile".to_string(), json!(profile));
    }
    let result = match mutate_session(cutex_session_id, &request.method, mutation_params) {
        Ok(result) => result,
        Err(error) => {
            let mut failed_params = runtime_event_params(
                target_generation,
                backend,
                "error",
                runtime_agent_id.clone(),
                reason.clone(),
            );
            failed_params
                .as_object_mut()
                .expect("runtime failed params object")
                .insert(
                    "error".to_string(),
                    json!({
                        "source": "cutex",
                        "code": error.code,
                        "message": error.message,
                        "retryable": error.retryable,
                        "details": error.details,
                    }),
                );
            append_cutex_event(
                event_repository,
                cutex_session_id,
                correlation,
                "cutex/runtime/failed",
                failed_params,
            )
            .map_err(post_operation_persistence_error)?;
            return Err(error);
        }
    };
    let result_generation = result
        .get("runtimeGeneration")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_user_input_error("runtime mutation omitted runtimeGeneration"))?;
    let result_status = result
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| {
            matches!(
                *status,
                "starting" | "online" | "offline" | "closing" | "closed" | "error"
            )
        })
        .ok_or_else(|| invalid_user_input_error("runtime mutation returned an invalid status"))?;
    let result_runtime_agent_id = result.get("runtimeAgentId").cloned().or(runtime_agent_id);
    let launch_profile_receipt =
        validate_launch_profile_receipt(launch_profile.as_deref(), result.get("launchProfile"))?;
    let foreground_required_reason = match result.get("foregroundRequiredReason") {
        Some(Value::String(reason)) if !reason.is_empty() => Some(reason.clone()),
        Some(_) => {
            return Err(invalid_user_input_error(
                "runtime mutation returned an invalid foregroundRequiredReason",
            ))
        }
        None => None,
    };
    let terminal_method = match result_status {
        "starting" => "cutex/runtime/onlineRequested",
        "closing" => "cutex/runtime/closing",
        _ => terminal_method,
    };
    append_cutex_event(
        event_repository,
        cutex_session_id,
        correlation.clone(),
        terminal_method,
        runtime_event_params(
            result_generation,
            backend,
            result_status,
            result_runtime_agent_id,
            reason,
        ),
    )
    .map_err(post_operation_persistence_error)?;
    if let Some(reason) = foreground_required_reason {
        append_cutex_event(
            event_repository,
            cutex_session_id,
            correlation,
            "cutex/host/foregroundRequired",
            json!({
                "runtimeGeneration": result_generation,
                "reason": reason,
                "requiredAt": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .map_err(post_operation_persistence_error)?;
    }
    let mut response = json!({
        "runtimeGeneration": result_generation,
        "status": result_status,
    });
    if let Some(receipt) = launch_profile_receipt {
        response
            .as_object_mut()
            .expect("runtime mutation response object")
            .insert("launchProfile".to_string(), receipt);
    }
    Ok(response)
}

fn validate_launch_profile_receipt(
    requested: Option<&str>,
    receipt: Option<&Value>,
) -> Result<Option<Value>, super::user_input::UserInputExecutionError> {
    let Some(receipt) = receipt else {
        return requested.map_or(Ok(None), |_| {
            Err(invalid_user_input_error(
                "runtime mutation omitted launchProfile for an explicit override request",
            ))
        });
    };
    let object = receipt.as_object().ok_or_else(|| {
        invalid_user_input_error("runtime mutation returned an invalid launchProfile receipt")
    })?;
    let source = object.get("source").and_then(Value::as_str);
    if object.len() != 6
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "requested"
                    | "selected"
                    | "effective"
                    | "source"
                    | "applicationScope"
                    | "persisted"
            )
        })
        || requested.is_some_and(|requested| {
            object.get("requested").and_then(Value::as_str) != Some(requested)
        })
        || requested.is_none()
            && !matches!(
                source,
                Some("session_configured" | "global_default" | "unknown")
            )
        || object
            .get("effective")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        || object.get("selected").and_then(Value::as_str)
            != object.get("effective").and_then(Value::as_str)
        || !matches!(
            source,
            Some("one_launch_override" | "session_configured" | "global_default" | "unknown")
        )
        || requested.is_some() && source != Some("one_launch_override")
        || !object
            .get("applicationScope")
            .and_then(Value::as_str)
            .is_some_and(|scope| matches!(scope, "runtime" | "tui" | "runtime_and_tui"))
        || object.get("persisted").and_then(Value::as_bool) != Some(false)
    {
        return Err(invalid_user_input_error(
            "runtime mutation returned an invalid launchProfile receipt",
        ));
    }
    Ok(Some(receipt.clone()))
}

fn runtime_event_params(
    runtime_generation: u64,
    backend: &str,
    status: &str,
    runtime_agent_id: Option<Value>,
    reason: Option<String>,
) -> Value {
    let mut params = json!({
        "runtimeGeneration": runtime_generation,
        "backend": backend,
        "status": status,
        "runtimeAgentId": runtime_agent_id.unwrap_or(Value::Null),
        "occurredAt": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(reason) = reason {
        params
            .as_object_mut()
            .expect("runtime event params object")
            .insert("reason".to_string(), Value::String(reason));
    }
    params
}

fn dispatch_session_defaults_update(
    event_repository: &EventRepository,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["expectedRevision", "patch"])?;
    require_session_revision(session, object)?;
    let patch = object
        .get("patch")
        .and_then(Value::as_object)
        .filter(|patch| !patch.is_empty())
        .ok_or_else(|| invalid_user_input_error("patch must be a non-empty object"))?;
    let allowed = [
        "backend",
        "managedCwd",
        "permissions",
        "approvalPolicy",
        "sandboxMode",
        "model",
        "reasoningEffort",
        "cliArgs",
        "groups",
    ];
    if patch.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid_user_input_error(
            "defaults patch contains a field outside registry version 2",
        ));
    }
    mutate_session(cutex_session_id, &request.method, request.params.clone())?;
    let revision = next_session_revision(session)?;
    let mut runtime_defaults = session
        .get("runtimeDefaults")
        .cloned()
        .unwrap_or_else(|| json!({}));
    for (key, value) in patch {
        runtime_defaults
            .as_object_mut()
            .expect("runtimeDefaults is an object")
            .insert(key.clone(), value.clone());
    }
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/session/defaultsUpdated",
        json!({
            "sessionRevision": revision,
            "runtimeDefaults": runtime_defaults,
            "updatedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(json!({ "revision": revision }))
}

fn dispatch_session_profile_mutation(
    event_repository: &EventRepository,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let expected = if request.method == "cutex/session/profile/set" {
        exact_params(&request.params, &["expectedRevision", "profile"])?
    } else {
        exact_params(&request.params, &["expectedRevision"])?
    };
    require_session_revision(session, expected)?;
    if request.method == "cutex/session/profile/set"
        && expected
            .get("profile")
            .and_then(Value::as_str)
            .is_none_or(|profile| profile.trim().is_empty())
    {
        return Err(invalid_user_input_error(
            "profile must be a non-empty string",
        ));
    }
    let result = mutate_session(cutex_session_id, &request.method, request.params.clone())?;
    let revision = result
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_user_input_error("profile mutation omitted revision"))?;
    let configured_profile = result
        .get("configuredProfile")
        .cloned()
        .unwrap_or(Value::Null);
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/session/profileUpdated",
        json!({
            "sessionRevision": revision,
            "configuredProfile": configured_profile,
            "updatedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(json!({
        "cutexSessionId": cutex_session_id,
        "revision": revision,
        "configuredProfile": configured_profile,
    }))
}

fn dispatch_session_groups_mutation(
    event_repository: &EventRepository,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["expectedRevision", "groups"])?;
    require_session_revision(session, object)?;
    let groups = object
        .get("groups")
        .and_then(Value::as_array)
        .filter(|groups| !groups.is_empty())
        .ok_or_else(|| invalid_user_input_error("groups must be a non-empty array"))?;
    let mut normalized = Vec::new();
    for group in groups {
        let group = group
            .as_str()
            .filter(|group| !group.is_empty())
            .ok_or_else(|| invalid_user_input_error("groups must contain non-empty strings"))?;
        if normalized.iter().any(|existing| existing == group) {
            return Err(invalid_user_input_error("groups must be unique"));
        }
        normalized.push(group.to_string());
    }
    let mutation_result =
        mutate_session(cutex_session_id, &request.method, request.params.clone())?;
    let resulting_groups = mutation_result
        .get("groups")
        .cloned()
        .unwrap_or_else(|| json!(normalized));
    let revision = next_session_revision(session)?;
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/session/groupsUpdated",
        json!({
            "sessionRevision": revision,
            "groups": resulting_groups,
            "updatedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(json!({ "revision": revision, "groups": resulting_groups }))
}

fn dispatch_session_visibility_mutation(
    event_repository: &EventRepository,
    mutate_session: ManagementSessionMutationHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["expectedRevision"])?;
    require_session_revision(session, object)?;
    mutate_session(cutex_session_id, &request.method, request.params.clone())?;
    let revision = next_session_revision(session)?;
    let visible = request.method.ends_with("/show");
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            thread_id: session
                .pointer("/native/threadId")
                .and_then(Value::as_str)
                .map(str::to_string),
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        if visible {
            "cutex/im/registered"
        } else {
            "cutex/im/unregistered"
        },
        json!({
            "registrationId": cutex_session_id,
            "provider": "cutex",
            "externalConversationId": session.pointer("/native/threadId")
                .and_then(Value::as_str)
                .unwrap_or(cutex_session_id),
            "changedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(json!({ "revision": revision }))
}

fn require_session_revision(
    session: &Value,
    params: &serde_json::Map<String, Value>,
) -> Result<(), super::user_input::UserInputExecutionError> {
    let expected_revision = required_safe_integer_param(params, "expectedRevision")?;
    let current_revision = session.get("revision").and_then(Value::as_u64).unwrap_or(0);
    if expected_revision != current_revision {
        return Err(super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "revision_conflict".to_string(),
            message: format!(
                "session revision conflict: expected {expected_revision}, current {current_revision}"
            ),
            retryable: true,
            details: json!({
                "expectedRevision": expected_revision,
                "currentRevision": current_revision,
                "resyncRequired": true,
            }),
            outcome_unknown: false,
        });
    }
    Ok(())
}

fn next_session_revision(
    session: &Value,
) -> Result<u64, super::user_input::UserInputExecutionError> {
    let current = session.get("revision").and_then(Value::as_u64).unwrap_or(0);
    if current >= super::model::MAX_SAFE_SEQUENCE {
        return Err(invalid_user_input_error(
            "session revision exhausted the JSON-safe integer range",
        ));
    }
    Ok(current + 1)
}

fn dispatch_focus_set(
    event_repository: &EventRepository,
    cutex_session_id: &str,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(
        &request.params,
        &["expectedRevision", "owner", "mobileMuted", "source"],
    )?;
    let expected_revision = required_safe_integer_param(object, "expectedRevision")?;
    let owner = required_string_param(object, "owner")?;
    let mobile_muted = object
        .get("mobileMuted")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid_user_input_error("mobileMuted must be boolean"))?;
    let source = required_string_param(object, "source")?.to_string();
    let focus = set_focus(
        cutex_session_id,
        expected_revision,
        owner,
        mobile_muted,
        Some(source),
    )
    .map_err(focus_mutation_error)?;
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/focus/changed",
        focus.clone(),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(focus)
}

fn dispatch_focus_clear(
    event_repository: &EventRepository,
    cutex_session_id: &str,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["expectedRevision"])?;
    let expected_revision = required_safe_integer_param(object, "expectedRevision")?;
    let focus = clear_focus(cutex_session_id, expected_revision).map_err(focus_mutation_error)?;
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/focus/changed",
        focus.clone(),
    )
    .map_err(post_operation_persistence_error)?;
    Ok(focus)
}

fn dispatch_user_input_submit(
    event_repository: &EventRepository,
    handle_user_input: ManagementUserInputHandler,
    cutex_session_id: &str,
    session: &Value,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let params = parse_user_input_submit_params(&request.params).map_err(|message| {
        super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "invalid_request".to_string(),
            message,
            retryable: false,
            details: json!({}),
            outcome_unknown: false,
        }
    })?;
    let thread_id = session
        .pointer("/native/threadId")
        .and_then(Value::as_str)
        .filter(|thread_id| !thread_id.is_empty())
        .ok_or_else(|| super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "session_offline".to_string(),
            message: "the cutex session has no bound app-server thread".to_string(),
            retryable: true,
            details: json!({ "cutexSessionId": cutex_session_id }),
            outcome_unknown: false,
        })?
        .to_string();
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            thread_id: Some(thread_id.clone()),
            client_user_message_id: Some(params.client_user_message_id.clone()),
            management_request_id: Some(request.request_id.clone()),
            ..Default::default()
        },
        "cutex/userInput/routeAccepted",
        json!({
            "managementRequestId": request.request_id,
            "clientUserMessageId": params.client_user_message_id,
            "origin": params.origin,
            "input": params.input,
            "strategy": params.strategy,
            "state": "routed_to_cutex",
            "acceptedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(persistence_user_input_error)?;
    let execution = handle_user_input(UserInputSubmitCommand {
        management_request_id: request.request_id.clone(),
        cutex_session_id: cutex_session_id.to_string(),
        thread_id: thread_id.clone(),
        params: params.clone(),
    });
    match execution {
        Ok(execution) => {
            if execution.disposition == UserInputDisposition::Queued {
                let queue = execution
                    .queue
                    .clone()
                    .expect("queued execution requires queue item");
                append_cutex_event(
                    event_repository,
                    cutex_session_id,
                    super::model::EventCorrelation {
                        thread_id: Some(thread_id),
                        client_user_message_id: Some(params.client_user_message_id.clone()),
                        management_request_id: Some(request.request_id.clone()),
                        queue_id: Some(queue.queue_id.clone()),
                        ..Default::default()
                    },
                    "cutex/userInput/queued",
                    serde_json::to_value(&queue).expect("queue item serialization"),
                )
                .map_err(post_operation_persistence_error)?;
            } else if matches!(
                execution.disposition,
                UserInputDisposition::Started | UserInputDisposition::Steered
            ) {
                append_cutex_event(
                    event_repository,
                    cutex_session_id,
                    super::model::EventCorrelation {
                        thread_id: Some(thread_id),
                        turn_id: execution.turn_id.clone(),
                        client_user_message_id: Some(params.client_user_message_id.clone()),
                        management_request_id: Some(request.request_id.clone()),
                        native_request_id: execution.native_request_id.clone(),
                        ..Default::default()
                    },
                    "cutex/userInput/submitted",
                    json!({
                        "managementRequestId": request.request_id,
                        "clientUserMessageId": params.client_user_message_id,
                        "origin": params.origin,
                        "input": params.input,
                        "disposition": execution.disposition,
                        "nativeRequestId": execution.native_request_id,
                        "nativeMethod": execution.native_method,
                        "turnId": execution.turn_id,
                        "appServerAccepted": true,
                        "submittedAt": chrono::Utc::now().to_rfc3339(),
                    }),
                )
                .map_err(post_operation_persistence_error)?;
            }
            Ok(json!({
                "clientUserMessageId": params.client_user_message_id,
                "disposition": execution.disposition,
                "appServerAccepted": execution.app_server_accepted,
                "nativeRequestId": execution.native_request_id,
                "turnId": execution.turn_id,
                "queue": execution.queue,
            }))
        }
        Err(error) => {
            let event_error = json!({
                "source": "cutex",
                "code": error.code,
                "message": error.message,
                "retryable": error.retryable,
                "details": error.details,
            });
            append_cutex_event(
                event_repository,
                cutex_session_id,
                super::model::EventCorrelation {
                    thread_id: Some(thread_id),
                    client_user_message_id: Some(params.client_user_message_id.clone()),
                    management_request_id: Some(request.request_id.clone()),
                    ..Default::default()
                },
                "cutex/userInput/failed",
                json!({
                    "managementRequestId": request.request_id,
                    "clientUserMessageId": params.client_user_message_id,
                    "origin": params.origin,
                    "input": params.input,
                    "stage": error.stage,
                    "error": event_error,
                    "failedAt": chrono::Utc::now().to_rfc3339(),
                }),
            )
            .map_err(post_operation_persistence_error)?;
            Err(error)
        }
    }
}

fn dispatch_queue_update(
    event_repository: &EventRepository,
    cutex_session_id: &str,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["queueId", "expectedRevision", "input"])?;
    let queue_id = required_string_param(object, "queueId")?;
    let expected_revision = required_safe_integer_param(object, "expectedRevision")?;
    let input = object
        .get("input")
        .and_then(Value::as_array)
        .filter(|input| !input.is_empty())
        .cloned()
        .ok_or_else(|| invalid_user_input_error("input must be a non-empty array"))?;
    for item in &input {
        validate_user_input(item).map_err(|message| invalid_user_input_error(&message))?;
    }
    let item = user_input_repository()
        .and_then(|repository| {
            repository.update(cutex_session_id, queue_id, expected_revision, input)
        })
        .map_err(queue_repository_error)?
        .ok_or_else(|| queue_not_found_error(queue_id))?;
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            client_user_message_id: Some(item.client_user_message_id.clone()),
            management_request_id: Some(request.request_id.clone()),
            queue_id: Some(item.queue_id.clone()),
            ..Default::default()
        },
        "cutex/userInput/queueUpdated",
        serde_json::to_value(&item).expect("queue item serialization"),
    )
    .map_err(persistence_user_input_error)?;
    Ok(serde_json::to_value(item).expect("queue item serialization"))
}

fn dispatch_queue_remove(
    event_repository: &EventRepository,
    cutex_session_id: &str,
    request: &ValidatedCutexRequest,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = exact_params(&request.params, &["queueId", "expectedRevision"])?;
    let queue_id = required_string_param(object, "queueId")?;
    let expected_revision = required_safe_integer_param(object, "expectedRevision")?;
    let removed = user_input_repository()
        .and_then(|repository| repository.remove(cutex_session_id, queue_id, expected_revision))
        .map_err(queue_repository_error)?
        .ok_or_else(|| queue_not_found_error(queue_id))?;
    let (queue_revision, _) = user_input_repository()
        .and_then(|repository| repository.list(cutex_session_id))
        .map_err(persistence_user_input_error)?;
    append_cutex_event(
        event_repository,
        cutex_session_id,
        super::model::EventCorrelation {
            client_user_message_id: Some(removed.client_user_message_id.clone()),
            management_request_id: Some(request.request_id.clone()),
            queue_id: Some(removed.queue_id.clone()),
            ..Default::default()
        },
        "cutex/userInput/queueRemoved",
        json!({
            "queueId": removed.queue_id,
            "clientUserMessageId": removed.client_user_message_id,
            "revision": removed.revision,
            "reason": "cancelled",
            "removedAt": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .map_err(persistence_user_input_error)?;
    Ok(json!({
        "revision": queue_revision,
        "affectedQueueIds": [queue_id],
    }))
}

fn dispatch_queue_flush(
    cutex_session_id: &str,
    request: &ValidatedCutexRequest,
    flush_user_input_queue: ManagementUserInputQueueFlusher,
) -> Result<Value, super::user_input::UserInputExecutionError> {
    let object = request
        .params
        .as_object()
        .ok_or_else(|| invalid_user_input_error("params must be an object"))?;
    if !object.contains_key("expectedQueueRevision")
        || object
            .keys()
            .any(|key| !matches!(key.as_str(), "expectedQueueRevision" | "maxItems"))
    {
        return Err(invalid_user_input_error(
            "queue flush requires expectedQueueRevision and optional maxItems",
        ));
    }
    let expected_revision = required_safe_integer_param(object, "expectedQueueRevision")?;
    let max_items = match object.get("maxItems") {
        None => 100,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=100).contains(value))
            .map(|value| value as usize)
            .ok_or_else(|| invalid_user_input_error("maxItems must be between 1 and 100"))?,
    };
    let (before_revision, before_items) = user_input_repository()
        .and_then(|repository| repository.list(cutex_session_id))
        .map_err(persistence_user_input_error)?;
    if before_revision != expected_revision {
        return Err(super::user_input::UserInputExecutionError {
            stage: "queue".to_string(),
            code: "revision_conflict".to_string(),
            message: format!(
                "queue revision conflict: expected {expected_revision}, current {before_revision}"
            ),
            retryable: true,
            details: json!({
                "expectedRevision": expected_revision,
                "currentRevision": before_revision,
                "resyncRequired": true,
            }),
            outcome_unknown: false,
        });
    }
    flush_user_input_queue(cutex_session_id, max_items).map_err(|error| {
        super::user_input::UserInputExecutionError {
            stage: "native_request".to_string(),
            code: "app_server_unavailable".to_string(),
            message: format!("{error:#}"),
            retryable: true,
            details: json!({}),
            outcome_unknown: false,
        }
    })?;
    let (revision, after_items) = user_input_repository()
        .and_then(|repository| repository.list(cutex_session_id))
        .map_err(persistence_user_input_error)?;
    let remaining = after_items
        .iter()
        .map(|item| item.queue_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let affected_queue_ids = before_items
        .into_iter()
        .filter_map(|item| (!remaining.contains(item.queue_id.as_str())).then_some(item.queue_id))
        .collect::<Vec<_>>();
    Ok(json!({
        "revision": revision,
        "affectedQueueIds": affected_queue_ids,
    }))
}

pub(super) fn append_cutex_event(
    repository: &EventRepository,
    cutex_session_id: &str,
    correlation: super::model::EventCorrelation,
    method: &str,
    params: Value,
) -> anyhow::Result<()> {
    repository.append(super::model::PendingEvent {
        cutex_session_id: cutex_session_id.to_string(),
        host_id: crate::platform::host::current_host_name(),
        source: super::model::EventSource::Cutex,
        schema: None,
        correlation,
        native: None,
        cutex: Some(super::model::CutexMessage {
            method: method.to_string(),
            params,
        }),
    })?;
    Ok(())
}

fn require_empty_params(params: &Value) -> Result<(), super::user_input::UserInputExecutionError> {
    if params.as_object().is_some_and(|params| params.is_empty()) {
        Ok(())
    } else {
        Err(invalid_user_input_error("params must be an empty object"))
    }
}

pub(super) fn exact_params<'a>(
    params: &'a Value,
    keys: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, super::user_input::UserInputExecutionError> {
    let object = params
        .as_object()
        .ok_or_else(|| invalid_user_input_error("params must be an object"))?;
    if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
        return Err(invalid_user_input_error(&format!(
            "params must contain exactly: {}",
            keys.join(", ")
        )));
    }
    Ok(object)
}

fn required_string_param<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str, super::user_input::UserInputExecutionError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_user_input_error(&format!("{key} must be a non-empty string")))
}

pub(super) fn required_safe_integer_param(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<u64, super::user_input::UserInputExecutionError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| *value <= super::model::MAX_SAFE_SEQUENCE)
        .ok_or_else(|| invalid_user_input_error(&format!("{key} must be a JSON-safe integer")))
}

pub(super) fn invalid_user_input_error(
    message: &str,
) -> super::user_input::UserInputExecutionError {
    super::user_input::UserInputExecutionError {
        stage: "route".to_string(),
        code: "invalid_request".to_string(),
        message: message.to_string(),
        retryable: false,
        details: json!({}),
        outcome_unknown: false,
    }
}

fn persistence_user_input_error(
    error: anyhow::Error,
) -> super::user_input::UserInputExecutionError {
    super::user_input::UserInputExecutionError {
        stage: "route".to_string(),
        code: "event_persistence_unavailable".to_string(),
        message: format!("{error:#}"),
        retryable: true,
        details: json!({}),
        outcome_unknown: false,
    }
}

pub(super) fn post_operation_persistence_error(
    error: anyhow::Error,
) -> super::user_input::UserInputExecutionError {
    super::user_input::UserInputExecutionError {
        stage: "native_request".to_string(),
        code: "event_persistence_unavailable".to_string(),
        message: format!("{error:#}"),
        retryable: false,
        details: json!({ "resyncRequired": true }),
        outcome_unknown: true,
    }
}

fn queue_repository_error(error: anyhow::Error) -> super::user_input::UserInputExecutionError {
    let message = format!("{error:#}");
    if message.contains("queue revision conflict") {
        super::user_input::UserInputExecutionError {
            stage: "queue".to_string(),
            code: "revision_conflict".to_string(),
            message,
            retryable: true,
            details: json!({ "resyncRequired": true }),
            outcome_unknown: false,
        }
    } else {
        persistence_user_input_error(error)
    }
}

fn focus_mutation_error(error: anyhow::Error) -> super::user_input::UserInputExecutionError {
    let message = format!("{error:#}");
    if message.contains("focus revision conflict") {
        super::user_input::UserInputExecutionError {
            stage: "route".to_string(),
            code: "revision_conflict".to_string(),
            message,
            retryable: true,
            details: json!({ "resyncRequired": true }),
            outcome_unknown: false,
        }
    } else {
        persistence_user_input_error(error)
    }
}

fn queue_not_found_error(queue_id: &str) -> super::user_input::UserInputExecutionError {
    super::user_input::UserInputExecutionError {
        stage: "queue".to_string(),
        code: "queue_item_not_found".to_string(),
        message: "the queue item does not exist".to_string(),
        retryable: false,
        details: json!({ "queueId": queue_id }),
        outcome_unknown: false,
    }
}

fn cutex_operation_error_status(code: &str, outcome_unknown: bool) -> u16 {
    if outcome_unknown {
        return 409;
    }
    match code {
        "invalid_request" => 400,
        "cutex_method_denied" => 403,
        "session_not_found" => 404,
        "queue_item_not_found" => 404,
        "idempotency_conflict"
        | "revision_conflict"
        | "already_retired"
        | "already_active"
        | "turn_conflict"
        | "session_offline" => 409,
        "app_server_unavailable" | "native_request_failed" | "interrupt_timeout" => 502,
        "runtime_stop_failed" => 502,
        "event_persistence_unavailable" | "persistence_uncertain" => 503,
        _ => 500,
    }
}

fn http_reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

fn native_request_session_id_from_path(path: &str) -> Option<String> {
    let encoded = path
        .strip_prefix("/v2/sessions/")?
        .strip_suffix("/app-server/requests")?;
    decode_session_id(encoded)
}

fn server_response_session_id_from_path(path: &str) -> Option<String> {
    let encoded = path
        .strip_prefix("/v2/sessions/")?
        .strip_suffix("/app-server/server-request-responses")?;
    decode_session_id(encoded)
}

fn cutex_request_session_id_from_path(path: &str) -> Option<String> {
    let encoded = path
        .strip_prefix("/v2/sessions/")?
        .strip_suffix("/cutex/requests")?;
    decode_session_id(encoded)
}

fn bootstrap_session_id_from_path(path: &str) -> Option<String> {
    let encoded = path
        .strip_prefix("/v2/sessions/")?
        .strip_suffix("/bootstrap")?;
    decode_session_id(encoded)
}

fn session_id_from_path(path: &str) -> Option<String> {
    let encoded = path.strip_prefix("/v2/sessions/")?;
    decode_session_id(encoded)
}

fn decode_session_id(encoded: &str) -> Option<String> {
    if encoded.is_empty() || encoded.contains('/') {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_path_digit(bytes[index + 1])?;
            let low = hex_path_digit(bytes[index + 2])?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    (!decoded.is_empty() && !decoded.contains('/') && !decoded.contains('\\')).then_some(decoded)
}

fn hex_path_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn request_id_hint(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("requestId")?
        .as_str()
        .filter(|request_id| !request_id.is_empty())
        .map(str::to_string)
}

fn response_id_hint(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .get("responseId")?
        .as_str()
        .filter(|response_id| !response_id.is_empty())
        .map(str::to_string)
}

fn replay_query(
    request: &SimpleHttpRequest,
    allow_last_event_id: bool,
) -> Result<ReplayQuery, ReplayError> {
    let stream_id = query_parameter(&request.path, "streamId")?;
    let query_after = query_parameter(&request.path, "after")?;
    let header_after = allow_last_event_id
        .then(|| request.headers.get("last-event-id").cloned())
        .flatten()
        .filter(|value| !value.is_empty());
    if query_after.is_some() && header_after.is_some() && query_after != header_after {
        return Err(ReplayError::ConflictingCursor);
    }
    let after = query_after.or(header_after);
    let limit = query_parameter(&request.path, "limit")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| ReplayError::InvalidQuery("limit must be an integer".to_string()))
        })
        .transpose()?
        .unwrap_or(100);
    Ok(ReplayQuery {
        stream_id,
        after,
        limit,
        cutex_session_id: query_parameter(&request.path, "cutexSessionId")?
            .filter(|value| !value.is_empty()),
    })
}

fn query_parameter(path: &str, key: &str) -> Result<Option<String>, ReplayError> {
    let Some((_, query)) = path.split_once('?') else {
        return Ok(None);
    };
    let values = url::form_urlencoded::parse(query.as_bytes())
        .filter_map(|(name, value)| (name == key).then(|| value.into_owned()))
        .collect::<Vec<_>>();
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(value.clone())),
        _ => Err(ReplayError::InvalidQuery(format!(
            "query parameter {key} must not be repeated"
        ))),
    }
}

fn write_replay_error(stream: &mut TcpStream, error: ReplayError) -> anyhow::Result<()> {
    match error {
        ReplayError::InvalidQuery(message) => write_v2_error(
            stream,
            400,
            "Bad Request",
            "invalid_request",
            &message,
            false,
            json!({}),
        ),
        ReplayError::ConflictingCursor => write_v2_error(
            stream,
            400,
            "Bad Request",
            "conflicting_cursor",
            "after and Last-Event-ID identify different cursors",
            false,
            json!({}),
        ),
        ReplayError::StreamChanged {
            requested_stream_id,
            current_stream_id,
        } => write_v2_error(
            stream,
            409,
            "Conflict",
            "stream_changed",
            "the host event stream identity changed",
            false,
            json!({
                "hostId": crate::platform::host::current_host_name(),
                "requestedStreamId": requested_stream_id,
                "currentStreamId": current_stream_id,
                "resyncRequired": true,
            }),
        ),
        ReplayError::CursorExpired {
            stream_id,
            cursor: _,
            earliest,
            latest,
        } => write_v2_error(
            stream,
            409,
            "Conflict",
            "cursor_expired",
            "the requested cursor is no longer retained",
            false,
            json!({
                "hostId": crate::platform::host::current_host_name(),
                "streamId": stream_id,
                "earliestCursor": earliest.as_ref().map(|boundary| boundary.cursor.clone()),
                "earliestSequence": earliest.as_ref().map(|boundary| boundary.sequence),
                "latestCursor": latest.as_ref().map(|boundary| boundary.cursor.clone()),
                "latestSequence": latest.as_ref().map(|boundary| boundary.sequence),
                "resyncRequired": true,
            }),
        ),
        ReplayError::Repository(error) => write_v2_error(
            stream,
            500,
            "Internal Server Error",
            "repository_error",
            "the durable management event repository failed",
            true,
            json!({ "diagnostic": format!("{error:#}") }),
        ),
    }
}

pub(super) fn write_v2_error(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    code: &str,
    message: &str,
    retryable: bool,
    details: serde_json::Value,
) -> anyhow::Result<()> {
    write_v2_request_error(
        stream, status, reason, None, code, message, retryable, details,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_v2_request_error(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    request_id: Option<&str>,
    code: &str,
    message: &str,
    retryable: bool,
    details: serde_json::Value,
) -> anyhow::Result<()> {
    let mut response = json!({
        "contractVersion": 2,
        "error": {
            "source": "cutex",
            "code": code,
            "message": message,
            "retryable": retryable,
            "details": details,
        }
    });
    if let Some(request_id) = request_id {
        response
            .as_object_mut()
            .expect("v2 error response is an object")
            .insert(
                "requestId".to_string(),
                Value::String(request_id.to_string()),
            );
    }
    write_json_response(stream, status, reason, &response)
}

fn write_request_outcome_unknown(
    stream: &mut TcpStream,
    request_id: &str,
    details: Value,
) -> anyhow::Result<()> {
    write_v2_request_error(
        stream,
        409,
        "Conflict",
        Some(request_id),
        "request_outcome_unknown",
        "cutex lost the native response after forwarding the request; it will not be replayed",
        false,
        details,
    )
}

#[allow(clippy::too_many_arguments)]
fn write_v2_server_response_error(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    response_id: Option<&str>,
    code: &str,
    message: &str,
    retryable: bool,
    details: Value,
) -> anyhow::Result<()> {
    let mut response = json!({
        "contractVersion": 2,
        "error": {
            "source": "cutex",
            "code": code,
            "message": message,
            "retryable": retryable,
            "details": details,
        }
    });
    if let Some(response_id) = response_id {
        response
            .as_object_mut()
            .expect("v2 error response is an object")
            .insert(
                "responseId".to_string(),
                Value::String(response_id.to_string()),
            );
    }
    write_json_response(stream, status, reason, &response)
}

pub(crate) fn write_payload_too_large(
    stream: &mut TcpStream,
    _content_length: usize,
    limit: usize,
) -> anyhow::Result<()> {
    write_v2_error(
        stream,
        413,
        "Payload Too Large",
        "payload_too_large",
        "the management request body exceeds the published limit",
        false,
        json!({
            "maxRequestBytes": limit,
        }),
    )
}

fn write_sse_headers(stream: &mut impl Write) -> anyhow::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n",
    )?;
    stream.flush()?;
    Ok(())
}

fn write_sse_event(stream: &mut impl Write, event: &EventEnvelope) -> anyhow::Result<()> {
    let data = serde_json::to_string(event)?;
    write!(
        stream,
        "id: {}\nevent: {SSE_EVENT_NAME}\ndata: {data}\n\n",
        event.cursor
    )?;
    stream.flush()?;
    Ok(())
}

fn write_sse_stream_changed(
    stream: &mut impl Write,
    host_id: &str,
    requested_stream_id: &str,
    current_stream_id: &str,
) -> anyhow::Result<()> {
    let data = serde_json::to_string(&json!({
        "contractVersion": 2,
        "error": {
            "source": "cutex",
            "code": "stream_changed",
            "message": "the host event stream identity changed",
            "retryable": false,
            "details": {
                "hostId": host_id,
                "requestedStreamId": requested_stream_id,
                "currentStreamId": current_stream_id,
                "resyncRequired": true,
            }
        }
    }))?;
    write!(
        stream,
        "event: cutex_management_stream_error\ndata: {data}\n\n"
    )?;
    stream.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::io::Write;
    use std::net::Shutdown;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;
    use fs2::FileExt;

    static NATIVE_FORWARD_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn seat_management_credential_is_not_the_agent_bus_bearer() {
        let raw_agent_bus_bearer = "ordinary-agent-bus-bearer";
        let scoped = task_service_seat_management_token(raw_agent_bus_bearer);
        assert_ne!(scoped, raw_agent_bus_bearer);
        assert_eq!(scoped.len(), 64);
        assert_eq!(
            scoped,
            task_service_seat_management_token(raw_agent_bus_bearer)
        );
        assert_eq!(
            v2_required_token(
                "/v2/task-service/seats/bind",
                Some(raw_agent_bus_bearer),
                None,
                None,
            ),
            None
        );
        assert_eq!(
            v2_required_token(
                "/v2/task-service/seats/bind",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                None,
            ),
            Some(scoped.as_str())
        );
        assert_eq!(
            v2_required_token(
                "/v2/release-rotation/template",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                None,
            ),
            Some(scoped.as_str())
        );
        assert_eq!(
            v2_required_token(
                "/v2/release-rotation/retry",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                None,
            ),
            Some(scoped.as_str())
        );
        assert_eq!(
            v2_required_token(
                "/v2/agent-management/actions",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                Some("agent-management-root"),
            ),
            Some(raw_agent_bus_bearer)
        );
        assert_eq!(
            v2_required_token(
                "/v2/agent-management/authority",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                Some("agent-management-root"),
            ),
            Some("agent-management-root")
        );
        assert_eq!(
            v2_required_token(
                "/v2/agent-management/legacy-director-ownership-import",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                Some("agent-management-root"),
            ),
            Some("agent-management-root")
        );
        assert_ne!(
            v2_required_token(
                "/v2/agent-management/legacy-director-ownership-import",
                Some(raw_agent_bus_bearer),
                Some(&scoped),
                Some("agent-management-root"),
            ),
            Some(raw_agent_bus_bearer),
            "ordinary Agent Bus authentication must not authorize ownership import"
        );
    }

    #[test]
    fn legacy_director_import_is_root_scoped_and_not_an_ambient_agent_route() {
        let path = "/v2/agent-management/legacy-director-ownership-import";
        assert!(agent_management_admin_path(path));
        assert!(!agent_management_admin_path("/v2/agent-management/actions"));
        assert_eq!(
            v2_required_token(
                path,
                Some("ordinary-management"),
                Some("seat-admin"),
                Some("agent-management-root"),
            ),
            Some("agent-management-root")
        );
        assert_eq!(
            v2_required_token(path, Some("ordinary-management"), Some("seat-admin"), None,),
            None
        );
    }

    #[test]
    fn direct_forged_agent_management_invocation_is_rejected() {
        let request = SimpleHttpRequest {
            method: "POST".to_string(),
            path: "/v2/agent-management/actions".to_string(),
            headers: HashMap::new(),
            body: serde_json::to_vec(&json!({
                "invocation": {
                    "caller_cutex_session": "cutex.director-forged",
                    "caller_runtime_agent_id": "runtime-forged"
                },
                "request": {
                    "schema": "cutex/agent-management/v1",
                    "action_id": "forged-management-action",
                    "project_id": "cutex-project",
                    "operation": { "kind": "query_managed" }
                }
            }))
            .unwrap(),
        };
        let response =
            capture_http_response(|stream| handle_agent_management_action(stream, &request));
        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        let body = response_json(&response);
        assert_eq!(body["error"]["code"], "ambient_agent_bus_required");
    }

    fn forward_test_native_request(
        cutex_session_id: &str,
        message: Value,
    ) -> Result<Value, ManagementNativeForwardError> {
        NATIVE_FORWARD_CALLS.fetch_add(1, Ordering::SeqCst);
        assert_eq!(cutex_session_id, "cutex-session-1");
        assert_eq!(message["id"], "native-request-1");
        assert_eq!(message["futureRequestField"], json!({ "kept": true }));
        Ok(json!({
            "id": message["id"].clone(),
            "result": { "thread": { "id": "thread-1" } },
            "futureResponseField": null
        }))
    }

    fn execute_test_user_input(
        command: UserInputSubmitCommand,
    ) -> Result<
        super::super::user_input::UserInputSubmitExecution,
        super::super::user_input::UserInputExecutionError,
    > {
        assert_eq!(command.management_request_id, "management-input-1");
        assert_eq!(command.params.origin.client_id, "device-1");
        Ok(super::super::user_input::UserInputSubmitExecution {
            disposition: UserInputDisposition::Started,
            app_server_accepted: true,
            native_request_id: Some(json!("native-input-1")),
            native_method: Some("turn/start".to_string()),
            turn_id: Some("turn-1".to_string()),
            queue: None,
        })
    }

    fn execute_test_runtime_mutation(
        cutex_session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, super::super::user_input::UserInputExecutionError> {
        assert_eq!(cutex_session_id, "cutex-session-1");
        assert_eq!(method, "cutex/runtime/offline");
        assert_eq!(params["expectedRuntimeGeneration"], 7);
        assert_eq!(params["reason"], "owner_requested");
        assert_eq!(params["force"], true);
        Ok(json!({
            "runtimeGeneration": 7,
            "runtimeAgentId": null,
            "status": "offline"
        }))
    }

    fn execute_test_runtime_online_requiring_foreground(
        cutex_session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, super::super::user_input::UserInputExecutionError> {
        assert_eq!(cutex_session_id, "cutex-session-1");
        assert_eq!(method, "cutex/runtime/online");
        assert_eq!(params["expectedRuntimeGeneration"], 7);
        assert_eq!(params["openVisibleTerminal"], true);
        Ok(json!({
            "runtimeGeneration": 8,
            "runtimeAgentId": "runtime-2",
            "status": "online",
            "foregroundRequiredReason": "desktop_launcher_unavailable"
        }))
    }

    fn execute_test_runtime_online_with_profile(
        cutex_session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, super::super::user_input::UserInputExecutionError> {
        assert_eq!(cutex_session_id, "cutex-session-1");
        assert_eq!(method, "cutex/runtime/online");
        assert_eq!(params["expectedRuntimeGeneration"], 7);
        assert_eq!(params["launchProfile"], "beta");
        Ok(json!({
            "runtimeGeneration": 8,
            "runtimeAgentId": "runtime-2",
            "status": "online",
            "launchProfile": {
                "requested": "beta",
                "selected": "beta-canonical",
                "effective": "beta-canonical",
                "source": "one_launch_override",
                "applicationScope": "runtime_and_tui",
                "persisted": false
            }
        }))
    }

    fn execute_test_profile_mutation(
        cutex_session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, super::super::user_input::UserInputExecutionError> {
        assert_eq!(cutex_session_id, "cutex-session-1");
        assert_eq!(params["expectedRevision"], 7);
        match method {
            "cutex/session/profile/set" => {
                assert_eq!(params["profile"], "alpha");
                Ok(json!({
                    "cutexSessionId": cutex_session_id,
                    "revision": 8,
                    "configuredProfile": "alpha",
                }))
            }
            "cutex/session/profile/clear" => Ok(json!({
                "cutexSessionId": cutex_session_id,
                "revision": 8,
                "configuredProfile": null,
            })),
            other => panic!("unexpected profile mutation {other}"),
        }
    }

    fn flush_test_user_input_queue(
        _cutex_session_id: &str,
        _max_items: usize,
    ) -> anyhow::Result<usize> {
        Ok(0)
    }

    fn request(path: &str) -> SimpleHttpRequest {
        SimpleHttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn owner_task_read_uses_real_http_auth_path_and_emits_only_the_safe_model() {
        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let credential = crate::task_service::OwnerTaskReadCredential {
            principal_id: "owner-reader".to_string(),
            audience: "host-a-backend".to_string(),
            token: crate::task_service::OwnerTaskReadToken::new("owner-reader-token-0123456789"),
            project_ids: vec![project.clone()],
            expires_at: Some("2099-01-01T00:00:00Z".to_string()),
        };
        let root = std::env::temp_dir().join(format!("cutex-owner-http-{}", uuid::Uuid::new_v4()));
        let provider = crate::task_service::TaskServiceProvider::open(&root).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream
                .write_all(
                    b"GET /v2/projects/project-alpha/tasks?limit=1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer owner-reader-token-0123456789\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let (mut stream, _) = listener.accept().unwrap();
        let request = crate::http::server::read_simple_http_request(&mut stream).unwrap();
        let request_project =
            owner_task_project_from_path(request.path.split('?').next().unwrap()).unwrap();
        let principal = crate::task_service::OwnerTaskReadCredential::authenticate(
            &[credential],
            request.headers.get("authorization").map(String::as_str),
            &request_project,
            chrono::Utc::now(),
        )
        .unwrap();
        handle_owner_task_read_with_provider(
            &mut stream,
            &request,
            &principal,
            &request_project,
            &provider,
        )
        .unwrap();
        drop(stream);
        let response = client.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        let body = response_json(&response);
        assert_eq!(body["schema"], "cutex/owner-task-read/v1");
        assert_eq!(body["project_id"], "project-alpha");
        assert_eq!(body["audience"], "host-a-backend");
        assert_eq!(body["items"], serde_json::json!([]));
        assert!(!response.contains("owner-reader-token"));
        assert!(!response.contains("opaque_contract"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closed_owner_query_hits_bounded_deadline_and_later_task_action_commits() {
        let root =
            std::env::temp_dir().join(format!("cutex-owner-query-cancel-{}", uuid::Uuid::new_v4()));
        let provider = crate::task_service::TaskServiceProvider::open(&root).unwrap();
        let external_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("task-service-provider-v2.lock"))
            .unwrap();
        external_lock.lock_exclusive().unwrap();
        let project = crate::agent_management::ProjectId::new("project-alpha").unwrap();
        let credential = crate::task_service::OwnerTaskReadCredential {
            principal_id: "owner-reader".to_string(),
            audience: "host-a-backend".to_string(),
            token: crate::task_service::OwnerTaskReadToken::new("owner-reader-token-0123456789"),
            project_ids: vec![project.clone()],
            expires_at: Some("2099-01-01T00:00:00Z".to_string()),
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let query_provider = provider.clone();
        let query_project = project.clone();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = crate::http::server::read_simple_http_request(&mut stream).unwrap();
            let principal = crate::task_service::OwnerTaskReadCredential::authenticate(
                &[credential],
                request.headers.get("authorization").map(String::as_str),
                &query_project,
                chrono::Utc::now(),
            )
            .unwrap();
            let _ = handle_owner_task_read_with_provider(
                &mut stream,
                &request,
                &principal,
                &query_project,
                &query_provider,
            );
            finished_tx.send(()).unwrap();
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .write_all(
                b"GET /v2/projects/project-alpha/tasks HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer owner-reader-token-0123456789\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        client.shutdown(Shutdown::Both).unwrap();
        drop(client);
        finished_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("indistinguishable closed requester must hit the bounded provider deadline");
        FileExt::unlock(&external_lock).unwrap();

        let coordinator = crate::task_service::AuthenticatedPrincipal::seated_session(
            crate::role_revision::CutexSessionId::new("director-session").unwrap(),
            crate::task_service::SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let contract = "post-cancellation project contract";
        let receipt = provider
            .create_project_revision(
                &coordinator,
                &crate::task_service::CreateProjectRevisionRequest {
                    schema: crate::task_service::ProviderActionSchema::V3,
                    action_id: crate::task_service::ActionId::new("post-cancellation-create")
                        .unwrap(),
                    project_id: project,
                    workflow_id: crate::task_service::WorkflowId::new("post-cancellation-workflow")
                        .unwrap(),
                    task_id: crate::role_revision::TaskId::new("post-cancellation-task").unwrap(),
                    task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
                    contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
                    opaque_contract: contract.to_string(),
                    completion_policy: crate::task_service::CompletionPolicy {
                        kind: crate::task_service::CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: crate::task_service::SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        assert_eq!(receipt.journal_sequence, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn last_event_id_and_query_cursor_must_not_conflict() {
        let mut request = request("/v2/events/stream?after=c2:query&streamId=stream-1");
        request
            .headers
            .insert("last-event-id".to_string(), "c2:header".to_string());
        assert!(matches!(
            replay_query(&request, true),
            Err(ReplayError::ConflictingCursor)
        ));
    }

    #[test]
    fn sse_stream_change_frame_matches_frozen_error_contract_and_has_no_cursor() {
        let mut bytes = Vec::new();
        write_sse_stream_changed(&mut bytes, "host-a", "old-stream", "new-stream")
            .expect("write stream change frame");
        let frame = String::from_utf8(bytes).expect("utf8 SSE frame");
        assert!(frame.starts_with("event: cutex_management_stream_error\n"));
        assert!(!frame.contains("\nid: "));
        let data = frame
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE data line");
        let body: Value = serde_json::from_str(data).expect("parse SSE error body");
        assert_eq!(body["error"]["code"], "stream_changed");
        assert_eq!(body["error"]["details"]["hostId"], "host-a");
        assert_eq!(body["error"]["details"]["requestedStreamId"], "old-stream");
        assert_eq!(body["error"]["details"]["currentStreamId"], "new-stream");
        assert_eq!(body["error"]["details"]["resyncRequired"], true);
    }

    #[test]
    fn page_query_keeps_opaque_cursor_and_session_scope() {
        let query = replay_query(
            &request(
                "/v2/events?streamId=stream-1&after=c2%3Aopaque&limit=17&cutexSessionId=cutex.session-1",
            ),
            false,
        )
        .expect("parse replay query");
        assert_eq!(query.stream_id.as_deref(), Some("stream-1"));
        assert_eq!(query.after.as_deref(), Some("c2:opaque"));
        assert_eq!(query.limit, 17);
        assert_eq!(query.cutex_session_id.as_deref(), Some("cutex.session-1"));
    }

    #[test]
    fn session_path_decoding_is_strict_and_does_not_apply_form_semantics() {
        assert_eq!(
            session_id_from_path("/v2/sessions/cutex%2Esession%2B1").as_deref(),
            Some("cutex.session+1")
        );
        assert_eq!(
            session_id_from_path("/v2/sessions/cutex+session").as_deref(),
            Some("cutex+session")
        );
        assert!(session_id_from_path("/v2/sessions/cutex%2Fsession").is_none());
        assert!(session_id_from_path("/v2/sessions/cutex%ZZsession").is_none());
    }

    #[test]
    fn only_visibility_show_can_resolve_a_hidden_session() {
        assert!(request_may_resolve_hidden_session(
            "cutex/session/visibility/show"
        ));
        assert!(!request_may_resolve_hidden_session(
            "cutex/session/visibility/hide"
        ));
        assert!(!request_may_resolve_hidden_session("cutex/runtime/online"));
    }

    #[test]
    fn unsupported_cutex_method_precheck_returns_forbidden() {
        let body = serde_json::to_vec(&json!({
            "requestId": "unsupported-method-1",
            "method": "cutex/session/notRegistered",
            "params": {}
        }))
        .expect("serialize request");
        let error = validate_cutex_request_body(&body).expect_err("method must be denied");
        assert_eq!(
            error,
            CutexRequestValidationError::UnsupportedMethod(
                "cutex/session/notRegistered".to_string()
            )
        );

        let response = capture_http_response(|stream| {
            write_cutex_request_validation_error(stream, Some("unsupported-method-1"), error)
        });

        assert!(response.starts_with("HTTP/1.1 403 Forbidden"));
        let body = response_json(&response);
        assert_eq!(body["requestId"], "unsupported-method-1");
        assert_eq!(body["error"]["code"], "cutex_method_denied");
        assert_eq!(
            body["error"]["details"]["method"],
            "cutex/session/notRegistered"
        );
        assert_eq!(
            body["error"]["details"]["registryVersion"],
            super::super::session::CUTEX_METHOD_REGISTRY_VERSION
        );
    }

    #[test]
    fn post_mutation_persistence_uncertainty_is_not_replayed() {
        let root =
            std::env::temp_dir().join(format!("cutex-v2-outcome-unknown-{}", uuid::Uuid::new_v4()));
        let idempotency =
            RequestIdempotencyRepository::open(&root, "outcome-test-process".to_string())
                .expect("open idempotency repository");
        let canonical_body = json!({
            "requestId": "retire-uncertain-1",
            "method": "cutex/session/retire",
            "params": {
                "expectedRevision": 4,
                "expectedRuntimeGeneration": 7
            }
        });
        let claim = match idempotency
            .begin("cutex-session-1", "retire-uncertain-1", &canonical_body)
            .expect("begin request")
        {
            BeginRequest::Forward(claim) => claim,
            other => panic!("expected fresh claim, got {other:?}"),
        };
        let response = capture_http_response(|stream| {
            write_cutex_operation_error(
                stream,
                &idempotency,
                &claim,
                "retire-uncertain-1",
                super::super::user_input::UserInputExecutionError {
                    stage: "persistence".to_string(),
                    code: "persistence_uncertain".to_string(),
                    message: "archive mutation may have persisted".to_string(),
                    retryable: true,
                    details: json!({ "writeStage": "store_replace" }),
                    outcome_unknown: true,
                },
            )
        });

        assert!(response.starts_with("HTTP/1.1 409 Conflict"));
        let body = response_json(&response);
        assert_eq!(body["requestId"], "retire-uncertain-1");
        assert_eq!(body["error"]["code"], "request_outcome_unknown");
        assert_eq!(body["error"]["retryable"], false);
        assert_eq!(body["error"]["details"]["resyncRequired"], true);
        assert_eq!(
            body["error"]["details"]["originalCode"],
            "persistence_uncertain"
        );
        assert_eq!(
            body["error"]["details"]["originalDetails"]["writeStage"],
            "store_replace"
        );
        assert!(matches!(
            idempotency
                .begin("cutex-session-1", "retire-uncertain-1", &canonical_body)
                .expect("repeat request"),
            BeginRequest::OutcomeUnknown
        ));

        std::fs::remove_dir_all(root).expect("remove outcome-unknown test root");
    }

    #[test]
    fn native_route_forwards_exact_message_and_deduplicates_completed_response() {
        NATIVE_FORWARD_CALLS.store(0, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("cutex-v2-native-route-{}", uuid::Uuid::new_v4()));
        let idempotency =
            RequestIdempotencyRepository::open(&root, "route-test-process".to_string())
                .expect("open idempotency repository");
        let session = json!({
            "native": { "threadId": "thread-1" },
            "runtime": { "appServerConnected": true }
        });
        let body = serde_json::to_vec(&json!({
            "requestId": "management-request-1",
            "native": {
                "message": {
                    "id": "native-request-1",
                    "method": "thread/read",
                    "params": { "threadId": "thread-1", "includeTurns": true },
                    "futureRequestField": { "kept": true }
                }
            }
        }))
        .expect("serialize request");
        let request = SimpleHttpRequest {
            method: "POST".to_string(),
            path: "/v2/sessions/cutex-session-1/app-server/requests".to_string(),
            headers: HashMap::new(),
            body: body.clone(),
        };

        let first = execute_native_route(&request, &session, &idempotency);
        assert!(first.starts_with("HTTP/1.1 200 OK"));
        let first_body = response_json(&first);
        assert_eq!(first_body["requestId"], "management-request-1");
        assert_eq!(first_body["native"]["message"]["id"], "native-request-1");
        assert!(first_body["native"]["message"]
            .get("futureResponseField")
            .is_some());

        let second = execute_native_route(&request, &session, &idempotency);
        assert_eq!(response_json(&second), first_body);
        assert_eq!(NATIVE_FORWARD_CALLS.load(Ordering::SeqCst), 1);

        let conflicting = SimpleHttpRequest {
            body: serde_json::to_vec(&json!({
                "requestId": "management-request-1",
                "native": {
                    "message": {
                        "id": "native-request-1",
                        "method": "thread/read",
                        "params": { "threadId": "thread-1", "includeTurns": false }
                    }
                }
            }))
            .expect("serialize conflicting request"),
            ..request
        };
        let conflict = execute_native_route(&conflicting, &session, &idempotency);
        assert!(conflict.starts_with("HTTP/1.1 409 Conflict"));
        assert_eq!(
            response_json(&conflict)["error"]["code"],
            "idempotency_conflict"
        );
        assert_eq!(NATIVE_FORWARD_CALLS.load(Ordering::SeqCst), 1);

        std::fs::remove_dir_all(root).expect("remove route test root");
    }

    #[test]
    fn server_response_parser_preserves_exact_signed_id_and_unknown_fields() {
        let body = serde_json::to_vec(&json!({
            "responseId": "response-1",
            "native": {
                "message": {
                    "id": i64::MIN,
                    "error": {
                        "code": -32001,
                        "message": "denied",
                        "data": null
                    },
                    "futureField": { "kept": true }
                }
            }
        }))
        .expect("serialize response");
        let response = validate_server_response_body(&body).expect("valid response");
        assert_eq!(response.response_id, "response-1");
        assert_eq!(response.native_message["id"], i64::MIN);
        assert_eq!(
            response.native_message["futureField"],
            json!({ "kept": true })
        );
    }

    #[test]
    fn runtime_generation_precondition_is_strict_and_json_safe() {
        assert_eq!(
            parse_runtime_generation_precondition("\"cutex-runtime-generation:304\"")
                .expect("valid generation"),
            304
        );
        assert_eq!(
            parse_runtime_generation_precondition("\"cutex-runtime-generation:9007199254740991\"")
                .expect("maximum JSON-safe generation"),
            MAX_SAFE_SEQUENCE
        );
        for invalid in [
            "cutex-runtime-generation:304",
            "W/\"cutex-runtime-generation:304\"",
            "\"cutex-runtime-generation:0\"",
            "\"cutex-runtime-generation:9007199254740992\"",
            "\"cutex-runtime-generation:-1\"",
            "\"cutex-runtime-generation:304\", \"other\"",
            "\"runtime-generation:304\"",
        ] {
            assert!(
                parse_runtime_generation_precondition(invalid).is_err(),
                "accepted invalid precondition {invalid}"
            );
        }
    }

    #[test]
    fn user_input_dispatch_appends_route_and_native_acceptance_events() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-user-input-events-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id).expect("open event repository");
        let request = ValidatedCutexRequest {
            request_id: "management-input-1".to_string(),
            method: "cutex/userInput/submit".to_string(),
            params: json!({
                "clientUserMessageId": "client-input-1",
                "origin": { "kind": "android", "clientId": "device-1" },
                "strategy": "auto",
                "input": [{ "type": "text", "text": "hello", "text_elements": [] }]
            }),
        };
        let result = dispatch_user_input_submit(
            &repository,
            execute_test_user_input,
            "cutex-session-1",
            &json!({ "native": { "threadId": "thread-1" } }),
            &request,
        )
        .expect("dispatch input");
        assert_eq!(result["nativeRequestId"], "native-input-1");
        assert_eq!(result["disposition"], "started");

        let page = repository
            .page(ReplayQuery {
                cutex_session_id: Some("cutex-session-1".to_string()),
                ..Default::default()
            })
            .expect("read events");
        assert_eq!(page.events.len(), 2);
        assert_eq!(
            page.events[0].cutex.as_ref().expect("route event").method,
            "cutex/userInput/routeAccepted"
        );
        let submitted = page.events[1].cutex.as_ref().expect("submitted event");
        assert_eq!(submitted.method, "cutex/userInput/submitted");
        assert_eq!(submitted.params["input"], request.params["input"]);
        assert_eq!(submitted.params["origin"], request.params["origin"]);
        assert_eq!(
            page.events[1].correlation.native_request_id,
            Some(json!("native-input-1"))
        );
        std::fs::remove_dir_all(root).expect("remove event repository");
    }

    #[test]
    fn runtime_mutation_enforces_generation_and_emits_frozen_lifecycle_events() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-runtime-mutation-events-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id).expect("open event repository");
        let session = json!({
            "native": { "threadId": "thread-1" },
            "runtime": {
                "backend": "host",
                "status": "online",
                "runtimeGeneration": 7,
                "runtimeAgentId": "runtime-1"
            }
        });
        let request = ValidatedCutexRequest {
            request_id: "runtime-request-1".to_string(),
            method: "cutex/runtime/offline".to_string(),
            params: json!({
                "expectedRuntimeGeneration": 7,
                "reason": "owner_requested",
                "force": true
            }),
        };
        let result = dispatch_runtime_mutation(
            &repository,
            execute_test_runtime_mutation,
            "cutex-session-1",
            &session,
            &request,
        )
        .expect("dispatch runtime mutation");
        assert_eq!(
            result,
            json!({ "runtimeGeneration": 7, "status": "offline" })
        );
        let page = repository
            .page(ReplayQuery::default())
            .expect("read events");
        assert_eq!(page.events.len(), 2);
        assert_eq!(
            page.events[0].cutex.as_ref().unwrap().method,
            "cutex/runtime/offlineRequested"
        );
        assert_eq!(
            page.events[1].cutex.as_ref().unwrap().method,
            "cutex/runtime/offline"
        );
        assert_eq!(
            page.events[0].cutex.as_ref().unwrap().params["status"],
            "closing"
        );
        assert_eq!(
            page.events[1].cutex.as_ref().unwrap().params["status"],
            "offline"
        );
        assert!(page.events[1].cutex.as_ref().unwrap().params["runtimeAgentId"].is_null());
        assert_eq!(
            page.events[1].correlation.management_request_id.as_deref(),
            Some("runtime-request-1")
        );

        let conflict = dispatch_runtime_mutation(
            &repository,
            execute_test_runtime_mutation,
            "cutex-session-1",
            &session,
            &ValidatedCutexRequest {
                request_id: "runtime-request-2".to_string(),
                method: "cutex/runtime/offline".to_string(),
                params: json!({ "expectedRuntimeGeneration": 6 }),
            },
        )
        .expect_err("stale generation must conflict");
        assert_eq!(conflict.code, "revision_conflict");
        assert_eq!(
            repository
                .page(ReplayQuery::default())
                .unwrap()
                .events
                .len(),
            2
        );
        std::fs::remove_dir_all(root).expect("remove event repository");
    }

    #[test]
    fn runtime_online_emits_typed_foreground_required_event() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-runtime-foreground-required-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id).expect("open event repository");
        let session = json!({
            "native": { "threadId": "thread-1" },
            "runtime": {
                "backend": "host_foreground",
                "status": "offline",
                "runtimeGeneration": 7,
                "runtimeAgentId": null
            }
        });
        let request = ValidatedCutexRequest {
            request_id: "runtime-online-1".to_string(),
            method: "cutex/runtime/online".to_string(),
            params: json!({
                "expectedRuntimeGeneration": 7,
                "openVisibleTerminal": true
            }),
        };

        let result = dispatch_runtime_mutation(
            &repository,
            execute_test_runtime_online_requiring_foreground,
            "cutex-session-1",
            &session,
            &request,
        )
        .expect("dispatch runtime mutation");
        assert_eq!(
            result,
            json!({ "runtimeGeneration": 8, "status": "online" })
        );

        let page = repository
            .page(ReplayQuery::default())
            .expect("read runtime events");
        let methods = page
            .events
            .iter()
            .map(|event| event.cutex.as_ref().unwrap().method.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            vec![
                "cutex/runtime/onlineRequested",
                "cutex/runtime/online",
                "cutex/host/foregroundRequired"
            ]
        );
        let foreground = page.events[2].cutex.as_ref().unwrap();
        assert_eq!(foreground.params["runtimeGeneration"], 8);
        assert_eq!(foreground.params["reason"], "desktop_launcher_unavailable");
        assert!(foreground.params["requiredAt"].as_str().is_some());
        assert_eq!(
            page.events[2].correlation.management_request_id.as_deref(),
            Some("runtime-online-1")
        );
        std::fs::remove_dir_all(root).expect("remove event repository");
    }

    #[test]
    fn runtime_online_returns_validated_one_launch_profile_receipt() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-runtime-launch-profile-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id).expect("open event repository");
        let session = json!({
            "native": { "threadId": "thread-1" },
            "runtime": {
                "backend": "host",
                "status": "offline",
                "runtimeGeneration": 7,
                "runtimeAgentId": null
            }
        });
        let request = ValidatedCutexRequest {
            request_id: "runtime-online-profile-1".to_string(),
            method: "cutex/runtime/online".to_string(),
            params: json!({
                "expectedRuntimeGeneration": 7,
                "launchProfile": " beta "
            }),
        };

        let result = dispatch_runtime_mutation(
            &repository,
            execute_test_runtime_online_with_profile,
            "cutex-session-1",
            &session,
            &request,
        )
        .expect("dispatch runtime mutation");
        assert_eq!(
            result,
            json!({
                "runtimeGeneration": 8,
                "status": "online",
                "launchProfile": {
                    "requested": "beta",
                    "selected": "beta-canonical",
                    "effective": "beta-canonical",
                    "source": "one_launch_override",
                    "applicationScope": "runtime_and_tui",
                    "persisted": false
                }
            })
        );
        std::fs::remove_dir_all(root).expect("remove event repository");
    }

    #[test]
    fn management_dispatch_routes_profile_mutations_and_normal_launch_receipts() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-profile-dispatch-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id).expect("open event repository");
        let session = json!({
            "revision": 7,
            "native": { "threadId": "thread-1" },
            "runtime": {
                "backend": "host",
                "status": "offline",
                "runtimeGeneration": 7,
                "runtimeAgentId": null,
            },
        });
        let set = dispatch_cutex_request(
            &repository,
            execute_test_user_input,
            flush_test_user_input_queue,
            execute_test_profile_mutation,
            "cutex-session-1",
            &session,
            &ValidatedCutexRequest {
                request_id: "profile-set-1".to_string(),
                method: "cutex/session/profile/set".to_string(),
                params: json!({ "expectedRevision": 7, "profile": "alpha" }),
            },
        )
        .expect("dispatch profile set");
        assert_eq!(
            set,
            json!({
                "cutexSessionId": "cutex-session-1",
                "revision": 8,
                "configuredProfile": "alpha",
            })
        );
        let clear = dispatch_cutex_request(
            &repository,
            execute_test_user_input,
            flush_test_user_input_queue,
            execute_test_profile_mutation,
            "cutex-session-1",
            &session,
            &ValidatedCutexRequest {
                request_id: "profile-clear-1".to_string(),
                method: "cutex/session/profile/clear".to_string(),
                params: json!({ "expectedRevision": 7 }),
            },
        )
        .expect("dispatch profile clear");
        assert_eq!(clear["configuredProfile"], Value::Null);

        let online = dispatch_cutex_request(
            &repository,
            execute_test_user_input,
            flush_test_user_input_queue,
            execute_test_runtime_online_with_profile,
            "cutex-session-1",
            &session,
            &ValidatedCutexRequest {
                request_id: "runtime-online-profile-2".to_string(),
                method: "cutex/runtime/online".to_string(),
                params: json!({ "expectedRuntimeGeneration": 7, "launchProfile": "beta" }),
            },
        )
        .expect("dispatch runtime online");
        assert_eq!(online["launchProfile"]["source"], "one_launch_override");
        assert_eq!(online["launchProfile"]["persisted"], false);

        let event_page = repository
            .page(ReplayQuery::default())
            .expect("read events");
        let methods = event_page
            .events
            .iter()
            .filter_map(|event| event.cutex.as_ref().map(|cutex| cutex.method.as_str()))
            .collect::<Vec<_>>();
        assert!(methods.contains(&"cutex/session/profileUpdated"));
        assert!(methods.contains(&"cutex/runtime/online"));
        std::fs::remove_dir_all(root).expect("remove event repository");
    }

    #[test]
    fn legacy_tui_only_normal_receipt_accepts_unknown_provenance() {
        let receipt = json!({
            "requested": "legacy-profile",
            "selected": "legacy-profile",
            "effective": "legacy-profile",
            "source": "unknown",
            "applicationScope": "tui",
            "persisted": false,
        });
        assert_eq!(
            validate_launch_profile_receipt(None, Some(&receipt)).expect("legacy receipt"),
            Some(receipt)
        );
    }

    #[test]
    fn bootstrap_checkpoint_precedes_events_racing_with_thread_read() {
        let root =
            std::env::temp_dir().join(format!("cutex-v2-bootstrap-race-{}", uuid::Uuid::new_v4()));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id.clone()).expect("open repository");
        let session = json!({
            "contractVersion": 2,
            "cutexSessionId": "cutex-session-1",
            "hostId": host_id,
            "revision": 1,
            "native": {
                "threadId": "thread-1",
                "schema": {
                    "protocol": "codex-app-server",
                    "majorVersion": 2,
                    "version": "0.144.1",
                    "sha256": "schema",
                    "channel": "experimental",
                    "capabilities": {},
                    "extensions": []
                }
            },
            "runtime": {
                "status": "online",
                "runtimeGeneration": 7,
                "appServerConnected": true
            },
            "focus": {
                "revision": 1,
                "owner": "none",
                "mobileMuted": false,
                "source": null,
                "updatedAt": "2026-07-13T00:00:00Z"
            },
            "management": {}
        });
        let bootstrap = build_bootstrap_resource(
            &session,
            &repository,
            |request| {
                assert_eq!(request["method"], "thread/read");
                repository
                    .append(super::super::model::PendingEvent {
                        cutex_session_id: "cutex-session-1".to_string(),
                        host_id: crate::platform::host::current_host_name(),
                        source: super::super::model::EventSource::Cutex,
                        schema: None,
                        correlation: Default::default(),
                        native: None,
                        cutex: Some(super::super::model::CutexMessage {
                            method: "cutex/test/raced".to_string(),
                            params: json!({}),
                        }),
                    })
                    .expect("append racing event");
                Ok(json!({
                    "id": request["id"].clone(),
                    "result": { "thread": { "id": "thread-1", "turns": [] } }
                }))
            },
            |_| {
                Ok(json!({
                    "userInputQueue": [],
                    "pendingServerRequests": [],
                    "agentBusMessages": []
                }))
            },
        )
        .expect("build bootstrap");
        assert_eq!(bootstrap["checkpoint"]["sequence"], 0);
        assert!(bootstrap["checkpoint"]["cursor"].is_null());
        let page = repository
            .page(ReplayQuery::default())
            .expect("replay raced event");
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].sequence, 1);
        assert_eq!(
            page.events[0].cutex.as_ref().expect("cutex event").method,
            "cutex/test/raced"
        );
        std::fs::remove_dir_all(root).expect("remove bootstrap repository");
    }

    #[test]
    fn offline_bootstrap_keeps_checkpoint_and_cutex_state_without_native_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-bootstrap-offline-{}",
            uuid::Uuid::new_v4()
        ));
        let host_id = crate::platform::host::current_host_name();
        let repository = EventRepository::open(&root, host_id.clone()).expect("open repository");
        let session = json!({
            "contractVersion": 2,
            "cutexSessionId": "cutex-session-offline",
            "hostId": host_id,
            "revision": 3,
            "native": { "threadId": "thread-offline", "schema": null },
            "runtime": {
                "status": "offline",
                "runtimeGeneration": 0,
                "appServerConnected": false
            },
            "focus": {
                "revision": 1,
                "owner": "none",
                "mobileMuted": false,
                "source": null,
                "updatedAt": "2026-07-13T00:00:00Z"
            },
            "management": {}
        });
        let bootstrap = build_bootstrap_resource(
            &session,
            &repository,
            |_| -> Result<Value, String> {
                panic!("offline bootstrap must not call the app-server")
            },
            |_| {
                Ok(json!({
                    "userInputQueue": [{ "queueId": "queue-offline-1" }],
                    "pendingServerRequests": [],
                    "agentBusMessages": []
                }))
            },
        )
        .expect("build offline bootstrap");

        assert!(bootstrap["schema"].is_null());
        assert!(bootstrap["native"].is_null());
        assert_eq!(bootstrap["checkpoint"]["sequence"], 0);
        assert_eq!(bootstrap["cutexState"]["runtime"]["status"], "offline");
        assert_eq!(
            bootstrap["cutexState"]["userInputQueue"][0]["queueId"],
            "queue-offline-1"
        );
        std::fs::remove_dir_all(root).expect("remove bootstrap repository");
    }

    fn execute_native_route(
        request: &SimpleHttpRequest,
        session: &Value,
        idempotency: &RequestIdempotencyRepository,
    ) -> String {
        capture_http_response(|stream| {
            handle_native_request_for_session(
                stream,
                request,
                forward_test_native_request,
                "cutex-session-1",
                session,
                idempotency,
            )
        })
    }

    fn capture_http_response(
        write_response: impl FnOnce(&mut TcpStream) -> anyhow::Result<()>,
    ) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind response listener");
        let address = listener.local_addr().expect("response listener address");
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).expect("connect response client");
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .expect("read route response");
            response
        });
        let (mut stream, _) = listener.accept().expect("accept response client");
        write_response(&mut stream).expect("write response");
        client.join().expect("response client")
    }

    fn response_json(response: &str) -> Value {
        serde_json::from_str(
            response
                .split_once("\r\n\r\n")
                .expect("HTTP response body")
                .1,
        )
        .expect("parse response JSON")
    }
}
