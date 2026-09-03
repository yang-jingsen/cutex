//! Minimal project-owned Agent lifecycle service.
//!
//! The provider owns project authority, Agent ownership metadata, action
//! idempotency, and receipts. Process/thread/Agent Bus work remains behind the
//! production lifecycle adapter so the durable state machine can be tested
//! without creating real Agents.

mod model;
mod provider;
mod store;

pub use model::*;
pub use provider::*;
pub use store::*;

pub const AGENT_MANAGEMENT_CONTRACT: &str = "cutex/agent-management/v1";
pub const AGENT_MANAGEMENT_START_CONTROL_TYPE: &str = "cutex.agent_management.start.v1";
pub const AGENT_MANAGEMENT_SYSTEM_SENDER: &str = "AgentManagementSystem";
pub const AGENT_MANAGEMENT_MAX_BODY_BYTES: usize = 256 * 1024;
