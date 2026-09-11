# Display polish v2 composition

Accepted base cc13616990582837a2dd346e6c2cf56a7bf335f7, tree
77b025235f9a8ef63ab5dd51ae258f0315b5cad4. Product pin commit
2e7ff4a7510bf9310c7bdf367b400f1e5d224219, tree bbe1fcc48eddfd4efca3ee4de1a1eedebbe46e27.
Later composition harness/docs do not alter compiled Rust. Default binaries and
exact native/Job identities are in ../artifacts/display-v2-composition-r1/build-manifest.json.

Only product delta: replace the accepted exact native family source and CLI
hash with cc4a080df1df4433f6fd67fee9c1c4fa4a42baab / 52b86844….
Server df95936f… and official host3e85d674… remain identical. The final-generated
aggregate schema is actually77e75b7f… (verified bytes), identical to the preceding
accepted aggregate. Native schema file-map digests84c9…/6115… are not aggregate
hashes. No wire/schema version bump, broad descendant allowlist or fallback.
New negative vectors reject old3d8 source and oldd97 CLI independently; prior
mixed server/host/schema/unknown-field tests remain. Old markers/receipts are
not relabeled as new and cannot silently authorize the new bundle.

## Expected display / next manual step

v2 default policy emits no redundant terminal Notice. Explicit service_summary
retains its actual plain-text summary with original input reference. Native
compact tools retain action labels and bounded output previews. Adjacent proven
references may group; non-adjacent/already committed scrollback remains a
supplement. No agent message is reordered and no Job timing is forced to group.

This is not yet a VM update. Following acceptance, a separate Human preparation
task must select these bytes and perform a fresh review in a new private fixture.
Do not reuse or rewrite pv1/pi1/hj1 markers, processes, auth or histories.

## Evidence boundaries

Default locked offline build passed. Six launch::stock::tests passed including
paired pin negatives and untouched earlier contract behavior. The previous
21 template tests are reused from cc136169 (template code unchanged).
The composition harness reuses the proven private setup, real root API, native
protocol, trusted presentation controller and CLI PTY. The Job producer is the
dedicated authenticated completion fixture, not a Job daemon execution. MCP
evidence here is actual configured discovery with nonempty schemas, not another
model-selected tool invocation.

Native exact-byte p6/p7 evidence is reused for compact successful MCP rendering,
two actual approvals, canonical result preservation, adjacent grouping and
cross-page replay. Those native tests used synthetic Job receipts; they do not
prove real Job execution. Prior Human Job execution is separately retained at
the unchanged service boundary. No new full-stack paid-provider, Windows,
large-history, sandbox or restart-race acceptance claim.

All new fixtures are task-owned local PID/network namespaces with pre-connect
tripwire and fake Responses only. No host service, VM, native/Job source,
credential or canonical mutation. The unexplained pj07 timeout remains deferred
per Human; this task does not reproduce it. No cleanup of old evidence or VM
fixtures. Resource allowance is24GiB for this task, free>=100GiB.

## Final result

PASS: cv01 on the frozen default Cutex/facade bytes, exact final native CLI/server/
host and unchanged aggregate schema. Real root review/activation exact replay,
Ready generation1, actual configured `cutex` MCP connected with7 nonempty tool
schemas. Trusted controller rejects foreign generation. Standalone presentation
append/status/replay creates no native turn or fake model request. Dedicated
authenticated Job completion is admitted; actual input A4 and independent v2
`Job summary` receipt retain original reference and exact `SERVICE_DISPLAY_BODY`.
Exact producer replay does not add another carrier. Changing the local policy
to v2 default suppression for a NEW event leaves it without an obligation.
The actual CLI PTY shows the display text and exits0 with restored termios.

Independent `python3 -B tests/presentation_history.py cv01` reads persisted native
JSONL after owned fixture cleanup:2 presentation records (standalone + summary),
exactly1 original A4 commit, exactly1 matching display receipt, display-only
sentinel absent from all model requests. Only1 local fake Responses request.
No raw canonical model text is edited by this composition. New suppressed-event
acceptance/no extra display is observed; its separate completion ACK timing is
not a new gate asserted by this harness. Template ACK-independence unit coverage
is reused. No forced adjacent ordering or fresh compact-MCP invocation claim:
the exact native p6/p7 proofs above remain the evidence for those consumer gates.

Evidence: ../cv01/PASS.json, configured-mcp.json, terminal.pty, private native
history, ../display-v2-cv01.log; the process returned0 with no failed attempt in
this task. Prior failed attempts and pj07 risk remain, not erased by this pass.
Six pin tests, default build, helper compilation/syntax, fmt/diff checks passed;
full task-scoped diff self-reviewed. No dependency/lock change or feature build.

Frozen composed manifest SHA256:
d4c56c783fddf74ca2dd2721f40f13099ece9a7fbc77e78e42b4cb79208d1166.
Default Cutex00a7998a933d17132aacbf9a8fe798f80966e3adcf529798e42d4304a8631396;
facade4c3936b4eb6a7cda0203fbe31c7c62c4c81a639a979c1da1f07e5d14b3e69c83.
Final retained taskroot24,057,991,168 bytes (<24GiB), filesystem free376,866,217,984
bytes (>100GiB). Only this fixture's owned namespace/processes ended; VM/Human
owners and auth remain untouched. No old artifacts or caches deleted this task.
Intended private integration candidate, not production release or VM exposure.
