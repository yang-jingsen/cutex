# S5a private Task Service MCP result

Intended use: **private integration candidate**, subject to Director acceptance;
not release, deployment, full tool migration, or autonomous stock Task execution.

Base: `9b8b1f3919e63ecf4bbbe8ea0137855d047c8d83`, tree
`d6b14c1a349b50a0c4237b959143e662d55b2189`. This report's commit is the immutable
descendant candidate; its exact commit/tree are in the Task Service submission.
Native reference K `ef53716b7673ad14c24b977667334e31e66110d8` was read via exact
Git objects only. Stock remains 0.153.4 / U
`3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`; S1 executable SHA-256
`56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da` unchanged.

## Implemented boundary

`cutex-mcp` adds `cutex_task_service` and `cutex_task_service_director` alongside
unchanged query_managed/send. See [operation/field contract](task-service-mcp-s5a.md).
Only semantic translation, native receipt redaction/validation, bounded transport
and the existing occurrence fence are added. Existing Task providers, receipts,
seats, notification delivery, stores and lifecycle behavior are unchanged.
Each prepare/execute request is fenced independently. The facade never receives
Human/root credentials. Missing-field diagnostics contain fixed field names only.

## Executed evidence

All paths below are under `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
No paid calls: dummy local Responses model; actual stock Core, configured MCP,
private Cutex Bus/Task/Management providers and persistent stores. Two genuine
native threads were explicitly adopted, activated with NEW reviewed manifests
pinning the rebuilt facade, then assigned private Director/Worker roles via
existing Human project/import APIs. The fixture drives Worker turns explicitly;
normal notifications remain queued/pending, not converted into Human messages.

| Criterion | Evidence / result |
| --- | --- |
| All 7 Worker and 8 Director operations | `s5a03/operations.json`: 36 real configured stock calls, 110 dummy Responses requests including bootstrap; every operation exercised |
| Create/assign/query, start/status/block/resume/submit, changes/repair/accept | Real stock/provider sequence; same attempt 1 retained through changes; original and repair result digests both persist |
| Decline/abort/fail/cancel | Separate real tasks; decline/cancel close, abort/fail produce aborted/failed attempts and retry_pending assignments |
| Replay / changed semantics | Worker exact start replay and changed-action conflict; existing Director semantic provider regression; composite conflict additionally tested on final artifact |
| Lost response | Actual prepared action sent over private TCP, response abandoned; stock exact replay leaves one attempt and one action receipt. Whether first send or replay wins is not assumed |
| Partial create_and_assign | Actual absent-recipient delivery uncertainty after persisted revision/assignment; exact continuation/action preserved on repeat. No atomicity claim |
| Role / assignment authority | Real stock Worker Director query denied; existing foreign Director-owned assignment denied; forged caller argument rejected without mutation |
| Occurrence / Core metadata | Real private Task route stale/foreign/missing header denial; actual facade missing Core metadata rejected with all network connections prohibited |
| Prior query/send compatibility | Real stock query and persisted exact-target send; Pending/required A4 is not delivered/A4 proof |
| Secret-free schema/results | Model-visible schemas/results checked; mechanical attempt/revision tokens remain internal; root token exists only in fixture setup |

`s5a03` intentionally remains a failed overall run: its final harness assertion
expected `message_id`, but the unchanged Bus returns `id`. Its 36 recorded tool
results and authoritative provider state were independently inspected, including
all terminal/repair and lost-response invariants; they are reused, not relabelled
as an overall PASS. `s5a04/PASS.json` is the six-call final-artifact supplement
before the narrow composite-substep receipt correction: 20 dummy requests,
readiness, create/assign/start/submit/accept, query/send, and four metadata denials.
The final correction's targeted supplement is recorded separately below.

`s5a05/PASS.json`: **7 real configured stock calls / 23 dummy requests**, including
the final composite changed-contract conflict, create/assign/start/submit/accept,
query/send, provider metadata denials and missing-Core rejection before network.
New private manifest facade SHA-256:
`98162123965e2a119f56a20c7e7d1bd892623400d47167c761d88d5670eb717c`.
No prior manifest or frozen artifact bytes were edited. Provider persistence
confirms one completed assignment and unchanged exact contract digest.

`s5a06/PASS.json`: **2 calls / 8 dummy requests**, same final facade digest.
`discovered-tools.json` captures actual stock Core tool-search output sent to the
fake model: both Task schemas, required assignment/revision descriptions, no
caller/session-authority, generation, attempt-token, expected-revision or bearer
fields. Actual missing assignment_id returns `missing_assignment_id`. This closes
the model-schema observer check without treating the ordinary top-level deferred
tool inventory (which does not contain these schemas) as sufficient evidence.

## Checks and failures retained

- Default-feature `cargo build --locked --bins`: passed (`s5a-build4.log`).
- Focused MCP lib tests and existing Director semantic provider test: passed;
  **10 + 1 = 11 tests**, respectively `s5a-tests4.log` and
  `s5a-provider-regression.log`. Earlier 7/9-test passes overlap and are not added
  to this total. No zero-selection acceptance.
- Complete scoped self-review includes new source/harness and client/server diff.
  Worker semantic translation/receipt checks match fixed K ignoring formatting;
  Director differs only for the documented failed composite-substep projection.
- `cargo fmt --all -- --check` and `git diff --check`: passed. No dependency or
  Cargo.lock churn. Default build compiles all affected paths; no separate
  redundant cargo check run. Existing unrelated warnings remain.
- Preserved setup failures: schema integer needed u64 (`s5a-tests1.log`);
  Read::by_ref disambiguation (`s5a-build1.log`); stock output-list decoding
  (`s5a01`); erroneous new-attempt assumption after request_changes (`s5a02`);
  wrong send receipt field (`s5a03`). These were harness/compile corrections,
  not provider policy changes or changed-name retries against an existing task.

## Reused / omitted / risks

Reused S4 readiness/registration-only and same-owner launch architecture, real
PTY/approval/sandbox and S2 binding evidence. No full S4 crash/PTY/sandbox rerun,
kernel build, real model, production provider, Windows/Android, live cgroup,
OAuth/account migration, inbound notification wake/A4/rendering or full
Management/rotation inventory claim. Pre-readiness/ambiguous mapping refusal
uses the unchanged S2/S4 shared fence; Task-specific live-race injection was not
repeated. Notification outcomes are ordinary provider state, not autonomous stock
completion. Legacy writers must still not access experimental stores.

S4 seconds-truncated PID-time risk and all fail-closed recovery exclusions remain.
The two Management socket-length failures remain classified by the prior exact
baseline comparison, not passing or newly rerun. Earlier S2 credential incident
and s46 denied-default-endpoint incident remain disclosed and unresolved; no
credential reread, remediation, rotation, or no-impact claim is made here.

Resource start 5.8 GiB retained / 454 GiB free; final 7.8 GiB / 452 GiB free
(well inside 20 GiB / 100 GiB floor).
All new caches, builds, private HOME/config/state and probes stayed in the owned
Mambo root. Owned children are stopped in finally; existing runtime is untouched.
Rollback before exposure: reject/revert candidate; keep prior manifests immutable.
Next-use recommendation: accept only for the private lightweight line. Inbound
integration and broader release/security/platform gates remain separate tasks.
