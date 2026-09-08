# S4 activation/recovery blocker — REFERENCE_ONLY

Base: `2145ea4da876bdbfc244c2ccec2e921dbda90d01`, tree `1caa0efa08c14ce65ce9fc10843bdf6b089911fd`, retaining accepted S2/S3. No product code was changed and no stock lifecycle command is implemented or supported by this result.

## Decisive finding

The required pre-implementation activation check fails: the current persistent model cannot distinguish an explicitly stock-bound offline Agent from an ordinary offline Agent after normal stop/recovery. Therefore a stock-only command with explicit arguments cannot by itself prevent a later ordinary online/restart from selecting K/default executable for that record.

Exact-base source trace:

1. `src/session/model.rs:132–152`: runtime binding has endpoint/PID/runtime directory/profile/schema provenance, but no persistent executable/home/config contract. `app_server_launch_claim_id` at line 240 is a crash fence, not backend intent.
2. `src/cli_app/management_lifecycle.rs:1113–1137`: an existing claim/binding blocks a fresh launch. This is useful for an uncertain in-flight launch but not a durable stock-only guard after a successful stop.
3. `src/cli_app/management_lifecycle.rs:1649` calls `clear_cutex_session_runtime_record`; `src/session/runtime_reconciliation.rs:300–318` clears the launch claim, app-server binding, PID and current runtime ID. Consequently storing provenance only in the occurrence binding cannot protect the next offline launch.
4. `src/cli_app/agent_management.rs:1220–1260` routes fenced restart into `start_cutex_session_online_with_profile_if` with no stock launch arguments. `src/cli_app/management_lifecycle.rs:285–339,911–967` then resolves the account and normal profile launch command. There is no surviving stock-only discriminator to reject that fallback.

This is a STATIC control-flow/model finding, not a claim that a private or production fallback was executed. No probe is necessary to establish that the proposed guard disappears on the normal clearing path.

## Minimum decision recommended

Authorize a small **durable explicit-launch requirement**, separate from mutable profile and transport backend, which survives stop, archive/restore and runtime reconciliation. For opted-in records all generic online/restart/foreground paths must reject before spawn unless supplied the exact authorized stock launch contract. The contract must bind the native home and bundle/config provenance; missing/corrupt evidence rejects. It must not silently interpret an absent contract as permission to resume a previously stock-bound record through K.

This does not require automatic stock restart or a new daemon. The subsequent implementation would add the smallest typed durable field/contract representation and its reviewed CAS activation operation, plus guards at shared launch entrypoints. Exact schema and who can activate/clear the requirement need Director approval; do not encode it in `profile`, `runtime_backend=host`, schema hash, claim ID, name, cwd or a hidden sidecar ledger. No secrets belong in that record. Clearing the requirement would be an explicit compatibility decision, not a cleanup side effect.

Alternative if durable schema is still prohibited: revise S4 to an isolated launch-only prototype whose dedicated private environment disables ordinary lifecycle entrypoints. That can test stock mechanics but cannot claim per-record safe coexistence with normal Cutex lifecycle. The recommended persistent guard is the smaller path to the requested coexistence guarantee.

## Status and omitted work

| Criterion | Status |
| --- | --- |
| Prevent default/K fallback after stop/recovery | Blocked by missing durable launch requirement |
| Preserve accepted S1/S2/S3 and release source | Preserved; documentation-only descendant |
| Stock command, two profiles, real resume/restart, MCP, approval PTY | Not implemented/run; stopped at explicitly required pre-implementation gate |
| Source inspection and scoped self-review | Completed; `git diff --check` applicable |
| Build/fmt/runtime tests | Omitted: no Rust or executable change, no new behavior to verify |

No new artifact/cache/probe output, no paid model calls, no live state/config/auth/process inspection, no global process arguments, no deployment or canonical changes. Existing S2 safety incident remains unchanged and was not revisited. Last task-root filesystem observation remains approximately 484 GiB free and 3.2 GiB retained; only this compact document is added. Rollback: none required for runtime; reject this documentation candidate if unsuitable. Intended use REFERENCE_ONLY, not INTEGRATION_CANDIDATE. Wait for the narrow schema/authority decision before implementation.
