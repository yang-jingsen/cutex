# Linux final cutover: read-only inventory, 2026-09-12

**Not deploy-ready merely after visual polish.** Remaining work is a bounded
release/cutover contract, not a new feature backlog. No backup, build, VM access,
provider call, lifecycle action, config write or cleanup performed here.
Report base340886d96af690341f2b40646741344b42d49da9; productb48a2f63 / tree9fe31ce6,
manifest e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d.
Pending native118c polish is not a frozen accepted bundle and was not built/pinned.
Human pv3 evidence (completed/action/exit0/1.048s/stdout19B) is positive partial
evidence, not independently verified final read/A4/ACK for that Human Job.

## Current observed facts

| Surface | Read-only current observation |
| --- | --- |
| Public Cutex shortcut | `/home/senxiu/Resources/Shortcuts/cutex` resolves release-fence-r2/artifacts/linux/cutex, SHA76dd20efa19ef9d6446c2ff9b1a2c4b9e2752d2158c79767eb7933b179f22c58; manifest source80da86a5/tree353af9b5 |
| Management | systemd user service active, PID2165961, mapped exact same76dd20 bytes |
| Bus | systemd user service active, PID1328036, retained release-r2-env-forwarding Cutex090e5fe73c0c7920b60cd8efbd27eb2be7886201b3f3ee6409e22b7a0e7a3f47; source2e0428fe |
| Public native | shortcut resolves release-r2-env-forwarding cute-codex; SHA d3f967b2c510d65b13c56ab8cfb94bd65a7e9735abb58f95ac3039d3c30984ef, K ef53716; adjacent host8f8120875bbd849082b9ed072c0cd454fdaf28cad1270d143f82aeb919ee54a5 |
| Durable runtime inventory | Exact current session store392 records,21 runtime claims, all21 PID executables resolve;15 release-r2-env-forwarding,5 r41,1 r44. All three artifact paths hash to d3f967 K. Zero explicit_launch markers. No process argv/env read |
| Canonical Bus | version2, not candidate Bus6 |
| PRH | active PID1336599 SHA3051edd697e2ccdcbc89769a2ec8d031c93cf708857a3f4b93223b96557d9f9f; manifest source2fcb9c4b |
| Typed PRH info/get | protocol1.0/linux_process_group_sentinel; host_instance ff1ed508-1615-4e05-b46e-f8d9fe0ea8ff; cutex-job-service definition_revision1, desired/observed running, restart_count0 |
| Actual Job | PID1336668, birth64203813; mapped old Job747290895f65f5e336e9ba5f0568af957e986b53d562b10fae3ec3ac9136bf5c (36f8b577), not f7bbe3c/ba1a8d4f. Definition uses old K launcher, no --completion-v2 |
| Selected aemeath MCP | profile cd6a39eb-3997-45c6-9824-5113fe36a4b8; cutex_job command is same old Job;5 arguments mcp-stdio + file locators + old K launcher; trusted three Bus env names retained, no values printed |

PRH observations used only `hostctl info` and `get cutex-job-service` with the
exact retained hostctl/state directory. Only allowlisted metadata was emitted.
Job state.json at the declared state root is absent; this inventory does **not**
infer no active/pending jobs from that absence. Maintenance must positively
establish quiescence via the supported service boundary before replacement.

## One backup: NOT COMPLETE at the agreed location

`/mnt/mambo/PersonaProjects/cutex-lightweight-current-tui-integration-r1/upgrade-baseline-once`
does not exist. Consequently there is no COMPLETE.json/MANIFEST.jsonl there.
release-post-f567-r3/maintenance is empty. Original preparation and post-VM
reports explicitly say no live backup/deployment was executed. Thus the agreed
one-time baseline is **not evidenced as made**, not “completed because Task closed.”
No search of unrelated backup trees and no first or second capture was executed.

The latest frozen snapshot.py plan covers Cutex config/profiles/durable/roster,
Bus/Task/Management/seat/audit/runtime journals, native homes/catalogs/rollouts,
Job/PRH state/config, service definitions, shortcut targets and rollback bytes.
Runtime-native homes are selected from a fresh identity inventory; symlink targets
must be independently covered. SQLite online snapshots precede frozen append
prefixes with cursor coverage and native read oracle. Caches/sockets/locks,
later-created files and later appended bytes are excluded. This is a coordinated
dependency-consistent boundary, **not one instantaneous whole-filesystem snapshot**.
Actual coverage cannot be certified until the sole capture yields verified COMPLETE.
Historical ~56GiB sizing is not a current estimate; execution recalculates it.

Use the latest v3 snapshot rules: incomplete capture is PARTIAL_UNRESUMABLE,
not an instruction to resume the older r1 algorithm. Do not create another root
or backup merely to refresh this inventory.

## Actual remaining gates, classified

1. **Ordinary release preparation:** accept final native polish bytes, compose exact
   Cutex pins/default artifacts/companions/Job and refresh the old packaged scripts.
   Current release-post-f567-r3 selects Cutex1a71/native0c, while artifact-kit-r1
   selects Jobf3. Neither is the accepted current b48a/f8c/f7 composition. Preserve
   the current TUI line explicitly. Do not silently replace files in frozen packages.
2. **One-time maintenance prerequisite:** execute the first approved baseline only
   in the Human maintenance window, after closing listed Cutex writers and freezing
   central writers. Recheck completeness/cutoff immediately before cutover.
3. **Runtime cutover scope must be explicit:** existing cutover.sh changes only
   central Cutex, then sends generation-bound `cutex/runtime/online` bridge reconnect.
   It explicitly records native_agents_restarted=false. It does not upgrade the21 K
   owners, public native shortcut, profile layout or Job. All native runtimes need
   restart for an actual native upgrade; discuss that with Human before exposure.
   A bounded old-K coexistence window is a separate explicit choice, not full upgrade.
4. **Real unsupported/provenance boundary:** stock activation requires offline/no
   claim and no active Task; an already-marked record cannot change its bundle
   (`explicit launch already activated; no downgrade API`). Current host markers=0,
   so that latter prohibition is not currently blocking these21 records, but no
   packaged/tested conversion of their existing K histories/config/authority to
   lightweight owners has been established. Do not label ordinary restart or new
   neutral creation as same-history migration. New reviewed subjects are proven;
   existing-owner conversion needs a named compatibility procedure/decision first.
5. **Job ownership/config cutover:** retain the single PRH owner. Existing source-backed
   plan supports current-host/revision-bound stop→update→start and config expectedVersion
   CAS, but was not executed and targets old f3 with K launcher. Refresh it for exact
   daemon==adapter/new launcher pairing, reviewed dedicated Job descriptor, --completion-v2,
   quiescence, current receipts and rollback boundaries. No second daemon, root-through-MCP,
   generic mcp_servers injection or silently changed global profile. Full aemeath config
   migration remains unsupported; tested minimal reviewed projection is available.
6. **Target sandbox acceptance:** host bwrap0.11.1 is present; userns policy values
   apparmor_restrict_unprivileged_userns=1, unprivileged_userns_clone=1,
   max_user_namespaces=957653. These are not an actual target-user native sandbox test.
   Accepted VM policy/execution cannot certify host readOnly. Retain a bounded target
   smoke in release scope; do not copy VM AppArmor policy or substitute fullaccess.

The former “no configured lightweight Job MCP” gap in post-VM RESULT-R2 is now
resolved by reviewed Job descriptors and jv04/pv3, not a new blocker. Windows,
capacity work and the accepted unexplained pj07 risk are not reasons to restart
a Linux feature campaign. No pj07 reproduction performed.

## Rollback and minimum next task

Original binary-only rollback is allowed only before activation/bootstrap/external
facts and after current-state guards; it reconnects without restoring stale data.
Candidate Bus6/Job2/new native history must not be opened by old writers. Refresh
the cutoff guard for the exact final schemas before claiming rollback safety.
After crossing it, use compatible forward repair. Whole baseline restore is not
automated: stop all affected Cutex/native/Task/Job/PRH writers, obtain fresh Human
authorization, and explicitly accept loss of all post-baseline business changes,
new threads/jobs, excluded files and later history bytes. A snapshot ages even if
it remains cryptographically complete.

Recommend **one bounded final Linux release-package/cutover task** after native
polish acceptance: freeze exact composition; update central+PRH+reviewed-profile
steps and live-writer/compatibility fences; settle retain-K-window versus named
existing-history upgrade; prepare one-baseline/rollback and target smoke commands.
Then Human executes the accepted maintenance window. No claim that current old
scripts alone achieve full lightweight deployment; no new feature backlog required.

Evidence: live read-only observations above; release-post-f567-r3 README,
scripts/{snapshot.py,baseline-backup.sh,common.sh,cutover.sh,pre-cutoff-rollback.sh};
post-VM maintenance-plan-r2/RUNBOOK.md; current source
src/agent_management/explicit_launch.rs:399–435; docs/provider-item-pin-integration.md
and reviewed-aemeath-provider.md. Only this compact report is written.
