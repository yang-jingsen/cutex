//! Same-action stock execution journal. No implicit downgrade or new identity.
use super::*;
use crate::launch::stock::{
    current_configuration, validate_native, StockBundle, StockConfiguration,
};
use crate::management::control_plane::HumanManagementPrincipal;
use crate::role_revision::CutexSessionId;
use crate::session::model::{CutexAppServerRuntimeBinding, CutexSessionRecord};
use crate::session::store::{
    load_cutex_session_store_from_path, save_locked_session_store, with_locked_session_store,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockRuntimeReview {
    pub subject: ExplicitLaunchSubject,
    pub contract: ExplicitLaunchContract,
    pub configuration: StockConfiguration,
    pub restart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_mcp: Option<crate::launch::job_mcp::ReviewedJobMcp>,
    #[serde(
        default,
        skip_serializing_if = "crate::launch::stock::CanonicalBytePolicy::is_default"
    )]
    pub receiver_canonical_byte_limit: crate::launch::stock::CanonicalBytePolicy,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StockRuntimeStage {
    Prepared,
    Claimed,
    Spawned,
    Ready,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockRuntimeReceipt {
    pub action_id: AgentActionId,
    pub review: StockRuntimeReview,
    pub stage: StockRuntimeStage,
    pub claim_id: String,
    pub runtime_agent_id: String,
    pub expected_generation: u64,
    pub binding: Option<CutexAppServerRuntimeBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<StockPublication>,
    pub error: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockPublication {
    pub path: std::path::PathBuf,
    pub device: u64,
    pub inode: u64,
}

pub trait StockRuntimeExecutor {
    /// Hold the exact kernel lease through child publication. On replay, a busy,
    /// missing or replaced lease cannot establish absence and must reject.
    fn publication(&mut self, receipt: &StockRuntimeReceipt) -> anyhow::Result<StockPublication>;
    /// Exact published occurrence only; false means still alive. Unknowns reject.
    fn published_owner_absent(&mut self, receipt: &StockRuntimeReceipt) -> anyhow::Result<bool>;
    fn stop(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()>;
    fn spawn(
        &mut self,
        record: &CutexSessionRecord,
        bundle: &StockBundle,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<CutexAppServerRuntimeBinding>;
    fn connect(
        &mut self,
        record: &CutexSessionRecord,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<()>;
    /// Must only stop the child created by this invocation, never another owner.
    fn cleanup_owned(&mut self) -> anyhow::Result<()>;
    fn retain_owner(&mut self);
}

impl AgentManagementProvider {
    pub fn review_stock_runtime(
        &self,
        path: &Path,
        id: &CutexSessionId,
        restart: bool,
        tasks: &crate::task_service::TaskServiceProvider,
    ) -> anyhow::Result<StockRuntimeReview> {
        let _mutation = self.store().lock_mutations()?;
        self.review_stock_runtime_locked(path, id, restart, tasks)
    }

    /// Caller holds the provider mutation lock; this acquires the Task fence.
    pub(crate) fn review_stock_runtime_locked(
        &self,
        path: &Path,
        id: &CutexSessionId,
        restart: bool,
        tasks: &crate::task_service::TaskServiceProvider,
    ) -> anyhow::Result<StockRuntimeReview> {
        tasks.with_archive_read_fence(|tasks| -> anyhow::Result<_> {
            let state = self.store().snapshot()?;
            let project = super::archive::guard(&state, id)
                .map_err(|e| anyhow::anyhow!("explicit stock protected-role/project guard: {e}"))?;
            no_task(tasks, id)?;
            let sessions = load_cutex_session_store_from_path(path)?;
            let record = sessions
                .sessions
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
            active(record, id)?;
            anyhow::ensure!(
                record.app_server_launch_claim_id.is_none(),
                "stock launch unresolved; replay original action"
            );
            if !restart {
                anyhow::ensure!(
                    !crate::session::archive::record_has_runtime_claim(record),
                    "stock start requires offline; explicitly review restart"
                );
            }
            let contract = record
                .explicit_launch
                .clone()
                .ok_or_else(|| anyhow::anyhow!("explicit stock activation required"))?;
            StockBundle::load(&contract)?;
            validate_native(record, &sessions, &contract)?;
            let configuration = current_configuration(record)?;
            let formal_name = record
                .formal_agent_name
                .clone()
                .or_else(|| state.agents.get(id).map(|a| a.spec.name.clone()))
                .ok_or_else(|| anyhow::anyhow!("formal Agent name unavailable"))?;
            Ok(StockRuntimeReview {
                subject: ExplicitLaunchSubject {
                    cutex_session_id: id.clone(),
                    formal_name,
                    durable_sha256: super::store::request_sha256(record)?,
                    authority_sha256: self.archive_authority_digest(&state, id)?,
                    current_project_id: project,
                    revision: record.revision,
                    runtime_generation: record.runtime_generation,
                },
                contract,
                configuration,
                restart,
                job_mcp: None,
                receiver_canonical_byte_limit: Default::default(),
            })
        })?
    }

    pub fn execute_stock_runtime(
        &self,
        _principal: &HumanManagementPrincipal,
        path: &Path,
        action_id: &AgentActionId,
        review: &StockRuntimeReview,
        tasks: &crate::task_service::TaskServiceProvider,
        runtime: &mut dyn StockRuntimeExecutor,
    ) -> anyhow::Result<StockRuntimeReceipt> {
        let _execution = super::provider::provider_execution_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("stock execution lock unavailable"))?;
        let _mutation = self.store().lock_mutations()?;
        self.execute_stock_runtime_locked(path, action_id, review, tasks, runtime)
    }

    /// Caller holds execution then mutation locks. Only the root wrapper and
    /// sealed, provider-authorized bootstrap permit may enter this helper.
    pub(crate) fn execute_stock_runtime_locked(
        &self,
        path: &Path,
        action_id: &AgentActionId,
        review: &StockRuntimeReview,
        tasks: &crate::task_service::TaskServiceProvider,
        runtime: &mut dyn StockRuntimeExecutor,
    ) -> anyhow::Result<StockRuntimeReceipt> {
        tasks.with_archive_read_fence(|tasks| -> anyhow::Result<_> {
            let id = &review.subject.cutex_session_id;
            let sessions = load_cutex_session_store_from_path(path)?;
            let mut receipt =
                if let Some(previous) = sessions.explicit_launch_receipts.get(action_id.as_str()) {
                    let ExplicitLaunchActionReceipt::Runtime(previous) = previous else {
                        anyhow::bail!("explicit_launch_action_conflict")
                    };
                    anyhow::ensure!(
                        &previous.review == review,
                        "explicit_launch_action_conflict"
                    );
                    if previous.stage == StockRuntimeStage::Ready {
                        return Ok(previous.clone());
                    }
                    previous.clone()
                } else {
                    let record = sessions
                        .sessions
                        .get(id.as_str())
                        .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
                    anyhow::ensure!(
                        super::store::request_sha256(record)? == review.subject.durable_sha256
                            && record.revision == review.subject.revision
                            && record.runtime_generation == review.subject.runtime_generation,
                        "stock confirmation stale"
                    );
                    StockRuntimeReceipt {
                        action_id: action_id.clone(),
                        review: review.clone(),
                        stage: StockRuntimeStage::Prepared,
                        claim_id: uuid::Uuid::new_v4().to_string(),
                        runtime_agent_id: format!("stock.{}", uuid::Uuid::new_v4()),
                        expected_generation: record
                            .runtime_generation
                            .checked_add(1)
                            .filter(|g| *g <= crate::management::v2::model::MAX_SAFE_SEQUENCE)
                            .ok_or_else(|| anyhow::anyhow!("runtime generation exhausted"))?,
                        binding: None,
                        publication: None,
                        error: None,
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    }
                };
            let state = self.store().snapshot()?;
            super::archive::guard(&state, id)?;
            no_task(tasks, id)?;
            anyhow::ensure!(
                self.archive_authority_digest(&state, id)? == review.subject.authority_sha256,
                "stock authority changed"
            );
            let record = sessions
                .sessions
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
            active(record, id)?;
            anyhow::ensure!(
                record.explicit_launch.as_ref() == Some(&review.contract),
                "stock launch requirement changed"
            );
            anyhow::ensure!(
                current_configuration(record)? == review.configuration,
                "stock configuration changed; fresh review required"
            );
            let bundle = StockBundle::load(&review.contract)?;
            if let Some(job) = &review.job_mcp {
                job.validate(&bundle)?;
            } else {
                anyhow::ensure!(!sessions.explicit_launch_receipts.values().any(|r| matches!(r,
                    ExplicitLaunchActionReceipt::Runtime(r) if r.review.subject.cutex_session_id == *id && r.review.job_mcp.is_some())),
                    "previously reviewed Job MCP cannot be silently omitted; provide an explicit descriptor");
            }
            anyhow::ensure!(bundle.common_ingress() || review.receiver_canonical_byte_limit.is_default(),
                "unchanged stock is registration-only; receiver ingress policy unsupported");
            validate_native(record, &sessions, &review.contract)?;
            if receipt.stage == StockRuntimeStage::Spawned
                && record.runtime_generation == review.subject.runtime_generation
                && runtime.published_owner_absent(&receipt)?
            {
                // A committed gate may lose its creator before release. Same
                // lease + exact process absence permits only this unregistered
                // occurrence to return to Claimed under the original action.
                with_locked_session_store(path, |store| {
                    let current = store.sessions.get_mut(id.as_str()).ok_or_else(|| anyhow::anyhow!("stock record missing"))?;
                    anyhow::ensure!(current.app_server_launch_claim_id.as_deref() == Some(&receipt.claim_id)
                        && current.app_server_runtime == receipt.binding
                        && current.runtime_generation == review.subject.runtime_generation
                        && current.revision == review.subject.revision
                        && current.explicit_launch.as_ref() == Some(&review.contract),
                        "published recovery occurrence changed");
                    current.app_server_runtime = None;
                    current.runtime_pid = None;
                    receipt.binding = None;
                    receipt.stage = StockRuntimeStage::Claimed;
                    store.explicit_launch_receipts.insert(action_id.to_string(), ExplicitLaunchActionReceipt::Runtime(receipt.clone()));
                    save_locked_session_store(path, store)
                })?;
            }
            if receipt.stage == StockRuntimeStage::Prepared {
                anyhow::ensure!(
                    super::store::request_sha256(record)? == review.subject.durable_sha256,
                    "stock prepared outcome uncertain; no repeated destructive stop"
                );
                save_receipt(path, &receipt)?;
                if review.restart {
                    if let Some(job) = &review.job_mcp { job.validate(&bundle)?; }
                    runtime.stop(record)?;
                }
                receipt.publication = Some(runtime.publication(&receipt)?);
                with_locked_session_store(path, |store| {
                    if let Some(job) = &review.job_mcp { job.validate(&bundle)?; }
                    let current = store
                        .sessions
                        .get_mut(id.as_str())
                        .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
                    active(current, id)?;
                    anyhow::ensure!(
                        current.revision == review.subject.revision,
                        "stock durable configuration changed after stop; stopped-not-started"
                    );
                    anyhow::ensure!(
                        current.explicit_launch.as_ref() == Some(&review.contract)
                            && current_configuration(current)? == review.configuration,
                        "stock configuration changed after stop; stopped-not-started"
                    );
                    anyhow::ensure!(
                        !crate::session::archive::record_has_runtime_claim(current),
                        "stock owner remains; no duplicate launch"
                    );
                    anyhow::ensure!(
                        current.runtime_generation == review.subject.runtime_generation,
                        "stock runtime generation changed after stop"
                    );
                    current.app_server_launch_claim_id = Some(receipt.claim_id.clone());
                    current.updated_at = chrono::Utc::now().to_rfc3339();
                    receipt.stage = StockRuntimeStage::Claimed;
                    store.explicit_launch_receipts.insert(
                        action_id.to_string(),
                        ExplicitLaunchActionReceipt::Runtime(receipt.clone()),
                    );
                    save_locked_session_store(path, store)
                })?;
            }
            if receipt.stage == StockRuntimeStage::Claimed {
                anyhow::ensure!(receipt.publication.is_some(), "publication_missing: legacy uncertain claim has no absence proof; no automatic retry");
                runtime.publication(&receipt)?;
                let current = load_cutex_session_store_from_path(path)?
                    .sessions
                    .remove(id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
                anyhow::ensure!(current.app_server_launch_claim_id.as_deref() == Some(&receipt.claim_id)
                    && current.app_server_runtime.is_none() && current.runtime_pid.is_none()
                    && current.runtime_generation == review.subject.runtime_generation
                    && current.revision == review.subject.revision,
                    "publication conflict: claim/configuration or runtime changed; no spawn");
                let binding = match runtime.spawn(&current, &bundle, &receipt) {
                    Ok(binding) => binding,
                    Err(error) => {
                        receipt.error = Some(format!("spawn failed: {error:#}; claim retained"));
                        save_receipt(path, &receipt)?;
                        return Err(error);
                    }
                };
                receipt.binding = Some(binding.clone());
                receipt.stage = StockRuntimeStage::Spawned;
                #[cfg(feature = "stock-launch-test-hook")]
                if std::env::var("CUTEX_STOCK_TEST_CREATOR_DEATH_ACTION").ok().as_deref() == Some(action_id.as_str()) {
                    eprintln!("private precommit creator death; owned child PID {}", binding.pid);
                    unsafe { libc::_exit(86); }
                }
                let persisted = with_locked_session_store(path, |store| {
                    if let Some(job) = &review.job_mcp { job.validate(&bundle)?; }
                    #[cfg(feature = "stock-launch-test-hook")]
                    { static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if std::env::var("CUTEX_STOCK_TEST_COMMIT_FAIL_ACTION")
                        .ok()
                        .as_deref()
                        == Some(action_id.as_str()) && !FAILED.swap(true, std::sync::atomic::Ordering::SeqCst)
                    {
                        anyhow::bail!(
                            "private injected precommit failure; owned child PID {}",
                            binding.pid
                        );
                    }
                    }
                    let current = store
                        .sessions
                        .get_mut(id.as_str())
                        .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
                    anyhow::ensure!(
                        current.revision == review.subject.revision,
                        "stock durable configuration changed during spawn"
                    );
                    anyhow::ensure!(
                        current.app_server_launch_claim_id.as_deref() == Some(&receipt.claim_id)
                            && current.explicit_launch.as_ref() == Some(&review.contract)
                            && current_configuration(current)? == review.configuration,
                        "stock claim/configuration changed during spawn"
                    );
                    current.runtime_pid = Some(binding.pid);
                    current.app_server_runtime = Some(binding.clone());
                    store.explicit_launch_receipts.insert(
                        action_id.to_string(),
                        ExplicitLaunchActionReceipt::Runtime(receipt.clone()),
                    );
                    save_locked_session_store(path, store)
                });
                if let Err(error) = persisted {
                    runtime.cleanup_owned()?;
                    return Err(
                        error.context("spawned child stopped; commit uncertain, claim retained")
                    );
                }
                #[cfg(feature = "stock-launch-test-hook")]
                if std::env::var("CUTEX_STOCK_TEST_PUBLISHED_DEATH_ACTION").ok().as_deref() == Some(action_id.as_str()) {
                    eprintln!("private published creator death; owned child PID {}", binding.pid);
                    unsafe { libc::_exit(87); }
                }
            }
            anyhow::ensure!(
                receipt.stage == StockRuntimeStage::Spawned,
                "stock claim has no committed child; outcome uncertain, no duplicate spawn"
            );
            let current = load_cutex_session_store_from_path(path)?
                .sessions
                .remove(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
            anyhow::ensure!(
                current.app_server_launch_claim_id.as_deref() == Some(&receipt.claim_id)
                    && current.app_server_runtime == receipt.binding,
                "stock owner changed before registration"
            );
            // A lost response reuses this exact binding/runtime/generation; no spawn.
            if let Err(error) = runtime.connect(&current, &receipt) {
                runtime.retain_owner();
                receipt.error = Some(format!(
                    "readiness incomplete: {error:#}; replay exact action"
                ));
                save_receipt(path, &receipt)?;
                return Ok(receipt);
            }
            runtime.retain_owner();
            with_locked_session_store(path, |store| {
                if let Some(job) = &review.job_mcp { job.validate(&bundle)?; }
                let current = store
                    .sessions
                    .get_mut(id.as_str())
                    .ok_or_else(|| anyhow::anyhow!("stock durable record missing"))?;
                anyhow::ensure!(
                    current.revision == review.subject.revision,
                    "stock durable configuration changed during readiness; claim retained"
                );
                anyhow::ensure!(
                    current.app_server_launch_claim_id.as_deref() == Some(&receipt.claim_id)
                        && current.app_server_runtime == receipt.binding
                        && current.current_runtime_agent_id.as_deref()
                            == Some(&receipt.runtime_agent_id)
                        && current.runtime_generation == receipt.expected_generation,
                    "stock registration readback mismatch; claim retained"
                );
                anyhow::ensure!(
                    current.runtime_pid == receipt.binding.as_ref().map(|b| b.pid),
                    "stock PID readback mismatch; claim retained"
                );
                anyhow::ensure!(
                    current_configuration(current)? == review.configuration
                        && current.explicit_launch.as_ref() == Some(&review.contract),
                    "stock config changed during readiness; claim retained"
                );
                current.app_server_launch_claim_id = None;
                current.updated_at = chrono::Utc::now().to_rfc3339();
                receipt.stage = StockRuntimeStage::Ready;
                receipt.error = None;
                receipt.updated_at = chrono::Utc::now().to_rfc3339();
                store.explicit_launch_receipts.insert(
                    action_id.to_string(),
                    ExplicitLaunchActionReceipt::Runtime(receipt.clone()),
                );
                save_locked_session_store(path, store)
            })?;
            Ok(receipt)
        })?
    }
}
fn save_receipt(path: &Path, receipt: &StockRuntimeReceipt) -> anyhow::Result<()> {
    with_locked_session_store(path, |store| {
        store.explicit_launch_receipts.insert(
            receipt.action_id.to_string(),
            ExplicitLaunchActionReceipt::Runtime(receipt.clone()),
        );
        save_locked_session_store(path, store)
    })
}
fn active(record: &CutexSessionRecord, id: &CutexSessionId) -> anyhow::Result<()> {
    anyhow::ensure!(
        record.cutex_session_id == id.as_str()
            && !record.is_retired()
            && record.agent_enabled
            && record.registration_class
                == crate::agent_bus::model::AgentRegistrationClass::Persistent,
        "exact active persistent stock Agent required"
    );
    Ok(())
}
fn no_task(
    tasks: &crate::task_service::TaskServiceSnapshot,
    id: &CutexSessionId,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !tasks
            .assignments
            .values()
            .any(|a| &a.assignee_cutex_session == id
                && a.state != crate::task_service::AssignmentState::Closed),
        "stock runtime protected by active task"
    );
    Ok(())
}

/// Called by the owned-child adapter only after exact process stop proof.
pub fn commit_stock_runtime_stop(path: &Path, expected: &CutexSessionRecord) -> anyhow::Result<()> {
    with_locked_session_store(path, |store| {
        let current = store
            .sessions
            .get(&expected.cutex_session_id)
            .ok_or_else(|| anyhow::anyhow!("stock record missing after stop"))?;
        anyhow::ensure!(
            current.app_server_runtime == expected.app_server_runtime
                && current.runtime_generation == expected.runtime_generation
                && current.explicit_launch == expected.explicit_launch,
            "stock stopped, but current occurrence changed; no clear"
        );
        crate::session::service::clear_cutex_session_runtime_record(
            store,
            &expected.cutex_session_id,
            false,
        )?;
        save_locked_session_store(path, store)
    })
}
