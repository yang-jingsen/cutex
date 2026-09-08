# Durable Agent import: decision and implementation packet

Assignment: `projects-durable-agent-import-r1-assignment`.
Exact base: `fed5e05f65c0921551b4973bb7475908141347f5`.
Status: implemented; independent acceptance review required before integration.

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
existing private Human principal. Typed requests bind the full candidate snapshot,
exact durable ID, confirmed formal name, action, destination and optional explicit
source detach. Selection and cancel do not mutate durable or roster state.

Lock order is provider execution, provider mutation, durable session store,
provider state. A durable fence covers the roster commit. Candidate CAS includes
durable revision/digest, roster record and current membership/operator context;
runtime liveness is informational. Existing project handlers retain source and
destination epoch/revision CAS and Director, seat, Operator/task protections.

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
