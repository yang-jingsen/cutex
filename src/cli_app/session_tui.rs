use std::collections::HashMap;
use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use chrono::DateTime;
use chrono::Utc;
use crossterm::cursor::Show;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use cutex::agent_bus::client::agent_bus_fetch_agents_if_healthy;
use cutex::agent_bus::model::{AgentBusAgent, AgentGroupUpdateMode};
use cutex::config::global_settings::apply_global_config_patch;
use cutex::config::store::load_codez_config;
use cutex::config::store::load_codez_config_checked;
use cutex::config::store::save_codez_config;
use cutex::management::v2::activity::load_session_activity_states;
use cutex::management::v2::activity::SessionActivityState;
use cutex::platform::host::current_host_name;
use cutex::profiles::model::CodezConfig;
use cutex::runtime::alden::{cute_alden_sessions, CuteAldenSession};
use cutex::session::model::{CutexSessionQuickActionMode, CutexSessionRecord, CutexSessionStore};
use cutex::session::projection::{
    cutex_session_is_attachable, cutex_session_lifecycle_state_with_agents,
    runtime_backend_short_label, CutexSessionLifecycleState,
};
use cutex::session::service::{
    adopt_cutex_session, cutex_session_display_name, cutex_session_is_managed,
    set_cutex_session_display_name_by_key, set_cutex_session_profile_by_key,
    unmanage_cutex_session, CutexSessionAdoptOptions, CutexSessionEnsureSeed,
};
use cutex::session::service::{
    persist_cutex_session_store_and_im_record, update_cutex_session_routing_by_key,
    update_cutex_session_runtime_defaults_by_key, CutexSessionValueUpdate,
};
use cutex::session::store::load_cutex_session_store;
use cutex::ui::format::compact_home_path;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};
use tui_input::{Input, InputRequest};

use super::account_store::{
    load_profile_catalog_read_only, load_profile_names_read_only, ProfileCatalogEntry,
};
use super::agent_bus_runtime;
use super::profile::{activate_account, remove_profile, rename_profile, update_profile_settings};
use super::profile_settings::ProfileSettingsPatch;
use super::session_tui_actions::{
    session_tui_actions_for_record, SessionTuiAction, SessionTuiActionItem,
};
use super::session_tui_profile_settings::{
    ProfileSettingsDraft, ProfileSettingsField, ProfileSettingsSnapshot,
};
use super::session_tui_recent::{
    RecentAdoptionRequest, RecentCatalog, RecentCommand, RecentLoadState, RecentSessionsWorkspace,
};
use super::session_tui_settings::{
    GlobalSettingsDraft, GlobalSettingsField, GlobalSettingsSnapshot, SecretSettingsAction,
    SessionSettingsChoice, SessionSettingsCommand, SessionSettingsDraft, SessionSettingsEditorKind,
    SessionSettingsField, SessionSettingsSnapshot, SessionTuiSettingCategory,
    SessionTuiSettingOption,
};
use super::session_tui_workspace::{SessionTuiWorkspace, WorkspaceSelection};
use super::session_tui_workspace_events::{workspace_event_from_key, WorkspaceEvent};
use super::session_tui_workspace_loading::{WorkspaceLoad, WorkspaceLoadPoll};
use super::session_tui_workspace_render::{render_workspace, WorkspaceRenderer};

const WIDE_LAYOUT_MIN_WIDTH: u16 = 96;
const EXTRA_WIDE_LAYOUT_MIN_WIDTH: u16 = 136;
const SETTINGS_TWO_PANE_MIN_WIDTH: u16 = 64;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ACTIVITY_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

type CutexTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectorTarget {
    Agent(String),
    RetiredAgent(String),
    RecentSessions,
    RetiredSessions,
    CutexProjects,
    Projects,
    Profiles,
    GlobalSettings,
}

impl SelectorTarget {
    fn workspace(&self) -> SessionTuiWorkspace {
        match self {
            Self::Agent(_) => SessionTuiWorkspace::Agents,
            Self::RecentSessions => SessionTuiWorkspace::RecentSessions,
            Self::RetiredAgent(_) | Self::RetiredSessions => SessionTuiWorkspace::RetiredSessions,
            Self::CutexProjects => SessionTuiWorkspace::CutexProjects,
            Self::Projects => SessionTuiWorkspace::Projects,
            Self::Profiles => SessionTuiWorkspace::Profiles,
            Self::GlobalSettings => SessionTuiWorkspace::GlobalSettings,
        }
    }

    fn agent_key(&self) -> Option<&str> {
        match self {
            Self::Agent(key) | Self::RetiredAgent(key) => Some(key),
            Self::RecentSessions
            | Self::RetiredSessions
            | Self::CutexProjects
            | Self::Projects
            | Self::Profiles
            | Self::GlobalSettings => None,
        }
    }

    fn is_profiles(&self) -> bool {
        matches!(self, Self::Profiles)
    }

    fn is_projects(&self) -> bool {
        matches!(self, Self::Projects)
    }

    fn is_cutex_projects(&self) -> bool {
        matches!(self, Self::CutexProjects)
    }

    fn is_retired_sessions(&self) -> bool {
        matches!(self, Self::RetiredSessions)
    }

    fn is_global_settings(&self) -> bool {
        matches!(self, Self::GlobalSettings)
    }

    fn is_system(&self) -> bool {
        matches!(
            self,
            Self::RecentSessions
                | Self::RetiredSessions
                | Self::CutexProjects
                | Self::Projects
                | Self::Profiles
                | Self::GlobalSettings
        )
    }

    fn uses_global_settings(&self) -> bool {
        matches!(self, Self::Profiles | Self::GlobalSettings)
    }
}

#[derive(Debug, Clone)]
struct SelectorRow {
    target: SelectorTarget,
    agent: String,
    configured_profile: Option<String>,
    lifecycle: Option<CutexSessionLifecycleState>,
    host: String,
    backend: String,
    managed_path: String,
    retired_at: Option<String>,
    revision: u64,
    activity_session_id: Option<String>,
    last_output_at: Option<String>,
    actions: Vec<SessionTuiActionItem>,
    settings: Vec<SessionTuiSettingCategory>,
    settings_snapshot: Option<SessionSettingsSnapshot>,
    global_settings_snapshot: Option<GlobalSettingsSnapshot>,
    attachable: bool,
    pinned: bool,
    managed: bool,
}

impl SelectorRow {
    fn launch_profile_control_available(&self) -> bool {
        let Some(lifecycle) = self.lifecycle else {
            return false;
        };
        let Some(snapshot) = self.settings_snapshot.as_ref() else {
            return false;
        };
        !snapshot.profile_names().is_empty()
            && self.actions.iter().any(|item| {
                item.action
                    .supports_launch_profile(lifecycle, self.attachable)
            })
    }

    fn action_control_count(&self) -> usize {
        self.actions.len() + usize::from(self.launch_profile_control_available())
    }

    fn action_for_control_index(&self, index: usize) -> Option<SessionTuiAction> {
        let action_index =
            index.checked_sub(usize::from(self.launch_profile_control_available()))?;
        self.actions.get(action_index).map(|item| item.action)
    }

    fn control_index_for_action(&self, action: SessionTuiAction) -> Option<usize> {
        self.actions
            .iter()
            .position(|item| item.action == action)
            .map(|index| index + usize::from(self.launch_profile_control_available()))
    }

    fn action_supports_launch_profile(&self, action: SessionTuiAction) -> bool {
        self.lifecycle
            .is_some_and(|lifecycle| action.supports_launch_profile(lifecycle, self.attachable))
    }

    fn launch_profile_choices(
        &self,
        global_default_profile: Option<&str>,
    ) -> Vec<SessionSettingsChoice> {
        let Some(snapshot) = self.settings_snapshot.as_ref() else {
            return Vec::new();
        };
        let default = match self.session_profile_override() {
            Some(profile) => profile.to_string(),
            None => match global_default_profile {
                Some(profile) => format!("global: {profile}"),
                None => "global: not configured".to_string(),
            },
        };
        std::iter::once(SessionSettingsChoice {
            label: format!("Session default ({default})"),
            value: None,
        })
        .chain(
            snapshot
                .profile_names()
                .iter()
                .map(|name| SessionSettingsChoice {
                    label: name.clone(),
                    value: Some(name.clone()),
                }),
        )
        .collect()
    }

    fn launch_profile_detail(
        &self,
        launch_profile: Option<&str>,
        global_default_profile: Option<&str>,
    ) -> String {
        match launch_profile {
            Some(profile) => format!("{profile} (this launch only)"),
            None => format!(
                "Session default: {}",
                self.session_profile_detail(global_default_profile)
            ),
        }
    }

    fn session_profile_detail(&self, global_default_profile: Option<&str>) -> String {
        match self.session_profile_override() {
            Some(profile) => profile.to_string(),
            None => match global_default_profile {
                Some(profile) => format!("{profile} (global)"),
                None => "global default not configured".to_string(),
            },
        }
    }

    fn session_profile_override(&self) -> Option<&str> {
        self.settings_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.value(SessionSettingsField::Profile))
            .filter(|profile| *profile != "-")
    }
}

#[derive(Debug)]
struct SelectorSnapshot {
    rows: Vec<SelectorRow>,
    warning: Option<String>,
}

#[derive(Debug)]
enum RuntimeCloseWorkerResult {
    Closed(SelectorSnapshot),
    ClosedRefreshFailed(String),
    Failed(String),
}

type SelectorEvent = WorkspaceEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectorControl {
    Continue,
    Exit,
    Selected(SessionTuiIntent),
    OpenRetiredSessions,
    OpenRecentSessions,
    Recent(RecentCommand),
    AdoptRecent(RecentAdoptionRequest),
    OpenProfileManager,
    OpenCutexProjects,
    OpenProjects,
    ApplySettings(SessionSettingsApplyRequest),
    ApplyGlobalSettings(GlobalSettingsApplyRequest),
    ApplyProfileSettings(ProfileSettingsApplyRequest),
    ManageSession(SessionManagementRequest),
    ManageProfile(ProfileManagementRequest),
    LoginProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionTuiIntent {
    pub(super) key: String,
    pub(super) action: SessionTuiAction,
    pub(super) launch_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionSettingsApplyRequest {
    key: String,
    draft: SessionSettingsDraft,
    profile_names: Vec<String>,
    changed_count: usize,
}

#[derive(Debug)]
struct SessionSettingsApplyResult {
    record: CutexSessionRecord,
    profile_names: Vec<String>,
    warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalSettingsApplyRequest {
    draft: GlobalSettingsDraft,
    profile_names: Vec<String>,
    changed_count: usize,
}

#[derive(Debug)]
struct GlobalSettingsApplyResult {
    config: CodezConfig,
    profile_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileSettingsApplyRequest {
    profile_id: String,
    patch: ProfileSettingsPatch,
    changed_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionManagementRequest {
    key: String,
    command: SessionSettingsCommand,
    profile_names: Vec<String>,
}

#[derive(Debug)]
struct SessionManagementResult {
    record: CutexSessionRecord,
    profile_names: Vec<String>,
    warning: Option<String>,
}

#[derive(Debug)]
struct RecentAdoptionResult {
    store: CutexSessionStore,
    snapshot: Result<SelectorSnapshot, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfileManagementCommand {
    Activate,
    Rename { new_name: String },
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileManagementRequest {
    profile_id: String,
    profile_name: String,
    command: ProfileManagementCommand,
}

#[derive(Debug)]
struct ProfileManagerStartup {
    notice: Option<String>,
    warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionTuiCycleOutcome {
    Exit,
    Selected(SessionTuiIntent),
    LoginProfile,
    CutexProjects,
    Projects,
}

#[derive(Debug)]
struct ProfileMutationReceipt {
    preferred_profile_id: Option<String>,
    notice: String,
}

#[derive(Debug, Clone)]
struct ProfileProjectionSnapshot {
    records: HashMap<String, CutexSessionRecord>,
    config: CodezConfig,
    profile_names: Vec<String>,
}

#[derive(Debug)]
struct ProfileManagementResult {
    profiles: Vec<ProfileCatalogEntry>,
    projection: ProfileProjectionSnapshot,
    preferred_profile_id: Option<String>,
    notice: String,
}

#[derive(Debug, Clone)]
struct PendingSettingsRefreshOverride {
    target: SelectorTarget,
    snapshot: SessionSettingsSnapshot,
    agent: String,
    configured_profile: Option<String>,
    backend: String,
    pinned: bool,
    managed: bool,
    actions: Option<Vec<SessionTuiActionItem>>,
    warning: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingGlobalSettingsRefreshOverride {
    snapshot: GlobalSettingsSnapshot,
}

#[derive(Debug, Clone)]
struct PendingProfileRefreshOverride {
    projection: ProfileProjectionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SelectorMode {
    Agents,
    RecentSessions,
    RetiredSessions {
        selected: usize,
    },
    Actions {
        agent_key: String,
        selected: usize,
        launch_profile: Option<String>,
    },
    Settings {
        target: SelectorTarget,
        category: usize,
        option: usize,
        focus: SettingsFocus,
        view: SettingsView,
    },
    ProfileManager {
        profiles: Vec<ProfileCatalogEntry>,
        selected: usize,
        focus: ProfileWorkspaceFocus,
        editor_selected: usize,
    },
    ConfirmRuntimeAction {
        agent_key: String,
        action: SessionTuiAction,
        launch_profile: Option<String>,
        confirmed: bool,
    },
    ClosingRuntime {
        agent_key: String,
        agent_name: String,
        action: SessionTuiAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsFocus {
    Categories,
    Options,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileWorkspaceFocus {
    Items,
    Editor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsView {
    Expanded,
    Categories,
}

#[derive(Debug, Clone)]
enum SettingsOverlay {
    Choice {
        field: SettingsEditField,
        choices: Vec<SessionSettingsChoice>,
        selected: usize,
        custom_value: Option<String>,
    },
    Text {
        field: SettingsEditField,
        input: Input,
        tags: bool,
        masked: bool,
    },
    Groups {
        field: SettingsEditField,
        inputs: Vec<Input>,
        selected: usize,
    },
    SecretAction {
        field: SettingsEditField,
        selected: usize,
    },
    ConfirmDiscard {
        selected: usize,
    },
    ConfirmManagement {
        command: SessionSettingsCommand,
        selected: usize,
    },
}

#[derive(Debug, Clone)]
enum ActionOverlay {
    LaunchProfile {
        choices: Vec<SessionSettingsChoice>,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileManagerAction {
    Activate,
    Rename,
    Remove,
}

impl ProfileManagerAction {
    fn label(self) -> &'static str {
        match self {
            Self::Activate => "Make active",
            Self::Rename => "Rename",
            Self::Remove => "Remove",
        }
    }
}

#[derive(Debug, Clone)]
enum ProfileOverlay {
    Actions {
        profile_id: String,
        profile_name: String,
        actions: Vec<ProfileManagerAction>,
        selected: usize,
    },
    RenameInput {
        profile_id: String,
        old_name: String,
        input: Input,
    },
    ConfirmRename {
        profile_id: String,
        old_name: String,
        new_name: String,
        selected: usize,
    },
    ConfirmRemove {
        profile_id: String,
        profile_name: String,
        selected: usize,
    },
    ConfirmAddProfile {
        selected: usize,
    },
    ConfirmDiscardProfile {
        destination: ProfileDiscardDestination,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileDiscardDestination {
    ProfileList,
    AgentList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsEditField {
    Session(SessionSettingsField),
    Global(GlobalSettingsField),
    Profile(ProfileSettingsField),
}

impl SettingsEditField {
    fn editor_kind(self) -> SessionSettingsEditorKind {
        match self {
            Self::Session(field) => field.editor_kind(),
            Self::Global(field) => field.editor_kind(),
            Self::Profile(field) => field.editor_kind(),
        }
    }
}

impl SettingsView {
    fn toggle(self) -> Self {
        match self {
            Self::Expanded => Self::Categories,
            Self::Categories => Self::Expanded,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Expanded => "Expanded",
            Self::Categories => "Categories",
        }
    }
}

#[derive(Debug, Clone)]
struct SelectorModel {
    rows: Vec<SelectorRow>,
    retired_rows: Vec<SelectorRow>,
    recent: RecentSessionsWorkspace,
    query: Input,
    workspace_selection: WorkspaceSelection<SelectorTarget>,
    mode: SelectorMode,
    enhanced_keyboard: bool,
    refreshing: bool,
    warning: Option<String>,
    notice: Option<String>,
    settings_draft: SessionSettingsDraft,
    global_settings_draft: GlobalSettingsDraft,
    profile_settings_draft: ProfileSettingsDraft,
    action_overlay: Option<ActionOverlay>,
    settings_overlay: Option<SettingsOverlay>,
    profile_overlay: Option<ProfileOverlay>,
    pending_settings_refresh_override: Option<PendingSettingsRefreshOverride>,
    pending_global_settings_refresh_override: Option<PendingGlobalSettingsRefreshOverride>,
    pending_profile_refresh_override: Option<PendingProfileRefreshOverride>,
    pending_startup_warning: Option<String>,
}

impl SelectorModel {
    fn new(mut rows: Vec<SelectorRow>, refreshing: bool, enhanced_keyboard: bool) -> Self {
        debug_assert!(rows
            .iter()
            .all(|row| { SessionTuiWorkspace::PRODUCTION.contains(&row.target.workspace()) }));
        sort_rows(&mut rows);
        let mut model = Self {
            rows,
            retired_rows: Vec::new(),
            recent: RecentSessionsWorkspace::default(),
            query: Input::default(),
            workspace_selection: WorkspaceSelection::default(),
            mode: SelectorMode::Agents,
            enhanced_keyboard,
            refreshing,
            warning: None,
            notice: None,
            settings_draft: SessionSettingsDraft::default(),
            global_settings_draft: GlobalSettingsDraft::default(),
            profile_settings_draft: ProfileSettingsDraft::default(),
            action_overlay: None,
            settings_overlay: None,
            profile_overlay: None,
            pending_settings_refresh_override: None,
            pending_global_settings_refresh_override: None,
            pending_profile_refresh_override: None,
            pending_startup_warning: None,
        };
        model.ensure_selection();
        model
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.value();
        if query.is_empty() {
            return self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    (row.target.is_system()
                        || row.lifecycle == Some(CutexSessionLifecycleState::Online)
                        || row.attachable
                        || row.pinned
                        || self.workspace_selection.is_transiently_visible(&row.target))
                    .then_some(index)
                })
                .collect();
        }

        let query = query.to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (row.target.is_system()
                    || (row.managed && row.agent.to_lowercase().contains(&query)))
                .then_some(index)
            })
            .collect()
    }

    fn visible_rows(&self) -> Vec<&SelectorRow> {
        self.visible_indices()
            .into_iter()
            .map(|index| &self.rows[index])
            .collect()
    }

    fn hidden_searchable_agent_count(&self) -> usize {
        if !self.query.value().is_empty() {
            return 0;
        }
        self.rows
            .iter()
            .filter(|row| {
                row.managed
                    && row.lifecycle != Some(CutexSessionLifecycleState::Online)
                    && !row.attachable
                    && !row.pinned
                    && !self.workspace_selection.is_transiently_visible(&row.target)
            })
            .count()
    }

    fn selected_visible_index(&self) -> Option<usize> {
        let selected_target = self.workspace_selection.selected()?;
        self.visible_indices()
            .into_iter()
            .position(|index| self.rows[index].target == *selected_target)
    }

    fn selected_row(&self) -> Option<&SelectorRow> {
        let selected_target = self.workspace_selection.selected()?;
        self.rows.iter().find(|row| row.target == *selected_target)
    }

    #[cfg(test)]
    fn selected_target(&self) -> Option<SelectorTarget> {
        self.workspace_selection.selected().cloned()
    }

    fn active_row(&self) -> Option<&SelectorRow> {
        match &self.mode {
            SelectorMode::Agents => self.selected_row(),
            SelectorMode::Actions { agent_key, .. }
            | SelectorMode::ConfirmRuntimeAction { agent_key, .. }
            | SelectorMode::ClosingRuntime { agent_key, .. } => self.row_for_action_key(agent_key),
            SelectorMode::Settings { target, .. } => {
                self.rows.iter().find(|row| row.target == *target)
            }
            SelectorMode::ProfileManager { .. } => {
                self.rows.iter().find(|row| row.target.is_profiles())
            }
            SelectorMode::RecentSessions => None,
            SelectorMode::RetiredSessions { selected } => self.retired_rows.get(*selected),
        }
    }

    fn row_for_action_key(&self, agent_key: &str) -> Option<&SelectorRow> {
        self.rows
            .iter()
            .chain(self.retired_rows.iter())
            .find(|row| row.target.agent_key() == Some(agent_key))
    }

    fn selected_action_index(&self) -> Option<usize> {
        match &self.mode {
            SelectorMode::Actions { selected, .. } => Some(*selected),
            _ => None,
        }
    }

    fn selected_launch_profile(&self) -> Option<&str> {
        match &self.mode {
            SelectorMode::Actions { launch_profile, .. } => launch_profile.as_deref(),
            _ => None,
        }
    }

    fn selected_setting_category_index(&self) -> Option<usize> {
        match &self.mode {
            SelectorMode::Settings { category, .. } => Some(*category),
            _ => None,
        }
    }

    fn selected_setting_option_index(&self) -> Option<usize> {
        match &self.mode {
            SelectorMode::Settings { option, .. } => Some(*option),
            _ => None,
        }
    }

    fn settings_focus(&self) -> Option<SettingsFocus> {
        match &self.mode {
            SelectorMode::Settings { focus, .. } => Some(*focus),
            _ => None,
        }
    }

    fn settings_view(&self) -> Option<SettingsView> {
        match &self.mode {
            SelectorMode::Settings { view, .. } => Some(*view),
            _ => None,
        }
    }

    fn settings_dirty_count(&self) -> usize {
        match &self.mode {
            SelectorMode::Settings { target, .. } if target.uses_global_settings() => {
                self.global_settings_draft.dirty_count()
            }
            SelectorMode::ProfileManager { selected: 0, .. } => {
                self.global_settings_draft.dirty_count()
            }
            SelectorMode::ProfileManager { .. } => self.profile_settings_draft.dirty_count(),
            _ => self.settings_draft.dirty_count(),
        }
    }

    fn settings_are_editable(&self) -> bool {
        self.active_row().is_some_and(|row| {
            row.settings_snapshot.is_some() || row.global_settings_snapshot.is_some()
        })
    }

    fn active_setting_category(&self) -> Option<&SessionTuiSettingCategory> {
        let category = self.selected_setting_category_index()?;
        self.active_row()?.settings.get(category)
    }

    fn active_setting_option(&self) -> Option<&SessionTuiSettingOption> {
        let option = self.selected_setting_option_index()?;
        self.active_setting_category()?.options.get(option)
    }

    fn active_setting_label(&self) -> Option<&'static str> {
        if self.selected_profile_is_default() {
            return match self.selected_profile_default_option()? {
                0 => Some("Default profile"),
                1 => Some("Direct default launch"),
                _ => None,
            };
        }
        if matches!(&self.mode, SelectorMode::ProfileManager { .. }) {
            return self
                .selected_profile_setting_option()
                .map(|option| option.label);
        }
        self.active_setting_option().map(|option| option.label)
    }

    fn active_settings_snapshot(&self) -> Option<&SessionSettingsSnapshot> {
        self.active_row()?.settings_snapshot.as_ref()
    }

    fn active_global_settings_snapshot(&self) -> Option<&GlobalSettingsSnapshot> {
        self.active_row()?.global_settings_snapshot.as_ref()
    }

    fn global_settings_snapshot(&self) -> Option<&GlobalSettingsSnapshot> {
        self.active_global_settings_snapshot().or_else(|| {
            self.rows
                .iter()
                .find_map(|row| row.global_settings_snapshot.as_ref())
        })
    }

    fn selected_profile(&self) -> Option<&ProfileCatalogEntry> {
        let SelectorMode::ProfileManager {
            profiles, selected, ..
        } = &self.mode
        else {
            return None;
        };
        selected
            .checked_sub(1)
            .and_then(|index| profiles.get(index))
    }

    fn selected_profile_index(&self) -> Option<usize> {
        match &self.mode {
            SelectorMode::ProfileManager { selected, .. } => Some(*selected),
            _ => None,
        }
    }

    fn selected_profile_is_add(&self) -> bool {
        matches!(
            &self.mode,
            SelectorMode::ProfileManager {
                profiles,
                selected,
                ..
            } if *selected == profiles.len().saturating_add(1)
        )
    }

    fn selected_profile_is_default(&self) -> bool {
        matches!(&self.mode, SelectorMode::ProfileManager { selected: 0, .. })
    }

    fn profile_workspace_focus(&self) -> Option<ProfileWorkspaceFocus> {
        match &self.mode {
            SelectorMode::ProfileManager { focus, .. } => Some(*focus),
            _ => None,
        }
    }

    fn selected_profile_default_option(&self) -> Option<usize> {
        match &self.mode {
            SelectorMode::ProfileManager {
                editor_selected, ..
            } if self.selected_profile_is_default() => Some(*editor_selected),
            _ => None,
        }
    }

    fn selected_profile_settings_snapshot(&self) -> Option<ProfileSettingsSnapshot> {
        self.selected_profile()
            .map(ProfileSettingsSnapshot::from_catalog_entry)
    }

    fn selected_profile_setting_categories(&self) -> Vec<SessionTuiSettingCategory> {
        self.selected_profile_settings_snapshot()
            .map(|snapshot| snapshot.categories(&self.profile_settings_draft))
            .unwrap_or_default()
    }

    fn selected_profile_setting_option(&self) -> Option<SessionTuiSettingOption> {
        let flat_index = match &self.mode {
            SelectorMode::ProfileManager {
                editor_selected, ..
            } => *editor_selected,
            _ => return None,
        };
        self.selected_profile_setting_categories()
            .into_iter()
            .flat_map(|category| category.options)
            .nth(flat_index)
    }

    fn profile_editor_option_count(&self) -> usize {
        self.selected_profile_setting_categories()
            .iter()
            .map(|category| category.options.len())
            .sum()
    }

    fn profile_default_value(&self, field: GlobalSettingsField) -> Option<String> {
        let snapshot = self.global_settings_snapshot()?;
        Some(self.global_settings_draft.value(snapshot, field))
    }

    fn current_default_profile_name(&self) -> Option<String> {
        let snapshot = self.global_settings_snapshot()?;
        if !self
            .global_settings_draft
            .field_is_dirty(GlobalSettingsField::DefaultProfile)
        {
            return snapshot.default_profile_name().map(str::to_string);
        }
        self.profile_default_value(GlobalSettingsField::DefaultProfile)
            .filter(|value| value != "-")
    }

    fn handle(&mut self, event: SelectorEvent) -> SelectorControl {
        if matches!(&self.mode, SelectorMode::Actions { .. }) && self.action_overlay.is_some() {
            return self.handle_action_overlay_event(event);
        }
        if matches!(&self.mode, SelectorMode::ProfileManager { .. })
            && self.settings_overlay.is_some()
        {
            return self.handle_settings_overlay_event(event, &SelectorTarget::Profiles);
        }
        if matches!(&self.mode, SelectorMode::ProfileManager { .. })
            && self.profile_overlay.is_some()
        {
            return self.handle_profile_overlay_event(event);
        }
        match self.mode.clone() {
            SelectorMode::Agents => self.handle_agent_event(event),
            SelectorMode::RecentSessions => self.handle_recent_sessions_event(event),
            SelectorMode::RetiredSessions { selected } => {
                self.handle_retired_sessions_event(event, selected)
            }
            SelectorMode::Actions {
                agent_key,
                selected,
                launch_profile,
            } => self.handle_action_event(event, agent_key, selected, launch_profile),
            SelectorMode::Settings {
                target,
                category,
                option,
                focus,
                view,
            } => self.handle_settings_event(event, target, category, option, focus, view),
            SelectorMode::ProfileManager {
                profiles,
                selected,
                focus,
                editor_selected,
            } => {
                self.handle_profile_manager_event(event, profiles, selected, focus, editor_selected)
            }
            SelectorMode::ConfirmRuntimeAction {
                agent_key,
                action,
                launch_profile,
                confirmed,
            } => {
                self.handle_confirmation_event(event, agent_key, action, launch_profile, confirmed)
            }
            SelectorMode::ClosingRuntime { .. } => SelectorControl::Continue,
        }
    }

    fn handle_agent_event(&mut self, event: SelectorEvent) -> SelectorControl {
        match event {
            SelectorEvent::Up => self.move_selection(-1),
            SelectorEvent::Down => self.move_selection(1),
            SelectorEvent::First => self.select_edge(false),
            SelectorEvent::Last => self.select_edge(true),
            SelectorEvent::Insert(character) => {
                self.query.handle(InputRequest::InsertChar(character));
                self.ensure_selection();
            }
            SelectorEvent::Backspace => {
                self.query.handle(InputRequest::DeletePrevChar);
                self.ensure_selection();
            }
            SelectorEvent::Delete => {
                self.query.handle(InputRequest::DeleteNextChar);
                self.ensure_selection();
            }
            SelectorEvent::ClearInput => {
                self.query.handle(InputRequest::DeleteLine);
                self.ensure_selection();
            }
            SelectorEvent::Escape if !self.query.value().is_empty() => {
                self.query.reset();
                self.ensure_selection();
            }
            SelectorEvent::OpenActions
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RecentSessions) =>
            {
                return SelectorControl::OpenRecentSessions;
            }
            SelectorEvent::Activate
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RecentSessions) =>
            {
                return SelectorControl::OpenRecentSessions;
            }
            SelectorEvent::OpenSettings
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RecentSessions) =>
            {
                return SelectorControl::OpenRecentSessions;
            }
            SelectorEvent::OpenActions
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RetiredSessions) =>
            {
                return SelectorControl::OpenRetiredSessions;
            }
            SelectorEvent::Activate
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RetiredSessions) =>
            {
                return SelectorControl::OpenRetiredSessions;
            }
            SelectorEvent::OpenSettings
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::RetiredSessions) =>
            {
                return SelectorControl::OpenRetiredSessions;
            }
            SelectorEvent::OpenActions
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::Profiles) =>
            {
                return SelectorControl::OpenProfileManager;
            }
            SelectorEvent::OpenActions | SelectorEvent::OpenSettings | SelectorEvent::Activate
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::CutexProjects) =>
            {
                return SelectorControl::OpenCutexProjects;
            }
            SelectorEvent::OpenActions | SelectorEvent::OpenSettings | SelectorEvent::Activate
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::Projects) =>
            {
                return SelectorControl::OpenProjects;
            }
            SelectorEvent::OpenActions => self.open_action_menu(),
            SelectorEvent::OpenSettings
                if self
                    .workspace_selection
                    .is_selected(&SelectorTarget::Profiles) =>
            {
                return SelectorControl::OpenProfileManager;
            }
            SelectorEvent::OpenSettings => self.open_settings(),
            SelectorEvent::Activate => return self.activate_primary_action(),
            SelectorEvent::Back => {}
            SelectorEvent::Escape | SelectorEvent::Exit => return SelectorControl::Exit,
        }
        SelectorControl::Continue
    }

    fn handle_recent_sessions_event(&mut self, event: SelectorEvent) -> SelectorControl {
        if self.recent.review().is_some() {
            match event {
                SelectorEvent::Up | SelectorEvent::First | SelectorEvent::Back => {
                    self.recent.set_review_confirmed(false)
                }
                SelectorEvent::Down | SelectorEvent::Last | SelectorEvent::OpenActions => {
                    self.recent.set_review_confirmed(true)
                }
                SelectorEvent::Activate if self.recent.review_confirmed() => {
                    if let Some(request) = self.recent.adoption_request() {
                        return SelectorControl::AdoptRecent(request);
                    }
                }
                SelectorEvent::Activate | SelectorEvent::Escape => self.recent.cancel_review(),
                SelectorEvent::Exit => return SelectorControl::Exit,
                SelectorEvent::Insert(_)
                | SelectorEvent::Backspace
                | SelectorEvent::Delete
                | SelectorEvent::ClearInput
                | SelectorEvent::OpenSettings => {}
            }
            return SelectorControl::Continue;
        }
        match event {
            SelectorEvent::Up => self.recent.move_selection(-1),
            SelectorEvent::Down => self.recent.move_selection(1),
            SelectorEvent::First => self.recent.select_edge(false),
            SelectorEvent::Last => self.recent.select_edge(true),
            SelectorEvent::Activate => {
                let retry = matches!(
                    self.recent.load_state(),
                    RecentLoadState::Failed(_) | RecentLoadState::ProviderIncompatible(_)
                );
                if retry && !self.recent.loading() {
                    return SelectorControl::Recent(RecentCommand::Retry);
                }
                self.recent.begin_review();
            }
            SelectorEvent::OpenActions
                if self.recent.next_cursor().is_some() && !self.recent.loading() =>
            {
                return SelectorControl::Recent(RecentCommand::LoadMore);
            }
            SelectorEvent::OpenActions
                if matches!(self.recent.load_state(), RecentLoadState::Failed(_))
                    && !self.recent.loading() =>
            {
                return SelectorControl::Recent(RecentCommand::Retry);
            }
            SelectorEvent::OpenActions => {}
            SelectorEvent::Back | SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                self.mode = SelectorMode::Agents;
                self.ensure_selection();
            }
            SelectorEvent::Exit => return SelectorControl::Exit,
            SelectorEvent::Insert(_)
            | SelectorEvent::Backspace
            | SelectorEvent::Delete
            | SelectorEvent::ClearInput => {}
        }
        SelectorControl::Continue
    }

    fn recent_catalog_reply(&mut self, reply: super::session_tui_recent::CatalogReply) {
        match load_cutex_session_store() {
            Ok(store) => self.recent.receive(reply, &store),
            Err(error) => {
                self.recent.reconciliation_failed(
                    reply,
                    format!("recent session reconciliation unavailable: {error:#}"),
                );
            }
        }
    }

    fn recent_loading_started(&mut self) {
        self.recent.mark_loading();
    }

    fn recent_adoption_succeeded(
        &mut self,
        request: &RecentAdoptionRequest,
        result: RecentAdoptionResult,
    ) {
        let selected = self.workspace_selection.selected().cloned();
        self.recent.adoption_succeeded(&result.store);
        self.workspace_selection.select(selected.clone());
        self.ensure_selection();
        self.notice = Some(format!("Adopted native thread {}", request.title));
        match result.snapshot {
            Ok(snapshot) => {
                self.rows = snapshot.rows;
                self.workspace_selection.select(selected);
                self.ensure_selection();
                self.warning = snapshot.warning;
            }
            Err(error) => {
                self.warning = Some(format!(
                    "Native thread was adopted, but agent refresh failed: {error}"
                ));
            }
        }
    }

    fn recent_adoption_failed(&mut self, message: String) {
        self.warning = Some(format!("recent session adoption failed: {message}"));
    }

    fn handle_retired_sessions_event(
        &mut self,
        event: SelectorEvent,
        selected: usize,
    ) -> SelectorControl {
        if self.retired_rows.is_empty() {
            match event {
                SelectorEvent::Back | SelectorEvent::Escape => self.mode = SelectorMode::Agents,
                SelectorEvent::Exit => return SelectorControl::Exit,
                _ => {}
            }
            return SelectorControl::Continue;
        }
        let mut next = selected.min(self.retired_rows.len() - 1);
        match event {
            SelectorEvent::Up => next = wrapped_index(next, -1, self.retired_rows.len()),
            SelectorEvent::Down => next = wrapped_index(next, 1, self.retired_rows.len()),
            SelectorEvent::First => next = 0,
            SelectorEvent::Last => next = self.retired_rows.len() - 1,
            SelectorEvent::Activate | SelectorEvent::OpenActions => {
                let row = &self.retired_rows[next];
                let Some(key) = row.target.agent_key() else {
                    return SelectorControl::Continue;
                };
                self.mode = SelectorMode::ConfirmRuntimeAction {
                    agent_key: key.to_string(),
                    action: SessionTuiAction::RestoreSession,
                    launch_profile: None,
                    confirmed: false,
                };
                return SelectorControl::Continue;
            }
            SelectorEvent::Back | SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                self.mode = SelectorMode::Agents;
                return SelectorControl::Continue;
            }
            SelectorEvent::Exit => return SelectorControl::Exit,
            SelectorEvent::Insert(_)
            | SelectorEvent::Backspace
            | SelectorEvent::Delete
            | SelectorEvent::ClearInput => {}
        }
        self.mode = SelectorMode::RetiredSessions { selected: next };
        SelectorControl::Continue
    }

    fn open_retired_sessions(&mut self, rows: Vec<SelectorRow>) {
        self.retired_rows = rows;
        self.mode = SelectorMode::RetiredSessions { selected: 0 };
        self.notice = None;
        self.warning = None;
    }

    fn handle_action_event(
        &mut self,
        event: SelectorEvent,
        agent_key: String,
        selected: usize,
        launch_profile: Option<String>,
    ) -> SelectorControl {
        let global_default_profile = self.current_default_profile_name();
        let Some((control_count, has_profile_control, profile_choices)) = self
            .rows
            .iter()
            .find(|row| row.target.agent_key() == Some(agent_key.as_str()))
            .map(|row| {
                (
                    row.action_control_count(),
                    row.launch_profile_control_available(),
                    row.launch_profile_choices(global_default_profile.as_deref()),
                )
            })
        else {
            self.mode = SelectorMode::Agents;
            return SelectorControl::Continue;
        };
        if control_count == 0 {
            self.mode = SelectorMode::Agents;
            return SelectorControl::Continue;
        }
        let mut next_selected = selected.min(control_count - 1);
        match event {
            SelectorEvent::Up => next_selected = wrapped_index(next_selected, -1, control_count),
            SelectorEvent::Down => next_selected = wrapped_index(next_selected, 1, control_count),
            SelectorEvent::First => next_selected = 0,
            SelectorEvent::Last => next_selected = control_count - 1,
            SelectorEvent::Activate if has_profile_control && next_selected == 0 => {
                let overlay_selected = launch_profile
                    .as_ref()
                    .and_then(|profile| {
                        profile_choices
                            .iter()
                            .position(|choice| choice.value.as_deref() == Some(profile.as_str()))
                    })
                    .unwrap_or(0);
                self.mode = SelectorMode::Actions {
                    agent_key,
                    selected: next_selected,
                    launch_profile,
                };
                self.action_overlay = Some(ActionOverlay::LaunchProfile {
                    choices: profile_choices,
                    selected: overlay_selected,
                });
                return SelectorControl::Continue;
            }
            SelectorEvent::Activate => {
                let action = self
                    .rows
                    .iter()
                    .find(|row| row.target.agent_key() == Some(agent_key.as_str()))
                    .and_then(|row| row.action_for_control_index(next_selected));
                if let Some(action) = action {
                    return self.select_action(agent_key, action, launch_profile);
                }
            }
            SelectorEvent::Back | SelectorEvent::Escape => {
                self.action_overlay = None;
                self.mode = SelectorMode::Agents;
                return SelectorControl::Continue;
            }
            SelectorEvent::Exit => return SelectorControl::Exit,
            SelectorEvent::OpenActions
            | SelectorEvent::OpenSettings
            | SelectorEvent::Insert(_)
            | SelectorEvent::Backspace
            | SelectorEvent::Delete
            | SelectorEvent::ClearInput => {}
        }
        self.mode = SelectorMode::Actions {
            agent_key,
            selected: next_selected,
            launch_profile,
        };
        SelectorControl::Continue
    }

    fn handle_action_overlay_event(&mut self, event: SelectorEvent) -> SelectorControl {
        let Some(ActionOverlay::LaunchProfile {
            choices,
            mut selected,
        }) = self.action_overlay.clone()
        else {
            return SelectorControl::Continue;
        };
        if choices.is_empty() {
            self.action_overlay = None;
            return SelectorControl::Continue;
        }
        match event {
            SelectorEvent::Up => selected = wrapped_index(selected, -1, choices.len()),
            SelectorEvent::Down => selected = wrapped_index(selected, 1, choices.len()),
            SelectorEvent::First => selected = 0,
            SelectorEvent::Last => selected = choices.len() - 1,
            SelectorEvent::Activate => {
                if let SelectorMode::Actions { launch_profile, .. } = &mut self.mode {
                    *launch_profile = choices[selected].value.clone();
                }
                self.action_overlay = None;
                self.warning = None;
                return SelectorControl::Continue;
            }
            SelectorEvent::Back
            | SelectorEvent::Escape
            | SelectorEvent::OpenActions
            | SelectorEvent::OpenSettings => {
                self.action_overlay = None;
                return SelectorControl::Continue;
            }
            SelectorEvent::Exit => return SelectorControl::Exit,
            SelectorEvent::Insert(_)
            | SelectorEvent::Backspace
            | SelectorEvent::Delete
            | SelectorEvent::ClearInput => {}
        }
        self.action_overlay = Some(ActionOverlay::LaunchProfile { choices, selected });
        SelectorControl::Continue
    }

    fn handle_settings_event(
        &mut self,
        event: SelectorEvent,
        target: SelectorTarget,
        category: usize,
        option: usize,
        focus: SettingsFocus,
        view: SettingsView,
    ) -> SelectorControl {
        if self.settings_overlay.is_some() {
            return self.handle_settings_overlay_event(event, &target);
        }

        if matches!(event, SelectorEvent::Insert('a' | 'A')) {
            if let Some(key) = target.agent_key() {
                let changed_count = self.settings_draft.dirty_count();
                if changed_count == 0 {
                    return SelectorControl::Continue;
                }
                return SelectorControl::ApplySettings(SessionSettingsApplyRequest {
                    key: key.to_string(),
                    draft: self.settings_draft.clone(),
                    profile_names: self
                        .active_settings_snapshot()
                        .map(|snapshot| snapshot.profile_names().to_vec())
                        .unwrap_or_default(),
                    changed_count,
                });
            }
            let changed_count = self.global_settings_draft.dirty_count();
            if changed_count == 0 {
                return SelectorControl::Continue;
            }
            return SelectorControl::ApplyGlobalSettings(GlobalSettingsApplyRequest {
                draft: self.global_settings_draft.clone(),
                profile_names: self
                    .active_global_settings_snapshot()
                    .map(|snapshot| snapshot.profile_names().to_vec())
                    .unwrap_or_default(),
                changed_count,
            });
        }
        if matches!(event, SelectorEvent::Insert('d' | 'D')) {
            if self.settings_draft.is_dirty() || self.global_settings_draft.is_dirty() {
                self.discard_settings_draft(&target);
                self.notice = Some("Draft discarded".to_string());
            }
            return SelectorControl::Continue;
        }

        let Some(settings) = self
            .rows
            .iter()
            .find(|row| row.target == target)
            .map(|row| row.settings.as_slice())
        else {
            self.mode = SelectorMode::Agents;
            return SelectorControl::Continue;
        };
        if settings.is_empty() {
            self.mode = SelectorMode::Agents;
            return SelectorControl::Continue;
        }

        let mut next_category = category.min(settings.len() - 1);
        let mut next_option = option.min(settings[next_category].options.len().saturating_sub(1));
        let mut next_focus = focus;
        if settings[next_category].options.is_empty() {
            let Some((category, option)) = setting_indices_at_flat_index(settings, 0) else {
                self.mode = SelectorMode::Agents;
                return SelectorControl::Continue;
            };
            next_category = category;
            next_option = option;
        }

        let mut open_editor = false;
        if matches!(event, SelectorEvent::Insert('v' | 'V')) {
            self.mode = SelectorMode::Settings {
                target,
                category: next_category,
                option: next_option,
                focus: SettingsFocus::Options,
                view: view.toggle(),
            };
            return SelectorControl::Continue;
        }

        match view {
            SettingsView::Expanded => match event {
                SelectorEvent::Up | SelectorEvent::Down => {
                    let direction = if event == SelectorEvent::Up { -1 } else { 1 };
                    if let Some((category, option)) =
                        moved_flat_setting(settings, next_category, next_option, direction)
                    {
                        next_category = category;
                        next_option = option;
                    }
                }
                SelectorEvent::First | SelectorEvent::Last => {
                    let flat_index = if event == SelectorEvent::First {
                        0
                    } else {
                        setting_option_count(settings) - 1
                    };
                    if let Some((category, option)) =
                        setting_indices_at_flat_index(settings, flat_index)
                    {
                        next_category = category;
                        next_option = option;
                    }
                }
                SelectorEvent::Activate => open_editor = true,
                SelectorEvent::Back | SelectorEvent::OpenSettings | SelectorEvent::Escape => {
                    self.request_leave_settings();
                    return SelectorControl::Continue;
                }
                SelectorEvent::Exit => return SelectorControl::Exit,
                SelectorEvent::OpenActions
                | SelectorEvent::Insert(_)
                | SelectorEvent::Backspace
                | SelectorEvent::Delete
                | SelectorEvent::ClearInput => {}
            },
            SettingsView::Categories => match event {
                SelectorEvent::Up | SelectorEvent::Down => {
                    let direction = if event == SelectorEvent::Up { -1 } else { 1 };
                    match next_focus {
                        SettingsFocus::Categories => {
                            next_category = wrapped_index(next_category, direction, settings.len());
                            next_option = 0;
                        }
                        SettingsFocus::Options => {
                            next_option = wrapped_index(
                                next_option,
                                direction,
                                settings[next_category].options.len(),
                            );
                        }
                        SettingsFocus::Value => {}
                    }
                }
                SelectorEvent::First => match next_focus {
                    SettingsFocus::Categories => {
                        next_category = 0;
                        next_option = 0;
                    }
                    SettingsFocus::Options => next_option = 0,
                    SettingsFocus::Value => {}
                },
                SelectorEvent::Last => match next_focus {
                    SettingsFocus::Categories => {
                        next_category = settings.len() - 1;
                        next_option = 0;
                    }
                    SettingsFocus::Options => {
                        next_option = settings[next_category].options.len().saturating_sub(1);
                    }
                    SettingsFocus::Value => {}
                },
                SelectorEvent::OpenActions | SelectorEvent::Activate => match next_focus {
                    SettingsFocus::Categories => next_focus = SettingsFocus::Options,
                    SettingsFocus::Options => next_focus = SettingsFocus::Value,
                    SettingsFocus::Value => {
                        if event == SelectorEvent::Activate {
                            open_editor = true;
                        }
                    }
                },
                SelectorEvent::Back => match next_focus {
                    SettingsFocus::Value => next_focus = SettingsFocus::Options,
                    SettingsFocus::Options => next_focus = SettingsFocus::Categories,
                    SettingsFocus::Categories => {
                        self.request_leave_settings();
                        return SelectorControl::Continue;
                    }
                },
                SelectorEvent::OpenSettings | SelectorEvent::Escape => {
                    self.request_leave_settings();
                    return SelectorControl::Continue;
                }
                SelectorEvent::Exit => return SelectorControl::Exit,
                SelectorEvent::Insert(_)
                | SelectorEvent::Backspace
                | SelectorEvent::Delete
                | SelectorEvent::ClearInput => {}
            },
        }
        self.mode = SelectorMode::Settings {
            target,
            category: next_category,
            option: next_option,
            focus: next_focus,
            view,
        };
        if open_editor {
            self.open_active_setting_editor();
        }
        SelectorControl::Continue
    }

    fn handle_profile_manager_event(
        &mut self,
        event: SelectorEvent,
        profiles: Vec<ProfileCatalogEntry>,
        selected: usize,
        focus: ProfileWorkspaceFocus,
        editor_selected: usize,
    ) -> SelectorControl {
        if matches!(event, SelectorEvent::Insert('a' | 'A')) {
            if selected == 0 {
                let changed_count = self.global_settings_draft.dirty_count();
                if changed_count == 0 {
                    return SelectorControl::Continue;
                }
                return SelectorControl::ApplyGlobalSettings(GlobalSettingsApplyRequest {
                    draft: self.global_settings_draft.clone(),
                    profile_names: self
                        .active_global_settings_snapshot()
                        .map(|snapshot| snapshot.profile_names().to_vec())
                        .unwrap_or_default(),
                    changed_count,
                });
            }
            let Some(profile_id) = self.selected_profile().map(|profile| profile.id.clone()) else {
                return SelectorControl::Continue;
            };
            let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                return SelectorControl::Continue;
            };
            let changed_count = self.profile_settings_draft.dirty_count();
            if changed_count == 0 {
                return SelectorControl::Continue;
            }
            let patch = match self.profile_settings_draft.patch(&snapshot) {
                Ok(patch) => patch,
                Err(error) => {
                    self.warning = Some(format!("profile settings edit failed: {error:#}"));
                    self.notice = None;
                    return SelectorControl::Continue;
                }
            };
            return SelectorControl::ApplyProfileSettings(ProfileSettingsApplyRequest {
                profile_id,
                patch,
                changed_count,
            });
        }
        if matches!(event, SelectorEvent::Insert('d' | 'D')) {
            let dirty = if selected == 0 {
                self.global_settings_draft.is_dirty()
            } else {
                self.profile_settings_draft.is_dirty()
            };
            if dirty {
                self.discard_active_profile_workspace_draft();
                self.notice = Some("Draft discarded".to_string());
            }
            return SelectorControl::Continue;
        }

        let item_count = profiles.len().saturating_add(2);
        let mut next_selected = selected.min(item_count.saturating_sub(1));
        let mut next_focus = focus;
        let mut next_editor_selected = editor_selected;
        let mut open_profile_actions = false;
        let mut open_default_editor = false;
        let mut open_profile_editor = false;
        let mut open_add = false;

        match focus {
            ProfileWorkspaceFocus::Items => match event {
                SelectorEvent::Up => next_selected = wrapped_index(next_selected, -1, item_count),
                SelectorEvent::Down => next_selected = wrapped_index(next_selected, 1, item_count),
                SelectorEvent::First => next_selected = 0,
                SelectorEvent::Last => next_selected = item_count.saturating_sub(1),
                SelectorEvent::Activate | SelectorEvent::OpenActions => {
                    if next_selected == item_count.saturating_sub(1) {
                        open_add = true;
                    } else {
                        next_focus = ProfileWorkspaceFocus::Editor;
                    }
                }
                SelectorEvent::Back | SelectorEvent::OpenSettings | SelectorEvent::Escape => {
                    self.request_leave_profile_workspace();
                    return SelectorControl::Continue;
                }
                SelectorEvent::Exit => return SelectorControl::Exit,
                SelectorEvent::Insert(_)
                | SelectorEvent::Backspace
                | SelectorEvent::Delete
                | SelectorEvent::ClearInput => {}
            },
            ProfileWorkspaceFocus::Editor => match event {
                SelectorEvent::Up if selected == 0 => {
                    next_editor_selected = wrapped_index(next_editor_selected, -1, 2)
                }
                SelectorEvent::Down if selected == 0 => {
                    next_editor_selected = wrapped_index(next_editor_selected, 1, 2)
                }
                SelectorEvent::First if selected == 0 => next_editor_selected = 0,
                SelectorEvent::Last if selected == 0 => next_editor_selected = 1,
                SelectorEvent::Activate if selected == 0 => open_default_editor = true,
                SelectorEvent::Up | SelectorEvent::Down
                    if selected < item_count.saturating_sub(1) =>
                {
                    let count = self.profile_editor_option_count();
                    let direction = if event == SelectorEvent::Up { -1 } else { 1 };
                    next_editor_selected = wrapped_index(next_editor_selected, direction, count);
                }
                SelectorEvent::First if selected < item_count.saturating_sub(1) => {
                    next_editor_selected = 0;
                }
                SelectorEvent::Last if selected < item_count.saturating_sub(1) => {
                    next_editor_selected = self.profile_editor_option_count().saturating_sub(1);
                }
                SelectorEvent::Activate if selected < item_count.saturating_sub(1) => {
                    open_profile_editor = true;
                }
                SelectorEvent::Activate | SelectorEvent::OpenActions
                    if selected == item_count.saturating_sub(1) =>
                {
                    open_add = true;
                }
                SelectorEvent::OpenActions => open_profile_actions = true,
                SelectorEvent::Back => {
                    self.request_leave_profile_editor(ProfileDiscardDestination::ProfileList);
                    return SelectorControl::Continue;
                }
                SelectorEvent::OpenSettings | SelectorEvent::Escape => {
                    self.request_leave_profile_workspace();
                    return SelectorControl::Continue;
                }
                SelectorEvent::Exit => return SelectorControl::Exit,
                SelectorEvent::Activate => {}
                SelectorEvent::Up
                | SelectorEvent::Down
                | SelectorEvent::First
                | SelectorEvent::Last
                | SelectorEvent::Insert(_)
                | SelectorEvent::Backspace
                | SelectorEvent::Delete
                | SelectorEvent::ClearInput => {}
            },
        }

        if next_selected != selected {
            self.profile_settings_draft = ProfileSettingsDraft::default();
            self.global_settings_draft = GlobalSettingsDraft::default();
            next_editor_selected = 0;
        }

        self.mode = SelectorMode::ProfileManager {
            profiles,
            selected: next_selected,
            focus: next_focus,
            editor_selected: next_editor_selected,
        };
        if open_default_editor {
            self.open_profile_default_editor();
        } else if open_profile_editor {
            self.open_profile_setting_editor();
        } else if open_profile_actions {
            self.open_profile_actions();
        } else if open_add {
            self.profile_overlay = Some(ProfileOverlay::ConfirmAddProfile { selected: 0 });
        }
        SelectorControl::Continue
    }

    fn open_profile_manager(&mut self, profiles: Vec<ProfileCatalogEntry>) {
        self.workspace_selection
            .select(Some(SelectorTarget::Profiles));
        self.global_settings_draft = GlobalSettingsDraft::default();
        self.profile_settings_draft = ProfileSettingsDraft::default();
        self.settings_overlay = None;
        self.profile_overlay = None;
        self.notice = None;
        if self
            .warning
            .as_deref()
            .is_some_and(|warning| warning.starts_with("profile catalog unavailable:"))
        {
            self.warning = None;
        }
        self.mode = SelectorMode::ProfileManager {
            profiles,
            selected: 0,
            focus: ProfileWorkspaceFocus::Items,
            editor_selected: 0,
        };
    }

    fn request_leave_profile_workspace(&mut self) {
        if self.global_settings_draft.is_dirty() || self.profile_settings_draft.is_dirty() {
            self.profile_overlay = Some(ProfileOverlay::ConfirmDiscardProfile {
                destination: ProfileDiscardDestination::AgentList,
                selected: 0,
            });
        } else {
            self.leave_settings();
        }
    }

    fn request_leave_profile_editor(&mut self, destination: ProfileDiscardDestination) {
        let dirty = if self.selected_profile_is_default() {
            self.global_settings_draft.is_dirty()
        } else {
            self.profile_settings_draft.is_dirty()
        };
        if dirty {
            self.profile_overlay = Some(ProfileOverlay::ConfirmDiscardProfile {
                destination,
                selected: 0,
            });
            return;
        }
        self.finish_leaving_profile_editor(destination);
    }

    fn finish_leaving_profile_editor(&mut self, destination: ProfileDiscardDestination) {
        match destination {
            ProfileDiscardDestination::ProfileList => {
                if let SelectorMode::ProfileManager { focus, .. } = &mut self.mode {
                    *focus = ProfileWorkspaceFocus::Items;
                }
            }
            ProfileDiscardDestination::AgentList => self.leave_settings(),
        }
    }

    fn discard_active_profile_workspace_draft(&mut self) {
        if self.selected_profile_is_default() {
            self.global_settings_draft = GlobalSettingsDraft::default();
        } else {
            self.profile_settings_draft = ProfileSettingsDraft::default();
        }
        self.settings_overlay = None;
        self.profile_overlay = None;
        if self.warning.as_deref().is_some_and(|warning| {
            warning.starts_with("profile settings edit failed:")
                || warning.starts_with("profile settings apply failed:")
                || warning.starts_with("Apply or discard staged profile settings")
        }) {
            self.warning = None;
        }
    }

    fn open_profile_default_editor(&mut self) {
        let Some(field) = self.selected_profile_default_option().map(|selected| {
            if selected == 0 {
                GlobalSettingsField::DefaultProfile
            } else {
                GlobalSettingsField::DefaultProfileDirectLaunch
            }
        }) else {
            return;
        };
        let Some(snapshot) = self.active_global_settings_snapshot() else {
            return;
        };
        let choices = snapshot.choices(field);
        let current = self.global_settings_draft.value(snapshot, field);
        let selected = choices.iter().position(|choice| {
            choice.value.as_deref() == (current != "-").then_some(current.as_str())
        });
        self.settings_overlay = Some(SettingsOverlay::Choice {
            field: SettingsEditField::Global(field),
            choices,
            selected: selected.unwrap_or(0),
            custom_value: selected.is_none().then_some(current),
        });
    }

    fn open_profile_setting_editor(&mut self) {
        let Some(option) = self.selected_profile_setting_option() else {
            return;
        };
        let Some(field) = option.profile_field else {
            self.notice = Some("Read-only profile information".to_string());
            return;
        };
        let Some(snapshot) = self.selected_profile_settings_snapshot() else {
            return;
        };
        self.settings_overlay = Some(match field.editor_kind() {
            SessionSettingsEditorKind::Choice => {
                let choices = self.profile_settings_draft.choices(&snapshot, field);
                let current = snapshot.editor_value(&self.profile_settings_draft, field);
                let selected = choices
                    .iter()
                    .position(|choice| choice.value.as_deref() == Some(current.as_str()));
                SettingsOverlay::Choice {
                    field: SettingsEditField::Profile(field),
                    choices,
                    selected: selected.unwrap_or(0),
                    custom_value: selected.is_none().then_some(current),
                }
            }
            SessionSettingsEditorKind::Text | SessionSettingsEditorKind::Tags => {
                let value = snapshot.editor_value(&self.profile_settings_draft, field);
                SettingsOverlay::Text {
                    field: SettingsEditField::Profile(field),
                    input: Input::new(value),
                    tags: false,
                    masked: false,
                }
            }
            SessionSettingsEditorKind::Secret => SettingsOverlay::SecretAction {
                field: SettingsEditField::Profile(field),
                selected: 0,
            },
        });
    }

    fn open_profile_actions(&mut self) {
        if self.selected_profile_is_default() {
            return;
        }
        if self.selected_profile_is_add() {
            self.profile_overlay = Some(ProfileOverlay::ConfirmAddProfile { selected: 0 });
            return;
        }
        if self.profile_settings_draft.is_dirty() {
            self.warning =
                Some("Apply or discard staged profile settings before actions".to_string());
            return;
        }
        let Some(profile) = self.selected_profile().cloned() else {
            self.warning = Some("No profile selected".to_string());
            return;
        };
        let mut actions = Vec::with_capacity(3);
        if !profile.active {
            actions.push(ProfileManagerAction::Activate);
        }
        actions.extend([ProfileManagerAction::Rename, ProfileManagerAction::Remove]);
        self.profile_overlay = Some(ProfileOverlay::Actions {
            profile_id: profile.id,
            profile_name: profile.name,
            actions,
            selected: 0,
        });
        if self.warning.as_deref().is_some_and(|warning| {
            warning.starts_with("profile change failed:")
                || warning.starts_with("profile settings apply failed:")
                || warning.starts_with("profile settings edit failed:")
        }) {
            self.warning = None;
        }
    }

    fn profile_manager_failed(&mut self, message: String) {
        self.warning = Some(format!("profile catalog unavailable: {message}"));
        self.notice = None;
    }

    fn handle_profile_overlay_event(&mut self, event: SelectorEvent) -> SelectorControl {
        let Some(overlay) = self.profile_overlay.clone() else {
            return SelectorControl::Continue;
        };
        match overlay {
            ProfileOverlay::Actions {
                profile_id,
                profile_name,
                actions,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Up => selected = wrapped_index(selected, -1, actions.len()),
                    SelectorEvent::Down => selected = wrapped_index(selected, 1, actions.len()),
                    SelectorEvent::First => selected = 0,
                    SelectorEvent::Last => selected = actions.len().saturating_sub(1),
                    SelectorEvent::Activate => {
                        let Some(action) = actions.get(selected).copied() else {
                            self.profile_overlay = None;
                            return SelectorControl::Continue;
                        };
                        match action {
                            ProfileManagerAction::Activate => {
                                return SelectorControl::ManageProfile(ProfileManagementRequest {
                                    profile_id,
                                    profile_name,
                                    command: ProfileManagementCommand::Activate,
                                });
                            }
                            ProfileManagerAction::Rename => {
                                self.profile_overlay = Some(ProfileOverlay::RenameInput {
                                    profile_id,
                                    old_name: profile_name.clone(),
                                    input: Input::new(profile_name),
                                });
                            }
                            ProfileManagerAction::Remove => {
                                self.profile_overlay = Some(ProfileOverlay::ConfirmRemove {
                                    profile_id,
                                    profile_name,
                                    selected: 0,
                                });
                            }
                        }
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Back | SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::OpenActions
                    | SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.profile_overlay = Some(ProfileOverlay::Actions {
                    profile_id,
                    profile_name,
                    actions,
                    selected,
                });
            }
            ProfileOverlay::RenameInput {
                profile_id,
                old_name,
                mut input,
            } => {
                match event {
                    SelectorEvent::Insert(character) => {
                        input.handle(InputRequest::InsertChar(character));
                    }
                    SelectorEvent::Backspace => {
                        input.handle(InputRequest::DeletePrevChar);
                    }
                    SelectorEvent::Delete => {
                        input.handle(InputRequest::DeleteNextChar);
                    }
                    SelectorEvent::ClearInput => {
                        input.handle(InputRequest::DeleteLine);
                    }
                    SelectorEvent::Back => {
                        input.handle(InputRequest::GoToPrevChar);
                    }
                    SelectorEvent::OpenActions => {
                        input.handle(InputRequest::GoToNextChar);
                    }
                    SelectorEvent::First => {
                        input.handle(InputRequest::GoToStart);
                    }
                    SelectorEvent::Last => {
                        input.handle(InputRequest::GoToEnd);
                    }
                    SelectorEvent::Activate => {
                        let new_name = input.value().trim().to_string();
                        if new_name.is_empty() {
                            self.warning = Some("Profile name cannot be empty".to_string());
                        } else if new_name == old_name {
                            self.warning = Some("Enter a different profile name".to_string());
                        } else {
                            self.warning = None;
                            self.profile_overlay = Some(ProfileOverlay::ConfirmRename {
                                profile_id,
                                old_name,
                                new_name,
                                selected: 0,
                            });
                            return SelectorControl::Continue;
                        }
                    }
                    SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Up | SelectorEvent::Down => {}
                }
                self.profile_overlay = Some(ProfileOverlay::RenameInput {
                    profile_id,
                    old_name,
                    input,
                });
            }
            ProfileOverlay::ConfirmRename {
                profile_id,
                old_name,
                new_name,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        return SelectorControl::ManageProfile(ProfileManagementRequest {
                            profile_id,
                            profile_name: old_name,
                            command: ProfileManagementCommand::Rename { new_name },
                        });
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.profile_overlay = Some(ProfileOverlay::ConfirmRename {
                    profile_id,
                    old_name,
                    new_name,
                    selected,
                });
            }
            ProfileOverlay::ConfirmRemove {
                profile_id,
                profile_name,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        return SelectorControl::ManageProfile(ProfileManagementRequest {
                            profile_id,
                            profile_name,
                            command: ProfileManagementCommand::Remove,
                        });
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.profile_overlay = Some(ProfileOverlay::ConfirmRemove {
                    profile_id,
                    profile_name,
                    selected,
                });
            }
            ProfileOverlay::ConfirmAddProfile { mut selected } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        return SelectorControl::LoginProfile;
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.profile_overlay = Some(ProfileOverlay::ConfirmAddProfile { selected });
            }
            ProfileOverlay::ConfirmDiscardProfile {
                destination,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        self.discard_active_profile_workspace_draft();
                        self.finish_leaving_profile_editor(destination);
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.profile_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.profile_overlay = Some(ProfileOverlay::ConfirmDiscardProfile {
                    destination,
                    selected,
                });
            }
        }
        SelectorControl::Continue
    }

    fn handle_settings_overlay_event(
        &mut self,
        event: SelectorEvent,
        target: &SelectorTarget,
    ) -> SelectorControl {
        let Some(overlay) = self.settings_overlay.clone() else {
            return SelectorControl::Continue;
        };
        match overlay {
            SettingsOverlay::Choice {
                field,
                choices,
                mut selected,
                custom_value,
            } => {
                let custom_offset = usize::from(custom_value.is_some());
                let choice_count = choices.len() + custom_offset;
                match event {
                    SelectorEvent::Up => selected = wrapped_index(selected, -1, choice_count),
                    SelectorEvent::Down => selected = wrapped_index(selected, 1, choice_count),
                    SelectorEvent::First => selected = 0,
                    SelectorEvent::Last => selected = choice_count.saturating_sub(1),
                    SelectorEvent::Activate => {
                        let value = if selected < custom_offset {
                            custom_value.clone()
                        } else {
                            choices
                                .get(selected - custom_offset)
                                .and_then(|choice| choice.value.clone())
                        };
                        if !self.stage_setting(target, field, value) {
                            self.settings_overlay = Some(SettingsOverlay::Choice {
                                field,
                                choices,
                                selected,
                                custom_value,
                            });
                        }
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Back | SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::OpenActions
                    | SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.settings_overlay = Some(SettingsOverlay::Choice {
                    field,
                    choices,
                    selected,
                    custom_value,
                });
            }
            SettingsOverlay::Groups {
                field,
                mut inputs,
                mut selected,
            } => {
                if inputs.is_empty() {
                    inputs.push(Input::default());
                }
                selected = selected.min(inputs.len().saturating_sub(1));
                match event {
                    SelectorEvent::Insert(character) => {
                        inputs[selected].handle(InputRequest::InsertChar(character));
                        ensure_group_editor_trailing_input(&mut inputs);
                    }
                    SelectorEvent::Backspace => {
                        inputs[selected].handle(InputRequest::DeletePrevChar);
                    }
                    SelectorEvent::Delete => {
                        inputs[selected].handle(InputRequest::DeleteNextChar);
                    }
                    SelectorEvent::ClearInput => {
                        inputs[selected].handle(InputRequest::DeleteLine);
                    }
                    SelectorEvent::Back => {
                        inputs[selected].handle(InputRequest::GoToPrevChar);
                    }
                    SelectorEvent::OpenActions => {
                        inputs[selected].handle(InputRequest::GoToNextChar);
                    }
                    SelectorEvent::First => {
                        inputs[selected].handle(InputRequest::GoToStart);
                    }
                    SelectorEvent::Last => {
                        inputs[selected].handle(InputRequest::GoToEnd);
                    }
                    SelectorEvent::Up => {
                        selected = selected.saturating_sub(1);
                    }
                    SelectorEvent::Down => {
                        selected = (selected + 1).min(inputs.len().saturating_sub(1));
                    }
                    SelectorEvent::Activate => {
                        let staged =
                            self.stage_setting(target, field, Some(group_editor_value(&inputs)));
                        if !staged {
                            self.settings_overlay = Some(SettingsOverlay::Groups {
                                field,
                                inputs,
                                selected,
                            });
                        }
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                }
                self.settings_overlay = Some(SettingsOverlay::Groups {
                    field,
                    inputs,
                    selected,
                });
            }
            SettingsOverlay::Text {
                field,
                mut input,
                tags,
                masked,
            } => {
                match event {
                    SelectorEvent::Insert(character) => {
                        input.handle(InputRequest::InsertChar(character));
                    }
                    SelectorEvent::Backspace => {
                        input.handle(InputRequest::DeletePrevChar);
                    }
                    SelectorEvent::Delete => {
                        input.handle(InputRequest::DeleteNextChar);
                    }
                    SelectorEvent::ClearInput => {
                        input.handle(InputRequest::DeleteLine);
                    }
                    SelectorEvent::Back => {
                        input.handle(InputRequest::GoToPrevChar);
                    }
                    SelectorEvent::OpenActions => {
                        input.handle(InputRequest::GoToNextChar);
                    }
                    SelectorEvent::First => {
                        input.handle(InputRequest::GoToStart);
                    }
                    SelectorEvent::Last => {
                        input.handle(InputRequest::GoToEnd);
                    }
                    SelectorEvent::Activate => {
                        let staged = if masked {
                            self.stage_secret_setting(
                                target,
                                field,
                                SecretSettingsAction::Replace(input.value().to_string()),
                            )
                        } else {
                            let raw_value = input.value();
                            let value = if tags {
                                Some(raw_value.trim().to_string())
                            } else if matches!(
                                field,
                                SettingsEditField::Global(
                                    GlobalSettingsField::AgentMessagePrefix
                                        | GlobalSettingsField::AgentMessageSuffix
                                )
                            ) {
                                (!raw_value.is_empty() && raw_value != "-")
                                    .then(|| raw_value.to_string())
                            } else {
                                let value = raw_value.trim();
                                (!value.is_empty() && value != "-").then(|| value.to_string())
                            };
                            self.stage_setting(target, field, value)
                        };
                        if !staged {
                            self.settings_overlay = Some(SettingsOverlay::Text {
                                field,
                                input,
                                tags,
                                masked,
                            });
                        }
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Up | SelectorEvent::Down => {}
                }
                self.settings_overlay = Some(SettingsOverlay::Text {
                    field,
                    input,
                    tags,
                    masked,
                });
            }
            SettingsOverlay::SecretAction {
                field,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Up => selected = wrapped_index(selected, -1, 3),
                    SelectorEvent::Down => selected = wrapped_index(selected, 1, 3),
                    SelectorEvent::First => selected = 0,
                    SelectorEvent::Last => selected = 2,
                    SelectorEvent::Activate if selected == 1 => {
                        self.settings_overlay = Some(SettingsOverlay::Text {
                            field,
                            input: Input::default(),
                            tags: false,
                            masked: true,
                        });
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Activate => {
                        let action = if selected == 0 {
                            SecretSettingsAction::Keep
                        } else {
                            SecretSettingsAction::Clear
                        };
                        if self.stage_secret_setting(target, field, action) {
                            self.settings_overlay = None;
                        } else {
                            self.settings_overlay =
                                Some(SettingsOverlay::SecretAction { field, selected });
                        }
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Back | SelectorEvent::Escape | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::OpenActions
                    | SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.settings_overlay = Some(SettingsOverlay::SecretAction { field, selected });
            }
            SettingsOverlay::ConfirmDiscard { mut selected } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        self.discard_settings_draft(target);
                        self.leave_settings();
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.settings_overlay = Some(SettingsOverlay::ConfirmDiscard { selected });
            }
            SettingsOverlay::ConfirmManagement {
                command,
                mut selected,
            } => {
                match event {
                    SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => selected = 0,
                    SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                        selected = 1;
                    }
                    SelectorEvent::Activate if selected == 1 => {
                        let Some(key) = target.agent_key() else {
                            self.settings_overlay = None;
                            return SelectorControl::Continue;
                        };
                        return SelectorControl::ManageSession(SessionManagementRequest {
                            key: key.to_string(),
                            command,
                            profile_names: self
                                .active_settings_snapshot()
                                .map(|snapshot| snapshot.profile_names().to_vec())
                                .unwrap_or_default(),
                        });
                    }
                    SelectorEvent::Activate
                    | SelectorEvent::Escape
                    | SelectorEvent::OpenSettings => {
                        self.settings_overlay = None;
                        return SelectorControl::Continue;
                    }
                    SelectorEvent::Exit => return SelectorControl::Exit,
                    SelectorEvent::Insert(_)
                    | SelectorEvent::Backspace
                    | SelectorEvent::Delete
                    | SelectorEvent::ClearInput => {}
                }
                self.settings_overlay =
                    Some(SettingsOverlay::ConfirmManagement { command, selected });
            }
        }
        SelectorControl::Continue
    }

    fn open_active_setting_editor(&mut self) {
        if let Some(command) = self
            .active_setting_option()
            .and_then(|option| option.command)
        {
            if self.settings_draft.is_dirty() {
                self.warning =
                    Some("Apply or discard staged settings before changing management".to_string());
                return;
            }
            self.settings_overlay = Some(SettingsOverlay::ConfirmManagement {
                command,
                selected: 0,
            });
            return;
        }
        let Some(field) = self.active_setting_option().and_then(|option| {
            option
                .field
                .map(SettingsEditField::Session)
                .or_else(|| option.global_field.map(SettingsEditField::Global))
                .or_else(|| option.profile_field.map(SettingsEditField::Profile))
        }) else {
            return;
        };
        self.settings_overlay = Some(match field.editor_kind() {
            SessionSettingsEditorKind::Choice => {
                let (choices, current) = match field {
                    SettingsEditField::Session(field) => {
                        let Some(snapshot) = self.active_settings_snapshot() else {
                            return;
                        };
                        (
                            snapshot.choices(field),
                            self.settings_draft
                                .value(snapshot, field)
                                .map(str::to_string),
                        )
                    }
                    SettingsEditField::Global(field) => {
                        let Some(snapshot) = self.active_global_settings_snapshot() else {
                            return;
                        };
                        let current = self.global_settings_draft.value(snapshot, field);
                        (snapshot.choices(field), (current != "-").then_some(current))
                    }
                    SettingsEditField::Profile(field) => {
                        let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                            return;
                        };
                        let current = snapshot.editor_value(&self.profile_settings_draft, field);
                        (
                            self.profile_settings_draft.choices(&snapshot, field),
                            Some(current),
                        )
                    }
                };
                let selected = choices
                    .iter()
                    .position(|choice| choice.value.as_deref() == current.as_deref());
                if matches!(
                    field,
                    SettingsEditField::Session(SessionSettingsField::Profile)
                ) && choices.is_empty()
                    && current.is_none()
                {
                    if self.warning.is_none() {
                        self.warning = Some("No configured profiles available".to_string());
                    }
                    return;
                }
                SettingsOverlay::Choice {
                    field,
                    choices,
                    selected: selected.unwrap_or(0),
                    custom_value: selected.is_none().then_some(current).flatten(),
                }
            }
            SessionSettingsEditorKind::Text => SettingsOverlay::Text {
                field,
                input: Input::new(match field {
                    SettingsEditField::Session(field) => {
                        let Some(snapshot) = self.active_settings_snapshot() else {
                            return;
                        };
                        self.settings_draft
                            .value(snapshot, field)
                            .unwrap_or_default()
                            .to_string()
                    }
                    SettingsEditField::Global(field) => {
                        let Some(snapshot) = self.active_global_settings_snapshot() else {
                            return;
                        };
                        self.global_settings_draft.value(snapshot, field)
                    }
                    SettingsEditField::Profile(field) => {
                        let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                            return;
                        };
                        snapshot.editor_value(&self.profile_settings_draft, field)
                    }
                }),
                tags: false,
                masked: false,
            },
            SessionSettingsEditorKind::Tags => {
                let value = match field {
                    SettingsEditField::Session(field) => {
                        let Some(snapshot) = self.active_settings_snapshot() else {
                            return;
                        };
                        self.settings_draft
                            .value(snapshot, field)
                            .unwrap_or_default()
                            .to_string()
                    }
                    SettingsEditField::Global(field) => {
                        let Some(snapshot) = self.active_global_settings_snapshot() else {
                            return;
                        };
                        self.global_settings_draft.value(snapshot, field)
                    }
                    SettingsEditField::Profile(field) => {
                        let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                            return;
                        };
                        snapshot.editor_value(&self.profile_settings_draft, field)
                    }
                };
                if matches!(
                    field,
                    SettingsEditField::Session(SessionSettingsField::AgentGroups)
                ) {
                    SettingsOverlay::Groups {
                        field,
                        inputs: group_editor_inputs(&value),
                        selected: 0,
                    }
                } else {
                    SettingsOverlay::Text {
                        field,
                        input: Input::new(value),
                        tags: true,
                        masked: false,
                    }
                }
            }
            SessionSettingsEditorKind::Secret => {
                SettingsOverlay::SecretAction { field, selected: 0 }
            }
        });
    }

    fn stage_setting(
        &mut self,
        target: &SelectorTarget,
        field: SettingsEditField,
        value: Option<String>,
    ) -> bool {
        let result = match field {
            SettingsEditField::Session(field) => {
                let Some(snapshot) = self.active_settings_snapshot().cloned() else {
                    self.settings_overlay = None;
                    return false;
                };
                self.settings_draft.stage(&snapshot, field, value)
            }
            SettingsEditField::Global(field) => {
                let Some(snapshot) = self.active_global_settings_snapshot().cloned() else {
                    self.settings_overlay = None;
                    return false;
                };
                self.global_settings_draft.stage(&snapshot, field, value)
            }
            SettingsEditField::Profile(field) => {
                let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                    self.settings_overlay = None;
                    return false;
                };
                self.profile_settings_draft.stage(&snapshot, field, value)
            }
        };
        if let Err(error) = result {
            self.warning = Some(format!("settings edit failed: {error:#}"));
            return false;
        }
        self.settings_overlay = None;
        self.notice = None;
        if self.warning.as_deref().is_some_and(|warning| {
            warning.starts_with("settings edit failed:")
                || warning.starts_with("profile settings edit failed:")
        }) {
            self.warning = None;
        }
        self.reproject_settings(target);
        if target.is_profiles() {
            self.clamp_profile_editor_selection();
        }
        true
    }

    fn stage_secret_setting(
        &mut self,
        target: &SelectorTarget,
        field: SettingsEditField,
        action: SecretSettingsAction,
    ) -> bool {
        let result = match field {
            SettingsEditField::Global(field) => {
                let Some(snapshot) = self.active_global_settings_snapshot().cloned() else {
                    self.settings_overlay = None;
                    return false;
                };
                self.global_settings_draft
                    .stage_secret(&snapshot, field, action)
            }
            SettingsEditField::Profile(field) => {
                let Some(snapshot) = self.selected_profile_settings_snapshot() else {
                    self.settings_overlay = None;
                    return false;
                };
                self.profile_settings_draft
                    .stage_secret(&snapshot, field, action)
            }
            SettingsEditField::Session(_) => {
                self.warning =
                    Some("Secret editor is unavailable for this session setting".to_string());
                return false;
            }
        };
        if let Err(error) = result {
            self.warning = Some(format!("settings edit failed: {error:#}"));
            return false;
        }
        self.settings_overlay = None;
        self.notice = None;
        self.warning = None;
        self.reproject_settings(target);
        if target.is_profiles() {
            self.clamp_profile_editor_selection();
        }
        true
    }

    fn request_leave_settings(&mut self) {
        if self.settings_draft.is_dirty() || self.global_settings_draft.is_dirty() {
            self.settings_overlay = Some(SettingsOverlay::ConfirmDiscard { selected: 0 });
        } else {
            self.leave_settings();
        }
    }

    fn leave_settings(&mut self) {
        self.settings_draft = SessionSettingsDraft::default();
        self.global_settings_draft = GlobalSettingsDraft::default();
        self.profile_settings_draft = ProfileSettingsDraft::default();
        self.settings_overlay = None;
        self.profile_overlay = None;
        self.mode = SelectorMode::Agents;
        self.ensure_selection();
    }

    fn discard_settings_draft(&mut self, target: &SelectorTarget) {
        if target.uses_global_settings() {
            self.global_settings_draft = GlobalSettingsDraft::default();
        } else {
            self.settings_draft = SessionSettingsDraft::default();
        }
        self.settings_overlay = None;
        self.reproject_settings(target);
    }

    fn reproject_settings(&mut self, target: &SelectorTarget) {
        if target.is_profiles() {
            return;
        }
        let Some(row) = self.rows.iter_mut().find(|row| row.target == *target) else {
            return;
        };
        if let Some(snapshot) = row.settings_snapshot.as_ref() {
            row.settings = snapshot.categories(&self.settings_draft);
        } else if let Some(snapshot) = row.global_settings_snapshot.as_ref() {
            row.settings = snapshot.categories(&self.global_settings_draft);
        }
    }

    fn clamp_profile_editor_selection(&mut self) {
        let count = self.profile_editor_option_count();
        if let SelectorMode::ProfileManager {
            editor_selected, ..
        } = &mut self.mode
        {
            *editor_selected = (*editor_selected).min(count.saturating_sub(1));
        }
    }

    fn handle_confirmation_event(
        &mut self,
        event: SelectorEvent,
        agent_key: String,
        action: SessionTuiAction,
        launch_profile: Option<String>,
        confirmed: bool,
    ) -> SelectorControl {
        let mut next_confirmed = confirmed;
        match event {
            SelectorEvent::OpenActions | SelectorEvent::Down | SelectorEvent::Last => {
                next_confirmed = true;
            }
            SelectorEvent::Back | SelectorEvent::Up | SelectorEvent::First => {
                next_confirmed = false;
            }
            SelectorEvent::Activate if next_confirmed => {
                return SelectorControl::Selected(SessionTuiIntent {
                    key: agent_key,
                    action,
                    launch_profile,
                });
            }
            SelectorEvent::Activate | SelectorEvent::Escape => {
                if action == SessionTuiAction::RestoreSession {
                    let selected = self
                        .retired_rows
                        .iter()
                        .position(|row| row.target.agent_key() == Some(agent_key.as_str()))
                        .unwrap_or(0);
                    self.mode = SelectorMode::RetiredSessions { selected };
                    return SelectorControl::Continue;
                }
                let selected = self.action_index(&agent_key, action).unwrap_or(0);
                self.mode = SelectorMode::Actions {
                    agent_key,
                    selected,
                    launch_profile,
                };
                return SelectorControl::Continue;
            }
            SelectorEvent::Exit => return SelectorControl::Exit,
            SelectorEvent::OpenSettings
            | SelectorEvent::Insert(_)
            | SelectorEvent::Backspace
            | SelectorEvent::Delete
            | SelectorEvent::ClearInput => {}
        }
        self.mode = SelectorMode::ConfirmRuntimeAction {
            agent_key,
            action,
            launch_profile,
            confirmed: next_confirmed,
        };
        SelectorControl::Continue
    }

    fn open_action_menu(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.actions.is_empty() {
            return;
        }
        let Some(agent_key) = row.target.agent_key() else {
            return;
        };
        self.mode = SelectorMode::Actions {
            agent_key: agent_key.to_string(),
            selected: 0,
            launch_profile: None,
        };
    }

    fn open_settings(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.settings.is_empty() {
            return;
        }
        let target = row.target.clone();
        let view = if target.is_global_settings() {
            SettingsView::Categories
        } else {
            SettingsView::Expanded
        };
        self.settings_draft = SessionSettingsDraft::default();
        self.settings_overlay = None;
        self.notice = None;
        self.mode = SelectorMode::Settings {
            target,
            category: 0,
            option: 0,
            focus: if view == SettingsView::Expanded {
                SettingsFocus::Options
            } else {
                SettingsFocus::Categories
            },
            view,
        };
    }

    fn activate_primary_action(&mut self) -> SelectorControl {
        let Some(row) = self.selected_row() else {
            return SelectorControl::Continue;
        };
        if row.target.is_global_settings() {
            self.open_settings();
            return SelectorControl::Continue;
        }
        if row.target.is_profiles() {
            return SelectorControl::OpenProfileManager;
        }
        if row.target.is_cutex_projects() {
            return SelectorControl::OpenCutexProjects;
        }
        if row.target.is_projects() {
            return SelectorControl::OpenProjects;
        }
        let Some(action) = row
            .actions
            .iter()
            .find(|item| item.primary)
            .map(|item| item.action)
        else {
            return SelectorControl::Continue;
        };
        if action.requires_confirmation() {
            return SelectorControl::Continue;
        }
        let Some(agent_key) = row.target.agent_key() else {
            return SelectorControl::Continue;
        };
        SelectorControl::Selected(SessionTuiIntent {
            key: agent_key.to_string(),
            action,
            launch_profile: None,
        })
    }

    fn select_action(
        &mut self,
        agent_key: String,
        action: SessionTuiAction,
        launch_profile: Option<String>,
    ) -> SelectorControl {
        if launch_profile.is_some()
            && !self
                .rows
                .iter()
                .find(|row| row.target.agent_key() == Some(agent_key.as_str()))
                .is_some_and(|row| row.action_supports_launch_profile(action))
        {
            self.warning = Some(format!(
                "{} cannot apply a one-launch profile in the current state",
                action.label()
            ));
            return SelectorControl::Continue;
        }
        if action.requires_confirmation() {
            self.mode = SelectorMode::ConfirmRuntimeAction {
                agent_key,
                action,
                launch_profile,
                confirmed: false,
            };
            SelectorControl::Continue
        } else {
            SelectorControl::Selected(SessionTuiIntent {
                key: agent_key,
                action,
                launch_profile,
            })
        }
    }

    fn action_index(&self, agent_key: &str, action: SessionTuiAction) -> Option<usize> {
        self.rows
            .iter()
            .find(|row| row.target.agent_key() == Some(agent_key))?
            .control_index_for_action(action)
    }

    fn activate_close_shortcut(&mut self) -> SelectorControl {
        if !matches!(self.mode, SelectorMode::Agents) {
            return SelectorControl::Continue;
        }
        let Some(row) = self.selected_row() else {
            return SelectorControl::Continue;
        };
        let Some(agent_key) = row.target.agent_key() else {
            self.warning = Some("Select an agent with a runtime to close".to_string());
            return SelectorControl::Continue;
        };
        if !row
            .actions
            .iter()
            .any(|item| item.action == SessionTuiAction::CloseRuntime)
        {
            self.warning = Some(format!(
                "No runtime is available to close for {}",
                row.agent
            ));
            return SelectorControl::Continue;
        }
        SelectorControl::Selected(SessionTuiIntent {
            key: agent_key.to_string(),
            action: SessionTuiAction::CloseRuntime,
            launch_profile: None,
        })
    }

    fn runtime_close_started(&mut self, intent: &SessionTuiIntent) {
        debug_assert!(intent_runs_in_selector(intent));
        let agent_name = self
            .row_for_action_key(&intent.key)
            .map(|row| row.agent.clone())
            .unwrap_or_else(|| intent.key.clone());
        self.workspace_selection
            .select(Some(SelectorTarget::Agent(intent.key.clone())));
        self.mode = SelectorMode::ClosingRuntime {
            agent_key: intent.key.clone(),
            agent_name,
            action: intent.action,
        };
        self.notice = None;
        self.warning = None;
    }

    fn runtime_close_succeeded(&mut self, snapshot: SelectorSnapshot) {
        let Some((agent_key, agent_name, action)) = self.runtime_close_identity() else {
            return;
        };
        let target = if action == SessionTuiAction::RetireSession {
            SelectorTarget::RetiredSessions
        } else {
            SelectorTarget::Agent(agent_key)
        };
        self.mode = SelectorMode::Agents;
        self.workspace_selection.select(Some(target.clone()));
        self.workspace_selection.mark_transiently_visible(
            matches!(
                action,
                SessionTuiAction::CloseRuntime | SessionTuiAction::RestoreSession
            )
            .then_some(target),
        );
        self.replace_snapshot(snapshot);
        self.notice = Some(match action {
            SessionTuiAction::CloseRuntime => format!("Runtime closed: {agent_name}"),
            SessionTuiAction::RetireSession => format!("Retired session: {agent_name}"),
            SessionTuiAction::RestoreSession => format!("Restored offline: {agent_name}"),
            _ => unreachable!("only selector actions enter the operation worker"),
        });
    }

    fn runtime_close_refresh_failed(&mut self, message: String) {
        let Some((agent_key, agent_name, action)) = self.runtime_close_identity() else {
            return;
        };
        if action != SessionTuiAction::CloseRuntime {
            self.mode = SelectorMode::Agents;
            self.notice = None;
            self.warning = Some(format!(
                "{} completed, but refresh failed: {message}; reopen Retired sessions to resync",
                action.label()
            ));
            return;
        }
        let target = SelectorTarget::Agent(agent_key);
        if let Some(row) = self.rows.iter_mut().find(|row| row.target == target) {
            row.lifecycle = Some(CutexSessionLifecycleState::Offline);
            row.attachable = false;
            row.actions.clear();
        }
        self.mode = SelectorMode::Agents;
        self.workspace_selection.select(Some(target.clone()));
        self.workspace_selection
            .mark_transiently_visible(Some(target));
        self.ensure_selection();
        self.notice = Some(format!("Runtime closed: {agent_name}"));
        self.warning = Some(format!(
            "Runtime closed, but live refresh failed: {message}"
        ));
    }

    fn runtime_close_failed(&mut self, message: String) {
        let agent_name = self
            .runtime_close_identity()
            .map(|(_, agent_name, _)| agent_name)
            .unwrap_or_else(|| "selected agent".to_string());
        self.mode = SelectorMode::Agents;
        self.notice = None;
        self.warning = Some(format!(
            "Failed to close runtime for {agent_name}: {message}"
        ));
    }

    fn runtime_close_identity(&self) -> Option<(String, String, SessionTuiAction)> {
        match &self.mode {
            SelectorMode::ClosingRuntime {
                agent_key,
                agent_name,
                action,
            } => Some((agent_key.clone(), agent_name.clone(), *action)),
            _ => None,
        }
    }

    fn refresh_activity_states(&mut self, activity_states: &HashMap<String, SessionActivityState>) {
        for row in &mut self.rows {
            row.last_output_at = row
                .activity_session_id
                .as_deref()
                .and_then(|session_id| activity_states.get(session_id))
                .and_then(|activity| activity.last_output_at.clone());
        }
    }

    fn replace_snapshot(&mut self, snapshot: SelectorSnapshot) {
        let active_settings_target = match &self.mode {
            SelectorMode::Settings { target, .. } => Some(target.clone()),
            _ => None,
        };
        self.rows = snapshot.rows;
        sort_rows(&mut self.rows);
        let mut settings_warning = None;
        if let Some(settings_override) = self.pending_settings_refresh_override.take() {
            if let Some(row) = self
                .rows
                .iter_mut()
                .find(|row| row.target == settings_override.target)
            {
                row.settings = settings_override
                    .snapshot
                    .categories(&SessionSettingsDraft::default());
                row.settings_snapshot = Some(settings_override.snapshot);
                row.agent = settings_override.agent;
                row.configured_profile = settings_override.configured_profile;
                row.backend = settings_override.backend;
                row.pinned = settings_override.pinned;
                row.managed = settings_override.managed;
                if let Some(actions) = settings_override.actions {
                    row.actions = actions;
                }
            }
            settings_warning = settings_override.warning;
            sort_rows(&mut self.rows);
        }
        if let Some(settings_override) = self.pending_global_settings_refresh_override.take() {
            for row in self
                .rows
                .iter_mut()
                .filter(|row| row.target.uses_global_settings())
            {
                row.settings = settings_override
                    .snapshot
                    .categories(&GlobalSettingsDraft::default());
                if row.target.is_profiles() {
                    row.settings.clear();
                }
                row.global_settings_snapshot = Some(settings_override.snapshot.clone());
            }
        }
        if let Some(profile_override) = self.pending_profile_refresh_override.take() {
            self.apply_profile_projection(&profile_override.projection);
        }
        self.refreshing = false;
        let warning = combine_warnings(snapshot.warning, settings_warning);
        self.warning = combine_warnings(warning, self.pending_startup_warning.take());
        self.ensure_selection();
        if let Some(target) = active_settings_target {
            self.reproject_settings(&target);
        }
        self.normalize_mode_after_snapshot();
    }

    fn mark_refresh_failed(&mut self, message: String) {
        self.refreshing = false;
        self.pending_settings_refresh_override = None;
        self.pending_global_settings_refresh_override = None;
        self.pending_profile_refresh_override = None;
        self.warning = combine_warnings(Some(message), self.pending_startup_warning.take());
    }

    fn settings_apply_succeeded(
        &mut self,
        key: &str,
        record: &CutexSessionRecord,
        profile_names: &[String],
        changed_count: usize,
        launch_actions_changed: bool,
        warning: Option<String>,
    ) {
        let target = SelectorTarget::Agent(key.to_string());
        let snapshot = SessionSettingsSnapshot::from_record_with_profiles(record, profile_names);
        let actions = launch_actions_changed.then(|| {
            let attachable = self
                .rows
                .iter()
                .find(|row| row.target == target)
                .is_some_and(|row| row.attachable);
            settings_actions_for_record(record, attachable)
        });
        if self.refreshing {
            self.pending_settings_refresh_override = Some(PendingSettingsRefreshOverride {
                target: target.clone(),
                snapshot: snapshot.clone(),
                agent: cutex_session_display_name(record),
                configured_profile: record.profile.clone(),
                backend: runtime_backend_short_label(record.runtime_backend).to_string(),
                pinned: record.quick_action == CutexSessionQuickActionMode::Pinned,
                managed: cutex_session_is_managed(record),
                actions: actions.clone(),
                warning: warning.clone(),
            });
        }
        if let Some(row) = self.rows.iter_mut().find(|row| row.target == target) {
            row.settings = snapshot.categories(&SessionSettingsDraft::default());
            row.settings_snapshot = Some(snapshot);
            row.agent = cutex_session_display_name(record);
            row.configured_profile = record.profile.clone();
            row.backend = runtime_backend_short_label(record.runtime_backend).to_string();
            row.pinned = record.quick_action == CutexSessionQuickActionMode::Pinned;
            row.managed = cutex_session_is_managed(record);
            if let Some(actions) = actions {
                row.actions = actions;
            }
        }
        sort_rows(&mut self.rows);
        self.settings_draft = SessionSettingsDraft::default();
        self.settings_overlay = None;
        if self
            .warning
            .as_deref()
            .is_some_and(|warning| warning.starts_with("settings apply failed:"))
        {
            self.warning = None;
        }
        if warning.is_some() {
            self.warning = warning;
        }
        self.notice = Some(format!("Saved {changed_count} setting(s)"));
    }

    fn global_settings_apply_succeeded(
        &mut self,
        config: &CodezConfig,
        profile_names: &[String],
        changed_count: usize,
    ) {
        let snapshot = GlobalSettingsSnapshot::from_config_with_profiles(config, profile_names);
        if self.refreshing {
            self.pending_global_settings_refresh_override =
                Some(PendingGlobalSettingsRefreshOverride {
                    snapshot: snapshot.clone(),
                });
        }
        for row in self
            .rows
            .iter_mut()
            .filter(|row| row.target.uses_global_settings())
        {
            row.settings = snapshot.categories(&GlobalSettingsDraft::default());
            if row.target.is_profiles() {
                row.settings.clear();
            }
            row.global_settings_snapshot = Some(snapshot.clone());
        }
        self.global_settings_draft = GlobalSettingsDraft::default();
        self.settings_overlay = None;
        if self
            .warning
            .as_deref()
            .is_some_and(|warning| warning.starts_with("global settings apply failed:"))
        {
            self.warning = None;
        }
        self.notice = Some(format!("Saved {changed_count} setting(s)"));
    }

    fn global_settings_apply_failed(&mut self, message: String) {
        self.warning = Some(format!("global settings apply failed: {message}"));
        self.notice = None;
    }

    fn session_management_succeeded(
        &mut self,
        key: &str,
        command: SessionSettingsCommand,
        record: &CutexSessionRecord,
        profile_names: &[String],
        warning: Option<String>,
    ) {
        let target = SelectorTarget::Agent(key.to_string());
        let snapshot = SessionSettingsSnapshot::from_record_with_profiles(record, profile_names);
        let attachable = self
            .rows
            .iter()
            .find(|row| row.target == target)
            .is_some_and(|row| row.attachable);
        let actions = settings_actions_for_record(record, attachable);
        let managed = cutex_session_is_managed(record);
        if self.refreshing {
            self.pending_settings_refresh_override = Some(PendingSettingsRefreshOverride {
                target: target.clone(),
                snapshot: snapshot.clone(),
                agent: cutex_session_display_name(record),
                configured_profile: record.profile.clone(),
                backend: runtime_backend_short_label(record.runtime_backend).to_string(),
                pinned: record.quick_action == CutexSessionQuickActionMode::Pinned,
                managed,
                actions: Some(actions.clone()),
                warning: warning.clone(),
            });
        }
        if let Some(row) = self.rows.iter_mut().find(|row| row.target == target) {
            row.settings = snapshot.categories(&SessionSettingsDraft::default());
            row.settings_snapshot = Some(snapshot);
            row.agent = cutex_session_display_name(record);
            row.configured_profile = record.profile.clone();
            row.backend = runtime_backend_short_label(record.runtime_backend).to_string();
            row.pinned = record.quick_action == CutexSessionQuickActionMode::Pinned;
            row.managed = managed;
            row.actions = actions;
        }
        sort_rows(&mut self.rows);
        self.settings_draft = SessionSettingsDraft::default();
        self.settings_overlay = None;
        if self
            .warning
            .as_deref()
            .is_some_and(|warning| warning.starts_with("management change failed:"))
        {
            self.warning = None;
        }
        if warning.is_some() {
            self.warning = warning;
        }
        self.notice = Some(command.success_notice().to_string());
    }

    fn session_management_failed(&mut self, message: String) {
        self.warning = Some(format!("management change failed: {message}"));
        self.notice = None;
    }

    fn profile_management_succeeded(&mut self, result: ProfileManagementResult) {
        if self.refreshing {
            self.pending_profile_refresh_override = Some(PendingProfileRefreshOverride {
                projection: result.projection.clone(),
            });
        }
        self.apply_profile_projection(&result.projection);
        let SelectorMode::ProfileManager {
            selected,
            focus,
            editor_selected,
            ..
        } = self.mode.clone()
        else {
            return;
        };
        let selected = result
            .preferred_profile_id
            .as_deref()
            .and_then(|profile_id| {
                result
                    .profiles
                    .iter()
                    .position(|profile| profile.id == profile_id)
                    .map(|index| index + 1)
            })
            .unwrap_or_else(|| selected.min(result.profiles.len().saturating_add(1)));
        self.mode = SelectorMode::ProfileManager {
            profiles: result.profiles,
            selected,
            focus,
            editor_selected,
        };
        self.profile_settings_draft = ProfileSettingsDraft::default();
        self.settings_overlay = None;
        self.clamp_profile_editor_selection();
        self.profile_overlay = None;
        if self
            .warning
            .as_deref()
            .is_some_and(|warning| warning.starts_with("profile change failed:"))
        {
            self.warning = None;
        }
        self.notice = Some(result.notice);
    }

    fn profile_management_failed(&mut self, request: &ProfileManagementRequest, message: String) {
        self.warning = Some(format!("profile change failed: {message}"));
        self.notice = None;
        match &request.command {
            ProfileManagementCommand::Rename { new_name } => {
                self.profile_overlay = Some(ProfileOverlay::RenameInput {
                    profile_id: request.profile_id.clone(),
                    old_name: request.profile_name.clone(),
                    input: Input::new(new_name.clone()),
                });
            }
            ProfileManagementCommand::Remove => {
                self.profile_overlay = Some(ProfileOverlay::ConfirmRemove {
                    profile_id: request.profile_id.clone(),
                    profile_name: request.profile_name.clone(),
                    selected: 0,
                });
            }
            ProfileManagementCommand::Activate => {}
        }
    }

    fn profile_settings_apply_failed(&mut self, message: String) {
        self.warning = Some(format!("profile settings apply failed: {message}"));
        self.notice = None;
        self.settings_overlay = None;
    }

    fn profile_management_refresh_failed(&mut self, notice: String, message: String) {
        self.notice = Some(notice);
        self.warning = Some(format!("Profile changed, but UI refresh failed: {message}"));
        self.profile_settings_draft = ProfileSettingsDraft::default();
        self.settings_overlay = None;
        self.profile_overlay = None;
    }

    fn apply_profile_projection(&mut self, projection: &ProfileProjectionSnapshot) {
        for row in &mut self.rows {
            if row.target.uses_global_settings() {
                let snapshot = GlobalSettingsSnapshot::from_config_with_profiles(
                    &projection.config,
                    &projection.profile_names,
                );
                row.settings = snapshot.categories(&GlobalSettingsDraft::default());
                if row.target.is_profiles() {
                    row.settings.clear();
                }
                row.global_settings_snapshot = Some(snapshot);
                continue;
            }
            let Some(key) = row.target.agent_key() else {
                continue;
            };
            let Some(record) = projection.records.get(key) else {
                continue;
            };
            let snapshot = SessionSettingsSnapshot::from_record_with_profiles(
                record,
                &projection.profile_names,
            );
            row.settings = snapshot.categories(&SessionSettingsDraft::default());
            row.settings_snapshot = Some(snapshot);
            row.configured_profile = record.profile.clone();
        }
    }

    fn settings_apply_failed(&mut self, message: String) {
        self.warning = Some(format!("settings apply failed: {message}"));
        self.notice = None;
    }

    fn ensure_selection(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.workspace_selection.select(None);
            return;
        }
        let selection_still_visible = self.workspace_selection.selected().is_some_and(|target| {
            visible
                .iter()
                .any(|index| self.rows[*index].target == *target)
        });
        if !selection_still_visible {
            self.workspace_selection
                .select(Some(self.rows[visible[0]].target.clone()));
        }
    }

    fn normalize_mode_after_snapshot(&mut self) {
        match self.mode.clone() {
            SelectorMode::Agents => {}
            SelectorMode::RecentSessions => {}
            SelectorMode::RetiredSessions { selected } => {
                self.mode = SelectorMode::RetiredSessions {
                    selected: selected.min(self.retired_rows.len().saturating_sub(1)),
                };
            }
            SelectorMode::Actions {
                agent_key,
                selected,
                launch_profile,
            } => {
                let Some((actions_empty, control_count, launch_profile_valid)) = self
                    .rows
                    .iter()
                    .find(|row| row.target.agent_key() == Some(agent_key.as_str()))
                    .map(|row| {
                        let valid = launch_profile.as_ref().is_none_or(|profile| {
                            row.launch_profile_control_available()
                                && row.settings_snapshot.as_ref().is_some_and(|snapshot| {
                                    snapshot.profile_names().contains(profile)
                                })
                        });
                        (row.actions.is_empty(), row.action_control_count(), valid)
                    })
                else {
                    self.action_overlay = None;
                    self.mode = SelectorMode::Agents;
                    return;
                };
                if actions_empty {
                    self.action_overlay = None;
                    self.mode = SelectorMode::Agents;
                } else {
                    let launch_profile = if launch_profile_valid {
                        launch_profile
                    } else {
                        self.action_overlay = None;
                        self.warning =
                            Some("Selected one-launch profile is no longer available".to_string());
                        None
                    };
                    self.mode = SelectorMode::Actions {
                        agent_key,
                        selected: selected.min(control_count.saturating_sub(1)),
                        launch_profile,
                    };
                }
            }
            SelectorMode::Settings {
                target,
                category,
                option,
                focus,
                view,
            } => {
                let Some(row) = self.rows.iter().find(|row| row.target == target) else {
                    self.mode = SelectorMode::Agents;
                    return;
                };
                if row.settings.is_empty() {
                    self.mode = SelectorMode::Agents;
                } else {
                    let category = category.min(row.settings.len() - 1);
                    let option = option.min(row.settings[category].options.len().saturating_sub(1));
                    self.mode = SelectorMode::Settings {
                        target,
                        category,
                        option,
                        focus,
                        view,
                    };
                }
            }
            SelectorMode::ProfileManager {
                profiles,
                selected,
                focus,
                editor_selected,
            } => {
                let option_count = if selected == 0 {
                    2
                } else if selected <= profiles.len() {
                    self.profile_editor_option_count()
                } else {
                    1
                };
                self.mode = SelectorMode::ProfileManager {
                    selected: selected.min(profiles.len().saturating_add(1)),
                    profiles,
                    focus,
                    editor_selected: editor_selected.min(option_count.saturating_sub(1)),
                };
            }
            SelectorMode::ConfirmRuntimeAction {
                agent_key,
                action,
                launch_profile,
                confirmed,
            } => {
                let still_available = self
                    .rows
                    .iter()
                    .find(|row| row.target.agent_key() == Some(agent_key.as_str()))
                    .is_some_and(|row| {
                        row.control_index_for_action(action).is_some()
                            && launch_profile.as_ref().is_none_or(|profile| {
                                row.action_supports_launch_profile(action)
                                    && row.settings_snapshot.as_ref().is_some_and(|snapshot| {
                                        snapshot.profile_names().contains(profile)
                                    })
                            })
                    });
                if !still_available {
                    self.mode = SelectorMode::Agents;
                } else {
                    self.mode = SelectorMode::ConfirmRuntimeAction {
                        agent_key,
                        action,
                        launch_profile,
                        confirmed,
                    };
                }
            }
            SelectorMode::ClosingRuntime { .. } => {}
        }
    }

    fn move_selection(&mut self, direction: isize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.workspace_selection.select(None);
            return;
        }
        let current = self.selected_visible_index().unwrap_or(0);
        let next = wrapped_index(current, direction, visible.len());
        self.workspace_selection
            .select(Some(self.rows[visible[next]].target.clone()));
    }

    fn select_edge(&mut self, last: bool) {
        let visible = self.visible_indices();
        let index = if last {
            visible.last()
        } else {
            visible.first()
        };
        self.workspace_selection
            .select(index.map(|index| self.rows[*index].target.clone()));
    }
}

fn wrapped_index(current: usize, direction: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if direction < 0 {
        if current == 0 {
            len - 1
        } else {
            current - 1
        }
    } else if direction > 0 {
        (current + 1) % len
    } else {
        current.min(len - 1)
    }
}

fn setting_option_count(settings: &[SessionTuiSettingCategory]) -> usize {
    settings.iter().map(|category| category.options.len()).sum()
}

fn setting_flat_index(
    settings: &[SessionTuiSettingCategory],
    category: usize,
    option: usize,
) -> Option<usize> {
    let current = settings.get(category)?;
    (option < current.options.len()).then(|| {
        settings[..category]
            .iter()
            .map(|category| category.options.len())
            .sum::<usize>()
            + option
    })
}

fn setting_indices_at_flat_index(
    settings: &[SessionTuiSettingCategory],
    mut flat_index: usize,
) -> Option<(usize, usize)> {
    for (category_index, category) in settings.iter().enumerate() {
        if flat_index < category.options.len() {
            return Some((category_index, flat_index));
        }
        flat_index = flat_index.saturating_sub(category.options.len());
    }
    None
}

fn moved_flat_setting(
    settings: &[SessionTuiSettingCategory],
    category: usize,
    option: usize,
    direction: isize,
) -> Option<(usize, usize)> {
    let count = setting_option_count(settings);
    let current = setting_flat_index(settings, category, option)?;
    setting_indices_at_flat_index(settings, wrapped_index(current, direction, count))
}

fn expanded_setting_table_row_index(
    settings: &[SessionTuiSettingCategory],
    category: usize,
    option: usize,
) -> Option<usize> {
    let current = settings.get(category)?;
    (option < current.options.len()).then(|| {
        settings[..category]
            .iter()
            .map(|category| category.options.len() + 1)
            .sum::<usize>()
            + option
            + 1
    })
}

pub(crate) fn run() -> anyhow::Result<()> {
    require_interactive_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())?;

    let mut startup = None;
    loop {
        match run_terminal_cycle(startup.take())? {
            SessionTuiCycleOutcome::Exit => return Ok(()),
            SessionTuiCycleOutcome::Selected(intent) => {
                return super::session_tui_dispatch::dispatch_session_tui_intent(intent);
            }
            SessionTuiCycleOutcome::LoginProfile => {
                startup = Some(profile_login_startup(super::auth::login_interactive()));
            }
            SessionTuiCycleOutcome::CutexProjects => super::session_tui_cutex_projects::run()?,
            SessionTuiCycleOutcome::Projects => super::session_tui_projects::run()?,
        }
    }
}

fn run_terminal_cycle(
    startup: Option<ProfileManagerStartup>,
) -> anyhow::Result<SessionTuiCycleOutcome> {
    let store = load_reconciled_session_store()?;
    let config = load_codez_config();
    let (profile_names, profile_warning) = profile_names_with_warning();
    let (activity_states, activity_warning) = activity_states_with_warning();
    let initial_rows =
        selector_rows_from_store(&store, &[], &[], &config, &profile_names, &activity_states);
    let mut refresh = spawn_snapshot_refresh()?;
    let recent_catalog = RecentCatalog::spawn()?;
    let startup_profiles = startup.as_ref().map(|_| load_profile_catalog_read_only());
    let (mut terminal, restore, enhanced_keyboard) = open_terminal()?;
    let mut model = SelectorModel::new(initial_rows, refresh.is_loading(), enhanced_keyboard);
    model.warning = combine_warnings(profile_warning, activity_warning);
    if let Some(startup) = startup {
        match startup_profiles.expect("profile startup catalog should exist") {
            Ok(profiles) => model.open_profile_manager(profiles),
            Err(error) => model.profile_manager_failed(format!("{error:#}")),
        }
        model.notice = startup.notice;
        model.pending_startup_warning = startup.warning.clone();
        model.warning = combine_warnings(model.warning.take(), startup.warning);
    }

    let result = run_event_loop(&mut terminal, &mut model, &mut refresh, &recent_catalog);
    drop(terminal);
    drop(restore);
    result
}

fn profile_login_startup(result: anyhow::Result<()>) -> ProfileManagerStartup {
    match result {
        Ok(()) => ProfileManagerStartup {
            notice: Some("Profile added".to_string()),
            warning: None,
        },
        Err(error) => ProfileManagerStartup {
            notice: None,
            warning: Some(format!("Profile login did not complete: {error:#}")),
        },
    }
}

fn require_interactive_terminal(
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> anyhow::Result<()> {
    if stdin_is_terminal && stdout_is_terminal {
        Ok(())
    } else {
        anyhow::bail!("`cutex tui` requires an interactive terminal on stdin and stdout")
    }
}

fn spawn_snapshot_refresh() -> anyhow::Result<WorkspaceLoad<SelectorSnapshot>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("cutex-tui-refresh".to_string())
        .spawn(move || {
            let snapshot =
                load_live_snapshot().map_err(|error| format!("live refresh failed: {error:#}"));
            let _ = sender.send(snapshot);
        })
        .context("Failed to start Cutex TUI refresh worker")?;
    Ok(WorkspaceLoad::new(receiver))
}

fn spawn_runtime_close(
    intent: SessionTuiIntent,
) -> anyhow::Result<Receiver<RuntimeCloseWorkerResult>> {
    debug_assert_eq!(intent.action, SessionTuiAction::CloseRuntime);
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("cutex-tui-close".to_string())
        .spawn(move || {
            let result = match super::session_tui_dispatch::dispatch_session_tui_intent_in_selector(
                intent,
            ) {
                Ok(()) => match load_live_snapshot() {
                    Ok(snapshot) => RuntimeCloseWorkerResult::Closed(snapshot),
                    Err(error) => {
                        RuntimeCloseWorkerResult::ClosedRefreshFailed(format!("{error:#}"))
                    }
                },
                Err(error) => RuntimeCloseWorkerResult::Failed(format!("{error:#}")),
            };
            let _ = sender.send(result);
        })
        .context("Failed to start Cutex TUI runtime close worker")?;
    Ok(receiver)
}

fn load_live_snapshot() -> anyhow::Result<SelectorSnapshot> {
    let store = load_reconciled_session_store()?;
    let (alden_sessions, alden_warning) = match cute_alden_sessions() {
        Ok(sessions) => (sessions, None),
        Err(error) => (
            Vec::new(),
            Some(format!("cute-alden live state unavailable: {error:#}")),
        ),
    };
    let (profile_names, profile_warning) = profile_names_with_warning();
    let (activity_states, activity_warning) = activity_states_with_warning();
    let config = load_codez_config();
    let live_agents = agent_bus_fetch_agents_if_healthy(&config);
    Ok(SelectorSnapshot {
        rows: selector_rows_from_store(
            &store,
            &alden_sessions,
            &live_agents,
            &config,
            &profile_names,
            &activity_states,
        ),
        warning: combine_warnings(
            combine_warnings(alden_warning, profile_warning),
            activity_warning,
        ),
    })
}

fn load_reconciled_session_store() -> anyhow::Result<CutexSessionStore> {
    load_reconciled_session_store_with(
        cutex::im::registry::load_im_registry,
        super::session_reconcile::mirror_im_registry_into_cutex_session_store,
        load_cutex_session_store,
    )
}

fn load_reconciled_session_store_with(
    load_registry: impl FnOnce() -> anyhow::Result<cutex::im::registry::ImRegistry>,
    reconcile: impl FnOnce(&cutex::im::registry::ImRegistry) -> anyhow::Result<()>,
    load_store: impl FnOnce() -> anyhow::Result<CutexSessionStore>,
) -> anyhow::Result<CutexSessionStore> {
    let registry = load_registry()?;
    reconcile(&registry)?;
    load_store()
}

fn load_retired_selector_rows() -> anyhow::Result<Vec<SelectorRow>> {
    let store = load_cutex_session_store()?;
    Ok(retired_selector_rows_from_store(&store))
}

fn profile_names_with_warning() -> (Vec<String>, Option<String>) {
    match load_profile_names_read_only() {
        Ok(names) => (names, None),
        Err(error) => (
            Vec::new(),
            Some(format!("profile catalog unavailable: {error:#}")),
        ),
    }
}

fn activity_states_with_warning() -> (HashMap<String, SessionActivityState>, Option<String>) {
    match load_session_activity_states() {
        Ok(states) => (states, None),
        Err(error) => (
            HashMap::new(),
            Some(format!("session activity unavailable: {error:#}")),
        ),
    }
}

fn combine_warnings(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(format!("{left}; {right}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    }
}

fn selector_rows_from_store(
    store: &CutexSessionStore,
    alden_sessions: &[CuteAldenSession],
    live_agents: &[AgentBusAgent],
    config: &CodezConfig,
    profile_names: &[String],
    activity_states: &HashMap<String, SessionActivityState>,
) -> Vec<SelectorRow> {
    let mut rows = store
        .sessions
        .iter()
        .filter(|(_, record)| record.is_active())
        .map(|(key, record)| {
            let mut row = selector_row(key, record, alden_sessions, live_agents, profile_names);
            row.last_output_at = activity_states
                .get(&record.cutex_session_id)
                .and_then(|activity| activity.last_output_at.clone());
            row
        })
        .collect::<Vec<_>>();
    rows.push(retired_sessions_row(
        store
            .sessions
            .values()
            .filter(|record| record.is_retired() && cutex_session_is_managed(record))
            .count(),
    ));
    rows.push(recent_sessions_row());
    rows.push(cutex_projects_row());
    rows.push(projects_row());
    rows.push(profiles_row(config, profile_names));
    rows.push(global_settings_row_with_profiles(config, profile_names));
    rows
}

fn retired_selector_rows_from_store(store: &CutexSessionStore) -> Vec<SelectorRow> {
    let mut rows = store
        .sessions
        .iter()
        .filter(|(_, record)| record.is_retired() && cutex_session_is_managed(record))
        .map(|(key, record)| retired_selector_row(key, record))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.agent.cmp(&right.agent));
    rows
}

fn retired_selector_row(key: &str, record: &CutexSessionRecord) -> SelectorRow {
    SelectorRow {
        target: SelectorTarget::RetiredAgent(key.to_string()),
        agent: cutex_session_display_name(record),
        configured_profile: record.profile.clone(),
        lifecycle: Some(CutexSessionLifecycleState::Offline),
        host: nonempty_or_dash(&record.host_id),
        backend: runtime_backend_short_label(record.runtime_backend).to_string(),
        managed_path: record
            .managed_cwd
            .as_deref()
            .map(compact_home_path)
            .unwrap_or_else(|| "-".to_string()),
        retired_at: record.retired_at.clone(),
        revision: record.durable_revision(),
        activity_session_id: None,
        last_output_at: None,
        actions: vec![SessionTuiActionItem {
            action: SessionTuiAction::RestoreSession,
            detail: "Restore active and offline without launching",
            primary: true,
        }],
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: None,
        attachable: false,
        pinned: false,
        managed: true,
    }
}

fn selector_row(
    key: &str,
    record: &CutexSessionRecord,
    alden_sessions: &[CuteAldenSession],
    live_agents: &[AgentBusAgent],
    profile_names: &[String],
) -> SelectorRow {
    let settings_snapshot =
        SessionSettingsSnapshot::from_record_with_profiles(record, profile_names);
    let settings = settings_snapshot.categories(&SessionSettingsDraft::default());
    let mut actions = session_tui_actions_for_record(record, alden_sessions, live_agents);
    if cutex_session_is_managed(record) {
        actions.push(SessionTuiActionItem {
            action: SessionTuiAction::RetireSession,
            detail: "Archive this managed session after proving its runtime is offline",
            primary: false,
        });
    }
    SelectorRow {
        target: SelectorTarget::Agent(key.to_string()),
        agent: cutex_session_display_name(record),
        configured_profile: record.profile.clone(),
        lifecycle: Some(cutex_session_lifecycle_state_with_agents(
            record,
            alden_sessions,
            live_agents,
        )),
        host: nonempty_or_dash(&record.host_id),
        backend: runtime_backend_short_label(record.runtime_backend).to_string(),
        managed_path: record
            .managed_cwd
            .as_deref()
            .map(compact_home_path)
            .unwrap_or_else(|| "-".to_string()),
        retired_at: None,
        revision: record.durable_revision(),
        activity_session_id: Some(record.cutex_session_id.clone()),
        last_output_at: None,
        actions,
        settings,
        settings_snapshot: Some(settings_snapshot),
        global_settings_snapshot: None,
        attachable: cutex_session_is_attachable(record, alden_sessions),
        pinned: record.quick_action == CutexSessionQuickActionMode::Pinned,
        managed: cutex_session_is_managed(record),
    }
}

fn retired_sessions_row(retired_count: usize) -> SelectorRow {
    SelectorRow {
        target: SelectorTarget::RetiredSessions,
        agent: format!("Retired sessions ({retired_count})"),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "archive".to_string(),
        managed_path: "-".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: None,
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn projects_row() -> SelectorRow {
    SelectorRow {
        target: SelectorTarget::Projects,
        agent: "Workspaces".to_string(),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "Codex catalog".to_string(),
        managed_path: "paired app-server".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: None,
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn cutex_projects_row() -> SelectorRow {
    SelectorRow {
        target: SelectorTarget::CutexProjects,
        agent: "Cutex Projects".to_string(),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "permission model".to_string(),
        managed_path: "Agent Management provider".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: None,
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn recent_sessions_row() -> SelectorRow {
    SelectorRow {
        target: SelectorTarget::RecentSessions,
        agent: "Recent sessions".to_string(),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "native catalog".to_string(),
        managed_path: "-".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: None,
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn settings_actions_for_record(
    record: &CutexSessionRecord,
    currently_attachable: bool,
) -> Vec<SessionTuiActionItem> {
    let alden_sessions = if currently_attachable {
        record
            .alden_pid
            .zip(record.alden_session_name.clone())
            .map(|(pid, name)| CuteAldenSession {
                pid,
                name: Some(name),
            })
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    session_tui_actions_for_record(record, &alden_sessions, &[])
}

#[cfg(test)]
fn global_settings_row(config: &CodezConfig) -> SelectorRow {
    global_settings_row_with_profiles(config, &[])
}

fn global_settings_row_with_profiles(
    config: &CodezConfig,
    profile_names: &[String],
) -> SelectorRow {
    let settings_snapshot =
        GlobalSettingsSnapshot::from_config_with_profiles(config, profile_names);
    SelectorRow {
        target: SelectorTarget::GlobalSettings,
        agent: "Global settings".to_string(),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "config".to_string(),
        managed_path: "-".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: settings_snapshot.categories(&GlobalSettingsDraft::default()),
        settings_snapshot: None,
        global_settings_snapshot: Some(settings_snapshot),
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn profiles_row(config: &CodezConfig, profile_names: &[String]) -> SelectorRow {
    let settings_snapshot =
        GlobalSettingsSnapshot::from_config_with_profiles(config, profile_names);
    SelectorRow {
        target: SelectorTarget::Profiles,
        agent: "Profiles".to_string(),
        configured_profile: None,
        lifecycle: None,
        host: "-".to_string(),
        backend: "accounts".to_string(),
        managed_path: "-".to_string(),
        retired_at: None,
        revision: 0,
        activity_session_id: None,
        last_output_at: None,
        actions: Vec::new(),
        settings: Vec::new(),
        settings_snapshot: None,
        global_settings_snapshot: Some(settings_snapshot),
        attachable: false,
        pinned: false,
        managed: false,
    }
}

fn nonempty_or_dash(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn sort_rows(rows: &mut [SelectorRow]) {
    rows.sort_by(|left, right| {
        match (
            system_row_rank(&left.target),
            system_row_rank(&right.target),
        ) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => lifecycle_rank(left.lifecycle.expect("agent lifecycle"))
                .cmp(&lifecycle_rank(right.lifecycle.expect("agent lifecycle")))
                .then_with(|| right.pinned.cmp(&left.pinned))
                .then_with(|| left.agent.to_lowercase().cmp(&right.agent.to_lowercase()))
                .then_with(|| left.target.agent_key().cmp(&right.target.agent_key())),
        }
    });
}

fn system_row_rank(target: &SelectorTarget) -> Option<u8> {
    match target {
        SelectorTarget::Agent(_) | SelectorTarget::RetiredAgent(_) => None,
        SelectorTarget::RecentSessions => Some(0),
        SelectorTarget::RetiredSessions => Some(1),
        SelectorTarget::CutexProjects => Some(2),
        SelectorTarget::Projects => Some(3),
        SelectorTarget::Profiles => Some(4),
        SelectorTarget::GlobalSettings => Some(5),
    }
}

fn lifecycle_rank(state: CutexSessionLifecycleState) -> u8 {
    match state {
        CutexSessionLifecycleState::Online => 0,
        CutexSessionLifecycleState::Stale => 1,
        CutexSessionLifecycleState::Offline => 2,
    }
}

fn open_terminal() -> anyhow::Result<(CutexTerminal, TerminalRestore, bool)> {
    enable_raw_mode().context("Failed to enable terminal raw mode")?;
    let enhanced_keyboard = terminal_may_support_enhanced_keyboard()
        && supports_keyboard_enhancement().unwrap_or(false);
    let mut restore = TerminalRestore {
        enhanced_keyboard: false,
    };
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    if enhanced_keyboard {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .context("Failed to enable enhanced keyboard events")?;
        restore.enhanced_keyboard = true;
    }
    let terminal = Terminal::new(CrosstermBackend::new(stdout))
        .context("Failed to initialize Cutex TUI terminal")?;
    Ok((terminal, restore, enhanced_keyboard))
}

fn terminal_may_support_enhanced_keyboard() -> bool {
    terminal_environment_may_support_enhancement(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        [
            "KITTY_WINDOW_ID",
            "WEZTERM_PANE",
            "ALACRITTY_WINDOW_ID",
            "FOOT_CLIENT_PID",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some()),
    )
}

fn terminal_environment_may_support_enhancement(
    term: Option<&str>,
    term_program: Option<&str>,
    known_terminal_marker: bool,
) -> bool {
    known_terminal_marker
        || [term, term_program]
            .into_iter()
            .flatten()
            .map(str::to_ascii_lowercase)
            .any(|value| {
                ["kitty", "foot", "wezterm", "alacritty"]
                    .iter()
                    .any(|name| value.contains(name))
            })
}

struct TerminalRestore {
    enhanced_keyboard: bool,
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        if self.enhanced_keyboard {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }
}

fn run_event_loop(
    terminal: &mut CutexTerminal,
    model: &mut SelectorModel,
    refresh: &mut WorkspaceLoad<SelectorSnapshot>,
    recent_catalog: &RecentCatalog,
) -> anyhow::Result<SessionTuiCycleOutcome> {
    let mut runtime_close = None;
    let mut next_activity_refresh = Instant::now() + ACTIVITY_REFRESH_INTERVAL;
    loop {
        let now = Instant::now();
        if now >= next_activity_refresh {
            if let Ok(activity_states) = load_session_activity_states() {
                model.refresh_activity_states(&activity_states);
            }
            next_activity_refresh = now + ACTIVITY_REFRESH_INTERVAL;
        }
        receive_refresh(model, refresh);
        if let Some(reply) = recent_catalog.poll() {
            model.recent_catalog_reply(reply);
        }
        if receive_runtime_close(model, &mut runtime_close) {
            terminal.clear()?;
        }
        terminal.draw(|frame| render_selector(frame, model))?;

        if !event::poll(EVENT_POLL_INTERVAL)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) => {
                let control = if close_runtime_shortcut_from_key(key) {
                    Some(model.activate_close_shortcut())
                } else {
                    selector_event_from_key(key, model.enhanced_keyboard)
                        .map(|selector_event| model.handle(selector_event))
                };
                if let Some(control) = control {
                    match control {
                        SelectorControl::Continue => {}
                        SelectorControl::Exit => return Ok(SessionTuiCycleOutcome::Exit),
                        SelectorControl::Selected(intent) if intent_runs_in_selector(&intent) => {
                            model.runtime_close_started(&intent);
                            match spawn_runtime_close(intent) {
                                Ok(receiver) => runtime_close = Some(receiver),
                                Err(error) => model.runtime_close_failed(format!("{error:#}")),
                            }
                        }
                        SelectorControl::Selected(intent) => {
                            return Ok(SessionTuiCycleOutcome::Selected(intent));
                        }
                        SelectorControl::OpenRetiredSessions => {
                            match load_retired_selector_rows() {
                                Ok(rows) => model.open_retired_sessions(rows),
                                Err(error) => {
                                    model.warning =
                                        Some(format!("retired sessions unavailable: {error:#}"))
                                }
                            }
                        }
                        SelectorControl::OpenRecentSessions => {
                            model.mode = SelectorMode::RecentSessions;
                            model.notice = None;
                        }
                        SelectorControl::Recent(command) => {
                            let cursor = model.recent.cursor_for(command);
                            if !recent_catalog.request(command, cursor) {
                                model.warning = Some(
                                    "recent catalog worker stopped; retry by reopening the TUI"
                                        .to_string(),
                                );
                            } else {
                                model.recent_loading_started();
                            }
                        }
                        SelectorControl::AdoptRecent(request) => {
                            match adopt_recent_thread(&request) {
                                Ok(result) => model.recent_adoption_succeeded(&request, result),
                                Err(error) => model.recent_adoption_failed(format!("{error:#}")),
                            }
                        }
                        SelectorControl::OpenProfileManager => {
                            match load_profile_catalog_read_only() {
                                Ok(profiles) => model.open_profile_manager(profiles),
                                Err(error) => model.profile_manager_failed(format!("{error:#}")),
                            }
                        }
                        SelectorControl::OpenProjects => {
                            return Ok(SessionTuiCycleOutcome::Projects);
                        }
                        SelectorControl::OpenCutexProjects => {
                            return Ok(SessionTuiCycleOutcome::CutexProjects);
                        }
                        SelectorControl::ApplySettings(request) => {
                            match apply_session_settings(&request) {
                                Ok(result) => model.settings_apply_succeeded(
                                    &request.key,
                                    &result.record,
                                    &result.profile_names,
                                    request.changed_count,
                                    request.draft.launch_actions_are_dirty(),
                                    result.warning,
                                ),
                                Err(error) => model.settings_apply_failed(format!("{error:#}")),
                            }
                        }
                        SelectorControl::ApplyGlobalSettings(request) => {
                            match apply_global_settings(&request) {
                                Ok(result) => model.global_settings_apply_succeeded(
                                    &result.config,
                                    &result.profile_names,
                                    request.changed_count,
                                ),
                                Err(error) => {
                                    model.global_settings_apply_failed(format!("{error:#}"))
                                }
                            }
                        }
                        SelectorControl::ApplyProfileSettings(request) => {
                            match perform_profile_settings_update(&request) {
                                Ok(receipt) => match load_profile_management_result(receipt) {
                                    Ok(result) => model.profile_management_succeeded(result),
                                    Err((notice, error)) => model
                                        .profile_management_refresh_failed(
                                            notice,
                                            format!("{error:#}"),
                                        ),
                                },
                                Err(error) => {
                                    model.profile_settings_apply_failed(format!("{error:#}"))
                                }
                            }
                        }
                        SelectorControl::ManageSession(request) => {
                            match apply_session_management(&request) {
                                Ok(result) => model.session_management_succeeded(
                                    &request.key,
                                    request.command,
                                    &result.record,
                                    &result.profile_names,
                                    result.warning,
                                ),
                                Err(error) => model.session_management_failed(format!("{error:#}")),
                            }
                        }
                        SelectorControl::ManageProfile(request) => {
                            match perform_profile_management(&request) {
                                Ok(receipt) => match load_profile_management_result(receipt) {
                                    Ok(result) => model.profile_management_succeeded(result),
                                    Err((notice, error)) => model
                                        .profile_management_refresh_failed(
                                            notice,
                                            format!("{error:#}"),
                                        ),
                                },
                                Err(error) => {
                                    model.profile_management_failed(&request, format!("{error:#}"))
                                }
                            }
                        }
                        SelectorControl::LoginProfile => {
                            return Ok(SessionTuiCycleOutcome::LoginProfile);
                        }
                    }
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

fn apply_session_settings(
    request: &SessionSettingsApplyRequest,
) -> anyhow::Result<SessionSettingsApplyResult> {
    let profile_names = if matches!(
        request.draft.profile_update(),
        CutexSessionValueUpdate::Unchanged
    ) {
        request.profile_names.clone()
    } else {
        load_profile_names_read_only()?
    };
    let mut store = load_cutex_session_store()?;
    let record = apply_session_settings_to_store(&mut store, request, &profile_names)?;
    persist_cutex_session_store_and_im_record(&store, &request.key)?;
    let warning = if request.draft.agent_groups_are_dirty() {
        let target = record
            .codex_session_id
            .as_deref()
            .unwrap_or(request.key.as_str());
        live_group_propagation_warning(agent_bus_runtime::maybe_patch_live_agent_groups(
            target,
            &record.agent_groups,
            AgentGroupUpdateMode::Set,
        ))
    } else {
        None
    };
    Ok(SessionSettingsApplyResult {
        record,
        profile_names,
        warning,
    })
}

fn apply_session_management(
    request: &SessionManagementRequest,
) -> anyhow::Result<SessionManagementResult> {
    let mut store = load_cutex_session_store()?;
    apply_session_management_to_store(&mut store, request)?;
    persist_cutex_session_store_and_im_record(&store, &request.key)?;
    let record = store.sessions.get(&request.key).cloned().with_context(|| {
        format!(
            "cutex session disappeared after management change: {}",
            request.key
        )
    })?;
    let warning = if request.command == SessionSettingsCommand::Adopt {
        let target = record
            .codex_session_id
            .as_deref()
            .unwrap_or(request.key.as_str());
        live_management_group_propagation_warning(agent_bus_runtime::maybe_patch_live_agent_groups(
            target,
            &record.agent_groups,
            AgentGroupUpdateMode::Set,
        ))
    } else {
        None
    };
    Ok(SessionManagementResult {
        record,
        profile_names: request.profile_names.clone(),
        warning,
    })
}

/// Adopting a catalog thread goes only through the durable session service.
/// The fresh store read makes a second adoption (including a retired identity)
/// fail safely if another Cutex client changed state during the review.
fn adopt_recent_thread(request: &RecentAdoptionRequest) -> anyhow::Result<RecentAdoptionResult> {
    let mut store = load_cutex_session_store()?;
    if store.sessions.values().any(|record| {
        record.codex_session_id.as_deref() == Some(request.thread_id.as_str())
            && (record.is_retired() || cutex_session_is_managed(record))
    }) {
        anyhow::bail!("native thread is already managed or retired");
    }
    let outcome = adopt_cutex_session(
        &mut store,
        &request.thread_id,
        CutexSessionEnsureSeed {
            host_id: current_host_name(),
            cwd: request.cwd.clone(),
            profile: None,
        },
        CutexSessionAdoptOptions {
            display_name: Some(&request.title),
            managed_cwd: None,
            groups: Vec::new(),
            expose_to_im: false,
            pin: false,
        },
    )?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    Ok(RecentAdoptionResult {
        store,
        snapshot: load_live_snapshot().map_err(|error| format!("{error:#}")),
    })
}

fn perform_profile_management(
    request: &ProfileManagementRequest,
) -> anyhow::Result<ProfileMutationReceipt> {
    match &request.command {
        ProfileManagementCommand::Activate => {
            let account = activate_account(&request.profile_id)?;
            Ok(ProfileMutationReceipt {
                preferred_profile_id: Some(account.id),
                notice: format!("Active profile: {}", account.name),
            })
        }
        ProfileManagementCommand::Rename { new_name } => {
            let result = rename_profile(&request.profile_id, new_name)?;
            Ok(ProfileMutationReceipt {
                preferred_profile_id: Some(result.account.id),
                notice: format!("Renamed {} to {}", result.old_name, result.account.name),
            })
        }
        ProfileManagementCommand::Remove => {
            let result = remove_profile(&request.profile_id)?;
            Ok(ProfileMutationReceipt {
                preferred_profile_id: None,
                notice: format!("Removed profile {}", result.removed.name),
            })
        }
    }
}

fn perform_profile_settings_update(
    request: &ProfileSettingsApplyRequest,
) -> anyhow::Result<ProfileMutationReceipt> {
    let result = update_profile_settings(&request.profile_id, &request.patch)?;
    Ok(ProfileMutationReceipt {
        preferred_profile_id: Some(result.account.id),
        notice: if result.changed {
            format!("Saved {} profile setting(s)", request.changed_count)
        } else {
            "Profile already matched the staged settings".to_string()
        },
    })
}

fn load_profile_management_result(
    receipt: ProfileMutationReceipt,
) -> Result<ProfileManagementResult, (String, anyhow::Error)> {
    let notice = receipt.notice;
    let result = (|| -> anyhow::Result<ProfileManagementResult> {
        let profiles = load_profile_catalog_read_only()?;
        let profile_names = profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect::<Vec<_>>();
        let store = load_cutex_session_store()?;
        let config = load_codez_config_checked()?;
        Ok(ProfileManagementResult {
            profiles,
            projection: ProfileProjectionSnapshot {
                records: store.sessions,
                config,
                profile_names,
            },
            preferred_profile_id: receipt.preferred_profile_id,
            notice: notice.clone(),
        })
    })();
    result.map_err(|error| (notice, error))
}

fn apply_global_settings(
    request: &GlobalSettingsApplyRequest,
) -> anyhow::Result<GlobalSettingsApplyResult> {
    let profile_names = if request.draft.default_profile_is_dirty() {
        load_profile_names_read_only()?
    } else {
        request.profile_names.clone()
    };
    request.draft.validate_profile_catalog(&profile_names)?;
    let mut config = load_codez_config_checked()?;
    let changed = apply_global_settings_to_config(&mut config, request)?;
    if changed {
        save_codez_config(&config)?;
    }
    Ok(GlobalSettingsApplyResult {
        config,
        profile_names,
    })
}

fn apply_global_settings_to_config(
    config: &mut CodezConfig,
    request: &GlobalSettingsApplyRequest,
) -> anyhow::Result<bool> {
    let patch = request.draft.patch(config)?;
    apply_global_config_patch(config, &patch)
}

fn apply_session_management_to_store(
    store: &mut CutexSessionStore,
    request: &SessionManagementRequest,
) -> anyhow::Result<()> {
    let existing = store
        .sessions
        .get(&request.key)
        .cloned()
        .with_context(|| format!("cutex session is not known: {}", request.key))?;
    match request.command {
        SessionSettingsCommand::Adopt => {
            if cutex_session_is_managed(&existing) {
                anyhow::bail!("Agent is already managed");
            }
            adopt_cutex_session(
                store,
                &request.key,
                CutexSessionEnsureSeed {
                    host_id: existing.host_id,
                    cwd: existing.cwd,
                    profile: existing.profile,
                },
                CutexSessionAdoptOptions {
                    display_name: None,
                    managed_cwd: None,
                    groups: Vec::new(),
                    expose_to_im: false,
                    pin: false,
                },
            )?;
        }
        SessionSettingsCommand::Unmanage => {
            if !cutex_session_is_managed(&existing) {
                anyhow::bail!("Agent is already unmanaged");
            }
            unmanage_cutex_session(store, &request.key)?;
        }
    }
    Ok(())
}

fn apply_session_settings_to_store(
    store: &mut CutexSessionStore,
    request: &SessionSettingsApplyRequest,
    profile_names: &[String],
) -> anyhow::Result<CutexSessionRecord> {
    let profile_update = request.draft.profile_update();
    match profile_update {
        CutexSessionValueUpdate::Unchanged => {}
        CutexSessionValueUpdate::Set(profile) => {
            if !profile_names.iter().any(|candidate| candidate == profile) {
                anyhow::bail!("Profile is no longer configured: {profile}");
            }
        }
        CutexSessionValueUpdate::Clear => {}
    }
    if request.draft.routing_is_dirty() {
        update_cutex_session_routing_by_key(
            store,
            &request.key,
            &request.key,
            request.draft.routing_patch(),
        )?;
    }
    if let Some(agent_name) = request.draft.agent_name() {
        set_cutex_session_display_name_by_key(store, &request.key, agent_name)?;
    }
    match profile_update {
        CutexSessionValueUpdate::Unchanged => {}
        CutexSessionValueUpdate::Set(profile) => {
            set_cutex_session_profile_by_key(store, &request.key, Some(profile.clone()))?;
        }
        CutexSessionValueUpdate::Clear => {
            set_cutex_session_profile_by_key(store, &request.key, None)?;
        }
    }
    if request.draft.runtime_defaults_are_dirty() {
        update_cutex_session_runtime_defaults_by_key(
            store,
            &request.key,
            &request.key,
            request.draft.runtime_defaults_patch(),
        )?;
    }
    store.sessions.get(&request.key).cloned().with_context(|| {
        format!(
            "cutex session disappeared after settings update: {}",
            request.key
        )
    })
}

fn live_group_propagation_warning(result: anyhow::Result<Option<String>>) -> Option<String> {
    result
        .err()
        .map(|error| format!("Saved durable groups; live update failed: {error:#}"))
}

fn live_management_group_propagation_warning(
    result: anyhow::Result<Option<String>>,
) -> Option<String> {
    result
        .err()
        .map(|error| format!("Adopted agent; live group update failed: {error:#}"))
}

fn receive_refresh(model: &mut SelectorModel, refresh: &mut WorkspaceLoad<SelectorSnapshot>) {
    match refresh.poll() {
        WorkspaceLoadPoll::Pending => {}
        WorkspaceLoadPoll::Ready(snapshot) => model.replace_snapshot(snapshot),
        WorkspaceLoadPoll::Failed(message) => model.mark_refresh_failed(message),
    }
}

fn receive_runtime_close(
    model: &mut SelectorModel,
    runtime_close: &mut Option<Receiver<RuntimeCloseWorkerResult>>,
) -> bool {
    let result = match runtime_close.as_ref().map(Receiver::try_recv) {
        Some(Ok(result)) => result,
        Some(Err(TryRecvError::Empty)) | None => return false,
        Some(Err(TryRecvError::Disconnected)) => RuntimeCloseWorkerResult::Failed(
            "runtime close worker stopped before reporting a result".to_string(),
        ),
    };
    *runtime_close = None;
    match result {
        RuntimeCloseWorkerResult::Closed(snapshot) => model.runtime_close_succeeded(snapshot),
        RuntimeCloseWorkerResult::ClosedRefreshFailed(message) => {
            model.runtime_close_refresh_failed(message)
        }
        RuntimeCloseWorkerResult::Failed(message) => model.runtime_close_failed(message),
    }
    true
}

fn intent_runs_in_selector(intent: &SessionTuiIntent) -> bool {
    matches!(
        intent.action,
        SessionTuiAction::CloseRuntime
            | SessionTuiAction::RetireSession
            | SessionTuiAction::RestoreSession
    )
}

fn close_runtime_shortcut_from_key(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('x' | 'X'))
}

fn selector_event_from_key(key: KeyEvent, enhanced_keyboard: bool) -> Option<SelectorEvent> {
    workspace_event_from_key(key, enhanced_keyboard)
}

fn render_selector(frame: &mut Frame<'_>, model: &SelectorModel) {
    render_workspace(frame, model, &SelectorWorkspaceRenderer);
}

struct SelectorWorkspaceRenderer;

impl WorkspaceRenderer<SelectorModel> for SelectorWorkspaceRenderer {
    fn render(&self, frame: &mut Frame<'_>, model: &SelectorModel) {
        render_selector_contents(frame, model);
    }
}

fn render_selector_contents(frame: &mut Frame<'_>, model: &SelectorModel) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(area);
    render_header(frame, chunks[0], model);
    match &model.mode {
        SelectorMode::Agents => {
            render_filter(frame, chunks[1], model);
            render_table(frame, chunks[2], model);
        }
        SelectorMode::RecentSessions => {
            render_recent_context(frame, chunks[1], model);
            render_recent_workspace(frame, chunks[2], model);
        }
        SelectorMode::RetiredSessions { .. } => {
            render_retired_context(frame, chunks[1], model);
            render_retired_table(frame, chunks[2], model);
        }
        SelectorMode::Actions { .. } => {
            render_item_context(frame, chunks[1], model);
            render_action_table(frame, chunks[2], model);
            render_action_overlay(frame, chunks[2], model);
        }
        SelectorMode::Settings { .. } => {
            render_item_context(frame, chunks[1], model);
            render_settings_browser(frame, chunks[2], model);
            render_settings_overlay(frame, chunks[2], model);
        }
        SelectorMode::ProfileManager { .. } => {
            render_profile_context(frame, chunks[1], model);
            render_profile_manager(frame, chunks[2], model);
            render_settings_overlay(frame, chunks[2], model);
            render_profile_overlay(frame, chunks[2], model);
        }
        SelectorMode::ConfirmRuntimeAction { .. } => {
            render_item_context(frame, chunks[1], model);
            render_runtime_action_confirmation(frame, chunks[2], model);
        }
        SelectorMode::ClosingRuntime { .. } => {
            render_item_context(frame, chunks[1], model);
            render_runtime_close_progress(frame, chunks[2], model);
        }
    }
    render_footer(frame, chunks[3], model);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let (view, count) = match &model.mode {
        SelectorMode::Agents => ("agents", model.visible_indices().len()),
        SelectorMode::RecentSessions => ("recent sessions", model.recent.rows().len()),
        SelectorMode::RetiredSessions { .. } => ("retired sessions", model.retired_rows.len()),
        SelectorMode::Actions { .. } => (
            "actions",
            model
                .active_row()
                .map_or(0, SelectorRow::action_control_count),
        ),
        SelectorMode::Settings { .. } => (
            "settings",
            model.active_row().map_or(0, |row| {
                row.settings
                    .iter()
                    .map(|category| category.options.len())
                    .sum()
            }),
        ),
        SelectorMode::ProfileManager { profiles, .. } => {
            ("profiles", profiles.len().saturating_add(2))
        }
        SelectorMode::ConfirmRuntimeAction { .. } => ("confirm", 0),
        SelectorMode::ClosingRuntime { .. } => ("closing", 0),
    };
    let refresh = model
        .refreshing
        .then_some(Span::styled("  refreshing", Style::new().fg(Color::Yellow)));
    let mut spans = vec![
        Span::styled(
            "cutex",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {view}"),
            Style::new().add_modifier(Modifier::BOLD),
        ),
    ];
    if count > 0 || matches!(&model.mode, SelectorMode::Agents) {
        spans.push(Span::styled(
            format!("  {count} shown"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    if matches!(&model.mode, SelectorMode::Agents) {
        let hidden = model.hidden_searchable_agent_count();
        if hidden > 0 {
            spans.push(Span::styled(
                format!("  {hidden} offline searchable"),
                Style::new().fg(Color::DarkGray),
            ));
        }
    }
    if let Some(settings_view) = model.settings_view() {
        spans.push(Span::styled("  view ", Style::new().fg(Color::DarkGray)));
        if area.width < SETTINGS_TWO_PANE_MIN_WIDTH {
            spans.push(Span::styled(
                format!("[{}]", settings_view.label()),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ));
        } else {
            for view in [SettingsView::Expanded, SettingsView::Categories] {
                let label = if view == settings_view {
                    format!("[{}]", view.label())
                } else {
                    view.label().to_string()
                };
                let style = if view == settings_view {
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                spans.push(Span::styled(label, style));
                if view == SettingsView::Expanded {
                    spans.push(Span::raw(" "));
                }
            }
        }
        let dirty_count = model.settings_dirty_count();
        if dirty_count > 0 {
            spans.push(Span::styled(
                format!("  {dirty_count} pending"),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
    }
    if matches!(&model.mode, SelectorMode::ProfileManager { .. }) {
        let dirty_count = model.global_settings_draft.dirty_count();
        if dirty_count > 0 {
            spans.push(Span::styled(
                format!("  {dirty_count} pending"),
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ));
        }
    }
    spans.extend(refresh);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_item_context(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(row) = model.active_row() else {
        frame.render_widget(
            Paragraph::new("Selected item is no longer available")
                .block(Block::bordered().title(" Selection "))
                .style(Style::new().fg(Color::Yellow)),
            area,
        );
        return;
    };
    let mut spans = vec![Span::styled(
        row.agent.as_str(),
        Style::new().add_modifier(Modifier::BOLD),
    )];
    if let Some(lifecycle) = row.lifecycle {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(lifecycle.label(), lifecycle_style(lifecycle)));
        spans.push(Span::styled(
            format!("  {}  {}", row.host, row.backend),
            Style::new().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled(
            format!("  {}", row.backend),
            Style::new().fg(Color::DarkGray),
        ));
    }
    let title = match &row.target {
        SelectorTarget::RecentSessions => " Recent sessions ",
        SelectorTarget::RetiredSessions => " Retired sessions ",
        SelectorTarget::CutexProjects => " Cutex Projects ",
        SelectorTarget::Projects => " Codex Workspaces ",
        SelectorTarget::Profiles => " Profiles ",
        SelectorTarget::GlobalSettings => " Global settings ",
        SelectorTarget::Agent(_) | SelectorTarget::RetiredAgent(_) => " Agent ",
    };
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::bordered()
                .title(title)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_retired_context(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let selected = model
        .retired_rows
        .get(match model.mode {
            SelectorMode::RetiredSessions { selected } => selected,
            _ => 0,
        })
        .map(|row| row.agent.as_str())
        .unwrap_or("No retired sessions");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Retired sessions",
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {selected}"), Style::new().fg(Color::DarkGray)),
        ]))
        .block(Block::bordered().title(" Archive ")),
        area,
    );
}

fn render_recent_context(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    if model.recent.review().is_some() {
        let status = if model.recent.review_confirmed() {
            "[Adopt]  Cancel"
        } else {
            "Adopt  [Cancel]"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "Adopt native thread",
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {status}"), Style::new().fg(Color::Yellow)),
            ]))
            .block(Block::bordered().title(" Adoption review ")),
            area,
        );
        return;
    }
    let text = match model.recent.load_state() {
        RecentLoadState::Loading => "Loading native app-server threads…".to_string(),
        RecentLoadState::Ready => format!(
            "Native threads, newest first  {}",
            if model.recent.next_cursor().is_some() {
                "more available"
            } else {
                "end of catalog"
            }
        ),
        RecentLoadState::Empty => {
            "No native threads were returned by the paired app-server".to_string()
        }
        RecentLoadState::ProviderIncompatible(message) => {
            format!("Provider incompatible: {message}")
        }
        RecentLoadState::Failed(message) => format!("Catalog unavailable: {message}"),
    };
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title(" Recent sessions ")),
        area,
    );
}

fn render_recent_workspace(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    if let Some(row) = model.recent.review() {
        let lines = vec![
            Line::from(vec![
                Span::styled("Name / preview: ", Style::new().fg(Color::DarkGray)),
                Span::raw(row.title.clone()),
            ]),
            Line::from(vec![
                Span::styled("Native thread id: ", Style::new().fg(Color::DarkGray)),
                Span::raw(row.thread_id.clone()),
            ]),
            Line::from(vec![
                Span::styled("Cwd: ", Style::new().fg(Color::DarkGray)),
                Span::raw(
                    row.cwd
                        .as_deref()
                        .map(truncate_recent_display)
                        .unwrap_or_else(|| "unavailable".to_string()),
                ),
            ]),
            Line::from(vec![
                Span::styled("Provider / source: ", Style::new().fg(Color::DarkGray)),
                Span::raw(format!("{} / {}", row.provider, row.source)),
            ]),
            Line::from(vec![
                Span::styled("Project assignment: ", Style::new().fg(Color::DarkGray)),
                Span::raw(
                    row.project_id
                        .clone()
                        .unwrap_or_else(|| "unassigned".to_string()),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Cutex defaults: persistent management; native title as display name; default runtime; no groups; IM hidden; unpinned.",
                Style::new().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(if model.recent.review_confirmed() {
                Span::styled(
                    "Confirm adoption?  [Adopt]  Cancel",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(
                    "Confirm adoption?  Adopt  [Cancel]",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )
            }),
        ];
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title(" Review native thread ")),
            area,
        );
        return;
    }
    match model.recent.load_state() {
        RecentLoadState::Loading
        | RecentLoadState::Empty
        | RecentLoadState::ProviderIncompatible(_)
        | RecentLoadState::Failed(_)
            if model.recent.rows().is_empty() =>
        {
            frame.render_widget(
                Paragraph::new("The catalog loads asynchronously. Press Enter to retry a failure.")
                    .block(Block::bordered()),
                area,
            );
        }
        _ => {
            let rows = model.recent.rows().iter().map(|row| {
                Row::new([
                    Cell::from(row.title.clone()),
                    Cell::from(
                        row.cwd
                            .as_deref()
                            .map(truncate_recent_display)
                            .unwrap_or_else(|| "-".to_string()),
                    ),
                    Cell::from(format!("{} / {}", row.provider, row.source)),
                    Cell::from(row.project_id.clone().unwrap_or_else(|| "-".to_string())),
                    Cell::from(row.state.label()),
                ])
            });
            let table = Table::new(
                rows,
                [
                    Constraint::Min(22),
                    Constraint::Min(22),
                    Constraint::Length(18),
                    Constraint::Length(16),
                    Constraint::Length(16),
                ],
            )
            .header(
                Row::new([
                    "NAME / PREVIEW",
                    "CWD",
                    "PROVIDER / SOURCE",
                    "PROJECT",
                    "CUTEX STATE",
                ])
                .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
                .bottom_margin(1),
            )
            .column_spacing(1)
            .row_highlight_style(
                Style::new()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
            let mut state = TableState::default().with_selected(
                (!model.recent.rows().is_empty()).then_some(model.recent.selected()),
            );
            frame.render_stateful_widget(table, area, &mut state);
        }
    }
}

fn truncate_recent_display(value: &str) -> String {
    const MAX_RECENT_DISPLAY_CHARS: usize = 160;
    let mut output = value
        .chars()
        .take(MAX_RECENT_DISPLAY_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_RECENT_DISPLAY_CHARS {
        output.push('…');
    }
    output
}

fn render_retired_table(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let rows = model.retired_rows.iter().map(|row| {
        Row::new([
            Cell::from(row.agent.as_str()),
            Cell::from(row.configured_profile.as_deref().unwrap_or("-")),
            Cell::from(row.managed_path.as_str()),
            Cell::from(row.retired_at.as_deref().unwrap_or("-")),
            Cell::from(row.revision.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(16),
            Constraint::Min(20),
            Constraint::Length(22),
            Constraint::Length(14),
        ],
    )
    .header(
        Row::new(["AGENT", "PROFILE", "MANAGED PATH", "RETIRED AT", "REVISION"])
            .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
            .bottom_margin(1),
    )
    .column_spacing(2)
    .row_highlight_style(
        Style::new()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");
    let selected = match model.mode {
        SelectorMode::RetiredSessions { selected } => Some(selected),
        _ => None,
    };
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_profile_context(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let selected = model.selected_profile();
    let body = if let Some(profile) = selected {
        let default_profile = model.current_default_profile_name();
        let mut spans = vec![Span::styled(
            profile.name.as_str(),
            Style::new().add_modifier(Modifier::BOLD),
        )];
        if profile.active {
            spans.push(Span::styled("  active home", Style::new().fg(Color::Green)));
        }
        if default_profile.as_deref() == Some(profile.name.as_str()) {
            spans.push(Span::styled(
                "  launch default",
                Style::new().fg(Color::Cyan),
            ));
        }
        Line::from(spans)
    } else if model.selected_profile_is_default() {
        Line::from(vec![
            Span::styled("Default", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled("  launch policy", Style::new().fg(Color::DarkGray)),
        ])
    } else if model.selected_profile_is_add() {
        Line::from(vec![
            Span::styled("Add profile", Style::new().add_modifier(Modifier::BOLD)),
            Span::styled("  login", Style::new().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(Span::styled("Profiles", Style::new().fg(Color::DarkGray)))
    };
    frame.render_widget(
        Paragraph::new(body).block(
            Block::bordered()
                .title(" Profiles ")
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn render_profile_manager(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    if area.width >= SETTINGS_TWO_PANE_MIN_WIDTH {
        let list_width = (area.width * 42 / 100).clamp(30, 42);
        let panes =
            Layout::horizontal([Constraint::Length(list_width), Constraint::Min(24)]).split(area);
        render_profile_list(frame, panes[0], model);
        render_profile_details(frame, panes[1], model);
    } else {
        match model.profile_workspace_focus() {
            Some(ProfileWorkspaceFocus::Items) | None => render_profile_list(frame, area, model),
            Some(ProfileWorkspaceFocus::Editor) => render_profile_details(frame, area, model),
        }
    }
}

fn render_profile_list(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let SelectorMode::ProfileManager { profiles, .. } = &model.mode else {
        return;
    };
    let default_profile = model.current_default_profile_name();
    let default_style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut rows = if area.width >= 40 {
        vec![Row::new([
            Cell::from("Default").style(default_style),
            Cell::from("-"),
            Cell::from(default_profile.as_deref().unwrap_or("none"))
                .style(Style::new().fg(Color::Cyan)),
        ])]
    } else {
        vec![Row::new([
            Cell::from("Default").style(default_style),
            Cell::from(default_profile.as_deref().unwrap_or("none"))
                .style(Style::new().fg(Color::Cyan)),
        ])]
    };
    rows.extend(
        profiles
            .iter()
            .map(|profile| {
                let state = profile_list_state_label(profile, default_profile.as_deref());
                if area.width >= 40 {
                    Row::new([
                        Cell::from(profile.name.as_str()),
                        Cell::from(profile.cli_kind.as_str()).style(Style::new().fg(Color::Gray)),
                        Cell::from(state)
                            .style(profile_state_style(profile, default_profile.as_deref())),
                    ])
                } else {
                    Row::new([
                        Cell::from(profile.name.as_str()),
                        Cell::from(state)
                            .style(profile_state_style(profile, default_profile.as_deref())),
                    ])
                }
            })
            .collect::<Vec<_>>(),
    );
    let add_style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    if area.width >= 40 {
        rows.push(Row::new([
            Cell::from("Add profile").style(add_style),
            Cell::from("-"),
            Cell::from("new").style(Style::new().fg(Color::DarkGray)),
        ]));
    } else {
        rows.push(Row::new([
            Cell::from("Add profile").style(add_style),
            Cell::from("new").style(Style::new().fg(Color::DarkGray)),
        ]));
    }
    let (header, widths) = if area.width >= 40 {
        (
            Row::new(["PROFILE", "CLI", "STATUS"]),
            vec![
                Constraint::Min(12),
                Constraint::Length(7),
                Constraint::Length(14),
            ],
        )
    } else {
        (
            Row::new(["PROFILE", "STATUS"]),
            vec![Constraint::Min(12), Constraint::Length(14)],
        )
    };
    let table = Table::new(rows, widths)
        .header(
            header
                .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
                .bottom_margin(1),
        )
        .block(settings_panel_block(
            " Profiles ".to_string(),
            model.profile_workspace_focus() == Some(ProfileWorkspaceFocus::Items),
        ))
        .column_spacing(1)
        .row_highlight_style(settings_highlight_style(true))
        .highlight_symbol("> ");
    let mut state = TableState::default().with_selected(model.selected_profile_index());
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_profile_details(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(profile) = model.selected_profile() else {
        if model.selected_profile_is_default() {
            render_profile_default_editor(frame, area, model);
            return;
        }
        if model.selected_profile_is_add() {
            frame.render_widget(
                Paragraph::new(vec![
                    profile_detail_line("Name", "Add profile"),
                    profile_detail_line("Status", "ready"),
                    profile_detail_line("Flow", "login wizard"),
                ])
                .block(settings_panel_block(
                    " Add profile ".to_string(),
                    model.profile_workspace_focus() == Some(ProfileWorkspaceFocus::Editor),
                )),
                area,
            );
            return;
        }
        frame.render_widget(
            Paragraph::new("-")
                .block(settings_panel_block(" Details ".to_string(), false))
                .style(Style::new().fg(Color::DarkGray)),
            area,
        );
        return;
    };
    let categories = model.selected_profile_setting_categories();
    let rows = categories
        .iter()
        .flat_map(|category| {
            std::iter::once(
                Row::new([Cell::from(category.label), Cell::from("")])
                    .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            )
            .chain(category.options.iter().map(|option| {
                let label = if option.dirty {
                    format!("  {} *", option.label)
                } else {
                    format!("  {}", option.label)
                };
                let value_style = if option.profile_field.is_some() {
                    Style::new().fg(Color::Gray)
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                Row::new([
                    Cell::from(label),
                    Cell::from(option.value.as_str()).style(value_style),
                ])
            }))
        })
        .collect::<Vec<_>>();
    let header = Row::new(["SETTING", "VALUE"])
        .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let widths = if area.width < SETTINGS_TWO_PANE_MIN_WIDTH {
        [Constraint::Percentage(48), Constraint::Percentage(52)]
    } else {
        [Constraint::Percentage(40), Constraint::Percentage(60)]
    };
    let active = model.profile_workspace_focus() == Some(ProfileWorkspaceFocus::Editor);
    let title = if model.profile_settings_draft.is_dirty() {
        format!(
            " {} [{} staged] ",
            profile.name,
            model.profile_settings_draft.dirty_count()
        )
    } else {
        format!(" {} ", profile.name)
    };
    let table = Table::new(rows, widths)
        .header(header)
        .block(settings_panel_block(title, active))
        .column_spacing(2)
        .row_highlight_style(settings_highlight_style(active))
        .highlight_symbol(if active { "> " } else { "  " });
    let selected = match &model.mode {
        SelectorMode::ProfileManager {
            editor_selected, ..
        } => setting_indices_at_flat_index(&categories, *editor_selected).and_then(
            |(category, option)| expanded_setting_table_row_index(&categories, category, option),
        ),
        _ => None,
    };
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_profile_default_editor(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let default_profile = model
        .profile_default_value(GlobalSettingsField::DefaultProfile)
        .unwrap_or_else(|| "-".to_string());
    let direct_launch = model
        .profile_default_value(GlobalSettingsField::DefaultProfileDirectLaunch)
        .unwrap_or_else(|| "disabled".to_string());
    let fields = [
        (
            GlobalSettingsField::DefaultProfile,
            "Default profile",
            default_profile,
        ),
        (
            GlobalSettingsField::DefaultProfileDirectLaunch,
            "Direct default launch",
            direct_launch,
        ),
    ];
    let mut rows = vec![Row::new([Cell::from("Launch defaults"), Cell::from("")])
        .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))];
    rows.extend(fields.into_iter().map(|(field, label, value)| {
        let label = if model.global_settings_draft.field_is_dirty(field) {
            format!("  {label} *")
        } else {
            format!("  {label}")
        };
        Row::new([
            Cell::from(label),
            Cell::from(value).style(Style::new().fg(Color::Gray)),
        ])
    }));
    let active = model.profile_workspace_focus() == Some(ProfileWorkspaceFocus::Editor);
    let widths = if area.width < 64 {
        [Constraint::Percentage(60), Constraint::Percentage(40)]
    } else {
        [Constraint::Percentage(48), Constraint::Percentage(52)]
    };
    let table = Table::new(rows, widths)
        .block(settings_panel_block(" Default ".to_string(), active))
        .column_spacing(2)
        .row_highlight_style(settings_highlight_style(active))
        .highlight_symbol(if active { "> " } else { "  " });
    let selected = model
        .selected_profile_default_option()
        .map(|index| index + 1);
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn profile_detail_line(label: &'static str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<11}"), Style::new().fg(Color::DarkGray)),
        Span::raw(value.into()),
    ])
}

fn profile_list_state_label(
    profile: &ProfileCatalogEntry,
    default_profile: Option<&str>,
) -> &'static str {
    match (
        profile.active,
        default_profile == Some(profile.name.as_str()),
    ) {
        (true, true) => "home+default",
        (true, false) => "active home",
        (false, true) => "launch default",
        (false, false) => "-",
    }
}

fn profile_state_style(profile: &ProfileCatalogEntry, default_profile: Option<&str>) -> Style {
    if profile.active {
        Style::new().fg(Color::Green)
    } else if default_profile == Some(profile.name.as_str()) {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

fn render_profile_overlay(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(overlay) = model.profile_overlay.as_ref() else {
        return;
    };
    match overlay {
        ProfileOverlay::Actions {
            profile_name,
            actions,
            selected,
            ..
        } => {
            let modal = centered_rect(48, actions.len() as u16 + 2, area);
            frame.render_widget(Clear, modal);
            let items = actions
                .iter()
                .map(|action| {
                    let style = if *action == ProfileManagerAction::Remove {
                        Style::new().fg(Color::Yellow)
                    } else {
                        Style::new()
                    };
                    ListItem::new(action.label()).style(style)
                })
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(settings_panel_block(
                    format!(" {} actions ", profile_name),
                    true,
                ))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);
        }
        ProfileOverlay::RenameInput {
            old_name, input, ..
        } => {
            let modal = centered_rect(64, 3, area);
            frame.render_widget(Clear, modal);
            let input_width = modal.width.saturating_sub(2) as usize;
            let scroll = input.visual_scroll(input_width.saturating_sub(1).max(1));
            frame.render_widget(
                Paragraph::new(input.value())
                    .scroll((0, scroll as u16))
                    .block(settings_panel_block(format!(" Rename {} ", old_name), true)),
                modal,
            );
            if input_width > 0 {
                let cursor = input
                    .visual_cursor()
                    .saturating_sub(scroll)
                    .min(input_width.saturating_sub(1));
                frame.set_cursor_position((modal.x + 1 + cursor as u16, modal.y + 1));
            }
        }
        ProfileOverlay::ConfirmRename {
            old_name,
            new_name,
            selected,
            ..
        } => {
            let modal = centered_rect(68, 9, area);
            frame.render_widget(Clear, modal);
            let block = settings_panel_block(" Confirm rename ".to_string(), true);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [description_area, choices_area] =
                Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).areas(inner);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(format!("Rename {old_name} to {new_name}?")),
                    Line::from("Updates Global default, QuickRun, and durable session references."),
                ])
                .wrap(Wrap { trim: false }),
                description_area,
            );
            let items = ["Cancel", "Rename"]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, choices_area, &mut state);
        }
        ProfileOverlay::ConfirmRemove {
            profile_name,
            selected,
            ..
        } => {
            let modal = centered_rect(68, 10, area);
            frame.render_widget(Clear, modal);
            let block = settings_panel_block(" Confirm remove ".to_string(), true);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [description_area, choices_area] =
                Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(inner);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(format!("Remove profile {profile_name}?")),
                    Line::from("Clears Global default, QuickRun, and durable session references."),
                    Line::from("Materialized profile files are retained."),
                ])
                .wrap(Wrap { trim: false }),
                description_area,
            );
            let items = ["Cancel", "Remove"]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, choices_area, &mut state);
        }
        ProfileOverlay::ConfirmAddProfile { selected } => {
            let modal = centered_rect(68, 10, area);
            frame.render_widget(Clear, modal);
            let block = settings_panel_block(" Add profile ".to_string(), true);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [description_area, choices_area] =
                Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(inner);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("Start the existing Cutex login wizard?"),
                    Line::from(
                        "The terminal is restored before login and this manager reopens afterward.",
                    ),
                ])
                .wrap(Wrap { trim: false }),
                description_area,
            );
            let items = ["Cancel", "Continue"]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, choices_area, &mut state);
        }
        ProfileOverlay::ConfirmDiscardProfile {
            destination,
            selected,
        } => {
            let modal = centered_rect(52, 4, area);
            frame.render_widget(Clear, modal);
            let leave_label = match destination {
                ProfileDiscardDestination::ProfileList => "Discard and view profiles",
                ProfileDiscardDestination::AgentList => "Discard and view agents",
            };
            let items = ["Keep editing", leave_label]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(settings_panel_block(
                    " Unsaved profile changes ".to_string(),
                    true,
                ))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);
        }
    }
}

fn render_filter(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let input_width = area.width.saturating_sub(2) as usize;
    let cursor_width = input_width.saturating_sub(1).max(1);
    let scroll = model.query.visual_scroll(cursor_width);
    let input = Paragraph::new(model.query.value())
        .scroll((0, scroll as u16))
        .block(
            Block::bordered()
                .title(" Filter agents ")
                .border_style(Style::new().fg(Color::DarkGray)),
        );
    frame.render_widget(input, area);

    if area.height >= 3 && input_width > 0 {
        let cursor = model
            .query
            .visual_cursor()
            .saturating_sub(scroll)
            .min(input_width.saturating_sub(1));
        frame.set_cursor_position((area.x + 1 + cursor as u16, area.y + 1));
    }
}

fn render_table(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let visible = model.visible_rows();
    let wide = frame.area().width >= WIDE_LAYOUT_MIN_WIDTH;
    let extra_wide = frame.area().width >= EXTRA_WIDE_LAYOUT_MIN_WIDTH;
    let default_profile = selector_default_profile_name(model);
    let rows = visible
        .iter()
        .map(|row| selector_table_row(row, wide, extra_wide, default_profile))
        .collect::<Vec<_>>();
    let header_style = Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD);
    let header = if extra_wide {
        Row::new([
            "AGENT",
            "PROFILE",
            "STATE",
            "LAST OUTPUT",
            "HOST",
            "BACKEND",
            "MANAGED PATH",
            "PRIMARY ACTION",
        ])
    } else if wide {
        Row::new([
            "AGENT",
            "PROFILE",
            "STATE",
            "LAST OUTPUT",
            "MANAGED PATH",
            "PRIMARY ACTION",
        ])
    } else {
        Row::new(["AGENT", "PROFILE", "STATE", "PRIMARY"])
    }
    .style(header_style)
    .bottom_margin(1);
    let widths = if extra_wide {
        vec![
            Constraint::Min(16),
            Constraint::Length(18),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Length(13),
            Constraint::Length(9),
            Constraint::Min(20),
            Constraint::Length(16),
        ]
    } else if wide {
        vec![
            Constraint::Min(14),
            Constraint::Length(18),
            Constraint::Length(9),
            Constraint::Length(11),
            Constraint::Min(18),
            Constraint::Length(16),
        ]
    } else {
        vec![
            Constraint::Min(14),
            Constraint::Length(18),
            Constraint::Length(9),
            Constraint::Length(14),
        ]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(2)
        .row_highlight_style(
            Style::new()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = TableState::default().with_selected(model.selected_visible_index());
    frame.render_stateful_widget(table, area, &mut state);

    if visible.is_empty() && area.height > 2 {
        let message = if model.query.value().is_empty() {
            "No live or pinned agents".to_string()
        } else {
            format!("No managed agents match {:?}", model.query.value())
        };
        let empty_area = Rect {
            y: area.y + 2,
            height: area.height.saturating_sub(2),
            ..area
        };
        frame.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .style(Style::new().fg(Color::DarkGray)),
            empty_area,
        );
    }
}

fn render_action_table(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(row) = model.active_row() else {
        return;
    };
    let global_default_profile = model.current_default_profile_name();
    let mut rows = Vec::with_capacity(row.action_control_count());
    if row.launch_profile_control_available() {
        rows.push(
            Row::new([
                Cell::from("Launch profile").style(Style::new().fg(Color::Cyan)),
                Cell::from(row.launch_profile_detail(
                    model.selected_launch_profile(),
                    global_default_profile.as_deref(),
                ))
                .style(Style::new().fg(Color::Gray)),
            ])
            .style(Style::new().add_modifier(Modifier::BOLD)),
        );
    }
    rows.extend(row.actions.iter().map(|item| {
        let label = if item.primary {
            format!("{}  primary", item.action.label())
        } else {
            item.action.label().to_string()
        };
        let style = if item.action.requires_confirmation() {
            Style::new().fg(Color::Yellow)
        } else {
            Style::new()
        };
        Row::new([
            Cell::from(label).style(style),
            Cell::from(item.detail).style(Style::new().fg(Color::Gray)),
        ])
    }));
    let header = Row::new(["ACTION", "DETAILS"])
        .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let table = Table::new(rows, [Constraint::Length(24), Constraint::Min(24)])
        .header(header)
        .column_spacing(2)
        .row_highlight_style(
            Style::new()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = TableState::default().with_selected(model.selected_action_index());
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_action_overlay(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(ActionOverlay::LaunchProfile { choices, selected }) = model.action_overlay.as_ref()
    else {
        return;
    };
    let modal = centered_rect(54, choices.len() as u16 + 2, area);
    frame.render_widget(Clear, modal);
    let items = choices
        .iter()
        .map(|choice| ListItem::new(choice.label.as_str()))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(settings_panel_block(" Launch profile ".to_string(), true))
        .highlight_style(settings_highlight_style(true))
        .highlight_symbol("> ");
    let mut state = ListState::default().with_selected(Some(*selected));
    frame.render_stateful_widget(list, modal, &mut state);
}

fn render_settings_browser(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    match model.settings_view() {
        Some(SettingsView::Expanded) => render_expanded_settings(frame, area, model),
        Some(SettingsView::Categories) => render_categorized_settings(frame, area, model),
        None => {}
    }
}

fn render_expanded_settings(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(row) = model.active_row() else {
        return;
    };
    let rows = row
        .settings
        .iter()
        .flat_map(|category| {
            std::iter::once(
                Row::new([Cell::from(category.label), Cell::from("")])
                    .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            )
            .chain(category.options.iter().map(|option| {
                let label = if option.dirty {
                    format!("  {} *", option.label)
                } else {
                    format!("  {}", option.label)
                };
                Row::new([
                    Cell::from(label),
                    Cell::from(option.value.as_str()).style(Style::new().fg(Color::Gray)),
                ])
            }))
        })
        .collect::<Vec<_>>();
    let header = Row::new(["SETTING", "VALUE"])
        .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let widths = if area.width >= SETTINGS_TWO_PANE_MIN_WIDTH {
        [Constraint::Percentage(38), Constraint::Percentage(62)]
    } else {
        [Constraint::Percentage(48), Constraint::Percentage(52)]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(2)
        .row_highlight_style(settings_highlight_style(true))
        .highlight_symbol("> ");
    let selected = expanded_setting_table_row_index(
        &row.settings,
        model.selected_setting_category_index().unwrap_or(0),
        model.selected_setting_option_index().unwrap_or(0),
    );
    let mut state = TableState::default().with_selected(selected);
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_categorized_settings(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(focus) = model.settings_focus() else {
        return;
    };
    if area.width >= WIDE_LAYOUT_MIN_WIDTH {
        let panes = Layout::horizontal([
            Constraint::Length(22),
            Constraint::Length(34),
            Constraint::Min(24),
        ])
        .split(area);
        render_setting_categories(frame, panes[0], model);
        render_setting_options(frame, panes[1], model, false);
        render_setting_value(frame, panes[2], model);
    } else if area.width >= SETTINGS_TWO_PANE_MIN_WIDTH {
        let panes = Layout::horizontal([Constraint::Length(22), Constraint::Min(30)]).split(area);
        render_setting_categories(frame, panes[0], model);
        if focus == SettingsFocus::Value {
            render_setting_value(frame, panes[1], model);
        } else {
            render_setting_options(frame, panes[1], model, true);
        }
    } else {
        match focus {
            SettingsFocus::Categories => render_setting_categories(frame, area, model),
            SettingsFocus::Options => render_setting_options(frame, area, model, true),
            SettingsFocus::Value => render_setting_value(frame, area, model),
        }
    }
}

fn render_setting_categories(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(row) = model.active_row() else {
        return;
    };
    let active = model.settings_focus() == Some(SettingsFocus::Categories);
    let items = row
        .settings
        .iter()
        .map(|category| ListItem::new(format!("{}  {}", category.label, category.options.len())))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(settings_panel_block(" Categories ".to_string(), active))
        .highlight_style(settings_highlight_style(active))
        .highlight_symbol(if active { "> " } else { "  " });
    let mut state = ListState::default().with_selected(model.selected_setting_category_index());
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_setting_options(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &SelectorModel,
    show_values: bool,
) {
    let Some(category) = model.active_setting_category() else {
        return;
    };
    let active = model.settings_focus() == Some(SettingsFocus::Options);
    let title = format!(" {} options ", category.label);
    let mut state = TableState::default().with_selected(model.selected_setting_option_index());
    if show_values {
        let rows = category
            .options
            .iter()
            .map(|option| {
                let label = if option.dirty {
                    format!("{} *", option.label)
                } else {
                    option.label.to_string()
                };
                Row::new([
                    Cell::from(label),
                    Cell::from(option.value.as_str()).style(Style::new().fg(Color::Gray)),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(
            rows,
            [Constraint::Percentage(48), Constraint::Percentage(52)],
        )
        .block(settings_panel_block(title, active))
        .column_spacing(1)
        .row_highlight_style(settings_highlight_style(active))
        .highlight_symbol(if active { "> " } else { "  " });
        frame.render_stateful_widget(table, area, &mut state);
    } else {
        let rows = category
            .options
            .iter()
            .map(|option| {
                let label = if option.dirty {
                    format!("{} *", option.label)
                } else {
                    option.label.to_string()
                };
                Row::new([Cell::from(label)])
            })
            .collect::<Vec<_>>();
        let table = Table::new(rows, [Constraint::Min(12)])
            .block(settings_panel_block(title, active))
            .row_highlight_style(settings_highlight_style(active))
            .highlight_symbol(if active { "> " } else { "  " });
        frame.render_stateful_widget(table, area, &mut state);
    }
}

fn render_setting_value(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(option) = model.active_setting_option() else {
        return;
    };
    let active = model.settings_focus() == Some(SettingsFocus::Value);
    let body = vec![
        Line::from(Span::styled(
            if option.dirty {
                format!("{} *", option.label)
            } else {
                option.label.to_string()
            },
            Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(option.value.as_str()),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .block(settings_panel_block(" Current value ".to_string(), active))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn group_editor_inputs(value: &str) -> Vec<Input> {
    let mut inputs = value
        .split(|character: char| character == ',' || character.is_whitespace())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Input::new(value.to_string()))
        .collect::<Vec<_>>();
    ensure_group_editor_trailing_input(&mut inputs);
    inputs
}

fn ensure_group_editor_trailing_input(inputs: &mut Vec<Input>) {
    if inputs
        .last()
        .is_none_or(|input| !input.value().trim().is_empty())
    {
        inputs.push(Input::default());
    }
}

fn group_editor_value(inputs: &[Input]) -> String {
    inputs
        .iter()
        .map(|input| input.value().trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_settings_overlay(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(overlay) = model.settings_overlay.as_ref() else {
        return;
    };
    match overlay {
        SettingsOverlay::Choice {
            choices,
            selected,
            custom_value,
            ..
        } => {
            let modal = centered_rect(
                46,
                choices.len() as u16 + u16::from(custom_value.is_some()) + 2,
                area,
            );
            frame.render_widget(Clear, modal);
            let mut items = Vec::with_capacity(choices.len() + usize::from(custom_value.is_some()));
            if let Some(value) = custom_value {
                items.push(ListItem::new(format!("Current: {value}")));
            }
            items.extend(
                choices
                    .iter()
                    .map(|choice| ListItem::new(choice.label.as_str())),
            );
            let title = model
                .active_setting_label()
                .map(|label| format!(" {label} "))
                .unwrap_or_else(|| " Edit setting ".to_string());
            let list = List::new(items)
                .block(settings_panel_block(title, true))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);
        }
        SettingsOverlay::Groups {
            inputs, selected, ..
        } => {
            let visible_rows = inputs.len().clamp(1, 10) as u16;
            let modal = centered_rect(64, visible_rows + 2, area);
            frame.render_widget(Clear, modal);
            let items = inputs
                .iter()
                .map(|input| {
                    let value = input.value();
                    if value.trim().is_empty() {
                        ListItem::new(Line::from(Span::styled(
                            "<new group>",
                            Style::new().fg(Color::DarkGray),
                        )))
                    } else {
                        ListItem::new(value.to_string())
                    }
                })
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(settings_panel_block(" Message groups ".to_string(), true))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ")
                .scroll_padding(1);
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);

            let offset = state.offset();
            let visible_index = selected.saturating_sub(offset);
            let list_height = modal.height.saturating_sub(2) as usize;
            if *selected >= offset && visible_index < list_height {
                let input_width = modal.width.saturating_sub(4) as usize;
                if input_width > 0 {
                    let input = &inputs[*selected];
                    let scroll = input.visual_scroll(input_width.saturating_sub(1).max(1));
                    let cursor = input
                        .visual_cursor()
                        .saturating_sub(scroll)
                        .min(input_width.saturating_sub(1));
                    frame.set_cursor_position((
                        modal.x + 3 + cursor as u16,
                        modal.y + 1 + visible_index as u16,
                    ));
                }
            }
        }
        SettingsOverlay::Text {
            input,
            tags,
            masked,
            ..
        } => {
            let modal = centered_rect(64, if *tags { 5 } else { 3 }, area);
            frame.render_widget(Clear, modal);
            let title = model
                .active_setting_label()
                .map(|label| {
                    if *masked {
                        format!(" Replace {label} ")
                    } else {
                        format!(" {label} ")
                    }
                })
                .unwrap_or_else(|| " Edit setting ".to_string());
            let input_width = modal.width.saturating_sub(2) as usize;
            let scroll = input.visual_scroll(input_width.saturating_sub(1).max(1));
            let body = if *tags {
                let tags = input
                    .value()
                    .split(|character: char| character == ',' || character.is_whitespace())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("[{value}]"))
                    .collect::<Vec<_>>()
                    .join(" ");
                vec![
                    Line::from(input.value()),
                    Line::from(""),
                    Line::from(Span::styled(tags, Style::new().fg(Color::Gray))),
                ]
            } else if *masked {
                vec![Line::from("*".repeat(input.value().chars().count()))]
            } else {
                vec![Line::from(input.value())]
            };
            frame.render_widget(
                Paragraph::new(body)
                    .scroll((0, scroll as u16))
                    .block(settings_panel_block(title, true)),
                modal,
            );
            if input_width > 0 {
                let cursor = input
                    .visual_cursor()
                    .saturating_sub(scroll)
                    .min(input_width.saturating_sub(1));
                frame.set_cursor_position((modal.x + 1 + cursor as u16, modal.y + 1));
            }
        }
        SettingsOverlay::SecretAction { selected, .. } => {
            let modal = centered_rect(48, 5, area);
            frame.render_widget(Clear, modal);
            let title = model
                .active_setting_label()
                .map(|label| format!(" {label} "))
                .unwrap_or_else(|| " Edit secret ".to_string());
            let items = ["Keep stored value", "Replace", "Clear"]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(settings_panel_block(title, true))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);
        }
        SettingsOverlay::ConfirmDiscard { selected } => {
            let modal = centered_rect(48, 4, area);
            frame.render_widget(Clear, modal);
            let items = ["Keep editing", "Discard and leave"]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(settings_panel_block(" Unsaved changes ".to_string(), true))
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, modal, &mut state);
        }
        SettingsOverlay::ConfirmManagement { command, selected } => {
            let modal = centered_rect(68, 9, area);
            frame.render_widget(Clear, modal);
            let block = settings_panel_block(format!(" Confirm {} ", command.label()), true);
            let inner = block.inner(modal);
            frame.render_widget(block, modal);
            let [description_area, choices_area] =
                Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).areas(inner);
            let description = match command {
                SessionSettingsCommand::Adopt => vec![
                    Line::from("Manage future launches with the platform default backend."),
                    Line::from("Keeps the cutex session and cute-codex history."),
                ],
                SessionSettingsCommand::Unmanage => vec![
                    Line::from(
                        "Clears managed launch, permission defaults, visibility, and quick action.",
                    ),
                    Line::from("Keeps session/history and does not close the current runtime."),
                ],
            };
            frame.render_widget(
                Paragraph::new(description).wrap(Wrap { trim: false }),
                description_area,
            );
            let items = ["Cancel", command.label()]
                .into_iter()
                .map(ListItem::new)
                .collect::<Vec<_>>();
            let list = List::new(items)
                .highlight_style(settings_highlight_style(true))
                .highlight_symbol("> ");
            let mut state = ListState::default().with_selected(Some(*selected));
            frame.render_stateful_widget(list, choices_area, &mut state);
        }
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn settings_panel_block(title: String, active: bool) -> Block<'static> {
    Block::bordered().title(title).border_style(if active {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    })
}

fn settings_highlight_style(active: bool) -> Style {
    let style = Style::new().fg(Color::White).bg(Color::DarkGray);
    if active {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn render_runtime_action_confirmation(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let Some(row) = model.active_row() else {
        return;
    };
    let global_default_profile = model.current_default_profile_name();
    let SelectorMode::ConfirmRuntimeAction {
        action,
        launch_profile,
        confirmed,
        ..
    } = &model.mode
    else {
        return;
    };
    let cancel_style = if *confirmed {
        Style::new().fg(Color::Gray)
    } else {
        Style::new()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    };
    let action_style = if *confirmed {
        Style::new()
            .fg(Color::White)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Yellow)
    };
    let (title, prompt, action_label, detail) = match action {
        SessionTuiAction::CloseAndRestart => (
            " Confirm restart ",
            format!("Close and restart runtime for {}?", row.agent),
            "  Close and restart  ",
            format!(
                "Restart profile: {}",
                row.launch_profile_detail(
                    launch_profile.as_deref(),
                    global_default_profile.as_deref(),
                )
            ),
        ),
        SessionTuiAction::CloseRuntime => (
            " Confirm close ",
            format!("Close runtime for {}?", row.agent),
            "  Close runtime  ",
            "The durable Cutex session and cute-codex history are kept.".to_string(),
        ),
        SessionTuiAction::RetireSession => (
            " Confirm retire ",
            format!("Retire managed session {}?", row.agent),
            "  Retire session  ",
            format!(
                "Profile: {}  Managed path: {}  Runtime: {}",
                row.configured_profile.as_deref().unwrap_or("default"),
                row.managed_path,
                if row.lifecycle == Some(CutexSessionLifecycleState::Offline) {
                    "already offline"
                } else {
                    "will be stopped and proven offline"
                }
            ),
        ),
        SessionTuiAction::RestoreSession => (
            " Confirm restore ",
            format!("Restore {} as active and offline?", row.agent),
            "  Restore session  ",
            "No runtime will launch, resume, attach, or select a profile.".to_string(),
        ),
        _ => return,
    };
    let body = vec![
        Line::from(""),
        Line::from(Span::styled(
            prompt,
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(detail, Style::new().fg(Color::Gray))),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Cancel  ", cancel_style),
            Span::raw("    "),
            Span::styled(action_label, action_style),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .block(Block::bordered().title(title)),
        area,
    );
}

fn render_runtime_close_progress(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let SelectorMode::ClosingRuntime {
        agent_name, action, ..
    } = &model.mode
    else {
        return;
    };
    let (verb, detail, title) = match action {
        SessionTuiAction::CloseRuntime => (
            "Closing runtime",
            "Waiting for closed or offline status.",
            " Closing runtime ",
        ),
        SessionTuiAction::RetireSession => (
            "Retiring session",
            "Stopping and proving the runtime offline before archive commit.",
            " Retiring session ",
        ),
        SessionTuiAction::RestoreSession => (
            "Restoring session",
            "Restoring active and offline without launching a runtime.",
            " Restoring session ",
        ),
        _ => return,
    };
    let body = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("{verb} for {agent_name}..."),
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(detail, Style::new().fg(Color::Gray))),
        Line::from(Span::styled(
            "Session and history are kept.",
            Style::new().fg(Color::Gray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(title)),
        area,
    );
}

fn selector_table_row<'a>(
    row: &'a SelectorRow,
    wide: bool,
    extra_wide: bool,
    default_profile: Option<&str>,
) -> Row<'a> {
    let state = if let Some(lifecycle) = row.lifecycle {
        let label = selector_state_label(row);
        let style = if label == "detached" {
            Style::new().fg(Color::Yellow)
        } else {
            lifecycle_style(lifecycle)
        };
        Cell::from(label).style(style)
    } else if row.target.is_profiles() {
        Cell::from("accounts").style(Style::new().fg(Color::Cyan))
    } else if row.target.is_retired_sessions() {
        Cell::from("archive").style(Style::new().fg(Color::Cyan))
    } else if row.target.is_cutex_projects() {
        Cell::from("permissions").style(Style::new().fg(Color::Cyan))
    } else if row.target.is_projects() {
        Cell::from("workspace").style(Style::new().fg(Color::Cyan))
    } else {
        Cell::from("global").style(Style::new().fg(Color::Cyan))
    };
    let primary_label = if row.target.is_retired_sessions() {
        "browse"
    } else if row.target.is_profiles() {
        "manage"
    } else if row.target.is_cutex_projects() || row.target.is_projects() {
        "open"
    } else if row.target.is_global_settings() {
        "settings"
    } else {
        row.actions
            .iter()
            .find(|item| item.primary)
            .map(|item| item.action.label())
            .unwrap_or("-")
    };
    let primary = Cell::from(primary_label).style(Style::new().fg(Color::Cyan));
    let agent = if row.target.is_system() {
        Cell::from(row.agent.as_str()).style(Style::new().fg(Color::Cyan))
    } else {
        Cell::from(row.agent.as_str())
    };
    let profile = if row.target.is_system() {
        Cell::from("-").style(Style::new().fg(Color::DarkGray))
    } else if let Some(profile) = row.configured_profile.as_deref() {
        Cell::from(profile).style(Style::new().fg(Color::Magenta))
    } else {
        Cell::from(format!("Default ({})", default_profile.unwrap_or("unset")))
            .style(Style::new().fg(Color::Gray))
    };
    let last_output = Cell::from(format_last_output_at(
        row.last_output_at.as_deref(),
        Utc::now(),
    ))
    .style(Style::new().fg(Color::Gray));
    if extra_wide {
        Row::new([
            agent,
            profile,
            state,
            last_output,
            Cell::from(row.host.as_str()),
            Cell::from(row.backend.as_str()),
            Cell::from(row.managed_path.as_str()),
            primary,
        ])
    } else if wide {
        Row::new([
            agent,
            profile,
            state,
            last_output,
            Cell::from(row.managed_path.as_str()),
            primary,
        ])
    } else {
        Row::new([agent, profile, state, primary])
    }
}

fn format_last_output_at(value: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    let Ok(value) = DateTime::parse_from_rfc3339(value) else {
        return "-".to_string();
    };
    let elapsed = now.signed_duration_since(value.with_timezone(&Utc));
    let seconds = elapsed.num_seconds().max(0);
    match seconds {
        0..=4 => "now".to_string(),
        5..=59 => format!("{seconds}s ago"),
        60..=3_599 => format!("{}m ago", seconds / 60),
        3_600..=86_399 => format!("{}h ago", seconds / 3_600),
        86_400..=604_799 => format!("{}d ago", seconds / 86_400),
        _ => value.format("%Y-%m-%d").to_string(),
    }
}

fn selector_state_label(row: &SelectorRow) -> &'static str {
    match row.lifecycle {
        Some(CutexSessionLifecycleState::Online) if row.backend == "alden" && !row.attachable => {
            "detached"
        }
        Some(lifecycle) => lifecycle.label(),
        None if row.target.is_profiles() => "accounts",
        None if row.target.is_retired_sessions() => "archive",
        None if row.target.is_cutex_projects() => "permissions",
        None if row.target.is_projects() => "workspace",
        None => "global",
    }
}

fn selector_default_profile_name(model: &SelectorModel) -> Option<&str> {
    model
        .rows
        .iter()
        .find(|row| row.target.is_global_settings())
        .and_then(|row| row.global_settings_snapshot.as_ref())
        .and_then(GlobalSettingsSnapshot::default_profile_name)
}

fn lifecycle_style(state: CutexSessionLifecycleState) -> Style {
    match state {
        CutexSessionLifecycleState::Online => Style::new().fg(Color::Green),
        CutexSessionLifecycleState::Stale => Style::new().fg(Color::Yellow),
        CutexSessionLifecycleState::Offline => Style::new().fg(Color::DarkGray),
    }
}

fn footer_hints(hints: &[(&'static str, &'static str)]) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(hints.len() * 4);
    let key_style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    for (key, description) in hints {
        spans.push(Span::styled(*key, key_style));
        if !description.is_empty() {
            spans.push(Span::raw(" "));
            spans.push(Span::raw(*description));
        }
        spans.push(Span::raw("  "));
    }
    spans
}

fn read_only_footer_hints(hints: &[(&'static str, &'static str)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        "read-only  ",
        Style::new().fg(Color::DarkGray),
    )];
    spans.extend(footer_hints(hints));
    spans
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, model: &SelectorModel) {
    let narrow = area.width < WIDE_LAYOUT_MIN_WIDTH;
    let very_narrow = area.width < 66;
    let mut spans = if model.action_overlay.is_some() {
        if very_narrow {
            footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
        } else {
            footer_hints(&[("Up/Down", "choose"), ("Enter", "stage"), ("Esc", "cancel")])
        }
    } else if let Some(overlay) = model.profile_overlay.as_ref() {
        match overlay {
            ProfileOverlay::Actions { .. } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
            }
            ProfileOverlay::Actions { .. } => footer_hints(&[
                ("Up/Down", "choose"),
                ("Enter", "select"),
                ("Esc", "cancel"),
            ]),
            ProfileOverlay::RenameInput { .. } if very_narrow => footer_hints(&[
                ("L/R", ""),
                ("Bksp/Del", ""),
                ("Ctrl+U", ""),
                ("Enter/Esc", ""),
            ]),
            ProfileOverlay::RenameInput { .. } if narrow => footer_hints(&[
                ("Left/Right", ""),
                ("Bksp/Del", "edit"),
                ("Ctrl+U", "clear"),
                ("Enter", "review"),
                ("Esc", ""),
            ]),
            ProfileOverlay::RenameInput { .. } => footer_hints(&[
                ("Left/Right", "move"),
                ("Bksp/Del", "edit"),
                ("Ctrl+U", "clear"),
                ("Enter", "review"),
                ("Esc", "cancel"),
            ]),
            ProfileOverlay::ConfirmRename { .. }
            | ProfileOverlay::ConfirmRemove { .. }
            | ProfileOverlay::ConfirmAddProfile { .. }
            | ProfileOverlay::ConfirmDiscardProfile { .. }
                if very_narrow =>
            {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
            }
            ProfileOverlay::ConfirmRename { .. }
            | ProfileOverlay::ConfirmRemove { .. }
            | ProfileOverlay::ConfirmAddProfile { .. }
            | ProfileOverlay::ConfirmDiscardProfile { .. } => footer_hints(&[
                ("Up/Down", "choose"),
                ("Enter", "select"),
                ("Esc", "cancel"),
            ]),
        }
    } else if let Some(overlay) = model.settings_overlay.as_ref() {
        match overlay {
            SettingsOverlay::Choice { .. } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
            }
            SettingsOverlay::SecretAction { .. } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
            }
            SettingsOverlay::Choice { .. } => {
                footer_hints(&[("Up/Down", "choose"), ("Enter", "stage"), ("Esc", "cancel")])
            }
            SettingsOverlay::SecretAction { .. } => footer_hints(&[
                ("Up/Down", "choose"),
                ("Enter", "select"),
                ("Esc", "cancel"),
            ]),
            SettingsOverlay::Groups { .. } | SettingsOverlay::Text { .. } if very_narrow => {
                footer_hints(&[
                    ("Up/Down", ""),
                    ("L/R", ""),
                    ("Bksp/Del", ""),
                    ("Enter", ""),
                    ("Esc", ""),
                ])
            }
            SettingsOverlay::Groups { .. } | SettingsOverlay::Text { .. } if narrow => {
                footer_hints(&[
                    ("Up/Down", "line"),
                    ("Left/Right", "edit"),
                    ("Bksp/Del", "edit"),
                    ("Ctrl+U", "clear"),
                    ("Enter", "stage"),
                    ("Esc", ""),
                ])
            }
            SettingsOverlay::Groups { .. } | SettingsOverlay::Text { .. } => footer_hints(&[
                ("Up/Down", "line"),
                ("Left/Right", "move"),
                ("Bksp/Del", "edit"),
                ("Ctrl+U", "clear"),
                ("Enter", "stage"),
                ("Esc", "cancel"),
            ]),
            SettingsOverlay::ConfirmDiscard { .. } | SettingsOverlay::ConfirmManagement { .. }
                if very_narrow =>
            {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Esc", "")])
            }
            SettingsOverlay::ConfirmDiscard { .. } | SettingsOverlay::ConfirmManagement { .. } => {
                footer_hints(&[
                    ("Up/Down", "choose"),
                    ("Enter", "select"),
                    ("Esc", "cancel"),
                ])
            }
        }
    } else {
        match &model.mode {
            SelectorMode::Agents if very_narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Right", ""),
                ("Tab", ""),
                ("Esc", ""),
            ]),
            SelectorMode::Agents if narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Right", ""),
                ("Tab", "settings"),
                ("Ctrl+X", "close"),
                ("Esc", ""),
            ]),
            SelectorMode::Agents => {
                let mut spans = footer_hints(&[
                    ("Up/Down", "move"),
                    ("Enter", "primary"),
                    ("Right", "actions"),
                    ("Tab", "settings"),
                    ("Ctrl+X", "close"),
                ]);
                if model.enhanced_keyboard && area.width >= 120 {
                    spans.extend(footer_hints(&[("Shift+Enter", "actions")]));
                }
                spans.extend(footer_hints(&[("Esc", "clear/exit")]));
                spans
            }
            SelectorMode::RecentSessions if model.recent.review().is_some() => footer_hints(&[
                ("Up/Down", "choose"),
                ("Enter", "confirm"),
                ("Esc", "cancel"),
            ]),
            SelectorMode::RecentSessions
                if matches!(model.recent.load_state(), RecentLoadState::Failed(_)) =>
            {
                footer_hints(&[("Enter", "retry"), ("Left/Esc", "back")])
            }
            SelectorMode::RecentSessions => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "review adoption"),
                ("Right", "load more"),
                ("Left/Esc", "back"),
            ]),
            SelectorMode::RetiredSessions { .. } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Left/Esc", "")])
            }
            SelectorMode::RetiredSessions { .. } => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "restore"),
                ("Left/Esc", "back"),
            ]),
            SelectorMode::Actions { .. } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter", ""), ("Left/Esc", "")])
            }
            SelectorMode::Actions { .. } => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "select"),
                ("Left/Esc", "back"),
            ]),
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } if model.settings_are_editable() && very_narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("A", ""),
                ("D", ""),
                ("Tab/Esc", ""),
            ]),
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } if model.settings_are_editable() && narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", "edit"),
                ("V", "view"),
                ("A", "apply"),
                ("D", "discard"),
                ("Tab/Esc", ""),
            ]),
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } if model.settings_are_editable() => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "edit"),
                ("V", "view"),
                ("A", "apply"),
                ("D", "discard"),
                ("Left/Tab/Esc", "list"),
            ]),
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } if very_narrow => footer_hints(&[("Up/Down", ""), ("V", ""), ("Left/Tab/Esc", "")]),
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } if narrow => {
                read_only_footer_hints(&[("Up/Down", ""), ("V", "view"), ("Left/Tab/Esc", "")])
            }
            SelectorMode::Settings {
                view: SettingsView::Expanded,
                ..
            } => read_only_footer_hints(&[
                ("Up/Down", "move"),
                ("V", "switch view"),
                ("Left/Tab/Esc", "list"),
            ]),
            SelectorMode::Settings { .. } if model.settings_are_editable() && very_narrow => {
                footer_hints(&[
                    ("Up/Down", ""),
                    ("Enter", ""),
                    ("Left", ""),
                    ("A", ""),
                    ("D", ""),
                    ("Tab/Esc", ""),
                ])
            }
            SelectorMode::Settings { .. } if model.settings_are_editable() && narrow => {
                footer_hints(&[
                    ("Up/Down", ""),
                    ("Right/Enter", ""),
                    ("Left", ""),
                    ("V", ""),
                    ("A", ""),
                    ("D", ""),
                    ("Tab/Esc", ""),
                ])
            }
            SelectorMode::Settings { .. } if model.settings_are_editable() => footer_hints(&[
                ("Up/Down", ""),
                ("Right/Enter", "open"),
                ("Left", "back"),
                ("V", "view"),
                ("A", "apply"),
                ("D", "discard"),
                ("Tab/Esc", "list"),
            ]),
            SelectorMode::Settings { .. } if very_narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Left", ""),
                ("V", ""),
                ("Tab/Esc", ""),
            ]),
            SelectorMode::Settings { .. } if narrow => read_only_footer_hints(&[
                ("Up/Down", ""),
                ("Right/Enter", ""),
                ("Left", ""),
                ("V", "view"),
                ("Tab/Esc", ""),
            ]),
            SelectorMode::Settings { .. } => read_only_footer_hints(&[
                ("Up/Down", "move"),
                ("Right/Enter", "open"),
                ("Left", "previous"),
                ("V", "view"),
                ("Tab/Esc", "list"),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Items,
                ..
            } if very_narrow => {
                footer_hints(&[("Up/Down", ""), ("Enter/Right", ""), ("Tab/Esc", "")])
            }
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Items,
                ..
            } => footer_hints(&[
                ("Up/Down", "move"),
                ("Home/End", "edge"),
                ("Enter/Right", "open"),
                ("Tab/Esc", "agents"),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                selected: 0,
                ..
            } if very_narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Left", ""),
                ("A", ""),
                ("D", ""),
                ("Tab/Esc", ""),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                selected: 0,
                ..
            } if narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("A", ""),
                ("D", ""),
                ("Left/Tab", ""),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                selected: 0,
                ..
            } => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "edit"),
                ("Left", "profiles"),
                ("A", "apply"),
                ("D", "discard"),
                ("Tab/Esc", "agents"),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                ..
            } if very_narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Right", ""),
                ("A", ""),
                ("D", ""),
                ("Left/Tab", ""),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                ..
            } if narrow => footer_hints(&[
                ("Up/Down", ""),
                ("Enter", ""),
                ("Right", ""),
                ("A", ""),
                ("D", ""),
                ("Left/Tab", ""),
            ]),
            SelectorMode::ProfileManager {
                focus: ProfileWorkspaceFocus::Editor,
                ..
            } => footer_hints(&[
                ("Up/Down", "move"),
                ("Enter", "edit"),
                ("Right", "actions"),
                ("A", "apply"),
                ("D", "discard"),
                ("Left", "profiles"),
                ("Tab/Esc", "agents"),
            ]),
            SelectorMode::ConfirmRuntimeAction { .. } if very_narrow => {
                footer_hints(&[("Left/Right", ""), ("Enter", ""), ("Esc", "")])
            }
            SelectorMode::ConfirmRuntimeAction { .. } => footer_hints(&[
                ("Left/Right", "choose"),
                ("Enter", "confirm"),
                ("Esc", "back"),
            ]),
            SelectorMode::ClosingRuntime { .. } => vec![Span::styled(
                "Close in progress",
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )],
        }
    };
    let mut status_spans = Vec::new();
    if let Some(notice) = model.notice.as_deref() {
        status_spans.push(Span::styled(
            notice.to_string(),
            Style::new().fg(Color::Green),
        ));
    }
    if let Some(warning) = model.warning.as_deref() {
        if !status_spans.is_empty() {
            status_spans.push(Span::raw("  "));
        }
        status_spans.push(Span::styled(
            warning.to_string(),
            Style::new().fg(Color::Yellow),
        ));
    }
    let exit_spans = if matches!(&model.mode, SelectorMode::ClosingRuntime { .. }) {
        Vec::new()
    } else {
        footer_hints(&[("Ctrl+C", "exit")])
    };
    let mut combined_width = spans.iter().map(Span::width).sum::<usize>()
        + exit_spans.iter().map(Span::width).sum::<usize>()
        + status_spans.iter().map(Span::width).sum::<usize>();
    if !status_spans.is_empty()
        && !narrow
        && combined_width.saturating_add(2) >= usize::from(area.width)
        && matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Groups { .. } | SettingsOverlay::Text { .. })
        )
    {
        spans = footer_hints(&[
            ("L/R", ""),
            ("Bksp/Del", ""),
            ("Ctrl+U", "clear"),
            ("Enter/Esc", ""),
        ]);
        combined_width = spans.iter().map(Span::width).sum::<usize>()
            + exit_spans.iter().map(Span::width).sum::<usize>()
            + status_spans.iter().map(Span::width).sum::<usize>();
    }
    if !status_spans.is_empty()
        && (narrow || combined_width.saturating_add(2) >= usize::from(area.width))
    {
        spans = status_spans;
        spans.push(Span::raw("  "));
        spans.extend(exit_spans);
    } else {
        spans.extend(exit_spans);
        spans.extend(status_spans);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::Parser;
    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::cli::args::{Cli, CommandKind};
    use cutex::im::registry::ImRegistry;
    use cutex::profiles::deepseek;
    use cutex::profiles::model::RuntimeConfig;
    use cutex::session::model::CutexSessionRuntimeBackend;
    use ratatui::backend::TestBackend;

    use super::super::test_home::IsolatedTestHome;

    fn row(
        key: &str,
        agent: &str,
        lifecycle: CutexSessionLifecycleState,
        pinned: bool,
        managed: bool,
    ) -> SelectorRow {
        let mut actions = vec![
            SessionTuiActionItem {
                action: SessionTuiAction::ResumeAttach,
                detail: "Take over the managed TUI",
                primary: true,
            },
            SessionTuiActionItem {
                action: SessionTuiAction::Online,
                detail: "Bring the managed runtime online",
                primary: false,
            },
        ];
        if lifecycle == CutexSessionLifecycleState::Online {
            actions.push(SessionTuiActionItem {
                action: SessionTuiAction::CloseAndRestart,
                detail: "Close runtime, then bring it online with the selected profile",
                primary: false,
            });
            actions.push(SessionTuiActionItem {
                action: SessionTuiAction::CloseRuntime,
                detail: "Gracefully close runtime; keep session and history",
                primary: false,
            });
        }
        SelectorRow {
            target: SelectorTarget::Agent(key.to_string()),
            agent: agent.to_string(),
            configured_profile: Some("aemeath".to_string()),
            lifecycle: Some(lifecycle),
            host: "tethys".to_string(),
            backend: "alden".to_string(),
            managed_path: "~/Projects/cutex".to_string(),
            retired_at: None,
            revision: 1,
            activity_session_id: Some(key.to_string()),
            last_output_at: None,
            actions,
            settings: vec![
                SessionTuiSettingCategory {
                    label: "Identity",
                    options: vec![
                        SessionTuiSettingOption {
                            label: "Agent name",
                            value: agent.to_string(),
                            field: None,
                            global_field: None,
                            profile_field: None,
                            command: None,
                            dirty: false,
                        },
                        SessionTuiSettingOption {
                            label: "Host",
                            value: "tethys".to_string(),
                            field: None,
                            global_field: None,
                            profile_field: None,
                            command: None,
                            dirty: false,
                        },
                    ],
                },
                SessionTuiSettingCategory {
                    label: "Launch",
                    options: vec![SessionTuiSettingOption {
                        label: "Runtime backend",
                        value: "alden".to_string(),
                        field: None,
                        global_field: None,
                        profile_field: None,
                        command: None,
                        dirty: false,
                    }],
                },
            ],
            settings_snapshot: None,
            global_settings_snapshot: None,
            attachable: lifecycle == CutexSessionLifecycleState::Online,
            pinned,
            managed,
        }
    }

    fn global_row() -> SelectorRow {
        global_settings_row(&CodezConfig::default())
    }

    fn profiles_test_row() -> SelectorRow {
        profiles_row(&CodezConfig::default(), &[])
    }

    fn profile_catalog_entry(name: &str, active: bool) -> ProfileCatalogEntry {
        ProfileCatalogEntry {
            id: format!("id-{name}"),
            name: name.to_string(),
            email: Some(format!("{name}@example.test")),
            plan_type: Some("pro".to_string()),
            source: Some("official".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: "codex".to_string(),
            default_cli_args: vec!["--model".to_string(), "gpt-test".to_string()],
            agent_name: Some(format!("{name}-agent")),
            api_key_configured: false,
            codex_config: Some(Default::default()),
            codex_config_error: None,
            active,
        }
    }

    const EDITABLE_AGENT_KEY: &str = "cutex.editable";

    fn editable_record() -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            EDITABLE_AGENT_KEY.to_string(),
            Some("019e-editable".to_string()),
            "tethys".to_string(),
            "/tmp/editable".to_string(),
            None,
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("editable record");
        record.display_name_hint = Some("editable-agent".to_string());
        record.quick_action = CutexSessionQuickActionMode::Pinned;
        record.permission_defaults = Some("workspace".to_string());
        record.approval_policy = Some("on-request".to_string());
        record.sandbox_mode = Some("workspace-write".to_string());
        record.reasoning_defaults = Some("medium".to_string());
        record
    }

    fn editable_model(record: &CutexSessionRecord) -> SelectorModel {
        editable_model_with_profiles(record, &[])
    }

    fn editable_model_with_profiles(
        record: &CutexSessionRecord,
        profile_names: &[String],
    ) -> SelectorModel {
        SelectorModel::new(
            vec![selector_row(
                EDITABLE_AGENT_KEY,
                record,
                &[],
                &[],
                profile_names,
            )],
            false,
            false,
        )
    }

    fn stage_full_access(model: &mut SelectorModel) {
        assert_eq!(
            model.handle(SelectorEvent::OpenSettings),
            SelectorControl::Continue
        );
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                field: SettingsEditField::Session(SessionSettingsField::PermissionPreset),
                selected: 2,
                custom_value: None,
                ..
            })
        ));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("full-access")
        );
    }

    fn select_management_setting(model: &mut SelectorModel) {
        if matches!(model.mode, SelectorMode::Agents) {
            assert_eq!(
                model.handle(SelectorEvent::OpenSettings),
                SelectorControl::Continue
            );
        }
        let target = model.active_row().expect("active agent row").target.clone();
        let (category, option) = model
            .active_row()
            .expect("active agent row")
            .settings
            .iter()
            .enumerate()
            .find_map(|(category, settings)| {
                settings
                    .options
                    .iter()
                    .position(|option| option.label == "Management")
                    .map(|option| (category, option))
            })
            .expect("management setting");
        model.mode = SelectorMode::Settings {
            target,
            category,
            option,
            focus: SettingsFocus::Options,
            view: SettingsView::Expanded,
        };
    }

    fn select_global_setting(model: &mut SelectorModel, field: GlobalSettingsField) {
        if matches!(model.mode, SelectorMode::Agents) {
            assert_eq!(
                model.handle(SelectorEvent::Activate),
                SelectorControl::Continue
            );
        }
        let (category, option) = model
            .active_row()
            .expect("global row")
            .settings
            .iter()
            .enumerate()
            .find_map(|(category, settings)| {
                settings
                    .options
                    .iter()
                    .position(|option| option.global_field == Some(field))
                    .map(|option| (category, option))
            })
            .expect("global setting");
        model.mode = SelectorMode::Settings {
            target: SelectorTarget::GlobalSettings,
            category,
            option,
            focus: SettingsFocus::Options,
            view: SettingsView::Expanded,
        };
    }

    fn open_profiles(model: &mut SelectorModel, profiles: Vec<ProfileCatalogEntry>) {
        if !model.rows.iter().any(|row| row.target.is_profiles()) {
            let snapshot = model
                .rows
                .iter()
                .find(|row| row.target.is_global_settings())
                .and_then(|row| row.global_settings_snapshot.clone())
                .expect("Global settings snapshot");
            model.rows.push(SelectorRow {
                target: SelectorTarget::Profiles,
                agent: "Profiles".to_string(),
                configured_profile: None,
                lifecycle: None,
                host: "-".to_string(),
                backend: "accounts".to_string(),
                managed_path: "-".to_string(),
                retired_at: None,
                revision: 0,
                activity_session_id: None,
                last_output_at: None,
                actions: Vec::new(),
                settings: Vec::new(),
                settings_snapshot: None,
                global_settings_snapshot: Some(snapshot),
                attachable: false,
                pinned: false,
                managed: false,
            });
            sort_rows(&mut model.rows);
        }
        model.mode = SelectorMode::Agents;
        model
            .workspace_selection
            .select(Some(SelectorTarget::Profiles));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::OpenProfileManager
        );
        model.open_profile_manager(profiles);
    }

    fn rendered_text(width: u16, model: &SelectorModel) -> String {
        rendered_text_at(width, 16, model)
    }

    fn rendered_text_at(width: u16, height: u16, model: &SelectorModel) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_selector(frame, model))
            .expect("render selector");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer.cell((x, y)).expect("cell").symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn cutex_tui_is_an_explicit_command() {
        let cli = Cli::try_parse_from(["cutex", "tui"]).expect("parse tui command");
        assert!(matches!(cli.command, Some(CommandKind::Tui)));
    }

    #[test]
    fn selector_names_permission_projects_and_native_workspaces_unambiguously() {
        let permission_project = cutex_projects_row();
        let native_workspace = projects_row();
        assert_eq!(permission_project.agent, "Cutex Projects");
        assert_eq!(permission_project.backend, "permission model");
        assert_eq!(native_workspace.agent, "Workspaces");
        assert_eq!(native_workspace.backend, "Codex catalog");
        assert_ne!(permission_project.target, native_workspace.target);
    }

    #[test]
    fn non_terminal_invocations_are_rejected_before_ui_setup() {
        assert!(require_interactive_terminal(true, true).is_ok());
        assert!(require_interactive_terminal(false, true).is_err());
        assert!(require_interactive_terminal(true, false).is_err());
    }

    #[test]
    fn session_row_separates_lifecycle_from_attachability_and_keeps_takeover_primary() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.alden-row".to_string(),
            Some("019e-alden-row".to_string()),
            "tethys".to_string(),
            "/tmp/alden-row".to_string(),
            Some("aemeath".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("alden-row".to_string());
        record.runtime_backend = cutex::session::model::CutexSessionRuntimeBackend::CuteAlden;
        record.managed_cwd = Some("/tmp/managed".to_string());
        record.alden_session_name = Some("cutex.alden-row.runtime".to_string());
        record.alden_pid = Some(std::process::id());
        let alden_sessions = vec![CuteAldenSession {
            pid: std::process::id(),
            name: record.alden_session_name.clone(),
        }];

        let row = selector_row("cutex.alden-row", &record, &alden_sessions, &[], &[]);

        assert_eq!(row.agent, "alden-row");
        assert_eq!(row.lifecycle, Some(CutexSessionLifecycleState::Online));
        assert!(row.attachable);
        assert_eq!(row.host, "tethys");
        assert_eq!(row.backend, "alden");
        assert_eq!(row.managed_path, "/tmp/managed");
        assert_eq!(row.actions[0].action, SessionTuiAction::ResumeAttach);
        assert!(row.actions[0].primary);
        assert_eq!(
            row.actions
                .iter()
                .find(|item| item.primary)
                .map(|item| item.action.label()),
            Some("takeover"),
        );
    }

    #[test]
    fn detached_alden_row_names_the_missing_tui_and_opens_it_as_primary() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.detached-row".to_string(),
            Some("019e-detached-row".to_string()),
            "tethys".to_string(),
            "/tmp/detached-row".to_string(),
            Some("aemeath".to_string()),
            "2026-08-08T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("detached-row".to_string());
        record.runtime_backend = cutex::session::model::CutexSessionRuntimeBackend::CuteAlden;
        record.current_runtime_agent_id = Some("cutex.detached-row.runtime".to_string());
        record.app_server_runtime = Some(cutex::session::model::CutexAppServerRuntimeBinding {
            transport: cutex::session::model::CutexAppServerTransport::UnixSocket,
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
            id: "cutex.detached-row.runtime".to_string(),
            name: "detached-row.runtime".to_string(),
            base_name: Some("detached-row".to_string()),
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

        let row = selector_row("cutex.detached-row", &record, &[], &live_agents, &[]);

        assert_eq!(row.lifecycle, Some(CutexSessionLifecycleState::Online));
        assert!(!row.attachable);
        assert_eq!(selector_state_label(&row), "detached");
        assert_eq!(
            row.actions
                .iter()
                .find(|item| item.primary)
                .map(|item| item.action),
            Some(SessionTuiAction::OpenTui)
        );
    }

    #[test]
    fn agent_filter_keeps_profiles_and_global_settings_as_the_final_rows() {
        let mut model = SelectorModel::new(
            vec![
                row(
                    "online",
                    "alpha-online",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                row(
                    "pinned",
                    "beta-pinned",
                    CutexSessionLifecycleState::Offline,
                    true,
                    false,
                ),
                row(
                    "managed",
                    "qa-managed",
                    CutexSessionLifecycleState::Offline,
                    false,
                    true,
                ),
                row(
                    "history",
                    "qa-history",
                    CutexSessionLifecycleState::Offline,
                    false,
                    false,
                ),
                profiles_test_row(),
                global_row(),
            ],
            false,
            false,
        );

        let initial = model
            .visible_rows()
            .iter()
            .map(|row| match &row.target {
                SelectorTarget::Agent(key) | SelectorTarget::RetiredAgent(key) => key.as_str(),
                SelectorTarget::RecentSessions => "recent",
                SelectorTarget::RetiredSessions => "retired",
                SelectorTarget::CutexProjects => "cutex-projects",
                SelectorTarget::Projects => "workspaces",
                SelectorTarget::Profiles => "profiles",
                SelectorTarget::GlobalSettings => "global",
            })
            .collect::<Vec<_>>();
        assert_eq!(initial, vec!["online", "pinned", "profiles", "global"]);

        assert_eq!(
            model.handle(SelectorEvent::Insert('q')),
            SelectorControl::Continue
        );
        let filtered = model
            .visible_rows()
            .iter()
            .map(|row| match &row.target {
                SelectorTarget::Agent(key) | SelectorTarget::RetiredAgent(key) => key.as_str(),
                SelectorTarget::RecentSessions => "recent",
                SelectorTarget::RetiredSessions => "retired",
                SelectorTarget::CutexProjects => "cutex-projects",
                SelectorTarget::Projects => "workspaces",
                SelectorTarget::Profiles => "profiles",
                SelectorTarget::GlobalSettings => "global",
            })
            .collect::<Vec<_>>();
        assert_eq!(filtered, vec!["managed", "profiles", "global"]);
        assert_eq!(model.query.value(), "q");

        for query in ['g', 'p'] {
            model.handle(SelectorEvent::ClearInput);
            model.handle(SelectorEvent::Insert(query));
            let visible = model
                .visible_rows()
                .iter()
                .map(|row| match &row.target {
                    SelectorTarget::Agent(key) | SelectorTarget::RetiredAgent(key) => key.as_str(),
                    SelectorTarget::RecentSessions => "recent",
                    SelectorTarget::RetiredSessions => "retired",
                    SelectorTarget::CutexProjects => "cutex-projects",
                    SelectorTarget::Projects => "workspaces",
                    SelectorTarget::Profiles => "profiles",
                    SelectorTarget::GlobalSettings => "global",
                })
                .collect::<Vec<_>>();
            assert!(visible.ends_with(&["profiles", "global"]));
        }
    }

    #[test]
    fn empty_filter_reports_offline_managed_agents_as_searchable() {
        let mut model = SelectorModel::new(
            vec![
                row(
                    "online",
                    "alpha-online",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                row(
                    "offline-managed",
                    "beta-offline",
                    CutexSessionLifecycleState::Offline,
                    false,
                    true,
                ),
                row(
                    "offline-history",
                    "gamma-history",
                    CutexSessionLifecycleState::Offline,
                    false,
                    false,
                ),
                profiles_test_row(),
                global_row(),
            ],
            false,
            false,
        );

        assert_eq!(model.hidden_searchable_agent_count(), 1);
        model.handle(SelectorEvent::Insert('b'));
        assert_eq!(model.hidden_searchable_agent_count(), 0);
    }

    #[test]
    fn tui_store_load_reconciles_registry_before_reading_durable_store() {
        use std::cell::RefCell;

        let calls = RefCell::new(Vec::new());
        let store = load_reconciled_session_store_with(
            || {
                calls.borrow_mut().push("load registry");
                Ok(ImRegistry::default())
            },
            |_| {
                calls.borrow_mut().push("reconcile registry");
                Ok(())
            },
            || {
                calls.borrow_mut().push("load durable store");
                Ok(CutexSessionStore::default())
            },
        )
        .expect("load reconciled TUI store");

        assert!(store.sessions.is_empty());
        assert_eq!(
            calls.into_inner(),
            ["load registry", "reconcile registry", "load durable store"]
        );
    }

    #[test]
    fn main_navigation_wraps_between_first_agent_and_final_global_settings() {
        let mut model = SelectorModel::new(
            vec![
                row(
                    "alpha",
                    "alpha",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                row(
                    "beta",
                    "beta",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                profiles_test_row(),
                global_row(),
            ],
            false,
            false,
        );

        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("alpha".to_string()))
        );
        model.handle(SelectorEvent::Up);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::GlobalSettings)
        );
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("alpha".to_string()))
        );
        model.handle(SelectorEvent::Last);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::GlobalSettings)
        );
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("alpha".to_string()))
        );
    }

    #[test]
    fn agent_settings_default_to_expanded_and_preserve_selection_across_views() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );

        assert_eq!(
            model.handle(SelectorEvent::OpenSettings),
            SelectorControl::Continue
        );
        assert_eq!(
            model.mode,
            SelectorMode::Settings {
                target: SelectorTarget::Agent("agent".to_string()),
                category: 0,
                option: 0,
                focus: SettingsFocus::Options,
                view: SettingsView::Expanded,
            }
        );
        assert_eq!(
            expanded_setting_table_row_index(
                &model.active_row().expect("agent row").settings,
                0,
                0,
            ),
            Some(1)
        );

        model.handle(SelectorEvent::Up);
        assert_eq!(model.selected_setting_category_index(), Some(1));
        assert_eq!(model.selected_setting_option_index(), Some(0));
        assert_eq!(
            expanded_setting_table_row_index(
                &model.active_row().expect("agent row").settings,
                1,
                0,
            ),
            Some(4)
        );
        model.handle(SelectorEvent::Down);
        assert_eq!(model.selected_setting_category_index(), Some(0));
        model.handle(SelectorEvent::Down);
        assert_eq!(model.selected_setting_option_index(), Some(1));

        model.handle(SelectorEvent::Insert('v'));
        assert_eq!(model.settings_view(), Some(SettingsView::Categories));
        assert_eq!(model.settings_focus(), Some(SettingsFocus::Options));
        assert_eq!(model.selected_setting_category_index(), Some(0));
        assert_eq!(model.selected_setting_option_index(), Some(1));

        model.handle(SelectorEvent::Insert('V'));
        assert_eq!(model.settings_view(), Some(SettingsView::Expanded));
        assert_eq!(model.selected_setting_category_index(), Some(0));
        assert_eq!(model.selected_setting_option_index(), Some(1));
        model.handle(SelectorEvent::Back);
        assert_eq!(model.mode, SelectorMode::Agents);

        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::OpenSettings);
        assert_eq!(model.mode, SelectorMode::Agents);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("agent".to_string()))
        );
    }

    #[test]
    fn staged_permission_is_written_only_by_apply_and_refreshes_the_same_row() {
        let record = editable_record();
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        let mut model = editable_model(&record);

        stage_full_access(&mut model);
        assert_eq!(store.sessions.get(EDITABLE_AGENT_KEY), Some(&record));

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected apply request, got {control:?}"),
        };
        assert_eq!(request.key, EDITABLE_AGENT_KEY);
        assert_eq!(request.changed_count, 1);

        let updated =
            apply_session_settings_to_store(&mut store, &request, &[]).expect("apply draft");
        assert_eq!(updated.permission_defaults.as_deref(), Some("full-access"));
        assert_eq!(updated.approval_policy, record.approval_policy);
        assert_eq!(updated.sandbox_mode, record.sandbox_mode);
        assert_eq!(updated.model_defaults, record.model_defaults);
        assert_eq!(updated.reasoning_defaults, record.reasoning_defaults);

        model.settings_apply_succeeded(
            EDITABLE_AGENT_KEY,
            &updated,
            &[],
            request.changed_count,
            false,
            None,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 1 setting(s)"));
        let narrow = rendered_text_at(50, 16, &model);
        assert!(narrow.contains("Saved 1 setting(s)"));
        assert!(narrow.contains("Ctrl+C exit"));
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("full-access")
        );
        assert!(matches!(
            model.mode,
            SelectorMode::Settings {
                target: SelectorTarget::Agent(ref key),
                ..
            } if key == EDITABLE_AGENT_KEY
        ));
    }

    #[test]
    fn management_confirmation_defaults_to_cancel_and_returns_a_typed_request() {
        let record = editable_record();
        let mut model = editable_model(&record);
        select_management_setting(&mut model);

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::ConfirmManagement {
                command: SessionSettingsCommand::Adopt,
                selected: 0,
            })
        ));
        let confirmation = rendered_text_at(80, 18, &model);
        assert!(confirmation.contains("Confirm Adopt"));
        assert!(confirmation.contains("platform default backend"));
        assert!(confirmation.contains("cute-codex history"));

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.settings_overlay.is_none());

        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::ManageSession(SessionManagementRequest {
                key: EDITABLE_AGENT_KEY.to_string(),
                command: SessionSettingsCommand::Adopt,
                profile_names: Vec::new(),
            })
        );
    }

    #[test]
    fn management_command_is_blocked_until_the_settings_draft_is_resolved() {
        let record = editable_record();
        let mut model = editable_model(&record);
        stage_full_access(&mut model);
        select_management_setting(&mut model);

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(
            model.warning.as_deref(),
            Some("Apply or discard staged settings before changing management")
        );
    }

    #[test]
    fn management_commands_use_service_semantics_without_mutating_runtime_identity() {
        let mut local = editable_record();
        local.current_runtime_agent_id = Some("runtime-occurrence".to_string());
        local.agent_groups.clear();
        let untouched = CutexSessionRecord::new_at(
            "cutex.untouched".to_string(),
            Some("019e-untouched".to_string()),
            "tethys".to_string(),
            "/tmp/untouched".to_string(),
            None,
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("untouched record");
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), local.clone());
        store
            .sessions
            .insert("cutex.untouched".to_string(), untouched.clone());

        let adopt = SessionManagementRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            command: SessionSettingsCommand::Adopt,
            profile_names: Vec::new(),
        };
        apply_session_management_to_store(&mut store, &adopt).expect("adopt local agent");
        let adopted = store
            .sessions
            .get(EDITABLE_AGENT_KEY)
            .expect("adopted record")
            .clone();
        assert_eq!(
            adopted.registration_class,
            AgentRegistrationClass::Persistent
        );
        assert_eq!(
            adopted.runtime_backend,
            if cfg!(windows) {
                CutexSessionRuntimeBackend::HostForeground
            } else {
                CutexSessionRuntimeBackend::CuteAlden
            }
        );
        assert!(adopted.agent_enabled);
        assert!(!adopted.agent_groups.is_empty());
        assert_eq!(adopted.codex_session_id, local.codex_session_id);
        assert_eq!(
            adopted.current_runtime_agent_id,
            local.current_runtime_agent_id
        );
        assert_eq!(adopted.cwd, local.cwd);
        assert_eq!(store.sessions.get("cutex.untouched"), Some(&untouched));

        let managed = store
            .sessions
            .get_mut(EDITABLE_AGENT_KEY)
            .expect("managed record");
        managed.exposed_to_backend = true;
        managed.managed_cwd = Some("/tmp/managed".to_string());
        managed.default_cli_args = vec!["--no-alt-screen".to_string()];

        let unmanage = SessionManagementRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            command: SessionSettingsCommand::Unmanage,
            profile_names: Vec::new(),
        };
        apply_session_management_to_store(&mut store, &unmanage).expect("unmanage agent");
        let unmanaged = store
            .sessions
            .get(EDITABLE_AGENT_KEY)
            .expect("unmanaged record");
        assert_eq!(
            unmanaged.registration_class,
            AgentRegistrationClass::LocalOnly
        );
        assert!(!unmanaged.agent_enabled);
        assert!(!unmanaged.exposed_to_backend);
        assert_eq!(unmanaged.managed_cwd, None);
        assert_eq!(unmanaged.quick_action, CutexSessionQuickActionMode::Auto);
        assert!(unmanaged.default_cli_args.is_empty());
        assert_eq!(unmanaged.permission_defaults, None);
        assert_eq!(unmanaged.approval_policy, None);
        assert_eq!(unmanaged.sandbox_mode, None);
        assert_eq!(unmanaged.model_defaults, None);
        assert_eq!(unmanaged.reasoning_defaults, None);
        assert_eq!(unmanaged.codex_session_id, local.codex_session_id);
        assert_eq!(
            unmanaged.current_runtime_agent_id,
            local.current_runtime_agent_id
        );
        assert_eq!(store.sessions.get("cutex.untouched"), Some(&untouched));
    }

    #[test]
    fn management_success_refreshes_the_row_and_survives_a_stale_snapshot() {
        let local = editable_record();
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), local.clone());
        let request = SessionManagementRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            command: SessionSettingsCommand::Adopt,
            profile_names: Vec::new(),
        };
        apply_session_management_to_store(&mut store, &request).expect("adopt agent");
        let adopted = store
            .sessions
            .get(EDITABLE_AGENT_KEY)
            .expect("adopted record")
            .clone();
        let mut model = SelectorModel::new(
            vec![selector_row(EDITABLE_AGENT_KEY, &local, &[], &[], &[])],
            true,
            false,
        );
        model.handle(SelectorEvent::OpenSettings);

        model.session_management_succeeded(
            EDITABLE_AGENT_KEY,
            SessionSettingsCommand::Adopt,
            &adopted,
            &[],
            None,
        );
        let row = model.active_row().expect("updated row");
        assert!(row.managed);
        assert_eq!(
            row.backend,
            runtime_backend_short_label(adopted.runtime_backend)
        );
        assert!(row.settings.iter().any(|category| {
            category.options.iter().any(|option| {
                option.command == Some(SessionSettingsCommand::Unmanage)
                    && option.value == "unmanage"
            })
        }));
        assert_eq!(model.notice.as_deref(), Some("Adopted agent"));
        assert!(rendered_text_at(100, 24, &model).contains("Adopted agent  Ctrl+C exit"));

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![selector_row(EDITABLE_AGENT_KEY, &local, &[], &[], &[])],
            warning: None,
        });
        assert!(model.active_row().expect("overridden row").managed);
    }

    #[test]
    fn unmanage_confirmation_explains_that_the_current_runtime_is_untouched() {
        let mut record = editable_record();
        record.registration_class = AgentRegistrationClass::Persistent;
        let mut model = editable_model(&record);
        select_management_setting(&mut model);

        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::ConfirmManagement {
                command: SessionSettingsCommand::Unmanage,
                selected: 0,
            })
        ));
        let confirmation = rendered_text_at(80, 18, &model);
        assert!(confirmation.contains("Confirm Unmanage"));
        assert!(confirmation.contains("does not close the current runtime"));
    }

    #[test]
    fn profile_choice_is_staged_then_applied_to_only_the_selected_session() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        let untouched = CutexSessionRecord::new_at(
            "cutex.untouched".to_string(),
            None,
            "tethys".to_string(),
            "/tmp/untouched".to_string(),
            Some("alpha".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("untouched record");
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        store
            .sessions
            .insert("cutex.untouched".to_string(), untouched.clone());
        let mut model = editable_model_with_profiles(&record, &profile_names);

        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                field: SettingsEditField::Session(SessionSettingsField::Profile),
                choices,
                selected: 1,
                custom_value: None,
            }) if choices.iter().map(|choice| choice.label.as_str()).collect::<Vec<_>>() == ["Follow global default", "alpha", "beta"]
        ));
        let overlay = rendered_text_at(80, 24, &model);
        assert!(overlay.contains("Follow global default"));
        assert!(overlay.contains("alpha"));
        assert!(overlay.contains("beta"));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);

        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(store.sessions.get(EDITABLE_AGENT_KEY), Some(&record));
        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected apply request, got {control:?}"),
        };
        let updated = apply_session_settings_to_store(&mut store, &request, &profile_names)
            .expect("apply profile");

        assert_eq!(updated.profile.as_deref(), Some("beta"));
        assert_ne!(updated.updated_at, record.updated_at);
        assert_eq!(store.sessions.get("cutex.untouched"), Some(&untouched));
        model.settings_apply_succeeded(
            EDITABLE_AGENT_KEY,
            &updated,
            &profile_names,
            request.changed_count,
            false,
            None,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 1 setting(s)"));
        assert_eq!(
            model
                .active_row()
                .and_then(|row| row.configured_profile.as_deref()),
            Some("beta")
        );
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("beta")
        );
    }

    #[test]
    fn routing_tag_editor_stays_open_on_invalid_groups() {
        let mut record = editable_record();
        record.agent_groups = vec!["cutex".to_string()];
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        for _ in 0..11 {
            model.handle(SelectorEvent::Down);
        }

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Groups {
                field: SettingsEditField::Session(SessionSettingsField::AgentGroups),
                inputs,
                selected: 0,
            }) if inputs.len() == 2 && inputs[0].value() == "cutex"
        ));
        let editor = rendered_text_at(80, 24, &model);
        assert!(editor.contains("Message groups"));
        assert!(editor.contains("cutex"));
        for _ in 0..5 {
            model.handle(SelectorEvent::Backspace);
        }
        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Groups {
                field: SettingsEditField::Session(SessionSettingsField::AgentGroups),
                inputs,
                ..
            }) if inputs[0].value().is_empty()
        ));
        assert_eq!(model.settings_dirty_count(), 0);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("At least one")));
    }

    #[test]
    fn message_groups_editor_uses_one_editable_line_per_group() {
        let mut record = editable_record();
        record.agent_groups = vec!["cutex".to_string()];
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        for _ in 0..11 {
            model.handle(SelectorEvent::Down);
        }
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        for character in "waveline".chars() {
            model.handle(SelectorEvent::Insert(character));
        }

        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Groups {
                inputs,
                selected: 1,
                ..
            }) if inputs.len() == 3
                && inputs[0].value() == "cutex"
                && inputs[1].value() == "waveline"
        ));
        let rendered = rendered_text_at(80, 24, &model);
        let cutex_line = rendered
            .lines()
            .position(|line| line.contains("cutex"))
            .expect("first group line");
        let waveline_line = rendered
            .lines()
            .position(|line| line.contains("waveline"))
            .expect("second group line");
        assert!(cutex_line < waveline_line);
        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("cutex, waveline")
        );
    }

    #[test]
    fn routing_apply_updates_three_typed_fields_and_only_the_selected_session() {
        let mut record = editable_record();
        record.registration_class = AgentRegistrationClass::Persistent;
        record.agent_groups = vec!["cutex".to_string()];
        record.exposed_to_backend = false;
        record.quick_action = CutexSessionQuickActionMode::Auto;
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                SessionSettingsField::AgentGroups,
                Some("waveline, cutex".to_string()),
            )
            .expect("stage groups");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::WorkbenchVisibility,
                Some("visible".to_string()),
            )
            .expect("stage visibility");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::QuickAction,
                Some("pinned".to_string()),
            )
            .expect("stage quick action");
        let request = SessionSettingsApplyRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            draft,
            profile_names: Vec::new(),
            changed_count: 3,
        };
        let untouched = CutexSessionRecord::new_at(
            "cutex.untouched.routing".to_string(),
            None,
            "tethys".to_string(),
            "/tmp/untouched".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("untouched record");
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        store
            .sessions
            .insert("cutex.untouched.routing".to_string(), untouched.clone());

        let updated = apply_session_settings_to_store(&mut store, &request, &[])
            .expect("apply routing settings");

        assert_eq!(updated.agent_groups, ["waveline", "cutex"]);
        assert!(updated.exposed_to_backend);
        assert_eq!(updated.quick_action, CutexSessionQuickActionMode::Pinned);
        assert_eq!(updated.permission_defaults, record.permission_defaults);
        assert_eq!(
            store.sessions.get("cutex.untouched.routing"),
            Some(&untouched)
        );
    }

    #[test]
    fn identity_and_launch_apply_updates_one_session_and_homepage_projection() {
        let mut record = editable_record();
        record.runtime_backend = CutexSessionRuntimeBackend::Host;
        record.managed_cwd = Some("/tmp/old-managed".to_string());
        record.default_cli_args = vec!["--old".to_string()];
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                SessionSettingsField::AgentName,
                Some("renamed-agent".to_string()),
            )
            .expect("stage name");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::RuntimeBackend,
                Some("alden".to_string()),
            )
            .expect("stage backend");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::ManagedCwd,
                Some("/tmp/new-managed".to_string()),
            )
            .expect("stage cwd");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::ExtraCliArgs,
                Some("--model 'gpt next'".to_string()),
            )
            .expect("stage args");
        let request = SessionSettingsApplyRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            draft,
            profile_names: Vec::new(),
            changed_count: 4,
        };
        let untouched = CutexSessionRecord::new_at(
            "cutex.untouched.launch".to_string(),
            None,
            "tethys".to_string(),
            "/tmp/untouched".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("untouched record");
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        store
            .sessions
            .insert("cutex.untouched.launch".to_string(), untouched.clone());

        let updated = apply_session_settings_to_store(&mut store, &request, &[])
            .expect("apply identity and launch");

        assert_eq!(updated.display_name_hint.as_deref(), Some("renamed-agent"));
        assert_eq!(
            updated.runtime_backend,
            CutexSessionRuntimeBackend::CuteAlden
        );
        assert_eq!(updated.managed_cwd.as_deref(), Some("/tmp/new-managed"));
        assert_eq!(updated.default_cli_args, ["--model", "gpt next"]);
        assert_eq!(updated.permission_defaults, record.permission_defaults);
        assert_eq!(
            store.sessions.get("cutex.untouched.launch"),
            Some(&untouched)
        );

        let mut model = SelectorModel::new(
            vec![
                selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[]),
                global_row(),
            ],
            true,
            false,
        );
        model.handle(SelectorEvent::OpenSettings);
        model.settings_apply_succeeded(EDITABLE_AGENT_KEY, &updated, &[], 4, true, None);
        let row = model
            .rows
            .iter()
            .find(|row| row.target == SelectorTarget::Agent(EDITABLE_AGENT_KEY.to_string()))
            .expect("updated row");
        assert_eq!(row.agent, "renamed-agent");
        assert_eq!(row.backend, "alden");
        assert_eq!(row.actions[0].action, SessionTuiAction::ResumeAttach);

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![
                selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[]),
                global_row(),
            ],
            warning: None,
        });
        let row = model
            .rows
            .iter()
            .find(|row| row.target == SelectorTarget::Agent(EDITABLE_AGENT_KEY.to_string()))
            .expect("overridden row");
        assert_eq!(row.agent, "renamed-agent");
        assert_eq!(row.backend, "alden");
        assert_eq!(row.actions[0].action, SessionTuiAction::ResumeAttach);
    }

    #[test]
    fn failed_routing_validation_does_not_apply_a_profile_change() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        let snapshot = SessionSettingsSnapshot::from_record_with_profiles(
            &record,
            &["alpha".to_string(), "beta".to_string()],
        );
        let mut draft = SessionSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                SessionSettingsField::Profile,
                Some("beta".to_string()),
            )
            .expect("stage profile");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::WorkbenchVisibility,
                Some("visible".to_string()),
            )
            .expect("stage invalid local visibility");
        let request = SessionSettingsApplyRequest {
            key: EDITABLE_AGENT_KEY.to_string(),
            draft,
            profile_names: vec!["alpha".to_string(), "beta".to_string()],
            changed_count: 2,
        };
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());

        let error = apply_session_settings_to_store(
            &mut store,
            &request,
            &["alpha".to_string(), "beta".to_string()],
        )
        .expect_err("local visibility must fail");

        assert!(error.to_string().contains("Adopt"));
        assert_eq!(store.sessions.get(EDITABLE_AGENT_KEY), Some(&record));
    }

    #[test]
    fn live_group_warning_only_reports_a_failed_post_save_patch() {
        assert_eq!(live_group_propagation_warning(Ok(None)), None);
        assert_eq!(
            live_group_propagation_warning(Ok(Some("runtime-id".to_string()))),
            None
        );
        assert_eq!(
            live_group_propagation_warning(Err(anyhow::anyhow!("agent bus rejected update"))),
            Some("Saved durable groups; live update failed: agent bus rejected update".to_string())
        );
    }

    #[test]
    fn quick_action_apply_updates_filtering_and_survives_a_stale_refresh() {
        let record = editable_record();
        let mut updated = record.clone();
        updated.quick_action = CutexSessionQuickActionMode::Auto;
        let mut model = SelectorModel::new(
            vec![
                selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[]),
                global_row(),
            ],
            true,
            false,
        );
        model.handle(SelectorEvent::OpenSettings);

        model.settings_apply_succeeded(EDITABLE_AGENT_KEY, &updated, &[], 1, false, None);
        assert!(
            !model
                .rows
                .iter()
                .find(|row| row.target == SelectorTarget::Agent(EDITABLE_AGENT_KEY.to_string()))
                .expect("updated row")
                .pinned
        );

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![
                selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[]),
                global_row(),
            ],
            warning: None,
        });
        assert!(
            !model
                .rows
                .iter()
                .find(|row| row.target == SelectorTarget::Agent(EDITABLE_AGENT_KEY.to_string()))
                .expect("overridden row")
                .pinned
        );

        model.handle(SelectorEvent::Escape);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::GlobalSettings)
        );
    }

    #[test]
    fn profile_apply_revalidates_catalog_and_keeps_stale_draft_for_retry() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        let open_catalog = vec!["alpha".to_string(), "beta".to_string()];
        let current_catalog = vec!["alpha".to_string()];
        let mut model = editable_model_with_profiles(&record, &open_catalog);
        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected apply request, got {control:?}"),
        };
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());

        let error = apply_session_settings_to_store(&mut store, &request, &current_catalog)
            .expect_err("removed profile must fail validation");
        model.settings_apply_failed(error.to_string());

        assert!(error
            .to_string()
            .contains("Profile is no longer configured: beta"));
        assert_eq!(store.sessions.get(EDITABLE_AGENT_KEY), Some(&record));
        assert_eq!(model.settings_dirty_count(), 1);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("no longer configured")));
    }

    #[test]
    fn profile_editor_with_an_empty_catalog_can_follow_the_global_default() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        let mut model = editable_model(&record);
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Down);

        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                choices,
                selected: 0,
                custom_value: Some(value),
                ..
            }) if value == "alpha"
                && choices.iter().map(|choice| choice.label.as_str()).collect::<Vec<_>>()
                    == ["Follow global default"]
        ));
        let overlay = rendered_text_at(80, 24, &model);
        assert!(overlay.contains("Current: alpha"));
        assert!(overlay.contains("Follow global default"));

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);

        assert_eq!(model.settings_dirty_count(), 1);
        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected apply request, got {control:?}"),
        };
        let updated = apply_session_settings_to_store(&mut store, &request, &[])
            .expect("clear explicit profile");
        assert_eq!(updated.profile, None);
        model.settings_apply_succeeded(
            EDITABLE_AGENT_KEY,
            &updated,
            &[],
            request.changed_count,
            false,
            None,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("Follow global default")
        );
    }

    #[test]
    fn unknown_legacy_profile_is_visible_and_preserved_until_replaced() {
        let mut record = editable_record();
        record.profile = Some("removed-profile".to_string());
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut model = editable_model_with_profiles(&record, &profile_names);
        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Down);

        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                selected: 0,
                custom_value: Some(value),
                ..
            }) if value == "removed-profile"
        ));
        assert!(rendered_text_at(80, 24, &model).contains("Current: removed-profile"));
        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 0);
    }

    #[test]
    fn late_startup_refresh_cannot_visually_rollback_a_persisted_setting() {
        let record = editable_record();
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record.clone());
        let mut model = SelectorModel::new(
            vec![selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[])],
            true,
            false,
        );

        stage_full_access(&mut model);
        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected apply request, got {control:?}"),
        };
        let updated =
            apply_session_settings_to_store(&mut store, &request, &[]).expect("apply draft");
        model.settings_apply_succeeded(
            EDITABLE_AGENT_KEY,
            &updated,
            &[],
            request.changed_count,
            false,
            None,
        );

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![selector_row(EDITABLE_AGENT_KEY, &record, &[], &[], &[])],
            warning: None,
        });

        assert!(!model.refreshing);
        assert!(model.pending_settings_refresh_override.is_none());
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("full-access")
        );
    }

    #[test]
    fn explicit_discard_restores_the_snapshot_without_an_apply_request() {
        let record = editable_record();
        let mut model = editable_model(&record);

        stage_full_access(&mut model);
        assert_eq!(
            model.handle(SelectorEvent::Insert('D')),
            SelectorControl::Continue
        );

        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Draft discarded"));
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("workspace")
        );
        assert_eq!(
            model.handle(SelectorEvent::Insert('A')),
            SelectorControl::Continue
        );
    }

    #[test]
    fn dirty_settings_default_to_keep_editing_and_require_confirmed_discard() {
        let record = editable_record();
        let mut model = editable_model(&record);

        stage_full_access(&mut model);
        model.handle(SelectorEvent::Insert('v'));
        let categorized = rendered_text_at(120, 24, &model);
        assert!(categorized.contains("1 pending"));
        assert!(categorized.contains("Permission preset *"));
        assert!(categorized.contains("full-access"));
        assert!(categorized.contains("A apply"));

        model.handle(SelectorEvent::Escape);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::ConfirmDiscard { selected: 0 })
        ));
        let confirmation = rendered_text_at(120, 24, &model);
        assert!(confirmation.contains("Unsaved changes"));
        assert!(confirmation.contains("Keep editing"));
        assert!(confirmation.contains("Discard and leave"));

        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 1);
        assert!(matches!(model.mode, SelectorMode::Settings { .. }));

        model.handle(SelectorEvent::Escape);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.mode, SelectorMode::Agents);
        assert_eq!(model.settings_dirty_count(), 0);
    }

    #[test]
    fn model_text_overlay_keeps_command_characters_as_text_until_staged() {
        let record = editable_record();
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        for _ in 0..5 {
            model.handle(SelectorEvent::Down);
        }

        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Insert('a'));
        model.handle(SelectorEvent::Insert('d'));
        model.handle(SelectorEvent::Insert('v'));
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text { input, .. }) if input.value() == "adv"
        ));
        assert_eq!(model.settings_dirty_count(), 0);

        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("adv")
        );
    }

    #[test]
    fn text_overlay_supports_clear_delete_and_keeps_invalid_name_open() {
        let record = editable_record();
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Activate);

        model.handle(SelectorEvent::ClearInput);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Session(SessionSettingsField::AgentName),
                input,
                ..
            }) if input.value().is_empty()
        ));
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Agent name cannot be empty")));
        assert_eq!(model.settings_dirty_count(), 0);

        for character in "abc".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::First);
        model.handle(SelectorEvent::Delete);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text { input, .. }) if input.value() == "bc"
        ));
        assert!(rendered_text_at(120, 24, &model).contains("Ctrl+U clear"));

        model.handle(SelectorEvent::ClearInput);
        for character in "renamed-agent".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("renamed-agent")
        );
    }

    #[test]
    fn unknown_existing_choice_is_shown_and_preserved_until_replaced() {
        let mut record = editable_record();
        record.permission_defaults = Some(":workspace".to_string());
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                selected: 0,
                custom_value: Some(value),
                ..
            }) if value == ":workspace"
        ));
        assert!(rendered_text_at(80, 24, &model).contains("Current: :workspace"));

        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some(":workspace")
        );
    }

    #[test]
    fn failed_apply_keeps_the_complete_draft_available_for_retry() {
        let record = editable_record();
        let mut model = editable_model(&record);
        stage_full_access(&mut model);

        let first = model.handle(SelectorEvent::Insert('A'));
        model.settings_apply_failed("test persistence failure".to_string());

        assert!(matches!(first, SelectorControl::ApplySettings(_)));
        assert_eq!(model.settings_dirty_count(), 1);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("test persistence failure")));
        assert!(rendered_text_at(50, 16, &model).contains("settings apply failed"));
        let retry = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplySettings(request) => request,
            control => panic!("expected retry request, got {control:?}"),
        };
        assert_eq!(retry.changed_count, 1);
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert(EDITABLE_AGENT_KEY.to_string(), record);
        let updated =
            apply_session_settings_to_store(&mut store, &retry, &[]).expect("retry apply");
        model.settings_apply_succeeded(
            EDITABLE_AGENT_KEY,
            &updated,
            &[],
            retry.changed_count,
            false,
            None,
        );
        assert!(model.warning.is_none());
        assert_eq!(model.notice.as_deref(), Some("Saved 1 setting(s)"));
    }

    #[test]
    fn global_enter_opens_settings_without_dispatch() {
        let mut model = SelectorModel::new(vec![global_row()], false, false);

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(matches!(
            model.mode,
            SelectorMode::Settings {
                target: SelectorTarget::GlobalSettings,
                category: 0,
                option: 0,
                focus: SettingsFocus::Categories,
                view: SettingsView::Categories,
            }
        ));
        model.handle(SelectorEvent::Up);
        assert_eq!(
            model.selected_setting_category_index(),
            Some(model.active_row().expect("global row").settings.len() - 1)
        );
        model.handle(SelectorEvent::Down);
        assert_eq!(model.selected_setting_category_index(), Some(0));
    }

    #[test]
    fn global_profile_defaults_are_catalog_choices_staged_in_one_apply() {
        let config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut model = SelectorModel::new(
            vec![global_settings_row_with_profiles(&config, &profile_names)],
            false,
            false,
        );

        select_global_setting(&mut model, GlobalSettingsField::DefaultProfile);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                selected: 1,
                custom_value: None,
                ..
            })
        ));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 1);

        select_global_setting(&mut model, GlobalSettingsField::DefaultProfileDirectLaunch);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Up);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 2);

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected global apply request, got {control:?}"),
        };
        assert_eq!(request.profile_names, profile_names);
        request
            .draft
            .validate_profile_catalog(&request.profile_names)
            .expect("fresh profile catalog");
        let mut updated = config.clone();
        assert!(apply_global_settings_to_config(&mut updated, &request).expect("apply defaults"));
        assert_eq!(updated.default_profile.as_deref(), Some("beta"));
        assert!(updated.default_profile_direct_launch);

        model.global_settings_apply_succeeded(
            &updated,
            &request.profile_names,
            request.changed_count,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 2 setting(s)"));
    }

    #[test]
    fn profile_default_editor_saves_the_shared_global_defaults() {
        let config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut model = SelectorModel::new(
            vec![global_settings_row_with_profiles(&config, &profile_names)],
            false,
            false,
        );
        open_profiles(
            &mut model,
            vec![
                profile_catalog_entry("alpha", true),
                profile_catalog_entry("beta", false),
            ],
        );

        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model.profile_workspace_focus(),
            Some(ProfileWorkspaceFocus::Editor)
        );
        let narrow = rendered_text_at(50, 18, &model);
        assert!(narrow.contains("Default profile"));
        assert!(narrow.contains("Direct default launch"));
        let medium = rendered_text_at(80, 24, &model);
        assert!(medium.contains("Direct default launch"));
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                selected: 1,
                custom_value: None,
                ..
            })
        ));
        assert!(rendered_text_at(100, 24, &model).contains("Default profile"));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                selected: 1,
                custom_value: None,
                ..
            })
        ));
        assert!(rendered_text_at(100, 24, &model).contains("Direct default launch"));
        model.handle(SelectorEvent::Up);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 2);

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected profile default apply request, got {control:?}"),
        };
        let mut updated = config.clone();
        assert!(apply_global_settings_to_config(&mut updated, &request).expect("apply defaults"));
        assert_eq!(updated.default_profile.as_deref(), Some("beta"));
        assert!(updated.default_profile_direct_launch);

        model.global_settings_apply_succeeded(
            &updated,
            &request.profile_names,
            request.changed_count,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 2 setting(s)"));
        for target in [SelectorTarget::Profiles, SelectorTarget::GlobalSettings] {
            let snapshot = model
                .rows
                .iter()
                .find(|row| row.target == target)
                .and_then(|row| row.global_settings_snapshot.as_ref())
                .expect("shared default snapshot");
            assert_eq!(snapshot.default_profile_name(), Some("beta"));
            assert_eq!(
                GlobalSettingsDraft::default()
                    .value(snapshot, GlobalSettingsField::DefaultProfileDirectLaunch),
                "enabled"
            );
        }
    }

    #[test]
    fn profile_manager_opens_read_only_metadata_and_wraps_selection() {
        let config = CodezConfig {
            default_profile: Some("beta".to_string()),
            ..CodezConfig::default()
        };
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut model = SelectorModel::new(
            vec![global_settings_row_with_profiles(&config, &profile_names)],
            false,
            false,
        );
        open_profiles(
            &mut model,
            vec![
                profile_catalog_entry("alpha", true),
                profile_catalog_entry("beta", false),
            ],
        );
        assert!(matches!(
            model.mode,
            SelectorMode::ProfileManager { selected: 0, .. }
        ));
        let initial = rendered_text_at(100, 24, &model);
        assert!(initial.contains("PROFILE"));
        assert!(initial.contains("Default"));
        assert!(initial.contains("Add profile"));
        model.handle(SelectorEvent::Down);
        let alpha = rendered_text_at(100, 48, &model);
        assert!(alpha.contains("alpha@example.test"));
        assert!(alpha.contains("'--model' 'gpt-test'"));
        assert!(!alpha.contains("Auth"));
        assert!(!alpha.contains("TOP-SECRET"));
        model.handle(SelectorEvent::Up);
        let narrow = rendered_text_at(50, 18, &model);
        assert!(narrow.contains("Default"));
        assert!(narrow.contains("alpha"));
        assert!(narrow.contains("beta"));

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model
                .selected_profile()
                .map(|profile| profile.name.as_str()),
            Some("beta")
        );
        let beta = rendered_text_at(100, 24, &model);
        assert!(beta.contains("beta@example.test"));
        assert!(beta.contains("launch default"));

        model.handle(SelectorEvent::Down);
        assert!(model.selected_profile_is_add());
        model.handle(SelectorEvent::Down);
        assert!(model.selected_profile_is_default());
        model.handle(SelectorEvent::Up);
        assert!(model.selected_profile_is_add());
        model.handle(SelectorEvent::Up);
        assert_eq!(
            model
                .selected_profile()
                .map(|profile| profile.name.as_str()),
            Some("beta")
        );

        assert_eq!(
            model.handle(SelectorEvent::Escape),
            SelectorControl::Continue
        );
        assert!(matches!(model.mode, SelectorMode::Agents));
        assert_eq!(model.selected_target(), Some(SelectorTarget::Profiles));
    }

    #[test]
    fn durable_profile_defaults_to_an_expanded_staged_editor() {
        let config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let mut model = SelectorModel::new(
            vec![global_settings_row_with_profiles(
                &config,
                &["alpha".to_string()],
            )],
            false,
            false,
        );
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);

        let wide = rendered_text_at(100, 48, &model);
        assert!(wide.contains("SETTING"));
        assert!(wide.contains("Identity"));
        assert!(wide.contains("  Active home"));
        assert!(wide.contains("yes"));
        assert!(wide.contains("Imported metadata"));
        assert!(wide.contains("alpha@example.test"));
        assert!(wide.contains("Model"));
        assert!(wide.contains("Provider"));
        assert!(wide.contains("Managed sessions"));
        assert!(!wide.contains("Status"));

        let medium = rendered_text_at(80, 24, &model);
        assert!(medium.contains("home+default"));
        assert!(medium.contains("Right"));
        assert!(medium.contains("Left/Tab"));

        let narrow = rendered_text_at(50, 18, &model);
        assert!(narrow.contains("SETTING"));
        assert!(narrow.contains("Active home"));
        assert!(narrow.contains("Imported metadata"));
        assert!(!narrow.contains("PROFILE  CLI"));
    }

    #[test]
    fn profile_name_editor_stages_apply_and_discard_as_typed_operations() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Profile(ProfileSettingsField::Name),
                ..
            })
        ));
        model.handle(SelectorEvent::ClearInput);
        for character in "beta".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert_eq!(model.profile_settings_draft.dirty_count(), 1);
        assert!(rendered_text_at(80, 24, &model).contains("Name *"));

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyProfileSettings(request) => request,
            other => panic!("expected typed profile apply request, got {other:?}"),
        };
        assert_eq!(request.profile_id, "id-alpha");
        assert_eq!(request.changed_count, 1);
        assert_eq!(request.patch.name.as_deref(), Some("beta"));

        assert_eq!(
            model.handle(SelectorEvent::Insert('D')),
            SelectorControl::Continue
        );
        assert!(!model.profile_settings_draft.is_dirty());
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.notice.as_deref(), Some("Draft discarded"));
    }

    #[test]
    fn optional_profile_text_editors_start_empty_instead_of_with_display_placeholders() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        let mut profile = profile_catalog_entry("alpha", true);
        profile.agent_name = None;
        open_profiles(&mut model, vec![profile]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Profile(ProfileSettingsField::AgentName),
                ref input,
                ..
            }) if input.value().is_empty()
        ));
    }

    #[test]
    fn profile_api_key_editor_masks_replacement_and_emits_a_redacted_patch() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        let mut profile = profile_catalog_entry("alpha", true);
        profile.source = Some("api-key".to_string());
        profile.api_key_configured = false;
        open_profiles(&mut model, vec![profile]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        for _ in 0..7 {
            model.handle(SelectorEvent::Down);
        }
        assert_eq!(
            model
                .selected_profile_setting_option()
                .and_then(|option| option.profile_field),
            Some(ProfileSettingsField::ApiKey)
        );

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::SecretAction {
                field: SettingsEditField::Profile(ProfileSettingsField::ApiKey),
                selected: 0,
            })
        ));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Profile(ProfileSettingsField::ApiKey),
                masked: true,
                ..
            })
        ));

        let test_key = "sk-test-tui-replacement";
        for character in test_key.chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        let rendered = rendered_text_at(80, 24, &model);
        assert!(rendered.contains("***********************"));
        assert!(!rendered.contains(test_key));
        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.profile_settings_draft.dirty_count(), 1);
        assert_eq!(
            model
                .selected_profile_setting_option()
                .map(|option| option.value),
            Some("(replace staged)".to_string())
        );

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyProfileSettings(request) => request,
            other => panic!("expected profile apply request, got {other:?}"),
        };
        assert!(matches!(
            request.patch.api_key,
            super::super::profile_settings::ProfileApiKeyUpdate::Replace(ref value)
                if value == test_key
        ));
        assert!(!format!("{request:?}").contains(test_key));
    }

    #[test]
    fn compact_profile_editor_scrolls_to_and_stages_the_deepseek_preset() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        let mut profile = profile_catalog_entry("alpha", true);
        profile.source = Some("api-key".to_string());
        open_profiles(&mut model, vec![profile]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        for _ in 0..8 {
            model.handle(SelectorEvent::Down);
        }
        assert_eq!(
            model
                .selected_profile_setting_option()
                .and_then(|option| option.profile_field),
            Some(ProfileSettingsField::DeepSeekPreset)
        );
        assert!(rendered_text_at(50, 18, &model).contains("DeepSeek preset"));

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::Choice {
                field: SettingsEditField::Profile(ProfileSettingsField::DeepSeekPreset),
                selected: 0,
                ..
            })
        ));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.profile_settings_draft.dirty_count(), 1);
        assert!(rendered_text_at(50, 18, &model).contains("staged"));

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay,
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Profile(ProfileSettingsField::Model),
                ref input,
                ..
            }) if input.value() == deepseek::DEEPSEEK_DEFAULT_MODEL
        ));
    }

    #[test]
    fn dirty_profile_requires_explicit_discard_before_browsing_or_actions() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        let snapshot = model.selected_profile_settings_snapshot().unwrap();
        model
            .profile_settings_draft
            .stage(
                &snapshot,
                ProfileSettingsField::AgentName,
                Some("builder".to_string()),
            )
            .unwrap();

        model.handle(SelectorEvent::OpenActions);
        assert!(model.profile_overlay.is_none());
        assert_eq!(
            model.warning.as_deref(),
            Some("Apply or discard staged profile settings before actions")
        );
        model.handle(SelectorEvent::Back);
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::ConfirmDiscardProfile {
                destination: ProfileDiscardDestination::ProfileList,
                selected: 0,
            })
        ));
        model.handle(SelectorEvent::Activate);
        assert!(model.profile_overlay.is_none());
        assert!(model.profile_settings_draft.is_dirty());
        assert_eq!(
            model.profile_workspace_focus(),
            Some(ProfileWorkspaceFocus::Editor)
        );

        model.handle(SelectorEvent::Back);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(!model.profile_settings_draft.is_dirty());
        assert!(model.warning.is_none());
        assert_eq!(
            model.profile_workspace_focus(),
            Some(ProfileWorkspaceFocus::Items)
        );
    }

    #[test]
    fn failed_profile_apply_keeps_the_complete_draft_for_retry() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        let snapshot = model.selected_profile_settings_snapshot().unwrap();
        model
            .profile_settings_draft
            .stage(
                &snapshot,
                ProfileSettingsField::Runtime,
                Some("docker".to_string()),
            )
            .unwrap();
        model
            .profile_settings_draft
            .stage(
                &snapshot,
                ProfileSettingsField::DockerImage,
                Some("custom-image".to_string()),
            )
            .unwrap();

        model.profile_settings_apply_failed("profile disappeared".to_string());
        assert_eq!(model.profile_settings_draft.dirty_count(), 2);
        assert_eq!(
            model.warning.as_deref(),
            Some("profile settings apply failed: profile disappeared")
        );
        let retry = model.handle(SelectorEvent::Insert('A'));
        assert!(matches!(retry, SelectorControl::ApplyProfileSettings(_)));
    }

    #[test]
    fn profile_workspace_handles_an_empty_catalog_and_shares_the_global_default_draft() {
        let config = CodezConfig::default();
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        open_profiles(&mut model, Vec::new());
        assert!(rendered_text_at(50, 18, &model).contains("Add profile"));
        assert!(model.selected_profile_is_default());
        assert_eq!(model.handle(SelectorEvent::Down), SelectorControl::Continue);
        assert!(model.selected_profile_is_add());
        model.handle(SelectorEvent::Down);
        assert!(model.selected_profile_is_default());

        let snapshot = model
            .active_global_settings_snapshot()
            .expect("shared Global snapshot")
            .clone();
        model
            .global_settings_draft
            .stage(
                &snapshot,
                GlobalSettingsField::DefaultProfileDirectLaunch,
                Some("enabled".to_string()),
            )
            .expect("stage shared default");
        let rendered = rendered_text_at(80, 24, &model);
        assert!(rendered.contains("enabled"));
        assert!(rendered.contains("1 pending"));
        assert_eq!(
            model.handle(SelectorEvent::Escape),
            SelectorControl::Continue
        );
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::ConfirmDiscardProfile {
                destination: ProfileDiscardDestination::AgentList,
                selected: 0,
            })
        ));
    }

    #[test]
    fn add_profile_handoff_defaults_to_cancel_and_returns_to_the_manager_context() {
        let config = CodezConfig::default();
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        open_profiles(&mut model, Vec::new());
        model.handle(SelectorEvent::Down);

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::ConfirmAddProfile { selected: 0 })
        ));
        let confirmation = rendered_text_at(50, 18, &model);
        assert!(confirmation.contains("Add profile"));
        assert!(confirmation.contains("terminal is restored"));
        assert!(confirmation.contains("Cancel"));
        assert!(confirmation.contains("Continue"));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.profile_overlay.is_none());

        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::LoginProfile
        );

        let success = profile_login_startup(Ok(()));
        assert_eq!(success.notice.as_deref(), Some("Profile added"));
        assert!(success.warning.is_none());
        let failure = profile_login_startup(Err(anyhow::anyhow!("cancelled")));
        assert!(failure.notice.is_none());
        assert_eq!(
            failure.warning.as_deref(),
            Some("Profile login did not complete: cancelled")
        );
        model.pending_startup_warning = failure.warning.clone();
        model.warning = failure.warning.clone();
        model.replace_snapshot(SelectorSnapshot {
            rows: model.rows.clone(),
            warning: None,
        });
        assert_eq!(model.warning, failure.warning);
        assert!(model.pending_startup_warning.is_none());
    }

    #[test]
    fn profile_actions_are_explicit_and_activate_returns_a_typed_request() {
        let config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut model = SelectorModel::new(
            vec![global_settings_row_with_profiles(&config, &profile_names)],
            false,
            false,
        );
        open_profiles(
            &mut model,
            vec![
                profile_catalog_entry("alpha", true),
                profile_catalog_entry("beta", false),
            ],
        );

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model.handle(SelectorEvent::OpenActions),
            SelectorControl::Continue
        );
        let actions = rendered_text_at(80, 24, &model);
        assert!(actions.contains("beta actions"));
        assert!(actions.contains("Make active"));
        assert!(actions.contains("Rename"));
        assert!(actions.contains("Remove"));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::ManageProfile(ProfileManagementRequest {
                profile_id: "id-beta".to_string(),
                profile_name: "beta".to_string(),
                command: ProfileManagementCommand::Activate,
            })
        );

        model.profile_overlay = None;
        model.handle(SelectorEvent::Back);
        model.handle(SelectorEvent::Up);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::OpenActions);
        let active_actions = rendered_text_at(80, 24, &model);
        assert!(!active_actions.contains("Make active"));
        assert!(active_actions.contains("Rename"));
    }

    #[test]
    fn profile_rename_requires_review_and_defaults_to_cancel() {
        let config = CodezConfig::default();
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::RenameInput { .. })
        ));
        assert!(rendered_text_at(80, 24, &model).contains("Ctrl+C exit"));
        model.handle(SelectorEvent::ClearInput);
        for character in "gamma".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::ConfirmRename { selected: 0, .. })
        ));
        let confirmation = rendered_text_at(80, 24, &model);
        assert!(confirmation.contains("Rename alpha to gamma?"));
        assert!(confirmation.contains("durable session references"));
        let narrow_confirmation = rendered_text_at(50, 18, &model);
        assert!(narrow_confirmation.contains("Confirm rename"));
        assert!(narrow_confirmation.contains("Cancel"));
        assert!(narrow_confirmation.contains("Rename"));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.profile_overlay.is_none());

        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::ClearInput);
        for character in "gamma".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        let request = ProfileManagementRequest {
            profile_id: "id-alpha".to_string(),
            profile_name: "alpha".to_string(),
            command: ProfileManagementCommand::Rename {
                new_name: "gamma".to_string(),
            },
        };
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::ManageProfile(request.clone())
        );
        model.profile_management_failed(&request, "name already exists".to_string());
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::RenameInput { ref input, .. }) if input.value() == "gamma"
        ));
    }

    #[test]
    fn profile_remove_is_confirmed_and_explains_retained_files() {
        let mut model = SelectorModel::new(
            vec![global_settings_row(&CodezConfig::default())],
            false,
            false,
        );
        open_profiles(&mut model, vec![profile_catalog_entry("alpha", true)]);

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.profile_overlay,
            Some(ProfileOverlay::ConfirmRemove { selected: 0, .. })
        ));
        let confirmation = rendered_text_at(80, 24, &model);
        assert!(confirmation.contains("Remove profile alpha?"));
        assert!(confirmation.contains("Materialized profile files are retained"));
        let narrow_confirmation = rendered_text_at(50, 18, &model);
        assert!(narrow_confirmation.contains("Confirm remove"));
        assert!(narrow_confirmation.contains("Materialized profile"));
        assert!(narrow_confirmation.contains("Cancel"));
        assert!(narrow_confirmation.contains("Remove"));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.profile_overlay.is_none());

        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::ManageProfile(ProfileManagementRequest {
                profile_id: "id-alpha".to_string(),
                profile_name: "alpha".to_string(),
                command: ProfileManagementCommand::Remove,
            })
        );
    }

    #[test]
    fn profile_mutation_refreshes_all_profile_projections_after_a_stale_snapshot() {
        let stale_config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let stale_names = vec!["alpha".to_string(), "beta".to_string()];
        let mut stale_record = editable_record();
        stale_record.profile = Some("alpha".to_string());
        let stale_row = selector_row(
            &stale_record.cutex_session_id,
            &stale_record,
            &[],
            &[],
            &stale_names,
        );
        let mut model = SelectorModel::new(
            vec![
                stale_row.clone(),
                global_settings_row_with_profiles(&stale_config, &stale_names),
            ],
            true,
            false,
        );
        open_profiles(
            &mut model,
            vec![
                profile_catalog_entry("alpha", true),
                profile_catalog_entry("beta", false),
            ],
        );

        let mut updated_record = stale_record.clone();
        updated_record.profile = Some("gamma".to_string());
        let updated_config = CodezConfig {
            default_profile: Some("gamma".to_string()),
            ..CodezConfig::default()
        };
        let mut gamma = profile_catalog_entry("gamma", true);
        gamma.id = "id-alpha".to_string();
        let projection = ProfileProjectionSnapshot {
            records: HashMap::from([(updated_record.cutex_session_id.clone(), updated_record)]),
            config: updated_config,
            profile_names: vec!["gamma".to_string(), "beta".to_string()],
        };
        model.profile_management_succeeded(ProfileManagementResult {
            profiles: vec![gamma, profile_catalog_entry("beta", false)],
            projection,
            preferred_profile_id: Some("id-alpha".to_string()),
            notice: "Renamed alpha to gamma".to_string(),
        });
        assert_eq!(
            model
                .selected_profile()
                .map(|profile| profile.name.as_str()),
            Some("gamma")
        );
        assert!(model.pending_profile_refresh_override.is_some());

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![
                stale_row,
                profiles_row(&stale_config, &stale_names),
                global_settings_row_with_profiles(&stale_config, &stale_names),
            ],
            warning: None,
        });
        let agent_profile = model
            .rows
            .iter()
            .find(|row| row.target.agent_key() == Some(EDITABLE_AGENT_KEY))
            .and_then(|row| row.settings_snapshot.as_ref())
            .and_then(|snapshot| snapshot.value(SessionSettingsField::Profile));
        let agent_profile_projection = model
            .rows
            .iter()
            .find(|row| row.target.agent_key() == Some(EDITABLE_AGENT_KEY))
            .and_then(|row| row.configured_profile.as_deref());
        let global_default = model
            .rows
            .iter()
            .find(|row| row.target.is_global_settings())
            .and_then(|row| row.global_settings_snapshot.as_ref())
            .and_then(GlobalSettingsSnapshot::default_profile_name);
        assert_eq!(agent_profile, Some("gamma"));
        assert_eq!(agent_profile_projection, Some("gamma"));
        assert_eq!(global_default, Some("gamma"));
        assert_eq!(model.notice.as_deref(), Some("Renamed alpha to gamma"));
    }

    #[test]
    fn global_choice_and_text_editors_stage_then_discard_without_an_apply_request() {
        let config = CodezConfig::default();
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        select_global_setting(&mut model, GlobalSettingsField::ManagedSessions);

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                field: SettingsEditField::Global(GlobalSettingsField::ManagedSessions),
                selected: 1,
                custom_value: None,
                ..
            })
        ));
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 1);

        select_global_setting(&mut model, GlobalSettingsField::ProxyUrl);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Global(GlobalSettingsField::ProxyUrl),
                input,
                tags: false,
                masked: false,
            }) if input.value() == "-"
        ));
        model.handle(SelectorEvent::ClearInput);
        for character in "socks5h://127.0.0.1:7890".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 2);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("socks5h://127.0.0.1:7890")
        );

        assert_eq!(
            model.handle(SelectorEvent::Insert('D')),
            SelectorControl::Continue
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Draft discarded"));
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("-")
        );
    }

    #[test]
    fn global_secret_editor_never_renders_stored_or_replacement_payloads() {
        let config = CodezConfig {
            notify_service_token: Some("stored-notify-secret".to_string()),
            desktop_notify_token: Some("stored-desktop-secret".to_string()),
            ..CodezConfig::default()
        };
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        select_global_setting(&mut model, GlobalSettingsField::NotifyServiceToken);

        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::SecretAction {
                field: SettingsEditField::Global(GlobalSettingsField::NotifyServiceToken),
                selected: 0,
            })
        ));
        let action = rendered_text_at(80, 24, &model);
        assert!(action.contains("Keep stored value"));
        assert!(!action.contains("stored-notify-secret"));

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Global(GlobalSettingsField::NotifyServiceToken),
                masked: true,
                ..
            })
        ));
        for character in "replacement-secret".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        let replacement = rendered_text_at(80, 24, &model);
        assert!(replacement.contains("******************"));
        assert!(!replacement.contains("replacement-secret"));
        assert!(!replacement.contains("stored-notify-secret"));
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 1);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("(replace staged)")
        );

        select_global_setting(&mut model, GlobalSettingsField::DesktopNotifyToken);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Last);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 2);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("(clear staged)")
        );
        let staged = rendered_text_at(100, 24, &model);
        assert!(!staged.contains("replacement-secret"));
        assert!(!staged.contains("stored-desktop-secret"));

        model.handle(SelectorEvent::Insert('D'));
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Draft discarded"));
    }

    #[test]
    fn global_notification_editors_validate_and_apply_one_config_patch() {
        let config = CodezConfig {
            notify_service_token: Some("old-secret".to_string()),
            notify_service_user_message_content: Some("legacy-mode".to_string()),
            agent_bus_token: Some("preserved-bus-secret".to_string()),
            ..CodezConfig::default()
        };
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);

        select_global_setting(&mut model, GlobalSettingsField::NotifyMessageContent);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Choice {
                custom_value: Some(value),
                selected: 0,
                ..
            }) if value == "legacy-mode"
        ));
        model.handle(SelectorEvent::Activate);
        assert!(model.settings_overlay.is_none());
        assert_eq!(model.settings_dirty_count(), 0);
        assert!(model.warning.is_none());

        select_global_setting(&mut model, GlobalSettingsField::NotifyIdleTimeout);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::ClearInput);
        for character in "invalid".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Global(GlobalSettingsField::NotifyIdleTimeout),
                ..
            })
        ));
        assert_eq!(model.settings_dirty_count(), 0);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Unsupported integer value")));
        let invalid = rendered_text_at(100, 24, &model);
        assert!(invalid.contains("Unsupported integer value: invalid"));
        assert!(invalid.contains("Ctrl+C exit"));
        model.handle(SelectorEvent::ClearInput);
        model.handle(SelectorEvent::Insert('9'));
        model.handle(SelectorEvent::Insert('0'));
        model.handle(SelectorEvent::Activate);

        select_global_setting(&mut model, GlobalSettingsField::NotifyEvents);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text {
                field: SettingsEditField::Global(GlobalSettingsField::NotifyEvents),
                tags: true,
                masked: false,
                ..
            })
        ));
        model.handle(SelectorEvent::ClearInput);
        for character in "turn-completed approval_requested".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        assert!(rendered_text_at(100, 24, &model).contains("[turn-completed]"));
        model.handle(SelectorEvent::Activate);

        let snapshot = model
            .active_global_settings_snapshot()
            .expect("global snapshot")
            .clone();
        model
            .global_settings_draft
            .stage(
                &snapshot,
                GlobalSettingsField::NotifyMessageContent,
                Some("preview".to_string()),
            )
            .expect("stage message mode");
        model
            .global_settings_draft
            .stage(
                &snapshot,
                GlobalSettingsField::DesktopNotifyPort,
                Some("24251".to_string()),
            )
            .expect("stage desktop port");
        model
            .global_settings_draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::NotifyServiceToken,
                SecretSettingsAction::Replace("new-secret".to_string()),
            )
            .expect("stage secret");
        model.reproject_settings(&SelectorTarget::GlobalSettings);

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected global apply request, got {control:?}"),
        };
        assert_eq!(request.changed_count, 5);
        let mut updated = config.clone();
        assert!(apply_global_settings_to_config(&mut updated, &request).expect("apply global"));
        assert_eq!(updated.notify_service_idle_timeout_secs, Some(90));
        assert_eq!(
            updated.notify_service_events,
            Some(vec![
                "turn_completed".to_string(),
                "approval_requested".to_string()
            ])
        );
        assert_eq!(
            updated.notify_service_user_message_content.as_deref(),
            Some("preview")
        );
        assert_eq!(updated.desktop_notify_port, Some(24251));
        assert_eq!(updated.notify_service_token.as_deref(), Some("new-secret"));
        assert_eq!(
            updated.agent_bus_token.as_deref(),
            Some("preserved-bus-secret")
        );
        model.global_settings_apply_succeeded(
            &updated,
            &request.profile_names,
            request.changed_count,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 5 setting(s)"));
        assert!(!rendered_text_at(100, 24, &model).contains("new-secret"));
    }

    #[test]
    fn global_agent_bus_editors_apply_config_without_trimming_prefix_or_dispatching() {
        let mut config = CodezConfig::default();
        config.agent_bus_enabled = false;
        config.agent_bus_token = Some("stored-bus-secret".to_string());
        config.agent_message_suffix_template = Some("old suffix".to_string());
        config.notify_service_token = Some("preserved-notify-secret".to_string());
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);

        select_global_setting(&mut model, GlobalSettingsField::AgentMessagePrefix);
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.settings_overlay.as_ref(),
            Some(SettingsOverlay::Text { input, .. })
                if input.value() == "[message from {from}] "
        ));
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 0);

        select_global_setting(&mut model, GlobalSettingsField::AgentBusPort);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::ClearInput);
        for character in "59995".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Bridgeboard 24xxx")));
        assert_eq!(model.settings_dirty_count(), 0);
        model.handle(SelectorEvent::ClearInput);
        for character in "24261".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);

        select_global_setting(&mut model, GlobalSettingsField::AgentBusEnabled);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Up);
        model.handle(SelectorEvent::Activate);

        select_global_setting(&mut model, GlobalSettingsField::AgentBusToken);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        for character in "new-bus-secret".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        assert!(!rendered_text_at(100, 24, &model).contains("new-bus-secret"));
        model.handle(SelectorEvent::Activate);

        select_global_setting(&mut model, GlobalSettingsField::AgentMessagePrefix);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::ClearInput);
        for character in "<{from}> ".chars() {
            model.handle(SelectorEvent::Insert(character));
        }
        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model
                .active_setting_option()
                .map(|option| option.value.as_str()),
            Some("<{from}> ")
        );

        select_global_setting(&mut model, GlobalSettingsField::AgentMessageSuffix);
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::ClearInput);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.settings_dirty_count(), 5);

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected Global apply request, got {control:?}"),
        };
        let mut updated = config.clone();
        assert!(apply_global_settings_to_config(&mut updated, &request).expect("apply Agent Bus"));
        assert!(updated.agent_bus_enabled);
        assert_eq!(updated.agent_bus_port, Some(24261));
        assert_eq!(updated.agent_bus_token.as_deref(), Some("new-bus-secret"));
        assert_eq!(
            updated.agent_message_prefix_template.as_deref(),
            Some("<{from}> ")
        );
        assert_eq!(updated.agent_message_suffix_template, None);
        assert_eq!(
            updated.notify_service_token.as_deref(),
            Some("preserved-notify-secret")
        );
        model.global_settings_apply_succeeded(
            &updated,
            &request.profile_names,
            request.changed_count,
        );
        assert_eq!(model.notice.as_deref(), Some("Saved 5 setting(s)"));
        assert!(!rendered_text_at(100, 24, &model).contains("new-bus-secret"));
    }

    #[test]
    fn global_general_proxy_apply_updates_config_and_survives_a_stale_refresh() {
        let mut config = CodezConfig::default();
        config.notify_service_user_message_content = Some("legacy-mode".to_string());
        config.agent_bus_token = Some("preserved-secret".to_string());
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], true, false);
        model.handle(SelectorEvent::Activate);
        let snapshot = model
            .active_global_settings_snapshot()
            .expect("global snapshot")
            .clone();
        for (field, value) in [
            (GlobalSettingsField::ManagedSessions, "enabled"),
            (GlobalSettingsField::DockerSudo, "enabled"),
            (GlobalSettingsField::ProxyEnabled, "enabled"),
            (GlobalSettingsField::ProxyUrl, "socks5h://127.0.0.1:7890"),
            (GlobalSettingsField::ProxyNoProxy, "localhost"),
            (GlobalSettingsField::ProxyForceHttp, "disabled"),
        ] {
            model
                .global_settings_draft
                .stage(&snapshot, field, Some(value.to_string()))
                .expect("stage global field");
        }
        model.reproject_settings(&SelectorTarget::GlobalSettings);

        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected global apply request, got {control:?}"),
        };
        assert_eq!(request.changed_count, 6);
        let mut updated = config.clone();
        assert!(apply_global_settings_to_config(&mut updated, &request).expect("apply global"));
        assert!(updated.session.enabled);
        assert!(updated.docker_use_sudo);
        let proxy = updated.proxy.as_ref().expect("enabled proxy");
        assert_eq!(proxy.url.as_deref(), Some("socks5h://127.0.0.1:7890"));
        assert_eq!(proxy.no_proxy.as_deref(), Some("localhost"));
        assert!(!proxy.force_http_transport);
        assert_eq!(
            updated.notify_service_user_message_content.as_deref(),
            Some("legacy-mode")
        );
        assert_eq!(updated.agent_bus_token.as_deref(), Some("preserved-secret"));

        model.global_settings_apply_succeeded(
            &updated,
            &request.profile_names,
            request.changed_count,
        );
        assert_eq!(model.settings_dirty_count(), 0);
        assert_eq!(model.notice.as_deref(), Some("Saved 6 setting(s)"));
        assert!(model
            .active_row()
            .expect("updated global row")
            .settings
            .iter()
            .any(|category| category.options.iter().any(|option| {
                option.global_field == Some(GlobalSettingsField::ProxyUrl)
                    && option.value == "socks5h://127.0.0.1:7890"
            })));

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![global_settings_row(&config)],
            warning: None,
        });
        assert!(model
            .active_row()
            .expect("overridden global row")
            .settings
            .iter()
            .any(|category| category.options.iter().any(|option| {
                option.global_field == Some(GlobalSettingsField::ManagedSessions)
                    && option.value == "enabled"
            })));
    }

    #[test]
    fn failed_global_proxy_apply_keeps_the_complete_draft_for_retry() {
        let config = CodezConfig::default();
        let mut model = SelectorModel::new(vec![global_settings_row(&config)], false, false);
        model.handle(SelectorEvent::Activate);
        let snapshot = model
            .active_global_settings_snapshot()
            .expect("global snapshot")
            .clone();
        model
            .global_settings_draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyEnabled,
                Some("enabled".to_string()),
            )
            .expect("stage enabled");
        model
            .global_settings_draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyUrl,
                Some("ftp://127.0.0.1:21".to_string()),
            )
            .expect("stage invalid URL");
        model.reproject_settings(&SelectorTarget::GlobalSettings);
        let request = match model.handle(SelectorEvent::Insert('A')) {
            SelectorControl::ApplyGlobalSettings(request) => request,
            control => panic!("expected global apply request, got {control:?}"),
        };
        let mut unchanged = config.clone();
        let error =
            apply_global_settings_to_config(&mut unchanged, &request).expect_err("invalid scheme");
        model.global_settings_apply_failed(error.to_string());

        assert!(error.to_string().contains("Unsupported proxy scheme"));
        assert_eq!(model.settings_dirty_count(), 2);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("Unsupported proxy scheme")));
        assert_eq!(unchanged.proxy, None);
        assert!(!unchanged.docker_use_sudo);
    }

    #[test]
    fn view_key_remains_a_filter_character_on_the_agent_list() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "view-agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );

        model.handle(SelectorEvent::Insert('v'));

        assert_eq!(model.query.value(), "v");
        assert_eq!(model.mode, SelectorMode::Agents);
    }

    #[test]
    fn escape_clears_query_before_exiting() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        model.handle(SelectorEvent::Insert('a'));

        assert_eq!(
            model.handle(SelectorEvent::Escape),
            SelectorControl::Continue
        );
        assert!(model.query.value().is_empty());
        assert_eq!(model.handle(SelectorEvent::Escape), SelectorControl::Exit);
    }

    #[test]
    fn action_menu_opens_with_fallback_and_left_returns_to_agents() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );

        assert_eq!(
            model.handle(SelectorEvent::OpenActions),
            SelectorControl::Continue
        );
        assert_eq!(
            model.mode,
            SelectorMode::Actions {
                agent_key: "agent".to_string(),
                selected: 0,
                launch_profile: None,
            }
        );

        model.handle(SelectorEvent::Up);
        assert_eq!(model.selected_action_index(), Some(3));
        model.handle(SelectorEvent::Down);
        assert_eq!(model.selected_action_index(), Some(0));
        model.handle(SelectorEvent::Down);
        assert_eq!(model.selected_action_index(), Some(1));
        model.handle(SelectorEvent::Back);
        assert_eq!(model.mode, SelectorMode::Agents);
    }

    #[test]
    fn primary_enter_returns_a_typed_intent() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Selected(SessionTuiIntent {
                key: "agent".to_string(),
                action: SessionTuiAction::ResumeAttach,
                launch_profile: None,
            })
        );
    }

    #[test]
    fn direct_close_shortcut_returns_a_close_intent_without_confirmation() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );

        assert_eq!(
            model.activate_close_shortcut(),
            SelectorControl::Selected(SessionTuiIntent {
                key: "agent".to_string(),
                action: SessionTuiAction::CloseRuntime,
                launch_profile: None,
            })
        );
        assert_eq!(model.mode, SelectorMode::Agents);
    }

    #[test]
    fn runtime_close_progress_stays_in_selector_and_blocks_exit_input() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        let intent = match model.activate_close_shortcut() {
            SelectorControl::Selected(intent) => intent,
            control => panic!("expected close intent, got {control:?}"),
        };

        model.runtime_close_started(&intent);

        assert_eq!(
            model.mode,
            SelectorMode::ClosingRuntime {
                agent_key: "agent".to_string(),
                agent_name: "agent".to_string(),
                action: SessionTuiAction::CloseRuntime,
            }
        );
        assert_eq!(model.handle(SelectorEvent::Exit), SelectorControl::Continue);
        let rendered = rendered_text_at(80, 16, &model);
        assert!(rendered.contains("Closing runtime for agent..."));
        assert!(rendered.contains("Waiting for closed or offline status."));
        assert!(!rendered.contains("Ctrl+C exit"));
    }

    #[test]
    fn retire_is_final_non_primary_action_and_defaults_to_cancel() {
        let mut record = editable_record();
        record.registration_class = AgentRegistrationClass::Persistent;
        record.profile = Some("alpha".to_string());
        record.managed_cwd = Some("/tmp/editable-managed".to_string());
        let mut model = editable_model(&record);

        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Last);
        model.handle(SelectorEvent::Activate);

        assert!(matches!(
            model.mode,
            SelectorMode::ConfirmRuntimeAction {
                action: SessionTuiAction::RetireSession,
                confirmed: false,
                ..
            }
        ));
        let rendered = rendered_text_at(100, 20, &model);
        assert!(rendered.contains("Retire managed session editable-agent?"));
        assert!(rendered.contains("Profile: alpha"));
        assert!(rendered.contains("Managed path: /tmp/editable-managed"));

        model.handle(SelectorEvent::Activate);
        assert!(matches!(model.mode, SelectorMode::Actions { .. }));
    }

    #[test]
    fn retired_workspace_lists_archive_columns_and_restore_defaults_to_cancel() {
        let mut record = editable_record();
        record.archive_state = cutex::session::model::CutexSessionArchiveState::Retired;
        record.retired_at = Some("2026-08-14T00:01:00Z".to_string());
        record.revision = 7;
        record.registration_class = AgentRegistrationClass::Persistent;
        let mut store = CutexSessionStore::default();
        store.sessions.insert("archive-key".to_string(), record);
        let mut model = SelectorModel::new(vec![retired_sessions_row(1)], false, false);
        model.open_retired_sessions(retired_selector_rows_from_store(&store));

        let rendered = rendered_text_at(100, 20, &model);
        for column in ["AGENT", "PROFILE", "MANAGED PATH", "RETIRED AT", "REVISION"] {
            assert!(rendered.contains(column));
        }
        assert!(rendered_text_at(80, 24, &model).contains("Retired sessions"));
        assert!(rendered_text_at(52, 16, &model).contains("Retired sessions"));
        model.handle(SelectorEvent::Activate);
        assert!(matches!(
            model.mode,
            SelectorMode::ConfirmRuntimeAction {
                action: SessionTuiAction::RestoreSession,
                confirmed: false,
                ..
            }
        ));
        model.handle(SelectorEvent::Activate);
        assert!(matches!(model.mode, SelectorMode::RetiredSessions { .. }));
    }

    #[test]
    fn completed_runtime_close_refreshes_in_place_and_retains_the_offline_row() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        model.handle(SelectorEvent::Insert('a'));
        let intent = match model.activate_close_shortcut() {
            SelectorControl::Selected(intent) => intent,
            control => panic!("expected close intent, got {control:?}"),
        };
        model.runtime_close_started(&intent);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RuntimeCloseWorkerResult::Closed(SelectorSnapshot {
                rows: vec![row(
                    "agent",
                    "agent",
                    CutexSessionLifecycleState::Offline,
                    false,
                    true,
                )],
                warning: None,
            }))
            .expect("close result");
        let mut runtime_close = Some(receiver);

        assert!(receive_runtime_close(&mut model, &mut runtime_close));

        assert!(runtime_close.is_none());
        assert_eq!(model.mode, SelectorMode::Agents);
        assert_eq!(model.query.value(), "a");
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("agent".to_string()))
        );
        assert_eq!(model.visible_rows().len(), 1);
        assert_eq!(
            model.selected_row().and_then(|row| row.lifecycle),
            Some(CutexSessionLifecycleState::Offline)
        );
        assert_eq!(model.notice.as_deref(), Some("Runtime closed: agent"));
        model.query.reset();
        model.ensure_selection();
        assert_eq!(model.visible_rows().len(), 1);
    }

    #[test]
    fn terminal_screen_is_invalidated_only_after_runtime_close_completes() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        let (_sender, receiver) = mpsc::channel();
        let mut runtime_close = Some(receiver);

        assert!(!receive_runtime_close(&mut model, &mut runtime_close));
        assert!(runtime_close.is_some());
    }

    #[test]
    fn runtime_close_failure_returns_to_the_list_with_an_inline_error() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        let intent = match model.activate_close_shortcut() {
            SelectorControl::Selected(intent) => intent,
            control => panic!("expected close intent, got {control:?}"),
        };
        model.runtime_close_started(&intent);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RuntimeCloseWorkerResult::Failed(
                "management timeout".to_string(),
            ))
            .expect("close failure");
        let mut runtime_close = Some(receiver);

        assert!(receive_runtime_close(&mut model, &mut runtime_close));

        assert_eq!(model.mode, SelectorMode::Agents);
        assert_eq!(
            model.selected_row().and_then(|row| row.lifecycle),
            Some(CutexSessionLifecycleState::Online)
        );
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("management timeout")));
    }

    #[test]
    fn closed_runtime_with_failed_refresh_is_shown_offline_without_stale_actions() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        let intent = match model.activate_close_shortcut() {
            SelectorControl::Selected(intent) => intent,
            control => panic!("expected close intent, got {control:?}"),
        };
        model.runtime_close_started(&intent);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(RuntimeCloseWorkerResult::ClosedRefreshFailed(
                "store unavailable".to_string(),
            ))
            .expect("refresh failure");
        let mut runtime_close = Some(receiver);

        assert!(receive_runtime_close(&mut model, &mut runtime_close));

        assert!(runtime_close.is_none());
        let row = model.selected_row().expect("closed row remains selected");
        assert_eq!(row.lifecycle, Some(CutexSessionLifecycleState::Offline));
        assert!(row.actions.is_empty());
        assert_eq!(model.visible_rows().len(), 1);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("live refresh failed")));
    }

    #[test]
    fn only_close_runtime_is_dispatched_inside_the_selector() {
        let mut intent = SessionTuiIntent {
            key: "agent".to_string(),
            action: SessionTuiAction::CloseRuntime,
            launch_profile: None,
        };
        assert!(intent_runs_in_selector(&intent));
        intent.action = SessionTuiAction::CloseAndRestart;
        assert!(!intent_runs_in_selector(&intent));
    }

    #[test]
    fn direct_close_shortcut_rejects_rows_without_a_runtime() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Offline,
                true,
                true,
            )],
            false,
            false,
        );

        assert_eq!(model.activate_close_shortcut(), SelectorControl::Continue);
        assert!(model
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("No runtime is available to close")));
    }

    #[test]
    fn close_requires_explicit_confirmation_and_defaults_to_cancel() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "agent",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        model.handle(SelectorEvent::OpenActions);
        model.handle(SelectorEvent::Last);
        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model.mode,
            SelectorMode::ConfirmRuntimeAction {
                agent_key: "agent".to_string(),
                action: SessionTuiAction::CloseRuntime,
                launch_profile: None,
                confirmed: false,
            }
        );

        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert_eq!(
            model.mode,
            SelectorMode::Actions {
                agent_key: "agent".to_string(),
                selected: 3,
                launch_profile: None,
            }
        );

        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::OpenActions);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Selected(SessionTuiIntent {
                key: "agent".to_string(),
                action: SessionTuiAction::CloseRuntime,
                launch_profile: None,
            })
        );
    }

    #[test]
    fn close_and_restart_confirms_and_keeps_the_selected_launch_profile() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        record.registration_class = AgentRegistrationClass::Persistent;
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.alden_session_name = Some("cutex.editable.runtime".to_string());
        record.alden_pid = Some(std::process::id());
        let alden_sessions = vec![CuteAldenSession {
            pid: std::process::id(),
            name: record.alden_session_name.clone(),
        }];
        let mut model = SelectorModel::new(
            vec![selector_row(
                EDITABLE_AGENT_KEY,
                &record,
                &alden_sessions,
                &[],
                &["alpha".to_string(), "beta".to_string()],
            )],
            false,
            false,
        );

        model.handle(SelectorEvent::OpenActions);
        assert_eq!(model.selected_action_index(), Some(0));
        model.handle(SelectorEvent::Activate);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Activate);
        assert_eq!(model.selected_launch_profile(), Some("beta"));

        let restart_index = model
            .active_row()
            .and_then(|row| row.control_index_for_action(SessionTuiAction::CloseAndRestart))
            .expect("restart action index");
        while model.selected_action_index() != Some(restart_index) {
            model.handle(SelectorEvent::Down);
        }
        model.handle(SelectorEvent::Activate);
        assert_eq!(
            model.mode,
            SelectorMode::ConfirmRuntimeAction {
                agent_key: EDITABLE_AGENT_KEY.to_string(),
                action: SessionTuiAction::CloseAndRestart,
                launch_profile: Some("beta".to_string()),
                confirmed: false,
            }
        );
        let confirmation = rendered_text_at(80, 24, &model);
        assert!(confirmation.contains("Confirm restart"));
        assert!(confirmation.contains("Close and restart runtime for editable-agent?"));
        assert!(confirmation.contains("beta (this launch only)"));

        model.handle(SelectorEvent::OpenActions);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Selected(SessionTuiIntent {
                key: EDITABLE_AGENT_KEY.to_string(),
                action: SessionTuiAction::CloseAndRestart,
                launch_profile: Some("beta".to_string()),
            })
        );
    }

    #[test]
    fn restart_menu_resolves_an_inherited_global_profile_name() {
        let mut record = editable_record();
        record.profile = None;
        record.registration_class = AgentRegistrationClass::Persistent;
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.alden_session_name = Some("cutex.editable.runtime".to_string());
        record.alden_pid = Some(std::process::id());
        let alden_sessions = vec![CuteAldenSession {
            pid: std::process::id(),
            name: record.alden_session_name.clone(),
        }];
        let config = CodezConfig {
            default_profile: Some("colab".to_string()),
            ..CodezConfig::default()
        };
        let mut model = SelectorModel::new(
            vec![
                selector_row(
                    EDITABLE_AGENT_KEY,
                    &record,
                    &alden_sessions,
                    &[],
                    &["colab".to_string()],
                ),
                global_settings_row(&config),
            ],
            false,
            false,
        );

        model.handle(SelectorEvent::OpenActions);
        assert!(rendered_text_at(80, 24, &model).contains("Session default: colab (global)"));
        model.handle(SelectorEvent::Activate);
        assert!(rendered_text_at(80, 24, &model).contains("Session default (global: colab)"));
    }

    #[test]
    fn launch_profile_control_stages_without_dispatch_then_enriches_action_intent() {
        let mut record = editable_record();
        record.profile = Some("alpha".to_string());
        record.registration_class = AgentRegistrationClass::Persistent;
        let mut model =
            editable_model_with_profiles(&record, &["alpha".to_string(), "beta".to_string()]);

        assert_eq!(
            model.handle(SelectorEvent::OpenActions),
            SelectorControl::Continue
        );
        assert_eq!(model.selected_action_index(), Some(0));
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(matches!(
            model.action_overlay,
            Some(ActionOverlay::LaunchProfile { selected: 0, .. })
        ));

        model.handle(SelectorEvent::Down);
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Continue
        );
        assert!(model.action_overlay.is_none());
        assert_eq!(model.selected_launch_profile(), Some("beta"));
        assert!(rendered_text_at(80, 24, &model).contains("beta (this launch only)"));

        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Selected(SessionTuiIntent {
                key: EDITABLE_AGENT_KEY.to_string(),
                action: SessionTuiAction::Online,
                launch_profile: Some("beta".to_string()),
            })
        );
    }

    #[test]
    fn root_primary_and_live_takeover_do_not_inherit_the_restart_profile() {
        let mut offline = editable_record();
        offline.profile = Some("alpha".to_string());
        offline.registration_class = AgentRegistrationClass::Persistent;
        let mut model = editable_model_with_profiles(&offline, &["beta".to_string()]);
        assert!(matches!(
            model.handle(SelectorEvent::Activate),
            SelectorControl::Selected(SessionTuiIntent {
                launch_profile: None,
                ..
            })
        ));

        let mut live = offline;
        live.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        live.alden_session_name = Some("cutex.editable.runtime".to_string());
        live.alden_pid = Some(std::process::id());
        let live_session = CuteAldenSession {
            pid: std::process::id(),
            name: live.alden_session_name.clone(),
        };
        let row = selector_row(
            EDITABLE_AGENT_KEY,
            &live,
            &[live_session],
            &[],
            &["alpha".to_string(), "beta".to_string()],
        );
        assert!(row.launch_profile_control_available());
        assert!(!row.action_supports_launch_profile(SessionTuiAction::ResumeAttach));
        assert!(row.action_supports_launch_profile(SessionTuiAction::CloseAndRestart));
    }

    #[test]
    fn backspace_removes_one_unicode_grapheme() {
        let mut model = SelectorModel::new(Vec::new(), false, false);
        model.handle(SelectorEvent::Insert('e'));
        model.handle(SelectorEvent::Insert('\u{301}'));
        assert_eq!(model.query.value(), "e\u{301}");

        model.handle(SelectorEvent::Backspace);

        assert!(model.query.value().is_empty());
    }

    #[test]
    fn selection_survives_snapshot_refresh_by_durable_key() {
        let mut model = SelectorModel::new(
            vec![
                row(
                    "alpha",
                    "alpha",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                row(
                    "beta",
                    "beta",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
            ],
            true,
            false,
        );
        model.handle(SelectorEvent::Down);
        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("beta".to_string()))
        );

        model.replace_snapshot(SelectorSnapshot {
            rows: vec![
                row(
                    "beta",
                    "beta-renamed",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
                row(
                    "alpha",
                    "alpha",
                    CutexSessionLifecycleState::Online,
                    false,
                    true,
                ),
            ],
            warning: None,
        });

        assert_eq!(
            model.selected_target(),
            Some(SelectorTarget::Agent("beta".to_string()))
        );
        assert!(!model.refreshing);
    }

    #[test]
    fn responsive_layouts_keep_agent_first_and_show_managed_path() {
        let mut activity_row = row(
            "agent",
            "cutex-dev-v5",
            CutexSessionLifecycleState::Online,
            false,
            true,
        );
        activity_row.last_output_at =
            Some((Utc::now() - chrono::Duration::minutes(10)).to_rfc3339());
        let model = SelectorModel::new(vec![activity_row], false, false);

        let extra_wide = rendered_text(150, &model);
        let agent = extra_wide.find("AGENT").expect("agent heading");
        let profile = extra_wide.find("PROFILE").expect("profile heading");
        let state = extra_wide.find("STATE").expect("state heading");
        let last_output = extra_wide.find("LAST OUTPUT").expect("last output heading");
        let host = extra_wide.find("HOST").expect("host heading");
        let backend = extra_wide.find("BACKEND").expect("backend heading");
        let managed_path = extra_wide
            .find("MANAGED PATH")
            .expect("managed path heading");
        let primary = extra_wide.find("PRIMARY ACTION").expect("primary heading");
        assert!(
            agent < profile
                && profile < state
                && state < last_output
                && last_output < host
                && host < backend
                && backend < managed_path
                && managed_path < primary
        );
        assert!(extra_wide.contains("cutex-dev-v5"));
        assert!(extra_wide.contains("~/Projects/cutex"));
        assert!(extra_wide.contains("aemeath"));
        assert!(extra_wide.contains("10m ago"));
        assert!(extra_wide.contains("takeover"));

        let wide = rendered_text(120, &model);
        let agent = wide.find("AGENT").expect("agent heading");
        let profile = wide.find("PROFILE").expect("profile heading");
        let state = wide.find("STATE").expect("state heading");
        let last_output = wide.find("LAST OUTPUT").expect("last output heading");
        let managed_path = wide.find("MANAGED PATH").expect("managed path heading");
        let primary = wide.find("PRIMARY ACTION").expect("primary heading");
        assert!(
            agent < profile
                && profile < state
                && state < last_output
                && last_output < managed_path
                && managed_path < primary
        );
        assert!(!wide.contains("HOST"));
        assert!(!wide.contains("BACKEND"));
        assert!(wide.contains("~/Projects/cutex"));

        let minimum_wide = rendered_text(WIDE_LAYOUT_MIN_WIDTH, &model);
        assert!(minimum_wide.contains("LAST OUTPUT"));
        assert!(minimum_wide.contains("MANAGED PATH"));
        assert!(minimum_wide.contains("PRIMARY ACTION"));
        assert!(minimum_wide.contains("takeover"));

        let narrow = rendered_text(72, &model);
        let agent = narrow.find("AGENT").expect("agent heading");
        let profile = narrow.find("PROFILE").expect("profile heading");
        let state = narrow.find("STATE").expect("state heading");
        let primary = narrow.find("PRIMARY").expect("primary heading");
        assert!(agent < profile && profile < state && state < primary);
        assert!(!narrow.contains("HOST"));
        assert!(!narrow.contains("BACKEND"));
        assert!(!narrow.contains("LAST OUTPUT"));
        assert!(!narrow.contains("MANAGED PATH"));
        assert!(narrow.contains("aemeath"));
        assert!(narrow.contains("Ctrl+X close"));
        assert!(narrow.contains("Ctrl+C exit"));
    }

    #[test]
    fn last_output_time_uses_compact_stable_labels() {
        let now = DateTime::parse_from_rfc3339("2026-08-13T12:00:00Z")
            .expect("test timestamp")
            .with_timezone(&Utc);

        assert_eq!(format_last_output_at(None, now), "-");
        assert_eq!(
            format_last_output_at(Some("2026-08-13T11:59:58Z"), now),
            "now"
        );
        assert_eq!(
            format_last_output_at(Some("2026-08-13T11:59:18Z"), now),
            "42s ago"
        );
        assert_eq!(
            format_last_output_at(Some("2026-08-13T11:52:00Z"), now),
            "8m ago"
        );
        assert_eq!(
            format_last_output_at(Some("2026-08-13T08:00:00Z"), now),
            "4h ago"
        );
        assert_eq!(
            format_last_output_at(Some("2026-08-10T12:00:00Z"), now),
            "3d ago"
        );
        assert_eq!(
            format_last_output_at(Some("2026-08-01T12:00:00Z"), now),
            "2026-08-01"
        );
        assert_eq!(format_last_output_at(Some("invalid"), now), "-");
    }

    #[test]
    fn selector_rows_join_activity_by_durable_session_id() {
        let record = editable_record();
        let mut store = CutexSessionStore::default();
        store
            .sessions
            .insert("store-key".to_string(), record.clone());
        let mut initial_activity = SessionActivityState::default();
        initial_activity.revision = 1;
        initial_activity.last_output_at = Some("2026-08-13T06:00:00Z".to_string());
        let activity_states = HashMap::from([(record.cutex_session_id.clone(), initial_activity)]);

        let rows = selector_rows_from_store(
            &store,
            &[],
            &[],
            &CodezConfig::default(),
            &[],
            &activity_states,
        );

        assert_eq!(
            rows[0].target,
            SelectorTarget::Agent("store-key".to_string())
        );
        assert_eq!(
            rows[0].last_output_at.as_deref(),
            Some("2026-08-13T06:00:00Z")
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.target.clone())
                .collect::<Vec<_>>(),
            vec![
                SelectorTarget::Agent("store-key".to_string()),
                SelectorTarget::RetiredSessions,
                SelectorTarget::RecentSessions,
                SelectorTarget::CutexProjects,
                SelectorTarget::Projects,
                SelectorTarget::Profiles,
                SelectorTarget::GlobalSettings,
            ]
        );
        assert!(rows[1..].iter().all(|row| row.last_output_at.is_none()));

        let mut model = SelectorModel::new(rows, false, false);
        let mut refreshed_activity = SessionActivityState::default();
        refreshed_activity.revision = 2;
        refreshed_activity.last_output_at = Some("2026-08-13T06:00:01Z".to_string());
        model.refresh_activity_states(&HashMap::from([(
            record.cutex_session_id.clone(),
            refreshed_activity,
        )]));
        assert_eq!(
            model.rows[0].last_output_at.as_deref(),
            Some("2026-08-13T06:00:01Z")
        );
        assert!(model.rows[1..]
            .iter()
            .all(|row| row.last_output_at.is_none()));

        model.refresh_activity_states(&HashMap::new());
        assert!(model.rows[0].last_output_at.is_none());
    }

    #[test]
    fn agent_profile_column_distinguishes_persistent_override_from_global_default() {
        let mut explicit = row(
            "deepseek",
            "deepseek-agent",
            CutexSessionLifecycleState::Online,
            false,
            true,
        );
        explicit.configured_profile = Some("deepseek".to_string());
        let mut inherited = row(
            "inherited",
            "default-agent",
            CutexSessionLifecycleState::Online,
            false,
            true,
        );
        inherited.configured_profile = None;
        let config = CodezConfig {
            default_profile: Some("colab".to_string()),
            ..CodezConfig::default()
        };
        let mut model = SelectorModel::new(
            vec![explicit, inherited, global_settings_row(&config)],
            false,
            false,
        );

        for width in [120, 72] {
            let rendered = rendered_text(width, &model);
            assert!(rendered.contains("deepseek"));
            assert!(rendered.contains("Default (colab)"));
        }

        let updated_config = CodezConfig {
            default_profile: Some("deepseek".to_string()),
            ..CodezConfig::default()
        };
        model.global_settings_apply_succeeded(&updated_config, &[], 1);
        assert!(rendered_text(120, &model).contains("Default (deepseek)"));
    }

    #[test]
    fn action_and_confirmation_views_render_current_agent_context() {
        let mut model = SelectorModel::new(
            vec![row(
                "agent",
                "cutex-dev-v5",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            true,
        );
        model.handle(SelectorEvent::OpenActions);

        let actions = rendered_text(120, &model);
        assert!(actions.contains("cutex actions"));
        assert!(actions.contains("ACTION"));
        assert!(actions.contains("DETAILS"));
        assert!(actions.contains("takeover  primary"));
        assert!(actions.contains("close and restart"));
        assert!(actions.contains("close runtime"));

        model.handle(SelectorEvent::Last);
        model.handle(SelectorEvent::Activate);
        let confirmation = rendered_text(72, &model);
        assert!(confirmation.contains("Confirm close"));
        assert!(confirmation.contains("Close runtime for cutex-dev-v5?"));
        assert!(confirmation.contains("session and cute-codex history are kept"));
    }

    #[test]
    fn settings_browser_renders_expanded_agent_and_categorized_global_views() {
        let mut agent_model = SelectorModel::new(
            vec![row(
                "agent",
                "cutex-dev-v5",
                CutexSessionLifecycleState::Online,
                false,
                true,
            )],
            false,
            false,
        );
        agent_model.handle(SelectorEvent::OpenSettings);
        let wide = rendered_text_at(120, 24, &agent_model);
        assert!(wide.contains("view [Expanded] Categories"));
        let setting = wide.find("SETTING").expect("setting heading");
        let value = wide.find("VALUE").expect("value heading");
        assert!(setting < value);
        assert!(wide.contains("Identity"));
        assert!(wide.contains("  Agent name"));
        assert!(wide.contains("cutex-dev-v5"));
        assert!(wide.contains("Launch"));
        assert!(wide.contains("  Runtime backend"));
        assert!(wide.contains("V switch view"));
        assert!(!wide.contains("Identity options"));

        agent_model.handle(SelectorEvent::Down);
        agent_model.handle(SelectorEvent::Insert('v'));
        let categorized = rendered_text(120, &agent_model);
        assert!(categorized.contains("view Expanded [Categories]"));
        let categories = categorized.find("Categories").expect("category pane");
        let options = categorized.find("Identity options").expect("option pane");
        let value = categorized.find("Current value").expect("value pane");
        assert!(categories < options && options < value);
        assert!(categorized.contains("Host"));
        assert!(categorized.contains("tethys"));

        let mut global_model = SelectorModel::new(vec![global_row()], false, false);
        global_model.handle(SelectorEvent::Activate);
        global_model.handle(SelectorEvent::Down);
        let medium = rendered_text(80, &global_model);
        assert!(medium.contains("view Expanded [Categories]"));
        assert!(medium.contains("Global settings"));
        assert!(medium.contains("Defaults options"));
        assert!(medium.contains("Default profile"));
        assert!(medium.contains("Direct default launch"));
        assert!(medium.contains("Tab/Esc"));
        assert!(medium.contains("Ctrl+C exit"));

        let wide_boundary = rendered_text(96, &global_model);
        assert!(wide_boundary.contains("A apply"));
        assert!(wide_boundary.contains("D discard"));
        assert!(wide_boundary.contains("Ctrl+C exit"));

        global_model.handle(SelectorEvent::OpenActions);
        global_model.handle(SelectorEvent::Down);
        global_model.handle(SelectorEvent::Activate);
        let medium_value = rendered_text(80, &global_model);
        assert!(medium_value.contains("Current value"));
        assert!(medium_value.contains("Direct default launch"));

        global_model.handle(SelectorEvent::Insert('v'));
        let global_expanded = rendered_text_at(120, 24, &global_model);
        assert!(global_expanded.contains("view [Expanded] Categories"));
        assert!(global_expanded.contains("SETTING"));
        assert!(global_expanded.contains("VALUE"));
        assert!(global_expanded.contains("General"));
        assert!(global_expanded.contains("  Managed sessions"));

        let mut narrow_model = SelectorModel::new(vec![global_row()], false, false);
        narrow_model.handle(SelectorEvent::Activate);
        let narrow_categories = rendered_text(50, &narrow_model);
        assert!(narrow_categories.contains("Categories"));
        assert!(narrow_categories.contains("Notifications  12"));
        assert!(narrow_categories.contains("Ctrl+C exit"));
        assert!(!narrow_categories.contains("Managed sessions"));
        narrow_model.handle(SelectorEvent::OpenActions);
        let narrow_options = rendered_text(50, &narrow_model);
        assert!(narrow_options.contains("General options"));
        assert!(narrow_options.contains("Managed sessions"));
        narrow_model.handle(SelectorEvent::Activate);
        let narrow_value = rendered_text(50, &narrow_model);
        assert!(narrow_value.contains("Current value"));
        assert!(narrow_value.contains("Managed sessions"));

        let mut narrow_choice_model = SelectorModel::new(vec![global_row()], false, false);
        narrow_choice_model.handle(SelectorEvent::Activate);
        narrow_choice_model.handle(SelectorEvent::Insert('v'));
        narrow_choice_model.handle(SelectorEvent::Activate);
        let narrow_choice = rendered_text_at(50, 24, &narrow_choice_model);
        assert!(narrow_choice.contains("Managed sessions"));
        assert!(narrow_choice.contains("Ctrl+C exit"));

        let mut global_tail_model = SelectorModel::new(vec![global_row()], false, false);
        global_tail_model.handle(SelectorEvent::Activate);
        global_tail_model.handle(SelectorEvent::Last);
        global_tail_model.handle(SelectorEvent::OpenActions);
        global_tail_model.handle(SelectorEvent::Last);
        let global_tail = rendered_text(120, &global_tail_model);
        assert!(global_tail.contains("Agent Bus"));
        assert!(global_tail.contains("Message suffix"));
        assert!(global_tail.contains("Ctrl+C exit"));
    }

    #[test]
    fn footer_shortcuts_are_styled_separately_from_descriptions() {
        let record = editable_record();
        let mut model = editable_model(&record);
        model.handle(SelectorEvent::OpenSettings);
        let width = 120;
        let height = 16;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_selector(frame, &model))
            .expect("render selector");
        let buffer = terminal.backend().buffer();
        let footer_y = height - 1;
        let footer = (0..width)
            .map(|x| buffer.cell((x, footer_y)).expect("footer cell").symbol())
            .collect::<String>();

        for (phrase, key, description) in [
            ("A apply", "A", "apply"),
            ("D discard", "D", "discard"),
            ("Ctrl+C exit", "Ctrl+C", "exit"),
        ] {
            let start = footer.find(phrase).expect("footer shortcut") as u16;
            for offset in 0..key.len() as u16 {
                let cell = buffer
                    .cell((start + offset, footer_y))
                    .expect("shortcut cell");
                assert_eq!(cell.fg, Color::Cyan);
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
            let description_start = start + key.len() as u16 + 1;
            let description_cell = buffer
                .cell((description_start, footer_y))
                .expect("description cell");
            assert_eq!(description_cell.symbol(), &description[..1]);
            assert_ne!(description_cell.fg, Color::Cyan);
            assert!(!description_cell.modifier.contains(Modifier::BOLD));
        }
    }

    #[test]
    fn released_keys_and_modified_text_do_not_change_the_filter() {
        let released = KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(selector_event_from_key(released, false), None);
        assert!(!close_runtime_shortcut_from_key(released));
        assert_eq!(
            selector_event_from_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), false),
            Some(SelectorEvent::Insert('q'))
        );
        assert_eq!(
            selector_event_from_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                false
            ),
            Some(SelectorEvent::Exit)
        );
        assert_eq!(
            selector_event_from_key(
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
                false
            ),
            Some(SelectorEvent::ClearInput)
        );
        let direct_close = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert!(close_runtime_shortcut_from_key(direct_close));
        assert_eq!(selector_event_from_key(direct_close, false), None);
        assert_eq!(
            selector_event_from_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE), false),
            Some(SelectorEvent::Delete)
        );
    }

    #[test]
    fn shift_enter_is_bound_only_with_enhanced_keyboard_support() {
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(selector_event_from_key(shift_enter, false), None);
        assert_eq!(
            selector_event_from_key(shift_enter, true),
            Some(SelectorEvent::OpenActions)
        );
        assert_eq!(
            selector_event_from_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), false),
            Some(SelectorEvent::OpenActions)
        );
        assert_eq!(
            selector_event_from_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), false),
            Some(SelectorEvent::OpenSettings)
        );
    }

    #[test]
    fn enhancement_probe_is_limited_to_known_terminal_environments() {
        assert!(terminal_environment_may_support_enhancement(
            Some("xterm-kitty"),
            None,
            false
        ));
        assert!(terminal_environment_may_support_enhancement(
            Some("xterm-256color"),
            Some("WezTerm"),
            false
        ));
        assert!(terminal_environment_may_support_enhancement(
            None, None, true
        ));
        assert!(!terminal_environment_may_support_enhancement(
            Some("xterm-256color"),
            None,
            false
        ));
    }

    #[test]
    fn successful_recent_adoption_persists_native_thread_identity_and_refreshes_projection() {
        let _home = IsolatedTestHome::new("cutex-recent-adopt").expect("isolated home");
        let request = RecentAdoptionRequest {
            thread_id: "native-thread-123".to_string(),
            title: "Native preview".to_string(),
            cwd: "/native/work".to_string(),
        };

        let result = adopt_recent_thread(&request).expect("adopt native thread");
        let snapshot = result.snapshot.expect("agent projection");
        let record = result
            .store
            .sessions
            .values()
            .find(|record| record.codex_session_id.as_deref() == Some("native-thread-123"))
            .expect("adopted record");
        assert!(cutex_session_is_managed(record));
        assert!(snapshot.rows.iter().any(|row| {
            row.target
                .agent_key()
                .is_some_and(|key| key == record.cutex_session_id)
        }));
    }

    #[test]
    fn persisted_recent_adoption_stays_successful_when_agent_projection_fails() {
        let request = RecentAdoptionRequest {
            thread_id: "native-thread-123".to_string(),
            title: "Native preview".to_string(),
            cwd: "/native/work".to_string(),
        };
        let mut model = SelectorModel::new(vec![recent_sessions_row()], false, false);
        model.recent_adoption_succeeded(
            &request,
            RecentAdoptionResult {
                store: CutexSessionStore::default(),
                snapshot: Err("projection unavailable".to_string()),
            },
        );

        assert_eq!(
            model.notice.as_deref(),
            Some("Adopted native thread Native preview")
        );
        assert!(model.warning.as_deref().is_some_and(|warning| {
            warning.starts_with("Native thread was adopted, but agent refresh failed:")
        }));
    }
}
