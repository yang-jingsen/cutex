# Selected reviewed projection: consolidated implementation boundary

Task `cutex-selected-profile-reviewed-projection-r1`; base `aa7820714a6a2ed2ffe70d6e3f2ccb20c400d423`, tree `621fffdec5d986acedcf6dd4dcdb6bd1c35bf806`. **BLOCKED before product edits**, not an implemented integration candidate. The assessment remains valid, with the additional exact findings below.

## Required decision

The authorized implementation retains distinct ChatGPT file-auth custody and native refresh, one authoritative native history home, and all original effective settings. The fixed native has no demonstrated independent file-auth path: `get_auth_file(codex_home)` returns `codex_home/auth.json`. Current Cutex likewise requires the reviewed auth path to equal that path. Therefore merely allowing the octobre profile ID would review whichever account occupies the shared home, not preserve each selected profile's authentication. Copying/replacing that file per launch, creating separate/symlinked history homes, or treating account labels as equivalent is not authorized.

Recommend a narrowly scoped native-owner decision for an explicit owner-bound file-auth storage location independent of history home, preserving native refresh and account checks. No new OAuth implementation is needed. Native owner has been asked for an existing supported alternative, not commissioned to change code. If that existing path is demonstrated, use it instead; do not infer one from a config field or ignored environment variable.

The fixed server does have `account/login/start` with `chatgptAuthTokens`, but it installs `ExternalAuthBridge`. On unauthorized responses that bridge requests refreshed tokens from the client with a ten-second deadline. It is **not** the same file-auth/native-refresh contract. Choosing that alternative requires an explicit refresh ownership/custody design decision, not silently implementing a token supplier in Cutex. This report does not claim all native authentication is impossible.

Source evidence at server `b8e9cc3a1bc6e88a1d9bd7454e886882f58b9fb8`:

- `codex-rs/login/src/auth/storage.rs:154`: `get_auth_file` joins `auth.json` to home; file storage is based on this location.
- `codex-rs/app-server/src/request_processors/account_processor.rs:826–879`: external login installs an external auth bridge, not a separately selected auth file.
- `codex-rs/app-server/src/external_auth.rs:18–84`: client refresh request, timeout and token response handling.
- Cutex `src/launch/stock.rs:409–425,601–610`: existing review/home validation; `src/cli_app/stock_lifecycle.rs:82–123`: CODEX_HOME and file-store launch projection.

No auth values or account identities were read for this task; the earlier assessment's presence booleans are reused. No dummy process test would resolve this missing storage-selection API, so no success-shaped authentication simulation was added.

## Finite configuration findings, not another broad audit

| Setting | Exact finding | Consequence |
|---|---|---|
| aemeath/octobre model/effort | Fixed server `models-manager/models.json` advertises astra, sol and terra with low/medium/high/xhigh/max/ultra | All selected ChatGPT model/effort pairs have bundled catalog support. Remove the old test-only restriction in a new reviewed version once custody is resolved; not a remote account-entitlement claim |
| GLM model/effort | Configured local catalog exists, is not a symlink; models include glm-5.3 and glm-5.3-flash with low/high/max | Preserve durable glm-5.3/max, not profile default flash. Catalog bytes and custody must be reviewed; no real provider test performed |
| GLM API-key transport | Provider `env_key` is read by native `ModelProviderInfo::api_key`; custom requires_openai_auth=false returns unauthenticated if no provider bearer is present | File key presence alone is insufficient. A fixed per-child key environment materialization is the source-supported candidate; do not flip requires_openai_auth or place token in argv/config/review |
| GLM plugin | `plugins."sample@debug".enabled=true` | It is active configuration, not a disabled entry. Asset/marketplace resolution and its capabilities remain unproven; cannot silently omit it |
| Disabled skills | aemeath has 3 explicit disabled paths, octobre 1, GLM 2 | Preserve exclusions; do not require enabling or copying disabled skill bodies. Other discovered skills still need the shared native-home availability contract |
| Status line (all three) | `custom:bon-voyage`, `custom:profile`, model-with-reasoning, current-dir, context-used, weekly-limit; colors=true | Fixed CLI enum lacks both custom entries; parser drops unrecognized entries. Passing the array unchanged is not preserving the visible experience |
| Shell exclusions | CODEX_AUTH_FILE and CODEX_CONFIG_FILE excluded on all three | Typed exclusion projection is possible; these names do not establish native support for either auth/config alias |
| Notices | aemeath hide_rate_limit_model_nudge=true | Native typed field exists; no generic notice passthrough needed |
| GLM memory | generate_memories=false/use_memories=true; compaction 256000 | Preserve exact typed settings and referenced assets; not inferred duration/state or a reason to disable memory |
| Permissions | Existing resolver yields 33 full-access/never and one read-only/never | Canonical alias translation is a bounded Cutex change. Do not substitute the previous smoke's readOnly/on-request |

Additional source anchors:

- Fixed CLI `5fc4719`, `codex-rs/tui/src/bottom_pane/status_line_setup.rs:56–165,296,342,352`: finite enum and failed-parse filtering. No custom item provider was found in this surface. Native owner was asked to confirm any existing extension.
- Server `codex-rs/config/src/types.rs:778,868`: status-line colors and hide-rate-limit notice exist.
- Server `codex-rs/model-provider-info/src/lib.rs:337–353` and `codex-rs/model-provider/src/auth.rs:197–210,292–305`: provider key resolution and unauthenticated fallback. The future launcher must fail closed before spawn when the key cannot be safely materialized.
- Server `codex-rs/models-manager/models.json`: exact bundled model/effort evidence, not a remote query. GLM catalog inspection emitted only model IDs/effort metadata, not prompt bodies or model instructions.

The TUI issue is separate from authentication: authorize a bounded native status-item mapping/restoration, or explicitly accept a presentation-only difference. The standing requirement to retain original settings does not authorize dropping the custom items. Do not build a generic custom script/plugin engine to satisfy these two names. Enabled GLM plugin resolution must be completed before promising tool preservation; this report does not assert the plugin is absent merely because a narrow filename search found no manifest.

## Implementation continuation after the boundary is selected

Keep the proposed versioned typed projection and existing old-review behavior; no loosening of old aemeath mode. Implement model catalog validation, canonical permissions, typed safety/memory/TUI/skills and coherent reviewed Job replacement together with the chosen auth custody path, so the review describes the actual effective launch. Use optional new versioned fields with unknown-version rejection and old-writer rejection, not a profile-name allowlist pretending account isolation. Bind asset references and exact config digest in the existing review/claim/CAS path. GLM secret values must be supplied only at spawn via a non-debug/non-shell-rendered secret-bearing mechanism, not the currently Debug/to_shell_command-capable public LaunchCommand plan.

The decisive future private tests remain: two distinct dummy ChatGPT accounts sharing one history home without shared-file replacement; native-refresh/custody/substitution refusal; GLM dummy bearer reaching only an isolated loopback oracle through the selected fixed-route test boundary; catalog/unknown-field/asset failures; exact permissions and review conflicts. No real account calls are needed to prove those local boundaries. Real provider acceptance remains separate.

No partial product code was committed because relaxing identity/model guards before choosing the actual credential storage route would not produce the required safe usable projection. No new schema, pins, binaries or markers were introduced. Four damaged active histories remain quarantined; the confirmed 34-history native task is unchanged.

## Verification and custody

Performed: clean-base check, exact immutable source reads, bounded profile structural TOML inspection, bundled/custom catalog metadata inspection, scoped report review and diff check. Failed/omitted: no implementation, compilation, process/auth/review oracle, artifact build, deployment or model tests; no runtime equivalence claimed. One initial source read used the task-root rather than source subdirectory and returned file-not-found; corrected read-only, no runtime attempt.

Resources before work: task root approximately 18 GiB; filesystem free 345 GiB, within 24 GiB/100 GiB limits. Added only this small report, no caches/build outputs. No credentials copied/read into diagnostics, profile writes, host/VM/service mutations, snapshots or history changes. Pending decision is a precise native custody/experience boundary, not approval to rerun paid smoke or downgrade selected profiles.
