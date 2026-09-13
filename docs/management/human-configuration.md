# Human configuration

`POST /v2/agent-management/human-config` uses the existing local management root
credential. It does not require a Director seat or an assigned task.

- Show: `{"operation":"show","cutex_session_id":"cutex.…"}`.
- Set: `{"operation":"set","cutex_session_id":"cutex.…","action_id":"edit-1","patch":{"model":"example","reasoning":"high"}}`.
- Undo: `{"operation":"undo","cutex_session_id":"cutex.…","action_id":"undo-1","original_action_id":"edit-1"}`.

Patch fields are `profile`, `model`, `reasoning`, `cwd`, `approval`, `sandbox`,
and `bundle_manifest`. Omitted fields remain unchanged. Null clears an optional
override. `cwd` changes the managed launch directory; clearing it restores the
record's observed directory. Directories must exist. Approval accepts `never`,
`on-request`, `on-failure`, or `untrusted`; sandbox accepts `read-only`,
`workspace-write`, or `danger-full-access`. Explicit native agents also validate
the candidate against current profile/account configuration before saving.

`bundle_manifest` accepts a file path, resolves its canonical path, computes its
SHA-256, and validates it with `StockBundle::load`. It retains the existing native
home, native ID, and launch contract version. Each manifest must reference that
agent's native-home config. This updates package selection without remigrating
history. It cannot be null. Show and receipts expose its complete saved launch
contract to make the before/after native identity and package binding reviewable.

Changes apply to desired configuration only; they do not stop, restart, or modify
the active runtime. They take effect on the next applicable launch. Each mutation
saves a private per-action journal under `runtime/human-config-actions` beside the
session store, with the exact request, before/after values, and completion status.
Replaying an action ID with identical input returns its original receipt; changed
input conflicts. An interrupted write is reconciled under the session-store lock.

Undo compares only fields changed by its original action. Later independent
configuration edits and runtime updates survive. If one of those same fields has
changed, undo reports a conflict instead of overwriting the newer value. The user
can still explicitly set any desired replacement value through a new action.
