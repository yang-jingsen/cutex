# TUI shell and resume skills — 2026-09-14

## Cutex shell

The primary navigation is now CUTEX / Agents / Sessions / Projects / Tasks / Jobs / Settings. Jobs is explicitly an unconnected placeholder, not an empty result from Job Service. Settings opens the existing configuration browser directly; Profiles and other utilities remain reachable through their shortcuts and F1. Tasks now navigates right to Jobs, which navigates right to Settings. Existing draft navigation checks remain in place.

Common status rows show status only. F2 details appears once in contextual shortcut rows where supported. Agents/Sessions use a compact, wrapping two-line shortcut area instead of attempting to display the entire command directory. Projects and Tasks use the same bottom allocation and filter input renderer. Task refresh continues without a flashing refreshing label.

Colors: brand #F7B3CD, accent #E08EB2, focus #74BAC3, selected background #26344F. Top-level selection uses bold white text; inactive tabs stay gray. Existing per-project badge palettes and runtime error/success meanings are retained. This does not apply semantic colors to every agent/task name.

Validation: `cargo test --offline --locked --bin cutex session_tui -- --test-threads=1`: 270 passed, 3 failed, 2 ignored. A clean worktree of eb5ec37 reproduces the same three failures:

- management_commands_use_service_semantics_without_mutating_runtime_identity: fixture adoption has a runtime owner.
- management_success_refreshes_the_row_and_survives_a_stale_snapshot: fixture has an invalid session ID.
- new_agent_without_install_explains_runtime_selection: test assumes no installed local runtime.

No new test failures. An optimized release build succeeds. Actual PTY capture covers all six panels at 80 and 120 columns, including Tasks → Jobs → Settings. Capture tooling explicitly models alternate-screen resets and removes its inherited NO_COLOR environment override. Local images are terminal captures, not design mockups. Production data, terminal transcripts and screenshots are not included in Git.

## cute-codex skills (7e1b32737 / release-native-r8)

An actual remote resume first received a valid skills/list response for the current thread directory, then a startup response for the frontend process directory. The latter was accepted by App but did not match ChatWidget's directory, clearing all skill candidates. Both responses contained 44 skills; this was not a filesystem loading or permission failure.

Changing only --cd did not align the local frontend configuration in this remote path. Changing the process cwd did make the unmodified binary show candidates. The fix scopes background requests and stale response checks to ChatWidget's active directory and ignores responses without an entry for that directory. A genuine empty response for the active directory still clears the catalog.

Two regression tests passed. The rebuilt CLI resumed the actual ifm session from the mismatched launch directory and rendered bio-review-bridge in the popup without submitting a prompt. The related race-prone logic was inherited from upstream; there is no evidence that the custom event display changes introduced it.

Deployment uses the new CLI with unchanged app-server/Code Mode host copied beside it. 35 desired manifests updated; four online native processes restarted and verified Ready with the r8 executable path; the cute-codex shortcut now selects r8. Job Service had zero active Jobs when its launcher list was extended. No skill enable/disable rules or history storage were changed.
