use std::fs;
use std::fs::File;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use fs2::FileExt;
use serde_json::Value;

use cutex::agent_bus::audit::{append_agent_bus_audit_record, content_preview};
use cutex::agent_bus::delivery::AgentDeliveryMode;
use cutex::agent_bus::message::format_agent_message_content;
use cutex::agent_bus::model::{
    canonical_recipient_label, AgentBusAgent, AgentBusEnvelopeKind, AgentBusMessage,
    AgentBusSendRequest,
};
use cutex::agent_bus::routing::{
    agent_bus_agent_session_id_by_id, agent_bus_agent_snapshot_by_id,
    is_full_durable_cutex_session_id, normalize_agent_bus_session_id,
    resolve_agent_message_sender_name, resolve_agent_target_for_sender_with_sessions,
};
use cutex::agent_bus::server::{
    handle_agent_bus_request, notify_agent_bus_message_available, AgentBusRequestHandlers,
    TaskWorkerActionHost,
};
use cutex::agent_bus::service::{agent_bus_health_url, agent_bus_port, validate_agent_bus_port};
use cutex::agent_bus::store::{load_agent_bus_state_from_registry, AgentBusState};
use cutex::agent_management::{
    AgentManagementMessageMetadata, AGENT_MANAGEMENT_START_CONTROL_TYPE,
    AGENT_MANAGEMENT_SYSTEM_SENDER,
};
use cutex::config::paths::runtime_dir;
use cutex::config::store::load_codez_config;
use cutex::http::server::write_http_response;
use cutex::management::v2::agent_bus_state::agent_bus_message_repository;
use cutex::management::v2::agent_bus_state::AgentBusMessageRepository;
use cutex::management::v2::agent_bus_state::AgentBusQueuedMessage;
use cutex::management::v2::integration_events::append_agent_bus_message_sent;
use cutex::platform::now_epoch_secs;
use cutex::profiles::model::CodezConfig;
use cutex::role_revision::CutexSessionId;
use cutex::session::identity::default_cutex_session_id_for_codex_session;
use cutex::session::model::CutexSessionStore;
use cutex::session::service::cutex_session_key_for_user_id;
use cutex::session::store::load_cutex_session_store;

use super::agent_bus_forwarding;
use super::agent_bus_runtime;
use super::rotation;
use super::session_reconcile;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";
const TASK_WORKER_ACTION_ROOT: &str = "task-worker-actions-v1";
const TASK_WORKER_ACTION_HOST_LOCK: &str = "agent-bus-host.lock";
const TASK_WORKER_TASK_SERVICE_ROOT: &str = "task-service";
const TASK_WORKER_EVIDENCE_ROOT: &str = "evidence";
const TASK_SEAT_AUTHORITY_ROOT: &str = "seat-authority-v1";

struct OwnedTaskWorkerActionHost {
    host: Arc<TaskWorkerActionHost>,
    _ownership_lock: File,
    root: PathBuf,
}

pub(crate) fn request_handlers() -> AgentBusRequestHandlers {
    AgentBusRequestHandlers {
        reconcile_registration_agent: session_reconcile::reconcile_cutex_session_registration,
        reconcile_agent: session_reconcile::reconcile_cutex_session_from_agent,
        redrive_ordinary_messages,
        send_payload_response,
        release_rotation: rotation::handle_release_rotation,
        agent_management: super::agent_management::handle_agent_management,
    }
}

fn redrive_ordinary_messages(state: &Arc<Mutex<AgentBusState>>) -> anyhow::Result<usize> {
    let repository = agent_bus_message_repository()?;
    repository.migrate_legacy_v1()?;
    let sessions = load_cutex_session_store()?;
    redrive_ordinary_messages_with(repository, state, &sessions)
}

fn redrive_ordinary_messages_with(
    repository: &AgentBusMessageRepository,
    state: &Arc<Mutex<AgentBusState>>,
    sessions: &CutexSessionStore,
) -> anyhow::Result<usize> {
    let pending = repository.pending_v2()?;
    if pending.is_empty() {
        return Ok(0);
    }
    let mut redriven = 0usize;
    for record in pending {
        let target_id = match resolve_agent_target_for_sender_with_sessions(
            state,
            &record.target_cutex_session_id,
            None,
            true,
            Some(sessions),
        ) {
            Ok(target) => target,
            Err(_) => continue,
        };
        let target_name = cutex::agent_bus::groups::resolve_agent_display_name(state, &target_id)
            .unwrap_or_else(|| target_id.clone());
        let mut envelope = record.canonical_envelope;
        envelope.to = target_id.clone();
        let outcome = cutex::agent_bus::queue::enqueue_agent_bus_message_once_with_id(
            state,
            &envelope.from,
            &target_id,
            &target_name,
            &envelope.content,
            envelope.kind.clone(),
            envelope.delivery_mode.clone(),
            envelope.sender_kind.clone(),
            envelope.display_source.clone(),
            envelope.submit_mode.clone(),
            envelope.control_type.clone(),
            envelope.control_payload.clone(),
            envelope.external_action_id.clone(),
            envelope.external_message_id.clone(),
            envelope.from_cutex_session_id.clone(),
            envelope.to_cutex_session_id.clone(),
            Some(envelope.id.clone()),
            envelope.created_at_epoch_secs,
        )?;
        if !outcome.deduplicated {
            redriven = redriven.saturating_add(1);
        }
    }
    if redriven > 0 {
        notify_agent_bus_message_available();
    }
    Ok(redriven)
}

pub(crate) fn cmd_agent_serve(
    port: Option<u16>,
    token: Option<String>,
    handlers: AgentBusRequestHandlers,
) -> anyhow::Result<()> {
    let mut config = load_codez_config();
    if let Some(port) = port {
        config.agent_bus_port = Some(port);
    }
    if let Some(token) = token {
        config.agent_bus_token = Some(token);
    }
    run_agent_bus(config, handlers)
}

pub(crate) fn send_payload_response(
    state: &Arc<Mutex<AgentBusState>>,
    payload: AgentBusSendRequest,
    allow_federation: bool,
) -> anyhow::Result<Value> {
    send_payload_response_with_projection(state, payload, allow_federation, None)
}

pub(crate) struct AgentManagementSystemMessage<'a> {
    pub metadata: &'a AgentManagementMessageMetadata,
    pub from_agent_id: Option<String>,
    pub from_session_id: Option<String>,
    pub target_runtime_agent_id: &'a str,
    pub target_cutex_session_id: &'a cutex::role_revision::CutexSessionId,
    pub exact_message: &'a str,
    pub external_message_id: &'a str,
}

pub(crate) fn send_agent_management_system_message_response(
    state: &Arc<Mutex<AgentBusState>>,
    system: &cutex::agent_bus::identity::AgentManagementSystemPrincipal,
    message: AgentManagementSystemMessage<'_>,
) -> anyhow::Result<Value> {
    if !system.authenticate() {
        anyhow::bail!("Agent Management system principal authentication failed");
    }
    let payload = AgentBusSendRequest {
        to: message.target_runtime_agent_id.to_string(),
        all_groups: true,
        all_hosts: false,
        kind: AgentBusEnvelopeKind::Message,
        from: Some(message.metadata.requested_by_director.as_str().to_string()),
        from_agent_id: message.from_agent_id,
        from_session_id: message.from_session_id,
        to_session_id: Some(message.target_cutex_session_id.as_str().to_string()),
        content: message.exact_message.to_string(),
        delivery_mode: Some(AgentDeliveryMode::AfterTurn),
        queue_only: None,
        trigger_turn: None,
        sender_kind: Some(cutex::agent_bus::model::AgentMessageKind::Agent),
        display_source: None,
        submit_mode: None,
        control_type: None,
        control_payload: None,
        external_action_id: None,
        external_message_id: Some(message.external_message_id.to_string()),
    };
    send_payload_response_with_projection(state, payload, false, Some(message.metadata))
}

fn send_payload_response_with_projection(
    state: &Arc<Mutex<AgentBusState>>,
    payload: AgentBusSendRequest,
    allow_federation: bool,
    agent_management_projection: Option<&AgentManagementMessageMetadata>,
) -> anyhow::Result<Value> {
    let envelope_kind = payload.kind.clone();
    let delivery_mode = payload.resolved_delivery_mode();
    let sender_kind = payload.sender_kind.clone().unwrap_or_default();
    if sender_kind.is_task_service_system() {
        anyhow::bail!(
            "Task Service system messages require the in-process authenticated provider path"
        );
    }
    if agent_management_projection.is_none()
        && payload.control_type.as_deref() == Some(AGENT_MANAGEMENT_START_CONTROL_TYPE)
    {
        anyhow::bail!(
            "Agent Management system messages require the in-process authorized service path"
        );
    }
    if envelope_kind == AgentBusEnvelopeKind::Message
        && sender_kind.is_agent()
        && delivery_mode == AgentDeliveryMode::Interrupt
    {
        anyhow::bail!(
            "agent-bus interrupt delivery is outside management v2; use after_turn, soon, or passive"
        );
    }
    let display_source = agent_management_projection
        .map(|_| "Agent Management System".to_string())
        .or_else(|| {
            payload
                .display_source
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        });
    let submit_mode =
        (!sender_kind.is_agent()).then(|| payload.submit_mode.clone().unwrap_or_default());
    let sender = resolve_agent_message_sender_name(state, &payload);
    if agent_management_projection.is_none() && sender == AGENT_MANAGEMENT_SYSTEM_SENDER {
        anyhow::bail!(
            "Agent Management system sender requires the in-process authorized service path"
        );
    }
    let identity_bound = payload.from_session_id.is_some() || payload.to_session_id.is_some();
    let durable_sessions = is_full_durable_cutex_session_id(&payload.to)
        .then(load_cutex_session_store)
        .transpose()?;
    let target_id = match resolve_agent_target_for_sender_with_sessions(
        state,
        &payload.to,
        payload.from_agent_id.as_deref(),
        payload.all_groups,
        durable_sessions.as_ref(),
    ) {
        Ok(target_id) => target_id,
        Err(local_err) if allow_federation && payload.all_hosts && !identity_bound => {
            if let Some(response) =
                agent_bus_forwarding::try_forward_agent_bus_message(state, &payload, &sender)?
            {
                return Ok(response);
            }
            record_failed_agent_bus_attempt(state, &payload, &sender, &local_err)?;
            return Err(local_err);
        }
        Err(err) => {
            record_failed_agent_bus_attempt(state, &payload, &sender, &err)?;
            return Err(err);
        }
    };
    if allow_federation
        && payload.all_hosts
        && !identity_bound
        && is_full_durable_cutex_session_id(&payload.to)
    {
        agent_bus_forwarding::ensure_no_peer_target_collision(state, &payload, &target_id)?;
    }
    let target_agent = agent_bus_agent_snapshot_by_id(state, &target_id);
    let target_name = target_agent
        .as_ref()
        .map(|agent| agent.name.clone())
        .unwrap_or_else(|| target_id.clone());
    let registered_sender_session_id = payload
        .from_agent_id
        .as_deref()
        .and_then(|id| agent_bus_agent_session_id_by_id(state, id));
    let registered_target_session_id = agent_bus_agent_session_id_by_id(state, &target_id);
    let sender_session_id = registered_sender_session_id
        .clone()
        .or_else(|| normalize_agent_bus_session_id(payload.from_session_id.as_deref()));
    let target_session_id = registered_target_session_id
        .clone()
        .or_else(|| normalize_agent_bus_session_id(payload.to_session_id.as_deref()));
    let (from_cutex_session_id, to_cutex_session_id) = resolve_bound_message_sessions(
        &payload,
        &target_id,
        registered_sender_session_id.as_deref(),
        registered_target_session_id.as_deref(),
    )?;
    let config = load_codez_config();
    let content = if envelope_kind == AgentBusEnvelopeKind::Control {
        payload.content.clone()
    } else if sender_kind.is_agent() {
        format_agent_message_content(
            config.agent_message_prefix_template.as_deref(),
            config.agent_message_suffix_template.as_deref(),
            &sender,
            &target_name,
            &payload.content,
        )
    } else {
        payload.content.clone()
    };
    // The reserved control record carries the service provenance without
    // extending the public Agent Bus sender-kind wire enum.
    let queued_sender = agent_management_projection
        .map(|_| AGENT_MANAGEMENT_SYSTEM_SENDER)
        .unwrap_or(sender.as_str());
    let control_type = agent_management_projection
        .map(|_| AGENT_MANAGEMENT_START_CONTROL_TYPE.to_string())
        .or_else(|| payload.control_type.clone());
    let control_payload = agent_management_projection
        .map(serde_json::to_value)
        .transpose()?
        .or_else(|| payload.control_payload.clone());
    let queued_at_epoch = now_epoch_secs();
    let durable_ordinary = envelope_kind == AgentBusEnvelopeKind::Message && sender_kind.is_agent();
    let stable_reservation = if durable_ordinary {
        Some(cutex::agent_bus::queue::reserve_agent_bus_message_id(
            state,
            queued_sender,
            &target_id,
            &content,
            &envelope_kind,
            &delivery_mode,
            &sender_kind,
            display_source.as_deref(),
            submit_mode.as_ref(),
            control_type.as_deref(),
            control_payload.as_ref(),
            payload.external_action_id.as_deref(),
            payload.external_message_id.as_deref(),
            queued_at_epoch,
        )?)
    } else {
        None
    };
    let stable_message_id = stable_reservation
        .as_ref()
        .map(|reservation| reservation.message_id.clone());
    let queued_at_epoch = stable_reservation
        .as_ref()
        .map_or(queued_at_epoch, |reservation| {
            reservation.created_at_epoch_secs
        });
    if let Some(message_id) = stable_message_id.as_ref().filter(|_| {
        !stable_reservation
            .as_ref()
            .is_some_and(|reservation| reservation.already_enqueued)
    }) {
        let from_cutex_session_id = from_cutex_session_id
            .clone()
            .context("agent-bus sender has no durable cutex session identity")?;
        let to_cutex_session_id = to_cutex_session_id
            .clone()
            .context("agent-bus target has no durable cutex session identity")?;
        let canonical_envelope = AgentBusMessage {
            id: message_id.clone(),
            kind: envelope_kind.clone(),
            from: queued_sender.to_string(),
            to: target_id.clone(),
            from_cutex_session_id: Some(from_cutex_session_id.clone()),
            to_cutex_session_id: Some(to_cutex_session_id.clone()),
            content: content.clone(),
            delivery_mode: delivery_mode.clone(),
            trigger_turn: delivery_mode.trigger_turn(),
            created_at_epoch_secs: queued_at_epoch,
            sender_kind: sender_kind.clone(),
            display_source: display_source.clone(),
            submit_mode: submit_mode.clone(),
            control_type: control_type.clone(),
            control_payload: control_payload.clone(),
            external_action_id: payload.external_action_id.clone(),
            external_message_id: payload.external_message_id.clone(),
        };
        let semantic_sha256 = ordinary_message_semantic_sha256(
            target_agent
                .as_ref()
                .context("agent-bus target disappeared before durable digest commit")?,
            &to_cutex_session_id,
            &canonical_envelope,
        )?;
        let queued_at =
            chrono::DateTime::from_timestamp(queued_at_epoch as i64, 0).unwrap_or_else(Utc::now);
        agent_bus_message_repository()?.record_queued(AgentBusQueuedMessage {
            owner_cutex_session_id: to_cutex_session_id.clone(),
            message_id: message_id.clone(),
            from_cutex_session_id,
            to_cutex_session_id,
            from_runtime_agent_id: payload.from_agent_id.clone(),
            to_runtime_agent_id: Some(target_id.clone()),
            delivery_mode: delivery_mode.event_label().to_string(),
            content: payload.content.clone(),
            queued_at,
            canonical_envelope,
            semantic_sha256,
        })?;
    }
    let outcome = cutex::agent_bus::queue::enqueue_agent_bus_message_once_with_id(
        state,
        queued_sender,
        &target_id,
        &target_name,
        &content,
        envelope_kind,
        delivery_mode,
        sender_kind,
        display_source,
        submit_mode,
        control_type,
        control_payload,
        payload.external_action_id.clone(),
        payload.external_message_id.clone(),
        from_cutex_session_id.clone(),
        to_cutex_session_id.clone(),
        stable_message_id,
        queued_at_epoch,
    )?;
    let notify_waiters = !outcome.deduplicated;
    let deduplicated = outcome.deduplicated;
    let send_record = outcome.record;
    if let Err(err) = append_agent_bus_audit_record(serde_json::json!({
        "event": "sent",
        "timestamp": Utc::now().to_rfc3339(),
        "message_id": send_record.id.clone(),
        "kind": send_record.kind.clone(),
        "from": send_record.from.clone(),
        "to": send_record.to.clone(),
        "to_name": send_record.to_name.clone(),
        "delivery_mode": send_record.delivery_mode.clone(),
        "trigger_turn": send_record.trigger_turn,
        "queued": send_record.queued,
        "deduplicated": deduplicated,
        "sender_kind": send_record.sender_kind.clone(),
        "display_source": send_record.display_source.clone(),
        "submit_mode": send_record.submit_mode.clone(),
        "control_type": send_record.control_type.clone(),
        "external_action_id": send_record.external_action_id.clone(),
        "external_message_id": send_record.external_message_id.clone(),
        "content_chars": content.chars().count(),
        "content_preview": content_preview(&content, 500),
    })) {
        eprintln!("{YELLOW}warning:{RESET} failed to write agent audit log: {err:#}");
    }
    if send_record.kind == AgentBusEnvelopeKind::Message && send_record.sender_kind.is_agent() {
        let from_cutex_session_id = from_cutex_session_id
            .clone()
            .context("agent-bus sender has no durable cutex session identity")?;
        let to_cutex_session_id = to_cutex_session_id
            .clone()
            .context("agent-bus target has no durable cutex session identity")?;
        let queued_at =
            chrono::DateTime::from_timestamp(send_record.created_at_epoch_secs as i64, 0)
                .unwrap_or_else(Utc::now);
        if agent_management_projection.is_none() && !deduplicated {
            let sender_session =
                CutexSessionId::new(from_cutex_session_id.clone()).map_err(|_| {
                    anyhow::anyhow!(
                        "agent-bus sender has an invalid durable Cutex session identity"
                    )
                })?;
            if let Err(error) = append_agent_bus_message_sent(
                &sender_session,
                &send_record.id,
                serde_json::json!({
                    "messageId": send_record.id.clone(),
                    "fromCutexSessionId": from_cutex_session_id,
                    "toCutexSessionId": to_cutex_session_id,
                    "fromRuntimeAgentId": payload.from_agent_id.clone(),
                    "toRuntimeAgentId": target_id.clone(),
                    "deliveryMode": send_record.delivery_mode.event_label(),
                    "content": payload.content.clone(),
                    "sentAt": queued_at.to_rfc3339(),
                }),
            ) {
                eprintln!(
                    "{YELLOW}warning:{RESET} failed to project sender Agent Bus event: {error:#}"
                );
            }
        }
    }
    if notify_waiters {
        notify_agent_bus_message_available();
    }
    Ok(serde_json::json!({
        "ok": true,
        "id": send_record.id,
        "kind": send_record.kind,
        "from": send_record.from,
        "to": send_record.to,
        "to_name": send_record.to_name,
        "from_session_id": sender_session_id,
        "to_session_id": target_session_id,
        "from_runtime_agent_id": payload.from_agent_id,
        "to_runtime_agent_id": target_id,
        "from_cutex_session_id": from_cutex_session_id,
        "to_cutex_session_id": to_cutex_session_id,
        "delivery_mode": send_record.delivery_mode,
        "trigger_turn": send_record.trigger_turn,
        "queued": send_record.queued,
        "queueDurability": durable_ordinary.then_some("durable_v2"),
        "deliveryState": durable_ordinary.then_some("pending"),
        "requiredAckLevel": durable_ordinary.then_some("A4"),
        "deduplicated": deduplicated,
        "sender_kind": send_record.sender_kind,
        "display_source": send_record.display_source,
        "submit_mode": send_record.submit_mode,
        "control_type": send_record.control_type,
        "external_action_id": send_record.external_action_id,
        "external_message_id": send_record.external_message_id,
    }))
}

fn ordinary_message_semantic_sha256(
    target: &AgentBusAgent,
    to_cutex_session_id: &str,
    canonical_envelope: &AgentBusMessage,
) -> anyhow::Result<String> {
    let recipient_label =
        canonical_recipient_label(target.base_name.as_deref(), &target.name, &target.id);
    let params = cutex::app_server::bus_bridge::inter_agent_params(
        "",
        recipient_label,
        to_cutex_session_id,
        canonical_envelope,
    )?;
    Ok(cutex::app_server::bus_bridge::inter_agent_semantic_sha256(
        &params,
    ))
}

fn durable_cutex_session_id(session_id: Option<&str>) -> Option<String> {
    let session_id = session_id?.trim();
    if session_id.is_empty() {
        return None;
    }
    if session_id.starts_with("cutex.") {
        return Some(session_id.to_string());
    }
    let store = load_cutex_session_store().ok()?;
    cutex_session_key_for_user_id(&store, session_id)
        .and_then(|key| {
            store
                .sessions
                .get(&key)
                .map(|record| record.cutex_session_id.clone())
        })
        .or_else(|| Some(default_cutex_session_id_for_codex_session(session_id)))
}

fn resolve_bound_message_sessions(
    payload: &AgentBusSendRequest,
    target_runtime_agent_id: &str,
    registered_sender_session_id: Option<&str>,
    registered_target_session_id: Option<&str>,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    if payload.to_session_id.is_some() && target_runtime_agent_id != payload.to {
        anyhow::bail!("identity-bound Agent Bus target did not resolve by exact runtime id")
    }
    let from_cutex_session_id = durable_cutex_session_id(registered_sender_session_id);
    let to_cutex_session_id = durable_cutex_session_id(registered_target_session_id);
    verify_claimed_session(
        "sender",
        payload.from_session_id.as_deref(),
        registered_sender_session_id,
        from_cutex_session_id.as_deref(),
    )?;
    verify_claimed_session(
        "target",
        payload.to_session_id.as_deref(),
        registered_target_session_id,
        to_cutex_session_id.as_deref(),
    )?;
    Ok((from_cutex_session_id, to_cutex_session_id))
}

fn verify_claimed_session(
    label: &str,
    claimed_cutex_session_id: Option<&str>,
    registered_session_id: Option<&str>,
    registered_cutex_session_id: Option<&str>,
) -> anyhow::Result<()> {
    let Some(claimed_cutex_session_id) = claimed_cutex_session_id else {
        return Ok(());
    };
    if registered_session_id.is_none() {
        anyhow::bail!("identity-bound Agent Bus {label} has no registered session")
    }
    let claimed_cutex_session_id = durable_cutex_session_id(Some(claimed_cutex_session_id))
        .context("identity-bound Agent Bus claim has no durable Cutex identity")?;
    if registered_cutex_session_id != Some(claimed_cutex_session_id.as_str()) {
        anyhow::bail!("identity-bound Agent Bus {label} session is stale or colliding")
    }
    Ok(())
}

fn record_failed_agent_bus_attempt(
    state: &Arc<Mutex<AgentBusState>>,
    payload: &AgentBusSendRequest,
    sender: &str,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let _ = (state, payload, sender, error);
    // A target-resolution failure has not entered the durable-v2 queue lane.
    // The caller receives the typed failure and may retry; no incomplete
    // envelope is projected as a durable queued message.
    Ok(())
}

fn run_agent_bus(config: CodezConfig, handlers: AgentBusRequestHandlers) -> anyhow::Result<()> {
    run_agent_bus_with_task_action_root(
        config,
        handlers,
        &runtime_dir()?
            .join("task-service")
            .join(TASK_WORKER_ACTION_ROOT),
    )
}

fn run_agent_bus_with_task_action_root(
    config: CodezConfig,
    handlers: AgentBusRequestHandlers,
    task_action_root: &Path,
) -> anyhow::Result<()> {
    let port = agent_bus_port(&config);
    validate_agent_bus_port(port)?;
    let task_action_host = open_task_worker_action_host(task_action_root)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("Failed to bind cutex agent bus on 127.0.0.1:{port}"))?;
    println!(
        "cutex agent bus listening on {}",
        agent_bus_health_url(port)
    );
    println!(
        "recovered task worker action root {}",
        task_action_host.root.display()
    );
    agent_bus_runtime::register_agent_bus_handoff(port);

    let token = config.agent_bus_token.clone();
    let initial_state = match load_agent_bus_state_from_registry() {
        Ok(state) => state,
        Err(err) => {
            eprintln!("{YELLOW}warning:{RESET} failed to load agent bus registry: {err:#}");
            AgentBusState::default()
        }
    };
    let restored = initial_state.agents.len();
    if restored > 0 {
        println!("restored {restored} cutex agent registration(s) from registry");
    }
    let state = Arc::new(Mutex::new(initial_state));
    if let Err(error) = redrive_ordinary_messages(&state) {
        eprintln!(
            "{YELLOW}warning:{RESET} durable ordinary-message startup redrive failed: {error:#}"
        );
    }
    // Recover the durable completion outbox once at startup. If its target is
    // offline, registration/heartbeat will trigger the next bounded attempt.
    task_action_host
        .host
        .recover_completion_notifications(&state);
    task_action_host
        .host
        .spawn_task_watchdog(&state)
        .context("Failed to start Task Service stale-running watchdog")?;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let state = Arc::clone(&state);
                let token = token.clone();
                let task_actions = Arc::clone(&task_action_host.host);
                std::thread::spawn(move || {
                    if let Err(err) = handle_agent_bus_request(
                        &mut stream,
                        &state,
                        token.as_deref(),
                        handlers,
                        &task_actions,
                    ) {
                        let _ = write_http_response(
                            &mut stream,
                            500,
                            "Internal Server Error",
                            "text/plain",
                            format!("{err:#}").as_bytes(),
                        );
                    }
                });
            }
            Err(err) => eprintln!("{YELLOW}warning:{RESET} agent bus accept failed: {err}"),
        }
    }
    Ok(())
}

fn open_task_worker_action_host(root: &Path) -> anyhow::Result<OwnedTaskWorkerActionHost> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            anyhow::bail!(
                "task worker action root is not a direct directory: {}",
                root.display()
            );
        }
    }
    fs::create_dir_all(root).with_context(|| {
        format!(
            "Failed to create task worker action root: {}",
            root.display()
        )
    })?;
    secure_task_worker_action_directory(root)?;
    let lock_path = root.join(TASK_WORKER_ACTION_HOST_LOCK);
    let ownership_lock = open_task_worker_action_host_lock(&lock_path)?;
    ownership_lock.try_lock_exclusive().with_context(|| {
        format!(
            "Another process already owns the task worker action root: {}",
            root.display()
        )
    })?;
    let task_service_root = root.join(TASK_WORKER_TASK_SERVICE_ROOT);
    let evidence_root = root.join(TASK_WORKER_EVIDENCE_ROOT);
    let seat_authority_root = root
        .parent()
        .context("Task Service host root has no private parent")?
        .join(TASK_SEAT_AUTHORITY_ROOT);
    #[cfg(unix)]
    let private_children = vec![&task_service_root, &evidence_root];
    #[cfg(windows)]
    let private_children = vec![&task_service_root, &evidence_root, &seat_authority_root];
    for child in private_children {
        if let Ok(metadata) = fs::symlink_metadata(child) {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                anyhow::bail!(
                    "task worker private root is not a direct directory: {}",
                    child.display()
                );
            }
        }
        fs::create_dir(child).or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
        secure_task_worker_action_directory(child)?;
        #[cfg(windows)]
        cutex::platform::private_fs::secure_tree(child).with_context(|| {
            format!(
                "Failed to migrate task worker private state: {}",
                child.display()
            )
        })?;
    }
    let host = Arc::new(
        TaskWorkerActionHost::open_recovered(task_service_root, evidence_root, seat_authority_root)
            .with_context(|| {
                format!(
                    "Failed to recover task worker action stores before serve: {}",
                    root.display()
                )
            })?,
    );
    Ok(OwnedTaskWorkerActionHost {
        host,
        _ownership_lock: ownership_lock,
        root: root.to_path_buf(),
    })
}

fn open_task_worker_action_host_lock(path: &Path) -> anyhow::Result<File> {
    #[cfg(windows)]
    {
        return cutex::platform::private_fs::open_private_file_path(path, true, false)
            .with_context(|| {
                format!(
                    "Failed to open task worker action host lock: {}",
                    path.display()
                )
            });
    }
    #[cfg(unix)]
    {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        use std::os::unix::fs::OpenOptionsExt;

        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(path).with_context(|| {
            format!(
                "Failed to open task worker action host lock: {}",
                path.display()
            )
        })?;
        validate_task_worker_action_host_lock(&file)?;
        Ok(file)
    }
}

#[cfg(unix)]
fn validate_task_worker_action_host_lock(file: &File) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o7777 != 0o600
    {
        anyhow::bail!("task worker action host lock is not a private owner file");
    }
    Ok(())
}

#[cfg(unix)]
fn secure_task_worker_action_directory(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
        format!(
            "Failed to secure task worker action root: {}",
            path.display()
        )
    })
}

#[cfg(windows)]
fn secure_task_worker_action_directory(path: &Path) -> anyhow::Result<()> {
    cutex::platform::private_fs::secure_directory(path)
        .map(|_| ())
        .map_err(anyhow::Error::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::agent_bus::model::{AgentBusAgent, AgentMessageKind, AgentRegistrationClass};
    use cutex::agent_bus::routing::resolve_agent_target_for_sender;

    fn exact_route_agent(id: &str, session_id: &str) -> AgentBusAgent {
        AgentBusAgent {
            id: id.to_string(),
            name: id.to_string(),
            base_name: None,
            thread_name: None,
            path_key: None,
            session_id: Some(session_id.to_string()),
            cutex_session_id: None,
            profile: "test".to_string(),
            cwd: "/tmp".to_string(),
            pid: 1,
            host_id: None,
            groups: vec!["cutex".to_string()],
            registration_class: AgentRegistrationClass::LocalOnly,
            last_seen_epoch_secs: 1,
        }
    }

    fn exact_route_payload() -> AgentBusSendRequest {
        AgentBusSendRequest {
            to: "runtime-successor".to_string(),
            all_groups: true,
            all_hosts: false,
            kind: AgentBusEnvelopeKind::Message,
            from: None,
            from_agent_id: Some("runtime-director".to_string()),
            from_session_id: Some("cutex.director".to_string()),
            to_session_id: Some("cutex.successor".to_string()),
            content: "Start release review".to_string(),
            delivery_mode: Some(AgentDeliveryMode::AfterTurn),
            queue_only: None,
            trigger_turn: None,
            sender_kind: Some(AgentMessageKind::Agent),
            display_source: None,
            submit_mode: None,
            control_type: None,
            control_payload: None,
            external_action_id: Some("rotate-release-195".to_string()),
            external_message_id: Some("release-rotation:rotate-release-195:start".to_string()),
        }
    }

    #[test]
    fn agent_interrupt_is_rejected_before_target_resolution_or_queueing() {
        let payload: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "missing-target",
            "content": "do not enqueue",
            "senderKind": "agent",
            "deliveryMode": "interrupt"
        }))
        .expect("parse send request");
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let error = send_payload_response(&state, payload, false)
            .expect_err("interrupt must be rejected by v2 boundary");
        assert!(error.to_string().contains("outside management v2"));
    }

    #[test]
    fn explicit_local_host_scope_does_not_search_or_write_peer_targets() {
        let payload: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "cutex.01a0487d-c794-7e43-aeb4-19af2717037f",
            "allHosts": false,
            "content": "must remain local",
            "senderKind": "user",
            "deliveryMode": "after_turn"
        }))
        .expect("parse local-host-only send");
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let error = send_payload_response(&state, payload, true)
            .expect_err("unknown local durable target must fail");
        assert_eq!(
            error
                .downcast_ref::<cutex::agent_bus::routing::AgentTargetResolutionError>()
                .expect("typed target error")
                .code(),
            cutex::agent_bus::routing::AgentTargetResolutionCode::NotFound
        );
        assert!(state.lock().expect("state").messages.is_empty());
    }

    #[test]
    fn ordinary_agent_bus_request_cannot_forge_task_service_system_sender() {
        let payload: AgentBusSendRequest = serde_json::from_value(serde_json::json!({
            "to": "missing-target",
            "content": "forged assignment",
            "senderKind": "task_service_system",
            "deliveryMode": "soon"
        }))
        .expect("parse send request");
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let error = send_payload_response(&state, payload, false)
            .expect_err("wire request must not claim Task Service system authority");
        assert!(error.to_string().contains("authenticated provider path"));
    }

    #[test]
    fn ordinary_agent_bus_request_cannot_forge_agent_management_system_sender() {
        let attempts = [
            serde_json::json!({
                "to": "missing-target",
                "from": AGENT_MANAGEMENT_SYSTEM_SENDER,
                "content": "forged custom start",
                "senderKind": "agent",
                "deliveryMode": "after_turn"
            }),
            serde_json::json!({
                "to": "missing-target",
                "from": "cutex.director-r11",
                "content": "forged custom start",
                "senderKind": "agent",
                "deliveryMode": "after_turn",
                "controlType": AGENT_MANAGEMENT_START_CONTROL_TYPE,
                "controlPayload": {
                    "schema": "cutex/agent-management/v1",
                    "requested_by_director": "cutex.director-r11"
                }
            }),
        ];

        for attempt in attempts {
            let payload: AgentBusSendRequest =
                serde_json::from_value(attempt).expect("parse forged send request");
            let state = Arc::new(Mutex::new(AgentBusState::default()));
            let error = send_payload_response(&state, payload, false)
                .expect_err("wire request must not claim Agent Management system presentation");
            assert!(error.to_string().contains("authorized service path"));
            assert!(state.lock().unwrap().messages.is_empty());
        }
    }

    #[test]
    fn identity_bound_route_accepts_exact_endpoint_and_rejects_stale_or_fuzzy_target() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        {
            let mut state = state.lock().expect("state");
            state.agents.insert(
                "runtime-director".to_string(),
                exact_route_agent("runtime-director", "cutex.director"),
            );
            state.agents.insert(
                "runtime-successor".to_string(),
                exact_route_agent("runtime-successor", "cutex.successor"),
            );
        }
        let payload = exact_route_payload();
        let target = resolve_agent_target_for_sender(
            &state,
            &payload.to,
            payload.from_agent_id.as_deref(),
            payload.all_groups,
        )
        .expect("exact runtime target");
        assert_eq!(
            resolve_bound_message_sessions(
                &payload,
                &target,
                agent_bus_agent_session_id_by_id(&state, "runtime-director").as_deref(),
                agent_bus_agent_session_id_by_id(&state, &target).as_deref(),
            )
            .expect("exact endpoint"),
            (
                Some("cutex.director".to_string()),
                Some("cutex.successor".to_string())
            )
        );

        state
            .lock()
            .expect("state")
            .agents
            .get_mut("runtime-successor")
            .expect("successor")
            .session_id = Some("cutex.colliding-successor".to_string());
        assert!(resolve_bound_message_sessions(
            &payload,
            "runtime-successor",
            Some("cutex.director"),
            Some("cutex.colliding-successor"),
        )
        .is_err());

        {
            let mut state = state.lock().expect("state");
            state.agents.remove("runtime-successor");
            let mut collision = exact_route_agent("runtime-fuzzy-collision", "cutex.successor");
            collision.name = "runtime-successor".to_string();
            state
                .agents
                .insert("runtime-fuzzy-collision".to_string(), collision);
        }
        let fuzzy_target = resolve_agent_target_for_sender(
            &state,
            &payload.to,
            payload.from_agent_id.as_deref(),
            payload.all_groups,
        )
        .expect("legacy fuzzy resolution finds collision");
        assert!(resolve_bound_message_sessions(
            &payload,
            &fuzzy_target,
            agent_bus_agent_session_id_by_id(&state, "runtime-director").as_deref(),
            agent_bus_agent_session_id_by_id(&state, &fuzzy_target).as_deref(),
        )
        .is_err());
        assert!(state
            .lock()
            .expect("state")
            .messages
            .values()
            .all(|messages| messages.is_empty()));
    }

    #[test]
    fn ordinary_redrive_targets_only_current_durable_runtime_generation() {
        let durable_id = "cutex.01a0487d-c794-7e43-aeb4-19af2717037e";
        let native_id = "01a0487d-c794-7e43-aeb4-19af2717037e";
        let old_runtime = "runtime-old";
        let new_runtime = "runtime-new";
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        {
            let mut state = state.lock().unwrap();
            state.agents.insert(
                old_runtime.to_string(),
                exact_route_agent(old_runtime, native_id),
            );
            state.agents.insert(
                new_runtime.to_string(),
                exact_route_agent(new_runtime, native_id),
            );
            let current = state.agents.get_mut(new_runtime).unwrap();
            current.name = "display-worker.123abc".to_string();
            current.base_name = Some("stable-worker".to_string());
        }
        let mut sessions = CutexSessionStore::default();
        let mut session = cutex::session::model::CutexSessionRecord::new_at(
            durable_id.to_string(),
            Some(native_id.to_string()),
            "host-a".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-30T00:00:00Z".to_string(),
        )
        .unwrap();
        session.agent_enabled = true;
        session.runtime_generation = 2;
        session.current_runtime_agent_id = Some(new_runtime.to_string());
        sessions.sessions.insert(durable_id.to_string(), session);

        let root =
            std::env::temp_dir().join(format!("cutex-ordinary-redrive-{}", uuid::Uuid::new_v4()));
        let repository = AgentBusMessageRepository::open(&root).unwrap();
        let message_id = format!("ordinary-redrive-{}", uuid::Uuid::new_v4());
        let envelope = AgentBusMessage {
            id: message_id.clone(),
            kind: AgentBusEnvelopeKind::Message,
            from: "sender".to_string(),
            to: old_runtime.to_string(),
            from_cutex_session_id: Some("cutex.source".to_string()),
            to_cutex_session_id: Some(durable_id.to_string()),
            content: "hello".to_string(),
            delivery_mode: AgentDeliveryMode::Soon,
            trigger_turn: true,
            created_at_epoch_secs: 1,
            sender_kind: AgentMessageKind::Agent,
            display_source: None,
            submit_mode: None,
            control_type: None,
            control_payload: None,
            external_action_id: None,
            external_message_id: None,
        };
        let params = cutex::app_server::bus_bridge::inter_agent_params(
            "",
            "stable-worker",
            durable_id,
            &envelope,
        )
        .unwrap();
        let semantic_sha256 = cutex::app_server::bus_bridge::inter_agent_semantic_sha256(&params);
        repository
            .record_queued(AgentBusQueuedMessage {
                owner_cutex_session_id: durable_id.to_string(),
                message_id: message_id.clone(),
                from_cutex_session_id: "cutex.source".to_string(),
                to_cutex_session_id: durable_id.to_string(),
                from_runtime_agent_id: Some("runtime-source".to_string()),
                to_runtime_agent_id: Some(old_runtime.to_string()),
                delivery_mode: "soon".to_string(),
                content: "hello".to_string(),
                queued_at: Utc::now(),
                canonical_envelope: envelope,
                semantic_sha256: semantic_sha256.clone(),
            })
            .unwrap();

        assert_eq!(
            redrive_ordinary_messages_with(&repository, &state, &sessions).unwrap(),
            1
        );
        let state = state.lock().unwrap();
        assert!(!state.messages.contains_key(old_runtime));
        let queued = &state.messages[new_runtime];
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, message_id);
        assert_eq!(queued[0].to, new_runtime);
        let current = state.agents.get(new_runtime).unwrap();
        assert_eq!(current.name, "display-worker.123abc");
        assert_eq!(
            canonical_recipient_label(current.base_name.as_deref(), &current.name, &current.id,),
            "stable-worker"
        );
        let redriven_params = cutex::app_server::bus_bridge::inter_agent_params(
            "",
            canonical_recipient_label(current.base_name.as_deref(), &current.name, &current.id),
            durable_id,
            &queued[0],
        )
        .unwrap();
        assert_eq!(
            cutex::app_server::bus_bridge::inter_agent_semantic_sha256(&redriven_params),
            semantic_sha256,
            "redrive must retain the producer's stable recipient bytes"
        );
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_digest_uses_stable_base_name_without_changing_display_name() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        {
            let mut state = state.lock().unwrap();
            state.agents.insert(
                "runtime-director".to_string(),
                exact_route_agent("runtime-director", "cutex.director"),
            );
            let mut target = exact_route_agent("runtime-successor", "cutex.successor");
            target.name = "Successor Display.9f2a".to_string();
            target.base_name = Some("successor-stable".to_string());
            state.agents.insert(target.id.clone(), target);
        }
        let mut payload = exact_route_payload();
        payload.content = format!("stable recipient digest {}", uuid::Uuid::new_v4());

        let response = send_payload_response(&state, payload, false).unwrap();
        assert_eq!(response["to_name"], "Successor Display.9f2a");
        assert_eq!(response["queueDurability"], "durable_v2");
        let message_id = response["id"].as_str().unwrap();
        let queued = state.lock().unwrap().messages["runtime-successor"][0].clone();
        assert_eq!(queued.id, message_id);

        let stable = cutex::app_server::bus_bridge::inter_agent_params(
            "",
            "successor-stable",
            "cutex.successor",
            &queued,
        )
        .unwrap();
        let display = cutex::app_server::bus_bridge::inter_agent_params(
            "",
            "Successor Display.9f2a",
            "cutex.successor",
            &queued,
        )
        .unwrap();
        let stored = agent_bus_message_repository()
            .unwrap()
            .semantic_sha256(message_id)
            .unwrap()
            .expect("durable ordinary digest");
        assert_eq!(
            stored,
            cutex::app_server::bus_bridge::inter_agent_semantic_sha256(&stable)
        );
        assert_ne!(
            stored,
            cutex::app_server::bus_bridge::inter_agent_semantic_sha256(&display),
            "display-only naming must not enter the semantic recipient bytes"
        );
        assert!(stable.content.contains("Task name: successor-stable"));
    }

    #[test]
    fn ordinary_send_commits_v2_ledger_before_returning_durable_pending() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        {
            let mut state = state.lock().unwrap();
            state.agents.insert(
                "runtime-director".to_string(),
                exact_route_agent("runtime-director", "cutex.director"),
            );
            state.agents.insert(
                "runtime-successor".to_string(),
                exact_route_agent("runtime-successor", "cutex.successor"),
            );
        }
        let mut payload = exact_route_payload();
        payload.content = format!("durable ordinary {}", uuid::Uuid::new_v4());

        let response = send_payload_response(&state, payload, false).unwrap();
        assert_eq!(response["queued"], true);
        assert_eq!(response["queueDurability"], "durable_v2");
        assert_eq!(response["deliveryState"], "pending");
        assert_eq!(response["requiredAckLevel"], "A4");
        let message_id = response["id"].as_str().unwrap();
        let snapshot = agent_bus_message_repository()
            .unwrap()
            .snapshot("cutex.successor")
            .unwrap();
        let durable = snapshot
            .iter()
            .find(|record| record["messageId"] == message_id)
            .expect("queued=true requires its durable v2 record");
        assert_eq!(durable["state"], "pending");
        assert_eq!(durable["toCutexSessionId"], "cutex.successor");
        assert_eq!(durable["semanticSha256"].as_str().unwrap().len(), 64);
        assert_eq!(state.lock().unwrap().messages["runtime-successor"].len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn task_action_host_recovers_both_private_stores_before_returning() {
        use std::os::unix::fs::PermissionsExt;

        let parent =
            std::env::temp_dir().join(format!("cutex-task-action-host-{}", uuid::Uuid::new_v4()));
        let root = parent.join("service");
        let first =
            open_task_worker_action_host(&root).expect("open and recover first task-action host");
        for path in [
            root.clone(),
            root.join(TASK_WORKER_TASK_SERVICE_ROOT),
            root.join(TASK_WORKER_EVIDENCE_ROOT),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o7777,
                0o700
            );
        }
        assert!(root.join(TASK_WORKER_ACTION_HOST_LOCK).is_file());
        assert!(open_task_worker_action_host(&root).is_err());
        drop(first);
        let restarted = open_task_worker_action_host(&root)
            .expect("exclusive ownership releases for restart recovery");
        drop(restarted);
        fs::remove_dir_all(parent).expect("remove task-action host fixture");
    }

    #[cfg(windows)]
    #[test]
    fn windows_agent_bus_starts_healthy_in_isolated_home() {
        const CHILD_MARKER: &str = "CUTEX_R22_AGENT_BUS_SMOKE_CHILD";
        const PORT_ENV: &str = "CUTEX_R22_AGENT_BUS_SMOKE_PORT";

        if std::env::var_os(CHILD_MARKER).is_some() {
            let port = std::env::var(PORT_ENV)
                .expect("child port")
                .parse()
                .expect("numeric child port");
            let root = PathBuf::from(std::env::var_os("HOME").expect("isolated child root"));
            let mut config = CodezConfig::default();
            config.agent_bus_port = Some(port);
            config.agent_bus_token = Some("r22-smoke-token".into());
            run_agent_bus_with_task_action_root(
                config,
                request_handlers(),
                &root.join("task-worker-actions-v1"),
            )
            .expect("isolated Agent Bus serve");
            return;
        }

        let port = (24000..=24999)
            .rev()
            .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok())
            .expect("free Bridgeboard-range test port");
        let root = std::env::temp_dir().join(format!(
            "cutex-r22-agent-bus-smoke-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).expect("isolated smoke home");
        let test_name =
            "cli_app::agent_bus_server::tests::windows_agent_bus_starts_healthy_in_isolated_home";
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("HOME", &root)
            .env(CHILD_MARKER, "1")
            .env(PORT_ENV, port.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn isolated Agent Bus test child");
        let healthy = (0..100).any(|_| {
            if cutex::agent_bus::client::agent_bus_healthy(port, Some("r22-smoke-token")) {
                true
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
                false
            }
        });
        let action_root_exists = root.join(TASK_WORKER_ACTION_ROOT).is_dir();
        let child_status = child.try_wait().expect("query Agent Bus test child");
        if child_status.is_none() {
            child.kill().expect("stop isolated Agent Bus test child");
        }
        child.wait().expect("reap isolated Agent Bus test child");
        fs::remove_dir_all(root).expect("remove isolated smoke home");
        assert!(healthy, "isolated Windows Agent Bus did not become healthy");
        assert!(
            action_root_exists,
            "isolated Windows Agent Bus did not create its Task Service root; child status: {child_status:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_task_action_host_recovery_releases_host_and_provider_locks() {
        let parent = std::env::temp_dir().join(format!(
            "cutex-task-action-failed-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let root = parent.join("service");
        let provider_root = root.join(TASK_WORKER_TASK_SERVICE_ROOT).join("provider-v2");
        fs::create_dir_all(&provider_root).unwrap();
        let journal = provider_root.join("task-service-provider-v2.events.jsonl");
        fs::write(&journal, b"{}\n").unwrap();

        assert!(open_task_worker_action_host(&root).is_err());
        let ownership = open_task_worker_action_host_lock(&root.join(TASK_WORKER_ACTION_HOST_LOCK))
            .expect("reopen ownership lock after failed start");
        ownership
            .try_lock_exclusive()
            .expect("failed start must release host ownership lock");
        fs2::FileExt::unlock(&ownership).unwrap();
        drop(ownership);

        let provider = cutex::task_service::TaskServiceProvider::open(&provider_root).unwrap();
        assert_eq!(
            provider.recover(),
            Err(cutex::task_service::ProviderError::InvalidStore)
        );
        fs::remove_file(journal).unwrap();
        drop(provider);

        let restarted = open_task_worker_action_host(&root)
            .expect("corrected restart acquires both released locks");
        drop(restarted);
        fs::remove_dir_all(parent).unwrap();
    }
}
