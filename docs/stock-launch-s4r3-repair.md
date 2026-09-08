# S4r3 request-changes repair

Exact parent/reference: `1f039dcf58efa2958270e192f782f2203b708919`, tree
`6e92994827b089b596deb43feed26b3e60db5a9b`. The commit containing this report is
its descendant, not a replacement of that immutable result. Scope is the named
pre-binding recovery window and classification of two socket tests only.

## Recovery boundary

The existing root-Human action, provider/Task locks, durable CAS, reviewed config,
native/durable identities and explicit-launch requirement remain authoritative.
The runtime receipt now optionally carries `publication {path, device, inode}`.
This identifies a private, owner-only kernel flock witness, not another identity
store or a sidecar authority. Missing, replaced, symlinked, foreign or busy evidence
fails closed. The lock file must remain available while a receipt needs recovery.
Old final receipts omit the new optional field and retain their historical shape.

One child is forked in the existing Management owner. Before fork, command strings,
environment and descriptors are prepared. The child performs only async-signal-safe
syscalls, creates its session/process group, closes inherited descriptors except
stdio and the gate/lease, and waits. **It cannot execute stock until the provider
has committed its exact PID/start/binding under the original claim.** On release,
that same PID execs the verified stock binary; there is no extra owner, helper
daemon, supervisor, kernel patch or changed default launch path. Linux
`close_range` support is required; unsupported setup fails before native exec.

The child inherits the same flock open-file description. Creator death closes the
gate; an unreleased child exits without native execution and releases its lease.
An exclusive acquisition of the receipt-bound inode therefore proves there is no
unpublished child still able to execute. Replay reuses the **same action, claim,
runtime ID and expected generation**, with current authority/configuration/CAS
checks; it never guesses a PID or clears the explicit-launch marker.

Caught precommit errors stop/reap only the invocation's owned child. The receipt
remains Claimed, but is now recoverable by the same action after lease acquisition.
If the creator dies just after binding publication but before gate release, replay
also handles that unregistered occurrence: exact PID/start absence (or zombie),
no live stored Unix endpoint, matching lease and durable CAS permit replacing only
that old binding. A reused PID is never signalled. Live or ambiguous ownership
still refuses. Process-group cleanup is unchanged in scope; no arbitrary escaped
descendant or cgroup guarantee is added.

Residual boundaries are deliberate: legacy uncertain receipts without publication
evidence cannot be retroactively proven absent; corrupt/missing evidence, changed
authority/configuration and already-registered-but-dead uncertain occurrences do
not get guessed recovery. No general marker/claim-clear, downgrade, migration or
legacy-writer compatibility is added. Prepared-stage uncertainty after a prior
destructive stop remains separately fail-closed; this repair targets the requested
published-claim/child window, not every possible lifecycle crash.

## Focused executable evidence

All paths below are under `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
Private dummy profiles, fake Responses (one bootstrap response per mode), real
stock 0.153.4 and real private Cutex HTTP/provider persistence are used. No paid
calls, production credentials/stores/services or live Agent actions.

| Criterion | Evidence |
|---|---|
| Caught precommit failure, same-action recovery | `s425/PASS.json`; extended negatives in `s428/PASS.json` |
| Busy/missing/replaced lease rejected by actual provider, receipt unchanged | `s428`; original owned inode restored only by the fixture before valid replay |
| Creator dies before binding commit; same claim reaches Ready | `s426/PASS.json`, actual Management `_exit(86)` with no Rust cleanup |
| Creator dies after binding commit/before exec release | `s427/PASS.json`, actual `_exit(87)`; unregistered exact binding recovered |
| No native exec before commit/creator death; live lease rejects; same-lease replay | Default-feature bin-test executable: actual creator SIGKILL and the production gated-child adapter; native shell marker appears exactly once only after release |
| Stable native/durable IDs/history, generation 1, unrelated process survives | Each private recovery mode reads actual native history and provider state |
| Default-feature production executable, no injection hooks | `s429/PASS.json`: actual stock reaches Ready, original native history unchanged, exact replay stable |

Crash locations/errors are controlled test hooks, but creator death, inherited
descriptor/lock behavior, child exit and provider persistence are real. The
default-feature process oracle is independent of those hooks. It reports two
passing test entries: one actual parent oracle plus its subprocess fixture, **not
two independent crash proofs**. It also checks missing/replaced/busy evidence.
Native stock/PTY/MCP/sandbox evidence from `s424` and the prior result is reused,
not rerun as a full campaign. No inbound/A4/visible-only/renderer work was added.

Commands use locked Cargo, scrubbed environment, owned HOME/cache/target/tmp:

- `cargo build --locked --bins --features stock-launch-test-hook` for narrowly
  injected provider boundaries (`s4r3-repair-build3.log`).
- `cargo test --locked --bin cutex stock_publication -- --test-threads=1`
  (`s4r3-repair-process-final.log`), default features.
- `python3 tests/stock_launch_boundary.py NAME MODE`, with MODE
  `commit-recovery`, `creator-recovery`, or `published-recovery`.
- `cargo build --locked --bins` passed (`s4r3-repair-default-build.log`), followed
  by `default-publication` mode (`s429`); no test-hook dependency is required by
  the launch path.
- `cargo fmt --all -- --check` and `git diff --check` passed. Complete repair
  delta self-reviewed, including fork/descriptor/lease ownership, exact CAS,
  default-feature path and failure-stage handling. Existing warnings retained.

Failure evidence is retained: an initial moved-binding compile error was repaired;
the inode-negative test initially failed at its stricter file-mode guard and was
corrected to exercise a private-mode replaced inode. Sharing a target with the
baseline comparison exposed stale package artifacts; only reconstructible Cutex
package outputs (4.6 GiB) were removed with task-scoped `cargo clean -p cutex`, then
rebuilt. No source, unique result, prior failure evidence or stock artifact was
deleted. These failures are not recounted as passing checks.

## Socket failures: classification closed, not relabelled green

The named two production Management tests still fail at the existing 100-byte Unix
socket guard in the permitted Mambo root. A shortened fixture measured 108 bytes;
production geometry was not changed, and the fixture experiment was reverted.

The exact immutable parent was checked out in owned `socket-baseline` and compiled
with the **same** toolchain, HOME, TMPDIR, flags and target environment. Running
only `cli_app::management_context::tests::production_` selected exactly two tests.
Both failed with the same `app-server Unix socket path is too long` error:
`s4r3-socket-baseline-tests.log`. The repair-side comparison selected the same two
and failed identically: `s4r3-socket-repair-comparison.log`. This is now executable
baseline evidence, not an inference from source inspection. Classification:
existing fixture/environment limitation, not a recovery-delta regression; neither
test is claimed passed.

## Handoff limits

Future private lightweight integration candidate only, subject to Director
acceptance. Full power-loss/filesystem-corruption recovery, post-registration dead
owner policy, production OAuth/mixed writers, Windows/Android, arbitrary escaped
descendants, live cgroups and production migration remain outside this proof.
Both prior security incidents remain as disclosed in the original result; no
original sensitive output was reread and no remediation/no-impact claim is made.
All probe-owned children are stopped; private evidence and lock witnesses remain.
Reject/revert the candidate before exposure; that is not a store downgrade API.

Final resource observation: 5.8 GiB retained in the owned task root, 454 GiB free
on Mambo (repair began at 7.6 GiB/452 GiB free). All new source, baseline fixture,
build/cache/temp and evidence stayed there; no home cache, new stock download,
dependency/lockfile change, remote/canonical write or deployment.
