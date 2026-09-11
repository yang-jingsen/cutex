# Reviewed registration groups — repair proven, composed display still blocked

Base9d108802ce0716a9831b5f799f0d6c598491a618/tree90088fbd20d9d638e67e9a82665dc8126b3883b8.
Initial product4460ac4b5add23e45d2fb05d9a3d17d03a7c1e45 and its failed run remain
immutable. Final product7e51bab702d06bb919c5d41093b4fdcb86abac9f,
tree2f500a5e624c26664ea0e951322aacb815d75efc; no native/Job change.

Final default-dev manifest `../artifacts/reviewed-groups-r1/final/build-manifest.json`,
SHA256 `9a6e59805366ed13b050c06d5b3deb9f8a1b04eb1d6ee6649aa8517ae3d4527d`:
Cutex `17fe57df0de82da7537cee39eec16baed5d8bc8e55d351cda402a07c17b6593b`,
facade `a95631ed6ec264963ed6abe203db930f7b475113d290a4c7b1575e91a615c9c0`.
Native CLI8cde7956/f66dd912, server2eab060b/15c72a6b, Uhost3e85d674,
schema3fc00607; Jobf7bbe3c/ba1a8d4f. Full component hashes are in the manifest.
This is not an optimized release or live deployment.

## Boundary and tests

See [security/compatibility contract](reviewed-registration-groups.md).
No caller flag, public request field, store version or receipt rewrite was added.
The authenticated registration callback restores exact authoritative reviewed
groups only after receipt/marker/revision/current occurrence matching. Wrong
groups/PID/host/native/runtime identity and stale claim/generation fail closed.
Pinned bundle and process checks surround reconciliation; original store CAS
and Ready configuration/revision fences remain. The same corrected agent enters
the roster, preventing later heartbeat reconciliation from reintroducing defaults.
Unmarked default behavior remains unchanged.

13 registration tests pass on final product, including three new group tests,
existing retirement/CAS/roster visibility races and registration-only regression.
The new tests include ten bad owner/group/state variants and Ready refresh.
Existing runtime-review digest test passes: only the two approved telemetry
fields are excluded, not revision or semantic changes. Actual intentional
configuration-change rejection is unit-level/state-machine evidence here, not
a new private crash/restart campaign. No Ready guard code changed.

Initial broader registration test runs preserved failures because an existing
Task-context test requires private HOME, matching CUTEX_TEST_PRIVATE_HOME and its
marker. All requirements were provided in an owned new test directory; no
guard was relaxed. Logs: `reviewed-groups-tests.log`, `-private.log`,
`-exact-home.log`, `-final.log`, and final-product `-tests-r2.log` under task root.
This is test setup failure, not a native runtime failure.

Both default builds pass; four scoped Rust formatting checks, fixture AST,
synthetic PTY drain/termios test and diff checks pass. Full task-scoped source
self-review includes callback projection, same-store CAS, unmarked behavior,
process verification, bounded timeout and failure cleanup. No full suite or
native rebuild. Full bundle hashing still costs time under the existing roster
lock; no throughput or hostile-same-UID guarantee is claimed.

## Preserved first runtime failure and revised scope

`../history-groups-5iwfj0hi` on initial4460ac4 default bytes stopped at spawned:
`readiness incomplete: failed to register app-server runtime on the agent bus:
Failed to read agent bus response: Resource temporarily unavailable (os error11);
replay exact action`. It hit the old5s registration client response deadline
on the new callback path, which validated the entire bundle twice. Detailed
per-hash timing was not retained; the precise timeout receipt is retained below.
No registration completion or PTY success is claimed for that failed run.
This is one consumed failing real attempt; no counter reset in R2.

Correction validates the bundle once per callback, retaining process checks
before/after reconciliation, and uses one bounded120s registration HTTP request.
Other client deadlines and TUI90s are unchanged; no automatic retry/fallback.
Old build/manifest/failure retained, not relabelled fixed.

Human then deferred cesc because the real agent is working. No new cesc attempt
was started; its earlier isolated namespace/owned children had already exited.
No real cesc process/state was accessed or changed. Cesc-specific/read-only
composed proof is explicitly deferred, not replaced by full-access results.

## Final-byte non-cesc outcome

One final run `../history-groups-kkih9pyt`, fixed-cohort non-cesc
`cute-codex-log-wal-fix-r2`, native ID `01a074d9-7e48-7663-8aed-514ee44db3a1`.
Its project is cutex-stack-main in the reused all34 membership proof. The runtime
probe is a separate private copy, never the actual working agent.

| Criterion | Observed final result |
|---|---|
| Exact reviewed group order/revision | PASS: all7 original groups, revision6 before and after; no cwd group added |
| Real root review/activation/native readiness | PASS: Ready/current generation1, original native/durable ID |
| Actual valid re-registration | PASS: original groups and current occurrence unchanged |
| Wrong token/group/PID/runtime/thread/host | PASS: six actual requests rejected before roster/durable mutation; current HTTP500 error envelope retained |
| Unmarked default | PASS: actual private unmanaged registration retains ordinary group plus normal default |
| Native preserved history/read | PASS: actual thread/read2 turns; compaction/legacy inter-agent/command/file-change item types observed, bodies not exported |
| Independent readonly catalog/prefix | PASS: octobre explicit, gpt-5.6-sol/max, full-access/never, enabled memory, paginated, same ID/cwd, byte-identical prefix, zero appended bytes |
| TUI normal exit/restoration | PASS: code0, no signal/forced stop, exact terminal restored |
| Required visible status | FAIL: Bon voyage=false, profile label=false, pink RGB=false after90.139s;3645 bytes observed |
| Finite private UI state | Static tail markers Trust/trusted/Press matched; no automatic trust confirmation, no causal product claim from keywords alone |
| cesc/read-only, scpolya | DEFERRED: no new cesc or scpolya probe; Human instruction preserved |
| Full composed rehearsal / migration | NOT ACCEPTED: visual gate remains |

Thread/read completed in0.048s; observed notification methods were
deprecationNotice and remoteControl/status/changed. No turn was sent. Failure
phase is probe-0-attach with `reviewed historical CLI status not rendered`.
The second failing real attempt exhausts the combined R1/R2 allowance. No third
run, enlarged timeout, trust grant, feature toggle or fallback followed.

Evidence SHA256 (relative to task root):

- `history-groups-5iwfj0hi/FAILURE.json`: `3ea2f6ee65e43c890edfb9d3ba17f2f3d69ca818d97e209be895cbf949e0e57c`.
- `history-groups-kkih9pyt/FAILURE.json`: `f4fc56a7d3af74c6a43452542e760f19eab3fbc075a4532e7ebcdc97e8a54aee`.
- `history-groups-kkih9pyt/registration-oracle.json`: `d0e5aab167dafe12aa884fe68b41f01f86aba308683af6f286cdd43633e7bbbf`.
- `history-groups-kkih9pyt/native-oracle.json`: `2dba7897731109fa61bfb582ff312714914bdafc291beda789974219b0b134d9`.
- `history-groups-kkih9pyt/pty-cutex.01a074d9-7e48-7663-8aed-514ee44db3a1.json`: `faf90d27ef8fc4e72ab95f35b79032331f429e7c1c03c1fbb03982dd34a7310b`.
- `history-groups-kkih9pyt/read-01a074d9-7e48-7663-8aed-514ee44db3a1.json`: `3a001bf5acb6ecf719290c20311c15bb58dbcae261d3d8299c0f4e18e5f629f1`.

Smallest remaining verification decision: inspect the retained non-cesc UI-state
evidence and supported onboarding/config layers read-only, then authorize any
needed bounded manual/PTY observation separately. Do not infer a trust repair
from these three words, and do not make cesc available merely for coverage.
The registration-group repair is independently proven; it does not make the
whole migration ready. No current reusable terminal is left running: owned
processes and PID/network namespaces exited, evidence and synthetic auth remain.
Task root19GiB, final free357282181120 bytes; no cleanup to meet quota.

## Reused evidence and remaining scope

R3 all34 exact ordered groups, memberships and API import/replay; R2 filesystem
no-overwrite/symlink/interruption/race and all34 prefix equality; native legacy
reader/auth/status and R3 GLM normal-exit proof are reused, not rerun. Project
membership proof belongs to the all34 private registry; runtime probes are
separate synthetic-account stores, not reconstructed live authority epochs.
All34 source offline intentions and the nonlaunching outside-cohort vce
prerequisite are unchanged. Four damaged histories remain quarantined.

Native scpolya report `artifacts/inherited-history-status-r1/RESULT.md` SHA
`d4190216be5c2ef5c24fc999fb33881912cdba69bdda1d2e38ed8934b5612cd5` is independent
AUTHORITATIVE_INPUT. It does not resolve the original AND=false display cause
or prove original full composition. No scpolya run, trust fix or timeout increase
was performed here. Later probe observation records each text/color condition
and finite static UI-tail markers, never exports historical conversation text.

No model turns/Jobs/successful provider traffic, real auth read/copy/chmod,
live services/PRH/VM/Windows/backup/migration/canonical changes. Synthetic auth
and isolated historical copies are not real-account acceptance. Private native
startup network attempts fail inside the isolated network. Existing seconds-PID,
pj07, S2/S46 disclosures and old-writer/history rollback limits remain.

Still separate: real0775 auth custody maintenance; coordinated active-role
cutover and offline preservation; one operational snapshot (not captured here);
actual selected migration/release decision. No production authorization.
