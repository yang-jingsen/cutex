//! Agent bus service address and port helpers.

use anyhow::bail;

use crate::profiles::model::CodezConfig;

pub const DEFAULT_AGENT_BUS_PORT: u16 = 24260;
pub const DEFAULT_AGENT_BUS_PEER_TUNNEL_PORT: u16 = 24660;
pub const AGENT_BUS_BRIDGE_ID: &str = "cutex-agent-bus";

pub fn validate_agent_bus_port(port: u16) -> anyhow::Result<()> {
    if !(24000..=24999).contains(&port) {
        bail!("Agent bus port must be in the Bridgeboard 24xxx range");
    }
    Ok(())
}

pub fn agent_bus_port(config: &CodezConfig) -> u16 {
    config.agent_bus_port.unwrap_or(DEFAULT_AGENT_BUS_PORT)
}

pub fn agent_bus_base_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn agent_bus_health_url(port: u16) -> String {
    format!("{}/", agent_bus_base_url(port))
}
