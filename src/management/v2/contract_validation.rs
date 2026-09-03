use std::sync::OnceLock;

use crate::agent_management::ProjectId;
use crate::task_service::{ProviderReceipt, ProviderReceiptSchema, ProviderResult};
use serde_json::Value;
#[cfg(test)]
use sha2::Digest;
#[cfg(test)]
use sha2::Sha256;

const CUTEX_METHOD_SCHEMA: &str = include_str!("schema/cutex-methods-v2.json");

static CUTEX_EVENT_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static CUTEX_REQUEST_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
static CUTEX_RESPONSE_VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();

pub fn validate_cutex_event_message(message: &Value) -> Result<(), String> {
    validate_with(
        CUTEX_EVENT_VALIDATOR.get_or_init(|| validator_for_definition("eventMessage")),
        message,
        "cutex event",
    )?;
    validate_task_service_event_contract(message)
}

pub fn validate_cutex_request(request: &Value) -> Result<(), String> {
    validate_with(
        CUTEX_REQUEST_VALIDATOR.get_or_init(|| validator_for_definition("request")),
        request,
        "cutex request",
    )
}

pub fn validate_cutex_response(response: &Value) -> Result<(), String> {
    validate_with(
        CUTEX_RESPONSE_VALIDATOR.get_or_init(|| validator_for_definition("response")),
        response,
        "cutex response",
    )
}

fn validator_for_definition(definition: &str) -> jsonschema::Validator {
    let mut schema: Value = serde_json::from_str(CUTEX_METHOD_SCHEMA)
        .expect("embedded management v2 cutex method schema must be valid JSON");
    schema
        .as_object_mut()
        .expect("management v2 cutex method schema root must be an object")
        .insert(
            "$ref".to_string(),
            Value::String(format!("#/$defs/{definition}")),
        );
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(&schema)
        .expect("embedded management v2 cutex method schema must compile")
}

fn validate_with(
    validator: &jsonschema::Validator,
    value: &Value,
    label: &str,
) -> Result<(), String> {
    validator
        .validate(value)
        .map_err(|error| format!("{label} does not match management v2: {error}"))
}

fn validate_task_service_event_contract(message: &Value) -> Result<(), String> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(params) = message.get("params") else {
        return Ok(());
    };
    let result = match method {
        "cutex/taskService/assignmentCommitted" | "cutex/taskService/communicationRecorded" => {
            let receipt: ProviderReceipt = serde_json::from_value(params.clone())
                .map_err(|error| format!("invalid Task Service receipt: {error}"))?;
            validate_task_service_activity_receipt(&receipt, method).map(|_| ())
        }
        crate::management::v2::integration_events::TASK_SERVICE_ASSIGNMENT_TRANSITION_METHOD => {
            let fact: crate::management::v2::integration_events::TaskAssignmentTransitionFact =
                serde_json::from_value(params.clone())
                    .map_err(|error| format!("invalid Task Service transition fact: {error}"))?;
            if fact.schema
                != crate::management::v2::integration_events::TASK_SERVICE_ASSIGNMENT_TRANSITION_SCHEMA
            {
                Err("unsupported Task Service transition schema".to_string())
            } else {
                Ok(())
            }
        }
        crate::management::v2::integration_events::TASK_WATCHDOG_FIRST_STALE_METHOD
        | crate::management::v2::integration_events::TASK_WATCHDOG_DIRECTOR_ESCALATED_METHOD => {
            let fact: crate::task_service::TaskWatchdogFact =
                serde_json::from_value(params.clone())
                    .map_err(|error| format!("invalid Task watchdog fact: {error}"))?;
            if fact.schema != crate::task_service::TASK_WATCHDOG_FACT_SCHEMA
                || fact.event_key != fact.stage.event_key()
            {
                Err("inconsistent Task watchdog fact".to_string())
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    };
    result.map_err(|error| format!("cutex event does not match management v2: {error}"))
}

pub(crate) fn validate_task_service_activity_receipt(
    receipt: &ProviderReceipt,
    method: &str,
) -> Result<Option<ProjectId>, String> {
    let mut project_fields: Vec<(&str, Option<&ProjectId>)> = Vec::new();
    match (method, &receipt.result) {
        (
            "cutex/taskService/assignmentCommitted",
            ProviderResult::Assignment {
                assignment,
                send_attempt,
            },
        ) => {
            project_fields.push(("assignment", assignment.project_id.as_ref()));
            if let Some(retry) = assignment.retry_authorization.as_ref() {
                project_fields.push(("retry_authorization", retry.project_id.as_ref()));
            }
            if let Some(closure) = assignment.closure.as_ref() {
                project_fields.push(("assignment_closure", closure.project_id.as_ref()));
            }
            if let Some(send_attempt) = send_attempt.as_ref() {
                if send_attempt.assignment_id != assignment.assignment_id {
                    return Err(
                        "assignment receipt send attempt has a different assignment_id".to_string(),
                    );
                }
                project_fields.push(("send_attempt", send_attempt.project_id.as_ref()));
            }
        }
        ("cutex/taskService/communicationRecorded", ProviderResult::SendAttempt(send_attempt)) => {
            project_fields.push(("send_attempt", send_attempt.project_id.as_ref()));
        }
        ("cutex/taskService/assignmentCommitted", _) => {
            return Err("assignmentCommitted requires an assignment receipt".to_string())
        }
        ("cutex/taskService/communicationRecorded", _) => {
            return Err("communicationRecorded requires a SendAttempt receipt".to_string())
        }
        _ => return Err("unsupported Task Service activity method".to_string()),
    }

    match receipt.schema {
        ProviderReceiptSchema::V2 => {
            if let Some((field, _)) = project_fields.iter().find(|(_, project)| project.is_some()) {
                return Err(format!(
                    "v2 Task Service receipt contains project-scoped {field}"
                ));
            }
            Ok(None)
        }
        ProviderReceiptSchema::V3 => {
            let Some(project_id) = project_fields
                .first()
                .and_then(|(_, project)| project.as_ref())
                .copied()
            else {
                return Err("v3 Task Service receipt is unscoped".to_string());
            };
            for (field, candidate) in project_fields {
                match candidate {
                    Some(candidate) if candidate == project_id => {}
                    Some(_) => {
                        return Err(format!(
                            "v3 Task Service receipt has mixed project_id in {field}"
                        ))
                    }
                    None => return Err(format!("v3 Task Service receipt has unscoped {field}")),
                }
            }
            Ok(Some(project_id.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::management::v2::session::CUTEX_METHOD_REGISTRY_SCHEMA_SHA256;

    fn assignment_body(project_id: Option<&str>) -> Value {
        let mut assignment = json!({
            "assignment_id": "assignment-r23",
            "task_id": "CUTEX-R23",
            "task_revision": 1,
            "assignee_cutex_session": "cutex.worker",
            "state": "awaiting_ack",
            "local_revision": 1,
            "created_at": "2026-08-30T00:00:00Z",
            "acknowledged_at": null,
            "active_attempt": null,
            "retry_authorization": null,
            "closure": null
        });
        if let Some(project_id) = project_id {
            assignment["project_id"] = json!(project_id);
        }
        json!({ "assignment": assignment, "send_attempt": null })
    }

    fn send_attempt_body(project_id: Option<&str>) -> Value {
        let mut send_attempt = json!({
            "send_attempt_id": "send-r23",
            "assignment_id": "assignment-r23",
            "retry_ordinal": 0,
            "external_message_id": "external-r23",
            "local_revision": 1,
            "events": []
        });
        if let Some(project_id) = project_id {
            send_attempt["project_id"] = json!(project_id);
        }
        send_attempt
    }

    fn task_receipt_event(
        method: &str,
        schema: &str,
        kind: &str,
        project_id: Option<&str>,
    ) -> Value {
        let body = match kind {
            "assignment" => assignment_body(project_id),
            "send_attempt" => send_attempt_body(project_id),
            _ => json!({}),
        };
        json!({
            "method": method,
            "params": {
                "schema": schema,
                "action_id": format!("activity-{kind}-r23"),
                "request_sha256": "d".repeat(64),
                "attempt_binding": null,
                "committed_at": "2026-08-30T00:00:00Z",
                "journal_sequence": 8,
                "result": { "kind": kind, "body": body }
            }
        })
    }

    #[test]
    fn embedded_schema_hash_and_runtime_event_validation_are_exact() {
        assert_eq!(
            format!("{:x}", Sha256::digest(CUTEX_METHOD_SCHEMA.as_bytes())),
            CUTEX_METHOD_REGISTRY_SCHEMA_SHA256
        );
        validate_cutex_event_message(&json!({
            "method": "cutex/runtime/online",
            "params": {
                "runtimeGeneration": 2,
                "backend": "host",
                "status": "online",
                "runtimeAgentId": null,
                "occurredAt": "2026-07-13T00:00:00Z"
            }
        }))
        .expect("valid runtime event");
        validate_cutex_request(&json!({
            "requestId": "runtime-online-1",
            "method": "cutex/runtime/online",
            "params": {
                "expectedRuntimeGeneration": 2,
                "openVisibleTerminal": false,
                "launchProfile": "beta"
            }
        }))
        .expect("valid online lifecycle controls");
        assert!(validate_cutex_request(&json!({
            "requestId": "runtime-online-empty-profile",
            "method": "cutex/runtime/online",
            "params": {
                "expectedRuntimeGeneration": 2,
                "launchProfile": ""
            }
        }))
        .is_err());
        assert!(validate_cutex_request(&json!({
            "requestId": "runtime-online-2",
            "method": "cutex/runtime/online",
            "params": {
                "expectedRuntimeGeneration": 2,
                "force": true
            }
        }))
        .is_err());
        assert!(validate_cutex_event_message(&json!({
            "method": "cutex/runtime/online",
            "params": {
                "runtimeGeneration": 2,
                "backend": "host",
                "status": "not-a-status",
                "occurredAt": "2026-07-13T00:00:00Z"
            }
        }))
        .is_err());
    }

    #[test]
    fn managed_agent_bus_and_task_events_have_strict_additive_shapes() {
        for event in [
            json!({
                "method": "cutex/agentManagement/actionCompleted",
                "params": {
                    "schema": "cutex/agent-management-receipt/v1",
                    "action_id": "create-1",
                    "request_sha256": "a".repeat(64),
                    "operation": "create",
                    "project_id": "cutex-stack-main",
                    "completed_at": "2026-08-27T00:00:00Z",
                    "result": { "kind": "created", "agent": {} }
                }
            }),
            json!({
                "method": "cutex/agentManagement/actionFailed",
                "params": {
                    "schema": "cutex/agent-management-failure/v1",
                    "event_id": "agent-management:create-1:failure",
                    "action_id": "create-1",
                    "project_id": "cutex-stack-main",
                    "operation": "restart",
                    "code": "owner_action_required",
                    "detail": "external outcome unknown",
                    "routing_status": "routable",
                    "route_to_director_session": "cutex.director",
                    "target_cutex_session_id": "cutex.worker",
                    "created_at": "2026-08-27T00:00:00Z"
                }
            }),
            json!({
                "method": "cutex/agentManagement/actionPhaseTransitionCommitted",
                "params": {
                    "schema": "cutex/agent-management-phase-transition/v1",
                    "phase_event_id": "agent-management:rotate-1:phase:14",
                    "action_id": "rotate-1",
                    "project_id": "cutex-stack-main",
                    "operation": "director_rotate",
                    "phase": "successor_ready",
                    "phase_sequence": 14,
                    "committed_at": "2026-08-27T00:00:00Z",
                    "primary_presentation_target_cutex_session_id": "cutex.director-new",
                    "primary_presentation_target_metadata": {
                        "displayName": "New Director",
                        "cutexSessionId": "cutex.director-new",
                        "profile": "aemeath",
                        "model": "gpt-5.6-sol",
                        "reasoning": "high",
                        "role": null,
                        "runtimeBackend": "host"
                    },
                    "predecessor_cutex_session_id": "cutex.director-old",
                    "predecessor_metadata": null,
                    "successor_cutex_session_id": "cutex.director-new",
                    "successor_metadata": null,
                    "replace_policy": null,
                    "rotation_mode": "close_predecessor_then_create_with_message",
                    "authority_epoch": 8
                }
            }),
            json!({
                "method": "cutex/agentBus/messageSent",
                "params": {
                    "messageId": "message-1",
                    "fromCutexSessionId": "cutex.director",
                    "toCutexSessionId": "cutex.worker",
                    "fromRuntimeAgentId": "runtime-director",
                    "toRuntimeAgentId": "runtime-worker",
                    "deliveryMode": "after_turn",
                    "content": "follow-up",
                    "sentAt": "2026-08-27T00:00:00Z"
                }
            }),
            task_receipt_event(
                "cutex/taskService/assignmentCommitted",
                "cutex/task-service-receipt/v2",
                "assignment",
                None,
            ),
            task_receipt_event(
                "cutex/taskService/communicationRecorded",
                "cutex/task-service-receipt/v2",
                "send_attempt",
                None,
            ),
            json!({
                "method": "cutex/taskService/assignmentTransitionCommitted",
                "params": {
                    "schema": "cutex/task-service-assignment-transition/v1",
                    "transition": "review_ready",
                    "action_id": "submit-1",
                    "assignment_id": "assignment-1",
                    "task_id": "CUTEX-188",
                    "assignee_cutex_session_id": "cutex.worker",
                    "attempt_number": 1,
                    "closure_reason": null,
                    "detail": "evidence/result.md",
                    "committed_at": "2026-08-27T00:00:02Z",
                    "journal_sequence": 6
                }
            }),
        ] {
            validate_cutex_event_message(&event).expect("valid additive integration event");
        }

        assert!(validate_cutex_event_message(&json!({
            "method": "cutex/agentBus/messageSent",
            "params": {
                "messageId": "message-1",
                "fromCutexSessionId": "cutex.director",
                "toCutexSessionId": "cutex.worker",
                "deliveryMode": "after_turn",
                "content": "follow-up",
                "sentAt": "2026-08-27T00:00:00Z",
                "modelVisible": true
            }
        }))
        .is_err());
        assert!(validate_cutex_event_message(&json!({
            "method": "cutex/taskService/assignmentCommitted",
            "params": {
                "schema": "cutex/task-service-receipt/v2",
                "action_id": "assign-1",
                "request_sha256": "b".repeat(64),
                "attempt_binding": null,
                "committed_at": "2026-08-27T00:00:00Z",
                "journal_sequence": 4,
                "result": { "kind": "task_revision", "body": {} }
            }
        }))
        .is_err());
    }

    #[test]
    fn task_receipt_versions_and_project_lineage_are_strict() {
        for event in [
            task_receipt_event(
                "cutex/taskService/assignmentCommitted",
                "cutex/task-service-receipt/v2",
                "assignment",
                None,
            ),
            task_receipt_event(
                "cutex/taskService/communicationRecorded",
                "cutex/task-service-receipt/v2",
                "send_attempt",
                None,
            ),
            task_receipt_event(
                "cutex/taskService/assignmentCommitted",
                "cutex/task-service-receipt/v3",
                "assignment",
                Some("cutex-stack-main"),
            ),
            task_receipt_event(
                "cutex/taskService/communicationRecorded",
                "cutex/task-service-receipt/v3",
                "send_attempt",
                Some("cutex-stack-main"),
            ),
        ] {
            validate_cutex_event_message(&event).expect("valid exact Task Service receipt");
        }

        let unscoped_v3 = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v3",
            "assignment",
            None,
        );
        assert!(validate_cutex_event_message(&unscoped_v3).is_err());

        let scoped_v2 = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v2",
            "assignment",
            Some("cutex-stack-main"),
        );
        assert!(validate_cutex_event_message(&scoped_v2).is_err());

        let mut mixed = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v3",
            "assignment",
            Some("cutex-stack-main"),
        );
        mixed["params"]["result"]["body"]["send_attempt"] =
            send_attempt_body(Some("other-project"));
        assert!(validate_cutex_event_message(&mixed).is_err());

        let mut wrong_schema = mixed.clone();
        wrong_schema["params"]["schema"] = json!("cutex/task-service-receipt/v9");
        assert!(validate_cutex_event_message(&wrong_schema).is_err());

        let mut unknown = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v3",
            "assignment",
            Some("cutex-stack-main"),
        );
        unknown["params"]["result"]["body"]["assignment"]["forged"] = json!(true);
        assert!(validate_cutex_event_message(&unknown).is_err());

        let mut malformed = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v3",
            "assignment",
            Some("cutex-stack-main"),
        );
        malformed["params"]["result"]["body"]["assignment"]
            .as_object_mut()
            .unwrap()
            .remove("state");
        assert!(validate_cutex_event_message(&malformed).is_err());

        let wrong_kind = task_receipt_event(
            "cutex/taskService/assignmentCommitted",
            "cutex/task-service-receipt/v3",
            "send_attempt",
            Some("cutex-stack-main"),
        );
        assert!(validate_cutex_event_message(&wrong_kind).is_err());

        let transition = json!({
            "method": "cutex/taskService/assignmentTransitionCommitted",
            "params": {
                "schema": "cutex/task-service-assignment-transition/v1",
                "project_id": "cutex-stack-main",
                "transition": "review_ready",
                "action_id": "submit-r23",
                "assignment_id": "assignment-r23",
                "task_id": "CUTEX-R23",
                "assignee_cutex_session_id": "cutex.worker",
                "attempt_number": 1,
                "closure_reason": null,
                "detail": "result.md",
                "committed_at": "2026-08-30T00:00:00Z",
                "journal_sequence": 9
            }
        });
        validate_cutex_event_message(&transition).expect("project-scoped transition");
        let mut unknown_transition = transition;
        unknown_transition["params"]["forged"] = json!(true);
        assert!(validate_cutex_event_message(&unknown_transition).is_err());
    }

    #[test]
    fn runtime_response_accepts_one_launch_profile_receipt() {
        validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "runtime-online-1",
            "cutexSessionId": "session-1",
            "cutex": {
                "method": "cutex/runtime/online",
                "result": {
                    "runtimeGeneration": 3,
                    "status": "online",
                    "launchProfile": {
                        "requested": "profile-id",
                        "selected": "beta",
                        "effective": "beta",
                        "source": "one_launch_override",
                        "applicationScope": "runtime_and_tui",
                        "persisted": false
                    }
                }
            }
        }))
        .expect("valid one-launch receipt");

        assert!(validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "runtime-online-2",
            "cutexSessionId": "session-1",
            "cutex": {
                "method": "cutex/runtime/online",
                "result": {
                    "runtimeGeneration": 3,
                    "status": "online",
                    "launchProfile": {
                        "requested": "beta",
                        "selected": "beta",
                        "effective": "beta",
                        "source": "one_launch_override",
                        "applicationScope": "runtime",
                        "persisted": true
                    }
                }
            }
        }))
        .is_err());
    }

    #[test]
    fn runtime_response_accepts_legacy_tui_unknown_profile_provenance() {
        validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "runtime-online-legacy-1",
            "cutexSessionId": "session-1",
            "cutex": {
                "method": "cutex/runtime/online",
                "result": {
                    "runtimeGeneration": 3,
                    "status": "online",
                    "launchProfile": {
                        "requested": "legacy-profile",
                        "selected": "legacy-profile",
                        "effective": "legacy-profile",
                        "source": "unknown",
                        "applicationScope": "tui",
                        "persisted": false
                    }
                }
            }
        }))
        .expect("valid legacy TUI receipt");
    }

    #[test]
    fn archive_contract_accepts_exact_optional_reason_and_lifecycle_receipts() {
        for params in [
            json!({
                "expectedRevision": 4,
                "expectedRuntimeGeneration": 7
            }),
            json!({
                "expectedRevision": 4,
                "expectedRuntimeGeneration": 7,
                "reason": "owner_cleanup"
            }),
        ] {
            validate_cutex_request(&json!({
                "requestId": "retire-1",
                "method": "cutex/session/retire",
                "params": params
            }))
            .expect("valid retire request");
        }

        for params in [
            json!({
                "expectedRevision": 4,
                "expectedRuntimeGeneration": 7,
                "reason": false
            }),
            json!({
                "expectedRevision": 4,
                "expectedRuntimeGeneration": 7,
                "unknown": true
            }),
        ] {
            assert!(validate_cutex_request(&json!({
                "requestId": "retire-invalid",
                "method": "cutex/session/retire",
                "params": params
            }))
            .is_err());
        }

        validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "retire-1",
            "cutexSessionId": "cutex-session-1",
            "cutex": {
                "method": "cutex/session/retire",
                "result": {
                    "cutexSessionId": "cutex-session-1",
                    "revision": 5,
                    "lifecycle": "retired",
                    "runtimeGeneration": 7,
                    "status": "offline",
                    "retiredAt": "2026-08-10T00:01:00Z"
                }
            }
        }))
        .expect("valid retire response");
        validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "restore-1",
            "cutexSessionId": "cutex-session-1",
            "cutex": {
                "method": "cutex/session/restore",
                "result": {
                    "cutexSessionId": "cutex-session-1",
                    "revision": 6,
                    "lifecycle": "active",
                    "runtimeGeneration": 7,
                    "status": "offline",
                    "retiredAt": null
                }
            }
        }))
        .expect("valid restore response");
        assert!(validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "retire-state-alias",
            "cutexSessionId": "cutex-session-1",
            "cutex": {
                "method": "cutex/session/retire",
                "result": {
                    "cutexSessionId": "cutex-session-1",
                    "revision": 5,
                    "state": "retired",
                    "runtimeGeneration": 7,
                    "status": "offline",
                    "retiredAt": "2026-08-10T00:01:00Z"
                }
            }
        }))
        .is_err());
        assert!(validate_cutex_response(&json!({
            "contractVersion": 2,
            "requestId": "restore-wrong-lifecycle",
            "cutexSessionId": "cutex-session-1",
            "cutex": {
                "method": "cutex/session/restore",
                "result": {
                    "cutexSessionId": "cutex-session-1",
                    "revision": 6,
                    "lifecycle": "retired",
                    "runtimeGeneration": 7,
                    "status": "offline",
                    "retiredAt": "2026-08-10T00:01:00Z"
                }
            }
        }))
        .is_err());

        validate_cutex_event_message(&json!({
            "method": "cutex/session/retired",
            "params": {
                "cutexSessionId": "cutex-session-1",
                "sessionRevision": 5,
                "runtimeGeneration": 7,
                "retiredAt": "2026-08-10T00:01:00Z",
                "reason": "owner_cleanup"
            }
        }))
        .expect("retire event with reason");
        validate_cutex_event_message(&json!({
            "method": "cutex/session/retired",
            "params": {
                "cutexSessionId": "cutex-session-1",
                "sessionRevision": 5,
                "runtimeGeneration": 7,
                "retiredAt": "2026-08-10T00:01:00Z"
            }
        }))
        .expect("retire event without reason");
        validate_cutex_event_message(&json!({
            "method": "cutex/session/restored",
            "params": {
                "cutexSessionId": "cutex-session-1",
                "sessionRevision": 6,
                "runtimeGeneration": 7,
                "restoredAt": "2026-08-10T00:02:00Z"
            }
        }))
        .expect("restore event");
    }
}
