# S6c2 — private durable delivery adapter

Base: `5864e151a8be29b4f6af774289a58ba25e127b6d` (tree
`fcc5f201c764385601048e8dcb570b0d7af41d2b`). This is a private candidate,
not a deployment, full CLI release, or business execution guarantee.

## Boundary and persistence

Only the already-reviewed U+S6 explicit launch enables this branch of the
existing Bus bridge. Official stock stays registration-only; unmarked K uses
its existing handler. No new polling worker, inbox, scheduler or model tool.

The existing Bus record freezes the exact generic envelope/digest before first
submission. Store version 3 prevents the previous version-2 reader/writer from
silently dropping these fields. Candidate writers only: no live migration or
mixed-writer compatibility. Legacy canonical pending records without an earlier
native submission can be explicitly projected; missing canonical data or an
earlier legacy submission fails closed. Fresh Task completion projections must
postdate the recipient's exact explicit activation receipt. Pre-activation
notifications without a native envelope require an explicit migration/review
decision; this candidate provides no raw-state repair or migration API.

Native Task message keys include the exact durable recipient hash. A new
Director gets a distinct pending context obligation; restart of the same
durable recipient keeps the key. Existing project-seat routing stays
authoritative, not the creator or global Director. Missing session observation
does not select a legacy fallback. Delivered historical notifications are not
reformatted or resent.

For each message: canonical source/target validation, frozen envelope, current
native status, submit **only on positive unknown**, validated native A4, then
provider mutation try-lock -> seat lock -> session lock/current occurrence ->
existing Task context fact -> Bus frozen-envelope CAS -> transport ACK.
The try-lock avoids deadlock with a lifecycle action joining its bridge. A
changed generation cannot commit a stale result. Original native receipts stay
original; `externalInputCommitGeneration` records the first successful business
commit occurrence. Observed status is not delivered state. Task context facts
and Job outbox delivery are not Task/Job execution or result acceptance.

On lost response, status reconciliation reuses the same key. No status/transport
failure is treated as absence. Restart never reformats frozen text or clears the
explicit launch marker. Existing pending-record redrive remains the recovery
source. A recipient/seat change during an ambiguous partial business commit
fails closed; no historical ownership inference or forced move is introduced.

One verified client is retained by the existing bridge. Full artifact hashing
occurs on connection creation between matching identity snapshots. Every fence
rechecks contract and file device/inode/size/mtime+ctime nanoseconds/mode/owner,
plus the existing runtime/Ready/binding/process proof. This is trusted same-UID
operation, not malicious same-UID isolation. At most 256 event hints are drained
per boundary; overflow/disconnect closes and reconciles. Hints never ACK.

## Modes, templates and limits

Only exact `after_turn` and `passive` are supported. `soon`, interrupt, Human/
owner messages, management controls and unsupported Task controls stay pending
with an explicit error. Every polled item gets a chance; a held item does not
block independent eligible messages. Existing bounded poll backoff applies.
No native retry is automatically issued. A production trusted-controller retry
UI/API remains a separate authorization/design decision; the S6c1 typed client
is not made into a model tool or public retry endpoint here.

Native references inspected at fixed `c2aaceb4`: core `agent/control.rs`
completion watcher, `session_prefix.rs` completion formatter, and
`context/subagent_notification.rs`. We retain their concise source/result idea,
not their user-role fragment or token truncation. Source is authenticated
`agent/<durable ID>` or `service/cutex-task-service|cutex-job-service` in the
generic envelope. Representative text:

```
Message Type: MESSAGE
Payload:
[message from Formal Agent] actual sender content

Task Service transition ReviewReady for assignment a-private
(task t-private revision 1, attempt 1).

Job: job-private
Result: succeeded
Output reference: existing output reference
Summary (external data): existing bounded summary
```

No repeated transport IDs, receipts, generations or digests in prose. Formal
names are friendly wording only. No title/cwd/profile identity inference. No
arbitrary URL/file fetching. The 65536-byte transport and receiver full-item
byte policy remain native-enforced; no truncation or automatic policy disable.

## Composed frozen inputs

- Native U+S6 `c2aaceb411b7851806c62435b97895a63a7d34cd`, tree
  `bd85280058ddc4b851d2bd9ca1904c392d62d388`, U ancestor
  `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a` / 0.153.4.
- App-server SHA `b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60`;
  host `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45`;
  aggregate `00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d`.
- Actual frozen Job `36f8b577b5a067cbf5da4ddc10757ea60d306898`, tree
  `85735f00a206d2482b1a4d11defc8d9bfac44892`, executable SHA
  `747290895f65f5e336e9ba5f0568af957e986b53d562b10fae3ec3ac9136bf5c`.
  Its existing opaque A4 field carries the original native receipt unchanged.

Each private run creates a new reviewed manifest pinning its newly built Cutex
facade. No frozen manifest/artifact was overwritten or verification bypassed.

## Recovery and remaining gates

Reject/revert the source candidate before exposure. Keep private version-3
stores with candidate writers; do not hand them to an older binary as rollback.
Owned probe children are stopped; original runtimes were not touched.

Retain S4 seconds-truncated PID-time risk and two baseline Management socket
failures, S2/S46 incidents (unremediated/uninvestigated), production-auth and
same-UID limits. Full patched CLI/distribution, production controller recovery,
live cutover, Windows/Android/TUI/full-suite and real provider model acceptance
remain separate. No managed Create/rotation expansion or deployment authority.
