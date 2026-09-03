//! Shared cutex library surface.
//!
//! The current binary still owns most behavior in `main.rs`.  These modules are
//! the behavior-preserving extraction targets for the staged modularization
//! plan in `agent-log/260629-cutex-modularization-blueprint.md`.

pub mod agent_bus;
pub mod agent_management;
pub mod app_server;
pub mod bridgeboard;
pub mod catalog;
pub mod cli;
pub mod compat;
pub mod config;
pub mod http;
pub mod im;
pub mod launch;
pub mod management;
pub mod notify;
pub mod observability;
pub mod platform;
pub mod profiles;
pub mod role_revision;
pub mod rotation;
pub mod runtime;
pub mod seat;
pub mod session;
pub mod task_delivery;
/// Deterministic Task Service provider. The public surface is the revised v2
/// contract; the older transition engine remains crate-private implementation
/// evidence for the pre-deployment pilot.
pub mod task_service;
pub mod ui;
