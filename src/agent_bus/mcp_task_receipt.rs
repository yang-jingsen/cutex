use serde::Serialize;
use serde_json::Value;

use super::TaskServiceArgs;

const ACTION_RESPONSE_SCHEMA: &str = "cutex/task-service-action-response/v2";
const PROVIDER_RECEIPT_SCHEMA_V2: &str = "cutex/task-service-receipt/v2";
const PROVIDER_RECEIPT_SCHEMA_V3: &str = "cutex/task-service-receipt/v3";
const MODEL_RECEIPT_SCHEMA: &str = "cutex/task-service-tool-receipt/v1";
const MAX_PROJECT_ID_BYTES: usize = 256;

#[derive(Debug, Eq, PartialEq, Serialize)]
pub(super) struct ModelReceipt {
    schema: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    closure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

impl ModelReceipt {
    pub(super) fn no_write(code: impl Into<String>) -> Self {
        Self {
            schema: MODEL_RECEIPT_SCHEMA,
            status: "no_write",
            action_id: None,
            assignment_id: None,
            assignment_state: None,
            attempt_number: None,
            attempt_phase: None,
            closure_reason: None,
            code: Some(code.into()),
        }
    }

    pub(super) fn with_status(mut self, status: &'static str) -> Self {
        self.status = status;
        self
    }

    pub(super) fn for_action(mut self, args: &TaskServiceArgs) -> Self {
        self.action_id = Some(args.action_id.clone());
        self.assignment_id = Some(args.assignment_id.clone());
        self
    }
}

pub(super) fn model_no_write(code: impl Into<String>) -> ModelReceipt {
    let code = code.into();
    let status = match code.as_str() {
        "conflict" | "attempt_changed" => "conflict",
        "illegal_state" | "action_no_longer_legal" => "current_state",
        _ => "no_write",
    };
    ModelReceipt::no_write(code).with_status(status)
}

pub(super) fn sanitize_provider_response(args: &TaskServiceArgs, response: Value) -> ModelReceipt {
    if response.get("schema").and_then(Value::as_str) != Some(ACTION_RESPONSE_SCHEMA)
        || response.get("action_id").and_then(Value::as_str) != Some(&args.action_id)
    {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    }
    let Some(outcome) = response.get("outcome") else {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    };
    match outcome.get("kind").and_then(Value::as_str) {
        Some("no_write") => sanitize_no_write(args, outcome.get("body")),
        Some("committed") => sanitize_committed(args, outcome.get("body")),
        _ => ModelReceipt::no_write("invalid_provider_response").for_action(args),
    }
}

pub(super) fn sanitize_provider_receipt(args: &TaskServiceArgs, receipt: Value) -> ModelReceipt {
    sanitize_committed(args, Some(&receipt))
}

fn sanitize_no_write(args: &TaskServiceArgs, body: Option<&Value>) -> ModelReceipt {
    let code = body
        .and_then(|body| body.get("code"))
        .and_then(Value::as_str)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        .unwrap_or("provider_rejected");
    model_no_write(code).for_action(args)
}

fn sanitize_committed(args: &TaskServiceArgs, receipt: Option<&Value>) -> ModelReceipt {
    let Some(receipt) = receipt else {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    };
    let receipt_schema = receipt.get("schema").and_then(Value::as_str);
    if !matches!(
        receipt_schema,
        Some(PROVIDER_RECEIPT_SCHEMA_V2 | PROVIDER_RECEIPT_SCHEMA_V3)
    ) || receipt.get("action_id").and_then(Value::as_str) != Some(&args.action_id)
    {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    }
    let Some(result) = receipt.get("result") else {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    };
    if receipt_schema == Some(PROVIDER_RECEIPT_SCHEMA_V3)
        && !valid_v3_project_scope(receipt, result)
    {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    }
    match result.get("kind").and_then(Value::as_str) {
        Some("attempt") => sanitize_attempt(args, result.get("body")),
        Some("assignment") => sanitize_assignment(args, result.get("body")),
        _ => ModelReceipt::no_write("invalid_provider_response").for_action(args),
    }
}

fn valid_v3_project_scope(receipt: &Value, result: &Value) -> bool {
    let project_id = match result.get("kind").and_then(Value::as_str) {
        Some("attempt") => result
            .get("body")
            .and_then(|body| body.get("project_id"))
            .and_then(Value::as_str),
        Some("assignment") => result
            .get("body")
            .and_then(|body| body.get("assignment"))
            .and_then(|assignment| assignment.get("project_id"))
            .and_then(Value::as_str),
        _ => None,
    };
    project_id.is_some_and(|project_id| {
        valid_project_id(project_id) && project_ids_are_consistent(receipt, project_id)
    })
}

fn project_ids_are_consistent(value: &Value, expected: &str) -> bool {
    match value {
        Value::Object(object) => object.iter().all(|(key, value)| {
            if key == "project_id" {
                value.as_str() == Some(expected)
            } else {
                project_ids_are_consistent(value, expected)
            }
        }),
        Value::Array(values) => values
            .iter()
            .all(|value| project_ids_are_consistent(value, expected)),
        _ => true,
    }
}

fn valid_project_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROJECT_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

fn sanitize_attempt(args: &TaskServiceArgs, attempt: Option<&Value>) -> ModelReceipt {
    let Some(attempt) = attempt else {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    };
    let attempt_number = attempt.get("attempt_number").and_then(Value::as_u64);
    let phase = attempt
        .get("phase")
        .and_then(Value::as_str)
        .filter(|phase| {
            matches!(
                *phase,
                "running"
                    | "blocked"
                    | "review_ready"
                    | "completed"
                    | "failed"
                    | "cancelled"
                    | "aborted"
            )
        });
    if attempt.get("assignment_id").and_then(Value::as_str) != Some(&args.assignment_id)
        || attempt_number.is_none_or(|number| number == 0)
        || phase.is_none()
    {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    }
    ModelReceipt {
        schema: MODEL_RECEIPT_SCHEMA,
        status: "committed",
        action_id: Some(args.action_id.clone()),
        assignment_id: Some(args.assignment_id.clone()),
        assignment_state: None,
        attempt_number,
        attempt_phase: phase.map(str::to_string),
        closure_reason: None,
        code: None,
    }
}

fn sanitize_assignment(args: &TaskServiceArgs, body: Option<&Value>) -> ModelReceipt {
    let Some(assignment) = body.and_then(|body| body.get("assignment")) else {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    };
    let state = assignment
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| {
            matches!(
                *state,
                "awaiting_ack" | "active" | "retry_pending" | "closed"
            )
        });
    if assignment.get("assignment_id").and_then(Value::as_str) != Some(&args.assignment_id)
        || state.is_none()
    {
        return ModelReceipt::no_write("invalid_provider_response").for_action(args);
    }
    let closure_reason = assignment
        .get("closure")
        .and_then(|closure| closure.get("reason"))
        .and_then(Value::as_str)
        .filter(|reason| {
            matches!(
                *reason,
                "completed" | "failed" | "cancelled" | "declined" | "aborted"
            )
        })
        .map(str::to_string);
    ModelReceipt {
        schema: MODEL_RECEIPT_SCHEMA,
        status: "committed",
        action_id: Some(args.action_id.clone()),
        assignment_id: Some(args.assignment_id.clone()),
        assignment_state: state.map(str::to_string),
        attempt_number: None,
        attempt_phase: None,
        closure_reason,
        code: None,
    }
}
