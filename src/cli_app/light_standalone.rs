//! Ordinary foreground sessions using the installed light CLI's explicit inputs.
//! No durable identity, managed owner, or runtime review action is created here.
use anyhow::Context;
use cutex::launch::local_deployment::LocalDeployment;
use cutex::launch::selected_profile::Config;
use cutex::launch::selected_status::Status;
use cutex::profiles::model::{CliKind, RuntimeConfig, StoredAccount};
use std::path::{Path, PathBuf};

fn executable_path(program: &str) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) {
        return Path::new(program).canonicalize().ok();
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .find_map(|dir| dir.join(program).canonicalize().ok())
}

pub(super) fn command(
    account: &StoredAccount,
    args: &[String],
) -> anyhow::Result<Option<std::process::Command>> {
    if account.cli_kind != CliKind::Codex || !matches!(account.runtime, RuntimeConfig::Host) {
        return Ok(None);
    }
    let Some(deployment) = LocalDeployment::selected()? else {
        return Ok(None);
    };
    let bundle: cutex::launch::stock::StockBundle =
        serde_json::from_slice(&std::fs::read(&deployment.bundle_manifest)?)?;
    let cli = bundle.cli.as_ref().unwrap_or(&bundle.executable);
    let program = cutex::launch::program::codex_program();
    if executable_path(&program).as_deref() != Some(cli.path.canonicalize()?.as_path()) {
        return Ok(None);
    }
    let files = cutex::profiles::materialize::ensure_materialized_account_files(account)?;
    let mut value: toml::Value = toml::from_str(&std::fs::read_to_string(&files.config_path)?)?;
    let table = value
        .as_table_mut()
        .context("profile configuration must be a table")?;
    table.insert("cutex_provider_mode".into(), "selected_profile_v2".into());
    // Profiles may intentionally inherit model, effort, and TUI settings from the native home.
    // Resolve only those missing values; never rewrite the stored profile.
    if ["model", "model_reasoning_effort", "tui"]
        .iter()
        .any(|key| !table.contains_key(*key))
    {
        let shared: toml::Value = toml::from_str(&std::fs::read_to_string(
            deployment.native_home.join("config.toml"),
        )?)?;
        for key in ["model", "model_reasoning_effort", "tui"] {
            if !table.contains_key(key) {
                if let Some(value) = shared.get(key) {
                    table.insert(key.into(), value.clone());
                }
            }
        }
    }

    let legacy_job = table
        .get("mcp_servers")
        .and_then(|servers| servers.get("cutex_job"))
        .cloned();
    let (mut projection, model, reasoning) = Config::parse(&toml::to_string(&value)?)?.review(
        &account.id,
        files.auth_path,
        None,
        None,
    )?;
    if let Some(tui) = &projection.settings.tui {
        projection.status = Status::review(
            &files.custom_status_items_path,
            tui.status_line.as_deref().unwrap_or_default(),
            &account.name,
        )?;
    }
    let mut launch = cutex::launch::command::LaunchCommand::new(program)
        .env("CODEX_HOME", deployment.native_home.to_string_lossy())
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_INSTALL_DIR")
        .env_remove("CUTEX_AGENT_ID")
        .env_remove("CUTEX_RUNTIME_GENERATION")
        .env_remove("CUTEX_AGENT_BUS_URL")
        .env_remove("CUTEX_AGENT_BUS_TOKEN")
        .args(projection.migrated_native_args(
            true,
            &cutex::config::paths::host_codex_home_dir()?,
            &deployment.native_home,
        )?);
    launch = super::stock_lifecycle::option(
        launch,
        "model_provider",
        match projection.route {
            cutex::launch::selected_profile::Route::ChatgptFile => "openai",
            cutex::launch::selected_profile::Route::GlmApiKey => "GLM",
        },
    )?;
    launch = super::stock_lifecycle::option(launch, "model", model)?;
    if let Some(reasoning) = reasoning {
        launch = super::stock_lifecycle::option(launch, "model_reasoning_effort", reasoning)?;
    }
    if projection.requires_job {
        // Job's trusted-runtime adapter requires CUTEX_AGENT_ID. A plain
        // session has no managed identity; keep that adapter explicitly off.
        let mut job = legacy_job.context("Selected Job configuration missing")?;
        job.as_table_mut()
            .context("Job configuration must be a table")?
            .insert("enabled".into(), false.into());
        launch = super::stock_lifecycle::option(launch, "mcp_servers.cutex_job", job)?;
        if !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
        {
            eprintln!(
                "Ordinary session: Job Service is available after adoption as a managed Agent."
            );
        }
    }
    if let Some(status) = &projection.status {
        launch = launch
            .arg("--status-items-file")
            .arg(status.materialize()?.to_string_lossy());
    }
    launch = super::stock_lifecycle::option(launch, "tui.status_line",
        super::notify::status_line(projection.settings.tui.as_ref().and_then(|t| t.status_line.as_ref())))?;
    // Explicit invocation options take precedence over the selected profile.
    let mut command = launch.args(args.iter().cloned()).to_command();
    command.env("CUTEX_NOTIFICATION_CONTROL", std::env::current_exe()?);
    if let Some(secret) = projection.secret()? {
        secret.apply(&mut command);
    }
    Ok(Some(command))
}
