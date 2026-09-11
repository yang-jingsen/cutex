# Selected migration: history quarantine and reviewed profile gaps

Status: decision-ready READ-ONLY input; no migration or product implementation.
Base: `7f85283116438eb2988bb34fcb6836817168b1c4`, tree `18fa76a119bd2e2a97c3c4806415ae08d8d920b1`.
Selection and exact durable/native/path/offset anchors remain in `project-recent14-candidates.json` and `project-recent14-migration-candidates.md` at that commit. The fixed inclusive window is 2026-08-28T18:48:15Z–2026-09-11T18:48:15Z (Sydney: Aug 29 04:48:15–Sep 12 04:48:15 AEST). This report does not replace that immutable snapshot.

## Decision

Keep the confirmed 34-person cohort (18 online, 16 offline) intact. Do not migrate yet: the current real reviewed adapter accepts only the exact aemeath profile and **gpt-5.6-terra/low**; none of the 34 effective model/effort pairs matches. There are also permission-alias and configuration projection gaps. Octobre uses the same ChatGPT file-auth protocol shape, not a newly required OAuth implementation; its distinct profile/account must remain distinct. GLM needs a bounded explicit API-key provider mode. Deferring the two octobre and one GLM subjects is a Human scope choice, not an automatic exclusion, and would not fix the remaining 31 subjects' model/config gaps.

## Five damaged histories: activity is proven, migration is not

Only structural metadata was inspected; no conversation, reasoning, tool parameters or credentials are reproduced here.

| Formal name | Last valid actual activity (Sydney AEST) | Structural damage | Classification |
|---|---|---|---|
| cesc-current-direction-r1 | Sep 6 21:08:41.150 | 19 malformed mid-file lines; 14 invalid-control-character errors, 5 other JSON syntax errors | Eligible activity; quarantine |
| cesc-director-r1 | Sep 9 08:18:23.748 | 24 malformed mid-file lines; 21 invalid-control-character errors, 3 other syntax errors | Eligible activity; quarantine |
| cutex-director-r11 | Sep 4 01:05:19.511 | Line 42935, byte offset 89610613: 1675 NUL bytes plus LF | Eligible activity but retired; never reactivate implicitly |
| vcc-director-r1 | Sep 5 15:25:38.799 | Line 5109, offset 14792463: 1927 NUL bytes plus LF | Eligible activity; quarantine |
| vce-director-r1 | Sep 7 15:26:06.465 | Line 2925, offset 10949258: 3446 NUL bytes plus LF | Eligible activity; quarantine |

The first malformed anchors for the two CESC histories are line 77771/offset 78184135 and line 144612/offset 177434306 respectively; the original sidecar contains all anchors. Last valid actual message lines are respectively 102316, 190180, 43650, 16035 and 13062. All are within the fixed window. Thus the previous five “unknown” activity classifications are corrected: four active subjects have positive activity evidence but remain outside the confirmed 34 pending a decision; the fifth remains retired. The native owner's 34-history proof scope is unchanged and this distinction was communicated directly.

No invalid UTF-8 or salvageable concatenated complete JSON objects were found in the bad lines. The CESC fragments begin as JSON objects but cannot be decoded as complete objects; the NUL spans contain no object. Valid records follow the damage, so this is not merely an incomplete current EOF write. These shapes do not establish the originating fault (crash, overwrite, storage fault, etc.). Missing content cannot be reconstructed from this evidence.

Native source `b8e9cc3a1bc6e88a1d9bd7454e886882f58b9fb8`, `codex-rs/rollout/src/recorder.rs:1032–1092`, counts parse errors, skips failed records and returns history while `get_rollout_history` ignores the count. A successful resume therefore does **not** establish lossless history compatibility. Do not run its raw-line error logger against sensitive data for reporting.

Correction proposal only: retain original bytes and anchors; separately authorize a copied-history salvage candidate with an explicit omitted-record/loss manifest and native reconstruction checks. Do not repair originals, guess braces/content, or certify skipped messages/tool results/compactions as irrelevant. Until that loss decision, quarantine is the supported disposition.

## Effective profile inventory

Read-only sources: formal profile IDs from accounts metadata, each selected durable record, selected profile TOML, shared native TOML and global default-profile setting. Auth inspection was limited to mode and presence booleans, never values. All 34 have explicit durable model and effort, explicit approval `never`, and no extra CLI arguments. Four inherited profiles resolve to the actual global default aemeath, not a name/cwd guess.

| Profile | Subjects | Auth/provider structure | Material configuration |
|---|---:|---|---|
| aemeath (`cd6a39eb-3997-45c6-9824-5113fe36a4b8`) | 31, including 4 inherited | ChatGPT tokens present, file storage; no custom provider table; API-key nonempty=false | Profile default astra/medium is superseded by each durable override; configured `cutex_job`, skills, TUI status line/colors, shell environment exclusions, project settings, reviewer=user, service tier=default |
| octobre (`b341adf8-7af9-432c-a12b-0e9a674458ed`) | 2 | ChatGPT tokens present, file storage; no custom provider table; API-key nonempty=false | Model/provider/effort absent in profile, but both durable records explicitly sol/max; skills, TUI, shell exclusions, projects; no profile MCP |
| GLM (`3f38c782-bac6-403d-b11d-c801326e0bb1`) | 1 | API-key nonempty=true; tokens object absent; Responses provider, requires_openai_auth=false; `https://www.colabapi.com/v1`, env-key field present | Durable **glm-5.3/max**, not profile default glm-5.3-flash; memories generate=false/use=true, compaction limit 256000, custom model catalog path, plugin entry sample@debug, skills/TUI/shell exclusions/projects |

Octobre subjects: `cute-codex-log-wal-fix-r2`, `cutex-r37-release-deploy-r1`; GLM: `tethys-director-r2`; all three were offline in the selection. Identical auth protocol is not identical account identity. GLM key presence is not proof of valid credentials, forwarding or remote model support; no endpoint requests were made. Plugin entry presence is not proof it is enabled or compatible.

Effective model/effort counts: sol/medium 5, sol/high 8, sol/xhigh 5, sol/max 7; astra/low 3, astra/medium 1, astra/high 3; terra/medium 1; glm-5.3/max 1. Total 34.

Permissions: 33 effective danger-full-access/never; `cesc-tutor-r1` effective read-only/never. Existing strings include `danger-full-access`, `:danger-full-access`, `:read-only`. Preserve effective semantics and durable records, not the private terra-low/readOnly/on-request smoke settings. Shared native config currently has no MCP table; it does contain safety, skills/TUI and model defaults. Shared defaults must not override these explicit durable selections.

## Exact implementation boundary for a subsequent task

Current source anchors: `src/launch/aemeath_auth.rs:11–33` restricts profile ID/name/provider and terra/low; `src/launch/stock.rs:430` declares the deny-unknown profile projection, `:601` checks real identity, `:631–644` checks permission aliases, `:647–661` resolves model/effort and excludes max. `src/runtime/args.rs:197–233` already resolves the legacy permission aliases. Native `b8e9cc…:codex-rs/protocol/src/openai_models.rs:50` supports Max and arbitrary nonempty effort values; this establishes parser support, **not** every provider/model's catalog eligibility.

Recommended one bounded versioned reviewed projection:

1. Generalize the existing reviewed ChatGPT-file route to explicitly reviewed profile ID/account custody for aemeath and octobre, without relabeling accounts or implementing OAuth. Bind the exact effective model/effort using authoritative catalog support; no terra fallback. Preserve same-account native refresh and detect account replacement using the existing custody model, never token values in digests/logs.
2. Add one explicit GLM Responses/API-key route binding this provider endpoint, native supported credential mechanism and selected profile. Verify env-key versus file-auth materialization before claiming support. No generic endpoint/header/config passthrough. If it cannot preserve the actual account/model contract, return that specific gap.
3. Translate legacy permission aliases through the existing effective resolver into canonical launch arguments, including unchanged explicit `never`; retain sandbox and approval independently. Extend reviewed effort validation for the actually supported selected values, including max. Do not rewrite session identity or lower effort to fit the old test mode.
4. Explicitly project safety settings (shell exclusions, reviewer, trust/project rules), memory use/generation, compaction/catalog and required tool capabilities. Current generic unknown-field rejection remains valuable: add only typed reviewed fields. The old profile Job MCP must become the reviewed coherent Job descriptor, not arbitrary raw MCP forwarding or a silently removed tool. Skills configuration and availability require a supported native-home mapping; copying only TOML does not prove skills are available. Custom catalog, plugin enablement/availability and project rules need exact native support checks before activation; retain-and-ignore is not equivalent behavior.
5. TUI status line/colors and notices are presentation/experience settings, not authentication authority. They can use a separate explicit supported projection or documented retained-but-inactive choice; do not present retained source files as effective settings. Likewise plugin data remains retained but unproven, not implicitly active. No plugin engine or broad profile import is needed.
6. Review must show the effective projection and any unsupported field, pin profile/config/custody/bundle/Job provenance, and preserve runtime-review v2, expiry, semantic replay/conflict, claim and immediate true-state checks. New projection version must not reinterpret old reviews or silently drop fields under old writers. Inherited profile stays inherited and is freshly resolved through the authoritative record/default snapshot; profile is never identity.

This is a release projection change, not just wording or a three-profile label allowlist. Source files can remain retained read-only, but unsupported safety/memory/tool settings block equivalence until supported or explicitly chosen otherwise. No per-setting Human questionnaire is needed before implementing and testing this bounded projection contract.

## Activation and release sequencing

Native copied-history validation for the confirmed 34 is independent and still required; new-thread bootstrap success does not prove same-history resume. Preserve durable/native IDs, project membership, formal names, profiles and inactive state. Current old owners must be stopped only under their existing role/Task/authority coordination, then offline/no-active-claim CAS review can install a new explicit requirement. No live marker/history rewriting, dual writers or K fallback. Current Director and working agents are coordinated last; offline subjects remain offline after migration unless separately authorized.

The previously agreed one-time backup has not been captured (see `linux-final-cutover-readiness-inventory.md`); this task captured none. New Bus/Job/history cannot be handed back to old writers as rollback. A baseline restore would lose subsequent activity and requires an explicit decision.

Verification: read-only structural scans and exact source inspection only, not model/provider acceptance, runtime tests, builds or migration. Original histories, profiles, credentials, VM fixtures and running agents were not changed. No auth copied, no snapshot taken. The smallest next implementation is the reviewed projection above plus focused negative/CAS tests, followed by separately authorized private preserved-history proof and coordinated cutover preparation. The four additional damaged active histories need a separate cohort/loss decision; they are not silently added or discarded.
