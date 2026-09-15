//! Root-Human adoption of an already saved native identity. Never bootstraps.
use super::*;
use crate::management::control_plane::HumanManagementPrincipal;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanCreationDefaults {
    pub profile: String,
    pub model: String,
    pub reasoning: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAdoptRequest {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub session_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_defaults: Option<HumanCreationDefaults>,
    pub action_id: AgentActionId,
    pub native_id: String,
    pub cwd: String,
    pub formal_name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanAdoptReceipt {
    pub request: HumanAdoptRequest,
    pub record: crate::session::model::CutexSessionRecord,
    pub import_request: Option<DurableImportRequest>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanAdoptResult {
    pub adopted: HumanAdoptReceipt,
    pub imported: Option<DurableImportReceipt>,
    pub error: Option<String>,
}

impl AgentManagementProvider {
    /// Caller has verified the exact saved native reference at its source.
    /// Durable adoption is atomic with its receipt; import is a recoverable
    /// existing Human operation, explicitly without Project assignment.
    pub fn adopt_saved_native(
        &self,
        principal: &HumanManagementPrincipal,
        path: &Path,
        request: &HumanAdoptRequest,
        host: &str,
        tasks: &dyn ProjectTaskInspector,
    ) -> anyhow::Result<HumanAdoptResult> {
        anyhow::ensure!(
            !request.formal_name.trim().is_empty()
                && request.formal_name.trim() == request.formal_name
                && request.formal_name.chars().count() <= 128
                && !request.formal_name.chars().any(char::is_control),
            "explicit valid formal Agent name required"
        );
        anyhow::ensure!(
            crate::session::identity::normalize_codex_session_id(&request.native_id)
                .ok()
                .as_deref()
                == Some(request.native_id.as_str()),
            "exact native ID required"
        );
        anyhow::ensure!(
            Path::new(&request.cwd).is_absolute(),
            "absolute native cwd required"
        );
        let import_action = AgentActionId::new(format!("{}-import", request.action_id))?;
        let mut adopted = {
            let _mutation = self.store().lock_mutations()?;
            let roster = self.store().snapshot()?;
            crate::session::store::with_locked_session_store(path, |sessions| {
                if let Some(receipt) = sessions
                    .human_adoption_receipts
                    .get(request.action_id.as_str())
                {
                    anyhow::ensure!(
                        &receipt.request == request,
                        "adoption_action_payload_conflict"
                    );
                    return Ok(receipt.clone());
                }
                anyhow::ensure!(!sessions.sessions.values().any(|r| r.codex_session_id.as_deref() == Some(request.native_id.as_str()) && (r.registration_class == crate::agent_bus::model::AgentRegistrationClass::Persistent || r.is_retired())), "native identity already has a managed record; use existing Agent/import/Restore");
                let seed = crate::session::service::CutexSessionEnsureSeed {
                    host_id: host.into(), cwd: request.cwd.clone(), profile: None,
                };
                let key = if request.session_only {
                    crate::session::service::ensure_cutex_session_record_for_user_id(
                        sessions, &request.native_id, seed,
                    )?
                } else {
                    crate::session::service::adopt_cutex_session(
                        sessions, &request.native_id, seed,
                        crate::session::service::CutexSessionAdoptOptions {
                            display_name: Some(request.formal_name.as_str()),
                            managed_cwd: None, groups: Vec::new(), expose_to_im: false, pin: false,
                        },
                    )?.key
                };
                anyhow::ensure!(
                    !roster.agents.keys().any(|id| id.as_str() == key),
                    "existing roster identity cannot be readopted"
                );
                let record = sessions
                    .sessions
                    .get_mut(&key)
                    .expect("adoption created exact record");
                record.formal_agent_name = (!request.session_only).then(|| request.formal_name.clone());
                if request.session_only {
                    anyhow::ensure!(record.explicit_launch.is_none(), "session runtime already registered; resume existing session");
                    record.display_name_hint = Some(request.formal_name.clone());
                    record.agent_enabled = false;
                    record.registration_class = crate::agent_bus::model::AgentRegistrationClass::LocalOnly;
                }
                if let Some(defaults) = &request.creation_defaults {
                    anyhow::ensure!(!defaults.profile.trim().is_empty() && !defaults.model.trim().is_empty(), "Creation profile and model required");
                    record.profile = Some(defaults.profile.clone());
                    record.model_defaults = Some(defaults.model.clone());
                    record.reasoning_defaults = defaults.reasoning.clone();
                }
                let mut candidate=record.clone();
                if request.session_only {
                    crate::launch::local_deployment::LocalDeployment::selected()?
                        .ok_or_else(|| anyhow::anyhow!("Local runtime deployment required"))?
                        .adopt(&mut candidate, sessions)?;
                    candidate.bump_durable_revision()?;
                    sessions.sessions.insert(candidate.cutex_session_id.clone(), candidate.clone());
                }
                let receipt = HumanAdoptReceipt {
                    request: request.clone(),
                    record: candidate,
                    import_request: None,
                };
                sessions
                    .human_adoption_receipts
                    .insert(request.action_id.to_string(), receipt.clone());
                crate::session::store::save_locked_session_store(path, sessions)?;
                Ok(receipt)
            })?
        };
        if request.session_only {
            return Ok(HumanAdoptResult{adopted, imported:None, error:None});
        }
        let outcome = (|| -> anyhow::Result<DurableImportReceipt> {
            if adopted.import_request.is_none() {
                let candidate = self
                    .durable_agent_candidates(principal, path)?
                    .into_iter()
                    .find(|c| {
                        c.cutex_session_id
                            .as_ref()
                            .is_some_and(|id| id.as_str() == adopted.record.cutex_session_id)
                    })
                    .ok_or_else(|| anyhow::anyhow!("adopted candidate unavailable"))?;
                anyhow::ensure!(
                    candidate.durable_sha256
                        == super::durable_import::durable_candidate_digest(&adopted.record)?,
                    "adopted record changed; no refreshed CAS substitution"
                );
                let import = DurableImportRequest {
                    action_id: import_action,
                    candidate,
                    confirmed_formal_name: request.formal_name.clone(),
                    assignment: None,
                    detach: None,
                };
                adopted = crate::session::store::with_locked_session_store(path, |sessions| {
                    let receipt = sessions
                        .human_adoption_receipts
                        .get_mut(request.action_id.as_str())
                        .ok_or_else(|| anyhow::anyhow!("adoption receipt missing"))?;
                    anyhow::ensure!(
                        receipt.request == *request,
                        "adoption_action_payload_conflict"
                    );
                    if receipt.import_request.is_none() {
                        receipt.import_request = Some(import);
                    }
                    let receipt = receipt.clone();
                    crate::session::store::save_locked_session_store(path, sessions)?;
                    Ok(receipt)
                })?;
            }
            Ok(self.import_durable_agent(
                principal,
                path,
                adopted.import_request.as_ref().unwrap(),
                tasks,
            )?)
        })();
        match outcome {
            Ok(imported) => Ok(HumanAdoptResult { error:imported.error.clone(), adopted, imported:Some(imported) }),
            Err(error) => Ok(HumanAdoptResult { adopted, imported:None, error:Some(format!("Durable adoption committed; import incomplete: {error:#}. Retry the same action; no rollback claimed")) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ui_contract_d10_adoption_partial_import_keeps_original_fence() {
        let root = std::env::temp_dir().join(format!("cutex-d2-partial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("sessions.json");
        let provider = AgentManagementProvider::open(root.join("provider"))
            .unwrap()
            .with_current_names_path(path.clone());
        let principal = HumanManagementPrincipal::authenticated();
        let request = HumanAdoptRequest {
            session_only: false,
            creation_defaults: Some(HumanCreationDefaults { profile: "selected-profile".into(), model: "selected-model".into(), reasoning: Some("high".into()) }),
            action_id: AgentActionId::new("partial-adopt").unwrap(),
            native_id: "native-partial".into(),
            cwd: root.to_string_lossy().into_owned(),
            formal_name: "Explicit Name".into(),
        };
        let no_tasks = |_: &ProjectId, _: Option<&crate::role_revision::CutexSessionId>| Ok(false);
        // A foreign host is unsupported by the existing import contract.
        let first = provider
            .adopt_saved_native(
                &principal,
                &path,
                &request,
                "unsupported-remote-fixture",
                &no_tasks,
            )
            .unwrap();
        assert_eq!(first.adopted.record.profile.as_deref(), Some("selected-profile"));
        assert_eq!(first.adopted.record.model_defaults.as_deref(), Some("selected-model"));
        assert_eq!(first.adopted.record.reasoning_defaults.as_deref(), Some("high"));
        assert!(first.error.is_some());
        assert!(first.imported.is_none());
        assert!(first.adopted.import_request.is_some());
        assert_eq!(
            provider
                .adopt_saved_native(
                    &principal,
                    &path,
                    &request,
                    "unsupported-remote-fixture",
                    &no_tasks
                )
                .unwrap(),
            first
        );
        crate::session::store::with_locked_session_store(&path, |store| {
            store
                .sessions
                .get_mut(&first.adopted.record.cutex_session_id)
                .unwrap()
                .host_id = crate::platform::host::current_host_name();
            crate::session::store::save_locked_session_store(&path, store)
        })
        .unwrap();
        let retry = provider
            .adopt_saved_native(
                &principal,
                &path,
                &request,
                "unsupported-remote-fixture",
                &no_tasks,
            )
            .unwrap();
        assert!(retry.error.is_some());
        assert_eq!(
            retry.adopted, first.adopted,
            "original confirmation is not silently refreshed"
        );
        assert!(provider.store().snapshot().unwrap().agents.is_empty());
        assert_eq!(
            crate::session::store::load_cutex_session_store_from_path(&path)
                .unwrap()
                .sessions
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ui_contract_d08_d09_d10_adoption_exact_replay_name_and_single_identity() {
        const CHILD: &str = "CUTEX_ADOPTION_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let home = std::env::temp_dir().join(format!("cutex-adopt-home-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&home).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "agent_management::durable_adoption::tests::ui_contract_d08_d09_d10_adoption_exact_replay_name_and_single_identity", "--nocapture"])
                .env("HOME", &home).env(CHILD, "1").status().unwrap();
            std::fs::remove_dir_all(home).unwrap();
            assert!(status.success(), "isolated adoption test failed");
            return;
        }
        let root = std::env::temp_dir().join(format!("cutex-d2-adopt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("sessions.json");
        let provider = AgentManagementProvider::open(root.join("provider"))
            .unwrap()
            .with_current_names_path(path.clone());
        let principal = HumanManagementPrincipal::authenticated();
        let request = HumanAdoptRequest {
            session_only: false,
            creation_defaults: None,
            action_id: AgentActionId::new("adopt-test").unwrap(),
            native_id: "019e0995-cc8d-7f81-83cc-a4f09e8b4901".into(),
            cwd: root.to_string_lossy().into_owned(),
            formal_name: "Explicit Formal Name".into(),
        };
        let no_tasks = |_: &ProjectId, _: Option<&crate::role_revision::CutexSessionId>| Ok(false);
        let host = crate::platform::host::current_host_name();
        // Recent/settings may already have materialized an unmanaged record.
        // Adopt must keep its durable key and import that one identity.
        let original_key = crate::session::store::with_locked_session_store(&path, |store| {
            let key = crate::session::service::ensure_cutex_session_record_for_user_id(store, &request.native_id,
                crate::session::service::CutexSessionEnsureSeed { host_id: host.clone(), cwd: request.cwd.clone(), profile: None })?;
            crate::session::store::save_locked_session_store(&path, store)?;
            Ok(key)
        }).unwrap();
        let result = provider
            .adopt_saved_native(&principal, &path, &request, &host, &no_tasks)
            .unwrap();
        assert!(
            result.imported.as_ref().is_some_and(|r| r.complete),
            "{result:?}"
        );
        assert_eq!(result.adopted.record.cutex_session_id, original_key);
        assert_eq!(result.adopted.record.profile, None);
        assert_eq!(
            result.adopted.record.formal_agent_name.as_deref(),
            Some("Explicit Formal Name")
        );
        assert_eq!(result.adopted.record.thread_name, None);
        assert_eq!(
            provider
                .adopt_saved_native(&principal, &path, &request, &host, &no_tasks)
                .unwrap(),
            result
        );
        let mut changed = request.clone();
        changed.formal_name = "Changed".into();
        assert!(provider
            .adopt_saved_native(&principal, &path, &changed, &host, &no_tasks)
            .is_err());
        changed = request.clone();
        changed.action_id = AgentActionId::new("duplicate-native").unwrap();
        assert!(provider
            .adopt_saved_native(&principal, &path, &changed, &host, &no_tasks)
            .is_err());
        let store = crate::session::store::load_cutex_session_store_from_path(&path).unwrap();
        assert_eq!(store.sessions.len(), 1);
        let roster = provider.store().snapshot().unwrap();
        assert_eq!(roster.agents.len(), 1);
        assert_eq!(roster.current_project_memberships.len(), 1);
        assert!(roster
            .current_project_memberships
            .values()
            .all(|m| m.project_id.is_none()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
