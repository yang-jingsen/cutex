# One bounded restart diagnostic — pd01

Result: NOT REPRODUCED. The single authorized local-fake diagnostic reached Ready, generation 2, with the same durable/native identity. No production repair was made. This does not resolve or invalidate the retained pj07 timeout.

Source: 7f89f0eef7023da93eef49dd30c0f7a593686af1; tree 981220ef9a78f554e38e83ad17f23a4c760368e9; parent 817f64190d730ca298c90aa6394974daffc8add3. Instrumentation is gated by stock-launch-test-hook and the private fixture guard. Default 5414 source bytes and earlier manifests remain unchanged.

Diagnostic binary manifest (feature build, NOT default acceptance):

- Cutex: ee887fe8bf432863f464f0d229a999859d8ab6a748448e97d325891b802101c2
- Facade: 047079cdeca014f51edfae27f4da71c75c2ffbbac19b26563f1da566b86a3d95
- Frozen location: ../artifacts/presentation-job-r1/diagnostic-bin/
- Native remains exact accepted 3d8a73a747cf5b957a7ca0491c28d1517f6d7722, as pinned by the existing presentation manifest.

Evidence: ../pd01/DIAGNOSTIC.json and ../presentation-diagnostic-pd01.log. One actual private Cutex/Bus/native fixture, one fake Responses request, no paid model or Job daemon campaign. Deadline: restart request 240 seconds, overall 900 seconds; no retry. Only owned fixture children were cleaned up. Human fixtures were untouched.

Observed timestamped chain (milliseconds): bridge unregister began 1789098234303 and returned 4310; bridge worker join ended 1789098235997; manager event join ended 1789098236000; stop group began 6000; stop commit began 6100. New runtime registration and enclosing operation completed at 1789098283515. Neither bridge join, event join nor process stop hung in this attempt.

Missing datum: the phase at which the original pj07 execution stopped progressing. Its uninstrumented timeout cannot identify that phase. This successful instrumented attempt cannot establish an intermittent deadlock, prove a timing fix, or justify skipping joins/removing fences. No production semantic repair is recommended from this evidence alone. A separately authorized targeted reproduction would be needed if Director requires closure of the intermittent timeout; do not repeat automatically.

Earlier partial results and omissions remain in durable-presentation-private-job-result.md. In particular this diagnostic is not a replacement for the missing full generation-race convergence acceptance. Prior S2/S46 disclosures, PID-time limitations, old-writer incompatibility and deployment exclusions remain.

Resource note: one precisely identified rebuildable incremental cache (cutex-0bynjc82zdkyp) was removed before this build; immutable source/artifacts/failure evidence retained. Retained task root approximately 21.316 GB, below 20 GiB; no additional build is planned.
