# Reviewed registration groups — private candidate

Only authenticated service registration of an existing explicit-launch owner
may project exact durable groups. No request flag or new wire/schema field.
The session-store callback resolves the unique native/durable mapping and exact
runtime receipt, marker, revision, claim/stage/generation, PID, host, cwd, profile
and persistent active class. Unknown/foreign/stale ownership fails closed.
The submitted groups must match the existing normalization of the reviewed
durable groups; arbitrary requested changes are rejected, not adopted.

Before store CAS and roster publication, the callback verifies the pinned native
bundle/process (existing executable path, schema, process-group and seconds-based
birth check), reconciles with exact durable ordered groups, then rechecks process
identity. Store CAS and the final Ready revision/configuration checks are unchanged.
The roster and durable record therefore use the same group projection. Ordinary
unmarked registration defaults are unchanged. A genuine durable change invalidates
the old review; it is not normalized away. No generic reconciliation bypass.

Trust remains the existing private same-user authenticated service boundary,
not hostile same-UID isolation. A caller replaying the exact current owner fields
cannot choose new groups; this is not a new per-request signature protocol.
Existing seconds-truncated PID birth limitation remains. Bundle verification
does hash work during registration while the existing roster lock is held; no
throughput claim or weakened artifact check is made. One verified bundle is
reused for the two process observations within the same callback, not cached
across occurrences. The registration HTTP request alone has a bounded120s
deadline (old5s was insufficient for full artifact verification); all other
client deadlines and TUI90s remain unchanged. No automatic retry is added.

Base9d108802 / product6e3b815; native8cde/2eab, host/schema and Job unchanged.
Tests and actual composed outcome will be reported separately, not inferred from
source review. No live migration, profile/auth changes or old-history rewrite.
