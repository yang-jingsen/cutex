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

Normal task reads and lifecycle guards use `query_live`: current entities without
expanding historical action receipts. `receipt(action)` retrieves one exact
result. The compatibility `query` snapshot still expands receipts and is subject
to a deadline; it is not the hot path for status, lifecycle or notifications.
`watch(after, limit)` uses the event sequence index and reads only the requested
page. Startup loads current state, not a full journal replay. SQLite transaction
recovery handles interrupted writes.

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
