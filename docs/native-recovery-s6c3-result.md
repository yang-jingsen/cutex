# S6c3 result — private integration candidate

Exact base `2d49567f9c8fa52a819ea4c306e241992710beab`, tree
`2ff845835d8bfda596ccd7d4d85ef611a3d1e5af`. The submission commit containing
this report is its immutable descendant; commit/tree accompany the submission.
[Recovery usage and coverage inventory](native-recovery-s6c3.md).

## Actual evidence

Runs/logs are retained under `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
All composed tests used owned dummy homes/auth/profiles, real private Cutex
providers/stores, pinned native Unix RPC, fake Responses, isolated Linux network
namespace and explicit connect tripwire. This is not real-model acceptance.

| Criterion | Evidence / exact boundary |
|---|---|
| Root review/confirm/status/replay | `s6c3-02/PASS-RECOVERY.json`, default-feature executable: nonroot401, forged caller400, stale binding409, false confirmation409; exact replay returns same receipt, changed retryID409 |
| No automatic retry/wake | Same run: repeated review/status no extra model request; explicit permission increases model calls3->4 exactly; original A4 unchanged; no_output held state is actual native |
| Native history after retry | Independent `external_recovery_history.py s6c3-02 recovery`:4 original pairs,0 duplicates, matching original receipt; no new context pair manufactured by retry |
| Lost submit reply | Feature hook discards one actual native response after RPC/fence, preserving exact message key; subsequent real bridge status reconciliation completes it. Runs04/05/06 assert owned loss marker; this is not a network proxy |
| Task fact -> process death -> Bus CAS recovery | Run05 reaches actual Task context fact before Bus CAS; kills owned management+Bus; explicit same-durable/native restart; original receipt committed at generation2 and Task facts unchanged. Independent oracle:2 pairs,0 duplicates,1 Task fact, generations1/2 |
| Director change before commit | `s6c3-04/PASS-DIRECTOR-FENCE.json`: real root project-authority CAS at precommit gate; old native A4 remains truthful, Task/Bus delivery absent. Oracle:2 pairs,0 duplicates,0 Task delivery facts,1 pending A4. Deliberately incomplete coupled transfer, **not full rotation** |
| Accepted-result TerminalClosure | `s6c3-06/PASS-CUTPOINTS.json`: real Director create/assign, Worker start/submit, ReviewReady, Director accept and TerminalClosure. Oracle:3 pairs,0 duplicates,2 Task facts |
| Durable permission/replay | Store test covers prepared+completed replay, changed retry conflict, no downgrade to prepared, reopen and version4 preservation |
| C workflow inventory | Exact modes recorded in contract; Soon assignment/follow-up/control and watchdog native adapter remain deferred, not silently converted |

Run05's complete harness exits unsuccessfully **after the requested cutpoint
assertions passed**: optional Release accept returns `project_authority_absent`
from the existing Director transport. That denial is preserved, not relabeled
as successful acceptance. Updated harness stops that mode at its requested
cutpoint; it was not rerun solely to turn the final exit green. Separate run06
proves ordinary Director acceptance/closure. The independent run05 history/store
oracle passed against retained evidence.

Run03 similarly preserves the actual protected-Director explicit restart denial
(`archive_requires_explicit_director_rotation` through the existing guard),
after reaching the Task-fact gate. Run05 uses an ordinary Release-review recipient
with real private project/seat APIs; no protection bypass or fake state edit.

## Provenance and checks

Native source `c2aaceb411b7851806c62435b97895a63a7d34cd` (U+S6 patched),
app-server SHA `b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60`,
U host SHA `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45`,
schema SHA `00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d`.
No newer CLI/Soon artifact selected. Job SHA `747290895f65f5e336e9ba5f0568af957e986b53d562b10fae3ec3ac9136bf5c`
and S6c2 real Job proof reused, not rerun or rebuilt.

Feature executable used in03-06:
`cutex=5a11e44aa8a81031e9d9584f433c03649c11f72707e85865ff9b9ad89e1cd823`,
`cutex-mcp=5927516774af96b6a3f67987b7c2fabec66368909b0dbf51cc1461615480f16e`.
Every run creates/reviews a new private bundle through the accepted activation
path. No frozen manifest/binary was modified. Test-only hooks require the build
feature plus explicit private markers/selectors; production default omits them.
Final default build (not another full composed rerun):
`cutex=f7be8db725fab31a8454aa5b89cc9f497c4c8598edf86c7b18cc54f7448dde37`,
`cutex-mcp=8799423823afbd5070124492610aae7bc2e0dfd5d1c6c074a01f0a3b7a39f799`.

Cargo calls use `env -i`, private HOME/TMPDIR/CARGO_HOME/target, read-only
RUSTUP_HOME, debug-info0/jobs4; no concurrent target writers. Selections overlap:

- `cargo test --lib recovery -- --test-threads=1`:14 passed.
- `cargo test --lib external_bus_freeze -- --test-threads=1`:1 passed.
- `cargo test --lib external -- --test-threads=1`:26 passed.
- `cargo test --lib human_management_projects_tasks_and_writes_require -- --test-threads=1`:1 passed.
- `cargo test --lib task_delivery:: -- --test-threads=1`:19 passed.
- Default bins and feature-hook bins built; final default build and `cargo check --bins` passed.
- `cargo fmt --all`, final `cargo fmt --all -- --check` and `git diff --check` passed; no dependencies/Cargo.lock changes.
- Composed entrance: `bwrap --unshare-user --unshare-net --ro-bind / / --dev /dev`
  with only owned root writable bind, `env -i`, then
  `tests/external_recovery_boundary.py RUN CONTROLLER modes` or
  `tests/external_delivery_cutpoints.py RUN CONTROLLER director|taskfact|closure`.
- Independent `external_recovery_history.py` passed on02/04/05/06.

Preserved setup failures: initial root error-envelope type mismatch fixed before
build; run01 stale review caused by heartbeat-only timestamps, fixed by explicit
specification digest; first extended store test still expected version3, corrected
to4; initial oracle used snake_case instead of the existing camelCase store field,
corrected read-only. No native/product policy was weakened to make tests pass.

## Remaining limits / handoff

Prepared retry permission after changed occurrence/authority remains fail-closed,
not universally recoverable. Whole-authority digest can invalidate otherwise
unrelated reviews. No dedicated process death during retry-permission commit or
post-ACK-loss capture; native idempotent retry tests and repository tests are
narrower evidence. Current Director restart remains protected. No complete
coupled Director rotation campaign; observed authority divergence is safely
pending, not repaired automatically. No universal exactly-once guarantee.

Reuse unchanged S4 launch/PTY/sandbox, S5 outbound, native queue/P0, S6c1
policy/provenance and S6c2 Job/legacy evidence. Full suite/all-features,
real-model/production-auth, Windows/Android/TUI, full CLI/distribution,
live providers/stores/services/Agents and deployment omitted. S2/S46 incidents,
S4 seconds-truncated PID-time risk and2 baseline socket failures remain disclosed.

Scoped self-review covers all changed Rust, harness/oracle and docs: root auth,
strict schema, lock order, immutable receipts/version guard, current project
authority, actual fault placement and owned cleanup. No native/Job/Task business
state-machine/TUI edits or canonical/remote mutations.

Resources: retained task root9.7GiB, filesystem407GiB free at final resource
check (20GiB/100GiB limits respected). Only owned private children stopped.
Reject/revert candidate before exposure; do not run legacy writers against its
v4 private store. Next bounded integration should consume the coherent accepted
Soon+CLI/native/schema bundle and close the explicitly listed consumers, not
claim all Task workflows migrated. **Private candidate, not release/deployment.**
