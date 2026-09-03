use std::fs;
use std::process::Command;

use anyhow::{anyhow, Context};
use chrono::Utc;
use uuid::Uuid;

use cutex::config::env::*;
use cutex::config::paths::login_runtime_root;
use cutex::config::proxy::proxy_envs;
use cutex::config::store::load_codez_config;
use cutex::launch::docker::{default_docker_user_name, normalize_docker_user_name};
use cutex::launch::env::materialized_claude_config_dir;
use cutex::launch::program::{claude_program, codex_program};
use cutex::profiles::deepseek;
use cutex::profiles::import::import_snapshot;
use cutex::profiles::lookup::ensure_unique_name;
use cutex::profiles::materialize::materialize_imported_account_files;
use cutex::profiles::model::{
    runtime_label, AccountsStore, CliKind, ImportedSnapshot, RuntimeConfig, StoredAccount,
};
use cutex::profiles::store::save_store;

use super::account_store::load_store;
use super::prompt::{prompt_choice, prompt_line};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";

pub(crate) fn add(
    auth_path: &str,
    config_path: Option<&str>,
    docker_image: Option<String>,
    docker_user_name: Option<String>,
    name: &str,
    cli: &str,
) -> anyhow::Result<()> {
    let cli_kind: CliKind = cli.parse()?;
    let mut store = load_store()?;
    ensure_unique_name(&store, name, None)?;

    let runtime = runtime_from_option(docker_image, docker_user_name);

    if cli_kind == CliKind::Claude {
        let id = Uuid::new_v4().to_string();
        let account = StoredAccount {
            id,
            name: name.to_string(),
            email: None,
            plan_type: None,
            source: Some("anthropic".to_string()),
            runtime,
            proxy: None,
            session: None,
            cli_kind: CliKind::Claude,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: Some(Utc::now()),
        };
        ensure_claude_profile_dir(&account, auth_path)?;
        add_account_to_store(&mut store, account)
    } else {
        let snapshot = import_snapshot(auth_path, config_path)?;
        let mut account = StoredAccount::from_import(name.to_string(), &snapshot, runtime);
        account.cli_kind = cli_kind;
        materialize_imported_account_files(&account, &snapshot)?;
        add_account_to_store(&mut store, account)
    }
}

pub(crate) fn login(
    name: Option<&str>,
    cli: Option<&str>,
    api_key: Option<&str>,
    base_url: Option<&str>,
    provider: Option<&str>,
) -> anyhow::Result<()> {
    if name.is_none() && api_key.is_none() {
        return login_interactive();
    }

    let cli_str = cli.unwrap_or("codex");
    let cli_kind: CliKind = cli_str.parse()?;

    if let Some(key) = api_key {
        let profile_name = name.ok_or_else(|| anyhow!("--name is required with --api-key"))?;
        return login_api_key(
            profile_name,
            &cli_kind,
            key,
            base_url,
            provider.unwrap_or("custom"),
        );
    }

    let profile_name = name.ok_or_else(|| anyhow!("--name is required for official login"))?;
    let mut store = load_store()?;
    ensure_unique_name(&store, profile_name, None)?;

    match cli_kind {
        CliKind::Claude => login_claude_official(profile_name, &mut store),
        CliKind::Codex => login_codex_official(profile_name, &mut store),
    }
}

pub(crate) fn login_interactive() -> anyhow::Result<()> {
    println!("{BOLD}{CYAN}cutex login{RESET}\n");

    println!("{BOLD}Step 1:{RESET} Choose CLI");
    let cli_choice = prompt_choice(
        "CLI",
        &[
            ("codex", "OpenAI Codex"),
            ("claude", "Anthropic Claude Code"),
        ],
        1,
    )?;
    let cli_kind = if cli_choice == 2 {
        CliKind::Claude
    } else {
        CliKind::Codex
    };
    println!();

    println!("{BOLD}Step 2:{RESET} Choose auth method");
    let auth_choice = prompt_choice(
        "Auth",
        &[
            ("official", "OAuth login"),
            ("api-key", "Third-party API key + base URL"),
        ],
        1,
    )?;
    println!();

    if auth_choice == 2 {
        let default_url = match cli_kind {
            CliKind::Codex => "https://api.openai.com/v1",
            CliKind::Claude => "https://api.anthropic.com",
        };
        let url = prompt_line(&format!("{BOLD}Step 3:{RESET} API base URL"), default_url)?;
        println!();

        let key = prompt_line(&format!("{BOLD}Step 4:{RESET} API key"), "")?;
        if key.is_empty() {
            anyhow::bail!("API key cannot be empty");
        }
        println!();

        let prov = prompt_line(
            &format!("{BOLD}Step 5:{RESET} Provider name (for display)"),
            "custom",
        )?;
        println!();

        let name = prompt_line(&format!("{BOLD}Step 6:{RESET} Profile name"), "")?;
        if name.is_empty() {
            anyhow::bail!("Profile name cannot be empty");
        }

        return login_api_key(&name, &cli_kind, &key, Some(url.as_str()), &prov);
    }

    let name = prompt_line(&format!("{BOLD}Step 3:{RESET} Profile name"), "")?;
    if name.is_empty() {
        anyhow::bail!("Profile name cannot be empty");
    }
    println!();

    let mut store = load_store()?;
    ensure_unique_name(&store, &name, None)?;

    match cli_kind {
        CliKind::Claude => login_claude_official(&name, &mut store),
        CliKind::Codex => login_codex_official(&name, &mut store),
    }
}

fn login_api_key(
    name: &str,
    cli_kind: &CliKind,
    api_key: &str,
    base_url: Option<&str>,
    provider: &str,
) -> anyhow::Result<()> {
    let mut store = load_store()?;
    ensure_unique_name(&store, name, None)?;

    let id = Uuid::new_v4().to_string();
    let account = StoredAccount {
        id,
        name: name.to_string(),
        email: None,
        plan_type: Some(provider.to_string()),
        source: Some("api-key".to_string()),
        runtime: RuntimeConfig::Host,
        proxy: None,
        session: None,
        cli_kind: cli_kind.clone(),
        default_cli_args: Vec::new(),
        agent_name: None,
        last_used_at: Some(Utc::now()),
    };

    match cli_kind {
        CliKind::Codex => {
            let auth_json = serde_json::json!({
                "OPENAI_API_KEY": api_key,
                "tokens": null
            });
            let config_toml_str = codex_api_key_config_toml(provider, base_url);
            let snapshot = ImportedSnapshot {
                raw_auth_json: auth_json.to_string(),
                raw_config_toml: Some(config_toml_str),
                raw_model_catalog_json: deepseek::is_deepseek_provider(provider)
                    .then(|| deepseek::model_catalog_json().to_string()),
                email: None,
                plan_type: Some(provider.to_string()),
                source: "api-key".to_string(),
            };
            materialize_imported_account_files(&account, &snapshot)?;
        }
        CliKind::Claude => {
            let profile_dir = materialized_claude_config_dir(&account);
            fs::create_dir_all(&profile_dir)?;

            let api_key_path = profile_dir.join("api_key");
            fs::write(&api_key_path, api_key)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&api_key_path, fs::Permissions::from_mode(0o600))?;
            }

            if let Some(url) = base_url {
                let provider_json = serde_json::json!({
                    "provider": provider,
                    "base_url": url
                });
                fs::write(
                    profile_dir.join("provider.json"),
                    serde_json::to_string_pretty(&provider_json)?,
                )?;
            }
        }
    }

    add_account_to_store(&mut store, account)?;
    println!(
        "{GREEN}Profile {BOLD}{name}{RESET}{GREEN} created ({}, api-key, {provider}).{RESET}",
        cli_kind
    );
    Ok(())
}

pub(crate) fn codex_api_key_config_toml(provider: &str, base_url: Option<&str>) -> String {
    let provider = provider.trim();
    let mut root = toml::value::Table::new();
    if deepseek::is_deepseek_provider(provider) {
        deepseek::apply_profile_preset(&mut root, base_url)
            .expect("DeepSeek preset should produce a valid profile config");
    } else {
        root.insert(
            "model_provider".to_string(),
            toml::Value::String(provider.to_string()),
        );
        let mut provider_config = toml::value::Table::new();
        provider_config.insert(
            "name".to_string(),
            toml::Value::String(provider.to_string()),
        );
        if let Some(url) = base_url {
            provider_config.insert("base_url".to_string(), toml::Value::String(url.to_string()));
        }
        provider_config.insert(
            "env_key".to_string(),
            toml::Value::String("OPENAI_API_KEY".to_string()),
        );
        provider_config.insert(
            "wire_api".to_string(),
            toml::Value::String("responses".to_string()),
        );
        provider_config.insert(
            "requires_openai_auth".to_string(),
            toml::Value::Boolean(false),
        );
        let mut providers = toml::value::Table::new();
        providers.insert(provider.to_string(), toml::Value::Table(provider_config));
        root.insert("model_providers".to_string(), toml::Value::Table(providers));
    }
    toml::to_string_pretty(&root).expect("API-key profile config should serialize")
}

fn login_codex_official(name: &str, store: &mut AccountsStore) -> anyhow::Result<()> {
    let program = codex_program();
    let global_config = load_codez_config();

    let tmp_home = login_runtime_root()?.join(format!("codex-login-{}", Uuid::new_v4()));
    fs::create_dir_all(&tmp_home)
        .with_context(|| format!("Failed to create temp codex home: {}", tmp_home.display()))?;

    let tmp_config = tmp_home.join("config.toml");
    let config_contents = "cli_auth_credentials_store = \"file\"\n";
    fs::write(&tmp_config, config_contents)
        .with_context(|| format!("Failed to write temp config.toml: {}", tmp_config.display()))?;

    println!(
        "Starting `{}` for {BOLD}{}{RESET} using {}",
        format!("{program} login --device-auth"),
        name,
        tmp_home.display()
    );
    let mut command = Command::new(&program);
    scrub_codex_login_env(&mut command);
    command.env("CODEX_HOME", &tmp_home);
    for (key, value) in proxy_envs(global_config.proxy.as_ref(), None) {
        command.env(key, value);
    }
    let status = command
        .arg("login")
        .arg("--device-auth")
        .status()
        .with_context(|| format!("Failed to start {program} login"))?;

    if !status.success() {
        let _ = fs::remove_dir_all(&tmp_home);
        anyhow::bail!("{program} login exited with status {status}");
    }

    let auth_path = tmp_home.join("auth.json");
    if !auth_path.exists() {
        let _ = fs::remove_dir_all(&tmp_home);
        anyhow::bail!(
            "{program} login did not produce auth.json at {}",
            auth_path.display()
        );
    }

    let snapshot = import_snapshot(
        auth_path
            .to_str()
            .ok_or_else(|| anyhow!("Invalid auth.json path"))?,
        Some(
            tmp_config
                .to_str()
                .ok_or_else(|| anyhow!("Invalid config.toml path"))?,
        ),
    )?;
    let mut account = StoredAccount::from_import(name.to_string(), &snapshot, RuntimeConfig::Host);
    account.source = Some("official".to_string());
    materialize_imported_account_files(&account, &snapshot)?;
    add_account_to_store(store, account)?;

    let _ = fs::remove_dir_all(&tmp_home);
    Ok(())
}

pub(crate) fn codex_login_env_override_keys() -> &'static [&'static str] {
    &[
        CODEX_CONFIG_FILE_ENV_VAR,
        CODEX_AUTH_FILE_ENV_VAR,
        CODEX_CUSTOM_STATUS_ITEMS_FILE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_ENV_VAR,
        CODEX_LAUNCH_RUNTIME_ENV_VAR,
        CODEX_LAUNCH_PROFILE_SOURCE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_TYPE_ENV_VAR,
        CODEX_LAUNCH_PROFILE_EMAIL_ENV_VAR,
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
    ]
}

pub(crate) fn scrub_codex_login_env(command: &mut Command) {
    for key in codex_login_env_override_keys() {
        command.env_remove(key);
    }
}

fn login_claude_official(name: &str, store: &mut AccountsStore) -> anyhow::Result<()> {
    let program = claude_program();

    let tmp_claude_dir = login_runtime_root()?.join(format!("claude-login-{}", Uuid::new_v4()));
    fs::create_dir_all(&tmp_claude_dir).with_context(|| {
        format!(
            "Failed to create temp claude dir: {}",
            tmp_claude_dir.display()
        )
    })?;

    println!("Starting `{program}` for {BOLD}{name}{RESET} — please complete the OAuth login.",);
    println!("Using temp config dir: {}", tmp_claude_dir.display());
    println!("{DIM}Press Ctrl+C after login completes to return to cutex.{RESET}");

    let status = Command::new(&program)
        .env(CLAUDE_CONFIG_DIR_ENV_VAR, &tmp_claude_dir)
        .status()
        .with_context(|| format!("Failed to start {program}"))?;

    let credentials_path = tmp_claude_dir.join(".credentials.json");
    if !credentials_path.exists() {
        let _ = fs::remove_dir_all(&tmp_claude_dir);
        if !status.success() {
            anyhow::bail!("{program} exited with status {status} and no credentials were saved");
        }
        anyhow::bail!(
            "No credentials found at {} — login may not have completed.\nNote: if this system uses keychain auth, use `cutex add --cli claude --from-auth <path> --name {name}` instead.",
            credentials_path.display()
        );
    }

    let id = Uuid::new_v4().to_string();
    let account = StoredAccount {
        id: id.clone(),
        name: name.to_string(),
        email: None,
        plan_type: None,
        source: Some("anthropic".to_string()),
        runtime: RuntimeConfig::Host,
        proxy: None,
        session: None,
        cli_kind: CliKind::Claude,
        default_cli_args: Vec::new(),
        agent_name: None,
        last_used_at: Some(Utc::now()),
    };

    let profile_claude_dir = materialized_claude_config_dir(&account);
    fs::create_dir_all(&profile_claude_dir)?;

    let target_credentials = profile_claude_dir.join(".credentials.json");
    fs::copy(&credentials_path, &target_credentials)?;

    let settings_path = tmp_claude_dir.join("settings.json");
    if settings_path.exists() {
        fs::copy(&settings_path, profile_claude_dir.join("settings.json"))?;
    }

    let _ = fs::remove_dir_all(&tmp_claude_dir);
    add_account_to_store(store, account)?;
    println!("{GREEN}Claude profile {BOLD}{name}{RESET}{GREEN} created.{RESET}");
    Ok(())
}

fn ensure_claude_profile_dir(account: &StoredAccount, auth_path: &str) -> anyhow::Result<()> {
    let profile_dir = materialized_claude_config_dir(account);
    fs::create_dir_all(&profile_dir).with_context(|| {
        format!(
            "Failed to create Claude profile dir: {}",
            profile_dir.display()
        )
    })?;

    let target = profile_dir.join(".credentials.json");
    fs::copy(auth_path, &target)
        .with_context(|| format!("Failed to copy credentials to {}", target.display()))?;

    Ok(())
}

fn add_account_to_store(store: &mut AccountsStore, account: StoredAccount) -> anyhow::Result<()> {
    let source = account.source.as_deref().unwrap_or("unknown");
    let plan = account.plan_type.as_deref().unwrap_or("unknown");
    let email = account.email.as_deref().unwrap_or("-");
    let runtime = runtime_label(&account.runtime);

    store.accounts.push(account.clone());
    if store.active_account_id.is_none() {
        store.active_account_id = Some(account.id.clone());
    }
    save_store(store)?;

    println!(
        "{GREEN}Added{RESET} profile `{}` ({}, {}, {}, {})",
        account.name, source, plan, runtime, email
    );
    Ok(())
}

fn runtime_from_option(
    docker_image: Option<String>,
    docker_user_name: Option<String>,
) -> RuntimeConfig {
    match docker_image {
        Some(image) => RuntimeConfig::Docker {
            image,
            user_name: Some(
                normalize_docker_user_name(docker_user_name)
                    .unwrap_or_else(|_| default_docker_user_name()),
            ),
        },
        None => RuntimeConfig::Host,
    }
}
