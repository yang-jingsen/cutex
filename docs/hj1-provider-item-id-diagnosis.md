# hj1: provider input item ID exceeds 64 characters

Read-only diagnosis after Human's manual terra-low Job test. No repair, retry,
ACK, runtime mutation, credential inspection or paid request performed by the
diagnostic agent. Human's earlier manual execution is separate evidence.

## Exact boundary

- Product Cutex c69241c11b2ad99c7b0f2fcabfe91a67107d3dee; native
  0c425b5f9fca90835fd2b4377a1bca212532f66c; Job f3bc9c8.
- Guest fixture: `/home/cutex-linux-test/acceptance-upload/hj1/h203236`.
- Durable ID: `cutex.01a08d05-bbb5-7f32-a849-02cceb9b2564`.
- Native thread: `01a08d05-bbb5-7f32-a849-02cceb9b2564`.
- Local sanitized evidence: `human-errors.jsonl` beneath that fixture.

Terminal error (`willRetry=false`, also recorded in turn/completed):

> Invalid 'input[19].id': string too long. Expected a string with maximum
> length 64, but got a string with length 68 instead.

Error type `invalid_request_error`, code `string_above_max_length`.
No full conversation, auth, HTTP headers or provider request retained here.

## Correlated actual facts

The single matching native history JSONL has, at observed line 50, an
external_input envelope message ID; line 51 is a response_item of type
function_call_output, name external_event, namespace external, with the same ID:

`jsc_68d6f7ad5a9339150fb4076d3e698194aad4900e4e822ffdc3745bce03a6443b`

Length is 68. Exact Cutex c69241c
`src/cli_app/agent_bus_server.rs:410` implements completion_message_id as
`format!("jsc_{:x}", Sha256::digest(event_id.as_bytes()))`: four prefix characters
plus 64 hex characters. The persisted input item and provider error agree on
the length. This is an input-item identity compatibility defect, not excessive
Job stdout. The outbound serialized request was not independently captured;
the evidence is the actual provider error plus matching persisted item.

Job `job_e916aadff9364b09b537e1ee47d4e146` is exited, exitCode 0.
At inspection its completionDelivery is delivered; outbox acknowledged=true,
deliveryState=delivered, attemptCount=24. Those business delivery facts are
distinct from the failed subsequent model request. No claim that read_output
or a successful final model acknowledgement occurred.

Human reported an initial model statement that completion delivery was
disabled, followed by Esc and the error. The meaning/source of that initial
disabled statement remains unclassified. Sequence alone does not establish
Esc as the cause. No prior developer fix was identified or claimed equivalent.

## Repair ownership and limits

Director accepted this input and assigned native light-core-r1 task
`cute-codex-external-input-provider-item-id-fix-r1` on exact native 0c425:
provider projection and persisted-history compatibility boundary. Cutex
import-r1 remains read-only hj1 evidence owner. No producer ID shortening,
history rewriting, altered envelope/digest/receipt, ACK or held retry is
authorized by this report. Native repair must preserve business identity and
dedupe semantics while respecting provider item constraints.

No new paid test until repaired native and required Cutex pin composition are
authorized. Existing runtime/auth state was not cleaned or changed by this
diagnosis. This report contains no deployment or full real-provider acceptance.
