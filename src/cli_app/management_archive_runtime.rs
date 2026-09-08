//! Fail-closed Archive proof adapter; never uses PID-only stop success.
use cutex::agent_management::AgentArchiveRuntime;
use cutex::session::{archive::record_has_runtime_claim, model::CutexSessionRecord};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

#[derive(Default)]
pub(super) struct GuardedArchiveRuntime {
    events: Option<File>,
    generation: Option<u64>,
    never_started: bool,
}

impl GuardedArchiveRuntime {
    fn observations_offline(record: &CutexSessionRecord) -> anyhow::Result<()> {
        let status =
            super::management_context::load_app_server_runtime_status(&record.cutex_session_id)?;
        anyhow::ensure!(
            !status.is_some_and(|s| s.connected),
            "archive_manager_still_connected"
        );
        let config = cutex::config::store::load_codez_config();
        let agents = super::management_lifecycle::try_live_agents_for_management_identity(
            &config,
            record
                .codex_session_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("native_identity_missing"))?,
            record.last_runtime_agent_id.as_deref(),
        )?;
        anyhow::ensure!(agents.is_empty(), "archive_live_runtime_still_observed");
        Ok(())
    }

    fn captured_scope_empty(&mut self) -> anyhow::Result<()> {
        let events = self
            .events
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("unsupported_stop_proof"))?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            // Kernel cgroup removal requires emptiness. This is the captured
            // object, not a missing path or unavailable scope observation.
            if events.metadata()?.nlink() == 0 {
                return Ok(());
            }
        }
        events.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        events.read_to_string(&mut text)?;
        anyhow::ensure!(
            text.lines().any(|line| line == "populated 0"),
            "archive_containment_not_empty"
        );
        Ok(())
    }
}

impl AgentArchiveRuntime for GuardedArchiveRuntime {
    fn prepare(&mut self, record: &CutexSessionRecord, _restoring: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            cutex::runtime::lifecycle::cutex_session_host_is_local(
                &record.host_id,
                &cutex::platform::host::current_host_name()
            ),
            "unsupported_remote_stop_proof"
        );
        self.generation = Some(record.runtime_generation);
        self.never_started = record.runtime_history_known
            && record.runtime_generation == 0
            && !record_has_runtime_claim(record)
            && record.last_runtime_agent_id.is_none()
            && record.last_seen_at.is_none()
            && record.alden_session_name.is_none();
        let roster = cutex::agent_management::AgentManagementStore::open_default()?.snapshot()?;
        self.never_started |= roster.agent_archive_audit.values().any(|receipt| {
            receipt.stage == cutex::agent_management::AgentArchiveStage::Stopped
                && receipt.result.as_ref().is_some_and(|proof| {
                    proof.cutex_session_id == record.cutex_session_id
                        && proof.codex_session_id == record.codex_session_id
                        && proof.runtime_generation == record.runtime_generation
                })
        });
        if !record_has_runtime_claim(record) {
            let store = cutex::session::store::load_cutex_session_store()?;
            self.never_started |= store.agent_archive_receipts.values().any(|receipt| {
                receipt.stage == cutex::agent_management::AgentArchiveStage::Committed
                    && receipt.request.review.operation
                        == cutex::agent_management::AgentArchiveOperation::Archive
                    && receipt.result.as_ref().is_some_and(|proof| {
                        proof.cutex_session_id == record.cutex_session_id
                            && proof.codex_session_id == record.codex_session_id
                            && proof.runtime_generation == record.runtime_generation
                    })
            });
        }
        if self.never_started {
            return Self::observations_offline(record);
        }
        anyhow::ensure!(record.runtime_backend == cutex::session::model::CutexSessionRuntimeBackend::Host,
            "unsupported_stop_proof: this backend has no supported authoritative containment; Agent remains unarchived");
        #[cfg(target_os = "linux")]
        {
            let group = cutex::runtime::process_scope::managed_agent_scope_control_group(&record.cutex_session_id)?
                .ok_or_else(|| anyhow::anyhow!("unsupported_stop_proof: no affirmative runtime containment; Archive made no change"))?;
            let pid = record
                .runtime_pid
                .or_else(|| record.app_server_runtime.as_ref().map(|b| b.pid))
                .ok_or_else(|| anyhow::anyhow!("unsupported_stop_proof: missing occurrence PID"))?;
            let membership = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
            anyhow::ensure!(
                membership.lines().any(|line| line == format!("0::{group}")),
                "unsupported_stop_proof: occurrence not in authoritative scope"
            );
            // Hold the exact kernel object across stop, rather than re-resolve
            // a unit name which a later generation could reuse.
            self.events = Some(File::open(
                std::path::Path::new("/sys/fs/cgroup")
                    .join(group.trim_start_matches('/'))
                    .join("cgroup.events"),
            )?);
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("unsupported_stop_proof: online Archive requires supported containment")
    }

    fn stop_and_verify(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.generation == Some(record.runtime_generation),
            "archive_generation_conflict"
        );
        if self.never_started {
            return Self::observations_offline(record);
        }
        anyhow::ensure!(self.events.is_some(), "unsupported_stop_proof");
        super::app_server_runtime::runtime_manager()
            .interrupt_active_turn(&record.cutex_session_id)?;
        // Do not join the manager's event/bridge workers under provider and
        // durable fences: a worker may be finishing a persistence callback.
        // Killing the contained runtime disconnects its transport; observation
        // must independently confirm that, without treating a join as proof.
        let outcome = cutex::runtime::process_scope::terminate_managed_agent_scope(
            &record.cutex_session_id,
            false,
        )?;
        anyhow::ensure!(
            outcome.found && outcome.stopped,
            "archive_contained_stop_not_proven: {}",
            outcome.detail
        );
        self.captured_scope_empty()
    }

    fn verify_offline(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.generation == Some(record.runtime_generation),
            "archive_generation_conflict"
        );
        if !self.never_started {
            self.captured_scope_empty()?;
        }
        Self::observations_offline(record)
    }
}
