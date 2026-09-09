//! Native semantic translation/receipt validation from fixed K ef53716.
use super::Transport;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
const DIRECTOR_ACTION_SCHEMA: &str = "cutex/task-service-director-action/v2";
const DIRECTOR_RECEIPT_SCHEMA: &str = "cutex/task-service-director-receipt/v1";
const MODEL_RECEIPT_SCHEMA: &str = "cutex/task-service-director-tool-receipt/v1";
const MAX_ID_BYTES: usize = 256;
const MAX_SUMMARY_BYTES: usize = 4_096;
const MAX_CONTRACT_BYTES: usize = 131_072;
const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DirectorOperation {
    CreateRevision,
    Assign,
    CreateAndAssign,
    Query,
    AcceptResult,
    RequestChanges,
    FailResult,
    Cancel,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CompletionPolicy {
    DirectorAcceptance,
    ReleaseReview,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum QuerySelector {
    All,
    Task { task_id: String },
    Assignment { assignment_id: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectorArgs {
    operation: DirectorOperation,
    action_id: String,
    project_id: Option<String>,
    workflow_id: Option<String>,
    task_id: Option<String>,
    task_revision: Option<u64>,
    /// Accepted only for compatibility with older callers. The native
    /// integration always derives and forwards the authoritative value.
    contract_sha256: Option<String>,
    opaque_contract: Option<String>,
    completion_policy: Option<CompletionPolicy>,
    completion_authority_cutex_session_id: Option<String>,
    assignment_id: Option<String>,
    assignee_cutex_session_id: Option<String>,
    summary: Option<String>,
    selector: Option<QuerySelector>,
    decision_reference: Option<String>,
}

fn semantic_request(args: &DirectorArgs) -> Result<Value, &'static str> {
    validate_id(&args.action_id)?;
    let mut body = Map::from_iter([
        ("schema".to_string(), json!(DIRECTOR_ACTION_SCHEMA)),
        ("action_id".to_string(), json!(args.action_id)),
        ("operation".to_string(), json!(args.operation)),
    ]);
    match args.operation {
        DirectorOperation::CreateRevision => {
            reject_assign_query_decision(args)?;
            insert_project_id(args, &mut body)?;
            insert_create_fields(args, &mut body)?;
        }
        DirectorOperation::Assign => {
            reject_create_query_decision(args)?;
            insert_project_id(args, &mut body)?;
            insert_assign_fields(args, &mut body)?;
        }
        DirectorOperation::CreateAndAssign => {
            if args.selector.is_some() || args.decision_reference.is_some() {
                return Err("invalid_semantic_payload");
            }
            let mut create_revision = Map::new();
            insert_project_id(args, &mut create_revision)?;
            insert_create_fields(args, &mut create_revision)?;
            let mut assign = Map::new();
            insert_project_id(args, &mut assign)?;
            insert_assign_fields(args, &mut assign)?;
            body.insert(
                "create_revision".to_string(),
                Value::Object(create_revision),
            );
            body.insert("assign".to_string(), Value::Object(assign));
        }
        DirectorOperation::Query => {
            reject_create_assign_decision(args)?;
            let selector = args.selector.as_ref().ok_or("invalid_semantic_payload")?;
            validate_selector(selector)?;
            body.insert("selector".to_string(), json!(selector));
        }
        DirectorOperation::AcceptResult
        | DirectorOperation::RequestChanges
        | DirectorOperation::FailResult
        | DirectorOperation::Cancel => {
            reject_create_assign_query(args)?;
            let assignment_id = required_id(args.assignment_id.as_deref())?;
            body.insert("assignment_id".to_string(), json!(assignment_id));
            if let Some(reference) = args.decision_reference.as_deref() {
                validate_text(reference, MAX_SUMMARY_BYTES)?;
                body.insert("decision_reference".to_string(), json!(reference));
            }
        }
    }
    Ok(Value::Object(body))
}

fn insert_project_id(
    args: &DirectorArgs,
    body: &mut Map<String, Value>,
) -> Result<(), &'static str> {
    let project_id = required_id(args.project_id.as_deref())?;
    body.insert("project_id".to_string(), json!(project_id));
    Ok(())
}

fn insert_create_fields(
    args: &DirectorArgs,
    body: &mut Map<String, Value>,
) -> Result<(), &'static str> {
    let workflow_id = required_id(args.workflow_id.as_deref())?;
    let task_id = required_id(args.task_id.as_deref())?;
    let task_revision = args
        .task_revision
        .filter(|revision| *revision > 0 && *revision <= MAX_JSON_SAFE_INTEGER)
        .ok_or("invalid_semantic_payload")?;
    let opaque_contract = args
        .opaque_contract
        .as_deref()
        .ok_or("invalid_semantic_payload")?;
    validate_text(opaque_contract, MAX_CONTRACT_BYTES)?;
    let contract_sha256 = format!("{:x}", Sha256::digest(opaque_contract.as_bytes()));
    if let Some(submitted_sha256) = args.contract_sha256.as_deref() {
        validate_sha256(submitted_sha256)?;
        if submitted_sha256 != contract_sha256 {
            return Err("contract_sha256_mismatch");
        }
    }
    let completion_policy = args.completion_policy.ok_or("invalid_semantic_payload")?;
    body.extend([
        ("workflow_id".to_string(), json!(workflow_id)),
        ("task_id".to_string(), json!(task_id)),
        ("task_revision".to_string(), json!(task_revision)),
        ("contract_sha256".to_string(), json!(contract_sha256)),
        ("opaque_contract".to_string(), json!(opaque_contract)),
        ("completion_policy".to_string(), json!(completion_policy)),
    ]);
    if let Some(session_id) = args.completion_authority_cutex_session_id.as_deref() {
        validate_id(session_id)?;
        body.insert(
            "completion_authority_cutex_session_id".to_string(),
            json!(session_id),
        );
    }
    Ok(())
}

fn insert_assign_fields(
    args: &DirectorArgs,
    body: &mut Map<String, Value>,
) -> Result<(), &'static str> {
    let assignment_id = required_id(args.assignment_id.as_deref())?;
    let task_id = required_id(args.task_id.as_deref())?;
    let task_revision = args
        .task_revision
        .filter(|revision| *revision > 0 && *revision <= MAX_JSON_SAFE_INTEGER)
        .ok_or("invalid_semantic_payload")?;
    let assignee = required_id(args.assignee_cutex_session_id.as_deref())?;
    let summary = args.summary.as_deref().ok_or("invalid_semantic_payload")?;
    validate_text(summary, MAX_SUMMARY_BYTES)?;
    body.extend([
        ("assignment_id".to_string(), json!(assignment_id)),
        ("task_id".to_string(), json!(task_id)),
        ("task_revision".to_string(), json!(task_revision)),
        ("assignee_cutex_session_id".to_string(), json!(assignee)),
        ("summary".to_string(), json!(summary)),
    ]);
    Ok(())
}

fn reject_assign_query_decision(args: &DirectorArgs) -> Result<(), &'static str> {
    if args.assignment_id.is_some()
        || args.assignee_cutex_session_id.is_some()
        || args.summary.is_some()
        || args.selector.is_some()
        || args.decision_reference.is_some()
    {
        Err("invalid_semantic_payload")
    } else {
        Ok(())
    }
}

fn reject_create_query_decision(args: &DirectorArgs) -> Result<(), &'static str> {
    if create_only_field_present(args)
        || args.selector.is_some()
        || args.decision_reference.is_some()
    {
        Err("invalid_semantic_payload")
    } else {
        Ok(())
    }
}

fn reject_create_assign_decision(args: &DirectorArgs) -> Result<(), &'static str> {
    if args.project_id.is_some()
        || create_only_field_present(args)
        || args.task_id.is_some()
        || args.task_revision.is_some()
        || args.assignment_id.is_some()
        || args.assignee_cutex_session_id.is_some()
        || args.summary.is_some()
        || args.decision_reference.is_some()
    {
        Err("invalid_semantic_payload")
    } else {
        Ok(())
    }
}

fn reject_create_assign_query(args: &DirectorArgs) -> Result<(), &'static str> {
    if args.project_id.is_some()
        || create_only_field_present(args)
        || args.task_id.is_some()
        || args.task_revision.is_some()
        || args.assignee_cutex_session_id.is_some()
        || args.summary.is_some()
        || args.selector.is_some()
    {
        Err("invalid_semantic_payload")
    } else {
        Ok(())
    }
}

fn create_only_field_present(args: &DirectorArgs) -> bool {
    args.workflow_id.is_some()
        || args.contract_sha256.is_some()
        || args.opaque_contract.is_some()
        || args.completion_policy.is_some()
        || args.completion_authority_cutex_session_id.is_some()
}

fn validate_selector(selector: &QuerySelector) -> Result<(), &'static str> {
    match selector {
        QuerySelector::All => Ok(()),
        QuerySelector::Task { task_id } => validate_id(task_id),
        QuerySelector::Assignment { assignment_id } => validate_id(assignment_id),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderReceipt {
    schema: String,
    action_id: String,
    operation: DirectorOperation,
    status: ReceiptStatus,
    project_id: Option<String>,
    task_id: Option<String>,
    task_revision: Option<u64>,
    assignment_id: Option<String>,
    attempt_number: Option<u64>,
    closure_reason: Option<String>,
    code: Option<String>,
    detail: Option<String>,
    continuation: Option<ProviderContinuation>,
    tasks: Option<Vec<SemanticTask>>,
    assignments: Option<Vec<SemanticAssignment>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptStatus {
    Committed,
    CurrentState,
    Conflict,
    NoWrite,
    ResponseUncertain,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderContinuation {
    phase: String,
    retry_action_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SemanticTask {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    task_id: String,
    task_revision: u64,
    workflow_id: String,
    contract_sha256: String,
    completion_policy: CompletionPolicy,
    completion_authority_cutex_session_id: Option<String>,
    created_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SemanticAssignment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    assignment_id: String,
    task_id: String,
    task_revision: u64,
    assignee_cutex_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    assignee_display_name: Option<String>,
    state: String,
    active_attempt_number: Option<u64>,
    closure_reason: Option<String>,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acknowledged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    closed_at: Option<String>,
    attempts: Vec<SemanticAttempt>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SemanticAttempt {
    attempt_number: u64,
    phase: String,
    started_at: String,
    updated_at: String,
    latest_status_summary: Option<String>,
    result_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result_submitted_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct ModelContinuation {
    phase: String,
    retry_action_id: String,
}

#[derive(Debug, Serialize)]
struct ModelReceipt {
    schema: &'static str,
    status: ReceiptStatus,
    operation: String,
    action_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    closure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    continuation: Option<ModelContinuation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tasks: Option<Vec<SemanticTask>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assignments: Option<Vec<SemanticAssignment>>,
}

impl ModelReceipt {
    fn invalid_arguments() -> Self {
        Self {
            schema: MODEL_RECEIPT_SCHEMA,
            status: ReceiptStatus::NoWrite,
            operation: "invalid".to_string(),
            action_id: "unknown".to_string(),
            project_id: None,
            task_id: None,
            task_revision: None,
            assignment_id: None,
            attempt_number: None,
            closure_reason: None,
            code: Some("invalid_arguments".to_string()),
            continuation: None,
            tasks: None,
            assignments: None,
        }
    }

    fn no_write(args: &DirectorArgs, code: &str) -> Self {
        Self::empty(args, ReceiptStatus::NoWrite, Some(safe_code(code)))
    }

    fn response_uncertain(args: &DirectorArgs) -> Self {
        let mut receipt = Self::empty(
            args,
            ReceiptStatus::ResponseUncertain,
            Some("exact_retry_required"),
        );
        if matches!(args.operation, DirectorOperation::CreateAndAssign) {
            receipt.continuation = Some(ModelContinuation {
                phase: "outcome_unknown".to_string(),
                retry_action_id: args.action_id.clone(),
            });
        }
        receipt
    }

    fn empty(args: &DirectorArgs, status: ReceiptStatus, code: Option<&str>) -> Self {
        Self {
            schema: MODEL_RECEIPT_SCHEMA,
            status,
            operation: operation_name(args.operation).to_string(),
            action_id: args.action_id.clone(),
            project_id: args
                .project_id
                .clone()
                .filter(|project_id| validate_id(project_id).is_ok()),
            task_id: args.task_id.clone(),
            task_revision: args.task_revision,
            assignment_id: args.assignment_id.clone(),
            attempt_number: None,
            closure_reason: None,
            code: code.map(str::to_string),
            continuation: None,
            tasks: None,
            assignments: None,
        }
    }
}

fn sanitize_provider_receipt(args: &DirectorArgs, value: Value) -> ModelReceipt {
    let Ok(receipt) = serde_json::from_value::<ProviderReceipt>(value) else {
        return ModelReceipt::no_write(args, "invalid_provider_response");
    };
    let _ = receipt.detail;
    if receipt.schema != DIRECTOR_RECEIPT_SCHEMA
        || receipt.action_id != args.action_id
        || (receipt.operation != args.operation
            // The provider labels a failed first composite substep create_revision.
            // Keep its real conflict/uncertainty, not invalid_provider_response.
            && !(args.operation == DirectorOperation::CreateAndAssign
                && receipt.operation == DirectorOperation::CreateRevision
                && !matches!(receipt.status, ReceiptStatus::Committed | ReceiptStatus::CurrentState)))
        || !valid_optional_identity(receipt.project_id.as_deref())
        || !valid_optional_identity(receipt.task_id.as_deref())
        || !valid_optional_identity(receipt.assignment_id.as_deref())
        || receipt
            .task_revision
            .is_some_and(|revision| revision == 0 || revision > MAX_JSON_SAFE_INTEGER)
        || receipt
            .attempt_number
            .is_some_and(|attempt| attempt == 0 || attempt > MAX_JSON_SAFE_INTEGER)
        || receipt
            .closure_reason
            .as_deref()
            .is_some_and(|reason| !valid_closure_reason(reason))
        || receipt
            .code
            .as_deref()
            .is_some_and(|code| safe_code(code) != code)
        || !valid_semantic_results(&receipt.tasks, &receipt.assignments)
    {
        return ModelReceipt::no_write(args, "invalid_provider_response");
    }
    let continuation = match receipt.continuation {
        Some(continuation)
            if continuation.phase == "create_revision_committed"
                && continuation.retry_action_id == args.action_id =>
        {
            Some(ModelContinuation {
                phase: continuation.phase,
                retry_action_id: continuation.retry_action_id,
            })
        }
        Some(_) => return ModelReceipt::no_write(args, "invalid_provider_response"),
        None => None,
    };
    ModelReceipt {
        schema: MODEL_RECEIPT_SCHEMA,
        status: receipt.status,
        operation: operation_name(args.operation).to_string(),
        action_id: args.action_id.clone(),
        project_id: receipt.project_id,
        task_id: receipt.task_id,
        task_revision: receipt.task_revision,
        assignment_id: receipt.assignment_id,
        attempt_number: receipt.attempt_number,
        closure_reason: receipt.closure_reason,
        code: receipt.code,
        continuation,
        tasks: receipt.tasks,
        assignments: receipt.assignments,
    }
}

fn valid_semantic_results(
    tasks: &Option<Vec<SemanticTask>>,
    assignments: &Option<Vec<SemanticAssignment>>,
) -> bool {
    tasks.as_ref().is_none_or(|tasks| {
        tasks.len() <= 1_000
            && tasks.iter().all(|task| {
                valid_optional_identity(task.project_id.as_deref())
                    && validate_id(&task.task_id).is_ok()
                    && validate_id(&task.workflow_id).is_ok()
                    && task.task_revision > 0
                    && task.task_revision <= MAX_JSON_SAFE_INTEGER
                    && validate_sha256(&task.contract_sha256).is_ok()
                    && valid_timestamp(&task.created_at)
                    && task
                        .completion_authority_cutex_session_id
                        .as_deref()
                        .is_none_or(|id| validate_id(id).is_ok())
            })
    }) && assignments.as_ref().is_none_or(|assignments| {
        assignments.len() <= 1_000
            && assignments.iter().all(|assignment| {
                valid_optional_identity(assignment.project_id.as_deref())
                    && validate_id(&assignment.assignment_id).is_ok()
                    && validate_id(&assignment.task_id).is_ok()
                    && validate_id(&assignment.assignee_cutex_session_id).is_ok()
                    && assignment
                        .assignee_display_name
                        .as_deref()
                        .is_none_or(|value| validate_text(value, MAX_SUMMARY_BYTES).is_ok())
                    && assignment.task_revision > 0
                    && assignment.task_revision <= MAX_JSON_SAFE_INTEGER
                    && matches!(
                        assignment.state.as_str(),
                        "awaiting_ack" | "active" | "retry_pending" | "closed"
                    )
                    && assignment
                        .active_attempt_number
                        .is_none_or(|number| number > 0 && number <= MAX_JSON_SAFE_INTEGER)
                    && assignment
                        .closure_reason
                        .as_deref()
                        .is_none_or(valid_closure_reason)
                    && assignment.attempts.len() <= 1_000
                    && valid_timestamp(&assignment.created_at)
                    && assignment
                        .acknowledged_at
                        .as_deref()
                        .is_none_or(valid_timestamp)
                    && assignment.closed_at.as_deref().is_none_or(valid_timestamp)
                    && assignment.attempts.iter().all(valid_attempt)
            })
    })
}

fn valid_attempt(attempt: &SemanticAttempt) -> bool {
    attempt.attempt_number > 0
        && attempt.attempt_number <= MAX_JSON_SAFE_INTEGER
        && matches!(
            attempt.phase.as_str(),
            "running"
                | "blocked"
                | "review_ready"
                | "completed"
                | "failed"
                | "cancelled"
                | "aborted"
        )
        && valid_timestamp(&attempt.started_at)
        && valid_timestamp(&attempt.updated_at)
        && attempt
            .latest_status_summary
            .as_deref()
            .is_none_or(|value| validate_text(value, MAX_SUMMARY_BYTES).is_ok())
        && attempt
            .result_reference
            .as_deref()
            .is_none_or(|value| validate_text(value, MAX_SUMMARY_BYTES).is_ok())
        && attempt
            .result_submitted_at
            .as_deref()
            .is_none_or(valid_timestamp)
}

fn valid_timestamp(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_optional_identity(value: Option<&str>) -> bool {
    value.is_none_or(|value| validate_id(value).is_ok())
}

fn valid_closure_reason(value: &str) -> bool {
    matches!(
        value,
        "completed" | "failed" | "cancelled" | "declined" | "aborted"
    )
}

fn required_id(value: Option<&str>) -> Result<&str, &'static str> {
    let value = value.ok_or("invalid_semantic_payload")?;
    validate_id(value)?;
    Ok(value)
}

fn validate_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
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

fn validate_text(value: &str, max_bytes: usize) -> Result<(), &'static str> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        Err("invalid_semantic_payload")
    } else {
        Ok(())
    }
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

fn operation_name(operation: DirectorOperation) -> &'static str {
    match operation {
        DirectorOperation::CreateRevision => "create_revision",
        DirectorOperation::Assign => "assign",
        DirectorOperation::CreateAndAssign => "create_and_assign",
        DirectorOperation::Query => "query",
        DirectorOperation::AcceptResult => "accept_result",
        DirectorOperation::RequestChanges => "request_changes",
        DirectorOperation::FailResult => "fail_result",
        DirectorOperation::Cancel => "cancel",
    }
}

pub(super) fn invoke(args: Value, transport: &mut impl Transport) -> Value {
    let args: DirectorArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(_) => return json!(ModelReceipt::invalid_arguments()),
    };
    let request = match semantic_request(&args) {
        Ok(request) => request,
        Err(code) => return json!(ModelReceipt::no_write(&args, code)),
    };
    match transport.post("/api/task/v2/director-action", &request) {
        Ok(response) if response.get("http_status").is_some() => {
            json!(ModelReceipt::no_write(&args, "integration_rejected"))
        }
        Ok(response) => json!(sanitize_provider_receipt(&args, response)),
        Err(_) => json!(ModelReceipt::response_uncertain(&args)),
    }
}
