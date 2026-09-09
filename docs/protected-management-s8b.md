# S8b private protected lifecycle and completion MCP

Base 7db389cfec3c8b3cc405ed74bf52ed8a5022f1f9. Work in progress;
no deployment or real Agent operation is authorized by this candidate.

The existing root `explicit-launch` review/authorize route can freeze one
complete Create, Replace or DirectorRotate request. Neutral Create retains
intent version1; other variants require version2. Exact action, predecessor,
policy/mode/message, project authority digest, configuration and bundle/home
remain in that immutable review. Old S8a readers reject version2 intents.
Management store v2 and durable launch markers remain candidate-writers-only.
There is no default bundle selection, root MCP method, or marker-clear route.

The existing project Director consumes the intent through
`cutex_agent_management` with `bootstrap_intent` equal to the action ID.
This reference grants no authority and cannot mint or change an intent.
Legacy unmarked requests retain their normal path. Expired unused intents
reject before predecessor effects. Unknown pre-ID outcomes still require owner
action; captured IDs recover only that native thread and existing receipt.

Replacement policies and rotation modes use the existing lifecycle journal,
protected predecessor guards, Task Director transfer and Management authority
CAS. No replacement/rotation state machine is duplicated. Retained predecessor
operator behavior is existing provider policy, not a new grant invented here.
Original authenticated caller replay remains governed by existing historical
receipt authorization after transfer. Generic marked restart still refuses.

Reserved Management start messages now use generic ingress as `service`
source, with requested Director and explicit instructions. Native is still
the accepted a83dbb47 bundle3/contract2. Bus freezes and commits the original
envelope/A4; actual Management action/successor provenance is checked again at
business commit. A4 is context persistence, not successor work completion.

`cutex_task_service_terminal` exposes only `accept_result`, `request_changes`
and `fail_result`, with action ID, assignment ID and optional decision reference.
The narrow `/api/task/v2/terminal-semantic` adapter authenticates the current
runtime and resolves current seated principal and mechanical context inside
Cutex before calling the existing terminal provider. No model-supplied seat,
attempt token/revision, caller or root credential. Director/Worker tools and
raw existing terminal API semantics remain unchanged; cancel is not added.

Production bundle selection UX/default policy and existing-Agent migration
remain future decisions. A per-action private Human review is not a universal
future approval requirement. No old-parser downgrade, history rewriting,
OAuth/multi-account support, arbitrary descendant stop or full replacement claim.
