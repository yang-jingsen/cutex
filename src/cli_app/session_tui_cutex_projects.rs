//! Human-authenticated Cutex Project management workspace.

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use anyhow::Context;
use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use cutex::agent_management::{
    AgentActionId, CutexProjectSummary, CutexProjectWorkspace, ProjectAccessRole,
    ProjectAgentChoice, ProjectMemberLifecycle, ProjectPaletteColor, ProjectPresentationInput,
};
use cutex::management::control_plane::{
    HumanManagementOperatorActionRequest, HumanManagementOperatorKind,
    HumanManagementOperatorSchema, HumanManagementPresentationSchema,
    HumanManagementPresentationUpdateRequest, HumanManagementProjectMutationKind,
    HumanManagementProjectMutationRequest, HumanManagementProjectMutationSchema,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use tui_input::Input;
use uuid::Uuid;

use super::management_control_plane::ManagementControlClient;
use super::session_tui::footer_hints;
use super::session_tui_input::{self as input_policy, Command, Gate, Help, LeaveReview};
use super::session_tui_workspace::{primary_panel_tabs, PrimaryPanel, PrimaryPanelOutcome};

const POLL_INTERVAL: Duration = Duration::from_millis(80);
type ProjectTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone)]
struct PresentationEditor {
    display_name: String,
    badge_label: String,
    color: String,
    field: usize,
}

#[derive(Debug, Clone)]
struct ProjectCreateEditor {
    project_id: String,
    display_name: String,
    badge_label: String,
    color: String,
    director: usize,
    field: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProjectView {
    ConfirmImport,
    List,
    Details,
    Editor,
    Create,
    Actions,
    ConfirmProjectMutation,
    ConfirmOperator,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProjectSection {
    Overview,
    Members,
    Operators,
    Appearance,
}

impl ProjectSection {
    const ALL: [Self; 4] = [
        Self::Overview,
        Self::Members,
        Self::Operators,
        Self::Appearance,
    ];

    fn shifted(self, direction: isize) -> Self {
        let index = Self::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0);
        let len = Self::ALL.len() as isize;
        Self::ALL[((index as isize + direction).rem_euclid(len)) as usize]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Members => "Members",
            Self::Operators => "Operators",
            Self::Appearance => "Appearance",
        }
    }
}

#[derive(Debug, Clone)]
struct OperatorTarget {
    cutex_session_id: cutex::role_revision::CutexSessionId,
    name: String,
    lifecycle: ProjectMemberLifecycle,
    operation: HumanManagementOperatorKind,
    repair_action_id: Option<AgentActionId>,
}

#[derive(Debug, Clone)]
struct ProjectMutationTarget {
    label: String,
    operation: HumanManagementProjectMutationKind,
}

#[derive(Debug)]
pub(super) struct CutexProjectsModel {
    help: Option<Help>,
    leave_review: Option<LeaveReview>,
    text_cursors: [Option<usize>; 4],
    pub(super) open_settings_requested: bool,
    durable_candidates: Vec<cutex::agent_management::DurableAgentCandidate>,
    import_request: Option<cutex::agent_management::DurableImportRequest>,
    import_name: Input,
    import_name_focused: bool,
    projects: Vec<CutexProjectSummary>,
    archived_projects: Vec<CutexProjectSummary>,
    available_agents: Vec<ProjectAgentChoice>,
    selected: usize,
    query: Input,
    filter_focused: bool,
    show_archived: bool,
    details: Option<CutexProjectWorkspace>,
    section: ProjectSection,
    operator_selected: usize,
    pending_operator: Option<OperatorTarget>,
    pending_project_mutation: Option<ProjectMutationTarget>,
    action_selected: usize,
    confirm_selected: bool,
    editor: Option<PresentationEditor>,
    create_editor: Option<ProjectCreateEditor>,
    view: ProjectView,
    client: Option<ManagementControlClient>,
    failure: Option<String>,
    notice: Option<String>,
}

impl CutexProjectsModel {
    fn empty_with_failure(error: impl Into<String>) -> Self {
        Self {
            help: None,
            leave_review: None,
            text_cursors: [None; 4],
            open_settings_requested: false,
            durable_candidates: Vec::new(),
            import_request: None,
            import_name: Input::default(),
            import_name_focused: false,
            projects: Vec::new(),
            archived_projects: Vec::new(),
            available_agents: Vec::new(),
            selected: 0,
            query: Input::default(),
            filter_focused: false,
            show_archived: false,
            details: None,
            section: ProjectSection::Overview,
            operator_selected: 0,
            pending_operator: None,
            pending_project_mutation: None,
            action_selected: 0,
            confirm_selected: false,
            editor: None,
            create_editor: None,
            view: ProjectView::List,
            client: None,
            failure: Some(error.into()),
            notice: None,
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.value().trim().to_lowercase();
        self.projects
            .iter()
            .enumerate()
            .filter_map(|(index, project)| {
                ((!matches!(
                    project.lifecycle,
                    cutex::agent_management::ProjectLifecycle::Archived
                ) || self.show_archived)
                    && (query.is_empty()
                        || project
                            .presentation
                            .display_name
                            .to_lowercase()
                            .contains(&query)
                        || project.project_id.as_str().to_lowercase().contains(&query)
                        || project
                            .presentation
                            .badge_label
                            .to_lowercase()
                            .contains(&query)
                        || project
                            .director_name
                            .as_deref()
                            .unwrap_or(project.director_cutex_session_id.as_str())
                            .to_lowercase()
                            .contains(&query)))
                .then_some(index)
            })
            .collect()
    }

    fn retain_selection(&mut self) {
        self.selected = self
            .selected
            .min(self.visible_indices().len().saturating_sub(1));
    }

    fn selected_project(&self) -> Option<&CutexProjectSummary> {
        let index = *self.visible_indices().get(self.selected)?;
        self.projects.get(index)
    }

    fn operator_targets(&self) -> Vec<OperatorTarget> {
        let Some(details) = self.details.as_ref() else {
            return Vec::new();
        };
        let mut targets = details
            .agent_operators
            .iter()
            .map(|operator| OperatorTarget {
                cutex_session_id: operator.member.agent.cutex_session_id.clone(),
                name: operator.member.agent.spec.name.clone(),
                lifecycle: operator.member.lifecycle,
                operation: HumanManagementOperatorKind::Revoke,
                repair_action_id: None,
            })
            .chain(details.active_agents.iter().map(|member| {
                let repair_action_id = details
                    .legacy_operator_repair_candidates
                    .iter()
                    .find(|candidate| {
                        candidate.predecessor_cutex_session_id == member.agent.cutex_session_id
                    })
                    .map(|candidate| candidate.rotation_action_id.clone());
                OperatorTarget {
                    cutex_session_id: member.agent.cutex_session_id.clone(),
                    name: member.agent.spec.name.clone(),
                    lifecycle: member.lifecycle,
                    operation: HumanManagementOperatorKind::Grant,
                    repair_action_id,
                }
            }))
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| {
            operation_rank(left.operation)
                .cmp(&operation_rank(right.operation))
                .then_with(|| left.cutex_session_id.cmp(&right.cutex_session_id))
        });
        targets
    }

    fn begin_editor(&mut self) {
        self.text_cursors = [None; 4];
        let Some(details) = self.details.as_ref() else {
            return;
        };
        self.editor = Some(PresentationEditor {
            display_name: details.presentation.display_name.clone(),
            badge_label: details.presentation.badge_label.clone(),
            color: details.presentation.color.token(),
            field: 0,
        });
        self.view = ProjectView::Editor;
        self.failure = None;
    }

    fn begin_create(&mut self) {
        self.text_cursors = [None; 4];
        if self.available_agents.is_empty() {
            self.notice = Some(
                "No persistent durable Agent candidates are available. Adopt an Agent first; an Online runtime is not required.".to_string(),
            );
            return;
        }
        self.create_editor = Some(ProjectCreateEditor {
            project_id: String::new(),
            display_name: String::new(),
            badge_label: "CX".to_string(),
            color: ProjectPaletteColor::Cyan.token(),
            director: 0,
            field: 0,
        });
        self.view = ProjectView::Create;
        self.failure = None;
    }

    fn begin_operator_confirmation(&mut self) {
        let targets = self.operator_targets();
        let Some(target) = targets.get(self.operator_selected).cloned() else {
            self.notice = Some("No Operator action is available in this Project.".to_string());
            return;
        };
        self.pending_operator = Some(target);
        self.confirm_selected = false;
        self.view = ProjectView::ConfirmOperator;
    }

    fn project_actions(&self) -> Vec<ProjectMutationTarget> {
        let Some(details) = self.details.as_ref() else {
            return Vec::new();
        };
        match details.lifecycle {
            cutex::agent_management::ProjectLifecycle::Active => {
                let mut actions = self
                    .available_agents
                    .iter()
                    .map(|agent| ProjectMutationTarget {
                        label: format!(
                            "Review Add / explicit Detach → Add: {} ({})",
                            agent.name,
                            agent.cutex_session_id.as_str()
                        ),
                        operation: HumanManagementProjectMutationKind::AddMember {
                            cutex_session_id: agent.cutex_session_id.clone(),
                        },
                    })
                    .collect::<Vec<_>>();
                actions.extend(
                    details
                        .active_agents
                        .iter()
                        .filter(|member| {
                            member.agent.cutex_session_id != details.director.cutex_session_id
                        })
                        .map(|member| ProjectMutationTarget {
                            label: format!(
                                "Detach member {} ({})",
                                member.agent.spec.name,
                                member.agent.cutex_session_id.as_str()
                            ),
                            operation: HumanManagementProjectMutationKind::DetachMember {
                                cutex_session_id: member.agent.cutex_session_id.clone(),
                            },
                        }),
                );
                actions.push(ProjectMutationTarget {
                    label: "Archive Project (recoverable)".to_string(),
                    operation: HumanManagementProjectMutationKind::Archive,
                });
                actions
            }
            cutex::agent_management::ProjectLifecycle::Archived => vec![
                ProjectMutationTarget {
                    label: "Restore Project".to_string(),
                    operation: HumanManagementProjectMutationKind::Restore,
                },
                ProjectMutationTarget {
                    label: "Remove Project permanently; runtime Agents remain".to_string(),
                    operation: HumanManagementProjectMutationKind::Remove,
                },
            ],
            cutex::agent_management::ProjectLifecycle::Removed => Vec::new(),
        }
    }

    fn begin_project_actions(&mut self) {
        self.action_selected = 0;
        self.view = ProjectView::Actions;
        self.failure = None;
    }
}

fn operation_rank(operation: HumanManagementOperatorKind) -> u8 {
    match operation {
        HumanManagementOperatorKind::Revoke => 0,
        HumanManagementOperatorKind::Grant => 1,
    }
}

pub(super) fn run(
    previous_model: Option<CutexProjectsModel>,
) -> anyhow::Result<(PrimaryPanelOutcome, CutexProjectsModel)> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Cutex Projects requires an interactive terminal");
    }
    let needs_initial_load = previous_model.is_none();
    let mut model = previous_model.unwrap_or_else(|| {
        let mut loading = CutexProjectsModel::empty_with_failure("");
        loading.failure = None;
        loading.notice = Some("Loading Cutex Projects…".to_string());
        loading
    });
    let (mut terminal, restore) = open_terminal()?;
    if needs_initial_load {
        // Draw a complete first frame before any synchronous service discovery
        // or authenticated request. A slow or failed Management start must not
        // leave the user looking at a cleared terminal with no explanation.
        terminal.draw(|frame| render(frame, &model))?;
        model = load_model().unwrap_or_else(|error| {
            CutexProjectsModel::empty_with_failure(format!("Cutex Projects unavailable: {error:#}"))
        });
    }
    let result = run_loop(&mut terminal, &mut model);
    drop(terminal);
    drop(restore);
    Ok((result?, model))
}

fn load_model() -> anyhow::Result<CutexProjectsModel> {
    let client = ManagementControlClient::connect()
        .context("authenticated Human/Management control plane is required")?;
    let collection = client.projects()?;
    let mut projects = collection.projects;
    let archived_projects = collection.archived_projects;
    projects.extend(archived_projects.iter().cloned());
    let durable_candidates = client.durable_candidates()?;
    let available_agents = candidate_choices(&durable_candidates);
    Ok(CutexProjectsModel {
        help: None,
        leave_review: None,
        text_cursors: [None; 4],
        open_settings_requested: false,
        import_name_focused: false,
        durable_candidates,
        import_request: None,
        import_name: Input::default(),
        projects,
        archived_projects,
        available_agents,
        selected: 0,
        query: Input::default(),
        filter_focused: false,
        show_archived: false,
        details: None,
        section: ProjectSection::Overview,
        operator_selected: 0,
        pending_operator: None,
        pending_project_mutation: None,
        action_selected: 0,
        confirm_selected: false,
        editor: None,
        create_editor: None,
        view: ProjectView::List,
        client: Some(client),
        failure: None,
        notice: None,
    })
}

fn reload(model: &mut CutexProjectsModel, open_details: bool) -> anyhow::Result<()> {
    let selected_id = model
        .selected_project()
        .map(|project| project.project_id.clone());
    let client = model
        .client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Management control plane is unavailable"))?;
    let collection = client.projects()?;
    model.projects = collection.projects;
    model.archived_projects = collection.archived_projects;
    model
        .projects
        .extend(model.archived_projects.iter().cloned());
    model.durable_candidates = client.durable_candidates()?;
    model.available_agents = candidate_choices(&model.durable_candidates);
    let visible = model.visible_indices();
    model.selected = selected_id
        .and_then(|id| {
            visible
                .iter()
                .position(|index| model.projects[*index].project_id == id)
        })
        .unwrap_or(0)
        .min(visible.len().saturating_sub(1));
    if open_details {
        load_details(model)?;
    }
    Ok(())
}

fn load_details(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let project_id = model
        .selected_project()
        .map(|project| project.project_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no Cutex Project is selected"))?;
    let client = model
        .client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Management control plane is unavailable"))?;
    model.details = Some(client.project(&project_id)?);
    model.operator_selected = model
        .operator_selected
        .min(model.operator_targets().len().saturating_sub(1));
    model.view = ProjectView::Details;
    model.failure = None;
    Ok(())
}

fn save_editor(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let editor = model
        .editor
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("presentation editor is unavailable"))?;
    let details = model
        .details
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("project details are unavailable"))?;
    let color = editor.color.parse::<ProjectPaletteColor>()?;
    let request = HumanManagementPresentationUpdateRequest {
        schema: HumanManagementPresentationSchema::V1,
        project_id: details.project_id.clone(),
        expected_authority_epoch: details.authority_epoch,
        expected_presentation_revision: details.presentation.revision,
        presentation: ProjectPresentationInput {
            display_name: editor.display_name.clone(),
            badge_label: editor.badge_label.clone(),
            color,
        },
    };
    model
        .client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Management control plane is unavailable"))?
        .update_presentation(&request)?;
    model.editor = None;
    reload(model, true)?;
    model.section = ProjectSection::Appearance;
    model.notice =
        Some("Project appearance updated through Human/Management audit boundary".into());
    Ok(())
}

fn save_project_create(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let editor = model
        .create_editor
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("project create wizard is unavailable"))?;
    let project_id = cutex::agent_management::ProjectId::new(editor.project_id.clone())?;
    let director = model
        .available_agents
        .get(editor.director)
        .ok_or_else(|| anyhow::anyhow!("initial Director selection is unavailable"))?;
    let request = HumanManagementProjectMutationRequest {
        schema: HumanManagementProjectMutationSchema::V1,
        action_id: AgentActionId::new(format!("management-project-create-{}", Uuid::new_v4()))?,
        project_id: project_id.clone(),
        expected_authority_epoch: 0,
        expected_project_revision: 0,
        operation: HumanManagementProjectMutationKind::Create {
            director_cutex_session_id: director.cutex_session_id.clone(),
            presentation: ProjectPresentationInput {
                display_name: editor.display_name.clone(),
                badge_label: editor.badge_label.clone(),
                color: editor.color.parse::<ProjectPaletteColor>()?,
            },
        },
    };
    begin_import_confirmation(model, request)
}

fn candidate_choices(
    candidates: &[cutex::agent_management::DurableAgentCandidate],
) -> Vec<ProjectAgentChoice> {
    candidates
        .iter()
        .filter_map(|agent| {
            Some(ProjectAgentChoice {
                cutex_session_id: agent.cutex_session_id.clone()?,
                name: candidate_label(agent),
                current_project_id: agent.current_project_id.clone(),
            })
        })
        .collect()
}

fn candidate_label(agent: &cutex::agent_management::DurableAgentCandidate) -> String {
    format!(
        "{} · {} · {} · {}{}",
        agent
            .formal_name
            .as_deref()
            .unwrap_or("Formal name required"),
        if agent.in_roster {
            "roster"
        } else {
            "durable, not in roster"
        },
        agent
            .current_project_id
            .as_ref()
            .map(|p| p.as_str())
            .unwrap_or("unassigned"),
        if agent.online { "Online" } else { "Offline" },
        agent
            .rejection
            .as_ref()
            .map(|r| format!(" · Unavailable: {r}"))
            .unwrap_or_default()
    )
}

fn begin_import_confirmation(
    model: &mut CutexProjectsModel,
    assignment: HumanManagementProjectMutationRequest,
) -> anyhow::Result<()> {
    let id = match &assignment.operation {
        HumanManagementProjectMutationKind::Create {
            director_cutex_session_id,
            ..
        } => director_cutex_session_id,
        HumanManagementProjectMutationKind::AddMember { cutex_session_id } => cutex_session_id,
        _ => anyhow::bail!("unsupported import assignment"),
    };
    let candidate = model
        .durable_candidates
        .iter()
        .find(|c| c.cutex_session_id.as_ref() == Some(id))
        .cloned()
        .context("durable candidate unavailable; refresh")?;
    if let Some(reason) = &candidate.rejection {
        anyhow::bail!("Agent is unavailable: {reason}");
    }
    let detach = if let Some(source) = &candidate.current_project_id {
        if source == &assignment.project_id {
            anyhow::bail!("Agent already belongs to this Project");
        }
        let source = model
            .client
            .as_ref()
            .context("Management unavailable")?
            .project(source)?;
        Some(HumanManagementProjectMutationRequest {
            schema: HumanManagementProjectMutationSchema::V1,
            action_id: AgentActionId::new(format!("management-detach-{}", Uuid::new_v4()))?,
            project_id: source.project_id,
            expected_authority_epoch: source.authority_epoch,
            expected_project_revision: source.project_revision,
            operation: HumanManagementProjectMutationKind::DetachMember {
                cutex_session_id: id.clone(),
            },
        })
    } else {
        None
    };
    model.import_name = Input::new(candidate.formal_name.clone().unwrap_or_default());
    model.import_name_focused = candidate.formal_name.is_none();
    model.import_request = Some(cutex::agent_management::DurableImportRequest {
        action_id: AgentActionId::new(format!("management-import-{}", Uuid::new_v4()))?,
        confirmed_formal_name: candidate.formal_name.clone().unwrap_or_default(),
        candidate,
        assignment: Some(assignment),
        detach,
    });
    model.confirm_selected = false;
    model.view = ProjectView::ConfirmImport;
    Ok(())
}

fn execute_import_confirmation(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let request = model
        .import_request
        .as_mut()
        .context("import confirmation unavailable")?;
    request.confirmed_formal_name = model.import_name.value().to_string();
    let receipt = model
        .client
        .as_ref()
        .context("Management unavailable")?
        .import_durable_agent(request)?;
    if !receipt.complete {
        anyhow::bail!("Action {} incomplete: name set={}, imported={}, completed steps={:?}. {}. Retry the same confirmation to resume, or refresh to review current state.", receipt.action_id, receipt.named, receipt.imported, receipt.steps.keys().collect::<Vec<_>>(), receipt.error.as_deref().unwrap_or("unknown failure"));
    }
    model.import_request = None;
    model.create_editor = None;
    model.pending_project_mutation = None;
    model.view = ProjectView::List;
    reload(model, false)?;
    model.notice = Some(format!(
        "Action {} complete: formal name set={}, imported={}, Project steps={:?}",
        receipt.action_id,
        receipt.named,
        receipt.imported,
        receipt.steps.keys().collect::<Vec<_>>()
    ));
    Ok(())
}

fn execute_operator_action(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let target = model
        .pending_operator
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Operator action is unavailable"))?;
    let details = model
        .details
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("project details are unavailable"))?;
    let request = HumanManagementOperatorActionRequest {
        schema: HumanManagementOperatorSchema::V1,
        action_id: AgentActionId::new(format!("management-operator-{}", Uuid::new_v4()))?,
        project_id: details.project_id.clone(),
        expected_authority_epoch: details.authority_epoch,
        expected_grant_revision: details.operator_grant_revision,
        operation: target.operation,
        operator_cutex_session_id: target.cutex_session_id.clone(),
    };
    let receipt = model
        .client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Management control plane is unavailable"))?
        .operator_action(&request)?;
    model.pending_operator = None;
    reload(model, true)?;
    model.section = ProjectSection::Operators;
    model.notice = Some(format!(
        "Operator {:?} committed at grant revision {} (audit {})",
        receipt.operation, receipt.grant_revision, receipt.audit_event.event_id
    ));
    Ok(())
}

fn execute_project_mutation(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let target = model
        .pending_project_mutation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Project action is unavailable"))?;
    let operation = target.operation.clone();
    let details = model
        .details
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("project details are unavailable"))?;
    let request = HumanManagementProjectMutationRequest {
        schema: HumanManagementProjectMutationSchema::V1,
        action_id: AgentActionId::new(format!("management-project-{}", Uuid::new_v4()))?,
        project_id: details.project_id.clone(),
        expected_authority_epoch: details.authority_epoch,
        expected_project_revision: details.project_revision,
        operation: operation.clone(),
    };
    if matches!(
        operation,
        HumanManagementProjectMutationKind::AddMember { .. }
    ) {
        return begin_import_confirmation(model, request);
    }
    let receipt = model
        .client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Management control plane is unavailable"))?
        .project_mutation(&request)?;
    let leaves_active_list = matches!(
        operation,
        HumanManagementProjectMutationKind::Archive | HumanManagementProjectMutationKind::Remove
    );
    model.pending_project_mutation = None;
    model.confirm_selected = false;
    reload(model, !leaves_active_list)?;
    if leaves_active_list {
        model.details = None;
        model.view = ProjectView::List;
    }
    model.notice = Some(format!(
        "Project action committed at revision {} (audit {})",
        receipt.project_revision, receipt.audit_event.event_id
    ));
    Ok(())
}

fn run_loop(
    terminal: &mut ProjectTerminal,
    model: &mut CutexProjectsModel,
) -> anyhow::Result<PrimaryPanelOutcome> {
    loop {
        terminal.draw(|frame| render(frame, model))?;
        if !event::poll(POLL_INTERVAL)? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(key) => key,
            Event::Paste(text) => {
                handle_paste(model, &text);
                continue;
            }
            _ => continue,
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if let Some(outcome) = handle_key(model, key) {
            return Ok(outcome);
        }
    }
}

fn project_text_field(model: &mut CutexProjectsModel) -> Option<(&mut String, &mut Option<usize>)> {
    let (value, field) = match model.view {
        ProjectView::Editor => {
            let e = model.editor.as_mut()?;
            (
                match e.field {
                    0 => &mut e.display_name,
                    1 => &mut e.badge_label,
                    2 => &mut e.color,
                    _ => return None,
                },
                e.field,
            )
        }
        ProjectView::Create => {
            let e = model.create_editor.as_mut()?;
            (
                match e.field {
                    0 => &mut e.project_id,
                    1 => &mut e.display_name,
                    2 => &mut e.badge_label,
                    3 => &mut e.color,
                    _ => return None,
                },
                e.field,
            )
        }
        _ => return None,
    };
    Some((value, &mut model.text_cursors[field]))
}
fn handle_paste(model: &mut CutexProjectsModel, text: &str) {
    if model.help.is_some() || model.leave_review.is_some() {
        return;
    }
    if model.view == ProjectView::List && model.filter_focused {
        input_policy::paste(&mut model.query, text);
        model.retain_selection();
    } else if model.view == ProjectView::ConfirmImport && model.import_name_focused {
        input_policy::paste(&mut model.import_name, text);
    } else if let Some((value, cursor)) = project_text_field(model) {
        input_policy::paste_string(value, cursor, text);
    }
}
fn project_modal(model: &CutexProjectsModel) -> bool {
    matches!(
        model.view,
        ProjectView::ConfirmImport
            | ProjectView::ConfirmOperator
            | ProjectView::ConfirmProjectMutation
    )
}
fn project_dirty(model: &CutexProjectsModel) -> bool {
    model.editor.is_some() || model.create_editor.is_some()
}
fn project_commands(model: &CutexProjectsModel) -> Vec<(Command, Option<&'static str>)> {
    input_policy::BINDINGS
        .iter()
        .map(|b| {
            let reason = match b.command {
                Command::NewProject | Command::Archived if model.view != ProjectView::List => {
                    Some("Return to the Project list")
                }
                Command::Actions | Command::Edit | Command::Inspect
                    if !matches!(model.view, ProjectView::List | ProjectView::Details) =>
                {
                    Some("Finish the current editor/review")
                }
                Command::LoadMore | Command::Titles => Some("Available on Recent / Managed"),
                Command::NewProject if model.available_agents.is_empty() => {
                    Some("No eligible persistent Director candidate")
                }
                Command::Actions | Command::Edit | Command::Inspect
                    if model.visible_indices().is_empty() =>
                {
                    Some("Select a Project")
                }
                _ => None,
            };
            (b.command, reason)
        })
        .collect()
}
fn project_command(
    model: &mut CutexProjectsModel,
    command: Command,
) -> Option<PrimaryPanelOutcome> {
    if let Some((_, Some(reason))) = project_commands(model)
        .into_iter()
        .find(|(c, _)| *c == command)
    {
        model.notice = Some(reason.into());
        return None;
    }
    if matches!(
        command,
        Command::Page(_) | Command::Settings | Command::Exit | Command::Back
    ) {
        match input_policy::navigation_gate(false, project_modal(model), project_dirty(model)) {
            Gate::Block => return None,
            Gate::Review => {
                model.leave_review = Some(LeaveReview::new(
                    command,
                    matches!(model.view, ProjectView::Editor | ProjectView::Create),
                ));
                return None;
            }
            Gate::Allow => {}
        }
    }
    match command {
        Command::Archived => {
            if model.view == ProjectView::List {
                model.show_archived = !model.show_archived;
                model.retain_selection();
            }
            None
        }
        Command::Help => {
            model.help = Some(Help::default());
            None
        }
        Command::Page(panel) => {
            model.filter_focused = false;
            (panel != PrimaryPanel::Projects).then_some(PrimaryPanelOutcome::Switch(panel))
        }
        Command::Settings => {
            model.open_settings_requested = true;
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents))
        }
        Command::Exit => Some(PrimaryPanelOutcome::Exit),
        Command::Back => {
            model.view = ProjectView::List;
            None
        }
        Command::NewProject => {
            if model.view == ProjectView::List {
                model.begin_create();
            }
            None
        }
        Command::Actions | Command::Edit | Command::Inspect => {
            if model.view == ProjectView::List {
                if let Err(e) = load_details(model) {
                    model.failure = Some(format!("{e:#}"));
                    return None;
                }
            }
            if model.view == ProjectView::Details {
                match command {
                    Command::Actions => model.begin_project_actions(),
                    Command::Edit => model.begin_editor(),
                    _ => {}
                }
            }
            None
        }
        Command::Refresh => {
            // Keep reviewed identity/version and draft context frozen. A fresh
            // observation is requested after leaving this editor/review.
            if project_modal(model) || project_dirty(model) {
                model.notice = Some("Finish the draft/review before refreshing its target".into());
                return None;
            }
            let result = if model.view == ProjectView::List {
                reload(model, false)
            } else {
                load_details(model)
            };
            if let Err(e) = result {
                model.failure = Some(format!("{e:#}"));
            }
            None
        }
        _ => None,
    }
}
fn handle_key(model: &mut CutexProjectsModel, key: KeyEvent) -> Option<PrimaryPanelOutcome> {
    let text = (model.view == ProjectView::List && model.filter_focused)
        || (model.view == ProjectView::ConfirmImport && model.import_name_focused)
        || matches!(model.view, ProjectView::Editor | ProjectView::Create);
    if !super::session_tui_workspace_events::accepts_key(key, text) {
        return None;
    }
    if let Some(mut review) = model.leave_review.take() {
        match review.handle(key) {
            Some(0) => {}
            Some(1) => {
                model.editor = None;
                model.create_editor = None;
                model.view = ProjectView::List;
                return project_command(model, review.command);
            }
            Some(2) => {
                let result = if model.view == ProjectView::Editor {
                    save_editor(model)
                } else {
                    save_project_create(model)
                };
                if let Err(e) = result {
                    model.failure = Some(format!("{e:#}"));
                }
            }
            _ => model.leave_review = Some(review),
        }
        return None;
    }
    if let Some(mut help) = model.help.take() {
        match help.handle(key, &project_commands(model)) {
            Some(Some(command)) => return project_command(model, command),
            Some(None) => {}
            None => model.help = Some(help),
        }
        return None;
    }
    if input_policy::resolve(key) == Some(Command::Help) && !project_modal(model) {
        model.help = Some(Help::default());
        return None;
    }
    if model.view == ProjectView::List && model.filter_focused {
        if input_policy::edit(&mut model.query, key) {
            model.retain_selection();
            return None;
        }
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab
        ) {
            model.filter_focused = false;
            return None;
        }
    }
    if model.view == ProjectView::ConfirmImport && model.import_name_focused {
        if input_policy::edit(&mut model.import_name, key) {
            return None;
        }
        if matches!(
            key.code,
            KeyCode::Enter | KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab
        ) {
            model.import_name_focused = false;
            model.confirm_selected = key.code == KeyCode::BackTab;
            return None;
        }
    }
    if model.view == ProjectView::ConfirmImport
        && model
            .import_request
            .as_ref()
            .is_some_and(|r| r.candidate.formal_name.is_none())
    {
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            if (key.code == KeyCode::Tab && model.confirm_selected)
                || (key.code == KeyCode::BackTab && !model.confirm_selected)
            {
                model.import_name_focused = true;
            } else {
                model.confirm_selected = !model.confirm_selected;
            }
            return None;
        }
    }
    let palette = model.view == ProjectView::Editor
        && model.editor.as_ref().is_some_and(|e| e.field == 2)
        || model.view == ProjectView::Create
            && model.create_editor.as_ref().is_some_and(|e| e.field == 3);
    if !(palette
        && matches!(
            key.code,
            KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right
        ))
    {
        if let Some((value, cursor)) = project_text_field(model) {
            if input_policy::edit_string(value, cursor, key) {
                return None;
            }
        }
    }
    if let Some(command) = input_policy::resolve(key) {
        return project_command(model, command);
    }
    if key.code == KeyCode::Esc && matches!(model.view, ProjectView::Editor | ProjectView::Create) {
        return project_command(model, Command::Back);
    }
    if model.view == ProjectView::List {
        match key.code {
            KeyCode::Esc => {
                if model.query.value().is_empty() {
                    model.notice = Some("Ctrl+C exits Cutex".into());
                } else {
                    model.query.reset();
                    model.retain_selection();
                }
                return None;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                model.filter_focused = true;
                return None;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
                    && !c.is_control() =>
            {
                model.filter_focused = true;
                if c != '/' {
                    input_policy::edit(&mut model.query, key);
                    model.retain_selection();
                }
                return None;
            }
            KeyCode::Home => {
                model.selected = 0;
                return None;
            }
            KeyCode::End => {
                model.selected = model.visible_indices().len().saturating_sub(1);
                return None;
            }
            KeyCode::PageUp => {
                model.selected = model.selected.saturating_sub(10);
                return None;
            }
            KeyCode::PageDown => {
                model.selected =
                    (model.selected + 10).min(model.visible_indices().len().saturating_sub(1));
                return None;
            }
            _ => {}
        }
    }
    // The palette is a widget, not a text cursor or a page-navigation command.
    if palette && matches!(key.code, KeyCode::Left | KeyCode::Right) {
        if let Some((value, cursor)) = project_text_field(model) {
            let colors = ProjectPaletteColor::ALL;
            let index = colors
                .iter()
                .position(|color| color.token() == *value)
                .map_or(0, |index| {
                    if key.code == KeyCode::Left {
                        (index + colors.len() - 1) % colors.len()
                    } else {
                        (index + 1) % colors.len()
                    }
                });
            *value = colors[index].token();
            *cursor = None;
        }
        return None;
    }
    handle_project_widget_key(model, key)
}
fn handle_project_widget_key(
    model: &mut CutexProjectsModel,
    key: KeyEvent,
) -> Option<PrimaryPanelOutcome> {
    model.notice = None;
    match model.view {
        ProjectView::ConfirmImport => match key.code {
            KeyCode::Esc => {
                model.import_request = None;
                model.view = if model.create_editor.is_some() {
                    ProjectView::Create
                } else {
                    ProjectView::Details
                };
            }
            KeyCode::Left => model.confirm_selected = false,
            KeyCode::Right => model.confirm_selected = true,
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                model.confirm_selected = !model.confirm_selected
            }
            KeyCode::Enter if model.confirm_selected => match execute_import_confirmation(model) {
                Ok(()) => model.failure = None,
                Err(error) => {
                    model.failure = Some(format!("{error:#}"));
                    model.confirm_selected = false;
                }
            },
            KeyCode::Enter => {
                model.import_request = None;
                model.view = if model.create_editor.is_some() {
                    ProjectView::Create
                } else {
                    ProjectView::Details
                };
            }
            _ => {}
        },
        ProjectView::List => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                return Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents));
            }
            KeyCode::Left => {
                return PrimaryPanel::Projects
                    .adjacent(false)
                    .map(PrimaryPanelOutcome::Switch)
            }
            KeyCode::Right => {
                return PrimaryPanel::Projects
                    .adjacent(true)
                    .map(PrimaryPanelOutcome::Switch)
            }
            KeyCode::BackTab => {}
            KeyCode::Up => model.selected = model.selected.saturating_sub(1),
            KeyCode::Down => {
                model.selected =
                    (model.selected + 1).min(model.visible_indices().len().saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Tab => {
                model.section = ProjectSection::Overview;
                if let Err(error) = load_details(model) {
                    model.failure = Some(format!("{error:#}"));
                }
            }
            KeyCode::Char('/') => model.filter_focused = true,
            _ => {}
        },
        ProjectView::Details => match key.code {
            KeyCode::Esc => model.view = ProjectView::List,
            KeyCode::Left => model.section = model.section.shifted(-1),
            KeyCode::Right | KeyCode::Tab => model.section = model.section.shifted(1),
            KeyCode::BackTab if model.section == ProjectSection::Overview => {
                model.view = ProjectView::List
            }
            KeyCode::BackTab => model.section = model.section.shifted(-1),
            KeyCode::Up if model.section == ProjectSection::Operators => {
                model.operator_selected = model.operator_selected.saturating_sub(1)
            }
            KeyCode::Down if model.section == ProjectSection::Operators => {
                model.operator_selected = (model.operator_selected + 1)
                    .min(model.operator_targets().len().saturating_sub(1))
            }
            KeyCode::Enter => match model.section {
                ProjectSection::Operators => model.begin_operator_confirmation(),
                ProjectSection::Overview | ProjectSection::Members | ProjectSection::Appearance => {
                    model.notice = Some("This section has no mutating action.".to_string())
                }
            },
            _ => {}
        },
        ProjectView::Editor => match key.code {
            KeyCode::Esc => {
                model.editor = None;
                model.view = ProjectView::Details;
                model.failure = None;
            }
            KeyCode::Tab | KeyCode::Down | KeyCode::Right => {
                if let Some(editor) = model.editor.as_mut() {
                    editor.field = (editor.field + 1) % 3;
                }
            }
            KeyCode::BackTab | KeyCode::Up | KeyCode::Left => {
                if let Some(editor) = model.editor.as_mut() {
                    editor.field = (editor.field + 2) % 3;
                }
            }
            KeyCode::Char(' ') if model.editor.as_ref().is_some_and(|e| e.field == 2) => {
                if let Some(editor) = model.editor.as_mut().filter(|editor| editor.field == 2) {
                    let index = ProjectPaletteColor::ALL
                        .iter()
                        .position(|color| color.token() == editor.color);
                    editor.color = ProjectPaletteColor::ALL
                        [index.map_or(0, |index| index + 1) % ProjectPaletteColor::ALL.len()]
                    .token();
                }
            }
            KeyCode::Enter => match save_editor(model) {
                Ok(()) => model.failure = None,
                Err(error) => model.failure = Some(format!("{error:#}")),
            },
            _ => {}
        },
        ProjectView::Create => match key.code {
            KeyCode::Esc => {
                model.create_editor = None;
                model.view = ProjectView::List;
                model.failure = None;
            }
            KeyCode::Tab | KeyCode::Down => {
                if let Some(editor) = model.create_editor.as_mut() {
                    editor.field = (editor.field + 1).min(4);
                }
            }
            KeyCode::BackTab | KeyCode::Up => {
                if let Some(editor) = model.create_editor.as_mut() {
                    editor.field = editor.field.saturating_sub(1);
                }
            }
            KeyCode::Left | KeyCode::Right => {
                if let Some(editor) = model
                    .create_editor
                    .as_mut()
                    .filter(|editor| editor.field == 4)
                {
                    let len = model.available_agents.len();
                    if len > 0 {
                        editor.director = if key.code == KeyCode::Left {
                            (editor.director + len - 1) % len
                        } else {
                            (editor.director + 1) % len
                        };
                    }
                }
            }
            KeyCode::Char(' ') if model.create_editor.as_ref().is_some_and(|e| e.field == 3) => {
                if let Some(editor) = model
                    .create_editor
                    .as_mut()
                    .filter(|editor| editor.field == 3)
                {
                    let index = ProjectPaletteColor::ALL
                        .iter()
                        .position(|color| color.token() == editor.color);
                    editor.color = ProjectPaletteColor::ALL
                        [index.map_or(0, |index| index + 1) % ProjectPaletteColor::ALL.len()]
                    .token();
                }
            }
            KeyCode::Enter => {
                if model
                    .create_editor
                    .as_ref()
                    .is_some_and(|editor| editor.field < 4)
                {
                    model.create_editor.as_mut().unwrap().field += 1;
                } else {
                    match save_project_create(model) {
                        Ok(()) => model.failure = None,
                        Err(error) => model.failure = Some(format!("{error:#}")),
                    }
                }
            }
            _ => {}
        },
        ProjectView::Actions => match key.code {
            KeyCode::Esc => model.view = ProjectView::Details,
            KeyCode::Up => model.action_selected = model.action_selected.saturating_sub(1),
            KeyCode::Down => {
                model.action_selected = (model.action_selected + 1)
                    .min(model.project_actions().len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(target) = model.project_actions().get(model.action_selected).cloned() {
                    model.pending_project_mutation = Some(target);
                    model.confirm_selected = false;
                    model.view = ProjectView::ConfirmProjectMutation;
                }
            }
            _ => {}
        },
        ProjectView::ConfirmProjectMutation => match key.code {
            KeyCode::Esc => {
                model.pending_project_mutation = None;
                model.view = ProjectView::Actions;
            }
            KeyCode::Left => model.confirm_selected = false,
            KeyCode::Right => model.confirm_selected = true,
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                model.confirm_selected = !model.confirm_selected
            }
            KeyCode::Enter if model.confirm_selected => match execute_project_mutation(model) {
                Ok(()) => model.failure = None,
                Err(error) => {
                    model.failure = Some(format!("{error:#}"));
                    model.pending_project_mutation = None;
                    model.view = ProjectView::Details;
                }
            },
            KeyCode::Enter => {
                model.pending_project_mutation = None;
                model.view = ProjectView::Actions;
            }
            _ => {}
        },
        ProjectView::ConfirmOperator => match key.code {
            KeyCode::Esc => {
                model.pending_operator = None;
                model.view = ProjectView::Details;
            }
            KeyCode::Left => model.confirm_selected = false,
            KeyCode::Right => model.confirm_selected = true,
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                model.confirm_selected = !model.confirm_selected
            }
            KeyCode::Enter if model.confirm_selected => match execute_operator_action(model) {
                Ok(()) => model.failure = None,
                Err(error) => {
                    model.failure = Some(format!("{error:#}"));
                    model.pending_operator = None;
                    model.view = ProjectView::Details;
                }
            },
            KeyCode::Enter => {
                model.pending_operator = None;
                model.view = ProjectView::Details;
            }
            _ => {}
        },
    }
    None
}

fn render(frame: &mut Frame<'_>, model: &CutexProjectsModel) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(primary_panel_tabs(PrimaryPanel::Projects)),
        areas[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Cutex Projects",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  authenticated Human/Management boundary",
                Style::new().fg(Color::DarkGray),
            ),
        ])),
        areas[1],
    );
    match model.view {
        ProjectView::ConfirmImport => render_import_confirmation(frame, areas[2], model),
        ProjectView::List => render_list(frame, areas[2], model),
        ProjectView::Details => render_details(frame, areas[2], model),
        ProjectView::Editor => {
            render_details(frame, areas[2], model);
            render_editor(frame, areas[2], model.editor.as_ref());
        }
        ProjectView::Create => render_create_editor(frame, areas[2], model),
        ProjectView::Actions => {
            render_details(frame, areas[2], model);
            render_project_actions(frame, areas[2], model);
        }
        ProjectView::ConfirmProjectMutation => {
            render_details(frame, areas[2], model);
            render_project_mutation_confirmation(frame, areas[2], model);
        }
        ProjectView::ConfirmOperator => {
            render_details(frame, areas[2], model);
            render_operator_confirmation(frame, areas[2], model);
        }
    }
    let footer = if let Some(message) = model.failure.as_deref().or(model.notice.as_deref()) {
        vec![Span::raw(message.to_string())]
    } else {
        match model.view {
            ProjectView::ConfirmImport => footer_hints(&[
                ("Type", "formal name if required"),
                ("←/→/Tab", "Cancel/Confirm"),
                ("Enter", "selected choice"),
                ("Esc", "cancel"),
            ]),
            ProjectView::List if model.filter_focused => footer_hints(&[
                ("Type", "filter"),
                ("Tab/Enter", "finish"),
                ("Esc", "cancel"),
            ]),
            ProjectView::List => footer_hints(&[
                ("↑/↓", "select"),
                ("Enter/Tab", "details"),
                ("←/→", "tabs"),
                ("Alt+A", "actions/create"),
                ("Alt+E", "appearance"),
                ("Alt+T", "tasks"),
                ("/", "filter"),
                ("Ctrl+H", "archived"),
                ("F5", "refresh"),
                ("Esc", "back"),
            ]),
            ProjectView::Details => footer_hints(&[
                ("←/→/Tab", "section"),
                ("BackTab", "list"),
                ("↑/↓", "select"),
                ("Enter", "primary"),
                ("Alt+A", "actions"),
                ("Alt+E", "appearance"),
                ("Alt+T", "tasks"),
                ("F5", "refresh"),
                ("Esc", "list"),
            ]),
            ProjectView::Editor => footer_hints(&[
                ("Tab/←/→", "field"),
                ("Space", "color"),
                ("Enter", "save"),
                ("Esc", "cancel"),
            ]),
            ProjectView::Create => footer_hints(&[
                ("Enter/Tab", "next"),
                ("↑/↓", "step"),
                ("←/→", "Director"),
                ("Space", "palette"),
                ("Enter", "create on final step"),
                ("Esc", "cancel"),
            ]),
            ProjectView::Actions => {
                footer_hints(&[("↑/↓", "choose"), ("Enter", "review"), ("Esc", "details")])
            }
            ProjectView::ConfirmProjectMutation => footer_hints(&[
                ("←/→/Tab", "Cancel/Confirm"),
                ("Enter", "choose"),
                ("Esc", "cancel"),
            ]),
            ProjectView::ConfirmOperator => footer_hints(&[
                ("←/→/Tab", "Cancel/Confirm"),
                ("Enter", "choose"),
                ("Esc", "cancel"),
            ]),
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(footer))
            .wrap(Wrap { trim: true })
            .style(Style::new().fg(if model.failure.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            })),
        areas[3],
    );
    let field = match model.view {
        ProjectView::Editor => model.editor.as_ref().map(|e| {
            (
                e.field,
                match e.field {
                    0 => e.display_name.as_str(),
                    1 => e.badge_label.as_str(),
                    _ => e.color.as_str(),
                },
            )
        }),
        ProjectView::Create => model.create_editor.as_ref().and_then(|e| {
            Some((
                e.field,
                match e.field {
                    0 => e.project_id.as_str(),
                    1 => e.display_name.as_str(),
                    2 => e.badge_label.as_str(),
                    3 => e.color.as_str(),
                    _ => return None,
                },
            ))
        }),
        _ => None,
    };
    let input_area = Rect {
        x: areas[2].x,
        y: areas[2].bottom().saturating_sub(3),
        width: areas[2].width,
        height: 3.min(areas[2].height),
    };
    if let Some((index, value)) = field {
        input_policy::render_input(
            frame,
            input_area,
            &input_policy::string_input(value, model.text_cursors[index]),
            " Focused field · Tab next · Enter save/next ",
            true,
        );
    } else if model.view == ProjectView::ConfirmImport && model.import_name_focused {
        input_policy::render_input(
            frame,
            input_area,
            &model.import_name,
            " Formal Agent name · Tab to choices ",
            true,
        );
    }
    if model.view == ProjectView::List
        && !model.filter_focused
        && model.failure.is_none()
        && model.notice.is_none()
    {
        frame.render_widget(
            Paragraph::new(input_policy::footer(&project_commands(model)))
                .wrap(Wrap { trim: true }),
            areas[3],
        );
    }
    if let Some(help) = &model.help {
        help.render(frame, &project_commands(model));
    }
    if let Some(review) = &model.leave_review {
        review.render(frame);
    }
}

fn render_list(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    let filter_title = if model.filter_focused {
        " Filter (typing) "
    } else {
        " Filter name / project id / badge  [/] "
    };
    input_policy::render_input(
        frame,
        chunks[0],
        &model.query,
        filter_title,
        model.filter_focused,
    );
    let visible = model.visible_indices();
    let rows = visible.iter().map(|index| {
        let project = &model.projects[*index];
        Row::new([
            Cell::from(project.presentation.badge_label.clone())
                .style(project_badge_style(project.presentation.color)),
            Cell::from(project.presentation.display_name.clone()),
            Cell::from(project.project_id.to_string()),
            Cell::from(
                project
                    .director_name
                    .clone()
                    .unwrap_or_else(|| project.director_cutex_session_id.as_str().to_string()),
            ),
            Cell::from(project.active_member_count.to_string()),
            Cell::from(if project.retired_member_count == 0 {
                "-".to_string()
            } else {
                project.retired_member_count.to_string()
            }),
            Cell::from(match project.access_role {
                ProjectAccessRole::PrimaryDirector => "primary",
                ProjectAccessRole::AgentOperator => "operator",
                ProjectAccessRole::HumanManagement => "management",
            }),
        ])
    });
    let widths = if chunks[1].width >= 96 {
        vec![
            Constraint::Length(4),
            Constraint::Length(22),
            Constraint::Min(16),
            Constraint::Length(20),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(10),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Length(16),
            Constraint::Min(12),
            Constraint::Length(16),
            Constraint::Length(7),
            Constraint::Length(0),
            Constraint::Length(0),
        ]
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new([
                "BADGE",
                "PROJECT",
                "PROJECT ID",
                "DIRECTOR",
                "AGENTS",
                "RETIRED",
                "ROLE",
            ])
            .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD)),
        )
        .block(Block::bordered().title(if model.show_archived {
            " Canonical Projects + archived "
        } else {
            " Canonical Projects "
        }))
        .row_highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state =
        TableState::default().with_selected((!visible.is_empty()).then_some(model.selected));
    frame.render_stateful_widget(table, chunks[1], &mut state);
    if visible.is_empty() && chunks[1].height > 3 {
        frame.render_widget(
            Paragraph::new(if model.projects.is_empty() {
                "No canonical Cutex Projects exist."
            } else {
                "No Projects match this filter."
            })
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::DarkGray)),
            Rect {
                y: chunks[1].y.saturating_add(2),
                height: chunks[1].height.saturating_sub(3),
                ..chunks[1]
            },
        );
    }
}

fn render_details(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let Some(project) = model.details.as_ref() else {
        frame.render_widget(
            Paragraph::new("Project details are unavailable.")
                .block(Block::bordered().title(" Cutex Project ")),
            area,
        );
        return;
    };
    let chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).split(area);
    let mut tabs = Vec::new();
    for (index, section) in ProjectSection::ALL.into_iter().enumerate() {
        if index > 0 {
            tabs.push(Span::styled(" | ", Style::new().fg(Color::DarkGray)));
        }
        tabs.push(Span::styled(
            section.label(),
            if section == model.section {
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Gray)
            },
        ));
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" {} ", project.presentation.badge_label),
                    project_badge_style(project.presentation.color),
                ),
                Span::raw(" "),
                Span::styled(
                    project.presentation.display_name.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(tabs),
        ]),
        chunks[0],
    );
    match model.section {
        ProjectSection::Overview => render_overview(frame, chunks[1], project),
        ProjectSection::Members => render_members(frame, chunks[1], project),
        ProjectSection::Operators => render_operators(frame, chunks[1], model, project),
        ProjectSection::Appearance => render_appearance(frame, chunks[1], project),
    }
}

fn render_overview(frame: &mut Frame<'_>, area: Rect, project: &CutexProjectWorkspace) {
    let role = match project.access_role {
        ProjectAccessRole::PrimaryDirector => "Primary Director",
        ProjectAccessRole::AgentOperator => "Agent Operator",
        ProjectAccessRole::HumanManagement => "Human Management",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Canonical project_id: {}", project.project_id)),
            Line::from(format!("Authority epoch: {}", project.authority_epoch)),
            Line::from(format!(
                "Primary Director: {}",
                project.director.cutex_session_id.as_str()
            )),
            Line::from(format!("Access boundary: {role}")),
            Line::from(format!(
                "Members: {} active, {} retired, {} operators",
                project.active_agents.len(),
                project.retired_agents.len(),
                project.agent_operators.len()
            )),
            Line::from(format!(
                "Operator CAS revision: {}",
                project.operator_grant_revision
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(" Overview ")),
        area,
    );
}

fn render_members(frame: &mut Frame<'_>, area: Rect, project: &CutexProjectWorkspace) {
    let mut lines = vec![Line::from(Span::styled(
        "Primary Director",
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(format!(
        "  {}",
        project.director.cutex_session_id.as_str()
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Ordinary members",
        Style::new().add_modifier(Modifier::BOLD),
    )));
    lines.extend(project.active_agents.iter().map(|member| {
        Line::from(format!(
            "  {}  [{}]  {}",
            member.agent.spec.name,
            lifecycle_label(member.lifecycle),
            member.agent.cutex_session_id.as_str()
        ))
    }));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Retired members",
        Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )));
    lines.extend(project.retired_agents.iter().map(|member| {
        Line::from(format!(
            "  {}  {}",
            member.agent.spec.name,
            member.agent.cutex_session_id.as_str()
        ))
    }));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Members ")),
        area,
    );
}

fn render_operators(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &CutexProjectsModel,
    project: &CutexProjectWorkspace,
) {
    let targets = model.operator_targets();
    let mut lines = vec![Line::from(format!(
        "Grant set revision {} — every write also fences authority epoch {}",
        project.operator_grant_revision, project.authority_epoch
    ))];
    if targets.is_empty() {
        lines.push(Line::from("No grant/revoke target is available."));
    } else {
        lines.extend(targets.iter().enumerate().map(|(index, target)| {
            let selected = index == model.operator_selected;
            let verb = match target.operation {
                HumanManagementOperatorKind::Grant => "grant",
                HumanManagementOperatorKind::Revoke => "revoke",
            };
            let repair = target
                .repair_action_id
                .as_ref()
                .map(|action| format!("  REVIEW legacy retained rotation {action}"))
                .unwrap_or_default();
            Line::from(Span::styled(
                format!(
                    "{} {:6} {}  [{}]  {}{}",
                    if selected { ">" } else { " " },
                    verb,
                    target.name,
                    lifecycle_label(target.lifecycle),
                    target.cutex_session_id.as_str(),
                    repair
                ),
                if selected {
                    Style::new()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                },
            ))
        }));
    }
    if !project.legacy_operator_repair_candidates.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Legacy R11→R12 repair candidates are suggestions only; choose grant and confirm.",
            Style::new().fg(Color::Yellow),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Operators ")),
        area,
    );
}

fn render_appearance(frame: &mut Frame<'_>, area: Rect, project: &CutexProjectWorkspace) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "Display name: {}",
                project.presentation.display_name
            )),
            Line::from(format!("Badge label: {}", project.presentation.badge_label)),
            Line::from(format!(
                "Palette color: {}",
                project.presentation.color.token()
            )),
            Line::from(format!(
                "Presentation revision: {}",
                project.presentation.revision
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Badge is 1–2 terminal cells. Project identity and authority are immutable here.",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .block(Block::bordered().title(" Appearance ")),
        area,
    );
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, editor: Option<&PresentationEditor>) {
    let popup = centered_rect(62, 11, area);
    frame.render_widget(Clear, popup);
    let Some(editor) = editor else {
        return;
    };
    let field = |index, label: &str, value: String| {
        Line::from(vec![
            Span::styled(
                if editor.field == index { "> " } else { "  " },
                Style::new().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{label}: "),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(value),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![
            field(0, "Display name", editor.display_name.clone()),
            field(1, "Badge (1-2 cells)", editor.badge_label.clone()),
            field(2, "Color", editor.color.clone()),
            Line::from(""),
            Line::from(Span::styled(
                "Type cyan/blue/green/magenta/yellow/red or #RRGGBB · Space cycles palette",
                Style::new().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Human Management write: authority epoch + presentation revision CAS",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .block(Block::bordered().title(" Edit appearance ")),
        popup,
    );
}

fn render_import_confirmation(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let Some(request) = &model.import_request else {
        return;
    };
    let target = request
        .assignment
        .as_ref()
        .map(|a| format!("{:?} → {}", a.operation, a.project_id))
        .unwrap_or_else(|| "Import only; remains unassigned".into());
    let source = request.detach.as_ref().map(|d| format!("Explicit Detach from {} first (revision {}, authority {}). Protected roles may require Director rotation or grant revocation before detachment.", d.project_id, d.expected_project_revision, d.expected_authority_epoch)).unwrap_or_else(|| "No source detachment".into());
    frame.render_widget(Paragraph::new(vec![
        Line::from(format!("Durable Agent ID: {}", request.candidate.cutex_session_id.as_ref().map(|id| id.as_str()).unwrap_or("invalid — cannot confirm"))),
        Line::from(candidate_label(&request.candidate)),
        Line::from(format!("Formal Cutex Agent name: {}", model.import_name.value())),
        Line::from(if request.candidate.formal_name.is_none() { "Historical record: type a formal Agent name. No thread title is supplied." } else { "Existing authoritative formal name; name changes require a fresh review." }),
        Line::from(if request.candidate.in_roster { "Already in roster" } else { "Confirm authorizes setting the formal name if missing and importing into the roster." }),
        Line::from(source), Line::from(target),
        Line::from(format!("Action: {}", request.action_id)),
        Line::from("Each completed step is retained if a later step fails. Retry uses this exact action."),
        Line::from(if model.confirm_selected { "  Cancel     > Confirm" } else { "> Cancel       Confirm" }),
    ]).wrap(Wrap { trim: true }).block(Block::bordered().title(" Confirm durable Agent import / Project assignment ")), area);
}

fn render_create_editor(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let Some(editor) = model.create_editor.as_ref() else {
        return;
    };
    let director = model
        .available_agents
        .get(editor.director)
        .map(|agent| format!("{} ({})", agent.name, agent.cutex_session_id.as_str()))
        .unwrap_or_else(|| "No durable Agent with a validated identity".to_string());
    let field = |index, label: &str, value: String| {
        Line::from(vec![
            Span::styled(
                if editor.field == index { "> " } else { "  " },
                Style::new().fg(Color::Cyan),
            ),
            Span::styled(
                format!("{label}: "),
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::raw(value),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![
            field(0, "Immutable project_id", editor.project_id.clone()),
            field(1, "Display name", editor.display_name.clone()),
            field(2, "Badge (1-2 cells)", editor.badge_label.clone()),
            field(3, "Color", editor.color.clone()),
            field(4, "Initial Director", director),
            Line::from(model.durable_candidates.iter().filter(|row| row.cutex_session_id.is_none()).map(|row| format!("Rejected store key {:?}: {}", row.raw_store_key, row.rejection.as_deref().unwrap_or("invalid identity"))).collect::<Vec<_>>().join("; ")),
            Line::from(""),
            Line::from(Span::styled(
                "The final step commits Project, membership, authority, presentation, and the project Director seat.",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().border_style(Style::new().fg(Color::Cyan)).title(" Create Cutex Project ")),
        area,
    );
}

fn render_project_actions(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let popup = centered_rect(76, 16, area);
    frame.render_widget(Clear, popup);
    let actions = model.project_actions();
    let lines = if actions.is_empty() {
        vec![Line::from("No structural Project action is available.")]
    } else {
        actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                Line::from(Span::styled(
                    format!(
                        "{} {}",
                        if index == model.action_selected {
                            ">"
                        } else {
                            " "
                        },
                        action.label
                    ),
                    if index == model.action_selected {
                        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new()
                    },
                ))
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .border_style(Style::new().fg(Color::Cyan))
                .title(" Project Actions "),
        ),
        popup,
    );
}

fn render_project_mutation_confirmation(
    frame: &mut Frame<'_>,
    area: Rect,
    model: &CutexProjectsModel,
) {
    let popup = centered_rect(76, 10, area);
    frame.render_widget(Clear, popup);
    let description = model
        .pending_project_mutation
        .as_ref()
        .map(|target| target.label.clone())
        .unwrap_or_else(|| "Project action unavailable".to_string());
    let option = |confirmed: bool, label: &'static str| {
        Span::styled(
            format!(" {label} "),
            if model.confirm_selected == confirmed {
                Style::new()
                    .fg(Color::Black)
                    .bg(if confirmed {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Gray)
            },
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(description),
            Line::from(""),
            Line::from(vec![option(false, "Cancel"), Span::raw("  "), option(true, "Confirm")]),
            Line::from(""),
            Line::from(Span::styled(
                "Detach/remove fail while matching non-Closed assignments exist. Remove never closes runtime Agents.",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(" Confirm Project action ")),
        popup,
    );
}

fn render_operator_confirmation(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let popup = centered_rect(70, 10, area);
    frame.render_widget(Clear, popup);
    let description = model
        .pending_operator
        .as_ref()
        .map(|target| {
            format!(
                "{:?} Agent Operator {} ({})?",
                target.operation,
                target.name,
                target.cutex_session_id.as_str()
            )
        })
        .unwrap_or_else(|| "Operator action is unavailable.".to_string());
    let option = |confirmed: bool, label: &'static str| {
        Span::styled(
            format!(" {label} "),
            if model.confirm_selected == confirmed {
                Style::new()
                    .fg(Color::Black)
                    .bg(if confirmed {
                        Color::Yellow
                    } else {
                        Color::Cyan
                    })
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Gray)
            },
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(description),
            Line::from(""),
            Line::from(vec![
                option(false, "Cancel"),
                Span::raw("  "),
                option(true, "Confirm"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "A stale authority epoch or grant revision fails without a write.",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(" Confirm Operator action ")),
        popup,
    );
}

fn lifecycle_label(lifecycle: ProjectMemberLifecycle) -> &'static str {
    match lifecycle {
        ProjectMemberLifecycle::Online => "online",
        ProjectMemberLifecycle::Offline => "offline",
        ProjectMemberLifecycle::Unavailable => "unavailable",
    }
}

pub(super) fn palette_color(color: ProjectPaletteColor) -> Color {
    match color {
        ProjectPaletteColor::Cyan => Color::Cyan,
        ProjectPaletteColor::Blue => Color::LightBlue,
        ProjectPaletteColor::Green => Color::LightGreen,
        ProjectPaletteColor::Magenta => Color::LightMagenta,
        ProjectPaletteColor::Yellow => Color::Yellow,
        ProjectPaletteColor::Red => Color::LightRed,
        ProjectPaletteColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
    }
}

fn project_badge_style(color: ProjectPaletteColor) -> Style {
    Style::new()
        .fg(Color::White)
        .bg(palette_color(color))
        .add_modifier(Modifier::BOLD)
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn open_terminal() -> anyhow::Result<(ProjectTerminal, TerminalRestore)> {
    enable_raw_mode().context("Failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)
        .context("Failed to enter alternate screen")?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))
        .context("Failed to initialize Cutex Projects terminal")?;
    Ok((terminal, TerminalRestore))
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen, Show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_contract_production_durable_import_http_tui_create_add_cancel_auth_and_rename() {
        use cutex::session::{
            model::{CutexSessionRecord, CutexSessionStore},
            store::{load_cutex_session_store, save_cutex_session_store},
        };
        use std::{
            net::TcpListener,
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc,
            },
            thread,
        };
        let _home =
            crate::cli_app::test_home::IsolatedTestHome::new("cutex-import-http-tui").unwrap();
        let _isolated_bus = TcpListener::bind("127.0.0.1:0").unwrap();
        cutex::config::store::save_codez_config(&cutex::profiles::model::CodezConfig {
            agent_bus_port: Some(_isolated_bus.local_addr().unwrap().port()),
            ..Default::default()
        })
        .unwrap();
        let mut sessions = CutexSessionStore::default();
        for (id, name) in [
            ("cutex.director", Some("Formal Director")),
            ("cutex.worker", None),
        ] {
            let mut record = CutexSessionRecord::new(
                id.into(),
                Some(format!("native-{id}")),
                cutex::platform::host::current_host_name(),
                _home.root().to_string_lossy().into_owned(),
                None,
            )
            .unwrap();
            cutex::session::runtime_defaults::apply_managed_session_defaults(
                &mut record,
                name,
                None,
                vec!["test".into()],
                false,
                false,
            );
            record.thread_name = Some("NEVER USE THIS TITLE".into());
            record.display_name_hint = record.thread_name.clone();
            sessions.sessions.insert(id.into(), record);
        }
        let mut malformed = sessions.sessions["cutex.worker"].clone();
        malformed.codex_session_id = Some("bad-native".into());
        sessions.sessions.insert("bad key".into(), malformed);
        save_cutex_session_store(&sessions).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let server = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request =
                            cutex::http::server::read_simple_http_request(&mut stream).unwrap();
                        cutex::management::v2::server::handle_v2_request(
                            &mut stream,
                            &request,
                            Some("ordinary"),
                            Some("seat"),
                            Some("human-root"),
                            &[],
                            crate::cli_app::management_context::management_request_context(),
                        )
                        .unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        struct ServerGuard(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
        impl Drop for ServerGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
                self.1.take().unwrap().join().unwrap();
            }
        }
        let _server = ServerGuard(stop, Some(server));
        let client = ManagementControlClient::test_endpoint(base.clone(), "human-root".into());
        for wrong in ["ordinary", "seat", "bus", ""] {
            let denied = ManagementControlClient::test_endpoint(base.clone(), wrong.into());
            assert!(denied.durable_candidates().is_err());
        }
        let mut model = CutexProjectsModel::empty_with_failure("test");
        model.failure = None;
        model.client = Some(client.clone());
        model.durable_candidates = client.durable_candidates().unwrap();
        model.available_agents = candidate_choices(&model.durable_candidates);
        assert_eq!(model.available_agents.len(), 2);
        assert!(model
            .durable_candidates
            .iter()
            .any(|row| row.raw_store_key == "bad key"
                && row.cutex_session_id.is_none()
                && row.rejection.is_some()));
        let before = std::fs::read(cutex::session::store::cutex_sessions_path().unwrap()).unwrap();
        model.begin_create();
        let editor = model.create_editor.as_mut().unwrap();
        editor.project_id = "alpha".into();
        editor.display_name = "Alpha".into();
        save_project_create(&mut model).unwrap();
        assert_eq!(model.view, ProjectView::ConfirmImport);
        assert!(!model.confirm_selected);
        handle_key(&mut model, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(
            std::fs::read(cutex::session::store::cutex_sessions_path().unwrap()).unwrap(),
            before
        );
        assert!(
            cutex::agent_management::AgentManagementProvider::open_default()
                .unwrap()
                .store()
                .snapshot()
                .unwrap()
                .agents
                .is_empty()
        );
        // Use the production confirmation dispatcher. Refresh is supplied through
        // the same authenticated client, without starting a daemon.
        save_project_create(&mut model).unwrap();
        let create = model.import_request.clone().unwrap();
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            handle_key(
                &mut model,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, kind),
            );
        }
        assert_eq!(model.view, ProjectView::ConfirmImport);
        assert_eq!(
            std::fs::read(cutex::session::store::cutex_sessions_path().unwrap()).unwrap(),
            before
        );
        assert!(
            cutex::agent_management::AgentManagementProvider::open_default()
                .unwrap()
                .store()
                .snapshot()
                .unwrap()
                .agents
                .is_empty()
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(model.failure.is_none(), "{:?}", model.failure);
        assert_eq!(model.view, ProjectView::List);
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            handle_key(
                &mut model,
                KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, kind),
            );
            assert_eq!(model.view, ProjectView::List);
        }
        let receipt = client.import_durable_agent(&create).unwrap();
        assert!(receipt.complete, "{:?}", receipt.error);
        let project = client
            .project(&cutex::agent_management::ProjectId::new("alpha").unwrap())
            .unwrap();
        model.details = Some(project);
        model.durable_candidates = client.durable_candidates().unwrap();
        let worker = model
            .durable_candidates
            .iter()
            .find(|c| {
                c.cutex_session_id
                    .as_ref()
                    .is_some_and(|id| id.as_str() == "cutex.worker")
            })
            .unwrap()
            .clone();
        let add = HumanManagementProjectMutationRequest {
            schema: HumanManagementProjectMutationSchema::V1,
            action_id: AgentActionId::new("production-add").unwrap(),
            project_id: cutex::agent_management::ProjectId::new("alpha").unwrap(),
            expected_authority_epoch: 1,
            expected_project_revision: 1,
            operation: HumanManagementProjectMutationKind::AddMember {
                cutex_session_id: worker.cutex_session_id.clone().unwrap(),
            },
        };
        begin_import_confirmation(&mut model, add).unwrap();
        assert_eq!(model.import_name.value(), "");
        handle_paste(&mut model, "Explicit Worker");
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        );
        assert!(!model.import_name_focused && model.confirm_selected);
        handle_key(&mut model, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(model.import_name_focused);
        handle_key(&mut model, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        let mut add = model.import_request.clone().unwrap();
        add.confirmed_formal_name = model.import_name.value().into();
        let wrong = ManagementControlClient::test_endpoint(base, "ordinary".into());
        assert!(wrong.import_durable_agent(&add).is_err());
        assert!(
            !client
                .durable_candidates()
                .unwrap()
                .iter()
                .find(|c| c.cutex_session_id == worker.cutex_session_id)
                .unwrap()
                .in_roster
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(model.failure.is_none(), "{:?}", model.failure);
        assert!(client
            .durable_candidates()
            .unwrap()
            .iter()
            .any(|c| c.cutex_session_id == worker.cutex_session_id && c.in_roster));
        let receipt = client.import_durable_agent(&add).unwrap();
        assert!(receipt.complete && receipt.named && receipt.imported);
        let mut sessions = load_cutex_session_store().unwrap();
        cutex::session::service::set_cutex_session_display_name_by_key(
            &mut sessions,
            "cutex.worker",
            "Renamed Worker",
        )
        .unwrap();
        cutex::session::service::set_cutex_session_display_name_by_key(
            &mut sessions,
            "cutex.director",
            "Renamed Director",
        )
        .unwrap();
        cutex::session::service::set_cutex_session_profile_by_key(
            &mut sessions,
            "cutex.worker",
            Some("changed-profile".into()),
        )
        .unwrap();
        save_cutex_session_store(&sessions).unwrap();
        let collection = client.projects().unwrap();
        let imported = receipt.imported_agent.as_ref().unwrap();
        assert_eq!(imported.spec.profile, None);
        crate::cli_app::agent_management::validate_managed_recovery_record(
            &sessions.sessions["cutex.worker"],
            &imported.cutex_session_id,
            &imported.native_session_id,
            &imported.spec,
        )
        .expect(
            "imported nullable profile and later profile/name change preserve lifecycle identity",
        );
        assert_eq!(
            cutex::session::service::cutex_session_display_name(&sessions.sessions["cutex.worker"]),
            "Renamed Worker"
        );
        assert_eq!(
            collection.projects[0].director_name.as_deref(),
            Some("Renamed Director")
        );
        let project = client
            .project(&add.assignment.as_ref().unwrap().project_id)
            .unwrap();
        assert!(project
            .active_agents
            .iter()
            .any(|m| m.agent.spec.name == "Renamed Worker"));
        assert_eq!(
            project.director.member.unwrap().agent.spec.name,
            "Renamed Director"
        );
        assert_eq!(client.import_durable_agent(&add).unwrap(), receipt);
        assert!(client
            .durable_candidates()
            .unwrap()
            .iter()
            .any(|c| c.formal_name.as_deref() == Some("Renamed Worker")
                && c.current_project_id.is_some()));
    }
    use ratatui::backend::TestBackend;

    fn project(
        id: &str,
        name: &str,
        badge: &str,
        color: ProjectPaletteColor,
    ) -> CutexProjectSummary {
        serde_json::from_value(serde_json::json!({
            "project_id": id,
            "authority_epoch": 3,
            "director_cutex_session_id": "cutex.director",
            "access_role": "human_management",
            "operator_count": 0,
            "presentation": {
                "display_name": name, "badge_label": badge, "color": color.token(),
                "revision": 0, "stored": false
            },
            "active_member_count": 1, "retired_member_count": 0
        }))
        .unwrap()
    }

    fn model_with_projects() -> CutexProjectsModel {
        let mut model = CutexProjectsModel::empty_with_failure("fixture");
        model.failure = None;
        model.projects = vec![
            project(
                "cutex-stack-main",
                "Cutex Stack Main",
                "CS",
                ProjectPaletteColor::Blue,
            ),
            project(
                "render-lab",
                "Render Lab",
                "CX",
                ProjectPaletteColor::Magenta,
            ),
        ];
        model
    }

    fn rendered(model: &CutexProjectsModel, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
        format!("{:?}", terminal.backend().buffer())
    }

    #[test]
    fn ui_contract_k09_project_spaces_and_paste_parity() {
        for create in [false, true] {
            for field in 0..if create { 3 } else { 2 } {
                let mut typed = model_with_projects();
                typed.view = if create {
                    ProjectView::Create
                } else {
                    ProjectView::Editor
                };
                typed.editor = Some(PresentationEditor {
                    display_name: String::new(),
                    badge_label: String::new(),
                    color: "green".into(),
                    field,
                });
                typed.create_editor = Some(ProjectCreateEditor {
                    project_id: String::new(),
                    display_name: String::new(),
                    badge_label: String::new(),
                    color: "green".into(),
                    director: 0,
                    field,
                });
                let mut pasted = model_with_projects();
                pasted.view = typed.view;
                pasted.editor = typed.editor.clone();
                pasted.create_editor = typed.create_editor.clone();
                for c in "My Project 中文".chars() {
                    assert_eq!(
                        handle_key(
                            &mut typed,
                            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
                        ),
                        None
                    );
                }
                handle_paste(&mut pasted, "My Project 中文\r\n\t\u{1b}");
                if create {
                    let a = typed.create_editor.unwrap();
                    let b = pasted.create_editor.unwrap();
                    assert_eq!(
                        (&a.project_id, &a.display_name, &a.badge_label, &a.color),
                        (&b.project_id, &b.display_name, &b.badge_label, &b.color)
                    );
                    assert_eq!(
                        [a.project_id, a.display_name, a.badge_label][field],
                        "My Project 中文"
                    );
                } else {
                    let a = typed.editor.unwrap();
                    let b = pasted.editor.unwrap();
                    assert_eq!(
                        (&a.display_name, &a.badge_label, &a.color),
                        (&b.display_name, &b.badge_label, &b.color)
                    );
                    assert_eq!([a.display_name, a.badge_label][field], "My Project 中文");
                }
            }
        }
    }

    #[test]
    fn ui_contract_project_filter_consumes_shortcuts_and_paste() {
        let mut model = model_with_projects();
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        handle_paste(&mut model, "My Project\n");
        handle_key(&mut model, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(model.query.value(), "y Project");
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT),
        );
        assert_eq!(model.view, ProjectView::List);
        assert!(model.create_editor.is_none());
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert_eq!(model.query.value(), "");
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(!model.filter_focused);
        assert_eq!(model.view, ProjectView::List);
    }

    #[test]
    fn ui_contract_k07_k08_project_confirmations_focus_cancel_and_repeat() {
        for view in [
            ProjectView::ConfirmImport,
            ProjectView::ConfirmOperator,
            ProjectView::ConfirmProjectMutation,
        ] {
            let mut model = model_with_projects();
            model.view = view;
            assert!(!model.confirm_selected);
            for (code, selected) in [
                (KeyCode::Right, true),
                (KeyCode::Left, false),
                (KeyCode::Tab, true),
                (KeyCode::Tab, false),
                (KeyCode::BackTab, true),
            ] {
                handle_key(&mut model, KeyEvent::new(code, KeyModifiers::NONE));
                assert_eq!(model.confirm_selected, selected);
                assert_eq!(model.view, view);
                assert!(model.failure.is_none());
            }
            for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
                handle_key(
                    &mut model,
                    KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, kind),
                );
                assert_eq!(model.view, view);
                assert!(model.failure.is_none());
            }
            handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            );
            assert_ne!(model.view, view);
            assert!(model.failure.is_none()); // Cancel did not attempt a service call.
            model.view = view;
            model.confirm_selected = true;
            handle_key(&mut model, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_ne!(model.view, view);
            assert!(model.failure.is_none());
        }
        let mut model = model_with_projects();
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            handle_key(
                &mut model,
                KeyEvent::new_with_kind(KeyCode::Char('a'), KeyModifiers::ALT, kind),
            );
            assert!(model.create_editor.is_none());
        }
    }

    #[test]
    fn ui_contract_b1_project_filter_and_editor_cursor_parity() {
        let mut model = model_with_projects();
        let text = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 /中文🙂";
        for c in text.chars() {
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        assert_eq!(model.query.value(), text);
        for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Esc] {
            assert_eq!(
                handle_key(&mut model, KeyEvent::new(code, KeyModifiers::NONE)),
                None
            );
        }
        assert!(model.query.value().is_empty());
        assert!(model.notice.as_deref().unwrap().contains("Ctrl+C"));
        model.view = ProjectView::Editor;
        model.editor = Some(PresentationEditor {
            display_name: "ab".into(),
            badge_label: "CX".into(),
            color: "green".into(),
            field: 0,
        });
        handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        handle_paste(&mut model, "中文e\u{301}🙂\n\t\u{1b}");
        assert_eq!(
            model.editor.as_ref().unwrap().display_name,
            "a中文e\u{301}🙂b"
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
        );
        assert_eq!(model.editor.as_ref().unwrap().field, 0);
        handle_key(&mut model, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert!(model
            .editor
            .as_ref()
            .unwrap()
            .display_name
            .starts_with('中'));
        let mut terminal = Terminal::new(TestBackend::new(38, 18)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(terminal.get_cursor_position().unwrap().x < 37);
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(model.editor.as_ref().unwrap().display_name.is_empty());
    }

    #[test]
    fn ui_contract_b1_project_f1_new_settings_and_dirty_navigation_gate() {
        let mut model = model_with_projects();
        model.available_agents.push(ProjectAgentChoice {
            cutex_session_id: cutex::role_revision::CutexSessionId::new("cutex.director").unwrap(),
            name: "Formal Director".into(),
            current_project_id: None,
        });
        handle_key(&mut model, KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        let index = project_commands(&model)
            .iter()
            .position(|(c, _)| *c == Command::NewProject)
            .unwrap();
        for _ in 0..index {
            handle_key(&mut model, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(model.view, ProjectView::Create);
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT),
        );
        assert!(model.leave_review.is_some());
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(model.create_editor.is_some()); // Cancel is default
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT),
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents))
        );
        assert!(model.open_settings_requested);
        assert!(model.create_editor.is_none());
        model.view = ProjectView::ConfirmProjectMutation;
        for key in [
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
        ] {
            assert_eq!(handle_key(&mut model, key), None);
            assert_eq!(model.view, ProjectView::ConfirmProjectMutation);
        }
    }

    #[test]
    fn project_filter_matches_name_id_and_badge() {
        let mut model = model_with_projects();
        for query in ["render lab", "render-lab", "cx"] {
            model.query = Input::new(query.to_string());
            assert_eq!(model.visible_indices(), vec![1]);
        }
        model.query = Input::new("cs".to_string());
        assert_eq!(model.visible_indices(), vec![0]);
    }

    #[test]
    fn bare_action_letters_filter_while_alt_n_opens_create() {
        let mut model = model_with_projects();
        for character in ['a', 'e'] {
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
            assert_eq!(model.view, ProjectView::List);
            assert!(model.filter_focused);
        }

        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        assert_eq!(model.query.value(), "ae/a");
        model.filter_focused = false;
        model.available_agents.push(ProjectAgentChoice {
            cutex_session_id: cutex::role_revision::CutexSessionId::new("cutex.director-new")
                .unwrap(),
            name: "New Director".to_string(),
            current_project_id: None,
        });
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::ALT),
        );
        assert_eq!(model.view, ProjectView::Create);
        assert!(model.create_editor.is_some());
    }

    #[test]
    fn list_arrows_switch_tabs_palette_arrows_stay_in_widget() {
        let mut model = model_with_projects();
        assert_eq!(
            handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Recent))
        );
        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Tasks))
        );
        assert!(model.pending_operator.is_none());

        model.view = ProjectView::Editor;
        model.editor = Some(PresentationEditor {
            display_name: "Draft Name".to_string(),
            badge_label: "DN".to_string(),
            color: ProjectPaletteColor::Green.token(),
            field: 2,
        });
        handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(model.editor.as_ref().map(|editor| editor.field), Some(2));
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("blue")
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(model.editor.as_ref().map(|editor| editor.field), Some(2));
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("green")
        );
    }

    #[test]
    fn alt_shortcut_switch_preserves_filter_and_editor_input() {
        let mut model = model_with_projects();
        model.query = Input::new("render".to_string());
        model.editor = Some(PresentationEditor {
            display_name: "Draft Name".to_string(),
            badge_label: "DN".to_string(),
            color: ProjectPaletteColor::Green.token(),
            field: 1,
        });
        model.view = ProjectView::Editor;

        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT)
            ),
            None
        );
        assert!(model.leave_review.is_some());
        assert_eq!(model.query.value(), "render");
        assert_eq!(model.view, ProjectView::Editor);
        assert_eq!(
            model
                .editor
                .as_ref()
                .map(|editor| editor.display_name.as_str()),
            Some("Draft Name")
        );
    }

    #[test]
    fn refresh_preserves_editor_input_when_the_backing_reload_fails() {
        let mut model = model_with_projects();
        model.editor = Some(PresentationEditor {
            display_name: "Uncommitted Draft".to_string(),
            badge_label: "UD".to_string(),
            color: ProjectPaletteColor::Green.token(),
            field: 1,
        });
        model.view = ProjectView::Editor;

        assert_eq!(
            handle_key(&mut model, KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            None
        );
        assert_eq!(model.view, ProjectView::Editor);
        assert_eq!(
            model
                .editor
                .as_ref()
                .map(|editor| editor.display_name.as_str()),
            Some("Uncommitted Draft")
        );
        assert!(model.failure.is_none());
        assert!(model
            .notice
            .as_deref()
            .unwrap()
            .contains("before refreshing"));
    }

    #[test]
    fn operator_confirmation_defaults_to_cancel_and_left_right_never_commit() {
        let mut model = model_with_projects();
        model.view = ProjectView::ConfirmOperator;
        model.pending_operator = Some(OperatorTarget {
            cutex_session_id: cutex::role_revision::CutexSessionId::new("cutex.worker").unwrap(),
            name: "Worker".to_string(),
            lifecycle: ProjectMemberLifecycle::Online,
            operation: HumanManagementOperatorKind::Grant,
            repair_action_id: None,
        });
        assert!(!model.confirm_selected);
        for key in [KeyCode::Left, KeyCode::Right] {
            assert_eq!(
                handle_key(&mut model, KeyEvent::new(key, KeyModifiers::NONE)),
                None
            );
            assert_eq!(model.view, ProjectView::ConfirmOperator);
            assert!(model.pending_operator.is_some());
            assert_eq!(model.confirm_selected, key == KeyCode::Right);
        }
        handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(model.view, ProjectView::Details);
        assert!(model.pending_operator.is_none());
    }

    #[test]
    fn badge_style_is_white_on_project_color_and_cx_is_two_cells() {
        assert_eq!(unicode_width::UnicodeWidthStr::width("CX"), 2);
        let style = project_badge_style(ProjectPaletteColor::Magenta);
        assert_eq!(style.fg, Some(Color::White));
        assert_eq!(style.bg, Some(Color::LightMagenta));
        assert!(rendered(&model_with_projects(), 90, 18).contains("CX"));
    }

    #[test]
    fn custom_rgb_badge_uses_ratatui_rgb_background() {
        let style = project_badge_style(ProjectPaletteColor::Rgb(0x12, 0x34, 0x56));
        assert_eq!(style.fg, Some(Color::White));
        assert_eq!(style.bg, Some(Color::Rgb(0x12, 0x34, 0x56)));
    }

    #[test]
    fn editor_color_field_accepts_text_edits_and_keeps_palette_cycle_shortcut() {
        let mut model = model_with_projects();
        model.view = ProjectView::Editor;
        model.editor = Some(PresentationEditor {
            display_name: "Draft Name".to_string(),
            badge_label: "CX".to_string(),
            color: "#12345".to_string(),
            field: 2,
        });

        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char('6'), KeyModifiers::NONE),
        );
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("#123456")
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("#12345")
        );

        model.editor.as_mut().unwrap().color.clear();
        handle_paste(&mut model, "#12aB3c");
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("#12aB3c")
        );

        model.editor.as_mut().unwrap().color = "green".to_string();
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("magenta")
        );

        model.editor.as_mut().unwrap().color = "#123456".to_string();
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        );
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("cyan")
        );
    }

    #[test]
    fn editor_text_fields_take_printable_keys_before_local_action_shortcuts() {
        let mut model = model_with_projects();
        model.view = ProjectView::Editor;
        model.editor = Some(PresentationEditor {
            display_name: String::new(),
            badge_label: String::new(),
            color: String::new(),
            field: 1,
        });

        for character in ['C', 'X'] {
            assert_eq!(
                handle_key(
                    &mut model,
                    KeyEvent::new(KeyCode::Char(character), KeyModifiers::SHIFT),
                ),
                None
            );
        }
        assert_eq!(
            model
                .editor
                .as_ref()
                .map(|editor| editor.badge_label.as_str()),
            Some("CX")
        );

        model.editor.as_mut().unwrap().field = 2;
        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char('#'), KeyModifiers::SHIFT),
            ),
            None
        );
        for character in ['1', '2', 'a', 'B', '3', 'c'] {
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color.as_str()),
            Some("#12aB3c")
        );

        let before = model.editor.clone().unwrap();
        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char('m'), KeyModifiers::ALT),
            ),
            None
        );
        assert!(model.leave_review.is_some());
        let after = model.editor.as_ref().unwrap();
        assert_eq!(after.display_name, before.display_name);
        assert_eq!(after.badge_label, before.badge_label);
        assert_eq!(after.color, before.color);
        assert_eq!(after.field, before.field);
    }

    #[test]
    fn narrow_terminal_and_resize_render_without_panicking() {
        let model = model_with_projects();
        let mut terminal = Terminal::new(TestBackend::new(38, 9)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        terminal.backend_mut().resize(120, 24);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("PROJECT ID"));
    }

    #[test]
    fn empty_error_state_never_falls_back_to_codex_workspaces() {
        let model = CutexProjectsModel::empty_with_failure("not authorized");
        let text = rendered(&model, 80, 16);
        assert!(text.contains("Cutex Projects"));
        assert!(text.contains("No canonical Cutex Projects"));
        assert!(!text.contains("Codex Workspaces"));
    }
}
