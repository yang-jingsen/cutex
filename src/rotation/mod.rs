//! Mechanical, Release-only controlled replacement.
//!
//! This module intentionally does not implement general role rotation or
//! semantic handoff. It owns one strict Release template and a fail-closed
//! transaction whose authority commit is persisted by the seat store.

mod model;
mod provider;
mod template_store;

pub use model::*;
pub use provider::*;
pub use template_store::*;
