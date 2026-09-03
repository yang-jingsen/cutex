//! Private Owner-configured Release template store.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256 as Sha256Hasher};

use crate::agent_bus::identity::normalize_agent_groups;
use crate::role_revision::{Rfc3339, Sha256, MAX_JSON_SAFE_INTEGER};
use crate::task_service::ActionId;

use super::{ConfigureReleaseTemplateRequest, ReleaseTemplate, ReleaseTemplateReceipt};

const STORE_FILE: &str = "release-template-v1.json";
const LOCK_FILE: &str = "release-template-v1.lock";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ReleaseTemplateStoreSchema {
    #[serde(rename = "cutex/release-template-store/v1")]
    V1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTemplateSnapshot {
    pub schema: ReleaseTemplateStoreSchema,
    pub store_revision: u64,
    pub current_template: Option<ReleaseTemplate>,
    pub current_template_sha256: Option<Sha256>,
    pub receipts: BTreeMap<ActionId, ReleaseTemplateReceipt>,
}

impl ReleaseTemplateSnapshot {
    fn empty() -> Self {
        Self {
            schema: ReleaseTemplateStoreSchema::V1,
            store_revision: 0,
            current_template: None,
            current_template_sha256: None,
            receipts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseTemplateError {
    InvalidRequest(&'static str),
    Conflict(&'static str),
    PersistenceUnavailable,
    InvalidStore,
    Io(io::ErrorKind),
}

impl fmt::Display for ReleaseTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "release template error: {self:?}")
    }
}

impl std::error::Error for ReleaseTemplateError {}

impl From<io::Error> for ReleaseTemplateError {
    fn from(value: io::Error) -> Self {
        Self::Io(value.kind())
    }
}

#[derive(Clone)]
pub struct ReleaseTemplateStore {
    root: Arc<PathBuf>,
    process_lock: Arc<Mutex<()>>,
}

impl ReleaseTemplateStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReleaseTemplateError> {
        let root = root.into();
        prepare_private_root(&root)?;
        Ok(Self {
            root: Arc::new(root),
            process_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn open_default() -> anyhow::Result<Self> {
        Self::open(
            crate::config::paths::runtime_dir()?
                .join("release-rotation-v1")
                .join("template"),
        )
        .map_err(anyhow::Error::new)
    }

    pub fn configure(
        &self,
        request: &ConfigureReleaseTemplateRequest,
    ) -> Result<ReleaseTemplateReceipt, ReleaseTemplateError> {
        validate_template(&request.template)?;
        let request_sha256 = digest(request)?;
        let template_sha256 = digest(&request.template)?;
        self.with_locked_state(true, |mut state| {
            if let Some(receipt) = state.receipts.get(&request.action_id).cloned() {
                return if receipt.request_sha256 == request_sha256 {
                    Ok((state, receipt, false))
                } else {
                    Err(ReleaseTemplateError::Conflict("action_id_payload_conflict"))
                };
            }
            let current_version = state
                .current_template
                .as_ref()
                .map(|template| template.version);
            if current_version != request.expected_current_version {
                return Err(ReleaseTemplateError::Conflict("stale_template_version"));
            }
            let expected_next = current_version
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(ReleaseTemplateError::Conflict("template_version_overflow"))?;
            if request.template.version != expected_next {
                return Err(ReleaseTemplateError::InvalidRequest(
                    "template_version_must_increment_once",
                ));
            }
            let revision = state
                .store_revision
                .checked_add(1)
                .filter(|value| *value <= MAX_JSON_SAFE_INTEGER)
                .ok_or(ReleaseTemplateError::Conflict("store_revision_overflow"))?;
            let receipt = ReleaseTemplateReceipt {
                action_id: request.action_id.clone(),
                request_sha256,
                template_version: request.template.version,
                template_sha256: template_sha256.clone(),
                configured_at: now(),
            };
            state.store_revision = revision;
            state.current_template = Some(request.template.clone());
            state.current_template_sha256 = Some(template_sha256);
            state
                .receipts
                .insert(request.action_id.clone(), receipt.clone());
            Ok((state, receipt, true))
        })
    }

    pub fn query(&self) -> Result<ReleaseTemplateSnapshot, ReleaseTemplateError> {
        self.with_locked_state(false, |state| Ok((state.clone(), state, false)))
    }

    /// Execute a final cross-store authority commit while configuration is
    /// held at one exact current template. The nested result preserves the
    /// caller's typed error without weakening template-store failures.
    pub(crate) fn with_current_template<T, E>(
        &self,
        operation: impl FnOnce(&ReleaseTemplate, &Sha256) -> Result<T, E>,
    ) -> Result<Result<T, E>, ReleaseTemplateError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ReleaseTemplateError::PersistenceUnavailable)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        set_private_open_options(&mut options);
        let lock = match options.open(self.root.join(LOCK_FILE)) {
            Ok(lock) => lock,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ReleaseTemplateError::Conflict(
                    "release_template_not_configured",
                ))
            }
            Err(error) => return Err(error.into()),
        };
        lock.lock_shared()?;
        let snapshot = read_snapshot(&self.root)?;
        let template = snapshot
            .current_template
            .as_ref()
            .ok_or(ReleaseTemplateError::Conflict(
                "release_template_not_configured",
            ))?;
        let sha256 = snapshot
            .current_template_sha256
            .as_ref()
            .ok_or(ReleaseTemplateError::InvalidStore)?;
        Ok(operation(template, sha256))
    }

    fn with_locked_state<T>(
        &self,
        create: bool,
        operation: impl FnOnce(
            ReleaseTemplateSnapshot,
        )
            -> Result<(ReleaseTemplateSnapshot, T, bool), ReleaseTemplateError>,
    ) -> Result<T, ReleaseTemplateError> {
        let _process = self
            .process_lock
            .lock()
            .map_err(|_| ReleaseTemplateError::PersistenceUnavailable)?;
        let lock_path = self.root.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(create);
        set_private_open_options(&mut options);
        let lock = match options.open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                let (state, value, write) = operation(ReleaseTemplateSnapshot::empty())?;
                debug_assert!(!write);
                let _ = state;
                return Ok(value);
            }
            Err(error) => return Err(error.into()),
        };
        lock.lock_exclusive()?;
        let state = read_snapshot(&self.root)?;
        let (state, value, write) = operation(state)?;
        if write {
            write_snapshot(&self.root, &state)?;
        }
        Ok(value)
    }
}

pub fn validate_template(template: &ReleaseTemplate) -> Result<(), ReleaseTemplateError> {
    if template.version == 0 || template.version > MAX_JSON_SAFE_INTEGER {
        return Err(ReleaseTemplateError::InvalidRequest(
            "invalid_template_version",
        ));
    }
    validate_text(&template.successor_name, 128, "invalid_successor_name")?;
    validate_text(&template.cwd, 4096, "invalid_cwd")?;
    if !Path::new(&template.cwd).is_absolute() {
        return Err(ReleaseTemplateError::InvalidRequest(
            "successor_cwd_must_be_absolute",
        ));
    }
    if let Some(cwd) = &template.managed_cwd {
        validate_text(cwd, 4096, "invalid_managed_cwd")?;
    }
    validate_text(
        &template.role_package.reference,
        4096,
        "invalid_role_package_reference",
    )?;
    if !is_confined_role_package_reference(Path::new(&template.role_package.reference)) {
        return Err(ReleaseTemplateError::InvalidRequest(
            "invalid_role_package_reference",
        ));
    }
    if template.agent_groups.is_empty()
        || normalize_agent_groups(template.agent_groups.clone()) != template.agent_groups
    {
        return Err(ReleaseTemplateError::InvalidRequest(
            "agent_groups_must_be_normalized_and_unique",
        ));
    }
    if template.agent_groups.len() > 64 {
        return Err(ReleaseTemplateError::InvalidRequest(
            "too_many_agent_groups",
        ));
    }
    for (value, reason) in [
        (&template.profile, "invalid_profile"),
        (&template.model, "invalid_model"),
        (&template.reasoning, "invalid_reasoning"),
        (&template.permissions, "invalid_permissions"),
        (&template.approval_policy, "invalid_approval_policy"),
        (&template.sandbox_mode, "invalid_sandbox_mode"),
    ] {
        if let Some(value) = value {
            validate_text(value, 512, reason)?;
        }
    }
    if template.default_cli_args.len() > 128
        || template
            .default_cli_args
            .iter()
            .any(|value| validate_text(value, 4096, "invalid_default_cli_arg").is_err())
        || template.default_cli_args.iter().any(|value| {
            matches!(
                value.as_str(),
                "resume" | "fork" | "--thread-id" | "--session-id" | "--conversation-id"
            ) || value.starts_with("--thread-id=")
                || value.starts_with("--session-id=")
                || value.starts_with("--conversation-id=")
        })
    {
        return Err(ReleaseTemplateError::InvalidRequest(
            "invalid_default_cli_args",
        ));
    }
    Ok(())
}

fn is_confined_role_package_reference(reference: &Path) -> bool {
    if reference.is_absolute() {
        return false;
    }
    let mut has_normal_component = false;
    for component in reference.components() {
        match component {
            Component::Normal(_) => has_normal_component = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    has_normal_component
}

fn validate_text(
    value: &str,
    maximum_bytes: usize,
    reason: &'static str,
) -> Result<(), ReleaseTemplateError> {
    if value.trim() != value
        || value.is_empty()
        || value.len() > maximum_bytes
        || value.chars().any(char::is_control)
    {
        return Err(ReleaseTemplateError::InvalidRequest(reason));
    }
    Ok(())
}

pub(crate) fn digest<T: Serialize>(value: &T) -> Result<Sha256, ReleaseTemplateError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ReleaseTemplateError::InvalidRequest("invalid_request"))?;
    Sha256::new(format!("{:x}", Sha256Hasher::digest(bytes)))
        .map_err(|_| ReleaseTemplateError::InvalidStore)
}

fn prepare_private_root(root: &Path) -> Result<(), ReleaseTemplateError> {
    if let Ok(metadata) = fs::symlink_metadata(root) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ReleaseTemplateError::InvalidStore);
        }
    } else {
        fs::create_dir_all(root)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_snapshot(root: &Path) -> Result<ReleaseTemplateSnapshot, ReleaseTemplateError> {
    let mut file = match File::open(root.join(STORE_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ReleaseTemplateSnapshot::empty())
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| ReleaseTemplateError::InvalidStore)
}

fn write_snapshot(
    root: &Path,
    snapshot: &ReleaseTemplateSnapshot,
) -> Result<(), ReleaseTemplateError> {
    let bytes = serde_json::to_vec(snapshot).map_err(|_| ReleaseTemplateError::InvalidStore)?;
    let temporary = root.join(format!(".{STORE_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_private_open_options(&mut options);
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, root.join(STORE_FILE))?;
    File::open(root)?.sync_all()?;
    Ok(())
}

fn now() -> Rfc3339 {
    Rfc3339::new(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        .expect("UTC timestamp is normalized RFC3339")
}

fn set_private_open_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agent_bus::model::AgentRegistrationClass;
    use crate::rotation::{
        ReleaseRolePackage, ReleaseTemplateCommandSchema, ReleaseTemplateSchema,
    };
    use crate::session::model::{CutexSessionQuickActionMode, CutexSessionRuntimeBackend};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("cutex-release-template-{}", uuid::Uuid::new_v4()))
    }

    pub(crate) fn template(version: u64) -> ReleaseTemplate {
        ReleaseTemplate {
            schema: ReleaseTemplateSchema::V1,
            version,
            successor_name: "cutex-release-r6".to_string(),
            cwd: "/srv/cutex-release".to_string(),
            managed_cwd: Some("/srv/cutex-release".to_string()),
            runtime_backend: CutexSessionRuntimeBackend::Host,
            role_package: ReleaseRolePackage {
                reference: "roles/cutex-release/v6".to_string(),
                sha256: Sha256::new("1".repeat(64)).expect("hash"),
            },
            agent_groups: vec!["cutex".to_string()],
            profile: Some("release".to_string()),
            model: Some("gpt-5.6-sol".to_string()),
            reasoning: Some("high".to_string()),
            permissions: Some("workspace-write".to_string()),
            approval_policy: Some("never".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            exposed_to_backend: true,
            quick_action: CutexSessionQuickActionMode::Pinned,
            registration_class: AgentRegistrationClass::Persistent,
            default_cli_args: vec!["--no-alt-screen".to_string()],
        }
    }

    fn request(action: &str, version: u64) -> ConfigureReleaseTemplateRequest {
        ConfigureReleaseTemplateRequest {
            schema: ReleaseTemplateCommandSchema::V1,
            action_id: ActionId::new(action).expect("action"),
            expected_current_version: (version > 1).then_some(version - 1),
            template: template(version),
        }
    }

    #[test]
    fn versioned_configuration_is_cas_and_exactly_idempotent() {
        let root = root();
        let store = ReleaseTemplateStore::open(&root).expect("open");
        let first_request = request("template-1", 1);
        let first = store.configure(&first_request).expect("configure");
        assert_eq!(store.configure(&first_request).expect("replay"), first);
        let mut changed = first_request.clone();
        changed.template.cwd = "/other".to_string();
        assert_eq!(
            store.configure(&changed),
            Err(ReleaseTemplateError::Conflict("action_id_payload_conflict"))
        );
        let second = store.configure(&request("template-2", 2)).expect("v2");
        assert_eq!(second.template_version, 2);
        assert_eq!(
            store.configure(&request("stale", 1)),
            Err(ReleaseTemplateError::Conflict("stale_template_version"))
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn strict_template_rejects_volatile_or_predecessor_fields() {
        let value = serde_json::to_value(template(1)).expect("serialize");
        for forbidden in [
            "runtime_agent_id",
            "pid",
            "runtime_generation",
            "predecessor_thread_id",
            "codex_session_id",
        ] {
            let mut changed = value.clone();
            changed[forbidden] = serde_json::json!("forged");
            assert!(serde_json::from_value::<ReleaseTemplate>(changed).is_err());
        }
        let mut targeted = template(1);
        targeted.default_cli_args = vec!["resume".to_string(), "thread-old".to_string()];
        assert_eq!(
            validate_template(&targeted),
            Err(ReleaseTemplateError::InvalidRequest(
                "invalid_default_cli_args"
            ))
        );
    }

    #[test]
    fn template_rejects_absolute_and_parent_traversing_role_packages() {
        for reference in ["/outside/role.md", "../role.md", "roles/../role.md"] {
            let mut targeted = template(1);
            targeted.role_package.reference = reference.to_string();
            assert_eq!(
                validate_template(&targeted),
                Err(ReleaseTemplateError::InvalidRequest(
                    "invalid_role_package_reference"
                )),
                "reference {reference:?} must not escape configured cwd"
            );
        }
    }

    #[test]
    fn template_requires_absolute_successor_cwd() {
        let mut targeted = template(1);
        targeted.cwd = "relative/release".to_string();
        assert_eq!(
            validate_template(&targeted),
            Err(ReleaseTemplateError::InvalidRequest(
                "successor_cwd_must_be_absolute"
            ))
        );
    }
}
