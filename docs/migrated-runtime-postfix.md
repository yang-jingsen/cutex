# Migrated runtime startup after service restart

Migrated explicit-launch agents use the regular TUI Start/Restart review and Run actions. A caller does not need to reconstruct the Job descriptor from migration scripts. When `review_runtime` omits `job_mcp`, Management reuses the Job configuration from the latest Ready runtime receipt for the same agent and launch contract, or from that contract's maintenance receipt. It discovers the current Unix-socket peer and creates a new review against the configured executable, bundle, and credential paths.

An explicit `job_mcp` overrides the saved configuration. Saved receipts are immutable evidence, not a requirement that a daemon survive reboot. Current occurrence discovery happens only during a new review; Run continues to validate the reviewed PID, birth marker, executable and custody objects to catch changes between confirmation and execution. A changed deployment configuration may still require an explicit descriptor.

Starting an offline agent is allowed with assigned work so it can resume that work. Restart still refuses unclosed assignments because it stops an existing process. Archived projects require restoration; unresolved launch claims require recovery/replay. Starting or attaching does not archive or rotate a Director; the current runtime guard already treats these separately.
