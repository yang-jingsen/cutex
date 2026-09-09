# S6f result — private integration candidate

Outcome: bounded Soon/Task/CLI integration passed the listed private checks.
Intended use PRIVATE INTEGRATION_CANDIDATE, subject to Director acceptance;
not deployed, release-approved, or a full replacement claim.

Source base `2af181a87b54a193e8b8bf1a2ad78cd544475beb`, tree
`fbf1a641a1dbe7eaf3645e8ceaba719309045c73`. No canonical, native, Job,
TUI, installed artifact or live state changes.

Native input: `a83dbb47ba6aa775f5d4b679fafc532c4db74c7f`, tree
`3d4f5d0510d5b1b3800689fd09bef8e91b409953`. Manifest SHA256
`1b7e828279b1f3e152045ce94c31cf21acede34ddb0cc0eade727e4ae8056727`.
Exact CLI/server/host/schema hashes are constants in `src/launch/stock.rs`
and the immutable native manifest. This is patched U+S6e, not official stock.

## Evidence ledger

Owned root `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1` holds compact logs
and private fixture evidence. All Cargo HOME/cache/target/temp paths were owned
Mambo paths; no parallel target writers. Fake Responses is a protocol oracle,
not paid or real-model acceptance. Provider/Bus/native persistence is real.

| Check | Result / scope |
| --- | --- |
| `cargo test --lib launch::stock` | 5 passed, exact bundle/capability/config validation |
| `cargo test --lib external_` | 26 passed (latest `s6f-external-final-tests.log`) |
| affected Task delivery selection | 19 passed (`s6f-task-delivery-tests.log`) |
| `schedule_urgent_coordinator_wakes` | 2 passed; blocked/abort/decline/retries-exhausted provider policy |
| `native_completion_identity` | 1 passed; recipient/event/family separation |
| `exact_native_digest` | 1 passed; exact native Soon vector, generation-independent digest |
| binary `ingress_guard_tests` | 2 passed; named permissions and real owned-child pre-stop refusal |
| `explicit_launch_versions` | 1 passed; v1/v2, unknown version and changed evidence |
| default `cargo check --bins` | passed (`s6f-check-final-02.log`) |
| default build03 / final feature build | passed; final feature bytes frozen in `artifacts/s6f-final` |
| fmt / diff check | passed at current scoped delta |
| `s6f-03` | actual busy Soon assignment vs AfterTurn, follow-up, repair/accept; 6 unique pairs |
| `s6f-05` | actual Worker Soon reminder + Director AfterTurn escalation, follow-up/closure; 7 pairs |
| `s6f-c2` | actual supported Cutex-selected CLI, Human input, same owner/generation, restored termios |
| `s6f-h1` | configured MCP list and Task A4 passed; raw assignment-text PTY assertion failed |
| combined corrected h2 | passed MCP, visible Task Soon response, blocked/cancel Soon, actual EROFS, approval decline, interruption hold, restored termios |
| independent h2 history oracle | 3 unique pairs, 2 completion facts, 1 pending interrupted item; 7 read-only/on-request contexts |
| final-byte h3 | passed; new reviewed manifests, rejects old activation version without store change; repeats composed 3-pair/2-fact/1-held oracle |

The listed focused test selections total 57 duplicate-inclusive passes; do not
interpret as 57 unique tests. Two initial selectors selected zero and are
excluded. Source-level policy coverage is not native end-to-end proof for
every mutually exclusive Task terminal transition.

## Failures and limits

Run01: fixture HTTP timeout. Run02: real named-profile mismatch from legacy
resume override, corrected only for the new bundle. Run04: premature fixture
file observation, corrected. Initial CLI fixture: path exceeded existing Unix
socket limit; short owned fixture passed without production geometry changes.
H1: generic CLI does not expose raw Task assignment text as assumed by the
fixture; no Task renderer was added. Corrected test requires visible response
from actual canonical Task context, never duplicate Human input.
H2's PASS file has a stale namespace-smoke label; its actual command used the
approved fixed same-host-UID guard. The final harness fixes the label without
rewriting prior evidence. This guard is not hostile-child OS network isolation.

Earlier S6c3 recovery/authority cutpoints, S6c2 Job A4, S4 lifecycle guards and
native S6e timing/P0 evidence are reused at unchanged boundaries. This does not
prove every cutpoint again with Soon. No automatic retry, wake, migration,
business execution/completion from A4, or unsupported Interrupt conversion.

Omitted: real model/provider, live registration, production stores/services,
full workspace/all-features, Windows/Android, full Management bootstrap or
rotation, full CLI distribution/release acceptance. S2/S46 incidents remain
unremediated and uninvestigated; S4 seconds-truncated PID-time risk and two
baseline socket failures remain disclosed. No hostile same-UID sandbox claim.
The prior optional S6c3 Release-accept fixture failure is not reclassified as
passing: release-review/OwnerActionRequired full native workflow remains a
separate coverage gap. Decline/abort/retries-exhausted are provider-tested here,
not claimed as distinct native E2E executions. Single-item recovery and lost
ACK/authority crash cuts reuse the accepted unchanged S6c3/native evidence;
there is no new exhaustive Soon-specific crash campaign.

Resource final: 9.8GiB retained (including 90MB frozen Cutex/facade), 403GiB free.
Before exposure reject/revert candidate. After Soon history writes, retain
candidate-compatible private writers; no old-parser downgrade/history rewrite.

Supported command: `cutex session stock-attach EXACT_DURABLE_ID`, following
explicit root/Human review+activation and `review_runtime`/`run` on the existing
private explicit-launch endpoint. Bundle3/contract2 and exact pins are required;
no upgrade/clear API for an old marker. `native-soon-s6f-build.json` records the
final composed bytes. The final composed run uses the test-only fixed private
guard feature; default builds/checks and executable pre-stop tests are separate
evidence, not a default-build production sandbox claim.

Self-review covered every scoped source change plus new harness/oracles/docs.
No dependency/lock, native/Job/TUI, authority policy or service-ledger redesign.
Next gates: full CLI distribution/default-release composition, remaining
release-review/OwnerActionRequired and managed bootstrap/rotation workflows,
and explicit writer/history migration policy. Native raw Task-card rendering
is not implemented or claimed. All owned probe children were cleaned up by
their tracked fixture handles; original runtime was not changed.
