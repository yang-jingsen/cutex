//! Operation-specific help and diagnostics, shared by the published tool schema
//! and its preflight. Semantic validation and task authority remain in handlers.
use super::{DIRECTOR, TERMINAL, WORKER};
use serde_json::{json, Value};

struct Fields {
    required: &'static [&'static str],
    optional: &'static [&'static str],
}

fn fields(tool: &str, operation: &str) -> Option<Fields> {
    let (required, optional): (&[&str], &[&str]) = match (tool, operation) {
        (DIRECTOR, "create_revision") => (
            &[
                "project_id",
                "workflow_id",
                "task_id",
                "task_revision",
                "opaque_contract",
                "completion_policy",
            ],
            &["completion_authority_cutex_session_id"],
        ),
        (DIRECTOR, "assign") => (
            &[
                "project_id",
                "task_id",
                "task_revision",
                "assignment_id",
                "assignee_cutex_session_id",
                "summary",
            ],
            &[],
        ),
        (DIRECTOR, "create_and_assign") => (
            &[
                "project_id",
                "workflow_id",
                "task_id",
                "task_revision",
                "opaque_contract",
                "completion_policy",
                "assignment_id",
                "assignee_cutex_session_id",
                "summary",
            ],
            &["completion_authority_cutex_session_id"],
        ),
        (DIRECTOR, "query") => (&["selector"], &[]),
        (DIRECTOR, "accept_result" | "fail_result" | "cancel") => {
            (&["assignment_id"], &["decision_reference"])
        }
        (DIRECTOR | TERMINAL, "request_changes") => (&["assignment_id", "decision_reference"], &[]),
        (TERMINAL, "accept_result" | "fail_result") => {
            (&["assignment_id"], &["decision_reference"])
        }
        (WORKER, "report_status") => (&["assignment_id", "summary"], &["evidence_sha256"]),
        (WORKER, "block") => (&["assignment_id", "summary"], &[]),
        (WORKER, "submit") => (&["assignment_id", "result_sha256", "result_reference"], &[]),
        (WORKER, "start" | "resume" | "decline" | "abort_attempt") => (&["assignment_id"], &[]),
        _ => return None,
    };
    Some(Fields { required, optional })
}

pub(super) fn describe(tool: &str, properties: &mut Value) -> String {
    let operations: Vec<String> = properties["operation"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let mut help = String::from("Every call requires operation and action_id. For each operation, only the following additional fields are allowed; omit all others. ");
    for operation in &operations {
        let f = fields(tool, operation).unwrap();
        help.push_str(&format!(
            "{operation}: required [{}]; optional [{}]. ",
            f.required.join(", "),
            f.optional.join(", ")
        ));
    }
    for (key, property) in properties.as_object_mut().unwrap() {
        if key == "operation" || key == "action_id" {
            continue;
        }
        let required = operations
            .iter()
            .filter(|op| fields(tool, op).unwrap().required.contains(&key.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let optional = operations
            .iter()
            .filter(|op| fields(tool, op).unwrap().optional.contains(&key.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        let existing = property["description"].as_str().unwrap_or("");
        property["description"] = json!(format!(
            "Required for [{}]; optional for [{}]; forbidden for other operations. {existing}",
            required.join(", "),
            optional.join(", ")
        ));
    }
    help
}

pub(super) fn validate(tool: &str, args: &Value) -> Option<Value> {
    let operation = args["operation"].as_str()?;
    let f = fields(tool, operation)?;
    let object = args.as_object()?;
    let mut allowed = vec!["operation", "action_id"];
    allowed.extend_from_slice(f.required);
    allowed.extend_from_slice(f.optional);
    // Preserve the existing compatibility-only digest input without advertising
    // it as a field callers need to compute themselves.
    let compatibility_digest =
        tool == DIRECTOR && matches!(operation, "create_revision" | "create_and_assign");
    let known = [
        "project_id",
        "workflow_id",
        "task_id",
        "task_revision",
        "opaque_contract",
        "completion_policy",
        "completion_authority_cutex_session_id",
        "assignment_id",
        "assignee_cutex_session_id",
        "summary",
        "decision_reference",
        "selector",
        "contract_sha256",
        "evidence_sha256",
        "result_sha256",
        "result_reference",
    ];
    let forbidden = known
        .into_iter()
        .filter(|field| {
            object.get(*field).is_some_and(|v| !v.is_null())
                && !allowed.contains(field)
                && !(compatibility_digest && *field == "contract_sha256")
        })
        .collect::<Vec<_>>();
    if forbidden.is_empty() {
        return std::iter::once("action_id").chain(f.required.iter().copied())
            .find(|field| object.get(*field).is_none_or(Value::is_null))
            .map(|field| json!({"status":"no_write", "code":format!("missing_{field}"), "field":field, "operation":operation}));
    }
    Some(json!({
        "schema": match tool { DIRECTOR => "cutex/task-service-director-tool-receipt/v1", TERMINAL => "cutex/task-service-terminal-tool-receipt/v1", _ => "cutex/task-service-tool-receipt/v1" },
        "status":"no_write", "code":"field_not_allowed", "operation":operation,
        "fields":forbidden, "allowed_fields":allowed,
        "required_fields":std::iter::once("action_id").chain(f.required.iter().copied()).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
#[path = "mcp_task_arguments_tests.rs"]
mod tests;
