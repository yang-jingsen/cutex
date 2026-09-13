//! Persistent Runtime Host (PRH) protocol and supervision core.
//!
//! [`FakeHost`] exercises the complete v1 contract against an in-memory
//! registry and deterministic process model. The runnable host uses the same
//! contract with platform-specific Linux process-group or Windows Job Object
//! containment and a user-local transport.

pub mod fake;
pub mod file_logs;
pub mod file_registry;
#[cfg(target_os = "linux")]
pub mod linux_process;
pub mod local_client;
pub mod local_paths;
pub mod local_server;
pub mod logs;
pub mod model;
pub mod process_backend;
pub mod protocol;
pub mod registry;
pub mod transport;
pub mod tray_model;
#[cfg(target_os = "windows")]
pub mod windows_install;
#[cfg(target_os = "windows")]
pub mod windows_pipe;
#[cfg(target_os = "windows")]
pub mod windows_process;
#[cfg(target_os = "windows")]
mod windows_security;
#[cfg(target_os = "windows")]
pub mod windows_service;
pub mod windows_support;

pub use fake::{EventSubscription, FakeHost, FakeHostConfig};
pub type HostController = FakeHost;
pub use file_logs::{FileLogConfig, RotatingFileLogs};
pub use file_registry::FileRegistry;
pub use local_client::{LocalClient, LocalClientError};
pub use local_paths::{default_state_dir, RuntimePaths, SingleInstanceGuard};
pub use local_server::{run_local_server, seed_file_log_sequences, spawn_backend_event_pump};
pub use logs::{LogBufferConfig, LogSubscription};
pub use model::*;
pub use process_backend::{BackendEvent, ProcessBackend, ProcessBackendError};
pub use protocol::*;
pub use registry::{MemoryRegistry, RegistrySnapshot, RegistryStore, StoredDefinition};
pub use tray_model::{project_tray_status, TrayProjection, TrayServiceProjection};
#[cfg(target_os = "windows")]
pub use windows_install::{
    install_or_upgrade_windows, query_windows_tray_run_value, read_windows_installation_manifest,
    rollback_windows_installation, uninstall_windows, BinaryManifest, InstalledRelease,
    WindowsInstallOptions, WindowsInstallationManifest, WindowsUninstallReceipt,
};
#[cfg(target_os = "windows")]
pub use windows_process::{WindowsProcessBackend, WindowsProcessConfig};
#[cfg(target_os = "windows")]
pub use windows_security::{configure_operator_sid, current_user_sid_string};
#[cfg(target_os = "windows")]
pub use windows_service::{
    query_windows_service, restart_windows_service, start_windows_service, stop_windows_service,
    WindowsServiceStatus, DEFAULT_WINDOWS_INSTALL_ROOT, DEFAULT_WINDOWS_RUN_VALUE_NAME,
    DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME, DEFAULT_WINDOWS_SERVICE_NAME, DEFAULT_WINDOWS_STATE_DIR,
};
pub use windows_support::{build_windows_command_line, quote_windows_argument};
