# Command Notes

Current command surface:

```sh
cutex
cutex --quick
cutex --host
cutex -- <cli args...>
cutex --host -- <cli args...>

cutex wizard
cutex config

cutex profile list
cutex profile show
cutex profile show <profile>
cutex profile edit
cutex profile edit <profile>
cutex profile use <profile>
cutex profile copy <profile> --name <new-name>
cutex profile copy <profile> --name <new-name> --provider <provider>
cutex profile copy <profile> --name <new-name> --provider-base-url <url>
cutex profile copy <profile> --name <new-name> --provider <provider> --provider-base-url <url>
cutex profile clone-status-line
cutex profile clone-status-line --from <profile>
cutex profile pin-top <profile>
cutex profile pin-bottom <profile>
cutex profile set <profile> --name <new-name>
cutex profile set <profile> --source <label> --plan <label> --email <label>
cutex profile set <profile> --clear-source --clear-plan --clear-email
cutex profile set <profile> --default-cli-args='--sandbox danger-full-access'
cutex profile set <profile> --clear-default-cli-args
cutex profile set <profile> --host
cutex profile set <profile> --docker-image <image> --docker-user-name <name>
cutex profile set <profile> --proxy-url <url> --proxy-no-proxy <list> --proxy-force-http true
cutex profile set <profile> --proxy-disable
cutex profile set <profile> --proxy-inherit

cutex run <profile>
cutex run <profile> --host
cutex run <profile> --docker-image <image> --docker-user-name <name>
cutex run <profile> -- <cli args...>

cutex login
cutex login --name <name>
cutex login --name <name> --api-key <key> --base-url <url> --provider <provider>

cutex global show
cutex global edit
cutex global set --docker-use-sudo true
cutex global set --session-enable false
cutex global set --default-profile work
cutex global set --clear-default-profile
cutex global set --default-profile-direct-launch true
cutex global set --proxy-url <url>
cutex global set --proxy-url <url> --proxy-no-proxy <list> --proxy-force-http true
cutex global set --proxy-clear
cutex global set --docker-use-sudo false --proxy-url <url> --proxy-no-proxy <list> --proxy-force-http true
cutex global set --notify-idle-timeout <seconds> --notify-composer-idle-timeout <seconds> --notify-approval-timeout <seconds>
cutex global set --notify-startup-idle-timeout <seconds>
cutex global set --notify-events <csv>
cutex global set --notify-user-message-content none|preview|full
cutex global set --notify-user-message-preview-chars <chars>
cutex global set --rate-limit-threshold-warning-mode off|daily|always
cutex global set --rate-limit-model-nudge-mode off|daily|always

cutex notify desktop status
cutex notify desktop start
cutex notify desktop start --port 24250
cutex notify desktop enable
cutex notify desktop enable --port 24250
cutex notify desktop disable
cutex notify desktop test
cutex notify desktop install-ubuntu
cutex notify desktop install-ubuntu --port 24250
cutex notify desktop uninstall-ubuntu

cutex session list
cutex ss list
cutex session attach --name <name>
cutex ss attach --name <name>
```

Runtime behavior:

- `cutex` now launches the selected CLI directly. It no longer wraps launches in `tmux` or `zellij`.
- plain interactive launches can be wrapped in `cute-alden` when managed sessions are enabled so `cute-codex` can be detached and reattached
- plain `cutex` can launch directly into the configured fallback profile when `default-profile-direct-launch` is enabled
- Selection order is `CUTEX_CODEX_BIN` / `CODEZ_CODEX_BIN`, then `cute-codex`, then `cutex-codex`, then `codex`.
- `cutex` prints `CLI binary: ...` before each launch so fallback is obvious.
- `cutex` also prints a launch summary (`profile/runtime/proxy/provider/api/tool_proxy`) before starting the selected CLI.
- explicit `cutex session ...` / `cutex ss ...` commands manage live `cute-alden` sessions directly.
- `cutex session list` prints `PID<TAB>NAME`, matching `cute-alden --list`.
- `cutex runtime <profile> --host` rewrites the stored runtime for that profile.
- `cutex runtime <profile> --docker-image <image> --docker-user-name <name>` rewrites the stored runtime for that profile.
- `cutex profile set <profile> --host|--docker-image ...` is the unified runtime edit path.
- `cutex --host` and `cutex run <profile> --host` only affect the current invocation.

Profile display metadata:

- `cutex annotate <profile> --source <label>` overrides the displayed source/provider label.
- `cutex annotate <profile> --plan <label>` overrides the displayed plan label.
- `cutex annotate <profile> --email <label>` overrides the displayed email label.
- `cutex profile set <profile> ...` can update name/metadata/runtime/proxy in one command.
- `cutex profile set <profile> --session-enable|--session-disable|--session-inherit` overrides the managed-session default for one profile.
- `cutex profile set <profile> --default-cli-args='...'` stores per-profile startup args that are prepended to every launch of that profile.
- Source/plan/email are display-only. Runtime/proxy/session/default-cli-args change launch behavior.

Unified profile workflow:

- `cutex profile list` shows source/plan/runtime/proxy scope/provider/status-line count for all profiles.
- `cutex profile list` also includes resolved provider API base address (`api=`).
- `cutex profile show [profile]` shows full per-profile details including `Provider` and `ApiBase`; default target is active profile.
- `cutex profile copy <profile> --name <new-name>` clones auth/runtime/proxy/metadata and profile files into a new profile.
- `cutex profile copy ... --provider <id>` changes the copied profile's `model_provider`.
- `cutex profile copy ... --provider-base-url <url>` changes the copied provider's `base_url`; if `--provider` is omitted, cutex updates the source profile's current provider.
- `cutex profile set` is the consolidated edit command for profile-level settings.
- `cutex profile edit [profile]` is the interactive profile wizard. Boolean rows display as `[x]` / `[ ]`; text rows prompt for a replacement value, and `-` clears optional fields.
- `cutex profile clone-status-line [--from <profile>]` copies one profile's `[tui].status_line` to all profiles (default source is the active profile).
- `cutex profile pin-top <profile>` and `cutex profile pin-bottom <profile>` reorder list/select display order.
- Hidden legacy commands (`list/current/use/add/rename/remove/annotate/runtime/proxy`) remain accepted, but new workflows should use `profile`, `global`, and `wizard`.

Codex argument passthrough:

- Use `--` before Codex arguments in no-subcommand mode: `cutex -- --help`.
- Use `--` before Codex arguments in explicit run mode: `cutex run <profile> -- --help`.

Persistent config:

- Preferred unified entry is `cutex global`:
  - `cutex global show`
  - `cutex global edit`
  - `cutex global set --docker-use-sudo true|false`
  - `cutex global set --session-enable true|false`
  - `cutex global set --default-profile <profile>|--clear-default-profile`
  - `cutex global set --default-profile-direct-launch true|false`
  - `cutex global set --proxy-url <url> [--proxy-no-proxy <list>] [--proxy-force-http true|false]`
  - `cutex global set --proxy-clear`
  - `cutex global set --notify-idle-timeout <seconds> --notify-composer-idle-timeout <seconds> --notify-approval-timeout <seconds>`
  - `cutex global set --notify-events <csv> --notify-user-message-content none|preview|full --notify-user-message-preview-chars <chars>`
- `cutex wizard` opens the main interactive configuration wizard.
- `cutex config` is an alias for `cutex wizard`; it does not take `set/show/edit` subcommands.
- `cutex global edit` opens the global settings wizard. Use this wizard for fields that intentionally do not have a non-interactive setter, including external notify service URL/token.
- `cutex global set --notify-idle-timeout <seconds>` sets the short unchanged-composer `task_completed` idle timeout injected as `CODEX_NOTIFY_IDLE_TIMEOUT`.
- `cutex global set --notify-composer-idle-timeout <seconds>` sets the long composer idle timeout injected as `CODEX_NOTIFY_COMPOSER_IDLE_TIMEOUT`.
- `cutex global set --notify-approval-timeout <seconds>` sets the delayed approval notify timeout injected as `CODEX_NOTIFY_APPROVAL_TIMEOUT`.
- `cutex global set --notify-startup-idle-timeout <seconds>` sets the startup idle timeout injected as `CODEX_NOTIFY_STARTUP_IDLE_TIMEOUT`.
- `cutex global set --notify-events <csv>` sets the `cute-codex` notify allowlist injected as `CODEX_NOTIFY_EVENTS`.
- `cutex global set --notify-user-message-content none|preview|full` controls whether opt-in `user_message_sent` includes no text, a bounded preview, or full text. Injected as `CODEX_NOTIFY_USER_MESSAGE_CONTENT`.
- `cutex global set --notify-user-message-preview-chars <chars>` sets preview length for `user_message_sent`, injected as `CODEX_NOTIFY_USER_MESSAGE_PREVIEW_CHARS`.
- Managed `cute-alden` sessions keep the environment from the process that originally started them. After changing notify config or env overrides, start a fresh session before testing the new values.
- `~/.cutex/config.json` also stores the reusable `custom_status_items` catalog for `cute-codex`.
- `~/.cutex/config.json` can store a global proxy fallback used by profiles without an override.
- Built-in default is `docker-use-sudo=false`, `session=disabled`, `default-profile-direct-launch=false`, and no proxy.
- `~/.cutex/accounts.json` is now a lightweight profile index (name/runtime/proxy/metadata/order only).
- Per-profile auth/config live only in `~/.cutex/profiles/<account-id>/auth.json` and `config.toml`.
- Existing v2 stores are migrated once to v3, with backup written to `~/.cutex/accounts.v2.backup.json`.

Proxy configuration:

- `cutex proxy set socks5h://127.0.0.1:7890` sets a global proxy inherited by all profiles.
- `cutex proxy set-profile <profile> socks5h://127.0.0.1:7890` sets a profile-specific override.
- `cutex proxy disable-profile <profile>` makes one profile explicitly clear proxy env vars instead of inheriting the global proxy.
- `cutex proxy clear-profile <profile>` removes that override so the profile inherits the global setting again.
- Supported schemes are `http`, `https`, `socks5`, `socks5h`, `socks4`, and `socks4a`.
- Use `socks5h://...` when DNS resolution should happen through the proxy instead of on the local machine.
- Proxy env vars are injected into host and Docker launches. `cutex login --name <name>` uses the global proxy because the profile does not exist yet.
- When proxy is enabled, `cutex` also writes `shell_environment_policy.exclude` so common tool subprocesses (`curl`, `python`, `pip`, `npm`, `cargo`, etc.) do not inherit proxy env vars by default.
- For SOCKS proxies, `cutex` sets `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` (and lowercase variants) to the SOCKS URL so auth/provider clients that only check one family still route through the proxy.
- For Docker runtime, loopback proxy hosts (`127.0.0.1` / `localhost` / `::1`) are rewritten to `host.docker.internal`, and `docker run` includes `--add-host host.docker.internal:host-gateway`.
- `--force-http` defaults to `true`. It sets `CUTE_CODEX_FORCE_HTTP_TRANSPORT=1`, causing `cute-codex` provider traffic to use HTTP/SSE instead of Responses WebSocket transport.
- Realtime conversation transport is unavailable while forced HTTP mode is active.
- If you deliberately want WebSocket transport with a proxy, use `--force-http false`; this is not recommended for SOCKS privacy because the WebSocket path is not yet explicitly proxy-controlled.

Desktop notification bridge:

- Desktop notifications are independent from the external notify service and can be enabled/disabled separately.
- `cutex notify desktop enable [--port 24xxx]` enables the native Linux notification bridge, starts it if needed, and injects its URL into `cute-codex` launches.
- `cutex notify desktop disable` stops future launch injection without clearing the external notify service.
- `cutex notify desktop start [--port 24xxx]` starts the shared local bridge without changing the enabled flag.
- `cutex notify desktop install-ubuntu [--port 24xxx]` installs a user-level systemd service for Kubuntu/Ubuntu.
- Ports must be in the Bridgeboard 24xxx range; the built-in default is `24250`.
- When `bridgeboard` is available, the bridge registers handoff id `cutex-desktop-notify`.

Status-line customization:

- Catalog entries live in `~/.cutex/config.json` under `custom_status_items`.
- Per-profile selection and ordering live in `~/.cutex/profiles/<account-id>/config.toml` under `[tui].status_line`.
- `cutex` materializes a per-profile `custom-status-items.json` and passes it to `cute-codex` through `CODEX_CUSTOM_STATUS_ITEMS_FILE`.

Compatibility:

- The old environment variable names `CODEZ_CODEX_BIN` and `CODEZ_DOCKER_USE_SUDO` are still accepted.
- `cutex config` is a convenience alias for `cutex wizard`.
- Hidden legacy CLI commands remain accepted for old shell habits, but they are not the preferred command surface.
