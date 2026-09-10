# R4 bounded error-observer reproduction — terminal failure, no retry

Base63db0420fc3056b37ea8139b10d90c8d49bfdbb0. Fixture/docs only; all product
bytes unchanged: productionc69241c/tree76aa102d, default Cutex908ab036,
facade3b7b3736, native0c425, Jobf3bc9c8. Full pins in task-root
`artifacts/reviewed-aemeath-r1/build-manifest-v2.json`,
SHAe68255843e477012bba56e92d25bb92a5c199fd8999749f85642a303934d6326.
No rebuild, product policy change, forced-direct projection or held-message retry.

## Authorized fixture correction

Read native-owner `artifacts/terra-completion-error-r1/RESULT.md` and reused its
fixture-only extractor. The observer now reads nested `params.error`, retains
sanitized codexErrorInfo/willRetry/status/structural markers BEFORE deciding to
stop, and handles failed turn/completed separately. Intermediate willRetry=true
does not clean up or initiate any request. BadRequest/function_call_output
clues still stop immediately for the source owner. No original error message,
additionalDetails, headers, auth, URL or dynamic identifier is recorded.

Five synthetic checks passed (four supplied cases plus non-premature-cleanup
decision), with output retained as r4-offline-observer.json. These are NOT an
actual retry event test: r4 emitted no observed intermediate retry event.
AST syntax and scoped diff checks pass. No unchanged Rust/native suites rerun.

R4 used a fresh owned private guest root, healthy paginated Job schema before
generation, one corrected Job-focused Human turn and one normal completion turn.
Observation budget was5minutes per turn/10minutes model phase, with no manual
retry or retry-default changes. Existing ordinary approval remained: exact
submit only, plus optional exact current Job stdout read approval if requested.
State polling only observes Job files; it never generates a model polling turn.
Old r3 state and hold were not readied/released/retried or assigned a fake ACK.

## Actual result

| Boundary | R4 observation |
|---|---|
| Discovery | cutex_job connected, nonempty submit/read_output schemas |
| Model/effort | gpt-5.6-terra / reviewed low, no fallback |
| Human turn | One actual Core MCP submit, completed; model replied Submitted |
| Job process | One Job exited0,36 stdout bytes: private-read-success newline real-job-output; stderr0 |
| Subject/sandbox | Exact native/current runtime matches review; managed origin/read-only receiver |
| Native A4 | Independent rollout commit receipt eir1_cfb84f1a521a89a82630c233a4ccfb31713f3ead3b859713344e7f1a70d94a92 |
| Completion turn | Started, then error with **willRetry=false**, **code=other**, HTTPstatus=null, messagePresent=true, additionalDetailsPresent=false, markers=[], misalignmentPresent=false |
| Final result | Stopped on explicit terminal event; no read_output/final response |
| Durable delivery | Native commit/claim/hold(request_uncertain); Job accepted_pending, NOT acknowledged/delivered |

This is a real terminal notification, unlike the unknown retry flag in r3.
It does NOT identify provider rejection, network failure, BadRequest or malformed
function_call_output: none of those clues was retained. The lossy approved
extractor cannot reconstruct unknown raw text; do not infer it. No further
log-search campaign or new generation was made to fill that gap.

Job: job_2df7cc80b9384e9db25281e489edd359. Original native receipt and independent
Job/stdout/runtime comparisons are retained in r4-independent-oracle.json.
A4=context persistence, not successful model completion, Bus/Job ACK or business
acceptance. No such facts were synthesized. Normal transport retries remain
native-owned; exact internal HTTP count is unknown.

Two turn starts, one completed turn, three usage updates. Cumulative r4 input
43647/cached37120/output268(reasoning106)/total43915. Prior samples remain
separate; combined observed r2+r3+r4 total115475. Dollar cost unavailable.
The failure occurred before the observation deadline, with no manual retry.

## Cleanup, evidence, remaining decision

Exact staged and native guest auth files are absent, verified after finally;
recorded owned runtime executable is absent. No host auth/config writes or
syncback, native/Job source change, production endpoint, PRH or deployment action.
Guest upload stays under8GiB with50GiB free; no build/cache increase this task.

Evidence under task-root artifacts/reviewed-aemeath-r1/evidence:
r4-observations.json, r4-cleanup.json, r4-independent-oracle.json and
r4-offline-observer.json. Actual provider/Core/adapter/daemon/private service
paths, not fake model output or hand-authored Core metadata. Static tool-mode
inference/final serialized request limits remain as previously disclosed.

Return blocked/partial. Actual submit/process/output/native A4 remain proven;
read_output/final response and durable delivery ACK remain unproven. Director/
native owner must choose the next narrow diagnostic for terminal codeother;
no input-representation workaround, new auth scheme or paid retry is justified
by this result alone. Previous risks and deployment/profile/PRH/Windows gates
are unchanged. Preserve both held samples; no automatic replay or downgrade.
