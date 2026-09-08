# Phase C: shared read-only lists

Base: `14d6cb9a46de86703e828e963d5fc692396ea519` (accepted B2), following B1/Phase A. This is a source integration candidate, not a live acceptance or deployment.

## Identity and observations

`session_tui_view` is an ephemeral projection, not another store. Managed subjects carry the exact durable ID. Recent native subjects use the paired local catalog namespace plus `thread/list.id`, never native `session_id`, cwd, profile or title. A unique explicit durable/native mapping permits the join; ambiguous mappings remain native and cannot be adopted through the existing action.

Formal names come from the dedicated durable field, or the existing provider roster formal-name projection for legacy records. Missing formal names fall back to the durable ID, not a thread/display hint. Native titles remain separate presentation. Native workspace and Cutex Project are separate fields. Configured profile is next-launch configuration; effective profile requires an exact-ID runtime observation. Inheritance is not an observed effective profile.

Runtime/provider failures display Unavailable, not Offline/unassigned. Retained failed-refresh observations are stale. Project members use the provider snapshot; operator/member roles do not confer the viewing Human's permissions. Durable/provider retirement disagreement is reported, never reconciled by a UI OR expression or a write. Known durable-retired Recent rows are hidden; hidden retirement mismatches produce a warning. Cross-store retirement remains phase D.

## Keyboard and presentation migration

- Managed defaults to All active managed durable records, including offline Agents. Alt+O / F1 cycles All, Online, Pinned. Query filters only that scope.
- Managed, Recent and Members use one responsive column schema calculated after borders/highlight space. Name/Status lead, Members add Role, Recent adds native recency and authoritative Cutex Project when space permits. IDs/paths are available in read-only Inspector; path summaries preserve the suffix, including Windows-style separators.
- Projects default to Members: Director, Operators and ordinary active members, deduplicated by durable ID. Arrows/Home/End select; Enter or Alt+I inspects; Esc returns. Member Actions/Edit are explicitly deferred. Existing Project-list Create/Add/Actions/Edit remain their existing Human handlers.
- Alt+S / F1 opens a Global Settings navigation surface: General (the global settings entry), Profiles, Appearance, Workspaces (native catalog), Archive. Arrow/Enter works without Alt-capable terminals. The four primary tabs remain; the header exposes Global Settings when it fits. Alt+E on Managed remains selected-Agent settings. Back preserves the originating page, including Projects, and existing page query/selection state.
- B1 filter editing, modal priority, mutation Press gating, default takeover, Alt+V and dirty-form checks remain. No Tasks internal change. Existing staged-save limitations remain.

Selection is retained by stable subject across sorting/refresh; disappearance selects a clamped previous index. Tables retain their own scroll state. Recent pages deduplicate native IDs even within one page and preserve native recency ordering.

## Verification boundaries

Focused `ui_contract_c` tests cover exact identity/role union (including 1 Director + 2 Operators + 3 members), provider failures, retirement disagreement without writes, scope/query, 1000 rows, stable selection, native pagination, Unicode/suffix clipping and settings/Inspector routes. TestBackend geometry covers 60x18, 80x24, 100x30, 120x36, 160x48, 240x50 and widths 70–120. The affected suite also exercises existing real Human HTTP provider/Create/Add handlers in private temporary state and B1/B2 regressions.

Linux PTY regression uses the existing actual TerminalShell adapter and a mock foreground child; it is not live cute-codex interoperability. Windows and manual visual acceptance are omitted. No live catalogs, operator homes, services, Agent lifecycle operations, remote refs or deployment are involved. The exact checks/counts/source identity are recorded in the task RESULT.

Long Inspector text is wrapped and may extend below a short viewport; this phase does not add a full scrolling/copy inspector. Unicode clipping uses the locked cell-width API, not a new grapheme dependency. These are presentation limitations, not identity truncation in the model or mutation target. Phase D/E retain lifecycle/retirement reconciliation and deeper inspector/accessibility work.

Rollback before exposure: revert the candidate commit. No data migration or runtime rollback is required.
