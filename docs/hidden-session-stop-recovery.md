# Owner Stop and scope-timeout recovery

`cutex session offline ID` and `cutex session close ID`, including TUI runtime
Stop, use the configured dedicated Management root credential. They fail if
that credential is missing or collides with the Agent Bus bearer. No Agent or
task identity is inferred from a local command.

The existing `/v2/sessions/ID/cutex/requests` endpoint accepts that root bearer
for `cutex/runtime/offline` and `cutex/runtime/close` against an exact hidden
durable session. Host, registration-class, retirement and runtime-generation
checks still apply. Ordinary reads and ordinary-bearer mutations remain
visibility-filtered. Root credentials may already be used for ordinary
Management requests; their existing Human authority, not network locality,
authorizes this exception. Stopping does not change external visibility.

After systemd's first termination wait, PID fallback may finish the shutdown.
The stop implementation rechecks the complete scope before reporting failure.
An unavailable scope inspector never counts as an empty scope.

An authenticated Agent Management caller can replay its exact original Offline
request after `scope_terminate_timeout` with successful PID outcomes. The caller
must still hold current project authority. Recovery preserves the original
failure event, captures the original occurrence in the existing fence field,
checks scope/PID/socket/Agent-Bus/app-server/cute-alden absence, and clears that
exact claim under the session-store lock. The same action then completes; exact
replay returns its completed receipt. A crash after cleanup is recoverable
without clearing a second occurrence.

For pre-fix failures without a saved fence, recovery requires the exact PID set
in the original successful PID receipt and an app-server binding created before
the original action. A newer binding, changed occurrence, live/reused PID,
reachable endpoint, unavailable evidence or changed caller/request is fenced.
This allowlist covers Offline scope timeouts, not arbitrary lifecycle failures.
Replay the original action before separately clearing a legacy claim: once its
uncaptured binding is removed, the historical identity can no longer be proven.

Deploy the updated provider in the Agent Bus as well as the Management service
and CLI before relying on these fixes. Installing only a client executable does
not replace either running service. No native/migration pins change.
