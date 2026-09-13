# Agent Bus requester query encoding

The MCP adapter percent-encodes a URI query value, retaining RFC 3986
unreserved ASCII letters, digits, `-`, `.`, `_`, and `~`. Reserved characters,
percent signs, whitespace/control bytes and non-ASCII UTF-8 bytes remain encoded;
input cannot add query parameters or HTTP headers.

This preserves exact generated runtime IDs against Cutex
`1a71a850885ac6fd9d918dcd045bdd065d639e81`, whose `query_value` returns raw text.
Its stock producer (`src/agent_management/stock_runtime.rs`) emits
`stock.<UUID>`. The legacy `agent_id_for_launch`/`sanitize_session_component`
producer uses ASCII alphanumeric, dot, dash and underscore. Neither identity
nor registration is rewritten by this adapter fix.

This is **not** universal compatibility for arbitrary registered IDs. The
service-authenticated registration path retains `payload.id` without the
producer alphabet restriction. A custom ID containing reserved/non-ASCII
characters is safely encoded but still cannot match that raw Cutex parser.
Supporting those IDs requires a separate parser/protocol decision; do not
remove encoding, invent a replacement ID or silently decode twice.

Focused tests cover unreserved spelling, reserved/non-ASCII escaping, actual
owned-loopback request bytes, and exact subject selection including duplicate,
missing, wrong-thread and missing-durable observations. Full installed-runtime
tests must use the task's pinned private artifacts, not the historical test's
hardcoded operator installation path.
