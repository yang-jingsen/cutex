//! Bounded Human catalog for one host; never exports runtime configuration.
use crate::{
    http::server::{write_json_response, SimpleHttpRequest},
    management::server::ManagementRequestContext,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::TcpStream;
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    #[serde(default)]
    query: String,
    cursor: Option<String>,
    limit: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSession {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub host_id: String,
    pub profile: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    pub cwd: String,
    pub state: String,
    pub generation: u64,
    pub native_id: Option<String>,
}
pub(super) fn handle(
    stream: &mut TcpStream,
    request: &SimpleHttpRequest,
    context: ManagementRequestContext,
) -> anyhow::Result<()> {
    let query: Query = match serde_json::from_slice(&request.body) {
        Ok(q) => q,
        Err(_) => {
            return super::server::write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_query",
                "Expected query, cursor and limit",
                false,
                json!({}),
            )
        }
    };
    if query.query.len() > 256 || query.cursor.as_ref().is_some_and(|s| s.len() > 160) {
        return super::server::write_v2_error(
            stream,
            400,
            "Bad Request",
            "invalid_query",
            "Query too long",
            false,
            json!({}),
        );
    }
    let store = crate::session::store::load_cutex_session_store()?;
    let host = crate::platform::host::current_host_name();
    let mut value=project(&store,&host,&query,context.load_runtime_status);
    if let Ok(snapshot)=crate::agent_management::AgentManagementStore::open_default().and_then(|s|s.snapshot().map_err(anyhow::Error::new)) {
        if let Some(rows)=value["data"].as_array_mut(){for row in rows {
            if let Some((_,agent))=snapshot.agents.iter().find(|(id,_)|Some(id.as_str())==row["id"].as_str()) {
                if let Some(id)=crate::agent_management::current_project_id(&snapshot,agent) {
                    let presentation=crate::agent_management::effective_presentation(&id,snapshot.project_presentations.get(&id));
                    row["projectId"]=json!(id.as_str());row["projectName"]=json!(presentation.display_name);
                }
            }
        }}
    }
    write_json_response(stream,200,"OK",&value)
}
fn project(store:&crate::session::model::CutexSessionStore,host:&str,query:&Query,load:crate::management::server::ManagementRuntimeStatusLoader)->serde_json::Value {
    let needle = query.query.to_lowercase();
    let mut records: Vec<_> = store
        .sessions
        .values()
        .filter(|r| {
            r.is_active()
                && !r.is_retired()
                && (r.agent_enabled || r.is_owned_session())
                && crate::runtime::lifecycle::cutex_session_host_is_local(&r.host_id, &host)
        })
        .filter(|r| {
            query
                .cursor
                .as_ref()
                .is_none_or(|c| r.cutex_session_id.as_str() > c.as_str())
        })
        .filter(|r| {
            r.cutex_session_id.to_lowercase().contains(&needle)
                || crate::session::metadata::cutex_session_display_name(r)
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect();
    records.sort_by(|a, b| a.cutex_session_id.cmp(&b.cutex_session_id));
    records.dedup_by_key(|r| r.cutex_session_id.clone());
    let limit = query.limit.unwrap_or(50).clamp(1, 50);
    let more = records.len() > limit;
    records.truncate(limit);
    let next = if more {
        records.last().map(|r| r.cutex_session_id.clone())
    } else {
        None
    };
    let data: Vec<_> = records
        .into_iter()
        .map(|r| {
            let observation = (load)(&r.cutex_session_id)
                .ok()
                .flatten();
            let state = if observation
                .as_ref()
                .is_some_and(|s| s.connected && s.runtime_generation == r.runtime_generation)
            {
                "Online"
            } else if r.runtime_pid.is_none()
                && r.app_server_runtime.is_none()
                && r.app_server_launch_claim_id.is_none()
            {
                "Offline"
            } else {
                "Unobserved"
            };
            HostSession {
                id: r.cutex_session_id.clone(),
                name: crate::session::metadata::cutex_session_display_name(r)
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect(),
                kind: if r.is_owned_session() {
                    "Session"
                } else {
                    "Agent"
                }
                .into(),
                host_id: host.to_owned(),
                profile: r.profile.clone(),
                project_id:None,project_name:None,
                cwd: crate::session::service::cutex_session_launch_cwd(r)
                    .chars()
                    .take(1024)
                    .collect(),
                state: state.into(),
                generation: r.runtime_generation,
                native_id: r.codex_session_id.clone(),
            }
        })
        .collect();
    json!({"schema":"cutex/host-sessions/v1","hostId":host,"data":data,"nextCursor":next})
}
#[cfg(test)]
mod tests {
 use super::*;
 #[test] fn catalog_pages_exclude_foreign_records_and_runtime_secrets(){
  let mut store=crate::session::model::CutexSessionStore::default();
  for i in 0..56 {
   let id=format!("cutex.{}",uuid::Uuid::new_v4());
   let mut r=crate::session::model::CutexSessionRecord::new(id.clone(),None,if i==55{"foreign"}else{"local-test"}.into(),"/work".into(),Some("preset".into())).unwrap();
   r.agent_enabled=true;r.formal_agent_name=Some(format!("Worker-{i}"));r.thread_name=Some("SECRET_HISTORY_SENTINEL".into());store.sessions.insert(id,r);
  }
  let first=project(&store,"local-test",&Query{limit:Some(5000),..Default::default()}, |_|Ok(None));
  assert_eq!(first["data"].as_array().unwrap().len(),50);assert!(!first.to_string().contains("SECRET_HISTORY_SENTINEL"));
  let second=project(&store,"local-test",&Query{cursor:first["nextCursor"].as_str().map(str::to_owned),..Default::default()}, |_|Ok(None));assert_eq!(second["data"].as_array().unwrap().len(),5);assert!(second["nextCursor"].is_null());
  let one=project(&store,"local-test",&Query{query:"WORKER-54".into(),..Default::default()}, |_|Ok(None));assert_eq!(one["data"].as_array().unwrap().len(),1);assert_eq!(one["data"][0]["name"],"Worker-54");
 }
}
