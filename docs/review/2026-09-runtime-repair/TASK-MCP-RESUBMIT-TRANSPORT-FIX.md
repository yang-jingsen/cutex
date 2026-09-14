# Worker resubmit transport repair — 2026-09-14

The original worker rollout contains three identical submit calls at 2026-09-13
17:53 UTC (Sydney September 14 03:53), each returning response_uncertain after
approximately 10.5 seconds. They precede acceptance, rather than following closure.
The original resubmit action has no stored receipt. No completed assignment was
reopened, prepared again, or otherwise modified for this investigation.

## Confirmed defect and repair

The bounded Task/Management MCP HTTP client used read_to_end even after receiving
the full Content-Length body. A peer that keeps the connection open can therefore
turn a complete response into a timeout; two transport attempts match the observed
10-second duration. This is a reproducible defect, not proof that it caused the
historical incident: the old receipt does not retain the underlying transport error.

Read complete Content-Length responses immediately. Retain the 64 KiB header and
1 MiB body limits, reject ambiguous/unsupported framing, preserve EOF-delimited
responses, and continue treating truncated or timed-out partial bodies as errors.
Non-MCP transport behavior and provider lifecycle rules are unchanged.

Worker uncertain receipts now include prepare/action phase and guidance to retry
the same action ID with the identical payload. This is reconciliation, not a claim
that the server did not write: prepare itself can persist a prepared action.

Tool help now explains that submitting requires a running attempt. A revised
submission after review_ready requires request_changes first. Isolated provider
coverage verifies rejection before request_changes, successful revision after it,
and exact replay of the committed revision. The adapter separately verifies that
illegal_state is current_state, not response_uncertain.

## Validation

- 117 tests selected by agent_bus:: passed, including a real TCP server that keeps
  the connection open after a complete receipt and a 100 ms client read timeout.
- 23 MCP adapter tests passed after the final tool-help change.
- Isolated provider resubmit lifecycle test passed.
- Parser tests cover fragmented headers/body, incomplete responses, oversized and
  ambiguous framing, non-success responses, and EOF framing.
- git diff --check passed. Optimized release facade built for deployment.
- An initial broad `resubmit` keyword selection also ran unrelated
  app_server::bus_bridge::tests::failed_ack_retries_without_resubmitting, which
  failed its pending_acks assertion. This test uses FakeBus/FakeSubmitter and does
  not call the changed HTTP reader or Task adapter. That failure is recorded, not
  represented as a full-suite pass or fixed by this patch.

Historical cause remains unconfirmed. Genuine service delays can still produce
response_uncertain; phase reporting improves localization but does not eliminate
network/process failures. Never replace an uncertain action with a new ID.
