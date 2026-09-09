# S8a private reviewed Management bootstrap

Work in progress; not a release or live Management acceptance.
Base `29a0539f0395eb45581b813e4795ed1c8d3f0cea`.
Native stays `a83dbb47ba6aa775f5d4b679fafc532c4db74c7f`, bundle3 /
explicit-launch contract2. No native, Task, Job, TUI or default-launch change.

## Authority and command boundary

The existing root-authenticated
`POST /v2/agent-management/explicit-launch` accepts `review_bootstrap` with
the complete Management Create request, canonical native home, pinned bundle
manifest/digest and expiry. `authorize_bootstrap` confirms that exact review
against its Management store revision and project authority. Replaying the
same review returns the immutable intent; changed review conflicts.

Create includes `bootstrap_intent`, equal to its exact `action_id`, alongside
explicit formal name/spec, project and `bootstrap_only`. The existing
authenticated project Director action consumes the stored intent. A missing,
changed or wrong-action reference cannot enter the legacy executable path.
The root choice authorizes this bundle/configuration for this action; it does
not grant the Director role or impersonate Human in Agent-callable MCP.
Unmarked legacy Create remains unchanged. The new optional field is currently
the typed Management HTTP boundary, not an expanded MCP creation wizard.

Only this private Create/bootstrap-only subset is implemented. Operator
consumption, custom initial messages, replace and Director rotation are not
enabled by an intent. Production approval UX/defaults remain undecided: this
experiment is not a permanent requirement that Human approve every future Agent.

Configuration is reviewed before any native ID exists, using the actual profile
snapshot without constructing a placeholder identity. Only the existing dummy
API/host/allowlisted permissions subset is supported. Shared native home is
profile-independent. Formal name comes only from the explicit spec. Native
title and source metadata are not rewritten to invent Agent provenance.

The review includes effective Bus runtime groups using its existing normalizer.
Those legacy routing labels are not Cutex Project membership. The stock adapter
uses no `path_key`; legacy K readiness uses its existing seven-character key.
S8a readiness compares the explicitly reviewed stock group projection, without
changing Bus policy or deriving Project ownership from cwd.

## Stages and recovery

| Stage | Writer / persisted evidence | Recovery |
|---|---|---|
| Human intent | Management store v2: complete immutable review keyed by action | Exact replay; expiry/authority/config fences before create |
| NativeBootstrapPending | Existing Management action journal, before native request | No ID means uncertain/no second thread/start |
| NativeSessionCaptured | Exact returned native ID, including known-ID errors | Resume only this ID; require positive persistence ACK |
| Adoption | Existing durable store: adoption, formal config, launch marker and bootstrap receipt in one save | Same intent/native receipt returns same durable ID; conflicting mapping rejects |
| Adopted / Configured | Existing roster/action captures same durable/native ID and project | No silent reimport, default launch or marker removal |
| Stock runtime journal | Existing claim/publication/Spawned/Ready receipt under a derived bounded action ID | Existing same-claim owner recovery and original review; no K fallback |
| Complete | Existing Management receipt and phase audit after actual readiness checks | Historical exact replay, not a fresh launch |

The native ACK is specifically paginated
`thread/read(includeTurns=true)`: accepted foundation `aa382cdb5` and exact
native `request_processors/thread_processor.rs::apply_thread_read_store_fields`
await `loaded_thread.persist()` and propagate errors before returning. A start
ID or metadata-only read is not that ACK. The successful invocation can use its
ACK directly; recovery with only a captured ID must obtain the ACK again.
No neutral model turn, incidental delay, filesystem polling or catalog-difference
identity inference is used.

Unknown pre-ID outcomes deliberately require owner action. A potentially
created native thread is not deleted or rediscovered by guessing. Bootstrap
children are owned handles with Linux parent-death termination; this is not a
new arbitrary descendant/cgroup guarantee. Later runtime recovery retains the
accepted S4 occurrence/publication limitations.

## Compatibility and remaining work

First authorization writes Management store v2, so old enum parsers fail
closed. The durable store gains a typed bootstrap receipt, likewise unsupported
by old writers. Intent fields are validated on reads; no downgrade/clear API,
history rewriting or mixed-writer migration is offered. Candidate-owned stores
only. Existing Soon history/Bus v4/marker restrictions remain.

Next: separately authorize and exercise protected replacement/rotation using
this staged primitive, preserving predecessor stop, seat transfer and message
semantics. Pre-ID transparent recovery would still require a separate native
request identity/idempotency decision. Release-MCP terminal dispatch remains a
separate outbound adapter gap; no Release authority is added here.
