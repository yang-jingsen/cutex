# Job display v3 — decision-ready contract (no implementation)

Recommendation: one frozen structured completion snapshot, two projections:
concise immutable model text and a typed native view. Do not parse prose, invent
timing, or create another Notice simply to restyle the existing inbound event.
This is proposed authority for three bounded implementation tasks, not a claim
that today's wire accepts the new fields.

Inputs: Cutex checkout clean at8d97917eec7a5acc292ce317a25d936053bf6bf5/tree
1f168d286ab79f943e1ccff3c02be834a1144f83 (docs/fixtures descendant of accepted
3fb0c14, compiled product2e7ff4a); Job clean f3bc9c8ee3d2d35773c18a9dc6303c6ce9ca0d3e/
b2b8c671a55bcc45236e916d8d589e90083e289d. Native read from exact Git objects
cc4a080df1df4433f6fd67fee9c1c4fa4a42baab, not its concurrent writer's worktree.
Only this document is writable. Human pv2 and all auth/runtime state untouched.

## Current facts that determine the design

- Job `src/model.rs:137` JobRecord has actionId in request, state, exitCode,
  terminalReason, created/updated seconds, stdout/stderr observed/retained/truncated.
  There are no execution start/end/duration fields. jobId remains query/read key;
  actionId is submit idempotency label, never a lookup alias.
- `src/process.rs:72–111`: runner is spawned behind a gate; sentinel/output drains
  are prepared before gate release. Thus successful spawn alone is not execution
  start. `src/service.rs:282–328`: watcher waits child, then sentinel and drains,
  then records updatedAt. updated-created includes setup/collection and cannot be
  execution duration. Output byte counters are observed snapshots, not a promise
  all output survived collection or retention.
- `src/store.rs:92–114`: restart maps LaunchPending→LaunchUnknown and Running→
  Interrupted; it does not prove the old process's exit time. `:173–205` freezes
  result digest/outbox via event `job-terminal:<jobId>:<revision>` and or_insert.
- Job `src/completion.rs:282–364` regenerates v1 request/autosummary on every send
  from a smaller outbox. It lacks action/exit/time/count facts. Never rebuild old
  pending payloads with a new default formatter.
- Cutex `src/agent_bus/model.rs:116` completion v1 denies unknown fields;
  `src/cli_app/agent_bus_server.rs:546–637` authenticates the dedicated principal,
  builds canonical content, compares event semantic identity, then persists.
  `src/app_server/bus_bridge_external.rs:402–433` separately produces model text:
  full jobId in Job, outputReference, and default summary. This is the duplication.
- Native exact `codex-rs/protocol/src/external_input.rs` Message has only
  id/source/type/delivery/text, explicit digest fields and deny_unknown_fields.
  `protocol/src/presentation.rs` has title/body/format/refs and independent receipt,
  not arbitrary structured business data. Existing lanes can carry text, but
  cannot currently expose structured Job facts outside model text without a
  small versioned extension. `tui/src/history_cell/presentation.rs` is already a
  separate view over immutable records. `cutex_mcp_receipt.rs:36` recognizes core/v1.

## Producer contract for review-r1

Add optional typed execution observation to JobRecord and a frozen completion
snapshot to NEW outbox entries. Minimum snapshot fields:

```text
factsVersion: 1
actionId: existing request.actionId
exitCode?: actual observed ExitStatus.code
terminalReason?: existing actual reason
execution?: {
  basis: "runner_release_to_wait_v1",
  startObservedAtEpochMillis?: u64,
  exitObservedAtEpochMillis?: u64,
  observedRunDurationMillis?: u64
}
stdout/stderr?: {observedBytes, retainedBytes, truncated}
```

jobId, jobRevision, terminalStatus and outputReference remain existing typed
envelope fields; do not repeat them as a name. No argv/env/auth/process identity
in display metadata. Bounds/type checks required; no floats/negative durations.

Clock semantics: capture SystemTime and Instant at gate release; publish start
only if release succeeds. Keep Instant in the owned live-process observation,
not serialized as a cross-process clock. Capture exit SystemTime and elapsed
Instant immediately when child.wait succeeds, BEFORE sentinel/drain joins.
Persist terminal state/final observed stream counters/outbox in the existing
single store save after collection. Display elapsed is the observed runner
interval (includes launcher startup and scheduling), not CPU time, kernel-exact
command exec lifetime, descendant-tree lifetime or output-drain duration.
Wall-clock reversal must not produce a negative/derived duration: monotonic
duration is independent; invalid wall-clock reads remain absent.

| Path | Timing/exit facts |
|---|---|
| Normal observed exit | start + exit observation + monotonic interval; actual code when available |
| Cancelled then wait succeeds | same observed interval; cancelled remains cancelled even if code0 |
| Failure before gate release | no execution interval; existing launch-failure reason |
| wait error / uncertain launch | no invented end/duration/code; preserve only proved start |
| service restart while running | preserved known start; end/duration unknown, Interrupted is observation loss, not proved exit |
| legacy terminal record | absent metadata; no created/updated subtraction or background backfill |

Keep execution clocks distinct from created/updated. Missing is unknown, not0.
Do not refactor stop/containment or attempt to adopt old processes in this task.

### Persistence, wire and hash decisions

Recommend Job store version2 once new fields are written (old code accepts only1;
old writers must reject, not silently drop optional data). New reader supports
legacy1 and missing fields without manufacturing them. Core JobRecord may retain
core/v1 with strictly additive optional fields if the public consumer tests
confirm compatibility; completion wire must explicitly advance to
`cutex.job_service.completion.v2` because existing Cutex denies unknown fields.
Reuse endpoint/credential boundary; retain v1 queries/receipts for old entries.

Each outbox chooses its wire version at first creation and freezes the complete
typed request, including facts and omission of the redundant default summary.
No rereading current Job/name/config to change retry bytes. Existing outbox with
absent version means exact legacy v1 builder and original resultSha256, never
automatic promotion. Existing delivered receipts remain untouched.

For NEW v2 result hashes, define a versioned canonical snapshot (legacy result
fields plus facts, excluding its own hash and mutable delivery counters); hash
fixed typed JSON serialization/domain `cutex:job-result:v2` followed by one NUL byte. Freeze the result
and request together in the existing transaction. Keep legacy digest algorithm
byte-for-byte. Version/order/null-vs-absent test vectors are required, not a new
generic canonicalization dependency. Same event+changed facts is conflict;
lost response resends/query-reconciles the original event, never a new event ID.
No grant/permission change; no weakening current trusted runtime subject check.

## Cutex matching adapter contract

Accept v1 and v2 separately through the existing dedicated Job principal; bound
new fields and cross-check duplicated identity/state if present. New v2 snapshot
must be included in canonical event semantic identity, not advisory unbound JSON.
Freeze model projection at canonical acceptance (content is already a persisted
slot), BEFORE first native submit; later occurrence binding supplies exact native
thread/generation without changing content. Preserve old v1 projection and all
already-frozen native envelopes/receipts. Do not run current formatter against
old pending records and silently change their identity.

Default v2 producer sends no synthetic summary; outputReference remains in the
structured facts/raw transcript but is omitted from model text unless an actual
operation requires it. No automatic reference fetch. Example new model body:

```text
Job completed. jobId: job_dcf0f1baf1d547fe8621d4a5c7c9b75c
Action: human-display-v3-job-1
Exit code: 0. Observed run: 3.2 s.
Use jobId for query/read_output; do not resubmit.
```

Example only:0/3.2 require real fields. Full ID appears once in generated default
body. Do not rewrite arbitrary user/service summary contents to enforce a string
count; custom external data is bounded/marked and may itself contain IDs. actionId
never substitutes jobId. Do not modify Agent responses. Failed/cancelled/unknown
states use corresponding truthful wording; Exited alone does not imply exit0.

Native envelope schema/projection-version changes must be frozen in canonical
intent before first send and rejected by incompatible old Bus writers. Prefer
reuse of existing Bus record/version gates, not a new queue. A4→Bus delivered→
Job ACK and presentation independence remain unchanged. Offline/unsupported
receiver leaves pending/actionable; no auto-launch, fallback model-only resend
under a changed digest, silent fact drop or implicit permission changes.

## Native consumer boundary (coordinate with light-core-r1)

Recommended minimal addition: optional bounded generic structured-view payload
on the existing ExternalInput lane, e.g. `{schema, data}`. Native core validates
generic size/shape and hashes/persists it under an explicit new envelope version/
capability; it does not acquire a Job state machine. Native model materialization
uses only `text`; structured view is never copied into provider input. A4 still
means context persistence, not rendering success. Old v1 digests/history stay exact.

Native owner confirms this boundary in
`artifacts/display-v3-data-view-r1/ASSESSMENT.md` (read in its task root).
The authoritative mechanical history record AND derived ThreadItem/timeline
projection must carry view separately: current FCO response_item exposes only
model source/type/text. A TUI-parser-only patch cannot achieve the separation.
Verify sampling, compaction and resume model input also exclude view. Exact
encoding, byte/depth/field bounds and capability name are the next protocol
task's decision, not silently chosen by this assessment.

TUI understands one versioned `cutex.job-completion.v1` view schema and verifies
its types/provenance. Natural title derives from those facts plus exact same-thread
action↔Job mapping, never regex/prose/title/cwd guesses. Unknown schema falls back
to the retained plain input view with raw facts accessible, not guessed success.
No second Presentation is necessary for this same event. Existing independent
visible-only lane, receipt and explicit correlation remain; genuinely separate
summary still works as before. Its stored body is not rewritten for new styling.

```text
• Job completed · human-display-v3-job-1 · job_dcf0…b75c
  Exit 0 · Elapsed 3.2s
```

Only show `completed`/success when observed state+code prove it. Otherwise `Job
exited`, `Job failed`, `Job cancelled`, or `Job interrupted — result unknown`.
Details retain full IDs, absolute observation timestamps and stream counters.
Omit unavailable optional action/exit/duration fields entirely in the compact
view; do not add "unavailable" rows. With only terminal state Exited known, the
safe title is `Job exited`, not a success claim.
OutputPage metadata simplification is a native-only task using current returned
bytes; no Job service change is needed to hide routine offsets or retain a bounded
text preview. Gap/truncation remain visible exceptions. No fake filesystem links.

## Upgrade cost and implementation split

1. Native + Cutex agree/freeze the small structured-view wire/version/hash vectors.
   This is the remaining public-contract decision; current v1 cannot already do it.
2. review-r1 owns Job timing/store2/frozen completion v2 + actual owned process,
   failure/cancel/restart/clock and legacy retry tests. Do not enable v2 against an
   old consumer; explicit configuration/capability rollout, never probe by failure.
3. import-r1 owns Cutex dual-version intake, canonical content/view freeze and
   original ACK/replay/auth negatives. Native owner owns typed view and output
   formatting. One composed private proof after exact binaries; Human VM separate.

| Change | Necessary upgrade |
|---|---|
| New real execution facts / producer v2 | Job + Cutex, and native typed-view reader before enablement |
| Later title/color/layout over same facts | viewer only; can re-render stored structured facts without changing receipts/model text |
| Later generated model wording | Cutex new frozen projection for NEW events; no pending/history rewrite |
| New fact semantics | explicit data schema/producers/consumers; not a color-only update |
| Old native/writer with v2 data | reject before unsupported write/send; no downgrade/history rewrite |

Alternative: use the already supported explicitly linked Presentation carrying
independent display content and keep generic External input unchanged. This is
valid and cheaper when the requirement is a frozen human-written summary:
no new native wire, and existing receipts/replay/adjacent grouping apply. It does
not always produce a duplicate card: explicit adjacent grouping can combine the
visual presentation; late/non-adjacent arrival remains a truthful supplement.
It still needs a separate obligation/append/receipt and stores prose rather than
typed execution facts. Later layout can redraw that prose, but cannot reliably
select/relabel/reformat its facts without a parser or another producer event.

The proposed extension is worth its mechanical-history/projection/hash cost
specifically for re-renderable structured facts on the original inbound event,
with no second delivery race. If that requirement is deferred, linked frozen
Presentation is a sufficient smaller alternative, not an unsupported workaround.
Keep it for genuinely independent display content in either design. Encoding
JSON inside model text merely moves parsing into prose and does not separate data
from provider input; no opaque parser workaround.

Finite acceptance: real gate/start/wait timing excludes drains; restart unknowns;
legacy outbox exactbytes and changed-v2 replay conflict; full default model jobId
once; view payload absent from actual provider request; same receipt after restart;
new display/old plain fallback and independent summary; actionId never lookup;
auth/current-recipient/ACK guards unchanged. This assessment ran no builds/tests,
processes/VM/auth/model calls. It is source-backed design, not implemented behavior.
