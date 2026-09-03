//! Command construction for cute-codex, Claude-compatible CLIs, Docker, proxy
//! policy, and launch-time environment injection.
//!
//! Target boundary: this module builds commands; durable lifecycle policy lives
//! in `runtime`.

pub mod args;
pub mod command;
pub mod docker;
pub mod env;
pub mod profile;
pub mod program;
pub mod runtime;
