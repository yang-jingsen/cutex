# jv04: actual Job / canonical view / CLI replay passed

Private composed acceptance evidence, not deployment or real-provider acceptance.
Exactly one authorized jv04 run; jv01–jv03 failures remain unchanged.

## Immutable inputs

Base `1c04410966fda3fb5c8693ade206f3cd27d73109`, tree
`2d7a8fe63ca75f2b95371e7da9168e781ba3f12a`.
Fixture correction committed **before execution** as
`6a80d32fef793b51eeeaa4fb08e65f8d459fe087`, tree
`4444ccfaee95eb644c4f27d8f5158ec70d2af8e2`.
Product unchanged `b48a2f63a5e60e6b0c6e6d8ca97e4ae4882dd53f`, tree
`9fe31ce69c255c0572ac0919e9cd94cb5e0dff09`; no build or pin change.
Exact manifest `artifacts/job-view-v3-r2/build-manifest.json`, SHA256
`e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d`:
default-feature dev Cutex2412fb31 / facade0aa3ea1b; native f8c33add CLIb8307571,
b8e9cc server7bc7f3d7, official host3e85d674, stable schema c2a54d59;
Jobf7bbe3c ELFba1a8d4f. Full identities are bound by that manifest.

## Corrections and evidence level

The bounded fixture now preserves standard/Lite namespace and tool kind,
uses valid response IDs and raw custom JavaScript, correlates exact call/cell
outputs, and validates real MCP results rather than Script-completed alone.
Yielded execution can only wait on its original advertised cell. Submit is
once-only; failure cannot produce Submitted. Query/read each occur once after
the exact persisted canonical completion. Fresh approval prompts are used.

Eight offline tests passed: captured actual Lite and standard declarations;
namespace/kind/schema negatives; malformed/error/missing/duplicate outputs;
yield→wait→completion; receipt/action matching; output bytes/ranges;
old completion/prompt false positives; failed script cannot acknowledge success.
Yield and negative cases are **offline synthetic tests**, not runtime fault proof.
Full four-file fixture diff reviewed; `git diff --check` passed.

| Criterion | Actual jv04 result |
| --- | --- |
| Neutral/adopt/review/Ready, exact launch replay | Passed; generation 1, zero creation model calls |
| Configured native CodeMode / Job MCP | Actual initiating CLI; terra/low, s6-private fake Responses; four Job tools connected |
| Job submit and normal approval | One submit, one observed submit approval, actual committed receipt |
| Producer terminal / frozen v2 facts | One Job, exit 0; measured run 239 ms; original outbox acknowledged |
| Canonical model / non-model view | Default full Job ID once; producer facts equal frozen view; view excluded specifically from external-input provider projection |
| A4 / Bus / Job ACK | One persisted native commit pair; same original receipt across native, Bus and Job outbox |
| Query / read_output | Actual calls; stdout 13 bytes `v3-job-output`, offsets 0→13, gap=false, truncated=false |
| Native CLI display | Actual PTY action label, short ID, Job completed, code 0, Observed run 0.239 s, compact output shown |
| Reconnect and resume display | New connection and CLI, same owner; identical timeline/original receipt, no second Job or Notice |
| Ordinary runtime process restart | Not run; reconnect is not restart recovery |

Four local fake-provider requests, no paid calls. Fake responses selected actual
tools; they did not supply grants, completion records, ACKs or Job outputs.
Independent read-only `python3 -B tests/job_view_v3_history.py jv04` passed
against producer state/outbox, Bus canonical state, native rollout and captured
provider requests. Normal MCP results may contain raw facts/outputReference;
the exclusion assertion correctly targets only native external-input projection.
Observed stderr metadata is 103 bytes; no claim of empty stderr.

Actual display excerpt (terminal captures retained, no fabricated screenshot):

```text
Submitted job · v3-private-job · job_4a8b82c7…c9de
Job completed · v3-private-job · job_4a8b82c7…c9de
  Observed run 0.239 s
Queried job · v3-private-job · job_4a8b82c7…c9de
  Exit code: 0
Read job output · v3-private-job · job_4a8b82c7…c9de
v3-job-output acknowledged
```

Model completion text was independently observed as:

```text
Job completed. jobId: job_4a8b82c78cfb40cc8c9a2b800dc1c9de
Action: v3-private-job
Exit code: 0
Observed run: 239 ms
```

## Retained evidence

Root `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/jv04`:

| File | SHA256 |
| --- | --- |
| PASS.json | a246353a00797d076289af8de0ce34b9a97f13b940f781660a3237743db2083a |
| model-requests.json | e037ef8fec73bd6eba70514530318013d81fa42009f46f9a1605657f4f71307a |
| terminal-first.pty | 61b71b4465392bdf82d06ac7e813362d967b64ad587ef784266d95bf3557d65e |
| terminal.pty (replay) | 0d89ebe79e62973e4ba1cb54a4cc3116c0b8af68f1945c2f03ed5cfd149de47d |
| timeline.json | 9ab106fc7cfe0e8a9e4964c82eaa58c5f24cf7f6a31e8ebb88246ffe20b37765 |

Exact receipt `eir1_061a5878871473caceb670ef452e70f46c17844e1542cc82f38b6ec49b3d72f6`;
durable `cutex.01a09193-737f-7ba3-bd16-d917e5c76225`, native
`01a09193-737f-7ba3-bd16-d917e5c76225`. Additional actual submit/query/output,
launch, inventory and canonical histories remain in that private evidence root.

## Limits, cleanup and next use

No new jv04 error. Prior pin mismatch, Direct-mode fixture and Lite-declaration
failures remain documented, not relabeled as passing. No full fault campaign,
runtime restart, real-provider, Windows, VM/Human or production proof. Reuse
unchanged native compaction and Job clock/cancel/drain evidence; no new claims
about those boundaries. Accepted pj07 timeout remains deferred.

Owned CLI/services/native children cleaned by retained Popen handles and native
birth/executable/cwd guard; enclosing PID/network namespace exited successfully.
Histories preserved. No existing Human fixture, auth or live service touched.
Receiver full-access/on-request was confined by the existing outer private
bwrap user/network/PID namespace and read-only host; not a read-only receiver
or hostile-child sandbox acceptance claim.

Retained task root 18,313,703,424 bytes (<24 GiB); filesystem available
373,700,190,208 bytes (>100 GiB). No new build/cache or broad cleanup.
Recommended next step: Director acceptance followed by separately authorized
VM Human preparation using this exact composition. No automatic deployment.
