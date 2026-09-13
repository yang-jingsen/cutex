# Persistent Runtime Host

Persistent Runtime Host (PRH) is an independently buildable, local-only host
for registered background services. It is consumer-neutral: durable service
identity, process intent, health, operations, logs, and dependency semantics
do not refer to any particular application, task system, terminal, or UI.

This directory is a standalone Rust package. It is intentionally outside the
parent package's dependency graph, so building the parent does not build or
change PRH.

## Current stage

Stage 4 keeps the frozen version 1 protocol and accepted Linux/foreground
behavior, and adds a persistent Windows Service host plus an independent
per-user tray client. The package now includes:

- all v1 request/response/error/event types and capability negotiation;
- durable service definitions and separate occurrence-scoped runtime state;
- compare-and-swap in-memory and atomically persisted file registries;
- dependency validation and deterministic start/stop ordering;
- idempotent mutations, expected revisions, queryable operation phases, and
  request identity;
- bounded, non-blocking in-memory log drains with observable eviction,
  subscriber lag, encoding, truncation, and sink failure;
- bounded event replay with gap detection and a live-stream handoff;
- an in-memory fake backend used by deterministic contract tests;
- the same versioned newline-delimited JSON API over a private Unix socket on
  Linux or a private local named pipe on Windows;
- the foreground `prh-host` process and full headless `hostctl` client on both
  platforms, plus a real `ServiceMain`/control-handler path on Windows;
- direct, no-shell, no-PTY Linux process launch with TCP and HTTP probes;
- direct, suspended Windows process launch, an explicit three-handle stdio
  allowlist, assignment to a new per-occurrence Job before resume, and native
  TCP/WinHTTP probes;
- bounded process-output queues plus bounded, rotating per-service JSONL logs;
- an exclusive single-instance lock, owner-only local state, and stale Unix
  socket recovery;
- Linux process-group containment with a package-private per-occurrence
  sentinel for host-crash cleanup; and
- a native Windows notification-area client that independently projects SCM
  reachability and supervised-service status, with confirmed host lifecycle
  actions and no implicit host bootstrap; and
- manifest-driven, hash-verified, idempotent Windows install/upgrade,
  rollback, and scoped uninstall commands owned by `hostctl`.

The Windows service is LocalSystem, starts independently of logon or a terminal,
and grants its install-time operator SID only query/start/stop/interrogate
rights on that fixed service. The operator and LocalSystem alone receive the
protected state/pipe ACL. There is no remote API, scheduled task, persistent
SSH launcher, terminal wrapper, or consumer-specific integration. The internal
`prh-fixture-service` and `prh-linux-sentinel` binaries are test/support
companions, not product-facing hosted services.

## Run explicitly

### Linux

Build all package binaries, choose a private absolute state directory, and
start the host in the foreground:

```sh
cargo build --release --bins
target/release/prh-host --state-dir /absolute/path/to/prh-state
```

In another process, query the already-running endpoint:

```sh
target/release/hostctl --state-dir /absolute/path/to/prh-state info
target/release/hostctl --state-dir /absolute/path/to/prh-state list
```

### Windows

Foreground mode remains available explicitly:

```powershell
cargo build --release --bins
$state = "$env:TEMP\PersistentRuntimeHost-foreground"
.\target\release\prh-host.exe --state-dir $state
```

In another process, query that host or open the independent tray client:

```powershell
$state = "$env:TEMP\PersistentRuntimeHost-foreground"
.\target\release\hostctl.exe --state-dir $state info
.\target\release\hostctl.exe --state-dir $state list
.\target\release\prh-tray.exe --state-dir $state
```

`hostctl` protocol commands and `prh-tray` never bootstrap a missing host.
Foreground `prh-host` remains an explicit action. In an elevated prompt, the
Windows lifecycle commands install the production architecture from an already
built, reviewed release directory:

```powershell
.\target\release\hostctl.exe windows-install `
  --source-dir (Resolve-Path .\target\release) `
  --release-id <source-commit-or-release-version> `
  --source-revision <full-source-identity>

.\target\release\hostctl.exe windows-service-status
.\target\release\hostctl.exe windows-host-restart
.\target\release\hostctl.exe windows-rollback
.\target\release\hostctl.exe windows-uninstall
```

The default runtime root is
`D:\Programs\persistent-runtime-host\<release-id>`, the durable state root is
`C:\ProgramData\PersistentRuntimeHost\state-v1`, the stable service name is
`PersistentRuntimeHost`, and the operator's `HKCU\...\Run` entry starts only
the tray. Upgrade stops the registered predecessor before changing its image
path, verifies the successor through SCM and the local API, and restores the
prior registration on failure. Uninstall removes only the owned service and
matching tray Run value; it deliberately preserves binaries, manifests,
registry state, and logs.

A service definition is JSON matching `ServiceDefinition`. For example:

```json
{
  "id": "example-api",
  "display_name": "Example API",
  "executable": "/absolute/path/to/example-api",
  "arguments": ["--listen", "127.0.0.1:8123"],
  "working_directory": "/absolute/path/to/workdir",
  "environment": { "LOG_LEVEL": "info" },
  "start_policy": "manual",
  "restart_policy": {
    "mode": "bounded_on_failure",
    "max_restarts": 3,
    "window_ms": 60000,
    "backoff_ms": 250
  },
  "dependencies": [],
  "readiness_probe": {
    "kind": "tcp",
    "host": "127.0.0.1",
    "port": 8123,
    "interval_ms": 250,
    "timeout_ms": 100
  },
  "shutdown_policy": {
    "graceful_timeout_ms": 5000,
    "force_kill_timeout_ms": 2000
  }
}
```

Register and control it only after reviewing the executable and paths:

```sh
target/release/hostctl --state-dir /absolute/path/to/prh-state register definition.json
target/release/hostctl --state-dir /absolute/path/to/prh-state start example-api
target/release/hostctl --state-dir /absolute/path/to/prh-state logs example-api
target/release/hostctl --state-dir /absolute/path/to/prh-state stop example-api
target/release/hostctl --state-dir /absolute/path/to/prh-state shutdown
```

Run `hostctl --help` for revision, idempotency, cascade, cursor, operation, and
event-stream options. On Windows, use native absolute paths in the definition
and the corresponding `.exe` command paths.

## Local storage and containment

The state directory is owner-only. It contains `host.lock`,
`registry-v1.json`, and `logs-v1/`; Linux also contains `prh-v1.sock` with mode
`0600`. Windows derives a stable named-pipe name from the selected state
directory, rejects remote pipe clients, and gives only the configured operator
SID and LocalSystem access. Windows state directories and files receive
matching protected ACLs and reparse points are refused. In service mode
LocalSystem owns service-created objects; the explicit operator ACE remains
stable across upgrade and logon boundaries.

Registry replacement uses a synced temporary file plus atomic replacement
(`MOVEFILE_WRITE_THROUGH` on Windows). Log generations have explicit byte and
file-count bounds. Definitions and retained logs survive a host restart;
runtime snapshots, operation lookup, event replay, and the idempotency replay
table are scoped to one `host_instance_id`.

### Linux containment

Linux targets are launched directly with their argument vectors into a new
process group. A dedicated launcher thread remains alive for the host lifetime
because Linux parent-death signals are tied to the creating thread. Before an
occurrence is reported contained, its package-private sentinel acknowledges a
fresh EOF-based liveness channel. Host death closes that exact channel and the
sentinel kills the occurrence's process group. If the sentinel itself exits
while the group is live, the host kills the group fail-closed and exposes the
failure through service state and log diagnostics.

Containment covers the target and descendants that remain in its process
group. A target that deliberately creates a new session or escapes that group
is not successfully contained, and PRH makes no claim that such a process will
be cleaned up. This Linux mechanism is not described as equivalent to a Windows
Job Object.

### Windows containment

Each Windows occurrence gets one unnamed Job configured with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. PRH creates the exact target image
suspended with `CreateProcessW`, uses no shell or pseudo-terminal, restricts
inheritance to its stdin/stdout/stderr handles, assigns the process to the Job,
and only then permits its initial thread to resume. Assignment failure kills
the suspended target and fails the start; there is no uncontained fallback.

Descendants join the Job before their user code runs under normal Windows Job
semantics. When the target exits, PRH cleans remaining Job members before
publishing the occurrence exit. Requested stop first attempts `CTRL+BREAK`
where a shared console permits it, then terminates the whole Job within the
configured bound. Host death closes the last Job handle, activating
kill-on-close cleanup. The SCM recovery policy then starts a fresh host
process; occurrence runtime state is never confused with the persisted
definition registry.

## Verify

Run from this directory:

```sh
cargo fmt --check
cargo test --all-targets --locked --no-fail-fast
cargo clippy --all-targets --locked -- -D warnings
cargo check --target x86_64-pc-windows-gnu --all-targets --locked
cargo clippy --target x86_64-pc-windows-gnu --all-targets --locked -- -D warnings
```

With a MinGW linker and import libraries available, also run
`cargo test --target x86_64-pc-windows-gnu --all-targets --locked --no-run` to
link every Windows binary and test executable. Native-gated integration cases
are included for non-bootstrap, forced Job cleanup, foreground host-crash
cleanup, and a serial disposable Stage 4 SCM lifecycle. The Stage 4 case
exercises session-0 hosting, service-stop cleanup, service-crash cleanup and
recovery, per-user Run registration, repeated install/upgrade, explicit
rollback, and scoped uninstall. These cases are ignored by default and require
an explicitly approved elevated isolated Windows run; cross-linking alone is
not native acceptance.

The frozen contract is described in [docs/protocol-v1.md](docs/protocol-v1.md).
Canonical JSON examples live under `tests/fixtures/` and are checked in the
test suite.
