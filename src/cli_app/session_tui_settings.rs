use std::fmt;

use cutex::agent_bus::identity::normalize_agent_groups;
use cutex::agent_bus::service::DEFAULT_AGENT_BUS_PORT;
use cutex::config::global_settings::{
    parse_agent_bus_port, parse_desktop_notify_port, parse_notify_events,
    parse_notify_user_message_content, parse_optional_u64, parse_rate_limit_mode,
    ConfigValueUpdate, GlobalConfigPatch,
};
use cutex::config::proxy::proxy_config_from_parts;
use cutex::notify::service::DEFAULT_DESKTOP_NOTIFY_PORT;
use cutex::profiles::model::CodezConfig;
use cutex::session::model::{
    parse_cutex_session_quick_action_mode, parse_cutex_session_runtime_backend,
    CutexSessionQuickActionMode, CutexSessionRecord, CutexSessionRuntimeBackend,
};
use cutex::session::projection::runtime_backend_short_label;
use cutex::session::service::{
    cutex_session_display_name, cutex_session_is_managed, cutex_session_launch_cwd,
    normalize_cutex_session_managed_cwd_path, CutexSessionRoutingPatch,
    CutexSessionRuntimeDefaultsPatch, CutexSessionValueUpdate,
};

use super::prompt::{cli_args_label, parse_cli_args_value};
use super::session_tui_profile_settings::ProfileSettingsField;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionTuiSettingOption {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) field: Option<SessionSettingsField>,
    pub(super) global_field: Option<GlobalSettingsField>,
    pub(super) profile_field: Option<ProfileSettingsField>,
    pub(super) command: Option<SessionSettingsCommand>,
    pub(super) dirty: bool,
}

impl SessionTuiSettingOption {
    fn new(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            field: None,
            global_field: None,
            profile_field: None,
            command: None,
            dirty: false,
        }
    }

    fn editable(
        label: &'static str,
        value: impl Into<String>,
        field: SessionSettingsField,
        dirty: bool,
    ) -> Self {
        Self {
            label,
            value: value.into(),
            field: Some(field),
            global_field: None,
            profile_field: None,
            command: None,
            dirty,
        }
    }

    fn command(
        label: &'static str,
        value: impl Into<String>,
        command: SessionSettingsCommand,
    ) -> Self {
        Self {
            label,
            value: value.into(),
            field: None,
            global_field: None,
            profile_field: None,
            command: Some(command),
            dirty: false,
        }
    }

    fn global_editable(
        label: &'static str,
        value: impl Into<String>,
        field: GlobalSettingsField,
        dirty: bool,
    ) -> Self {
        Self {
            label,
            value: value.into(),
            field: None,
            global_field: Some(field),
            profile_field: None,
            command: None,
            dirty,
        }
    }

    pub(super) fn profile_editable(
        label: &'static str,
        value: impl Into<String>,
        field: ProfileSettingsField,
        dirty: bool,
    ) -> Self {
        Self {
            label,
            value: value.into(),
            field: None,
            global_field: None,
            profile_field: Some(field),
            command: None,
            dirty,
        }
    }

    pub(super) fn profile_read_only(label: &'static str, value: impl Into<String>) -> Self {
        Self::new(label, value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionTuiSettingCategory {
    pub(super) label: &'static str,
    pub(super) options: Vec<SessionTuiSettingOption>,
}

impl SessionTuiSettingCategory {
    fn new(label: &'static str, options: Vec<SessionTuiSettingOption>) -> Self {
        Self { label, options }
    }

    pub(super) fn profile(label: &'static str, options: Vec<SessionTuiSettingOption>) -> Self {
        Self::new(label, options)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionSettingsField {
    AgentName,
    Profile,
    PermissionPreset,
    ApprovalPolicy,
    SandboxMode,
    Model,
    ReasoningEffort,
    RuntimeBackend,
    ManagedCwd,
    ExtraCliArgs,
    AgentGroups,
    WorkbenchVisibility,
    QuickAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionSettingsCommand {
    Adopt,
    Unmanage,
}

impl SessionSettingsCommand {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Adopt => "Adopt",
            Self::Unmanage => "Unmanage",
        }
    }

    pub(super) fn success_notice(self) -> &'static str {
        match self {
            Self::Adopt => "Adopted agent",
            Self::Unmanage => "Unmanaged agent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionSettingsChoice {
    pub(super) label: String,
    pub(super) value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionSettingsEditorKind {
    Choice,
    Text,
    Tags,
    Secret,
}

const PERMISSION_PRESET_CHOICES: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("Read only", Some("read-only")),
    ("Workspace", Some("workspace")),
    ("Full access", Some("full-access")),
];

const APPROVAL_POLICY_CHOICES: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("On request", Some("on-request")),
    ("Never", Some("never")),
];

const SANDBOX_MODE_CHOICES: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("Read only", Some("read-only")),
    ("Workspace write", Some("workspace-write")),
    ("Danger full access", Some("danger-full-access")),
];

const REASONING_EFFORT_CHOICES: &[(&str, Option<&str>)] = &[
    ("Inherit", None),
    ("Minimal", Some("minimal")),
    ("Low", Some("low")),
    ("Medium", Some("medium")),
    ("High", Some("high")),
    ("Extra high", Some("xhigh")),
];

const RUNTIME_BACKEND_CHOICES: &[(&str, Option<&str>)] = &[
    ("Host", Some("host")),
    ("Native foreground", Some("native")),
    ("Docker", Some("docker")),
    ("Cute Alden", Some("alden")),
    ("Future", Some("future")),
];

const WORKBENCH_VISIBILITY_CHOICES: &[(&str, Option<&str>)] =
    &[("Visible", Some("visible")), ("Hidden", Some("hidden"))];

const QUICK_ACTION_CHOICES: &[(&str, Option<&str>)] = &[
    ("Auto", Some("auto")),
    ("Pinned", Some("pinned")),
    ("Hidden", Some("hidden")),
];

impl SessionSettingsField {
    pub(super) fn editor_kind(self) -> SessionSettingsEditorKind {
        match self {
            Self::Profile
            | Self::PermissionPreset
            | Self::ApprovalPolicy
            | Self::SandboxMode
            | Self::ReasoningEffort
            | Self::RuntimeBackend
            | Self::WorkbenchVisibility
            | Self::QuickAction => SessionSettingsEditorKind::Choice,
            Self::AgentName | Self::Model | Self::ManagedCwd | Self::ExtraCliArgs => {
                SessionSettingsEditorKind::Text
            }
            Self::AgentGroups => SessionSettingsEditorKind::Tags,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionSettingsSnapshot {
    agent_name: String,
    profile: Option<String>,
    profile_names: Vec<String>,
    permission_defaults: Option<String>,
    approval_policy: Option<String>,
    sandbox_mode: Option<String>,
    model_defaults: Option<String>,
    reasoning_defaults: Option<String>,
    runtime_backend: CutexSessionRuntimeBackend,
    session_cwd: String,
    working_directory: String,
    managed_cwd: Option<String>,
    default_cli_args: Vec<String>,
    default_cli_args_label: String,
    agent_groups: Vec<String>,
    agent_groups_label: String,
    exposed_to_backend: bool,
    quick_action: CutexSessionQuickActionMode,
    managed: bool,
    cutex_session_id: String,
    codex_session_id: Option<String>,
    host_id: String,
}

impl SessionSettingsSnapshot {
    #[cfg(test)]
    pub(super) fn from_record(record: &CutexSessionRecord) -> Self {
        Self::from_record_with_profiles(record, &[])
    }

    pub(super) fn from_record_with_profiles(
        record: &CutexSessionRecord,
        profile_names: &[String],
    ) -> Self {
        Self {
            agent_name: cutex_session_display_name(record),
            profile: record.profile.clone(),
            profile_names: profile_names.to_vec(),
            permission_defaults: record.permission_defaults.clone(),
            approval_policy: record.approval_policy.clone(),
            sandbox_mode: record.sandbox_mode.clone(),
            model_defaults: record.model_defaults.clone(),
            reasoning_defaults: record.reasoning_defaults.clone(),
            runtime_backend: record.runtime_backend,
            session_cwd: record.cwd.clone(),
            working_directory: cutex_session_launch_cwd(record).to_string(),
            managed_cwd: record.managed_cwd.clone(),
            default_cli_args: record.default_cli_args.clone(),
            default_cli_args_label: cli_args_label(&record.default_cli_args),
            agent_groups: record.agent_groups.clone(),
            agent_groups_label: record.agent_groups.join(", "),
            exposed_to_backend: record.exposed_to_backend,
            quick_action: record.quick_action,
            managed: cutex_session_is_managed(record),
            cutex_session_id: record.cutex_session_id.clone(),
            codex_session_id: record.codex_session_id.clone(),
            host_id: record.host_id.clone(),
        }
    }

    pub(super) fn value(&self, field: SessionSettingsField) -> Option<&str> {
        match field {
            SessionSettingsField::AgentName => Some(self.agent_name.as_str()),
            SessionSettingsField::Profile => self.profile.as_deref(),
            SessionSettingsField::PermissionPreset => self.permission_defaults.as_deref(),
            SessionSettingsField::ApprovalPolicy => self.approval_policy.as_deref(),
            SessionSettingsField::SandboxMode => self.sandbox_mode.as_deref(),
            SessionSettingsField::Model => self.model_defaults.as_deref(),
            SessionSettingsField::ReasoningEffort => self.reasoning_defaults.as_deref(),
            SessionSettingsField::RuntimeBackend => {
                Some(runtime_backend_short_label(self.runtime_backend))
            }
            SessionSettingsField::ManagedCwd => self.managed_cwd.as_deref(),
            SessionSettingsField::ExtraCliArgs => Some(self.default_cli_args_label.as_str()),
            SessionSettingsField::AgentGroups => Some(self.agent_groups_label.as_str()),
            SessionSettingsField::WorkbenchVisibility => Some(if self.exposed_to_backend {
                "visible"
            } else {
                "hidden"
            }),
            SessionSettingsField::QuickAction => Some(self.quick_action.label()),
        }
    }

    pub(super) fn choices(&self, field: SessionSettingsField) -> Vec<SessionSettingsChoice> {
        if field == SessionSettingsField::Profile {
            let mut choices = vec![SessionSettingsChoice {
                label: "Follow global default".to_string(),
                value: None,
            }];
            choices.extend(self.profile_names.iter().map(|name| SessionSettingsChoice {
                label: name.clone(),
                value: Some(name.clone()),
            }));
            return choices;
        }
        let choices = match field {
            SessionSettingsField::PermissionPreset => PERMISSION_PRESET_CHOICES,
            SessionSettingsField::ApprovalPolicy => APPROVAL_POLICY_CHOICES,
            SessionSettingsField::SandboxMode => SANDBOX_MODE_CHOICES,
            SessionSettingsField::ReasoningEffort => REASONING_EFFORT_CHOICES,
            SessionSettingsField::RuntimeBackend => RUNTIME_BACKEND_CHOICES,
            SessionSettingsField::WorkbenchVisibility => WORKBENCH_VISIBILITY_CHOICES,
            SessionSettingsField::QuickAction => QUICK_ACTION_CHOICES,
            SessionSettingsField::AgentName
            | SessionSettingsField::Profile
            | SessionSettingsField::Model
            | SessionSettingsField::ManagedCwd
            | SessionSettingsField::ExtraCliArgs
            | SessionSettingsField::AgentGroups => &[],
        };
        choices
            .iter()
            .map(|(label, value)| SessionSettingsChoice {
                label: (*label).to_string(),
                value: value.map(str::to_string),
            })
            .collect()
    }

    pub(super) fn profile_names(&self) -> &[String] {
        &self.profile_names
    }

    pub(super) fn categories(
        &self,
        draft: &SessionSettingsDraft,
    ) -> Vec<SessionTuiSettingCategory> {
        vec![
            SessionTuiSettingCategory::new(
                "Identity",
                vec![
                    self.editable_option("Agent name", SessionSettingsField::AgentName, draft),
                    self.editable_option("Profile", SessionSettingsField::Profile, draft),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Permissions",
                vec![
                    self.editable_option(
                        "Permission preset",
                        SessionSettingsField::PermissionPreset,
                        draft,
                    ),
                    self.editable_option(
                        "Approval policy",
                        SessionSettingsField::ApprovalPolicy,
                        draft,
                    ),
                    self.editable_option("Sandbox mode", SessionSettingsField::SandboxMode, draft),
                    self.editable_option("Model", SessionSettingsField::Model, draft),
                    self.editable_option(
                        "Reasoning effort",
                        SessionSettingsField::ReasoningEffort,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Launch",
                vec![
                    self.editable_option(
                        "Runtime backend",
                        SessionSettingsField::RuntimeBackend,
                        draft,
                    ),
                    SessionTuiSettingOption::new(
                        "Working directory",
                        draft.effective_working_directory(self),
                    ),
                    self.editable_option("Managed cwd", SessionSettingsField::ManagedCwd, draft),
                    self.editable_option(
                        "Extra CLI args",
                        SessionSettingsField::ExtraCliArgs,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Routing",
                vec![
                    self.editable_option(
                        "Message groups",
                        SessionSettingsField::AgentGroups,
                        draft,
                    ),
                    if self.managed {
                        self.editable_option(
                            "Workbench visibility",
                            SessionSettingsField::WorkbenchVisibility,
                            draft,
                        )
                    } else {
                        SessionTuiSettingOption::new(
                            "Workbench visibility",
                            if self.exposed_to_backend {
                                "visible"
                            } else {
                                "hidden"
                            },
                        )
                    },
                    self.editable_option("Quick action", SessionSettingsField::QuickAction, draft),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Details",
                vec![
                    SessionTuiSettingOption::new("Managed", enabled(self.managed)),
                    SessionTuiSettingOption::command(
                        "Management",
                        if self.managed { "unmanage" } else { "adopt" },
                        if self.managed {
                            SessionSettingsCommand::Unmanage
                        } else {
                            SessionSettingsCommand::Adopt
                        },
                    ),
                    SessionTuiSettingOption::new("Cutex session", self.cutex_session_id.clone()),
                    SessionTuiSettingOption::new(
                        "Cute-codex session",
                        optional(self.codex_session_id.as_deref()),
                    ),
                    SessionTuiSettingOption::new("Host", nonempty(&self.host_id)),
                ],
            ),
        ]
    }

    fn editable_option(
        &self,
        label: &'static str,
        field: SessionSettingsField,
        draft: &SessionSettingsDraft,
    ) -> SessionTuiSettingOption {
        let value = draft.value(self, field);
        let value = if field == SessionSettingsField::Profile && value.is_none() {
            "Follow global default".to_string()
        } else {
            optional(value)
        };
        SessionTuiSettingOption::editable(label, value, field, draft.field_is_dirty(field))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct SessionSettingsDraft {
    agent_name: Option<String>,
    profile: CutexSessionValueUpdate<String>,
    permission_defaults: CutexSessionValueUpdate<String>,
    approval_policy: CutexSessionValueUpdate<String>,
    sandbox_mode: CutexSessionValueUpdate<String>,
    model_defaults: CutexSessionValueUpdate<String>,
    reasoning_defaults: CutexSessionValueUpdate<String>,
    runtime_backend: Option<CutexSessionRuntimeBackend>,
    managed_cwd: CutexSessionValueUpdate<String>,
    default_cli_args: Option<Vec<String>>,
    default_cli_args_label: Option<String>,
    agent_groups: Option<Vec<String>>,
    agent_groups_label: Option<String>,
    exposed_to_backend: Option<bool>,
    quick_action: Option<CutexSessionQuickActionMode>,
}

impl SessionSettingsDraft {
    pub(super) fn stage(
        &mut self,
        snapshot: &SessionSettingsSnapshot,
        field: SessionSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        match field {
            SessionSettingsField::AgentName => {
                let name = value.unwrap_or_default().trim().to_string();
                if name.is_empty() {
                    anyhow::bail!("Agent name cannot be empty");
                }
                self.agent_name = (name != snapshot.agent_name).then_some(name);
                return Ok(());
            }
            SessionSettingsField::RuntimeBackend => {
                let value = value
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("runtime backend cannot inherit"))?;
                let backend = parse_cutex_session_runtime_backend(value)?;
                self.runtime_backend = (backend != snapshot.runtime_backend).then_some(backend);
                return Ok(());
            }
            SessionSettingsField::ManagedCwd => {
                self.managed_cwd = match value {
                    Some(value) => {
                        let normalized = normalize_cutex_session_managed_cwd_path(&value)?;
                        if snapshot.managed_cwd.as_deref() == Some(normalized.as_str()) {
                            CutexSessionValueUpdate::Unchanged
                        } else {
                            CutexSessionValueUpdate::Set(normalized)
                        }
                    }
                    None if snapshot.managed_cwd.is_none() => CutexSessionValueUpdate::Unchanged,
                    None => CutexSessionValueUpdate::Clear,
                };
                return Ok(());
            }
            SessionSettingsField::ExtraCliArgs => {
                let args = parse_cli_args_value(value.as_deref().unwrap_or_default())?;
                if args == snapshot.default_cli_args {
                    self.default_cli_args = None;
                    self.default_cli_args_label = None;
                } else {
                    self.default_cli_args_label = Some(cli_args_label(&args));
                    self.default_cli_args = Some(args);
                }
                return Ok(());
            }
            SessionSettingsField::AgentGroups => {
                let raw = value.unwrap_or_default();
                let groups = normalize_agent_groups(
                    raw.split(|character: char| character == ',' || character.is_whitespace())
                        .map(str::to_string)
                        .collect(),
                );
                if groups.is_empty() && !snapshot.agent_groups.is_empty() {
                    anyhow::bail!("At least one non-empty group is required");
                }
                if groups == snapshot.agent_groups {
                    self.agent_groups = None;
                    self.agent_groups_label = None;
                } else {
                    self.agent_groups_label = Some(groups.join(", "));
                    self.agent_groups = Some(groups);
                }
                return Ok(());
            }
            SessionSettingsField::WorkbenchVisibility => {
                let visible = match value.as_deref() {
                    Some("visible") => true,
                    Some("hidden") => false,
                    Some(other) => anyhow::bail!("unsupported workbench visibility: {other}"),
                    None => anyhow::bail!("workbench visibility cannot inherit"),
                };
                self.exposed_to_backend =
                    (visible != snapshot.exposed_to_backend).then_some(visible);
                return Ok(());
            }
            SessionSettingsField::QuickAction => {
                let value = value
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("quick action cannot inherit"))?;
                let mode = parse_cutex_session_quick_action_mode(value)?;
                self.quick_action = (mode != snapshot.quick_action).then_some(mode);
                return Ok(());
            }
            _ => {}
        }
        let update = if value.as_deref() == snapshot.value(field) {
            CutexSessionValueUpdate::Unchanged
        } else if let Some(value) = value {
            CutexSessionValueUpdate::Set(value)
        } else {
            CutexSessionValueUpdate::Clear
        };
        *self.field_mut(field) = update;
        Ok(())
    }

    pub(super) fn value<'a>(
        &'a self,
        snapshot: &'a SessionSettingsSnapshot,
        field: SessionSettingsField,
    ) -> Option<&'a str> {
        match field {
            SessionSettingsField::AgentName => {
                return self.agent_name.as_deref().or_else(|| snapshot.value(field));
            }
            SessionSettingsField::RuntimeBackend => {
                return self
                    .runtime_backend
                    .map(runtime_backend_short_label)
                    .or_else(|| snapshot.value(field));
            }
            SessionSettingsField::ManagedCwd => {
                return match &self.managed_cwd {
                    CutexSessionValueUpdate::Unchanged => snapshot.value(field),
                    CutexSessionValueUpdate::Set(value) => Some(value.as_str()),
                    CutexSessionValueUpdate::Clear => None,
                };
            }
            SessionSettingsField::ExtraCliArgs => {
                return self
                    .default_cli_args_label
                    .as_deref()
                    .or_else(|| snapshot.value(field));
            }
            SessionSettingsField::AgentGroups => {
                return self
                    .agent_groups_label
                    .as_deref()
                    .or_else(|| snapshot.value(field));
            }
            SessionSettingsField::WorkbenchVisibility => {
                return self
                    .exposed_to_backend
                    .map(|visible| if visible { "visible" } else { "hidden" })
                    .or_else(|| snapshot.value(field));
            }
            SessionSettingsField::QuickAction => {
                return self
                    .quick_action
                    .map(CutexSessionQuickActionMode::label)
                    .or_else(|| snapshot.value(field));
            }
            _ => {}
        }
        match self.field(field) {
            CutexSessionValueUpdate::Unchanged => snapshot.value(field),
            CutexSessionValueUpdate::Set(value) => Some(value.as_str()),
            CutexSessionValueUpdate::Clear => None,
        }
    }

    pub(super) fn field_is_dirty(&self, field: SessionSettingsField) -> bool {
        match field {
            SessionSettingsField::AgentName => self.agent_name.is_some(),
            SessionSettingsField::RuntimeBackend => self.runtime_backend.is_some(),
            SessionSettingsField::ManagedCwd => {
                !matches!(self.managed_cwd, CutexSessionValueUpdate::Unchanged)
            }
            SessionSettingsField::ExtraCliArgs => self.default_cli_args.is_some(),
            SessionSettingsField::AgentGroups => self.agent_groups.is_some(),
            SessionSettingsField::WorkbenchVisibility => self.exposed_to_backend.is_some(),
            SessionSettingsField::QuickAction => self.quick_action.is_some(),
            _ => !matches!(self.field(field), CutexSessionValueUpdate::Unchanged),
        }
    }

    pub(super) fn dirty_count(&self) -> usize {
        [
            SessionSettingsField::AgentName,
            SessionSettingsField::Profile,
            SessionSettingsField::PermissionPreset,
            SessionSettingsField::ApprovalPolicy,
            SessionSettingsField::SandboxMode,
            SessionSettingsField::Model,
            SessionSettingsField::ReasoningEffort,
            SessionSettingsField::RuntimeBackend,
            SessionSettingsField::ManagedCwd,
            SessionSettingsField::ExtraCliArgs,
            SessionSettingsField::AgentGroups,
            SessionSettingsField::WorkbenchVisibility,
            SessionSettingsField::QuickAction,
        ]
        .into_iter()
        .filter(|field| self.field_is_dirty(*field))
        .count()
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty_count() > 0
    }

    pub(super) fn runtime_defaults_patch(&self) -> CutexSessionRuntimeDefaultsPatch {
        CutexSessionRuntimeDefaultsPatch {
            runtime_backend: self.runtime_backend,
            managed_cwd: self.managed_cwd.clone(),
            permission_defaults: self.permission_defaults.clone(),
            approval_policy: self.approval_policy.clone(),
            sandbox_mode: self.sandbox_mode.clone(),
            model_defaults: self.model_defaults.clone(),
            reasoning_defaults: self.reasoning_defaults.clone(),
            default_cli_args: self.default_cli_args.clone(),
            ..CutexSessionRuntimeDefaultsPatch::default()
        }
    }

    pub(super) fn agent_name(&self) -> Option<&str> {
        self.agent_name.as_deref()
    }

    pub(super) fn profile_update(&self) -> &CutexSessionValueUpdate<String> {
        &self.profile
    }

    pub(super) fn routing_patch(&self) -> CutexSessionRoutingPatch {
        CutexSessionRoutingPatch {
            agent_groups: self.agent_groups.clone(),
            exposed_to_backend: self.exposed_to_backend,
            quick_action: self.quick_action,
        }
    }

    pub(super) fn routing_is_dirty(&self) -> bool {
        self.agent_groups.is_some()
            || self.exposed_to_backend.is_some()
            || self.quick_action.is_some()
    }

    pub(super) fn agent_groups_are_dirty(&self) -> bool {
        self.agent_groups.is_some()
    }

    pub(super) fn launch_actions_are_dirty(&self) -> bool {
        self.runtime_backend.is_some()
            || !matches!(self.managed_cwd, CutexSessionValueUpdate::Unchanged)
    }

    fn effective_working_directory(&self, snapshot: &SessionSettingsSnapshot) -> String {
        match &self.managed_cwd {
            CutexSessionValueUpdate::Unchanged => snapshot.working_directory.clone(),
            CutexSessionValueUpdate::Set(path) => path.clone(),
            CutexSessionValueUpdate::Clear => snapshot.session_cwd.clone(),
        }
    }

    pub(super) fn runtime_defaults_are_dirty(&self) -> bool {
        [
            SessionSettingsField::RuntimeBackend,
            SessionSettingsField::ManagedCwd,
            SessionSettingsField::ExtraCliArgs,
            SessionSettingsField::PermissionPreset,
            SessionSettingsField::ApprovalPolicy,
            SessionSettingsField::SandboxMode,
            SessionSettingsField::Model,
            SessionSettingsField::ReasoningEffort,
        ]
        .into_iter()
        .any(|field| self.field_is_dirty(field))
    }

    fn field(&self, field: SessionSettingsField) -> &CutexSessionValueUpdate<String> {
        match field {
            SessionSettingsField::Profile => &self.profile,
            SessionSettingsField::PermissionPreset => &self.permission_defaults,
            SessionSettingsField::ApprovalPolicy => &self.approval_policy,
            SessionSettingsField::SandboxMode => &self.sandbox_mode,
            SessionSettingsField::Model => &self.model_defaults,
            SessionSettingsField::ReasoningEffort => &self.reasoning_defaults,
            SessionSettingsField::AgentName
            | SessionSettingsField::RuntimeBackend
            | SessionSettingsField::ManagedCwd
            | SessionSettingsField::ExtraCliArgs
            | SessionSettingsField::AgentGroups
            | SessionSettingsField::WorkbenchVisibility
            | SessionSettingsField::QuickAction => {
                unreachable!("routing fields do not use optional string patches")
            }
        }
    }

    fn field_mut(&mut self, field: SessionSettingsField) -> &mut CutexSessionValueUpdate<String> {
        match field {
            SessionSettingsField::Profile => &mut self.profile,
            SessionSettingsField::PermissionPreset => &mut self.permission_defaults,
            SessionSettingsField::ApprovalPolicy => &mut self.approval_policy,
            SessionSettingsField::SandboxMode => &mut self.sandbox_mode,
            SessionSettingsField::Model => &mut self.model_defaults,
            SessionSettingsField::ReasoningEffort => &mut self.reasoning_defaults,
            SessionSettingsField::AgentName
            | SessionSettingsField::RuntimeBackend
            | SessionSettingsField::ManagedCwd
            | SessionSettingsField::ExtraCliArgs
            | SessionSettingsField::AgentGroups
            | SessionSettingsField::WorkbenchVisibility
            | SessionSettingsField::QuickAction => {
                unreachable!("routing fields do not use optional string patches")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobalSettingsField {
    ManagedSessions,
    DockerSudo,
    DefaultProfile,
    DefaultProfileDirectLaunch,
    ProxyEnabled,
    ProxyUrl,
    ProxyNoProxy,
    ProxyForceHttp,
    NotifyServiceUrl,
    NotifyServiceToken,
    NotifyIdleTimeout,
    NotifyComposerTimeout,
    NotifyApprovalTimeout,
    NotifyStartupTimeout,
    NotifyEvents,
    NotifyMessageContent,
    NotifyPreviewChars,
    DesktopNotifyEnabled,
    DesktopNotifyPort,
    DesktopNotifyToken,
    RateLimitThresholdWarning,
    RateLimitModelNudge,
    AgentBusEnabled,
    AgentBusPort,
    AgentBusToken,
    AgentMessagePrefix,
    AgentMessageSuffix,
}

impl GlobalSettingsField {
    pub(super) fn editor_kind(self) -> SessionSettingsEditorKind {
        match self {
            Self::ManagedSessions
            | Self::DockerSudo
            | Self::DefaultProfile
            | Self::DefaultProfileDirectLaunch
            | Self::ProxyEnabled
            | Self::ProxyForceHttp
            | Self::NotifyMessageContent
            | Self::DesktopNotifyEnabled
            | Self::RateLimitThresholdWarning
            | Self::RateLimitModelNudge
            | Self::AgentBusEnabled => SessionSettingsEditorKind::Choice,
            Self::ProxyUrl
            | Self::ProxyNoProxy
            | Self::NotifyServiceUrl
            | Self::NotifyIdleTimeout
            | Self::NotifyComposerTimeout
            | Self::NotifyApprovalTimeout
            | Self::NotifyStartupTimeout
            | Self::NotifyPreviewChars
            | Self::DesktopNotifyPort
            | Self::AgentBusPort
            | Self::AgentMessagePrefix
            | Self::AgentMessageSuffix => SessionSettingsEditorKind::Text,
            Self::NotifyEvents => SessionSettingsEditorKind::Tags,
            Self::NotifyServiceToken | Self::DesktopNotifyToken | Self::AgentBusToken => {
                SessionSettingsEditorKind::Secret
            }
        }
    }
}

const ENABLED_CHOICES: &[(&str, Option<&str>)] =
    &[("Enabled", Some("enabled")), ("Disabled", Some("disabled"))];
const NOTIFY_MESSAGE_CONTENT_CHOICES: &[(&str, Option<&str>)] = &[
    ("Default", None),
    ("None", Some("none")),
    ("Preview", Some("preview")),
    ("Full", Some("full")),
];
const RATE_LIMIT_MODE_CHOICES: &[(&str, Option<&str>)] = &[
    ("Default", None),
    ("Off", Some("off")),
    ("Daily", Some("daily")),
    ("Always", Some("always")),
];

#[derive(Clone, PartialEq, Eq)]
pub(super) enum SecretSettingsAction {
    Keep,
    Replace(String),
    Clear,
}

impl fmt::Debug for SecretSettingsAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keep => formatter.write_str("Keep"),
            Self::Replace(_) => formatter.write_str("Replace(<redacted>)"),
            Self::Clear => formatter.write_str("Clear"),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct GlobalSettingsSnapshot {
    config: CodezConfig,
    profile_names: Vec<String>,
}

impl GlobalSettingsSnapshot {
    #[cfg(test)]
    pub(super) fn from_config(config: &CodezConfig) -> Self {
        Self::from_config_with_profiles(config, &[])
    }

    pub(super) fn from_config_with_profiles(
        config: &CodezConfig,
        profile_names: &[String],
    ) -> Self {
        Self {
            config: config.clone(),
            profile_names: profile_names.to_vec(),
        }
    }

    pub(super) fn profile_names(&self) -> &[String] {
        &self.profile_names
    }

    pub(super) fn default_profile_name(&self) -> Option<&str> {
        self.config.default_profile.as_deref()
    }

    pub(super) fn choices(&self, field: GlobalSettingsField) -> Vec<SessionSettingsChoice> {
        match field {
            GlobalSettingsField::ManagedSessions
            | GlobalSettingsField::DockerSudo
            | GlobalSettingsField::DefaultProfileDirectLaunch
            | GlobalSettingsField::ProxyEnabled
            | GlobalSettingsField::ProxyForceHttp
            | GlobalSettingsField::DesktopNotifyEnabled
            | GlobalSettingsField::AgentBusEnabled => ENABLED_CHOICES,
            GlobalSettingsField::NotifyMessageContent => NOTIFY_MESSAGE_CONTENT_CHOICES,
            GlobalSettingsField::RateLimitThresholdWarning
            | GlobalSettingsField::RateLimitModelNudge => RATE_LIMIT_MODE_CHOICES,
            GlobalSettingsField::DefaultProfile => {
                return std::iter::once(SessionSettingsChoice {
                    label: "None".to_string(),
                    value: None,
                })
                .chain(self.profile_names.iter().map(|name| SessionSettingsChoice {
                    label: name.clone(),
                    value: Some(name.clone()),
                }))
                .collect();
            }
            GlobalSettingsField::ProxyUrl
            | GlobalSettingsField::ProxyNoProxy
            | GlobalSettingsField::NotifyServiceUrl
            | GlobalSettingsField::NotifyServiceToken
            | GlobalSettingsField::NotifyIdleTimeout
            | GlobalSettingsField::NotifyComposerTimeout
            | GlobalSettingsField::NotifyApprovalTimeout
            | GlobalSettingsField::NotifyStartupTimeout
            | GlobalSettingsField::NotifyEvents
            | GlobalSettingsField::NotifyPreviewChars
            | GlobalSettingsField::DesktopNotifyPort
            | GlobalSettingsField::DesktopNotifyToken
            | GlobalSettingsField::AgentBusPort
            | GlobalSettingsField::AgentBusToken
            | GlobalSettingsField::AgentMessagePrefix
            | GlobalSettingsField::AgentMessageSuffix => &[],
        }
        .iter()
        .map(|(label, value)| SessionSettingsChoice {
            label: (*label).to_string(),
            value: value.map(str::to_string),
        })
        .collect()
    }

    pub(super) fn categories(&self, draft: &GlobalSettingsDraft) -> Vec<SessionTuiSettingCategory> {
        let config = &self.config;
        vec![
            SessionTuiSettingCategory::new(
                "General",
                vec![
                    self.editable_option(
                        "Managed sessions",
                        GlobalSettingsField::ManagedSessions,
                        draft,
                    ),
                    self.editable_option("Docker sudo", GlobalSettingsField::DockerSudo, draft),
                    SessionTuiSettingOption::new(
                        "Custom status items",
                        config.custom_status_items.len().to_string(),
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Defaults",
                vec![
                    self.editable_option(
                        "Default profile",
                        GlobalSettingsField::DefaultProfile,
                        draft,
                    ),
                    self.editable_option(
                        "Direct default launch",
                        GlobalSettingsField::DefaultProfileDirectLaunch,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Proxy",
                vec![
                    self.editable_option("Enabled", GlobalSettingsField::ProxyEnabled, draft),
                    self.editable_option("URL", GlobalSettingsField::ProxyUrl, draft),
                    self.editable_option("NO_PROXY", GlobalSettingsField::ProxyNoProxy, draft),
                    self.editable_option(
                        "Force HTTP transport",
                        GlobalSettingsField::ProxyForceHttp,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Notifications",
                vec![
                    self.editable_option(
                        "Service URL",
                        GlobalSettingsField::NotifyServiceUrl,
                        draft,
                    ),
                    self.editable_option(
                        "Service token",
                        GlobalSettingsField::NotifyServiceToken,
                        draft,
                    ),
                    self.editable_option(
                        "Idle timeout",
                        GlobalSettingsField::NotifyIdleTimeout,
                        draft,
                    ),
                    self.editable_option(
                        "Composer timeout",
                        GlobalSettingsField::NotifyComposerTimeout,
                        draft,
                    ),
                    self.editable_option(
                        "Approval timeout",
                        GlobalSettingsField::NotifyApprovalTimeout,
                        draft,
                    ),
                    self.editable_option(
                        "Startup timeout",
                        GlobalSettingsField::NotifyStartupTimeout,
                        draft,
                    ),
                    self.editable_option("Events", GlobalSettingsField::NotifyEvents, draft),
                    self.editable_option(
                        "Message content",
                        GlobalSettingsField::NotifyMessageContent,
                        draft,
                    ),
                    self.editable_option(
                        "Preview chars",
                        GlobalSettingsField::NotifyPreviewChars,
                        draft,
                    ),
                    self.editable_option(
                        "Desktop notifications",
                        GlobalSettingsField::DesktopNotifyEnabled,
                        draft,
                    ),
                    self.editable_option(
                        "Desktop port",
                        GlobalSettingsField::DesktopNotifyPort,
                        draft,
                    ),
                    self.editable_option(
                        "Desktop token",
                        GlobalSettingsField::DesktopNotifyToken,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Rate limits",
                vec![
                    self.editable_option(
                        "Threshold warning",
                        GlobalSettingsField::RateLimitThresholdWarning,
                        draft,
                    ),
                    self.editable_option(
                        "Model nudge",
                        GlobalSettingsField::RateLimitModelNudge,
                        draft,
                    ),
                ],
            ),
            SessionTuiSettingCategory::new(
                "Agent Bus",
                vec![
                    self.editable_option("Enabled", GlobalSettingsField::AgentBusEnabled, draft),
                    self.editable_option("Port", GlobalSettingsField::AgentBusPort, draft),
                    self.editable_option("Token", GlobalSettingsField::AgentBusToken, draft),
                    self.editable_option(
                        "Message prefix",
                        GlobalSettingsField::AgentMessagePrefix,
                        draft,
                    ),
                    self.editable_option(
                        "Message suffix",
                        GlobalSettingsField::AgentMessageSuffix,
                        draft,
                    ),
                ],
            ),
        ]
    }

    fn editable_option(
        &self,
        label: &'static str,
        field: GlobalSettingsField,
        draft: &GlobalSettingsDraft,
    ) -> SessionTuiSettingOption {
        SessionTuiSettingOption::global_editable(
            label,
            draft.value(self, field),
            field,
            draft.field_is_dirty(field),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct GlobalSettingsDraft {
    managed_sessions: Option<bool>,
    docker_sudo: Option<bool>,
    default_profile: ConfigValueUpdate<String>,
    default_profile_direct_launch: Option<bool>,
    proxy_enabled: Option<bool>,
    proxy_url: ConfigValueUpdate<String>,
    proxy_no_proxy: ConfigValueUpdate<String>,
    proxy_force_http: Option<bool>,
    notify_service_url: ConfigValueUpdate<String>,
    notify_service_token: ConfigValueUpdate<String>,
    notify_idle_timeout: ConfigValueUpdate<u64>,
    notify_composer_timeout: ConfigValueUpdate<u64>,
    notify_approval_timeout: ConfigValueUpdate<u64>,
    notify_startup_timeout: ConfigValueUpdate<u64>,
    notify_events: ConfigValueUpdate<Vec<String>>,
    notify_message_content: ConfigValueUpdate<String>,
    notify_preview_chars: ConfigValueUpdate<u64>,
    desktop_notify_enabled: Option<bool>,
    desktop_notify_port: ConfigValueUpdate<u16>,
    desktop_notify_token: ConfigValueUpdate<String>,
    rate_limit_threshold_warning: ConfigValueUpdate<String>,
    rate_limit_model_nudge: ConfigValueUpdate<String>,
    agent_bus_enabled: Option<bool>,
    agent_bus_port: ConfigValueUpdate<u16>,
    agent_bus_token: ConfigValueUpdate<String>,
    agent_message_prefix: ConfigValueUpdate<String>,
    agent_message_suffix: ConfigValueUpdate<String>,
}

impl GlobalSettingsDraft {
    pub(super) fn stage(
        &mut self,
        snapshot: &GlobalSettingsSnapshot,
        field: GlobalSettingsField,
        value: Option<String>,
    ) -> anyhow::Result<()> {
        match field {
            GlobalSettingsField::ManagedSessions => {
                self.managed_sessions = changed_bool(
                    snapshot.config.session.enabled,
                    parse_enabled(value.as_deref())?,
                );
            }
            GlobalSettingsField::DockerSudo => {
                self.docker_sudo = changed_bool(
                    snapshot.config.docker_use_sudo,
                    parse_enabled(value.as_deref())?,
                );
            }
            GlobalSettingsField::DefaultProfile => {
                if value.as_deref() != snapshot.config.default_profile.as_deref() {
                    if let Some(name) = value.as_deref() {
                        if !snapshot
                            .profile_names
                            .iter()
                            .any(|candidate| candidate == name)
                        {
                            anyhow::bail!("Profile is no longer available: {name}");
                        }
                    }
                }
                self.default_profile =
                    changed_optional_value(snapshot.config.default_profile.as_ref(), value);
            }
            GlobalSettingsField::DefaultProfileDirectLaunch => {
                self.default_profile_direct_launch = changed_bool(
                    snapshot.config.default_profile_direct_launch,
                    parse_enabled(value.as_deref())?,
                );
            }
            GlobalSettingsField::ProxyEnabled => {
                let next = parse_enabled(value.as_deref())?;
                let current = snapshot
                    .config
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.enabled);
                self.proxy_enabled = changed_bool(current, next);
                if !next {
                    self.proxy_url = ConfigValueUpdate::Unchanged;
                    self.proxy_no_proxy = ConfigValueUpdate::Unchanged;
                    self.proxy_force_http = None;
                }
            }
            GlobalSettingsField::ProxyUrl => {
                self.proxy_url = changed_optional_string(
                    snapshot
                        .config
                        .proxy
                        .as_ref()
                        .and_then(|proxy| proxy.url.as_deref()),
                    value,
                );
            }
            GlobalSettingsField::ProxyNoProxy => {
                self.proxy_no_proxy = changed_optional_string(
                    snapshot
                        .config
                        .proxy
                        .as_ref()
                        .and_then(|proxy| proxy.no_proxy.as_deref()),
                    value,
                );
            }
            GlobalSettingsField::ProxyForceHttp => {
                let current = snapshot
                    .config
                    .proxy
                    .as_ref()
                    .map(|proxy| proxy.force_http_transport)
                    .unwrap_or(true);
                self.proxy_force_http = changed_bool(current, parse_enabled(value.as_deref())?);
            }
            GlobalSettingsField::NotifyServiceUrl => {
                self.notify_service_url =
                    changed_optional_string(snapshot.config.notify_service_url.as_deref(), value);
            }
            GlobalSettingsField::NotifyServiceToken
            | GlobalSettingsField::DesktopNotifyToken
            | GlobalSettingsField::AgentBusToken => {
                anyhow::bail!("Use Keep, Replace, or Clear for secret settings");
            }
            GlobalSettingsField::NotifyIdleTimeout => {
                self.notify_idle_timeout = changed_optional_value(
                    snapshot.config.notify_service_idle_timeout_secs.as_ref(),
                    parse_optional_u64(value.as_deref().unwrap_or("-"))?,
                );
            }
            GlobalSettingsField::NotifyComposerTimeout => {
                self.notify_composer_timeout = changed_optional_value(
                    snapshot
                        .config
                        .notify_service_composer_idle_timeout_secs
                        .as_ref(),
                    parse_optional_u64(value.as_deref().unwrap_or("-"))?,
                );
            }
            GlobalSettingsField::NotifyApprovalTimeout => {
                self.notify_approval_timeout = changed_optional_value(
                    snapshot
                        .config
                        .notify_service_approval_timeout_secs
                        .as_ref(),
                    parse_optional_u64(value.as_deref().unwrap_or("-"))?,
                );
            }
            GlobalSettingsField::NotifyStartupTimeout => {
                self.notify_startup_timeout = changed_optional_value(
                    snapshot
                        .config
                        .notify_service_startup_idle_timeout_secs
                        .as_ref(),
                    parse_optional_u64(value.as_deref().unwrap_or("-"))?,
                );
            }
            GlobalSettingsField::NotifyEvents => {
                self.notify_events = changed_optional_value(
                    snapshot.config.notify_service_events.as_ref(),
                    parse_notify_events(value.as_deref().unwrap_or("-")),
                );
            }
            GlobalSettingsField::NotifyMessageContent => {
                let current = snapshot.config.notify_service_user_message_content.as_ref();
                self.notify_message_content = if value.as_deref() == current.map(String::as_str) {
                    ConfigValueUpdate::Unchanged
                } else {
                    changed_optional_value(
                        current,
                        parse_notify_user_message_content(value.as_deref().unwrap_or("-"))?,
                    )
                };
            }
            GlobalSettingsField::NotifyPreviewChars => {
                self.notify_preview_chars = changed_optional_value(
                    snapshot
                        .config
                        .notify_service_user_message_preview_chars
                        .as_ref(),
                    parse_optional_u64(value.as_deref().unwrap_or("-"))?,
                );
            }
            GlobalSettingsField::DesktopNotifyEnabled => {
                self.desktop_notify_enabled = changed_bool(
                    snapshot.config.desktop_notify_enabled,
                    parse_enabled(value.as_deref())?,
                );
            }
            GlobalSettingsField::DesktopNotifyPort => {
                let next = parse_desktop_notify_port(value.as_deref().unwrap_or("-"))?;
                self.desktop_notify_port = match next {
                    Some(next)
                        if next
                            == snapshot
                                .config
                                .desktop_notify_port
                                .unwrap_or(DEFAULT_DESKTOP_NOTIFY_PORT) =>
                    {
                        ConfigValueUpdate::Unchanged
                    }
                    Some(next) => ConfigValueUpdate::Set(next),
                    None if snapshot.config.desktop_notify_port.is_none() => {
                        ConfigValueUpdate::Unchanged
                    }
                    None => ConfigValueUpdate::Clear,
                };
            }
            GlobalSettingsField::RateLimitThresholdWarning => {
                let current = snapshot.config.rate_limit_threshold_warning_mode.as_ref();
                self.rate_limit_threshold_warning =
                    if value.as_deref() == current.map(String::as_str) {
                        ConfigValueUpdate::Unchanged
                    } else {
                        changed_optional_value(
                            current,
                            parse_rate_limit_mode(value.as_deref().unwrap_or("-"))?,
                        )
                    };
            }
            GlobalSettingsField::RateLimitModelNudge => {
                let current = snapshot.config.rate_limit_model_nudge_mode.as_ref();
                self.rate_limit_model_nudge = if value.as_deref() == current.map(String::as_str) {
                    ConfigValueUpdate::Unchanged
                } else {
                    changed_optional_value(
                        current,
                        parse_rate_limit_mode(value.as_deref().unwrap_or("-"))?,
                    )
                };
            }
            GlobalSettingsField::AgentBusEnabled => {
                self.agent_bus_enabled = changed_bool(
                    snapshot.config.agent_bus_enabled,
                    parse_enabled(value.as_deref())?,
                );
            }
            GlobalSettingsField::AgentBusPort => {
                let next = parse_agent_bus_port(value.as_deref().unwrap_or("-"))?;
                self.agent_bus_port = match next {
                    Some(next)
                        if next
                            == snapshot
                                .config
                                .agent_bus_port
                                .unwrap_or(DEFAULT_AGENT_BUS_PORT) =>
                    {
                        ConfigValueUpdate::Unchanged
                    }
                    Some(next) => ConfigValueUpdate::Set(next),
                    None if snapshot.config.agent_bus_port.is_none() => {
                        ConfigValueUpdate::Unchanged
                    }
                    None => ConfigValueUpdate::Clear,
                };
            }
            GlobalSettingsField::AgentMessagePrefix => {
                self.agent_message_prefix = changed_optional_literal(
                    snapshot.config.agent_message_prefix_template.as_deref(),
                    value,
                );
            }
            GlobalSettingsField::AgentMessageSuffix => {
                self.agent_message_suffix = changed_optional_literal(
                    snapshot.config.agent_message_suffix_template.as_deref(),
                    value,
                );
            }
        }
        Ok(())
    }

    pub(super) fn stage_secret(
        &mut self,
        snapshot: &GlobalSettingsSnapshot,
        field: GlobalSettingsField,
        action: SecretSettingsAction,
    ) -> anyhow::Result<()> {
        let update = match field {
            GlobalSettingsField::NotifyServiceToken => {
                changed_secret(snapshot.config.notify_service_token.as_deref(), action)?
            }
            GlobalSettingsField::DesktopNotifyToken => {
                changed_secret(snapshot.config.desktop_notify_token.as_deref(), action)?
            }
            GlobalSettingsField::AgentBusToken => {
                changed_secret(snapshot.config.agent_bus_token.as_deref(), action)?
            }
            _ => anyhow::bail!("Selected setting is not a secret"),
        };
        match field {
            GlobalSettingsField::NotifyServiceToken => self.notify_service_token = update,
            GlobalSettingsField::DesktopNotifyToken => self.desktop_notify_token = update,
            GlobalSettingsField::AgentBusToken => self.agent_bus_token = update,
            _ => unreachable!("secret field checked above"),
        }
        Ok(())
    }

    pub(super) fn value(
        &self,
        snapshot: &GlobalSettingsSnapshot,
        field: GlobalSettingsField,
    ) -> String {
        match field {
            GlobalSettingsField::ManagedSessions => enabled(
                self.managed_sessions
                    .unwrap_or(snapshot.config.session.enabled),
            ),
            GlobalSettingsField::DockerSudo => {
                enabled(self.docker_sudo.unwrap_or(snapshot.config.docker_use_sudo))
            }
            GlobalSettingsField::DefaultProfile => optional_owned(effective_optional_value(
                snapshot.config.default_profile.as_ref(),
                &self.default_profile,
            )),
            GlobalSettingsField::DefaultProfileDirectLaunch => enabled(
                self.default_profile_direct_launch
                    .unwrap_or(snapshot.config.default_profile_direct_launch),
            ),
            GlobalSettingsField::ProxyEnabled => enabled(self.proxy_enabled.unwrap_or_else(|| {
                snapshot
                    .config
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.enabled)
            })),
            GlobalSettingsField::ProxyUrl => optional_owned(effective_optional_string(
                snapshot
                    .config
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.url.as_deref()),
                &self.proxy_url,
            )),
            GlobalSettingsField::ProxyNoProxy => optional_owned(effective_optional_string(
                snapshot
                    .config
                    .proxy
                    .as_ref()
                    .and_then(|proxy| proxy.no_proxy.as_deref()),
                &self.proxy_no_proxy,
            )),
            GlobalSettingsField::ProxyForceHttp => {
                enabled(self.proxy_force_http.unwrap_or_else(|| {
                    snapshot
                        .config
                        .proxy
                        .as_ref()
                        .map(|proxy| proxy.force_http_transport)
                        .unwrap_or(true)
                }))
            }
            GlobalSettingsField::NotifyServiceUrl => optional_owned(effective_optional_string(
                snapshot.config.notify_service_url.as_deref(),
                &self.notify_service_url,
            )),
            GlobalSettingsField::NotifyServiceToken => secret_update(
                snapshot.config.notify_service_token.as_deref(),
                &self.notify_service_token,
            ),
            GlobalSettingsField::NotifyIdleTimeout => optional_number(effective_optional_value(
                snapshot.config.notify_service_idle_timeout_secs.as_ref(),
                &self.notify_idle_timeout,
            )),
            GlobalSettingsField::NotifyComposerTimeout => {
                optional_number(effective_optional_value(
                    snapshot
                        .config
                        .notify_service_composer_idle_timeout_secs
                        .as_ref(),
                    &self.notify_composer_timeout,
                ))
            }
            GlobalSettingsField::NotifyApprovalTimeout => {
                optional_number(effective_optional_value(
                    snapshot
                        .config
                        .notify_service_approval_timeout_secs
                        .as_ref(),
                    &self.notify_approval_timeout,
                ))
            }
            GlobalSettingsField::NotifyStartupTimeout => optional_number(effective_optional_value(
                snapshot
                    .config
                    .notify_service_startup_idle_timeout_secs
                    .as_ref(),
                &self.notify_startup_timeout,
            )),
            GlobalSettingsField::NotifyEvents => effective_optional_value(
                snapshot.config.notify_service_events.as_ref(),
                &self.notify_events,
            )
            .as_deref()
            .map(csv_or_dash)
            .unwrap_or_else(|| "-".to_string()),
            GlobalSettingsField::NotifyMessageContent => optional_owned(effective_optional_value(
                snapshot.config.notify_service_user_message_content.as_ref(),
                &self.notify_message_content,
            )),
            GlobalSettingsField::NotifyPreviewChars => optional_number(effective_optional_value(
                snapshot
                    .config
                    .notify_service_user_message_preview_chars
                    .as_ref(),
                &self.notify_preview_chars,
            )),
            GlobalSettingsField::DesktopNotifyEnabled => enabled(
                self.desktop_notify_enabled
                    .unwrap_or(snapshot.config.desktop_notify_enabled),
            ),
            GlobalSettingsField::DesktopNotifyPort => match &self.desktop_notify_port {
                ConfigValueUpdate::Unchanged => snapshot
                    .config
                    .desktop_notify_port
                    .unwrap_or(DEFAULT_DESKTOP_NOTIFY_PORT),
                ConfigValueUpdate::Set(port) => *port,
                ConfigValueUpdate::Clear => DEFAULT_DESKTOP_NOTIFY_PORT,
            }
            .to_string(),
            GlobalSettingsField::DesktopNotifyToken => secret_update(
                snapshot.config.desktop_notify_token.as_deref(),
                &self.desktop_notify_token,
            ),
            GlobalSettingsField::RateLimitThresholdWarning => {
                optional_owned(effective_optional_value(
                    snapshot.config.rate_limit_threshold_warning_mode.as_ref(),
                    &self.rate_limit_threshold_warning,
                ))
            }
            GlobalSettingsField::RateLimitModelNudge => optional_owned(effective_optional_value(
                snapshot.config.rate_limit_model_nudge_mode.as_ref(),
                &self.rate_limit_model_nudge,
            )),
            GlobalSettingsField::AgentBusEnabled => enabled(
                self.agent_bus_enabled
                    .unwrap_or(snapshot.config.agent_bus_enabled),
            ),
            GlobalSettingsField::AgentBusPort => match &self.agent_bus_port {
                ConfigValueUpdate::Unchanged => snapshot
                    .config
                    .agent_bus_port
                    .unwrap_or(DEFAULT_AGENT_BUS_PORT),
                ConfigValueUpdate::Set(port) => *port,
                ConfigValueUpdate::Clear => DEFAULT_AGENT_BUS_PORT,
            }
            .to_string(),
            GlobalSettingsField::AgentBusToken => secret_update(
                snapshot.config.agent_bus_token.as_deref(),
                &self.agent_bus_token,
            ),
            GlobalSettingsField::AgentMessagePrefix => optional_literal(
                effective_optional_string(
                    snapshot.config.agent_message_prefix_template.as_deref(),
                    &self.agent_message_prefix,
                )
                .as_deref(),
            ),
            GlobalSettingsField::AgentMessageSuffix => optional_literal(
                effective_optional_string(
                    snapshot.config.agent_message_suffix_template.as_deref(),
                    &self.agent_message_suffix,
                )
                .as_deref(),
            ),
        }
    }

    pub(super) fn field_is_dirty(&self, field: GlobalSettingsField) -> bool {
        match field {
            GlobalSettingsField::ManagedSessions => self.managed_sessions.is_some(),
            GlobalSettingsField::DockerSudo => self.docker_sudo.is_some(),
            GlobalSettingsField::DefaultProfile => {
                !matches!(self.default_profile, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::DefaultProfileDirectLaunch => {
                self.default_profile_direct_launch.is_some()
            }
            GlobalSettingsField::ProxyEnabled => self.proxy_enabled.is_some(),
            GlobalSettingsField::ProxyUrl => {
                !matches!(self.proxy_url, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::ProxyNoProxy => {
                !matches!(self.proxy_no_proxy, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::ProxyForceHttp => self.proxy_force_http.is_some(),
            GlobalSettingsField::NotifyServiceUrl => {
                !matches!(self.notify_service_url, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyServiceToken => {
                !matches!(self.notify_service_token, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyIdleTimeout => {
                !matches!(self.notify_idle_timeout, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyComposerTimeout => {
                !matches!(self.notify_composer_timeout, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyApprovalTimeout => {
                !matches!(self.notify_approval_timeout, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyStartupTimeout => {
                !matches!(self.notify_startup_timeout, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyEvents => {
                !matches!(self.notify_events, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyMessageContent => {
                !matches!(self.notify_message_content, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::NotifyPreviewChars => {
                !matches!(self.notify_preview_chars, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::DesktopNotifyEnabled => self.desktop_notify_enabled.is_some(),
            GlobalSettingsField::DesktopNotifyPort => {
                !matches!(self.desktop_notify_port, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::DesktopNotifyToken => {
                !matches!(self.desktop_notify_token, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::RateLimitThresholdWarning => !matches!(
                self.rate_limit_threshold_warning,
                ConfigValueUpdate::Unchanged
            ),
            GlobalSettingsField::RateLimitModelNudge => {
                !matches!(self.rate_limit_model_nudge, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::AgentBusEnabled => self.agent_bus_enabled.is_some(),
            GlobalSettingsField::AgentBusPort => {
                !matches!(self.agent_bus_port, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::AgentBusToken => {
                !matches!(self.agent_bus_token, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::AgentMessagePrefix => {
                !matches!(self.agent_message_prefix, ConfigValueUpdate::Unchanged)
            }
            GlobalSettingsField::AgentMessageSuffix => {
                !matches!(self.agent_message_suffix, ConfigValueUpdate::Unchanged)
            }
        }
    }

    pub(super) fn dirty_count(&self) -> usize {
        [
            GlobalSettingsField::ManagedSessions,
            GlobalSettingsField::DockerSudo,
            GlobalSettingsField::DefaultProfile,
            GlobalSettingsField::DefaultProfileDirectLaunch,
            GlobalSettingsField::ProxyEnabled,
            GlobalSettingsField::ProxyUrl,
            GlobalSettingsField::ProxyNoProxy,
            GlobalSettingsField::ProxyForceHttp,
            GlobalSettingsField::NotifyServiceUrl,
            GlobalSettingsField::NotifyServiceToken,
            GlobalSettingsField::NotifyIdleTimeout,
            GlobalSettingsField::NotifyComposerTimeout,
            GlobalSettingsField::NotifyApprovalTimeout,
            GlobalSettingsField::NotifyStartupTimeout,
            GlobalSettingsField::NotifyEvents,
            GlobalSettingsField::NotifyMessageContent,
            GlobalSettingsField::NotifyPreviewChars,
            GlobalSettingsField::DesktopNotifyEnabled,
            GlobalSettingsField::DesktopNotifyPort,
            GlobalSettingsField::DesktopNotifyToken,
            GlobalSettingsField::RateLimitThresholdWarning,
            GlobalSettingsField::RateLimitModelNudge,
            GlobalSettingsField::AgentBusEnabled,
            GlobalSettingsField::AgentBusPort,
            GlobalSettingsField::AgentBusToken,
            GlobalSettingsField::AgentMessagePrefix,
            GlobalSettingsField::AgentMessageSuffix,
        ]
        .into_iter()
        .filter(|field| self.field_is_dirty(*field))
        .count()
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.dirty_count() != 0
    }

    pub(super) fn patch(&self, current: &CodezConfig) -> anyhow::Result<GlobalConfigPatch> {
        let proxy_is_dirty = [
            GlobalSettingsField::ProxyEnabled,
            GlobalSettingsField::ProxyUrl,
            GlobalSettingsField::ProxyNoProxy,
            GlobalSettingsField::ProxyForceHttp,
        ]
        .into_iter()
        .any(|field| self.field_is_dirty(field));
        let proxy = if !proxy_is_dirty {
            ConfigValueUpdate::Unchanged
        } else {
            let current_proxy = current.proxy.as_ref();
            let enabled = self
                .proxy_enabled
                .unwrap_or_else(|| current_proxy.is_some_and(|proxy| proxy.enabled));
            if !enabled {
                if !matches!(self.proxy_url, ConfigValueUpdate::Unchanged)
                    || !matches!(self.proxy_no_proxy, ConfigValueUpdate::Unchanged)
                    || self.proxy_force_http.is_some()
                {
                    anyhow::bail!("Enable Proxy before saving its URL or transport settings");
                }
                ConfigValueUpdate::Clear
            } else {
                ConfigValueUpdate::Set(proxy_config_from_parts(
                    true,
                    effective_optional_string(
                        current_proxy.and_then(|proxy| proxy.url.as_deref()),
                        &self.proxy_url,
                    ),
                    effective_optional_string(
                        current_proxy.and_then(|proxy| proxy.no_proxy.as_deref()),
                        &self.proxy_no_proxy,
                    ),
                    self.proxy_force_http.unwrap_or_else(|| {
                        current_proxy
                            .map(|proxy| proxy.force_http_transport)
                            .unwrap_or(true)
                    }),
                )?)
            }
        };
        Ok(GlobalConfigPatch {
            docker_use_sudo: self.docker_sudo,
            session_enabled: self.managed_sessions,
            default_profile: self.default_profile.clone(),
            default_profile_direct_launch: self.default_profile_direct_launch,
            proxy,
            notify_service_url: self.notify_service_url.clone(),
            notify_service_token: self.notify_service_token.clone(),
            notify_service_idle_timeout_secs: self.notify_idle_timeout.clone(),
            notify_service_composer_idle_timeout_secs: self.notify_composer_timeout.clone(),
            notify_service_approval_timeout_secs: self.notify_approval_timeout.clone(),
            notify_service_startup_idle_timeout_secs: self.notify_startup_timeout.clone(),
            notify_service_events: self.notify_events.clone(),
            notify_service_user_message_content: self.notify_message_content.clone(),
            notify_service_user_message_preview_chars: self.notify_preview_chars.clone(),
            rate_limit_threshold_warning_mode: self.rate_limit_threshold_warning.clone(),
            rate_limit_model_nudge_mode: self.rate_limit_model_nudge.clone(),
            desktop_notify_enabled: self.desktop_notify_enabled,
            desktop_notify_port: self.desktop_notify_port.clone(),
            desktop_notify_token: self.desktop_notify_token.clone(),
            agent_bus_enabled: self.agent_bus_enabled,
            agent_bus_port: self.agent_bus_port.clone(),
            agent_bus_token: self.agent_bus_token.clone(),
            agent_message_prefix_template: self.agent_message_prefix.clone(),
            agent_message_suffix_template: self.agent_message_suffix.clone(),
        })
    }

    pub(super) fn default_profile_is_dirty(&self) -> bool {
        !matches!(self.default_profile, ConfigValueUpdate::Unchanged)
    }

    pub(super) fn validate_profile_catalog(&self, profile_names: &[String]) -> anyhow::Result<()> {
        if let ConfigValueUpdate::Set(name) = &self.default_profile {
            if !profile_names.iter().any(|candidate| candidate == name) {
                anyhow::bail!("Profile is no longer available: {name}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn global_setting_categories(config: &CodezConfig) -> Vec<SessionTuiSettingCategory> {
    GlobalSettingsSnapshot::from_config(config).categories(&GlobalSettingsDraft::default())
}

fn parse_enabled(value: Option<&str>) -> anyhow::Result<bool> {
    match value.map(str::trim) {
        Some("enabled") => Ok(true),
        Some("disabled") => Ok(false),
        Some(value) => anyhow::bail!("Unsupported enabled state: {value}"),
        None => anyhow::bail!("Enabled state cannot be cleared"),
    }
}

fn changed_bool(current: bool, next: bool) -> Option<bool> {
    (current != next).then_some(next)
}

fn changed_optional_value<T: PartialEq>(
    current: Option<&T>,
    next: Option<T>,
) -> ConfigValueUpdate<T> {
    match next {
        Some(next) if current == Some(&next) => ConfigValueUpdate::Unchanged,
        Some(next) => ConfigValueUpdate::Set(next),
        None if current.is_none() => ConfigValueUpdate::Unchanged,
        None => ConfigValueUpdate::Clear,
    }
}

fn changed_optional_string(
    current: Option<&str>,
    next: Option<String>,
) -> ConfigValueUpdate<String> {
    let next = next
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "-");
    match next {
        Some(next) if current == Some(next.as_str()) => ConfigValueUpdate::Unchanged,
        Some(next) => ConfigValueUpdate::Set(next),
        None if current.is_none() => ConfigValueUpdate::Unchanged,
        None => ConfigValueUpdate::Clear,
    }
}

fn changed_optional_literal(
    current: Option<&str>,
    next: Option<String>,
) -> ConfigValueUpdate<String> {
    let next = next.filter(|value| !value.is_empty() && value != "-");
    match next {
        Some(next) if current == Some(next.as_str()) => ConfigValueUpdate::Unchanged,
        Some(next) => ConfigValueUpdate::Set(next),
        None if current.is_none() => ConfigValueUpdate::Unchanged,
        None => ConfigValueUpdate::Clear,
    }
}

fn effective_optional_string(
    current: Option<&str>,
    update: &ConfigValueUpdate<String>,
) -> Option<String> {
    match update {
        ConfigValueUpdate::Unchanged => current.map(str::to_string),
        ConfigValueUpdate::Set(value) => Some(value.clone()),
        ConfigValueUpdate::Clear => None,
    }
}

fn effective_optional_value<T: Clone>(
    current: Option<&T>,
    update: &ConfigValueUpdate<T>,
) -> Option<T> {
    match update {
        ConfigValueUpdate::Unchanged => current.cloned(),
        ConfigValueUpdate::Set(value) => Some(value.clone()),
        ConfigValueUpdate::Clear => None,
    }
}

fn changed_secret(
    current: Option<&str>,
    action: SecretSettingsAction,
) -> anyhow::Result<ConfigValueUpdate<String>> {
    match action {
        SecretSettingsAction::Keep => Ok(ConfigValueUpdate::Unchanged),
        SecretSettingsAction::Clear if current.is_none() => Ok(ConfigValueUpdate::Unchanged),
        SecretSettingsAction::Clear => Ok(ConfigValueUpdate::Clear),
        SecretSettingsAction::Replace(value) => {
            let value = value.trim();
            if value.is_empty() {
                anyhow::bail!("Replacement secret cannot be empty");
            }
            if current == Some(value) {
                Ok(ConfigValueUpdate::Unchanged)
            } else {
                Ok(ConfigValueUpdate::Set(value.to_string()))
            }
        }
    }
}

fn secret_update(current: Option<&str>, update: &ConfigValueUpdate<String>) -> String {
    match update {
        ConfigValueUpdate::Unchanged => secret(current),
        ConfigValueUpdate::Set(_) => "(replace staged)".to_string(),
        ConfigValueUpdate::Clear => "(clear staged)".to_string(),
    }
}

fn optional_owned(value: Option<String>) -> String {
    optional(value.as_deref())
}

fn nonempty(value: &str) -> String {
    optional(Some(value))
}

fn optional(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn optional_literal(value: Option<&str>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn enabled(value: bool) -> String {
    if value { "enabled" } else { "disabled" }.to_string()
}

fn secret(value: Option<&str>) -> String {
    if value.is_some_and(|value| !value.is_empty()) {
        "(set)".to_string()
    } else {
        "-".to_string()
    }
}

fn optional_number(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn csv_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cutex::agent_bus::model::AgentRegistrationClass;
    use cutex::profiles::model::ProxyConfig;
    use cutex::session::model::{CutexSessionQuickActionMode, CutexSessionRuntimeBackend};

    fn flattened(categories: &[SessionTuiSettingCategory]) -> String {
        categories
            .iter()
            .flat_map(|category| {
                category
                    .options
                    .iter()
                    .map(|option| format!("{}:{}={}", category.label, option.label, option.value))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn agent_projection_groups_durable_settings_without_runtime_identity() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.settings".to_string(),
            Some("019e-settings".to_string()),
            "host-a".to_string(),
            "/tmp/original".to_string(),
            Some("aemeath".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("settings-agent".to_string());
        record.managed_cwd = Some("/tmp/managed".to_string());
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.permission_defaults = Some("workspace".to_string());
        record.model_defaults = Some("gpt-test".to_string());
        record.agent_groups = vec!["cutex".to_string(), "waveline".to_string()];
        record.quick_action = CutexSessionQuickActionMode::Pinned;
        record.current_runtime_agent_id = Some("volatile-runtime-id".to_string());

        let categories = SessionSettingsSnapshot::from_record(&record)
            .categories(&SessionSettingsDraft::default());
        assert_eq!(
            categories
                .iter()
                .map(|category| category.label)
                .collect::<Vec<_>>(),
            vec!["Identity", "Permissions", "Launch", "Routing", "Details",]
        );
        let settings = flattened(&categories);

        assert!(settings.contains("Identity:Agent name=settings-agent"));
        assert!(settings.contains("Launch:Runtime backend=alden"));
        assert!(settings.contains("Launch:Working directory=/tmp/managed"));
        assert!(settings.contains("Permissions:Model=gpt-test"));
        assert!(settings.contains("Routing:Message groups=cutex, waveline"));
        assert!(settings.contains("Details:Management=adopt"));
        assert!(settings.contains("Details:Cutex session=cutex.settings"));
        assert!(!settings.contains("volatile-runtime-id"));
        assert_eq!(
            categories
                .iter()
                .flat_map(|category| category.options.iter())
                .filter(|option| option.field.is_some())
                .count(),
            12
        );
    }

    #[test]
    fn management_projection_offers_adopt_for_local_and_unmanage_for_persistent() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.management".to_string(),
            Some("019e-management".to_string()),
            "host-a".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("record");

        let local = SessionSettingsSnapshot::from_record(&record)
            .categories(&SessionSettingsDraft::default())
            .into_iter()
            .find(|category| category.label == "Details")
            .and_then(|category| {
                category
                    .options
                    .into_iter()
                    .find(|option| option.label == "Management")
            })
            .expect("local management command");
        assert_eq!(local.value, "adopt");
        assert_eq!(local.command, Some(SessionSettingsCommand::Adopt));
        assert_eq!(local.field, None);

        record.registration_class = AgentRegistrationClass::Persistent;
        let managed = SessionSettingsSnapshot::from_record(&record)
            .categories(&SessionSettingsDraft::default())
            .into_iter()
            .find(|category| category.label == "Details")
            .and_then(|category| {
                category
                    .options
                    .into_iter()
                    .find(|option| option.label == "Management")
            })
            .expect("managed management command");
        assert_eq!(managed.value, "unmanage");
        assert_eq!(managed.command, Some(SessionSettingsCommand::Unmanage));
        assert_eq!(managed.field, None);
    }

    #[test]
    fn permission_draft_projects_effective_values_and_only_changed_patch_fields() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.draft".to_string(),
            Some("019e-draft".to_string()),
            "host-a".to_string(),
            "/tmp/draft".to_string(),
            None,
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        record.permission_defaults = Some("workspace".to_string());
        record.approval_policy = Some("on-request".to_string());
        record.model_defaults = Some("gpt-old".to_string());
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                SessionSettingsField::PermissionPreset,
                Some("full-access".to_string()),
            )
            .expect("stage permission");
        draft
            .stage(&snapshot, SessionSettingsField::ApprovalPolicy, None)
            .expect("stage approval");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::Model,
                Some("gpt-next".to_string()),
            )
            .expect("stage model");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::SandboxMode,
                Some("danger-full-access".to_string()),
            )
            .expect("stage sandbox");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::ReasoningEffort,
                Some("high".to_string()),
            )
            .expect("stage reasoning");

        assert_eq!(draft.dirty_count(), 5);
        assert_eq!(
            draft.value(&snapshot, SessionSettingsField::PermissionPreset),
            Some("full-access")
        );
        assert_eq!(
            draft.value(&snapshot, SessionSettingsField::ApprovalPolicy),
            None
        );
        let categories = snapshot.categories(&draft);
        let permissions = categories
            .iter()
            .find(|category| category.label == "Permissions")
            .expect("permissions");
        assert_eq!(permissions.options[0].value, "full-access");
        assert!(permissions.options[0].dirty);
        assert_eq!(permissions.options[1].value, "-");
        assert!(permissions.options[1].dirty);
        assert_eq!(permissions.options[2].value, "danger-full-access");
        assert!(permissions.options[2].dirty);
        assert_eq!(permissions.options[4].value, "high");
        assert!(permissions.options[4].dirty);

        let patch = draft.runtime_defaults_patch();
        assert_eq!(
            patch.permission_defaults,
            CutexSessionValueUpdate::Set("full-access".to_string())
        );
        assert_eq!(patch.approval_policy, CutexSessionValueUpdate::Clear);
        assert_eq!(
            patch.sandbox_mode,
            CutexSessionValueUpdate::Set("danger-full-access".to_string())
        );
        assert_eq!(
            patch.model_defaults,
            CutexSessionValueUpdate::Set("gpt-next".to_string())
        );
        assert_eq!(
            patch.reasoning_defaults,
            CutexSessionValueUpdate::Set("high".to_string())
        );

        draft
            .stage(
                &snapshot,
                SessionSettingsField::Model,
                Some("gpt-old".to_string()),
            )
            .expect("restore model");
        assert_eq!(draft.dirty_count(), 4);
        assert!(!draft.field_is_dirty(SessionSettingsField::Model));
    }

    #[test]
    fn permission_editor_kinds_expose_finite_choices_and_model_text() {
        let record = CutexSessionRecord::new_at(
            "cutex.choices".to_string(),
            None,
            "host-a".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let permission_choices = snapshot.choices(SessionSettingsField::PermissionPreset);
        assert_eq!(
            SessionSettingsField::PermissionPreset.editor_kind(),
            SessionSettingsEditorKind::Choice
        );
        assert_eq!(permission_choices[0].value, None);
        assert!(permission_choices
            .iter()
            .any(|choice| choice.value.as_deref() == Some("full-access")));
        assert_eq!(
            SessionSettingsField::Model.editor_kind(),
            SessionSettingsEditorKind::Text
        );
        assert_eq!(
            SessionSettingsField::RuntimeBackend.editor_kind(),
            SessionSettingsEditorKind::Choice
        );
        assert_eq!(
            snapshot
                .choices(SessionSettingsField::RuntimeBackend)
                .iter()
                .map(|choice| (choice.label.as_str(), choice.value.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("Host", Some("host")),
                ("Native foreground", Some("native")),
                ("Docker", Some("docker")),
                ("Cute Alden", Some("alden")),
                ("Future", Some("future")),
            ]
        );
    }

    #[test]
    fn identity_and_launch_draft_reuses_typed_parsers_and_updates_effective_cwd() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.launch.draft".to_string(),
            None,
            "host-a".to_string(),
            "/tmp/original".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("old agent".to_string());
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record.managed_cwd = Some("/tmp/managed".to_string());
        record.default_cli_args = vec!["--no-alt-screen".to_string()];
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                SessionSettingsField::AgentName,
                Some("  renamed agent  ".to_string()),
            )
            .expect("stage name");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::RuntimeBackend,
                Some("native".to_string()),
            )
            .expect("stage backend");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::ManagedCwd,
                Some(r"  D:\Projects\example-project  ".to_string()),
            )
            .expect("stage Windows cwd");
        draft
            .stage(
                &snapshot,
                SessionSettingsField::ExtraCliArgs,
                Some("--model 'gpt next' --no-alt-screen".to_string()),
            )
            .expect("stage CLI args");

        assert_eq!(draft.dirty_count(), 4);
        assert_eq!(draft.agent_name(), Some("renamed agent"));
        assert_eq!(
            draft.value(&snapshot, SessionSettingsField::ManagedCwd),
            Some(r"D:\Projects\example-project")
        );
        assert_eq!(
            draft.effective_working_directory(&snapshot),
            r"D:\Projects\example-project"
        );
        assert_eq!(
            draft.value(&snapshot, SessionSettingsField::ExtraCliArgs),
            Some("'--model' 'gpt next' '--no-alt-screen'")
        );
        let patch = draft.runtime_defaults_patch();
        assert_eq!(
            patch.runtime_backend,
            Some(CutexSessionRuntimeBackend::HostForeground)
        );
        assert_eq!(
            patch.managed_cwd,
            CutexSessionValueUpdate::Set(r"D:\Projects\example-project".to_string())
        );
        assert_eq!(
            patch.default_cli_args,
            Some(vec![
                "--model".to_string(),
                "gpt next".to_string(),
                "--no-alt-screen".to_string(),
            ])
        );
        assert!(draft.launch_actions_are_dirty());
    }

    #[test]
    fn launch_clear_and_invalid_values_preserve_the_last_valid_draft() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.launch.validation".to_string(),
            None,
            "host-a".to_string(),
            "/tmp/original".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("record");
        record.display_name_hint = Some("old agent".to_string());
        record.managed_cwd = Some("/tmp/managed".to_string());
        record.default_cli_args = vec!["--no-alt-screen".to_string()];
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();

        draft
            .stage(&snapshot, SessionSettingsField::ManagedCwd, None)
            .expect("clear cwd");
        draft
            .stage(&snapshot, SessionSettingsField::ExtraCliArgs, None)
            .expect("clear args");
        assert_eq!(
            draft.effective_working_directory(&snapshot),
            "/tmp/original"
        );
        assert_eq!(
            draft.runtime_defaults_patch().managed_cwd,
            CutexSessionValueUpdate::Clear
        );
        assert_eq!(
            draft.runtime_defaults_patch().default_cli_args,
            Some(vec![])
        );
        let valid = draft.clone();

        assert!(draft
            .stage(&snapshot, SessionSettingsField::AgentName, None)
            .is_err());
        assert!(draft
            .stage(
                &snapshot,
                SessionSettingsField::RuntimeBackend,
                Some("unknown".to_string()),
            )
            .is_err());
        assert!(draft
            .stage(
                &snapshot,
                SessionSettingsField::ManagedCwd,
                Some("-".to_string()),
            )
            .is_err());
        assert!(draft
            .stage(
                &snapshot,
                SessionSettingsField::ExtraCliArgs,
                Some("--model 'unterminated".to_string()),
            )
            .is_err());
        assert_eq!(draft, valid);
    }

    #[test]
    fn routing_projection_only_exposes_workbench_visibility_for_managed_sessions() {
        let mut local = CutexSessionRecord::new_at(
            "cutex.routing.local".to_string(),
            None,
            "host-a".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("local record");
        local.agent_groups = vec!["cutex".to_string()];
        let local_snapshot = SessionSettingsSnapshot::from_record(&local);
        let local_routing = local_snapshot
            .categories(&SessionSettingsDraft::default())
            .into_iter()
            .find(|category| category.label == "Routing")
            .expect("local routing");

        assert_eq!(
            local_routing
                .options
                .iter()
                .map(|option| option.field)
                .collect::<Vec<_>>(),
            [
                Some(SessionSettingsField::AgentGroups),
                None,
                Some(SessionSettingsField::QuickAction),
            ]
        );

        local.registration_class = AgentRegistrationClass::Persistent;
        let managed_snapshot = SessionSettingsSnapshot::from_record(&local);
        let managed_routing = managed_snapshot
            .categories(&SessionSettingsDraft::default())
            .into_iter()
            .find(|category| category.label == "Routing")
            .expect("managed routing");
        assert_eq!(
            managed_routing
                .options
                .iter()
                .map(|option| option.field)
                .collect::<Vec<_>>(),
            [
                Some(SessionSettingsField::AgentGroups),
                Some(SessionSettingsField::WorkbenchVisibility),
                Some(SessionSettingsField::QuickAction),
            ]
        );
    }

    #[test]
    fn routing_draft_normalizes_tags_and_projects_one_typed_patch() {
        let mut record = CutexSessionRecord::new_at(
            "cutex.routing.draft".to_string(),
            None,
            "host-a".to_string(),
            "/tmp".to_string(),
            None,
            "2026-08-06T00:00:00Z".to_string(),
        )
        .expect("record");
        record.registration_class = AgentRegistrationClass::Persistent;
        record.agent_groups = vec!["cutex".to_string()];
        let snapshot = SessionSettingsSnapshot::from_record(&record);
        let mut draft = SessionSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                SessionSettingsField::AgentGroups,
                Some(" waveline, cutex  waveline ".to_string()),
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

        assert_eq!(draft.dirty_count(), 3);
        assert_eq!(
            draft.value(&snapshot, SessionSettingsField::AgentGroups),
            Some("waveline, cutex")
        );
        assert_eq!(
            draft.routing_patch(),
            CutexSessionRoutingPatch {
                agent_groups: Some(vec!["waveline".to_string(), "cutex".to_string()]),
                exposed_to_backend: Some(true),
                quick_action: Some(CutexSessionQuickActionMode::Pinned),
            }
        );
        assert!(!draft.runtime_defaults_are_dirty());

        let original_patch = draft.routing_patch();
        let error = draft
            .stage(
                &snapshot,
                SessionSettingsField::AgentGroups,
                Some(" ,  ".to_string()),
            )
            .expect_err("empty groups must be rejected");
        assert!(error.to_string().contains("At least one"));
        assert_eq!(draft.routing_patch(), original_patch);
    }

    #[test]
    fn profile_editor_uses_the_read_only_catalog_and_stages_outside_runtime_defaults() {
        let record = CutexSessionRecord::new_at(
            "cutex.profile".to_string(),
            None,
            "host-a".to_string(),
            "/tmp".to_string(),
            Some("alpha".to_string()),
            "2026-08-05T00:00:00Z".to_string(),
        )
        .expect("record");
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let snapshot = SessionSettingsSnapshot::from_record_with_profiles(&record, &profile_names);
        let choices = snapshot.choices(SessionSettingsField::Profile);
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["Follow global default", "alpha", "beta"]
        );
        assert_eq!(choices[0].value, None);

        let mut draft = SessionSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                SessionSettingsField::Profile,
                Some("beta".to_string()),
            )
            .expect("stage profile");

        assert_eq!(draft.dirty_count(), 1);
        assert_eq!(
            draft.profile_update(),
            &CutexSessionValueUpdate::Set("beta".to_string())
        );
        assert!(!draft.runtime_defaults_are_dirty());

        draft
            .stage(&snapshot, SessionSettingsField::Profile, None)
            .expect("stage inherited profile");
        assert_eq!(draft.profile_update(), &CutexSessionValueUpdate::Clear);
        assert_eq!(draft.dirty_count(), 1);
    }

    #[test]
    fn global_projection_is_categorized_and_masks_every_stored_token() {
        let config = CodezConfig {
            notify_service_token: Some("notify-secret".to_string()),
            desktop_notify_token: Some("desktop-secret".to_string()),
            agent_bus_token: Some("bus-secret".to_string()),
            ..CodezConfig::default()
        };

        let categories = global_setting_categories(&config);
        assert_eq!(
            categories
                .iter()
                .map(|category| category.label)
                .collect::<Vec<_>>(),
            vec![
                "General",
                "Defaults",
                "Proxy",
                "Notifications",
                "Rate limits",
                "Agent Bus",
            ]
        );
        let settings = flattened(&categories);

        assert!(settings.contains("Defaults:Default profile="));
        assert!(!settings.contains("Manage profiles"));
        assert_eq!(settings.matches("=(set)").count(), 3);
        assert!(!settings.contains("notify-secret"));
        assert!(!settings.contains("desktop-secret"));
        assert!(!settings.contains("bus-secret"));
        assert!(!settings.contains("runtime_agent_id"));
        assert_eq!(
            categories
                .iter()
                .flat_map(|category| category.options.iter())
                .filter(|option| option.global_field.is_some())
                .count(),
            27
        );
    }

    #[test]
    fn global_profile_defaults_use_catalog_choices_and_one_typed_patch() {
        let config = CodezConfig {
            default_profile: Some("alpha".to_string()),
            ..CodezConfig::default()
        };
        let profile_names = vec!["alpha".to_string(), "beta".to_string()];
        let snapshot = GlobalSettingsSnapshot::from_config_with_profiles(&config, &profile_names);
        let choices = snapshot.choices(GlobalSettingsField::DefaultProfile);
        assert_eq!(
            choices
                .iter()
                .map(|choice| (choice.label.as_str(), choice.value.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("None", None),
                ("alpha", Some("alpha")),
                ("beta", Some("beta")),
            ]
        );

        let mut draft = GlobalSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::DefaultProfile,
                Some("beta".to_string()),
            )
            .expect("stage default profile");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::DefaultProfileDirectLaunch,
                Some("enabled".to_string()),
            )
            .expect("stage direct launch");

        assert_eq!(draft.dirty_count(), 2);
        assert_eq!(
            draft.patch(&config).expect("profile defaults patch"),
            GlobalConfigPatch {
                default_profile: ConfigValueUpdate::Set("beta".to_string()),
                default_profile_direct_launch: Some(true),
                ..GlobalConfigPatch::default()
            }
        );
        assert!(draft.validate_profile_catalog(&profile_names).is_ok());
        assert!(draft
            .validate_profile_catalog(&["alpha".to_string()])
            .expect_err("stale profile must fail")
            .to_string()
            .contains("no longer available"));
    }

    #[test]
    fn stale_configured_default_can_be_preserved_or_cleared_without_becoming_a_choice() {
        let config = CodezConfig {
            default_profile: Some("removed-profile".to_string()),
            ..CodezConfig::default()
        };
        let snapshot =
            GlobalSettingsSnapshot::from_config_with_profiles(&config, &["available".to_string()]);
        let mut draft = GlobalSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                GlobalSettingsField::DefaultProfile,
                Some("removed-profile".to_string()),
            )
            .expect("preserve stale current value");
        assert_eq!(draft.dirty_count(), 0);
        assert!(!snapshot
            .choices(GlobalSettingsField::DefaultProfile)
            .iter()
            .any(|choice| choice.value.as_deref() == Some("removed-profile")));

        draft
            .stage(&snapshot, GlobalSettingsField::DefaultProfile, None)
            .expect("clear stale default");
        assert_eq!(draft.dirty_count(), 1);
        assert_eq!(
            draft.patch(&config).expect("clear patch").default_profile,
            ConfigValueUpdate::Clear
        );
    }

    #[test]
    fn global_general_proxy_draft_builds_one_typed_patch() {
        let mut config = CodezConfig::default();
        config.proxy = Some(ProxyConfig {
            enabled: true,
            url: Some("http://127.0.0.1:7890".to_string()),
            no_proxy: None,
            force_http_transport: true,
        });
        let snapshot = GlobalSettingsSnapshot::from_config(&config);
        let mut draft = GlobalSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ManagedSessions,
                Some("enabled".to_string()),
            )
            .expect("stage sessions");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::DockerSudo,
                Some("enabled".to_string()),
            )
            .expect("stage sudo");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyUrl,
                Some("socks5h://127.0.0.1:7891".to_string()),
            )
            .expect("stage URL");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyNoProxy,
                Some("localhost,127.0.0.1".to_string()),
            )
            .expect("stage NO_PROXY");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyForceHttp,
                Some("disabled".to_string()),
            )
            .expect("stage transport");

        assert_eq!(draft.dirty_count(), 5);
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::ProxyUrl),
            "socks5h://127.0.0.1:7891"
        );
        assert_eq!(
            draft.patch(&config).expect("valid patch"),
            GlobalConfigPatch {
                docker_use_sudo: Some(true),
                session_enabled: Some(true),
                proxy: ConfigValueUpdate::Set(ProxyConfig {
                    enabled: true,
                    url: Some("socks5h://127.0.0.1:7891".to_string()),
                    no_proxy: Some("localhost,127.0.0.1".to_string()),
                    force_http_transport: false,
                }),
                ..GlobalConfigPatch::default()
            }
        );
        let projected = flattened(&snapshot.categories(&draft));
        assert!(projected.contains("General:Managed sessions=enabled"));
        assert!(projected.contains("Proxy:Force HTTP transport=disabled"));
    }

    #[test]
    fn global_proxy_validation_keeps_the_complete_draft_for_correction() {
        let config = CodezConfig::default();
        let snapshot = GlobalSettingsSnapshot::from_config(&config);
        let mut draft = GlobalSettingsDraft::default();
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyEnabled,
                Some("enabled".to_string()),
            )
            .expect("stage enabled");
        assert!(draft.patch(&config).is_err());
        assert_eq!(draft.dirty_count(), 1);

        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyUrl,
                Some("ftp://127.0.0.1:21".to_string()),
            )
            .expect("stage invalid URL for apply validation");
        let valid_draft = draft.clone();
        let error = draft.patch(&config).expect_err("scheme must be rejected");
        assert!(error.to_string().contains("Unsupported proxy scheme"));
        assert_eq!(draft, valid_draft);

        draft
            .stage(
                &snapshot,
                GlobalSettingsField::ProxyEnabled,
                Some("disabled".to_string()),
            )
            .expect("disable proxy");
        assert_eq!(draft.dirty_count(), 0);
    }

    #[test]
    fn global_notification_rate_draft_builds_one_masked_typed_patch() {
        let mut config = CodezConfig::default();
        config.notify_service_token = Some("old-notify-secret".to_string());
        config.notify_service_composer_idle_timeout_secs = Some(30);
        config.desktop_notify_token = Some("old-desktop-secret".to_string());
        let snapshot = GlobalSettingsSnapshot::from_config(&config);
        let mut draft = GlobalSettingsDraft::default();

        for (field, value) in [
            (
                GlobalSettingsField::NotifyServiceUrl,
                Some("https://notify.test/push"),
            ),
            (GlobalSettingsField::NotifyIdleTimeout, Some("90")),
            (GlobalSettingsField::NotifyComposerTimeout, None),
            (GlobalSettingsField::NotifyApprovalTimeout, Some("60")),
            (GlobalSettingsField::NotifyStartupTimeout, Some("120")),
            (
                GlobalSettingsField::NotifyEvents,
                Some("turn-completed approval_requested"),
            ),
            (GlobalSettingsField::NotifyMessageContent, Some("preview")),
            (GlobalSettingsField::NotifyPreviewChars, Some("80")),
            (GlobalSettingsField::DesktopNotifyEnabled, Some("enabled")),
            (GlobalSettingsField::DesktopNotifyPort, Some("24251")),
            (
                GlobalSettingsField::RateLimitThresholdWarning,
                Some("daily"),
            ),
            (GlobalSettingsField::RateLimitModelNudge, Some("always")),
        ] {
            draft
                .stage(&snapshot, field, value.map(str::to_string))
                .expect("stage notification field");
        }
        draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::NotifyServiceToken,
                SecretSettingsAction::Replace("new-notify-secret".to_string()),
            )
            .expect("replace notify secret");
        draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::DesktopNotifyToken,
                SecretSettingsAction::Clear,
            )
            .expect("clear desktop secret");

        assert_eq!(draft.dirty_count(), 14);
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::NotifyServiceToken),
            "(replace staged)"
        );
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::DesktopNotifyToken),
            "(clear staged)"
        );
        let projected = flattened(&snapshot.categories(&draft));
        assert!(!projected.contains("old-notify-secret"));
        assert!(!projected.contains("new-notify-secret"));
        assert!(!projected.contains("old-desktop-secret"));
        assert_eq!(
            draft.patch(&config).expect("notification patch"),
            GlobalConfigPatch {
                notify_service_url: ConfigValueUpdate::Set("https://notify.test/push".to_string()),
                notify_service_token: ConfigValueUpdate::Set("new-notify-secret".to_string()),
                notify_service_idle_timeout_secs: ConfigValueUpdate::Set(90),
                notify_service_composer_idle_timeout_secs: ConfigValueUpdate::Clear,
                notify_service_approval_timeout_secs: ConfigValueUpdate::Set(60),
                notify_service_startup_idle_timeout_secs: ConfigValueUpdate::Set(120),
                notify_service_events: ConfigValueUpdate::Set(vec![
                    "turn_completed".to_string(),
                    "approval_requested".to_string(),
                ]),
                notify_service_user_message_content: ConfigValueUpdate::Set("preview".to_string()),
                notify_service_user_message_preview_chars: ConfigValueUpdate::Set(80),
                rate_limit_threshold_warning_mode: ConfigValueUpdate::Set("daily".to_string()),
                rate_limit_model_nudge_mode: ConfigValueUpdate::Set("always".to_string()),
                desktop_notify_enabled: Some(true),
                desktop_notify_port: ConfigValueUpdate::Set(24251),
                desktop_notify_token: ConfigValueUpdate::Clear,
                ..GlobalConfigPatch::default()
            }
        );
    }

    #[test]
    fn global_notification_validation_and_secret_keep_leave_draft_coherent() {
        let mut config = CodezConfig::default();
        config.notify_service_token = Some("stored-secret".to_string());
        config.notify_service_user_message_content = Some("legacy-message".to_string());
        config.rate_limit_threshold_warning_mode = Some("legacy-rate".to_string());
        let snapshot = GlobalSettingsSnapshot::from_config(&config);
        let mut draft = GlobalSettingsDraft::default();

        assert!(draft
            .stage(
                &snapshot,
                GlobalSettingsField::NotifyIdleTimeout,
                Some("not-a-number".to_string()),
            )
            .is_err());
        assert!(draft
            .stage(
                &snapshot,
                GlobalSettingsField::DesktopNotifyPort,
                Some("8080".to_string()),
            )
            .is_err());
        assert!(draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::NotifyServiceToken,
                SecretSettingsAction::Replace("  ".to_string()),
            )
            .is_err());
        assert_eq!(draft.dirty_count(), 0);
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::NotifyMessageContent),
            "legacy-message"
        );
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::RateLimitThresholdWarning),
            "legacy-rate"
        );
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::NotifyMessageContent,
                Some("legacy-message".to_string()),
            )
            .expect("keep legacy message mode");
        draft
            .stage(
                &snapshot,
                GlobalSettingsField::RateLimitThresholdWarning,
                Some("legacy-rate".to_string()),
            )
            .expect("keep legacy rate mode");
        assert_eq!(draft.dirty_count(), 0);

        draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::NotifyServiceToken,
                SecretSettingsAction::Replace("replacement".to_string()),
            )
            .expect("stage replacement");
        assert_eq!(draft.dirty_count(), 1);
        draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::NotifyServiceToken,
                SecretSettingsAction::Keep,
            )
            .expect("restore keep");
        assert_eq!(draft.dirty_count(), 0);
    }

    #[test]
    fn global_agent_bus_draft_validates_port_masks_token_and_preserves_templates() {
        let mut config = CodezConfig::default();
        config.agent_bus_enabled = false;
        config.agent_bus_token = Some("stored-bus-secret".to_string());
        config.agent_message_suffix_template = Some("old suffix".to_string());
        let snapshot = GlobalSettingsSnapshot::from_config(&config);
        let mut draft = GlobalSettingsDraft::default();

        draft
            .stage(
                &snapshot,
                GlobalSettingsField::AgentMessagePrefix,
                Some("[message from {from}] ".to_string()),
            )
            .expect("keep exact default prefix");
        assert_eq!(draft.dirty_count(), 0);
        assert!(draft
            .stage(
                &snapshot,
                GlobalSettingsField::AgentBusPort,
                Some("59995".to_string()),
            )
            .is_err());
        assert_eq!(draft.dirty_count(), 0);

        for (field, value) in [
            (GlobalSettingsField::AgentBusEnabled, "enabled"),
            (GlobalSettingsField::AgentBusPort, "24261"),
            (GlobalSettingsField::AgentMessagePrefix, "<{from}> "),
            (GlobalSettingsField::AgentMessageSuffix, " /done"),
        ] {
            draft
                .stage(&snapshot, field, Some(value.to_string()))
                .expect("stage Agent Bus field");
        }
        draft
            .stage_secret(
                &snapshot,
                GlobalSettingsField::AgentBusToken,
                SecretSettingsAction::Replace("new-bus-secret".to_string()),
            )
            .expect("replace Agent Bus token");

        assert_eq!(draft.dirty_count(), 5);
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::AgentBusToken),
            "(replace staged)"
        );
        assert_eq!(
            draft.value(&snapshot, GlobalSettingsField::AgentMessagePrefix),
            "<{from}> "
        );
        assert_eq!(
            draft.patch(&config).expect("Agent Bus patch"),
            GlobalConfigPatch {
                agent_bus_enabled: Some(true),
                agent_bus_port: ConfigValueUpdate::Set(24261),
                agent_bus_token: ConfigValueUpdate::Set("new-bus-secret".to_string()),
                agent_message_prefix_template: ConfigValueUpdate::Set("<{from}> ".to_string()),
                agent_message_suffix_template: ConfigValueUpdate::Set(" /done".to_string()),
                ..GlobalConfigPatch::default()
            }
        );
        let projected = flattened(&snapshot.categories(&draft));
        assert!(!projected.contains("stored-bus-secret"));
        assert!(!projected.contains("new-bus-secret"));
    }
}
