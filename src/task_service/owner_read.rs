//! Project-scoped, read-only Task projection for non-Agent Owner backends.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::OnceLock;

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::agent_management::ProjectId;
use crate::observability::{SafeOutputProjection, SafeToolCallProjection};
use crate::role_revision::{CutexSessionId, TaskId, TaskRevision};

use super::{
    AssignmentState, AttemptPhase, ClosureReason, DirectorAssignmentView, DirectorAttemptView,
    DirectorTaskView, SemanticCompletionPolicy, TaskServiceSnapshot,
};

pub const OWNER_TASK_DEFAULT_LIMIT: usize = 50;
pub const OWNER_TASK_MAX_LIMIT: usize = 100;
const OWNER_TASK_MAX_ASSIGNMENTS_PER_ITEM: usize = 64;
const OWNER_TASK_MAX_ATTEMPTS_PER_ASSIGNMENT: usize = 32;
const OWNER_TASK_MAX_PROJECTS_PER_PRINCIPAL: usize = 64;
const OWNER_TASK_CURSOR_TTL_MINUTES: i64 = 15;
static OWNER_TASK_CURSOR_SIGNING_SECRET: OnceLock<[u8; 32]> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct OwnerTaskReadToken(String);

impl OwnerTaskReadToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OwnerTaskReadToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerTaskReadToken([REDACTED])")
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerTaskReadCredential {
    pub principal_id: String,
    pub audience: String,
    pub token: OwnerTaskReadToken,
    pub project_ids: Vec<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl OwnerTaskReadCredential {
    pub fn authenticate(
        credentials: &[Self],
        authorization: Option<&str>,
        project_id: &ProjectId,
        now: DateTime<Utc>,
    ) -> Result<OwnerTaskReadPrincipal, OwnerTaskReadError> {
        let token = authorization
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| !value.is_empty())
            .ok_or(OwnerTaskReadError::Unauthorized)?;
        let mut matches = credentials.iter().filter(|credential| {
            constant_time_equal(credential.token.as_str().as_bytes(), token.as_bytes())
        });
        let credential = matches.next().ok_or(OwnerTaskReadError::Unauthorized)?;
        if matches.next().is_some() {
            return Err(OwnerTaskReadError::Unauthorized);
        }
        credential.validate()?;
        let expires_at = credential
            .expires_at
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
            .transpose()
            .map_err(|_| OwnerTaskReadError::Unauthorized)?
            .map(|value| value.with_timezone(&Utc));
        if expires_at.is_some_and(|expiry| expiry <= now) {
            return Err(OwnerTaskReadError::Unauthorized);
        }
        if !credential.project_ids.contains(project_id) {
            return Err(OwnerTaskReadError::ProjectDenied);
        }
        Ok(OwnerTaskReadPrincipal {
            principal_id: credential.principal_id.clone(),
            audience: credential.audience.clone(),
            project_ids: credential.project_ids.iter().cloned().collect(),
            expires_at,
        })
    }

    fn validate(&self) -> Result<(), OwnerTaskReadError> {
        if !bounded_label(&self.principal_id)
            || !bounded_label(&self.audience)
            || self.token.as_str().len() < 16
            || self.token.as_str().len() > 512
            || self.project_ids.is_empty()
            || self.project_ids.len() > OWNER_TASK_MAX_PROJECTS_PER_PRINCIPAL
            || self.project_ids.iter().collect::<BTreeSet<_>>().len() != self.project_ids.len()
        {
            return Err(OwnerTaskReadError::Unauthorized);
        }
        if let Some(expires_at) = self.expires_at.as_deref() {
            DateTime::parse_from_rfc3339(expires_at)
                .map_err(|_| OwnerTaskReadError::Unauthorized)?;
        }
        Ok(())
    }
}

fn bounded_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | ':')
        })
}

#[derive(Clone)]
pub struct OwnerTaskReadPrincipal {
    pub principal_id: String,
    pub audience: String,
    project_ids: BTreeSet<ProjectId>,
    expires_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for OwnerTaskReadPrincipal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OwnerTaskReadPrincipal")
            .field("principal_id", &self.principal_id)
            .field("audience", &self.audience)
            .field("project_ids", &self.project_ids)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnerTaskReadFilter {
    pub states: BTreeSet<String>,
    pub assignee: Option<CutexSessionId>,
    pub updated_since: Option<String>,
    pub task_id: Option<TaskId>,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OwnerTaskReadSchema {
    #[serde(rename = "cutex/owner-task-read/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerTaskReadItem {
    pub task: DirectorTaskView,
    pub updated_at: String,
    pub assignments: Vec<DirectorAssignmentView>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub assignments_truncated: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerTaskReadResponse {
    pub schema: OwnerTaskReadSchema,
    pub project_id: ProjectId,
    pub audience: String,
    pub items: Vec<OwnerTaskReadItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CursorPayload {
    schema: OwnerTaskReadSchema,
    principal_id: String,
    audience: String,
    project_id: ProjectId,
    filter_sha256: String,
    updated_at: String,
    task_id: TaskId,
    task_revision: TaskRevision,
    issued_at: String,
    expires_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnerTaskReadError {
    Unauthorized,
    ProjectDenied,
    InvalidQuery(&'static str),
    InvalidCursor,
    Unavailable,
}

pub fn project_owner_tasks(
    snapshot: &TaskServiceSnapshot,
    activity: &HashMap<String, crate::management::v2::activity::SessionActivityState>,
    principal: &OwnerTaskReadPrincipal,
    project_id: &ProjectId,
    filter: &OwnerTaskReadFilter,
    now: DateTime<Utc>,
) -> Result<OwnerTaskReadResponse, OwnerTaskReadError> {
    if !principal.project_ids.contains(project_id) {
        return Err(OwnerTaskReadError::ProjectDenied);
    }
    let limit = if filter.limit == 0 {
        OWNER_TASK_DEFAULT_LIMIT
    } else {
        filter.limit
    };
    if limit > OWNER_TASK_MAX_LIMIT || filter.states.len() > 8 {
        return Err(OwnerTaskReadError::InvalidQuery("query_bounds"));
    }
    let updated_since = filter
        .updated_since
        .as_deref()
        .map(DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| OwnerTaskReadError::InvalidQuery("updated_since"))?;
    let filter_sha256 = filter_digest(filter, limit)?;
    let after = filter
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, principal, project_id, &filter_sha256, now))
        .transpose()?;

    let mut items = Vec::new();
    for revisions in snapshot.task_revisions.values() {
        for task in revisions.values() {
            if task.project_id.as_ref() != Some(project_id)
                || filter
                    .task_id
                    .as_ref()
                    .is_some_and(|id| id != &task.task_id)
            {
                continue;
            }
            let mut matching_assignments = snapshot
                .assignments
                .values()
                .filter(|assignment| {
                    assignment.project_id.as_ref() == Some(project_id)
                        && assignment.task_id == task.task_id
                        && assignment.task_revision == task.task_revision
                        && filter
                            .assignee
                            .as_ref()
                            .is_none_or(|session| session == &assignment.assignee_cutex_session)
                        && (filter.states.is_empty()
                            || filter
                                .states
                                .contains(assignment_state_name(assignment.state)))
                })
                .collect::<Vec<_>>();
            if (!filter.states.is_empty() || filter.assignee.is_some())
                && matching_assignments.is_empty()
            {
                continue;
            }
            matching_assignments
                .sort_by(|left, right| left.assignment_id.cmp(&right.assignment_id));
            let assignments_truncated =
                matching_assignments.len() > OWNER_TASK_MAX_ASSIGNMENTS_PER_ITEM;
            matching_assignments.truncate(OWNER_TASK_MAX_ASSIGNMENTS_PER_ITEM);
            let updated_at = task_updated_at(snapshot, task, &matching_assignments);
            if updated_since.as_ref().is_some_and(|minimum| {
                DateTime::parse_from_rfc3339(&updated_at).is_ok_and(|value| value <= *minimum)
            }) {
                continue;
            }
            let authority = None;
            let task_view = DirectorTaskView {
                project_id: task.project_id.clone(),
                task_id: task.task_id.clone(),
                task_revision: task.task_revision,
                workflow_id: task.workflow_id.clone(),
                contract_sha256: task.contract_sha256.clone(),
                completion_policy: match task.completion_policy.kind {
                    super::CompletionPolicyKind::DirectorAcceptance => {
                        SemanticCompletionPolicy::DirectorAcceptance
                    }
                    super::CompletionPolicyKind::ReleaseReview => {
                        SemanticCompletionPolicy::ReleaseReview
                    }
                },
                completion_authority_cutex_session_id: authority,
                created_at: task.created_at.as_str().to_string(),
            };
            let assignments = matching_assignments
                .into_iter()
                .map(|assignment| assignment_view(snapshot, activity, project_id, assignment))
                .collect();
            items.push(OwnerTaskReadItem {
                task: task_view,
                updated_at,
                assignments,
                assignments_truncated,
            });
        }
    }
    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then(left.task.task_id.cmp(&right.task.task_id))
            .then(left.task.task_revision.cmp(&right.task.task_revision))
    });
    if let Some(cursor) = after {
        items.retain(|item| item_is_after_cursor(item, &cursor));
    }
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = has_more
        .then(|| items.last())
        .flatten()
        .map(|item| encode_cursor(principal, project_id, &filter_sha256, item, now))
        .transpose()?;
    Ok(OwnerTaskReadResponse {
        schema: OwnerTaskReadSchema::V1,
        project_id: project_id.clone(),
        audience: principal.audience.clone(),
        items,
        next_cursor,
    })
}

fn item_is_after_cursor(item: &OwnerTaskReadItem, cursor: &CursorPayload) -> bool {
    item.updated_at < cursor.updated_at
        || (item.updated_at == cursor.updated_at
            && (item.task.task_id > cursor.task_id
                || (item.task.task_id == cursor.task_id
                    && item.task.task_revision > cursor.task_revision)))
}

fn assignment_view(
    snapshot: &TaskServiceSnapshot,
    activity: &HashMap<String, crate::management::v2::activity::SessionActivityState>,
    project_id: &ProjectId,
    assignment: &super::Assignment,
) -> DirectorAssignmentView {
    let session_activity = activity.get(assignment.assignee_cutex_session.as_str());
    let attempts = snapshot
        .attempts
        .get(&assignment.assignment_id)
        .into_iter()
        .flat_map(|attempts| attempts.values())
        .filter(|attempt| attempt.project_id.as_ref() == Some(project_id))
        .take(OWNER_TASK_MAX_ATTEMPTS_PER_ASSIGNMENT)
        .map(|attempt| DirectorAttemptView {
            attempt_number: attempt.attempt_number.get(),
            phase: attempt_phase_name(attempt.phase).to_string(),
            started_at: attempt.started_at.as_str().to_string(),
            updated_at: attempt.updated_at.as_str().to_string(),
            latest_status_summary: attempt
                .status_receipts
                .last()
                .and_then(|status| crate::observability::sanitize_visible_output(&status.summary)),
            latest_status_at: attempt
                .status_receipts
                .last()
                .map(|status| status.recorded_at.as_str().to_string()),
            last_output: exact_output(
                session_activity,
                project_id,
                assignment,
                attempt.attempt_number.get(),
            ),
            last_tool_call: exact_tool(
                session_activity,
                project_id,
                assignment,
                attempt.attempt_number.get(),
            ),
            result_reference: attempt.result_receipts.last().and_then(|result| {
                crate::observability::sanitize_visible_output(&result.result_reference)
            }),
            result_submitted_at: attempt
                .result_receipts
                .last()
                .map(|result| result.submitted_at.as_str().to_string()),
        })
        .collect();
    let display = crate::app_server::participants::ParticipantMetadataResolver::resolve(
        &crate::app_server::participants::RegistryParticipantMetadataResolver,
        assignment.assignee_cutex_session.as_str(),
    )
    .display_name;
    DirectorAssignmentView {
        project_id: assignment.project_id.clone(),
        assignment_id: assignment.assignment_id.clone(),
        task_id: assignment.task_id.clone(),
        task_revision: assignment.task_revision,
        assignee_cutex_session_id: assignment.assignee_cutex_session.clone(),
        assignee_display_name: display,
        state: assignment_state_name(assignment.state).to_string(),
        active_attempt_number: assignment.active_attempt.map(|number| number.get()),
        closure_reason: assignment.closure.as_ref().map(|closure| closure.reason),
        created_at: assignment.created_at.as_str().to_string(),
        acknowledged_at: assignment
            .acknowledged_at
            .as_ref()
            .map(|value| value.as_str().to_string()),
        closed_at: assignment
            .closure
            .as_ref()
            .map(|value| value.closed_at.as_str().to_string()),
        attempts,
    }
}

fn exact_output(
    state: Option<&crate::management::v2::activity::SessionActivityState>,
    project_id: &ProjectId,
    assignment: &super::Assignment,
    attempt: u64,
) -> Option<SafeOutputProjection> {
    state
        .and_then(|state| state.last_output.as_ref())
        .filter(|output| {
            output.association.project_id.as_ref() == Some(project_id)
                && output.association.cutex_session_id == assignment.assignee_cutex_session.as_str()
                && output
                    .association
                    .matches_task(assignment.assignment_id.as_str(), attempt)
        })
        .cloned()
}

fn exact_tool(
    state: Option<&crate::management::v2::activity::SessionActivityState>,
    project_id: &ProjectId,
    assignment: &super::Assignment,
    attempt: u64,
) -> Option<SafeToolCallProjection> {
    state
        .and_then(|state| state.last_tool_call.as_ref())
        .filter(|tool| {
            tool.association.project_id.as_ref() == Some(project_id)
                && tool.association.cutex_session_id == assignment.assignee_cutex_session.as_str()
                && tool
                    .association
                    .matches_task(assignment.assignment_id.as_str(), attempt)
        })
        .cloned()
}

fn task_updated_at(
    snapshot: &TaskServiceSnapshot,
    task: &super::TaskRevisionRecord,
    assignments: &[&super::Assignment],
) -> String {
    let mut latest = task.created_at.as_str();
    for assignment in assignments {
        latest = latest.max(assignment.created_at.as_str());
        if let Some(value) = assignment.acknowledged_at.as_ref() {
            latest = latest.max(value.as_str());
        }
        if let Some(value) = assignment.closure.as_ref() {
            latest = latest.max(value.closed_at.as_str());
        }
        if let Some(attempts) = snapshot.attempts.get(&assignment.assignment_id) {
            for attempt in attempts.values() {
                latest = latest.max(attempt.updated_at.as_str());
            }
        }
    }
    latest.to_string()
}

fn filter_digest(filter: &OwnerTaskReadFilter, limit: usize) -> Result<String, OwnerTaskReadError> {
    let bytes = serde_json::to_vec(&(
        &filter.states,
        filter.assignee.as_ref().map(CutexSessionId::as_str),
        &filter.updated_since,
        filter.task_id.as_ref().map(TaskId::as_str),
        limit,
        "updated_at_desc_task_id_asc_revision_asc",
    ))
    .map_err(|_| OwnerTaskReadError::InvalidQuery("filter"))?;
    Ok(format!("{:x}", Sha256Hasher::digest(bytes)))
}

fn encode_cursor(
    principal: &OwnerTaskReadPrincipal,
    project_id: &ProjectId,
    filter_sha256: &str,
    item: &OwnerTaskReadItem,
    now: DateTime<Utc>,
) -> Result<String, OwnerTaskReadError> {
    let expiry = principal
        .expires_at
        .unwrap_or_else(|| now + Duration::minutes(OWNER_TASK_CURSOR_TTL_MINUTES))
        .min(now + Duration::minutes(OWNER_TASK_CURSOR_TTL_MINUTES));
    let payload = CursorPayload {
        schema: OwnerTaskReadSchema::V1,
        principal_id: principal.principal_id.clone(),
        audience: principal.audience.clone(),
        project_id: project_id.clone(),
        filter_sha256: filter_sha256.to_string(),
        updated_at: item.updated_at.clone(),
        task_id: item.task.task_id.clone(),
        task_revision: item.task.task_revision,
        issued_at: now.to_rfc3339(),
        expires_at: expiry.to_rfc3339(),
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| OwnerTaskReadError::Unavailable)?;
    let signature = cursor_signature(server_cursor_signing_secret(), &bytes);
    Ok(format!(
        "{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        signature
    ))
}

fn decode_cursor(
    cursor: &str,
    principal: &OwnerTaskReadPrincipal,
    project_id: &ProjectId,
    filter_sha256: &str,
    now: DateTime<Utc>,
) -> Result<CursorPayload, OwnerTaskReadError> {
    if cursor.len() > 4096 {
        return Err(OwnerTaskReadError::InvalidCursor);
    }
    let (payload, signature) = cursor
        .split_once('.')
        .ok_or(OwnerTaskReadError::InvalidCursor)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| OwnerTaskReadError::InvalidCursor)?;
    let expected_signature = cursor_signature(server_cursor_signing_secret(), &bytes);
    if !constant_time_equal(expected_signature.as_bytes(), signature.as_bytes()) {
        return Err(OwnerTaskReadError::InvalidCursor);
    }
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| OwnerTaskReadError::InvalidCursor)?;
    let expiry = DateTime::parse_from_rfc3339(&payload.expires_at)
        .map_err(|_| OwnerTaskReadError::InvalidCursor)?
        .with_timezone(&Utc);
    let issued_at = DateTime::parse_from_rfc3339(&payload.issued_at)
        .map_err(|_| OwnerTaskReadError::InvalidCursor)?
        .with_timezone(&Utc);
    if payload.schema != OwnerTaskReadSchema::V1
        || payload.principal_id != principal.principal_id
        || payload.audience != principal.audience
        || &payload.project_id != project_id
        || payload.filter_sha256 != filter_sha256
        || issued_at > now
        || expiry <= now
        || expiry > now + Duration::minutes(OWNER_TASK_CURSOR_TTL_MINUTES)
        || expiry - issued_at > Duration::minutes(OWNER_TASK_CURSOR_TTL_MINUTES)
        || expiry <= issued_at
        || principal
            .expires_at
            .is_some_and(|principal_expiry| expiry > principal_expiry)
    {
        return Err(OwnerTaskReadError::InvalidCursor);
    }
    Ok(payload)
}

fn server_cursor_signing_secret() -> &'static [u8; 32] {
    OWNER_TASK_CURSOR_SIGNING_SECRET.get_or_init(|| {
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        let mut secret = [0_u8; 32];
        secret[..16].copy_from_slice(first.as_bytes());
        secret[16..].copy_from_slice(second.as_bytes());
        secret
    })
}

fn cursor_signature(key: &[u8], payload: &[u8]) -> String {
    const BLOCK_BYTES: usize = 64;
    let mut normalized = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        normalized[..32].copy_from_slice(&Sha256Hasher::digest(key));
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36_u8; BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_pad[index] ^= normalized[index];
        outer_pad[index] ^= normalized[index];
    }
    let mut inner = Sha256Hasher::new();
    inner.update(inner_pad);
    inner.update(b"cutex-owner-task-cursor-v1");
    inner.update(payload);
    let mut outer = Sha256Hasher::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    format!("{:x}", outer.finalize())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn assignment_state_name(state: AssignmentState) -> &'static str {
    match state {
        AssignmentState::AwaitingAck => "awaiting_ack",
        AssignmentState::Active => "active",
        AssignmentState::RetryPending => "retry_pending",
        AssignmentState::Closed => "closed",
    }
}

fn attempt_phase_name(phase: AttemptPhase) -> &'static str {
    match phase {
        AttemptPhase::Running => "running",
        AttemptPhase::Blocked => "blocked",
        AttemptPhase::ReviewReady => "review_ready",
        AttemptPhase::Completed => "completed",
        AttemptPhase::Failed => "failed",
        AttemptPhase::Cancelled => "cancelled",
        AttemptPhase::Aborted => "aborted",
    }
}

#[allow(dead_code)]
fn _closure_reason_is_wire_safe(reason: ClosureReason) -> ClosureReason {
    reason
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_revision::{Sha256, TaskRevision};
    use crate::task_service::*;

    fn project(value: &str) -> ProjectId {
        ProjectId::new(value).unwrap()
    }
    fn action(value: &str) -> ActionId {
        ActionId::new(value).unwrap()
    }
    fn sha(value: &str) -> Sha256 {
        crate::task_service::sha256_bytes(value.as_bytes())
    }

    fn credential(project_ids: Vec<ProjectId>) -> OwnerTaskReadCredential {
        OwnerTaskReadCredential {
            principal_id: "owner-reader".to_string(),
            audience: "host-a-backend".to_string(),
            token: OwnerTaskReadToken::new("owner-reader-token-0123456789"),
            project_ids,
            expires_at: Some("2099-01-01T00:00:00Z".to_string()),
        }
    }

    fn create_project_task(
        provider: &TaskServiceProvider,
        coordinator: &AuthenticatedPrincipal,
        project_id: &ProjectId,
        suffix: &str,
    ) {
        let contract = format!("contract-{suffix}");
        provider
            .create_project_revision(
                coordinator,
                &CreateProjectRevisionRequest {
                    schema: ProviderActionSchema::V3,
                    action_id: action(&format!("create-{suffix}")),
                    project_id: project_id.clone(),
                    workflow_id: WorkflowId::new(format!("workflow-{suffix}")).unwrap(),
                    task_id: TaskId::new(format!("task-{suffix}")).unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha(&contract),
                    opaque_contract: contract,
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
    }

    #[test]
    fn authorization_matrix_legacy_exclusion_and_bound_cursor_are_fail_closed() {
        let root = std::env::temp_dir().join(format!("cutex-owner-read-{}", uuid::Uuid::new_v4()));
        let provider = TaskServiceProvider::open(&root).unwrap();
        let coordinator = AuthenticatedPrincipal::seated_session(
            CutexSessionId::new("director-session").unwrap(),
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let alpha = project("alpha");
        let beta = project("beta");
        provider
            .create_revision(
                &coordinator,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: action("create-legacy"),
                    workflow_id: WorkflowId::new("workflow-legacy").unwrap(),
                    task_id: TaskId::new("task-legacy").unwrap(),
                    task_revision: TaskRevision::new(1).unwrap(),
                    contract_sha256: sha("legacy"),
                    opaque_contract: "legacy".to_string(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::DirectorAcceptance,
                        authority_seat_id: SeatId::new("director").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        for suffix in ["alpha-1", "alpha-2", "alpha-3"] {
            create_project_task(&provider, &coordinator, &alpha, suffix);
        }
        create_project_task(&provider, &coordinator, &beta, "beta-1");

        let credentials = vec![credential(vec![alpha.clone(), beta.clone()])];
        let now = DateTime::parse_from_rfc3339("2026-08-29T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let principal = OwnerTaskReadCredential::authenticate(
            &credentials,
            Some("Bearer owner-reader-token-0123456789"),
            &alpha,
            now,
        )
        .unwrap();
        assert!(matches!(
            OwnerTaskReadCredential::authenticate(
                &[credential(vec![alpha.clone()])],
                Some("Bearer owner-reader-token-0123456789"),
                &beta,
                now,
            ),
            Err(OwnerTaskReadError::ProjectDenied)
        ));
        assert!(matches!(
            OwnerTaskReadCredential::authenticate(&credentials, Some("Bearer wrong"), &alpha, now),
            Err(OwnerTaskReadError::Unauthorized)
        ));

        let snapshot = provider.query().unwrap();
        let started = std::time::Instant::now();
        let first = project_owner_tasks(
            &snapshot,
            &HashMap::new(),
            &principal,
            &alpha,
            &OwnerTaskReadFilter {
                limit: 1,
                ..Default::default()
            },
            now,
        )
        .unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(first.items.len(), 1);
        assert!(first
            .items
            .iter()
            .all(|item| item.task.project_id.as_ref() == Some(&alpha)));
        let encoded = serde_json::to_string(&first).unwrap();
        for forbidden in [
            "legacy",
            "opaque_contract",
            "attempt_token",
            "provider_revision",
            "journal_sha256",
        ] {
            assert!(!encoded.contains(forbidden), "leaked {forbidden}");
        }
        let cursor = first.next_cursor.clone().expect("next page");
        let (payload, _) = cursor.split_once('.').unwrap();
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap();
        let mut forged_payload: CursorPayload = serde_json::from_slice(&payload_bytes).unwrap();
        forged_payload.expires_at = (now + Duration::hours(1)).to_rfc3339();
        let forged_bytes = serde_json::to_vec(&forged_payload).unwrap();
        let bearer_forged = format!(
            "{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&forged_bytes),
            cursor_signature(b"owner-reader-token-0123456789", &forged_bytes)
        );
        let forged_filter = OwnerTaskReadFilter {
            limit: 1,
            cursor: Some(bearer_forged),
            ..Default::default()
        };
        assert_eq!(
            project_owner_tasks(
                &snapshot,
                &HashMap::new(),
                &principal,
                &alpha,
                &forged_filter,
                now,
            ),
            Err(OwnerTaskReadError::InvalidCursor)
        );
        let server_signed_overlong = format!(
            "{}.{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&forged_bytes),
            cursor_signature(server_cursor_signing_secret(), &forged_bytes)
        );
        assert_eq!(
            project_owner_tasks(
                &snapshot,
                &HashMap::new(),
                &principal,
                &alpha,
                &OwnerTaskReadFilter {
                    limit: 1,
                    cursor: Some(server_signed_overlong),
                    ..Default::default()
                },
                now,
            ),
            Err(OwnerTaskReadError::InvalidCursor)
        );
        let second = project_owner_tasks(
            &snapshot,
            &HashMap::new(),
            &principal,
            &alpha,
            &OwnerTaskReadFilter {
                limit: 1,
                cursor: Some(cursor.clone()),
                ..Default::default()
            },
            now,
        )
        .unwrap();
        assert_ne!(first.items[0].task.task_id, second.items[0].task.task_id);
        let mut tampered = cursor.clone();
        tampered.push('x');
        assert_eq!(
            project_owner_tasks(
                &snapshot,
                &HashMap::new(),
                &principal,
                &alpha,
                &OwnerTaskReadFilter {
                    limit: 1,
                    cursor: Some(tampered),
                    ..Default::default()
                },
                now,
            ),
            Err(OwnerTaskReadError::InvalidCursor)
        );
        let beta_principal = OwnerTaskReadCredential::authenticate(
            &credentials,
            Some("Bearer owner-reader-token-0123456789"),
            &beta,
            now,
        )
        .unwrap();
        assert_eq!(
            project_owner_tasks(
                &snapshot,
                &HashMap::new(),
                &beta_principal,
                &beta,
                &OwnerTaskReadFilter {
                    limit: 1,
                    cursor: Some(cursor),
                    ..Default::default()
                },
                now,
            ),
            Err(OwnerTaskReadError::InvalidCursor)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
