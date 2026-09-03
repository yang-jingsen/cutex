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
cutex run <profile> --agent --group aria example-project -- --sandbox danger-full-access
```

`--group` scopes collaboration visibility. It accepts multiple values after one
flag and can also be repeated. When no explicit group is supplied, cutex adds a
project-local default group so agents in the same project can see each other
without cluttering unrelated projects.

List peers and send a message:

```sh
cutex agent list
cutex agent send <agent-name-or-id> "please report status"
cutex agent groups add <agent-name-or-session-id> shared-review
cutex agent groups set <agent-name-or-session-id> aria example-project
```

Normal sends use `delivery_mode=after_turn`: the recipient sees the message in
its TUI immediately, but the model processes it after the current turn/action
finishes. If the recipient is idle, it can process the message immediately.

For urgent follow-up, use `--soon`. For FYI or heartbeat messages that should
be shown but not start a model turn, use `--queue-only`, which maps to
`delivery_mode=passive`:

```sh
cutex agent send <agent-name-or-id> "please check this now" --soon
cutex agent send <agent-name-or-id> "FYI: transfer is still running" --queue-only
```

`--interrupt` is reserved for explicit interruption semantics and should not be
used as the default.

In collaboration mode, patched `cute-codex` also exposes native model tools
(`cutex_agent_list` and `cutex_agent_send`), so agents can message peers without
shelling out. Native tool sends default to `after_turn` and accept
`delivery_mode` values `after_turn`, `soon`, `passive`, and `interrupt`.
Incoming messages are shown in the target TUI history and are injected into
model context once through mailbox delivery.

The bus persists live agent registrations to
`~/.cutex/runtime/agent-bus-registry.json`. If the bus process is restarted, it
restores the known registrations before accepting requests. Agents that are
truly gone are still removed by the normal stale heartbeat/poll pruning; message
queues are intentionally not persisted across bus restarts.

### IM/Workbench Registration

Agent-bus collaboration and IM/workbench registration are separate. A temporary
agent can join the bus without becoming a durable contact. Register coding
sessions by Codex session id, not by thread display name:

```sh
cutex im register <session-id> --name aria-data --group aria example-project
cutex im register-current --group aria example-project
cutex im status-current
cutex im groups add <session-id> shared-review
cutex im unregister <session-id>
cutex im unregister-current
cutex im list
```

Inside patched `cute-codex`, `/im-reg [name]` calls
`cutex im register-current` and `/im-unreg` calls
`cutex im unregister-current`. These commands only update cutex IM metadata;
they do not enter model context.

The naming layers are intentionally separate:

- `session_id` is the durable canonical key used by the backend and IM routing.
- `thread_name` is the Codex/TUI thread title and can change through `/rename`
  or the `thread_name.set` management action.
- `display_name` is the IM/workbench contact name stored in the cutex registry.

`unregister` only hides/unmanages the session from IM/workbench. It does not
delete Codex history and does not stop a running process. Group changes update
the persistent registration and, when the session is currently live on the bus,
also patch the live agent groups.

### Management API

`cutex management serve` exposes a backend-facing localhost HTTP API for IM
clients and workbench services. It is separate from the model-visible agent bus:
registered sessions are durable contacts, while live bus agents are transient
runtime processes.

Start the API on a fixed Bridgeboard-friendly port:

```sh
cutex management serve --port 24270 --token '<management-token>'
```

For a trusted backend on Tailscale, bind only the host's Tailscale IP instead
of all interfaces:

```sh
cutex management serve --bind <tailscale-ip> --port 24270 --token '<management-token>'
```

The service registers a `cutex-management-api` Bridgeboard handoff when
Bridgeboard is available. `/` is an unauthenticated health check; `/v1/*`
endpoints require `Authorization: Bearer <management-token>`. If no management
token is provided, the current agent-bus token is reused.

List IM-registered sessions merged with live presence:

```http
GET /v1/sessions
Authorization: Bearer <management-token>
```

Records include `session_id`, `display_name`, optional `thread_name`, `host_id`,
`host_label`, `cwd`, `project_label`, `profile`, `groups`, `visible`,
`presence`, and `run_status`.
`presence.runtime_agent_id` is set only when the registered session is currently
online on the local/forwarded agent bus.

Read or change the live Codex thread title through management actions:

```http
POST /v1/sessions/<session-id>/actions
Authorization: Bearer <management-token>
Content-Type: application/json

{ "type": "thread_name.get", "payload": {} }
```

```http
POST /v1/sessions/<session-id>/actions
Authorization: Bearer <management-token>
Content-Type: application/json

{ "type": "thread_name.set", "payload": { "thread_name": "aria-ceo" } }
```

Thread-name observer events use `type=thread_name_updated` and
`payload.client_visibility=metadata`; they are not chat messages.

Send a user message to a session:

```http
POST /v1/sessions/<session-id>/messages
Authorization: Bearer <management-token>
Content-Type: application/json

{
  "sender_type": "user",
  "text": "hello from Android",
  "source": "waveline-android",
  "external_message_id": "android-local-...",
  "conversation_id": "aemeath-direct"
}
```

Messages are queued to the live runtime with `delivery_mode=after_turn`. The
response separates bus queueing from model receipt:

```json
{
  "delivered": false,
  "queued": true,
  "received": false,
  "session_id": "...",
  "runtime_agent_id": "...",
  "message_id": "...",
  "visible_in_im": true,
  "error": null
}
```

Read safe session events with a cursor:

```http
GET /v1/sessions/<session-id>/events?after=<cursor>
Authorization: Bearer <management-token>
```

The event stream merges management delivery summaries with mechanical
cute-codex observer events. Events have stable monotonically sortable cursors so
polling clients can pass the previous `next_cursor` and avoid duplicates.

Event records include:

- `event_id`
- `cursor`
- `session_id`
- `type`: `message_received`, `message_injected`, `state_changed`,
  `progress`, `tool_started`, `tool_progress`, `tool_finished`,
  `command_started`, `command_finished`, `approval_requested`,
  `approval_resolved`, `final_reply`, `error`, or `cancelled`
- `run_status`: `idle`, `queued`, `running`, `waiting`, `blocked`,
  `completed`, `failed`, `cancelled`, or `unknown`
- `phase`, `title`, `summary`
- `timestamp`
- optional `message_id`
- optional `final_reply`
- optional `detail_if_safe`

`cute-codex` observer events are mechanical runtime events and are not injected
into model context. They are emitted from TUI-visible lifecycle points: message
receipt, turn start/completion/failure/interruption, approval wait/resolution,
safe tool and command lifecycle, context compaction/rate-limit UI changes, and
visible final replies. Observer payloads deliberately exclude raw bus envelopes,
tokens, provider secrets, prompts, unfiltered terminal output, tool arguments,
environment dumps, file contents, and hidden reasoning.

For a local diagnostic event without running cute-codex:

```sh
cutex management observer-test --session-id <session-id> --type progress --summary "observer smoke"
```

### Cross-Host Agent Messaging

The agent bus is still a localhost-only HTTP service. To let agents on another
machine join it, forward the remote bus over SSH instead of exposing the port
directly.

On the bus owner, for example `host-a`, choose a shared token and start or
register the bus:

```sh
cutex global set --agent-bus-enable true --agent-bus-port 24260 --agent-bus-token '<shared-token>'
cutex agent list
bridgeboard handoff --id cutex-agent-bus --title "cutex agent bus" --port 24260 --owner-host host-a --pid-from-port --health-url http://127.0.0.1:24260/ --tunnel-mode local_forward --require-healthy
```

On the peer machine, for example host-b, create a local tunnel to the host-a
bus. If Bridgeboard peer tunneling is configured, use the cutex wrapper:

```sh
cutex agent remote-up host-a --token '<shared-token>'
```

The wrapper runs `bridgeboard up --peer host-a --local-port 24660
cutex-agent-bus`, then configures local cutex to use port `24660`. The explicit
peer selector avoids local/remote id shadowing, and the separate local port
avoids colliding with a local agent bus already listening on `24260`.

Raw SSH works as a fallback. Keep the tunnel process running in another
terminal and use a free local `24xxx` port if `24260` is already in use:

```sh
ssh -N -L 24660:127.0.0.1:24260 host-a
cutex global set --agent-bus-enable true --agent-bus-port 24660 --agent-bus-token '<shared-token>'
cutex run <profile> --agent
```

The peer agent will register through the tunnel into the owner bus. From either
side, use the normal commands:

```sh
cutex agent list
cutex agent send <agent-name-or-id> "hello from another host"
```

The bus keeps using `last_seen` heartbeats for liveness, so remote Windows or
Linux process ids are not interpreted as local host-a process ids.

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
