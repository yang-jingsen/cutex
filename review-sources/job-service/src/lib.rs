mod completion;
mod grant;
mod mcp_adapter;
mod model;
mod process;
mod server;
mod service;
mod store;

pub use grant::{CallerGrantIssuer, GrantIssuer, TrustedSandboxContext, file_sha256};
pub use mcp_adapter::{McpAdapterConfig, serve_mcp_stdio};
pub use model::*;
pub use server::{ServerConfig, serve_local};
pub use service::{JobService, ServiceConfig};

#[doc(hidden)]
pub fn private_runner_main(args: &[String]) -> Result<(), JobError> {
    process::runner_main(args)
}

#[doc(hidden)]
pub fn private_sentinel_main(args: &[String]) -> Result<(), JobError> {
    process::sentinel_main(args)
}
pub use completion::{CompletionDeliveryConfig, CompletionDeliveryWorker};
