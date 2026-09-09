# Next bounded Management prerequisite (analysis only)

Input: accepted S6f `94a3f2b34e04c70eaa4faf88c1a4772f4872d30a`;
native `a83dbb47ba6aa775f5d4b679fafc532c4db74c7f`. No Management
create/replace/rotation implementation is included in S6g.

## Existing boundary

`AgentLifecycle::bootstrap_native` in `src/agent_management/provider.rs`
returns a native ID. `create_steps` already persists NativeBootstrapPending,
captures known native IDs even on an uncertain error, then adopts, configures,
and starts the durable identity through staged action records. Unknown bootstrap
without a known ID is explicitly OwnerActionRequired: no second create.
The two reconciliation hooks default to unavailable, not guessed ownership.

`src/cli_app/management_lifecycle.rs::start_cutex_session_new_thread` is an
existing generic runtime path, not a supported lightweight bootstrap adapter.
The reviewed stock bundle3/explicit contract2 in `src/launch/stock.rs` requires
an already-known native ID, stable native home, pinned manifest and supported
effective configuration. It cannot be silently applied before bootstrap or
replaced with a Boolean backend preference. Generic marked-session launch
guards must remain in force.

## Recommendation

Authorize a separate opt-in Management bootstrap intent in the existing action
journal, binding the reviewed coherent bundle, shared native home, permissions
and configuration snapshot **before** the first native create. Extend the
lifecycle adapter explicitly, not its fallback executable selection. Retain
existing project/seat/CAS and formal-name review; profile remains mutable
configuration, never identity. A native title cannot supply a formal name.

There is also an explicit authority decision: Agent-callable Management create
must not impersonate the root Human-only existing-thread activation API. The
recommended first subset is a one-action Human-reviewed bootstrap intent,
consumed by the existing provider action under its project authority. Approval
must cover binding the newly captured native/durable IDs to that exact intent
before online, rather than granting ambient activation or a global mutable
bundle default. The smallest prospective schema addition belongs on the
existing Management action record/request, with staged captured identities and
semantic replay—not a second identity store or a model-callable root endpoint.

Use the accepted native positive persistence acknowledgement before reporting
bootstrap success. Capture the exact returned native ID in the same staged
action, adopt once, install the reviewed explicit launch requirement, and
register one owned occurrence. These are recoverable stages, not an atomic
cross-store create transaction. A failed later stage resumes the same action
and known ID. Replace/rotation must retain their existing predecessor and seat
guards; successful neutral bootstrap alone proves neither operation.

The minimum unresolved guarantee is recovery when the native process may have
created a thread but the generated ID was never received/captured. A positive
flush acknowledgement does not resolve that earlier window. First increment
should retain an explicit uncertain/no-retry state, with no name/cwd/profile or
catalog-difference guesses. Transparent recovery would require separately
approved native request identity/idempotent lookup, not a Cutex duplicate-create
retry. No such native extension is authorized here.

## Decisive next tests

Private root-authorized intent review/CAS and agent denial; stale config/bundle
refusal before spawn; one neutral thread with durable persistence ACK; same-ID
adoption/marker/configuration/registration; owned crash at each journal stage;
known-ID recovery without duplicate creation; pre-ID uncertainty remains
actionable and fenced; unknown intent/version and generic K fallback denied.
Only after that primitive passes should a subsequent bounded task exercise
managed create and then protected replace/rotation end to end.

No live migration, legacy mixed writers, marker clearing, old Soon-parser
downgrade, production auth or full Management readiness is implied.
