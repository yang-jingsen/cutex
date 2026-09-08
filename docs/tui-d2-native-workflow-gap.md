# D2 native workflow adapter decision — reference only

## Follow-up after R13 adapter authorization

R13 authorized the bounded native-only adapter; the initial authority question
below is resolved. The current stop is a concrete native persistence boundary,
not a renewed request to authorize ordinary adapter work.

Three private, model-free observations:

1. Direct native `thread/start` with `ephemeral=false` returned a thread ID and
   rollout path. Immediate creator exit followed by fresh-process resume failed
   `-32600: no rollout found`; no native files were present.
2. Adding a `thread/read includeTurns=true` roundtrip returned `list_turns is not
   supported yet`, but the subsequent fresh-process resume succeeded and native
   files existed. Thus persistence is possible without a model turn; this does
   not establish a reliable persistence acknowledgement.
3. The actual new Rust application adapter used `thread/read includeTurns=false`
   followed by creator drop and fresh-process resume. Read succeeded but resume
   again failed `no rollout found`. It correctly returned uncertainty and never
   wrote/adopted a Cutex identity. This is the second real failure of the
   create/read/exit/resume experiment, so SOP section 11/real-platform stopping
   guidance applies; no blind delay/retry loop or fabricated success was added.

The unexposed adapter in `cli_app/session_native_workflow.rs` clears inherited
environment, allowlists explicit profile/auth and normal terminal context,
returns child status instead of process exit, and has a failing opt-in native
oracle. It is reference-only, not ready to wire into forms. Catalog transport
has only a constructor accepting an explicitly configured child Command.

Exact executed native test:
`cargo test --offline --locked --bin cutex ui_contract_d2_real_native_bootstrap -- --ignored --nocapture`
with scrubbed environment, private HOME and
`CUTEX_CODEX_BIN=/home/senxiu/Resources/Shortcuts/cute-codex`.
Result: 1 selected, 0 passed, 1 failed at persistence proof. Log:
`/tmp/cutex-d2-real-native.log`. The direct probe is
`scripts/tui-d2-native-probe.py`; roots
`/tmp/cutex-d2-native-uef2wq61` (failure) and
`/tmp/cutex-d2-native-2mhhclq2` (success) are private evidence.
No model prompt/turn, production HOME, service, identity or notification used.

Next decision: obtain/verify a native persisted-thread acknowledgement (possibly
the existing saved catalog `thread/list` boundary while keeping the creator
alive), or defer New Agent bootstrap until a Human interactive first session is
known resumable. A fixed sleep is not acknowledgement. No paid Hi recipe,
native-history file writing, kernel changes or second-identity retry is proposed.
Remaining D2 forms/member workflows are not implemented or accepted.

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
