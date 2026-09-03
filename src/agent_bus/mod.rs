//! Inter-agent registry, message routing, delivery modes, and federation.
//!
//! Target boundary: keep volatile `runtime_agent_id` routing here, while durable
//! `cutex_session` identity remains in `session`.

pub mod audit;
pub mod client;
pub mod delivery;
pub mod federation;
pub mod groups;
pub mod identity;
pub mod launch;
pub mod message;
pub mod model;
pub mod queue;
pub mod routing;
pub mod server;
pub mod service;
pub mod store;
