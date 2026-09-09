use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

const WORKER_PREPARE_RESPONSE_SCHEMA: &str = "cutex/task-service-worker-prepare-response/v2";
const WORKER_PROVIDER_SCHEMA: &str = "cutex/task-service-worker-provider/v2";
const ACTION_RESPONSE_SCHEMA: &str = "cutex/task-service-action-response/v2";
const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptMechanicalContext {
    attempt_number: u64,
    attempt_token: String,
    expected_attempt_revision: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerMechanicalContext {
    expected_assignment_revision: u64,
    attempt: Option<AttemptMechanicalContext>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkerProviderEnvelope {
    schema: String,
    action: Value,
    context: WorkerMechanicalContext,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerPrepareResponse {
    schema: String,
    outcome: WorkerPrepareOutcome,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
enum WorkerPrepareOutcome {
    Prepared(WorkerProviderEnvelope),
    Committed(Value),
    NoWrite(ProviderNoWrite),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderNoWrite {
    code: String,
    detail: String,
}

pub(super) enum PrepareResult {
    Prepared(Vec<u8>),
    Committed(Value),
    NoWrite(String),
    Invalid,
}

pub(super) fn parse_prepare_response(
    action: &Value,
    attempt_required: bool,
    response: Value,
) -> PrepareResult {
    let Ok(response) = serde_json::from_value::<WorkerPrepareResponse>(response) else {
        return PrepareResult::Invalid;
    };
    if response.schema != WORKER_PREPARE_RESPONSE_SCHEMA {
        return PrepareResult::Invalid;
    }
    match response.outcome {
        WorkerPrepareOutcome::NoWrite(no_write) => {
            let _ = no_write.detail;
            PrepareResult::NoWrite(safe_code(&no_write.code).to_string())
        }
        WorkerPrepareOutcome::Committed(receipt) => PrepareResult::Committed(receipt),
        WorkerPrepareOutcome::Prepared(envelope) => {
            if envelope.schema != WORKER_PROVIDER_SCHEMA
                || envelope.action != *action
                || !valid_context(&envelope.context, attempt_required)
            {
                return PrepareResult::Invalid;
            }
            match serde_json::to_vec(&envelope) {
                Ok(body) => PrepareResult::Prepared(body),
                Err(_) => PrepareResult::Invalid,
            }
        }
    }
}

pub(super) fn is_mechanical_conflict(action_id: &str, response: &Value) -> bool {
    if response.get("schema").and_then(Value::as_str) != Some(ACTION_RESPONSE_SCHEMA)
        || response.get("action_id").and_then(Value::as_str) != Some(action_id)
    {
        return false;
    }
    let Some(body) = response
        .get("outcome")
        .filter(|outcome| outcome.get("kind").and_then(Value::as_str) == Some("no_write"))
        .and_then(|outcome| outcome.get("body"))
    else {
        return false;
    };
    if body.get("code").and_then(Value::as_str) != Some("conflict") {
        return false;
    }
    matches!(
        body.get("detail").and_then(Value::as_str),
        Some(
            "task service provider error: Conflict(\"assignment_revision_conflict\")"
                | "task service provider error: Conflict(\"attempt_revision_conflict\")"
                | "task service provider error: Conflict(\"attempt_handle_conflict\")"
        )
    )
}

fn valid_context(context: &WorkerMechanicalContext, attempt_required: bool) -> bool {
    valid_revision(context.expected_assignment_revision)
        && context.attempt.is_some() == attempt_required
        && context.attempt.as_ref().is_none_or(|attempt| {
            attempt.attempt_number > 0
                && attempt.attempt_number <= MAX_JSON_SAFE_INTEGER
                && valid_id(&attempt.attempt_token)
                && valid_revision(attempt.expected_attempt_revision)
        })
}

fn valid_revision(revision: u64) -> bool {
    revision > 0 && revision <= MAX_JSON_SAFE_INTEGER
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
}

fn safe_code(code: &str) -> &str {
    if !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        code
    } else {
        "provider_rejected"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn rejects_foreign_action_and_missing_or_invalid_mechanical_context() {
        let action = json!({"operation":"submit","body":{"action_id":"same"}});
        let valid = json!({"schema":WORKER_PREPARE_RESPONSE_SCHEMA,"outcome":{"kind":"prepared","body":{"schema":WORKER_PROVIDER_SCHEMA,"action":action,"context":{"expected_assignment_revision":2,"attempt":{"attempt_number":1,"attempt_token":"private-token","expected_attempt_revision":3}}}}});
        assert!(matches!(
            parse_prepare_response(&action, true, valid.clone()),
            PrepareResult::Prepared(_)
        ));
        for (key, value) in [
            ("attempt", Value::Null),
            ("expected_assignment_revision", json!(0)),
        ] {
            let mut bad = valid.clone();
            bad["outcome"]["body"]["context"][key] = value;
            assert!(matches!(
                parse_prepare_response(&action, true, bad),
                PrepareResult::Invalid
            ));
        }
        let mut bad = valid.clone();
        bad["outcome"]["body"]["action"]["body"]["action_id"] = json!("foreign");
        assert!(matches!(
            parse_prepare_response(&action, true, bad),
            PrepareResult::Invalid
        ));
        assert!(matches!(
            parse_prepare_response(&action, false, valid),
            PrepareResult::Invalid
        ));
    }
}
