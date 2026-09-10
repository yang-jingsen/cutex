# Private reviewed Job MCP launch

The root-Human explicit-launch API accepts optional `job_mcp` on
`review_runtime` and `review_bootstrap`. Omitted/None preserves the old launch
surface; no arbitrary MCP/profile configuration is accepted. This is a private
Linux increment on Cutex1a71 with exact native0c425 and Jobf3bc9c8, not a migration
of the existing aemeath/PRH/K configuration.

The dedicated version1 descriptor names `adapter` and `launcher` VerifiedFiles,
local Unix `endpoint`, distinct `api_token_file`/`grant_key_file`, and the
operator-declared `daemon_pid`/`daemon_start_ticks`. The adapter hash is fixed to
the accepted Job build; launcher must be the exact coherent0c CLI from the
reviewed bundle. No env values, shell, additional arguments, cwd override or
caller authority is accepted.

Review verifies canonical paths/private credential custody (without reading
credential contents), distinct file identities, socket identity and actual
SO_PEERCRED UID/PID. The declared daemon must map the same Job executable/path
and exact process birth. The normalized review captures nonsecret device/inode,
size and change timestamps for endpoint/credential objects. Changes require new
Human review. This establishes actual local peer identity, not remote or
hostile-same-UID isolation. The daemon's configured launcher allowance remains
an explicit trusted-operator declaration, independently exercised by the actual
private daemon/adapter execution test; the Job API has no launcher-introspection
endpoint. A static declaration alone is not claimed as proof of job acceptance.

The normalized object participates in original review/action equality and is
stored in existing runtime receipts or bootstrap intent. Bootstrap carries it
unchanged to the resulting runtime review. Validation repeats before destructive
stop, claim, spawn and readiness commits. A previously recorded Job review
cannot silently become None on restart; supply an explicit descriptor. There
is no clear/downgrade API in this increment. Replays return the original receipt;
materially changed input under the same action conflicts. Failed/uncertain
launches retain the existing claim/recovery semantics, not another owner.

Serialization is additive with omitted None. New Some fields are inside existing
deny_unknown_fields review types: old binaries fail to read these opted-in
receipts/intents rather than silently dropping capability. Candidate-owned stores
only; no mixed writer/downgrade promise. Existing no-Job records remain compatible.

Only reviewed Some injects `mcp_servers.cutex_job` with argv exactly
`mcp-stdio ENDPOINT API_TOKEN_FILE GRANT_KEY_FILE LAUNCHER` and trusted runtime
environment names `CUTEX_AGENT_ID`, `CUTEX_AGENT_BUS_URL`, `CUTEX_AGENT_BUS_TOKEN`.
No token values enter descriptor/model schemas/logs. The existing Cutex control
namespace remains direct-only. Job tools retain native MCP approval defaults;
they are neither auto-approved nor added to the control-only namespace.

Core owns native thread/sandbox metadata. Job's actual issuer checks the current
Bus runtime/thread mapping and exact request cwd against canonical sandboxCwd;
it does not independently verify turn or explicit generation fields. No missing
metadata fallback, external-policy emulation or fullaccess downgrade is added.
Actual nested helper selection can override the outer launcher through native
SandboxState, so accepted evidence must name the observed helper bytes, not
assume launcher pin alone proves them. First supported pair is private coherent
0c/Core/helper and corrected Job; K mixed pairing is not claimed.

No arbitrary aemeath MCP table is stripped/accepted. Generic SharedConfig and
ProfileConfig remain deny-unknown. Windows, existing live PRH replacement,
production credentials/provider, Agent migration and deployment remain outside
this change.
