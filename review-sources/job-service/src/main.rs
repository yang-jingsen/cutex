use cutex_job_service::{
    CompletionDeliveryConfig, CompletionDeliveryWorker, JobError, JobService, ServerConfig,
    ServiceConfig,
};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("__runner") => cutex_job_service::private_runner_main(&args[1..]),
        Some("__sentinel") => cutex_job_service::private_sentinel_main(&args[1..]),
        Some("serve") => serve(&args[1..]),
        Some("mcp-stdio") => mcp_stdio(&args[1..]),
        _ => Err(JobError::Invalid(
            "usage: cutex-job-service serve STATE_ROOT SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER [--completion ENDPOINT TOKEN_FILE | --completion-v2 ENDPOINT TOKEN_FILE] | mcp-stdio SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER".into(),
        )),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn mcp_stdio(args: &[String]) -> Result<(), JobError> {
    if args.len() != 4 {
        return Err(JobError::Invalid(
            "mcp-stdio requires SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER".into(),
        ));
    }
    let api_token = read_secret(&args[1])?;
    let grant_key = read_secret(&args[2])?;
    let launcher = std::fs::canonicalize(&args[3])?;
    let launcher_sha256 = cutex_job_service::file_sha256(&launcher)?;
    cutex_job_service::serve_mcp_stdio(cutex_job_service::McpAdapterConfig {
        socket_path: args[0].clone().into(),
        api_token,
        grant_key,
        launcher_path: launcher.to_string_lossy().into_owned(),
        launcher_sha256,
    })
}

fn serve(args: &[String]) -> Result<(), JobError> {
    if args.len() != 5 && args.len() != 8 {
        return Err(JobError::Invalid(
            "serve requires STATE_ROOT SOCKET API_TOKEN_FILE GRANT_KEY_FILE SANDBOX_LAUNCHER [--completion ENDPOINT TOKEN_FILE]"
                .into(),
        ));
    }
    if args.len() == 8 && !matches!(args[5].as_str(), "--completion" | "--completion-v2") {
        return Err(JobError::Invalid(
            "optional completion configuration requires --completion or --completion-v2 ENDPOINT TOKEN_FILE".into(),
        ));
    }
    let api = read_secret(&args[2])?;
    let grant = read_secret(&args[3])?;
    let launcher = std::fs::canonicalize(&args[4])?;
    let launcher_path = launcher.to_string_lossy().into_owned();
    let launcher_sha = cutex_job_service::file_sha256(&launcher)?;
    let service = JobService::open(ServiceConfig {
        state_root: args[0].clone().into(),
        grant_key: grant,
        api_token: api,
        runner_executable: std::env::current_exe()?,
        per_stream_output_bytes: 1024 * 1024,
        max_read_bytes: 64 * 1024,
        cancel_grace: Duration::from_secs(2),
        max_jobs: 1024,
        max_active_jobs: 16,
        allowed_launchers: [(launcher_path, launcher_sha)].into_iter().collect(),
        completion_wire_version: if args.get(5).is_some_and(|value| value == "--completion-v2") {
            cutex_job_service::CompletionWireVersion::V2
        } else {
            cutex_job_service::CompletionWireVersion::V1
        },
    })?;
    let _completion_worker = if args.len() == 8 {
        Some(CompletionDeliveryWorker::start(
            service.clone(),
            CompletionDeliveryConfig {
                endpoint: args[6].clone(),
                token_file: args[7].clone().into(),
                request_timeout: Duration::from_secs(5),
                minimum_backoff: Duration::from_secs(2),
                maximum_backoff: Duration::from_secs(5 * 60),
                idle_interval: Duration::from_millis(250),
            },
        )?)
    } else {
        None
    };
    cutex_job_service::serve_local(
        service,
        ServerConfig {
            socket_path: args[1].clone().into(),
        },
    )
}

fn read_secret(path: &str) -> Result<Vec<u8>, JobError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(JobError::Invalid(
            "secret file must be regular, current-user owned, and mode 0600 or stricter".into(),
        ));
    }
    let value = std::fs::read(path)?;
    if value.len() < 32 || value.len() > 4096 {
        return Err(JobError::Invalid(
            "secret file length must be 32..=4096 bytes".into(),
        ));
    }
    Ok(value)
}
