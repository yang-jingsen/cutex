# Change Logs

- 2026-04-14 `v18` `2026-04-14-zellij-no-alt-v18`
  - In zellij mode, auto-add `codex --no-alt-screen` to preserve scrollback.

- 2026-04-14 `v19` `2026-04-14-zellij-no-alt-no-mouse-v19`
  - In zellij config, set `mouse_mode false` to reduce wheel capture by zellij/apps.

- 2026-04-14 `v20` `2026-04-14-docker-no-bwrap-v20`
  - For Docker runtime, auto-add `codex --sandbox danger-full-access` unless the user already set a sandbox option.
  - Keep zellij inline mode path and compact bottom bar layout.

- 2026-04-14 `v21` `2026-04-14-profile-context-v21`
  - Preserve `model_context_window` and `model_auto_compact_token_limit` in imported profile `config.toml` data.
  - Write those keys back when switching accounts so per-profile context limits survive round-trips.

- 2026-04-14 `planning`
  - Added a TODO in `codez/log.md` for auto-saving/binding a profile after successful login/import.

- 2026-04-14 `planning`
  - Added a TODO in `codez/log.md` for a future Docker mode that keeps the temporary container/environment alive instead of cleaning it up immediately.

- 2026-04-14 `docker`
  - Added `images/codez-codex-dev/Dockerfile` for a Codex development image that keeps the existing codez toolchain but adds missing native build dependencies for compiling upstream `codex`.

- 2026-04-15 `v22` `2026-04-15-launch-status-env-v22`
  - Inject `CODEX_LAUNCH_PROFILE` and `CODEX_LAUNCH_RUNTIME` when `codez` launches `codex` on the host.
  - Pass the same launcher metadata through Docker via `docker run -e ...` so upstream `codex` can render them inside the container too.
  - Bumped the visible build marker so manual testing can confirm the new binary is in use.

- 2026-04-15 `v23` `2026-04-15-profile-statusline-v23`
  - Preserve `tui.status_line` in imported profile `config.toml` data.
  - Write `tui.status_line` back when switching accounts so profile-specific native status-line choices survive round-trips.

- 2026-04-15 `v24` `2026-04-15-account-paths-v24`
  - Stop rewriting the shared `CODEX_HOME/auth.json` and `CODEX_HOME/config.toml` on account switch.
  - Materialize per-account files under `~/.codez-cli/profiles/<account-id>/`.
  - Launch `codex` with `CODEX_AUTH_FILE` and `CODEX_CONFIG_FILE` so each account can keep an isolated auth/config pair while still sharing the same Codex home for sessions and history.
  - Support `CODEZ_CODEX_BIN` so `codez` can launch a patched local `codex` binary instead of the one from `PATH`.
  - Add `codez/scripts/migrate_legacy_accounts.py` to convert legacy `accounts.json` entries into the new per-account file layout without mutating the original store.

- 2026-04-15 `v25` `2026-04-15-home-layout-v25`
  - Move the host-side shared Codex home from `~/.codex-codez` to `~/.codez-cli/codex-home`.
  - Move the Docker runtime home mount from `~/.codez-cli/runtime/thirdparty/userhome` to `~/.codez-cli/runtime/docker-home`.
  - Automatically migrate those legacy directories to the new paths on first use when the new target path does not already exist.
  - Move temporary `codez login` homes under `~/.codez-cli/runtime/login/` so `codez` no longer leaves login-specific temp directories directly under `$HOME`.

- 2026-04-15 `v27` `2026-04-15-cutex-brand-v27`
  - Finish the launcher-side `cutex` rebrand while keeping legacy `CODEZ_*` environment variables as compatibility fallbacks.
  - Prefer `cutex-codex` automatically when it exists on `PATH`, but still fall back to `codex`.
  - Rename temporary tmux/zellij session labels and wrapped-command error messages from `codez` to `cutex`.
  - Update command/docs examples to use `cutex` and the new `CUTEX_*` environment variable names.

- 2026-04-15 `docker`
  - Change `images/codex-base/Dockerfile` to build `cutex-codex` from the local `codex/codex-rs` checkout instead of installing the npm `@openai/codex` package.
  - Expose `/usr/local/bin/codex` as a symlink to the bundled `cutex-codex` binary so existing Docker launch paths still work.
  - Add a root `.dockerignore` so repository-root Docker builds do not upload `target/` and `target-host/` artifacts into the build context.
  - Update the Docker docs to use `docker build -f images/codex-base/Dockerfile ... .` instead of building from the image subdirectory alone.

- 2026-04-15 `v28` `2026-04-15-cute-codex-v28`
  - Rename the patched Codex binary from `cutex-codex` to `cute-codex` for all current user-facing CLI text and build targets.
  - Make `cutex` prefer `cute-codex` on `PATH`, while still falling back to legacy `cutex-codex` and then plain `codex`.
  - Update the runtime Docker image to build `cute-codex`, while keeping compatibility symlinks for `cutex-codex` and `codex`.
  - Update Docker docs/examples to use the clearer image tag `cute-codex-base`.

- 2026-04-15 `docker`
  - Add `images/cute-codex-builder/Dockerfile` as a dedicated builder image for compiling `cute-codex`.
  - Keep that builder image free of any preinstalled `codex` or `cute-codex` binary so the compile pipeline cannot form a circular dependency.
  - Pin the Rust toolchain to `1.93.0` to match `codex/codex-rs/rust-toolchain.toml`.
  - Include `cmake` and `procps` alongside the existing native build dependencies so compilation and process inspection work inside the builder container.

- 2026-04-15 `unreleased` `custom-statusline-framework`
  - Let `cutex` materialize a per-profile `custom-status-items.json` catalog and pass its path through `CODEX_CUSTOM_STATUS_ITEMS_FILE`.
  - Add launcher-side config support for a reusable custom status-line item catalog so new custom items, labels, colors, and data sources can be changed without recompiling `cute-codex`.
  - Teach `cute-codex` to load that catalog at startup, expose custom items alongside built-in status-line items, and keep selection/order in `tui.status_line`.
  - Render custom items with native `ratatui` styling instead of ANSI escape sequences embedded in strings.
  - Cover the new path with tests for the picker preview flow and a `ChatWidget` regression test that loads a catalog item from `CODEX_CUSTOM_STATUS_ITEMS_FILE`.

- 2026-04-15 `v29` `2026-04-15-cutex-config-v29`
  - Move the host-side cutex data root from `~/.codez-cli` to `~/.cutex`, with automatic migration from the legacy root on first run.
  - Keep per-profile `config.toml` files under `~/.cutex/profiles/<account-id>/` as the durable source of truth so manual per-account edits are no longer overwritten on activation.
  - Sync edited per-profile config back into the store on activation so launcher metadata and future launches see updated profile-scoped settings.
  - Print the `cutex build` banner before profile selection / launch setup instead of only after the runtime handoff begins.
  - Add new custom status-line launch sources for `launch_profile_source`, `launch_profile_type`, and `launch_profile_email` alongside the existing profile/runtime items.
  - Make `cutex current` print the active profile id and resolved per-profile `config.toml` path for easier manual edits.

- 2026-04-15 `v30` `2026-04-15-cute-codex-build-tag-v30`
  - Add a compile-time `CUTE_CODEX_BUILD_TAG` hook for `cute-codex` so local builds can append a short manual marker like `FT` or `FT-D` in the TUI header.
  - Keep update checks and semantic version handling on the real package version (`0.120.0` etc.) while only changing the displayed header label.
  - Render the tagged version label consistently in both the startup session header and the `/status` card header.

- 2026-04-15 `docker`
  - Change `images/codex-base/Dockerfile` to copy the prebuilt Docker-targeted `codex/codex-rs/target-docker/release/cute-codex` directly into `/usr/local/bin/cute-codex`.
  - Change `images/codez-codex-dev/Dockerfile` to do the same, while keeping the Rust toolchain and zellij installed for development work.
  - Stop recompiling `cute-codex` inside runtime image builds and stop installing the official npm `codex` package in the dev image.

- 2026-04-15 `v31` `2026-04-15-cutex-direct-v31`
  - Remove `tmux`/`zellij` launch support from `cutex` and always launch `cute-codex` directly.
  - Simplify persistent config down to the remaining meaningful default: `docker-use-sudo`.
  - Rename the Docker-internal mounted profile directory from `.codez-profiles` to `.cutex-profiles`.
  - Stop hard-failing startup on legacy Docker home migration; direct-launch commands can start even if an old `~/.cutex/runtime/thirdparty/userhome` path still exists and cannot be renamed.
  - Rewrite the command and Docker docs around the direct-launch model and native `cute-codex` status-line customization.

- 2026-04-16 `v32` `2026-04-16-proxy-v32`
  - Add `cutex proxy` subcommands for global and per-profile proxy control: `show`, `set`, `clear`, `set-profile`, `disable-profile`, and `clear-profile`.
  - Store proxy settings both globally in `~/.cutex/config.json` and optionally per profile in `accounts.json`, with per-profile override precedence.
  - Inject proxy environment variables into host launches, Docker launches, and `cutex login`, including SOCKS-aware handling that keeps `HTTP_PROXY` / `HTTPS_PROXY` empty when the proxy URL is `socks*`.
  - Add `CUTE_CODEX_FORCE_HTTP_TRANSPORT` launch control so proxied `cute-codex` sessions can force provider traffic onto HTTP/SSE.
  - Update command and Docker docs with the supported proxy schemes, `socks5h` DNS guidance, and the forced-HTTP transport caveat.

- 2026-04-20 `v35` `2026-04-20-tool-proxy-v35`
  - Expand tool-side proxy exclusion so proxied sessions keep provider/auth traffic on the proxy while `curl`, `python`, `pip`, `npm`, and similar shell tools default back to direct connections.
  - Add `PIP_PROXY` to the managed exclusion set and keep the existing case-insensitive shell environment filtering path.
  - Cover the materialized profile-config path with a test so `shell_environment_policy.exclude` is verified after real profile file generation, not just merge helpers.
