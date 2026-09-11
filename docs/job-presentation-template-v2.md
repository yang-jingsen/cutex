# Job display policy v2 — source candidate

Base: 43ffc74eab58c55b9a5cc112f816d5fade74666e, tree
e1037df0b5ca80c3e0930403bb3f3d55fa11c188. Accepted production ancestor:
5414db97f82f7615d6605925cca282dcccdf75d0, tree c6e7b45549d9cea45cab85a9507e2658bdc26c84.
The base also retains explicitly disclosed feature-only diagnostic/latch changes
and docs/fixture descendants; none is silently relabeled as default-byte evidence.
This task changes only presentation.rs and agent_bus_state.rs plus this document.

## Preview and configuration

Production default remains `private_job_presentation: null` (no display opt-in).
For explicitly selected NEW private events, this local configuration suppresses
an extra terminal Notice by default:

```json
{"private_job_presentation":{"version":2,"recipients":["cutex.11111111-1111-4111-8111-111111111111"]}}
```

`"template":"suppress"` is equivalent. The real existing inbound event and MCP
items remain; there is no additional carrier, model query, or agent-reply edit.

An operator can deliberately select `"template":"service_summary"`. For an
authenticated completion whose actual `summary` is `Passed: 12 · Failed: 0`,
the independent plain-text presentation is:

```text
Job summary
Passed: 12 · Failed: 0
```

Source remains service/cutex-job-service, with the original exact ExternalInput
reference. This example is not fabricated runtime data. No injected exit code,
duration, output-read state, action name, or current-status claim. The summary
is preserved verbatim, including whitespace; absent/blank summary produces no
obligation or fallback prose. Configuration chooses the template, not an LLM or
fuzzy comparison. Explicit summary selection does not promise semantic novelty
relative to the model input. No new Job wire field or service push API exists.

## Persistence and compatibility

- Explicit v1 configuration keeps its exact old template. No automatic upgrade.
- Existing obligations, frozen payloads, IDs and receipts replay using their
  stored template version. No deletion, reformatting, backfill or suppression.
- New summary obligations use existing obligation `version:2`; the ID's framed
  version field and prefix distinguish v1/v2. Native wire/receipt version stays1.
- Bus store remains v5. Its existing old-reader `obligation.version == 1` check
  rejects a v2 obligation; no extra store field or new ledger is introduced.
- Suppression is frozen as absence of an obligation on the canonical record.
  The existing exact-record replay branch cannot add one later, even if policy
  changes. This records the outcome, not a separate policy audit snapshot.
- Unknown config/template/obligation versions reject. A v1 config cannot specify
  a v2 template. Load validation regenerates only the expected template for the
  stored version to check integrity; it never rewrites the persisted payload.
- A4 → Bus delivered → Job ACK is unchanged. Display receipt/absence is not
  execution success or A4, and does not delay the original ACK. Existing bounded
  display retry and occurrence/authority fences are unchanged.

## Verification and omissions

`cargo test --locked --offline --lib presentation -- --test-threads=1`:
21 passed, 0 failed (default features). Four new tests cover explicit config,
suppression/replay/ACK independence, summary receipt/reopen/version conflict,
and missing/blank/exact Unicode summary plus forged source. Existing v1 frozen
template, receipt replay, no-backfill and native hash-vector tests also passed.
These are private Rust/provider-repository tests with synthetic A4, not fresh
native, real Job daemon, provider or VM end-to-end proof. Private HOME/TMP/cache
only; no model calls, auth access or live endpoints. Full task diff self-reviewed.
`cargo check --locked --offline --bins` passed (existing unrelated unused/dead
code warnings); `cargo fmt --all -- --check` and `git diff --check` passed.

Native compact rendering/adjacent grouping is a separate owner's task. Its final
coherent bundle is not yet selected here; this task changes no pins and builds no
release binary. Composed verification awaits separately authorized exact native
manifest. No new VM/manual test request; pv1 and retained auth remain untouched.
Earlier unexplained pj07 restart timeout and O(history) risk remain deferred.

Resource: removed only three verified reconstructible old incremental caches
(cutex-1nuu5yiulrfsf, cutex-1ds6qr9z5gr61, cutex-1mqmz8s71zqfy). Source and frozen
artifacts retained. Preflight root was22,293,848,064 bytes (already above20GiB);
after initial cache removal19,701,710,848. Test compilation briefly reached
21,620,027,392 bytes; stopped adding builds and reclaimed the third cache before
the scoped check. This is disclosed, not a claim that peak stayed below budget.
No native/V8/dependency upgrade, canonical mutation or deployment.
Final retained root20,989,976,576 bytes (<20GiB), filesystem free >100GiB.
Intended use: private source INTEGRATION_CANDIDATE, pending Director acceptance;
not deployed and not an updated frozen DEFAULT5414 artifact.
