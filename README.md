# cutex

`cutex` is a local-first session manager and profile launcher for
[`cute-codex`](https://github.com/yang-jingsen/cute-codex). It keeps Codex
profiles separate, tracks durable sessions across terminal and process
lifetimes, and provides the runtime services used by collaborating agents,
workbench clients, Agent Management, and Task Service.

Running `cutex` with no arguments opens the interactive session TUI. Direct
profile launches and the complete management surface remain available through
the CLI.

## What it provides

- A continuous TUI for live, offline, unmanaged, and retired Codex sessions.
- Durable session identity, metadata, launch defaults, runtime state, and
  duplicate-resume protection.
- Host, Docker, visible foreground, and detachable `cute-alden` runtime paths.
- A manager-owned `cute-codex app-server` runtime that can remain online while
  its visible TUI is detached.
- Multiple isolated ChatGPT/API profiles, with per-profile provider, proxy,
  runtime, status-line, and CLI settings.
- A local Management v2 API for session/workbench state, runtime lifecycle,
  events, and native app-server requests.
- Opt-in Agent Bus discovery and messaging, including cross-host discovery
  through Bridgeboard.
- Project-scoped Agent Management with authenticated Director authority.
- A durable Task Service for assignments, attempts, receipts, logical seats,
  progress, completion, and stale-work reminders.
- Per-agent/profile/model usage reporting derived from locally observed native
  usage events.

## Runtime model

The main pieces have deliberately different responsibilities:

| Component | Responsibility |
| --- | --- |
| `cute-codex` | Native Codex TUI, app-server protocol, and model/tool execution |
| Cutex profile store | Authentication/config isolation and launch policy |
| Durable session store | Stable Cutex identity, Codex thread identity, cwd, groups, profile intent, and lifecycle metadata |
| Managed app-server | Long-lived native runtime for one durable session |
| Visible TUI peer | An attachable terminal client; it may come and go without ending the managed app-server |
| Management host | Local HTTP control plane, runtime recovery, events, Task Service, and administrative routes |
| Agent Bus | Live agent discovery and authenticated message delivery |

`online`, `offline`, and `close` apply to the managed runtime. Closing a
terminal client is not the same operation. Likewise, `retire` archives a
durable session record; it is not an alias for closing a runtime or deleting
Codex history.

A durable session records its owning runtime host. Lifecycle mutations are
performed by that host. Another machine can reach the host through the
Management API/Bridgeboard path, but should not pretend that a remote PID or
runtime endpoint is local.

## Install

Requirements:

- a Rust toolchain supported by this workspace;
- a compatible `cute-codex` (preferred) or Codex-compatible CLI;
- `codex-code-mode-host` beside `cute-codex` when Code Mode is enabled;
- `cute-alden` when detachable/reattachable terminal sessions are required;
- Docker and Bridgeboard only for the corresponding optional features.

Build from source:

```sh
cargo build --release --locked
```

The result is `target/release/cutex` on Unix-like systems and
`target/release/cutex.exe` on Windows. Put it on `PATH` together with the
runtime dependencies you intend to use.

A Cutex release that enables cute-codex Code Mode is one three-program bundle:
`cutex`, `cute-codex`, and `codex-code-mode-host` (with `.exe` suffixes on
Windows). `cute-codex` resolves the host relative to its own executable, so the
two cute-codex programs must be regular files in the same release directory.
Build the latter two from the same cute-codex source commit, then reject an
incomplete bundle before changing a live shortcut or service:

```sh
scripts/verify-release-bundle.sh /path/to/linux-bundle
```

```powershell
./scripts/verify-release-bundle.ps1 -Bundle C:\path\to\windows-bundle
```

The verifier runs target-native version/help smokes for all three programs.
Disabling Code Mode is an explicit product/configuration decision, not an
automatic packaging fallback.

Cutex resolves the Codex-compatible binary in this order:

1. `CUTEX_CODEX_BIN`
2. `cute-codex`
3. `cutex-codex`
4. `codex`

Set `CUTEX_CODEX_BIN` when the preferred CLI is not available on `PATH`.

## Quick start

Create the first profile and review global settings:

```sh
cutex login
cutex wizard
```

Open the main TUI:

```sh
cutex
# equivalent explicit entry points
cutex tui
cutex start
```

Launch the default profile without opening the selector:

```sh
cutex --quick
```

Launch a named profile without changing the stored active profile:

```sh
cutex run <profile>
```

Arguments after `--` are passed to the selected Codex-compatible CLI:

```sh
cutex --quick -- --help
cutex run <profile> -- --help
```

Use `cutex --help` and `cutex <command> --help` for the exact command surface
of the installed build.

## Session TUI

The TUI merges durable Cutex records, workbench registration, `cute-alden`
attachments, Agent Bus presence, and locally projected activity. It continues
refreshing while open and uses responsive columns for narrow and wide
terminals.

The main list also contains entry rows for Profiles, Global settings, and the
retired-session archive. From an agent row you can:

- run the primary lifecycle action with `Enter`;
- open the complete action list with `Right` (or `Shift+Enter` where enhanced
  keyboard reporting is available);
- open session settings with `Tab`;
- close a known runtime with `Ctrl+X`, after confirmation;
- search by typing, clear the search with `Ctrl+U`, and exit with `Ctrl+C`.

Available actions depend on the selected record and current lifecycle state.
They include attaching or taking over an existing terminal, opening a TUI for
an online app-server, bringing a managed runtime online, resuming in the
current or managed cwd, closing/restarting a runtime, and retiring/restoring a
session. Close/restart and archive operations require confirmation.

Session settings cover durable profile intent, display name, groups,
workbench visibility, quick-action policy, managed cwd, runtime backend,
permissions, approval policy, sandbox, model, reasoning effort, and extra CLI
arguments. The Profiles workspace can add, activate, rename, remove, and edit
profiles; staged edits are applied explicitly.

For scripted or filtered selection, the compatibility picker remains useful:

```sh
cutex session list --sort recent
cutex start --offline
cutex start --project <text> --group <group>
```

## Durable sessions

A native Codex thread can exist before Cutex manages it. Adoption adds Cutex
metadata and future lifecycle intent; it does not copy or delete the native
history:

```sh
cutex session adopt <session-id> --current-cwd --pin
cutex session adopt <session-id> --name <name> --group <group> --im
```

Common inspection and lifecycle commands:

```sh
cutex session list
cutex session show <session-id>
cutex session online <session-id>
cutex session foreground <session-id>
cutex session offline <session-id>
cutex session close <session-id>
cutex session retire <session-id>
cutex session retired
cutex session restore <session-id>
```

Important boundaries:

- `online` starts or reconnects the manager-owned runtime according to the
  durable session defaults.
- `foreground` resumes visibly in the invoking terminal.
- `offline` and `close` stop runtime state while preserving the durable record
  and native history.
- `unmanage` removes Cutex management metadata without deleting history or
  killing an independently running runtime.
- `hide` only removes workbench/IM visibility.
- `retire` requires the managed runtime to be safely offline and moves the
  record to the archive; `restore` returns it as active and offline.
- `duplicate-check` and takeover commands help avoid opening a second runtime
  for the same native thread.

Use `cutex session --help` for profile, group, cwd, quick-action, and runtime
default subcommands.

## Profiles and launch policy

Profiles live under `~/.cutex/profiles/<profile-id>/` and keep their own auth
and Codex configuration. A profile can select a host or Docker runtime, proxy
behavior, provider configuration, display metadata, status line, and default
CLI arguments.

```sh
cutex profile list
cutex profile show [<profile>]
cutex profile edit [<profile>]
cutex profile use <profile>
cutex profile copy <profile> --name <new-name>
cutex profile set <profile> --host
cutex profile set <profile> --default-cli-args='--sandbox workspace-write'
```

`cutex run <profile>` selects a profile for one invocation. It does not rewrite
the active profile. A durable session may also store a profile for future
managed launches, while `session online --profile <profile>` and
`session foreground --profile <profile>` apply a one-launch override.

Global settings are available through both CLI and TUI:

```sh
cutex global show
cutex global edit
cutex global set --default-profile <profile>
cutex global set --default-profile-direct-launch true
```

`cutex config` remains an alias for `cutex wizard`.

## Managed runtime and Management v2

Managed launches normally ensure that the local Management service is
available. To run it explicitly in the foreground:

```sh
cutex management serve --port 24270
```

The service binds to loopback by default, ensures the Agent Bus is available,
and adopts valid persisted app-server runtimes on startup. The unauthenticated
health check is `GET /`; Management resources and actions use `/v2/*` and a
bearer credential when configured.

Management v2 exposes durable session projections, per-session bootstrap and
event state, lifecycle/settings mutations, user input, safe native app-server
request forwarding, host events, Task Service project views, Agent Management,
seat administration, and Release rotation routes. Clients should follow the
current versioned contracts rather than assume an older `/v1` payload.

For a trusted cross-host deployment, bind only an explicitly selected private
interface and use a dedicated token:

```sh
cutex management serve --bind <private-ip> --port 24270 --token '<token>'
```

Bridgeboard can create a local forward to another runtime host:

```sh
cutex management remote-up <host>
```

Do not expose the Management or Agent Bus ports directly to an untrusted
network. Privileged administration requires a dedicated Management root token;
it must be distinct from the Agent Bus token. Secrets should be stored in the
private config, not embedded in shell history or committed request documents.

## Agent collaboration

Agent collaboration is opt-in for ordinary direct launches:

```sh
cutex --agent
cutex run <profile> --agent --group <group>
```

Without `--agent`, a direct launch does not expose Cutex Agent Bus tools to the
model. With collaboration enabled, agents can discover and message permitted
peers through the CLI or the native `cutex_agent_list` and `cutex_agent_send`
tools supplied by compatible `cute-codex` builds.

```sh
cutex agent list
cutex agent send <agent-id-or-name> "please report status"
cutex agent send <agent-id-or-name> "FYI only" --queue-only
cutex agent send <agent-id-or-name> "please check now" --soon
```

Normal delivery is `after_turn`; `--soon` requests prompt handling and
`--queue-only` is passive. Explicit interruption is available but is not the
default delivery mode. Durable session identity, mutable thread name, and
workbench display name are separate naming layers.

Cross-host discovery uses Bridgeboard-backed host queries. The legacy
`cutex agent remote-up` bus-forwarding command remains available, but new
control-plane integrations should prefer the Management host connection.

## Agent Management

Agent Management is a project-scoped, authenticated lifecycle service for
durable Cutex agents. It supports create, query, online, offline, restart,
close, replace, explicit Agent Operator grant/revoke, and Primary Director
rotation operations:

```sh
cutex agent manage query-managed --request-file <private-json-file>
cutex agent manage create --request-file <private-json-file>
cutex agent manage grant-operator --request-file <private-json-file>
```

The caller's current durable session and authenticated live runtime determine
authority. Collaboration groups, cwd labels, request prose, and model-supplied
identity fields do not grant project authority. A caller authorized for more
than one project must provide an explicit project selector.

Requests use the strict `cutex/agent-management/v1` document contract and a
stable `action_id`. Exact replay is idempotent; changed-payload reuse conflicts.
Administrative project-authority repair and legacy ownership import are
separate root-only Management commands, not general Agent operations.

Every project retains one Primary Director. Exact, durable Agent Operator
grants can delegate ordinary same-project lifecycle work without conferring
Director rotation, grant/revoke, presentation, ownership-import, reservation,
or Task Service decision authority. The Task Service Director seat remains
separate, and no Observer grant is inferred.

See [docs/agent-management-provider.md](docs/agent-management-provider.md) for
the provider boundary, authority rules, and recovery behavior.

## Task Service

Task Service is the durable coordination layer used for project assignments.
It records task and attempt revisions, authenticated assignment delivery,
idempotent action receipts, Worker progress and terminal outcomes, logical-seat
occupancy, and watchdog reminders without treating chat text as authority.

Compatible agent runtimes expose the typed `cutex_task_service` tool. The CLI
transport for a strict local action or query is:

```sh
cutex agent task-action --request-file <private-json-file>
```

Seat binding/query and other owner-only operations are deliberately separate
Management commands:

```sh
cutex management seat query
cutex management seat bind --help
```

Task Service state is authoritative and durable; Agent Bus delivery and TUI
presentation are transport/projection layers. Retrying an action therefore
reuses its stable action identity instead of inventing a second semantic
operation.

## Usage reporting

```sh
cutex usage
cutex usage --last 7d --period day --group-by agent
cutex usage --period reset --reset-window primary
cutex usage --group-by model --json
```

The local ledger contains identifiers, timestamps, attribution labels, token
counts, and derived pricing coverage. It does not store prompts, responses,
credentials, or hidden reasoning. API-equivalent cost values are estimates;
unknown providers/models remain explicitly unpriced.

## Optional integrations

Docker profiles preserve a stable project path while selecting a container
runtime:

```sh
cutex profile set <profile> --docker-image <image> --docker-user-name <name>
cutex run <profile>
cutex run <profile> --host  # one-launch host override
```

See [docs/docker-image-notes.md](docs/docker-image-notes.md) for image and
environment requirements.

On Linux desktops, the optional native notification bridge can be enabled with:

```sh
cutex notify desktop enable --port 24250
cutex notify desktop status
cutex notify desktop test "cutex desktop test"
```

Ubuntu/Kubuntu user-service installation is available through
`cutex notify desktop install-ubuntu`. Desktop notification support is separate
from any external notification endpoint.

## Local data and security

Primary state lives under `~/.cutex/`:

- `accounts.json`: lightweight profile index and display metadata;
- `config.json`: global settings and shared status-line catalog;
- `profiles/<profile-id>/auth.json`: private profile authentication;
- `profiles/<profile-id>/config.toml`: private profile Codex configuration;
- `cutex-sessions.json`: durable session records;
- `codex-home/`: aggregate host-side Codex home and native session index;
- `runtime/`: local runtime endpoints, projections, journals, and service logs.

These files can contain credentials, account metadata, private project paths,
session identifiers, and operational records. Do not publish the directory or
copy it into a release tree. Cutex state is distinct from the user's default
`~/.codex` home unless a launch is intentionally configured otherwise.

## Development

```sh
cargo fmt --check
cargo test --locked -- --test-threads=1
cargo clippy --all-targets --locked
cargo build --release --locked
```

Additional command notes and implementation contracts are under [`docs/`](docs/).
