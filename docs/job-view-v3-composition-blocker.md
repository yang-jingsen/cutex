# Job view v3 private composition: exact Job pin blocker

Base docs `34fd39ae1107002bd6842ed870ec6b308d0c8c3c`; compiled product
`8d58216bf31db74ad55b438757bf68af812f057b`, tree
`302a892ba1070b5298cc7541567c0e98a924d031`.
Consumed default manifest `../artifacts/job-v2-intake-r1/build-manifest.json`
(relative to source), SHA256
`9620f3cfa6ddc358f5ef2155c3bfc195d97223e96d2bc5468c34eef764402550`.
Cutex `25c14a98507ea3624dd64bdc5729e74d9617d98275fb2590ff8e8c056d0dce5b`;
facade `0aa3ea1b1fd04b46eafafdf3b538b051f830942b11a6ee8aa31a4848b47a5416`.

## Observed, not inferred

One new owned fixture `../jv01` ran under private bwrap user/network/PID
namespaces, read-only host filesystem and task-owned writable root. Existing
loopback pre-connect tripwire was reused. Private neutral native creation,
positive persistence read, durable adoption, exact native bundle review and
activation succeeded. Bus/Management and the actual new Job daemon listened.
No live endpoints, credentials, VM or Human fixtures were accessed.

The actual root `review_runtime` HTTP request with the new Job descriptor
returned HTTP 500:

```text
owner_action_required: unsupported Job adapter bytes
```

Authoritative cause: `src/launch/job_mcp.rs:8` still pins
`d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2`
(old Job f3bc9c8); `JobMcpDescriptor::review`, lines 51–54, rejects every other
adapter digest before peer validation or runtime spawn. The requested accepted
Job f7bbe3c4 has verified ELF digest
`ba1a8d4f3e0b5f739e666e3f515d40b0c543e9e75953759ff181f1e71b29c521`.
The existing peer check also requires the daemon executable to equal the
adapter, so mixing an old adapter with the new daemon is not a supported remedy.

Native composition: f8c33add CLI b8307571, b8e9cc3a server 7bc7f3d7, official
host 3e85d674, stable schema c2a54d59, exact identities in the consumed manifest
and native `artifacts/job-view-output-reference-bound-r1/build-manifest.json`.
The manifest names the new Job but does not change the compiled Job eligibility
check. Thus the previous intake result is not a launch-ready composed bundle.

## Finite outcome

| Criterion | Actual result |
| --- | --- |
| Neutral creation / adoption / native activation | Passed, zero model calls |
| Actual new Job daemon startup | Passed; no Job submitted |
| Reviewed Job runtime readiness | Blocked by exact old adapter pin |
| Core CodeMode submit/query/read, A4/ACK, TUI/replay | Not reached |
| Full source / native / Job changes | None |

Durable generation remained 0, launch claim absent. Fake provider request log
is empty; no Job state.json was created and submission was never reached. One attempt only; no automatic repeat or
old-binary substitution. This is not the unexplained pj07 restart timeout.
The script's `finally` completed and owned subprocess handles were stopped and
waited; the private PID namespace ended. Private history, dummy credential
files and failed evidence remain; no Human/runtime cleanup was performed.

Failure log: `../tmp/job-view-v3-jv01.log`; private state: `../jv01`.
`tests/job_view_v3_composition.py` is a partial harness, not a passed test or
Human-ready entry. Later-stage assertions and approvals were not exercised.
Its proposed all-request outputReference sentinel assertion must be narrowed
before continuation: normal MCP query results may legitimately contain that
reference. The native external-input projection, not unrelated tool output,
is the relevant no-view-leak oracle. No claim is made from unexecuted checks.

## Smallest next decision

Authorize the precise Job eligibility pin/test update to accepted f7bbe3c4,
preserving launcher/peer/custody/CAS guards and explicitly deciding new-review
compatibility (no old marker rewrite). Rebuild default Cutex/facade, freeze a
new manifest, then resume this bounded composition in a new private fixture.
This task forbids product changes, so no patch or bypass was attempted.

Task-scoped fixture/doc diff reviewed; whitespace check passed. No build,
dependency or broad suite. Retained root 16,878,441,135 bytes; filesystem free
374,192,701,440 bytes, within 24 GiB / 100 GiB limits. Intended reference blocker
only, not composed acceptance, deployment or VM preparation approval.
