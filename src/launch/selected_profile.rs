//! Versioned, finite selected-profile projection. Not a general TOML importer.
//! Credential values never belong to this serializable review domain.
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::stock::VerifiedFile;
use crate::role_revision::Sha256;

pub const OCTOBRE_ID: &str = "b341adf8-7af9-432c-a12b-0e9a674458ed";
pub const GLM_ID: &str = "3f38c782-bac6-403d-b11d-c801326e0bb1";
pub const GLM_ENDPOINT: &str = "https://www.colabapi.com/v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    ChatgptFile,
    GlmApiKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthCustody {
    pub path: PathBuf,
    pub parent_device: u64,
    pub parent_inode: u64,
    pub owner: u32,
    pub account: Option<Sha256>,
    // Opaque API keys have no independently verifiable account ID. A file
    // replacement/write needs review; no key bytes or key digest are retained.
    pub api_file: Option<super::job_mcp::PrivateObject>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub version: Version,
    pub route: Route,
    pub auth: AuthCustody,
    pub settings: Settings,
    pub catalog: Option<VerifiedFile>,
    pub requires_job: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<super::selected_status::Status>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Version;
impl TryFrom<u8> for Version {
    type Error = &'static str;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        if v == 2 {
            Ok(Self)
        } else {
            Err("unsupported selected projection version")
        }
    }
}
impl From<Version> for u8 {
    fn from(_: Version) -> u8 {
        2
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub approvals_reviewer: Option<String>,
    pub service_tier: Option<String>,
    pub shell_environment_policy: Option<ShellPolicy>,
    #[serde(default)]
    pub projects: BTreeMap<String, Trust>,
    pub memories: Option<Memories>,
    pub model_auto_compact_token_limit: Option<u64>,
    pub skills: Option<Skills>,
    pub tui: Option<Tui>,
    pub notice: Option<Notice>,
    #[serde(default)]
    pub plugins: BTreeMap<String, Plugin>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellPolicy {
    pub exclude: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trust {
    pub trust_level: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memories {
    pub generate_memories: bool,
    pub use_memories: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skills {
    pub config: Vec<Skill>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub enabled: bool,
    pub path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tui {
    pub status_line: Vec<String>,
    pub status_line_use_colors: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_rate_limit_model_nudge: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
    pub enabled: bool,
}

// Explicit fields, rather than flatten + arbitrary config values. An opted-in
// private materialization adds the mode; original profiles are never rewritten.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub cutex_provider_mode: String,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub cli_auth_credentials_store: String,
    pub approvals_reviewer: Option<String>,
    pub service_tier: Option<String>,
    pub shell_environment_policy: Option<ShellPolicy>,
    #[serde(default)]
    pub projects: BTreeMap<String, Trust>,
    pub memories: Option<Memories>,
    pub model_auto_compact_token_limit: Option<u64>,
    pub model_catalog_json: Option<PathBuf>,
    pub skills: Option<Skills>,
    pub tui: Option<Tui>,
    pub notice: Option<Notice>,
    #[serde(default)]
    pub plugins: BTreeMap<String, Plugin>,
    #[serde(default)]
    pub model_providers: BTreeMap<String, GlmProvider>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, LegacyJob>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlmProvider {
    pub name: String,
    pub base_url: String,
    pub wire_api: String,
    pub requires_openai_auth: bool,
    pub env_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyJob {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_vars: Vec<String>,
}

impl Config {
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        toml::from_str(raw)
            .map_err(|_| anyhow::anyhow!("unsupported selected profile fields or types"))
    }
    pub fn review(
        self,
        id: &str,
        auth_path: PathBuf,
        selected_model: Option<&String>,
        selected_effort: Option<&String>,
    ) -> anyhow::Result<(Projection, String, Option<String>)> {
        ensure!(
            self.cutex_provider_mode == "selected_profile_v2",
            "explicit selected profile v2 required"
        );
        ensure!(
            self.cli_auth_credentials_store == "file",
            "selected profile requires File storage"
        );
        let route = match id {
            super::aemeath_auth::PROFILE_ID | OCTOBRE_ID => Route::ChatgptFile,
            GLM_ID => Route::GlmApiKey,
            _ => anyhow::bail!("profile ID outside selected projection scope"),
        };
        match route {
            Route::ChatgptFile => ensure!(
                self.model_provider.as_deref().is_none_or(|v| v == "openai")
                    && self.model_providers.is_empty(),
                "ChatGPT projection forbids provider bearer/endpoint overrides"
            ),
            Route::GlmApiKey => {
                ensure!(
                    self.model_provider.as_deref() == Some("GLM")
                        && self.model_providers.len() == 1,
                    "exact GLM provider required"
                );
                let p = self
                    .model_providers
                    .get("GLM")
                    .context("GLM provider missing")?;
                ensure!(
                    p.base_url == GLM_ENDPOINT
                        && p.wire_api == "responses"
                        && !p.requires_openai_auth
                        && p.env_key == "OPENAI_API_KEY"
                        && p.name == "GLM",
                    "unsupported GLM endpoint/TLS/credential mapping"
                );
            }
        }
        let model = selected_model
            .cloned()
            .or(self.model)
            .context("effective selected model missing")?;
        let effort = selected_effort.cloned().or(self.model_reasoning_effort);
        let catalog = self
            .model_catalog_json
            .map(|path| -> anyhow::Result<VerifiedFile> {
                validate_asset(&path)?;
                Ok(VerifiedFile {
                    sha256: crate::agent_management::file_sha256(&path)?,
                    path,
                })
            })
            .transpose()?;
        validate_model(&route, &model, effort.as_deref(), catalog.as_ref())?;
        ensure!(
            self.mcp_servers.keys().all(|k| k == "cutex_job"),
            "unreviewed MCP server forbidden"
        );
        for job in self.mcp_servers.values() {
            ensure!(
                !job.command.is_empty() && job.args.len() <= 32 && job.env_vars.len() <= 8,
                "invalid legacy Job declaration"
            );
        }
        // Original declaration remains covered by the profile digest. No byte
        // of its command/args/env is executed; an independent reviewed Job
        // descriptor is mandatory at the root review boundary.
        let settings = Settings {
            approvals_reviewer: self.approvals_reviewer,
            service_tier: self.service_tier,
            shell_environment_policy: self.shell_environment_policy,
            projects: self.projects,
            memories: self.memories,
            model_auto_compact_token_limit: self.model_auto_compact_token_limit,
            skills: self.skills,
            tui: self.tui,
            notice: self.notice,
            plugins: self.plugins,
        };
        settings.validate()?;
        let auth = review_auth(&auth_path, &route)?;
        Ok((
            Projection {
                version: Version,
                route,
                auth,
                settings,
                catalog,
                requires_job: !self.mcp_servers.is_empty(),
                status: None,
            },
            model,
            effort,
        ))
    }
}

impl Settings {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.approvals_reviewer
                .as_deref()
                .is_none_or(|v| v == "user"),
            "unsupported approval reviewer"
        );
        ensure!(
            self.service_tier.as_deref().is_none_or(|v| v == "default"),
            "unsupported service tier"
        );
        ensure!(
            self.model_auto_compact_token_limit.is_none_or(|v| v > 0),
            "invalid compaction limit"
        );
        for (path, trust) in &self.projects {
            ensure!(
                Path::new(path).is_absolute()
                    && matches!(trust.trust_level.as_str(), "trusted" | "untrusted"),
                "invalid reviewed project trust"
            );
        }
        if let Some(shell) = &self.shell_environment_policy {
            ensure!(
                shell.exclude.len() <= 64
                    && shell
                        .exclude
                        .iter()
                        .all(|s| !s.is_empty() && !s.chars().any(char::is_control)),
                "invalid shell exclusions"
            );
        }
        if let Some(skills) = &self.skills {
            for skill in &skills.config {
                ensure!(
                    skill.path.is_absolute()
                        && !skill
                            .path
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "invalid skill exclusion path"
                );
                ensure!(
                    !skill.enabled,
                    "enabled skill asset needs explicit custody contract"
                );
                // Disabled missing paths must stay disabled, never auto-created.
            }
        }
        ensure!(
            self.plugins.keys().all(|k| k == "sample@debug"),
            "unsupported plugin projection"
        );
        if let Some(tui) = &self.tui {
            ensure!(
                tui.status_line.iter().all(|s| matches!(
                    s.as_str(),
                    "model-with-reasoning"
                        | "current-dir"
                        | "context-used"
                        | "weekly-limit"
                        | "custom:profile"
                        | "custom:bon-voyage"
                )),
                "unsupported selected status item"
            );
        }
        Ok(())
    }
}

pub fn validate_model(
    route: &Route,
    model: &str,
    effort: Option<&str>,
    catalog: Option<&VerifiedFile>,
) -> anyhow::Result<()> {
    match route {
        Route::ChatgptFile => {
            ensure!(catalog.is_none(), "ChatGPT custom catalog not reviewed");
            // Exact 2eab lineage models-manager/models.json selected subset.
            ensure!(
                matches!(model, "gpt-6-astra" | "gpt-5.6-sol" | "gpt-5.6-terra")
                    && effort.is_some_and(|r| matches!(
                        r,
                        "low" | "medium" | "high" | "xhigh" | "max" | "ultra"
                    )),
                "model/effort absent from pinned selected catalog"
            );
        }
        Route::GlmApiKey => {
            ensure!(model == "glm-5.3", "selected GLM model must remain glm-5.3");
            let catalog = catalog.context("GLM reviewed catalog required")?;
            catalog.validate()?;
            let bytes = bounded_asset(&catalog.path)?;
            use sha2::Digest;
            ensure!(
                format!("{:x}", sha2::Sha256::digest(&bytes)) == catalog.sha256.as_str(),
                "catalog changed during projection"
            );
            let v: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid model catalog"))?;
            let models = v["models"].as_array().context("catalog models missing")?;
            let matched: Vec<_> = models
                .iter()
                .filter(|m| m["slug"].as_str() == Some(model))
                .collect();
            ensure!(
                matched.len() == 1
                    && matched[0]["supported_reasoning_levels"]
                        .as_array()
                        .is_some_and(|a| a
                            .iter()
                            .any(|v| v["effort"].as_str() == effort && effort.is_some())),
                "GLM catalog model/effort missing or ambiguous"
            );
        }
    }
    Ok(())
}

pub(super) fn bounded_asset(path: &Path) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut f = options.open(path)?;
    let before = f.metadata()?;
    ensure!(before.is_file(), "reviewed asset is not regular");
    ensure!(
        f.metadata()?.len() <= 4 * 1024 * 1024,
        "reviewed asset too large"
    );
    let mut bytes = Vec::new();
    (&mut f).take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let after = f.metadata()?;
        let named = std::fs::symlink_metadata(path)?;
        ensure!(
            before.dev() == named.dev()
                && before.ino() == named.ino()
                && before.len() == after.len()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "reviewed asset changed while reading"
        );
    }
    ensure!(bytes.len() <= 4 * 1024 * 1024, "reviewed asset grew");
    Ok(bytes)
}
pub(super) fn validate_asset(path: &Path) -> anyhow::Result<()> {
    super::stock::canonical(path)?;
    ensure!(path.is_file(), "reviewed asset missing");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(path)?;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0 && m.nlink() == 1,
            "reviewed asset custody invalid"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn review_auth(path: &Path, route: &Route) -> anyhow::Result<AuthCustody> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        path.is_absolute()
            && path.components().all(|c| matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "ordinary absolute auth path required"
    );
    let uid = unsafe { libc::geteuid() };
    for ancestor in path.ancestors().skip(1) {
        let m = std::fs::symlink_metadata(ancestor)?;
        ensure!(
            m.is_dir() && !m.file_type().is_symlink() && (m.uid() == 0 || m.uid() == uid),
            "untrusted auth ancestor"
        );
        ensure!(
            m.mode() & 0o022 == 0,
            "auth ancestor is group/other writable"
        );
    }
    let parent = std::fs::symlink_metadata(path.parent().context("auth parent missing")?)?;
    ensure!(
        parent.uid() == uid && parent.mode() & 0o7777 == 0o700,
        "auth parent must be owned0700"
    );
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && m.uid() == uid && m.mode() & 0o7777 == 0o600 && m.nlink() == 1,
        "auth file must be owned regular0600 single link"
    );
    let (account, api_file) = match route {
        Route::ChatgptFile => (
            Some(super::aemeath_auth::review(path.to_path_buf())?.account_sha256),
            None,
        ),
        Route::GlmApiKey => {
            let _secret = read_api_key(path)?;
            (
                None,
                Some(super::job_mcp::PrivateObject {
                    device: m.dev(),
                    inode: m.ino(),
                    size: m.len(),
                    changed_seconds: m.ctime(),
                    changed_nanos: m.ctime_nsec(),
                }),
            )
        }
    };
    let parent_after = std::fs::symlink_metadata(path.parent().unwrap())?;
    ensure!(
        parent_after.dev() == parent.dev()
            && parent_after.ino() == parent.ino()
            && parent_after.mode() == parent.mode(),
        "auth parent changed"
    );
    Ok(AuthCustody {
        path: path.into(),
        parent_device: parent.dev(),
        parent_inode: parent.ino(),
        owner: uid,
        account,
        api_file,
    })
}
#[cfg(not(target_os = "linux"))]
pub fn review_auth(_: &Path, _: &Route) -> anyhow::Result<AuthCustody> {
    anyhow::bail!("selected auth requires Linux")
}

// Deliberately neither Debug nor Serialize. Used only immediately before child
// spawn, never inserted into LaunchCommand's printable plan.
pub struct ApiKey(String);
impl ApiKey {
    pub fn apply(&self, command: &mut std::process::Command) {
        command.env("OPENAI_API_KEY", &self.0);
    }
    pub fn as_exec_env(&self) -> anyhow::Result<std::ffi::CString> {
        std::ffi::CString::new(format!("OPENAI_API_KEY={}", self.0))
            .map_err(|_| anyhow::anyhow!("invalid API credential"))
    }
}
#[cfg(target_os = "linux")]
fn read_api_key(path: &Path) -> anyhow::Result<ApiKey> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file()
            && before.len() <= 1024 * 1024
            && before.uid() == unsafe { libc::geteuid() }
            && before.mode() & 0o7777 == 0o600
            && before.nlink() == 1,
        "invalid API credential custody"
    );
    let mut bytes = Vec::new();
    (&mut file).take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    ensure!(
        before.dev() == named.dev()
            && before.ino() == named.ino()
            && before.len() == after.len()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "API credential changed while reading"
    );
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid API credential JSON"))?;
    ensure!(
        v.as_object().is_some_and(|o| o.keys().all(|k| matches!(
            k.as_str(),
            "OPENAI_API_KEY" | "auth_mode" | "tokens" | "last_refresh"
        ))) && v
            .get("auth_mode")
            .is_none_or(|m| m.is_null() || m.as_str() == Some("apikey"))
            && v.get("tokens").is_none_or(|v| v.is_null())
            && v.get("last_refresh").is_none_or(|v| v.is_null()
                || v.as_str()
                    .is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())),
        "unsupported API credential fields"
    );
    let key = v["OPENAI_API_KEY"]
        .as_str()
        .filter(|k| !k.is_empty() && !k.chars().any(char::is_control))
        .context("API credential missing")?;
    Ok(ApiKey(key.into()))
}
impl Projection {
    /// The same nonsecret argv projection is used by owner launch and the
    /// independent process test. Remote attach deliberately omits auth-file.
    pub fn native_args(&self, owner: bool) -> anyhow::Result<Vec<String>> {
        self.validate()?;
        let mut args = Vec::new();
        if owner {
            args.extend([
                "--auth-file".into(),
                self.auth
                    .path
                    .to_str()
                    .context("auth path must be UTF-8")?
                    .into(),
            ]);
        }
        let mut option = |key: &str, value: serde_json::Value| -> anyhow::Result<()> {
            let value = toml::Value::try_from(value)?;
            args.extend(["-c".into(), format!("{key}={value}")]);
            Ok(())
        };
        option("cli_auth_credentials_store", serde_json::json!("file"))?;
        if self.route == Route::GlmApiKey {
            option(
                "model_providers.GLM",
                serde_json::json!({"name":"GLM","base_url":GLM_ENDPOINT,"wire_api":"responses","requires_openai_auth":false,"env_key":"OPENAI_API_KEY"}),
            )?;
        }
        let settings = serde_json::to_value(&self.settings)?;
        for (key, value) in settings.as_object().context("typed settings missing")? {
            if !value.is_null() && !value.as_object().is_some_and(|v| v.is_empty()) {
                option(key, value.clone())?;
            }
        }
        if let Some(catalog) = &self.catalog {
            option(
                "model_catalog_json",
                serde_json::json!(catalog
                    .path
                    .to_str()
                    .context("catalog path must be UTF-8")?),
            )?;
        }
        Ok(args)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.settings.validate()?;
        let wants_status = self
            .settings
            .tui
            .as_ref()
            .is_some_and(|t| t.status_line.iter().any(|s| s.starts_with("custom:")));
        ensure!(
            wants_status == self.status.is_some(),
            "selected status projection missing or inappropriate"
        );
        if let Some(status) = &self.status {
            status.validate()?;
        }
        ensure!(
            review_auth(&self.auth.path, &self.route)? == self.auth,
            "selected auth custody/account changed"
        );
        if let Some(c) = &self.catalog {
            validate_asset(&c.path)?;
            c.validate()?;
        }
        Ok(())
    }
    pub fn secret(&self) -> anyhow::Result<Option<ApiKey>> {
        self.validate()?;
        #[cfg(target_os = "linux")]
        if self.route == Route::GlmApiKey {
            let key = read_api_key(&self.auth.path)?;
            ensure!(
                review_auth(&self.auth.path, &self.route)? == self.auth,
                "API credential changed before spawn"
            );
            return Ok(Some(key));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_catalog_keeps_models_and_max_without_fallback() {
        for model in ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra"] {
            for effort in ["low", "medium", "high", "xhigh", "max"] {
                assert!(validate_model(&Route::ChatgptFile, model, Some(effort), None).is_ok());
            }
        }
        for (model, effort) in [
            ("unknown", Some("low")),
            ("gpt-5.6-sol", Some("guessed")),
            ("gpt-5.6-sol", None),
        ] {
            assert!(validate_model(&Route::ChatgptFile, model, effort, None).is_err());
        }
        assert!(validate_model(&Route::GlmApiKey, "glm-5.3", Some("max"), None).is_err());
    }
    #[test]
    fn typed_profile_rejects_unknown_fields_and_credential_overrides() {
        let base = "cutex_provider_mode='selected_profile_v2'\ncli_auth_credentials_store='file'\n";
        assert!(Config::parse(base).is_ok());
        for extra in ["api_key='sentinel'", "notify=['shell']", "[model_providers.GLM]\nname='GLM'\nbase_url='https://www.colabapi.com/v1'\nwire_api='responses'\nrequires_openai_auth=false\nenv_key='OPENAI_API_KEY'\nhttp_headers={Authorization='sentinel'}", "[shell_environment_policy]\nset={SECRET='sentinel'}"] {
            assert!(Config::parse(&format!("{base}{extra}")).is_err());
        }
        for version in [0, 1, 3, 255] {
            assert!(serde_json::from_value::<Version>(version.into()).is_err());
        }
        assert_eq!(serde_json::to_value(Version).unwrap(), serde_json::json!(2));
    }
    #[test]
    fn settings_preserve_disabled_missing_skills_and_missing_plugin_intent() {
        let mut s = Settings::default();
        s.skills = Some(Skills {
            config: vec![Skill {
                enabled: false,
                path: "/private/missing/SKILL.md".into(),
            }],
        });
        s.plugins
            .insert("sample@debug".into(), Plugin { enabled: true });
        assert!(s.validate().is_ok());
        s.skills.as_mut().unwrap().config[0].enabled = true;
        assert!(s.validate().is_err());
        s.skills = None;
        s.tui = Some(Tui {
            status_line: vec!["custom:profile".into()],
            status_line_use_colors: true,
        });
        assert!(s.validate().is_ok());
        s.tui
            .as_mut()
            .unwrap()
            .status_line
            .push("custom:unknown".into());
        assert!(s.validate().is_err());
    }
    #[test]
    fn actual_catalog_reference_is_pinned_not_remote_claim() {
        // Fixture source identity is independently checked by the protocol test.
        assert_eq!(
            super::super::stock::S6E_SERVER_COMMIT,
            "2eab060b191a0fe59e22b28785d02d76eafb7fc4"
        );
        assert!(Settings {
            approvals_reviewer: Some("auto".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn selected_routes_reject_foreign_identity_and_endpoint_before_auth_read() {
        let raw = "cutex_provider_mode='selected_profile_v2'\ncli_auth_credentials_store='file'\n";
        assert!(Config::parse(raw)
            .unwrap()
            .review("foreign", "/no-auth-read".into(), None, None)
            .unwrap_err()
            .to_string()
            .contains("profile ID"));
        let provider="model_provider='GLM'\n[model_providers.GLM]\nname='GLM'\nbase_url='http://www.colabapi.com/v1'\nwire_api='responses'\nrequires_openai_auth=false\nenv_key='OPENAI_API_KEY'\n";
        let error = Config::parse(&format!("{raw}{provider}"))
            .unwrap()
            .review(GLM_ID, "/no-auth-read".into(), None, None)
            .unwrap_err();
        assert!(error.to_string().contains("endpoint/TLS"));
        let arbitrary="model_provider='openai'\n[model_providers.openai]\nname='GLM'\nbase_url='https://www.colabapi.com/v1'\nwire_api='responses'\nrequires_openai_auth=false\nenv_key='OPENAI_API_KEY'\n";
        assert!(Config::parse(&format!("{raw}{arbitrary}"))
            .unwrap()
            .review(OCTOBRE_ID, "/no-auth-read".into(), None, None)
            .unwrap_err()
            .to_string()
            .contains("forbids provider"));
    }
}
