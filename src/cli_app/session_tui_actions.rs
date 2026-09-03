use cutex::agent_bus::model::AgentBusAgent;
use cutex::runtime::alden::CuteAldenSession;
use cutex::session::model::{CutexSessionRecord, CutexSessionRuntimeBackend};
use cutex::session::projection::{
    cutex_session_has_live_managed_core, cutex_session_is_attachable,
    cutex_session_lifecycle_state_with_agents, primary_start_action_kind_for_record,
    CutexSessionLifecycleState, StartQuickActionKind,
};
use cutex::session::service::cutex_session_is_managed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionTuiAction {
    ResumeAttach,
    AttachExisting,
    TakeoverExisting,
    OpenTui,
    Online,
    ResumeHere,
    ResumeManaged,
    CloseAndRestart,
    CloseRuntime,
    RetireSession,
    RestoreSession,
}

impl SessionTuiAction {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ResumeAttach => "takeover",
            Self::AttachExisting => "attach",
            Self::TakeoverExisting => "takeover existing",
            Self::OpenTui => "open TUI",
            Self::Online => "online",
            Self::ResumeHere => "resume here",
            Self::ResumeManaged => "resume managed",
            Self::CloseAndRestart => "close and restart",
            Self::CloseRuntime => "close runtime",
            Self::RetireSession => "retire session",
            Self::RestoreSession => "restore session",
        }
    }

    pub(super) fn requires_confirmation(self) -> bool {
        matches!(
            self,
            Self::CloseAndRestart | Self::CloseRuntime | Self::RetireSession | Self::RestoreSession
        )
    }

    pub(super) fn supports_launch_profile(
        self,
        lifecycle: CutexSessionLifecycleState,
        attachable: bool,
    ) -> bool {
        match self {
            Self::ResumeAttach => !attachable,
            Self::OpenTui => true,
            Self::Online => lifecycle != CutexSessionLifecycleState::Online,
            Self::CloseAndRestart => true,
            Self::AttachExisting
            | Self::TakeoverExisting
            | Self::ResumeHere
            | Self::ResumeManaged
            | Self::CloseRuntime
            | Self::RetireSession
            | Self::RestoreSession => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionTuiActionItem {
    pub action: SessionTuiAction,
    pub detail: &'static str,
    pub primary: bool,
}

pub(super) fn session_tui_actions_for_record(
    record: &CutexSessionRecord,
    alden_sessions: &[CuteAldenSession],
    live_agents: &[AgentBusAgent],
) -> Vec<SessionTuiActionItem> {
    if record.is_retired() {
        return Vec::new();
    }
    let attachable = cutex_session_is_attachable(record, alden_sessions);
    let lifecycle = cutex_session_lifecycle_state_with_agents(record, alden_sessions, live_agents);
    let tui_detached = record.runtime_backend == CutexSessionRuntimeBackend::CuteAlden
        && !attachable
        && cutex_session_has_live_managed_core(record, live_agents);
    let mut actions = Vec::new();

    let primary = if tui_detached {
        Some(SessionTuiAction::OpenTui)
    } else {
        primary_start_action_kind_for_record(record, alden_sessions)
            .and_then(action_from_quick_kind)
    };
    if let Some(primary) = primary {
        push_action(&mut actions, primary, primary_action_detail(primary), true);
    }

    if record.runtime_backend == CutexSessionRuntimeBackend::CuteAlden && attachable {
        push_action(
            &mut actions,
            SessionTuiAction::AttachExisting,
            "Join the existing TUI without takeover",
            false,
        );
        push_action(
            &mut actions,
            SessionTuiAction::TakeoverExisting,
            "Take control of the existing TUI",
            false,
        );
    }

    let managed_runtime = cutex_session_is_managed(record)
        || record.exposed_to_backend
        || record.app_server_runtime.is_some()
        || matches!(
            record.runtime_backend,
            CutexSessionRuntimeBackend::CuteAlden | CutexSessionRuntimeBackend::HostForeground
        );
    if managed_runtime {
        push_action(
            &mut actions,
            SessionTuiAction::Online,
            "Bring the managed runtime online",
            false,
        );
    }

    let foreground_resume = record.codex_session_id.is_some()
        && record.runtime_backend != CutexSessionRuntimeBackend::CuteAlden
        && record.runtime_backend != CutexSessionRuntimeBackend::HostForeground
        && record.app_server_runtime.is_none();
    if foreground_resume {
        push_action(
            &mut actions,
            SessionTuiAction::ResumeHere,
            "Resume in this terminal using the current cwd",
            false,
        );
        push_action(
            &mut actions,
            SessionTuiAction::ResumeManaged,
            "Resume in this terminal using the managed cwd",
            false,
        );
    }

    let runtime_known = lifecycle != CutexSessionLifecycleState::Offline
        || record.app_server_runtime.is_some()
        || record.alden_pid.is_some()
        || record.runtime_pid.is_some()
        || record.current_runtime_agent_id.is_some();
    if runtime_known {
        if managed_runtime {
            push_action(
                &mut actions,
                SessionTuiAction::CloseAndRestart,
                "Close runtime, then bring it online with the selected profile",
                false,
            );
        }
        push_action(
            &mut actions,
            SessionTuiAction::CloseRuntime,
            "Close runtime gracefully; keep session and history",
            false,
        );
    }

    actions
}

fn action_from_quick_kind(kind: StartQuickActionKind) -> Option<SessionTuiAction> {
    match kind {
        StartQuickActionKind::OpenDetails => None,
        StartQuickActionKind::Attach => Some(SessionTuiAction::AttachExisting),
        StartQuickActionKind::Takeover => Some(SessionTuiAction::TakeoverExisting),
        StartQuickActionKind::ResumeAttach => Some(SessionTuiAction::ResumeAttach),
        StartQuickActionKind::VisibleTui => Some(SessionTuiAction::OpenTui),
        StartQuickActionKind::Online => Some(SessionTuiAction::Online),
        StartQuickActionKind::ResumeHere => Some(SessionTuiAction::ResumeHere),
        StartQuickActionKind::ResumeManaged => Some(SessionTuiAction::ResumeManaged),
    }
}

fn primary_action_detail(action: SessionTuiAction) -> &'static str {
    match action {
        SessionTuiAction::ResumeAttach => "Bring runtime online if needed, then take over TUI",
        SessionTuiAction::AttachExisting => "Join the existing TUI",
        SessionTuiAction::TakeoverExisting => "Take control of the existing TUI",
        SessionTuiAction::OpenTui => "Open the visible TUI for the managed app-server",
        SessionTuiAction::Online => "Bring the managed runtime online",
        SessionTuiAction::ResumeHere => "Resume in this terminal using the current cwd",
        SessionTuiAction::ResumeManaged => "Resume in this terminal using the managed cwd",
        SessionTuiAction::CloseAndRestart => {
            "Close runtime, then bring it online with the selected profile"
        }
        SessionTuiAction::CloseRuntime => "Close runtime gracefully; keep session and history",
        SessionTuiAction::RetireSession => {
            "Archive this managed session; this is distinct from Close runtime"
        }
        SessionTuiAction::RestoreSession => "Restore this archived session as active and offline",
    }
}

fn push_action(
    actions: &mut Vec<SessionTuiActionItem>,
    action: SessionTuiAction,
    detail: &'static str,
    primary: bool,
) {
    if actions.iter().any(|item| item.action == action) {
        return;
    }
    actions.push(SessionTuiActionItem {
        action,
        detail,
        primary,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::session::model::{CutexAppServerRuntimeBinding, CutexAppServerTransport};

    fn record(backend: CutexSessionRuntimeBackend) -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            "cutex.actions".to_string(),
            Some("019e-actions".to_string()),
            "tethys".to_string(),
            "/tmp/actions".to_string(),
            Some("aemeath".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        record.runtime_backend = backend;
        record.registration_class = AgentRegistrationClass::Persistent;
        record
    }

    fn action_kinds(actions: &[SessionTuiActionItem]) -> Vec<SessionTuiAction> {
        actions.iter().map(|item| item.action).collect()
    }

    #[test]
    fn live_alden_actions_keep_takeover_primary_and_offer_graceful_close() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.alden_session_name = Some("cutex.actions.runtime".to_string());
        record.alden_pid = Some(std::process::id());
        let alden_sessions = vec![CuteAldenSession {
            pid: std::process::id(),
            name: record.alden_session_name.clone(),
        }];

        let actions = session_tui_actions_for_record(&record, &alden_sessions, &[]);

        assert_eq!(actions[0].action, SessionTuiAction::ResumeAttach);
        assert!(actions[0].primary);
        assert_eq!(actions[0].action.label(), "takeover");
        assert_eq!(
            action_kinds(&actions),
            vec![
                SessionTuiAction::ResumeAttach,
                SessionTuiAction::AttachExisting,
                SessionTuiAction::TakeoverExisting,
                SessionTuiAction::Online,
                SessionTuiAction::CloseAndRestart,
                SessionTuiAction::CloseRuntime,
            ]
        );
        assert!(actions
            .last()
            .expect("close action")
            .action
            .requires_confirmation());
    }

    #[test]
    fn offline_alden_does_not_offer_attach_or_close_without_a_runtime() {
        let record = record(CutexSessionRuntimeBackend::CuteAlden);

        let actions = session_tui_actions_for_record(&record, &[], &[]);

        assert_eq!(
            action_kinds(&actions),
            vec![SessionTuiAction::ResumeAttach, SessionTuiAction::Online]
        );
    }

    #[test]
    fn retired_record_has_no_tui_actions_even_with_stale_runtime_claims() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-10T00:01:00Z".to_string());
        record.current_runtime_agent_id = Some("stale-runtime".to_string());
        record.alden_pid = Some(std::process::id());

        assert!(session_tui_actions_for_record(&record, &[], &[]).is_empty());
    }

    #[test]
    fn detached_alden_tui_offers_open_tui_without_relabeling_live_takeover() {
        let mut record = record(CutexSessionRuntimeBackend::CuteAlden);
        record.current_runtime_agent_id = Some("cutex.actions.runtime".to_string());
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
            id: "cutex.actions.runtime".to_string(),
            name: "actions.runtime".to_string(),
            base_name: Some("actions".to_string()),
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

        let actions = session_tui_actions_for_record(&record, &[], &live_agents);

        assert_eq!(
            action_kinds(&actions),
            vec![
                SessionTuiAction::OpenTui,
                SessionTuiAction::Online,
                SessionTuiAction::CloseAndRestart,
                SessionTuiAction::CloseRuntime,
            ]
        );
        assert!(actions[0].primary);
        assert_eq!(actions[0].action.label(), "open TUI");
    }

    #[test]
    fn windows_native_actions_open_tui_and_never_offer_takeover() {
        let mut record = record(CutexSessionRuntimeBackend::HostForeground);
        record.current_runtime_agent_id = Some("cutex.actions.runtime".to_string());

        let actions = session_tui_actions_for_record(&record, &[], &[]);
        let kinds = action_kinds(&actions);

        assert_eq!(actions[0].action, SessionTuiAction::OpenTui);
        assert_eq!(actions[0].action.label(), "open TUI");
        assert!(kinds.contains(&SessionTuiAction::Online));
        assert!(kinds.contains(&SessionTuiAction::CloseAndRestart));
        assert!(kinds.contains(&SessionTuiAction::CloseRuntime));
        assert!(!kinds.contains(&SessionTuiAction::ResumeAttach));
        assert!(!kinds.contains(&SessionTuiAction::TakeoverExisting));
    }

    #[test]
    fn offline_managed_host_offers_online_and_foreground_resume_without_close() {
        let record = record(CutexSessionRuntimeBackend::Host);

        let actions = session_tui_actions_for_record(&record, &[], &[]);

        assert_eq!(
            action_kinds(&actions),
            vec![
                SessionTuiAction::Online,
                SessionTuiAction::ResumeHere,
                SessionTuiAction::ResumeManaged,
            ]
        );
        assert!(actions[0].primary);
    }

    #[test]
    fn one_launch_profile_support_tracks_process_creation_not_action_names() {
        assert!(SessionTuiAction::ResumeAttach
            .supports_launch_profile(CutexSessionLifecycleState::Offline, false,));
        assert!(!SessionTuiAction::ResumeAttach
            .supports_launch_profile(CutexSessionLifecycleState::Online, true,));
        assert!(SessionTuiAction::OpenTui
            .supports_launch_profile(CutexSessionLifecycleState::Online, false,));
        assert!(!SessionTuiAction::Online
            .supports_launch_profile(CutexSessionLifecycleState::Online, false,));
        assert!(!SessionTuiAction::CloseRuntime
            .supports_launch_profile(CutexSessionLifecycleState::Offline, false,));
        assert!(SessionTuiAction::CloseAndRestart
            .supports_launch_profile(CutexSessionLifecycleState::Online, true,));
    }
}
