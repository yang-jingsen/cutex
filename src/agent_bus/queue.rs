//! Agent bus message queue, ack, and send de-duplication helpers.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use serde_json::Value;
use uuid::Uuid;

use crate::agent_bus::delivery::AgentDeliveryMode;
use crate::agent_bus::model::validate_task_service_assignment_summary;
use crate::agent_bus::model::AgentBusEnvelopeKind;
use crate::agent_bus::model::AgentBusMessage;
use crate::agent_bus::model::AgentBusRecentSend;
use crate::agent_bus::model::AgentBusSendOutcome;
use crate::agent_bus::model::AgentMessageKind;
use crate::agent_bus::model::TaskServiceAssignmentMetadata;
use crate::agent_bus::model::TaskServiceCompletionMetadata;
use crate::agent_bus::model::TaskServiceWorkerFollowupMetadata;
use crate::agent_bus::model::UserSubmitMode;
use crate::agent_bus::store::prune_recent_agent_sends;
use crate::agent_bus::store::AgentBusState;

#[derive(Debug, Clone)]
pub struct AgentBusSendIdReservation {
    pub message_id: String,
    pub created_at_epoch_secs: u64,
    pub already_enqueued: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn reserve_agent_bus_message_id(
    state: &Arc<Mutex<AgentBusState>>,
    sender: &str,
    target_id: &str,
    content: &str,
    kind: &AgentBusEnvelopeKind,
    delivery_mode: &AgentDeliveryMode,
    sender_kind: &AgentMessageKind,
    display_source: Option<&str>,
    submit_mode: Option<&UserSubmitMode>,
    control_type: Option<&str>,
    control_payload: Option<&Value>,
    external_action_id: Option<&str>,
    external_message_id: Option<&str>,
    now: u64,
) -> anyhow::Result<AgentBusSendIdReservation> {
    let dedupe_key = agent_bus_send_dedupe_key(
        sender,
        target_id,
        content,
        kind,
        delivery_mode,
        sender_kind,
        display_source,
        submit_mode,
        control_type,
        control_payload,
        external_action_id,
        external_message_id,
    );
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
    prune_recent_agent_sends(&mut state, now);
    if let Some(record) = state.recent_sends.get(&dedupe_key) {
        return Ok(AgentBusSendIdReservation {
            message_id: record.id.clone(),
            created_at_epoch_secs: record.created_at_epoch_secs,
            already_enqueued: true,
        });
    }
    let (message_id, created_at_epoch_secs) = state
        .send_reservations
        .entry(dedupe_key)
        .or_insert_with(|| (Uuid::new_v4().to_string(), now));
    Ok(AgentBusSendIdReservation {
        message_id: message_id.clone(),
        created_at_epoch_secs: *created_at_epoch_secs,
        already_enqueued: false,
    })
}

pub fn ack_agent_messages(
    state: &Arc<Mutex<AgentBusState>>,
    agent_id: &str,
    message_ids: &[String],
) -> anyhow::Result<usize> {
    if message_ids.is_empty() {
        return Ok(0);
    }
    let message_ids = message_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
    let Some(queue) = state.messages.get_mut(agent_id) else {
        return Ok(0);
    };
    let before = queue.len();
    queue.retain(|message| !message_ids.contains(message.id.as_str()));
    let acked = before.saturating_sub(queue.len());
    if queue.is_empty() {
        state.messages.remove(agent_id);
    }
    Ok(acked)
}

pub fn poll_agent_messages(
    state: &Arc<Mutex<AgentBusState>>,
    agent_id: &str,
    ack_mode: bool,
    now: u64,
) -> anyhow::Result<(String, Vec<AgentBusMessage>)> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
    let agent_name = if let Some(agent) = state.agents.get_mut(agent_id) {
        agent.last_seen_epoch_secs = now;
        agent.name.clone()
    } else {
        agent_id.to_string()
    };
    let messages = if ack_mode {
        state
            .messages
            .get(agent_id)
            .map(|queue| queue.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        state
            .messages
            .remove(agent_id)
            .map(|queue| queue.into_iter().collect::<Vec<_>>())
            .unwrap_or_default()
    };
    Ok((agent_name, messages))
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_agent_bus_message_once(
    state: &Arc<Mutex<AgentBusState>>,
    sender: &str,
    target_id: &str,
    target_name: &str,
    content: &str,
    kind: AgentBusEnvelopeKind,
    delivery_mode: AgentDeliveryMode,
    sender_kind: AgentMessageKind,
    display_source: Option<String>,
    submit_mode: Option<UserSubmitMode>,
    control_type: Option<String>,
    control_payload: Option<Value>,
    external_action_id: Option<String>,
    external_message_id: Option<String>,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    enqueue_agent_bus_message_once_with_participants(
        state,
        sender,
        target_id,
        target_name,
        content,
        kind,
        delivery_mode,
        sender_kind,
        display_source,
        submit_mode,
        control_type,
        control_payload,
        external_action_id,
        external_message_id,
        None,
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_agent_bus_message_once_with_participants(
    state: &Arc<Mutex<AgentBusState>>,
    sender: &str,
    target_id: &str,
    target_name: &str,
    content: &str,
    kind: AgentBusEnvelopeKind,
    delivery_mode: AgentDeliveryMode,
    sender_kind: AgentMessageKind,
    display_source: Option<String>,
    submit_mode: Option<UserSubmitMode>,
    control_type: Option<String>,
    control_payload: Option<Value>,
    external_action_id: Option<String>,
    external_message_id: Option<String>,
    from_cutex_session_id: Option<String>,
    to_cutex_session_id: Option<String>,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    enqueue_agent_bus_message_once_with_id(
        state,
        sender,
        target_id,
        target_name,
        content,
        kind,
        delivery_mode,
        sender_kind,
        display_source,
        submit_mode,
        control_type,
        control_payload,
        external_action_id,
        external_message_id,
        from_cutex_session_id,
        to_cutex_session_id,
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn enqueue_agent_bus_message_once_with_id(
    state: &Arc<Mutex<AgentBusState>>,
    sender: &str,
    target_id: &str,
    target_name: &str,
    content: &str,
    kind: AgentBusEnvelopeKind,
    delivery_mode: AgentDeliveryMode,
    sender_kind: AgentMessageKind,
    display_source: Option<String>,
    submit_mode: Option<UserSubmitMode>,
    control_type: Option<String>,
    control_payload: Option<Value>,
    external_action_id: Option<String>,
    external_message_id: Option<String>,
    from_cutex_session_id: Option<String>,
    to_cutex_session_id: Option<String>,
    stable_message_id: Option<String>,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    let dedupe_key = agent_bus_send_dedupe_key(
        sender,
        target_id,
        content,
        &kind,
        &delivery_mode,
        &sender_kind,
        display_source.as_deref(),
        submit_mode.as_ref(),
        control_type.as_deref(),
        control_payload.as_ref(),
        external_action_id.as_deref(),
        external_message_id.as_deref(),
    );
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("agent bus state lock poisoned"))?;
    prune_recent_agent_sends(&mut state, now);
    if let Some(record) = state.recent_sends.get(&dedupe_key).cloned() {
        return Ok(AgentBusSendOutcome {
            record,
            deduplicated: true,
        });
    }
    if let Some(stable_id) = stable_message_id.as_deref() {
        if let Some(message) = state
            .messages
            .get(target_id)
            .and_then(|queue| queue.iter().find(|message| message.id == stable_id))
            .cloned()
        {
            return Ok(AgentBusSendOutcome {
                record: AgentBusRecentSend {
                    id: message.id,
                    kind: message.kind,
                    from: message.from,
                    to: message.to,
                    to_name: target_name.to_string(),
                    delivery_mode: message.delivery_mode,
                    trigger_turn: message.trigger_turn,
                    queued: true,
                    created_at_epoch_secs: message.created_at_epoch_secs,
                    sender_kind: message.sender_kind,
                    display_source: message.display_source,
                    submit_mode: message.submit_mode,
                    control_type: message.control_type,
                    control_payload: message.control_payload,
                    external_action_id: message.external_action_id,
                    external_message_id: message.external_message_id,
                },
                deduplicated: true,
            });
        }
    }

    let message_id = stable_message_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let trigger_turn = delivery_mode.trigger_turn();
    let message = AgentBusMessage {
        id: message_id.clone(),
        kind: kind.clone(),
        from: sender.to_string(),
        to: target_id.to_string(),
        from_cutex_session_id,
        to_cutex_session_id,
        content: content.to_string(),
        delivery_mode: delivery_mode.clone(),
        trigger_turn,
        created_at_epoch_secs: now,
        sender_kind: sender_kind.clone(),
        display_source: display_source.clone(),
        submit_mode: submit_mode.clone(),
        control_type: control_type.clone(),
        control_payload: control_payload.clone(),
        external_action_id: external_action_id.clone(),
        external_message_id: external_message_id.clone(),
    };
    state
        .messages
        .entry(target_id.to_string())
        .or_default()
        .push_back(message);
    let record = AgentBusRecentSend {
        id: message_id,
        kind,
        from: sender.to_string(),
        to: target_id.to_string(),
        to_name: target_name.to_string(),
        delivery_mode,
        trigger_turn,
        queued: true,
        created_at_epoch_secs: now,
        sender_kind,
        display_source,
        submit_mode,
        control_type,
        control_payload,
        external_action_id,
        external_message_id,
    };
    state.recent_sends.insert(dedupe_key, record.clone());
    state
        .send_reservations
        .retain(|_, (reserved_id, _)| reserved_id != &record.id);
    Ok(AgentBusSendOutcome {
        record,
        deduplicated: false,
    })
}

pub(crate) fn enqueue_task_service_system_message_once(
    state: &Arc<Mutex<AgentBusState>>,
    principal: &crate::agent_bus::identity::TaskServiceSystemPrincipal,
    target_id: &str,
    target_name: &str,
    content: &str,
    metadata: &TaskServiceAssignmentMetadata,
    external_action_id: &str,
    external_message_id: &str,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    if !principal.authenticate() {
        return Err(anyhow!(
            "Task Service system principal authentication failed"
        ));
    }
    let contract = metadata.require_valid_contract()?;
    validate_task_service_assignment_summary(content, contract)?;
    enqueue_agent_bus_message_once_with_participants(
        state,
        "cutex-task-service",
        target_id,
        target_name,
        content,
        AgentBusEnvelopeKind::Message,
        AgentDeliveryMode::Soon,
        AgentMessageKind::TaskServiceSystem,
        Some("Cutex Task Service".to_string()),
        None,
        Some("cutex.task_service.assignment.v2".to_string()),
        Some(serde_json::to_value(metadata)?),
        Some(external_action_id.to_string()),
        Some(external_message_id.to_string()),
        None,
        None,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_task_service_completion_message_once(
    state: &Arc<Mutex<AgentBusState>>,
    principal: &crate::agent_bus::identity::TaskServiceSystemPrincipal,
    target_id: &str,
    target_name: &str,
    content: &str,
    metadata: &TaskServiceCompletionMetadata,
    delivery_mode: AgentDeliveryMode,
    external_action_id: &str,
    external_message_id: &str,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    if !principal.authenticate() {
        return Err(anyhow!(
            "Task Service system principal authentication failed"
        ));
    }
    enqueue_agent_bus_message_once_with_id(
        state,
        "cutex-task-service",
        target_id,
        target_name,
        content,
        AgentBusEnvelopeKind::Message,
        delivery_mode,
        AgentMessageKind::TaskServiceSystem,
        Some("Cutex Task Service".to_string()),
        None,
        Some("cutex.task_service.completion.v1".to_string()),
        Some(serde_json::to_value(metadata)?),
        Some(external_action_id.to_string()),
        Some(external_message_id.to_string()),
        None,
        None,
        Some(format!("tsc_{}", metadata.notification_id.as_str())),
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_task_service_worker_followup_message_once(
    state: &Arc<Mutex<AgentBusState>>,
    principal: &crate::agent_bus::identity::TaskServiceSystemPrincipal,
    target_id: &str,
    target_name: &str,
    content: &str,
    metadata: &TaskServiceWorkerFollowupMetadata,
    external_action_id: &str,
    external_message_id: &str,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    if !principal.authenticate() {
        return Err(anyhow!(
            "Task Service system principal authentication failed"
        ));
    }
    if content != metadata.decision_reference || content.trim().is_empty() {
        return Err(anyhow!("invalid Task Service Worker follow-up message"));
    }
    enqueue_agent_bus_message_once_with_id(
        state,
        "cutex-task-service",
        target_id,
        target_name,
        content,
        AgentBusEnvelopeKind::Message,
        AgentDeliveryMode::Soon,
        AgentMessageKind::TaskServiceSystem,
        Some("Cutex Task Service".to_string()),
        None,
        Some("cutex.task_service.worker_followup.v1".to_string()),
        Some(serde_json::to_value(metadata)?),
        Some(external_action_id.to_string()),
        Some(external_message_id.to_string()),
        None,
        None,
        Some(format!("tsf_{}", metadata.notification_id.as_str())),
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn enqueue_task_service_watchdog_message_once(
    state: &Arc<Mutex<AgentBusState>>,
    principal: &crate::agent_bus::identity::TaskServiceSystemPrincipal,
    target_id: &str,
    target_name: &str,
    content: &str,
    metadata: &crate::task_service::TaskWatchdogMessageMetadata,
    delivery_mode: AgentDeliveryMode,
    external_message_id: &str,
    now: u64,
) -> anyhow::Result<AgentBusSendOutcome> {
    if !principal.authenticate() {
        return Err(anyhow!(
            "Task Service system principal authentication failed"
        ));
    }
    if metadata.schema != crate::task_service::TASK_WATCHDOG_MESSAGE_SCHEMA
        || metadata.notification_id != external_message_id
        || content.trim().is_empty()
    {
        return Err(anyhow!("invalid Task watchdog system message"));
    }
    enqueue_agent_bus_message_once_with_id(
        state,
        "cutex-task-service",
        target_id,
        target_name,
        content,
        AgentBusEnvelopeKind::Message,
        delivery_mode,
        AgentMessageKind::TaskServiceSystem,
        Some("Cutex Task Service".to_string()),
        None,
        Some("cutex.task_service.watchdog.v1".to_string()),
        Some(serde_json::to_value(metadata)?),
        Some(metadata.notification_id.clone()),
        Some(external_message_id.to_string()),
        None,
        None,
        Some(format!("tsw_{}", metadata.notification_id)),
        now,
    )
}

fn agent_bus_send_dedupe_key(
    from: &str,
    to: &str,
    content: &str,
    kind: &AgentBusEnvelopeKind,
    delivery_mode: &AgentDeliveryMode,
    sender_kind: &AgentMessageKind,
    display_source: Option<&str>,
    submit_mode: Option<&UserSubmitMode>,
    control_type: Option<&str>,
    control_payload: Option<&Value>,
    external_action_id: Option<&str>,
    external_message_id: Option<&str>,
) -> String {
    let control_payload = control_payload.map(Value::to_string).unwrap_or_default();
    format!(
        "{from}\u{1f}{to}\u{1f}{:?}\u{1f}{}\u{1f}{:?}\u{1f}{}\u{1f}{:?}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{content}",
        kind,
        delivery_mode.label(),
        sender_kind,
        display_source.unwrap_or(""),
        submit_mode,
        control_type.unwrap_or(""),
        control_payload,
        external_action_id.unwrap_or(""),
        external_message_id.unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_service_messages_require_opaque_system_principal_and_keep_structured_metadata() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let metadata = TaskServiceAssignmentMetadata {
            project_id: None,
            schema: crate::task_service::TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            coordinator_cutex_session: Some(
                crate::role_revision::CutexSessionId::new("cutex.director").unwrap(),
            ),
            assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
            task_id: crate::role_revision::TaskId::new("CUTEX-queue").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
            contract_sha256: crate::role_revision::Sha256::new("a".repeat(64)).unwrap(),
            opaque_contract: Some("perform the opaque contract".to_string()),
            send_attempt_id: crate::task_service::SendAttemptId::new("send-1").unwrap(),
        };
        let contract_sha256 = crate::task_service::sha256_bytes(
            metadata.opaque_contract.as_deref().unwrap().as_bytes(),
        );
        let metadata = TaskServiceAssignmentMetadata {
            contract_sha256,
            ..metadata
        };
        let principal = crate::agent_bus::identity::task_service_system_principal();
        let outcome = enqueue_task_service_system_message_once(
            &state,
            &principal,
            "runtime-now",
            "worker",
            "brief assignment summary",
            &metadata,
            "assign-action",
            "message-1",
            42,
        )
        .unwrap();
        assert_eq!(outcome.record.from, "cutex-task-service");
        assert_eq!(
            outcome.record.sender_kind,
            AgentMessageKind::TaskServiceSystem
        );
        let queued = state.lock().unwrap().messages["runtime-now"][0].clone();
        assert_eq!(queued.sender_kind, AgentMessageKind::TaskServiceSystem);
        assert_eq!(
            queued.control_type.as_deref(),
            Some("cutex.task_service.assignment.v2")
        );
        assert_eq!(
            queued.control_payload.unwrap()["assignment_id"],
            "assignment-1"
        );

        let payload = state.lock().unwrap().messages["runtime-now"][0]
            .control_payload
            .clone()
            .unwrap();
        assert_eq!(payload["opaque_contract"], "perform the opaque contract");

        let mut missing = metadata.clone();
        missing.opaque_contract = None;
        assert!(enqueue_task_service_system_message_once(
            &state,
            &principal,
            "runtime-now",
            "worker",
            "summary",
            &missing,
            "assign-action-missing",
            "message-missing",
            43,
        )
        .is_err());

        let mut tampered = metadata;
        tampered.opaque_contract = Some("tampered".to_string());
        assert!(enqueue_task_service_system_message_once(
            &state,
            &principal,
            "runtime-now",
            "worker",
            "summary",
            &tampered,
            "assign-action-tampered",
            "message-tampered",
            44,
        )
        .is_err());

        let contract = "perform the opaque contract";
        let duplicate_metadata = TaskServiceAssignmentMetadata {
            contract_sha256: crate::task_service::sha256_bytes(contract.as_bytes()),
            opaque_contract: Some(contract.to_string()),
            ..tampered
        };
        assert!(enqueue_task_service_system_message_once(
            &state,
            &principal,
            "runtime-now",
            "worker",
            contract,
            &duplicate_metadata,
            "assign-action-duplicate",
            "message-duplicate",
            45,
        )
        .is_err());
    }

    #[test]
    fn ack_removes_only_requested_message_ids() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        enqueue_agent_bus_message_once(
            &state,
            "sender",
            "target",
            "target-name",
            "first",
            AgentBusEnvelopeKind::Message,
            AgentDeliveryMode::AfterTurn,
            AgentMessageKind::Agent,
            None,
            None,
            None,
            None,
            None,
            None,
            1,
        )
        .expect("first message should enqueue");
        let second = enqueue_agent_bus_message_once(
            &state,
            "sender",
            "target",
            "target-name",
            "second",
            AgentBusEnvelopeKind::Message,
            AgentDeliveryMode::AfterTurn,
            AgentMessageKind::Agent,
            None,
            None,
            None,
            None,
            None,
            None,
            1,
        )
        .expect("second message should enqueue")
        .record
        .id;

        assert_eq!(
            ack_agent_messages(&state, "target", std::slice::from_ref(&second))
                .expect("ack should work"),
            1
        );
        let state = state.lock().expect("state should lock");
        let queue = state
            .messages
            .get("target")
            .expect("queue should still have first message");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].content, "first");
    }

    #[test]
    fn concurrent_durable_send_reservations_share_one_identity_until_enqueue() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let reserve = |now| {
            reserve_agent_bus_message_id(
                &state,
                "sender",
                "target",
                "hello",
                &AgentBusEnvelopeKind::Message,
                &AgentDeliveryMode::Soon,
                &AgentMessageKind::Agent,
                None,
                None,
                None,
                None,
                None,
                None,
                now,
            )
            .expect("reservation should succeed")
        };
        let first = reserve(10);
        let concurrent = reserve(11);
        assert_eq!(first.message_id, concurrent.message_id);
        assert_eq!(
            first.created_at_epoch_secs,
            concurrent.created_at_epoch_secs
        );
        assert!(!first.already_enqueued);
        assert!(!concurrent.already_enqueued);

        let enqueue = |id: String, now| {
            enqueue_agent_bus_message_once_with_id(
                &state,
                "sender",
                "target",
                "target-name",
                "hello",
                AgentBusEnvelopeKind::Message,
                AgentDeliveryMode::Soon,
                AgentMessageKind::Agent,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("cutex.sender".to_string()),
                Some("cutex.target".to_string()),
                Some(id),
                now,
            )
            .expect("enqueue should succeed")
        };
        let original = enqueue(first.message_id.clone(), first.created_at_epoch_secs);
        let replay = enqueue(concurrent.message_id, concurrent.created_at_epoch_secs);
        assert!(!original.deduplicated);
        assert!(replay.deduplicated);
        assert_eq!(original.record.id, replay.record.id);
        assert_eq!(state.lock().unwrap().messages["target"].len(), 1);

        let after_enqueue = reserve(12);
        assert!(after_enqueue.already_enqueued);
        assert_eq!(after_enqueue.message_id, original.record.id);
    }

    #[test]
    fn poll_without_ack_drains_queue_but_ack_mode_keeps_it() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        enqueue_agent_bus_message_once(
            &state,
            "sender",
            "target",
            "target-name",
            "hello",
            AgentBusEnvelopeKind::Message,
            AgentDeliveryMode::AfterTurn,
            AgentMessageKind::Agent,
            None,
            None,
            None,
            None,
            None,
            None,
            1,
        )
        .expect("message should enqueue");

        let (_agent_name, messages) =
            poll_agent_messages(&state, "target", true, 2).expect("ack poll should work");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            state
                .lock()
                .expect("state should lock")
                .messages
                .get("target")
                .map(|queue| queue.len()),
            Some(1)
        );

        let (_agent_name, messages) =
            poll_agent_messages(&state, "target", false, 3).expect("drain poll should work");
        assert_eq!(messages.len(), 1);
        assert!(!state
            .lock()
            .expect("state should lock")
            .messages
            .contains_key("target"));
    }

    #[test]
    fn completion_system_message_has_stable_identity_priority_and_external_dedupe() {
        let state = Arc::new(Mutex::new(AgentBusState::default()));
        let metadata = TaskServiceCompletionMetadata {
            project_id: None,
            schema: crate::task_service::TASK_SERVICE_PROVIDER_ACTION_SCHEMA.to_string(),
            notification_id: crate::task_service::NotificationId::new("notification-1").unwrap(),
            assignment_id: crate::task_service::AssignmentId::new("assignment-1").unwrap(),
            task_id: crate::role_revision::TaskId::new("CUTEX-queue").unwrap(),
            task_revision: crate::role_revision::TaskRevision::new(1).unwrap(),
            attempt_number: Some(crate::role_revision::AttemptNumber::new(1).unwrap()),
            transition_action_id: crate::task_service::ActionId::new("block-1").unwrap(),
            kind: crate::task_service::CompletionNotificationKind::Blocked,
            target_seat_id: crate::task_service::SeatId::new("cutex-director").unwrap(),
        };
        let principal = crate::agent_bus::identity::task_service_system_principal();
        let first = enqueue_task_service_completion_message_once(
            &state,
            &principal,
            "runtime-now",
            "director",
            "blocked",
            &metadata,
            AgentDeliveryMode::Soon,
            "block-1",
            "notification-1",
            1,
        )
        .unwrap();
        let replay = enqueue_task_service_completion_message_once(
            &state,
            &principal,
            "runtime-now",
            "director",
            "blocked",
            &metadata,
            AgentDeliveryMode::Soon,
            "block-1",
            "notification-1",
            100,
        )
        .unwrap();
        assert_eq!(first.record.id, "tsc_notification-1");
        assert_eq!(first.record.id, replay.record.id);
        assert!(replay.deduplicated);
        assert_eq!(replay.record.delivery_mode, AgentDeliveryMode::Soon);
        assert_eq!(state.lock().unwrap().messages["runtime-now"].len(), 1);
    }
}
