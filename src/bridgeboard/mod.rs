//! Bridgeboard handoff, service discovery, and tunnel helpers.
//!
//! Target boundary: other modules should ask this adapter for peer services
//! instead of invoking Bridgeboard shell commands directly.

pub mod agent_bus;
