# Selected34 rehearsal R3 — exact registry passed; registration policy blocker

Private verification only, not migration acceptance. No product source changed.
Base `342b5cd06d2165d75dfa6638d4d2e8d5077df9b0`, tree
`35648f92824648bb1470cec12502c92636d1e212`. Fixture exit correction `0211b09`.
Product remains `6e3b815ca13d8305fb267e30677ee73110e8a96d`, tree
`18cc305a49c340595f451b3e7218a953ae9fc9e3`.

Exact default-dev composed manifest: `../artifacts/selected-history-r2/build-manifest.json`,
SHA256 `726496f7160429e512c3be52470ba4fa44334e528e5a11d5b3e8375a3e891ad3`.
Cutex `279c12be5210c04a861fb9aa393eb2f770bc0b6eb83a9b701a632068319ec23d`;
facade `d3bacc0caabf3f1a92818c0c62b349c06c18470c1b8ed6a7818c6e9d3e96b929`.
Native CLI 8cde7956 / server 2eab060b / U host / schema 3fc00607;
Job f7bbe3c / ba1a8d4f, full component identities in that manifest. No rebuild.

## Exact groups and authority

`../history-r3-f91s2qf2/PASS.json` SHA256
`7271ff4fb75502c99976462fde8a3afcc4ec2442fd710ac39b251705797173f6`:
all34 supported adoption, groups-set, durable import/replay and project APIs pass.
Independent final reads match ALL34 original ordered group arrays and project
memberships. All35 records stay generation0/offline, including the explicitly
separate, nonlaunching vce Director prerequisite (no copied history). Fresh
private authority epochs/Task seats are reconstruction, not host epoch cloning.
Original 16 offline subjects remain inactive in this materialization store.

Reuse R2's fixed plan, all34 independent prefix equality, deterministic dry-run,
and five actual filesystem/race/interruption checks without another bulk copy.
The 34 selected histories remain separate from four quarantined damaged ones.

## Proven blocker, not a guessed timeout cause

First R3 failed runtime attempt `../history-r3-311fohz6`, cesc-tutor-r1:
`probe-0-run` returns HTTP500 `owner_action_required: stock durable configuration
changed during readiness; claim retained`. It fails BEFORE the PTY test.

Original/reviewed groups are `project:acc368b`, `project:822bf9a`, `cesc2`,
`worker`. Registration inserts `project:acc368bc`; reconciliation changes the
durable revision from review's 5 to 6. This is not a project-membership transfer.
Receipt stays spawned, generation1, not Ready. No success is inferred from spawn.

Exact current source chain:

- `src/agent_bus/groups.rs:17`: registration always adds the cwd-derived default
  when absent, even with a supplied nonempty group list; line40 calls it.
- `src/session/runtime_reconciliation.rs:242`: copies registration groups into
  the durable record and bumps revision on change.
- `src/agent_management/stock_runtime.rs:493`: Ready correctly rejects a changed
  review revision. This guard must not be weakened.
- `src/session/service.rs:100`: groups-set can restore exact groups before
  import, but cannot prevent the later registration mutation.

The R2 all34 normalization comparison identifies 29 affected records, including
both octobre subjects and the sole read-only subject. Do not rerun this known
failure or silently accept extra groups. A narrowly authorized product decision
is required: preserve trusted reviewed durable groups during marked-owner
registration/reconciliation, without weakening unmarked registration defaults,
membership authority or Ready CAS. No such product change is made here.

Independent readonly `native-oracle.json` in this failed root confirms same ID,
sol/high, read-only/never, paginated history and enabled memory; prefix unchanged,
zero appended bytes. It explicitly reports spawned and no PTY observation.

## Actual independent runtime coverage

GLM `tethys-director-r2`, `../history-r3-60pne22a`: real root review, activation,
Ready, historical read, same-owner CLI attach and NORMAL exit pass. PTY exit0,
no signal/forced termination, exact terminal restoration. Independent readonly
native catalog/prefix oracle confirms glm-5.3/max, full-access/never, legacy
history, enabled memory, original ID and byte-identical prefix with zero append.
Account/key paths are synthetic; this is not GLM entitlement/provider proof.

Second R3 failure `../history-r3-xn_3f0px`, inherited aemeath `scpolya-2`:
root review/Ready generation1 and actual thread/read pass (171 turns, including
80 compactions and 54 legacy inter-agent items, counts only retained). Catalog
independently confirms gpt-5.6-sol/max, full-access/never, legacy, enabled memory,
same ID and byte-identical 119MiB-class prefix. The CLI did not match the required
profile/Bon voyage/pink condition within90s: `reviewed historical CLI status not
rendered`. It nevertheless exits normally with code0, no signal or forced stop,
and exact terminal restoration (3622 screen bytes; no screen text retained).
This is a display-condition failure, not a proven native exit defect. Sanitized
owner errors show isolated-network catalog/MCP/WebSocket failures; they do not
prove why the status condition was absent. No third failing attempt is allowed;
no further runs were made. Do not label this full TUI success or a pj07 recurrence.

| Criterion | Actual result |
|---|---|
| All34 exact groups/membership/import replay | Pass, fresh private registry, no launches |
| All34 history/config materialization and filesystem negative gates | Reused R2 actual proof |
| Explicit GLM legacy Ready/read/PTY/status/normal exit | Pass, synthetic account/catalog only |
| Inherited aemeath legacy Ready/read/model/memory/prefix | Pass, independent catalog plus native read |
| Inherited aemeath normal exit/restoration | Pass, separately recorded despite display failure |
| Inherited aemeath expected visible status | Failed condition within90s, cause not established |
| Paginated/read-only cesc | Spawned metadata/prefix pass; Ready blocked by group mutation |
| Both octobre / remaining affected runtime variants | Not rerun: known registration policy boundary |
| Whole34 migration / real account / deployment | Not accepted, outside private result |

Immutable evidence anchors (relative to the task root):

- `history-r3-311fohz6/FAILURE.json`: `6e7d389966e924aef9d7763af600e6f0f191dd9a19072fb21b17df6beb2fbd8a`.
- `history-r3-60pne22a/PASS.json`: `d8f8b01523b48d243c088633a01db7c4556b5feaa9f4c9307c769fd73a41a47d`.
- `history-r3-60pne22a/native-oracle.json`: `26eea5d755f33c2c789b486efac53d884ccaf49479606f2a1ea047a483316082`.
- `history-r3-xn_3f0px/FAILURE.json`: `40a8e2c5d763c616dec7f61bab513a58ec11012ffc5b6a6f9264d4e8435cc2b5`.
- `history-r3-xn_3f0px/native-oracle.json`: `4afd7a25804908f0b5e3ae764bc57d00c7c3fb597273eea61b3bea6a79bdbb59`.
- `history-r3-xn_3f0px/pty-cutex.019f4b34-82e6-7f72-9027-34df7bdcb82e.json`: `5df3e13a05247759bdcd5c613013f218fb86c0b21d0e5c95a7bbd852953d9c51`.

The exit fixture now drains output while waiting and retains code/signal,
forced-termination flag, terminal before/after, screen hash/size, and sanitized
child error diagnostics on failure. Historical rendered text is not retained.
A separate real subprocess test writes 1MiB while exiting and proves drain plus
terminal restoration. GLM success does NOT retroactively establish the R2 exit
failure cause and is unrelated to deferred pj07 restart risk.

## Tests, disclosures and next boundary

Offline PTY drain test and eight synthetic error-redaction tests pass. The
postmortem oracle initially assumed Ready and an older sandbox serialization;
readonly failures were retained in the work transcript, then corrected to
report spawned separately and accept the exact native `PermissionProfile`
disabled representation of danger-full-access (`protocol/src/models.rs:673`).
These diagnostic failures were not runtime attempts or permission changes.
Four changed fixture AST checks and scoped diff check pass. Self-review covers
the complete R3 fixture diff, allowed modes, exact group check, failure/cleanup
paths and readonly oracle. No product changes or new unit/build claim; R2
product tests/build and accepted native decoder/auth/status tests are reused.

No model input/turn or Job submission. Native startup can attempt catalog/PS
MCP/WebSocket connections; they fail in the isolated network. No successful
external provider traffic, real credentials, live profile/service changes or
VM actions. Nested-userns warnings do not establish a new sandbox guarantee.
Tracked owned children and private namespaces are stopped; histories, synthetic
auth and evidence remain. Old failed R2 roots are unchanged.
Owned root remains19GiB; final free check357635043328 bytes, above100GiB. No
cleanup to meet quota, no new whole34 copy. Representative copies are separate
single-link files; private native derived indexes are the only native catalog
writes. No original history/catalog/config is modified.

Rehearsal remains incomplete pending the exact-group registration decision and
affected representative proofs (especially octobre and read-only normal exit).
No generalized authority redesign is needed. Real auth ancestor0775 maintenance,
active-role coordination, one operational snapshot and actual cutover remain
separate. No backup was taken; old writers are not a rollback reader for new
stores/history. Offline subjects must remain offline; retired subjects never
revive. No migration or production release claim.
