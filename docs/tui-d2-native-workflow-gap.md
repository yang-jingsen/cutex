# D2 native workflow adapter decision — reference only

Inspected accepted base `929c67d821f7c8efe9394ac066c9ec64dfc515d8`,
tree `58d5df7c170883411101e7a1095031f97848f2e9`. No workflow is enabled by
this document. D1R2 source remains unchanged.

## Exact gap

D2 explicitly requires returning a bounded gap when there is no supported
unmanaged adapter, rather than relabelling managed creation as New Session.
There are lower-level native command primitives, but no existing application
workflow found that provides native-only creation/resume, inherited optional
profile, deliberate isolation from inherited Agent identity, and return to the
outer shell. The distinction is an absent verified adapter, not a claim that
the native CLI cannot create or resume sessions.

- `cli_app/launch.rs::run_profile` resolves a required profile and invokes
  `launch_process::run_codex_process`. The latter initializes management/notify
  facilities, optionally wraps the launch, and calls `std::process::exit`.
- `runtime/managed_launch.rs::maybe_wrap_launch_with_session` wraps an empty
  argument launch when configured; therefore this general wizard is not an
  unconditional native-only/return-safe entry.
- `cli_app/session_runtime.rs::cmd_session_resume_foreground_inner` requires a
  durable record and also exits for its legacy foreground branch. Fabricating a
  durable record for an unmanaged Recent row is not an acceptable replacement.
- `launch/profile.rs::profile_launch_command` is a usable lower-level builder,
  not a native workflow. `agent_mode=false` omits new bus env values but
  `LaunchCommand::to_command` still inherits parent environment; it does not
  prove the child lacks inherited Agent/management context.
- The existing creation implementation,
  `cli_app/agent_management.rs::native_bootstrap_plan`, requires an explicit
  profile and runs `cutex run ... --agent -- exec --json ... Hi.`. Its owner is
  Agent Management creation, not unassigned root-Human creation.
- The model-free `management_lifecycle::start_cutex_session_new_thread` path
  requires an already-created durable Release session and establishes its
  managed runtime. It is not an unmanaged native-session service.

## Proposed bounded decision

Authorize a native-only application adapter first, built from the existing
profile/native command primitives: explicit cwd and optional profile inheritance,
no managed wrapping/service activation, explicit inherited Agent-context removal,
child exit returned to TerminalShell, and no automatic retry after spawn or
unknown outcome. Validate the real installed native binary with private state
and no model turn, then use that accepted boundary for D06/D07. Reuse the native
`thread/start` protocol for a separately root-Human-confirmed New Agent
bootstrap only after its create/uncertainty semantics are explicit; do not call
the Director-owned create API or its paid neutral-turn recipe.

Alternative: defer New Session/New Agent plus no-candidate creation roundtrip;
continue only existing Adopt/import/assignment and member action integration
under a narrowed D2 contract. Native-only Enter remains disabled until its
adapter is verified. No kernel or bus-source change is proposed.

## Evidence / unexecuted acceptance

STATIC: traced the production entry points above, existing recent adoption,
typed import and dispatch requirements. Current AGENTS/SOP and expert sections
5.2/5.3, effect/return rules and D06–D12/D19–D20 read.
REAL_PROCESS_BOUNDARY (help only): installed
`/home/senxiu/Resources/Shortcuts/cute-codex --help` under scrubbed environment,
private HOME/CODEX_HOME confirmed interactive native/resume/app-server commands.
This is not evidence of native no-auto-adopt behavior. No native session,
provider mutation, model turn, live service or operator state was created.

No D2 production-route acceptance test was run or claimed. Unchanged D1R2
evidence is reused, not rerun. This reference-only decision packet is the only
source delta; implementation waits at the explicit missing-adapter gate.
