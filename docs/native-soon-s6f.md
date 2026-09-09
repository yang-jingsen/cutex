# S6f private Soon / coherent CLI integration

Base: `2af181a87b54a193e8b8bf1a2ad78cd544475beb`.
This is not a deployment or mixed-writer migration contract.

## Explicit compatibility boundary

New bundle version 3 pins native commit
`a83dbb47ba6aa775f5d4b679fafc532c4db74c7f`, direct server, adjacent CLI,
official companion, schema and private facade/config hashes. It requires a NEW
root-reviewed durable explicit-launch contract version 2. Existing markers
cannot be replaced or cleared by this adapter. Legacy bundle versions retain
their exact pins and behavior; an arbitrary capability Boolean cannot authorize
a different executable. Initialize must report externalInputVersion 1 and
externalInputDeliveries containing `soon`. Older ingress remains limited to
after_turn/passive; plain official stock remains registration-only.

`cutex session stock-attach EXACT_DURABLE_ID` selects the pinned coherent CLI
and the already-bound owner/thread. It does not start a second native writer.
The receiver's reviewed permission setting is selected at launch using the
native named permission profile. Resume must not supply the legacy sandbox
override that erases this named profile. Readiness verifies the actual active
profile; attach does not change receiver permissions. Remote native
`--sandbox/-s` is not a supported receiver configuration mechanism.

## Business projection

Task assignment, follow-up, completion and watchdog messages use their existing
provider policy, authenticated metadata and exact current durable recipient.
Recipient-scoped deterministic Bus IDs separate assignment/follow-up/watchdog
families. Fresh projections must postdate explicit activation and match the
authoritative outbox. Pre-activation ambiguous records require review; they are
not silently migrated. Frozen canonical bytes and original A4 receipts survive
replay. Existing Task fact -> Bus CAS -> transport ACK ordering remains.

| Event family | Existing mode | Recipient / current evidence |
| --- | --- | --- |
| assignment.v2 | Soon | exact assigned Worker; real busy-turn run03 |
| worker_followup.v1 | Soon | exact Worker; real request_changes run03 |
| ReviewReady | AfterTurn | current completion authority; real run03 |
| accepted TerminalClosure | AfterTurn | current completion authority; real run03 |
| blocked / declined / attempt aborted | Soon | blocked real h2; decline/abort provider tests |
| retries exhausted / owner action required | Soon | existing completion policy; provider semantics retained |
| fail / cancel TerminalClosure | Soon | cancel real h2; fail policy retained, not native E2E |
| watchdog | authoritative Soon / AfterTurn | real run05 Worker reminder and project Director escalation |
| Management bootstrap / rotation | unchanged | not stock bootstrap acceptance |
| Interrupt / durable sleep control | unsupported | no silent Soon or AfterTurn conversion |

Templates retain concise event/action/contract or decision text. Required
Assignment ID occurs once; receipt/digest/generation are not prose. Distinct
ReviewReady notifications may legitimately have equal text; uniqueness is
checked by native message ID, not text comparison.

## Evidence and remaining boundaries

Run03 (default build03) exercised actual private provider -> Bus -> native:
assignment joined the gated active regular turn, while an AfterTurn message
received a different turn ID; request_changes, repair and accept completed.
Independent read-only oracle found six unique native pairs, exact Bus receipts,
three completion facts and corresponding effective model input. Worker actions
were driven by the harness, not autonomous MCP calls.

Failures retained: run01 timed out at the fixture's 60-second HTTP bound;
run02 exposed a genuine named-profile mismatch caused by the old resume sandbox
override. New-bundle-only correction passed run03. The first oracle assertion
incorrectly treated equal ReviewReady prose as duplicate identity; the oracle
now checks exact native IDs and bounds equal text by distinct committed events.

Run05 additionally delivered both watchdog stages, followed by repair/closure;
independent oracle verified seven pairs and three completion facts. Run04's
fixture attempted to read the not-yet-created Bus repository; its replacement
waits for actual creation without modifying provider read semantics.

CLI run c2 passed real `stock-attach`, Human input, same owner/generation and
exit0 with restored termios. Its initial long-path fixture failed the existing
Unix socket bound; the short owned fixture passed without production geometry
changes. Combined MCP/approval/sandbox, pause and changed-boundary negative
coverage completed in h2 and final-byte h3. Both passed actual
MCP list, visible Task response, blocked/cancel Soon, filesystem EROFS for both
workspace/outside writes, declined approval and a subsequent Soon held by
interruption. Independent oracle found three unique native pairs, two completion
facts and one pending/no-receipt item, with seven read-only/on-request contexts.
H2's immutable PASS file retains an obsolete namespace-smoke description; its
actual invocation used the approved same-host-UID guard, as recorded here and
in its command/evidence. The final harness corrects this description.
S6c3 recovery/authority cutpoints and S6c2 Job evidence are reused,
not rerun or upgraded to Soon-specific proof. Native A4 is context publication,
not model output or business completion. Held/uncertain states never authorize
automatic retry. S2/S46 disclosures and S4 PID-time limitations remain unchanged.

Candidate-only stores: older native enum parsers cannot read Soon history;
Bus v4 and explicit marker compatibility do not permit legacy writers. Before
exposure reject/revert the candidate; after private Soon writes, preserve the
candidate-compatible store/history rather than attempting downgrade or rewrite.

The first guarded combined probe h1 passed configured MCP listing and actual
Task delivery, then failed an over-specific PTY assertion expecting raw
`Assignment ID` text. The generic CLI did not show that raw input text. The
corrected oracle emits a distinct fixture reply only after observing the exact
Task context in real model input and checks its terminal visibility. This is
not a native Task card/raw-input renderer claim; no native rendering was changed.
