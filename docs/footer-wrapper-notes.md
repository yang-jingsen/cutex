# Status-Line Notes

`cutex` no longer manages any external footer wrapper or terminal multiplexer.

Current model:

- `cutex` launches the selected CLI directly.
- Selection order is `CUTEX_CODEX_BIN`, then `cute-codex`, then `cutex-codex`, then `codex`.
- `cutex` prints `CLI binary: ...` before each launch.
- Native status-line rendering lives inside `cute-codex`.
- Reusable custom item definitions live in `~/.cutex/config.json`.
- Per-profile item selection and ordering live in `~/.cutex/profiles/<account-id>/config.toml`.
- `cutex` materializes `custom-status-items.json` per profile and passes it through `CODEX_CUSTOM_STATUS_ITEMS_FILE`.

This replaced the older `tmux`/`zellij` experiments because those wrappers complicated scrolling, copy behavior, and shutdown handling.
