# Selected profile projection R2 — interim candidate

Status: independent implementation and private component verification complete; full selected-profile preservation BLOCKED on accepted custom-status wiring. Not migration or real-provider acceptance.

## Immutable inputs and output

- Base: `11b511cd7baa49afa3fa2122806e139101f90592`.
- Product checkpoint: `6c6b72827810efac2b72561e8eb72a582ac410e5`, tree `28f238aec766a18c3bd654a61c0ee2388dc3733a`.
- Default-feature dev artifacts: `../artifacts/selected-projection-r2/build-manifest.json` (relative to checkout), SHA256 `d244d49b2b8e80e44c8b9236a0e3896948d06ba90d6aa8dbc49f3eb3518d23c2`.
- Cutex SHA256 `8e4594b1c51bdd25df159cf3ed2d4195f64b72fcaf32a2db80af01beee875afa`; facade `480070e89a02849e22bd1c4a82d9cd56b9e1d463b7053ed787fc0301053f3f2f`.
- Accepted native `2eab060b191a0fe59e22b28785d02d76eafb7fc4`; independent-auth manifest SHA256 `6392a1e686be1940aa51d2a944e89300d6fde852c988f45684cf8b439ed5678c`. Exact CLI/server/host/schema pins are in the composed manifest and checked against frozen bytes.
- Job remains `f7bbe3c42fbf30bfb5fe379c6f6cd5af951991b9`, ELF `ba1a8d4f3e0b5f739e666e3f515d40b0c543e9e75953759ff181f1e71b29c521`.

## Implemented boundary

Explicit selected-profile v2 freezes typed configuration, route, model/effort, custody and catalog. Aemeath and octobre keep distinct profile/account identities with native owner-only `--auth-file`; remote attach omits this flag. Safe same-account atomic auth-file replacement is supported without freezing token bytes or the file inode forever. Parent custody/account substitution still fails closed. GLM uses the exact configured Responses route and a non-Debug per-child API key mechanism, not argv or serialized configuration secrets.

Effective durable overrides/inheritance, permission aliases and approval policy remain separate. Typed safety, memory, compaction, catalog, disabled skills, trust, plugin and supported TUI settings are retained. Raw MCP forwarding remains prohibited; the legacy Job declaration requires the separately reviewed coherent descriptor. Unknown fields, missing assets and unsupported effective settings reject. Existing reviews retain their original interpretation and completed replay bytes; new projection fields are versioned and fail closed for incompatible writers.

| Selected group | Implemented component | Remaining preservation boundary |
|---|---|---|
| 31 aemeath, including 4 inherited | File-account route, effective models/efforts including catalog-supported max, typed settings | Custom status; actual auth ancestors currently fail custody; no live materialization |
| 2 octobre | Same protocol, distinct account/profile, no relabeling | Same boundaries |
| 1 GLM | Exact glm-5.3/max catalog and fixed Responses/API-key route | Custom status; synthetic-only auth verification; enabled but missing sample@debug retained as missing, not installed |

The explicit mode/config projection is not a live migration materializer. Original profiles are untouched and do not automatically opt in. The 34 coherent subjects are not declared ready; four damaged additional histories remain quarantined. Offline subjects remain offline.

## Verification

- Default `cargo check --offline --bins` and final `cargo build --offline --bins`: pass. Dev artifacts, not release optimization.
- Selected-profile unit tests: 5 pass. Stock tests: 8 pass, 1 separately gated manifest test; that manifest test was explicitly run and passed.
- Runtime-review tests: 3 pass, including legacy/v2 and changed-projection replay conflict.
- Default executable ingress guards: 2 pass, including pre-stop rejection.
- Scoped rustfmt, diff whitespace checks and full changed-source self-review: pass. Existing warnings and an unused compatibility spawn-wrapper warning remain.
- Actual isolated private test: `tests/selected_profile_private.py` / `.rs`, final root `../selected-r2-yjdr0356`, result log SHA256 `9313b2652f00a71baf1d18fd3cb451c7c9af5be80677a8c9f718f3f32e174c48`, 1 pass in 11.14s. Earlier private successful attempts retained.

The private test used actual accepted native processes with two dummy account files and the same native home, actual account/read, production argument construction, inherited/explicit profile resolution, same-account inode replacement, foreign-account/path/permission negatives, and catalog validation. A separate child sent a dummy bearer to a loopback listener through the secret-spawn mechanism. This is NOT a native GLM HTTPS/provider acceptance test. No turn/start or paid model request occurred. Native refresh protocol proof is reused from the accepted native evidence, not inferred from synthetic file replacement.

Full root-review/bootstrap/registration/Job MCP composed flow, all 34 original profile execution, custom-status rendering, live history rehearsal, Windows and full workspace tests were not run. No runtime failures were hidden; source-inspection path errors were corrected without runtime retries. Owned test children exited; synthetic evidence remains. No real auth was read/copied or host settings changed. Root usage was 18 GiB with 335 GiB free at final check.

## Exact next dependency

Native custom-status candidate `8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6` is review-ready, NOT yet Director-accepted. Its `artifacts/custom-status-r1/INTERFACE.md` specifies a TUI-only `--status-items-file`, version 1, two fixed IDs, bounded static text/styles. Native explicitly requests retaining rejection until acceptance and separate wiring authorization. This checkpoint therefore still rejects `custom:profile` and `custom:bon-voyage`; it does not silently discard them or activate speculative CLI bytes.

Next task: accept the exact native candidate, authorize the bounded reviewed status-file wiring and exact pin refresh, then perform preserved-profile private composition. Real auth ancestor custody preparation is a separate explicit operation: current 0775 ancestors are not changed here. No snapshot, migration, rollout or downgrade is authorized by this report. Old writer/history incompatibilities and earlier S2/S46/pj07 disclosures remain unchanged and unremediated.
