# S5a Task Service outbound MCP mapping

Base: `9b8b1f3919e63ecf4bbbe8ea0137855d047c8d83`. Native adapter reference:
K `ef53716b7673ad14c24b977667334e31e66110d8`, `core/src/tools/handlers/task_service{,_director,_protocol,_receipt,_spec,_director_spec}.rs`.
Provider and its stores remain authoritative. No inbound notification/wake support is added.

| Tool / operations | Required semantic fields beyond operation/action_id | Existing route |
| --- | --- | --- |
| cutex_task_service: start, resume, decline, abort_attempt | assignment_id | worker-prepare then actions |
| report_status | assignment_id, summary (4096 UTF-8 bytes); optional evidence_sha256 | same |
| block | assignment_id, summary (2048 UTF-8 bytes) | same |
| submit | assignment_id, result_sha256, result_reference (4096 bytes) | same |
| cutex_task_service_director: create_revision | project_id, workflow_id, task_id, task_revision, opaque_contract, completion_policy | director-action |
| assign | project_id, task_id, task_revision, assignment_id, assignee_cutex_session_id, summary | same |
| create_and_assign | union of create/assign fields | same; provider-owned two-step continuation, not atomic |
| query | selector: all, task + task_id, or assignment + assignment_id | same |
| accept_result, request_changes, fail_result, cancel | assignment_id; optional decision_reference | same |

Routes are `/api/task/v2/…`. Creation optionally specifies the intended
completion_authority_cutex_session_id; this is a provider-validated target, never
caller authority. Exact opaque_contract UTF-8 bytes are hashed locally. Native
compatibility digest input is not advertised; if accepted it must exactly match.
Positive task_revision is required for create/assign, up to JSON-safe integer.
Unknown fields/operations and inappropriate per-operation fields reject.

Worker preparation obtains mechanical revision/attempt tokens privately; none
are model arguments or results. Exact replay, one mechanical-conflict/uncertain
retry and sanitized native receipt envelopes are retained. Director uncertain
responses require exact action replay; continuation is not inferred success.

Every HTTP step carries S2 Core thread/current-generation fence plus the normal
runtime Bus credential. The existing Bus validates the fence before resolving
Task principals; the facade never holds a Human/root credential. Existing native
requests without MCP headers retain their contract. Query/send remain unchanged.

The adapter also recognizes the existing provider's `create_revision`-labelled
failed first substep of `create_and_assign`, retaining conflict/uncertainty under
the requested composite operation. A successful create-only receipt is never
accepted as composite success. This narrow correction differs from K's native
receipt checker, which otherwise supplies the unchanged semantic validation and
redaction logic. Native handlers in K are not changed.

Current provider behavior: request_changes reopens the same attempt, not a new
start; abort_attempt/fail_result end an attempt and leave its assignment
retry_pending. Decline/cancel/accept close assignments. A failed delivery in
create_and_assign can leave both revision and assignment persisted: retain the
provider's continuation and retry the exact action, never create a substitute.

S4 lifecycle,
seconds-truncated PID-time risk, two baseline socket failures, S2/s46 incidents,
candidate-writers-only storage and all release exclusions remain unchanged.
