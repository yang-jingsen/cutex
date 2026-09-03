# Agent Usage Accounting Plan

## Objective

Add ccusage/sub2api-style usage accounting to Cutex so an operator can identify
which durable Cutex agents, profiles, and models account for observed token
activity over time. Preserve objective native counts first, then derive clearly
versioned Codex-rate-card credits and API-equivalent cost estimates.

Acceptance for the first candidate: after Cutex observes native app-server
usage events, `cutex usage` reports replay-safe per-agent token totals and time
series grouped by hour, day, ISO week, or observed reset period without reading
prompts, credentials, or raw conversation history.

## Semantics

- `thread/tokenUsage/updated` is the primary source. A reducer tracks the
  cumulative `total` counter per `(cutex_session_id, thread_id)`.
- A first snapshot establishes a baseline. Resume/fork history is not charged
  to the current observation period.
- When the cumulative detailed fields advance, prefer `last`; fall back to a
  non-negative cumulative difference. An unchanged cumulative snapshot is a
  duplicate even when Codex repeats a non-zero `last` after a rate-limit-only
  update.
- A change only to `totalTokens`, with no detailed token movement, is treated
  as a synthetic context estimate and is not charged.
- Counter regression or an inconsistent snapshot starts a new baseline rather
  than inventing usage.
- `thread/settings/updated` and `model/rerouted` provide model attribution.
  The runtime binding's immutable `launched_profile` provides profile
  attribution; the current global default is never substituted for legacy
  unknown bindings.
- `account/rateLimits/updated` contributes only objective reset boundaries
  (`resetsAt` and `windowDurationMins`) for time grouping. `usedPercent` is not
  stored, correlated, apportioned, or estimated.
- Derived credits use an identified, versioned public Codex token rate card.
  Derived USD is an API-equivalent estimate, not a subscription bill. Raw token
  records remain authoritative if either price table changes.

## Storage

Use a private, host-local projection below `~/.cutex/runtime/management-v2`:

- an atomically replaced cumulative baseline per durable Cutex session and
  native thread;
- append-only monthly JSONL ledger files with one timestamped record per valid
  native usage delta;
- reset-boundary records only when a profile/limit period changes;
- a recoverable pending-entry transaction so a process crash cannot silently
  lose a record; duplicate ledger lines are harmless by stable event id;
- model, provider, reasoning effort, service tier, immutable launched profile,
  and independent projection revision.

The projection stores only identifiers, attribution labels, timestamps, and
counts. It stores no prompt, response, API key, OAuth token, full base URL, or
transcript. It is not part of `cutex-sessions.json`, durable revision,
lifecycle, or runtime fencing.

## Stages

| Stage | Status | Deliverable | Validation |
| --- | --- | --- | --- |
| 1. Reducer and ledger | Implemented | Replay-safe token reducer, attribution updates, reset boundaries, recoverable append-only time ledger, private atomic state | `cargo test --locked management::v2::usage` (10 passed) |
| 2. CLI report | Implemented | `cutex usage` grouped by agent/profile/model and hour/day/week/reset with text and JSON output, coverage timestamps, token mix, versioned Codex-credit and API-equivalent estimates | `cargo test --locked usage` (21 passed) plus empty-ledger CLI smoke |
| 3. Runtime integration | Implemented | Immutable launched profile reaches the derived recorder after canonical append; failure warns without disconnecting | Manager `6`, runtime `12`, and full locked `276/334/0` passed |
| 4. Read surfaces | Deferred | Local text/JSON CLI is sufficient for this source candidate; typed Management/tethysUNE aggregation and a dedicated TUI view require a later contract | No main-list column or Management schema change |
| 5. Platform acceptance | Artifact built | Linux release built and Windows GNU compile-check passed | Deployment and live smoke require separate approval and rollback receipt |

## Non-goals For The First Candidate

- storing, correlating, or estimating ChatGPT weekly `usedPercent`;
- scraping or persisting credentials;
- provider-specific billing for DeepSeek or API-key profiles;
- scanning all Codex history as an implicit migration;
- treating a price-derived credit/USD value as more authoritative than native
  token counts;
- cross-host aggregation inside a host-local file;
- changing native cute-codex, Management v2 lifecycle, Agent Bus routing, or
  tethysUNE.

## Future Integration

A Management read projection can expose host-local report rows without making
usage a session lifecycle field. tethysUNE can then aggregate explicit host
responses. A later TUI view can graph the same hour/day/week/reset aggregates;
the main Agent list does not need another column.

## Current Stage 3 Plan

Goal: feed the derived ledger from each managed app-server occurrence while
preserving canonical event publication and runtime connectivity.

| Task | Status | Notes |
| --- | --- | --- |
| Carry immutable launch profile | Completed | Event context copies `binding.launched_profile`; legacy unknown remains unknown |
| Record after canonical append | Completed | Projection failures warn and do not disconnect the managed runtime |
| Focused validation | Completed | Manager event forwarding and all usage tests pass |
| Final candidate gates | Completed | Full locked suite, exact baseline Clippy, release build/hash, Windows GNU check, and required project logs passed |

## Final Candidate Evidence

Executable source checkpoint: `457efcc50f5325c80b6b099388c9ea4a9c4f54dc`.

- `cargo test --locked usage`: 18 library and 3 binary tests passed.
- `cargo test --locked app_server::manager`: 6 passed.
- `cargo test --locked cli_app::app_server_runtime`: 12 passed.
- `cargo test -q --locked`: 276 library, 334 binary, and 0 doc failures.
- `cargo clippy --all-targets --locked --message-format short`: exact existing
  baseline, `21/24` library and `4/26` binary; no usage lint remains.
- `cargo check --locked --target x86_64-pc-windows-gnu`: passed with only the
  existing conditional unused-import warnings.
- `cargo build --release --locked`: passed; SHA-256
  `1e48a6c6cf1233d6e965bf4a7f0219348306fd41541b8870407062f99240a41a`.
- Changed-file rustfmt and `git diff --check`: passed.
- No merge, deployment, restart, Windows release build, or live-session
  mutation occurred.
