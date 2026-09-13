# Stage 1 handoff

Date: 2026-09-07 (Australia/Sydney)

## Source identity

- Base release snapshot: `cdbeaa9e2cba8ac88172bb0392dbe54283eb811a`
- Task branch: `persistent-runtime-host-r1-stage1`
- Package: `persistent-runtime-host/`
- Parent Cutex package files and behavior: unchanged
- Implementation provenance: original PRH-owned code; no tethysUNE code copied,
  imported, modified, or depended upon

## Completed boundary

The standalone contract/fake-backend stage is complete: protocol v1, service
and runtime models, operation and dependency semantics, registry CAS boundary,
bounded log/event behavior, golden wire fixtures, and deterministic fake host.

Acceptance coverage includes idempotency, changed-request key conflict,
disconnect/retry, stale revisions, registry writer conflict, missing/cyclic
dependencies, concurrent starts, start/stop/cascade order, host-start policy,
event replay/live handoff, log backpressure/failure/binary bounds, health versus
process identity, stale run isolation, bounded failure restart, and shutdown
ordering/start fencing.

## Verification record

Run from `persistent-runtime-host/`:

```text
$ cargo fmt --check
exit 0

$ cargo test --all-targets --locked --no-fail-fast
25 passed; 0 failed

$ cargo clippy --all-targets --locked -- -D warnings
exit 0; 0 warnings

$ cargo test --doc --locked
0 doctests; exit 0

$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked
exit 0; 0 rustdoc warnings

$ cargo test --release --all-targets --locked --no-fail-fast
25 passed; 0 failed

$ cargo check --target x86_64-pc-windows-gnu --locked
exit 0 (compile check only; not Windows runtime acceptance)
```

## Intentionally incomplete

Stage 2 has not started. There is no endpoint, host process, `hostctl`, direct
OS process launch, file log rotation, single-instance lock, Linux process-group
containment, fixture executable, installer, autostart, or production service
use. Windows Job containment and native tray remain Stage 3. No consumer or
third-party repository integration was added.

## Remaining Stage 1-to-2 risks

- The fake backend completes operations synchronously. Real readiness delays,
  restart backoff/window timing, shutdown timeouts, and process-exit races need
  platform-backed tests in Stage 2.
- Registry persistence, operation/idempotency retention across a host restart,
  endpoint framing/permissions, and file log rotation are intentionally not
  implemented by the Stage 1 memory boundary.
- No containment claim has been made. Linux process-group cleanup and later
  Windows Job behavior require their platform acceptance tests.
