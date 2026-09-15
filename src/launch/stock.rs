//! Manifest-selected Linux runtime artifacts with checked file integrity and protocol compatibility.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};

use crate::agent_management::{file_sha256, ExplicitLaunchContract};
use crate::role_revision::Sha256;
use crate::session::model::{CutexSessionRecord, CutexSessionRuntimeBackend, CutexSessionStore};

pub const STOCK_COMMIT: &str = "3d2ee51ca2d5db578f328aa75e20aa22c0197c9a";
pub const STOCK_EXECUTABLE_SHA256: &str =
    "56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da";
pub const STOCK_HOST_SHA256: &str =
    "3e85d67471825f73d02ff5f7e047ca1f6ca8caa3f59e4c6e8d9ca6ca7302cb45";
pub const STOCK_SCHEMA_SHA256: &str =
    "b06f77062369d481a59cc70720c12b89cb9dd49c385863923262102d3ad6c978";
pub const S6_COMMIT: &str = "c2aaceb411b7851806c62435b97895a63a7d34cd";
pub const S6_EXECUTABLE_SHA256: &str =
    "b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60";
pub const S6_SCHEMA_SHA256: &str =
    "00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d";
// Exact accepted durable-presentation CLI/server family. Older
// bundle receipts remain historical facts, not permission to launch old bytes.
pub const S6E_COMMIT: &str = "8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6";
// Coherent accepted auth/legacy-reader family, not a descendant allowlist.
pub const S6E_SERVER_COMMIT: &str = "2eab060b191a0fe59e22b28785d02d76eafb7fc4";
pub const S6E_EXECUTABLE_SHA256: &str =
    "15c72a6bd476a60af9ba99cfbc0672a8376e8bb447cda2f1745e72aef2070c49";
pub const S6E_CLI_SHA256: &str = "f66dd9126fb73b8f6bc04cd275f619630c96c4255e6281dcbeb1fe41f8b691cf";
pub const S6E_SCHEMA_SHA256: &str =
    "3fc006075408b0a001f9cb3f706625c0161e083ab0691d6be6116622b434f1d3";

pub fn is_private_native_schema(hash: &str) -> bool {
    matches!(
        hash,
        STOCK_SCHEMA_SHA256 | S6_SCHEMA_SHA256 | S6E_SCHEMA_SHA256
    )
}

pub fn validate_ingress_capability(schema: &str, init: &serde_json::Value) -> anyhow::Result<()> {
    if matches!(schema, S6_SCHEMA_SHA256 | S6E_SCHEMA_SHA256) {
        ensure!(
            init.get("externalInputVersion")
                .and_then(serde_json::Value::as_u64)
                == Some(1),
            "reviewed native owner omitted ExternalInput v1 capability"
        );
        if schema == S6E_SCHEMA_SHA256 {
            ensure!(
                init.get("externalInputVersions")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|versions| [1, 2]
                        .iter()
                        .all(|v| versions.iter().any(|item| item.as_u64() == Some(*v)))),
                "reviewed structured-view bundle omitted v1/v2 submit capability"
            );
            ensure!(
                init.get("presentationVersion")
                    .and_then(serde_json::Value::as_u64)
                    == Some(1),
                "reviewed presentation bundle omitted presentation v1 capability"
            );
            ensure!(
                init.get("externalInputDeliveries")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|modes| modes.iter().any(|m| m.as_str() == Some("soon"))),
                "reviewed Soon bundle omitted supported-delivery capability; no downgrade"
            );
        }
    }
    Ok(())
}

/// Human receiver policy, never an ingress message/tool field. Null is invalid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CanonicalBytePolicy {
    Limit(std::num::NonZeroU32),
    Off(OffPolicy),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OffPolicy {
    #[serde(rename = "off")]
    Off,
}
impl Default for CanonicalBytePolicy {
    fn default() -> Self {
        Self::Limit(std::num::NonZeroU32::new(10000).unwrap())
    }
}
impl CanonicalBytePolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExternalInputBinding {
    pub version: u32,
    pub owner_id: String,
    pub thread_id: String,
    pub runtime_generation: u64,
    #[serde(default, skip_serializing_if = "CanonicalBytePolicy::is_default")]
    pub canonical_byte_limit: CanonicalBytePolicy,
}
impl ExternalInputBinding {
    #[cfg(not(any(unix, windows)))]
    pub fn stage(&self, _directory: &Path) -> anyhow::Result<PathBuf> {
        anyhow::bail!("ExternalInput requires private Unix transport")
    }
    #[cfg(not(any(unix, windows)))]
    pub fn verify(&self, _directory: &Path) -> anyhow::Result<()> {
        anyhow::bail!("ExternalInput requires private Unix transport")
    }
    pub fn from_review(receipt: &crate::agent_management::StockRuntimeReceipt) -> Self {
        Self {
            version: 1,
            owner_id: receipt.review.subject.cutex_session_id.as_str().to_string(),
            thread_id: receipt.review.contract.native_id.clone(),
            runtime_generation: receipt.expected_generation,
            canonical_byte_limit: receipt.review.receiver_canonical_byte_limit.clone(),
        }
    }
    #[cfg(unix)]
    fn private_directory(path: &Path) -> anyhow::Result<()> {
        use std::os::unix::fs::MetadataExt;
        canonical(path)?;
        let m = std::fs::symlink_metadata(path)?;
        ensure!(
            m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "ingress binding requires owned private runtime directory"
        );
        Ok(())
    }
    #[cfg(unix)]
    pub fn stage(&self, directory: &Path) -> anyhow::Result<PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        Self::private_directory(directory)?;
        let path = directory.join("external-input.json");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.flush()?;
        Ok(path)
    }
    #[cfg(unix)]
    pub fn verify(&self, directory: &Path) -> anyhow::Result<()> {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        Self::private_directory(directory)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(directory.join("external-input.json"))?;
        let m = file.metadata()?;
        ensure!(
            m.is_file()
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o7777 == 0o600
                && m.len() <= 4096,
            "invalid ingress binding file"
        );
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        ensure!(
            serde_json::from_slice::<Self>(&bytes)? == *self,
            "ingress binding changed"
        );
        Ok(())
    }
    #[cfg(windows)]
    pub fn stage(&self, directory: &Path) -> anyhow::Result<PathBuf> {
        use std::io::Write;
        use crate::platform::private_fs;
        let (_guard, identity) = private_fs::open_validated_directory(directory)?;
        let mut file = private_fs::open_child(directory, identity, "external-input.json",
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, false)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.flush()?;
        Ok(directory.join("external-input.json"))
    }
    #[cfg(windows)]
    pub fn verify(&self, directory: &Path) -> anyhow::Result<()> {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        use crate::platform::private_fs;
        let (_guard, identity) = private_fs::open_validated_directory(directory)?;
        let file = std::fs::OpenOptions::new().read(true).share_mode(1)
            .custom_flags(0x00200000).open(directory.join("external-input.json"))?;
        private_fs::validate_private_file(&file)?;
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 4096 && serde_json::from_slice::<Self>(&bytes)? == *self,
            "ingress binding changed");
        private_fs::validate_binding(directory, identity)?;
        Ok(())
    }

}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedFile {
    pub path: PathBuf,
    pub sha256: Sha256,
}
impl VerifiedFile {
    pub fn validate(&self) -> anyhow::Result<()> {
        canonical(&self.path)?;
        ensure!(self.path.is_file(), "stock reference is not a file");
        ensure!(
            file_sha256(&self.path)? == self.sha256,
            "stock reference changed"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockBundle {
    pub version: u32,
    pub upstream_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_patch_commit: Option<String>,
    pub executable: VerifiedFile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<VerifiedFile>,
    pub code_mode_host: VerifiedFile,
    pub facade: VerifiedFile,
    /// Pinned stock experimental schema, or accepted S6 generated schema.
    pub schema: VerifiedFile,
    /// Mutable shared config; its current content is validated on each load.
    pub shared_config: VerifiedFile,
    /// Historical migration digest, retained for receipts, not a launch restriction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_config_sha256: Option<Sha256>,
}

impl StockBundle {
    pub fn common_ingress(&self) -> bool {
        (self.version == 2
            && self.native_patch_commit.as_deref() == Some(S6_COMMIT)
            && self.executable.sha256.as_str() == S6_EXECUTABLE_SHA256
            && self.schema.sha256.as_str() == S6_SCHEMA_SHA256
            && self.cli.is_none())
            || self.soon_ingress()
    }
    pub fn soon_ingress(&self) -> bool {
        self.local_deployment()
            || (self.version == 3
                && self.native_patch_commit.as_deref() == Some(S6E_COMMIT)
                && self.executable.sha256.as_str() == S6E_EXECUTABLE_SHA256
                && self.schema.sha256.as_str() == S6E_SCHEMA_SHA256
                && self
                    .cli
                    .as_ref()
                    .is_some_and(|c| c.sha256.as_str() == S6E_CLI_SHA256))
    }

    /// Version 4 selects locally installed build bytes, rather than a compiled
    /// allowlist of source commits. The protocol remains explicitly supported;
    /// every selected file is still hashed by validate_components.
    fn local_deployment(&self) -> bool {
        self.version == 4 && self.cli.is_some() && self.schema.sha256.as_str() == S6E_SCHEMA_SHA256
    }
    fn validate_identity(&self) -> anyhow::Result<()> {
        if self.version == 4 {
            ensure!(
                self.local_deployment(),
                "local runtime deployment requires CLI and supported protocol schema"
            );
            return Ok(());
        }
        ensure!(
            self.upstream_commit == STOCK_COMMIT,
            "unsupported native upstream source"
        );
        let stock = self.version == 1
            && self.cli.is_none()
            && self.native_patch_commit.is_none()
            && self.executable.sha256.as_str() == STOCK_EXECUTABLE_SHA256
            && self.schema.sha256.as_str() == STOCK_SCHEMA_SHA256;
        ensure!(
            stock || self.common_ingress(),
            "unpinned native bundle/source/executable/schema"
        );
        ensure!(
            self.code_mode_host.sha256.as_str() == STOCK_HOST_SHA256,
            "unpinned native companion"
        );
        Ok(())
    }
    pub fn load(contract: &ExplicitLaunchContract) -> anyhow::Result<Self> {
        Self::load_contract(contract)
    }

    /// An existing process keeps its startup configuration. Reading messages,
    /// history, or stopping it must not depend on later edits to config.toml.
    pub fn load_running(contract: &ExplicitLaunchContract) -> anyhow::Result<Self> {
        contract.validate()?;
        canonical(&contract.native_home)?;
        canonical(&contract.bundle_manifest)?;
        ensure!(contract.native_home.is_dir(), "native home missing");
        ensure!(file_sha256(&contract.bundle_manifest)? == contract.bundle_sha256,
            "bundle evidence missing or changed");
        let bundle: Self = serde_json::from_slice(&std::fs::read(&contract.bundle_manifest)?)
            .context("invalid stock bundle manifest")?;
        bundle.validate_components_with_config(false)?;
        ensure!(contract.version == if contract.version == 4 && bundle.version == 4 {4} else if bundle.soon_ingress() { if contract.migration_action_id.is_some() {3} else {2} } else { 1 },
            "runtime contract version does not match its bundle");
        Ok(bundle)
    }

    fn load_contract(contract: &ExplicitLaunchContract) -> anyhow::Result<Self> {
        contract.validate()?;
        let bundle = Self::load_verified_references(
            &contract.native_home,
            &contract.bundle_manifest,
            &contract.bundle_sha256,
        )?;
        ensure!(contract.version == if contract.version == 4 && bundle.version == 4 {4} else if bundle.soon_ingress() { if contract.migration_action_id.is_some() {3} else {2} } else { 1 },
            "new coherent Soon bundle requires explicit version-2 activation; old markers cannot opt in");
        Ok(bundle)
    }

    /// Validate execution evidence before a native identity exists. This does
    /// not authorize launch or manufacture an ExplicitLaunchContract.
    pub fn load_references(
        native_home: &Path,
        manifest: &Path,
        digest: &Sha256,
    ) -> anyhow::Result<Self> {
        Self::load_verified_references(native_home, manifest, digest)
    }

    fn load_verified_references(
        native_home: &Path, manifest: &Path, digest: &Sha256,
    ) -> anyhow::Result<Self> {
        canonical(native_home)?;
        canonical(manifest)?;
        ensure!(native_home.is_dir(), "native home missing");
        ensure!(
            &file_sha256(manifest)? == digest,
            "bundle evidence missing or changed"
        );
        let mut bundle: Self = serde_json::from_slice(&std::fs::read(manifest)?)
            .context("invalid stock bundle manifest")?;
        let raw = std::fs::read_to_string(&bundle.shared_config.path)?;
        bundle.validate_shared_configuration(&raw)?;
        // Config is user-editable state, not an immutable build artifact.
        // Old manifest hashes remain historical evidence, never launch gates.
        bundle.shared_config.sha256 = file_sha256(&bundle.shared_config.path)?;
        bundle.validate_components()?;
        ensure!(
            bundle.shared_config.path.canonicalize()? == native_home.join("config.toml").canonicalize()?,
            "wrong shared config/home"
        );
        bundle.validate_shared_configuration(&std::fs::read_to_string(&bundle.shared_config.path)?)?;
        Ok(bundle)
    }

    fn validate_shared_configuration(&self, raw: &str) -> anyhow::Result<()> {
        validate_shared_config(raw)
    }

    /// Identical artifact fences, before a migration's shared config exists.
    pub(crate) fn validate_components(&self) -> anyhow::Result<()> {
        self.validate_components_with_config(true)
    }

    fn validate_components_with_config(&self, validate_config: bool) -> anyhow::Result<()> {
        let bundle = self;
        ensure!(cfg!(any(target_os = "linux", windows)), "stock runtime unsupported on this platform");
        #[cfg(windows)]
        ensure!(bundle.local_deployment(), "Windows requires a local v4 runtime deployment");
        bundle.validate_identity()?;
        if let Some(cli) = &bundle.cli {
            cli.validate()?;
            ensure!(
                cli.path.parent() == bundle.executable.path.parent(),
                "coherent CLI must be beside app-server and host"
            );
        }
        for file in [
            &bundle.executable,
            &bundle.code_mode_host,
            &bundle.facade,
            &bundle.schema,
        ] {
            file.validate()?;
        }
        if validate_config { bundle.shared_config.validate()?; }
        ensure!(
            bundle.code_mode_host.path
                == bundle
                    .executable
                    .path
                    .parent()
                    .context("stock binary parent missing")?
                    .join(if cfg!(windows) { "codex-code-mode-host.exe" } else { "codex-code-mode-host" }),
            "stock companion must be beside executable"
        );
        let schema: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&bundle.schema.path)?)?;
        ensure!(schema.is_object(), "invalid stock protocol schema");
        Ok(())
    }
}

/// Hash launch-affecting values, preserving all non-presentation configuration.
pub fn launch_config_digest(raw: &str) -> anyhow::Result<Sha256> {
    use sha2::Digest;
    validate_shared_config(raw)?;
    let mut value: toml::Value = toml::from_str(raw)?;
    value.as_table_mut().context("config must be a table")?.remove("tui");
    Sha256::new(format!("{:x}", sha2::Sha256::digest(serde_json::to_vec(&value)?)))
        .map_err(anyhow::Error::msg)
}

/// Locally installed native runtimes own their config schema. Do not keep a
/// second, progressively stale allowlist that rejects ordinary native settings.
pub fn validate_shared_config(raw: &str) -> anyhow::Result<()> {
    let value: toml::Value = toml::from_str(raw)
        .map_err(|error: toml::de::Error| anyhow::anyhow!("invalid native configuration: {}", error.message()))?;
    ensure!(value.is_table(), "native config must be a table");
    Ok(())
}


pub fn canonical(path: &Path) -> anyhow::Result<()> {
    ensure!(path.is_absolute(), "absolute stock path required");
    let resolved = path.canonicalize()?;
    #[cfg(not(windows))]
    ensure!(resolved == path, "canonical existing stock path required");
    #[cfg(windows)]
    {
        fn spelling(path: &Path) -> String {
            let text = path.to_string_lossy().replace('/', "\\");
            if let Some(unc) = text.strip_prefix(r"\\?\UNC\") { format!(r"\\{unc}") }
            else { text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned() }
        }
        ensure!(spelling(&resolved).eq_ignore_ascii_case(&spelling(path)),
            "canonical existing stock path required");
    }
    Ok(())
}

/// Current nonsecret configuration, bound separately from durable identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockConfiguration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_projection: Option<super::selected_profile::Projection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aemeath_auth: Option<super::aemeath_auth::ReviewedAemeathAuth>,
    pub profile_name: String,
    pub profile_id: String,
    pub inherited: bool,
    pub profile_sha256: Sha256,
    pub account_sha256: Sha256,
    pub model: String,
    pub reasoning: Option<String>,
    pub model_provider: String,
    pub provider: DummyProvider,
    pub sandbox: String,
    pub approval: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DummyProvider {
    pub name: String,
    pub base_url: String,
    pub wire_api: String,
    pub requires_openai_auth: bool,
    pub supports_websockets: bool,
}
impl StockConfiguration {
    pub fn validate_job_requirement(&self, present: bool) -> anyhow::Result<()> {
        ensure!(
            self.selected_projection
                .as_ref()
                .is_none_or(|p| !p.requires_job || present),
            "selected profile requires an explicit reviewed coherent Job descriptor"
        );
        Ok(())
    }
    pub fn validate_auth_home(&self, home: &Path) -> anyhow::Result<()> {
        if let Some(projection) = &self.selected_projection {
            ensure!(
                self.aemeath_auth.is_none(),
                "conflicting reviewed auth modes"
            );
            projection.validate()?;
            if let Some(status) = &projection.status {
                status.validate_projection(
                    &projection
                        .settings
                        .tui
                        .as_ref()
                        .context("status selection missing")?
                        .status_line
                        .as_deref()
                        .unwrap_or_default(),
                    &self.profile_name,
                )?;
            }
            return Ok(());
        }
        match &self.aemeath_auth {
            Some(auth) => {
                ensure!(
                    auth.version == 1 && auth.path == home.join("auth.json"),
                    "reviewed native auth home/version mismatch"
                );
                ensure!(
                    super::aemeath_auth::review(auth.path.clone())? == *auth,
                    "reviewed native auth custody/account changed"
                );
            }
            None => ensure!(
                !home.join("auth.json").try_exists()?,
                "stock fake subset does not consume native auth files"
            ),
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConfig {
    #[serde(default)]
    cutex_provider_mode: ProviderMode,
    model: String,
    model_provider: String,
    model_reasoning_effort: Option<String>,
    #[serde(default)]
    model_providers: BTreeMap<String, DummyProvider>,
}

#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ProviderMode {
    #[default]
    Fake,
    AemeathChatgptV1,
}

pub fn current_configuration(record: &CutexSessionRecord) -> anyhow::Result<StockConfiguration> {
    configuration_for_record(
        record,
        record
            .explicit_launch
            .as_ref()
            .is_some_and(|c| c.version == 4 || (c.version == 3 && c.migration_action_id.is_some())),
    )
}

/// Only maintenance review calls this before installing the explicit v3 marker.
/// The original profile and independent auth path remain authoritative.
pub(crate) fn migration_configuration(
    record: &CutexSessionRecord,
) -> anyhow::Result<StockConfiguration> {
    configuration_for_record(record, true)
}

fn configuration_for_record(
    record: &CutexSessionRecord,
    migration: bool,
) -> anyhow::Result<StockConfiguration> {
    ensure!(
        record.default_cli_args.is_empty(),
        "stock does not accept arbitrary durable CLI overrides"
    );
    let (sandbox, approval) = crate::runtime::args::effective_runtime_permission_defaults(record);
    let mut configuration = configuration_for_selection(
        record.profile.as_ref(),
        record.permission_defaults.as_deref(),
        sandbox,
        approval,
        record.model_defaults.as_ref(),
        record.reasoning_defaults.as_ref(),
        migration,
    )?;
    if !record.agent_enabled && record.registration_class == crate::agent_bus::model::AgentRegistrationClass::LocalOnly {
        if let Some(projection)=configuration.selected_projection.as_mut() { projection.requires_job=false; }
    }
    Ok(configuration)
}

/// Configuration is independent of identity. Bootstrap has no durable/native
/// ID yet, so it must not construct a placeholder session to resolve a profile.
pub fn bootstrap_configuration(
    spec: &crate::agent_management::ManagedAgentSpec,
) -> anyhow::Result<StockConfiguration> {
    ensure!(
        spec.runtime_backend == "host",
        "stock bootstrap requires host backend"
    );
    configuration_for_selection(
        spec.profile.as_ref(),
        Some(&spec.permissions),
        Some(spec.sandbox_mode.clone()),
        Some(spec.approval_policy.clone()),
        Some(&spec.model),
        Some(&spec.reasoning),
        false,
    )
}

/// Installed native defaults accept the same selected-profile projection as
/// Human New, while keeping the typed request's explicit model and permissions.
pub fn local_bootstrap_configuration(
    spec: &crate::agent_management::ManagedAgentSpec,
) -> anyhow::Result<StockConfiguration> {
    configuration_for_selection(
        spec.profile.as_ref(),
        Some(&spec.permissions),
        Some(spec.sandbox_mode.clone()),
        Some(spec.approval_policy.clone()),
        Some(&spec.model),
        Some(&spec.reasoning),
        true,
    )
}

/// Human new-agent defaults resolve the installed profile without inventing an identity.
pub fn local_configuration() -> anyhow::Result<StockConfiguration> {
    local_configuration_with_profile(None)
}

pub fn local_configuration_with_profile(profile: Option<&String>) -> anyhow::Result<StockConfiguration> {
    let config = crate::config::store::load_codez_config_checked()?;
    let name = profile.or(config.default_profile.as_ref()).context("Select a default profile in Settings / Defaults")?;
    let inherited = crate::config::new_session::inherited(name)?;
    let defaults = config.new_session_defaults.get(name);
    configuration_for_selection(
        Some(name),
        Some("danger-full-access"),
        Some("danger-full-access".into()),
        Some("never".into()),
        defaults.and_then(|d| d.model.as_ref()).or(inherited.model.as_ref()),
        defaults.and_then(|d| d.reasoning.as_ref()).or(inherited.reasoning.as_ref()),
        true,
    )
}

fn configuration_for_selection(
    selected_profile: Option<&String>,
    permission: Option<&str>,
    sandbox: Option<String>,
    approval: Option<String>,
    selected_model: Option<&String>,
    selected_reasoning: Option<&String>,
    migration: bool,
) -> anyhow::Result<StockConfiguration> {
    use crate::profiles::model::{AccountsStore, CliKind, RuntimeConfig};
    let config = crate::config::store::load_codez_config_checked()?;
    let inherited = selected_profile.is_none();
    let name = selected_profile
        .or(config.default_profile.as_ref())
        .context("stock inherited profile unavailable; explicit current default required")?;
    ensure!(!name.trim().is_empty(), "stock profile is empty");
    // Read current store without canonicalizing, selecting another account or writing it.
    let raw_accounts: serde_json::Value =
        serde_json::from_slice(&std::fs::read(crate::config::paths::accounts_path()?)?)?;
    ensure!(
        raw_accounts.as_object().is_some_and(|o| o
            .keys()
            .all(|k| matches!(k.as_str(), "version" | "accounts" | "active_account_id"))),
        "unsupported stock account store fields"
    );
    let raw_entries = raw_accounts["accounts"]
        .as_array()
        .context("invalid stock account store")?;
    for entry in raw_entries
        .iter()
        .filter(|a| a["name"].as_str() == Some(name))
    {
        ensure!(
            entry.as_object().is_some_and(|o| o.keys().all(|k| matches!(
                k.as_str(),
                "id" | "name"
                    | "email"
                    | "plan_type"
                    | "source"
                    | "runtime"
                    | "proxy"
                    | "session"
                    | "cli_kind"
                    | "default_cli_args"
                    | "agent_name"
                    | "last_used_at"
            ))),
            "unsupported stock account fields"
        );
    }
    let accounts: AccountsStore = serde_json::from_value(raw_accounts)?;
    ensure!(
        accounts.version == crate::profiles::model::STORE_VERSION,
        "unsupported stock account store version"
    );
    let matches: Vec<_> = accounts
        .accounts
        .iter()
        .filter(|a| &a.name == name)
        .collect();
    ensure!(matches.len() == 1, "stock profile missing or ambiguous");
    let account = matches[0];
    ensure!(
        uuid::Uuid::parse_str(&account.id)?.to_string() == account.id,
        "stock profile id must be an exact UUID"
    );
    ensure!(
        matches!(account.runtime, RuntimeConfig::Host)
            && account.cli_kind == CliKind::Codex
            && account.session.is_none()
            && account.default_cli_args.is_empty(),
        "unsupported stock account/runtime/options"
    );
    let files = crate::profiles::materialize::materialized_account_files(account)?;
    canonical(&files.config_path)?;
    let raw = std::fs::read_to_string(&files.config_path)?;
    let mode: toml::Value =
        toml::from_str(&raw).map_err(|_| anyhow::anyhow!("invalid profile configuration"))?;
    if migration
        || mode
            .get("cutex_provider_mode")
            .and_then(toml::Value::as_str)
            == Some("selected_profile_v2")
    {
        let projected_raw = if migration {
            let mut value = mode.clone();
            let table = value.as_table_mut().context("profile table required")?;
            ensure!(
                !table.contains_key("cutex_provider_mode")
                    || table["cutex_provider_mode"].as_str() == Some("selected_profile_v2"),
                "conflicting original provider mode"
            );
            table.insert(
                "cutex_provider_mode".into(),
                toml::Value::String("selected_profile_v2".into()),
            );
            toml::to_string(&value)?
        } else {
            raw.clone()
        };
        let (mut projection, model, reasoning) =
            super::selected_profile::Config::parse(&projected_raw)?.review(
                &account.id,
                files.auth_path.clone(),
                selected_model,
                selected_reasoning,
            )?;
        if let Some(tui) = &projection.settings.tui {
            projection.status = super::selected_status::Status::review(
                &files.custom_status_items_path,
                tui.status_line.as_deref().unwrap_or_default(),
                &account.name,
            )?;
        }
        let sandbox = sandbox.context("explicit selected sandbox required")?;
        let approval = approval.context("explicit selected approval required")?;
        validate_selected_permissions(permission, &sandbox, &approval)?;
        let chatgpt = projection.route == super::selected_profile::Route::ChatgptFile;
        use sha2::Digest;
        let profile_sha256 = file_sha256(&files.config_path)?;
        ensure!(
            format!("{:x}", sha2::Sha256::digest(raw.as_bytes())) == profile_sha256.as_str(),
            "selected profile changed during review"
        );
        let provider_id = projection.provider_id().to_owned();
        let provider = if let Some(api) = &projection.api {
            DummyProvider { name: api.provider.name.clone(), base_url: api.provider.base_url.clone(),
                wire_api: api.provider.wire_api.clone(), requires_openai_auth: false, supports_websockets: false }
        } else {
            DummyProvider { name: if chatgpt { "OpenAI" } else { "GLM" }.into(),
                base_url: if chatgpt { super::aemeath_auth::ENDPOINT } else { super::selected_profile::GLM_ENDPOINT }.into(),
                wire_api: "responses".into(), requires_openai_auth: chatgpt, supports_websockets: chatgpt }
        };
        return Ok(StockConfiguration {
            selected_projection: Some(projection),
            aemeath_auth: None,
            profile_name: name.clone(),
            profile_id: account.id.clone(),
            inherited,
            profile_sha256,
            account_sha256: Sha256::new(format!(
                "{:x}",
                sha2::Sha256::digest(serde_json::to_vec(account)?)
            ))
            .map_err(|_| anyhow::anyhow!("invalid selected account digest"))?,
            model,
            reasoning,
            model_provider: provider_id,
            provider,
            sandbox,
            approval,
        });
    }
    ensure!(
        !files.auth_path.try_exists()?,
        "stock private subset does not consume profile auth files"
    );
    let profile: ProfileConfig = toml::from_str(&raw)
        .map_err(|_| anyhow::anyhow!("unsupported stock profile configuration"))?;
    let (provider, aemeath_auth) = match profile.cutex_provider_mode {
        ProviderMode::Fake => {
            ensure!(
                account.email.is_none() && account.plan_type.is_none(),
                "real account authentication is unsupported by private fake subset"
            );
            ensure!(
                profile.model_providers.len() == 1,
                "exactly one dummy provider required"
            );
            let provider = profile
                .model_providers
                .get(&profile.model_provider)
                .context("configured provider missing")?
                .clone();
            let url = url::Url::parse(&provider.base_url)?;
            ensure!(
                url.scheme() == "http"
                    && matches!(url.host_str(), Some("127.0.0.1") | Some("[::1]"))
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && !provider.requires_openai_auth
                    && !provider.supports_websockets
                    && provider.wire_api == "responses",
                "only private loopback unauthenticated fake Responses provider is supported"
            );
            (provider, None)
        }
        ProviderMode::AemeathChatgptV1 => {
            super::aemeath_auth::validate_selection(
                &account.id,
                name,
                &profile.model_provider,
                profile.model_providers.is_empty(),
            )?;
            let auth = super::aemeath_auth::review(
                crate::config::paths::host_codex_home_dir()?.join("auth.json"),
            )?;
            (
                DummyProvider {
                    name: "OpenAI".into(),
                    base_url: super::aemeath_auth::ENDPOINT.into(),
                    wire_api: "responses".into(),
                    requires_openai_auth: true,
                    supports_websockets: true,
                },
                Some(auth),
            )
        }
    };
    let sandbox = sandbox.context("explicit stock sandbox required")?;
    ensure!(
        matches!(
            sandbox.as_str(),
            "read-only" | "workspace-write" | "danger-full-access"
        ),
        "unsupported stock sandbox"
    );
    // Approval remains explicit: do not silently turn full access into never.
    let approval = approval.context("explicit stock approval policy required")?;
    ensure!(
        matches!(approval.as_str(), "untrusted" | "on-request" | "never"),
        "unsupported stock approval policy"
    );
    if let Some(permission) = permission {
        ensure!(
            matches!(
                (permission, sandbox.as_str()),
                ("read-only", "read-only")
                    | ("workspace", "workspace-write")
                    | ("full-access", "danger-full-access")
            ),
            "unknown or inconsistent stock permission alias"
        );
    }
    let model = selected_model.cloned().unwrap_or(profile.model);
    ensure!(
        !model.trim().is_empty() && !model.chars().any(char::is_control),
        "invalid stock model"
    );
    let reasoning = selected_reasoning
        .cloned()
        .or(profile.model_reasoning_effort);
    if aemeath_auth.is_some() {
        super::aemeath_auth::validate_model(&model, reasoning.as_deref())?;
    }
    ensure!(
        reasoning
            .as_deref()
            .is_none_or(|r| matches!(r, "none" | "minimal" | "low" | "medium" | "high" | "xhigh")),
        "unsupported stock reasoning"
    );
    use sha2::Digest;
    Ok(StockConfiguration {
        selected_projection: None,
        aemeath_auth,
        profile_name: name.clone(),
        profile_id: account.id.clone(),
        inherited,
        profile_sha256: file_sha256(&files.config_path)?,
        account_sha256: Sha256::new(format!(
            "{:x}",
            sha2::Sha256::digest(serde_json::to_vec(account)?)
        ))
        .map_err(|_| anyhow::anyhow!("invalid account digest"))?,
        model,
        reasoning,
        model_provider: profile.model_provider,
        provider,
        sandbox,
        approval,
    })
}

fn validate_selected_permissions(
    permission: Option<&str>,
    sandbox: &str,
    approval: &str,
) -> anyhow::Result<()> {
    ensure!(
        matches!(approval, "never" | "on-request" | "untrusted"),
        "unsupported selected approval"
    );
    let expected = match permission {
        Some("full-access" | "danger-full-access" | ":danger-full-access" | "danger") => {
            "danger-full-access"
        }
        Some("read-only" | ":read-only" | "readonly" | "read") => "read-only",
        Some("workspace" | ":workspace" | "workspace-write") => "workspace-write",
        _ => anyhow::bail!("unknown selected permission alias"),
    };
    ensure!(
        sandbox == expected,
        "inconsistent selected sandbox and permission alias"
    );
    Ok(())
}

pub fn validate_native(
    record: &CutexSessionRecord,
    sessions: &CutexSessionStore,
    contract: &ExplicitLaunchContract,
) -> anyhow::Result<PathBuf> {
    ensure!(
        crate::runtime::lifecycle::cutex_session_host_is_local(
            &record.host_id,
            &crate::platform::host::current_host_name()
        ),
        "stock host is not local"
    );
    ensure!(
        record.runtime_backend == CutexSessionRuntimeBackend::Host,
        "stock subset requires existing host backend"
    );
    ensure!(
        record.codex_session_id.as_deref() == Some(&contract.native_id),
        "stock native identity mismatch"
    );
    ensure!(
        sessions
            .sessions
            .values()
            .filter(|r| r.codex_session_id.as_deref() == Some(&contract.native_id))
            .count()
            == 1,
        "ambiguous durable/native mapping"
    );
    if contract.version == 4 {
        ensure!(
            contract.native_home
                == crate::config::paths::runtime_dir()?
                    .join("light/agents")
                    .join(&contract.native_id)
                    .canonicalize()?,
            "local runtime home does not match saved identity"
        );
    } else if contract.version == 3 {
        #[cfg(target_os = "linux")]
        crate::agent_management::validate_migration_home(record, sessions, contract)?;
        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("maintenance home unsupported on this platform");
    } else {
        ensure!(
            crate::config::paths::host_codex_home_dir()?.canonicalize()? == contract.native_home,
            "stock home is not authoritative native home"
        );
    }
    current_configuration(record)?.validate_auth_home(&contract.native_home)?;
    super::native_history::current(&contract.native_home, &contract.native_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_permission_aliases_preserve_effective_policy() {
        for (alias, sandbox) in [
            (":danger-full-access", "danger-full-access"),
            ("danger-full-access", "danger-full-access"),
            (":read-only", "read-only"),
        ] {
            for approval in ["never", "on-request"] {
                assert!(validate_selected_permissions(Some(alias), sandbox, approval).is_ok());
            }
            assert!(validate_selected_permissions(Some(alias), "guessed", "never").is_err());
        }
        assert!(
            validate_selected_permissions(Some("unknown"), "danger-full-access", "never").is_err()
        );
        assert!(validate_selected_permissions(Some(":read-only"), "read-only", "always").is_err());
    }
    #[test]
    #[ignore = "requires the task-owned accepted native manifest"]
    fn accepted_auth_manifest_matches_compiled_pins() {
        let path=Path::new("/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/custom-status-r1/build-manifest.json");
        let bytes = std::fs::read(path).expect("accepted immutable native manifest");
        use sha2::Digest;
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&bytes)),
            "6727a35e9154ed53f1384e3c3bcc5362097dd4ef6e3c9de7dd93dec1d3556063"
        );
        let m: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m["source_commit"], S6E_COMMIT);
        for (name, expected) in [
            ("bundle/codex", S6E_CLI_SHA256),
            ("bundle/codex-app-server", S6E_EXECUTABLE_SHA256),
            ("bundle/codex-code-mode-host", STOCK_HOST_SHA256),
            (
                "bundle/codex_app_server_protocol.schemas.json",
                S6E_SCHEMA_SHA256,
            ),
        ] {
            assert_eq!(m["files"][name]["sha256"], expected);
        }
    }
    #[test]
    fn local_deployment_accepts_rebuilt_bytes_but_requires_supported_protocol() {
        let file = |hash: &str| VerifiedFile {
            path: "/private/file".into(),
            sha256: Sha256::new(hash.to_string()).unwrap(),
        };
        let mut bundle = StockBundle {
            version: 4,
            upstream_commit: "local-upstream".into(),
            native_patch_commit: Some("local-fix".into()),
            executable: file(&"a".repeat(64)),
            cli: Some(file(&"b".repeat(64))),
            code_mode_host: file(&"c".repeat(64)),
            facade: file(&"d".repeat(64)),
            schema: file(S6E_SCHEMA_SHA256),
            launch_config_sha256: None,
            shared_config: file(&"e".repeat(64)),
        };
        bundle.validate_identity().unwrap();
        assert!(bundle.common_ingress() && bundle.soon_ingress());
        // Artifact eligibility is not successful integrity validation.
        assert!(bundle.validate_components().is_err());
        bundle.schema = file(S6_SCHEMA_SHA256);
        assert!(bundle.validate_identity().is_err());
        bundle.schema = file(S6E_SCHEMA_SHA256);
        bundle.cli = None;
        assert!(bundle.validate_identity().is_err());
    }

    #[test]
    fn exact_s6_bundle_identity_not_boolean_capability() {
        let file = |hash: &str| VerifiedFile {
            path: "/private/file".into(),
            sha256: Sha256::new(hash.to_string()).unwrap(),
        };
        let mut b = StockBundle {
            version: 2,
            upstream_commit: STOCK_COMMIT.into(),
            native_patch_commit: Some(S6_COMMIT.into()),
            executable: file(S6_EXECUTABLE_SHA256),
            cli: None,
            code_mode_host: file(STOCK_HOST_SHA256),
            facade: file(STOCK_HOST_SHA256),
            schema: file(S6_SCHEMA_SHA256),
            launch_config_sha256: None,
            shared_config: file(STOCK_HOST_SHA256),
        };
        b.validate_identity().unwrap();
        assert!(b.common_ingress());
        b.native_patch_commit = Some(STOCK_COMMIT.into());
        assert!(b.validate_identity().is_err());
        b.native_patch_commit = Some(S6_COMMIT.into());
        b.executable = file(STOCK_EXECUTABLE_SHA256);
        assert!(b.validate_identity().is_err());
        b.version = 1;
        b.native_patch_commit = None;
        b.schema = file(STOCK_SCHEMA_SHA256);
        b.validate_identity().unwrap();
        assert!(!b.common_ingress());
        b.version = 3;
        b.native_patch_commit = Some(S6E_COMMIT.into());
        b.executable = file(S6E_EXECUTABLE_SHA256);
        b.schema = file(S6E_SCHEMA_SHA256);
        assert!(b.validate_identity().is_err());
        b.cli = Some(file(S6E_CLI_SHA256));
        b.validate_identity().unwrap();
        assert!(b.common_ingress() && b.soon_ingress());
        let accepted = b.clone();
        // Each mixed/spoofed component rejects independently, including the
        // previous coherent bundle. No descendant/version wildcard or fallback.
        for (field, value) in [
            (
                "native_patch_commit",
                "cc4a080df1df4433f6fd67fee9c1c4fa4a42baab",
            ),
            (
                "cli",
                "52b868441c65cf12370ffbd7dea30d972b801f875b7e3d69b41ce2a0501fd2a9",
            ),
            (
                "executable",
                "df95936f3f0d1ff62efb978fee32b85e45da0f7f3166de7606cce3b49fb12018",
            ),
            (
                "schema",
                "77e75b7fc47c9b8a9caacf5a7d31c040c6520679a9833f1c2b0e55937ed39c27",
            ),
            (
                "native_patch_commit",
                "3d8a73a747cf5b957a7ca0491c28d1517f6d7722",
            ),
            (
                "cli",
                "d97f8d4f32377b22066d25dd545b79b32bb9cacb8b501f9db9a21c290504ad60",
            ),
            (
                "native_patch_commit",
                "ca580a783fc1ab34613be4f81ceab96ef4d393a2",
            ),
            (
                "executable",
                "cac03d6d1b77e4d3681a2d71b25435a537bc257807c28ea0f5aff491c622b69d",
            ),
            (
                "cli",
                "9e887b3a3439154fc00f8bb2a44b0f534ba33c587a5aa0f2aed354b7842bef15",
            ),
            (
                "schema",
                "459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9",
            ),
            (
                "native_patch_commit",
                "0c425b5f9fca90835fd2b4377a1bca212532f66c",
            ),
            (
                "executable",
                "9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde",
            ),
            (
                "cli",
                "f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4",
            ),
            (
                "native_patch_commit",
                "a83dbb47ba6aa775f5d4b679fafc532c4db74c7f",
            ),
            (
                "executable",
                "4638b86221593dd4bab1f66b504641836ac1adb864e946cbe42cb5cbf9f05a74",
            ),
            (
                "cli",
                "f360100339560e6a57eb08dea38ed740450904ce7bd72ea7b8a37238fff97bd6",
            ),
            ("code_mode_host", STOCK_EXECUTABLE_SHA256),
            ("schema", S6_SCHEMA_SHA256),
            ("executable", STOCK_HOST_SHA256),
        ] {
            let mut raw = serde_json::to_value(&accepted).unwrap();
            if field == "native_patch_commit" {
                raw[field] = value.into();
            } else {
                raw[field]["sha256"] = value.into();
            }
            let rejected: StockBundle = serde_json::from_value(raw).unwrap();
            assert!(rejected.validate_identity().is_err(), "{field}");
        }
        let mut old = accepted.clone();
        old.native_patch_commit = Some("a83dbb47ba6aa775f5d4b679fafc532c4db74c7f".into());
        old.executable = file("4638b86221593dd4bab1f66b504641836ac1adb864e946cbe42cb5cbf9f05a74");
        old.cli = Some(file(
            "f360100339560e6a57eb08dea38ed740450904ce7bd72ea7b8a37238fff97bd6",
        ));
        assert!(old.validate_identity().is_err());
        assert!(!old.common_ingress() && !old.soon_ingress());
        old.native_patch_commit = Some("0c425b5f9fca90835fd2b4377a1bca212532f66c".into());
        old.executable = file("9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde");
        old.cli = Some(file(
            "f93c92bfe528636eae87450d918700d90d24db7d8f2eee0f4dd0fe5cbee998f4",
        ));
        assert!(old.validate_identity().is_err());
        assert!(!old.common_ingress() && !old.soon_ingress());
        b.version = 2;
        assert!(b.validate_identity().is_err());
        b.version = 3;
        b.cli = Some(file(STOCK_EXECUTABLE_SHA256));
        assert!(b.validate_identity().is_err());
        let mut raw = serde_json::to_value(b).unwrap();
        raw["externalInput"] = true.into();
        assert!(serde_json::from_value::<StockBundle>(raw).is_err());
    }
    #[test]
    fn soon_requires_positive_reviewed_capability() {
        let legacy = serde_json::json!({"externalInputVersion":1});
        assert!(validate_ingress_capability(S6_SCHEMA_SHA256, &legacy).is_ok());
        assert!(validate_ingress_capability(S6E_SCHEMA_SHA256, &legacy).is_err());
        assert!(validate_ingress_capability(S6E_SCHEMA_SHA256, &serde_json::json!({"externalInputVersion":1,"externalInputDeliveries":["after_turn","passive"]})).is_err());
        assert!(validate_ingress_capability(S6E_SCHEMA_SHA256, &serde_json::json!({"externalInputVersion":1,"externalInputDeliveries":["after_turn","passive","soon"]})).is_err());
        assert!(validate_ingress_capability(S6E_SCHEMA_SHA256, &serde_json::json!({"externalInputVersion":1,"presentationVersion":1,"externalInputDeliveries":["after_turn","passive","soon"]})).is_err());
        assert!(validate_ingress_capability(S6E_SCHEMA_SHA256, &serde_json::json!({"externalInputVersion":1,"externalInputVersions":[1,2],"presentationVersion":1,"externalInputDeliveries":["after_turn","passive","soon"]})).is_ok());
        assert!(validate_ingress_capability(
            S6E_SCHEMA_SHA256,
            &serde_json::json!({"externalInputVersion":2,"externalInputDeliveries":["soon"]})
        )
        .is_err());
    }
    #[test]
    fn aemeath_profile_mode_is_explicit_and_unknown_mcp_stays_rejected() {
        let base = "model='gpt-5.6-terra'\nmodel_provider='openai'\nmodel_reasoning_effort='low'\n";
        let legacy: ProfileConfig = toml::from_str(base).unwrap();
        assert!(legacy.cutex_provider_mode == ProviderMode::Fake);
        let explicit: ProfileConfig =
            toml::from_str(&format!("{base}cutex_provider_mode='aemeath_chatgpt_v1'\n")).unwrap();
        assert!(explicit.cutex_provider_mode == ProviderMode::AemeathChatgptV1);
        assert!(toml::from_str::<ProfileConfig>(&format!(
            "{base}cutex_provider_mode='aemeath_chatgpt_v2'\n"
        ))
        .is_err());
        assert!(toml::from_str::<ProfileConfig>(&format!(
            "{base}[mcp_servers.arbitrary]\ncommand='anything'\n"
        ))
        .is_err());
    }
    #[test]
    fn presentation_changes_do_not_change_launch_semantics() {
        let base = "cutex_projection_version=2\nmodel='gpt-5'\n";
        let expected = launch_config_digest(base).unwrap();
        for tui in ["[tui]\nstatus_line=['model-name']\n", "[tui]\nanimations=false\n"] {
            assert_eq!(launch_config_digest(&format!("{base}{tui}")).unwrap(), expected);
        }
        assert_ne!(launch_config_digest(&base.replace("gpt-5", "other-model")).unwrap(), expected);
        assert!(launch_config_digest(&format!("{base}unknown=true\n")).is_ok());
    }

    #[test]
    fn ordinary_settings_do_not_require_migration_marker() {
        for raw in [
            "model='gpt-6-astra'\nmodel_reasoning_effort='xhigh'",
            "sandbox_mode='danger-full-access'\napprovals_reviewer='user'",
            "service_tier='priority'\nplan_mode_reasoning_effort='high'",
            "[tui]\nstatus_line=['model-name']",
            "[[skills.config]]\npath='/skills/example/SKILL.md'\nenabled=false",
            "[plugins.example]\nenabled=false",
        ] { validate_shared_config(raw).unwrap(); }
    }

    #[test]
    fn stock_unknown_bundle_fields_and_versions_reject_without_spawn() {
        let json = serde_json::json!({"version":1,"native_id":"not-a-native-id","native_home":"/","bundle_manifest":"/","bundle_sha256":"a".repeat(64)});
        let contract: ExplicitLaunchContract = serde_json::from_value(json.clone()).unwrap();
        assert!(contract.validate().is_err());
        let mut changed = json;
        changed["fallback"] = serde_json::json!("codex");
        assert!(serde_json::from_value::<ExplicitLaunchContract>(changed).is_err());
    }
    #[test]
    fn stock_requirement_survives_runtime_clear_and_serialization() {
        let mut record = CutexSessionRecord::new(
            "cutex.stock".into(),
            Some(uuid::Uuid::new_v4().to_string()),
            "private".into(),
            "/private".into(),
            None,
        )
        .unwrap();
        assert!(crate::agent_management::require_default_launch(&record).is_ok());
        record.explicit_launch = Some(ExplicitLaunchContract {
            version: 999,
            migration_action_id: None,
            native_id: record.codex_session_id.clone().unwrap(),
            native_home: "/private".into(),
            bundle_manifest: "/private/bundle".into(),
            bundle_sha256: Sha256::new("a".repeat(64)).unwrap(),
        });
        let contract = record.explicit_launch.clone();
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(record.cutex_session_id.clone(), record);
        crate::session::service::clear_cutex_session_runtime_record(
            &mut store,
            "cutex.stock",
            false,
        )
        .unwrap();
        let record = &store.sessions["cutex.stock"];
        assert_eq!(record.explicit_launch, contract);
        assert!(crate::agent_management::require_default_launch(&record).is_err());
        let decoded: CutexSessionRecord =
            serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
        assert_eq!(decoded.explicit_launch, contract);
        crate::session::service::set_cutex_session_profile_by_key(
            &mut store,
            "cutex.stock",
            Some("changed-profile".into()),
        )
        .unwrap();
        let record = store.sessions.get_mut("cutex.stock").unwrap();
        let revision = record.durable_revision();
        crate::session::archive::commit_retire(
            record,
            revision,
            0,
            true,
            "2026-09-09T00:00:00Z".into(),
        )
        .unwrap();
        assert_eq!(record.explicit_launch, contract);
        let revision = record.durable_revision();
        crate::session::archive::commit_restore(record, revision, "2026-09-09T00:00:01Z".into())
            .unwrap();
        assert_eq!(record.explicit_launch, contract);
        assert_eq!(record.profile.as_deref(), Some("changed-profile"));
        assert!(crate::agent_management::require_default_launch(record).is_err());
    }
}

#[cfg(test)]
mod native_plugin_config_tests {
    use super::*;
    const CONFIG: &str = r#"
cutex_projection_version=2
[plugins."visualize@openai-bundled"]
enabled=true
[marketplaces.openai-bundled]
source_type="local"
source="/native/.tmp/bundled-marketplaces/openai-bundled"
"#;
    #[test]
    fn native_plugins_and_marketplaces_survive_adoption_config_and_remain_bound() {
        validate_shared_config(CONFIG).unwrap();
        let hash = launch_config_digest(CONFIG).unwrap();
        assert_ne!(hash, launch_config_digest(&CONFIG.replace("enabled=true", "enabled=false")).unwrap());
        assert_ne!(hash, launch_config_digest(&CONFIG.replace("/native/", "/other/")).unwrap());
        let rich = format!("{CONFIG}last_updated='today'\nlast_revision='revision'\nref='main'\nsparse_paths=['plugins']\n[plugins.\"visualize@openai-bundled\".mcp_servers.visualize]\ndefault_tools_approval_mode='prompt'\nenabled_tools=['view']\ndisabled_tools=['delete']\n[plugins.\"visualize@openai-bundled\".mcp_servers.visualize.tools.view]\napproval_mode='auto'\noutput_token_limit=1000\n");
        validate_shared_config(&rich).unwrap();
        // Native owns semantic validation of plugin options.
        assert!(validate_shared_config(&CONFIG.replace("source_type=\"local\"", "source_type=\"git\"")).is_ok());
    }
    #[test]
    #[ignore = "explicit read-only live shared config probe"]
    fn actual_shared_config_is_accepted_after_projection_marker() {
        let path = std::env::var("CUTEX_SHARED_CONFIG_TEST_FILE").unwrap();
        let mut value: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        value.as_table_mut().unwrap().insert("cutex_projection_version".into(), 2.into());
        let raw = toml::to_string(&value).unwrap();
        validate_shared_config(&raw).unwrap();
        launch_config_digest(&raw).unwrap();
    }
    #[test]
    fn rejected_config_reports_field_without_echoing_source_document() {
        let error = validate_shared_config("private_unknown_key='SENSITIVE_SENTINEL").unwrap_err().to_string();
        assert!(error.contains("invalid native configuration"));
        assert!(!error.contains("SENSITIVE_SENTINEL"));
        assert!(validate_shared_config(&format!("{CONFIG}unknown_option=true\n")).is_ok());
    }
}
