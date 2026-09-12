# Same-ID maintenance r2: blocked private verification

**NOT runner-ready, NOT migration-accepted.** Two authorized private attempts
were consumed. No live migration, stop, auth access/copy, permission change,
backup, provider request, model turn or Job execution occurred. No third attempt.

## Frozen implementation and artifacts

- Base `f321dae15a75e44192b2d723b0dbbec72997733a` (product `7e51bab7`).
- Product checkpoint `b3efed61b0124c992dfece38e6b38faf500ce346`, tree
  `94fe87369a7217a8e42781c92284d98e55869def`.
- Fixture-only correction `a4df1ecd953773f4862837d21cf2e0815a004a02`, tree
  `105307f7b3eb246597c57b61fd7cca208f8bf5e7`.
- Artifact root `../artifacts/same-id-maintenance-r2`; manifest SHA256
  `1826797753767a79ff9cf8505a77036f524c4bcbd26c22e1d1723094339de8ef`.
- Default-feature dev Cutex `ab0b662f96464f0240f81bef506c755af2ccfafd58c436ddc60e16a81b49c51a`;
  facade `fba5b6bd3a48b55c5f25643d3a9daa0e0852d298205b0436868d3ffa7f7355ea`.
  No release-optimization claim. The separately labeled test executable is
  fixture setup only, not a distribution binary.
- Native CLI `8cde7956` / server `2eab060b`, official host/schema and Job
  `f7bbe3c4` unchanged; complete component hashes are in that manifest.

The mutation surface is confined to the new maintenance module/held-file helper,
the existing root explicit-launch dispatcher/receipt variant, sealed stock
runtime entry, per-target home validation and typed profile/skill path projection.
Ordinary Agent/MCP lifecycle permissions are not extended. See
[interface](same-id-maintenance-interface.md). Full source delta is
`git diff f321dae15a75e44192b2d723b0dbbec72997733a b3efed61b0124c992dfece38e6b38faf500ce346`.

## Decisive observed blocker

1. `../same-id-5ygj54qb`: source native launch failed because the fixture used
   nonexistent `:full-access`. Native said: `default_permissions refers to unknown
   built-in profile :full-access`. Corrected to derive the existing named profile
   from the selected sandbox (`:danger-full-access` here). No migration began.
   FAIL SHA `a5637a437fb4560672e09589c3d836dbbbf060fe3ebf874c3d0a4bd6297ed095`;
   source stderr SHA `68c22e10a26365b0fc6cb17e3056349d097f7f4f73dd06d68b625e14da7281ab`.
2. `../same-id-4pfez9rk`: actual pinned native resumed the original frozen
   `cutex-director-r13` history ID, returned the matching ID, then exited normally
   with code **0** after its owned connection closed and SIGTERM. The original
   copied prefix still matched its frozen hash. However `state_5.sqlite` was
   **4096 bytes** and `state_5.sqlite-wal` **1,895,232 bytes**, both mode0600.
   The fixture stopped at the product's named pending-WAL prerequisite, before
   private service/authority setup or review/apply/start. This is **not** evidence
   of corrupt WAL, a native crash, or pj07's cause. Normal exit is simply not
   evidence that WAL is empty.
   FAIL SHA `5ca514ada0711e6ea12e59761b99de10c2d3a48fca101e0149c9708c55a467c4`.

The implemented catalog reader uses SQLite `immutable=1` only after rejecting a
nonempty WAL, so it cannot safely consume this normal post-exit state. It never
silently ignores WAL, checkpoints the original, or substitutes guessed modes.
All owned attempt children were stopped/waited; the second attempt's only child
reported exit0. Private histories and failures remain; no retained Human fixture
was accessed or stopped.

## Evidence boundaries

| Criterion | Result |
|---|---|
| Default lib/bins check and Cutex/facade build | Passed; existing warnings retained |
| Fixed34 selection / original16 offline / excluded IDs | Unit passed; no reselection |
| Semantic digest excludes only last_seen_at/updated_at | Unit passed |
| Unknown request fields/receipt variants/config refusal | Unit passed |
| Actual SQLite read-only metadata / no SHM / pending-WAL refusal | Component test passed |
| Held-file exclusive copy, stale/partial/symlink refusal | 2 actual filesystem unit oracles passed |
| Existing review digest legacy/replay compatibility | 3 tests passed |
| Explicit launch version evidence | 1 test passed |
| Actual original-history native resume and normal exit | Second attempt passed before catalog gate |
| New root CLI review/apply/start, Task/authority preservation | **Not reached, unproven** |
| New Agent/foreign/stale actual authorization negatives | Prepared fixture only, **not evidence** |
| New migration interruption/replay and current-generation Ready | **Not reached, unproven** |
| All34 configuration/materialization comparison | Prior accepted rehearsal reused, not rerun or upgraded to this entry |

Prior34 registry/filesystem/history/auth/group proofs remain referenced by the
accepted rehearsal reports. They do not prove this new Human maintenance
authority path. No visual matrix, runtime restart campaign, paid test, live
cutover or rollback test was performed. The protected Task setup helper uses a
real private seated provider, but was not reached; no fabricated Task ACK exists.

## Smallest next decision / continuation

Allow one bounded correction and private verification of **WAL-aware read-only
catalog capture**: capture verified DB and WAL handles/stability, copy only those
files into an exclusive private temporary catalog namespace, read the committed
SQLite view there, and fence original identities before/after. Original source
must remain byte-for-byte untouched; no source checkpoint, ignored WAL, DB field
editing or live service restart. Preserve only required semantic metadata and
close owned temporary handles. This is a proposed correction, not implemented or
proven here. Alternatively provide an already-supported authoritative read-only
catalog export from the offline source owner; do not assume one exists.

After that gate, complete the already prepared one-representative root
review/apply/start/Task preservation oracle and the release owner's targeted
mutation-surface review. The existing Human runner must remain NOT_READY.
The separate existing close-404 issue is also not solved by this offline-only
entry. Auth0775 maintenance remains release-owned. Original offline16 remain
offline; four damaged histories remain quarantined. The completed87.93GiB
backup is untouched and not repeated. No automatic old-writer rollback after
new tagged receipts/home/history writes; restore requires new loss approval.

Resources observed: task root approximately19GiB, Mambo approximately240GiB free
(limits24GiB / minimum100GiB). Original artifacts, failures and pj07 risk retained.
