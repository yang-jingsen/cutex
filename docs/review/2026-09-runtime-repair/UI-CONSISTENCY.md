# Cutex UI consistency review — 2026-09-14, r42

Review branch: `review/runtime-repair-20260914`.

Please prioritize correctness of key routing, state transitions and visual consistency
across Agents, Recent Sessions, Projects, Tasks, Jobs, Archive and Settings.

## Source map

- `src/cli_app/session_tui.rs`: Agents, Recent shell, Settings, Profiles, Archive;
  model/key routing/rendering. Large module: assess factoring by responsibility.
- `src/cli_app/session_tui_projects.rs`, `session_tui_tasks.rs`,
  `session_tui_jobs.rs`: other panel implementations.
- `src/cli_app/session_tui_layout.rs`: theme, headings, list/details geometry, tabs.
- `src/cli_app/session_tui_input.rs`: shared key bindings, filters, command help.
- `src/cli_app/session_tui_terminal.rs`: narrow replacement of CJK wide-tail cells.
- `src/cli_app/session_tui_settings.rs`, `session_tui_profile_settings.rs`: Settings
  projections, drafts, save/discard and profile editing.
- `src/notify/session.rs`: three-level preference labels and styles.
- `review-sources/job-service/`: included independent Job Service source.
- cute-codex is a separate repository, `yang-jingsen/cute-codex`, branch
  `review/runtime-repair-20260914`; no native changes in this UI release.

## Expected style and interactions

Shared list pages split left/right first; left has filter/context above list, right
has Details. Left has a width cap. Settings retains three columns. Jobs remains a
placeholder. Archive now uses shared geometry, with a scope explanation above its
list (not a searchable filter). Alt+I exposes archive details in narrow terminals;
Enter still reviews restoration. Esc returns to the originating Agents/Recent page.

Editable/actionable Settings entries are white; read-only entries light gray. No
emoji. Notification previews use configured labels/colors/bold, including selected
choices. Selection background must not override semantic foreground or bold.
STATUS unavailable is N/A with seven-column width; underlying reasons stay in Details.

## Remaining review focus

- Avoid generic navigation intercepting Esc before a local editor/subpage unwinds.
- Check equivalent Alt+I, Alt+B, Enter, Esc, filter and panel bindings across pages.
- Archive intentionally lacks search for now; assess whether it should share the
  complete list-page filter model. Check long paths and short terminal behavior.
- Notification config read errors currently fall back to default preview; consider
  an explicit validation indication. Reads happen when notification previews render.
- Filter-title explicit reset has a buffer regression test; PyCharm Reworked terminal
  visual verification is pending. Avoid broad full-screen redraw workarounds.
- Three existing TUI tests fail due to runtime/fixture assumptions; a broader archive
  test also expects 'retired' where the service emits 'archived'. Do not represent
  the full suite as green. See local verification record for counts.

User intends a review of the implementation, not automatic introduction of additional
approval gates or restrictions. Keep cute-codex changes small as upstream changes often.
