# S5b result — private candidate, not a release

Parent/base `5a81e04280bdb655d0e9013512f68257ff283582`, tree
`0646245790c3826f1a25b9d5fff59ea5eebc4818`. The immutable candidate is the
commit containing this report; exact head/tree accompany the Task submission.
Native public reference is K `ef53716b7673ad14c24b977667334e31e66110d8`.
Scope: facade semantic/schema helpers, existing authenticated HTTP adapters,
tests/docs only. No provider lifecycle, authority, ledger, or kernel edits.

## Operation evidence

All eight native Management operations and their public required fields are
translated; see [field mapping](management-mcp-s5b.md). Unknown/inappropriate
fields, grant/revoke, and Human stock activation are not exposed.

| Operation | Actual stock MCP / real private provider | Not established |
| --- | --- | --- |
| query_managed | Complete; imported null historical provenance supported; exact replay and persisted receipt equal | Production access |
| offline | Complete on running owned stock Worker; exact replay; changed close under same action conflicts | systemd/cgroup stop |
| close | Complete, separate already-offline disposable Agent permanently retired; replay equal | Stock bootstrap/replacement |
| online, restart | Provider owner_action_required; stock marker/binding/generation unchanged, no fallback | Successful generic stock activation/restart |
| create, replace, director_rotate | Provider invalid_request for empty profile; valid payload denied for non-Director, no spawn/state change | Successful stock bootstrap, replace, or rotation; these are NOT end-to-end accepted |
| cutex_agent_list | Local group-visible real runtime/durable/native/formal-name projection; true all_hosts/all_groups and forged fields rejected | Cross-host/all-group discovery; offline registry enumeration |

Rotation result-family translation is unit-tested; existing provider executable
tests cover missing/stale Task seat preflight and immutable completed rotation
replay after unrelated legacy seat rebind. Lifecycle callbacks in those provider
tests are controlled fakes, not successful stock rotation.

## Executed boundaries

Final `s5b04`: **34 actual configured stock MCP calls**, including replay/error
calls, **105 model-free dummy Responses requests** (three native bootstrap turns
included). Current real Cutex Bus/Management/Task providers and private persisted
stores; private root API only fixture setup, normal runtime Bus credential in
facade. Actual Core tool-search schemas were captured. No authority/token fields
are accepted from model arguments. Dynamic Cutex child connections use the
existing owned-endpoint tripwire; missing Core identity rejects before transport.

Actual checks: Worker/foreign-project denial; stale/foreign/missing occurrence on
Management and list; Operator cannot target Director or disable privileged self;
active Task blocks existing Human stock runtime review. Existing Task
create_and_assign/start/submit/accept and legacy query/send pass through stock.
Every successful Management response equals its persisted action response.
Close records roster retired_at; an additional read-only `jq -e` oracle verifies
durable lifecycle=retired, retiredAt present, native ID/formal name retained and
no runtime binding. Offline retains explicit_launch and native
identity, clears runtime binding; owned stock PID is absent/zombie and a separate
owned subprocess remains alive. All owned children are cleaned up by the harness.

The successful stop uses the EXISTING SystemctlUnavailable direct-process
fallback: a private executable allowlist omits systemctl/systemd-run. No mocked
scope success, production user bus, or new stop policy. This does not prove
cgroup/escaped-descendant containment. `observation.active` means durable Agent
active, not runtime Online; offline correctly retains active identity.

Units cover all native request/result families and provider serialization digest,
safe uncertain/owner-action outcomes, requiredness/forgery, historical imported
and moved provenance, and missing/ambiguous list mappings without title/name/profile
inference. Timeout/5xx or rejected receipt is not a rollback/no-write guarantee.

## Checks and retained failures

All commands used scrubbed environment and owned Mambo HOME/TMPDIR/Cargo caches.
`cargo test --locked --lib agent_bus::mcp -- --test-threads=1`: **16 pass** final.
Three separately selected provider tests: **1 pass each**:

- `operator_authority_denies_impersonation_privileged_targets_and_control_planes`
- `director_rotation_preflight_rejects_missing_or_stale_task_service_seat_before_launch`
- `completed_project_director_rotation_replay_ignores_unrelated_legacy_seat_rebind`

Thus **19 distinct final selected Rust tests**, not a full-suite claim. Earlier
overlapping MCP runs selected 11/15/16/16; total Rust passes across all these runs
and the final selections are 77, not 77 distinct tests. Default-feature
`cargo build --locked --bins` passes; existing eight bin warnings remain.
`cargo fmt --all -- --check` and `git diff --check` pass. Complete scoped source
diff reviewed, including HTTP authorization order, no root route, semantic digest,
scope refusal, historical receipts, error redaction, and fixture-only process cleanup.

Evidence under `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`:
`s5b04/{PASS,operations,discovered-tools,management-state-summary}.json`, new
`bundle.json`, `s5b-tests-final.log`, three named provider logs, `s5b-build5.log`.
Reproduce with `python3 tests/stock_management_mcp_boundary.py NEW_OWNED_DIR`
after the isolated default bins build; harness refuses an existing evidence dir.

Failures preserved: `s5b01` committed query was rejected by old native recursive
project check (fixed with exact current typed historical-record recognition);
`s5b02` reached 29 calls then offline returned honest owner_action_required because
systemctl could not connect without a user bus environment. No successful scope
stop inferred. `s5b03` passed 34 calls; final `s5b04` repeats that boundary with
final diagnostics/timeout and independent owned-process observation. Across all
four runs: 98 overlapping tool calls /306 fake Responses, not unique scenarios.

Final facade SHA256 `9e57c188264928c08e9391b242ae54b5e42a97de96e33493895dab4649af9dc6`;
new reviewed `s5b04/bundle.json` SHA256
`8619f520c973c8773e4b48e6a495a175750f0023aba03749f2793314e724fe5d`.
It pins this facade through unchanged explicit review/activation; no old manifest
or artifact bytes were edited. Stock remains 0.153.4/U
`3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`, executable SHA256
`56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da`, package
`a822187e1a2420c61c5926721bfbd878701ed95547c9bb0d4de4498a16ba1821`.

## Limits / next use

Keep S4/S5a private candidate-writers-only restriction, seconds-truncated PID-time
risk, two measured baseline Management socket-length failures, and unresolved
S2 credential exposure / s46 default-endpoint incident disclosures. No investigation,
remediation, no-impact, or production safety claim is added here. Reuse unchanged
S4 launch/PTY/approval/sandbox evidence; do not claim it rerun. Real cgroups, Windows,
OAuth/real accounts, inbound/A4/after-turn/notification wake, Android, autonomous
full-tools operation, and successful stock create/replace/rotation remain unproven.

Resources before: 7.8 GiB retained /452 GiB free; final: **8.2 GiB retained /451 GiB
free**, below 20 GiB retained and above 100 GiB free. No new home
cache, kernel build, live deployment, canonical refs, or unrelated cleanup.
Recommendation: PRIVATE INTEGRATION_ALLOWED only after Director acceptance.
Next feature requires a separately accepted stock bootstrap or inbound contract;
do not reinterpret this outbound translation as either. Rollback: reject/revert
candidate before exposure; all probes are private and shut down.
