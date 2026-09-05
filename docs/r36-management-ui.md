# R36 authenticated Management TUI

This change is based on release input
`64703b756c311d3b9bdb93c4f1600b32902c5374`. It changes only Cutex. It does
not deploy a binary, update a live store, close a live session, or copy the
paired `cute-codex` work into this repository.

## Human/Management authentication boundary

The normal `cutex` TUI does not require `CUTEX_AGENT_ID` to open Projects or
Tasks. These panels use the dedicated configured Management root credential
and the root-scoped local HTTP routes below:

- `GET /v2/agent-management/projects`
- `GET /v2/agent-management/projects/{project_id}`
- `POST /v2/agent-management/project-presentation`
- `POST /v2/agent-management/operator-actions`
- `POST /v2/task-service/management-query`

The Management server creates the non-serializable
`HumanManagementPrincipal` only after bearer authentication. Request bodies
cannot claim a caller session, runtime Agent ID, Primary Director, seat, or
project authority. Ordinary Management, Agent Bus, and Task Service seat
credentials do not authenticate these routes.

Project collection and detail reads retain the provider's canonical
`ProjectId`, authority epoch, exact Primary Director, exact Operator grants,
and managed roster. Presentation values remain display-only.

The Tasks route is read-only. It reads the exact current `cutex-director` seat
occupant, holds a current seated-principal snapshot for the query, and admits
only projects whose current Agent Management Primary Director is that exact
durable session. The existing Director projection then filters Task Service
records to that exact project set. Agent names, cwd, groups, runtime IDs,
native workspaces, and presentation values never supply authority. Online
status in the TUI is joined only by exact durable `cutex_session_id`.

## Operator writes and legacy review

Human grant/revoke is a separate, durable action domain. Every request has a
stable action ID and compares both the project authority epoch and the full
Operator grant-set revision. A successful write stores an idempotency receipt,
increments the grant revision, and appends an immutable Operator audit event.
The grant, receipt, and event explicitly record that Human Management
performed the action while preserving the exact Primary Director as the
authority anchor.

Legacy retained-predecessor rotations are projected as review candidates only
when an exact completed retained rotation names the current successor and
current authority epoch, and the exact active predecessor still lacks an
Operator grant. Reading the candidate performs no write. The user must choose
Grant and pass the normal confirmation and CAS path; Cutex never silently
repairs live state.

## Navigation and presentation

The four direct workspace shortcuts are `Alt+M` Managed, `Alt+R` Recent,
`Alt+P` Cutex Projects, and `Alt+T` Tasks. The generic Codex project catalog is
named **Workspaces** and remains a secondary Managed-list entry.

- `Tab`/`BackTab` traverse focus inside the current workspace, including the
  Managed list, the unified Agent Inspector, and back. They never switch
  workspaces.
- On an ordinary top-level list, `Left`/`Right` move to the adjacent main tab
  in the order Managed, Recent, Cutex Projects, Tasks, without wrapping. In an
  Inspector, editor, or modal they only expand, collapse, or move local focus.
  They never invoke a lifecycle operation or commit any other write.
- `Up`/`Down` select rows or an explicit confirmation choice.
- `Enter` opens the explicitly selected default, detail, or action.
- `Alt+A` opens actions, `Alt+E` opens Edit/Settings, and `F5` refreshes.
- `Esc` returns to the parent view or cancels the current review.

Every workspace model is retained across a direct switch, including its
selection, filter, scroll anchor, and view mode. Modal and editor input is also
retained rather than discarded by a global shortcut.

Managed Agent rows take their stable primary label only from
`ManagedAgentRecord.spec.name`. Pressing `Alt+V` toggles an optional secondary
native thread-title line. It changes no stored value, and no title-only mode is
offered. Unmanaged Recent rows may continue to use the native thread title.

`Tab` on a Managed Agent row focuses one right-side Inspector with Overview,
Actions, and Settings sections. `Alt+A` opens Actions and `Alt+E` opens Settings;
Settings links to the selected Agent's Project badge editor. `Enter` on an
online row attaches to or enters its exact route. An offline row first shows a
cancel-by-default `Start & attach?` review. Dispatch failure stays in the TUI
with an inline error and never launches a fallback terminal.

Project detail sections are Overview, Members, Operators, and Appearance.
Project filtering matches canonical ID, display name, and badge. Appearance
updates compare both authority epoch and presentation revision. Badges occupy
one or two terminal cells; `CX` is valid, and the deterministic default for
`cutex-stack-main` is `CS`. Both the Home Agent list and Projects panel render
badge text in white on the configured project color.

Tasks refresh on a one-second cadence and preserve selection across refreshes.
Both Projects and Tasks have narrow-terminal and resize regression coverage.
