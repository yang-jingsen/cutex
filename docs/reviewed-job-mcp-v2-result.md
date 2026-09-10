# Reviewed Job MCP runtime v2 result

Base `8154450b219ebeefe5c621dc16de25916814fdc6` / tree
`a04fbebc02359dbd22fff308e2ad797d12f74b30`, descendant of accepted Linux1a71.
Production delta is confined to `agent_management/stock_runtime.rs`; the other
changes are private probes and documentation. No native/Job/Windows source edit.
Compatibility contract: [runtime v2](reviewed-job-mcp-runtime-v2.md).

## Frozen default bytes and changed-boundary proof

Built from production source `0ab546c80ab3350a5381e98448afda2082f2abd4`, tree
`a33d64ed8b674a4a1d13efbc345c452f56e2e3de`. Later harness/docs commits do not
change either executable's source. Default features, no failure-hook feature.

| Component | SHA256 |
|---|---|
| Cutex | `1d512b9438109d182d57f80c7f3dc75a4930a132f3db9f259d9c9dedfa8d12e2` |
| cutex-mcp | `e124c89049515f3548f96a952412a915979533f02db11013825db8d647c75f41` |
| Native0c CLI | `f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4` |
| Native0c app-server/helper | `9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde` |
| U host | `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45` |
| Schema | `459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9` |
| Job f3bc9c8 | `d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2` |

Guest `job-launch-r1/v2` uses the exact new Cutex/facade and unchanged pinned
native/Job. A real root restart review took45.149seconds. Natural registration
changed only the two telemetry timestamps; original/before/after digest remained
`2aac356f3f3adc58a8d854bb265f931a5ff989c2bc0095e6ee42687b4a80e2fe`.
Exactly one confirmation reached Ready generation2, preserving durable/native
identity; exact replay returned the same receipt. No review refresh/reissue,
heartbeat injection/pause or persistence sleep.

After restart, actual Core-selected configured Job MCP submitted/replayed one
Job, queried/read its output, read the private file and failed both attempted
writes. Exit0, stdout43bytes/stderr128bytes; wrong cwd was rejected. One Job,
seven fake Responses requests, no manual metadata/grant signing. Completion
became delivered with original native A4
`eir1_154807f6c5e2047d3b0a3aad6a6ae855c3b620571f4dc4c5c7fc9fe8e3d9c99f`.
A4 is a context fact, not model/business execution success. The actual shell
exit/output is a separate Job fact.

Create c3 `Some` passed actual root review/authorize -> authenticated Director
HTTP -> neutral native ACK -> durable adoption -> Ready generation1. Exact
replay returned the original receipt, explicit formal name/profile remained,
neutral history was empty, zero model calls, and native inventory reported
connected `cutex_job` with submit/query/read_output/cancel. This is native
discovery, not a claimed model-selected Management create.

Separate c4 `None` passed on the same exact bytes with its cwd trusted before
review: one durable/native identity, Ready generation1, exact replay, empty
neutral history, zero model calls. Native inventory contains only `cutex`, not
`cutex_job`. No Job descriptor was silently inherited from the Director runtime.

Seven negative-only stdio requests against the same real adapter/Bus/daemon
rejected missing metadata, foreign thread, external policy, forged subscriber,
empty argv, wrong API credential and missing occurrence without creating a Job.
These deliberately malformed transport inputs are NOT substitutes for the
positive actual Core automatic-metadata proof above. No grants were hand-signed.

## Checks and preserved failures

- Default `cargo check --locked --bins` and Cutex/facade build passed.
- `cargo test --locked --lib runtime_review`:3 pass.
- `cargo test --locked --lib job_mcp`:5 pass.
- `cargo test --locked --lib stock`:10 pass (includes the same3 new tests).
- `cargo test --locked --lib bootstrap`:8 pass.
- Total26 executions /23 distinct tests; fmt and diff checks passed.
- First new test compile failed on an unnecessary ambiguous `.into()`; fixed
  without product behavior change. Original log retained.
- Create fixture c2 reused the Director cwd and received `active_agent_collision`
  with `no_write`. No native create was retried after an uncertain result. c2
  is retained; corrected c3 uses distinct private successor directories.
- c3 `None` captured a native ID but failed closed at adoption with
  `bootstrap_adoption_uncertain: stock reference changed`. Comparing every
  pinned file showed only shared config changed: native added the missing
  `other-cwd` trusted-project entry. The original action/native evidence is
  untouched and not retried/relabelled successful. Separate c4 checks None in
  the already pretrusted `new-agent` fixture directory; no pin relaxation or
  shared-config change after review is allowed.

All completed fixture processes exited through their owned-handle cleanup;
the VM remains running. Original c2/c3 files and captured uncertain native
history are preserved. No original action was changed into a duplicate create.

The unit/provider tests prove full-record legacy hashing, v1 wire omission,
unknown-version refusal, historical Ready replay without runtime calls or file
changes, and changed version/config/authority replay conflicts. V2 tests compare
the exact full serialized object minus only two keys and mutate identity,
revision/generation, native ID, profile/name/cwd, permissions, claims/PID/binding,
lifecycle and other configuration fields. These are not claimed as every field
being independently exercised by a real native process.

Earlier actual Core fullaccess a2, final-byte approval decline d1, and r2 actual
readOnly/helper/Job proof remain immutable and are reused where unchanged.
The original heartbeat failure/diagnosis remains in its original document.
No blanket OS-egress, hostile-same-UID, real-provider, full suite, Windows, PRH
cutover, migration, live deployment or production credential acceptance.
Prior S2/S46/PID/socket risks remain disclosed, not remediated here.
Temporal stale-occurrence/retirement race permutations were not independently
repeated end to end on these bytes; missing/foreign occurrence refusal and
unchanged provider guards are not labelled equivalent proof of every race.
Validation remains expensive (the observed review took45seconds); this task
does not weaken hashing/custody checks or introduce a performance cache.
Resource observation: Mambo task13GiB/free376GiB; guest upload5.2GiB/free51GiB,
within the respective20/8GiB budgets and100/30GiB free-space floors.

## Handoff

**Feature integration candidate only**, subject to Director acceptance. The
minimum remaining external gate is separately authorized real-provider and
deployment/PRH pairing validation; this result does not authorize it. Do not
mix old writers, rewrite old reviews/history, or treat rollback as safe history
downgrade. Before exposure reject/revert this candidate; preserve experimental
stores and reviewed artifacts for explicit recovery instead of clearing markers.

Compact evidence is in owner root `artifacts/reviewed-job-launch-v2/`:
`build-manifest.json`, `v2-result.json`, `v2-review.json`, `c3-some.json`,
`c4-result.json`, `negative-adapter.json`, and preserved c2/c3 refusal receipts.
Guest full fixtures remain under `acceptance-upload/job-launch-r1/{v2,c2,c3,c4}`.
No claim that direct negative transport is an automatic Core call, that A4 is
business success, or that every stale/retired temporal permutation was exercised.
