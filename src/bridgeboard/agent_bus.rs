//! Bridgeboard peer discovery helpers for cutex agent bus federation.

use serde::Deserialize;
use url::Url;

use crate::agent_bus::service::{
    agent_bus_base_url, AGENT_BUS_BRIDGE_ID, DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT,
    DEFAULT_AGENT_BUS_PORT,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBusPeerEndpoint {
    pub host: String,
    pub port: u16,
    pub base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct BridgeboardServiceRecord {
    pub id: String,
    pub owner_host: String,
    pub port: u16,
    #[serde(default)]
    pub local_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

pub fn agent_bus_peer_endpoint_from_bridgeboard_record(
    record: BridgeboardServiceRecord,
    local_host: &str,
) -> Option<AgentBusPeerEndpoint> {
    if record.id != AGENT_BUS_BRIDGE_ID || record.owner_host.eq_ignore_ascii_case(local_host) {
        return None;
    }
    let host = record.owner_host.to_ascii_lowercase();
    let base_url = record
        .local_url
        .or(record.url)
        .unwrap_or_else(|| agent_bus_base_url(record.port));
    let (port, base_url) = normalize_agent_bus_peer_url(record.port, &base_url);
    Some(AgentBusPeerEndpoint {
        host,
        port,
        base_url,
    })
}

pub fn normalize_agent_bus_peer_url(service_port: u16, base_url: &str) -> (u16, String) {
    let base_url = base_url.trim_end_matches('/');
    let port = agent_bus_port_from_base_url(base_url).unwrap_or(service_port);
    if service_port == DEFAULT_AGENT_BUS_PORT && port == DEFAULT_AGENT_BUS_PORT {
        (
            DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT,
            agent_bus_base_url(DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT),
        )
    } else {
        (port, base_url.to_string())
    }
}

#[cfg(test)]
fn parse_bridgeboard_ports_agent_bus_peers(
    text: &str,
    local_host: &str,
) -> Vec<AgentBusPeerEndpoint> {
    text.lines()
        .filter_map(|line| {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 9 || parts.get(1).copied() != Some(AGENT_BUS_BRIDGE_ID) {
                return None;
            }
            let host = parts.get(2)?.to_ascii_lowercase();
            if host.eq_ignore_ascii_case(local_host) {
                return None;
            }
            let base_url = parts.last()?.to_string();
            let service_port = parts.first()?.parse::<u16>().ok()?;
            let (port, base_url) = normalize_agent_bus_peer_url(service_port, &base_url);
            Some(AgentBusPeerEndpoint {
                host,
                port,
                base_url,
            })
        })
        .collect()
}

fn agent_bus_port_from_base_url(base_url: &str) -> Option<u16> {
    Url::parse(base_url).ok()?.port_or_known_default()
}

pub fn dedupe_agent_bus_peer_endpoints(
    peers: Vec<AgentBusPeerEndpoint>,
) -> Vec<AgentBusPeerEndpoint> {
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for peer in peers {
        let key = format!("{}\u{1f}{}", peer.host, peer.base_url);
        if seen.insert(key) {
            deduped.push(peer);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridgeboard_ports_parser_discovers_peer_agent_bus() {
        let text = "\
PORT   ID                 OWNER        MODE      LIFE       RESTART    DESIRED  STATUS          URL\n\
24260  cutex-agent-bus    host-a       external  manual     never      -        running         http://127.0.0.1:24260/\n\
24261  cutex-agent-bus    host-b       external  manual     never      -        remote-record   http://127.0.0.1:24261/\n";

        let peers = parse_bridgeboard_ports_agent_bus_peers(text, "host-a");

        assert_eq!(
            peers,
            vec![AgentBusPeerEndpoint {
                host: "host-b".to_string(),
                port: 24261,
                base_url: "http://127.0.0.1:24261".to_string(),
            }]
        );
    }

    #[test]
    fn bridgeboard_ports_parser_uses_tunnel_url_port_for_peer_agent_bus() {
        let text = "\
PORT   ID                 OWNER        MODE      LIFE       RESTART    DESIRED  STATUS                 URL\n\
24260  cutex-agent-bus    host-a       external  manual     never      -        external-running:123   http://127.0.0.1:24260/\n\
24260  cutex-agent-bus    host-b       external  manual     never      -        remote-record          http://127.0.0.1:24660/\n";

        let peers = parse_bridgeboard_ports_agent_bus_peers(text, "host-a");

        assert_eq!(
            peers,
            vec![AgentBusPeerEndpoint {
                host: "host-b".to_string(),
                port: 24660,
                base_url: "http://127.0.0.1:24660".to_string(),
            }]
        );
    }

    #[test]
    fn bridgeboard_ports_parser_maps_default_peer_port_to_tunnel() {
        let text = "\
PORT   ID                 OWNER        MODE      LIFE       RESTART    DESIRED  STATUS          URL\n\
24260  cutex-agent-bus    host-b       external  manual     never      -        remote-record   http://127.0.0.1:24260/\n";

        let peers = parse_bridgeboard_ports_agent_bus_peers(text, "host-a");

        assert_eq!(
            peers,
            vec![AgentBusPeerEndpoint {
                host: "host-b".to_string(),
                port: 24660,
                base_url: "http://127.0.0.1:24660".to_string(),
            }]
        );
    }

    #[test]
    fn bridgeboard_json_peer_record_maps_owner_local_url_to_tunnel() {
        let peer = agent_bus_peer_endpoint_from_bridgeboard_record(
            BridgeboardServiceRecord {
                id: AGENT_BUS_BRIDGE_ID.to_string(),
                owner_host: "host-a".to_string(),
                port: DEFAULT_AGENT_BUS_PORT,
                local_url: Some("http://127.0.0.1:24260/".to_string()),
                url: Some("http://127.0.0.1:24260/".to_string()),
            },
            "host-b",
        )
        .expect("peer record should produce endpoint");

        assert_eq!(
            peer,
            AgentBusPeerEndpoint {
                host: "host-a".to_string(),
                port: DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT,
                base_url: "http://127.0.0.1:24660".to_string(),
            }
        );
    }
}
