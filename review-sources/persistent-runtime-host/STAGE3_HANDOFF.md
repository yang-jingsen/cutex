# Stage 3 handoff

Date: 2026-09-07 (Australia/Sydney)

## Source identity

- Accepted Stage 2 baseline: `a8e9866b9cd82783e5f4d59415c48331c9729999`
- Stage 2 tree: `88c983e863d76d1133ba03b6a3ee0a8f6420e803`
- Task branch: `persistent-runtime-host-r1-stage1`
- Package: `persistent-runtime-host/`
- Parent Cutex package files and behavior: unchanged
- Implementation provenance: original PRH-owned code; no third-party product
  checkout was copied, imported, modified, or added as a dependency

## Completed boundary

Stage 3 adds a Windows host and native tray while preserving the frozen v1
contract and the accepted Linux behavior:

- one unnamed Job Object per Windows service occurrence, configured with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`;
- exact-image `CreateProcessW` launch with a mutable quoted command line,
  controlled Unicode environment, no shell, no pseudo-terminal, and the target
  initially suspended;
- a `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` containing exactly redirected stdin,
  stdout, and stderr; all PRH-owned containment and parent pipe handles are
  non-inheritable;
- mandatory Job assignment before `ResumeThread`, guarded by an explicit
  launch-state transition; assignment failure terminates the suspended process
  and has no naked fallback;
- bounded, independently drained output with observable loss, native TCP and
  WinHTTP readiness/health probes, best-effort graceful `CTRL+BREAK`, bounded
  whole-Job force termination, and descendant cleanup before exit publication;
- a current-user/LocalSystem-only, remote-rejecting named pipe with overlapped,
  cancellable bounded I/O and unchanged newline-delimited v1 envelopes;
- protected current-user/LocalSystem ACLs for Windows state plus owner and
  reparse-point checks, and write-through atomic registry replacement;
- the full existing `hostctl` command surface on Windows, still with no
  implicit host creation; and
- a native Win32 notification-area client that shows host/service truth and
  provides start, stop, restart, log viewing, refresh, and local alert/error
  dismissal without mutating host state.

The portable Stage 3 tests cover Windows argument quoting, exact command-line
shape, case-insensitive environment replacement and block termination, stable
named-pipe derivation, the three-handle inheritance contract, launch ordering,
and tray status/action/alert projection.

## Verification record

Run from `persistent-runtime-host/`:

```text
$ cargo fmt --all -- --check
exit 0

$ cargo test --all-targets --locked --no-fail-fast
45 passed; 0 failed

$ cargo clippy --all-targets --locked -- -D warnings
exit 0; 0 warnings

$ cargo test --doc --locked
0 doctests; exit 0

$ RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --locked
exit 0; 0 rustdoc warnings

$ cargo test --release --all-targets --locked --no-fail-fast
45 passed; 0 failed

$ cargo check --target x86_64-pc-windows-gnu --all-targets --locked
exit 0

$ cargo clippy --target x86_64-pc-windows-gnu --all-targets --locked -- -D warnings
exit 0; 0 warnings

$ cargo test --target x86_64-pc-windows-gnu --all-targets --locked --no-run
exit 0; all Windows library, binary, and integration-test executables linked
with a temporary extracted MinGW toolchain; no executable was run
```

For the final link command,
`CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` pointed to a temporary
`x86_64-w64-mingw32-gcc-posix` installation external to the repository,
assembled from Ubuntu's GCC 13.2.0 MinGW compiler/runtime, MinGW-w64 13.0.0
headers/import libraries, and MinGW binutils. The temporary tooling is not a
package dependency or committed artifact.

The Linux suite retains its seven real-process Stage 2 cases and adds seven
portable Stage 3 checks (one launch-gate unit test and six integration tests).
The Windows target also contains three native-gated integration cases for
missing-host non-bootstrap, forced whole-Job cleanup, and kill-on-close cleanup
of a real target plus descendant. The two real-process cases are marked ignored
so they require an explicit, approved invocation such as
`cargo test --test windows_stage3_native -- --ignored --test-threads=1` on the
isolated Windows test system. All three cases linked here but were not
executed. No production service is registered or mutated.

## Explicit acceptance gap and exclusions

- No native Windows host, target, descendant, named-pipe, ACL, console-control,
  Job Object, WinHTTP, or tray interaction was executed in this stage. The
  linked cross-build is compile/link evidence, not native Windows acceptance.
- No real Windows machine was touched or mutated.
- Stage 2 Linux containment retains its documented process-group escape limit;
  the Windows implementation makes no claim about processes that obtain a
  separately authorized Job breakaway outside PRH's configured Job semantics.
- There is no installer, autostart, Windows service registration, production
  service registration, deployment, consumer integration, Linux tray, or
  remote API.
