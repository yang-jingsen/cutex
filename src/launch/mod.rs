//! Command construction for cute-codex, Claude-compatible CLIs, Docker, proxy
//! policy, and launch-time environment injection.
//!
//! Target boundary: this module builds commands; durable lifecycle policy lives
//! in `runtime`.

pub mod aemeath_auth;
pub mod args;
pub mod command;
pub mod docker;
pub mod env;
pub mod job_mcp;
pub mod local_deployment;
pub mod profile;
pub mod program;
pub mod runtime;
pub mod selected_profile;
pub mod selected_status;
pub mod stock;

pub mod session_display;

pub mod native_history;
