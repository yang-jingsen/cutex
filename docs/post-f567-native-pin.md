# Post-F5/F6/F7 exact native pin

Integration candidate only; no deployment or live migration.

Base: `00783435494dfb2543c485d805a225baf7211dd8`, tree
`0005463189e6c4b6176ee612ea5c16197722afda` (combined current TUI retained).

The version-3 bundle now requires native commit
`0c425b5f9fca90835fd2b4377a1bca212532f66c`, app-server
`9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde`,
CLI `f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4`.
Host and schema remain unchanged. The S6E constant names denote the unchanged
protocol family, not permission to accept arbitrary descendants. New runtime
provenance labels identify F567/0c425b5f. Bundle/marker/wire versions do not change.

## Compatibility and replay

Old a83 bundles fail identity validation for new activation/launch, pending
launch continuation, and attach. No K/default fallback is added. No persisted
marker, receipt, native history, or frozen artifact is rewritten. Existing exact
completed activation/Ready receipt replay remains a historical return through
the unchanged early receipt branches; it does not spawn a process. Execution
and reconnect still load and validate the pinned bundle. There is no authorized
live a83 migration in this task. Earlier official-stock v1 and c2aaceb4 v2
identity branches, and the unmarked K path, are unchanged.

## Verification

Frozen native manifest SHA-256
`db374163b53b366eef064669ae2bebea9de59071f758abf815dce2d6f799627d`
and all three executable/host hashes plus schema were recomputed and matched.
The named Release blocker digest also matched the assignment.

Default-feature focused commands (private HOME/TMPDIR/Cargo paths on Mambo):

- `cargo test --locked --lib launch::stock::tests`: 5 passed. Exact new bundle,
  individually mixed old source/server/CLI, spoofed executable/host/schema,
  complete old a83 rejection, earlier v1/v2, capability, config and marker guards.
- `cargo test --locked --lib bootstrap`: 8 passed. Existing provider recovery,
  no-fallback and bootstrap projection fixtures; not a new native pilot.
- `cargo test --locked --bin cutex stock_lifecycle`: 2 passed, including actual
  owned subprocess survival at the generic restart pre-stop fence.
- `cargo build --locked --bin cutex --bin cutex-mcp`: passed, default features.
- `cargo fmt --all`, full scoped diff inspection and `git diff --check`: passed.

15 distinct selected tests; no failed test/build. Existing unused/dead-code
warnings remain, outside this delta. New pin tests are identity/unit oracles,
not actual provider activation on new bytes. Release owns the next private
provider/bootstrap/pilot consumer verification and packaging after acceptance.
Native F567 protocol, host and prior CLI/PTY/sandbox evidence are reused by
reference to the frozen native manifest, not rerun or promoted to new Cutex
end-to-end acceptance. No native/V8/full workspace build, live endpoints,
Windows, capacity-100 or visible-only work was performed.

Resource before: 11 GiB owned root, 391 GiB filesystem free; artifact manifest
records final footprint. Existing S2/S46 incident disclosures, PID precision/
descendant limits and two historical socket failures remain unchanged. No
production readiness or remediation claim. Before exposure, reject this
candidate to roll back source choice; do not downgrade candidate-written state
or rewrite old histories. Next: Release consumes exact candidate/default bytes
and runs its authorized private consumer gate.
