# Durable Agent import: decision and implementation packet

Assignment: `projects-durable-agent-import-r1-assignment`.
Exact base: `fed5e05f65c0921551b4973bb7475908141347f5`.
Status: implemented; independent acceptance review required before integration.

## Bounded candidate-review repair

Repair assignment: `projects-durable-import-repair-r1`, exact repair base
`d60feac5a4ee3e454346c12676378f4a7d220db3` (tree
`9c31521ad41e59daf58cd09c622bc593e1a0d0cc`). Original base remains unchanged.

Candidate responses now carry `raw_store_key` and optional validated
`cutex_session_id`. Malformed keys produce individually rejected rows without
inventing an ID or suppressing valid rows. The picker excludes unvalidated IDs
from selectable choices; Create also displays their raw-key rejection reasons.
Typed rejected candidates cannot pass confirmation. Duplicate durable native
identities are marked rejected during query as well as fenced at commit. Whole
store read/parse failures remain errors. Pre-repair requests lacking raw_store_key
retain their exact serialized shape and digest for receipt replay.

All four provider project list/detail readers and fresh Agent QueryManaged now
share `current_name_snapshot`. Production `open_default` binds the durable store;
isolated providers use an explicit `with_current_names_path` adapter. It projects
formal_agent_name by exact durable ID into a read-only roster snapshot, falling
back only when the dedicated field is absent. Missing/key-mismatched durable
records or malformed formal names report an observation error, never a title.
The projection observes the roster snapshot as a whole, so unavailable durable
evidence for any roster record can fail a current read. No historical receipt,
digest or roster entry is rewritten. Replaying an original action returns its
original name; a fresh query uses the current name. Human-only duplicate overlay
code was removed. No cross-store rename transaction or authority changes added.

Repair verification: Management library suite 113 passed; affected binary
Management suite 55 passed (two previously disclosed user-systemd cases skipped);
Projects HTTP/TUI suite 13 passed. New cases cover mixed/all-invalid candidates,
duplicate native IDs, corrupt-store failure, root HTTP serialization/disabled
malformed selection with working Create/Add, exact pre-repair wire replay, explicit
rename across all provider readers, same-name/different-ID records, legacy formal
fallback and failed observation without relabeling replay. Prior broad evidence
is reused, not rerun solely for certainty. Temporary-state/simulated boundaries
and the four original user-systemd omissions remain unchanged.

## Identity, naming and eligibility

Identity is the exact validated durable store key and `cutex_session_id`.
Director-approved `formal_agent_name: Option<String>` is authoritative.
Explicit creation, adoption with a name, and rename write it. Recent adoption
without an explicit name preserves the thread title only as presentation metadata.
Historical unnamed records require explicit Human name entry in confirmation;
no title, display hint, cwd or global profile is inferred. Existing roster
records can fall back to their formal `spec.name`. Project and Home projections
read current durable formal names by exact ID, so rename does not change identity.

Eligible records are active, nonretired, enabled Persistent Agents with valid
revision and matching key/ID, a local host, supported Host/HostForeground/CuteAlden
backend, valid native session ID, canonical nonempty groups, usable absolute
nonroot effective launch cwd, no pending launch claim and valid present config
strings. Hidden quick actions, Ephemeral/LocalOnly, unsupported backend/remote,
malformed configuration and retired roster records fail with reasons. Duplicate
native ownership is rejected. Online runtime is not a prerequisite; the
Management adapter separately joins exact current runtime identity/liveness.

Director-approved nullable `ManagedAgentSpec.profile` retains durable intent.
Create still requires an explicit nonempty profile. Existing JSON strings remain
compatible. Lifecycle validation does not use profile as identity. Other absent
runtime configuration values follow the existing observation representation,
and the receipt retains the complete source record. Imported records have null
historical creation project/Director provenance, not fabricated owners. Current
membership is explicit and authoritative, including an unassigned import.
New null-bearing records require updated readers; mixed-version/downgrade
compatibility is not claimed.

## Typed authorization and transaction boundary

Affected code: `agent_management/durable_import.rs`, model/store/projects/provider;
session model/defaults/service/store; Management v2 server/context and client;
Projects TUI and Home/name projections.

Dedicated Management-root-only candidate GET and import POST routes mint the
existing private Human principal. Typed requests bind the reviewed semantic
candidate snapshot, exact durable ID, confirmed formal name, action, destination
and optional explicit source detach. Selection and cancel do not mutate durable
or roster state.

Lock order is provider execution, provider mutation, durable session store,
provider state. A durable fence covers the roster commit. Candidate CAS includes
durable revision/digest, roster record and current membership/operator context;
runtime liveness is informational. Existing project handlers retain source and
destination epoch/revision CAS and Director, seat, Operator/task protections.

The durable digest is a versioned typed projection of stable reviewed facts:
durable/native identity, revision, archive/retirement and unresolved launch-claim
state, formal name, host/cwd, nullable profile and remaining launch/config policy,
enabled/groups/registration/exposure/quick-action settings. It deliberately
excludes native title/display hints, runtime endpoint/PID/generation bindings,
last-seen/user-observation fields and timestamps. Those fields do not authorize
import and can change during ordinary registration while the review is open.
Roster/Agent/membership/operator digests remain separate fences. A profile is
mutable reviewed configuration, not identity: a change before confirmation still
requires a fresh review, while a completed action replays its original receipt.
Candidates generated with the former whole-record digest fail closed after this
change and must be refreshed; no digest-version compatibility bypass is provided.

A confirmed composite has its own semantic request digest and receipt. Historical
naming is separately receipted atomically with the name, including previous/new
revision and timestamps: replay cannot mistake an identical external rename for
its own completed step. Import atomically stores the unassigned roster record,
source snapshot, receipt and audit. Explicit Detach then Create/Add use existing
project transactions and per-step receipts. Partial failure reports incomplete,
preserves recovery evidence and never reports successful assignment. Exact
completed replay returns its original receipt; changed-action replay conflicts.
Partial recovery fences intervening name/profile/config/membership/retirement
changes. No raw old-state repair or legacy ownership inference is used.

## Verification and boundaries

Focused coverage uses real provider, Management authentication/HTTP routing,
client and production TUI key/confirmation handlers with temporary durable stores.
Tests cover actual adoption, offline Create/Add, historical naming, cancel,
wrong auth, profile/name/cwd nonidentity, stale snapshots, replay conflicts,
retirement, unsupported records, injected naming/assignment interruptions,
identical external rename, explicit cross-project move, guarded Director and
Operator/task cases, authoritative rename projections, nullable profile and
ordinary lifecycle after import. Runtime lifecycle effects are simulated.

The final provider Management suite passes 110 tests. Earlier broad library run
passed 697 tests; its eight TMPDIR-dependent failures all passed after supplying
isolated TMPDIR. Binary regression runs isolate HOME and runtime discovery.
Four existing user-systemd-dependent binary tests require a live user service
environment and are excluded from final isolated verification; no DBUS/XDG
connection to the production user manager is supplied. Exact final command
results are recorded in the Worker result packet.

No production acceptance claim: no live sessions, roster/project edits, runtime
launches, canonical refs, push or deployment. Independent boundary review and
Director acceptance remain required.

Suggested shared TODO: Durable Agent import implemented on isolated task commit;
formal-name provenance, nullable profile/creation provenance, root-only confirmed
Import + Create/Add/Detach, fenced recovery and focused regressions complete.
Pending independent authority-boundary review and Director integration acceptance;
four live-user-systemd regressions intentionally outside isolated test scope.
