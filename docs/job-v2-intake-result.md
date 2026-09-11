# Job v2 intake / structured view result

Private integration candidate, not deployment or full composed acceptance.

Base: `382f68c9838ca05040f877c2f2aadb78965f6e88`.
Product: `8d58216bf31db74ad55b438757bf68af812f057b`, tree `302a892ba1070b5298cc7541567c0e98a924d031`.
Default-feature dev binaries and exact dependency hashes: `../artifacts/job-v2-intake-r1/build-manifest.json` relative to the task root source directory. Native final bundle is f8c33add (server b8e9cc3a), Job f7bbe3c4. No frozen upstream artifacts changed.

## Contract delivered

- Dedicated authenticated Job v1/v2 intake and matching query/receipt schema. Legacy content/hash remain unchanged; same v2 event with changed facts conflicts.
- Bus store 6 freezes the bounded authenticated snapshot, model text and non-model `cutex.job-completion.v1` view before native submission. Older writers reject store 6. Frozen projection version 1 must not be reinterpreted by future formatters.
- Native submit v2 uses independently checked canonical-view/digest vectors. Status/retry/receipt remain v1. Required advertised versions and exact bundle pins are checked; no changed-digest fallback. Legacy delivery refuses v2 mechanical content instead of leaking it into model input.
- Model default contains full jobId once, action label and genuinely known status/code/duration. Missing values are omitted; exited is not assumed successful. Output reference remains a fact, not a clickable file or lookup alias. Producer result digest is authenticated and bound, not independently recomputed from an incomplete producer result snapshot.
- A4, Bus delivered, Job ACK and independent Presentation retain their separate meanings. V2 has no independent custom summary field: default suppression adds no redundant Notice. Existing frozen v1 obligations remain intact.

Expected model text, not a claimed screenshot:

```text
Job completed. jobId: job_0123456789abcdef
Action: action-display-1
Exit code: 0
Observed run: 2245 ms
```

The native renderer receives separate structured facts for a short-ID/action display; query/read still require jobId.

## Verification

| Boundary | Result |
| --- | --- |
| External input canonicalization/version/receipt tests | 13 passed |
| Stock exact bundle/launch tests | 6 passed |
| Canonical Bus store/replay/presentation tests | 12 passed |
| Job-related library tests | 16 passed |
| Cutex completion lane executable tests | 4 passed |
| Tests compilation, default bins build, fmt, diff check | Passed |

Filters overlap; counts are not a unique-test total. Final library/build log: task-root `tmp/job-v2-final-checks.log`.

The executable lane includes an actual private HTTP server process: token authorization, v1/v2 acceptance/query/replay/conflict and no legacy backfill. Store tests use an actual Unix WebSocket transport with a fake native consumer, checking top-level view separation, frozen replay and wrong-source/owner/model rejection. These are not actual native A4, provider sampling exclusion or a real Job daemon composed test. Private fixture bootstrap is not production registration. Existing native exclusion/history/vector and Job timing/outbox proofs are reused from their accepted reports, not relabeled as this task's end-to-end evidence.

Native reference: `cutex-light-core-r1/artifacts/job-view-output-reference-bound-r1/build-manifest.json` and `artifacts/structured-view-v3-r1/` interface/vectors. Job reference: `cutex-job-frozen-completion-facts-v2-r1/artifacts/BUILD_MANIFEST.json` and its `docs/frozen-completion-facts-v2.md`.

## Failures, limits and next boundary

Initial private toolchain selection needed explicit existing read-only RUSTUP_HOME; a missing test-only view initializer was repaired; formatting was corrected. No dependency installation or lock churn. Full scoped source diff reviewed. Resource measurement briefly exceeded 24 GiB by about 159 MiB; removed only 9.76 GB of verified reconstructible inactive task-owned incremental cache, then disabled incremental compilation. No source, immutable artifact or Human fixture cleanup. Final resource measurement accompanies submission.

Next: actual new default Cutex + accepted Job + coherent native A4/model-exclusion/display composition in a fresh private fixture. No VM, paid model, credential, Human runtime, native/Job source, Windows, full workspace, PTY or deployment check was performed here. Existing pj07 timeout remains deferred. Prior security incident disclosures are unchanged; no remediation claimed.

Candidate-only writers are required: Bus 6 and Job store 2 are not downgrade-safe; even Job v1 delivery does not imply an old-writer-safe store. New binary selection requires new explicit review, not old-marker rewriting. No migration or rollback-by-history-rewrite is offered.
