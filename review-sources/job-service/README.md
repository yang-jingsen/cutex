# Cutex Job Service core + trusted MCP adapter + durable completion sender JS3B

Status: integration candidate, Linux/tethys only, not deployed.

This standalone Rust package owns noninteractive, non-escalated command processes after the submitting Agent turn/runtime ends. It provides a private Unix-socket core API, a stdio MCP adapter, persistent job records and completion outbox, bounded output, idempotent submission, revision-guarded cancellation, per-job process-group containment, and an optional service-owned Cutex completion-delivery worker. MCP installation and PRH registration remain separate tasks.

## Trusted execution boundary

The model supplies only action ID, argv, and absolute cwd. The stdio adapter takes `codex/sandbox-state-meta` and `threadId` from MCP request `_meta`, never tool arguments. It authenticates its runtime occurrence through an explicitly forwarded loopback Cutex Agent Bus URL/token, requires the Bus native session to equal `_meta.threadId`, and uses the current non-null durable `cutex_session_id` projection as subscriber identity. It neither accepts nor infers a session from name, title, cwd, tool arguments, or ambient daemon identity.

The adapter issues short-lived HMAC execution and per-operation caller grants. The core verifies signature, canonical request digest, expiry, current OS UID, cwd, durable subscriber, exact sandbox launcher digest, operation, and job ID before acting. Signed `managed` profiles retain their materialized filesystem/network policy. Signed `disabled` profiles run as the same OS user without a sandbox and without any elevation. `external`, missing, expired, changed, foreign, or unverifiable contexts fail before launch or access.

The runner clears the ambient environment, sets only fixed `PATH=/usr/bin:/bin`, closes stdin, and invokes the digest-bound launcher as:

```text
cute-codex sandbox --sandbox-state-json <trusted-meta> -- <argv...>
```

The adapter supports the installed MCP 2026 discovery protocol and legacy MCP initialization and advertises `codex/sandbox-state-meta`. Secret files are current-user-owned mode `0600` or stricter. Their values never appear in MCP schemas, results, job output, or environment injection. The threat model excludes a malicious same-UID process, which can inspect other same-UID process state; deployment must therefore keep Job Service, Cutex, and managed Agent runtimes under the same trusted OS account only.

## Process and persistence guarantees

Submission atomically persists `launch_pending` before process creation. Exact `actionId` replay returns the original job; different semantics conflict. After spawn, the runner remains behind a pipe gate until a separate sentinel is watching a daemon-liveness pipe. The runner uses a new process group. Daemon death closes both gates: before release the runner refuses to execute; after release the sentinel sends bounded TERM/KILL to the exact group. Cancellation revalidates `/proc/<pid>/stat` birth ticks and the expected job revision before signaling.

On restart, persisted `launch_pending` becomes `launch_unknown`; persisted `running` becomes `interrupted`. Neither is relaunched. Terminal state creates a durable, idempotent outbox record before any delivery attempt. With no completion configuration the job remains fully queryable and reports `completionDelivery.state=disabled`; there is no Project or Task dependency.

## Durable Cutex completion delivery

Enable the background sender explicitly on the single command-owning daemon:

```text
cutex-job-service serve STATE_ROOT SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER \
  --completion http://127.0.0.1:PORT /absolute/private/job-service-completion.token
```

`--completion` retains the v1 wire. A composed deployment with a compatible
Cutex/native consumer may explicitly select the frozen structured-facts wire
with `--completion-v2` using the same endpoint and token arguments. The default
does not probe or auto-upgrade. See
[`docs/frozen-completion-facts-v2.md`](docs/frozen-completion-facts-v2.md).

The endpoint must be exact IPv4 loopback HTTP. The dedicated Cutex completion token is read on each attempt from an absolute, direct, current-user-owned private file beneath a current-user-owned directory that is not writable by other users. Its value is never placed in model arguments, tool schemas, command environment, job output, or completion content. There is no endpoint or token discovery and the ordinary Agent Bus credential is not accepted as configuration.

Each terminal result has one stable `eventId`, canonical terminal body, and digest. Attempt intent is persisted before HTTP. An ambiguous or lost submit response retries the same event and body; changed semantics are never rekeyed. Once Cutex durably accepts `pending`, the daemon mechanically queries the accepted event with bounded backoff—this is service I/O, not a model turn or Task progress. Reversible `archived` remains pending without waking or auto-onlining; explicit Restore lets a later query observe delivery. Permanent Close becomes retained terminal `orphaned` and leaves the retry queue. Replacement durable IDs and Director rotation never inherit delivery.

The persisted projection distinguishes `disabled`, `sending`, `retry_pending`, `accepted_pending`, `archived`, `delivered`, `orphaned`, `unavailable`, `authorization_failed`, `conflict`, and `operator_required`. Only A4-backed `delivered` and explicit permanent `orphaned` clear the retention obligation; pending acceptance does not. Authorization, semantic conflict, malformed/foreign receipts, and other operator states stop automatic high-rate retry. Transient network/server errors and unavailable classification use bounded exponential retry. Shutdown interrupts the worker's idle wait and joins it; an in-flight request is bounded by the configured timeout, while its durable `sending` intent recovers as `retry_pending` after restart. Running commands still do not survive daemon death and are never relaunched.

State writes use owner-only directories/files, a single-owner `flock`, synced temporary-file replacement and directory sync. Persistent job records retain the request digest, argument count, cwd and environment **names**, but not argv or environment values. Output is continuously drained so children cannot block; each stdout/stderr prefix has a hard byte cap and reports observed/retained bytes plus truncation. Arbitrary output may contain secrets, so it remains private and is never copied automatically into outbox notices.

## Local API

Start a development daemon with owner-only raw secret files (32–4096 bytes, mode `0600` or stricter):

```text
cutex-job-service serve STATE_ROOT SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER
```

The socket and its directory are mode `0600`/`0700`; Linux `SO_PEERCRED` must match the daemon UID. Each newline-delimited JSON request also supplies the API token hex. Maximum request size is 1 MiB. Methods:

- `submit`: `{request, grant}`
- `query`: `{jobId, callerGrant}`
- `cancel`: `{jobId, expectedRevision, callerGrant}`
- `readOutput`: `{jobId, stream, offset, maxBytes, callerGrant}`; bytes are hex and reads are capped
- `pendingOutbox`: `{}` (service-owner diagnostics only)
- `acknowledgeOutbox`: `{eventId, resultSha256}`; refuses anything except already terminal `delivered`/`orphaned`

The library's unscoped query/cancel/output methods and outbox methods are service-owner administration boundaries. They are not MCP tools. The only TCP client is the optional exact-loopback protected completion lane. There is no anonymous listener, PTY, stdin write, escalation, scheduler, Project/Task mutation, cleanup of active jobs, or model-driven polling.

## MCP adapter

Start the adapter under a managed Cutex runtime with explicit forwarding of `CUTEX_AGENT_ID`, `CUTEX_AGENT_BUS_URL`, and `CUTEX_AGENT_BUS_TOKEN`:

```text
cutex-job-service mcp-stdio SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER
```

Its compact tools are `submit`, `query`, `cancel`, and `read_output`. `submit` returns the durable receipt immediately. Query is a manual diagnostic, not a polling workflow. Every call revalidates the current runtime/native-thread/durable-session projection and every job access is restricted to the original durable subscriber. Outbox acknowledgement is deliberately unavailable to Agents.

## Defaults and capacity

The daemon default is 16 concurrent jobs, 1,024 retained job records, 1 MiB per stream per job, 64 KiB per output read, two-second graceful cancellation, and no request environment injection. JS1 intentionally performs no automatic record cleanup; reaching capacity rejects new work. A later retention action may delete only explicitly eligible acknowledged terminal records—never active jobs or unacknowledged terminal delivery.

## Evidence boundary

`tests/core.rs` covers real owned subprocesses, exact replay/conflict, subscriber-scoped access, stale/foreign/wrong-operation caller grants, actual sandbox write/network denial, bounded output, revision/birth-guarded cancellation with an unrelated process surviving, owner-death cleanup, restart projection to `interrupted`, authenticated same-UID full access, forged/unknown/external grant rejection, and private Unix protocol authentication. `src/bin/js1-test-owner.rs` requires the non-default `test-harness` feature and is not present in a normal production build.

`tests/mcp_real_path.rs` starts isolated Job Service, Agent Bus, and Responses fixtures, then invokes the installed cute-codex `0.153.4` binary with a private temporary `CODEX_HOME` and actual configured stdio MCP transport. It proves that the client-generated thread, authenticated durable projection, active `disabled` permission profile, cwd, submission, and actual subprocess reach the persisted core boundary without a model call. Managed read-only filesystem/network probes remain direct OS-oracle tests with a synthetic issuer; no live MCP, Cutex, Agent, project, task, or user-manager state is touched.

The source provenance is cute-codex `ef53716b7673ad14c24b977667334e31e66110d8` and Cutex `e7d01585661d350bffeea468d1db202628047490`. The former constructs trusted MCP `_meta`; the latter launches managed app servers with runtime occurrence and Agent Bus credentials and projects the native session to a durable Cutex identity. JS2 is an integration candidate, not installed or deployed.

## Runtime upgrade and execution directories

A job's `cwd` is its execution directory, independently of the originating
`sandboxCwd` policy anchor. The runner enters the original sandbox first, then
changes directory and executes argv with positional arguments. This preserves
managed filesystem/network limits and supports child worktrees, spaces and
Unicode paths. Failure to access the requested directory is a job failure.

Daemon startup accepts repeated `--allow-launcher PATH` arguments in addition to
the positional launcher. During a native upgrade retain launchers used by live
older agents and add the newly installed launcher. Each canonical path remains
bound to its file digest; arbitrary interpreters are not substituted for the
native launcher. The authenticated local `capabilities` method returns
`allowedLaunchers` (path to SHA-256) and `independentExecutionCwd` for installation
preflight. API credentials remain hex-encoded raw secret-file bytes.

Run `cargo test --features test-harness` to include the owner-death helper binary.
The real MCP test uses a local mock Responses server, submits in a child worktree,
and checks the actual command output and inherited disabled profile.
