# S6c1: private launch and generic ingress client

Base `c7ad7e1d3e1a14fc9a96cd19fc21d5b87016d11a` (production parent
`19db822ad800f99cbd8344426d259ea16412a286`). This slice does not wire Bus,
Task Service or Job delivery/ACKs. It is not deployed or a managed Create API.

## Exact native inputs

The supported second bundle is **U+S6 patched**, not unchanged stock:

- U `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`.
- Native `c2aaceb411b7851806c62435b97895a63a7d34cd`, tree
  `bd85280058ddc4b851d2bd9ca1904c392d62d388`.
- Frozen direct app-server SHA-256
  `b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60`.
- Compatible official-U code-mode host SHA-256
  `3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45`,
  adjacent to that app-server. Native build manifest records its reused IPC proof.
- Experimental schema aggregate SHA-256
  `00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d`,
  from `artifacts/s6c1-schema-handoff` in the native owner's root. Its
  `PROVENANCE.json` identifies the exact committed precomputed archive blob
  `98a18de4f6cf84bee81ee8f6c931cc7222ab8a7b` and archive SHA-256
  `ae17a692e5fdef5625f9f5950facf95d7cf8d3ca6f840007ff4fb71d69df4eb5`.
  No native regeneration/build. The loose tracked JSON is the stable schema,
  not the experimental aggregate. The direct app-server has no schema-generation
  CLI subcommand; that initial private command failure is not a native API gap.

The native README's Private ExternalInput v1 section and exact
`app-server-protocol/src/protocol/v2/external_input.rs`,
`protocol/src/external_input{,_status}.rs` define the wire contract. The S6a
proposal is background; accepted S6b2r2 receiver-byte policy supersedes its
earlier sizing assumptions. No tokenizer or model-name sizing inference.

## Launch contract

`StockBundle` version 1 retains the exact official executable/schema/companion
pins and registration-only behavior. Version 2 requires `native_patch_commit`
equal to the accepted S6 commit, the exact patched executable and experimental
schema, and the compatible pinned host. A Boolean capability cannot bless an
arbitrary binary. The reviewed facade and shared config remain hash-bound.

Existing root-authenticated `/v2/agent-management/explicit-launch` review,
activation and run retain their CAS/action journal. A Human `review_runtime`
request can add `receiver_canonical_byte_limit`: positive integer through
u32::MAX or exact `"off"`; missing means 10000 decimal bytes. Null, zero,
fractional/negative/Boolean/unknown values reject. The value is included in the
review and immutable run receipt, so changed-action replay conflicts. Default
is omitted in serialized old receipts; legacy behavior is unchanged. A
non-default policy on unchanged stock rejects before stop/spawn.

For U+S6 the trusted launch owner creates `external-input.json` in the fresh
private runtime directory, with create-new/nonsymlink mode 0600 and current
UID ownership. It contains version, exact durable owner/native thread, reserved
generation and receiver policy. It is checked before releasing the existing
gated child. Native loads it once via `--external-input-binding-file`. It is
derived launch evidence, not a second authority store. Missing/changed files
fail closed; no marker clear or K fallback. Configuration changes require an
explicit reviewed restart. Existing profile/home/permissions semantics remain.

The manager requires `initialize.externalInputVersion == 1` before resuming the
exact thread. Both private bundle families disable **legacy K** activity/Bus
ingress. Their Bus bridge remains registration-only in this slice, including
U+S6. No inbound polling, business delivery, transport ACK or notification-route
change is introduced. U+S6 schema provenance is not mislabeled as K.

Only a direct app-server artifact is accepted here. The existing stock attach
command explicitly rejects the patched bundle rather than running `resume`
against an app-server or silently substituting another CLI. A pinned compatible
patched CLI is a later distribution boundary, not proved by this slice.

## Trusted client and recovery

`app_server::external_input::ExternalInputClient` is a library controller adapter,
not an MCP tool, public HTTP action or background worker. It connects only to
the private Unix endpoint resolved from the current durable record and unique
matching Ready receipt. It checks the launch marker/bundle, exact native ID,
generation, runtime binding, current runtime agent, archive state and derived
binding, unique native mapping and exact recorded native process. The same occurrence is rechecked around every RPC result, including
errors. Existing-thread connection does not create/resume a second writer.

`submit`, ordered bounded `status` (1..100) and explicit `retry` use strict typed
wire fields. Envelope and receipt digests match the native length-framed UTF-8
algorithms, with generation outside semantic identity. Receipt identity,
ordinal, owner/thread/item/key, response count/order and processing-state shape
are validated. Unknown fields/states fail closed. RPC errors retain their native
error source; transport failure is not `unknown`, A4, rollback or completion.

`statusChanged` is only a validated thread/message hint. No automatic poll or
retry worker is added. Held/no-output/uncertain states remain visible. Retry
requires the calling trusted controller's explicit decision and original
attempt/retry identity; native performs the historical CAS. Repeated submit is
not a retry permission. A4 means native canonical pair, flush and publication,
not output or business completion, and not fsync/power-loss durability.

This conservative private adapter revalidates bundle files at its fences; it is
not a high-throughput delivery claim. A long-lived controller must consume
`next_hint`/events or close its connection; the existing transport's bounded
event queue/backpressure remains, rather than adding a hidden drain worker.

Before A4, the future caller must retain its existing durable pending item and
exact envelope. Lost response: status on the same key; if native positively
reports absence, same-key submit is allowed. Restart uses the same owner/thread
and new occurrence generation; historical receipt/digest remain unchanged.
No no-ID create recovery, owner guessing or replacement routing is added.

## Next boundary and exclusions

S6c2 must freeze business-to-generic envelopes in the **existing** Bus/outbox
record, map authenticated durable source identities, and validate current
occurrence again at the existing business-commit CAS. Client response fencing
does not make a later unrelated business write atomic. Keep existing context
facts -> durable delivered -> transport ACK ordering. Do not infer Task/Job
completion from native output or create a new inbox/ledger.

Future templates should expose concise source/event/action and only necessary
tool IDs once. Durable identity and receipt machinery stay outside prose; no
name/title/cwd authentication. This slice implements no business templates.

Retained limits: private Linux candidate writers only, trusted same-UID native
controllers (not malicious same-UID isolation), S4 seconds-truncated PID-time
risk and two baseline Management socket-length failures, S2/S46 incidents
unchanged. No managed Create/replace/rotate, marker downgrade, production auth,
history migration, TUI/Android/Windows, full native tool inventory or deployment.
Rollback before exposure is reject/revert this candidate; private probe cleanup
stops only tracked owned processes. Never downgrade a history-bearing marked
record to an older writer based on this source revert.
