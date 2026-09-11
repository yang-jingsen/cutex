# jv02 CodeMode entrance: bounded read-only decision

No third run, new provider request, runtime, build or product edit. Product pin
repair b48a2f63 and its frozen default manifest remain unchanged. This is
decision evidence, not completed composition acceptance.

## Exact cause

Preserved jv02 review selected `unknown-private-model`, no reasoning override,
fake loopback provider, full-access/on-request. Actual first request advertised
ordinary direct tools and `mcp__cutex_job` (cancel/query/read_output/submit), but
no custom `exec`. Nevertheless the fake provider emitted custom `exec`, then a
false `Submitted` reply. Native correctly persisted `unsupported custom tool
call: exec`. No Job or approval occurred.

Exact native b8e9cc3a source establishes why, without a catalog network call:

- `codex-rs/models-manager/src/model_info.rs:186`: unknown-model fallback has
  `tool_mode: None`.
- `codex-rs/features/src/lib.rs:989`: CodeMode defaults false; CodeModeHost
  defaults true separately. Having the companion does not enable the tool.
- `codex-rs/core/src/tools/mod.rs:68`: explicit model tool_mode wins; otherwise
  CodeModeOnly/CodeMode feature flags determine mode, else Direct.
- `codex-rs/models-manager/models.json`: exact `gpt-5.6-terra` entry has
  `tool_mode: code_mode_only` and supports low. This is the bundled catalog,
  not an inferred model alias or remote provider claim.

The passed native `artifacts/display-polish-v2-r1/polish_probe.py:6` explicitly
enabled `[features] code_mode=true` and `executed_tool_call_metadata=true`.
jv02 did neither. That earlier fixture used fake Job receipts, so it establishes
the CodeMode/PTY entrance, not real Job execution. Its metadata flag is a
separate recorder setting (native `core/src/session/session.rs:1396` and
`tools/parallel.rs:99`); do not silently add it or infer all display evidence
from enabling CodeMode alone.

## Recommended minimal continuation

Use the supported existing reviewed profile fields in a NEW fake-only fixture:

```toml
model = "gpt-5.6-terra"
model_reasoning_effort = "low"
model_provider = "s6-private"
```

Keep the existing dummy loopback provider unchanged, no authentication or paid
route. This needs no product flag: Cutex `src/launch/stock.rs` ProfileConfig and
StockConfiguration already bind model/reasoning/provider to the review, and
`src/cli_app/stock_lifecycle.rs:96` forwards these exact fields. Fresh review is
required; do not alter any previous review or marker. The fixture correction is
prepared, not run. Before returning a fake call, assert that its exact tool kind,
namespace and name occur in the actual request. Do not accept `Submitted`
without the actual single Job record/receipt.

Native-only alternative flag is `-c features.code_mode=true`, but the current
Cutex shared/profile allowlists reject arbitrary features and durable CLI args.
It is NOT a supported Cutex command to append today, and is unnecessary for the
catalog-selected model above. No generic allowlist extension is recommended.

If Director instead chooses a direct-MCP-only proof, the already advertised
path requires no CodeMode:

```json
{"type":"function_call","namespace":"mcp__cutex_job","name":"submit","call_id":"one-private-submit","arguments":"{...actual actionId, argv, cwd...}"}
```

That uses the same real Core metadata/approval/Job adapter, not manual grants,
but does not satisfy the specifically requested nested CodeMode criterion.
Prefer the catalog-selected fixture correction and retain that criterion.
After authorization, the next executable entrance is the unchanged private
bwrap command from r2 with a NEW run name (e.g. jv03); no command was executed
as part of this diagnosis. Use the initiating CLI for interactive input and
validate normal approval routing, not a fabricated approval result.

## Offline verification and limits

`tests/job_view_v3_tool_contract.py` reads only preserved jv02 tool declarations.
Three negative cases pass (absent exec, unknown MCP tool, missing arguments);
advertised direct submit shape passes, and a synthetic advertised custom exec
shape passes. This is a bounded shape check, not a complete JSON Schema engine
or runtime execution proof. Prepared composition transform/syntax check passes
without executing its setup or body. Both checks create no service or request.
The fake provider now checks advertised shape before emitting a call and
requires actual Job state before `Submitted`.

Full CodeMode/Job/A4/view/PTY/replay acceptance remains unrun. No claim of
success from catalog source alone. jv01/jv02 failures, original immutable
reports and histories are untouched; VM/Human fixtures and real credentials
remain untouched. Recommended decision: authorize one corrected private
continuation under the existing containment, not a product capability expansion.
