//! OS-specific process, terminal, filesystem, and permission helpers.
//!
//! Target boundary: centralize `cfg(unix)` / `cfg(windows)` behavior here
//! instead of scattering platform checks through domain logic.

pub mod command;
pub mod host;
#[cfg(windows)]
pub mod private_fs;
pub mod process;

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
