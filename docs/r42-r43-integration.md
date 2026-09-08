# R42.1 / latest baseline integration

## Direction and preservation

Merge latest `04dfc0c8ad726bede69fa47110ddb806fac3cea6` (first parent)
with R42.1 `b1dfc8e5fdd417a5e01b00af72476da6b8f21f16` (second parent).
Common base: `fed5e05f65c0921551b4973bb7475908141347f5`.
This retains the accepted recent first-parent baseline and both histories;
reverse direction offers no semantic advantage. No squash, rebase or cherry-pick.

The merge has no textual conflicts. All 34 non-overlap changed paths have
exact originating-tip blobs. The sole overlapping path, session_tui.rs,
retains the exact R42 delta (stable patch ID
`31da183926a52fba9322d090ae501cecdc9be08e`) atop the latest baseline.
No merge-specific production code changes were needed.

## Semantic inspection

Managed restart interrupts the active native turn before runtime stop;
force behavior records interrupt failure. History repair remains an explicit,
confirmed offline operation, reachable through CLI and selector dispatch.
It backs up the native rollout and repairs orphan terminal events, preserving
the prefix and normalizing affected ordinal suffixes for pagination.
It does not write durable formal name, profile, roster or project membership.
Existing nullable profile/provenance, exact identity, confirmed Projects
import/Create/Add/Move, CAS/replay/audit and scoped notification routing remain
intact. Delivered notification history is unchanged.

## Verification

All cargo invocations used --offline and a scrubbed environment with private
temporary HOME, explicit cargo/rustup tool homes, no live DBUS/XDG manager.

| Test filter | Target | Passed |
| --- | --- | ---: |
| runtime::codex_home | lib | 10 |
| app_server::manager | lib | 10 |
| agent_management | lib | 113 |
| agent_bus::server | lib | 44 |
| task_delivery::provider_adapter | lib | 9 |
| task_service | lib | 88 |
| session_tui | bin cutex | 202 |
| management | bin cutex | 55 |
| repair_history | bin cutex | 1 |

Management bin filter explicitly excludes
production_one_launch_override_leaves_durable_configured_intent_unchanged and
production_runtime_handler_constructs_runtime_tui_and_combined_profile_receipts:
these require a user systemd manager. Other live-systemd legacy stop tests
were not selected. No live restart, native-client pagination, or deployment
acceptance is claimed; pagination evidence is the ordinal repair regression.
The isolated production binary compiled and printed session repair-history
--help. cargo fmt --all --check and staged/unstaged git diff --check passed.
Compiler warnings remain; no unrelated warning cleanup was attempted.

No unresolved merge-induced semantic issue was identified. Director/Release
acceptance, remote-main archival and publication are separate authorized steps.
