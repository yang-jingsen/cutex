//! Semantic adapter ported from fixed K ef53716; no provider authority lives here.
use super::Transport;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[path = "mcp_task_protocol.rs"]
mod protocol;
#[path = "mcp_task_receipt.rs"]
mod receipt;
use protocol::PrepareResult;
use receipt::{
    model_no_write, sanitize_provider_receipt, sanitize_provider_response, ModelReceipt,
};
const ACTION_SCHEMA: &str = "cutex/task-service-action/v2";
const MAX_SEMANTIC_TEXT_BYTES: usize = 4096;
const MAX_BLOCKER_SUMMARY_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WorkerOperation {
    Start,
    ReportStatus,
    Block,
    Resume,
    Submit,
    Decline,
    AbortAttempt,
}

impl WorkerOperation {
    fn attempt_required(self) -> bool {
        matches!(
            self,
            Self::ReportStatus | Self::Block | Self::Resume | Self::Submit | Self::AbortAttempt
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskServiceArgs {
    operation: WorkerOperation,
    assignment_id: String,
    action_id: String,
    summary: Option<String>,
    evidence_sha256: Option<String>,
    result_sha256: Option<String>,
    result_reference: Option<String>,
}

fn provider_request(args: &TaskServiceArgs) -> Result<Value, &'static str> {
    validate_id(&args.assignment_id)?;
    validate_id(&args.action_id)?;
    let mut body = serde_json::Map::from_iter([
        ("schema".to_string(), json!(ACTION_SCHEMA)),
        ("action_id".to_string(), json!(args.action_id)),
        ("assignment_id".to_string(), json!(args.assignment_id)),
    ]);
    match args.operation {
        WorkerOperation::ReportStatus => {
            if args.result_sha256.is_some() || args.result_reference.is_some() {
                return Err("invalid_semantic_payload");
            }
            let summary = args
                .summary
                .as_deref()
                .filter(|value| !value.trim().is_empty() && value.len() <= MAX_SEMANTIC_TEXT_BYTES)
                .ok_or("invalid_semantic_payload")?;
            body.insert("summary".to_string(), json!(summary));
            if let Some(evidence_sha256) = args.evidence_sha256.as_deref() {
                validate_sha256(evidence_sha256)?;
                body.insert("evidence_sha256".to_string(), json!(evidence_sha256));
            }
        }
        WorkerOperation::Submit => {
            if args.summary.is_some() || args.evidence_sha256.is_some() {
                return Err("invalid_semantic_payload");
            }
            let result_sha256 = args
                .result_sha256
                .as_deref()
                .ok_or("invalid_semantic_payload")?;
            validate_sha256(result_sha256)?;
            let result_reference = args
                .result_reference
                .as_deref()
                .filter(|value| !value.trim().is_empty() && value.len() <= MAX_SEMANTIC_TEXT_BYTES)
                .ok_or("invalid_semantic_payload")?;
            body.insert("result_sha256".to_string(), json!(result_sha256));
            body.insert("result_reference".to_string(), json!(result_reference));
        }
        WorkerOperation::Block => {
            if args.evidence_sha256.is_some()
                || args.result_sha256.is_some()
                || args.result_reference.is_some()
            {
                return Err("invalid_semantic_payload");
            }
            let summary = args
                .summary
                .as_deref()
                .filter(|value| {
                    !value.trim().is_empty() && value.len() <= MAX_BLOCKER_SUMMARY_BYTES
                })
                .ok_or("invalid_semantic_payload")?;
            body.insert("summary".to_string(), json!(summary));
        }
        WorkerOperation::Start
        | WorkerOperation::Resume
        | WorkerOperation::Decline
        | WorkerOperation::AbortAttempt => {
            if args.summary.is_some()
                || args.evidence_sha256.is_some()
                || args.result_sha256.is_some()
                || args.result_reference.is_some()
            {
                return Err("invalid_semantic_payload");
            }
        }
    }
    Ok(json!({ "operation": args.operation, "body": body }))
}

fn validate_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
    {
        return Err("invalid_identity");
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("invalid_semantic_payload");
    }
    Ok(())
}

pub(super) fn invoke(args: Value, transport: &mut impl Transport) -> Value {
    let args: TaskServiceArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(_) => return json!(ModelReceipt::no_write("invalid_arguments")),
    };
    let action = match provider_request(&args) {
        Ok(action) => action,
        Err(code) => return json!(ModelReceipt::no_write(code).for_action(&args)),
    };
    // As in K: reprepare once on a mechanical race or an uncertain action reply.
    for retry in 0..2 {
        let prepare = json!({"schema":"cutex/task-service-worker-prepare/v2","action":action});
        let response = match transport.post("/api/task/v2/worker-prepare", &prepare) {
            Ok(response) => response,
            Err(_) => {
                return json!(ModelReceipt::no_write("response_uncertain")
                    .with_status("response_uncertain")
                    .for_action(&args))
            }
        };
        if response.get("http_status").is_some() {
            return json!(ModelReceipt::no_write("integration_rejected").for_action(&args));
        }
        let envelope = match protocol::parse_prepare_response(
            &action,
            args.operation.attempt_required(),
            response,
        ) {
            PrepareResult::Prepared(body) => {
                serde_json::from_slice(&body).expect("serialized envelope")
            }
            PrepareResult::Committed(receipt) => {
                return json!(sanitize_provider_receipt(&args, receipt))
            }
            PrepareResult::NoWrite(code) => return json!(model_no_write(code).for_action(&args)),
            PrepareResult::Invalid => {
                return json!(ModelReceipt::no_write("invalid_provider_response").for_action(&args))
            }
        };
        match transport.post("/api/task/v2/actions", &envelope) {
            Ok(response) if response.get("http_status").is_some() => {
                return json!(ModelReceipt::no_write("integration_rejected").for_action(&args))
            }
            Ok(response)
                if retry == 0 && protocol::is_mechanical_conflict(&args.action_id, &response) =>
            {
                continue
            }
            Ok(response) => return json!(sanitize_provider_response(&args, response)),
            Err(_) if retry == 0 => continue,
            Err(_) => {
                return json!(ModelReceipt::no_write("response_uncertain")
                    .with_status("response_uncertain")
                    .for_action(&args))
            }
        }
    }
    unreachable!()
}
