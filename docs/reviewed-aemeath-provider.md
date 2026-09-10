# Private reviewed aemeath ChatGPT mode

Bounded Linux candidate, based on a175e9e. Not general OAuth/provider support,
profile migration, host deployment or a new credential service.

The private projected profile explicitly sets
`cutex_provider_mode = "aemeath_chatgpt_v1"`. Absence means the existing fake
mode. Unknown modes/fields reject. Only profile UUID
`cd6a39eb-3997-45c6-9824-5113fe36a4b8`, name `aemeath`, builtin `openai`, no
provider overrides, model `gpt-5.6-terra` and effort `low` are supported. The
direct private native0c authenticated `model/list` returned this exact model
and advertised low; this catalog query did not create a thread/model turn.

The profile is an explicitly constructed minimal projection, not a copy with
unknown fields silently removed: model, provider, low effort and this mode only.
Original unrelated MCP, projects, shell/skills/TUI/service-tier/reviewer settings
are not migrated. Shared native config stays separately pinned. Generic
`mcp_servers` is still refused; reviewed Job remains its dedicated descriptor.
No model/name/cwd/profile-based durable ownership inference is added.

## Auth custody and authorization

Human explicitly supplies native-supported `auth.json` only in the private
authoritative CODEX_HOME, mode0600 with owner-private directory. No materialized
profile auth file is consumed and host auth is not edited/synced back. Existing
root runtime review or action-specific root bootstrap intent must confirm the
complete new configuration before execution; merely placing auth is not an
Agent-callable activation permission. Director project authority stays separate.

`StockConfiguration.aemeath_auth` is optional and omitted for old/fake reviews.
Present version1 binds exact canonical auth path, opened file device/inode/UID,
native directory device/inode and a domain-separated hash of native
`tokens.account_id`. No access/refresh/ID token or token digest is persisted,
serialized in reviews, put in argv or exposed as a model tool. File reads are
bounded, no-follow, same-handle and checked before/after; wrong owner, public
mode, hardlink, symlink, malformed/unknown auth fields or account drift reject.

Native0c `login/src/auth/storage.rs::FileAuthStorage::save` truncates/writes the
same file and flushes. Thus ordinary native token refresh preserves this custody
and account binding, while file replacement/account change requires fresh
review. A concurrent incomplete refresh read fails closed rather than accepting
mixed bytes. The native backend, not Cutex, authenticates/refreshes the existing
native token format. This is not hostile-same-UID security or a new OAuth flow.

The selected reviewed provider is HTTPS
`https://chatgpt.com/backend-api/codex`, Responses, native OpenAI auth and no
WebSockets. Native file credential storage is explicit; request and stream
retry maxima are0. No loopback proxy, alternate endpoint, model fallback or K
fallback is supplied. Native-supported refresh endpoints remain native-owned.

Profile/account configuration digests, runtime-review digest v2, expiry, root
confirmation, Task/authority guards, claim CAS and original action replay remain.
Old completed receipts serialize/replay without the new optional field. Old
writers reject new fields; no pending review is upgraded and no mixed-writer
or old-parser downgrade/migration is promised. Auth cleanup after the smoke
intentionally makes further launch fail closed until a new explicit credential
and review ceremony; it does not clear durable launch requirements or receipts.

## Required evidence

Unit custody/refresh/account-replacement and mode/model/profile refusals;
existing fake/runtime-review/Job/bootstrap regressions; default artifacts;
then one private ordinary-UID native/Core/Job real-provider smoke with finite
turns and actual MCP approval, usage reporting, no hand-supplied Core metadata,
and exact guest credential cleanup. Catalog preflight alone is not that smoke.
