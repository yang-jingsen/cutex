# Task completion notification project routing

Assignment: task-notification-project-director-routing-r1. Exact base:
6125d76cc9bc62184e101cfaf319bbfadae3ffac, tree
cf444f478b58be263c4f78b9aa5a420655983a89. Accepted durable import remains included.

## Cause and decision

Task revisions persist a CompletionPolicy with an authority_seat_id, not a frozen
Director session ID. Completion outbox records likewise persist project_id and
target_seat_id. Those representations were correct. The stale R12 value came
from fresh query and notification adapters joining cutex-director against global
occupancies instead of project_director_occupancies. Coordinator activity and
watchdog escalation/presentation used the same incorrect global lookup.

Use one strict task_seat_occupancy resolver. For a project-bearing cutex-director
target, require that project's materialized, nonzero-epoch occupancy, usable
Active/Archived state and no active transfer fence. Missing/pending/removed/fenced
seats yield no recipient; never fall back to the global Director. Explicit other
seat policies, including release_review, keep their existing seat semantics.
Truly project-less records retain global resolution; V3 quarantine of legacy
unscoped outbox records is unchanged. No name/group/profile/cwd inference.

Semantic task creation validates the requested/default completion authority
against this resolver. Fresh Director and Human Owner queries project the current
durable occupant; unavailable scoped authority is null (or an unavailable read
when the seat store itself cannot be read). No task/seat migration or historical
receipt rewrite is needed. Coordinator activity and watchdog use the same rule.

## Dispatch and recovery

Existing seat-store locking now fences recipient resolution and enqueue against
rotation. The order is seat observation lock, provider read/fact update, then
Agent Bus state; no lock is held across network delivery. Pending completion and
watchdog retries remove stale in-memory queue/dedupe copies and reuse the same
notification/external-message ID. Delivered outbox facts are skipped unchanged.
Queued completion notifications retain bounded retry scheduling until Delivered;
availability probes include project seats. Completion polling revalidates the
current recipient; watchdog owner escalation polling resolves its task policy,
while FirstStale assignee reminders remain independent of seat authority.

Explicit release notifications and Worker follow-ups retain their distinct
targets. Notification correlation IDs, provider action receipts, fact CAS,
terminal transitions and persisted audit history are unchanged. Existing bus
queue/audit records identify actual runtime delivery; no old receipt is relabeled.

## Evidence and boundaries

Focused temporary-state production-handler tests cover two projects with distinct
Directors and stale global predecessor, ReviewReady/TerminalClosure, pending
rotation/fence/retry while predecessor remains online, delivered no-resend and
deduplicated replay, fresh creation/query, wrong-project/missing authority,
explicit release policy, legacy project-less dispatch and V3 quarantine.
Watchdog production dispatch is tested with an isolated running observation
fixture; reminder identity stays unchanged while escalation follows rotation.
The preexisting scoped quarantine fixture now materializes its Director explicitly
instead of relying on the rejected global fallback.

Affected suites: Task Service 88; delivery adapter 9; Agent Bus server 44;
Management v2 server 28; seat 7. Final exact checks/logs and immutable lineage are
in the Worker ROUTING-RESULT packet. No full unrelated suite, live notifications,
production seat/task/roster changes, installed build, deployment, push or restart.
No cute-codex protocol change was required. Checks establish isolated source
behavior, not live acceptance. Messages already handed to a runtime before the
rotation boundary cannot be recalled; committed Delivered history is never resent.

Suggested shared TODO: project task notification routing now follows scoped
Director seats across creation/query/completion/watchdog/retry; stale global seat
is no longer a recipient fallback. Focused validation complete; Director acceptance
and Human-controlled deployment/restart remain separate.
