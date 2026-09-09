//! One-action Human bundle selection, separate from Director create authority.
//! This is not a bearer credential and never grants a project role.
use super::*;
use crate::launch::stock::{StockBundle, StockConfiguration};
use crate::role_revision::{CutexSessionId, Sha256};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(feature = "stock-launch-test-hook")]
pub fn bootstrap_test_fault(variable: &str, action: &AgentActionId) -> bool {
    let Ok(private) = std::env::var("CUTEX_TEST_PRIVATE_HOME") else {
        return false;
    };
    let path = std::path::Path::new(&private);
    path.is_absolute()
        && std::env::var("HOME").as_deref() == Ok(private.as_str())
        && path.join(".cutex-test-private-home").is_file()
        && std::env::var(variable).as_deref() == Ok(action.as_str())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapAdoptionReceipt {
    pub intent_sha256: Sha256,
    pub native_id: String,
    pub cutex_session_id: CutexSessionId,
    pub revision: u64,
}

/// Only the authorized provider create path can construct this borrowed permit.
/// No deserializer, public constructor, root credential or model authority.
pub struct BootstrapExecutionPermit<'a> {
    pub(super) provider: &'a AgentManagementProvider,
    pub(super) intent: &'a BootstrapIntentReview,
}

impl BootstrapExecutionPermit<'_> {
    pub fn review(&self) -> &BootstrapIntentReview {
        self.intent
    }

    pub fn adopt(&self, path: &std::path::Path, native: &str) -> anyhow::Result<CutexSessionId> {
        use crate::session::service::*;
        use crate::session::store::*;
        self.intent.validate_evidence()?;
        self.intent
            .validate_authority(&self.provider.store().snapshot()?)?;
        let spec = self.intent.spec()?;
        let digest = super::store::request_sha256(self.intent)?;
        let key = format!("bootstrap-adopt:{}", digest.as_str());
        let contract = ExplicitLaunchContract {
            version: 2,
            native_id: native.to_owned(),
            native_home: self.intent.native_home.clone(),
            bundle_manifest: self.intent.bundle_manifest.clone(),
            bundle_sha256: self.intent.bundle_sha256.clone(),
        };
        contract.validate()?;
        with_locked_session_store(path, |sessions| {
            if let Some(prior) = sessions.explicit_launch_receipts.get(&key) {
                let ExplicitLaunchActionReceipt::Bootstrap(prior) = prior else {
                    anyhow::bail!("bootstrap adoption action conflict")
                };
                anyhow::ensure!(
                    prior.intent_sha256 == digest && prior.native_id == native,
                    "bootstrap adoption replay conflict"
                );
                let record = sessions
                    .sessions
                    .get(prior.cutex_session_id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("bootstrap adopted identity missing"))?;
                anyhow::ensure!(
                    record.codex_session_id.as_deref() == Some(native)
                        && record.explicit_launch.as_ref() == Some(&contract)
                        && !record.is_retired()
                        && record.agent_enabled
                        && record.formal_agent_name.as_ref() == Some(&spec.name)
                        && record.profile == spec.profile,
                    "bootstrap adopted binding changed"
                );
                return Ok(prior.cutex_session_id.clone());
            }
            anyhow::ensure!(
                !sessions
                    .sessions
                    .values()
                    .any(|s| s.codex_session_id.as_deref() == Some(native)),
                "bootstrap native identity already mapped without same-action receipt"
            );
            let adopted = adopt_cutex_session(
                sessions,
                native,
                CutexSessionEnsureSeed {
                    host_id: crate::platform::host::current_host_name(),
                    cwd: spec.cwd.clone(),
                    profile: spec.profile.clone(),
                },
                CutexSessionAdoptOptions {
                    display_name: Some(&spec.name),
                    managed_cwd: Some(spec.cwd.clone()),
                    groups: spec.groups.clone(),
                    expose_to_im: spec.expose_to_im,
                    pin: spec.pin,
                },
            )?;
            let record = sessions
                .sessions
                .get_mut(&adopted.key)
                .ok_or_else(|| anyhow::anyhow!("bootstrap adoption missing"))?;
            record.formal_agent_name = Some(spec.name.clone());
            // Existing Bus routing metadata, explicitly included in review;
            // this is not Cutex Project membership or an identity inference.
            record.agent_groups = self.intent.runtime_groups.clone();
            record.profile = spec.profile.clone();
            record.runtime_backend = crate::session::model::CutexSessionRuntimeBackend::Host;
            record.permission_defaults = Some(spec.permissions.clone());
            record.approval_policy = Some(spec.approval_policy.clone());
            record.sandbox_mode = Some(spec.sandbox_mode.clone());
            record.model_defaults = Some(spec.model.clone());
            record.reasoning_defaults = Some(spec.reasoning.clone());
            record.explicit_launch = Some(contract.clone());
            let id = CutexSessionId::new(adopted.key)
                .map_err(|_| anyhow::anyhow!("invalid adopted durable ID"))?;
            let receipt = BootstrapAdoptionReceipt {
                intent_sha256: digest,
                native_id: native.to_owned(),
                cutex_session_id: id.clone(),
                revision: record.revision,
            };
            crate::launch::stock::validate_native(
                &sessions.sessions[id.as_str()],
                sessions,
                &contract,
            )?;
            sessions
                .explicit_launch_receipts
                .insert(key, ExplicitLaunchActionReceipt::Bootstrap(receipt));
            save_locked_session_store(path, sessions)?;
            Ok(id)
        })
    }

    pub fn online(
        &self,
        path: &std::path::Path,
        id: &CutexSessionId,
        tasks: &crate::task_service::TaskServiceProvider,
        runtime: &mut dyn StockRuntimeExecutor,
    ) -> anyhow::Result<StockRuntimeReceipt> {
        self.intent.validate_evidence()?;
        self.intent
            .validate_authority(&self.provider.store().snapshot()?)?;
        let action = AgentActionId::new(format!(
            "bootstrap-runtime:{}",
            super::store::request_sha256(self.intent)?.as_str()
        ))?;
        let sessions = crate::session::store::load_cutex_session_store_from_path(path)?;
        let intent_digest = super::store::request_sha256(self.intent)?;
        let Some(ExplicitLaunchActionReceipt::Bootstrap(adopted)) = sessions
            .explicit_launch_receipts
            .get(&format!("bootstrap-adopt:{}", intent_digest.as_str()))
        else {
            anyhow::bail!("bootstrap adoption receipt missing; no launch permitted")
        };
        let record = sessions
            .sessions
            .get(id.as_str())
            .ok_or_else(|| anyhow::anyhow!("bootstrap adopted record missing"))?;
        anyhow::ensure!(
            adopted.intent_sha256 == intent_digest
                && &adopted.cutex_session_id == id
                && record.codex_session_id.as_deref() == Some(adopted.native_id.as_str())
                && record
                    .explicit_launch
                    .as_ref()
                    .is_some_and(|contract| contract.native_id == adopted.native_id
                        && contract.version == 2
                        && contract.native_home == self.intent.native_home
                        && contract.bundle_manifest == self.intent.bundle_manifest
                        && contract.bundle_sha256 == self.intent.bundle_sha256),
            "bootstrap adoption/intent binding changed; no launch permitted"
        );
        let review = match sessions.explicit_launch_receipts.get(action.as_str()) {
            Some(ExplicitLaunchActionReceipt::Runtime(prior)) => {
                anyhow::ensure!(
                    &prior.review.subject.cutex_session_id == id
                        && prior.review.configuration == self.intent.configuration,
                    "bootstrap runtime receipt conflict"
                );
                prior.review.clone()
            }
            Some(_) => anyhow::bail!("bootstrap runtime action conflict"),
            None => self
                .provider
                .review_stock_runtime_locked(path, id, false, tasks)?,
        };
        anyhow::ensure!(
            review.configuration == self.intent.configuration,
            "bootstrap runtime configuration changed"
        );
        let reconnect = matches!(
            sessions.explicit_launch_receipts.get(action.as_str()),
            Some(ExplicitLaunchActionReceipt::Runtime(prior))
                if prior.stage == StockRuntimeStage::Ready
        );
        let receipt = self
            .provider
            .execute_stock_runtime_locked(path, &action, &review, tasks, runtime)?;
        if reconnect {
            // Receipt replay is immutable, but a new Cutex process must attach
            // its connection to the SAME still-current owner. No spawn/stop or
            // new generation is permitted by this recovery step.
            let fence = || -> anyhow::Result<crate::session::model::CutexSessionRecord> {
                let current = crate::session::store::load_cutex_session_store_from_path(path)?
                    .sessions
                    .remove(id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("captured successor missing"))?;
                anyhow::ensure!(
                    !current.is_retired()
                        && current.agent_enabled
                        && current.revision == review.subject.revision
                        && current.app_server_launch_claim_id.is_none()
                        && current.app_server_runtime == receipt.binding
                        && current.runtime_pid == receipt.binding.as_ref().map(|b| b.pid)
                        && current.runtime_generation == receipt.expected_generation
                        && current.current_runtime_agent_id.as_deref()
                            == Some(&receipt.runtime_agent_id)
                        && current.explicit_launch.as_ref() == Some(&review.contract)
                        && crate::launch::stock::current_configuration(&current)?
                            == review.configuration,
                    "captured successor occurrence/configuration changed; no reconnect"
                );
                Ok(current)
            };
            runtime.connect(&fence()?, &receipt)?;
            runtime.retain_owner();
            fence()?;
        }
        Ok(receipt)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapIntentReview {
    pub version: u32,
    pub request: AgentManagementRequest,
    pub director: CutexSessionId,
    pub authority_sha256: Sha256,
    pub store_revision: u64,
    pub native_home: PathBuf,
    pub bundle_manifest: PathBuf,
    pub bundle_sha256: Sha256,
    pub configuration: StockConfiguration,
    pub runtime_groups: Vec<String>,
    pub expires_at_unix: i64,
}

impl BootstrapIntentReview {
    pub(crate) fn spec(&self) -> anyhow::Result<&ManagedAgentSpec> {
        anyhow::ensure!(
            matches!(self.version, 1 | 2),
            "unsupported bootstrap intent version"
        );
        self.request.validate()?;
        if self.version == 1 {
            anyhow::ensure!(
                matches!(
                    &self.request.operation,
                    AgentOperation::Create {
                        start_mode: AgentStartMode::BootstrapOnly,
                        frozen_message: None,
                        ..
                    }
                ),
                "version1 supports neutral Create only"
            );
        }
        let spec = self.request.operation.bootstrap_spec().ok_or_else(|| {
            anyhow::anyhow!("reviewed bootstrap requires create/replace/director_rotate")
        })?;
        let intent = self
            .request
            .operation
            .bootstrap_intent()
            .ok_or_else(|| anyhow::anyhow!("reviewed bootstrap intent reference missing"))?;
        anyhow::ensure!(
            intent == &self.request.action_id,
            "bootstrap requires exact intent reference"
        );
        anyhow::ensure!(
            self.request.project_id.is_some(),
            "bootstrap requires exact project"
        );
        Ok(spec)
    }

    pub(crate) fn validate_evidence(&self) -> anyhow::Result<()> {
        let spec = self.spec()?;
        anyhow::ensure!(
            self.runtime_groups
                == crate::agent_bus::groups::normalize_registered_agent_groups(
                    spec.groups.clone(),
                    None,
                    &spec.cwd
                ),
            "bootstrap reviewed runtime groups changed"
        );
        anyhow::ensure!(
            crate::config::paths::host_codex_home_dir()?.canonicalize()? == self.native_home,
            "bootstrap home is not authoritative native home"
        );
        anyhow::ensure!(
            !self.native_home.join("auth.json").try_exists()?,
            "private bootstrap does not consume native auth"
        );
        let bundle = StockBundle::load_references(
            &self.native_home,
            &self.bundle_manifest,
            &self.bundle_sha256,
        )?;
        anyhow::ensure!(
            bundle.version == 3 && bundle.soon_ingress(),
            "bootstrap requires the exact reviewed coherent bundle3"
        );
        anyhow::ensure!(
            crate::launch::stock::bootstrap_configuration(spec)? == self.configuration,
            "bootstrap reviewed configuration changed"
        );
        Ok(())
    }

    pub(crate) fn validate_authority(&self, state: &AgentManagementSnapshot) -> anyhow::Result<()> {
        let project = self
            .request
            .project_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("bootstrap project missing"))?;
        let authority = state
            .projects
            .get(project)
            .ok_or_else(|| anyhow::anyhow!("bootstrap project missing"))?;
        anyhow::ensure!(
            authority.authorized_director_session == self.director
                && super::store::request_sha256(authority)? == self.authority_sha256,
            "bootstrap project authority changed"
        );
        Ok(())
    }
}

impl AgentManagementProvider {
    pub(crate) fn authorize_bootstrap_intent(
        &self,
        review: &BootstrapIntentReview,
    ) -> anyhow::Result<BootstrapIntentReview> {
        let _execution = super::provider::provider_execution_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("bootstrap execution lock unavailable"))?;
        let _mutation = self.store().lock_mutations()?;
        let state = self.store().snapshot()?;
        if let Some(prior) = state.bootstrap_intents.get(&review.request.action_id) {
            anyhow::ensure!(prior == review, "bootstrap intent action conflict");
            return Ok(prior.clone());
        }
        review.validate_evidence()?;
        review.validate_authority(&state)?;
        anyhow::ensure!(
            review.expires_at_unix > chrono::Utc::now().timestamp(),
            "bootstrap intent expired"
        );
        anyhow::ensure!(
            state.store_revision == review.store_revision,
            "bootstrap intent review stale"
        );
        anyhow::ensure!(
            !state.actions.contains_key(&review.request.action_id),
            "bootstrap action already started"
        );
        self.store()
            .with_state(true, |mut current| {
                if current.store_revision != review.store_revision {
                    return Err(AgentManagementError::Conflict(
                        "bootstrap_intent_review_stale",
                    ));
                }
                current.schema = AgentManagementStoreSchema::V2;
                current
                    .bootstrap_intents
                    .insert(review.request.action_id.clone(), review.clone());
                Ok((current, review.clone(), true))
            })
            .map_err(Into::into)
    }
}
