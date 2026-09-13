# Frozen Job completion facts v2

This producer contract is additive to Job core v1. It does not change command
authority, delivery acknowledgement, cancellation, or process containment.

## Execution observation

New Job records may contain `execution` with basis
`runner_release_to_wait_v1`. Start is observed only after the private runner
gate is successfully released. Exit and monotonic duration are observed
immediately after a successful child `wait`, before sentinel and output-drain
collection. The duration is runner wall elapsed time, not CPU time or complete
descendant lifetime. Clock failure omits the affected epoch field; a wall-clock
reversal never changes the monotonic duration. Wait failure and restart retain a
proved start but do not invent exit time or duration. Legacy records remain
absent rather than being backfilled from created/updated timestamps.

Store version 2 is written once execution observations or v2 outbox data exist.
This reader accepts versions 1 and 2 and does not backfill version 1. The older
writer already rejects version 2.

## Frozen completion wire

The service default remains `cutex.job_service.completion.v1`. An operator may
select v2 explicitly with `--completion-v2 ENDPOINT TOKEN_FILE`; it must only be
enabled with a compatible Cutex receiver. Every new v2 terminal outbox freezes
the complete request in the same state transaction. Retry and restart resend
those bytes; later configuration or Job projection changes cannot regenerate
them. An outbox without `wireVersion` remains the exact legacy v1 builder and
digest path.

The v2 request field order is:

```text
schema, eventId, jobId, jobRevision, terminalStatus, resultSha256,
targetCutexSessionId, facts, outputReference
```

`facts` has `factsVersion:1`, `actionId`, optional actual non-signal
`exitCode`, optional bounded `terminalReason`, optional `execution`, and final
stdout/stderr `{retainedBytes,observedBytes,truncated}`. It contains no argv,
environment, credentials, sandbox details, or process identity. Missing facts
are omitted, never serialized as false zeroes.

The v2 result digest is SHA-256 over:

```text
UTF-8("cutex:job-result:v2") || 0x00 || fixed typed result JSON
```

The typed result JSON field order is `jobId,revision,state,exitCode,reason,
stdout,stderr,outputReference,facts`. Legacy top-level `exitCode` and `reason`
retain JSON nulls when absent; optional fields within facts/execution are
omitted. The result excludes its own hash and mutable delivery counters.

Published test vector: `job_0123456789abcdef`, revision 3, action
`action-display-1`, exited code 0, execution 10100→12345 ms with monotonic 2245
ms, stdout 9 observed/5 retained/truncated and empty stderr produces:

```text
a6beba755d57cffcadf03cd758d661b300febe5183d5fcd016321a37fe969ce1
```

The existing Job completion endpoint, dedicated credential, stable event ID,
query reconciliation, and A4/orphan/archive dispositions are unchanged.
