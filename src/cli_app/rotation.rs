//! Production lifecycle adapter for mechanical Release rotation.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use chrono::Utc;
use sha2::{Digest, Sha256};

use cutex::agent_bus::delivery::AgentDeliveryMode;
use cutex::agent_bus::model::{
    AgentBusEnvelopeKind, AgentBusSendRequest, AgentBusSendResponse, AgentMessageKind,
};
use cutex::agent_bus::store::AgentBusState;
use cutex::role_revision::CutexSessionId;
use cutex::rotation::{
    ManagementReleaseRotationRequest, ReleaseRotationError, ReleaseRotationInvocation,
    ReleaseRotationLifecycle, ReleaseRotationOutcome, ReleaseRotationProvider,
    ReleaseRotationRequest, ReleaseRotationResponse, ReleaseRotationResponseSchema,
    ReleaseRotationStatus, ReleaseTemplate, RetryReleaseRotationRequest,
};
use cutex::session::model::CutexSessionRecord;
use cutex::session::service::{
    coding_registration_from_cutex_session_record, cutex_session_key_for_user_id_including_retired,
    persist_cutex_session_store_and_im_record,
};
use cutex::session::store::load_cutex_session_store;
use cutex::task_service::ActionId;

use super::{app_server_runtime, management_archive, management_lifecycle};

const RELEASE_ROLE_PACKAGE_MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn handle_release_rotation(
    _state: &Arc<Mutex<AgentBusState>>,
    invocation: ReleaseRotationInvocation,
    request: ReleaseRotationRequest,
) -> anyhow::Result<serde_json::Value> {
    // Agent Bus authenticates the live Director and holds the Task Service
    // execution fence. The Management owner must execute runtime lifecycle,
    // because it owns the app-server manager and process claims.
    let config = cutex::config::store::load_codez_config();
    let token = cutex::management::service::task_service_seat_credential(&config, None)?;
    let body = serde_json::to_vec(&ManagementReleaseRotationRequest {
        invocation,
        request,
    })?;
    cutex::management::remote::management_http_json(
        &cutex::management::service::management_base_url(
            cutex::management::service::DEFAULT_MANAGEMENT_PORT,
        ),
        "POST",
        "/v2/release-rotation/director-request",
        Some(&token),
        Some(&body),
    )
}

pub(crate) fn execute_release_rotation(
    invocation: ReleaseRotationInvocation,
    request: ReleaseRotationRequest,
) -> anyhow::Result<serde_json::Value> {
    let action_id = request.action_id.clone();
    let lifecycle = CutexReleaseRotationLifecycle {
        director_runtime_agent_id: Some(invocation.director_runtime_agent_id),
    };
    let outcome = match ReleaseRotationProvider::open_default() {
        Ok(provider) => match provider.request_director(
            &invocation.director_cutex_session,
            &request,
            invocation.predecessor_has_nonterminal_assignment,
            &lifecycle,
        ) {
            Ok(receipt) if receipt.status == ReleaseRotationStatus::Complete => {
                ReleaseRotationOutcome::Complete { receipt }
            }
            Ok(receipt) => ReleaseRotationOutcome::Blocked { receipt },
            Err(ReleaseRotationError::Blocked(receipt)) => {
                ReleaseRotationOutcome::Blocked { receipt }
            }
            Err(error) => ReleaseRotationOutcome::NoWrite {
                code: error.code().to_string(),
                reason: error.to_string(),
            },
        },
        Err(_) => ReleaseRotationOutcome::NoWrite {
            code: "persistence_unavailable".to_string(),
            reason: "Release rotation provider is unavailable".to_string(),
        },
    };
    serde_json::to_value(ReleaseRotationResponse {
        schema: ReleaseRotationResponseSchema::V1,
        action_id,
        outcome,
    })
    .map_err(Into::into)
}

pub(crate) fn handle_release_rotation_retry(
    request: RetryReleaseRotationRequest,
) -> anyhow::Result<serde_json::Value> {
    let action_id = request.action_id.clone();
    let lifecycle = CutexReleaseRotationLifecycle {
        director_runtime_agent_id: None,
    };
    let outcome = match ReleaseRotationProvider::open_default() {
        Ok(provider) => match provider.retry_root(&request, &lifecycle) {
            Ok(receipt) if receipt.status == ReleaseRotationStatus::Complete => {
                ReleaseRotationOutcome::Complete { receipt }
            }
            Ok(receipt) => ReleaseRotationOutcome::Blocked { receipt },
            Err(ReleaseRotationError::Blocked(receipt)) => {
                ReleaseRotationOutcome::Blocked { receipt }
            }
            Err(error) => ReleaseRotationOutcome::NoWrite {
                code: error.code().to_string(),
                reason: error.to_string(),
            },
        },
        Err(_) => ReleaseRotationOutcome::NoWrite {
            code: "persistence_unavailable".to_string(),
            reason: "Release rotation provider is unavailable".to_string(),
        },
    };
    serde_json::to_value(ReleaseRotationResponse {
        schema: ReleaseRotationResponseSchema::V1,
        action_id,
        outcome,
    })
    .map_err(Into::into)
}

struct CutexReleaseRotationLifecycle {
    director_runtime_agent_id: Option<String>,
}

impl ReleaseRotationLifecycle for CutexReleaseRotationLifecycle {
    fn predecessor_has_active_turn(&self, predecessor: &CutexSessionId) -> anyhow::Result<bool> {
        let record = load_record(predecessor)?;
        match app_server_runtime::runtime_manager().status(predecessor.as_str())? {
            Some(status) if status.connected => Ok(status.active_turn_id.is_some()),
            Some(_) | None if record.app_server_runtime.is_none() => Ok(false),
            Some(_) | None => {
                anyhow::bail!("predecessor app-server status is unavailable")
            }
        }
    }

    fn preflight_successor(&self, template: &ReleaseTemplate) -> anyhow::Result<()> {
        load_verified_role_package(template).map(|_| ())
    }

    fn predecessor_thread_id(
        &self,
        predecessor: &CutexSessionId,
    ) -> anyhow::Result<Option<String>> {
        Ok(load_record(predecessor)?.codex_session_id)
    }

    fn offline_predecessor(&self, predecessor: &CutexSessionId) -> anyhow::Result<()> {
        let record = load_record(predecessor)?;
        let Some(entry) = coding_registration_from_cutex_session_record(&record) else {
            if record.app_server_runtime.is_none()
                && record.runtime_pid.is_none()
                && record.current_runtime_agent_id.is_none()
            {
                return Ok(());
            }
            anyhow::bail!("predecessor cannot project exact runtime identity")
        };
        let config = cutex::config::store::load_codez_config();
        let live = management_lifecycle::try_live_agents_for_management_entry(&config, &entry)?;
        let stopped =
            management_lifecycle::stop_cutex_session_runtime_for_entry(&entry, &live, false)?;
        if !stopped.stopped {
            anyhow::bail!("predecessor runtime did not stop")
        }
        Ok(())
    }

    fn retire_predecessor(&self, predecessor: &CutexSessionId) -> anyhow::Result<()> {
        let record = load_record(predecessor)?;
        if record.is_retired() {
            return Ok(());
        }
        management_archive::mutate_management_v2_archive(
            predecessor.as_str(),
            "cutex/session/retire",
            &serde_json::json!({
                "expectedRevision": record.revision,
                "expectedRuntimeGeneration": record.runtime_generation,
            }),
        )
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("typed predecessor retirement failed: {error:?}"))
    }

    fn create_successor_session(
        &self,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<CutexSessionId> {
        let successor = CutexSessionId::new(format!("cutex.{}", uuid::Uuid::new_v4()))
            .map_err(|_| anyhow::anyhow!("failed to allocate durable successor identity"))?;
        let timestamp = Utc::now().to_rfc3339();
        let mut record = CutexSessionRecord::new_at(
            successor.as_str().to_string(),
            None,
            cutex::platform::host::current_host_name(),
            template.cwd.clone(),
            template.profile.clone(),
            timestamp,
        )?;
        record.thread_name = Some(template.successor_name.clone());
        record.display_name_hint = Some(template.successor_name.clone());
        record.managed_cwd = template.managed_cwd.clone();
        record.runtime_backend = template.runtime_backend;
        record.agent_enabled = true;
        record.agent_groups = template.agent_groups.clone();
        record.registration_class = template.registration_class;
        record.exposed_to_backend = template.exposed_to_backend;
        record.quick_action = template.quick_action;
        record.default_cli_args = template.default_cli_args.clone();
        record.permission_defaults = template.permissions.clone();
        record.approval_policy = template.approval_policy.clone();
        record.sandbox_mode = template.sandbox_mode.clone();
        record.model_defaults = template.model.clone();
        record.reasoning_defaults = template.reasoning.clone();
        let mut store = load_cutex_session_store()?;
        if store.sessions.values().any(|existing| {
            existing.is_active()
                && (existing.display_name_hint.as_deref() == Some(template.successor_name.as_str())
                    || existing.thread_name.as_deref() == Some(template.successor_name.as_str()))
        }) {
            anyhow::bail!("configured successor name is already active")
        }
        store
            .sessions
            .insert(successor.as_str().to_string(), record);
        persist_cutex_session_store_and_im_record(&store, successor.as_str())?;
        Ok(successor)
    }

    fn verify_successor_session(
        &self,
        successor: &CutexSessionId,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<bool> {
        let record = load_record(successor)?;
        Ok(record.is_active()
            && record.codex_session_id.is_none()
            && record.app_server_runtime.is_none()
            && record.thread_name.as_deref() == Some(template.successor_name.as_str())
            && record.display_name_hint.as_deref() == Some(template.successor_name.as_str())
            && record.cwd == template.cwd
            && record.managed_cwd == template.managed_cwd
            && record.runtime_backend == template.runtime_backend
            && record.agent_enabled
            && record.agent_groups == template.agent_groups
            && record.profile == template.profile
            && record.model_defaults == template.model
            && record.reasoning_defaults == template.reasoning
            && record.permission_defaults == template.permissions
            && record.approval_policy == template.approval_policy
            && record.sandbox_mode == template.sandbox_mode
            && record.exposed_to_backend == template.exposed_to_backend
            && record.quick_action == template.quick_action
            && record.registration_class == template.registration_class
            && record.default_cli_args == template.default_cli_args)
    }

    fn start_successor_thread(
        &self,
        successor: &CutexSessionId,
        template: &ReleaseTemplate,
    ) -> anyhow::Result<String> {
        let instructions = load_verified_role_package(template)?;
        management_lifecycle::start_cutex_session_new_thread(successor.as_str(), Some(instructions))
            .map(|started| started.thread_id)
    }

    fn verify_successor_thread(
        &self,
        successor: &CutexSessionId,
        thread_id: &str,
    ) -> anyhow::Result<bool> {
        let record = load_record(successor)?;
        Ok(record.is_active()
            && record.codex_session_id.as_deref() == Some(thread_id)
            && record.app_server_runtime.is_some()
            && record.runtime_pid.is_some())
    }

    fn launch_successor_runtime(&self, successor: &CutexSessionId) -> anyhow::Result<()> {
        management_lifecycle::finish_cutex_session_new_thread_online(successor.as_str())
    }

    fn deliver_director_message(
        &self,
        director: &CutexSessionId,
        successor: &CutexSessionId,
        action_id: &ActionId,
        exact_message: &str,
    ) -> anyhow::Result<String> {
        let successor_record = load_record(successor)?;
        let target_thread = successor_record
            .codex_session_id
            .context("Release successor has no native thread")?;
        let target_runtime_agent_id = successor_record
            .current_runtime_agent_id
            .context("Release successor has no current runtime identity")?;
        let director_record = load_record(director)?;
        let current_director_runtime_agent_id = director_record
            .current_runtime_agent_id
            .context("current Director runtime identity is unavailable")?;
        let director_runtime_agent_id = match self.director_runtime_agent_id.as_deref() {
            Some(runtime_agent_id) if runtime_agent_id != current_director_runtime_agent_id => {
                anyhow::bail!("requesting Director runtime identity is stale")
            }
            Some(runtime_agent_id) => runtime_agent_id.to_string(),
            None => current_director_runtime_agent_id,
        };
        let director_thread = director_record
            .codex_session_id
            .context("current Director has no native thread identity")?;
        let payload = director_starting_message_request(
            director_runtime_agent_id.clone(),
            director,
            successor,
            target_runtime_agent_id.clone(),
            action_id,
            exact_message,
        );
        let config = cutex::config::store::load_codez_config();
        let body = serde_json::to_vec(&payload)?;
        let response = cutex::agent_bus::client::agent_bus_http_json(
            &cutex::agent_bus::service::agent_bus_base_url(
                cutex::agent_bus::service::agent_bus_port(&config),
            ),
            "POST",
            "/api/messages/send",
            config.agent_bus_token.as_deref(),
            Some(&body),
        )?;
        let response: AgentBusSendResponse = serde_json::from_value(response)
            .context("Agent Bus delivery returned an invalid receipt")?;
        verified_director_message_id(
            &response,
            &director_runtime_agent_id,
            &target_runtime_agent_id,
            director,
            successor,
            &director_thread,
            &target_thread,
            action_id,
        )
    }
}

fn director_starting_message_request(
    director_runtime_agent_id: String,
    director: &CutexSessionId,
    successor: &CutexSessionId,
    target_runtime_agent_id: String,
    action_id: &ActionId,
    exact_message: &str,
) -> AgentBusSendRequest {
    AgentBusSendRequest {
        to: target_runtime_agent_id,
        all_groups: true,
        all_hosts: false,
        kind: AgentBusEnvelopeKind::Message,
        from: None,
        from_agent_id: Some(director_runtime_agent_id),
        from_session_id: Some(director.as_str().to_string()),
        to_session_id: Some(successor.as_str().to_string()),
        content: exact_message.to_string(),
        delivery_mode: Some(AgentDeliveryMode::AfterTurn),
        queue_only: None,
        trigger_turn: None,
        sender_kind: Some(AgentMessageKind::Agent),
        display_source: None,
        submit_mode: None,
        control_type: None,
        control_payload: None,
        external_action_id: Some(action_id.as_str().to_string()),
        external_message_id: Some(format!("release-rotation:{}:start", action_id.as_str())),
    }
}

#[allow(clippy::too_many_arguments)]
fn verified_director_message_id(
    response: &AgentBusSendResponse,
    director_runtime_agent_id: &str,
    successor_runtime_agent_id: &str,
    director: &CutexSessionId,
    successor: &CutexSessionId,
    director_thread: &str,
    successor_thread: &str,
    action_id: &ActionId,
) -> anyhow::Result<String> {
    let expected_external_message_id = format!("release-rotation:{}:start", action_id.as_str());
    if response.id.trim().is_empty()
        || response.to != successor_runtime_agent_id
        || response.from_runtime_agent_id.as_deref() != Some(director_runtime_agent_id)
        || response.to_runtime_agent_id.as_deref() != Some(successor_runtime_agent_id)
        || response.from_cutex_session_id.as_deref() != Some(director.as_str())
        || response.to_cutex_session_id.as_deref() != Some(successor.as_str())
        || response.from_session_id.as_deref() != Some(director_thread)
        || response.to_session_id.as_deref() != Some(successor_thread)
        || response.delivery_mode != Some(AgentDeliveryMode::AfterTurn)
        || !response.queued
        || response.external_action_id.as_deref() != Some(action_id.as_str())
        || response.external_message_id.as_deref() != Some(expected_external_message_id.as_str())
    {
        anyhow::bail!("Agent Bus delivery resolved a different runtime or session identity")
    }
    Ok(response.id.clone())
}

fn load_record(cutex_session_id: &CutexSessionId) -> anyhow::Result<CutexSessionRecord> {
    let store = load_cutex_session_store()?;
    let key = cutex_session_key_for_user_id_including_retired(&store, cutex_session_id.as_str())
        .context("durable Cutex session does not exist")?;
    store
        .sessions
        .get(&key)
        .cloned()
        .context("durable Cutex session disappeared")
}

fn load_verified_role_package(template: &ReleaseTemplate) -> anyhow::Result<String> {
    if !Path::new(&template.cwd).is_absolute() {
        anyhow::bail!("configured successor cwd must be absolute")
    }
    let reference = PathBuf::from(&template.role_package.reference);
    if !is_confined_role_package_reference(&reference) {
        anyhow::bail!("configured role package reference must stay beneath successor cwd")
    }
    let canonical_cwd = fs::canonicalize(&template.cwd)
        .context("failed to canonicalize configured successor cwd")?;
    if !canonical_cwd.is_dir() {
        anyhow::bail!("configured successor cwd is not a directory")
    }
    let path = fs::canonicalize(canonical_cwd.join(reference))
        .context("failed to canonicalize configured role package")?;
    if !path.starts_with(&canonical_cwd) {
        anyhow::bail!("configured role package escapes successor cwd")
    }
    let metadata = fs::metadata(&path).with_context(|| {
        format!(
            "failed to inspect configured role package: {}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        anyhow::bail!("configured role package is not a file")
    }
    if metadata.len() > RELEASE_ROLE_PACKAGE_MAX_BYTES {
        anyhow::bail!("configured role package exceeds one MiB")
    }
    let file = fs::File::open(&path)
        .with_context(|| format!("failed to open configured role package: {}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(RELEASE_ROLE_PACKAGE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read configured role package: {}", path.display()))?;
    if bytes.len() as u64 > RELEASE_ROLE_PACKAGE_MAX_BYTES {
        anyhow::bail!("configured role package exceeds one MiB")
    }
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != template.role_package.sha256.as_str() {
        anyhow::bail!("configured role package hash mismatch")
    }
    String::from_utf8(bytes).context("configured role package is not UTF-8")
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

#[cfg(test)]
mod tests {
    use super::*;
    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::role_revision::Sha256 as TypedSha256;
    use cutex::rotation::{ReleaseRolePackage, ReleaseTemplateSchema};
    use cutex::session::model::{CutexSessionQuickActionMode, CutexSessionRuntimeBackend};

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("cutex-release-adapter-{}", uuid::Uuid::new_v4()))
    }

    fn template(cwd: &Path, reference: &str, bytes: &[u8]) -> ReleaseTemplate {
        ReleaseTemplate {
            schema: ReleaseTemplateSchema::V1,
            version: 1,
            successor_name: "cutex-release-r7".to_string(),
            cwd: cwd.display().to_string(),
            managed_cwd: Some(cwd.display().to_string()),
            runtime_backend: CutexSessionRuntimeBackend::Host,
            role_package: ReleaseRolePackage {
                reference: reference.to_string(),
                sha256: TypedSha256::new(format!("{:x}", Sha256::digest(bytes))).expect("hash"),
            },
            agent_groups: vec!["cutex".to_string()],
            profile: None,
            model: None,
            reasoning: None,
            permissions: None,
            approval_policy: None,
            sandbox_mode: None,
            exposed_to_backend: false,
            quick_action: CutexSessionQuickActionMode::Auto,
            registration_class: AgentRegistrationClass::Persistent,
            default_cli_args: Vec::new(),
        }
    }

    #[test]
    fn production_role_package_loader_confines_canonical_successor_cwd() {
        let root = root();
        let cwd = root.join("cwd");
        let roles = cwd.join("roles");
        fs::create_dir_all(&roles).expect("roles");
        let bytes = b"verified release role";
        fs::write(roles.join("release.md"), bytes).expect("role package");
        let valid = template(&cwd, "roles/release.md", bytes);
        assert_eq!(
            load_verified_role_package(&valid).expect("confined package"),
            "verified release role"
        );
        let mut wrong_hash = valid.clone();
        wrong_hash.role_package.sha256 = TypedSha256::new("0".repeat(64)).expect("wrong hash");
        assert!(load_verified_role_package(&wrong_hash).is_err());

        let relative_cwd = template(Path::new("relative-cwd"), "roles/release.md", bytes);
        assert!(load_verified_role_package(&relative_cwd).is_err());
        let missing_cwd = template(&root.join("missing-cwd"), "roles/release.md", bytes);
        assert!(load_verified_role_package(&missing_cwd).is_err());
        let missing_package = template(&cwd, "roles/missing.md", bytes);
        assert!(load_verified_role_package(&missing_package).is_err());

        let oversized = vec![b'x'; 1024 * 1024 + 1];
        fs::write(roles.join("oversized.md"), &oversized).expect("oversized package");
        assert!(
            load_verified_role_package(&template(&cwd, "roles/oversized.md", &oversized,)).is_err()
        );

        for reference in ["../outside.md", "/outside.md"] {
            let invalid = template(&cwd, reference, bytes);
            assert!(load_verified_role_package(&invalid).is_err());
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn production_role_package_loader_rejects_symlink_escape_before_read() {
        use std::os::unix::fs::symlink;

        let root = root();
        let cwd = root.join("cwd");
        let roles = cwd.join("roles");
        fs::create_dir_all(&roles).expect("roles");
        let outside = root.join("outside.md");
        let bytes = b"outside release role";
        fs::write(&outside, bytes).expect("outside package");
        symlink(&outside, roles.join("release.md")).expect("escaping symlink");
        let invalid = template(&cwd, "roles/release.md", bytes);
        assert!(load_verified_role_package(&invalid).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn production_starting_message_is_exact_after_turn_request() {
        let director = CutexSessionId::new("cutex.director").expect("director");
        let successor = CutexSessionId::new("cutex.release-new").expect("successor");
        let action = ActionId::new("rotate-release-195").expect("action");
        let request = director_starting_message_request(
            "cutex.director.runtime".to_string(),
            &director,
            &successor,
            "cutex.release.runtime".to_string(),
            &action,
            "Review the frozen candidate.",
        );
        assert_eq!(request.to, "cutex.release.runtime");
        assert_eq!(
            request.from_agent_id.as_deref(),
            Some("cutex.director.runtime")
        );
        assert_eq!(request.from_session_id.as_deref(), Some("cutex.director"));
        assert_eq!(request.to_session_id.as_deref(), Some("cutex.release-new"));
        assert_eq!(request.delivery_mode, Some(AgentDeliveryMode::AfterTurn));
        assert_eq!(request.content, "Review the frozen candidate.");
        assert_eq!(
            request.external_action_id.as_deref(),
            Some("rotate-release-195")
        );
        assert_eq!(
            request.external_message_id.as_deref(),
            Some("release-rotation:rotate-release-195:start")
        );
        assert!(request.all_groups);
        assert_eq!(request.queue_only, None);
        assert_eq!(request.trigger_turn, None);
    }

    fn exact_message_response(
        director_runtime: &str,
        successor_runtime: &str,
        director: &CutexSessionId,
        successor: &CutexSessionId,
        action: &ActionId,
    ) -> AgentBusSendResponse {
        AgentBusSendResponse {
            id: "message-1".to_string(),
            from: Some("director".to_string()),
            to: successor_runtime.to_string(),
            to_name: Some("cutex-release".to_string()),
            from_session_id: Some("thread-director".to_string()),
            to_session_id: Some("thread-successor".to_string()),
            from_runtime_agent_id: Some(director_runtime.to_string()),
            to_runtime_agent_id: Some(successor_runtime.to_string()),
            from_cutex_session_id: Some(director.as_str().to_string()),
            to_cutex_session_id: Some(successor.as_str().to_string()),
            delivery_mode: Some(AgentDeliveryMode::AfterTurn),
            trigger_turn: false,
            queued: true,
            queue_durability: Some("durable_v2".to_string()),
            delivery_state: Some("pending".to_string()),
            required_ack_level: Some("A4".to_string()),
            deduplicated: false,
            external_action_id: Some(action.as_str().to_string()),
            external_message_id: Some(format!("release-rotation:{}:start", action.as_str())),
        }
    }

    #[test]
    fn production_delivery_receipt_accepts_only_exact_created_identity() {
        let director = CutexSessionId::new("cutex.director").expect("director");
        let successor = CutexSessionId::new("cutex.release-new").expect("successor");
        let action = ActionId::new("rotate-release-195").expect("action");
        let exact = exact_message_response(
            "runtime-director",
            "runtime-successor",
            &director,
            &successor,
            &action,
        );
        assert_eq!(
            verified_director_message_id(
                &exact,
                "runtime-director",
                "runtime-successor",
                &director,
                &successor,
                "thread-director",
                "thread-successor",
                &action,
            )
            .expect("exact receipt"),
            "message-1"
        );

        let mut stale_runtime = exact;
        stale_runtime.to_runtime_agent_id = Some("runtime-collision".to_string());
        assert!(verified_director_message_id(
            &stale_runtime,
            "runtime-director",
            "runtime-successor",
            &director,
            &successor,
            "thread-director",
            "thread-successor",
            &action,
        )
        .is_err());
    }
}
