use anyhow::{anyhow, Context};

use cutex::agent_bus::client::agent_bus_fetch_agents_if_healthy;
use cutex::agent_bus::model::AgentBusAgent;
use cutex::config::store::load_codez_config;
use cutex::runtime::alden::{cute_alden_sessions, CuteAldenSession};
use cutex::session::model::{CutexSessionRecord, CutexSessionStore, CutexSessionUserAction};
use cutex::session::projection::{
    cutex_session_is_attachable, cutex_session_lifecycle_state_with_agents,
};
use cutex::session::service::cutex_session_display_name;
use cutex::session::store::load_cutex_session_store;

use super::session_tui::SessionTuiIntent;
use super::session_tui_actions::{session_tui_actions_for_record, SessionTuiAction};
use super::{root_wizard, session, session_attach};

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionTuiDispatchPlan {
    ResumeAttach {
        id: String,
        launch_profile: Option<String>,
    },
    AttachExisting {
        key: String,
        name: String,
    },
    TakeoverExisting {
        key: String,
        id: String,
    },
    OpenTui {
        id: String,
        launch_profile: Option<String>,
    },
    Online {
        key: String,
        id: String,
        launch_profile: Option<String>,
    },
    ResumeHere {
        key: String,
        record: CutexSessionRecord,
    },
    ResumeManaged {
        id: String,
    },
    CloseAndRestart {
        id: String,
        launch_profile: Option<String>,
    },
    CloseRuntime {
        id: String,
    },
    RetireSession {
        id: String,
    },
    RestoreSession {
        id: String,
    },
}

impl SessionTuiDispatchPlan {
    fn recorded_user_action(&self) -> Option<(&str, CutexSessionUserAction)> {
        match self {
            Self::AttachExisting { key, .. } => Some((key, CutexSessionUserAction::Attach)),
            Self::TakeoverExisting { key, .. } => Some((key, CutexSessionUserAction::Takeover)),
            Self::Online { key, .. } => Some((key, CutexSessionUserAction::Online)),
            Self::ResumeHere { key, .. } => Some((key, CutexSessionUserAction::ResumeHere)),
            Self::ResumeAttach { .. }
            | Self::OpenTui { .. }
            | Self::ResumeManaged { .. }
            | Self::CloseAndRestart { .. }
            | Self::CloseRuntime { .. }
            | Self::RetireSession { .. }
            | Self::RestoreSession { .. } => None,
        }
    }
}

pub(super) fn dispatch_session_tui_intent(intent: SessionTuiIntent) -> anyhow::Result<()> {
    let output =
        runtime_close_output_for_surface(&intent, SessionTuiDispatchSurface::PostTerminal)?;
    dispatch_session_tui_intent_with_close_output(intent, output)
}

pub(super) fn dispatch_session_tui_intent_in_selector(
    intent: SessionTuiIntent,
) -> anyhow::Result<()> {
    let output = runtime_close_output_for_surface(&intent, SessionTuiDispatchSurface::Selector)?;
    dispatch_session_tui_intent_with_close_output(intent, output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionTuiDispatchSurface {
    PostTerminal,
    Selector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeCloseOutput {
    Print,
    Suppress,
}

fn runtime_close_output_for_surface(
    intent: &SessionTuiIntent,
    surface: SessionTuiDispatchSurface,
) -> anyhow::Result<RuntimeCloseOutput> {
    match surface {
        SessionTuiDispatchSurface::PostTerminal => Ok(RuntimeCloseOutput::Print),
        SessionTuiDispatchSurface::Selector
            if matches!(
                intent.action,
                SessionTuiAction::CloseRuntime
                    | SessionTuiAction::RetireSession
                    | SessionTuiAction::RestoreSession
            ) =>
        {
            Ok(RuntimeCloseOutput::Suppress)
        }
        SessionTuiDispatchSurface::Selector => {
            anyhow::bail!("only Close runtime, Retire session, or Restore session may be dispatched inside the session selector")
        }
    }
}

fn dispatch_session_tui_intent_with_close_output(
    intent: SessionTuiIntent,
    close_output: RuntimeCloseOutput,
) -> anyhow::Result<()> {
    let store = load_cutex_session_store()?;
    let alden_sessions = cute_alden_sessions().unwrap_or_default();
    let config = load_codez_config();
    let live_agents = agent_bus_fetch_agents_if_healthy(&config);
    let plan = dispatch_plan_for_intent(&intent, &store, &alden_sessions, &live_agents)?;
    if !matches!(
        intent.action,
        SessionTuiAction::RetireSession | SessionTuiAction::RestoreSession
    ) {
        root_wizard::set_codez_codex_home()
            .context("Failed to prepare Cutex runtime environment for the selected action")?;
    }
    execute_dispatch_plan(plan, close_output)
}

fn dispatch_plan_for_intent(
    intent: &SessionTuiIntent,
    store: &CutexSessionStore,
    alden_sessions: &[CuteAldenSession],
    live_agents: &[AgentBusAgent],
) -> anyhow::Result<SessionTuiDispatchPlan> {
    let record = store.sessions.get(&intent.key).cloned().ok_or_else(|| {
        anyhow!(
            "selected Cutex session is no longer available: {}",
            intent.key
        )
    })?;
    if record.is_retired() && intent.action != SessionTuiAction::RestoreSession {
        anyhow::bail!(
            "selected Cutex session is retired: {}; refresh the session list before acting",
            intent.key
        );
    }
    let available = session_tui_actions_for_record(&record, alden_sessions, live_agents);
    if intent.action == SessionTuiAction::RestoreSession {
        if !record.is_retired() {
            anyhow::bail!(
                "selected Cutex session is already active; refresh the archive workspace"
            );
        }
        return Ok(SessionTuiDispatchPlan::RestoreSession {
            id: record.cutex_session_id,
        });
    }
    if intent.action == SessionTuiAction::RetireSession
        && !cutex::session::service::cutex_session_is_managed(&record)
    {
        anyhow::bail!("Retire session is only available for managed Cutex sessions");
    }
    if intent.action != SessionTuiAction::RetireSession
        && !available.iter().any(|item| item.action == intent.action)
    {
        anyhow::bail!(
            "{} is no longer available for {}; reopen `cutex tui` to refresh actions",
            intent.action.label(),
            cutex_session_display_name(&record)
        );
    }
    if intent.launch_profile.is_some()
        && !intent.action.supports_launch_profile(
            cutex_session_lifecycle_state_with_agents(&record, alden_sessions, live_agents),
            cutex_session_is_attachable(&record, alden_sessions),
        )
    {
        anyhow::bail!(
            "{} can no longer apply a one-launch profile for {}",
            intent.action.label(),
            cutex_session_display_name(&record)
        );
    }

    let id = record
        .codex_session_id
        .as_deref()
        .unwrap_or(record.cutex_session_id.as_str())
        .to_string();
    let plan = match intent.action {
        SessionTuiAction::ResumeAttach => SessionTuiDispatchPlan::ResumeAttach {
            id,
            launch_profile: intent.launch_profile.clone(),
        },
        SessionTuiAction::AttachExisting => {
            let name = record
                .alden_session_name
                .clone()
                .context("selected Cutex session has no cute-alden session name")?;
            SessionTuiDispatchPlan::AttachExisting {
                key: intent.key.clone(),
                name,
            }
        }
        SessionTuiAction::TakeoverExisting => SessionTuiDispatchPlan::TakeoverExisting {
            key: intent.key.clone(),
            id,
        },
        SessionTuiAction::OpenTui => SessionTuiDispatchPlan::OpenTui {
            id,
            launch_profile: intent.launch_profile.clone(),
        },
        SessionTuiAction::Online => SessionTuiDispatchPlan::Online {
            key: intent.key.clone(),
            id,
            launch_profile: intent.launch_profile.clone(),
        },
        SessionTuiAction::ResumeHere => SessionTuiDispatchPlan::ResumeHere {
            key: intent.key.clone(),
            record,
        },
        SessionTuiAction::ResumeManaged => SessionTuiDispatchPlan::ResumeManaged { id },
        SessionTuiAction::CloseAndRestart => SessionTuiDispatchPlan::CloseAndRestart {
            id,
            launch_profile: intent.launch_profile.clone(),
        },
        SessionTuiAction::CloseRuntime => SessionTuiDispatchPlan::CloseRuntime { id },
        SessionTuiAction::RetireSession => SessionTuiDispatchPlan::RetireSession {
            id: record.cutex_session_id,
        },
        SessionTuiAction::RestoreSession => unreachable!("handled above"),
    };
    Ok(plan)
}

fn execute_dispatch_plan(
    plan: SessionTuiDispatchPlan,
    close_output: RuntimeCloseOutput,
) -> anyhow::Result<()> {
    if let Some((key, action)) = plan.recorded_user_action() {
        session::record_cutex_session_user_action(key, action)?;
    }

    match plan {
        SessionTuiDispatchPlan::ResumeAttach { id, launch_profile } => {
            session::cmd_session_resume_alden_with_profile(&id, launch_profile.as_deref())
        }
        SessionTuiDispatchPlan::AttachExisting { name, .. } => {
            session_attach::cmd_session_attach(&name, false)
        }
        SessionTuiDispatchPlan::TakeoverExisting { id, .. } => session::cmd_session_takeover(&id),
        SessionTuiDispatchPlan::OpenTui { id, launch_profile } => {
            session::cmd_session_foreground_with_profile(&id, launch_profile.as_deref())
        }
        SessionTuiDispatchPlan::Online {
            id, launch_profile, ..
        } => session::cmd_session_online_with_profile(&id, launch_profile.as_deref(), true)
            .map(|_| ()),
        SessionTuiDispatchPlan::ResumeManaged { id } => session::cmd_session_foreground(&id),
        SessionTuiDispatchPlan::ResumeHere { record, .. } => {
            session::cmd_session_resume_foreground(&record, None)
        }
        SessionTuiDispatchPlan::CloseAndRestart { id, launch_profile } => {
            session::cmd_session_close_and_restart_with_profile(
                &id,
                launch_profile.as_deref(),
                true,
            )
            .map(|_| ())
        }
        SessionTuiDispatchPlan::CloseRuntime { id } => match close_output {
            RuntimeCloseOutput::Print => session::cmd_session_close_and_wait(&id).map(|_| ()),
            RuntimeCloseOutput::Suppress => {
                session::cmd_session_close_and_wait_quiet(&id).map(|_| ())
            }
        },
        SessionTuiDispatchPlan::RetireSession { id } => session::retire_session(&id),
        SessionTuiDispatchPlan::RestoreSession { id } => session::restore_session(&id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::session::model::{
        CutexAppServerRuntimeBinding, CutexAppServerTransport, CutexSessionRuntimeBackend,
    };

    fn record(backend: CutexSessionRuntimeBackend) -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            "cutex.dispatch".to_string(),
            Some("019e-dispatch".to_string()),
            "tethys".to_string(),
            "/tmp/dispatch".to_string(),
            Some("aemeath".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("dispatch-agent".to_string());
        record.runtime_backend = backend;
        record.registration_class = AgentRegistrationClass::Persistent;
        record
    }

    fn store_with(record: CutexSessionRecord) -> CutexSessionStore {
        let mut store = CutexSessionStore::default();
        store.sessions.insert("durable-key".to_string(), record);
        store
    }

    fn intent(action: SessionTuiAction) -> SessionTuiIntent {
        SessionTuiIntent {
            key: "durable-key".to_string(),
            action,
            launch_profile: None,
        }
    }

    fn intent_with_profile(action: SessionTuiAction, profile: &str) -> SessionTuiIntent {
        SessionTuiIntent {
            key: "durable-key".to_string(),
            action,
            launch_profile: Some(profile.to_string()),
        }
    }

    #[test]
    fn live_alden_intents_map_to_existing_attach_takeover_and_lifecycle_routes() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.alden_session_name = Some("cutex.dispatch.runtime".to_string());
        record.alden_pid = Some(std::process::id());
        let alden_sessions = vec![CuteAldenSession {
            pid: std::process::id(),
            name: record.alden_session_name.clone(),
        }];
        let store = store_with(record);

        assert_eq!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::ResumeAttach),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("resume-attach plan"),
            SessionTuiDispatchPlan::ResumeAttach {
                id: "019e-dispatch".to_string(),
                launch_profile: None,
            }
        );
        assert_eq!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::AttachExisting),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("attach plan"),
            SessionTuiDispatchPlan::AttachExisting {
                key: "durable-key".to_string(),
                name: "cutex.dispatch.runtime".to_string(),
            }
        );
        assert_eq!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::TakeoverExisting),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("takeover plan"),
            SessionTuiDispatchPlan::TakeoverExisting {
                key: "durable-key".to_string(),
                id: "019e-dispatch".to_string(),
            }
        );
        assert!(matches!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::Online),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("online plan"),
            SessionTuiDispatchPlan::Online { .. }
        ));
        assert!(matches!(
            dispatch_plan_for_intent(
                &intent_with_profile(SessionTuiAction::CloseAndRestart, "beta"),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("close-and-restart plan"),
            SessionTuiDispatchPlan::CloseAndRestart {
                launch_profile: Some(ref profile),
                ..
            } if profile == "beta"
        ));
        assert!(matches!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::CloseRuntime),
                &store,
                &alden_sessions,
                &[],
            )
            .expect("close plan"),
            SessionTuiDispatchPlan::CloseRuntime { .. }
        ));
    }

    #[test]
    fn managed_host_intents_keep_current_and_managed_cwd_routes_distinct() {
        let record = record(CutexSessionRuntimeBackend::Host);
        let store = store_with(record.clone());

        let online = dispatch_plan_for_intent(&intent(SessionTuiAction::Online), &store, &[], &[])
            .expect("online plan");
        assert_eq!(
            online.recorded_user_action(),
            Some(("durable-key", CutexSessionUserAction::Online))
        );

        let resume_here =
            dispatch_plan_for_intent(&intent(SessionTuiAction::ResumeHere), &store, &[], &[])
                .expect("resume-here plan");
        assert_eq!(
            resume_here,
            SessionTuiDispatchPlan::ResumeHere {
                key: "durable-key".to_string(),
                record,
            }
        );
        assert_eq!(
            resume_here.recorded_user_action(),
            Some(("durable-key", CutexSessionUserAction::ResumeHere))
        );

        assert_eq!(
            dispatch_plan_for_intent(&intent(SessionTuiAction::ResumeManaged), &store, &[], &[],)
                .expect("resume-managed plan"),
            SessionTuiDispatchPlan::ResumeManaged {
                id: "019e-dispatch".to_string(),
            }
        );
    }

    #[test]
    fn host_foreground_intent_maps_to_open_tui_and_never_takeover() {
        let record = record(CutexSessionRuntimeBackend::HostForeground);
        let store = store_with(record);

        assert_eq!(
            dispatch_plan_for_intent(&intent(SessionTuiAction::OpenTui), &store, &[], &[],)
                .expect("open-TUI plan"),
            SessionTuiDispatchPlan::OpenTui {
                id: "019e-dispatch".to_string(),
                launch_profile: None,
            }
        );
        assert!(dispatch_plan_for_intent(
            &intent(SessionTuiAction::TakeoverExisting),
            &store,
            &[],
            &[],
        )
        .is_err());
    }

    #[test]
    fn detached_alden_intent_maps_open_tui_to_the_existing_restore_route() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.current_runtime_agent_id = Some("cutex.dispatch.runtime".to_string());
        record.app_server_runtime = Some(CutexAppServerRuntimeBinding {
            transport: CutexAppServerTransport::UnixSocket,
            endpoint: "unix:///tmp/runtime/app.sock".to_string(),
            pid: std::process::id(),
            runtime_dir: "/tmp/runtime".to_string(),
            launched_profile: Some("aemeath".to_string()),
            launch_profile_source: None,
            auth_token_path: None,
            diagnostic_journal_path: "/tmp/runtime/events.jsonl".to_string(),
            schema_version: "test".to_string(),
            schema_sha256: "hash".to_string(),
            started_at: "2026-08-08T00:00:00Z".to_string(),
        });
        let live_agents = vec![AgentBusAgent {
            id: "cutex.dispatch.runtime".to_string(),
            name: "dispatch.runtime".to_string(),
            base_name: Some("dispatch".to_string()),
            thread_name: None,
            path_key: None,
            session_id: record.codex_session_id.clone(),
            cutex_session_id: None,
            profile: "aemeath".to_string(),
            cwd: record.cwd.clone(),
            pid: std::process::id(),
            host_id: Some(cutex::platform::host::current_host_name()),
            groups: Vec::new(),
            registration_class: AgentRegistrationClass::Persistent,
            last_seen_epoch_secs: 42,
        }];
        let store = store_with(record);

        assert_eq!(
            dispatch_plan_for_intent(
                &intent(SessionTuiAction::OpenTui),
                &store,
                &[],
                &live_agents,
            )
            .expect("detached TUI restore plan"),
            SessionTuiDispatchPlan::OpenTui {
                id: "019e-dispatch".to_string(),
                launch_profile: None,
            }
        );
    }

    #[test]
    fn stale_attach_intent_is_rejected_before_user_action_recording() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.alden_session_name = Some("cutex.dispatch.runtime".to_string());
        let store = store_with(record);

        let error =
            dispatch_plan_for_intent(&intent(SessionTuiAction::AttachExisting), &store, &[], &[])
                .expect_err("stale attach must fail");

        assert!(error.to_string().contains("no longer available"));
    }

    #[test]
    fn stale_intent_for_retired_record_is_rejected_before_dispatch() {
        let mut record = record(CutexSessionRuntimeBackend::Host);
        record.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        record.current_runtime_agent_id = Some("stale-runtime".to_string());
        let store = store_with(record);

        let error =
            dispatch_plan_for_intent(&intent(SessionTuiAction::CloseRuntime), &store, &[], &[])
                .expect_err("retired session must reject stale action");

        assert!(error.to_string().contains("is retired"));
    }

    #[test]
    fn archive_intents_use_the_existing_transaction_without_launch_routes() {
        let active = record(CutexSessionRuntimeBackend::Host);
        let retire = dispatch_plan_for_intent(
            &intent(SessionTuiAction::RetireSession),
            &store_with(active),
            &[],
            &[],
        )
        .expect("retire plan");
        assert_eq!(
            retire,
            SessionTuiDispatchPlan::RetireSession {
                id: "cutex.dispatch".to_string(),
            }
        );

        let mut retired = record(CutexSessionRuntimeBackend::Host);
        retired.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        retired.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        let restore = dispatch_plan_for_intent(
            &intent(SessionTuiAction::RestoreSession),
            &store_with(retired),
            &[],
            &[],
        )
        .expect("restore plan");
        assert_eq!(
            restore,
            SessionTuiDispatchPlan::RestoreSession {
                id: "cutex.dispatch".to_string(),
            }
        );
    }

    #[test]
    fn graceful_close_plan_has_no_incompatible_user_action_record() {
        let mut record = record(CutexSessionRuntimeBackend::Host);
        record.current_runtime_agent_id = Some("volatile-runtime".to_string());
        let store = store_with(record);

        let close =
            dispatch_plan_for_intent(&intent(SessionTuiAction::CloseRuntime), &store, &[], &[])
                .expect("close plan");

        assert_eq!(
            close,
            SessionTuiDispatchPlan::CloseRuntime {
                id: "019e-dispatch".to_string(),
            }
        );
        assert_eq!(close.recorded_user_action(), None);

        let restart = dispatch_plan_for_intent(
            &intent_with_profile(SessionTuiAction::CloseAndRestart, "beta"),
            &store,
            &[],
            &[],
        )
        .expect("restart plan");
        assert_eq!(
            restart,
            SessionTuiDispatchPlan::CloseAndRestart {
                id: "019e-dispatch".to_string(),
                launch_profile: Some("beta".to_string()),
            }
        );
        assert_eq!(restart.recorded_user_action(), None);
    }

    #[test]
    fn selector_dispatch_suppresses_output_only_for_close_runtime() {
        let close = intent(SessionTuiAction::CloseRuntime);
        assert_eq!(
            runtime_close_output_for_surface(&close, SessionTuiDispatchSurface::PostTerminal)
                .expect("post-terminal close output"),
            RuntimeCloseOutput::Print
        );
        assert_eq!(
            runtime_close_output_for_surface(&close, SessionTuiDispatchSurface::Selector)
                .expect("selector close output"),
            RuntimeCloseOutput::Suppress
        );

        let restart = intent(SessionTuiAction::CloseAndRestart);
        assert_eq!(
            runtime_close_output_for_surface(&restart, SessionTuiDispatchSurface::PostTerminal)
                .expect("post-terminal restart output"),
            RuntimeCloseOutput::Print
        );
        assert!(
            runtime_close_output_for_surface(&restart, SessionTuiDispatchSurface::Selector)
                .is_err()
        );
    }

    #[test]
    fn one_launch_profile_is_carried_only_when_fresh_state_can_start_a_process() {
        let offline = store_with(record(CutexSessionRuntimeBackend::CuteAlden));
        assert_eq!(
            dispatch_plan_for_intent(
                &intent_with_profile(SessionTuiAction::ResumeAttach, "beta"),
                &offline,
                &[],
                &[],
            )
            .expect("offline Alden launch plan"),
            SessionTuiDispatchPlan::ResumeAttach {
                id: "019e-dispatch".to_string(),
                launch_profile: Some("beta".to_string()),
            }
        );

        let mut live_record = record(CutexSessionRuntimeBackend::CuteAlden);
        live_record.alden_session_name = Some("cutex.dispatch.runtime".to_string());
        live_record.alden_pid = Some(std::process::id());
        let live_session = CuteAldenSession {
            pid: std::process::id(),
            name: live_record.alden_session_name.clone(),
        };
        let live = store_with(live_record);
        let error = dispatch_plan_for_intent(
            &intent_with_profile(SessionTuiAction::ResumeAttach, "beta"),
            &live,
            &[live_session],
            &[],
        )
        .expect_err("existing Alden process must reject profile override");
        assert!(error.to_string().contains("can no longer apply"));
    }
}
