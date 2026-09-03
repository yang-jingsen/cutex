# Tasks workspace integration

This change is intentionally modular: the Task Service table, its refresh
model, rendering tests, and Director-query client live in
`src/cli_app/session_tui_tasks.rs` and do not depend on agent selector data.

This change is rebased on `9be37fbc485f472853e24d30be52d9f494c48391`. The
home panel retains distinct Cutex Projects, Codex Workspaces, and Tasks rows;
the Tasks route is isolated in `session_tui_tasks::run()`. The Tasks module
continues to issue only
`cutex/task-service-director-action/v2` queries; do not replace this with a
cwd, group, display-name, or native-workspace lookup.

The v2 Director route now filters task records against the authenticated
Director session's exact Agent Management project authority. Presentation
metadata is display/filter-only and cannot grant or select that scope. For
each returned task, the UI resolves Project display name and badge only by an
exact canonical `project_id` lookup in the authoritative Project store. A
missing exact record remains visibly unavailable; it never falls back to a
similar name, badge, workspace, cwd, or group.
