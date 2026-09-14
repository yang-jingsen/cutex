# Delayed Task notifications: verified evidence, 2026-09-14

Follow-up report: /tmp/cesc2_task_service_delayed_resubmit_notifications_20260914_ZH.md.
This is related to the same assignment, but independent of the MCP response-framing
repair. Read-only inspection of the authoritative SQLite aggregates confirms:

| Notification | Created (UTC) | Queued (UTC) | Delivered fact (UTC) |
| --- | --- | --- | --- |
| ReviewReady | Sep 13 17:50:56.257710724 | Sep 13 17:50:56.636235889 | Sep 14 00:08:11.121595265 |
| TerminalClosure | Sep 14 00:02:07.321468991 | Sep 14 00:02:07.940446184 | Sep 14 00:08:10.964874555 |

Both records have target_unavailable uncertainty facts at Sep 14 00:07:24 UTC.
The ReviewReady belongs to the original submit action, not the uncertain revised
submit. The assignment remains closed/completed; its immutable current root is
eda0b555aedd03f66a7b94785dd49b9d29697d43990f18682a8510cf05231a6f.
This evidence establishes late, reversed delivery, not a database state reversal.
The exact cause of the entire six-hour backlog is not yet established.

`TaskServiceAgentBusDispatcher::dispatch_completion_with_seats` iterates the
completion_notifications BTreeMap by notification ID. IDs are SHA-256-derived,
not chronological. The terminal notification begins tsn-ad7393 and the earlier
review notification tsn-ce312a, so recovery visits terminal first. This is a
confirmed ordering defect consistent with the observed delivery facts.

Follow-up implementation should order pending recovery notifications by their
actual occurrence time (or persisted event sequence), expose stable notification
ID/transition action/time to consumers, and explain that a delayed transition is
historical rather than the latest authoritative assignment state. Sorting a single
recovery batch alone cannot promise global order across targets or delivery modes.
Existing queued records and their audit history must not be rewritten or silently
dropped. Validate using an isolated submit/close/offline/recovery fixture.

This document records investigation, not deployment of a notification-order fix.
