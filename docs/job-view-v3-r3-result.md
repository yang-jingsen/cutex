# jv03: Ready passed; fixture rejected Responses Lite tool declaration

One authorized run only. No fourth attempt. Base fixture/docs
`1e279d4e6ea39a9a5786e6103446b68175089af0`, tree
`7e19b4b5b532c5169a5a66ff0feffe4d99e75cbf`.
Product remains `b48a2f63a5e60e6b0c6e6d8ca97e4ae4882dd53f`, tree
`9fe31ce69c255c0572ac0919e9cd94cb5e0dff09`; no product changes or rebuild.
Exact default manifest `artifacts/job-view-v3-r2/build-manifest.json`, SHA256
`e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d`:
Cutex2412fb31, facade0aa3ea1b, native f8c33add CLIb8307571 / b8e9cc server7bc7f3d7 /
official host3e85d674 / stable schema c2a54d59, Jobf7bbe3c ELFba1a8d4f.
Full hashes and source identities remain in that unchanged manifest.

## Achieved / not achieved

| Boundary | Result |
| --- | --- |
| Fresh neutral creation, adoption, explicit review/activation | Passed, no model calls during creation |
| New Job pin / peer custody / runtime launch | Ready generation 1, exact replay passed |
| Reviewed selection | gpt-5.6-terra / low / s6-private loopback fake only |
| Actual configured MCP | cutex and cutex_job connected, 7 and 4 tools |
| Same-owner CLI | Opened and initiated the sole user turn |
| Actual CodeMode advertisement | Present in Responses Lite additional_tools, functions namespace |
| Fake tool response | Local guard rejected it before emitting a call |
| Job command / approval / A4 / ACK / display / replay | Not reached |

Only one local fake-provider request. No real provider, paid model, credential,
VM/Human fixture or host service access. No Job state.json was created and no
Job was submitted. No false Submitted reply was produced this time.

## Exact failure and bounded diagnosis

`jv03/fixture-error.json` records:

```text
fixture requested an unadvertised tool: exec
```

This is a **fixture assertion bug**, not a native tool refusal or absence of
CodeMode. Captured `jv03/model-requests.json` has no top-level `tools`, but its
first `input` item is:

```text
type: additional_tools, role: developer
tools: namespace functions
  custom exec
  function wait
  function request_user_input
```

The guard examined only `request.get('tools', [])`. The exact selected catalog
entry has both `tool_mode: code_mode_only` and `use_responses_lite: true`.
Native b8e9cc `codex-rs/core/src/client.rs:940–979` deliberately serializes
Responses Lite tool declarations into `ResponseItem::AdditionalTools` and
sets top-level tools to None. Thus the model-selection correction did work;
the offline guard covered only the old standard-Responses shape. Prior p6
standard Responses evidence did not cover this shape.

The guard returned local HTTP400 and the fixture detected its error immediately,
stopped and cleaned up. This is not pj07, not a provider rejection, and not a
Job-service failure. No alternate direct MCP path or feature override was tried.

## Follow-up decision, not an automatic retry

Before any further run, teach the fixture to extract the exact structured tool
declarations from standard top-level tools **or** Responses Lite additional_tools
and preserve the namespace. Assert `functions.exec` against the actual schema;
do not infer absent tools from prose or simply disable the guard. Check the
exact accepted custom-call namespace encoding and the captured Lite request in
an offline test, including negative namespace/name cases. This is fixture-only;
no product option or security-policy expansion is indicated by this evidence.
No fourth attempt is authorized or performed here.

The prepared read-only `tests/job_view_v3_history.py` oracle was not run because
no Job/A4 exists. It is not acceptance evidence. Later display/shortID/output
and replay assertions likewise remain unexecuted.

## Evidence and safety

Preserved `jv03/launch.json`, `inventory.json`, `terminal.pty`,
`model-requests.json`, `fixture-error.json`, native JSONL and
`tmp/job-view-v3-jv03.log`; jv01/jv02 remain unchanged. Pre-run existing
tool-contract offline tests and prepared transform/syntax checks passed, but
their missing Lite coverage is explicitly disclosed. Full scoped fixture/docs
diff self-reviewed, diff check passed. No build or broader test campaign.

Tracked runtime birth was checked before owned group cleanup; owned child
handles were waited and the private user/network/PID namespace ended. Private
histories/dummy files retained, no old Human runtime or auth cleanup. Outer
bwrap read-only host + owned writable root + loopback isolation/tripwire retained;
receiver was explicit full-access/on-request within that test containment,
not a new read-only sandbox guarantee. Root17,350,418,388 bytes, free373,713,985,536
bytes, within24GiB/100GiB. Intended reference blocker plus preserved pin-repair
candidate, not full composition, VM-ready or deployment acceptance.
