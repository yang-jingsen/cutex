# R35 integration inputs

This Cutex integration is based on
`9be37fbc485f472853e24d30be52d9f494c48391` and combines these accepted inputs:

- Tasks workspace: `794b98ffb81ffda2cda327376ae287d2d518b847`
- reservation reconciliation: `303ab950a231edf4c332ab40243be9202f3a70e6`
- project Agent Operators: `a5a998584ce3226151f9ca505163f89d530911db`

The paired cute-codex resize fix is
`b8de72af5f0ecb2352e37fe76be3e33d2ec510d5`. It remains a separate release
input and is intentionally not copied into this repository.

The overlapping Agent Management changes are integrated as one store model:
legacy snapshots default both reconciliation and Operator collections to empty,
unknown top-level additive fields survive writes, and reservation receipts and
events coexist with Operator grants, grant revisions, and audit events. Task
Service Director queries continue to derive project scope only from exact
Primary Director authority; an Operator grant never supplies Director-seat
authority.

After independent review, release preparation must pair this Cutex commit with
the cute-codex commit above, rebuild both artifacts through the normal release
process, and repeat deployment preflight. This integration itself does not push,
deploy, or change live state.
