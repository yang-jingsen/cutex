# Legacy Agent reservation recovery

This generic runbook reconciles one legacy Agent Management reservation that
cannot be retried because an older action stopped at `owner_action_required`.
Replace every angle-bracketed value with incident-specific data and retain the
exact bytes of the corrected rotation request before starting.

The reconciliation command is root-only. Its selectors are CAS assertions,
not proof that the successor is absent. The provider independently reads Agent
Management ownership, durable sessions, current and stale Agent Bus
registrations, process state, and native index/rollout evidence. Any present,
ambiguous, malformed, or unavailable source preserves the reservation.

## 1. Deploy and preflight

Deploy a `cutex` binary containing reservation reconciliation and restart only
the local Management v2 service. Do not edit `agent-management-v1.json` or any
native session store. Save these inputs and their SHA-256 digests in the
incident record:

- the accepted legacy Primary Director ownership-import request;
- the reservation-reconciliation request;
- the byte-identical corrected rotation request.

Before mutation, query the Agent Management snapshot through the established
root diagnostic procedure and confirm:

- the target project authority names the expected Primary Director and epoch;
- the target action is `owner_action_required`, has the expected request digest,
  and reserves the expected successor name and absolute cwd;
- it has no known successor Cutex session, native session, retry permit, or
  historical occurrence fence;
- there is no matching Agent Management record, durable session, current or
  stale Agent Bus endpoint, process occurrence, or native rollout/index match.

Stop if any assertion differs.

## 2. Import legacy Primary Director ownership

Prepare a strict ownership-import request with a fresh action ID:

```json
{
  "schema": "cutex/agent-management/legacy-director-ownership-import/v1",
  "action_id": "<fresh-ownership-import-action-id>",
  "project_id": "<project-id>",
  "director_cutex_session_id": "<primary-director-cutex-session-id>",
  "expected_authorized_director_session": "<primary-director-cutex-session-id>",
  "expected_authority_epoch": 1
}
```

Run:

```sh
cutex management agent-ownership-import --request-file ./ownership-import.json
```

Require `status=complete`. Exact replay may report `replayed=true`; any
`no_write` outcome stops the runbook.

## 3. Reconcile the legacy reservation

Read the exact target `request_sha256` from the preflight snapshot and place it
in this strict request; do not substitute the digest of the corrected action:

```json
{
  "schema": "cutex/agent-management/reservation-reconciliation/v1",
  "action_id": "<fresh-reconciliation-action-id>",
  "project_id": "<project-id>",
  "target_action_id": "<legacy-target-action-id>",
  "expected_target_request_sha256": "<64-lowercase-hex-from-snapshot>",
  "expected_phase": "owner_action_required",
  "expected_reserved_agent_name": "<successor-agent-name>",
  "expected_reserved_agent_cwd": "<absolute-successor-cwd>",
  "expected_authorized_director_session": "<primary-director-cutex-session-id>",
  "expected_authority_epoch": 1
}
```

Run:

```sh
cutex management agent-reservation-reconcile --request-file ./reservation-reconcile.json
```

Require `status=complete`, target response `status=no_write`, and code
`reservation_reconciled_no_write`. Repeating the identical admin request must
return the same receipt with `replayed=true`. A changed request must conflict.

## 4. Replay the corrected rotation

Submit the previously saved corrected request bytes through the authenticated
Agent Management tool from the current Primary Director. Do not regenerate,
reformat, or change the request after reconciliation.

Require one complete receipt naming exactly one new durable successor session.

## 5. Verify and close

Verify all of the following before declaring recovery complete:

- project authority names the successor at the expected next epoch;
- the Task Service Director seat names that same successor session;
- predecessor lifecycle state matches the requested rotation mode;
- retained-predecessor modes grant the predecessor the documented Operator role;
- exactly one active Agent Management record, durable session, native SID,
  runtime occurrence, and Agent Bus endpoint exists for the successor name/cwd;
- the corrected action replays its complete receipt from identical bytes;
- the legacy target action replays immutable
  `reservation_reconciled_no_write` and retains its original failure event plus
  the appended reconciliation and `no_write` phase events;
- no duplicate successor message or external message ID exists.

If the authority surfaces disagree or duplicate successor evidence appears,
stop. Preserve all receipts and stores; do not retry, delete, or hand-edit state.
