use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;
use serde_json::json;
use serde_json::Value;
use uuid::Uuid;

use cutex::app_server::client::AppServerEvent;
use cutex::app_server::commands::ThreadReadParams;
use cutex::app_server::commands::TurnInterruptParams;
use cutex::management::v2::model::CutexMessage;
use cutex::management::v2::model::EventCorrelation;
use cutex::management::v2::model::EventSource;
use cutex::management::v2::model::PendingEvent;
use cutex::management::v2::repository::management_v2_repository;
use cutex::management::v2::user_input::user_input_repository;
use cutex::management::v2::user_input::ClientIdentityDecision;
use cutex::management::v2::user_input::QueueEnqueueDecision;
use cutex::management::v2::user_input::UserInputDisposition;
use cutex::management::v2::user_input::UserInputExecutionError;
use cutex::management::v2::user_input::UserInputStrategy;
use cutex::management::v2::user_input::UserInputSubmitCommand;
use cutex::management::v2::user_input::UserInputSubmitExecution;
use cutex::platform::host::current_host_name;

use super::app_server_runtime;

const INTERRUPT_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
const INTERRUPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

static SESSION_INPUT_LOCKS: OnceLock<Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>> =
    OnceLock::new();

pub(crate) fn submit_v2(
    command: UserInputSubmitCommand,
) -> Result<UserInputSubmitExecution, UserInputExecutionError> {
    let session_lock = session_input_lock(&command.cutex_session_id)
        .map_err(|error| execution_error("route", "internal_error", error, true, false))?;
    let _session_guard = session_lock.lock().map_err(|_| {
        execution_error_message(
            "route",
            "internal_error",
            "app-server session input lock was poisoned",
            true,
            false,
        )
    })?;
    let repository = user_input_repository().map_err(|error| {
        execution_error("route", "event_persistence_unavailable", error, true, false)
    })?;
    submit_v2_with_repository(command, repository, read_thread)
}

fn submit_v2_with_repository(
    command: UserInputSubmitCommand,
    repository: &cutex::management::v2::user_input::UserInputRepository,
    read_native_thread: impl Fn(&str, &str) -> anyhow::Result<Value>,
) -> Result<UserInputSubmitExecution, UserInputExecutionError> {
    match repository
        .register_identity(&command.cutex_session_id, &command.params)
        .map_err(|error| {
            execution_error("route", "event_persistence_unavailable", error, true, false)
        })? {
        ClientIdentityDecision::Conflict => {
            return Err(execution_error_message(
                "route",
                "idempotency_conflict",
                "clientUserMessageId was already used with different input or origin",
                false,
                false,
            ));
        }
        ClientIdentityDecision::New | ClientIdentityDecision::Existing => {}
    }

    let thread = match read_native_thread(&command.cutex_session_id, &command.thread_id) {
        Ok(thread) => Some(thread),
        Err(_)
            if command.params.strategy == UserInputStrategy::Queue
                && command.params.expected_turn_id.is_none() =>
        {
            None
        }
        Err(error) => {
            return Err(execution_error(
                "native_request",
                "app_server_unavailable",
                error,
                true,
                false,
            ))
        }
    };
    if let Some(turn_id) = thread
        .as_ref()
        .and_then(|thread| client_message_turn_id(thread, &command.params.client_user_message_id))
    {
        return Ok(UserInputSubmitExecution {
            disposition: UserInputDisposition::Deduplicated,
            app_server_accepted: true,
            native_request_id: None,
            native_method: None,
            turn_id: Some(turn_id),
            queue: None,
        });
    }
    let active_turn_id = thread.as_ref().and_then(active_turn_id_from_thread);
    if let Some(expected_turn_id) = command.params.expected_turn_id.as_deref() {
        if active_turn_id.as_deref() != Some(expected_turn_id) {
            return Err(UserInputExecutionError {
                stage: "route".to_string(),
                code: "turn_conflict".to_string(),
                message: "expectedTurnId does not match the active native turn".to_string(),
                retryable: true,
                details: json!({
                    "expectedTurnId": expected_turn_id,
                    "activeTurnId": active_turn_id,
                }),
                outcome_unknown: false,
            });
        }
    }

    if command.params.strategy == UserInputStrategy::Queue {
        return match repository
            .enqueue(
                &command.cutex_session_id,
                &command.thread_id,
                &command.management_request_id,
                &command.params,
            )
            .map_err(|error| {
                execution_error("queue", "event_persistence_unavailable", error, true, false)
            })? {
            QueueEnqueueDecision::Queued(queue) => Ok(UserInputSubmitExecution {
                disposition: UserInputDisposition::Queued,
                app_server_accepted: false,
                native_request_id: None,
                native_method: None,
                turn_id: active_turn_id,
                queue: Some(queue),
            }),
            QueueEnqueueDecision::Deduplicated(queue) => Ok(UserInputSubmitExecution {
                disposition: UserInputDisposition::Deduplicated,
                app_server_accepted: false,
                native_request_id: None,
                native_method: None,
                turn_id: active_turn_id,
                queue: Some(queue),
            }),
            QueueEnqueueDecision::ClientMessageConflict => Err(execution_error_message(
                "queue",
                "idempotency_conflict",
                "clientUserMessageId was already used with different input or origin",
                false,
                false,
            )),
        };
    }

    if let Some(active_turn_id) = active_turn_id {
        match command.params.strategy {
            UserInputStrategy::Auto => {
                return send_user_input_native_request(
                    &command,
                    "turn/steer",
                    json!({
                        "threadId": command.thread_id,
                        "input": command.params.input,
                        "clientUserMessageId": command.params.client_user_message_id,
                        "expectedTurnId": active_turn_id,
                    }),
                    UserInputDisposition::Steered,
                );
            }
            UserInputStrategy::Interrupt => {
                app_server_runtime::runtime_manager()
                    .commands(&command.cutex_session_id)
                    .and_then(|commands| {
                        commands
                            .turn_interrupt(&TurnInterruptParams {
                                thread_id: command.thread_id.clone(),
                                turn_id: active_turn_id.clone(),
                            })
                            .map(|_| ())
                            .map_err(Into::into)
                    })
                    .map_err(|error| {
                        execution_error("interrupt", "app_server_unavailable", error, true, true)
                    })?;
                wait_for_turn_to_stop(&command.cutex_session_id, &active_turn_id).map_err(
                    |error| execution_error("interrupt", "interrupt_timeout", error, true, true),
                )?;
            }
            UserInputStrategy::Queue => unreachable!("explicit queue returned above"),
        }
    }

    send_user_input_native_request(
        &command,
        "turn/start",
        json!({
            "threadId": command.thread_id,
            "input": command.params.input,
            "clientUserMessageId": command.params.client_user_message_id,
        }),
        UserInputDisposition::Started,
    )
}

fn send_user_input_native_request(
    command: &UserInputSubmitCommand,
    method: &str,
    params: Value,
    disposition: UserInputDisposition,
) -> Result<UserInputSubmitExecution, UserInputExecutionError> {
    use cutex::app_server::client::AppServerClientError;

    let native_request_id = format!("cutex-user-input-{}", Uuid::new_v4());
    let handle = app_server_runtime::runtime_manager()
        .handle(&command.cutex_session_id)
        .map_err(|error| {
            execution_error(
                "native_request",
                "app_server_unavailable",
                error,
                true,
                false,
            )
        })?;
    let response = handle
        .request_raw_message(json!({
            "id": native_request_id,
            "method": method,
            "params": params,
        }))
        .map_err(|error| {
            let outcome_unknown = matches!(
                error,
                AppServerClientError::Transport(_)
                    | AppServerClientError::Disconnected(_)
                    | AppServerClientError::Timeout { .. }
            );
            execution_error_message(
                "native_request",
                if outcome_unknown {
                    "request_outcome_unknown"
                } else {
                    "app_server_unavailable"
                },
                &error.to_string(),
                !outcome_unknown,
                outcome_unknown,
            )
        })?;
    if let Some(error) = response.get("error") {
        return Err(UserInputExecutionError {
            stage: "native_request".to_string(),
            code: "native_request_failed".to_string(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("native user-input request failed")
                .to_string(),
            retryable: false,
            details: json!({ "nativeError": error }),
            outcome_unknown: false,
        });
    }
    let turn_id = response
        .pointer("/result/turn/id")
        .or_else(|| response.pointer("/result/turnId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    app_server_runtime::runtime_manager()
        .note_active_turn(&command.cutex_session_id, turn_id.clone())
        .map_err(|error| execution_error("native_request", "internal_error", error, true, false))?;
    Ok(UserInputSubmitExecution {
        disposition,
        app_server_accepted: true,
        native_request_id: Some(Value::String(native_request_id)),
        native_method: Some(method.to_string()),
        turn_id,
        queue: None,
    })
}

fn execution_error(
    stage: &str,
    code: &str,
    error: impl std::fmt::Display,
    retryable: bool,
    outcome_unknown: bool,
) -> UserInputExecutionError {
    execution_error_message(stage, code, &error.to_string(), retryable, outcome_unknown)
}

fn execution_error_message(
    stage: &str,
    code: &str,
    message: &str,
    retryable: bool,
    outcome_unknown: bool,
) -> UserInputExecutionError {
    UserInputExecutionError {
        stage: stage.to_string(),
        code: code.to_string(),
        message: message.to_string(),
        retryable,
        details: json!({}),
        outcome_unknown,
    }
}

pub(crate) fn handle_runtime_event(
    cutex_session_id: &str,
    event: &AppServerEvent,
) -> anyhow::Result<()> {
    let AppServerEvent::Notification(notification) = event else {
        return Ok(());
    };
    let should_flush = notification.method == "turn/completed"
        || (notification.method == "thread/status/changed"
            && notification
                .params
                .as_ref()
                .and_then(|params| params.pointer("/status/type"))
                .and_then(Value::as_str)
                == Some("idle"));
    if should_flush {
        flush_queued_if_idle(cutex_session_id)?;
    }
    Ok(())
}

pub(crate) fn flush_queued_if_idle(cutex_session_id: &str) -> anyhow::Result<bool> {
    let session_lock = session_input_lock(cutex_session_id)?;
    let _session_guard = session_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("app-server session input lock was poisoned"))?;
    let repository = user_input_repository()?;
    let mut changed = false;
    loop {
        let Some((management_request_id, thread_id, queue_item)) =
            repository.front(cutex_session_id)?
        else {
            return Ok(changed);
        };
        let thread = read_thread(cutex_session_id, &thread_id)?;
        if client_message_turn_id(&thread, &queue_item.client_user_message_id).is_some() {
            let removed = repository
                .remove(cutex_session_id, &queue_item.queue_id, queue_item.revision)?
                .context("queued user input disappeared during materialization cleanup")?;
            append_queue_removed_event(
                cutex_session_id,
                &management_request_id,
                &removed,
                "submitted",
            )?;
            changed = true;
            continue;
        }
        if active_turn_id_from_thread(&thread).is_some() {
            return Ok(changed);
        }
        let command = UserInputSubmitCommand {
            management_request_id: management_request_id.clone(),
            cutex_session_id: cutex_session_id.to_string(),
            thread_id: thread_id.clone(),
            params: cutex::management::v2::user_input::UserInputSubmitParams {
                client_user_message_id: queue_item.client_user_message_id.clone(),
                origin: queue_item.origin.clone(),
                strategy: UserInputStrategy::Auto,
                input: queue_item.input.clone(),
                expected_turn_id: None,
            },
        };
        let execution = match send_user_input_native_request(
            &command,
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": queue_item.input,
                "clientUserMessageId": queue_item.client_user_message_id,
            }),
            UserInputDisposition::Started,
        ) {
            Ok(execution) => execution,
            Err(error) => {
                append_input_event(
                    cutex_session_id,
                    EventCorrelation {
                        thread_id: Some(command.thread_id.clone()),
                        client_user_message_id: Some(command.params.client_user_message_id.clone()),
                        management_request_id: Some(management_request_id.clone()),
                        queue_id: Some(queue_item.queue_id.clone()),
                        ..Default::default()
                    },
                    "cutex/userInput/failed",
                    json!({
                        "managementRequestId": management_request_id,
                        "clientUserMessageId": command.params.client_user_message_id,
                        "origin": command.params.origin,
                        "input": command.params.input,
                        "stage": error.stage,
                        "error": {
                            "source": "cutex",
                            "code": error.code,
                            "message": error.message,
                            "retryable": error.retryable,
                            "details": error.details,
                        },
                        "failedAt": Utc::now().to_rfc3339(),
                    }),
                )?;
                anyhow::bail!("queued native user input failed: {}", error.message);
            }
        };
        append_input_event(
            cutex_session_id,
            EventCorrelation {
                thread_id: Some(command.thread_id.clone()),
                turn_id: execution.turn_id.clone(),
                client_user_message_id: Some(command.params.client_user_message_id.clone()),
                management_request_id: Some(management_request_id.clone()),
                queue_id: Some(queue_item.queue_id.clone()),
                native_request_id: execution.native_request_id.clone(),
                ..Default::default()
            },
            "cutex/userInput/submitted",
            json!({
                "managementRequestId": management_request_id,
                "clientUserMessageId": command.params.client_user_message_id,
                "origin": command.params.origin,
                "input": command.params.input,
                "disposition": execution.disposition,
                "nativeRequestId": execution.native_request_id,
                "nativeMethod": execution.native_method,
                "turnId": execution.turn_id,
                "appServerAccepted": true,
                "submittedAt": Utc::now().to_rfc3339(),
            }),
        )?;
        let removed = repository
            .remove(cutex_session_id, &queue_item.queue_id, queue_item.revision)?
            .context("queued user input disappeared after native submission")?;
        append_queue_removed_event(
            cutex_session_id,
            &command.management_request_id,
            &removed,
            "submitted",
        )?;
        return Ok(true);
    }
}

fn append_queue_removed_event(
    cutex_session_id: &str,
    management_request_id: &str,
    removed: &cutex::management::v2::user_input::UserInputQueueItem,
    reason: &str,
) -> anyhow::Result<()> {
    append_input_event(
        cutex_session_id,
        EventCorrelation {
            client_user_message_id: Some(removed.client_user_message_id.clone()),
            management_request_id: Some(management_request_id.to_string()),
            queue_id: Some(removed.queue_id.clone()),
            ..Default::default()
        },
        "cutex/userInput/queueRemoved",
        json!({
            "queueId": removed.queue_id,
            "clientUserMessageId": removed.client_user_message_id,
            "revision": removed.revision,
            "reason": reason,
            "removedAt": Utc::now().to_rfc3339(),
        }),
    )
}

fn append_input_event(
    cutex_session_id: &str,
    correlation: EventCorrelation,
    method: &str,
    params: Value,
) -> anyhow::Result<()> {
    management_v2_repository()?.append(PendingEvent {
        cutex_session_id: cutex_session_id.to_string(),
        host_id: current_host_name(),
        source: EventSource::Cutex,
        schema: None,
        correlation,
        native: None,
        cutex: Some(CutexMessage {
            method: method.to_string(),
            params,
        }),
    })?;
    Ok(())
}

fn read_thread(cutex_session_id: &str, thread_id: &str) -> anyhow::Result<Value> {
    Ok(app_server_runtime::runtime_manager()
        .commands(cutex_session_id)?
        .thread_read(&ThreadReadParams {
            thread_id: thread_id.to_string(),
            include_turns: true,
        })?)
}

fn active_turn_id_from_thread(response: &Value) -> Option<String> {
    response
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .and_then(|turns| {
            turns
                .iter()
                .rev()
                .find(|turn| turn.get("status").and_then(Value::as_str) == Some("inProgress"))
        })
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn client_message_turn_id(response: &Value, client_message_id: &str) -> Option<String> {
    response
        .pointer("/thread/turns")
        .and_then(Value::as_array)?
        .iter()
        .find(|turn| {
            turn.get("items")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("userMessage")
                            && item.get("clientId").and_then(Value::as_str)
                                == Some(client_message_id)
                    })
                })
        })
        .and_then(|turn| turn.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn wait_for_turn_to_stop(cutex_session_id: &str, turn_id: &str) -> anyhow::Result<()> {
    let manager = app_server_runtime::runtime_manager();
    let deadline = Instant::now() + INTERRUPT_WAIT_TIMEOUT;
    loop {
        let active_turn_id = manager
            .status(cutex_session_id)?
            .and_then(|status| status.active_turn_id);
        if active_turn_id.as_deref() != Some(turn_id) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for interrupted turn {turn_id} to stop");
        }
        std::thread::sleep(INTERRUPT_POLL_INTERVAL);
    }
}

fn session_input_lock(cutex_session_id: &str) -> anyhow::Result<std::sync::Arc<Mutex<()>>> {
    let mut locks = SESSION_INPUT_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("app-server session input lock registry was poisoned"))?;
    Ok(locks
        .entry(cutex_session_id.to_string())
        .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
        .clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::management::v2::user_input::UserInputOrigin;
    use cutex::management::v2::user_input::UserInputOriginKind;
    use cutex::management::v2::user_input::UserInputRepository;

    #[test]
    fn native_thread_history_correlates_user_client_message_id() {
        let thread = serde_json::json!({
            "thread": {
                "turns": [
                    {
                        "id": "turn-1",
                        "status": "completed",
                        "items": [
                            {
                                "type": "userMessage",
                                "id": "item-1",
                                "clientId": "message-1",
                                "content": [{"type": "text", "text": "hello"}]
                            }
                        ]
                    },
                    {
                        "id": "turn-2",
                        "status": "inProgress",
                        "items": []
                    }
                ]
            }
        });

        assert_eq!(
            client_message_turn_id(&thread, "message-1").as_deref(),
            Some("turn-1")
        );
        assert_eq!(
            active_turn_id_from_thread(&thread).as_deref(),
            Some("turn-2")
        );
        assert!(client_message_turn_id(&thread, "missing").is_none());
    }

    #[test]
    fn explicit_queue_is_durable_while_app_server_is_offline() {
        let root = std::env::temp_dir().join(format!(
            "cutex-v2-offline-user-input-queue-{}",
            uuid::Uuid::new_v4()
        ));
        let repository = UserInputRepository::open(&root).expect("open user-input repository");
        let command = UserInputSubmitCommand {
            management_request_id: "management-offline-queue-1".to_string(),
            cutex_session_id: "cutex.offline-queue".to_string(),
            thread_id: "thread-offline-queue".to_string(),
            params: cutex::management::v2::user_input::UserInputSubmitParams {
                client_user_message_id: "client-offline-queue-1".to_string(),
                origin: UserInputOrigin {
                    kind: UserInputOriginKind::Android,
                    client_id: "integration-phone".to_string(),
                },
                strategy: UserInputStrategy::Queue,
                input: vec![json!({
                    "type": "text",
                    "text": "queue while offline",
                    "text_elements": []
                })],
                expected_turn_id: None,
            },
        };

        let execution = submit_v2_with_repository(command, &repository, |_, _| {
            anyhow::bail!("app-server is offline")
        })
        .expect("explicit queue must not require app-server state");

        assert_eq!(execution.disposition, UserInputDisposition::Queued);
        assert!(!execution.app_server_accepted);
        assert_eq!(execution.turn_id, None);
        assert!(execution.queue.is_some());
        let (revision, items) = repository
            .list("cutex.offline-queue")
            .expect("read durable queue");
        assert_eq!(revision, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].client_user_message_id, "client-offline-queue-1");
        std::fs::remove_dir_all(root).expect("remove user-input repository");
    }
}
