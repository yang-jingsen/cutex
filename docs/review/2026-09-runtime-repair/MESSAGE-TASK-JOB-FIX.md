# Agent messages, Task contracts and Job launch compatibility

This follow-up fixes the failures reported by cesc-director-r2 on 2026-09-14.

## Task contract handoff

Normal typed/human Worker creation writes a Ready runtime receipt, not the older
Activation/Bootstrap migration receipts. Native Task ingress now accepts this
lineage when its durable owner, native ID and native home match. The original
creation timestamp survives restart and next-launch bundle selection. Provider
outbox identity, assignment target and contract hash checks remain in place.
The notification already carried the full contract; it was blocked before ingress.

New MCP tool `cutex_task_service_read` accepts only `assignment_id` and returns
the exact immutable task revision, full `opaque_contract`, and its SHA-256. It
uses authenticated `POST /api/task/v2/query` with operation `read_contract` and
an assignment ID body. Only that assignment's assignee can read through this
surface. It works before start, creates no attempt, and exposes no attempt token
or prepared action. The MCP facade must be rebuilt along with Cutex.

## Agent message display

A successful send receipt returns a runtime `to` as well as durable
`to_cutex_session_id` and `to_name`. The old TUI compared only `to` to the caller's
argument and fell back to raw MCP output. It now recognizes those target forms,
uses the returned name, and shows `Sent message to NAME · MODE` with a short
preview. Sent means accepted into the queue; it does not assert peer consumption.

New received Agent envelopes freeze inert `cutex.agent-message.v1` display facts
(sender ID, local formal name, delivery mode) under external-input v2. The model
text and authenticated source remain unchanged. TUI displays a pink bullet and
`Received message from NAME · MODE`, with a bounded dim preview and full details
retained in the transcript. Unknown or mismatched facts use the generic fallback.
Already frozen historical envelopes are not rewritten. Existing UI processes
need to reconnect using the new native CLI to render the new presentation.

## Job Service

The originating `sandboxCwd` is a policy anchor, not a requirement that every job
execute in the thread root. The native sandbox CLI itself returns to that anchor,
so simply deleting the equality check would execute in the wrong directory.
The Job runner now enters the unchanged sandbox and uses positional shell
arguments to change to the requested cwd and exec the original argv. Managed
permissions still restrict filesystem/network access; disabled remains disabled.

Daemon startup supports repeated `--allow-launcher PATH`, retaining exact path
and digest matching. The authenticated `capabilities` method enables Cutex to
check launcher compatibility before a new runtime review. Installation updates
replace a saved Job descriptor for new launches only when endpoint and both
credential paths are unchanged. Existing action receipts are not rewritten.
Keep launchers used by live older agents in the allowlist during upgrade.

## Validation

- Cutex library: 886 passed, four existing ignored tests; added exact-contract,
  no-write/wrong-assignee, MCP long-body, Ready-without-migration, restart/package
  update lineage and display-envelope tests. Launcher capability wire test also
  passes, including binary credential hex encoding and changed digest rejection.
- Job Service: full suite with `--features test-harness` passes (7 library,
  4 completion, 11 core, 1 real MCP tests). The first invocation without that
  feature lacked the owner-death helper executable; rerun with the feature passed.
- Job child-cwd process test preserves spaces, quotes and Unicode argv, and still
  denies writes under the original read-only policy. Updated real native MCP test
  uses a local mock model, an additional allowlisted launcher and child worktree;
  asserts actual cwd in stdout. No production model or experiment is used.
- cute-codex message tests: 13 passed, with reviewed send/receive snapshots.
  Full TUI suite: 4091 passed, 37 failed, one IDE IPC timeout, six skipped.
  Failures are outside the changed message files (including pre-existing version
  snapshot differences, v0.0.0 versus v0.153.4). Failure artifacts are retained
  privately; unrelated expected snapshots were not changed. Scoped Clippy fix
  completed with existing warnings, and `just fmt` completed.

Deployment and current source commit references are recorded separately with the
release acceptance results. Production agents/tasks are preserved; cesc's exec
experiments are not moved to Job Service.
