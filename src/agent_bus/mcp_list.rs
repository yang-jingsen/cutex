//! Read-only projection of existing Bus visibility and authoritative durable mapping.
use crate::agent_bus::{model::AgentBusAgent, routing::project_current_durable_session_ids};
use crate::session::model::CutexSessionStore;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    all_groups: bool,
    #[serde(default)]
    all_hosts: bool,
}
pub(super) fn validate_args(value: Value) -> anyhow::Result<()> {
    let args: Args = serde_json::from_value(value)?;
    anyhow::ensure!(
        !args.all_groups && !args.all_hosts,
        "unsupported_scope: this prototype lists local group-visible Agents only"
    );
    Ok(())
}
pub(super) fn tool() -> Value {
    json!({"name":"cutex_agent_list","description":"List local group-visible registered Agents with verified durable/native/runtime mappings. Formal name may be unavailable; never resolve by title/cwd. This prototype defaults all_hosts=false; all_groups=true or all_hosts=true is explicitly unsupported. Runtime observation is not delivery/A4. Offline durable Agents belong to query_managed, not this registry list.","inputSchema":{"type":"object","properties":{"all_groups":{"type":"boolean","description":"Only false supported; default false."},"all_hosts":{"type":"boolean","description":"Only false supported; default false (unlike native cross-host default)."}},"required":[],"additionalProperties":false}})
}

pub(crate) fn project(
    requester: &str,
    mut agents: Vec<AgentBusAgent>,
    sessions: &CutexSessionStore,
) -> Value {
    project_current_durable_session_ids(&mut agents, sessions);
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let cutoff = now.saturating_sub(crate::agent_bus::store::AGENT_BUS_STALE_HEARTBEAT_SECS);
    let rows:Vec<_>=agents.iter().map(|a| {
        let record=a.cutex_session_id.as_ref().and_then(|id|sessions.sessions.get(id)).filter(|r| {
            a.cutex_session_id.as_ref()==Some(&r.cutex_session_id)
                && r.codex_session_id.is_some()
                && sessions.sessions.values().filter(|other|other.codex_session_id==r.codex_session_id).count()==1
                && crate::agent_bus::routing::is_full_durable_cutex_session_id(&r.cutex_session_id)
        });
        let name=record.and_then(|r|r.formal_agent_name.as_deref()).filter(|s|!s.trim().is_empty());
        let current=a.last_seen_epoch_secs>=cutoff && crate::platform::process::process_is_running(a.pid);
        json!({"runtime_agent_id":a.id,"cutex_session_id":record.map(|r|&r.cutex_session_id),"native_session_id":a.session_id,"formal_name":name,
            "mapping_observation":if record.is_some(){"current"}else{"unavailable_or_ambiguous"},
            "name_observation":if name.is_some(){"durable_formal_name"}else{"unavailable"},
            "runtime_observation":if current {"registered_online"}else{"stale_or_unavailable"},
            "runtime_generation":record.map(|r|r.runtime_generation),"configured_profile":record.and_then(|r|r.profile.as_deref()),
            "observed_runtime_profile":a.profile,"cwd":a.cwd,"groups":a.groups,"registration_class":a.registration_class,
            "last_seen_epoch_secs":a.last_seen_epoch_secs,"this":a.id==requester})
    }).collect();
    json!({"ok":true,"current_agent_id":requester,"scope":"local_group_visible","agents":rows})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_never_broadens_silently_and_caller_is_not_an_argument() {
        assert!(validate_args(json!({})).is_ok());
        for args in [
            json!({"all_hosts":true}),
            json!({"all_groups":true}),
            json!({"agent_id":"spoof"}),
            json!({"allHosts":true}),
        ] {
            assert!(validate_args(args).is_err());
        }
    }

    #[test]
    fn projection_uses_exact_current_mapping_and_never_name_title_or_profile_identity() {
        use crate::session::model::CutexSessionRecord;
        let id = "cutex.01a0487d-c794-7e43-aeb4-19af2717037e";
        let native = "01a0487d-c794-7e43-aeb4-19af2717037e";
        let mut record = CutexSessionRecord::new_at(
            id.into(),
            Some(native.into()),
            "host-a".into(),
            "/private".into(),
            None,
            "2026-09-09T00:00:00Z".into(),
        )
        .unwrap();
        record.current_runtime_agent_id = Some("owned-runtime".into());
        record.runtime_generation = 3;
        record.agent_enabled = true;
        record.formal_agent_name = Some("Formal".into());
        let agent:AgentBusAgent=serde_json::from_value(json!({"id":"owned-runtime","name":"NOT FORMAL","thread_name":"TITLE NOT NAME",
            "session_id":native,"profile":"mutable","cwd":"/private","pid":std::process::id(),"host_id":"host-a",
            "groups":["private"],"registration_class":"persistent","last_seen_epoch_secs":0})).unwrap();
        let mut sessions = CutexSessionStore::default();
        sessions.sessions.insert(id.into(), record.clone());
        let row = project("owned-runtime", vec![agent.clone()], &sessions)["agents"][0].clone();
        assert_eq!(row["cutex_session_id"], id);
        assert_eq!(row["formal_name"], "Formal");
        assert_eq!(row["runtime_observation"], "stale_or_unavailable");
        record.formal_agent_name = None;
        record.profile = Some("changed-profile".into());
        sessions.sessions.insert(id.into(), record.clone());
        let row = project("owned-runtime", vec![agent.clone()], &sessions)["agents"][0].clone();
        assert_eq!(row["cutex_session_id"], id);
        assert!(row["formal_name"].is_null());
        assert_eq!(row["name_observation"], "unavailable");
        sessions.sessions.insert("duplicate-native".into(), record);
        let row = project("owned-runtime", vec![agent.clone()], &sessions)["agents"][0].clone();
        assert!(row["cutex_session_id"].is_null());
        assert_eq!(row["mapping_observation"], "unavailable_or_ambiguous");
        assert!(
            project("owned-runtime", vec![agent], &CutexSessionStore::default())["agents"][0]
                ["cutex_session_id"]
                .is_null()
        );
    }
}
