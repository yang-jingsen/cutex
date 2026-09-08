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
            self.version == 1,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchReview {
    pub agent: AgentArchiveReview,
    pub contract: ExplicitLaunchContract,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitLaunchReceipt {
    pub action_id: AgentActionId,
    pub review: ExplicitLaunchReview,
    pub activated_revision: u64,
    pub committed_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExplicitLaunchRequest {
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
        principal: &HumanManagementPrincipal,
        path: &Path,
        request: &ExplicitLaunchRequest,
        tasks: &crate::task_service::TaskServiceProvider,
    ) -> anyhow::Result<serde_json::Value> {
        match request {
            ExplicitLaunchRequest::Review {
                cutex_session_id,
                contract,
            } => {
                contract.validate()?;
                let agent = self.review_agent_archive(
                    principal,
                    path,
                    cutex_session_id,
                    AgentArchiveOperation::Archive,
                )?;
                let sessions = crate::session::store::load_cutex_session_store_from_path(path)?;
                let record = &sessions.sessions[cutex_session_id.as_str()];
                validate_activation(record, contract)?;
                Ok(serde_json::to_value(ExplicitLaunchReview {
                    agent,
                    contract: contract.clone(),
                })?)
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
                            anyhow::ensure!(
                                &receipt.review == review,
                                "explicit_launch_action_conflict"
                            );
                            return Ok(serde_json::to_value(receipt)?);
                        }
                        review.contract.validate()?;
                        let state = self.store().snapshot()?;
                        let id = &review.agent.cutex_session_id;
                        super::archive::guard(&state, id)?;
                        anyhow::ensure!(
                            self.archive_authority_digest(&state, id)?
                                == review.agent.authority_sha256,
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
                            .get_mut(id.as_str())
                            .ok_or_else(|| anyhow::anyhow!("durable record missing"))?;
                        anyhow::ensure!(
                            super::store::request_sha256(record)? == review.agent.durable_sha256
                                && record.revision == review.agent.revision
                                && record.runtime_generation == review.agent.runtime_generation,
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
                        sessions
                            .explicit_launch_receipts
                            .insert(action_id.to_string(), receipt.clone());
                        // Requirement and immutable action/audit receipt share the durable CAS write.
                        crate::session::store::save_locked_session_store(path, sessions)?;
                        Ok(serde_json::to_value(receipt)?)
                    })
                })?
            }
        }
    }
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
