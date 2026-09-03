# Agent Management v1 native provider boundary

Agent Management project authority comes only from the authenticated runtime's
current durable Cutex session and the Agent Management provider store. Agent Bus
groups are collaboration and discovery labels. Neither `CUTEX_AGENT_GROUPS` nor
any `project:*` group grants, selects, or implies Agent Management authority.

## CLI contract for the cute-codex 0.150 consumer

Invoke exactly one operation with:

```text
cutex agent manage <operation> --request-file <private-json-path>
```

`<operation>` is one of `create`, `query-managed`, `online`, `offline`,
`restart`, `close`, `replace`, or `director-rotate`. The request file must be a
private regular file. The caller process supplies its ambient `CUTEX_AGENT_ID`;
normal Cutex Agent Bus URL and token configuration authenticate that live
runtime. The consumer must not read or interpret `CUTEX_AGENT_GROUPS`.

The request is strict Agent Management v1 JSON:

```json
{
  "schema": "cutex/agent-management/v1",
  "action_id": "stable-action-id",
  "project_id": "optional-project-selector",
  "operation": "query_managed"
}
```

`project_id` may be omitted. It is a selector, not an authority claim. Caller
session, runtime, seat, Director, and authority-epoch claims are forbidden in
this model-authored document. Operation-specific fields retain the existing v1
shape.

The CLI writes one `cutex/agent-management/v1` response as compact JSON to
stdout. A service-level denial is a successful CLI exchange with a typed
`no_write` outcome. Local file, transport, or response-decoding failure exits
nonzero. Complete receipts retain their concrete `project_id`, canonical parsed
request SHA-256, operation, result, and normal Agent Management v1 identities.

## Project selection

After Agent Bus authentication, Cutex resolves the runtime Agent ID to one
active, non-retired durable Cutex session whose current runtime occurrence is
the sender. It then reads the Agent Management provider store:

- one authorized project plus no selector selects that project;
- multiple authorized projects require a selector;
- a selector must name a project currently authorized to that caller;
- no authorized projects denies ordinary Workers and retired Directors;
- an exact existing action replay retains its originally resolved project while
  still requiring the caller to hold current authority for that project.

Selection failures do not reserve an action or mutate provider state. Stable
typed codes include `stale_runtime_identity`, `not_authorized_director`,
`project_selection_required`, and `project_not_authorized`.

## Crash-safe predecessor closure

Replace with `close_before_create` or `close_after_ready`, and Director Rotate
with `close_predecessor_then_create_with_message`, durably commit an exact
`predecessor_closing` intent before the provider invokes the external close.
The intent remains bound to the original action digest, project, expected
predecessor, successor reservation, policy/mode, and current Director
authority.

If the process dies after external retirement but before
`predecessor_closed`, exact replay observes only that named managed predecessor.
It requires the complete durable/native identity, managed specification,
positive runtime generation, inactive lifecycle state, and absence of every
runtime and Agent Bus endpoint. The provider then CAS-retires the unchanged
ownership record and advances the original action. Partial, mismatched,
cross-project, authority-changed, or externally unknowable state fails closed.
For `close_after_ready`, replay validates and reuses the already-created exact
successor. It does not bootstrap again or resend the idempotent handoff.

## Coupled Director authority rotation

`director_rotate` owns both Director authority surfaces: the Agent Management
project authority and the Task Service `cutex-director` seat. Before closing a
predecessor or creating a successor, the provider reads the durable seat store
and requires its current occupant to be the request's exact expected
predecessor. A missing seat, a different occupant, or a transfer-action identity
already used for another payload fails closed before launch. This preflight does
not infer authority from Agent Bus groups, chat identity, or ambient state.

After the successor is ready and any frozen handoff message has been delivered,
Agent Management transfers the seat with an internal compare-and-swap request.
Its action identity is a domain-separated SHA-256 derivation of the immutable
`director_rotate` action ID. The seat operation accepts only the expected
predecessor or an exact durable replay already naming that action's exact
successor; the general administrative seat bind is not used as the rotation
primitive. While the project-authority boundary is pending, the seat store
durably fences unrelated administrative rebinds. Agent Management then commits
the project-authority CAS and releases that fence. Close-predecessor and both
retained-predecessor modes preserve their existing lifecycle and message
behavior around these authority steps.

If the process stops after the seat CAS but before project authority commits,
the original action remains at `authority_transfer_pending` with its exact
successor identity, while the seat store retains the transfer receipt and active
boundary fence. Exact replay verifies that evidence, reuses the successor
without another bootstrap or message, commits project authority once, and
finishes the fence. A completed response is returned or replayed only after the
stored project authority and current Task Service seat both name the receipt's
exact successor. Pre-existing or later unrelated mismatches are reported
without rebinding either surface; recovery never falls back to administrative
bind or silently heals a different action.

## Minimal downstream change

The cute-codex 0.150 handler should remove ambient-project discovery and all
`CUTEX_AGENT_GROUPS` parsing. Keep `CUTEX_AGENT_ID`, the existing argv operation
mapping, private request file, timeout, and typed response handling. Add an
optional `project_id` selector to the native tool/provider request; omit it for
the common one-project Director. Continue to reject model-supplied caller,
runtime, seat, Director, or authority claims.

The handler can still compute `request_sha256` from its stable serialized
request bytes. For an explicit selector, require the receipt/failure project to
match it. For implicit selection, accept one syntactically valid concrete
project from the typed provider payload and require that same project
consistently within the payload; the authenticated Cutex provider is the source
of that resolved value.

## Root-only legacy Director ownership import

A project whose authority predates Agent Management may have an authorized
Director but no `ManagedAgentRecord`. An Owner may import exactly that missing
record once through the dedicated Management root credential:

```text
cutex management agent-ownership-import \
  --request-file /private/path/legacy-director-import.json
```

The request is closed JSON and contains only stable action identity plus the
project-authority CAS:

```json
{
  "schema": "cutex/agent-management/legacy-director-ownership-import/v1",
  "action_id": "legacy-director-import-tethys-r2-01",
  "project_id": "tethysune",
  "director_cutex_session_id": "cutex.tethys-director-r2",
  "expected_authorized_director_session": "cutex.tethys-director-r2",
  "expected_authority_epoch": 1
}
```

Before the atomic Agent Management write, the server loads the exact durable
session key and validates active lifecycle, local host ownership, native
session identity, canonical persistent-Agent state, and every field of the
managed spec. Those facts are never accepted from the request, Agent Bus
groups, cwd hashes, runtime IDs, or prose. The operation leaves project
authority unchanged. Its typed receipt records the authoritative session
revision and runtime generation used as evidence. Exact action replay returns
the immutable receipt with `replayed: true`; changed-payload reuse and every
identity, authority, lifecycle, or ownership conflict fail closed.

This route is authenticated by the dedicated Agent Management root credential,
the same administrative credential class as `management agent-authority`. It
is not an `AgentOperation`, is not registered in the Director/Worker tool, and
does not expose a general adoption mechanism.

## Exact create retry before native session creation

Create keeps its original action ID, request digest, project, reservation,
specification, groups, and message identity when profile/bootstrap preflight
fails before a native command can be spawned. The production lifecycle now
validates the selected profile and its credential/config files in-process. A
failure at that boundary is definite: no native SID or durable Agent can have
been created. The action records a `native_bootstrap_retryable` permit and does
not cache the `owner_action_required` response as terminal.

An identical request from the same still-authorized project Director consumes
that permit in the durable store before attempting startup again. This
consume-before-launch fence makes concurrent exact retries and service restart
safe: at most one retry can launch, and a crash after permit consumption fails
closed instead of launching again. Success continues the original action and
uses the original idempotent message identity. A changed payload conflicts and
a fresh action remains blocked by the unresolved reservation.

Future native bootstrap uses `codex exec --json`. The provider accepts a SID
only from the exact JSONL event `{"type":"thread.started","thread_id":"<uuid>"}`
on stdout. UUIDs in profile diagnostics, stderr, or any other structured event
are ignored. Missing, conflicting, malformed, or non-UTF-8 structured output
fails closed. A nonzero command that emitted one valid creation event retains
that exact SID for the existing captured-session continuation and never
launches a second native session.

R23 also recognizes the exact historical `native_session_unknown` receipt
shape emitted when the old wrapper completed nonzero without printing a SID.
That receipt text is only a reconciliation trigger, not proof of absence. Before
upgrading it, the lifecycle provider must read the Agent Bus registry, durable
Cutex session store, the direct cute-alden session registry, native session
index, and native rollout tree. Runtime evidence is correlated with the launch
profile, runtime name, exact requested groups, deterministic cute-alden name,
and reserved managed cwd as available; those markers are evidence only and
never grant authority. Native index and rollout records are merged by native
session ID so an explicitly unrelated cwd in the same attempt window is
ignored, while an index-only record without a cwd marker remains ambiguous.
Reconciliation selects the launch profile's actual Codex home: the aggregate
host Codex home for a host profile, or the host-mounted container `.codex` home
for a Docker profile. Rollout filenames are local wall-clock discovery hints,
not attempt-time proof. Inclusion uses the embedded RFC3339 creation timestamp
and enclosing `session_meta` event timestamp; both must be present, valid,
consistent, and inside the attempt window. Missing, malformed, conflicting, or
window-straddling embedded timestamps fail closed.

Only complete absence across every provider-owned source authorizes retry.
Exact present evidence, ambiguous/mismatched evidence, malformed evidence, and
an unavailable source remain fenced without launching. On exact replay the
provider projects a transient typed owner-action code ending in `present`,
`ambiguous`, or `unavailable` with a concise reason. The original action,
response, and failure event remain byte-for-byte unchanged in the durable
store. The original caller must still be the current project Director;
immutable completed/no-write receipts and all other owner-action-required
results remain unchanged.

The exact historical `native_session_ambiguous` receipt produced by legacy UUID
scraping has a separate no-relaunch continuation. Under the current project
Director's authority, the provider reads the most recent durably journaled
`native_bootstrap_pending` attempt window and reconciles Agent Bus, durable
session, deterministic cute-alden, and selected-profile native rollout/index
evidence. Only one valid UUID that agrees across all present sources may be
atomically recorded as `native_session_captured`; the same action then adopts
that existing thread. Zero, multiple, conflicting, malformed, stale, or
unavailable evidence returns a transient typed reconciliation fence and never
authorizes another bootstrap. Concurrent replay and service restart converge
on the captured SID, original request digest/reservation/spec/groups, and
original message ID. The historical failure event remains immutable.

## Cutex Projects read and presentation projection

`AgentManagementProvider::list_cutex_projects` and `read_cutex_project` expose
the bounded workspace projection for an authenticated Agent Management
principal. The provider store's exact `ProjectId` key is the sole ownership
identity. Reads require the caller's durable Cutex session to occupy the
current Director seat and return only that exact project's authority epoch,
Director, active durable Agents, retired durable Agents, and runtime
observations where available. Groups, cwd, names, runtime IDs, and native Codex
workspace records never select a project or grant access.

Project presentation is a separate non-authoritative record keyed by canonical
`ProjectId`. It contains only a display name, a one- or two-cell badge, a color
from the finite high-contrast palette, update provenance, and an independent
revision. Missing presentation records produce deterministic defaults without
writing. Only the authenticated current Director can update presentation, and
the canonical ID and authority fields are absent from the editable payload.
Optimistic presentation revisions prevent lost updates. Additive store and
presentation fields survive presentation-only writes.

The terminal UI presents this projection as **Cutex Projects**. The unrelated
native Codex catalog remains available as **Workspaces / Codex Workspaces** and
retains its existing create and import operations.
