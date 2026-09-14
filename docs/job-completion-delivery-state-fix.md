# Job completion state and custom presentation repair

2026-09-14. Job source `cdac8c5`; optimized deployment `release-job-delivery-r1`. Cutex CLI remains r41; cute-codex remains native r11.

## Corrected contract

`completionDelivery.enabled` reports whether the Job daemon has a configured completion worker. With it enabled, a nonterminal job reports `awaiting_terminal` and a terminal event initially reports `ready`. `disabled` is reserved for absent completion configuration. Later states continue to describe retries, receiver acceptance and delivery. MCP discovery/tool descriptions explain this and that read_output/query do not consume notifications. Delivery remains after-turn; delivered is durable runtime acknowledgement, not human consumption.

## Presentation

The PRH Job definition now uses `--completion-v2`. New terminal events freeze the action ID, exit code, execution timestamps and output summaries consumed by the existing Cutex bridge and cute-codex custom Job view. Exit code 0 maps to Job completed; unknown exit status is never assumed successful. Existing theme colors and first-line event time apply. No cute-codex source changes or native-owner restarts were needed. Historical v1 events are not rewritten or replayed.

## Deployment and evidence

The idle daemon was replaced under PRH; both original jobs and both acknowledged outbox receipts were preserved. Only the new enabled field was added to their delivery summaries. The selected local deployment's Job descriptor now identifies the new adapter/daemon; existing MCP adapters remain usable with the unchanged core protocol, and new launches use the selected descriptor. All 11 allowed launchers were retained. Eight native owner/host processes remain running.

Tests: 23 Job library/core/completion tests, 5 Cutex Job bridge/fact tests, and 4 native Job/custom-view tests passed. The installed new adapter's initialize/tools-list descriptions and live daemon capabilities were checked; persisted receipts remained delivered. No production test job or agent message was submitted. Deployment metadata and private backups are local in `release-job-delivery-r1/`, not in Git.
