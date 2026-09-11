# Private Job presentation — component checkpoint

2026-09-11. **Incomplete, blocked on final native/TUI coherent bundle. Not an integration acceptance or deployment instruction.**

Base `6ba94a592ac8b1f29083b6e1bab55b071c8754dd`, tree `f6e1ae948700523d67b23e5608b55ba945a5d8c5`, product ancestor `d89a1585bb9d2967a1078fa95019c8023af81297`. Initial worktree clean. Wire input is frozen native `a6a9d626232c24c655d3c20c22b17bc89d70a40a`; no native source edits or mixed foundation-server/old-CLI package.

## Implemented at this checkpoint

- Typed presentation append/status, separate native semantic/receipt framing and exact response validation. Status must explicitly contain `receipt:null` for known absence: an omitted field/error is not absence. No input/wake/held retry or business ACK API.
- Reuses existing pinned `ExternalInputClient` connection and occurrence fences, with a separate `presentationVersion == 1` check. Existing exact artifact validation still accepts the old ca580 family only; consequently this checkpoint deliberately cannot enable new presentation against an unreviewed foundation binary.
- Local service configuration `private_job_presentation`, default absent. Strict inner shape: `{"version":1,"recipients":["cutex.11111111-1111-4111-8111-111111111111"]}`. Only explicit canonical durable IDs (max32), no wildcard/name matching or hidden environment authorization. Read using the checked config loader at NEW Job acceptance. Do not apply this sample to live configuration.
- The existing dedicated authenticated Job handler selects the policy; ordinary Job request fields, service token/principal, input after_turn semantics and Job source remain unchanged. Existing/replayed messages never gain presentation retroactively. No ordinary MCP/input display duplicates.
- Bus v5 optional obligation freezes template and source/canonical identity in the same write as new canonical acceptance. Later input Commit establishes the exact native reference; full display payload/target/digest freezes before append. Native receipt is independent of input receipt/state/ACK. New v5 is rejected by the existing old-reader 2/3/4 predicate; recovery-action writes now preserve v5 rather than downgrading to4.
- Existing owned bridge performs bounded display recovery after input delivery/ACK and on empty polls, including input records already marked delivered. At most4 due records per sweep, ordered by next-attempt time/ID, 30s per-record backoff; one failed record does not remove other obligations. No new daemon/store or session activation. Existing canonical store has no pruning; this patch introduces no deletion. Any future pruning must retain unfinished obligations.
- Provider/seat/session occurrence fences rechecked before local display CAS. Native status/lost response reconciles exact frozen identity, never creates a replacement ID. Observations/errors stay in `presentation` fields; original A4→Bus delivered→Job ACK is unchanged and does not await display success.

## Actual template preview (asserted from service canonical projection)

```text
Job 终态通知
Job: job-1
状态：exited
输出读取状态：未观测。
摘要（外部数据）：bounded result data
```

This is plainText; native TUI chooses safe presentation layout. It is not a rendered screenshot. `exited` alone does not prove exit0. Read-output state is not observed by this completion API; therefore the body does not say “not yet read” or “success”. The display body differs from model input. Reference is the actual frozen externalInput message ID, not a guessed MCP invocation. No free-form correlationId/eventType fields are invented on the frozen wire.

## Verification performed

All Cargo outputs/cache/tmp were task-owned Mambo paths. Rust1.95.0, locked/default features, no dependency/Cargo.lock changes. Test invocations used `env -i`, private HOME, explicit toolchain PATH/CARGO_HOME/CARGO_TARGET_DIR/TMPDIR; no operator state/endpoints or native processes used.

- `cargo check --locked --bins --lib`: PASS (`../presentation-check-01.log`).
- `cargo test --locked --lib presentation -- --test-threads=1`: final template run **17 PASS** (`../presentation-tests-03.log`), including7 new tests and10 inherited matching regressions. Earlier14/17 runs retained, not additional distinct coverage.
- `cargo test --locked --lib management::v2::agent_bus_state -- --test-threads=1`: **8 PASS** after adding exact input/A4 validation (`../presentation-store-tests-final.log`). Three overlap the17: final two selections total25 executions /22 distinct tests. Model-free repository tests use explicitly synthetic A4 receipts; they do NOT prove native persistence or real crash recovery.
- `cargo check --locked --tests --bins`: PASS (`../presentation-check-tests-final.log`), existing unused/dead-code warnings retained.
- `cargo fmt --all`, task delta self-review, `git diff --check`: PASS. No unrelated reformat observed. Reviewed semantic deltas, strict parse/null behavior, v5 preservation, frozen target/CAS, source checks and independent ACK path.
- Independent hash comparator: exact fixed native protocol vector (semantic `4e7717dc…`, receipt `29e63d45…`) matches Cutex. Tests also cover UTF8 limits, unknown field/version/privileged source, reference order, explicit policy, no-backfill, original input receipt unchanged, reopen/frozen replay/conflict, spoofed source, bounded work rotation and downgrade rejection.

Failed actions retained: one malformed apply_patch hunk was rejected before edits; a few narrow source-location lookups missed paths and were corrected. No compiler/test failure in the recorded runs. Self-review corrected a missing-vs-null parser ambiguity and unobserved read-output wording before this checkpoint; these are covered by focused tests.

## Named dependency and remaining acceptance

Need TUI owner's final immutable CLI + server + compatible host + aggregate/per-file schema + manifest identities. Foundation appserver alone is not enough. Once supplied: update exact eligible family under new reviewed manifest (never rewrite old markers/receipts), freeze default Cutex/facade bytes, run actual private Job-principal→canonical→native input A4 + independent display→timeline/TUI matrix. No current bundle or artifact manifest is claimed for this checkpoint.

Still unrun: actual new native capability/protocol/private authenticated service path; standalone idle zero-turn; real lost-reply and process death after input ACK or native append before CAS; current-generation/authority race; real UI/PTY, actual display sentinel exclusion on final bytes; mixed-bundle negatives for new pins. The foundation's native-only evidence is reused background, not composed proof. No paid model, auth copy, VM/hj1/pi1/r3/r4 changes, native/Job/Windows edits, live cleanup/deployment or full suite.

Performance limit: shared worker is synchronous; existing RPC timeout is30s. A due sweep can spend multiple RPC timeouts (up to4 records, status then append), plus exact occurrence/artifact checks. Current input ACK is already finished; later input polls can be delayed. This is bounded, not a throughput or hostile-UID guarantee. Pending/blocked reasons are stored as explicit error text alongside frozen/receipt state; final composed tests must verify actual unsupported/reference/error observations remain truthful. No automatic held-turn permission is introduced.

Resource observation: task root ~16GiB (before component build ~14GiB), Mambo free368GiB at preflight (>100GiB floor); ceiling20GiB remains. No new live child processes or credentials; older Human fixtures were neither accessed nor cleaned. Preserve this checkpoint and prior evidence; resume only in the same sole-writer lane. Reject/revert the candidate before exposure, never downgrade a v5 store with an old writer.
