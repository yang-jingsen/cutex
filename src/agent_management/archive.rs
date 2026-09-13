//! Reversible Human Archive. Permanent roster Close is a separate operation.
use super::*;
use crate::management::control_plane::HumanManagementPrincipal;
use crate::role_revision::{CutexSessionId, Sha256};
use crate::session::{archive::record_has_runtime_claim, model::CutexSessionRecord};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentArchiveOperation {
    Archive,
    Restore,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentArchiveReviewRequest {
    pub cutex_session_id: CutexSessionId,
    pub operation: AgentArchiveOperation,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::{
        load_cutex_session_store_from_path, save_cutex_session_store_to_path,
    };
    struct Offline {
        stops: usize,
        reject: bool,
    }
    impl AgentArchiveRuntime for Offline {
        fn prepare(&mut self, _: &CutexSessionRecord, _: bool) -> anyhow::Result<()> {
            anyhow::ensure!(!self.reject, "unsupported_stop_proof");
            Ok(())
        }
        fn stop_and_verify(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
            self.stops += 1;
            Ok(())
        }
        fn verify_offline(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
            Ok(())
        }
    }
    struct Fixture {
        root: std::path::PathBuf,
        path: std::path::PathBuf,
        provider: AgentManagementProvider,
        tasks: crate::task_service::TaskServiceProvider,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("cutex-archive-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("cutex-sessions.json");
            let provider = AgentManagementProvider::open(root.join("provider")).unwrap();
            let tasks = crate::task_service::TaskServiceProvider::open(root.join("tasks")).unwrap();
            let mut sessions = crate::session::model::CutexSessionStore::default();
            let mut record = CutexSessionRecord::new_at(
                "cutex.archive-test".into(),
                Some("native-test".into()),
                crate::platform::host::current_host_name(),
                root.to_string_lossy().into_owned(),
                None,
                "2026-09-09T00:00:00Z".into(),
            )
            .unwrap();
            record.agent_enabled = true;
            record.registration_class = crate::agent_bus::model::AgentRegistrationClass::Persistent;
            record.formal_agent_name = Some("Formal Agent".into());
            record.thread_name = Some("Never a formal name".into());
            sessions
                .sessions
                .insert(record.cutex_session_id.clone(), record);
            save_cutex_session_store_to_path(&path, &sessions).unwrap();
            Self {
                root,
                path,
                provider,
                tasks,
            }
        }
        fn request(&self, operation: AgentArchiveOperation, action: &str) -> AgentArchiveRequest {
            AgentArchiveRequest {
                reason: None,
                action_id: AgentActionId::new(action).unwrap(),
                review: self
                    .provider
                    .review_agent_archive(
                        &HumanManagementPrincipal::authenticated(),
                        &self.path,
                        &CutexSessionId::new("cutex.archive-test").unwrap(),
                        operation,
                    )
                    .unwrap(),
            }
        }
        fn execute(
            &self,
            request: &AgentArchiveRequest,
            runtime: &mut Offline,
        ) -> Result<AgentArchiveReceipt, AgentManagementError> {
            self.provider.execute_agent_archive(
                &HumanManagementPrincipal::authenticated(),
                &self.path,
                request,
                &self.tasks,
                runtime,
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn d1r2_archive_restore_identity_and_exact_historical_replay() {
        let f = Fixture::new();
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        let request = f.request(AgentArchiveOperation::Archive, "archive-test");
        let receipt = f.execute(&request, &mut runtime).unwrap();
        assert_eq!(receipt.stage, AgentArchiveStage::Committed);
        assert!(receipt.result.as_ref().unwrap().is_retired());
        let restore = f.request(AgentArchiveOperation::Restore, "restore-test");
        let restored = f.execute(&restore, &mut runtime).unwrap();
        let record = restored.result.unwrap();
        assert!(record.is_active());
        assert!(!record_has_runtime_claim(&record));
        assert_eq!(record.codex_session_id.as_deref(), Some("native-test"));
        assert_eq!(record.formal_agent_name.as_deref(), Some("Formal Agent"));
        assert_eq!(record.profile, None);
        assert_eq!(f.execute(&request, &mut runtime).unwrap(), receipt);
        assert_eq!(runtime.stops, 1);
        assert!(f.provider.store().snapshot().unwrap().agents.is_empty());
        let mut changed = request.clone();
        changed.review.formal_name = "changed".into();
        assert!(f.execute(&changed, &mut runtime).is_err());
    }

    #[test]
    fn d1r2_unsupported_and_stale_confirmation_have_no_archive_effect() {
        let f = Fixture::new();
        let request = f.request(AgentArchiveOperation::Archive, "archive-unsupported");
        let before = std::fs::read(&f.path).unwrap();
        let mut runtime = Offline {
            stops: 0,
            reject: true,
        };
        assert!(f.execute(&request, &mut runtime).is_err());
        assert_eq!(std::fs::read(&f.path).unwrap(), before);
        assert!(f
            .provider
            .store()
            .snapshot()
            .unwrap()
            .agent_archive_actions
            .is_empty());
        let mut sessions = load_cutex_session_store_from_path(&f.path).unwrap();
        sessions
            .sessions
            .get_mut("cutex.archive-test")
            .unwrap()
            .profile = Some("new-config".into());
        save_cutex_session_store_to_path(&f.path, &sessions).unwrap();
        runtime.reject = false;
        assert!(f.execute(&request, &mut runtime).is_err());
        assert_eq!(runtime.stops, 0);
    }

    #[test]
    fn d1r2_public_task_provider_assignment_blocks_durable_only_archive() {
        use crate::task_service::*;
        use sha2::{Digest, Sha256 as Hash};
        let f = Fixture::new();
        let request = f.request(AgentArchiveOperation::Archive, "task-protected-archive");
        let director = AuthenticatedPrincipal::seated_session(
            CutexSessionId::new("cutex.director").unwrap(),
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let task_id = crate::role_revision::TaskId::new("archive-task").unwrap();
        let revision = crate::role_revision::TaskRevision::new(1).unwrap();
        f.tasks
            .create_revision(
                &director,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-archive-task").unwrap(),
                    workflow_id: WorkflowId::new("archive-workflow").unwrap(),
                    task_id: task_id.clone(),
                    task_revision: revision,
                    contract_sha256: Sha256::new(format!("{:x}", Hash::digest(b"contract")))
                        .unwrap(),
                    opaque_contract: "contract".into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: SeatId::new("release").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        f.tasks
            .assign_and_dispatch(
                &director,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign-archive-task").unwrap(),
                    assignment_id: AssignmentId::new("archive-assignment").unwrap(),
                    task_id,
                    task_revision: revision,
                    assignee_cutex_session: request.review.cutex_session_id.clone(),
                    send_attempt_id: SendAttemptId::new("archive-send").unwrap(),
                    external_message_id: "archive-message".into(),
                },
                1,
                "assignment",
            )
            .unwrap();
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        assert!(matches!(
            f.execute(&request, &mut runtime),
            Err(AgentManagementError::Conflict(
                "archive_requires_active_task_resolution"
            ))
        ));
        assert_eq!(runtime.stops, 0);
        assert!(load_cutex_session_store_from_path(&f.path)
            .unwrap()
            .sessions["cutex.archive-test"]
            .is_active());
    }

    #[test]
    fn restore_allows_pending_assignment_for_archived_agent() {
        use crate::task_service::*;
        use sha2::{Digest, Sha256 as Hash};
        let f = Fixture::new();
        let archive = f.request(AgentArchiveOperation::Archive, "before-assignment");
        f.execute(&archive, &mut Offline { stops:0, reject:false }).unwrap();
        let request = f.request(AgentArchiveOperation::Restore, "restore-with-assignment");
        let director = AuthenticatedPrincipal::seated_session(
            CutexSessionId::new("cutex.director").unwrap(),
            SeatId::new("director").unwrap(),
            1,
        )
        .unwrap();
        let task_id = crate::role_revision::TaskId::new("archive-task").unwrap();
        let revision = crate::role_revision::TaskRevision::new(1).unwrap();
        f.tasks
            .create_revision(
                &director,
                &CreateRevisionRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("create-archive-task").unwrap(),
                    workflow_id: WorkflowId::new("archive-workflow").unwrap(),
                    task_id: task_id.clone(),
                    task_revision: revision,
                    contract_sha256: Sha256::new(format!("{:x}", Hash::digest(b"contract")))
                        .unwrap(),
                    opaque_contract: "contract".into(),
                    completion_policy: CompletionPolicy {
                        kind: CompletionPolicyKind::ReleaseReview,
                        authority_seat_id: SeatId::new("release").unwrap(),
                    },
                },
                None,
            )
            .unwrap();
        f.tasks
            .assign_and_dispatch(
                &director,
                &AssignAndDispatchRequest {
                    schema: ProviderActionSchema::V2,
                    action_id: ActionId::new("assign-archive-task").unwrap(),
                    assignment_id: AssignmentId::new("archive-assignment").unwrap(),
                    task_id,
                    task_revision: revision,
                    assignee_cutex_session: request.review.cutex_session_id.clone(),
                    send_attempt_id: SendAttemptId::new("archive-send").unwrap(),
                    external_message_id: "archive-message".into(),
                },
                1,
                "assignment",
            )
            .unwrap();
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        f.execute(&request, &mut runtime).unwrap();
        assert_eq!(runtime.stops, 0);
        assert!(load_cutex_session_store_from_path(&f.path)
            .unwrap()
            .sessions["cutex.archive-test"]
            .is_active());
    }

    #[test]
    fn d1r2_operator_and_director_guards_apply_without_roster_membership() {
        let f = Fixture::new();
        let request = f.request(AgentArchiveOperation::Archive, "role-protected-archive");
        let project = ProjectId::new("project").unwrap();
        f.provider
            .store()
            .with_state(true, |mut state| {
                state.projects.insert(
                    project.clone(),
                    ProjectAuthority {
                        project_id: project.clone(),
                        authorized_director_session: request.review.cutex_session_id.clone(),
                        authority_epoch: 1,
                        updated_at: super::super::store::now(),
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        let seat_action = crate::task_service::ActionId::new("runtime-director-seat").unwrap();
        f.provider
            .director_seats
            .prepare_project_director(&seat_action, &project, &request.review.cutex_session_id)
            .unwrap();
        f.provider
            .director_seats
            .activate_project_director(&seat_action, &project, &request.review.cutex_session_id)
            .unwrap();
        let state = f.provider.store().snapshot().unwrap();
        assert_eq!(
            runtime_guard(&state, &request.review.cutex_session_id).unwrap(),
            Some(project.clone())
        );
        assert!(f
            .provider
            .runtime_authority_digest(&state, &request.review.cutex_session_id)
            .is_ok());
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        assert!(matches!(
            f.execute(&request, &mut runtime),
            Err(AgentManagementError::Conflict(
                "archive_requires_explicit_director_rotation"
            ))
        ));
        f.provider
            .store()
            .with_state(true, |mut state| {
                let director = CutexSessionId::new("cutex.other-director").unwrap();
                state
                    .projects
                    .get_mut(&project)
                    .unwrap()
                    .authorized_director_session = director.clone();
                state
                    .operator_grants
                    .entry(project.clone())
                    .or_default()
                    .insert(
                        request.review.cutex_session_id.clone(),
                        AgentOperatorGrant {
                            project_id: project.clone(),
                            operator_cutex_session_id: request.review.cutex_session_id.clone(),
                            grant_revision: 1,
                            granted_at: super::super::store::now(),
                            granted_by_primary_director_session: director,
                            performed_by_human_management: true,
                        },
                    );
                Ok((state, (), true))
            })
            .unwrap();
        assert!(matches!(
            f.execute(&request, &mut runtime),
            Err(AgentManagementError::Conflict(
                "archive_requires_explicit_operator_revoke"
            ))
        ));
        assert_eq!(runtime.stops, 0);
    }

    #[test]
    fn d1r2_stopped_failure_is_not_success_and_atomic_receipt_recovers() {
        struct FailedProof;
        impl AgentArchiveRuntime for FailedProof {
            fn prepare(&mut self, _: &CutexSessionRecord, _: bool) -> anyhow::Result<()> {
                Ok(())
            }
            fn stop_and_verify(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
                Ok(())
            }
            fn verify_offline(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
                anyhow::bail!("injected post-stop observation failure")
            }
        }
        let f = Fixture::new();
        let request = f.request(AgentArchiveOperation::Archive, "post-stop-failure");
        let receipt = f
            .provider
            .execute_agent_archive(
                &HumanManagementPrincipal::authenticated(),
                &f.path,
                &request,
                &f.tasks,
                &mut FailedProof,
            )
            .unwrap();
        assert_eq!(receipt.stage, AgentArchiveStage::StoppedNotArchived);
        assert!(load_cutex_session_store_from_path(&f.path)
            .unwrap()
            .sessions["cutex.archive-test"]
            .is_active());
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        assert_eq!(f.execute(&request, &mut runtime).unwrap(), receipt);
        assert_eq!(runtime.stops, 0);
        let next = f.request(AgentArchiveOperation::Archive, "atomic-success");
        let committed = f.execute(&next, &mut runtime).unwrap();
        // Simulate loss before the provider's final journal write, while the
        // durable transition+receipt survived. No historical result relabeling.
        f.provider
            .store()
            .with_state(true, |mut state| {
                state.agent_archive_actions.remove(&next.action_id);
                Ok((state, (), true))
            })
            .unwrap();
        let mut sessions = load_cutex_session_store_from_path(&f.path).unwrap();
        sessions
            .sessions
            .get_mut("cutex.archive-test")
            .unwrap()
            .formal_agent_name = Some("New current name".into());
        save_cutex_session_store_to_path(&f.path, &sessions).unwrap();
        assert_eq!(f.execute(&next, &mut runtime).unwrap(), committed);
        assert_eq!(runtime.stops, 1);
    }

    #[test]
    fn d1r2_corrupt_sources_do_not_become_empty_success() {
        let f = Fixture::new();
        let request = f.request(AgentArchiveOperation::Archive, "corrupt-source");
        std::fs::write(&f.path, b"{broken").unwrap();
        let mut runtime = Offline {
            stops: 0,
            reject: false,
        };
        assert!(f.execute(&request, &mut runtime).is_err());
        assert_eq!(runtime.stops, 0);
    }

    #[test]
    fn d1r2_imported_member_missing_project_and_permanent_history_conflict() {
        let f = Fixture::new();
        let mut sessions = load_cutex_session_store_from_path(&f.path).unwrap();
        sessions
            .sessions
            .get_mut("cutex.archive-test")
            .unwrap()
            .agent_groups = vec!["test".into()];
        save_cutex_session_store_to_path(&f.path, &sessions).unwrap();
        let principal = HumanManagementPrincipal::authenticated();
        let candidate = f
            .provider
            .durable_agent_candidates(&principal, &f.path)
            .unwrap()
            .remove(0);
        f.provider
            .import_durable_agent(
                &principal,
                &f.path,
                &DurableImportRequest {
                    action_id: AgentActionId::new("import-archive-member").unwrap(),
                    candidate,
                    confirmed_formal_name: "Formal Agent".into(),
                    assignment: None,
                    detach: None,
                },
                &|_: &ProjectId, _: Option<&CutexSessionId>| Ok(false),
            )
            .unwrap();
        let id = CutexSessionId::new("cutex.archive-test").unwrap();
        f.provider
            .store()
            .with_state(true, |mut state| {
                state.current_project_memberships.insert(
                    id.clone(),
                    CurrentProjectMembership {
                        cutex_session_id: id.clone(),
                        project_id: Some(ProjectId::new("missing-project").unwrap()),
                        revision: 1,
                        updated_at: super::super::store::now(),
                    },
                );
                Ok((state, (), true))
            })
            .unwrap();
        assert!(matches!(
            f.provider.review_agent_archive(
                &principal,
                &f.path,
                &id,
                AgentArchiveOperation::Archive
            ),
            Err(AgentManagementError::Conflict(
                "archive_current_project_missing"
            ))
        ));
        f.provider
            .store()
            .with_state(true, |mut state| {
                state.agents.get_mut(&id).unwrap().retired_at = Some(super::super::store::now());
                Ok((state, (), true))
            })
            .unwrap();
        let before = serde_json::to_value(f.provider.store().snapshot().unwrap()).unwrap();
        assert!(matches!(
            f.provider.review_agent_archive(
                &principal,
                &f.path,
                &id,
                AgentArchiveOperation::Restore
            ),
            Err(AgentManagementError::Conflict(
                "permanent_retirement_is_not_restorable"
            ))
        ));
        assert_eq!(
            serde_json::to_value(f.provider.store().snapshot().unwrap()).unwrap(),
            before
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn d1r2_real_owned_process_tree_stop_before_archive_preserves_unrelated_process() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        struct ProcessOracle {
            root: u32,
            child: u32,
            reaper: Option<std::thread::JoinHandle<std::process::ExitStatus>>,
            unrelated: std::process::Child,
        }
        impl AgentArchiveRuntime for ProcessOracle {
            fn prepare(&mut self, record: &CutexSessionRecord, _: bool) -> anyhow::Result<()> {
                anyhow::ensure!(
                    record.runtime_pid == Some(self.root) && record.runtime_generation == 1,
                    "oracle occurrence mismatch"
                );
                anyhow::ensure!(
                    unsafe { libc::getpgid(self.root as i32) } == self.root as i32,
                    "oracle group mismatch"
                );
                Ok(())
            }
            fn stop_and_verify(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
                anyhow::ensure!(
                    unsafe { libc::kill(-(self.root as i32), libc::SIGTERM) } == 0,
                    "owned group stop failed"
                );
                self.reaper.take().unwrap().join().unwrap();
                Ok(())
            }
            fn verify_offline(&mut self, _: &CutexSessionRecord) -> anyhow::Result<()> {
                anyhow::ensure!(
                    !crate::platform::process::process_is_running(self.root)
                        && !crate::platform::process::process_is_running(self.child),
                    "owned tree survived"
                );
                anyhow::ensure!(
                    self.unrelated.try_wait()?.is_none(),
                    "unrelated process disturbed"
                );
                Ok(())
            }
        }
        impl Drop for ProcessOracle {
            fn drop(&mut self) {
                if let Some(reaper) = self.reaper.take() {
                    unsafe {
                        libc::kill(-(self.root as i32), libc::SIGKILL);
                    }
                    let _ = reaper.join();
                }
                let _ = self.unrelated.kill();
                let _ = self.unrelated.wait();
            }
        }
        let f = Fixture::new();
        let mut root = Command::new("python3").process_group(0).args(["-c",
            "import os,time,signal\npid=os.fork()\nif pid == 0:\n time.sleep(30)\n os._exit(0)\ndef stop(*args):\n os.waitpid(pid,0)\n os._exit(0)\nsignal.signal(signal.SIGTERM,stop)\nprint(pid,flush=True)\ntime.sleep(30)"])
            .stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(root.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let root_pid = root.id();
        let mut oracle = ProcessOracle {
            root: root_pid,
            child: line.trim().parse().unwrap(),
            reaper: Some(std::thread::spawn(move || root.wait().unwrap())),
            unrelated: Command::new("sleep").arg("30").spawn().unwrap(),
        };
        let mut sessions = load_cutex_session_store_from_path(&f.path).unwrap();
        let record = sessions.sessions.get_mut("cutex.archive-test").unwrap();
        record.runtime_pid = Some(root_pid);
        record.runtime_generation = 1;
        save_cutex_session_store_to_path(&f.path, &sessions).unwrap();
        let request = f.request(AgentArchiveOperation::Archive, "real-tree-archive");
        let receipt = f
            .provider
            .execute_agent_archive(
                &HumanManagementPrincipal::authenticated(),
                &f.path,
                &request,
                &f.tasks,
                &mut oracle,
            )
            .unwrap();
        assert_eq!(receipt.stage, AgentArchiveStage::Committed);
        assert!(receipt.result.unwrap().is_retired());
        assert!(oracle.unrelated.try_wait().unwrap().is_none());
    }
}

/// Immutable confirmation evidence, never refreshed by execute.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentArchiveReview {
    pub cutex_session_id: CutexSessionId,
    pub formal_name: String,
    pub operation: AgentArchiveOperation,
    pub durable_sha256: Sha256,
    pub authority_sha256: Sha256,
    pub current_project_id: Option<ProjectId>,
    pub revision: u64,
    pub runtime_generation: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentArchiveRequest {
    #[serde(default)]
    pub reason: Option<String>,
    pub action_id: AgentActionId,
    pub review: AgentArchiveReview,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentArchiveStage {
    Prepared,
    Stopped,
    Committed,
    StoppedNotArchived,
    Uncertain,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentArchiveReceipt {
    pub request: AgentArchiveRequest,
    pub request_sha256: Sha256,
    pub stage: AgentArchiveStage,
    pub result: Option<CutexSessionRecord>,
    pub error: Option<String>,
    pub committed_at: crate::role_revision::Rfc3339,
}

/// Runtime proof is acquired before any stop or journal write. Implementations
/// must bind containment to this exact occurrence, not just a scope name/PID.
pub trait AgentArchiveRuntime {
    fn prepare(&mut self, record: &CutexSessionRecord, restoring: bool) -> anyhow::Result<()>;
    fn stop_and_verify(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()>;
    fn verify_offline(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()>;
}

fn conflict(code: &'static str) -> AgentManagementError {
    AgentManagementError::Conflict(code)
}

pub(super) fn authority_digest(
    state: &AgentManagementSnapshot,
    id: &CutexSessionId,
) -> Result<Sha256, AgentManagementError> {
    // Roles may exist without a roster row. Inspect all explicit authorities.
    let project = state
        .agents
        .get(id)
        .and_then(|a| current_project_id(state, a));
    super::store::request_sha256(&(
        state.agents.get(id),
        state.current_project_memberships.get(id),
        project.as_ref().and_then(|p| state.projects.get(p)),
        project.as_ref().and_then(|p| state.project_states.get(p)),
        project
            .as_ref()
            .and_then(|p| state.operator_grant_revisions.get(p)),
        state
            .projects
            .values()
            .filter(|p| &p.authorized_director_session == id)
            .collect::<Vec<_>>(),
        state
            .operator_grants
            .iter()
            .filter_map(|(p, g)| g.get(id).map(|g| (p, g)))
            .collect::<Vec<_>>(),
    ))
}

/// Runtime start/restart protects the current authority topology without
/// treating the authority holder itself as an archive target. A seated
/// Director or granted Operator is allowed to run; retirement and broken
/// project membership still fail closed.
pub(super) fn runtime_guard(
    state: &AgentManagementSnapshot,
    id: &CutexSessionId,
) -> Result<Option<ProjectId>, AgentManagementError> {
    let agent = state.agents.get(id);
    if agent.is_some_and(|a| a.retired_at.is_some()) {
        return Err(conflict("permanent_retirement_is_not_runnable"));
    }
    let roster_project = agent.and_then(|a| current_project_id(state, a));
    let director_projects = state
        .projects
        .iter()
        .filter_map(|(project, authority)| {
            (&authority.authorized_director_session == id).then_some(project)
        })
        .collect::<Vec<_>>();
    if director_projects.len() > 1 {
        return Err(conflict("director_authorized_for_multiple_projects"));
    }
    let director_project = director_projects.first().copied().cloned();
    if roster_project.is_some() && director_project.is_some() && roster_project != director_project
    {
        return Err(conflict("runtime_project_authority_mismatch"));
    }
    let project = roster_project.or(director_project);
    if let Some(project) = &project {
        if !state.projects.contains_key(project) {
            return Err(conflict("runtime_current_project_missing"));
        }
        if state.project_tombstones.contains_key(project)
            || state
                .project_states
                .get(project)
                .is_some_and(|p| p.lifecycle != ProjectLifecycle::Active)
        {
            return Err(conflict("restore_current_project_before_agent_lifecycle"));
        }
    }
    Ok(project)
}

pub(super) fn guard(
    state: &AgentManagementSnapshot,
    id: &CutexSessionId,
) -> Result<Option<ProjectId>, AgentManagementError> {
    if state
        .projects
        .values()
        .any(|p| &p.authorized_director_session == id)
    {
        return Err(conflict("archive_requires_explicit_director_rotation"));
    }
    if state.operator_grants.values().any(|g| g.contains_key(id)) {
        return Err(conflict("archive_requires_explicit_operator_revoke"));
    }
    let agent = state.agents.get(id);
    if agent.is_some_and(|a| a.retired_at.is_some()) {
        return Err(conflict("permanent_retirement_is_not_restorable"));
    }
    let project = agent.and_then(|a| current_project_id(state, a));
    if let Some(project) = &project {
        if !state.projects.contains_key(project) {
            return Err(conflict("archive_current_project_missing"));
        }
        if state.project_tombstones.contains_key(project)
            || state
                .project_states
                .get(project)
                .is_some_and(|p| p.lifecycle != ProjectLifecycle::Active)
        {
            return Err(conflict("restore_current_project_before_agent_lifecycle"));
        }
    }
    Ok(project)
}

impl AgentManagementProvider {
    pub(super) fn runtime_authority_digest(
        &self,
        state: &AgentManagementSnapshot,
        id: &CutexSessionId,
    ) -> Result<Sha256, AgentManagementError> {
        let project = runtime_guard(state, id)?;
        let seats = self
            .director_seats
            .query()
            .map_err(super::provider::seat_authority_error)?;
        if let Some(project) = &project {
            if seats
                .active_project_director_transfers
                .contains_key(project)
            {
                return Err(conflict("runtime_project_authority_transfer_in_progress"));
            }
            if let Some(authority) = state.projects.get(project) {
                let seat = seats.project_director_occupancies.get(project);
                if &authority.authorized_director_session == id
                    && seat.is_none_or(|seat| &seat.occupant_cutex_session != id)
                {
                    return Err(conflict("runtime_director_seat_mismatch"));
                }
                if let Some(seat) = seat {
                    // Legacy seat stores omit this field; the seat subsystem
                    // canonically interprets an absent state as Active.
                    let seat_state = seats
                        .project_director_states
                        .get(project)
                        .copied()
                        .unwrap_or(crate::seat::ProjectDirectorSeatState::Active);
                    if authority.authorized_director_session != seat.occupant_cutex_session
                        || seat_state != crate::seat::ProjectDirectorSeatState::Active
                    {
                        return Err(conflict("runtime_project_authority_incompatible"));
                    }
                }
            }
        }
        super::store::request_sha256(&(
            authority_digest(state, id)?,
            project
                .as_ref()
                .and_then(|p| seats.project_director_occupancies.get(p)),
            project
                .as_ref()
                .and_then(|p| seats.project_director_states.get(p)),
        ))
    }

    pub(super) fn archive_authority_digest(
        &self,
        state: &AgentManagementSnapshot,
        id: &CutexSessionId,
    ) -> Result<Sha256, AgentManagementError> {
        let seats = self
            .director_seats
            .query()
            .map_err(super::provider::seat_authority_error)?;
        let project = state
            .agents
            .get(id)
            .and_then(|a| current_project_id(state, a));
        if seats
            .project_director_occupancies
            .values()
            .any(|s| &s.occupant_cutex_session == id)
        {
            return Err(conflict("archive_requires_explicit_director_rotation"));
        }
        if let Some(project) = &project {
            if seats
                .active_project_director_transfers
                .contains_key(project)
            {
                return Err(conflict("archive_project_authority_transfer_in_progress"));
            }
            if let Some(seat) = seats.project_director_occupancies.get(project) {
                if state
                    .projects
                    .get(project)
                    .is_none_or(|a| a.authorized_director_session != seat.occupant_cutex_session)
                    || seats
                        .project_director_states
                        .get(project)
                        .is_some_and(|s| *s != crate::seat::ProjectDirectorSeatState::Active)
                {
                    return Err(conflict("archive_project_authority_incompatible"));
                }
            }
        }
        super::store::request_sha256(&(
            authority_digest(state, id)?,
            project
                .as_ref()
                .and_then(|p| seats.project_director_occupancies.get(p)),
            project
                .as_ref()
                .and_then(|p| seats.project_director_states.get(p)),
        ))
    }
    pub fn review_agent_archive(
        &self,
        _principal: &HumanManagementPrincipal,
        path: &Path,
        id: &CutexSessionId,
        operation: AgentArchiveOperation,
    ) -> Result<AgentArchiveReview, AgentManagementError> {
        let _mutation = self.store().lock_mutations()?;
        let state = self.store().snapshot()?;
        let project = guard(&state, id)?;
        let sessions = crate::session::store::load_cutex_session_store_from_path(path)
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let record = sessions
            .sessions
            .get(id.as_str())
            .ok_or(conflict("durable_agent_missing"))?;
        if record.cutex_session_id != id.as_str()
            || !record.agent_enabled
            || record.registration_class
                != crate::agent_bus::model::AgentRegistrationClass::Persistent
        {
            return Err(conflict("requires_exact_persistent_durable_agent"));
        }
        if record.is_retired() != (operation == AgentArchiveOperation::Restore) {
            return Err(conflict("archive_state_conflict"));
        }
        if record.app_server_launch_claim_id.is_some() {
            return Err(conflict("archive_runtime_launch_unresolved"));
        }
        let formal_name = record
            .formal_agent_name
            .clone()
            .or_else(|| state.agents.get(id).map(|a| a.spec.name.clone()))
            .ok_or(conflict("explicit_formal_agent_name_required"))?;
        if formal_name.trim().is_empty() || formal_name.chars().any(char::is_control) {
            return Err(conflict("malformed_formal_agent_name"));
        }
        Ok(AgentArchiveReview {
            cutex_session_id: id.clone(),
            formal_name,
            operation,
            durable_sha256: super::store::request_sha256(record)?,
            authority_sha256: self.archive_authority_digest(&state, id)?,
            current_project_id: project,
            revision: record.revision,
            runtime_generation: record.runtime_generation,
        })
    }

    /// Holds the Task Service read fence for the duration of this
    /// operation. Every non-closed assignment, including project-less ones,
    /// protects the exact durable identity.
    pub fn execute_agent_archive(
        &self,
        _principal: &HumanManagementPrincipal,
        path: &Path,
        request: &AgentArchiveRequest,
        tasks: &crate::task_service::TaskServiceProvider,
        runtime: &mut dyn AgentArchiveRuntime,
    ) -> Result<AgentArchiveReceipt, AgentManagementError> {
        let _execution = super::provider::provider_execution_lock()
            .lock()
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        let _mutation = self.store().lock_mutations()?;
        let digest = super::store::request_sha256(request)?;
        if let Some(receipt) = self
            .store()
            .snapshot()?
            .agent_archive_actions
            .get(&request.action_id)
        {
            if receipt.request_sha256 != digest {
                return Err(conflict("archive_action_payload_conflict"));
            }
            if matches!(
                receipt.stage,
                AgentArchiveStage::Committed | AgentArchiveStage::StoppedNotArchived
            ) {
                return Ok(receipt.clone());
            }
        }
        let committed = crate::session::store::with_locked_session_store(path, |sessions| {
            Ok(sessions
                .agent_archive_receipts
                .get(request.action_id.as_str())
                .cloned())
        })
        .map_err(|_| AgentManagementError::PersistenceUnavailable)?;
        if let Some(receipt) = committed {
            if receipt.request_sha256 != digest {
                return Err(conflict("archive_action_payload_conflict"));
            }
            self.save_agent_archive_receipt(&receipt)?;
            return Ok(receipt);
        }
        tasks
            .with_archive_read_fence(|tasks| {
                self.execute_agent_archive_locked(path, request, tasks, runtime)
            })
            .map_err(|_| AgentManagementError::PersistenceUnavailable)?
    }

    fn execute_agent_archive_locked(
        &self,
        path: &Path,
        request: &AgentArchiveRequest,
        tasks: &crate::task_service::TaskServiceSnapshot,
        runtime: &mut dyn AgentArchiveRuntime,
    ) -> Result<AgentArchiveReceipt, AgentManagementError> {
        let state = self.store().snapshot()?;
        let digest = super::store::request_sha256(request)?;
        if let Some(receipt) = state.agent_archive_actions.get(&request.action_id) {
            if receipt.request_sha256 != digest {
                return Err(conflict("archive_action_payload_conflict"));
            }
            if matches!(
                receipt.stage,
                AgentArchiveStage::Committed | AgentArchiveStage::StoppedNotArchived
            ) {
                return Ok(receipt.clone());
            }
        }
        let id = &request.review.cutex_session_id;
        guard(&state, id)?;
        if self.archive_authority_digest(&state, id)? != request.review.authority_sha256 {
            return Err(conflict("archive_authority_confirmation_stale"));
        }
        if request.review.operation == AgentArchiveOperation::Archive && tasks.assignments.values().any(|a| {
            &a.assignee_cutex_session == id
                && a.state != crate::task_service::AssignmentState::Closed
        }) {
            return Err(conflict("archive_requires_active_task_resolution"));
        }
        let outcome = crate::session::store::with_locked_session_store(path, |sessions| {
            // Final receipt and durable transition are one atomic file write.
            if let Some(committed) = sessions
                .agent_archive_receipts
                .get(request.action_id.as_str())
            {
                anyhow::ensure!(
                    committed.request_sha256 == digest,
                    "archive_action_payload_conflict"
                );
                self.save_agent_archive_receipt(committed)?;
                return Ok(committed.clone());
            }
            let before = sessions
                .sessions
                .get(id.as_str())
                .ok_or_else(|| anyhow::anyhow!("durable_agent_missing"))?
                .clone();
            anyhow::ensure!(
                super::store::request_sha256(&before)? == request.review.durable_sha256,
                "archive_durable_confirmation_stale"
            );
            let name = before
                .formal_agent_name
                .as_ref()
                .or_else(|| state.agents.get(id).map(|a| &a.spec.name));
            anyhow::ensure!(
                before.cutex_session_id == id.as_str()
                    && name == Some(&request.review.formal_name)
                    && before.revision == request.review.revision
                    && before.runtime_generation == request.review.runtime_generation
                    && before.agent_enabled
                    && before.registration_class
                        == crate::agent_bus::model::AgentRegistrationClass::Persistent
                    && state
                        .agents
                        .get(id)
                        .and_then(|a| current_project_id(&state, a))
                        == request.review.current_project_id,
                "archive_confirmation_evidence_mismatch"
            );
            let restoring = request.review.operation == AgentArchiveOperation::Restore;
            anyhow::ensure!(
                before.app_server_launch_claim_id.is_none(),
                "archive_runtime_launch_unresolved"
            );
            if restoring {
                crate::session::archive::validate_restore_preconditions(
                    &before,
                    request.review.revision,
                )?;
            } else {
                crate::session::archive::validate_retire_preconditions(
                    &before,
                    request.review.revision,
                    request.review.runtime_generation,
                )?;
            }
            let native = before
                .codex_session_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("durable_native_identity_missing"))?;
            anyhow::ensure!(
                crate::session::identity::normalize_codex_session_id(native)
                    .ok()
                    .as_deref()
                    == Some(native)
                    && sessions
                        .sessions
                        .values()
                        .filter(|r| r.codex_session_id.as_deref() == Some(native))
                        .count()
                        == 1,
                "ambiguous_or_malformed_durable_native_identity"
            );
            runtime.prepare(&before, restoring)?;
            let mut receipt = AgentArchiveReceipt {
                request: request.clone(),
                request_sha256: digest,
                stage: AgentArchiveStage::Prepared,
                result: None,
                error: None,
                committed_at: super::store::now(),
            };
            self.save_agent_archive_receipt(&receipt)?;
            if !restoring {
                if let Err(error) = runtime.stop_and_verify(&before) {
                    receipt.stage = AgentArchiveStage::Uncertain;
                    receipt.error = Some(format!("stop_not_proven: {error:#}"));
                    self.save_agent_archive_receipt(&receipt)?;
                    return Ok(receipt);
                }
                receipt.stage = AgentArchiveStage::Stopped;
                // At this stage result is the exact reviewed occurrence for
                // which containment was proven empty, not an archived result.
                receipt.result = Some(before.clone());
                self.save_agent_archive_receipt(&receipt).map_err(|error| anyhow::anyhow!("stopped_not_archived: stop proof journal unavailable: {error}; replay exact action"))?;
            }
            if let Err(error) = runtime.verify_offline(&before) {
                receipt.stage = AgentArchiveStage::StoppedNotArchived;
                receipt.error = Some(format!("stopped_not_archived: {error:#}"));
                self.save_agent_archive_receipt(&receipt)?;
                return Ok(receipt);
            }
            crate::session::runtime_reconciliation::clear_cutex_session_runtime_record(
                sessions,
                id.as_str(),
                true,
            )?;
            let record = sessions
                .sessions
                .get_mut(id.as_str())
                .expect("locked exact record");
            if restoring {
                crate::session::archive::commit_restore(
                    record,
                    before.revision,
                    receipt.committed_at.as_str().to_string(),
                )?;
            } else {
                crate::session::archive::commit_retire(
                    record,
                    before.revision,
                    before.runtime_generation,
                    true,
                    receipt.committed_at.as_str().to_string(),
                )?;
            }
            anyhow::ensure!(
                !record_has_runtime_claim(record),
                "archive_runtime_claim_remaining"
            );
            receipt.stage = AgentArchiveStage::Committed;
            receipt.result = Some(record.clone());
            sessions
                .agent_archive_receipts
                .insert(request.action_id.to_string(), receipt.clone());
            if let Err(error) = crate::session::store::save_locked_session_store(path, sessions) {
                receipt.stage = AgentArchiveStage::Uncertain;
                receipt.error = Some(format!(
                    "durable_commit_uncertain_replay_same_action: {error:#}"
                ));
            }
            self.save_agent_archive_receipt(&receipt)?;
            Ok(receipt)
        });
        outcome.map_err(|error| AgentManagementError::OwnerActionRequired(error.to_string()))
    }

    fn save_agent_archive_receipt(
        &self,
        receipt: &AgentArchiveReceipt,
    ) -> Result<(), AgentManagementError> {
        self.store().with_state(true, |mut state| {
            let key = format!("{}/{:?}", receipt.request.action_id, receipt.stage);
            state
                .agent_archive_audit
                .entry(key)
                .or_insert_with(|| receipt.clone());
            state
                .agent_archive_actions
                .insert(receipt.request.action_id.clone(), receipt.clone());
            Ok((state, (), true))
        })
    }
}
