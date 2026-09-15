//! Typed selected-profile projection. Not a general TOML importer.
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
    ApiKey,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_parent_file_id: Option<[u8; 16]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_api_file_id: Option<[u8; 16]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    pub version: Version,
    pub route: Route,
    pub auth: AuthCustody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<ApiConnection>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_login_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reasoning_summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_supports_reasoning_summaries: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_verbosity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mode_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_model: Option<String>,
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
#[serde(rename_all = "kebab-case")]
pub enum ResumeCwd {
    Session,
    Current,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tui {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_cwd: Option<ResumeCwd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line_use_colors: Option<bool>,
    /// Native owns presentation settings. Keep their values nested under tui;
    /// they do not select providers, credentials, tools or permissions.
    #[serde(default, flatten)]
    pub presentation: BTreeMap<String, serde_json::Value>,
    /// Native persisted tooltip counters, not model availability authority.
    /// Absent/empty preserves the original reviewed serialization exactly.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_availability_nux: BTreeMap<String, u32>,
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
    pub windows: Option<BTreeMap<String, serde_json::Value>>,
    pub forced_login_method: Option<String>,
    pub model_context_window: Option<i64>,
    pub model_reasoning_summary: Option<String>,
    pub model_supports_reasoning_summaries: Option<bool>,
    pub model_verbosity: Option<String>,
    pub plan_mode_reasoning_effort: Option<String>,
    pub review_model: Option<String>,
    pub cutex_provider_mode: String,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub model_reasoning_effort: Option<String>,
    #[serde(default = "file_storage")]
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
    pub model_providers: BTreeMap<String, ApiProvider>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, LegacyJob>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiProvider {
    pub name: String,
    pub base_url: String,
    #[serde(default = "responses_protocol")]
    pub wire_api: String,
    pub requires_openai_auth: bool,
    pub env_key: String,
}
fn file_storage() -> String { "file".into() }
fn responses_protocol() -> String { "responses".into() }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConnection {
    pub provider_id: String,
    pub provider: ApiProvider,
}
impl ApiConnection {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(!self.provider_id.is_empty() && !self.provider_id.chars().any(char::is_control), "invalid provider id");
        let url = url::Url::parse(&self.provider.base_url)?;
        ensure!(matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            && url.username().is_empty() && url.password().is_none(), "invalid API base URL");
        ensure!(self.provider.wire_api == "responses", "this runtime requires the Responses API protocol");
        ensure!(!self.provider.requires_openai_auth && self.provider.env_key == "OPENAI_API_KEY",
            "API-key profile must use its selected API credential file");
        Ok(())
    }
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
        ensure!(uuid::Uuid::parse_str(id).is_ok(), "invalid profile ID");
        let auth_value: serde_json::Value = serde_json::from_slice(&bounded_asset(&auth_path)?)
            .map_err(|_| anyhow::anyhow!("invalid selected credential file"))?;
        let api_key = auth_value.get("OPENAI_API_KEY").and_then(serde_json::Value::as_str)
            .is_some_and(|key| !key.is_empty());
        let route = if api_key { Route::ApiKey } else { Route::ChatgptFile };
        let api = if api_key {
            let provider_id = self.model_provider.clone().unwrap_or_else(|| "openai".into());
            let provider = match self.model_providers.get(&provider_id) {
                Some(provider) => provider.clone(),
                None if provider_id == "openai" => ApiProvider {
                    name: "OpenAI".into(), base_url: "https://api.openai.com/v1".into(),
                    wire_api: "responses".into(), requires_openai_auth: false, env_key: "OPENAI_API_KEY".into(),
                },
                None => anyhow::bail!("selected API provider definition missing"),
            };
            let connection = ApiConnection { provider_id, provider };
            connection.validate()?;
            Some(connection)
        } else {
            ensure!(self.model_provider.as_deref().is_none_or(|v| v == "openai")
                && self.model_providers.is_empty(),
                "subscription credentials require the native OpenAI subscription route");
            None
        };
        let model = selected_model
            .cloned()
            .or(self.model)
            .context("effective selected model missing")?;
        let effort = selected_effort.cloned().or(self.model_reasoning_effort);
        let catalog = self
            .model_catalog_json
            .map(|path| -> anyhow::Result<VerifiedFile> {
                let path = if path.is_absolute() { path } else {
                    auth_path.parent().context("profile directory missing")?.join(path)
                }.canonicalize()?;
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
            windows: self.windows,
            forced_login_method: self.forced_login_method,
            model_context_window: self.model_context_window,
            model_reasoning_summary: self.model_reasoning_summary,
            model_supports_reasoning_summaries: self.model_supports_reasoning_summaries,
            model_verbosity: self.model_verbosity,
            plan_mode_reasoning_effort: self.plan_mode_reasoning_effort,
            review_model: self.review_model,

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
                api,
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
                .is_none_or(|v| matches!(v, "user" | "auto_review" | "guardian_subagent")),
            "unsupported approval reviewer"
        );
        ensure!(
            self.service_tier.as_deref().is_none_or(|v| matches!(v, "default" | "fast" | "priority" | "flex")),
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
                    "invalid skill declaration path"
                );
                // Preserve enabled and disabled intent. Native resolves skill
                // contents; this projection does not create or execute them.
            }
        }
        ensure!(
            self.plugins
                .keys()
                .all(|k| !k.trim().is_empty() && !k.chars().any(char::is_control)),
            "invalid plugin declaration key"
        );
        if let Some(tui) = &self.tui {
            ensure!(
                tui.model_availability_nux.len() <= 64
                    && tui.model_availability_nux.keys().all(|model| {
                        !model.is_empty()
                            && model.len() <= 128
                            && model.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')
                            })
                    }),
                "unsupported model tooltip counter keys"
            );
            ensure!(
                tui.status_line
                    .iter()
                    .flatten()
                    .all(|s| !s.starts_with("custom:")
                        || matches!(s.as_str(), "custom:profile" | "custom:bon-voyage" | "custom:notification")),
                "unsupported selected status item"
            );
        }
        Ok(())
    }
}

pub(super) fn validate_model_identifier(model: &str) -> anyhow::Result<()> {
    ensure!(
        !model.trim().is_empty() && !model.chars().any(char::is_control),
        "model identifier must be nonempty and contain no control characters"
    );
    Ok(())
}

pub(super) fn supported_effort(effort: &str) -> bool {
    matches!(
        effort,
        "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra" | "persistent"
    )
}

pub fn validate_model(
    route: &Route,
    model: &str,
    effort: Option<&str>,
    catalog: Option<&VerifiedFile>,
) -> anyhow::Result<()> {
    validate_model_identifier(model)?;
    match route {
        Route::ChatgptFile => {
            ensure!(catalog.is_none(), "ChatGPT custom catalog not supported");
            ensure!(
                effort.is_none_or(supported_effort),
                "unsupported reasoning effort"
            );
        }
        Route::GlmApiKey | Route::ApiKey => {
            let Some(catalog) = catalog else {
                ensure!(*route == Route::ApiKey, "legacy GLM catalog required");
                ensure!(effort.is_none_or(supported_effort), "unsupported reasoning effort");
                return Ok(());
            };
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
                        .is_some_and(|a| (*route == Route::ApiKey && (effort.is_none()
                            || (a.is_empty() && effort.is_none_or(supported_effort)))) || a
                            .iter()
                            .any(|v| v["effort"].as_str() == effort || (effort.is_none() && *route == Route::ApiKey))),
                "catalog model/effort missing or ambiguous"
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
        Route::GlmApiKey | Route::ApiKey => {
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
        windows_parent_file_id: None,
        windows_api_file_id: None,
        account,
        api_file,
    })
}
#[cfg(not(any(target_os = "linux", windows)))]
pub fn review_auth(_: &Path, _: &Route) -> anyhow::Result<AuthCustody> {
    anyhow::bail!("selected auth requires Linux")
}

#[cfg(windows)]
#[path = "selected_profile_windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::review_auth;
#[cfg(windows)]
use windows::read_api_key;

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
    pub fn provider_id(&self) -> &str {
        self.api.as_ref().map(|api| api.provider_id.as_str())
            .unwrap_or(if self.route == Route::GlmApiKey { "GLM" } else { "openai" })
    }
    /// The same nonsecret argv projection is used by owner launch and the
    /// independent process test. Remote attach deliberately omits auth-file.
    pub fn native_args(&self, owner: bool) -> anyhow::Result<Vec<String>> {
        self.validate_for_launch(owner)?;
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
        if let Some(api) = &self.api {
            api.validate()?;
            option("model_providers", serde_json::to_value(BTreeMap::from([
                (&api.provider_id, &api.provider)
            ]))?)?;
        }
        let settings = serde_json::to_value(&self.settings)?;
        for (key, value) in settings.as_object().context("typed settings missing")? {
            // The existing remote owner also owns its approval reviewer.
            if !owner && key == "approvals_reviewer" {
                continue;
            }
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
    /// Maintenance moves the native user skill root but not the original
    /// profile. Preserve exact enabled/disabled intent at the copied location.
    /// No path is discovered from names or rendered text.
    pub fn migrated_native_args(
        &self,
        owner: bool,
        source_home: &Path,
        destination: &Path,
    ) -> anyhow::Result<Vec<String>> {
        let mut args = self.native_args(owner)?;
        if let Some(skills) = &self.settings.skills {
            let mut mapped = skills.clone();
            for skill in &mut mapped.config {
                if let Ok(relative) = skill.path.strip_prefix(source_home.join("skills")) {
                    skill.path = destination.join("skills").join(relative);
                }
            }
            let value = toml::Value::try_from(&mapped)?;
            args.extend(["-c".into(), format!("skills={value}")]);
        }
        Ok(args)
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_for_launch(true)
    }
    fn validate_for_launch(&self, owner: bool) -> anyhow::Result<()> {
        self.settings.validate()?;
        ensure!(self.api.is_some() == (self.route == Route::ApiKey), "API connection and credential route mismatch");
        if let Some(api) = &self.api { api.validate()?; }
        let wants_status = self.settings.tui.as_ref().is_some_and(|t| {
            t.status_line
                .iter()
                .flatten()
                .any(|s| super::selected_status::is_static_id(s))
        });
        ensure!(
            wants_status == self.status.is_some(),
            "selected status projection missing or inappropriate"
        );
        if let Some(status) = &self.status {
            if owner { status.validate()?; } else { status.validate_frozen()?; }
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
        #[cfg(any(target_os = "linux", windows))]
        if matches!(self.route, Route::GlmApiKey | Route::ApiKey) {
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
    fn native_windows_options_are_accepted_and_preserved() {
        let config = Config::parse("cutex_provider_mode='selected_profile_v2'\n[windows]\nsandbox='unelevated'\n").unwrap();
        let settings = Settings { windows: config.windows, ..Default::default() };
        settings.validate().unwrap();
        assert_eq!(serde_json::to_value(&settings).unwrap()["windows"], serde_json::json!({"sandbox":"unelevated"}));
    }

    #[test]
    fn native_presentation_is_preserved_without_inventing_status_defaults() {
        let original = serde_json::json!({
            "animations": false, "show_tooltips": true, "theme": "night-owl",
            "terminal_title": ["activity", "project"],
            "keymap": {"global": {"copy": "ctrl+c"}}, "resume_cwd": "session",
        });
        let tui: Tui = serde_json::from_value(original.clone()).unwrap();
        assert!(tui.status_line.is_none());
        assert!(tui.status_line_use_colors.is_none());
        assert_eq!(serde_json::to_value(&tui).unwrap(), original);
        let settings = Settings {
            tui: Some(tui),
            ..Default::default()
        };
        settings.validate().unwrap();
        let minimal: Tui = toml::from_str("animations=false").unwrap();
        assert_eq!(toml::to_string(&minimal).unwrap(), "animations = false\n");
        let invalid: Result<Tui, _> = toml::from_str("animations=false\nresume_cwd='other'");
        assert!(invalid.is_err());
        let mut native = settings.clone();
        native.tui.as_mut().unwrap().status_line = Some(vec!["git-branch".into()]);
        native.validate().unwrap();
        native.tui.as_mut().unwrap().status_line = Some(vec!["custom:unknown".into()]);
        assert!(native.validate().is_err());
    }

    #[test]
    fn native_resume_cwd_preference_is_accepted_and_preserved() {
        for value in ["session", "current"] {
            let config = format!(
                "status_line = []\nstatus_line_use_colors = true\nresume_cwd = \"{value}\"\n"
            );
            let tui: Tui = toml::from_str(&config).unwrap();
            let encoded = toml::to_string(&tui).unwrap();
            assert!(encoded.contains(&format!("resume_cwd = \"{value}\"")));
        }
        assert!(toml::from_str::<Tui>(
            "status_line=[]\nstatus_line_use_colors=true\nresume_cwd='other'"
        )
        .is_err());
    }

    #[test]
    fn selected_models_allow_new_names_and_native_efforts_without_fallback() {
        for model in ["gpt-6-astra", "gpt-5.6-luna", "future-model/revision-2"] {
            assert!(validate_model(&Route::ChatgptFile, model, None, None).is_ok());
            for effort in [
                "none",
                "minimal",
                "low",
                "medium",
                "high",
                "xhigh",
                "max",
                "ultra",
                "persistent",
            ] {
                assert!(validate_model(&Route::ChatgptFile, model, Some(effort), None).is_ok());
            }
        }
        for (model, effort) in [
            ("", Some("low")),
            ("bad\nmodel", Some("low")),
            ("gpt-5.6-sol", Some("guessed")),
        ] {
            assert!(validate_model(&Route::ChatgptFile, model, effort, None).is_err());
        }
        assert!(validate_model(&Route::GlmApiKey, "glm-5.3", Some("max"), None).is_err());
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn new_account_identity_uses_configured_route_and_forwards_enabled_settings() {
        use base64::Engine;
        use std::os::unix::fs::PermissionsExt;
        // Auth validation intentionally rejects /tmp and group-writable source
        // ancestors. Keep this disposable fixture in the actual user's home,
        // independently of the test harness's redirected HOME.
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0u8; 16384];
        assert_eq!(
            unsafe {
                libc::getpwuid_r(
                    libc::geteuid(),
                    &mut passwd,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            },
            0
        );
        assert!(!result.is_null());
        let home = unsafe { std::ffi::CStr::from_ptr(passwd.pw_dir) }
            .to_str()
            .unwrap();
        let root = Path::new(home).join(format!(".cutex-profile-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let auth = root.join("auth.json");
        let id_token = format!(
            "header.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(br#"{"https://api.openai.com/auth":{"chatgpt_user_id":"fixture-user"}}"#)
        );
        std::fs::write(&auth, serde_json::to_vec(&serde_json::json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,
            "tokens":{"id_token":id_token,"access_token":"fixture-access","refresh_token":"fixture-refresh","account_id":"fixture-account"},"last_refresh":null})).unwrap()).unwrap();
        std::fs::set_permissions(&auth, std::fs::Permissions::from_mode(0o600)).unwrap();
        let config = "cutex_provider_mode='selected_profile_v2'\ncli_auth_credentials_store='file'\nmodel='future-model/revision-2'\n[[skills.config]]\npath='/private/my-skill/SKILL.md'\nenabled=true\n[plugins.'custom@tools']\nenabled=true\n";
        let (projection, model, effort) = Config::parse(config)
            .unwrap()
            .review(&uuid::Uuid::new_v4().to_string(), auth, None, None)
            .unwrap();
        assert_eq!(projection.route, Route::ChatgptFile);
        assert_eq!(model, "future-model/revision-2");
        assert_eq!(effort, None);
        let args = projection.native_args(true).unwrap().join(" ");
        assert!(args.contains("custom@tools"));
        assert!(args.contains("/private/my-skill/SKILL.md"));
        assert!(args.contains("enabled = true"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn glm_model_selection_uses_supplied_catalog_instead_of_hardcoded_slug() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("selected-glm-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("catalog.json");
        std::fs::write(
            &path,
            br#"{"models":[{"slug":"glm-next","supported_reasoning_levels":[{"effort":"max"}]}]}"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = VerifiedFile {
            sha256: crate::agent_management::file_sha256(&path).unwrap(),
            path,
        };
        assert!(validate_model(&Route::GlmApiKey, "glm-next", Some("max"), Some(&file)).is_ok());
        assert!(
            validate_model(&Route::GlmApiKey, "missing-model", Some("max"), Some(&file)).is_err()
        );
        assert!(validate_model(&Route::GlmApiKey, "glm-next", Some("high"), Some(&file)).is_err());
        std::fs::remove_dir_all(root).unwrap();
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
    fn settings_preserve_enabled_and_disabled_skills_and_arbitrary_plugins() {
        let mut s = Settings::default();
        s.skills = Some(Skills {
            config: vec![Skill {
                enabled: false,
                path: std::env::temp_dir().join("missing/SKILL.md"),
            }],
        });
        s.plugins
            .insert("sample@debug".into(), Plugin { enabled: true });
        assert!(s.validate().is_ok());
        s.skills.as_mut().unwrap().config[0].enabled = true;
        s.plugins
            .insert("custom@tools".into(), Plugin { enabled: false });
        assert!(s.validate().is_ok());
        s.plugins.insert("".into(), Plugin { enabled: true });
        assert!(s.validate().is_err());
        s.plugins.remove("");
        s.skills = None;
        s.tui = Some(Tui {
            resume_cwd: Some(ResumeCwd::Session),
            status_line: Some(vec!["custom:profile".into()]),
            status_line_use_colors: Some(true),
            presentation: BTreeMap::new(),
            model_availability_nux: BTreeMap::new(),
        });
        assert!(s.validate().is_ok());
        s.tui
            .as_mut()
            .unwrap()
            .status_line
            .as_mut()
            .unwrap()
            .push("custom:unknown".into());
        assert!(s.validate().is_err());
    }
    #[test]
    fn tooltip_counters_preserve_native_values_without_changing_legacy_bytes() {
        let old =
            serde_json::json!({"status_line":["custom:profile"],"status_line_use_colors":true});
        let mut tui: Tui = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(&tui).unwrap(), old);
        tui.model_availability_nux.insert("gpt-5.5".into(), 2);
        tui.model_availability_nux.insert("gpt-5.6-sol".into(), 1);
        let mut settings = Settings {
            tui: Some(tui),
            ..Default::default()
        };
        assert!(settings.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["tui"]["model_availability_nux"]["gpt-5.5"],
            2
        );
        settings
            .tui
            .as_mut()
            .unwrap()
            .model_availability_nux
            .insert("bad/key".into(), 1);
        assert!(settings.validate().is_err());
        let mut malformed = old;
        malformed["model_availability_nux"] = serde_json::json!({"gpt-5.5":-1});
        assert!(serde_json::from_value::<Tui>(malformed).is_err());
    }
    #[test]
    fn native_service_tiers_and_reviewers_are_supported() {
        for tier in ["default", "fast", "priority", "flex"] {
            Settings { service_tier: Some(tier.into()), ..Default::default() }.validate().unwrap();
        }
        for reviewer in ["user", "auto_review", "guardian_subagent"] {
            Settings { approvals_reviewer: Some(reviewer.into()), ..Default::default() }.validate().unwrap();
        }
        assert!(Settings { service_tier: Some("invalid".into()), ..Default::default() }.validate().is_err());
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
    fn api_connections_accept_provider_names_and_endpoints_without_brand_fences() {
        for (id, endpoint) in [("GLM", "https://api.z.ai/api/coding/paas/v4"),
            ("colab", "https://www.colabapi.com/v1"), ("deepseek", "https://api.deepseek.com/v1"),
            ("local", "http://127.0.0.1:8080/v1"), ("openai", "https://api.openai.com/v1")] {
            let connection = ApiConnection { provider_id: id.into(), provider: ApiProvider {
                name:id.into(), base_url:endpoint.into(), wire_api:"responses".into(),
                requires_openai_auth:false, env_key:"OPENAI_API_KEY".into() } };
            connection.validate().unwrap();
        }
        assert!(validate_model(&Route::ApiKey, "manual-model", None, None).is_ok());
        assert!(validate_model(&Route::ApiKey, "manual-model", Some("high"), None).is_ok());
    }
}
