# PRH local management protocol v1

Status: frozen Stage 1 contract, implemented by the Linux and Windows local hosts

Protocol version: `1.0`

## Compatibility rules

Every request and response carries `{ major, minor }`, a request ID, and a
capability list. A server rejects an unsupported major with the typed
`unsupported_protocol` error. Additive optional fields and unknown
capabilities are tolerated. An incompatible field or semantic change requires
a new major version; it must not silently alter v1.

Requests use a `request` object tagged by `method` and `params`. Responses use
`status: ok` with a correspondingly tagged `response`, or `status: error` with
a stable typed error. Server-pushed events carry the negotiated protocol,
subscription ID, and a monotonic event sequence.

Golden JSON shapes are normative examples:

- `tests/fixtures/register_service_request_v1.json`
- `tests/fixtures/unsupported_protocol_response_v1.json`
- `tests/fixtures/service_state_event_v1.json`

## Methods

| Method | Kind | v1 result/semantics |
| --- | --- | --- |
| `GetHostInfo` | read | Product/version, host instance, protocol and capabilities |
| `GetHostStatus` | read | Host phase, registry revision and service snapshots |
| `ShutdownHost` | mutation | Closes the start/restart fence, then reverse-order stop |
| `ListServices` | read | Stable-ID-sorted definition and runtime snapshots |
| `GetService` | read | One definition and its occurrence-scoped runtime |
| `EnsureRunning` | mutation | Starts dependencies first and converges on one occurrence |
| `EnsureStopped` | mutation | Leaves dependencies running; optional dependent cascade |
| `Restart` | mutation | Rejects live dependents unless cascade is explicit |
| `Reconcile` | mutation | Reapplies desired and on-host-start intent |
| `GetOperation` | read | Current/final operation phase by durable operation ID |
| `ReadLogs` | read | Bounded cursor page, optionally filtered by run ID |
| `SubscribeEvents` | stream setup | Bounded replay, gap marker, cursor and live stream token |
| `RegisterService` | mutation | Adds revision 1 after validation and graph checks |
| `UpdateService` | mutation | Replaces a stopped definition and increments its revision |
| `RemoveService` | mutation | Removes only a stopped definition with no dependents |

All mutations require a non-empty idempotency key. Retrying identical input
with the same key returns the original logical outcome and operation; using the
key for different input returns `idempotency_conflict`. The request ID is
connection/request correlation and may change on retry.

Operation lookup, idempotency replay, runtime snapshots, and event history are
scoped to the `host_instance_id` returned by `GetHostInfo`. Definitions and
retained file logs are durable across host instances. A client observing a
changed host instance must refresh state; v1 does not promise replay of an
operation or idempotency record created by a prior host process.

`expected_revision` is optional optimistic concurrency control. It addresses
the registry revision for `RegisterService` and all-services `Reconcile`, and
the target definition revision for other service mutations. A mismatch returns
`revision_mismatch` before changing runtime or durable state.

## Identity and state

`ServiceId` is the durable identity. PID, process birth marker, run ID,
operation ID, endpoint, health result, and UI state are observations or
occurrence identifiers and never redefine the service.

A definition contains an absolute executable path, an argument vector, an
absolute working directory, controlled environment additions, start/restart
policies, dependencies, an optional TCP/HTTP readiness probe, and a bounded
shutdown policy. There is no generic shell command or arbitrary health/shutdown
hook in v1.

Runtime state independently records desired state, observed state, health,
definition revision, run and operation identity, operation phase, process
identity, start/exit data, restart count, and graceful/forced/timed-out stop
outcome. Health becoming unhealthy does not imply process absence and does not
permit a second occurrence. Asynchronous observations are accepted only for
their exact run ID.

## Dependencies and shutdown

Definitions must reference registered dependencies and the graph must be
acyclic. Starting recursively starts dependencies before their dependents.
Stopping a service never stops its dependencies. Stopping or restarting a
dependency used by a running service returns `dependency_in_use` unless the
caller requests cascade. Cascade stops dependents before dependencies; restart
then restores only dependents that were running.

Host shutdown changes phase to `shutting_down` before the first stop. That
phase rejects new start/mutation work and disables unexpected-exit restart.
Services then stop in reverse topological order and the host becomes `stopped`.

## Logs and events

Log entries are labeled with both service and run IDs. Their cursor is
monotonic per service, so old and new occurrences cannot be conflated. UTF-8 is
sent directly; other bytes use base64. Buffers have entry and byte bounds.
Eviction, oversized-entry truncation, slow-subscriber loss, and the latest sink
failure are exposed in diagnostics. Producers use non-blocking delivery, so a
missing, closed, or slow reader cannot stop draining service output.

Event history is bounded. `SubscribeEvents` reports the oldest available
sequence and `replay_gap`; a client detecting a gap must refresh snapshots
before consuming the live stream. Stream delivery is advisory and state is
always recoverable through the read methods.

## Local transports

The Linux host exposes one owner-only Unix socket named `prh-v1.sock` inside
the selected owner-only state directory. The Windows host exposes a byte-mode
named pipe whose stable name is derived from the normalized absolute state
directory. Its protected DACL grants access only to LocalSystem and either the
explicit install-time operator SID (service mode) or current user (foreground
mode), and the server rejects remote clients.

Both transports carry the same newline-delimited JSON frames with a 4 MiB
payload limit. A normal connection carries one request and one response. A
successful `SubscribeEvents` connection carries its response and then
`EventEnvelope` frames until disconnect or host shutdown. Read and write
deadlines bound non-stream connections; Windows uses cancellable overlapped
I/O so a deadline does not leave a stack operation live.

The endpoint is discovery-only from the client's perspective: failure to
connect is `HostUnavailable` and never triggers implicit host creation. Stale
Unix-socket cleanup occurs only inside an explicitly started Linux host after
it acquires the single-instance lock. A non-socket collision is refused rather
than replaced. Windows host, tray, and `hostctl` use the same pipe derivation
and unchanged v1 envelopes.

The host applies restart backoff without blocking log/event draining, counts
restart attempts inside the configured rolling window, and stops after the
configured budget. Requested starts and dependency starts wait for readiness
before returning. Health checks after readiness update health only; they never
infer process identity or create a parallel occurrence. Linux uses its direct
process-group backend; Windows creates the exact target suspended, assigns it
to a per-occurrence kill-on-close Job, then resumes it. Neither backend uses a
shell or pseudo-terminal.

## Stable error codes

`unsupported_protocol`, `invalid_request`, `invalid_argument`, `not_found`,
`already_exists`, `revision_mismatch`, `idempotency_conflict`,
`dependency_missing`, `dependency_cycle`, `dependency_in_use`, `service_busy`,
`host_shutting_down`, `operation_not_found`, `registry_conflict`,
`log_unavailable`, and `internal`.

Errors carry a human-readable message, a retryable flag, and optional typed
details. Clients branch on the code, never the message.
