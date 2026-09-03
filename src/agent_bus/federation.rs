//! Agent bus peer discovery, forwarding support, and federated list helpers.

use std::collections::HashSet;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;

use crate::agent_bus::client::agent_bus_http_json;
use crate::agent_bus::model::AgentBusAgent;
use crate::agent_bus::routing::groups_overlap;
use crate::agent_bus::service::agent_bus_base_url;
use crate::agent_bus::service::AGENT_BUS_BRIDGE_ID;
use crate::agent_bus::service::DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT;
use crate::bridgeboard::agent_bus::agent_bus_peer_endpoint_from_bridgeboard_record;
use crate::bridgeboard::agent_bus::dedupe_agent_bus_peer_endpoints;
use crate::bridgeboard::agent_bus::AgentBusPeerEndpoint;
use crate::bridgeboard::agent_bus::BridgeboardServiceRecord;
use crate::http::client::connect_local_port;
use crate::http::client::http_base_url_root_status_ok;
use crate::platform::command::command_exists_in_path;
use crate::platform::host::current_host_name;

const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub fn fetch_peer_agent_bus_agents(
    peer: &AgentBusPeerEndpoint,
) -> anyhow::Result<Vec<AgentBusAgent>> {
    ensure_agent_bus_peer_endpoint(peer);
    let value = agent_bus_http_json(
        &peer.base_url,
        "GET",
        "/api/federation/agents?all_groups=true",
        None,
        None,
    )?;
    serde_json::from_value(value).context("Failed to parse peer agent list response")
}

pub fn discover_agent_bus_peer_endpoints() -> Vec<AgentBusPeerEndpoint> {
    let mut peers = Vec::new();
    if connect_local_port(
        DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT,
        Duration::from_millis(200),
    )
    .is_ok()
    {
        peers.push(AgentBusPeerEndpoint {
            host: "peer".to_string(),
            port: DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT,
            base_url: agent_bus_base_url(DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT),
        });
        return peers;
    }
    if !command_exists_in_path("bridgeboard") {
        return Vec::new();
    }
    let local_host = current_host_name().to_ascii_lowercase();
    if let Ok(output) = Command::new("bridgeboard")
        .arg("list")
        .arg("--json")
        .arg("--peers")
        .output()
    {
        if output.status.success() {
            if let Ok(records) =
                serde_json::from_slice::<Vec<BridgeboardServiceRecord>>(&output.stdout)
            {
                for record in records {
                    if let Some(peer) =
                        agent_bus_peer_endpoint_from_bridgeboard_record(record, &local_host)
                    {
                        peers.push(peer);
                    }
                }
            }
        }
    }
    dedupe_agent_bus_peer_endpoints(peers)
}

pub fn ensure_agent_bus_peer_endpoint(peer: &AgentBusPeerEndpoint) {
    if agent_bus_base_url_healthy(&peer.base_url) {
        return;
    }
    if !command_exists_in_path("bridgeboard") {
        return;
    }
    let _ = Command::new("bridgeboard")
        .arg("up")
        .arg("--peer")
        .arg(&peer.host)
        .arg("--local-port")
        .arg(peer.port.to_string())
        .arg(AGENT_BUS_BRIDGE_ID)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn agent_bus_base_url_healthy(base_url: &str) -> bool {
    http_base_url_root_status_ok(base_url, Duration::from_millis(500))
}

pub fn fetch_federated_agent_bus_agents() -> Vec<AgentBusAgent> {
    let mut agents = Vec::new();
    for peer in discover_agent_bus_peer_endpoints() {
        match fetch_peer_agent_bus_agents(&peer) {
            Ok(mut peer_agents) => agents.append(&mut peer_agents),
            Err(err) => eprintln!(
                "{YELLOW}warning:{RESET} failed to fetch peer agent bus {} on {}: {err:#}",
                peer.base_url, peer.host
            ),
        }
    }
    dedupe_agents_by_id(agents)
}

pub fn filter_federated_agents_for_request(
    agents: Vec<AgentBusAgent>,
    requester: Option<&str>,
    requester_groups: Option<&[String]>,
    all_groups: bool,
) -> Vec<AgentBusAgent> {
    if all_groups || requester.is_none() {
        return agents;
    }
    let Some(requester_groups) = requester_groups else {
        return Vec::new();
    };
    agents
        .into_iter()
        .filter(|agent| groups_overlap(&agent.groups, requester_groups))
        .collect()
}

pub fn dedupe_agents_by_id(agents: Vec<AgentBusAgent>) -> Vec<AgentBusAgent> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for agent in agents {
        if seen.insert(agent.id.clone()) {
            deduped.push(agent);
        }
    }
    deduped
}
