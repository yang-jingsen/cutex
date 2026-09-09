# S4r2 safe checkpoint — PARTIAL / REFERENCE_ONLY

Director requested suspension at the nearest safe source checkpoint for the Human release window. This is **not** completion of S4r2 and must not be integrated, deployed, or used to activate private Agents yet. Exact parent is `9f2aac78b758a02f4b3171dd524e5d55b90ca987` (tree `2b6bf22f707a4c38e9313f14c5820c9188ed05b8`), retaining accepted S3/S2. The commit containing this document is the immutable continuation point.

## Present partial code

- Optional typed `CutexSessionRecord.explicit_launch` and durable `explicit_launch_receipts` map. Requirement is distinct from profile/name/backend and is not cleared by the existing runtime-clear helper.
- `agent_management/explicit_launch.rs`: draft v1 home/native/bundle-reference contract; review/activation method with Human principal, provider-first mutation lock, Task read fence, durable record digest/revision/generation checks, offline/no-claim validation, existing archive protection checks, and one durable save for activation plus immutable receipt. Exact action replay returns its receipt; changed review conflicts. No clear API.
- Root-admin-only `/v2/agent-management/explicit-launch` route and production context callback. This exposes draft Review/Activate only, **not** an executable launch command. Public authentication has not yet been exercised for this delta.
- Initial generic launch guards in managed online, foreground/takeover and CLI restart, plus provider Restart before its normal stop path. Complete route/race coverage is not established.
- Registration-only bridge option (default false) skips poll after heartbeat refresh. Not yet selected by a stock launch path or tested.

## Still required before any candidate claim

1. Complete the contract: manifest must validate pinned U executable, facade/companions, actual stock schema and allowed configuration provenance. Current code checks only canonical paths/reference bytes/version/native UUID; it does **not** yet establish bundle semantics, native rollout existence/uniqueness, current profile/config snapshot or local host/backend eligibility. Do not activate using this incomplete validator.
2. Implement the opt-in CLI/lifecycle branch, strict dummy-profile configuration resolution/review, launch-claim recovery, actual stock owner, authoritative registration readback and current generation. No launch/resume/attach command was added.
3. Audit all generic entrypoints and prestop races, including provider recovery branches and foreground entrypoints that may reconcile before reaching the current guard. Ensure preserved metadata across archive/restore, profile edits and every reconciliation helper. Current guards alone are not a proven complete fence.
4. Wire registration-only mode and truthful stock schema/source projection; preserve actual approval/sandbox behavior, single owner and shared native home. No inbound support/A4 claim.
5. Add/run S4r2's independent real private provider/auth/CAS/replay tests, same-thread two-profile launch/restart tests, failures/concurrency/uncertain recovery, MCP denials and Linux approval PTY. No new unit or integration tests were added/run in this partial checkpoint. New struct fields may require test-only fixture initializer updates: `cargo check --bins` does not compile all test configurations.
6. Resolve draft reuse of Archive review/guard vocabulary into an accurate explicit-launch review contract without silently changing protection policy. Inspect complete delta and finalize docs/compatibility restrictions before submission as an integration candidate.

## Checks and resource boundary

`cargo check --locked --bins` passed (4.35 seconds final incremental check, existing warnings). One initial compile error converting typed `ValueError` into anyhow was fixed; evidence remains `s4-check.log`. Final check log: `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/s4-checkpoint-check.log`. `cargo fmt --all` completed; `git diff --check` passed. Changes were reviewed at checkpoint level, not full security/acceptance self-verification. No broad test campaign was started for the release pause.

Cargo used scrubbed environment and existing task-owned home/tmp/cache/target, read-only Rust toolchain. Mambo currently approximately 483 GiB free; existing task root plus bounded incremental check output stays below 20 GiB. No kernel build. All check/format child handles completed. **No S4 stock/private service/runtime processes were launched**, so no probe cleanup or live lifecycle action was required. No global process arguments, live config/auth, production state or S2 leaked output were inspected; no credential remediation claim. S2 reports remain unchanged.

Resume only after Director reissues/continues authority following the release window. Next step: complete pure manifest/config validation and its focused tests, then finish shared prestop guards before enabling the new launch branch. The experimental private store must have no legacy Cutex writers. Stop/cleanup must never erase the durable explicit-launch requirement. No automatic downgrade or mixed-writer compatibility is promised. No current-release files, canonical refs, remote refs or deployed artifacts changed.
