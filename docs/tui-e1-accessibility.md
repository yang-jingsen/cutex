# E1 detail accessibility and bounded cleanup

Base: accepted D2 subset `768a6ada20665faf41031700612a58b1452d5262`.

Managed, Recent and Project member Inspect now support Up/Down, PageUp/PageDown,
Home/End to reach every wrapped field. Open Inspect with Alt+I or F1 commands.
Esc returns without changing the list query, stable selection or table offset.
Scroll keys belong to focused Inspect only; text editing retains its cursor
semantics. Each page owns its Inspector offset; opening Inspect starts at top.
Resize clamps the line offset to the new viewport, not the selected list row.

F2 opens a read-only snapshot of status/error and current confirmation details.
It is also actionable through F1's shared command binding. Up/Down, Page keys
and Home/End scroll; Esc returns to the original editor/review. Text/paste,
Enter and activation shortcuts do not act on the underlying state while this
snapshot is open. It is not a fresh service observation or rollback of an
in-flight write. Reopen it to capture later status. Confirmation choices remain
visible at the bottom of small-screen reviews, default Cancel; their targets,
original evidence and submission handlers are unchanged.

Detail wrapping budgets terminal cells and retains full unbroken IDs, paths,
names, profile/project/status and error text. Control characters are displayed
as escapes instead of terminal controls. Combining/CJK/emoji scalars are
retained; actual emoji glyph shaping still depends on the terminal. The lists'
column schemas and geometry are unchanged. No new clipboard dependency.

Cleanup removes only `session::retire_session` and `session::restore_session`,
which had no callers. Their replacement is the existing production
`SelectorControl::ExecuteArchive` -> `session_archive::execute_confirmed_archive`
route, retaining D1R2 review/action and provider protections. The inherited
authenticated HTTP archive/import/Project regression exercises that route.
Other CLI/provider helpers are not removed or reinterpreted.

## Evidence and remaining boundaries

Focused render tests cover 60x18, 80x24, 100x30, 120x36, 160x48, 240x50 and
every width 70–120; long CJK/combining/emoji text, Windows path tails, errors,
modal choices and unchanged list geometry. Production-route tests cover editor
cursor ownership, read-only paste isolation, three-page Inspect return and F1
actionable Details fallback. Existing empty/large-list and lifecycle tests are
included in the affected TUI suite, not a new backend proof campaign.

The Linux private PTY oracle uses actual TerminalShell/ShellEvents and the
production key resolver: F2, End, Esc, 120x30 -> 60x18 resize, selected-ID return
and termios restoration. Fixtures are in memory; no catalog/Agent/runtime is
launched. The first oracle run sent Esc before async resize was observed and
failed its assertion; the corrected oracle waits for the actual resize event,
without a sleep or simulated resize acknowledgement, and passed. Existing B2
mock-child handoff/error cleanup evidence is reused unchanged.

New Agent/New Session and optional Adopt profile/Project wizard remain deferred
D2 work. No native bootstrap retry, paid model turn, domain/authority/schema,
Task behavior, dependency or lockfile change. Windows, real managed runtime and
cgroup, theme-by-theme manual terminal acceptance, and the combined TUI+JS
release remain unrun gates. This is source completion, not deployment.
Rollback before exposure: reject/revert the candidate commit.
