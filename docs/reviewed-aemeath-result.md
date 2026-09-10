# Reviewed aemeath real-provider result — partial / blocked

2026-09-11. Base a175e9edbddfd0264b4d400336580587ebfa7411,
tree5712e2f9d1729b770ffd6efc665a0c43b1853881. Production source
c69241c11b2ad99c7b0f2fcabfe91a67107d3dee,
tree76aa102d90f2b60686a51f80a93fec40906782f0. Subsequent commits only
preserve the harness and this result. Private candidate; NOT full acceptance,
deployment, profile migration or PRH cutover.

## Exact bytes and scope

Default Cutex908ab03660b3736efd49842bdf896c2b369021e8f52fb99bdde52682709cc7cb;
facade3b7b3736c401956e3cda55b6346f8416da968bbe32bafd7ec312b8b1fc87ea71.
Frozen manifest: `artifacts/reviewed-aemeath-r1/build-manifest-v2.json` under
the task root, with native0c425/CLI/helper/host/schema and Jobf3bc9c8 unchanged.
No native/Job/TUI changes, dependency/lock churn or host mutation.

Contract: [reviewed-aemeath-provider.md](reviewed-aemeath-provider.md).
Explicit real mode only; old absent mode remains fake. Native account/user
metadata and stable private file custody are reviewed, not mutable token bytes.
Normal same-account native refresh is compatible; replacement/account/user,
profile/config/provider/model or capability changes require fresh review.
No pending review upgrade or old-writer compatibility claim. Generic MCP stays
denied; only the previously reviewed dedicated Job descriptor is injected.

## Actual private evidence

Fixture roots: guest `acceptance-upload/ar1/{h,r1,r2}`; no live identities.
Source/auth profile read-only; minimal explicit guest projection excludes
unrelated profile MCP, skills/TUI/project/reviewer/service-tier settings.

| Criterion | Observation |
|---|---|
| Exact model mapping | Actual native authenticated model/list: gpt-5.6-terra supports low; no model turn for catalog |
| Reviewed real-auth launch | Final default bytes reached Ready through actual private root review/activation/runtime APIs |
| Reviewed Management create | Actual scoped Director request with root-reviewed intent completed; neutral history had zero turns; same resulting ID resumed |
| Real provider | Exact gpt-5.6-terra / low returned TERRA_LOW_READY; no fake response or proxy |
| Configured Job inventory | Actual native mcpServerStatus/list included cutex_job (harness assertion) |
| Real model Job call | NOT PASSED: second response said it could not access cutex_job; no tool invocation and zero Jobs |
| Job output / completion A4 | Not reached, not inferred from server discovery or prior fake evidence |
| ReadOnly/approval | Receiver configured read-only/on-request; actual Job approval/path execution not reached this run; earlier fake/Core readOnly evidence reused only at its original level |
| Cleanup | Exact copied guest auth files absent, no host auth writes/syncback; both recorded runtime PIDs no longer had executables |

Two native usage updates: cumulative input27554, cached input18944, output97
(reasoning67), total27651. No monetary billing information or exact internal
HTTP retry count is available. No third turn/fallback/GLM call was attempted.
Native builtin provider rejects overrides, including retry settings; retained
native retry defaults are explicitly disclosed, not claimed disabled.

The missing Job call is an observed model response, NOT proof that the native
tool capability is absent. Actual tool visibility/namespace selection versus
model behavior remains unresolved. The finite observer expired waiting for a
Job that was never created (`Empty`); no wake, polling turn, manual Core meta,
grant fabrication or duplicate-create retry was used to manufacture success.

## Failures preserved

Initial default1a867e6 launch failed before model turns: native refused a
reserved builtin openai provider override. Corrected to native builtin routing,
with no endpoint override; original bytes/state remain in bin/r1. Initial
compile error converting a digest error was fixed; its log remains retained.
Final r2 real-provider observation is preserved separately; no claim of full
smoke success. Local evidence directory contains catalog, both observations and
credential cleanup receipts without auth contents.

## Checks / self-review

Final focused commands: `cargo test --locked --lib aemeath` (5), `stock` (11),
`job_mcp` (5), `bootstrap` (8):29 executions/28 distinct, one overlapping test.
Default `cargo build --locked --bin cutex --bin cutex-mcp`, fmt check and diff
check pass; prior default bins check passed before the small builtin fix.
Known pre-existing unused/dead-code warnings remain. Earlier run28 executions
is not added to this final count. Complete scoped source diff reviewed:
auth helper, profile selection, optional review field, bootstrap/runtime auth
fences, native launch args, tests, harness and docs. No authority/receipt/CAS
weakening or arbitrary MCP/provider passthrough.

Tests cover refresh stability, file/account/user replacement, invalid auth,
unsupported route/mode/model/profile/provider, public/symlink refusal and
unknown MCP. Existing runtime-v2/legacy replay/bootstrap/Job tests retained.
No claim of live refresh-token expiry exercise, hostile same-UID isolation,
Windows, full suite, general OAuth/multi-account or production migration.

Resource observation: task root13GiB / Mambo376GiB free; guest upload5.5GiB /
50GiB free. Existing artifacts retained; no broad cleanup. Prior S2/S46/PID/
socket risks unchanged and not remediated or investigated here.

## Narrow next decision

Keep this source as a partial integration candidate. Resolve real-model Job
namespace/tool visibility using exact native schema/inventory and the two
preserved turns before authorizing another bounded generation; no new auth
scheme or provider fallback is justified. Then complete actual Job/output/A4.
Host rollout, PRH Job ownership/replacement and full-profile migration remain
separate gates. Reject/revert candidate before exposure; do not downgrade or
rewrite these experimental private records with old writers.
