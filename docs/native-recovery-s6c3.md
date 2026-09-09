# S6c3 — explicit private recovery and Task coverage

Base `2d49567f9c8fa52a819ea4c306e241992710beab`, tree
`2ff845835d8bfda596ccd7d4d85ef611a3d1e5af`. This is a private candidate,
not a production cutover. Native and Job pins remain those in
[S6c2](native-delivery-s6c2.md); no CLI or generic Soon capability is enabled.

## Root recovery contract

POST `/v2/agent-management/native-recovery` uses the existing dedicated
root Management credential. Agent/Bus credentials and MCP cannot invoke it.
There is no new token mode, model tool, receiver-policy setter or queue-wide retry.

1. `{"operation":"review","cutex_session_id":"EXACT_DURABLE_ID","message_id":"EXACT_FROZEN_MESSAGE_ID"}`
   returns the actual native status, original envelope/receipt, current occurrence
   binding, durable specification and authority digests, and repeat warning.
   The message must already have an authoritative frozen Bus envelope.
2. `{"operation":"confirm","action_id":"STABLE_ACTION","retry_id":"STABLE_RETRY","review":REVIEW,"confirm_repeat":true}`
   confirms that exact review. Only held work qualifies. Retry may repeat model
   requests and tool effects; it does not retract the original A4 or imply success.
3. `{"operation":"status","action_id":"STABLE_ACTION"}` reads the stored
   permission/receipt without connecting to native or waking anything.

Management mutation -> seat -> durable locks protect current recipient and
occurrence checks. Native RPC has before/after fences. The Bus store records a
prepared permission before retry and the immutable native reply afterward.
Store version 4 prevents old v3 writers silently dropping recovery actions;
later envelope freezes never downgrade this version. Candidate writers only.
Exact completed replay returns the original result; changed semantics conflict.
`result:null` means prepared/uncertain, not rollback or permission to choose a
new retry ID. The same action reconciles the native idempotent retry.

Ordinary heartbeat timestamps are excluded from the specification digest; all
identity/configuration/revision/generation fields remain bound. Authority hashing
is conservative: unrelated roster/seat changes can stale a review. An uncertain
permission whose occurrence/authority changed fails closed rather than granting
another retry; cross-occurrence recovery of such a permission remains a limit.
No implicit marker clear, native restart, role grant or Human impersonation.

Project Director completion delivery now checks both current Management project
authority and the existing Task seat before RPC and final business commit. An
incomplete coupled transfer is an actionable pending conflict. An old recipient's
genuine native context remains historical fact, never reassigned or falsely ACKed.
Task context fact -> Bus CAS -> transport ACK ordering is unchanged.

## Current workflow inventory

| Existing source / operation | Actual mode and current native coverage |
|---|---|
| Ordinary inter-agent send | after_turn/passive supported; Soon explicitly pending/unsupported |
| `assignment.v2`, assign | Soon enforced; not translated or silently converted |
| `worker_followup.v1`, request_changes follow-up | Soon enforced; not translated |
| Worker submit -> ReviewReady completion | after_turn; actual provider/native proof |
| Director accept_result -> TerminalClosure | after_turn; bounded S6c3 proof recorded in result |
| Blocked, Declined, AttemptAborted, RetriesExhausted, OwnerActionRequired | Soon completion; unsupported native mode |
| Other fail/cancel closure branches | Soon; not equivalent to accepted-result closure |
| Watchdog worker / owner escalation | worker Soon, owner after_turn; native watchdog canonical/context/turn-binding adapter still absent even for after_turn |
| Management bootstrap/rotation messages | existing lifecycle/control requirements not implemented by this native adapter; no fabricated Human turn |
| Job completion | existing S6c2 after_turn/outbox/A4 evidence reused unchanged |
| Task MCP outbound semantic actions | S5a parity retained; outbound invocation is not autonomous inbound wake |

Source anchors: `agent_bus/queue.rs` assignment/follow-up enforced modes;
`task_service/provider.rs` notification branches; `task_service/watchdog.rs`;
`app_server/bus_bridge.rs` existing watchdog context/turn helpers. Exact K reference
is `ef53716b7673ad14c24b977667334e31e66110d8`, native U+S6 `c2aaceb4`.
No native Task/Job discriminant or business state machine was added.

Director explicitly deferred C's Soon sub-slice. Design-only S6D and separately
accepted private S7a CLI do not authorize changing this bundle. Integrate a later
coherent accepted Soon+CLI/app-server/schema bundle once, then implement missing
canonical consumers with their actual modes and context/turn facts. Do not claim
full Task replacement, bootstrap/rotation or automatic wake yet.

Templates remain concise S6c2 source/event/payload projections; exact tool-required
assignment IDs occur where needed, mechanical receipt/digest/generation stays
outside prose. A4 remains persisted context, not task execution or acceptance.
