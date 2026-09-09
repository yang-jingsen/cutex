# S6c2 result — private integration candidate

Base `5864e151a8be29b4f6af774289a58ba25e127b6d`, tree
`fcc5f201c764385601048e8dcb570b0d7af41d2b`. The immutable submission commit
containing this report is the candidate; its exact commit/tree accompany the
Task Service submission. No merge, remote/canonical edit or deployment.
Implementation commit `e1aa3bae3ff316e47c274031b3f77a0a7be218c6` has that
exact base as its sole parent; the submission adds only this final resource record.
Contract/provenance/recovery details: [native-delivery-s6c2.md](native-delivery-s6c2.md).

## Evidence

All evidence below is under owned
`/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`. Private network namespaces,
explicit endpoint connect tripwire, dummy profiles/auth, owned children and
fake Responses were used. No paid model or production provider acceptance.

| Criterion | Actual evidence / limitation |
|---|---|
| Ordinary durable Bus -> native A4 | `s6c2-04`, final default-feature `s6c2-07`: actual provider/store/bridge/native Unix RPC; original receipt in Bus |
| Task completion routing/context fact | Same runs: real Human project/import/member APIs, authenticated Director create/assign and Worker start/submit; ReviewReady reaches exact project Director; provider delivered-context reference equals native receipt |
| Job completion/outbox | Same runs: frozen J daemon, actual harmless subprocess, signed grant verification, submit replay same Job ID, real completion HTTP/outbox reaches delivered only with Bus/native A4. Grant issuer is a private fixture, **not** a fresh Core/MCP grant issuance proof; altered subscriber with original grant rejected |
| Independent history/model input | `external_delivery_history.py` on 04 and 07: three adjacent native commit/context pairs, recomputed envelope and receipt digests, matching frozen Bus envelopes/receipts, zero duplicate pairs; effective model input observations `[3,2,1]`, never duplicate within a request |
| Owner death after A4 before business commit | `s6c2-05` feature-gated real process fault: kill owned management process; explicit same-ID restart; original native receipt committed at generation 2 |
| Stale generation during business commit | Same run: concurrent explicit restart while A4 gate held; lifecycle try-lock rejects old commit, generation 3 reconciles original receipt. Independent history: two pairs, no duplicates |
| Passive/unsupported/no-output scheduling | `s6c2-06`, default feature: idle passive pending without A4; independent after_turn consumes it; soon stays explicit pending/error without blocking eligible work; no_output reaches A4 then held/no_output, repeated status never wakes/retries |
| Source, receipt and replay validation | Focused Rust tests: unsupported modes/roles/missing durable identity, UTF8 cap, frozen replay/reopen/foreign owner/changed semantics/receipt conflicts, current occurrence validation, file replacement/same-length write, recipient-scoped Task key |
| Legacy/stock/MCP compatibility | Existing Bus and bridge suites below; unchanged S6c1 stock registration-only and S4/S5 MCP/launch proofs reused. No new inbound claim for official stock |

Final default executable SHAs used in 07:
`cutex=e56e94f4e6b8ffb276132c8541d9e73a7a517f5e4c431f38e22233b72d8ce65b`,
`cutex-mcp=397a36649aa820c0300bc23d441bc7c6a418596a0252539e10e8657f4bc19247`.
The run's new `bundle-0.json`/`bundle-1.json` were reviewed/activated via the
existing private root API; no frozen artifact was edited. Formatting-only edits
after this build do not change the tested semantics.

## Commands/results

All Cargo calls used `env -i`, task-owned HOME/TMPDIR/CARGO_HOME/target,
read-only existing RUSTUP_HOME, four build jobs and debug-info disabled.
No concurrent target writers. Counts are **per selection, overlap-inclusive**,
not a claimed number of unique tests.

- `cargo test --lib external -- --test-threads=1`: 24 passed (`s6c2-unit-05.log`).
- `cargo test --lib task_delivery:: -- --test-threads=1`: 19 passed.
- `cargo test --lib agent_bus:: -- --test-threads=1`: 100 passed.
- `cargo test --lib native_completion_identity -- --test-threads=1`: 1 passed.
- `cargo test --bin cutex completion_identity_is_stable -- --test-threads=1`:
  1 passed (real binary handler unit surface).
- `cargo test --bin cutex ordinary_agent_bus_request_cannot_forge -- --test-threads=1`:
  3 passed (Task/Job/Management system-source forgery denied).
- `python3 tests/external_delivery_regressions.py`: 47 bridge selections passed,
  each in its required separately marked private HOME.
- `cargo build --bins`: passed; `cargo build --bins --features stock-launch-test-hook`
  passed for the real fault probe; `cargo check --bins`: passed. Existing warnings
  retained (including 8 binary warnings); no dependency/lockfile changes.
- `cargo fmt --all`, `cargo fmt --all -- --check`, `git diff --check`: passed.
- Composed runs use `bwrap --unshare-user --unshare-net` plus the exact owned
  writable task-root bind and `env -i`; invoke
  `tests/external_delivery_boundary.py RUN CONTROLLER [fault|modes]`.
  The reused controller executable only identifies the accepted setup entrance
  in these runs; delivery itself uses the actual production bridge.
- Read-only `tests/external_delivery_history.py s6c2-07` and `s6c2-05` passed.

Preserved failures: initial compile needed explicit conversion of typed ID
validation errors; 01 expected wrong send-response field; 02 expected wrong
Worker response envelope. Corrected against actual schemas, not repeated blind
requests. The first broad bridge invocation failed 19/47 because the required
private-HOME marker/env was absent (28 passed); explicit per-test isolated
fixtures then passed all 47. Initial private fixture response logging in 02 was
subsequently reduced to nonsecret outcome facts; it never used production auth.
No native/Job product patch was made to obtain passing tests.

## Limits and next decision

This matrix does **not** prove every requested fault as a separate real-process
case: no dedicated socket reply-drop proxy, Bus-daemon kill after Task context
fact but before Bus CAS, or post-ACK-loss transport capture; no real concurrent
Director rotation in the composed native run. Existing routing/ACK/provider
tests and the new real generation/crash tests are narrower evidence. Current
recipient mismatch in ambiguous historical partial commits remains fail-closed
and visible, not an automatic repair or falsely successful delivery.

Offline/archive/permanent retirement and wrong-generation refusals retain
existing provider/occurrence guards; their full real lifecycle matrix was not
rerun. No new controller retry authority was invented: held/policy/uncertain
pending work needs a subsequent explicitly authorized controller surface.
Task assignment/follow-up/watchdog ingress beyond existing canonical supported
projections is not claimed; the actual new Task proof is completion ReviewReady.
Legacy management-event rendering was not expanded; the native original receipt
and delivery state live in the existing authoritative Bus snapshot.

Reuse unchanged native S6b2r2 queue/crash/P0 and S6c1 policy/registration tests,
S4 process/PTY/sandbox and S5 outbound proofs. Full workspace/all-features,
real provider models, patched full CLI/distribution, Windows/Android/TUI,
production auth/live stores/services/Agents and deployment are omitted gates.
S2/S46 incidents remain unremediated/uninvestigated; S4 PID-time and socket
baseline risks remain. No business result execution/acceptance inferred from A4.

Resources: approximately 8.6 GiB task root before, 9.6 GiB retained after
the final binary-test build;
filesystem free 418 GiB at last check (above 100 GiB floor, below 20 GiB cap).
Only owned private children were stopped. Keep frozen manifests/evidence and
candidate-writer private stores; reject/revert source before any exposure.

Recommendation: Director review as a **private integration candidate with the
listed recovery/coverage limits**, not release or automatic product cutover.
Next narrow work is authorized controller recovery plus the remaining composed
fault/rotation cases, then patched full-CLI/distribution acceptance.

Scoped self-review covered the complete changed Rust/harness/document delta:
launch selection, connection/artifact fences, source templates, lock ordering,
Task recipient keys, frozen-store CAS, original Job receipt serialization and
private test cleanup. No Task/Job business state machine, authority grant,
native source, TUI, dependency or Cargo.lock changes.
