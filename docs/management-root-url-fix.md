# Management root CLI URL join

Candidate on exact base `18c44412c7ded809765cea6d6cdd95c79f488ce4`
(tree `c6009c0d86ab7c1cfdc84eaea5dd04a067a4cfc7`). Combined TUI and
F567 native pins retained. No schema, receipt, authority or lifecycle change.

The shared blocking Management client now inserts exactly one separator at
the base/endpoint boundary. Root bases with or without a trailing slash and
endpoint paths with or without a leading slash work. Configured path prefixes
remain prefixes; endpoint queries remain intact. This is deliberately not
`Url::join`, which could discard the base path or replace its authority.
No redirects, credential-bearing helper, proxy or router normalization is added.
The stock CLI's existing local-root/credential checks and HTTP transport remain
unchanged. This does not introduce support for query-bearing base URLs or a
general URL-resolution framework.

## Evidence

- Named R2 blocker SHA matched the assignment.
- `cargo test --locked --lib management::remote::tests`: 1 passed, covering
  four base/path slash combinations, root, prefix/query and authority retention.
- Default `cargo build --locked --bin cutex --bin cutex-mcp`: passed.
- `cargo check --locked --bins`, `cargo fmt --all -- --check`, scoped self-review
  and `git diff --check`: passed. Existing unused/dead-code warnings retained.
- `python3 tests/management_root_url_boundary.py root-url-02`: real default
  Cutex CLI `session stock --management-url http://127.0.0.1:<ownedport>/`
  reached the exact root-authenticated `/v2/agent-management/explicit-launch`
  handler and provider review. Both slash variants returned the intended
  `stock durable record missing` for an intentionally absent private subject,
  not 404. This is a negative review roundtrip, not successful activation.
  Real private Bus/Management processes were tracked and stopped; fake root
  credential stayed in private config, not argv or reports. The existing
  connect tripwire rejected an unowned TCP endpoint before syscall.

Failure preserved: `root-url-01` stopped at the connect tripwire because the
first fixture omitted Management startup's required private Bus. No default
endpoint was contacted. `root-url-02` supplied an owned Bus and corrected the
fixture to supported 24xxx ports; no product authority relaxation.

All fixture state/builds/temp remain under the owner Mambo root. No native
runtime, model, production endpoint/home or Agent lifecycle was used. Release
still owns the successful authorization/Create/verify proof through exact
packaged `pilot.sh`; native/PTY/full suite/Windows/capacity/visible-only checks
were not repeated. Earlier F567, TUI and lifecycle evidence and its incident/
PID/socket limits remain unchanged. No deployment or production-readiness claim.
Reject/revert this candidate before exposure for rollback; no state migration.

Default artifact manifest beside frozen binaries records final commit/tree,
digests and resource footprint. Intended use: integration candidate pending
Director acceptance, then Release's bounded private package consumer gate.
