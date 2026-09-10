# Exact review-freshness diagnosis and independent read-only proof

Director-authorized continuation of `fc31cc0e4188080f0b939ca93a9f88c196e10f55`.
**Decision input, not integration acceptance. No production/CAS logic changed.**
Existing fullaccess a2 and final-byte approval-decline d1 evidence is reused.
Original a1/a2/decline1/r1 results are preserved.

## Proved stale-review cause

One real root `review_runtime(restart=true, job_mcp=...)` was observed; **no
restart confirmation or retry was sent**. The fixture validated its compact,
field-order-preserving JSON digest against the actual provider's offline review
before launch. Source uses `serde_json::to_vec(record)` for the digest and the
same typed record with pretty JSON for persistence; no fields were omitted from
the oracle. All snapshots are dummy private records, with no credential values.

| Boundary | Elapsed seconds | Record revision / generation | Digest prefix |
|---|---:|---|---|
| Before review | 0.000 | 4 / 1 | `7f36bc74a3d7` |
| Natural registration file event | 23.296 | 4 / 1 | `2dab30921d37` |
| Review returned | 46.299 | 4 / 1 | current `2dab30921d37`; returned review still `7f36bc74a3d7` |

The **only** changed record fields were:

- `last_seen_at`: `2026-09-10T17:55:25.341127570+00:00` → `2026-09-10T17:55:55.541241700+00:00`.
- `updated_at`: `2026-09-10T17:55:32.242829194+00:00` → `2026-09-10T17:55:55.541241700+00:00`.

Formal identity, native/home mapping, lifecycle, profile/configuration, revision,
runtime ID/generation/PID/binding and claim fields were unchanged. The returned
review's configuration, contract, normalized Job descriptor, authority digest
and project matched the original launch review. This is observation-only churn,
not a newly authorized identity/configuration/occurrence change.

Source ordering explains the measured result:

1. `agent_management/stock_runtime.rs::review_stock_runtime_locked` loads the
   record, then validates bundle/native/configuration, and hashes that earlier
   record.
2. `agent_management/explicit_launch.rs::ReviewRuntime` additionally validates
   bundle and Job evidence before returning the review.
3. `app_server/bus_bridge.rs` normally refreshes registration every30seconds;
   `session/runtime_reconciliation.rs` refreshes these two timestamps even when
   the occurrence and durable specification do not change.
4. `execute_stock_runtime_locked` compares the original full-record digest.
   Thus this returned review is already unusable under the existing fence.

The total review RPC took46.299seconds and spanned the natural refresh. There is
no per-phase timing, so do not attribute all46seconds specifically to hashing.
The observation proves the snapshot/validation ordering problem without a
sleep, artificial heartbeat, modified refresh cadence, cache or reissued review.
It confirms a concrete mechanism for stale reviews; the earlier r1 review was
not retained, so its individual field-level history is still not reconstructed.

## Independent actual read-only Job result

The first Ready owner was kept unchanged. Real Core-selected configured MCP
calls, with Core-supplied metadata and the real Job grant issuer/daemon, passed:

- Read `probe-readable` and returned `private-read-success`.
- Attempted two writes in the private cwd; both failed, neither file exists,
  and the controlled shell completed with exit0. This was execution, not a
  startup failure. stdout43bytes, stderr128bytes.
- Repeated identical submit action produced exactly one Job and one successful
  controlled `/bin/sh` exec in the owned trace.
- A separate existing wrong cwd was rejected with
  `sandboxCwd differs from the bound request cwd`; no second Job.
- Actual query/read_output returned output, and completion reached Bus delivered
  plus original native A4 receipt `eir1_18f364672211244464bb78e8e370f36c5d8d8f7d657ced708e59f4ec9ec41d00`.

The trace covered only the owned daemon's process tree. Selected actual sandbox
metadata forwarded to the CLI was `permissionProfile.type=managed`, restricted
filesystem with root read access, `network=restricted`, exact
`sandboxCwd=file:///home/cutex-linux-test/acceptance-upload/job-launch-r1/r2`, and
`useLegacyLandlock=false`. Restricted network is an observed policy value, **not
a new network-denial test**.

Actual executed components:

| Component | SHA256 |
|---|---|
| Job daemon/adapter f3bc9c8 | `d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2` |
| Outer native0c CLI | `f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4` |
| Actual nested sandbox helper | `9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde` |

The observed `codexLinuxSandboxExe` was the owned native-home arg0 alias
`tmp/arg0/codex-arg0TOBbf0/codex-linux-sandbox`; it resolves to the frozen
`vm-r1/native/bin/codex-app-server` with the helper hash above. Both paths occur
in the exec trace. `/usr/bin/bwrap` and the harmless shell/cat command were also
executed. This is coherent0c pairing, not an inferred K compatibility claim.
Existing guest bwrap/AppArmor setup was reused unchanged. No OS or privilege
configuration was altered; no hostile-same-UID or full OS-egress claim.

## Minimum proposed fence decision — not implemented

Recommend an explicitly versioned **runtime-review** semantic digest that omits
only `last_seen_at` and `updated_at`, retaining every other record field plus the
existing revision, generation, authority, Task, configuration, bundle, custody
and actual-owner/liveness checks. New version must be explicit/fail-closed;
legacy reviews retain their original full-record algorithm and historical
receipt bytes. Same-action changed semantics must still conflict. Do not
silently reinterpret old/pending confirmations or refresh them at execution.

This is a CAS policy/compatibility decision for Director before implementation.
An alternative is snapshot/validation locking that preserves the full-record
algorithm, but long-held locks would delay registration and require separate
lock-order/liveness analysis. No lock or digest change is included here.

## Identities, checks and remaining gates

Actual final default Cutex `94d8392840d5a5a0922189d8d81e22e0d4f2fce75b2139899475d8538b3300ca`,
facade `7f78bd66b124c50d6eddc3799d0b44de481ba0df2a1025604820e851e2f223bf`.
Native0c425, Uhost/schema and Job pins unchanged from prior manifest. Seven fake
Responses protocol requests, zero paid/real-provider calls. Job uses actual
runtime/thread binding; no claim that Job verifies a separate turn/generation
token. No manual metadata or grant signing substituted for Core.

Evidence: guest `acceptance-upload/job-launch-r1/r2/{RESULT.json,
review-diagnosis.json,review-observations.json,returned-restart-review.json,
job-exec.trace,model-requests.json}`. Local compact copies are under
`artifacts/reviewed-job-launch-r1/r2-*`. Raw trace is retained in the guest;
only selected nonsecret metadata/paths are reported.

Only the fixture and this report changed since fc31cc0. Python syntax and
scoped diff checks passed. Prior default build/check/fmt, five Job tests and
15 stock/bootstrap tests reused; no production source change requires rebuilding
native/Job or rerunning those suites. Fullaccess and decline proofs not repeated.

Open: successful reviewed restart after an accepted fence repair; actual
Create-Some/None discovery and remaining configured identity/metadata negative
gates from the original task. This diagnosis and readOnly proof do not declare
the whole feature complete. Windows, real aemeath/provider, production PRH,
services/profiles/backup, deployment and mixed-writer migration remain excluded.
Private owned child cleanup completed, guest remains running; prior risks and
candidate-only compatibility restrictions remain unchanged.
