# S2 result — private stock outbound Cutex boundary

Intended use: INTEGRATION_CANDIDATE for a future lightweight line, subject to Director acceptance. Not deployed, not current-release integration, not full fork equivalence. The candidate commit containing this report is a direct descendant of base `2e0428fe7d903bf9034fd01d32e5fd8939429d6c` (tree `84f5d2f7df5f7a0125f74ff2a9e300743473e48e`); exact resulting identities accompany Task Service submission.

## Fixed inputs

- Stock 0.153.4, upstream `3d2ee51ca2d5db578f328aa75e20aa22c0197c9a`, tree `c527c7a5f5f199231dc0d819264e9e2180eb126a`.
- Official package SHA256 `a822187e1a2420c61c5926721bfbd878701ed95547c9bb0d4de4498a16ba1821`; executable SHA256 `56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da`, reused read-only from S1.
- Accepted S1 harness `d57cfe4f299b474ca8e6e0f235f62e687535532f`, tree `e5e066163f79a0a7732582b4c94a8af3f1447d68`; Job reference `36f8b577b5a067cbf5da4ddc10757ea60d306898` unchanged.

## Criteria and evidence

| Criterion | Result / boundary |
| --- | --- |
| Native thread, durable ID, runtime generation | Real stock app-server plus actual Cutex registration and persistent stores; no edited identity fixture or fake provider |
| Project authority | Real private Human import/Create/Add bootstrap; actual Management query and seat authorization; Human credential absent from facade |
| Independent security checks | Wrong token, spoofed runtime, foreign thread, missing/stale generation, ephemeral registration, Human route with Bus token and fenced close denied |
| Query replay / scope | Exact replay equal; changed-action scope conflict; fresh unauthorized project denied |
| Configured stock MCP | Eight fake Responses requests drive actual stock tools, real stdio facade and HTTP providers; caller-authority fields absent from schema and spoofed argument rejected |
| Outbound send | Real persisted exact durable sender/recipient, passive mode, exact replay same message/deduplicated; changed content same external ID distinct message, per native semantics |
| Offline / inbound | Ordinary offline send 409 verified; registered target succeeds. Three persisted records remain pending with no A4; no stock inbound claim |
| Same-owner CLI | Real Linux controlling PTY, Unix remote owner, visible response, exit 0, cooked-mode restoration, unchanged original turns and owner PID; no second writer or extra model request |
| Rotation fence | Actual successor registration increments generation; predecessor and stale generation denied |

Passing run: `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/run/PASS.json`; supporting bounded evidence includes `tool-outputs.json`, `inventory.json`, direct provider results and `cli.bin`. Stores remain private evidence. Fake Responses simulate model output only, not Cutex authority or persistence. No real Cutex production registration was exercised.

## Checks

All Cargo commands used scrubbed environment, private HOME/TMPDIR/CARGO_HOME/CARGO_TARGET_DIR, locked dependencies and read-only existing Rust toolchain. No Cargo manifest/lock change.

- Baseline `cargo build --locked --bin cutex`: passed.
- Candidate `cargo build --locked --bins` and `cargo check --locked --bins`: passed; existing eight binary warnings retained.
- `cargo test --locked --lib agent_bus::mcp::tests -- --test-threads=1`: 3 passed, run twice.
- `cargo test --locked --lib agent_bus::client::tests -- --test-threads=1`: 8 passed.
- `cargo test --locked --lib agent_management_sender_discards_project_chat_groups`: 1 passed. An earlier short-name `--exact` invocation selected zero and is not evidence.
- Total 12 unique Rust tests, 15 duplicate-inclusive passes before final documentation wording check; real harness fifth attempt passed. `cargo fmt --all -- --check`, `git diff --check` and Python syntax check passed.
- Full scoped source delta reviewed. Earlier compile errors and four probe failures retained: helper shadowing, offline-provider expectation, independent offline confirmation, changed-content dedupe expectation. Corrected harness expectations preserve native provider semantics, not a new policy. The final tool-description wording clarifies the bounded dedupe window without changing execution.

S1 OS sandbox and transport proofs reused, not rerun. No Windows, current managed runtime/cgroup full-stack, production credentials/auth exercise, deployment, native inbound/A4, or final combined release acceptance. The next minimal task is supported stock launch/registration configuration integration; inbound delivery remains a separate protocol decision.

## Resource and safety record

Task root `/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1`: approximately 3.2 GiB retained including private build/cache and preserved failed attempts. Mambo available space approximately 487 GiB before / 484 GiB after; free inodes 60,711,748 before / 60,691,043 after. Below 20 GiB added and above 100 GiB free. No kernel build, home cache, unrelated cleanup or source writes in S1/Job/release checkout.

Validation hygiene deviation: an overbroad final process-arguments cleanup check displayed an existing production Cutex service command-line credential in tool output. It was not used or copied into task files, reports or repository. Director was notified immediately without its value. Exact argument name/purpose are not asserted from retained context; no further secret inspection was performed. No rotation, restart or historical tool-output removal was attempted or claimed. Human owns any production credential response. Subsequent checks are restricted to owned child handles. Published materials must not contain that value.

Owned probe process groups are stopped by tracked handles. No live services, seats, runtime or release files were changed. Rollback before exposure: reject/revert this unexposed candidate; retained private evidence can remain. Native offline/dedupe limitations above are explicit, accepted adapter boundaries, not repaired backend guarantees.
