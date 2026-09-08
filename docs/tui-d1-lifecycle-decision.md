# D1 lifecycle decision gate — reference only

Investigated accepted source `bb67dd04e453562c159008d58c9ab6a53ec727d4` / tree `e8e16d58c08968d760c657e275032c45ddb44756`. No lifecycle implementation change is authorized by these findings alone.

## Decision required

Same-identity reversible UI Retire/Restore is not currently equivalent to Agent Management Close. The active SOP §4.7 defines retired roster Agents as immutable history. `AgentOperation` has no Restore, `active_agent` excludes historical retired ownership, and fresh import cannot resurrect a retired roster record. Composing current Close with current durable Restore therefore cannot satisfy the requested round trip. A thin adapter alone does not resolve this product conflict.

**Proposed policy:** distinguish reversible **Archive/Restore** from permanent **Close**. Archive uses the durable archive state as the reversible authority, while leaving permanent roster retirement/history untouched. Provider ordinary reads and lifecycle eligibility must explicitly observe that archive state, not a UI OR filter. Reject archive of the current Director, granted Operators and active-task Agents until the existing explicit rotation/revoke/task-resolution paths complete. Preserve current membership while archived; Restore validates the current project/role state and restores the same durable ID Offline without resurrecting grants, seats or historical membership. If the current project is archived or otherwise incompatible, return an actionable conflict rather than silently detaching. Permanently closed roster history remains non-restorable.

This proposal is **not implemented or selected**. Human/R13 must approve the reversible-vs-permanent distinction and retained-current-membership policy.

Alternatives:

1. Keep Close/retired-roster permanent and expose Restore only for durable-only archives. Smaller service change, but explicitly narrows the requested imported-Agent same-identity round trip.
2. Make roster retirement reversible. Requires an explicit policy change to immutable retired history and a defined current-state/receipt/grant restoration model; it is not an in-scope convenience patch or permission to rewrite historical records.

Minimum impact after decision: one typed Human application operation and current-state projection/eligibility adapter over existing provider lock, durable CAS, task/role guards and archive runtime proof. It needs stable action receipts and recoverable stage reporting. No UI dual-write, implicit detach/add or new native identity is needed. Whether roster `retired_at` is written depends on the selected policy, so implementation must wait.

## Reproduced current behavior

The test `ui_contract_production_durable_import_http_tui_create_add_cancel_auth_and_rename_and_d1_archive_gap` uses a private home, actual Human Management HTTP handlers for import/Create/Add/project reads, and the actual TUI-dispatched `session::retire_session` / `restore_session` adapters. A local HTTP Agent Bus oracle returns an empty roster; the process-local runtime manager has no fixture runtime. The characterization asserts current undesirable behavior, not desired acceptance.

| Fixture / operation | Durable and native identity | Roster / authority / visibility |
|---|---|---|
| Durable-only ordinary Retire → Restore | Retired then active Offline; exact ID/native ID/formal name/profile retained; no runtime claim or launch | No roster record created; Archive discoverable; retired Recent row hidden and non-adoptable |
| Imported ordinary Retire | Durable retired; native ID/name preserved | Whole provider snapshot unchanged, including receipts/audit; Project still has ordinary active member: **F12 reproduced** |
| Imported ordinary Restore | Same durable ID active Offline | Roster/membership unchanged because archive never changed them; this is not restoration after Management Close |
| Current Director Retire → Restore | Archive succeeds, then restores same durable identity | Project Director pointer unchanged throughout: **archive path does not enforce Director protection** |
| Agent Management Close | Existing provider test confirms retired roster and inactive runtime observation | No supported same-identity Restore operation; fresh import of retired ownership rejected; original import replay stays exact |

No native history file is created/deleted by the tested archive adapter; native-ID preservation is asserted. This is not a live native-history or runtime-stop acceptance claim.

## Source trace / guard differences

- `session_tui_dispatch::execute_dispatch_plan` → `session::retire_session/restore_session` → `session_archive` → `management_context::mutate_archive_session` → `management_archive` → `session::archive` → `persist_cutex_session_store_and_im_record`.
- Archive preconditions check durable revision, runtime generation for Retire, archive state and offline observations. They do not read Agent Management role/grant/project epochs or Task Service activity. Commit persists durable/IM, not roster.
- `AgentManagementProvider::close_existing` performs provider-authorized offline/retire, then sets roster `retired_at`; applicable provider role/grant protections belong to that separate path. `active_agent` and ordinary query/import exclude retired ownership. No roster Restore operation exists.
- `projects::current_project_id`, project detail and counts use roster membership/retirement; formal-name projection does not synchronize archive state. C correctly reports mismatch but cannot resolve authority semantics.
- Durable archive errors distinguish revision/generation conflict, runtime stop failure and persistence uncertainty; there is no combined durable/roster application receipt. The current CLI refreshes a record before submitting its CAS, so this is not proof that a prior TUI confirmation's original version survives to submission.

## Evidence and stop boundary

Run in scrubbed environment/private homes, offline locked dependencies:

- `cargo test --offline --locked --bin cutex d1_archive_gap`: 1 passed (characterizes the defects above, real local HTTP protocol/handlers).
- `cargo test --offline --locked --lib session::archive`: 8 passed (durable revision/generation, offline/stop ordering, partial failure, Restore identity; injected runtime transaction).
- `cargo test --offline --locked --lib lifecycle_preserves_durable_and_native_identity_and_close_retires`: 1 passed (provider public execute, FakeLifecycle).
- `cargo test --offline --locked --lib replay_after_retirement_is_exact_but_new_import_cannot_resurrect_roster`: 1 passed (provider import/replay boundary, private store).

Wrong Human-root credentials remain tested at the actual HTTP handler in the reproduction. **Actual process termination is not proven:** the runtime/empty-roster oracle is controlled, not a live Agent. No permission/stop guarantee is claimed from a mock. Operator/active-task/archived-project round trips, live generation races and combined stop/commit recovery remain unimplemented/unexecuted because the product conflict triggers the explicit first gate. Existing archive unit failure tests are not a substitute for that eventual process oracle. No new page actions, migrations, runtime/services, dependencies, canonical refs or live state were changed.

D13: durable-only fixture passes within the stated offline boundary. D14: failed desired consistency, reproduced. D15: deferred (no new page lifecycle controls). D16: Director protection fails on current archive path; remaining protected fixtures await policy. D17/D18: existing durable-unit evidence only; combined roster/authority/CAS/recovery contract is not established. This packet is **REFERENCE_ONLY**, not an integration or deployment candidate.
