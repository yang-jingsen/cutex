# Job Service completion lane v1

This source-only lane accepts terminal Job Service facts over the loopback Agent
Bus listener. It does not run commands, mutate Task/Project state, create or
start Agents, or install Job Service integration.

## Authentication

The Agent Bus host creates a dedicated owner-private credential at
`<runtime-dir>/task-service/job-service-completion.token`. The Job Service
sender must read that file out of band and send it as a Bearer credential. The
ordinary Agent Bus credential is rejected. Successful route authentication
mints an in-process `JobServiceSystemPrincipal`; neither the principal nor its
credential is present in the JSON schema or delivered message.

The endpoint is loopback-only because it shares the existing Agent Bus
listener, which binds `127.0.0.1`. JS3B must preserve the same-OS-account and
private-file boundary and must not copy the credential into model-visible
arguments, environment, output, or notices.

## API

`POST /api/job-service/v1/completions` accepts strict camelCase JSON:

```json
{
  "schema": "cutex.job_service.completion.v1",
  "eventId": "stable-terminal-event-id",
  "jobId": "stable-job-id",
  "jobRevision": 3,
  "terminalStatus": "exited",
  "resultSha256": "<64 lowercase hexadecimal characters>",
  "targetCutexSessionId": "cutex.<full-uuid>",
  "summary": "optional bounded untrusted data",
  "outputReference": "optional bounded opaque reference"
}
```

Terminal status is one of `exited`, `failed`, `cancelled`, `interrupted`, or
`launch_unknown`. The body limit is 32 KiB; summary and output reference are
each limited to 2 KiB. The output reference is displayed only and is never
fetched or executed.

`POST /api/job-service/v1/completions/query` accepts the same schema string and
`eventId`. Both routes require the dedicated credential.

The response reports `status`, stable `eventId`/`messageId`, `disposition`,
`deduplicated`, and an A4 receipt only after native context persistence.
Dispositions are `pending`, `delivered`, `archived`, `orphaned`, or
`not_found`. `pending` is not an exactly-once model-execution claim.

The event ID deterministically reserves one message ID. An exact replay returns
the existing state. Reusing it with changed semantic content returns
`status=no_write,errorCode=event_conflict` and creates no second logical event.
The durable Agent Bus record is committed before any volatile enqueue.

## Target and lifecycle semantics

Delivery is bound only to the exact durable `targetCutexSessionId`. Redrive
resolves the current runtime generation from the session registry, so an old
runtime/thread is not reused. Offline targets remain durable pending. A later
runtime of the same durable identity can receive the message; a replacement
durable identity and Director rotation cannot inherit it.

Target classification combines two existing exact-ID authoritative reads.
Agent Management roster `retired_at` means permanent Management Close and
terminally orphans the completion. The durable session's legacy `Retired`
spelling means reversible archive only when the roster is not permanently
retired: the lane persists but does not enqueue or auto-online it, and explicit
restore permits normal registration redrive. A successfully read roster with
no exact entry plus a valid persistent durable Agent is supported as
durable-only. Reader failure, missing/malformed durable state, and nonpersistent
records are classification-unavailable and fail closed; they are never treated
as confirmed roster absence. Classification is refreshed immediately before
enqueue and on every redrive.

Completion metadata has no Project or Task identifiers and carries no Task
authority. Delivered values are explicitly labeled untrusted data. Full job
output and secrets are not forwarded.
