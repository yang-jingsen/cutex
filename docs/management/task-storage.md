# Task Service storage

Task protocol schemas remain unchanged. The provider stores current aggregates,
exact action receipts and compact watch events in `task-service.sqlite3` under
the existing provider root. Every mutation commits these together in one SQLite
transaction. An action replay returns its original receipt without another event;
a reused action ID with different input still conflicts.

Only changed aggregates are written. Immutable JSON nodes share repeated values,
including the growing status/result history in an Attempt and its prior receipts.
Array prefixes share a tree, so another status does not duplicate all old status
bodies. Receipts reference the committed Attempt version and remain independent
of later changes. Current rows, receipt indexes and events are committed together.

Worker context and execution use indexed assignment reads; prepare/execute and
Human Cancel/Reassign load only affected assignments, their task/workflow and own
attempt history, plus the exact prepared action. Delta commits retain store CAS
and reject insertion over an existing aggregate. Committed replay checks the
single action receipt before loading any current aggregates. Notification bridge
reads use exact outbox IDs. `query_live` remains a global current-state projection
for dashboards and cross-task coordinator operations; callers must not use it
for single-task recovery. The compatibility `query` additionally expands all
historical receipts and is deadline-bound.

Runtime admission reads only lightweight active assignment records. Maintenance
and archival authority digests still require their complete historical projection.
Bus startup calls `initialize_store`, validating/opening the SQLite store without
materializing task history. SQLite transaction recovery handles interrupted
writes. `watch(after, limit)` uses the event sequence index and requested page.

Expired prepared actions remain identity tombstones so an old action cannot
retarget a later attempt. Closed assignments and superseded/terminal attempt
bindings do not count against executable preparation capacity. The capacity
check reads identity/phase fields rather than unrelated attempt status arrays.

The JSON snapshot/full-state-journal format is not automatically imported. At
this installation the user explicitly authorized discarding old Task Service
data after validation. Stop both Management and Bus before resetting their Task
Service subtree; preserve agent/thread state, seat authority, Job credentials and
ordinary messages. An old-format root is diagnosed explicitly until it is reset.
Do not delete the parent `runtime/task-service` directory: it also contains seat
and Job state. This deployment reset is not an implicit behavior of ordinary
startup or a requirement to migrate future task data.

`cutex human doctor [agent]` is a local read-only diagnostic usable without
starting Management or Bus. It reports installed/running executable paths,
agent runtime identity, recent startup errors/actions and Task Service file sizes.
