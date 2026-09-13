# Human administration

`cutex human` is the entry for a user or an agent acting at the user's request.
It uses the existing local Management credential automatically. It does not
require a Director seat, another login, or proof of a physical human. The name
communicates intent; processes running as the same OS user are not isolated from
this credential. Ordinary commands retain their existing grouping.

Common commands:

```
cutex human new NAME --cwd /path/to/project
cutex human start NAME
cutex human attach NAME
cutex human stop NAME
cutex human restart NAME --cancel-tasks
cutex human stop NAME --force --cancel-tasks
cutex human recover NAME
cutex human action ACTION_ID
cutex human tasks list --assignee NAME
cutex human tasks cancel ASSIGNMENT_ID
cutex human tasks reassign ASSIGNMENT_ID NEW_OWNER --action-id TRANSFER_ID
cutex human config show NAME
cutex human config set NAME model=MODEL reasoning=high cwd=/path
cutex human config set NAME profile=null
cutex human config undo NAME ORIGINAL_ACTION_ID
```

Names may be a unique formal/display name, native UUID, or exact durable ID.
`stop` interrupts the runtime while retaining history and task records;
`--cancel-tasks` also closes the selected agent's current assignments. `--force`
uses the existing forced stop path when graceful interruption is unavailable.
Task-only cancel/reassign updates bookkeeping without sending a message or
stopping a runtime. Reassignment returns a new assignment awaiting the new
owner; it does not silently dispatch a model prompt.

Configuration changes affect the next launch. The current runtime keeps its own
launch configuration and package identity for Attach/Stop. Changes return an
action ID and before/after values. Undo restores only those fields and refuses
to overwrite newer changes to the same fields; a new explicit Set remains
available. Bundle selection accepts a manifest path and computes its hash.

The TUI exposes Start/Attach/Restart and interrupted-start recovery. Alt+N in
Agents or Recent opens the New Agent form. Ordinary saved-session Adopt uses
the selected local runtime and preserves the saved native ID. New agent creation
persists an empty native thread; starting a model turn remains a separate action.

Local runtime installation:

```
cutex human install-runtime /path/to/template.json --source-home /path/to/native-home --job-descriptor /path/to/job.json
cutex human config set NAME bundle_manifest=/path/to/agent-specific-bundle.json
```

Installing selects the default for future New/Adopt. Updating an existing agent
uses a per-home manifest; it does not copy its history or restart it. Prior
packages remain available for undo and running owners.

`CUTEX_MANAGEMENT_URL=http://host:port` selects an already running Management
endpoint for these commands without starting another service. Credentials still
come from the caller's configured home. This also supports isolated local repair
and testing. Do not submit a second start merely because its HTTP response was
lost: the client queries the original action and reports its action ID if the
result remains unknown.
