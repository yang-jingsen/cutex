# Reviewed Job MCP: partial verification checkpoint

Base: `1a71a850885ac6fd9d918dcd045bdd065d639e81`, tree
`3c6a7eb13c42a33321fcb830fbe8b29895b331c4`. Linux-only descendant;
no Windows/TUI/native/Job source changes. Contract: [reviewed-job-mcp-launch.md](reviewed-job-mcp-launch.md).

## Actual evidence and open gates

| Criterion | Observation |
|---|---|
| Optional dedicated descriptor, root review, custody and peer proof | Implemented; actual private root review/launch passed |
| Native configured Job tool discovery | Passed on default Cutex/Core bytes, four Job tools advertised |
| Actual Core invocation, grant issuer, daemon execution | Passed; fake Responses selected `mcp__cutex_job.submit`; no injected `_meta` or manual grant |
| Exact replay/query/output | Passed: repeated action produced one exited job; actual query/read_output returned `configured-core-output` |
| Terminal notification | Passed: authoritative Job delivery plus Bus delivered record and native `codex.external-input-receipt.v1` |
| Ordinary MCP approval | Two actual native elicitation requests observed and explicitly accepted by fixture; no auto-approve config |
| Approval decline | Not reached: separate fixture hit existing socket-length limit before runtime launch |
| Read-only own-file read/two denied writes and actual nested helper | Pending; prepared fixture not executed; no sandbox claim from fullaccess |
| Restart/descriptor omission/tamper/CAS/provider negatives | Source guards implemented; focused custody/peer/schema tests pass; expanded actual matrix pending |
| Bootstrap/create carrying descriptor | Implemented through existing reviewed intent; inherited bootstrap tests pass; Some end-to-end not yet proved |
| Missing metadata/wrong thread/external policy/wrong cwd/retired runtime | Existing Job/provider contracts unchanged; this task's configured-path negative matrix remains pending |
| Default None | Schema and unchanged stock regressions pass; no new capability without Some; separate actual None discovery gate pending |

The actual successful fixture is guest
`/home/cutex-linux-test/acceptance-upload/job-launch-r1/a2`.
Its result contains a legacy inherited `boundary` string saying harness-driven
metadata; that label is incorrect. `actualCore:true`, model requests, native
elicitation events, and the fixture establish actual native invocation. The
corrected current fixture label is not a rewritten historical result.

Successful fixture SHA256:
`d3ce9d05e366d97fe002dbf958568dbe52f1447901b673579ce69e062573efe0`.
Immutable base setup fixture SHA256:
`ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e`.
Current `scripts/reviewed_job_vm.py` additionally prepares short-path read-only,
restart/negative and decline scenarios; those additions are **not executed
evidence**. It checks the exact setup fixture hash before use.

## Preserved failures and bounded continuation decision

1. `a1`: fixture emitted Code Mode `exec`, although this private unknown-model
   configuration advertised ordinary namespaced function tools. No Job was
   created. Corrected to actual advertised `mcp__cutex_job` function calls and
   same-owner thread subscription; `a2` passed.
2. `decline1`: runtime launch rejected its longer private socket pathname before
   native start: `owner_action_required: app-server Unix socket path is too long`.
   Approval decline was not tested. This is the existing path limit, not an
   authorization failure or proof of a Job defect.

Both original attempts remain. New VM attempts paused under this task's
repeated-unexpected-failure instruction. Proposed continuation is only shorter
owned fixture basenames (e.g. `d1`, `r1`) with existing socket geometry unchanged;
no source limit relaxation, privilege change, new provider authority, or live
mutation is proposed. Full task acceptance is not claimed.

## Checks and byte identities

Default Cargo `check --locked --bins` passed. Focused default library tests:
`job_mcp` 4 passed; `stock` 7 passed; `bootstrap` 8 passed (19 final selections,
distinct names; preliminary repeat executions are not additional coverage).
`cargo fmt --all -- --check` and `git diff --check` passed after formatting only
the changed lines. Existing unrelated warnings remain. Full suite, Windows,
real provider and production deployment omitted.

Successful a2 default binaries:

- Cutex `7fd063c1f8083b60d83cc8361cc24a6ccd57d1f77ea40401d17dd79b9b8e6b8f`
- facade `7f78bd66b124c50d6eddc3799d0b44de481ba0df2a1025604820e851e2f223bf`
- Job f3bc9c8 `d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2`
- native0c425 CLI `f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4`
- native0c425 Core `9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde`
- U host `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45`
- schema `459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9`

Native/Job bytes reused, no rebuild. All fake actors/auth/endpoints/configs were
private guest fixtures; parent HTTP calls and inherited Cutex connect guard
restrict owned endpoints. No hostile-child OS-egress or full sandbox guarantee.
Owned child groups cleaned by fixture finally blocks; VM remains running.
No real credentials/provider, host PRH/service/profile/backup, canonical refs or
deployment touched. Prior S2/S46/PID/socket limitations remain, not remediated.

Mambo owner root approximately13GiB,377GiB free; guest acceptance-upload4.4GiB,
51GiB free. Original artifacts/failures preserved. This checkpoint is reference
only pending the outstanding acceptance gates; rollback means reject it before
exposure, not downgrade opted-in receipt stores or rewrite history.
