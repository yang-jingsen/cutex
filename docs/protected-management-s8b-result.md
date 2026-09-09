# S8b private protected lifecycle / completion MCP result

Source/evidence complete. PRIVATE INTEGRATION_CANDIDATE, subject to Director
acceptance; not deployed or full lightweight replacement.

## Fixed source and bytes

Accepted base: `7db389cfec3c8b3cc405ed74bf52ed8a5022f1f9`, tree
`262d312f5fb37efd8ed34a4d7b13f88190d56a8e`.
Implementation lineage: `cb496ad` → `cfc82ef` → `db957eb` →
`527c48d2f0616f4576b57a7ba7180e6449d232e3` (tree
`5743f40e9745b8d90293df97eeac0ba7dab8148f`). Later report/oracle changes
do not change production code. Final handoff supplies the immutable result head.

Owned evidence root: `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
Frozen default artifacts: `artifacts/s8b-default-527c48d/` (features empty,
dev profile with debug=0). Manifest SHA256
`0b7ef3966cc4b664c50d9cc7bff3cda2ebfb888299af3d8a35a2268cc6fa8fcd`.
Cutex SHA256 `6086437e672db68d59de830bfb194af3aecc9e00d052f2b0385cfbad7532029e`;
facade `a736d862811704a9b64dd2f055682a97b532d978197602c7320e9b5d47be29ce`.
Separate crash-hook artifacts: `artifacts/s8b-hook-527c48d/`, manifest
`7c64567b711396bd4348ce3c6b2e3741a0a94f3522147aab4683e25481772757`.
They are NOT the default-byte proof.

Native remains patched U+S6e `a83dbb47ba6aa775f5d4b679fafc532c4db74c7f`,
not unchanged stock. Both manifests retain exact accepted native CLI/server/
U host/schema hashes and manifest `1b7e828279b1f3e152045ce94c31cf21acede34ddb0cc0eade727e4ae8056727`.
No native source or artifact was modified.

## Finite acceptance matrix

| Boundary | Evidence / status |
|---|---|
| Root-reviewed exact Create/Replace/Rotate intent; no ambient root MCP | Typed v2 intent binds complete request and existing project authority; private default MCP Create/replay and unauthorized Release caller passed |
| Replace close-after-ready, native bootstrap and predecessor retirement | `s8b06` PASS on default bytes: actual configured MCP, ready successor before predecessor retirement, exact replay |
| Retained Director rotation and next completion recipient | `s8b06` PASS: both Management and Task Director pointers agree on exact successor; subsequent terminal completion reaches successor, not predecessor |
| Creator death after Task seat transfer, before Management completion | `s8b05` PASS on explicit hook bytes: same action, same durable/native successor, native file count remains 3, both authority pointers complete; independent oracle passes |
| Release seated MCP request_changes/accept_result, exact replay/stale seat | `s8b06` PASS through real configured Core MCP metadata; stale seat unauthorized, exact accept replay stable, changed fail_result under same action conflicts |
| Release fail_result | New strict translation unit; existing provider terminal semantics reused; no new successful native-MCP fail case claimed |
| Other predecessor policies / protected roles / stale epoch | 67 provider regressions include close-before/after recovery, retained modes, role protection and authority divergence; not every policy has a stock native E2E |
| Spoofed model authority / unknown fields / old Task/query/send | 18 focused MCP regressions pass; unchanged occurrence fences retained |
| Generic Management message is authenticated service data | `s8b06` PASS: actual service-source native A4 and effective fake-Responses model input; no fake Human role |

The controller crash oracle is separate from native A4. A4 proves context
persistence, not task execution or acceptance. Private fake Responses selects
real configured Core MCP tools; it is not real-model autonomy. Harness-driven
Worker actions are not a claim of autonomous task wake/handling.
The default run executed 23 actual configured MCP calls, including denials and
replays. Independent `protected_management_history.py s8b06` verifies six native
context pairs, recomputed semantic/receipt digests, zero duplicate pairs,
original Management receipts and three matching Task completion delivered facts.
The same oracle on `s8b05` independently verifies the partial-transfer cutpoint
and same-ID recovery (bootstrap-only: no external context pair is expected).

## Checks and failure-preserving repairs

Commands use scrubbed environment, task-owned HOME/TMPDIR/CARGO_HOME/target,
one Cargo writer, and existing rustup read-only. Relevant checks:

- `cargo test --lib agent_management::provider::tests -- --test-threads=1`: 67 pass (run twice across fixes).
- `cargo test --lib agent_bus::mcp -- --test-threads=1`: 18 pass (two successful runs; initial new-field allowlist omission failed one test, then repaired).
- `tests/private_bridge_regressions.py <default lib-test executable> s8b-bridge-units`: 48 pass, each in a distinct verified private HOME/process. These overlap the smaller four-test external projection runs.
- `cargo check --lib --bins`, default `cargo build --bins`, separate `--features stock-launch-test-hook` build, `cargo fmt --check`, `git diff --check`: pass on implementation source.
- Full scoped production delta reviewed; no dependency/Cargo.lock, TUI, native, Job or canonical edits.

Focused suites contain 133 distinct passing tests (67 Management + 18 MCP +
48 bridge); the repeated/subset invocations above are not additional distinct
coverage. Existing warnings are unused import/dead-code warnings, not failures.
The native-bound fixtures use `bwrap --unshare-user --unshare-net`, an explicit
`S8_PRIVATE_BIN_DIR` pointing at the frozen default/hook directory, and a NEW
owned run name. Entry points are `tests/protected_management_boundary.py` and
`tests/protected_management_recovery_boundary.py`; both reuse the existing
private controller setup executable and pinned native artifacts.

Preserved failures are not relabeled success:

1. Initial broad bridge selection: 28 pass / 19 rejected missing explicit private-home guard. Corrected per-process fixture runs all 48 successfully; no production home access.
2. `s8b01` / `s8b02`: actual MCP Create/replay passed; Replace refused legacy group projection before predecessor stop. `s8b01` timed out awaiting a final receipt; `s8b02` was interrupted after the same recorded failure was identified. The harness now recognizes failure events instead of waiting for a success receipt.
3. `s8b03`: real cutpoint reached; recovery refused absent in-process successor connection. Fixed by exact original Ready occurrence reconnect, not another launch. `s8b05` proves the repair.
4. `s8b04`: predecessor scope inspection refused because systemd was visible but private DBUS/XDG state was absent. Reused accepted S5b restricted executable PATH for the real SystemctlUnavailable direct-process fallback. No fake scope response or production systemd contact is used to green the test.
5. Independent recovery oracle initially required a Bus ledger even for bootstrap-only/no messages. Corrected to allow its absence only in that no-message case; any native external commit still requires the real ledger.

Private fixtures use owned namespace/endpoints and the existing pre-connect
tripwire, fake actors/config/auth and frozen bytes. Only recorded owned children
are cleaned up. Direct-process fallback is not cgroup/hostile-child containment.
No paid/public model, live Agent/project/seat/credential/service operation,
deployment, push, history cleanup or migration occurred.
All recorded native PIDs from completed/failed S8b fixtures were verified absent
after owned cleanup. Retained task footprint before/after rounds to 11GiB;
Mambo free space is 402GiB, above the 100GiB floor and below the task's 20GiB cap.
Unoptimized artifact validation can exceed the native-compatible MCP request
budget: the fixture observed the original action journal, then replayed the
same action/payload. No timeout was treated as absence or a new create request.

## Remaining boundaries and recommendation

Reuse unchanged S8a positive persistence ACK/pre-ID uncertainty, S6f/g Task/CLI,
native pause/sandbox/A4 and Job evidence; no broad rerun. Windows, Android,
production OAuth/accounts, live cgroup/runtime acceptance, full-suite and full
replacement/distribution acceptance remain unrun. S2/S46 incidents remain
unremediated; retained seconds-truncated PID-time/arbitrary-descendant limits
and two earlier socket baseline failures remain disclosed, not fixed here.

Recommend a subsequent explicit production bundle-selection policy/UX decision:
Human selects a verified bundle/config policy; existing Director authority
continues to authorize scoped lifecycle actions. This private per-action review
is not a universal future Human-approval rule. Existing-Agent migration needs
an explicit compatibility and candidate-writer cutover decision; do not clear
markers, infer native ownership, or downgrade Soon/Busv4/intent stores.
Unknown pre-ID creation still requires owner action and no duplicate retry.
Rollback before exposure is rejection/revert of the candidate, not rewriting
candidate histories or feeding them to older parsers.
