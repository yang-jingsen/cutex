# cutex

`cutex` is a profile launcher and configuration wizard for `cute-codex`.
It keeps account credentials, profile configuration, runtime options, proxy
settings, notification settings, and status-line preferences in one local store,
then starts the selected CLI with the right environment.

## Features

- Multiple profiles for official ChatGPT auth or API-key providers.
- Interactive profile and global configuration wizards.
- Host or Docker runtime per profile, with one-shot `--host` overrides.
- Per-profile startup arguments, such as a default sandbox mode.
- Optional proxy injection for auth and model-provider traffic.
- Shared local desktop notification bridge for Linux/Kubuntu.
- Optional managed detachable sessions through `cute-alden`.
- Per-profile `cute-codex` status-line configuration.
- Opt-in agent-to-agent messaging: one `cute-codex` session can discover and
  message another local `cutex --agent` session.

## Install

Build from source:

```sh
cargo build --release
```

The binary is written to:

```sh
target/release/cutex
```

Put that binary on `PATH`. `cutex` resolves the Codex-compatible CLI in this
order:

1. `CUTEX_CODEX_BIN`
2. `CODEZ_CODEX_BIN`
3. `cute-codex`
4. `cutex-codex`
5. `codex`

For normal use, install `cute-codex` on `PATH` or set `CUTEX_CODEX_BIN` to its
absolute path.

## Quick Start

Create a profile:

```sh
cutex login
```

Open the main wizard:

```sh
cutex wizard
```

`cutex config` is an alias for `cutex wizard`.

List and inspect profiles:

```sh
cutex profile list
cutex profile show
cutex profile show <profile>
```

Run a profile:

```sh
cutex run <profile>
```

Pass arguments to `cute-codex`:

```sh
cutex run <profile> -- --help
cutex -- --help
```

Set a global default profile:

```sh
cutex global set --default-profile <profile>
cutex global set --default-profile-direct-launch true
```

## Agent Messaging

`cutex` can run a shared local agent bus so multiple `cute-codex` sessions can
talk to each other. This is opt-in per launch; plain `cutex` keeps the native
solo behavior and does not expose agent tools to the model.

Start a collaborating session:

```sh
cutex --agent
cutex run <profile> --agent
```

List peers and send a message:

```sh
cutex agent list
cutex agent send <agent-name-or-id> "please report status"
```

Normal sends wake the recipient so it can answer promptly. Use `--queue-only`
only for FYI or heartbeat messages that should be delivered without starting a
new turn:

```sh
cutex agent send <agent-name-or-id> "FYI: transfer is still running" --queue-only
```

In collaboration mode, patched `cute-codex` also exposes native model tools
(`cutex_agent_list` and `cutex_agent_send`), so agents can message peers without
shelling out. Incoming messages are shown in the target TUI history and are
injected into model context once through mailbox delivery.

## Profiles

Profiles are stored under `~/.cutex/profiles/<profile-id>/`. Each profile can
have its own auth file, config file, runtime, proxy behavior, display metadata,
status line, and default CLI arguments.

Common commands:

```sh
cutex profile edit
cutex profile edit <profile>
cutex profile use <profile>
cutex profile set <profile> --default-cli-args='--sandbox danger-full-access'
cutex profile set <profile> --clear-default-cli-args
cutex profile set <profile> --host
cutex profile set <profile> --docker-image <image> --docker-user-name <name>
```

Copy a profile and optionally change its provider:

```sh
cutex profile copy <profile> --name <new-name>
cutex profile copy <profile> --name <new-name> --provider <provider>
cutex profile copy <profile> --name <new-name> --provider-base-url <url>
```

## Global Settings

Show or edit global settings:

```sh
cutex global show
cutex global edit
```

Useful non-interactive settings:

```sh
cutex global set --session-enable false
cutex global set --docker-use-sudo false
cutex global set --proxy-url <url>
cutex global set --proxy-clear
cutex global set --notify-idle-timeout <seconds>
cutex global set --notify-composer-idle-timeout <seconds>
cutex global set --notify-approval-timeout <seconds>
cutex global set --notify-events <csv>
cutex global set --rate-limit-threshold-warning-mode off|daily|always
cutex global set --rate-limit-model-nudge-mode off|daily|always
```

Some sensitive values, including external notification service URL/token, are
edited through the interactive global wizard rather than documented as command
line examples.

## Desktop Notifications

`cutex` can run a shared local bridge that receives `cute-codex` notification
payloads and forwards them to the native Linux desktop notification system.
This is separate from any external notification service.

```sh
cutex notify desktop enable --port 24250
cutex notify desktop status
cutex notify desktop test "cutex desktop test"
```

For Ubuntu/Kubuntu systemd user service installation:

```sh
cutex notify desktop install-ubuntu --port 24250
systemctl --user status cutex-desktop-notify.service
```

Disable desktop notification injection:

```sh
cutex notify desktop disable
```

Remove the systemd user service:

```sh
cutex notify desktop uninstall-ubuntu
```

Ports must be in the Bridgeboard `24xxx` range. The default is `24250`.

## Docker Runtime

Profiles can launch inside Docker while keeping the current project path stable:

```sh
cutex profile set <profile> --docker-image <image> --docker-user-name <name>
cutex run <profile>
```

Force a Docker-configured profile onto the host for one invocation:

```sh
cutex run <profile> --host
cutex --host
```

See `docs/docker-image-notes.md` for image requirements and environment details.

## Configuration Files

Primary files:

- `~/.cutex/accounts.json`: profile index and metadata.
- `~/.cutex/config.json`: global settings and shared status-line catalog.
- `~/.cutex/profiles/<profile-id>/auth.json`: per-profile Codex auth.
- `~/.cutex/profiles/<profile-id>/config.toml`: per-profile Codex config.

These files may contain credentials or account metadata. Do not publish them.

## Development

Run tests:

```sh
cargo test -- --test-threads=1
```

Format:

```sh
cargo fmt
```

Build:

```sh
cargo build --release
```

More command details are in `docs/commands.md`.
