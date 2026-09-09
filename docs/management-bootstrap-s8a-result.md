# S8a result — explicit reviewed Management bootstrap

PRIVATE INTEGRATION_CANDIDATE, subject to Director acceptance. No live change,
rollout approval, full lightweight replacement or production Management claim.

## Source and bytes

Base `29a0539f0395eb45581b813e4795ed1c8d3f0cea`, tree
`e1da734aa0f7ce0c9952659e150beccfddb27363`.
Implementation lineage is base → `0718619f2a4ff0c12fbb521481ac8f944ff026a8`
→ `c5f30bbf3afb353a4f668a6e313a7c41b8418548` (tree
`0110c9339bdf9164605448aa3db9de322b72b766`). The final handoff descendant
changes only tests/docs; its exact commit/tree is in the Task Service receipt.

All paths below are relative to owned root
`/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`.

Default-feature, debug=0 binaries frozen from c5f30bb:

- `artifacts/s8a-default-c5f30bb/build-manifest.json`, SHA256
  `c8ab4577e0e58a7f263d0984901351aadab760fca1b75f303f9d1a574e3e1fe4`.
- Cutex `f8bf4b61926ca325c5c738e0ded821556cf2e142760cb523be14e64420087fdf`.
- Facade `932bef8e45ab2839bacc5bb571fb0c7a7376f8530976ea941dc3631aad8b5217`.

Separate fault-hook bytes from 0718619:
`artifacts/s8a-hooks-0718619/build-manifest.json`, SHA256
`0dd37a61f15d8859cef595a92a6a8b0567c4af048eedaebda0e8960011a54eeb`.
They are not default distribution evidence. The c5 follow-up adds an independent
adoption-receipt/identity fence before online and a non-Unix refusal; the tested
pre/post-ID cutpoints are unchanged.

Both manifests pin unchanged patched native a83dbb47ba6aa775f5d4b679fafc532c4db74c7f,
CLI f3601003, app-server 4638b862, official U host 3e85d674, schema 45986122,
and native manifest 1b7e8282 (full hashes in manifests). This is U+S6e, **not**
unchanged stock. No native binary/source or old manifest was modified.

## Finite acceptance matrix

| Criterion | Actual evidence / boundary |
|---|---|
| Root choice, separate Director authority | `s8a06`: real private root review/authorize, exact replay, existing authenticated project Director Create. Agent Bus credential receives 401 on root route. No root credential in MCP. |
| Refusal before create | Same run: missing intent, wrong action, wrong project/config, unknown review version, foreign Director and expired review reject; durable identity set unchanged and zero model calls. Full bundle/schema validation also covered by existing stock tests. |
| Neutral bootstrap | Actual pinned thread/start followed by the accepted paginated read(true) persistence ACK; zero fake Responses calls until explicit Human CLI input. Not a metadata-only read or timing barrier. |
| Adoption → marker → readiness | One new durable/native mapping, one bootstrap receipt, contract2/bundle3 marker, formal request name, current generation1, actual external-input handshake. Management receipt Complete and identical replay. |
| Identity/config separation | Explicit `alpha` profile retained; native title changed independently, formal name unchanged; shared config hash unchanged. Native session source is truthfully `vscode`. Prior two-profile launch proof reused, not repeated. |
| Usability on default bytes | `s8a06`: actual `cutex session stock-attach <new durable ID>`, same bound thread, one genuine Human prompt and one fake Responses reply, exit0 and termios restored. Independent history oracle confirms one Human turn/file/adoption. No autonomous real-model claim. |
| Creator death before captured ID | `s8a05`, hook bytes: abrupt exit86, Pending with no captured native/durable ID; restart and same action stay OwnerActionRequired, zero model calls, no second native create, two original durable records only. Not successful creation. |
| Creator death after captured ID | `s8a04`, hook bytes: abrupt exit86, exact captured native ID; restarted provider resumes it, obtains ACK, adopts once, reaches Complete/replay with unchanged native file count. Independent journal/history oracle passes. Later PTY teardown failed, so this run proves recovery stages, not terminal acceptance. |
| No default fallback / compatibility | Explicit missing-reference unit test verifies zero legacy bootstrap calls; same-action exact receipt and native mapping checks guard adoption/online. Management store v2 and typed durable bootstrap receipt reject old parsers. Existing generic marked-launch guard regressions reused. |

Final corrected fixture `s8a08` also passed on the same frozen default bytes,
including explicit `/exit`, exit0/termios restoration and the independent oracle.
Its new durable ID is `cutex.01a0861f-aeef-76c3-9b48-518c195c0cc1`, native ID
`01a0861f-aeef-76c3-9b48-518c195c0cc1`: one adoption receipt/file/Human turn,
zero neutral model calls. This repeats the default path solely to validate the
PTY fixture correction; it is not an additional distinct product criterion.

`tests/management_bootstrap_history.py` independently reads only fixture journals
and native history, without calling providers. It checks exact IDs, one adoption
receipt/file, actual roster project, source, one real Human input and zero neutral
sampling. `stage-only` explicitly does not certify terminal success.

## Checks and reproduction

Passed focused Rust selections: 67 `agent_management::provider::tests`,
5 `launch::stock::tests`, 3 `agent_bus::mcp::tests` = **75 distinct tests**.
The new reference-refusal test was also run individually (one duplicate execution).
An initial unqualified `--exact` selected zero; it is not counted as evidence.
Default `cargo check --lib --bins`, `cargo build --bins`, feature-hook bin build,
`cargo fmt --all -- --check` and `git diff --check` passed. Existing unused/dead-code
warnings remain. Complete scoped source diff was reviewed; no dependency/lock churn.

Build environment: scrubbed `env -i`, task-owned HOME/TMPDIR/CARGO_HOME/target,
existing rustup read-only, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_BUILD_JOBS=4`.
Only one Cargo writer. Private runtime command shape (new evidence directory):

```sh
bwrap --unshare-user --unshare-net --ro-bind / / --dev /dev \
  --bind "$ROOT" "$ROOT" --die-with-parent \
  env -i PATH=/usr/bin:/bin PYTHONDONTWRITEBYTECODE=1 \
  HOME="$ROOT/home" TMPDIR="$ROOT/tmp" \
  S8_PRIVATE_BIN_DIR="$ROOT/artifacts/s8a-default-c5f30bb" \
  python3 tests/management_bootstrap_boundary.py NEW_RUN \
  "$ROOT/target/debug/deps/external_input_controller-aa6dcf9121932446"
python3 tests/management_bootstrap_history.py NEW_RUN
```

The controller executable is an inherited setup input, not a substitute for
the default Cutex bytes under test. Hook mode adds `pre-id` or `post-id` and uses
the separate hook artifact. Root setup uses actual private APIs; fixture actors,
credentials and fake Responses are not production registration. The inherited
connect tripwire refuses non-owned endpoints before syscall; user/network
namespaces contain private listeners. No hostile same-UID or new sandbox proof.

## Preserved failures and limitations

- `s8a01`: fixture compared whole mutable session snapshots; heartbeat changes
  invalidated that assertion. Corrected to immutable creation fields.
- `s8a02`: real readiness exposed legacy seven-character versus stock eight-character
  routing-group projection. Fixed by explicit reviewed stock runtime groups,
  not by changing Bus policy or inferring Project membership. The newly created
  owned runtime was explicitly validated and stopped after correcting cleanup cwd.
- `s8a03`: CLI encountered untrusted new fixture cwd. Trust was moved into the
  original private shared config before manifest pinning; no runtime config rewrite.
- `s8a04`: post-ID recovery passed, but racing Ctrl+C teardown ended -15 without
  terminal restoration. `s8a06` supplies clean default-byte terminal evidence.
- `s8a07`: burst `/exit` input was treated as pasted input; the explicit wait timed
  out. Cleanup nevertheless exited0/restored termios. The fixture now submits a
  bracketed-paste command only after its rendered text, using the existing prompt helper.
- An early independent oracle counted native environment-context user-role data
  as a second Human turn. Corrected to exact prompt plus turn_context, not filtered
  away product history. All original runs are retained.

Only a finite pre-ID/post-ID crash matrix is claimed, not all disk-failure or
concurrent schedules. Known-ID ACK failure retains the exact ID and does not create
another; exhaustive injected native I/O failures are inherited/source-level here.
Existing S4 PID-time precision risk, two socket baseline failures, and S2/S46
security incident disclosures remain unchanged and unremediated. No leaked-output
inspection, remediation or no-impact claim. No arbitrary-descendant stop guarantee.

Unchanged S4/S5/S6f/S6g native/Task/Job/permission/approval proofs are reused;
no full suite, paid provider, Windows/Android, production OAuth, TUI/Job/native
build, live deployment or mixed-writer migration test. Runtime MCP full inventory
was not re-proved; this task uses CLI as its permitted post-create usability oracle.

Retained root is 11GiB, Mambo free 402GiB (within 20GiB/100GiB limits).
All owned probe children are cleaned up; evidence and binaries are retained.
No user data/history deletion. Before exposure, reject/revert the candidate;
do not point older writers at these v2/Soon/explicit-marker stores.

## Next bounded task

Protected replacement/Director rotation must explicitly carry this reviewed
bootstrap primitive through predecessor/seat/message stages, with original CAS
and uncertainty receipts; not implemented here. Unknown pre-ID transparent
recovery still needs a separate native request-idempotency decision. Release-MCP
terminal dispatch is a separate adapter gap. Production trusted-bundle-choice UX
and approval policy remain unresolved; this private root intent is not a rule
requiring Human approval for every future Agent.
