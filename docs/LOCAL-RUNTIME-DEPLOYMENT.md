# Local runtime deployment

Linux can select a locally installed light runtime in
`~/.cutex/runtime/light/deployment.json`:

```json
{
  "native_home": "/absolute/source/codex-home",
  "bundle_manifest": "/absolute/install/bundle.json",
  "job_mcp": null
}
```

`LocalDeployment::install` verifies the artifact template and writes this selection
privately. The template uses the existing StockBundle fields, with `version: 4`.
It records file hashes for the selected CLI, app-server, companion, facade, schema
and config. Version 4 permits local executable rebuilds; it still checks every
file's bytes and the currently supported generated protocol schema. Initializing
the owner still verifies ExternalInput v1/v2, presentation v1 and Soon delivery.
Old bundle versions retain their historical validation rules.

Normal saved-session Adopt uses this selection for newly managed local records.
It preserves the native UUID and durable identity, copies the one stopped rollout
into `runtime/light/agents/<native UUID>`, writes source provenance, and projects
the current shared config into that home. Source history remains available for
recovery; the managed runtime uses the new copy. Skills, memories and plugins
remain shared links. Credentials continue to come from the selected profile.
An open writable source rollout or recorded runtime owner must be stopped first.
A new-agent helper creates and persists an empty native thread, stops its creator,
and calls the same root adoption API. It does not send a model prompt.

These homes use launch contract version 4. Existing managed records and the 34
migration homes are not converted by default selection. Without an installed
selection the historical Adopt path remains available. A malformed installed
selection produces an actionable error instead of silently choosing a different
backend.

The Management API runtime review should use deployment `job_mcp` when no
explicit descriptor or prior agent descriptor exists, refreshing its live daemon
occurrence. The descriptor's launcher must equal the selected bundle CLI; it no
longer needs a separately compiled CLI hash. Installing native repairs does not
change an already running owner. Existing agents require a human configuration
update to their new per-home manifest, followed by the intended restart.
