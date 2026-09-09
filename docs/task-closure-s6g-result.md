# S6g private Task closure / default bundle evidence

Outcome: listed gates passed. Intended use PRIVATE INTEGRATION_CANDIDATE,
subject to Director acceptance; not deployed or a full replacement claim.

Base: `94a3f2b34e04c70eaa4faf88c1a4772f4872d30a`, tree
`f5ea26c9322adfda030f7c0c6082197ee6b4d68f`. Production behavior is unchanged;
the delta is a test-only trusted provider fixture, private protocol/PTY oracles,
and this bounded evidence/next-step packet. No native/TUI/Job/canonical edits.

## Authority finding

The earlier S6c3 Release failure is preserved, not relabeled as passing. Its
Release-only actor called `/api/task/v2/director-action`, which correctly
requires current project Director authority. The existing authenticated
`/api/task/v2/terminal` boundary instead resolves the current seat and delegates
to the Task provider's completion-authority check. S6g uses distinct private
Director, Worker and Release actors, with Release bound through the existing
root fixture seat API. No provider authorization was weakened.

Configured MCP uses actual native Core metadata and normal per-runtime facade
credentials. The fake Responses scheduler targets an exact native thread only
to avoid one fixture actor consuming another actor's prescribed tool call;
it does not choose provider identity or authority. Director create/assign/cancel
and Worker start/submit use the configured MCP path. Release terminal decisions
use the existing authenticated HTTP route with mechanical CAS context obtained
from its seated query, not model arguments. This is not a new Release MCP tool.

## Finite policy / proof matrix

| Transition | Authoritative recipient / mode | Evidence boundary |
| --- | --- | --- |
| assignment | exact Worker / Soon | S6g configured MCP + actual Task/Bus/native |
| release_review submission | current Release completion seat / AfterTurn | S6g distinct Release native owner |
| authorized accept | current project coordinator / AfterTurn | S6g terminal decision + completion fact/A4 |
| fail_result | OwnerActionRequired to coordinator / Soon | S6g terminal decision; **not** terminal completion |
| coordinator cancel | coordinator TerminalClosure / Soon | S6g MCP cancel after failed review |
| RetriesExhausted | coordinator / Soon | explicit test-only system provider transition; no automatic producer claim |
| decline / abort | coordinator / Soon | reused provider policy; not separate native campaigns |
| request_changes / repair | exact Worker / Soon | reused S6f actual native flow |
| blocked / watchdog | current coordinator or Worker per existing policy | reused S6f h3/run05 |

Correction to S6f's prose table: accepted TerminalClosure targets the
**coordinator**, not the Release completion seat. The source already did this;
the distinction becomes observable with the separate Release actor here.
`fail_result` schedules OwnerActionRequired, not a successful terminal closure.

Example actual assignment text (mechanical metadata remains outside prose):

```text
Assignment ID: a-s6g-accept
Task: t-s6g-accept revision 1
Action: perform the assigned work using Task Service tools.
Contract:
Private Release review: inspect the bounded result and decide explicitly.
```

Existing completion text names the transition and assignment/task once. A4 is
context publication, not acceptance, execution, visible raw card, or tool output.

## Default artifacts and verification

`task-closure-s6g-build.json` pins separately frozen DEFAULT-feature Cutex/facade
and the unchanged accepted native CLI/app-server/host/schema. New private
manifest review/activation binds those exact bytes; no prior manifest rewrite.
The supported attach command remains `cutex session stock-attach EXACT_DURABLE_ID`
after explicit root review/activation and runtime review/run. Receiver named
permissions are selected before creation; attach supplies no legacy sandbox
override and grants no elevation.

Evidence is under the owned Mambo root. Run `s6g01` passed the initial real
default-byte Release accept/fail/cancel flow. Final expanded `s6g03` passed on
the frozen default bytes. The independent history oracle checks canonical
digest, original receipt ID, adjacent native context pair, exact recipient,
Task delivery fact references and effective fake-Responses input. It found
**8 unique native pairs, 0 duplicates, 6 completion facts and 20 read-only /
on-request contexts**. Two ReviewReady notifications reached Release; accepted
closure, OwnerActionRequired, cancelled closure and exhaustion reached the
coordinator. The offline fourth recipient remained generation 0, unlaunched.
The two Task outcomes were Completed and Failed/Cancelled, not inferred from A4.
Stale Release-seat action refused without Task-store change, and already
delivered snapshots remained unchanged across seat changes and terminal replay.

Actual configured MCP: 11 calls (10 committed, offline assign explicitly
`response_uncertain` with durable SendPrepared retained). No duplicate assign
retry was used. The system controller records exhaustion for that exact send.
Default-byte same-thread `stock-attach` showed a unique new Human reply, retained
owner/generation, exited 0 and restored termios. No nested sandbox command was
run in this namespace; effective receiver policy is independently recorded.

Focused default tests: urgent coordinator policy 2; terminal/query
forgery and semantic replay 2; shared Director/Worker/Release seats 1;
aggregate terminal CAS 1. Default `cargo build --bins`, `cargo check --bins`
and affected lib-test compilation passed. Formatting corrections were confined
to the new test helper; no dependencies or lockfile changes.
The explicit exhaustion controller added 1 pass in final `s6g03`: 7 focused
passes total, excluding the duplicate controller invocation in preserved run02.
`cargo fmt --check`, `git diff --check` and full scoped self-review passed.

Recorded composed invocation (all paths below are private owned artifacts):

```sh
bwrap --unshare-user --unshare-net --ro-bind / / --dev /dev \
  --bind /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1 /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1 \
  --die-with-parent env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  HOME=/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/home \
  TMPDIR=/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/tmp \
  python3 tests/task_closure_default_boundary.py s6g03 \
  /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/target/debug/deps/external_input_controller-aa6dcf9121932446 \
  /mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/target/debug/deps/cutex-f8078bf232693b9e
python3 tests/task_closure_default_history.py s6g03
```

The ingress controller argument is inherited setup validation only; the S6g
campaign does not run its prior controller suite. The second test executable
invokes exactly one ignored, private-home-gated system controller test. It is
not the default Cutex daemon/facade and is labeled separately. To reproduce,
choose a new short owned run directory; the harness refuses an existing one.

## Limits / next use

Private user/network namespaces contain the default-byte smoke. Parent probes
also retain the explicit owned-endpoint tripwire. No public/paid provider or
production account/state was used. Fake Responses proves protocol integration,
not autonomous model intelligence. The explicit exhaustion controller calls the
real provider; current source has no automatic RetriesExhausted producer, so no
scheduler or automatic retry-exhaustion guarantee is claimed.

Preserved `s6g02`: distinct Release accept/fail/cancel and stale-seat refusal
passed, and the trusted test controller persisted RetriesExhausted. The fixture
then timed out waiting for its sixth Bus notification: an out-of-process
provider write does not request the Bus's in-process drain. Corrected `s6g03`
uses an exact terminal-action replay; that public handler explicitly requests
the completion drain on a Committed response. It does not add a business
transition or invent an automatic exhaustion producer. This is a concrete
scheduling edge, not an incidental roundtrip or sleep-based persistence proof.
The PTY oracle also requires a unique new-turn reply rather than accepting a
historical resumed reply. Run02 never reached its weaker PTY check.

Reuse S6f h3's exact-native EROFS/approval/pause proof at unchanged launch/native
semantics; it remains **test-feature** evidence, not a default distribution
sandbox campaign. Reuse S6c3 fact/Bus crash and authority-fence cutpoints,
S6c2 Job A4 and earlier native queue/P0 proofs. No wholesale rerun or stronger
exactly-once guarantee. Full workspace/all-features, real provider, Windows,
Android, Management bootstrap/rotation and live release acceptance remain out.

S2/S46 incidents remain unremediated and uninvestigated; S4 seconds-truncated
PID-time and two baseline socket failures remain disclosed. Candidate-compatible
writers only: old parsers cannot read Soon history; no Bus v4/marker downgrade,
history rewrite or migration promise. Reject/revert before exposure; preserve
private candidate histories rather than downgrade after writes.

See `lightweight-management-next-s6g.md` for the single recommended next task:
explicit staged bundle bootstrap intent, positive persistence ACK and known-ID
recovery; unknown pre-ID outcome stays fenced, with no duplicate-create retry.
No implementation of that next task is authorized by this packet.

Resource before/after: 9.8GiB -> 9.9GiB retained under the owned root;
403GiB filesystem free before and after. Frozen default Cutex/facade are
separate from immutable S6f feature artifacts; native artifacts were reused
read-only. Owned child cleanup completed on success/failure. No unrelated
deletion, deployment, service restart or live Agent mutation occurred.
