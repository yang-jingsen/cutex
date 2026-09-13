//! Human recovery of interrupted runtime actions, independent of migration plans.
use super::*;
use crate::config::atomic::write_private_pretty_json_atomic;
use crate::management::control_plane::HumanManagementPrincipal;
use crate::role_revision::CutexSessionId;
use crate::session::store::{
    load_cutex_session_store_from_path, save_locked_session_store, with_locked_session_store,
};
use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HumanRuntimeRecovery {
    pub action_id: AgentActionId,
    pub cutex_session_id: CutexSessionId,
    pub original_action_id: AgentActionId,
    pub claim_id: String,
    pub completed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeActionStatus {
    pub action_id: AgentActionId,
    pub state: String,
    pub receipt: Option<StockRuntimeReceipt>,
}

fn recovery_path(path: &Path, action: &AgentActionId) -> anyhow::Result<PathBuf> {
    Ok(path
        .parent()
        .context("session store parent missing")?
        .join("runtime/human-actions")
        .join(format!(
            "{}.json",
            super::store::request_sha256(action)?.as_str()
        )))
}

impl AgentManagementProvider {
    pub fn runtime_action_status(
        &self,
        path: &Path,
        action: &AgentActionId,
    ) -> anyhow::Result<RuntimeActionStatus> {
        let sessions = load_cutex_session_store_from_path(path)?;
        let receipt = match sessions.explicit_launch_receipts.get(action.as_str()) {
            Some(ExplicitLaunchActionReceipt::Runtime(r)) => Some(r.clone()),
            Some(_) => anyhow::bail!("action is not a runtime start; use its specific status"),
            None => None,
        };
        let state = match &receipt {
            None => "not_started",
            Some(r) if r.stage == StockRuntimeStage::Ready => "succeeded",
            Some(r) if r.error.is_some() => "failed",
            Some(_) => "in_progress",
        };
        Ok(RuntimeActionStatus {
            action_id: action.clone(),
            state: state.into(),
            receipt,
        })
    }

    pub fn recover_failed_runtime(
        &self,
        _principal: &HumanManagementPrincipal,
        path: &Path,
        id: &CutexSessionId,
        action: &AgentActionId,
        runtime: &mut dyn StockRuntimeExecutor,
    ) -> anyhow::Result<HumanRuntimeRecovery> {
        let _mutation = self.store().lock_mutations()?;
        let journal = recovery_path(path, action)?;
        let previous: Option<HumanRuntimeRecovery> = match std::fs::read(&journal) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        if let Some(previous) = &previous {
            ensure!(
                &previous.cutex_session_id == id && &previous.action_id == action,
                "recovery action conflicts with another request"
            );
            if previous.completed {
                return Ok(previous.clone());
            }
        }
        with_locked_session_store(path, |sessions| {
            let record = sessions
                .sessions
                .get(id.as_str())
                .context("agent not found")?;
            // A crash after the atomic session update leaves a prepared journal.
            // Finishing that journal must never affect a subsequent runtime.
            if let Some(previous) = &previous {
                if record.app_server_launch_claim_id.as_deref() != Some(&previous.claim_id) {
                    let mut done = previous.clone();
                    done.completed = true;
                    write_private_pretty_json_atomic(&journal, &done, "human runtime recovery")?;
                    return Ok(done);
                }
            }
            let claim = record
                .app_server_launch_claim_id
                .as_deref()
                .context("no interrupted start claim; refresh status or attach")?;
            let receipt = sessions
                .explicit_launch_receipts
                .values()
                .find_map(|r| match r {
                    ExplicitLaunchActionReceipt::Runtime(r)
                        if r.claim_id == claim && &r.review.subject.cutex_session_id == id =>
                    {
                        Some(r.clone())
                    }
                    _ => None,
                })
                .context("interrupted start receipt missing; inspect before repair")?;
            ensure!(
                receipt.stage != StockRuntimeStage::Ready,
                "runtime reached Ready; attach or stop it instead"
            );
            ensure!(
                record.runtime_pid.is_none()
                    && record.app_server_runtime.is_none()
                    && record.current_runtime_agent_id.is_none(),
                "agent has a recorded runtime; stop or reconcile that runtime first"
            );
            ensure!(
                receipt.publication.is_some(),
                "publication evidence missing; cannot determine child absence"
            );
            // Keep the original creator/child lease until the state update is durable.
            runtime.publication(&receipt)?;
            if receipt.binding.is_some() {
                ensure!(
                    runtime.published_owner_absent(&receipt)?,
                    "original runtime still exists; attach or stop it instead"
                );
            }
            let mut recovery = HumanRuntimeRecovery {
                action_id: action.clone(),
                cutex_session_id: id.clone(),
                original_action_id: receipt.action_id.clone(),
                claim_id: claim.into(),
                completed: false,
            };
            write_private_pretty_json_atomic(&journal, &recovery, "human runtime recovery")?;
            let record = sessions.sessions.get_mut(id.as_str()).unwrap();
            record.app_server_launch_claim_id = None;
            record.updated_at = chrono::Utc::now().to_rfc3339();
            // Keep the original failure receipt, generation, identity and history intact.
            save_locked_session_store(path, sessions)?;
            recovery.completed = true;
            write_private_pretty_json_atomic(&journal, &recovery, "human runtime recovery")?;
            Ok(recovery)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::stock::StockBundle;
    use crate::session::model::{CutexAppServerRuntimeBinding, CutexSessionRecord};
    use serde_json::json;

    struct Executor {
        busy: bool,
    }
    impl StockRuntimeExecutor for Executor {
        fn publication(&mut self, r: &StockRuntimeReceipt) -> anyhow::Result<StockPublication> {
            ensure!(!self.busy, "publication lease busy");
            Ok(r.publication.clone().unwrap())
        }
        fn published_owner_absent(&mut self, _: &StockRuntimeReceipt) -> anyhow::Result<bool> {
            Ok(false)
        }
        fn stop(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
            panic!("recovery must not stop")
        }
        fn spawn(
            &mut self,
            _: &CutexSessionRecord,
            _: &StockBundle,
            _: &StockRuntimeReceipt,
        ) -> anyhow::Result<CutexAppServerRuntimeBinding> {
            panic!("recovery must not spawn")
        }
        fn connect(
            &mut self,
            _: &CutexSessionRecord,
            _: &StockRuntimeReceipt,
        ) -> anyhow::Result<()> {
            panic!("recovery must not connect")
        }
        fn cleanup_owned(&mut self) -> anyhow::Result<()> {
            panic!("recovery must not signal")
        }
        fn retain_owner(&mut self) {
            panic!("no owner")
        }
    }
    struct Fixture {
        root: PathBuf,
        path: PathBuf,
        provider: AgentManagementProvider,
        id: CutexSessionId,
        original: StockRuntimeReceipt,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!("cutex-recovery-{}", uuid::Uuid::new_v4()));
        let path = root.join("cutex-sessions.json");
        let provider = AgentManagementProvider::open(root.join("provider")).unwrap();
        let mut record = CutexSessionRecord::new(
            "cutex.recovery-test".into(),
            Some(uuid::Uuid::new_v4().to_string()),
            "private".into(),
            "/private".into(),
            None,
        )
        .unwrap();
        let id = CutexSessionId::new(record.cutex_session_id.clone()).unwrap();
        record.app_server_launch_claim_id = Some("claim-original".into());
        let original: StockRuntimeReceipt = serde_json::from_value(json!({
            "action_id":"original-start", "stage":"claimed", "claim_id":"claim-original", "runtime_agent_id":"runtime-original", "expected_generation":1,
            "binding":null,"publication":{"path":"/private/claim","device":1,"inode":2},"error":"spawn failed: TMPDIR missing","updated_at":"2026-09-13T00:00:00Z",
            "review":{
                "subject":{"cutex_session_id":record.cutex_session_id,"formal_name":"Recovery test","durable_sha256":"a".repeat(64),"authority_sha256":"b".repeat(64),"current_project_id":null,"revision":0,"runtime_generation":0},
                "contract":{"version":2,"native_id":record.codex_session_id,"native_home":"/private","bundle_manifest":"/private/manifest","bundle_sha256":"c".repeat(64)},
                "configuration":{"profile_name":"alpha","profile_id":"private-profile","inherited":false,"profile_sha256":"c".repeat(64),"account_sha256":"d".repeat(64),"model":"private-model","reasoning":null,"model_provider":"private","provider":{"name":"private","base_url":"http://127.0.0.1:1/v1","wire_api":"responses","requires_openai_auth":false,"supports_websockets":false},"sandbox":"read-only","approval":"on-request"},"restart":false
            }
        })).unwrap();
        with_locked_session_store(&path, |s| {
            s.sessions.insert(id.as_str().into(), record);
            s.explicit_launch_receipts.insert(
                original.action_id.as_str().into(),
                ExplicitLaunchActionReceipt::Runtime(original.clone()),
            );
            save_locked_session_store(&path, s)
        })
        .unwrap();
        Fixture {
            root,
            path,
            provider,
            id,
            original,
        }
    }
    #[test]
    fn recovery_clears_only_absent_owner_claim_and_is_idempotent() {
        let f = fixture();
        let action = AgentActionId::new("human-recovery-test").unwrap();
        let before = load_cutex_session_store_from_path(&f.path).unwrap();
        let done = f
            .provider
            .recover_failed_runtime(
                &HumanManagementPrincipal::authenticated(),
                &f.path,
                &f.id,
                &action,
                &mut Executor { busy: false },
            )
            .unwrap();
        assert!(done.completed);
        let after = load_cutex_session_store_from_path(&f.path).unwrap();
        let r = &after.sessions[f.id.as_str()];
        assert!(r.app_server_launch_claim_id.is_none());
        assert_eq!(
            r.codex_session_id,
            before.sessions[f.id.as_str()].codex_session_id
        );
        assert_eq!(
            r.runtime_generation,
            before.sessions[f.id.as_str()].runtime_generation
        );
        assert_eq!(
            serde_json::to_value(&after.explicit_launch_receipts).unwrap(),
            serde_json::to_value(&before.explicit_launch_receipts).unwrap()
        );
        assert_eq!(
            done,
            f.provider
                .recover_failed_runtime(
                    &HumanManagementPrincipal::authenticated(),
                    &f.path,
                    &f.id,
                    &action,
                    &mut Executor { busy: true }
                )
                .unwrap()
        );
        let state = f
            .provider
            .runtime_action_status(&f.path, &f.original.action_id)
            .unwrap();
        assert_eq!(state.state, "failed");
    }
    #[test]
    fn busy_publication_or_recorded_process_cannot_be_recovered() {
        let f = fixture();
        let action = AgentActionId::new("blocked-recovery").unwrap();
        let before = std::fs::read(&f.path).unwrap();
        assert!(f
            .provider
            .recover_failed_runtime(
                &HumanManagementPrincipal::authenticated(),
                &f.path,
                &f.id,
                &action,
                &mut Executor { busy: true }
            )
            .is_err());
        assert_eq!(before, std::fs::read(&f.path).unwrap());
        assert!(!recovery_path(&f.path, &action).unwrap().exists());
        with_locked_session_store(&f.path, |s| {
            s.sessions.get_mut(f.id.as_str()).unwrap().runtime_pid = Some(1234);
            save_locked_session_store(&f.path, s)
        })
        .unwrap();
        assert!(f
            .provider
            .recover_failed_runtime(
                &HumanManagementPrincipal::authenticated(),
                &f.path,
                &f.id,
                &action,
                &mut Executor { busy: false }
            )
            .is_err());
    }
    #[test]
    fn prepared_recovery_replay_does_not_clear_a_later_claim() {
        let f = fixture();
        let action = AgentActionId::new("interrupted-human-recovery").unwrap();
        let prepared = HumanRuntimeRecovery {
            action_id: action.clone(),
            cutex_session_id: f.id.clone(),
            original_action_id: f.original.action_id.clone(),
            claim_id: f.original.claim_id.clone(),
            completed: false,
        };
        write_private_pretty_json_atomic(
            &recovery_path(&f.path, &action).unwrap(),
            &prepared,
            "test",
        )
        .unwrap();
        with_locked_session_store(&f.path, |s| {
            let r = s.sessions.get_mut(f.id.as_str()).unwrap();
            r.app_server_launch_claim_id = Some("new-claim".into());
            r.runtime_pid = Some(5678);
            save_locked_session_store(&f.path, s)
        })
        .unwrap();
        let before = std::fs::read(&f.path).unwrap();
        assert!(
            f.provider
                .recover_failed_runtime(
                    &HumanManagementPrincipal::authenticated(),
                    &f.path,
                    &f.id,
                    &action,
                    &mut Executor { busy: true }
                )
                .unwrap()
                .completed
        );
        assert_eq!(before, std::fs::read(&f.path).unwrap());
    }
}
