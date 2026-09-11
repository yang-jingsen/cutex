# Job v3 composition r2: pin repaired; CodeMode fixture entrance blocked

## Immutable repair and artifacts

Base `e04cda7053758e717b2fffdf7428c733022c78eb` preserves jv01 failure.
Product `b48a2f63a5e60e6b0c6e6d8ca97e4ae4882dd53f`, tree
`9fe31ce69c255c0572ac0919e9cd94cb5e0dff09`.
Only product file changed: `src/launch/job_mcp.rs`: exact accepted Job pin,
old/wrong eligibility negatives, and manifest-versus-compiled-pin test.
No schema, native/Job code, custody, permission, peer, CAS or fallback change.
New reviews accept only f7bbe3c4/ba1a8d; old reviews/markers are not rewritten.

Default-feature dev binaries are frozen in task-root
`artifacts/job-view-v3-r2/`, manifest SHA256
`e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d`:

- Cutex `2412fb31a83c585e7e130d10f55ac4cdfe7499b416092eda3338a8d1f6dffff8`.
- Facade `0aa3ea1b1fd04b46eafafdf3b538b051f830942b11a6ee8aa31a4848b47a5416` (unchanged bytes).
- Native f8c33add CLI b8307571, b8e9cc3a server 7bc7f3d7, official host 3e85d674, stable schema c2a54d59: full hashes in manifest, unchanged accepted coherent bundle.
- Job f7bbe3c42fbf30bfb5fe379c6f6cd5af951991b9, ELF ba1a8d4f3e0b5f739e666e3f515d40b0c543e9e75953759ff181f1e71b29c521.

Five focused Job launch tests passed. The one normally ignored external-manifest
test was explicitly run and passed against the exact accepted producer manifest.
Default bins build, fmt and diff checks passed. Log: `tmp/job-v3-pin-checks.log`.
Full task-scoped source and fixture diff self-reviewed. No dependency churn.

## Actual jv02 result

Fresh task-owned Mambo user/network/PID namespace; read-only host bind, owned
writable root, loopback fake Responses and existing pre-connect tripwire. No
VM/Human fixture, live endpoint, real credential or paid provider interaction.
The native receiver explicitly used full-access inside this outer private
test containment, with on-request approvals. This is not a new read-only or
hostile-child sandbox proof.

| Boundary | Observed result |
| --- | --- |
| Neutral native thread, adoption, exact review/activation | Passed, zero model calls at creation |
| New Job daemon and adapter peer/custody review | Passed; old pin blocker resolved |
| Runtime readiness, exact action replay | Ready generation 1; same receipt |
| Actual configured MCP | cutex connected with 7 tools; cutex_job with 4 |
| Same-owner CLI attach | Actual terminal opened on bound thread |
| CodeMode call | Refused: `unsupported custom tool call: exec` |
| Job submit / query / output / A4 / ACK / view replay | Not reached |

Private subject `cutex.01a0916c-c7d2-7aa1-8146-387c2b4f0ee4`, native
`01a0916c-c7d2-7aa1-8146-387c2b4f0ee4`.
Evidence: `jv02/launch.json`, `inventory.json`, `terminal.pty`,
`model-requests.json`, private native JSONL and `tmp/job-view-v3-jv02.log`.

The fake provider emitted a custom `exec` call despite the actual advertised
tool schema lacking `exec`. The request advertised ordinary `exec_command`
and `mcp__cutex_job`; the persisted custom-tool output explicitly says
`unsupported custom tool call: exec`. The fixture used
`unknown-private-model`, for which native also displayed its fallback-metadata
warning. These facts prove a fixture/capability entrance mismatch, not that
the warning alone caused it or that real Job execution failed.

The synthetic next response said `Submitted` although no Job had been submitted.
That text is not accepted as evidence. The fixture then timed out waiting for
a submit approval that never appeared. Exactly 2 local fake-provider requests;
no Job state.json, no approvals, no command execution or external input receipt.
This is not pj07 and no restart-timeout investigation was performed.

## Stop and next scope

jv01 and jv02 are both retained. Per the explicit two-roundtrip boundary, no
third run or alternate direct-MCP fallback was attempted. Source pin repair is
complete; composed acceptance remains blocked. The partial test is not a
Human-ready entry. Its provider-input assertion was corrected to inspect only
the native external-event projection, but was not reached.

Small next step: verify the exact native CodeMode enablement/model capability
contract against the supported Cutex review/config surface, then authorize the
minimal fixture correction (or a bounded launcher option only if necessary).
Require advertised `exec` before issuing the fake custom call, and fail
immediately on a tool error rather than returning a success-shaped fake reply.
Use the actual initiating CLI for the interactive turn/approval path or prove
the existing observer routing; this later boundary was not exercised here.
Do not silently inject unsupported shared/profile features or widen approval.

The finally cleanup completed: tracked runtime birth was checked before group
termination, owned child handles were waited, and the private PID namespace
ended. Histories/dummy fixture files remain; no old Human cleanup occurred.
Root 17,346,116,999 bytes and filesystem free 373,723,209,728 bytes are within
24 GiB / 100 GiB. No full suite, new native compaction/cancel/clock campaign,
Windows, VM, provider or deployment claim. Prior accepted proofs and pj07 risk
remain unchanged. Intended pin-repair candidate plus reference blocker, not
full composed acceptance.
