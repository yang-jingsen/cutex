//! Human-only execution selection. No removal or implicit backend fallback.
use super::*;
use crate::management::control_plane::HumanManagementPrincipal;
use crate::role_revision::{CutexSessionId, Sha256};
use crate::session::model::CutexSessionRecord;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchContract {
    pub version: u32,
    pub native_id: String,
    pub native_home: PathBuf,
    pub bundle_manifest: PathBuf,
    pub bundle_sha256: Sha256,
}

impl ExplicitLaunchContract {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.version, 1 | 2),
            "unsupported explicit launch contract version"
        );
        anyhow::ensure!(
            uuid::Uuid::parse_str(&self.native_id)?.to_string() == self.native_id,
            "exact native UUID required"
        );
        for path in [&self.native_home, &self.bundle_manifest] {
            anyhow::ensure!(
                path.is_absolute() && path.canonicalize()? == *path,
                "canonical existing explicit launch path required"
            );
        }
        anyhow::ensure!(self.native_home.is_dir(), "native home missing");
        anyhow::ensure!(
            file_sha256(&self.bundle_manifest)? == self.bundle_sha256,
            "bundle evidence missing or changed"
        );
        Ok(())
    }
}

pub fn file_sha256(path: &Path) -> anyhow::Result<Sha256> {
    use sha2::Digest;
    use std::io::Read;
    let mut input = std::fs::File::open(path)?;
    let mut hash = sha2::Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Sha256::new(format!("{:x}", hash.finalize())).map_err(|_| anyhow::anyhow!("invalid digest"))
}

pub fn require_default_launch(record: &CutexSessionRecord) -> anyhow::Result<()> {
    anyhow::ensure!(record.explicit_launch.is_none(), "explicit_stock_launch_required: generic launch/restart/attach is disabled; owner is unchanged");
    Ok(())
}

#[test]
fn explicit_launch_versions_require_unchanged_manifest_evidence() {
    let root = std::env::temp_dir().join(format!("s6f-contract-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let manifest = root.join("bundle.json");
    std::fs::write(&manifest, b"private evidence").unwrap();
    let mut contract = ExplicitLaunchContract {
        version: 1,
        native_id: uuid::Uuid::new_v4().to_string(),
        native_home: root.clone(),
        bundle_manifest: manifest.clone(),
        bundle_sha256: file_sha256(&manifest).unwrap(),
    };
    contract.validate().unwrap();
    contract.version = 2;
    contract.validate().unwrap();
    contract.version = 3;
    assert!(contract.validate().is_err());
    contract.version = 2;
    std::fs::write(&manifest, b"changed evidence").unwrap();
    assert!(contract.validate().is_err());
    std::fs::remove_file(manifest).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchReview {
    pub subject: ExplicitLaunchSubject,
    pub contract: ExplicitLaunchContract,
    pub configuration: crate::launch::stock::StockConfiguration,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchSubject {
    pub cutex_session_id: CutexSessionId,
    pub formal_name: String,
    pub durable_sha256: Sha256,
    pub authority_sha256: Sha256,
    pub current_project_id: Option<ProjectId>,
    pub revision: u64,
    pub runtime_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchReceipt {
    pub action_id: AgentActionId,
    pub review: ExplicitLaunchReview,
    pub activated_revision: u64,
    pub committed_at: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "receipt", rename_all = "snake_case")]
pub enum ExplicitLaunchActionReceipt {
    Bootstrap(BootstrapAdoptionReceipt),
    Activation(ExplicitLaunchReceipt),
    Runtime(StockRuntimeReceipt),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExplicitLaunchRequest {
    ReviewBootstrap {
        request: AgentManagementRequest,
        native_home: PathBuf,
        bundle_manifest: PathBuf,
        bundle_sha256: Sha256,
        expires_at_unix: i64,
    },
    AuthorizeBootstrap {
        review: BootstrapIntentReview,
    },
    ReviewRuntime {
        cutex_session_id: CutexSessionId,
        restart: bool,
        #[serde(default)]
        receiver_canonical_byte_limit: crate::launch::stock::CanonicalBytePolicy,
    },
    Run {
        action_id: AgentActionId,
        review: StockRuntimeReview,
    },
    Review {
        cutex_session_id: CutexSessionId,
        contract: ExplicitLaunchContract,
    },
    Activate {
        action_id: AgentActionId,
        review: ExplicitLaunchReview,
    },
}

impl AgentManagementProvider {
    pub fn explicit_launch_action(
        &self,
        _principal: &HumanManagementPrincipal,
        path: &Path,
        request: &ExplicitLaunchRequest,
        tasks: &crate::task_service::TaskServiceProvider,
    ) -> anyhow::Result<serde_json::Value> {
        match request {
            ExplicitLaunchRequest::ReviewBootstrap {
                request,
                native_home,
                bundle_manifest,
                bundle_sha256,
                expires_at_unix,
            } => {
                let _mutation = self.store().lock_mutations()?;
                let state = self.store().snapshot()?;
                let project = request
                    .project_id
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("bootstrap requires exact project"))?;
                let authority = state
                    .projects
                    .get(project)
                    .ok_or_else(|| anyhow::anyhow!("bootstrap project missing"))?;
                let spec = request.operation.bootstrap_spec().ok_or_else(|| {
                    anyhow::anyhow!("bootstrap intent requires create/replace/director_rotate")
                })?;
                let review = BootstrapIntentReview {
                    version: if matches!(
                        &request.operation,
                        AgentOperation::Create {
                            start_mode: AgentStartMode::BootstrapOnly,
                            frozen_message: None,
                            ..
                        }
                    ) {
                        1
                    } else {
                        2
                    },
                    request: request.clone(),
                    director: authority.authorized_director_session.clone(),
                    authority_sha256: super::store::request_sha256(authority)?,
                    store_revision: state.store_revision,
                    native_home: native_home.clone(),
                    bundle_manifest: bundle_manifest.clone(),
                    bundle_sha256: bundle_sha256.clone(),
                    configuration: crate::launch::stock::bootstrap_configuration(spec)?,
                    runtime_groups: crate::agent_bus::groups::normalize_registered_agent_groups(
                        spec.groups.clone(),
                        None,
                        &spec.cwd,
                    ),
                    expires_at_unix: *expires_at_unix,
                };
                review.validate_evidence()?;
                anyhow::ensure!(
                    *expires_at_unix > chrono::Utc::now().timestamp(),
                    "bootstrap intent expired"
                );
                Ok(serde_json::to_value(review)?)
            }
            ExplicitLaunchRequest::AuthorizeBootstrap { review } => Ok(serde_json::to_value(
                self.authorize_bootstrap_intent(review)?,
            )?),
            ExplicitLaunchRequest::ReviewRuntime {
                cutex_session_id,
                restart,
                receiver_canonical_byte_limit,
            } => self
                .review_stock_runtime(path, cutex_session_id, *restart, tasks)
                .and_then(|mut r| {
                    anyhow::ensure!(
                        crate::launch::stock::StockBundle::load(&r.contract)?.common_ingress()
                            || receiver_canonical_byte_limit.is_default(),
                        "unchanged stock is registration-only; receiver ingress policy unsupported"
                    );
                    r.receiver_canonical_byte_limit = receiver_canonical_byte_limit.clone();
                    Ok(serde_json::to_value(r)?)
                }),
            ExplicitLaunchRequest::Run { .. } => {
                anyhow::bail!("explicit stock runtime executor required")
            }
            ExplicitLaunchRequest::Review {
                cutex_session_id,
                contract,
            } => {
                let _mutation = self.store().lock_mutations()?;
                tasks.with_archive_read_fence(|tasks| -> anyhow::Result<_> {
                    let state = self.store().snapshot()?;
                    let current_project_id = super::archive::guard(&state, cutex_session_id)
                        .map_err(|e| {
                            anyhow::anyhow!("explicit launch protected-role/project guard: {e}")
                        })?;
                    ensure_no_task(tasks, cutex_session_id)?;
                    let sessions = crate::session::store::load_cutex_session_store_from_path(path)?;
                    let record = sessions
                        .sessions
                        .get(cutex_session_id.as_str())
                        .ok_or_else(|| anyhow::anyhow!("explicit launch durable record missing"))?;
                    validate_activation(record, contract)?;
                    crate::launch::stock::StockBundle::load(contract)?;
                    crate::launch::stock::validate_native(record, &sessions, contract)?;
                    let configuration = crate::launch::stock::current_configuration(record)?;
                    let formal_name = record
                        .formal_agent_name
                        .clone()
                        .or_else(|| {
                            state
                                .agents
                                .get(cutex_session_id)
                                .map(|a| a.spec.name.clone())
                        })
                        .ok_or_else(|| anyhow::anyhow!("explicit formal Agent name required"))?;
                    anyhow::ensure!(
                        !formal_name.trim().is_empty()
                            && !formal_name.chars().any(char::is_control),
                        "malformed formal Agent name"
                    );
                    Ok(serde_json::to_value(ExplicitLaunchReview {
                        subject: ExplicitLaunchSubject {
                            cutex_session_id: cutex_session_id.clone(),
                            formal_name,
                            durable_sha256: super::store::request_sha256(record)?,
                            authority_sha256: self
                                .archive_authority_digest(&state, cutex_session_id)?,
                            current_project_id,
                            revision: record.revision,
                            runtime_generation: record.runtime_generation,
                        },
                        contract: contract.clone(),
                        configuration,
                    })?)
                })?
            }
            ExplicitLaunchRequest::Activate { action_id, review } => {
                let _execution = super::provider::provider_execution_lock()
                    .lock()
                    .map_err(|_| anyhow::anyhow!("execution lock unavailable"))?;
                let _mutation = self.store().lock_mutations()?;
                tasks.with_archive_read_fence(|tasks| -> anyhow::Result<_> {
                    crate::session::store::with_locked_session_store(path, |sessions| {
                        if let Some(receipt) =
                            sessions.explicit_launch_receipts.get(action_id.as_str())
                        {
                            let ExplicitLaunchActionReceipt::Activation(receipt) = receipt else {
                                anyhow::bail!("explicit_launch_action_conflict")
                            };
                            anyhow::ensure!(
                                &receipt.review == review,
                                "explicit_launch_action_conflict"
                            );
                            return Ok(serde_json::to_value(receipt)?);
                        }
                        crate::launch::stock::StockBundle::load(&review.contract)?;
                        let state = self.store().snapshot()?;
                        let id = &review.subject.cutex_session_id;
                        super::archive::guard(&state, id)?;
                        anyhow::ensure!(
                            self.archive_authority_digest(&state, id)?
                                == review.subject.authority_sha256,
                            "explicit_launch_authority_stale"
                        );
                        anyhow::ensure!(
                            !tasks
                                .assignments
                                .values()
                                .any(|a| &a.assignee_cutex_session == id
                                    && a.state != crate::task_service::AssignmentState::Closed),
                            "explicit_launch_active_task"
                        );
                        let record = sessions
                            .sessions
                            .get(id.as_str())
                            .ok_or_else(|| anyhow::anyhow!("durable record missing"))?;
                        crate::launch::stock::validate_native(record, sessions, &review.contract)?;
                        anyhow::ensure!(
                            crate::launch::stock::current_configuration(record)?
                                == review.configuration,
                            "explicit_launch_configuration_stale"
                        );
                        let record = sessions
                            .sessions
                            .get_mut(id.as_str())
                            .ok_or_else(|| anyhow::anyhow!("durable record missing"))?;
                        anyhow::ensure!(
                            super::store::request_sha256(record)? == review.subject.durable_sha256
                                && record.revision == review.subject.revision
                                && record.runtime_generation == review.subject.runtime_generation,
                            "explicit_launch_confirmation_stale"
                        );
                        validate_activation(record, &review.contract)?;
                        record.explicit_launch = Some(review.contract.clone());
                        record.revision = record
                            .revision
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("revision exhausted"))?;
                        record.updated_at = chrono::Utc::now().to_rfc3339();
                        let receipt = ExplicitLaunchReceipt {
                            action_id: action_id.clone(),
                            review: review.clone(),
                            activated_revision: record.revision,
                            committed_at: record.updated_at.clone(),
                        };
                        sessions.explicit_launch_receipts.insert(
                            action_id.to_string(),
                            ExplicitLaunchActionReceipt::Activation(receipt.clone()),
                        );
                        // Requirement and immutable action/audit receipt share the durable CAS write.
                        crate::session::store::save_locked_session_store(path, sessions)?;
                        Ok(serde_json::to_value(receipt)?)
                    })
                })?
            }
        }
    }
}

fn ensure_no_task(
    tasks: &crate::task_service::TaskServiceSnapshot,
    id: &CutexSessionId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !tasks
            .assignments
            .values()
            .any(|a| &a.assignee_cutex_session == id
                && a.state != crate::task_service::AssignmentState::Closed),
        "explicit_launch_active_task"
    );
    Ok(())
}

fn validate_activation(
    record: &CutexSessionRecord,
    contract: &ExplicitLaunchContract,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        record.explicit_launch.is_none(),
        "explicit launch already activated; no downgrade API"
    );
    anyhow::ensure!(
        !record.is_retired()
            && record.agent_enabled
            && record.codex_session_id.as_deref() == Some(contract.native_id.as_str()),
        "active exact adopted identity required"
    );
    anyhow::ensure!(
        !crate::session::archive::record_has_runtime_claim(record),
        "explicit launch requires proven offline/no claim"
    );
    anyhow::ensure!(
        record.registration_class == crate::agent_bus::model::AgentRegistrationClass::Persistent,
        "persistent identity required"
    );
    Ok(())
}
