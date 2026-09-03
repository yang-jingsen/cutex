//! Deterministic Task Service Stage 1 durable core.
//!
//! This module owns typed task state, a hash-chained journal, replayable
//! snapshot, exact-receipt transitions, and process-local page/watch delivery.
//! Semantic transitions remain separate from transport, authorization, UI, and
//! deployment adapters. The watchdog submodule owns a distinct durable
//! liveness ledger that observes bounded runtime projections without mutating
//! semantic Task state.

mod digest;
mod director_transport;
mod journal;
mod model;
mod operations;
mod owner_read;
mod persist;
mod provider;
mod store;
mod watchdog;

pub(crate) use digest::canonical_command_digest;
pub use director_transport::*;
pub use model::AttemptToken as LegacyAttemptToken;
pub(crate) use model::*;
pub(crate) use operations::TaskService;
pub use owner_read::*;
pub use provider::*;
pub use watchdog::*;

#[cfg(test)]
mod tests;
