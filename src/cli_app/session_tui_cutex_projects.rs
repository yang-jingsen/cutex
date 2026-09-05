//! Human-authenticated Cutex Project management workspace.

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use anyhow::Context;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use cutex::agent_management::{
    AgentActionId, CutexProjectSummary, CutexProjectWorkspace, ProjectAccessRole,
    ProjectMemberLifecycle, ProjectPaletteColor, ProjectPresentationInput,
};
use cutex::management::control_plane::{
    HumanManagementOperatorActionRequest, HumanManagementOperatorKind,
    HumanManagementOperatorSchema, HumanManagementPresentationSchema,
    HumanManagementPresentationUpdateRequest,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use tui_input::{Input, InputRequest};
use uuid::Uuid;

use super::management_control_plane::ManagementControlClient;
use super::session_tui_workspace::{
    primary_panel_shortcut, primary_panel_tabs, PrimaryPanel, PrimaryPanelOutcome,
};

const POLL_INTERVAL: Duration = Duration::from_millis(80);
type ProjectTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone)]
struct PresentationEditor {
    display_name: String,
    badge_label: String,
    color: ProjectPaletteColor,
    field: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ProjectView {
    List,
    Details,
    Editor,
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

#[derive(Debug)]
pub(super) struct CutexProjectsModel {
    projects: Vec<CutexProjectSummary>,
    selected: usize,
    query: Input,
    filter_focused: bool,
    details: Option<CutexProjectWorkspace>,
    section: ProjectSection,
    operator_selected: usize,
    pending_operator: Option<OperatorTarget>,
    confirm_selected: bool,
    editor: Option<PresentationEditor>,
    view: ProjectView,
    client: Option<ManagementControlClient>,
    failure: Option<String>,
    notice: Option<String>,
}

impl CutexProjectsModel {
    fn empty_with_failure(error: impl Into<String>) -> Self {
        Self {
            projects: Vec::new(),
            selected: 0,
            query: Input::default(),
            filter_focused: false,
            details: None,
            section: ProjectSection::Overview,
            operator_selected: 0,
            pending_operator: None,
            confirm_selected: false,
            editor: None,
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
                (query.is_empty()
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
                        .contains(&query))
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
        let Some(details) = self.details.as_ref() else {
            return;
        };
        self.editor = Some(PresentationEditor {
            display_name: details.presentation.display_name.clone(),
            badge_label: details.presentation.badge_label.clone(),
            color: details.presentation.color,
            field: 0,
        });
        self.view = ProjectView::Editor;
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
    let mut model = previous_model.unwrap_or_else(|| {
        load_model().unwrap_or_else(|error| {
            CutexProjectsModel::empty_with_failure(format!("Cutex Projects unavailable: {error:#}"))
        })
    });
    let (mut terminal, restore) = open_terminal()?;
    let result = run_loop(&mut terminal, &mut model);
    drop(terminal);
    drop(restore);
    Ok((result?, model))
}

fn load_model() -> anyhow::Result<CutexProjectsModel> {
    let client = ManagementControlClient::connect()
        .context("authenticated Human/Management control plane is required")?;
    let projects = client.projects()?.projects;
    Ok(CutexProjectsModel {
        projects,
        selected: 0,
        query: Input::default(),
        filter_focused: false,
        details: None,
        section: ProjectSection::Overview,
        operator_selected: 0,
        pending_operator: None,
        confirm_selected: false,
        editor: None,
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
    model.projects = client.projects()?.projects;
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
    let request = HumanManagementPresentationUpdateRequest {
        schema: HumanManagementPresentationSchema::V1,
        project_id: details.project_id.clone(),
        expected_authority_epoch: details.authority_epoch,
        expected_presentation_revision: details.presentation.revision,
        presentation: ProjectPresentationInput {
            display_name: editor.display_name.clone(),
            badge_label: editor.badge_label.clone(),
            color: editor.color,
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

fn run_loop(
    terminal: &mut ProjectTerminal,
    model: &mut CutexProjectsModel,
) -> anyhow::Result<PrimaryPanelOutcome> {
    loop {
        terminal.draw(|frame| render(frame, model))?;
        if !event::poll(POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if let Some(outcome) = handle_key(model, key) {
            return Ok(outcome);
        }
    }
}

fn handle_key(model: &mut CutexProjectsModel, key: KeyEvent) -> Option<PrimaryPanelOutcome> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(PrimaryPanelOutcome::Exit);
    }
    if let Some(panel) = primary_panel_shortcut(key) {
        return (panel != PrimaryPanel::Projects).then_some(PrimaryPanelOutcome::Switch(panel));
    }
    if key.modifiers == KeyModifiers::NONE && key.code == KeyCode::F(5) {
        let view = model.view;
        let editor = model.editor.clone();
        let pending_operator = model.pending_operator.clone();
        let confirm_selected = model.confirm_selected;
        let result = if view == ProjectView::List {
            reload(model, false)
        } else {
            load_details(model)
        };
        if view != ProjectView::List {
            // Refresh the backing projection without consuming a draft or
            // review that belongs to this workspace.
            model.view = view;
            model.editor = editor;
            model.pending_operator = pending_operator;
            model.confirm_selected = confirm_selected;
        }
        match result {
            Ok(()) => model.failure = None,
            Err(error) => model.failure = Some(format!("{error:#}")),
        }
        return None;
    }
    model.notice = None;
    match model.view {
        ProjectView::List if model.filter_focused => match key.code {
            KeyCode::Esc | KeyCode::Enter => model.filter_focused = false,
            KeyCode::Tab | KeyCode::BackTab => model.filter_focused = false,
            KeyCode::Left => {
                model.query.handle(InputRequest::GoToPrevChar);
            }
            KeyCode::Right => {
                model.query.handle(InputRequest::GoToNextChar);
            }
            KeyCode::Backspace => {
                model.query.handle(InputRequest::DeletePrevChar);
                model.retain_selection();
            }
            KeyCode::Delete => {
                model.query.handle(InputRequest::DeleteNextChar);
                model.retain_selection();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                model.query.handle(InputRequest::InsertChar(character));
                model.retain_selection();
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
            KeyCode::Char('a') => {
                model.section = ProjectSection::Operators;
                if let Err(error) = load_details(model) {
                    model.failure = Some(format!("{error:#}"));
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                model.section = ProjectSection::Appearance;
                match load_details(model) {
                    Ok(()) => model.begin_editor(),
                    Err(error) => model.failure = Some(format!("{error:#}")),
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
            KeyCode::Char('a') => {
                if model.section == ProjectSection::Operators {
                    model.begin_operator_confirmation();
                } else {
                    model.section = ProjectSection::Operators;
                }
            }
            KeyCode::Char('e') | KeyCode::Char('E') => {
                model.section = ProjectSection::Appearance;
                model.begin_editor();
            }
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
            KeyCode::Char(' ') => {
                if let Some(editor) = model.editor.as_mut().filter(|editor| editor.field == 2) {
                    let index = ProjectPaletteColor::ALL
                        .iter()
                        .position(|color| *color == editor.color)
                        .unwrap_or(0);
                    editor.color =
                        ProjectPaletteColor::ALL[(index + 1) % ProjectPaletteColor::ALL.len()];
                }
            }
            KeyCode::Backspace => {
                if let Some(editor) = model.editor.as_mut() {
                    match editor.field {
                        0 => {
                            editor.display_name.pop();
                        }
                        1 => {
                            editor.badge_label.pop();
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(editor) = model.editor.as_mut() {
                    match editor.field {
                        0 => editor.display_name.push(character),
                        1 => editor.badge_label.push(character),
                        _ => {}
                    }
                }
            }
            KeyCode::Enter => match save_editor(model) {
                Ok(()) => model.failure = None,
                Err(error) => model.failure = Some(format!("{error:#}")),
            },
            _ => {}
        },
        ProjectView::ConfirmOperator => match key.code {
            KeyCode::Esc => {
                model.pending_operator = None;
                model.view = ProjectView::Details;
            }
            KeyCode::Up | KeyCode::Down => model.confirm_selected = !model.confirm_selected,
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
            KeyCode::Left | KeyCode::Right => {}
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
        ProjectView::List => render_list(frame, areas[2], model),
        ProjectView::Details => render_details(frame, areas[2], model),
        ProjectView::Editor => {
            render_details(frame, areas[2], model);
            render_editor(frame, areas[2], model.editor.as_ref());
        }
        ProjectView::ConfirmOperator => {
            render_details(frame, areas[2], model);
            render_operator_confirmation(frame, areas[2], model);
        }
    }
    let footer = model
        .failure
        .as_deref()
        .or(model.notice.as_deref())
        .unwrap_or(match model.view {
            ProjectView::List if model.filter_focused => {
                "Type to filter name / project id / badge  Tab/Enter finish  Esc cancel"
            }
            ProjectView::List => {
                "↑/↓ select  Enter/Tab details  ←/→ tabs  a actions  e edit  / filter  F5 refresh  Esc back"
            }
            ProjectView::Details => {
                "←/→/Tab section  BackTab list  ↑/↓ select  Enter primary  a actions  e edit  F5 refresh  Esc list"
            }
            ProjectView::Editor => "Tab/←/→ field  Space color  Enter save  Esc cancel",
            ProjectView::ConfirmOperator => {
                "↑/↓ Cancel/Confirm  Enter choose  Esc cancel  (←/→ never commits)"
            }
        });
    frame.render_widget(
        Paragraph::new(footer)
            .wrap(Wrap { trim: true })
            .style(Style::new().fg(if model.failure.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            })),
        areas[3],
    );
}

fn render_list(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).split(area);
    let filter_title = if model.filter_focused {
        " Filter (typing) "
    } else {
        " Filter name / project id / badge  [/] "
    };
    frame.render_widget(
        Paragraph::new(model.query.value()).block(Block::bordered().title(filter_title)),
        chunks[0],
    );
    let visible = model.visible_indices();
    let rows = visible.iter().map(|index| {
        let project = &model.projects[*index];
        Row::new([
            Cell::from(project.presentation.badge_label.clone())
                .style(project_badge_style(project.presentation.color)),
            Cell::from(project.presentation.display_name.clone()),
            Cell::from(project.project_id.to_string()),
            Cell::from(match project.access_role {
                ProjectAccessRole::PrimaryDirector => "primary",
                ProjectAccessRole::AgentOperator => "operator",
                ProjectAccessRole::HumanManagement => "management",
            }),
            Cell::from(project.director_cutex_session_id.as_str().to_string()),
            Cell::from(project.operator_count.to_string()),
        ])
    });
    let widths = if chunks[1].width >= 96 {
        vec![
            Constraint::Length(4),
            Constraint::Length(22),
            Constraint::Min(16),
            Constraint::Length(10),
            Constraint::Length(24),
            Constraint::Length(4),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Length(16),
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(0),
            Constraint::Length(0),
        ]
    };
    let table = Table::new(rows, widths)
        .header(
            Row::new(["ID", "NAME", "PROJECT ID", "ROLE", "DIRECTOR", "OPS"])
                .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD)),
        )
        .block(Block::bordered().title(" Canonical Projects "))
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
            field(2, "Palette color", editor.color.token().to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Human Management write: authority epoch + presentation revision CAS",
                Style::new().fg(Color::DarkGray),
            )),
        ])
        .block(Block::bordered().title(" Edit appearance ")),
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
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let terminal = Terminal::new(CrosstermBackend::new(stdout))
        .context("Failed to initialize Cutex Projects terminal")?;
    Ok((terminal, TerminalRestore))
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, Show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn list_arrows_switch_adjacent_tabs_and_editor_arrows_only_move_focus() {
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
            color: ProjectPaletteColor::Green,
            field: 2,
        });
        handle_key(&mut model, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(model.editor.as_ref().map(|editor| editor.field), Some(1));
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color),
            Some(ProjectPaletteColor::Green)
        );
        handle_key(
            &mut model,
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(model.editor.as_ref().map(|editor| editor.field), Some(2));
        assert_eq!(
            model.editor.as_ref().map(|editor| editor.color),
            Some(ProjectPaletteColor::Green)
        );
    }

    #[test]
    fn alt_shortcut_switch_preserves_filter_and_editor_input() {
        let mut model = model_with_projects();
        model.query = Input::new("render".to_string());
        model.editor = Some(PresentationEditor {
            display_name: "Draft Name".to_string(),
            badge_label: "DN".to_string(),
            color: ProjectPaletteColor::Green,
            field: 1,
        });
        model.view = ProjectView::Editor;

        assert_eq!(
            handle_key(
                &mut model,
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::ALT)
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Tasks))
        );
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
            color: ProjectPaletteColor::Green,
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
        assert!(model.failure.is_some());
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
            assert!(!model.confirm_selected);
        }
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
