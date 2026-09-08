# D2 safe workflow subset — integration candidate, not full D2

Accepted product base: `929c67d821f7c8efe9394ac066c9ec64dfc515d8`.
Reference lineage: `0854002baccf91f999dd275f7baa0d15c0a38e91` then
`325831f6f579bc8a735382fd393c12594dfdf556`. Those references remain immutable.
This document supersedes their pending-adapter wording for the subset below.

## Supported subset

- Recent unmanaged Enter resumes the exact saved native ID from the paired
  local catalog. It does not adopt/import, initialize management services or
  change the global profile. The native child receives an allowlisted clean
  environment and returns through TerminalShell. Unavailable identity/cwd
  observations fail closed. No automatic retry follows an uncertain launch.
- Recent Alt+A explicitly reviews durable adoption **and roster import**,
  without assignment. Formal name starts blank, never from the native title.
  The editor uses shared text/paste/cursor handling; confirmation defaults to
  Cancel. Repeat/Release do not submit. Profile remains inherited (`None`).
- Root-authenticated `POST /v2/agent-management/adopt-saved-native` checks the
  exact saved native ID/cwd at the configured local source. The provider locks
  roster before durable state, rejects any existing native mapping (including
  local-only, archived and permanent identities), and atomically saves the
  durable record with its action receipt. Existing import handles roster
  audit/CAS/journal; no implicit Project assignment or role creation occurs.
- Durable adoption and import are recoverable separate stages. The stored
  import intent retains its original candidate fence. Same-action retries
  reconcile; changed payloads conflict. A committed durable stage with failed
  import is reported as partial, never rolled back or duplicated. Fresh name,
  profile or membership observations do not substitute a new confirmation.
  Historical import receipts/audit are not rewritten.
- Projects Create can retain a draft while selecting/Adopting a saved Recent
  session. Returning refreshes candidates while retaining the selected durable
  ID when available. Existing confirmed Create/Add/Detach+Move stay unchanged.
- Project Members and managed Recent Enter/Alt+A/Alt+E use the existing Managed
  action/settings resolver, keyed by durable ID. Return restores the origin
  page and Managed selection; Project member observations refresh afterward.
  Existing Archive uses D1R2 review/action guards. Archived members direct the
  user to Archive utility; permanent retirement cannot Restore.
- Foreground attach/resume returns to its caller instead of exiting the outer
  process. Nonzero child status becomes an error containing that status; CLI
  top-level error exit is no longer necessarily the child's exact numeric code.

## Explicitly incomplete / release boundaries

New Agent and New Session forms remain unavailable. Native bootstrap cannot
claim persistence from `thread/read(false)`; the second real failed
create/read/exit/resume experiment is retained in the reference commit and was
not repeated. `thread/read(true)` is not a verified flush barrier on the
installed binary. No model turn, fixed-delay workaround or kernel change is
authorized. The experimental bootstrap function is test-only and opt-in.

Adopt currently exposes inherited profile and an explicit unassigned result;
optional profile/Project selection within one wizard is deferred. Use existing
Projects Create/Add as a separate confirmation. Existing local-only durable
records are rejected rather than silently upgraded or given another identity.
Cancel before submission writes nothing; leaving after an uncertain response
does not undo committed work. Retrying requires the retained action ID; a new
review cannot duplicate an already committed native mapping.

The actual installed native saved-session protocol and interactive PTY child
were exercised in private state without a prompt/model turn. The PTY oracle
proves return, unchanged Cutex durable/roster snapshots and terminal restoration,
not a paid usable-session or managed-runtime acceptance. Authenticated HTTP
tests simulate only fresh native observation; real provider persistence and
production replay callback are exercised. Existing Linux shell PTY uses a mock
foreground child. Windows, live cute-alden/cgroup/runtime, deployment and manual
end-to-end acceptance remain release gates. Phase E Inspector accessibility and
final cross-terminal exercise remain separate.

Rollback before exposure: reject/revert the candidate source commit. No live
configuration, service, identity, remote ref or installed artifact was changed.

## Executed checks

All Cargo invocations used `--offline --locked`, a scrubbed environment and
private HOME; no production DBUS/catalog/manager. Final relevant evidence:

- `cargo test --lib agent_management`: 122 passed (includes the first adoption
  test; before the additional partial-stage test).
- `cargo test --lib durable_adoption`: 2 passed, including unsupported import,
  retained original candidate after durable change, exact replay, duplicate
  native ID, formal name distinct from title, inherited profile and unassigned
  membership. These overlap the 122 above; do not add them as unique tests.
- `cargo test --bin cutex session_tui`: 236 passed, 1 opt-in PTY test ignored.
  Includes root HTTP rejection of ordinary/seat/bus/no credentials, adoption
  persistence and production replay callback, Create/Add/Move and archive guards.
- Final focus adjustment: `cargo test --bin cutex ui_contract_k07_k08`:
  3 passed (overlap the TUI suite).
- Saved private native protocol: 1 opt-in test passed; no bootstrap/model turn.
  Actual native PTY script passed return/unchanged-store/termios checks.
- `scripts/tui-b2-pty.py`: passed 20 page round trips, mock foreground raw/cooked
  handoff, error restore, single input ownership and alternate-screen balance.
- `cargo check`, `cargo fmt --all -- --check`, `git diff --check`: passed.
  Existing dead-code/platform warnings remain; no dependency/lockfile changes.

Criterion status: D06 supported; D07/New Agent deferred; D08–D10 partial
(explicit-name unassigned adoption/import, not the full optional wizard);
D11–D12 retained Create/Add/Move with draft escape; D19–D20 shared actions and
return tested within the process boundaries above. No full D2/live acceptance.
