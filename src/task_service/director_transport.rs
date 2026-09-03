//! Semantic authenticated Director transport. Mechanical provider values are
//! deliberately absent from these wire types.

use serde::{Deserialize, Serialize};

use crate::role_revision::{CutexSessionId, Sha256, TaskId, TaskRevision};

use super::{ActionId, AssignmentId, ClosureReason, WorkflowId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DirectorActionSchema {
    #[serde(rename = "cutex/task-service-director-action/v1")]
    V1,
    #[serde(rename = "cutex/task-service-director-action/v2")]
    V2,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DirectorReceiptSchema {
    #[serde(rename = "cutex/task-service-director-receipt/v1")]
    V1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCompletionPolicy {
    DirectorAcceptance,
    ReleaseReview,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRevisionSemanticRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub workflow_id: WorkflowId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub contract_sha256: Sha256,
    pub opaque_contract: String,
    pub completion_policy: SemanticCompletionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_authority_cutex_session_id: Option<CutexSessionId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignSemanticRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub assignee_cutex_session_id: CutexSessionId,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentDecisionRequest {
    pub assignment_id: AssignmentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_reference: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectorQuerySelector {
    All {},
    Task { task_id: TaskId },
    Assignment { assignment_id: AssignmentId },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectorSemanticOperation {
    CreateRevision(CreateRevisionSemanticRequest),
    Assign(AssignSemanticRequest),
    CreateAndAssign {
        create_revision: CreateRevisionSemanticRequest,
        assign: AssignSemanticRequest,
    },
    Query {
        selector: DirectorQuerySelector,
    },
    AcceptResult(AssignmentDecisionRequest),
    RequestChanges(AssignmentDecisionRequest),
    FailResult(AssignmentDecisionRequest),
    Cancel(AssignmentDecisionRequest),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectorActionRequest {
    pub schema: DirectorActionSchema,
    pub action_id: ActionId,
    #[serde(flatten)]
    pub action: DirectorSemanticOperation,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorActionStatus {
    Committed,
    CurrentState,
    Conflict,
    NoWrite,
    ResponseUncertain,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorContinuation {
    pub phase: String,
    pub retry_action_id: ActionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorTaskView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub workflow_id: WorkflowId,
    pub contract_sha256: Sha256,
    pub completion_policy: SemanticCompletionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_authority_cutex_session_id: Option<CutexSessionId>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorAttemptView {
    pub attempt_number: u64,
    pub phase: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output: Option<crate::observability::SafeOutputProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_call: Option<crate::observability::SafeToolCallProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_submitted_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorAssignmentView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    pub assignment_id: AssignmentId,
    pub task_id: TaskId,
    pub task_revision: TaskRevision,
    pub assignee_cutex_session_id: CutexSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_display_name: Option<String>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_attempt_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_reason: Option<ClosureReason>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    pub attempts: Vec<DirectorAttemptView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorActionReceipt {
    pub schema: DirectorReceiptSchema,
    pub action_id: ActionId,
    pub operation: String,
    pub status: DirectorActionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<crate::agent_management::ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_revision: Option<TaskRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment_id: Option<AssignmentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure_reason: Option<ClosureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<DirectorContinuation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<DirectorTaskView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<DirectorAssignmentView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_semantic_request_has_no_mechanical_authority_fields() {
        let request: DirectorActionRequest = serde_json::from_value(json!({
            "schema": "cutex/task-service-director-action/v1",
            "action_id": "director-create-1",
            "operation": "create_revision",
            "workflow_id": "workflow-1",
            "task_id": "task-1",
            "task_revision": 1,
            "contract_sha256": "a".repeat(64),
            "opaque_contract": "# exact contract",
            "completion_policy": "director_acceptance"
        }))
        .expect("semantic request");
        assert!(matches!(
            request.action,
            DirectorSemanticOperation::CreateRevision(_)
        ));

        for forbidden in [
            "expected_workflow_revision",
            "attempt_token",
            "runtime_agent_id",
            "seat_id",
        ] {
            let mut value = serde_json::to_value(&request).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .insert(forbidden.to_string(), json!(1));
            assert!(serde_json::from_value::<DirectorActionRequest>(value).is_err());
        }
    }

    #[test]
    fn query_selector_and_create_and_assign_are_flat_and_strict() {
        let query: DirectorActionRequest = serde_json::from_value(json!({
            "schema": "cutex/task-service-director-action/v1",
            "action_id": "query-1",
            "operation": "query",
            "selector": { "kind": "assignment", "assignment_id": "assignment-1" }
        }))
        .unwrap();
        assert!(matches!(
            query.action,
            DirectorSemanticOperation::Query { .. }
        ));

        assert!(serde_json::from_value::<DirectorActionRequest>(json!({
            "schema": "cutex/task-service-director-action/v1",
            "action_id": "query-1",
            "operation": "query",
            "selector": { "kind": "all", "provider_revision": 7 }
        }))
        .is_err());
    }
}
