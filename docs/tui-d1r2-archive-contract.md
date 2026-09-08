# Reversible Agent Archive (D1R2)

Human policy: Archive hides an ordinary Agent without deleting native history,
changing identity/profile/formal name, or removing current Project membership.
Restore returns that same identity Offline; it never restores historical grants,
seats or membership. Missing/archived current Projects require explicit repair or
Project restoration first. Permanent Management Close remains roster
`retired_at` history and cannot be restored by this operation.

The durable legacy `Retired` archive-state spelling remains wire-compatible and
is reversible only when the roster is not permanently retired. Current provider
queries project this state by exact durable ID, without persisting a second
archive flag. Project workspaces expose `archived_agents`, summaries separate
`archived_member_count`; ordinary Managed/Recent/Project member lists hide them.
Project Members Ctrl+H includes archives; the Archive utility lists reversible
records. Fresh Create/Add and ordinary managed lifecycle reject archived Agents.

## Authority and confirmation

Root-Human POST `/v2/agent-management/archive-review` returns typed exact ID,
formal name, operation, current Project, durable revision/generation and hashes
of original durable and relevant authority evidence. POST
`/v2/agent-management/archive-actions` consumes that review and an action ID.
Existing CLI `session retire` (alias `archive`) and Restore use this service;
legacy session mutation routes cannot bypass it. Management Close is labelled
permanent retire. TUI confirmation defaults Cancel and retains the original
request across uncertainty; it never refreshes CAS behind the confirmation.

Director seats, Operator grants and every nonclosed Task assignment protect the
identity, including durable-only Agents. Provider-first locking holds provider,
Task read fence and durable store fence across the decision/stop/commit.
Unknown/corrupt observations reject mutation. Names come only from dedicated
formal name or explicit historical roster name, never thread title or cwd.

## Stop proof and recovery

Online Archive supports only an affirmatively identified Linux Host containment:
exact occurrence PID membership plus a captured cgroup events object, existing
scope stop and independent empty check. PID-only stop, absent scopes, remote or
unsupported online backends reject before stop/journaling. This is not a claim
about arbitrary processes escaping containment. Manager and AgentBus offline
observations must also succeed.

New durable records track `runtime_history_known`; legacy missing means unknown
(omitted again on serialization, preserving historical record receipts).
Generation zero with tracking and no occurrence history/claims plus successful
offline observations supports never-managed-started Agents. Prior affirmative
Stopped/Committed archive proof bound to exact ID/native ID/generation also
supports recovery and Restore. An unresolved launch claim rejects mutation.

Provider action stages are Prepared, Stopped, Committed, StoppedNotArchived and
Uncertain. Stopped.result is the reviewed occurrence whose stop was proven;
Committed.result is the resulting durable record. Immutable per-stage audit and
the final receipt committed atomically with durable state allow exact-action
reconciliation after losing the provider final write. Changed payload replay
conflicts; historical final replay is unchanged after rename or Restore.
StoppedNotArchived reports actual stop without archive success. An unavailable
proof/journal after a stop may remain uncertain and fail closed; cancellation
does not roll back a stop. CLI errors retain the exact retry request.

## Verification boundary

Private authenticated HTTP tests exercise production routing/provider/CLI-TUI
adapters and disk persistence, including membership-preserving Archive/Restore,
protected Director, wrong token, unsupported real process, archived Project and
immutable replay. Library tests cover Task/Operator guards, stale confirmation,
corrupt sources, permanent retirement, missing Project, lost provider final
receipt and an owned POSIX parent/child stop with unrelated process preserved.
The latter uses an injected process-group oracle, not the production systemd
adapter. No live runtime, systemd user manager, production identity or Windows
acceptance is claimed. The inherited PTY oracle uses a mock foreground child.

Archive is only a lower-retention-priority signal: no deletion, cleanup daemon,
notification/outbox policy change or automatic activation is implemented here.
D2 lifecycle entry expansion and E long Inspector accessibility remain separate.
