# Same-ID maintenance interface (implementation checkpoint, not released)

The dedicated Linux Human root credential boundary is the existing command:

```sh
cutex session stock --request /ABS/private/request.json --management-url http://127.0.0.1:PORT/
```

Keep request/result files owner-private. The command reads the existing local
Management root credential; never put credentials in JSON or argv. Agent/MCP
credentials do not authorize these operations. Ordinary lifecycle guards remain.

Request operations are `maintenance_review`, `maintenance_apply`,
`maintenance_status`, and `maintenance_start`:

```json
{"operation":"maintenance_review","request":{"action_id":"migration-EXACT-ID-v1","cutex_session_id":"cutex.EXACT-ID","destination":"/ABS/NEW-HOME","bundle":{},"expires_at_unix":0,"job_mcp":null}}
```

`bundle` is the existing typed exact `StockBundle` (not arbitrary configuration):
its shared-config reference initially identifies the original native home's
config.toml. Use the frozen artifact manifest, original selected profile and
reviewed Job descriptor. Expiry must be future and at most 24 hours. The review
response is the complete typed review; retain it unchanged for:

```json
{"operation":"maintenance_apply","review":{}}
```

The `{}` above means the exact saved review object, not a newly constructed
review. Status and start use the original migration action ID:

```json
{"operation":"maintenance_status","action_id":"migration-EXACT-ID-v1"}
{"operation":"maintenance_start","action_id":"migration-EXACT-ID-v1"}
```

Phases: not_started -> preparing -> prepared -> applied -> activated. Apply does
not start. Original offline cohort entries cannot use maintenance_start. Status
is read-only. Exact completed replay returns the prior receipt. An uncertain
HTTP response requires status inspection, not a new action or destination.
An incomplete destination is retained and refused; there is no automatic delete,
overwrite, rollback, role transfer or claim clearing.

## Preconditions and release boundary

- Exact selected34 target must already be offline, with no pending launch or
  runtime claim. This entry does not fix a failed existing close operation.
- Management project authority and Task seats/assignments remain intact and are
  fenced across review/apply/start. Active tasks do not grant ordinary lifecycle
  permission; only the sealed, action-bound Human maintenance path can proceed.
- `/usr/bin/python3` with standard `sqlite3` is required. Verified DB and WAL
  are copied into an exclusive disposable directory; SQLite resolves committed
  metadata there. Original catalog bytes are never checkpointed or repaired,
  and source SHM is never copied. A corrupt, changing or not fully recognized
  WAL (including unsupported trailing frames) refuses continuation.
- New native home is exclusive, private, source-disjoint. Original histories,
  profiles and auth files are not rewritten. Auth custody preparation is a
  separate release prerequisite; migration does not copy credentials.
- New tagged receipts/markers require the new writer. Do not restore old
  binaries against new stores/history. Existing backup is not repeated; restoring
  it later requires explicit approval of subsequent data loss.

This document describes source under verification. Exact final commit, default
artifact manifest and private process oracle are still required before runner
readiness. No live migration or service stop has been executed.
