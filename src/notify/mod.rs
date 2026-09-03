//! External notification bridges, including desktop notification support.
//!
//! Target boundary: notification delivery is separate from management events
//! and the agent bus.

pub mod desktop;
pub mod launch;
pub mod service;
