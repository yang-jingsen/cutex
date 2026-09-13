# Human task recovery

`POST /v2/task-service/human-recovery` uses the existing local management root
credential. It works without a Director seat, project membership, or task
assignment to the caller. An agent acting at the user's request can use it.
Ordinary task operations retain their caller and attempt authorization checks.

Requests:

- `{"operation":"query"}` lists assignment bookkeeping across projects. Optional
  `assignee` filters by durable Cutex session ID. The response contains
  `assignments` and `journal_sequence`; it omits worker attempt tokens.
- `{"operation":"cancel","action_id":"...","assignment_id":"..."}` closes
  the assignment and cancels its active nonterminal attempt atomically. It
  returns the existing ProviderReceipt shape with an assignment result.
- `{"operation":"reassign","action_id":"...","assignment_id":"...","new_assignment_id":"...","assignee":"..."}`
  closes the original assignment and creates a fresh AwaitingAck assignment for
  the same task revision and project, in one transaction. Existing attempts stay
  on the original assignment. The response contains `receipt`,
  `previous_assignment_id`, and `dispatch: "not_requested"`.

Cancellation and reassignment change task bookkeeping only. They do not stop a
worker, interrupt a model turn, or send an assignment message. Reassignment is
useful for an offline target; the new owner can start the fresh assignment when
online. The caller coordinates any runtime interruption and delivery separately.

Reusing an action ID with identical input returns its original durable receipt.
Changing input under that action ID returns a conflict without another write.
Cancelling an already closed assignment succeeds without reopening it. A
reassignment can also recreate an assignment from an already closed one.

Project-schema activation no longer freezes existing projectless assignments.
Their existing assignee, coordinator/terminal authority, and attempt checks still
apply, so an unrelated worker cannot take their work or submit a stale attempt.
