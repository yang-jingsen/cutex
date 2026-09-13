# Stage 2 handoff

Date: 2026-09-07 (Australia/Sydney)

## Source identity

- Stage 1 baseline: `4111587b485deca7f02095f06585c75049622bd6`
- Base release snapshot: `cdbeaa9e2cba8ac88172bb0392dbe54283eb811a`
- Task branch: `persistent-runtime-host-r1-stage1`
- Package: `persistent-runtime-host/`
- Parent Cutex package files and behavior: unchanged
- Implementation provenance: original PRH-owned code; no third-party product
  checkout was copied, imported, modified, or added as a dependency

## Completed boundary

Stage 2 is a runnable Linux local host over the unchanged v1 contract:

- foreground `prh-host`, owner-only Unix endpoint, and full headless `hostctl`;
- explicit bootstrap only: a missing endpoint never starts a host;
- owner-only state, exclusive instance lock, stale-socket recovery, and refusal
  to replace non-socket/symlink state collisions;
- atomic file-registry CAS persistence and bounded per-service JSONL rotation;
- direct executable/argv launch with no shell and no PTY, a controlled base
  environment, continuous independently drained stdout/stderr, a bounded
  backend queue, and observable loss/failure diagnostics;
- real TCP and HTTP readiness/health probes;
- rolling-window bounded restart with non-blocking backoff scheduling;
- bounded graceful-to-force shutdown and process-group descendant cleanup;
- exact process birth markers and host-instance-qualified run/operation IDs;
- isolated Linux fixture executables for parent/descendant, high-output,
  graceful, forced, readiness, restart, and crash paths.

## Linux containment acceptance

Every target is created directly in a new process group by a dedicated launch
thread that lives for the backend lifetime. This is required because Linux
`PR_SET_PDEATHSIG` follows the creating thread, not merely the thread group.

Each successfully contained occurrence also owns a package-private sentinel.
The sentinel is not a target wrapper and has no API, durable state, independent
restart, or product lifecycle. A fresh socket-pair EOF channel identifies the
exact host occurrence without PID reuse. The sentinel must print `READY`
before the backend reports the target contained. Host death closes the channel;
the sentinel repeatedly kills the process group and exits. Unexpected sentinel
loss is detected by the host, which kills the group fail-closed and reports the
failure in service state and log diagnostics.

Real-process tests validate:

- target `/proc/<pid>/exe` is the fixture executable itself;
- target leaders and fixture descendants share the expected process group;
- normal and forced stop leave neither target, descendant, nor sentinel;
- host `SIGKILL` is detected via EOF and leaves no contained process or
  sentinel, after which an explicit host restart recovers the stale socket;
- sentinel `SIGKILL` fails closed, marks the occurrence failed, reports the
  containment failure, and leaves no occurrence;
- a capacity-one output queue does not stop pipe draining and reports dropped
  chunks;
- real failure restart waits for backoff and stops at its rolling-window
  budget.

## Verification record

Run from `persistent-runtime-host/`:

```text
$ cargo fmt --all -- --check
exit 0

$ cargo test --all-targets --locked --no-fail-fast
38 passed; 0 failed

$ cargo clippy --all-targets --locked -- -D warnings
exit 0; 0 warnings

$ cargo test --doc --locked
0 doctests; exit 0

$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked
exit 0; 0 rustdoc warnings

$ cargo test --release --all-targets --locked --no-fail-fast
38 passed; 0 failed

$ cargo check --target x86_64-pc-windows-gnu --locked
exit 0 (compile check only; not Windows runtime acceptance)
```

The seven `tests/linux_stage2.rs` cases launch real local host, CLI, target,
descendant, and sentinel processes. No production service is registered or
mutated.

## Explicit limits and remaining stages

- Linux process-group containment covers descendants that remain in the
  occurrence group. A target that deliberately calls `setsid` or otherwise
  escapes that group is not successfully contained; no broader claim is made.
- Definitions and retained logs persist. Runtime state, operation lookup,
  event replay, and idempotency replay are scoped to one `host_instance_id`.
- Stage 3 remains untouched: there is no Windows Job backend, native tray, or
  Windows runtime acceptance. A cross-build is not acceptance.
- There is no installer, autostart, system service, production registration,
  deployment, consumer integration, Linux tray, or remote API.
