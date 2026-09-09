//! Minimal project-owned Agent lifecycle service.
//!
//! The provider owns project authority, Agent ownership metadata, action
//! idempotency, and receipts. Process/thread/Agent Bus work remains behind the
//! production lifecycle adapter so the durable state machine can be tested
//! without creating real Agents.

mod archive;
mod bootstrap_intent;
mod durable_adoption;
mod durable_import;
mod explicit_launch;
mod model;
mod projects;
mod provider;
mod stock_runtime;
mod store;

pub use archive::*;
pub use bootstrap_intent::*;
pub use durable_adoption::*;
pub use durable_import::*;
pub use explicit_launch::*;
pub use model::*;
pub use projects::*;
pub use provider::*;
pub use stock_runtime::*;
pub use store::*;

pub const AGENT_MANAGEMENT_CONTRACT: &str = "cutex/agent-management/v1";
pub const AGENT_MANAGEMENT_START_CONTROL_TYPE: &str = "cutex.agent_management.start.v1";
pub const AGENT_MANAGEMENT_SYSTEM_SENDER: &str = "AgentManagementSystem";
pub const AGENT_MANAGEMENT_MAX_BODY_BYTES: usize = 256 * 1024;
