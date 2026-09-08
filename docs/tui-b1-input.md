# B1 keyboard migration (Managed, Recent, Projects)

This supersedes the Phase A keyboard exceptions, not its storage or identity
contract. Tasks keeps its legacy keys. Terminal/catalog ownership remains B2.

## Input and focus

All three root lists use type-to-filter. `/` starts input; within input it is
ordinary text, as are spaces and action letters. Left/Right, Home/End,
Backspace/Delete and Ctrl+U edit the focused field. Paste removes control
characters and inserts at the cursor; Unicode cursor placement and horizontal
scrolling use the locked tui-input API. Ctrl+X is consumed inside text editors,
never a runtime close. Project drafts keep their existing String values and
store only cursor positions separately; their old push/pop handlers are gone.

Enter/Escape leave filter input and retain the query. The next list Escape
clears it; an empty root stays put and hints Ctrl+C. Root row movement now stops
at the ends rather than wrapping. Home/End and PageUp/PageDown select within the
list, not while editing. Tab/ShiftTab traverse local focus without activation;
root lists alternate rows/filter. Inspector is explicitly reachable by Alt+I
or F1. Field editors retain their existing Enter validation/staging/save paths.

## Commands and review

The shared Binding table drives shortcuts, root command hints and actionable
F1 help. F1 + arrows + Enter invokes the same command adapter as its shortcut;
disabled entries show reasons. Alt+A is object Actions, Alt+E object Edit,
Alt+I Inspect, Alt+S global Settings, Alt+M/R/P/T page navigation. Existing
Project Create moves to Alt+N; Recent Load more moves from bare n to Alt+L.
Ctrl+H still toggles archived Projects. Alt+V still toggles Managed titles;
default Enter takeover is unchanged. New Agent/New Session are not introduced.

Navigation uses a shared busy/modal/dirty gate. Unsaved fields/drafts offer
Cancel (default) or Discard and leave; Save is offered only through an existing
form save path. Save stays for its result instead of pretending an asynchronous
write was cancelled or an exit succeeded. An unstaged individual settings field
must first be staged with Enter before the form can be saved. Confirmation/busy
views do not allow page shortcuts to change the reviewed scope.

Project F5 defers while a draft or confirmation owns its target/version; finish
that view, then refresh. Managed refresh retains staged drafts; an observed
runtime confirmation revision/name/lifecycle change invalidates the review.
Import's editable formal-name field has separate focus from Cancel/Confirm;
Tab traverses field and choices. Thread titles remain metadata and are never
used or suggested as formal Agent names.

Release events are ignored. Repeat edits/navigates but does not activate a
mutation or command. Existing synchronous provider calls and runtime busy guard
remain the effect boundary. No store, permission, session identity, backend
creation, retirement or membership semantics change.

## Verification boundary

`ui_contract_` tests enter the real production key router/page handler, including
F1 fallback, dirty navigation, Unicode paste/cursor and Repeat/Release. Isolated
HTTP Create/Add uses real provider handlers and private state. TestBackend is
not PTY/Windows/live-runtime acceptance. B2 still owns the single terminal shell,
catalog lifetime and layout/settings-row work; B1 only adds a small existing
page-handoff flag for Projects → global Settings, without owning terminals.
