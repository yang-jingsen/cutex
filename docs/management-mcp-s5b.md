# S5b bounded Management/list MCP contract

Base `5a81e04280bdb655d0e9013512f68257ff283582`; native reference K
`ef53716b7673ad14c24b977667334e31e66110d8` agent_management/spec and
cutex_agent_bus/spec handlers. Existing providers remain effect/authority owners.

| Operation | Required beyond action_id/operation (optional project_id) | Execution boundary |
| --- | --- | --- |
| query_managed | none | existing provider query |
| online/offline/restart/close | cutex_session_id | exact durable target; close is permanent |
| create | spec, start_mode | native bootstrap, not new stock support |
| replace | predecessor_cutex_session_id, policy, successor, start_mode | existing provider transaction/receipts only |
| director_rotate | expected_predecessor_cutex_session, expected_authority_epoch, mode, successor | existing project/Task authority transition only |

spec/successor require name,cwd,profile,runtime_backend,model,reasoning,permissions,
approval_policy,sandbox_mode,groups; expose_to_im/pin optional. frozen_message
is optional only on create/replace/rotate. Native enum values unchanged. No
grant/revoke/root stock activation passthrough, no implicit formal name/profile.
Typed current-provider serialization supplies the receipt comparison digest.
Current typed roster records retain historical creation provenance (null after
Human import, possibly another project after an explicit move). The adapter
recognizes that exact record type; it does not confuse that field with current
membership or weaken checks on the response's project/authority. Immutable
provider receipt bytes/values are never relabeled.

Management uses the native 30-second value as the HTTP socket read timeout and
the 1 MiB response body limit (bounded HTTP framing too), not the old wrapper's
whole-child-process wall-clock timeout. Provider no_write/owner_action_required
outcomes pass through. Transport timeout/5xx has response_uncertain code in the
native no_write-shaped envelope, explicitly NOT proof of no commit or rollback:
retry the exact action/payload. An invalid receipt likewise cannot prove rollback.

cutex_agent_list accepts native optional all_groups/all_hosts fields, but true is
explicitly unsupported in this local prototype; omitted means false (different
from native's implicit cross-host default). Existing Bus group visibility applies
using authenticated runtime, never a model requester. Only current exact durable
mapping is actionable; unavailable names/mappings remain null with observation
reason, never a title/cwd/name guess. Listing reports registered runtime state,
not inbound delivery/A4 or all durable/offline Agents. query_managed is the
separate managed roster query. Existing send scope/dedupe/offline rules unchanged.

Every request uses Core thread and current generation plus normal Bus credential;
provider Director/Operator/Task protections remain authoritative. Stock-marked
generic online/restart remain rejected without K fallback. This
task does not add managed stock bootstrap or stock rotation. Translation and
successful lifecycle execution are separate criteria in the final result.
