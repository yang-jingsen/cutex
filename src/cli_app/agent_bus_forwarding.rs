use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use serde_json::Value;

use cutex::agent_bus::audit::append_agent_bus_audit_record;
use cutex::agent_bus::client::agent_bus_http_json;
use cutex::agent_bus::federation::{
    discover_agent_bus_peer_endpoints, fetch_peer_agent_bus_agents,
};
use cutex::agent_bus::groups::agent_groups_for_id;
use cutex::agent_bus::model::AgentBusSendRequest;
use cutex::agent_bus::routing::{
    agent_bus_agent_session_id_by_id, is_full_durable_cutex_session_id,
    normalize_agent_bus_session_id, resolve_agent_target_from_agent_list,
    AgentTargetResolutionCode, AgentTargetResolutionError,
};
use cutex::agent_bus::store::AgentBusState;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub(crate) fn ensure_no_peer_target_collision(
    state: &Arc<Mutex<AgentBusState>>,
    payload: &AgentBusSendRequest,
    local_target_id: &str,
) -> anyhow::Result<()> {
    let sender_groups = payload
        .from_agent_id
        .as_deref()
        .and_then(|id| agent_groups_for_id(state, id));
    let mut agents = Vec::new();
    for peer in discover_agent_bus_peer_endpoints() {
        match fetch_peer_agent_bus_agents(&peer) {
            Ok(peer_agents) => agents.extend(peer_agents),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to inspect peer Agent Bus durable targets on {}",
                        peer.host
                    )
                });
            }
        }
    }
    if agents.is_empty() {
        return Ok(());
    }
    match resolve_agent_target_from_agent_list(
        &agents,
        &payload.to,
        sender_groups.as_deref(),
        payload.all_groups,
    ) {
        Ok(peer_target) if peer_target.id == local_target_id => Ok(()),
        Ok(_) => Err(anyhow::Error::new(AgentTargetResolutionError::ambiguous(
            format!(
                "Durable cutex session `{}` has multiple current endpoints across hosts",
                payload.to
            ),
        ))),
        Err(error)
            if error
                .downcast_ref::<AgentTargetResolutionError>()
                .is_some_and(|error| error.code() == AgentTargetResolutionCode::NotFound) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn try_forward_agent_bus_message(
    state: &Arc<Mutex<AgentBusState>>,
    payload: &AgentBusSendRequest,
    sender: &str,
) -> anyhow::Result<Option<Value>> {
    let sender_groups = payload
        .from_agent_id
        .as_deref()
        .and_then(|id| agent_groups_for_id(state, id));
    let mut peer_agents = Vec::new();
    for peer in discover_agent_bus_peer_endpoints() {
        let agents = match fetch_peer_agent_bus_agents(&peer) {
            Ok(agents) => agents,
            Err(err) if is_full_durable_cutex_session_id(&payload.to) => {
                return Err(err).with_context(|| {
                    format!(
                        "Failed to inspect peer Agent Bus durable targets on {}",
                        peer.host
                    )
                });
            }
            Err(err) => {
                eprintln!(
                    "{YELLOW}warning:{RESET} failed to fetch peer agent bus {} on {}: {err:#}",
                    peer.base_url, peer.host
                );
                continue;
            }
        };
        peer_agents.extend(agents.into_iter().map(|agent| (peer.clone(), agent)));
    }
    let mut agents = Vec::new();
    for (_, agent) in &peer_agents {
        if !agents
            .iter()
            .any(|candidate: &cutex::agent_bus::model::AgentBusAgent| candidate.id == agent.id)
        {
            agents.push(agent.clone());
        }
    }
    let target = match resolve_agent_target_from_agent_list(
        &agents,
        &payload.to,
        sender_groups.as_deref(),
        payload.all_groups,
    ) {
        Ok(target) => target,
        Err(error)
            if error
                .downcast_ref::<AgentTargetResolutionError>()
                .is_some_and(|error| error.code() == AgentTargetResolutionCode::NotFound) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let peer = peer_agents
        .iter()
        .find(|(_, agent)| agent.id == target.id)
        .map(|(peer, _)| peer)
        .context("resolved peer Agent Bus target lost its route")?;
    let mut forwarded = payload.clone();
    forwarded.to = target.id.clone();
    forwarded.all_groups = true;
    forwarded.all_hosts = false;
    forwarded.from = forwarded
        .from
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some(sender.to_string()));
    forwarded.from_session_id = forwarded.from_session_id.or_else(|| {
        payload
            .from_agent_id
            .as_deref()
            .and_then(|id| agent_bus_agent_session_id_by_id(state, id))
    });
    forwarded.to_session_id = forwarded
        .to_session_id
        .or_else(|| normalize_agent_bus_session_id(target.session_id.as_deref()));
    forwarded.from_agent_id = None;
    let body = serde_json::to_vec(&forwarded)?;
    let response = agent_bus_http_json(
        &peer.base_url,
        "POST",
        "/api/federation/messages/send",
        None,
        Some(&body),
    )
    .with_context(|| {
        format!(
            "Failed to forward agent message to {} via {}",
            target.name, peer.base_url
        )
    })?;
    if let Err(err) = append_agent_bus_audit_record(serde_json::json!({
        "event": "forwarded",
        "timestamp": Utc::now().to_rfc3339(),
        "peer_host": peer.host,
        "peer_base_url": peer.base_url,
        "target": payload.to,
        "resolved_target_id": target.id,
        "resolved_target_name": target.name,
        "response": response,
    })) {
        eprintln!("{YELLOW}warning:{RESET} failed to write agent audit log: {err:#}");
    }
    Ok(Some(response))
}
