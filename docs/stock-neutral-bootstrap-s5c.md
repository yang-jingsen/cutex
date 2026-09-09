# S5c — neutral native persistence: narrow first-owner acknowledgement gap

Intended use: AUTHORITATIVE_INPUT / private reproducible harness, not product
Create/replace/rotate implementation or release. Parent is accepted S5b
`19db822ad800f99cbd8344426d259ea16412a286`, tree
`b0ad4c471a2255628ba5279bd48393b116022703`. Only this document and
`tests/stock_neutral_persistence.py` change; production behavior is unchanged.

## Decision

Stock can persist and recover a completely neutral thread without a model turn.
However, the proposed **positive first-owner acknowledgement does not work for
a pristine thread**: `thread/read(includeTurns=true)` materializes JSONL, then
returns `-32601: list_turns is not supported yet`. Do not treat that error as a
successful create or use an extra incidental roundtrip as a barrier.

One explicitly error-aware recovery of the **same returned ID** succeeded in a
fresh stock process, with `thread/read(includeTurns=true)` returning zero turns.
This proves recoverable partial persistence, not a positive original creation
acknowledgement. The original operation remains classified unacknowledged in the
evidence. No second `thread/start` was issued.

Recommended next decision: a narrowly owned Core/app-server fix making the
existing read-with-turns persistence barrier publish pending metadata through
the fallible live-thread persistence path before hydration. Do not add a paid
neutral turn, expected-error bootstrap recipe, guessed identity reconciliation,
or a general new lifecycle service. If remaining exactly on the unmodified
stock artifact is mandatory, keep normal managed creation deferred; a deliberate
error-aware recovery workflow would require its own acceptance, not silent
reinterpretation of this failed first-owner acknowledgement.

## Exact source causal chain (fixed U only)

U `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`, tree
`c527c7a5f5f199231dc0d819264e9e2180eb126a`, read using Git objects in the named
kernel reference repository. Relevant paths/lines at that immutable commit:

1. `codex-rs/app-server-protocol/src/protocol/v2/thread.rs:62`: thread/start
   supports ephemeral=false and experimental historyMode=paginated. No public
   caller-selected native thread ID or create idempotency key appears here.
2. `app-server/src/request_processors/thread_processor.rs:1420`: paginated is
   the default when the store supports it; this probe requests it explicitly.
   Start returns runtime metadata, not a persistence acknowledgement.
3. Same file `:2853`: metadata-only read may return the loaded snapshot before
   materialization. Thus includeTurns=false is not a barrier.
4. Same file `:2992`–`:3013`: loaded paginated read with turns awaits
   `thread_store.persist_thread(...Standard)`, then paginated turn hydration.
5. `thread-store/src/local/live_writer.rs:141`, `:309`–`:359`:
   persist -> durable_write -> RolloutRecorder.persist. JSONL completes before
   SQLite history projection; projection failure may warn. This path does not
   flush the LiveThread's pending metadata.
6. `rollout/src/recorder.rs:988`, `:1705`, `:1770`–`:1807`, `:1852`:
   Persist sends a oneshot command; writer opens the deferred file, writes
   SessionMeta and pending items, awaits file.flush, then sends the result ACK.
   Response success after that chain has a real happens-before relationship.
   This is process-death visibility, **not sync_all/fsync/power-loss durability**.
7. `thread-store/src/live_thread.rs:257`: the higher-level LiveThread.persist
   also awaits flush_pending_metadata_update after persist_thread. The direct
   app-server store call above bypasses this step.
8. `thread-store/src/local/thread_history/read.rs:176`: paginated list_turns
   rejects an absent state-DB row or legacy row as Unsupported. The private
   failure has **zero indexed rows for the exact ID**, despite one canonical
   session_meta JSONL record. This explains the observed error without timing.

The narrow prospective repair is a fallible live-thread persistence entry from
CodexThread used by the existing app-server read-with-turns path; propagate both
writer and metadata errors. `CodexThread::ensure_rollout_materialized` currently
returns no Result, so do not use it to fabricate a successful acknowledgement.
Core/kernel source ownership and a new pinned artifact/manifest must be assigned
by Director; this task neither patches nor commissions that work.

Shutdown is not a substitute: recorder `:1716` returns success for deferred,
empty pending history without materializing it. App-server graceful teardown
(`app-server/src/lib.rs:1187`) waits for thread shutdown, but that alone is not
an unconditional empty-thread persistence promise. Metadata update is also not
a neutral no-op barrier: thread_processor `:1888` requires a real field.

Named historical D2 fact was read once from
`325831f6f579bc8a735382fd393c12594dfdf556:docs/tui-d2-native-workflow-gap.md`.
Its includeTurns error / later resumability was not promoted into a guarantee.

## Real protocol and process evidence

Evidence root `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/s5c01`:

| Criterion | Observed |
| --- | --- |
| One neutral persistent native create | One thread/start, ephemeral=false, paginated; exact ID `01a083ab-c9f4-7211-a7e1-1e6eacbf1471` retained |
| Positive barrier before initial creator exit | **Failed**: read(true) returns list_turns Unsupported; never acknowledged |
| Actual partial persistence | Read-only SQLite oracle: zero exact-ID thread rows; one canonical JSONL record, type session_meta |
| Creator death before successful acknowledgement | Owned creator killed/reaped in failure cleanup; creation remains unacknowledged |
| Exact-ID recovery, no duplicate create | Fresh owner resume(excludeTurns=true) succeeds, then read(true) succeeds with turns=[]; graceful stdin-EOF exit 0 |
| Native source truthful | Recovered source=vscode, historyMode=paginated; no source/name/title override |
| Normal first-owner acknowledged exit + fresh resume | Not reached because first barrier failed |
| Creator death after original acknowledged barrier | Not reached; do not claim this positive scenario passed |
| No model/provider network | Zero turn/start, no Responses server/auth. Linux seccomp inherited by stock denies creating IPv4/IPv6 sockets; independent owned Python process verifies both denied, Unix allowed |
| Cutex application integration | Not invoked; no durable adoption, roster, project, seat, or claim edits |

The harness uses private HOME/CODEX_HOME/TMPDIR, scrubbed environment, stdio
JSON-RPC, tracked child handles, and no inherited network descriptors. No sleeps,
filesystem-until-lucky polling, external endpoint access, auth copying, or model
prompt is used. Its timeout only bounds a protocol wait, not persistence proof.
There is no mock native/provider implementation. No Cutex probe was invoked, so
no Cutex endpoint allowlist was needed. No systemd/user service is used.

Commands actually run (scrubbed environment, PYTHONDONTWRITEBYTECODE=1):

```
python3 tests/stock_neutral_persistence.py s5c01
# exit 1, first proposed barrier rejected; failure retained in s5c01-run.log
python3 tests/stock_neutral_persistence.py s5c01 --recover-existing
# exit 0, RECOVERY.json; zero new creates, same ID, empty neutral history
```

The first failure is retained rather than overwritten or relabeled PASS. The
harness was then improved to retain a typed barrier error and clean up failed
initialization; the failing create campaign was not repeated. Syntax compilation
and complete scoped diff/check are the relevant mechanical gates. No Cargo or
kernel build, S4/S5 suite rerun, PTY, paid model, or production acceptance.

Stock executable remains verified 0.153.4 at the unchanged S1 artifact path:
`/mnt/mambo/PersonaProjects/cutex-upstream-lightweight-r1/stock/bin/codex`.
Package SHA256 `a822187e1a2420c61c5926721bfbd878701ed95547c9bb0d4de4498a16ba1821`;
executable `56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da`.
Prior verified provenance is reused; no download or replacement occurred.

## Smallest later Cutex integration contract (proposal only)

1. Existing provider authority/action reservation binds request, current project
   epoch/role and reviewed config; journal native creation **intent before send**.
2. Send thread/start once to one claimed native owner/home. Journal returned
   exact native ID immediately, separately from a successful persistence ACK.
3. Await a corrected positive persistence/readiness barrier. Any error, creator
   death, or lost response retains the action/owner fence and returned ID. Recovery
   resumes that exact ID only; never issue a replacement create to hide uncertainty.
4. Only after confirmed neutral persistence proceed through separately receipted
   explicit formal-name durable adoption, roster/config, optional stock marker,
   runtime claim/registration readiness and optional message/authority transfer.
   Native creation, durable adoption, roster import, and marker activation are
   **not one atomic transaction**. Existing generic stock launch guards stay.

If the creator dies before the caller receives the native ID, current public
thread/start has no client idempotency identity to reconcile by. Keep a no-ID
uncertain reservation for explicit resolution; never guess from cwd/title/name,
profile, creation time, or catalog differences. Automatic recovery of this case
would require a separately approved native client-key/reserved-ID contract.

Current S4 marker activation is Human/root-only. Agent-requested managed stock
bootstrap must not impersonate Human to enable a marker: Director must approve
the narrow provider activation/config-authority policy and any receipt-stage
schema needed before Create/replace/rotate implementation. This report grants
neither that authority nor a marker downgrade/clear operation.

Resources: 3.4 MiB new private evidence, no cache/build increase; existing task
root remains about 8.2 GiB with about 451 GiB filesystem free. All owned native
processes reaped. Existing S2/s46 incident disclosures, S4 PID-time/candidate-
writer restrictions and other private-only exclusions remain unchanged. No live
migration, deployment, auth investigation/remediation, or no-impact claim.
