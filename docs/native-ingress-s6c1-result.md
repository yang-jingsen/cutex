# S6c1 result — private integration candidate

Exact parent: `c7ad7e1d3e1a14fc9a96cd19fc21d5b87016d11a`.
This document belongs to the immutable candidate commit containing it; submission
records its commit/tree. Contract and complete native/schema pins are in
`native-ingress-s6c1.md`. No deployment or business-delivery acceptance.

## Criteria and executable evidence

| Boundary | Result |
|---|---|
| Exact U+S6 bundle, owner binding, version handshake | Passed actual private Human/provider activation and native launch; unchanged stock remains registration-only and rejects client ingress |
| Current occurrence and identity fences | Passed wrong owner/thread/generation, stale client after restart, root-auth denial, exact action replay; recorded executable/process checked around RPC |
| Generic canonical submit/status/retry | Passed actual native A4, stable replay/status across generations 1/2/3, lost-reply reconciliation, explicit held retry once and retry conflict |
| Receiver policy | Passed default 10000 rejection, off and raised 20000 acceptance; typed invalid-value tests; policy is not in sender RPC schema |
| History oracle | Three adjacent commit/context pairs, ordinals 1/2/3, independently recomputed digests/receipts, no duplicate pairs or per-request duplicate canonical bodies, one native metadata row |
| Generic restart fence | Production pre-stop function rejected marked entry and left the real owned subprocess alive; no full Management restart campaign repeated |
| Compatibility | 29 distinct focused Rust regressions passed; existing MCP/query/send/Task tests retained; no business routing/ACK changes |

Final real composed run: task-root `s6c04/PASS.json`, `s6c04-run.log` and
`s6c04-history-oracle.json`. The root is
`/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.
Durable ID `cutex.01a084e4-62b8-70d0-9600-7feb02e34f80`, native ID
`01a084e4-62b8-70d0-9600-7feb02e34f80`; three runtime generations preserve both.
Four dummy Responses requests cover three events (one held then explicitly
retried). Real Cutex providers/stores and frozen native Unix RPC were exercised;
dummy profiles/auth, private fixture setup and fake Responses are not real
provider/model acceptance. Owned children were cleaned up.

Final tested Cutex binary SHA-256:
`a3dfb03bb1c4f5b3b9f91ff124c1f280b71d64aa27cad64ac8216e02fae74df3`.
Facade SHA-256:
`39afef4f28e5666548afb7cc4f18bbe25d194731d0b13431aeb158ec070756d4`.
New reviewed `s6c04/bundle-0.json` SHA-256:
`f73bd6c7bd64ceb64d3428e4e41385c09427a9487b2b66c247da0259bf73752e`.
Accepted native binary/schema/companion artifacts remained read-only.

## Commands and scope

All Cargo commands used task-owned HOME, TMPDIR, CARGO_HOME and target, scrubbed
environment and no concurrent target writer. Passed:

- `cargo build --bins`; `cargo check --bins` (default features).
- `cargo test --test external_input_controller --no-run`.
- `cargo test --lib external_input -- --test-threads=1`: 7.
- `cargo test --lib launch::stock -- --test-threads=1`: 4.
- `cargo test --lib stock_registration_only -- --test-threads=1`: 1.
- `cargo test --lib mcp -- --test-threads=1`: 16.
- `cargo test --bin cutex marked_generic_restart_stop_fence -- --test-threads=1`: 1.
- `python3 tests/external_input_schema.py`: six exact generated wire shapes.
- `python3 tests/external_input_history.py s6c04`: independent read-only oracle.
- `cargo fmt --all -- --check`; `git diff --check`; full scoped diff self-review.

An earlier successful six-test client run overlaps the final seven: 35 Rust test
executions, **29 distinct**, excluding composed controller invocations. Earlier
`s6c03` passed the overlapping composed proof before the final PID and hint
checks; it is not additional independent coverage. Build warnings remain the
existing unused/dead surfaces. No dependency or lockfile changes.

Composed entrance: `tests/external_input_boundary.py s6c04 <controller-test-bin>`
under `bwrap --unshare-user --unshare-net --ro-bind / / --dev /dev`, with only the
task root bind-writable and scrubbed private HOME/TMPDIR. The script requires
network isolation and uses the inherited explicit endpoint connect tripwire.
The exact controller executable and command are in the run evidence.

## Preserved failures and limitations

`s6c01`: missing dummy bootstrap configuration caused initialize timeout; the
tripwire denied an unallowlisted connection before syscall. Corrected private
bootstrap configuration, not production policy. `s6c02`: namespace `/dev/null`
permission failure prevented spawn and retained the claim. A bounded namespace
oracle identified it; `--dev /dev` repaired the harness. No blind same-create
retry or state editing. A nested bubblewrap sandbox warning did not prevent
Ready; this no-tool probe does not establish sandbox execution. Initial schema
CLI failure was resolved by the frozen experimental archive, without rebuilding
native. Failure logs remain under their original run names.

Reuse accepted S4/S5 and native S6b2r2 launch/PTY/sandbox/queue/P0 evidence;
no new PTY (interaction shell unchanged), full workspace/all-features, native
crash campaign, paid model, production OAuth, Windows, Android, TUI or live
acceptance. Patched bundle CLI attach deliberately rejects until a compatible
pinned CLI artifact is supplied. A4 is context publication, not business or
output completion. No Bus/Task/Job ACK or inbound worker is enabled.

Retain S4 seconds-truncated PID-start-time risk, two established baseline
Management socket-length failures, and unresolved prior S2/S46 incident
disclosures. Same-UID trust is not hostile same-UID isolation. Conservative
rehashing and bounded event-drain requirements are documented in the contract.

Resources: task root 8.2 GiB before, 8.6 GiB retained after; filesystem free
425 GiB before, 424 GiB after (rounded). Under 20 GiB / above 100 GiB floors.
No home-cache growth, native build, unrelated cleanup or live mutations.

Next: S6c2 may connect existing durable Bus/outbox records to this trusted client,
with source mapping and business CAS/ACK ordering; no second inbox. Director
acceptance is required. Before exposure, reject/revert candidate for rollback;
never downgrade a history-bearing marked private store to legacy writers.
