# S4r3 private stock launch candidate

Historical report of `1f039dcf58efa2958270e192f782f2203b708919`.
The requested recovery repair and measured socket-test classification supersede
the corresponding limitations below: [repair result](stock-launch-s4r3-repair.md).

Base: `11e28adaa9d3629d609e07862f0d468642dbfdfd`, tree
`a22b8d2a6c8ab42d6c3d2fcf6004c9f091ae1a58`. Descends the S3/S2
decision/prototype; this is not the current release or a TUI merge.
The commit containing this report is the immutable source candidate. No deployment.

## Supported entry and contract

`cutex session stock --request REQUEST.json --management-url http://127.0.0.1:PORT`
submits a typed root-Human operation to an **already running**, explicitly selected
private Management endpoint. It never starts a service. Requests are `review`,
`activate`, `review_runtime` (explicit `restart` boolean), and `run`. Review output
is supplied unchanged to activation/run with a stable action ID. Credentials come
from the private Management configuration, never command arguments/model schema.
`cutex session stock-attach EXACT_DURABLE_ID` runs native CLI resume against the
same ready Unix owner; configuration overrides belong after the `resume` command.

The optional durable `explicit_launch` version-1 requirement binds exact native
UUID, canonical authoritative native home, and a hashed bundle manifest. The
manifest pins upstream commit, executable, enabled code-mode companion, facade,
generated experimental schema and unchanged shared configuration. Executable,
companion and schema also have compiled fixed hashes. Facade hash/path are reviewed
inputs. This metadata is **not identity, profile, or a new runtime backend**.
Identity, native history and formal Agent name remain unchanged; source stays the
actual native source. No forced top-level classification.

Current configuration is independently reviewed and revalidated: exact configured
or actual inherited profile, account/profile hashes, allowlisted model/reasoning,
Host/Codex, explicit sandbox and approval. Only private loopback fake Responses
profiles without auth files/accounts are supported. Unknown fields, arbitrary CLI
options, proxy/backend/auth aliases and inconsistent permission aliases reject.
The shared native home is profile-independent; launch writes neither global
default nor shared config, and copies/symlinks no auth or rollout.

Root authentication uses the existing HTTP boundary; provider mutation lock,
Task read fence and durable CAS protect activation and execution. Existing
Director/Operator/project/task/archive/permanent-retirement guards remain.
Activation receipt and marker share the durable commit. Exact final replay returns
the historical receipt; changed semantics conflict. Generic online/restart/attach/
takeover/recovery reject marked records before fallback (restart also carries a
fresh pre-stop fence). Unmarked paths retain existing behavior.

Runtime journal stages are Prepared, Claimed, Spawned, Ready. One process-group
owner is spawned; registration readback must match exact durable/native identity,
PID, runtime ID, generation and claim before Ready. Authenticated MCP is denied
before Ready. Lost registration/readiness after a committed binding reuses that
binding and action, never another native thread. Registration-only bridge has no
inbound polling/ack/submission; Cutex activity ingress is explicitly unsupported.

**Recovery limit:** a crash/failure before the child binding is durably committed
leaves Claimed and refuses replay/new launch. In the caught commit-failure path the
owned child is stopped; the marker/claim are retained and the error reports that
stage. There is no claim-clear, marker-clear, guessed-PID recovery or K fallback.
This is safe refusal, **not successful automatic recovery**. Completing automatic
recovery of the pre-binding crash window needs a bounded child-publication/
reconciliation protocol; it is not proven by the lost-readiness test.

This experimental store permits candidate writers only. Older writers may drop
the optional field; mixed-writer compatibility, downgrade/history migration and
production writer cutover are not supported. Stopping/archive/restore/profile edits
do not remove the requirement. Once stock may have written history, reverting a
binary is not permission to expose that private store to older writers.

## Fixed artifact provenance

Stock 0.153.4 / U `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`;
official package SHA-256
`a822187e1a2420c61c5926721bfbd878701ed95547c9bb0d4de4498a16ba1821`;
executable `56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da`.
Companion `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45`;
experimental schema `b06f77062369d481a59cc70720c12b89cb9dd49c385863923262102d3ad6c978`.
Artifacts reused read-only from S1; no download/kernel build. S1 harness
`d57cfe4f299b474ca8e6e0f235f62e687535532f` and S2 evidence are unchanged.

## Evidence and boundaries

Owned root: `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
`tests/stock_launch_boundary.py` uses real stock native RPC/CLI and real private
Cutex providers/stores, explicit public adoption/import/project APIs, owned
subprocesses and a fake Responses server (no paid calls). Authentication is real
Cutex; profiles, model responses and project membership are private fixtures, not
production registration. Test-hook builds inject registration denial/lost readiness
and precommit failure; hooks are disabled by default and grant no authority.

| Criterion | Actual evidence |
|---|---|
| Root auth/review/CAS/replay, foreign native/home/version denial | Private production HTTP handlers in lifecycle harness |
| Two profiles, same durable/native/home/history, unchanged global/shared config | `s419/PASS.json`, actual alpha-to-beta explicit restart |
| Same-owner native CLI, approval required, decline, cooked terminal recovery | `s418` and `s419`, production `stock-attach` Linux PTY |
| Actual Core read-only file/network denial | `s419/sandbox-output.json`: EROFS(30), EPERM(1) |
| Outbound configured stock MCP query/send, stale/foreign/missing denial | `s419/PASS.json`, real provider and persisted recipient; pre-Ready denial |
| Lost readiness | Same-action, same binding/current generation replay in private harness |
| Marker preservation / generic refusal / no inbound | Focused production-helper/bridge tests; bridge transport is a counter fixture |
| Concurrent exact runtime replay | `s424`: two concurrent production requests return the same receipt/owner |
| Registration auth failure + lost readiness recovery | `s424`: actual private Bus denial, then same binding re-registration/readback |
| Workspace/full-access effective sandbox | `s424`: workspace write allowed/network EPERM; full-access write and owned loopback connection allowed; approval remains on-request |
| Commit-failure cleanup and no duplicate replay | `s423`: injected precommit error, actual owned child absent, unrelated process alive, claim retained; automatic recovery **not implemented** |

`s424/PASS.json` is the final combined 12-fake-response run (four generations).
Its original `stage` label says two-launches-outbound; that undercounting report
label was subsequently corrected in the harness without rerunning the proof.
No evidence file was rewritten. `s423` is a separate failure-boundary run.

Focused commands used locked Cargo, scrubbed environment, task-owned
`CARGO_HOME`, `CARGO_TARGET_DIR`, `HOME`, and `TMPDIR`:

- `cargo build --locked --bins --features stock-launch-test-hook`: passed
  (`s4r3-build13.log`); hooks are private probe-only, not default enabled.
- `cargo check --locked --bins`: passed (`s4r3-final-check2.log`, final source).
- `cargo test --locked --lib stock_`: 5 passed.
- Bin `cli_app::app_server_runtime::tests`: 17 passed after shortening two
  test socket paths; separate new `stock_generic` test: 1 passed.
- Bin `cli_app::management_lifecycle::profile_tests`: 16 passed.
- Lib `agent_management::archive`: 8 passed; `runtime::lifecycle::tests`: 3 passed.
- Bin `cli_app::management_context::tests`: 6 passed, **2 environment-blocked**
  by the unchanged default runtime's 100-byte Unix path limit. Both fixture HOME
  placements (task tmp and shorter task root) were tried and preserved. No default
  runtime geometry was changed to make these tests green; no baseline binary rerun.
- Thus 56 distinct selected tests passed, 2 distinct path-limited tests did not.
  Earlier repeat invocations overlap and are not added to that count.
- `cargo fmt --all -- --check` and `git diff --check`: passed. Full scoped delta
  reviewed, including added files and dispatch/guard/receipt/auth boundaries.
  No dependency or lockfile changes. Existing warnings remain; no broad cleanup.

Not exhaustively rerun: full inherited task/role matrix, all TUI tests (untouched),
real power-loss/fsync faults, arbitrary process-tree escape, production auth/runtime.
The controlled commit error is not a simulated success or a real filesystem crash.

The native CLI diagnostic relay (`s416`) is separate from the production command
oracle. Earlier failed attempts are retained, not relabelled passing. The workspace
observer failure (`s420`, `s422`) had actual completed native tool output but lacked
observer subscription after restart; the harness now explicitly subscribes.

No inbound/A4/after-turn/custom event rendering/full MCP inventory, production
OAuth/keyring/multiple accounts, live managed cgroup, Windows/Android, or full
production migration acceptance. Process-group proof covers the owned tested
descendants, not arbitrary escaped descendants or a cgroup guarantee. S1/Job OS
evidence is reused only where unchanged, not substituted for stock Core tests.

## Safety disclosures (separate incidents)

The earlier S2 production-credential exposure remains unresolved. No original
sensitive output was reread; no rotation/remediation/no-impact claim is made.

S4r3 `s46` separately attempted generic CLI online before its early marker guard
existed. Endpoint resolution contacted the default Management endpoint with a
**dummy fixture credential** and received HTTP 401. No successful mutation was
evidenced; impact was not investigated and is not claimed absent. R13 was notified.
The source now rejects locally before endpoint resolution. Subsequent dynamic
Cutex probes use the explicit owned-port connection tripwire in
`tests/stock_probe_connect_guard.c`; unexpected connect exits before syscall.
This is a test safeguard, not a sandbox, and does not instrument static stock.
Only tracked private child handles/owned PID evidence are inspected or stopped.

## Remaining integration decision

Future private lightweight line only, subject to Director acceptance. The
pre-binding recovery limitation must remain explicit; no live deployment or
stock/fork equivalence follows from this candidate. Private probe children are
stopped; evidence/stores are retained. Reject/revert the source candidate before
exposure, but do not use that as a store downgrade procedure.

Resource observation: approximately 6.0 GiB retained/453 GiB free at this resumed
segment's start; final observation 7.6 GiB retained/452 GiB free. All new
build/cache/fixture output stayed in the owned Mambo root, below 20 GiB and above
the 100 GiB free floor. One generated Python bytecode cache was removed; unique
source and failure evidence were preserved. The final handoff records exact
commit/tree and final disk observation separately.
