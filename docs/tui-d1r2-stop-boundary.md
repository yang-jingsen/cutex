# D1R2: whole-tree stop boundary decision

Human's 2026-09-09 reversible Archive policy is accepted: durable archive authority, retained current membership, default-hidden Project members, guarded Restore Offline, and unchanged permanent roster retirement. This packet does not reopen that policy.

## New implementation boundary

The existing stop primitive cannot establish the requested whole-tree proof for non-systemd Unix runtimes. `management_lifecycle::stop_cutex_session_runtime_for_entry` first tries the managed scope, then calls `platform::process::terminate_process_and_wait` for recorded PIDs. `runtime::process_scope::terminate_managed_agent_scope` explicitly returns `found=false, stopped=true` when scope support is unavailable or the scope is absent. On Unix, `process_tree_pids` returns only `vec![pid]`. Consequently the fallback can report `stopped=true` while a tool descendant remains alive. A service adapter must not elevate this result into a whole-tree Archive proof.

`tests/archive_stop_boundary.rs::d1r2_real_pid_fallback_leaves_descendant_alive` reproduces this using the actual public process helper and a Python-owned parent/child, with a separate unrelated process. The parent is reaped concurrently. The helper reports stopped, the descendant remains running, and the unrelated process remains running. The fixture then terminates its own remaining processes. No systemd, runtime manager, Agent Bus, user Agent or persistent store is accessed. This is a real process-boundary characterization, not an end-to-end Archive acceptance test.

## Decision requested

Recommended minimal policy: refuse Archive's online stop path when the runtime has no authoritative whole-tree containment/proof; return an actionable unsupported-stop-proof error without archiving. Keep the full guarantee for supported contained runtimes. Confirm whether this backend eligibility restriction is acceptable before implementing it. Merely recommending a separate existing PID-only Stop would not establish the missing proof.

Alternative: authorize a bounded runtime/process termination change and specify the required descendant/fork/escape ownership guarantee. The current task excludes kernel expansion; a PID descendant snapshot alone is not equivalent to stable containment, so I have not silently selected that weaker guarantee or changed process ownership semantics. This alternative need not redesign Task Service or alter Archive policy.

## Result and verification

Implementation base: `0003d8821a64a6766889bbd8e57efe11565478fa` (D1 reference), parent/product base `bb67dd04e453562c159008d58c9ab6a53ec727d4`. Delta is this document plus one characterization test; production code and lifecycle behavior are unchanged.

Run with `env -i`, a private HOME, explicit Cargo/Rustup caches, and no operator service variables:

`cargo test --offline --locked --test archive_stop_boundary -- --nocapture`: **1 passed**, nonzero selection, 0.11 seconds execution; the passing assertion demonstrates the defect, not compliance. Existing library warnings remain. Formatting/diff checks are recorded in the external result.

D13–D18 application implementation, authenticated persistence matrix, archive reader/count changes, replay/CAS/crash tests and TUI wiring are **not completed**. They are paused at this required stop-proof boundary. Existing D1 evidence remains reference-only. No broad regression rerun, live acceptance, Windows or PTY claim; shell code is unchanged. No deployment, deletion of history, retention changes, canonical/remote edits or dependency changes.

Intended use: **REFERENCE_ONLY blocker**, not an integration candidate. Resume the approved Archive implementation after the minimal stop-proof eligibility or runtime scope decision.
