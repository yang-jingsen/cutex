//! Version-isolated Codex app-server transport and protocol boundary.
//!
//! This module speaks JSON-RPC and preserves native app-server methods. Runtime
//! launch, management projection, and agent-bus policy live in their existing
//! subsystems.

pub mod activity_bridge;
pub mod bus_bridge;
pub mod client;
pub mod commands;
pub mod journal;
pub mod manager;
pub mod participants;
pub mod protocol;
pub mod runtime;
