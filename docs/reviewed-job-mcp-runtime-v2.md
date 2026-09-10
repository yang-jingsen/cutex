# Runtime review digest v2

Private Linux candidate following `8154450b219ebeefe5c621dc16de25916814fdc6`.
This is the specifically authorized runtime-review policy change, not a global
durable revision change or a profile/MCP migration.

New `review_runtime` responses explicitly contain `digest_version: 2`.
The durable-record digest is SHA256 of compact canonical JSON of the complete
serialized record, removing only `last_seen_at` and `updated_at`. Every other
field remains included. Revision, generation, identity, profile, permissions,
claim, PID, binding, lifecycle and explicit launch metadata are not exceptions.
The original configuration, authority/Task checks, bundle evidence, Job custody
and actual owner validation still apply.

Absent `digest_version` means the historical full typed-record SHA256 algorithm
(v1). V1 is omitted when serializing, preserving historical receipt bytes.
Unknown versions fail deserialization. Execute uses the submitted version; it
does not refresh or upgrade a pending review. Exact completed replay returns
the original snapshot. A changed version or payload under that action conflicts.
Old writers reject the new field through the existing deny-unknown schema;
candidate-only writers remain required. No downgrade/migration is provided.

Before stop/spawn, execution reobserves the durable record after expensive
bundle/Job validation and compares against the original review. This does not
hold the durable lock during runtime stop, pause heartbeat, cache evidence or
reissue a token. Existing post-stop claim CAS remains authoritative; a later
conflict can still report stopped-not-started rather than a fictitious rollback.

Job `None` remains opt-out, while `Some` is the prior explicit Human-reviewed
descriptor. Arbitrary shared/profile MCP configuration remains rejected.
Native0c/Jobf3bc9c8 and the existing source/authority/permissions contracts are
unchanged. No real provider, production profile, PRH or deployment is involved.
