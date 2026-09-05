# Tasks workspace integration

The Task Service table, one-second refresh model, rendering tests, and
Human/Management query client live in `src/cli_app/session_tui_tasks.rs` and do
not derive authority from the Agent selector.

As of R36, the normal no-environment TUI calls the root-authenticated
`POST /v2/task-service/management-query` route. The request is a strict query
document and cannot claim an Agent, Director, project, or seat identity. The
server anchors the read to the exact current `cutex-director` seat occupant and
intersects it with exact Agent Management Primary Director authority before
using the existing Director projection. An Operator grant never supplies Task
Service Director authority.

Presentation metadata is display/filter-only. For each returned task, the UI
resolves the Project display name and badge only by an exact canonical
`project_id` lookup in the authenticated response. A missing exact record
remains visibly unavailable; it never falls back to a similar name, badge,
workspace, cwd, group, runtime ID, or Agent name. Online status is likewise an
exact `cutex_session_id` join against the live Agent Bus roster.

See [r36-management-ui.md](r36-management-ui.md) for the shared credential and
navigation contract.
