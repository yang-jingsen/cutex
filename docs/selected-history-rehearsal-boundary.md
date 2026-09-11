# Selected-history private rehearsal: authority boundary

Status: blocked before apply; not migration or runtime acceptance.

Base: `75b86bc53a9f2deae1edfb8fea390fd8965cdb9b`, tree
`fca7d660770a866341f08ab776a7f12f562ff80a`. No product changes.
The accepted default composition remains `selected-projection-r3/build-manifest.json`
SHA256 `3bc284978dc2e5cb61c3b1021ca24bd87c3adbe7f338dceeada97d92786098e3`.

## Concrete decision required

The fixed selected34 do not contain the Director of `vce2026`. Its selected
member is `vec-submission-exporter-r1`,
`cutex.01a05b4f-0cfc-7680-91da-111bd4aadad5`. A narrow read-only check of
the current Management projects map confirms its Director is
`cutex.01a05aca-2487-7472-b950-2b854922938e`, outside the selected34.
This check read metadata only; no history bodies or authentication files.

The current supported API cannot create this project in an empty private
store without an active managed Director record:

- `src/agent_management/projects.rs:1230`: Create requires zero CAS and an
  existing nonretired managed Director, then creates both authority and seat.
- `src/agent_management/projects.rs:899`: AddMember requires an existing
  project authority and its current epoch. It cannot preserve membership by
  naming an absent project.
- `src/agent_management/durable_import.rs`: import preserves durable identity
  and can invoke a separately authorized project mutation; it is not an
  authority-snapshot restore API.
- `src/session/service.rs:232`: existing native-ID adoption gives the expected
  deterministic durable ID, but does not import project authority.

Therefore do not add the unselected Director, assign the selected Worker as
Director, omit its project, or edit the Management store to pass the test.
Recommended smallest decision: authorize a narrowly scoped **private authority
fixture** retaining the exact outside-cohort Director identity as a nonlaunching
prerequisite, explicitly separate from the 34 migration subjects. Its history
must not be repaired/copied/launched. If that is unacceptable, authorize an
explicit partial rehearsal excluding this project's runtime proof while keeping
its selected subject blocked in the plan. Neither choice has been executed.
This is a fresh-store rehearsal constraint, not evidence that an in-place live
migration must change project authority.

## Inputs and finite plan

Selection is frozen at commit `7f85283116438eb2988bb34fcb6836817168b1c4`,
`docs/project-recent14-candidates.json`: 34 subjects, 18 observed online and
16 offline. Do not rerun activity selection. Project counts are cesc2 11,
cutex-stack-main 16, ifm 3, scpolya 1, tethys-une 2, vce2026 1.
The other five project Directors are in the cohort. The scpolya association
is an explicit current membership override; preserve that fact in the plan.

Reuse native `artifacts/selected-k-history-r1/prefix-manifest.json`,
`metadata-results.json`, `identity-results.json` and the existing frozen prefixes.
All34 have enabled catalog memory; 30 paginated and four legacy histories;
all have matching native IDs and no recorded parent/fork identities. The
accepted `artifacts/legacy-interagent-reader-r1/RESULT.md` covers decoder and
four representative native read/resume cases, not Cutex project import.
No new >1GB copy or original-history scan was performed.

After the decision, the bounded implementation sequence is:

1. Emit selected-only metadata plan, binding frozen history hashes and original
   identity/profile/permission/memory/status intent. Any current metadata read
   must have its own capture boundary and report drift rather than change cohort.
2. Materialize only into a fresh owner-private destination with no-overwrite,
   no-symlink and independent filesystem tests. Never hardlink writable history
   to a frozen source. Preserve interrupted destinations for diagnosis.
3. Use supported adoption/default/profile/import APIs and explicitly authorized
   project fixture operations; no raw canonical store editing.
4. Use synthetic account files and substituted private asset paths. Exercise
   representative historical/profile/permission variants without model turns;
   verify unchanged original prefix hashes and private derived-index writes.
5. Report offline subjects as offline plans, not automatically started agents.

This sequence is a proposed plan, not an implemented or tested apply command.
No new runtime attempts were made; no filesystem safety oracle or final
preserved-history Cutex attach proof is claimed. These remain acceptance work.

## Remaining cutover prerequisites

Resolve the private authority fixture boundary, then finish materialization and
representative composed tests. Real account custody (currently 0775 ancestors),
maintenance coordination, single operational snapshot and live cutover are
separate and not executed. Four damaged active histories remain quarantined;
retired records are not revived. New stores/history cannot be rolled back by
opening them with old writers. Existing pj07 risk remains deferred.

Resource observation: owned task root approximately 18GiB; filesystem available
359139876864 bytes. No builds, credential access, runtime cleanup, VM changes,
provider traffic or operational snapshot. Only this report was added.
