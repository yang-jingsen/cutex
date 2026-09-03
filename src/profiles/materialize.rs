//! Materialized profile file paths and pure profile-file helpers.

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use chrono::Utc;

use crate::config::paths::config_dir;
use crate::config::paths::host_codex_home_dir;
use crate::config::proxy::effective_proxy_config;
use crate::config::store::load_codez_config;
use crate::config::text::read_optional_text;
use crate::config::text::write_optional_text_if_changed;
use crate::profiles::deepseek;
use crate::profiles::model::AuthData;
use crate::profiles::model::CliKind;
use crate::profiles::model::CodezConfig;
use crate::profiles::model::CustomStatusItemCatalogEntry;
use crate::profiles::model::CustomStatusItemRender;
use crate::profiles::model::CustomStatusItemSource;
use crate::profiles::model::CustomStatusItemStyle;
use crate::profiles::model::CustomStatusItemsCatalogFile;
use crate::profiles::model::ImportedSnapshot;
use crate::profiles::model::MaterializedAccountFiles;
use crate::profiles::model::StoredAccount;
use crate::profiles::profile_config::extract_profile_config_toml;
use crate::profiles::profile_config::merge_and_write_config_toml;
use crate::profiles::profile_config::normalize_profile_config_for_account;
use crate::profiles::profile_config::parse_toml_table;
use crate::profiles::profile_config::profile_uses_local_model_catalog;

pub fn materialized_profiles_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("profiles"))
}

pub fn materialized_account_files(
    account: &StoredAccount,
) -> anyhow::Result<MaterializedAccountFiles> {
    let dir = materialized_profiles_dir()?.join(&account.id);
    Ok(MaterializedAccountFiles {
        auth_path: dir.join("auth.json"),
        config_path: dir.join("config.toml"),
        model_catalog_path: dir.join("models.json"),
        custom_status_items_path: dir.join("custom-status-items.json"),
    })
}

fn default_custom_status_items() -> Vec<CustomStatusItemCatalogEntry> {
    vec![
        CustomStatusItemCatalogEntry {
            id: "custom:bon-voyage".to_string(),
            title: "Bon voyage".to_string(),
            description: None,
            source: CustomStatusItemSource::Static {
                value: "Bon voyage !".to_string(),
            },
            render: CustomStatusItemRender::Value,
            style: CustomStatusItemStyle {
                fg: Some("#F6A3C8".to_string()),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underlined: false,
            },
        },
        CustomStatusItemCatalogEntry {
            id: "custom:profile".to_string(),
            title: "Profile".to_string(),
            description: None,
            source: CustomStatusItemSource::LaunchProfile,
            render: CustomStatusItemRender::Value,
            style: CustomStatusItemStyle {
                fg: Some("#FFFFFF".to_string()),
                bg: None,
                bold: true,
                dim: false,
                italic: false,
                underlined: false,
            },
        },
    ]
}

pub fn normalize_custom_status_items(
    items: &[CustomStatusItemCatalogEntry],
) -> Vec<CustomStatusItemCatalogEntry> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let defaults = default_custom_status_items();

    for item in items.iter().chain(defaults.iter()) {
        let id = item.id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }

        let title = item.title.trim();
        normalized.push(CustomStatusItemCatalogEntry {
            id: id.to_string(),
            title: if title.is_empty() {
                id.to_string()
            } else {
                title.to_string()
            },
            description: item
                .description
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            source: item.source.clone(),
            render: item.render.clone(),
            style: item.style.clone(),
        });
    }

    normalized
}

pub fn custom_status_items_catalog_json(config: &CodezConfig) -> anyhow::Result<Option<String>> {
    let items = normalize_custom_status_items(&config.custom_status_items);
    if items.is_empty() {
        return Ok(None);
    }

    Ok(Some(serde_json::to_string_pretty(
        &CustomStatusItemsCatalogFile { items },
    )?))
}

pub fn set_materialized_file_permissions(_files: &MaterializedAccountFiles) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&_files.auth_path, perms.clone())?;
        if _files.config_path.exists() {
            fs::set_permissions(&_files.config_path, perms.clone())?;
        }
        if _files.model_catalog_path.exists() {
            fs::set_permissions(&_files.model_catalog_path, perms.clone())?;
        }
        if _files.custom_status_items_path.exists() {
            fs::set_permissions(&_files.custom_status_items_path, perms)?;
        }
    }
    Ok(())
}

pub fn ensure_materialized_account_dir(files: &MaterializedAccountFiles) -> anyhow::Result<()> {
    if let Some(parent) = files.auth_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create account dir: {}", parent.display()))?;
    }
    Ok(())
}

pub fn ensure_materialized_account_files(
    account: &StoredAccount,
) -> anyhow::Result<MaterializedAccountFiles> {
    let files = materialized_account_files(account)?;
    ensure_materialized_account_dir(&files)?;

    if !files.auth_path.exists() {
        anyhow::bail!(
            "Profile '{}' is missing auth.json at {}. Re-import or restore this profile file.",
            account.name,
            files.auth_path.display()
        );
    }
    read_and_validate_account_auth(account, &files.auth_path)?;

    let profile_config = read_optional_text(&files.config_path)?
        .map(|existing| {
            extract_profile_config_toml(&existing).with_context(|| {
                format!(
                    "Failed to parse existing config.toml for profile '{}' at {}",
                    account.name,
                    files.config_path.display()
                )
            })
        })
        .transpose()?
        .flatten();
    let profile_config = normalize_profile_config_for_account(account, profile_config)?;
    let codez_config = load_codez_config();
    merge_and_write_config_toml(
        &files.config_path,
        profile_config.as_deref(),
        effective_proxy_config(account, &codez_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;
    ensure_profile_local_model_catalog(account, &files, profile_config.as_deref())?;
    let custom_status_items_json = custom_status_items_catalog_json(&codez_config)?;
    write_optional_text_if_changed(
        &files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&files)?;
    Ok(files)
}

pub fn validate_materialized_account_files(
    account: &StoredAccount,
) -> anyhow::Result<MaterializedAccountFiles> {
    let files = materialized_account_files(account)?;
    validate_materialized_account_files_at(account, &files)?;
    Ok(files)
}

fn validate_materialized_account_files_at(
    account: &StoredAccount,
    files: &MaterializedAccountFiles,
) -> anyhow::Result<()> {
    read_and_validate_account_auth(account, &files.auth_path)?;

    let config_toml = fs::read_to_string(&files.config_path).with_context(|| {
        format!(
            "Profile '{}' is missing or cannot read config.toml at {}. Open the profile editor to materialize it before using a one-launch override.",
            account.name,
            files.config_path.display()
        )
    })?;
    toml::from_str::<toml::Value>(&config_toml).with_context(|| {
        format!(
            "Failed to parse config.toml for profile '{}' at {}",
            account.name,
            files.config_path.display()
        )
    })?;

    if profile_uses_local_model_catalog(&config_toml)? {
        read_and_validate_model_catalog(account, files)?;
    }

    if files.custom_status_items_path.exists() {
        let custom_status_items = fs::read_to_string(&files.custom_status_items_path)
            .with_context(|| {
                format!(
                    "Failed to read custom-status-items.json for profile '{}' at {}",
                    account.name,
                    files.custom_status_items_path.display()
                )
            })?;
        serde_json::from_str::<CustomStatusItemsCatalogFile>(&custom_status_items).with_context(
            || {
                format!(
                    "Failed to parse custom-status-items.json for profile '{}' at {}",
                    account.name,
                    files.custom_status_items_path.display()
                )
            },
        )?;
    }

    Ok(())
}

fn read_and_validate_account_auth(
    account: &StoredAccount,
    auth_path: &Path,
) -> anyhow::Result<serde_json::Value> {
    let auth_json = fs::read_to_string(auth_path).with_context(|| {
        format!(
            "Profile '{}' is missing or cannot read auth.json at {}. Re-import or restore this profile file.",
            account.name,
            auth_path.display()
        )
    })?;
    let auth = serde_json::from_str::<serde_json::Value>(&auth_json).with_context(|| {
        format!(
            "Failed to parse auth.json for profile '{}' at {}",
            account.name,
            auth_path.display()
        )
    })?;
    validate_account_auth_payload(account, auth_path, &auth)?;
    Ok(auth)
}

fn validate_account_auth_payload(
    account: &StoredAccount,
    auth_path: &Path,
    auth: &serde_json::Value,
) -> anyhow::Result<()> {
    if account.source.as_deref() != Some("api-key") {
        return Ok(());
    }

    let has_api_key = auth
        .get("OPENAI_API_KEY")
        .or_else(|| auth.get("openai_api_key"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|key| !key.is_empty());
    if !has_api_key {
        anyhow::bail!(
            "Profile '{}' is configured for API-key authentication, but {} has no non-empty OPENAI_API_KEY. Re-import the profile or restore its API key.",
            account.name,
            auth_path.display()
        );
    }
    Ok(())
}

pub fn materialize_imported_account_files(
    account: &StoredAccount,
    snapshot: &ImportedSnapshot,
) -> anyhow::Result<()> {
    let files = materialized_account_files(account)?;
    ensure_materialized_account_dir(&files)?;

    write_optional_text_if_changed(&files.auth_path, Some(&snapshot.raw_auth_json))?;
    let codez_config = load_codez_config();
    let profile_config =
        normalize_profile_config_for_account(account, snapshot.raw_config_toml.clone())?;
    merge_and_write_config_toml(
        &files.config_path,
        profile_config.as_deref(),
        effective_proxy_config(account, &codez_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;
    write_optional_text_if_changed(
        &files.model_catalog_path,
        snapshot.raw_model_catalog_json.as_deref(),
    )?;
    ensure_profile_local_model_catalog(account, &files, profile_config.as_deref())?;
    let custom_status_items_json = custom_status_items_catalog_json(&codez_config)?;
    write_optional_text_if_changed(
        &files.custom_status_items_path,
        custom_status_items_json.as_deref(),
    )?;
    set_materialized_file_permissions(&files)?;
    Ok(())
}

pub fn sync_active_codex_home_files(
    account: &StoredAccount,
    files: &MaterializedAccountFiles,
) -> anyhow::Result<()> {
    if account.cli_kind != CliKind::Codex {
        return Ok(());
    }

    let codex_home = host_codex_home_dir()?;
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create CODEX_HOME: {}", codex_home.display()))?;

    let active_auth_path = codex_home.join("auth.json");
    let auth_json = fs::read_to_string(&files.auth_path)
        .with_context(|| format!("Failed to read auth.json: {}", files.auth_path.display()))?;
    write_optional_text_if_changed(&active_auth_path, Some(&auth_json))?;

    let profile_config = read_optional_text(&files.config_path)?
        .map(|existing| {
            extract_profile_config_toml(&existing).with_context(|| {
                format!(
                    "Failed to parse existing config.toml for profile '{}' at {}",
                    account.name,
                    files.config_path.display()
                )
            })
        })
        .transpose()?
        .flatten();
    let global_config = load_codez_config();
    merge_and_write_config_toml(
        &codex_home.join("config.toml"),
        profile_config.as_deref(),
        effective_proxy_config(account, &global_config)
            .map(|proxy| proxy.enabled)
            .unwrap_or(false),
    )?;

    let active_model_catalog_path = codex_home.join("models.json");
    let model_catalog = if profile_config
        .as_deref()
        .map(profile_uses_local_model_catalog)
        .transpose()?
        .unwrap_or(false)
    {
        Some(read_and_validate_model_catalog(account, files)?)
    } else {
        None
    };
    write_optional_text_if_changed(&active_model_catalog_path, model_catalog.as_deref())?;

    let active_custom_status_items_path = codex_home.join("custom-status-items.json");
    let custom_status_items = read_optional_text(&files.custom_status_items_path)?;
    write_optional_text_if_changed(
        &active_custom_status_items_path,
        custom_status_items.as_deref(),
    )?;

    set_active_codex_home_file_permissions(
        &active_auth_path,
        &codex_home.join("config.toml"),
        &active_model_catalog_path,
        &active_custom_status_items_path,
    )?;
    Ok(())
}

fn set_active_codex_home_file_permissions(
    _auth_path: &Path,
    _config_path: &Path,
    _model_catalog_path: &Path,
    _custom_status_items_path: &Path,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        for path in [
            _auth_path,
            _config_path,
            _model_catalog_path,
            _custom_status_items_path,
        ] {
            if path.exists() {
                fs::set_permissions(path, perms.clone())?;
            }
        }
    }
    Ok(())
}

fn ensure_profile_local_model_catalog(
    account: &StoredAccount,
    files: &MaterializedAccountFiles,
    profile_config_toml: Option<&str>,
) -> anyhow::Result<()> {
    let Some(profile_config_toml) = profile_config_toml else {
        return Ok(());
    };
    if !profile_uses_local_model_catalog(profile_config_toml)? {
        return Ok(());
    }

    if !files.model_catalog_path.exists() {
        let provider = parse_toml_table(profile_config_toml)?
            .get("model_provider")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        if provider
            .as_deref()
            .is_some_and(deepseek::is_deepseek_provider)
        {
            write_optional_text_if_changed(
                &files.model_catalog_path,
                Some(deepseek::model_catalog_json()),
            )?;
        } else {
            anyhow::bail!(
                "Profile '{}' references a profile-local models.json but it is missing at {}",
                account.name,
                files.model_catalog_path.display()
            );
        }
    }

    read_and_validate_model_catalog(account, files)?;
    Ok(())
}

fn read_and_validate_model_catalog(
    account: &StoredAccount,
    files: &MaterializedAccountFiles,
) -> anyhow::Result<String> {
    let contents = fs::read_to_string(&files.model_catalog_path).with_context(|| {
        format!(
            "Profile '{}' is missing or cannot read models.json at {}",
            account.name,
            files.model_catalog_path.display()
        )
    })?;
    serde_json::from_str::<serde_json::Value>(&contents).with_context(|| {
        format!(
            "Failed to parse models.json for profile '{}' at {}",
            account.name,
            files.model_catalog_path.display()
        )
    })?;
    Ok(contents)
}

pub fn legacy_auth_json_from_auth_data(auth: &AuthData) -> anyhow::Result<String> {
    let value = match auth {
        AuthData::ApiKey { key } => serde_json::json!({
            "OPENAI_API_KEY": key,
            "tokens": null,
            "last_refresh": null,
        }),
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id,
            },
            "last_refresh": Utc::now(),
        }),
    };

    Ok(serde_json::to_string_pretty(&value)?)
}

#[cfg(test)]
mod tests {
    use super::validate_materialized_account_files_at;
    use crate::profiles::model::{CliKind, MaterializedAccountFiles, RuntimeConfig, StoredAccount};
    use std::fs;
    use uuid::Uuid;

    fn test_account() -> StoredAccount {
        StoredAccount {
            id: "profile-id".to_string(),
            name: "alpha".to_string(),
            email: None,
            plan_type: None,
            source: None,
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        }
    }

    fn api_key_account() -> StoredAccount {
        StoredAccount {
            source: Some("api-key".to_string()),
            ..test_account()
        }
    }

    fn test_files() -> (std::path::PathBuf, MaterializedAccountFiles) {
        let dir = std::env::temp_dir().join(format!("cutex-profile-validate-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("test profile directory");
        let files = MaterializedAccountFiles {
            auth_path: dir.join("auth.json"),
            config_path: dir.join("config.toml"),
            model_catalog_path: dir.join("models.json"),
            custom_status_items_path: dir.join("custom-status-items.json"),
        };
        (dir, files)
    }

    #[test]
    fn validation_reads_complete_materialization_without_changing_files() {
        let (dir, files) = test_files();
        fs::write(&files.auth_path, r#"{"OPENAI_API_KEY":"test"}"#).expect("auth");
        fs::write(&files.config_path, "model = \"gpt-test\"\n").expect("config");
        fs::write(&files.custom_status_items_path, r#"{"items":[]}"#).expect("status");
        let before = [
            fs::read(&files.auth_path).expect("auth before"),
            fs::read(&files.config_path).expect("config before"),
            fs::read(&files.custom_status_items_path).expect("status before"),
        ];

        validate_materialized_account_files_at(&test_account(), &files)
            .expect("materialization should validate");

        assert_eq!(fs::read(&files.auth_path).expect("auth after"), before[0]);
        assert_eq!(
            fs::read(&files.config_path).expect("config after"),
            before[1]
        );
        assert_eq!(
            fs::read(&files.custom_status_items_path).expect("status after"),
            before[2]
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validation_requires_existing_valid_auth_and_config() {
        let (dir, files) = test_files();
        fs::write(&files.auth_path, "not-json").expect("invalid auth");
        fs::write(&files.config_path, "model = \"gpt-test\"\n").expect("config");
        let error = validate_materialized_account_files_at(&test_account(), &files)
            .expect_err("invalid auth must fail");
        assert!(error.to_string().contains("Failed to parse auth.json"));

        fs::write(&files.auth_path, "{}").expect("auth");
        fs::remove_file(&files.config_path).expect("remove config");
        let error = validate_materialized_account_files_at(&test_account(), &files)
            .expect_err("missing config must fail");
        assert!(error
            .to_string()
            .contains("missing or cannot read config.toml"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validation_requires_a_nonempty_key_for_api_key_profiles() {
        let (dir, files) = test_files();
        fs::write(
            &files.auth_path,
            r#"{"OPENAI_API_KEY":null,"tokens":{"refresh_token":"stale"}}"#,
        )
        .expect("auth");
        fs::write(&files.config_path, "model = \"gpt-test\"\n").expect("config");

        let error = validate_materialized_account_files_at(&api_key_account(), &files)
            .expect_err("missing API key must fail before launch");

        assert!(error
            .to_string()
            .contains("configured for API-key authentication"));
        assert!(error.to_string().contains("OPENAI_API_KEY"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validation_accepts_the_legacy_lowercase_api_key_alias() {
        let (dir, files) = test_files();
        fs::write(&files.auth_path, r#"{"openai_api_key":" test-key "}"#).expect("auth");
        fs::write(&files.config_path, "model = \"gpt-test\"\n").expect("config");

        validate_materialized_account_files_at(&api_key_account(), &files)
            .expect("lowercase API key alias should validate");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn validation_requires_a_valid_profile_local_model_catalog_when_configured() {
        let (dir, files) = test_files();
        fs::write(&files.auth_path, "{}").expect("auth");
        fs::write(&files.config_path, "model_catalog_json = \"models.json\"\n").expect("config");

        let error = validate_materialized_account_files_at(&test_account(), &files)
            .expect_err("missing catalog must fail");
        assert!(error
            .to_string()
            .contains("missing or cannot read models.json"));

        fs::write(&files.model_catalog_path, "not-json").expect("invalid catalog");
        let error = validate_materialized_account_files_at(&test_account(), &files)
            .expect_err("invalid catalog must fail");
        assert!(error.to_string().contains("Failed to parse models.json"));

        fs::write(&files.model_catalog_path, r#"{"models":[]}"#).expect("valid catalog");
        validate_materialized_account_files_at(&test_account(), &files)
            .expect("valid catalog should pass");
        let _ = fs::remove_dir_all(dir);
    }
}
