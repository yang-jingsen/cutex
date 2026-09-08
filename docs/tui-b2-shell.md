# B2 shell and layout boundary

Managed, Recent and Projects share one long-lived terminal guard and one
synchronous event source. Page loops borrow both serially. Ordinary switching
does not enter/leave the alternate screen or create snapshot/catalog workers.
The initial snapshot request remains owned by the shell; F5 can explicitly
request a replacement after completion. Background mutations retain the
existing busy navigation gate.

Recent starts lazily on first entry. Its local-host catalog connection, page
cache and page state survive ordinary switching. Each request has a generation;
superseded replies are discarded before projection. This is one fixed local
catalog scope, not a new cross-host/provider selector. Replies update only the
retained Recent state. F5 refreshes the first page; Alt+L loads the next page;
Enter after a failure retries the failed cursor. Failures preserve cached rows
and leave Loading. A stopped worker reports that reopening the TUI is required.
Dropping the shell closes its channels; existing bounded in-flight catalog calls
are not represented as synchronously cancelled.

## Foreground and legacy handoffs

Foreground session dispatch, profile login and Native Workspaces use an explicit
terminal handoff. The outer reader is synchronous and suspended, the outer
guard restores cooked mode, the existing pathway runs, and the shell re-enters
after success or failure. A terminal re-entry error is fatal, distinct from an
action error; it cannot continue with a missing terminal. Foreground completion
now returns to the retained UI instead of ending the selector.

The retained page models are the return context: page/project, stable selection,
query, draft and Inspector preference remain in memory. Root TableState/offsets
are retained as well. Settings/Profiles navigation does not select a fake Agent
or discard the previous Agent selection. No new persistent return-context store
or lifecycle operation is introduced.

Tasks keeps its exact legacy runner, keyboard and business behavior. It is
invoked through the same explicit suspend/resume boundary and owns a separate
terminal during that interval. This is **not** a claim of one terminal lifetime
across Tasks or Native Workspaces.

## Management and geometry

The Agent collection no longer contains Retired, Workspaces, Profiles or Global
Settings navigation rows. AppContext owns the global/default snapshot separately
from the list. Small legacy settings-renderer adapters remain in AppContext;
they are not Agent rows, counted identities or selectable list entries. Profile
and global-save refresh overrides update this context as well as real Agents.

F1 exposes General (Global settings), Profiles, Appearance (Inspector toggle),
Native Workspaces and Agent Archive through the shared binding table. Direct
keys are Alt+S/G/B/W/Z. From Projects, open Settings then use this management
navigation; unavailable direct utility entries state that route. Archive
reuses the existing retired view, not a new lifecycle implementation.

Inspector geometry depends on viewport and explicit in-memory preference, not
whether a row exists. Empty/no-match selections use a placeholder. A double
pane needs at least 72 list-inner cells and 38 Inspector-inner cells, plus two
borders each and a gap: 115 total. Inspector targets 32%, capped at 56 inner
cells, while the list keeps the larger share. Narrow Inspect uses the existing
detail pane and returns without resetting query/selection/offset. These are
tested starting constraints, not Windows/user-terminal visual acceptance.

The three pages have CUTEX branding and separate status/help slots. Errors no
longer replace help. Recent uses the common title/filter/body slots rather than
an additional bordered context row; short list panes compact the filter before
losing their rows. Shared shell/input theme tokens use portable terminal
colors, text markers and bold focus; detailed column/read-model styling remains
phase C. B1 editing, default takeover, Alt+V titles and dirty-save limitations
remain; Project F5 still explicitly defers during a draft/review.

## Verification boundaries

The affected suite includes production key handlers, private HTTP provider
Create/Add tests, request-generation/failure fixtures, independent settings
projection, Inspector geometry and return-context tests. The factory/cache
test uses an injected resource; it does not start a native catalog process.

`scripts/tui-b2-pty.py <cutex-test-executable>` drives a real Linux PTY through
the production page loops/event source: 20 three-page round trips, then the
actual terminal adapter with a mock foreground shell child. It checks termios,
alternate-screen enter/leave counts, success/error resume and final cleanup.
The terminal-child test is guarded in the ordinary suite and is exercised by
this script separately. Tasks local-state regression uses its real key handler;
the live Tasks provider runner is not invoked. No real cute-codex session,
operator catalog, paid workload, Windows terminal or live acceptance is claimed.

Phase C still owns AgentSessionView, VisibleColumns and Project member tables;
phase D owns creation/adoption and lifecycle consistency. No store/schema,
permissions, deployment or runtime-dispatch implementation changes are included.
Rollback before exposure is reverting this source candidate; no state migration.
