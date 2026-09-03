//! Cutex permission Project workspace.
//!
//! This UI consumes only the authenticated Agent Management projection. It is
//! intentionally separate from the native Codex workspace catalog.

use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use anyhow::Context;
use crossterm::cursor::Show;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use cutex::agent_management::{
    AgentManagementError, AgentManagementInvocation, AgentManagementProvider,
    AgentRuntimeObservation, CutexProjectSummary, CutexProjectWorkspace, ProjectAccessRole,
    ProjectMemberLifecycle, ProjectPaletteColor, ProjectPresentationInput,
    ProjectPresentationUpdateRequest, ProjectRuntimeObserver,
};
use cutex::role_revision::CutexSessionId;
use cutex::session::service::cutex_session_launch_cwd;
use cutex::session::store::load_cutex_session_store;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

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
}

#[derive(Debug)]
struct CutexProjectsModel {
    projects: Vec<CutexProjectSummary>,
    selected: usize,
    details: Option<CutexProjectWorkspace>,
    editor: Option<PresentationEditor>,
    view: ProjectView,
    invocation: Option<AgentManagementInvocation>,
    failure: Option<String>,
    notice: Option<String>,
}

impl CutexProjectsModel {
    fn empty_with_failure(error: impl Into<String>) -> Self {
        Self {
            projects: Vec::new(),
            selected: 0,
            details: None,
            editor: None,
            view: ProjectView::List,
            invocation: None,
            failure: Some(error.into()),
            notice: None,
        }
    }

    fn selected_project(&self) -> Option<&CutexProjectSummary> {
        self.projects.get(self.selected)
    }

    fn begin_editor(&mut self) {
        let Some(details) = self.details.as_ref() else {
            return;
        };
        if details.access_role != ProjectAccessRole::PrimaryDirector {
            self.failure = Some(
                "Only the Primary Director may edit Project presentation settings.".to_string(),
            );
            return;
        }
        self.editor = Some(PresentationEditor {
            display_name: details.presentation.display_name.clone(),
            badge_label: details.presentation.badge_label.clone(),
            color: details.presentation.color,
            field: 0,
        });
        self.view = ProjectView::Editor;
        self.failure = None;
    }
}

pub(super) fn run() -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Cutex Projects requires an interactive terminal");
    }
    let mut model = load_model().unwrap_or_else(|error| {
        CutexProjectsModel::empty_with_failure(format!("Cutex Projects unavailable: {error:#}"))
    });
    let (mut terminal, restore) = open_terminal()?;
    let result = run_loop(&mut terminal, &mut model);
    drop(terminal);
    drop(restore);
    result
}

fn load_model() -> anyhow::Result<CutexProjectsModel> {
    let agent = super::agent_context::current_live_agent()
        .context("an authenticated live Cutex Agent is required")?;
    let caller = agent
        .cutex_session_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("current Agent has no durable Cutex session identity"))?;
    let invocation = AgentManagementInvocation {
        caller_cutex_session: CutexSessionId::new(caller.to_string())
            .map_err(|error| anyhow::anyhow!(error))?,
        caller_runtime_agent_id: agent.id,
    };
    let provider = AgentManagementProvider::open_default()?;
    let projects = provider
        .list_cutex_projects(&invocation)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(CutexProjectsModel {
        projects,
        selected: 0,
        details: None,
        editor: None,
        view: ProjectView::List,
        invocation: Some(invocation),
        failure: None,
        notice: None,
    })
}

fn reload(model: &mut CutexProjectsModel, open_details: bool) -> anyhow::Result<()> {
    let invocation = model
        .invocation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("authenticated Agent Manager context is unavailable"))?;
    let selected_id = model
        .selected_project()
        .map(|project| project.project_id.clone());
    let provider = AgentManagementProvider::open_default()?;
    model.projects = provider
        .list_cutex_projects(invocation)
        .map_err(|error| anyhow::anyhow!(error))?;
    model.selected = selected_id
        .and_then(|id| {
            model
                .projects
                .iter()
                .position(|project| project.project_id == id)
        })
        .unwrap_or(0)
        .min(model.projects.len().saturating_sub(1));
    if open_details {
        load_details(model)?;
    }
    Ok(())
}

fn load_details(model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    let project_id = model
        .selected_project()
        .map(|project| project.project_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no authorized Cutex Project is selected"))?;
    let invocation = model
        .invocation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("authenticated Agent Manager context is unavailable"))?;
    let provider = AgentManagementProvider::open_default()?;
    let observer = CliProjectRuntimeObserver::load()?;
    model.details = Some(
        provider
            .read_cutex_project(invocation, &project_id, &observer)
            .map_err(|error| anyhow::anyhow!(error))?,
    );
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
    let request = ProjectPresentationUpdateRequest {
        project_id: details.project_id.clone(),
        expected_presentation_revision: details.presentation.revision,
        presentation: ProjectPresentationInput {
            display_name: editor.display_name.clone(),
            badge_label: editor.badge_label.clone(),
            color: editor.color,
        },
    };
    let invocation = model
        .invocation
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("authenticated Agent Manager context is unavailable"))?;
    AgentManagementProvider::open_default()?
        .update_project_presentation(invocation, &request)
        .map_err(|error| anyhow::anyhow!(error))?;
    model.editor = None;
    reload(model, true)?;
    model.notice = Some("Project presentation updated".to_string());
    Ok(())
}

fn run_loop(terminal: &mut ProjectTerminal, model: &mut CutexProjectsModel) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| render(frame, model))?;
        if !event::poll(POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        model.notice = None;
        match model.view {
            ProjectView::List => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                KeyCode::Up => model.selected = model.selected.saturating_sub(1),
                KeyCode::Down => {
                    model.selected =
                        (model.selected + 1).min(model.projects.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Err(error) = load_details(model) {
                        model.failure = Some(format!("{error:#}"));
                    }
                }
                KeyCode::Char('r') => match reload(model, false) {
                    Ok(()) => model.failure = None,
                    Err(error) => model.failure = Some(format!("{error:#}")),
                },
                _ => {}
            },
            ProjectView::Details => match key.code {
                KeyCode::Esc | KeyCode::Left => model.view = ProjectView::List,
                KeyCode::Char('e') => model.begin_editor(),
                KeyCode::Char('r') => {
                    if let Err(error) = load_details(model) {
                        model.failure = Some(format!("{error:#}"));
                    }
                }
                _ => {}
            },
            ProjectView::Editor => match key.code {
                KeyCode::Esc => {
                    model.editor = None;
                    model.view = ProjectView::Details;
                    model.failure = None;
                }
                KeyCode::Tab | KeyCode::Down => {
                    if let Some(editor) = model.editor.as_mut() {
                        editor.field = (editor.field + 1) % 3;
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    if let Some(editor) = model.editor.as_mut() {
                        editor.field = (editor.field + 2) % 3;
                    }
                }
                KeyCode::Left | KeyCode::Right => {
                    if let Some(editor) = model.editor.as_mut().filter(|editor| editor.field == 2) {
                        let index = ProjectPaletteColor::ALL
                            .iter()
                            .position(|color| *color == editor.color)
                            .unwrap_or(0);
                        let offset = usize::from(key.code == KeyCode::Right);
                        editor.color =
                            ProjectPaletteColor::ALL[(index + ProjectPaletteColor::ALL.len() - 1
                                + offset * 2)
                                % ProjectPaletteColor::ALL.len()];
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
        }
    }
}

fn render(frame: &mut Frame<'_>, model: &CutexProjectsModel) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Cutex Projects",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  provider-owned permission boundaries",
                Style::new().fg(Color::DarkGray),
            ),
        ])),
        areas[0],
    );
    match model.view {
        ProjectView::List => render_list(frame, areas[1], model),
        ProjectView::Details => render_details(frame, areas[1], model.details.as_ref()),
        ProjectView::Editor => {
            render_details(frame, areas[1], model.details.as_ref());
            render_editor(frame, areas[1], model.editor.as_ref());
        }
    }
    let footer = model
        .failure
        .as_deref()
        .or(model.notice.as_deref())
        .unwrap_or(match model.view {
            ProjectView::List => "Enter details  r refresh  Esc back",
            ProjectView::Details => "e edit presentation  r refresh  Esc list",
            ProjectView::Editor => "Tab field  Left/Right color  Enter save  Esc cancel",
        });
    frame.render_widget(
        Paragraph::new(footer)
            .wrap(Wrap { trim: true })
            .style(Style::new().fg(if model.failure.is_some() {
                Color::Red
            } else {
                Color::DarkGray
            })),
        areas[2],
    );
}

fn render_list(frame: &mut Frame<'_>, area: Rect, model: &CutexProjectsModel) {
    let rows = model.projects.iter().map(|project| {
        Row::new([
            Cell::from(project.presentation.badge_label.clone())
                .style(Style::new().fg(palette_color(project.presentation.color))),
            Cell::from(project.presentation.display_name.clone()),
            Cell::from(project.project_id.to_string()),
            Cell::from(match project.access_role {
                ProjectAccessRole::PrimaryDirector => "primary",
                ProjectAccessRole::AgentOperator => "operator",
            }),
            Cell::from(project.director_cutex_session_id.as_str().to_string()),
            Cell::from(project.operator_count.to_string()),
            Cell::from(project.active_member_count.to_string()),
            Cell::from(project.retired_member_count.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(22),
            Constraint::Min(16),
            Constraint::Length(9),
            Constraint::Length(24),
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new([
            "BADGE",
            "NAME",
            "PROJECT ID",
            "YOUR ROLE",
            "PRIMARY DIRECTOR",
            "OPS",
            "ACTIVE",
            "RETIRED",
        ])
        .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD)),
    )
    .block(Block::bordered().title(" Authorized Cutex Projects "))
    .row_highlight_style(
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("> ");
    let mut state =
        TableState::default().with_selected((!model.projects.is_empty()).then_some(model.selected));
    frame.render_stateful_widget(table, area, &mut state);
    if model.projects.is_empty() {
        frame.render_widget(
            Paragraph::new("No authorized Cutex Projects are visible for this Agent Manager.")
                .alignment(Alignment::Center)
                .style(Style::new().fg(Color::DarkGray)),
            Rect {
                y: area.y.saturating_add(3),
                height: area.height.saturating_sub(4),
                ..area
            },
        );
    }
}

fn render_details(frame: &mut Frame<'_>, area: Rect, details: Option<&CutexProjectWorkspace>) {
    let Some(project) = details else {
        frame.render_widget(
            Paragraph::new("Project details are unavailable.")
                .block(Block::bordered().title(" Cutex Project ")),
            area,
        );
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("[{}] ", project.presentation.badge_label),
                Style::new()
                    .fg(palette_color(project.presentation.color))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                project.presentation.display_name.clone(),
                Style::new().add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!("Canonical project_id: {}", project.project_id)),
        Line::from(format!("Authority epoch: {}", project.authority_epoch)),
        Line::from(format!(
            "Primary Director: {}{}",
            project.director.cutex_session_id.as_str(),
            project
                .director
                .member
                .as_ref()
                .map(|member| format!("  [{}]", lifecycle_label(member.lifecycle)))
                .unwrap_or_else(|| "  [provider seat only]".to_string())
        )),
        Line::from(format!(
            "Your role: {}",
            match project.access_role {
                ProjectAccessRole::PrimaryDirector => "Primary Director",
                ProjectAccessRole::AgentOperator => "Agent Operator",
            }
        )),
        Line::from(format!(
            "Operator grant revision: {}",
            project.operator_grant_revision
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Agent Operators",
            Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        )),
    ];
    if project.agent_operators.is_empty() {
        lines.push(Line::from("  None"));
    } else {
        lines.extend(project.agent_operators.iter().map(|operator| {
            Line::from(format!(
                "  {}  [{}]  {}",
                operator.member.agent.spec.name,
                lifecycle_label(operator.member.lifecycle),
                operator.grant.operator_cutex_session_id.as_str()
            ))
        }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Ordinary active/offline Agents",
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    if project.active_agents.is_empty() {
        lines.push(Line::from("  None"));
    } else {
        lines.extend(project.active_agents.iter().map(|member| {
            Line::from(format!(
                "  {}  [{}]  {}",
                member.agent.spec.name,
                lifecycle_label(member.lifecycle),
                member.agent.cutex_session_id.as_str()
            ))
        }));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Retired members",
        Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD),
    )));
    if project.retired_agents.is_empty() {
        lines.push(Line::from("  None"));
    } else {
        lines.extend(project.retired_agents.iter().map(|member| {
            Line::from(format!(
                "  {}  retired {}  {}",
                member.agent.spec.name,
                member
                    .agent
                    .retired_at
                    .as_ref()
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                member.agent.cutex_session_id.as_str()
            ))
        }));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Cutex Project details "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, editor: Option<&PresentationEditor>) {
    let Some(editor) = editor else {
        return;
    };
    let popup = centered_rect(58, 9, area);
    let field = |index, label: &str, value: String| {
        Line::from(vec![
            Span::styled(
                format!("{label}: "),
                Style::new().fg(if editor.field == index {
                    Color::Cyan
                } else {
                    Color::Gray
                }),
            ),
            Span::raw(value),
        ])
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            field(0, "Display name", editor.display_name.clone()),
            field(1, "Badge (1-2 cells)", editor.badge_label.clone()),
            field(2, "Palette color", editor.color.token().to_string()),
            Line::from(""),
            Line::from("Canonical project_id and authority are immutable here."),
        ])
        .block(Block::bordered().title(" Edit presentation only "))
        .wrap(Wrap { trim: false }),
        popup,
    );
}

fn lifecycle_label(value: ProjectMemberLifecycle) -> &'static str {
    match value {
        ProjectMemberLifecycle::Online => "online",
        ProjectMemberLifecycle::Offline => "offline",
        ProjectMemberLifecycle::Unavailable => "observation unavailable",
    }
}

pub(super) fn palette_color(value: ProjectPaletteColor) -> Color {
    match value {
        ProjectPaletteColor::Cyan => Color::Cyan,
        ProjectPaletteColor::Blue => Color::LightBlue,
        ProjectPaletteColor::Green => Color::LightGreen,
        ProjectPaletteColor::Magenta => Color::LightMagenta,
        ProjectPaletteColor::Yellow => Color::Yellow,
        ProjectPaletteColor::Red => Color::LightRed,
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

struct CliProjectRuntimeObserver {
    sessions: cutex::session::model::CutexSessionStore,
    live_agents: Vec<cutex::agent_bus::model::AgentBusAgent>,
}

impl CliProjectRuntimeObserver {
    fn load() -> anyhow::Result<Self> {
        Ok(Self {
            sessions: load_cutex_session_store()?,
            live_agents: cutex::agent_bus::client::agent_bus_fetch_agents_if_healthy(
                &cutex::config::store::load_codez_config(),
            ),
        })
    }
}

impl ProjectRuntimeObserver for CliProjectRuntimeObserver {
    fn observe(
        &self,
        cutex_session_id: &CutexSessionId,
    ) -> Result<AgentRuntimeObservation, AgentManagementError> {
        let record = self
            .sessions
            .sessions
            .get(cutex_session_id.as_str())
            .ok_or(AgentManagementError::NotFound("durable_session_not_found"))?;
        let native_session_id = record
            .codex_session_id
            .clone()
            .ok_or(AgentManagementError::NotFound("native_session_not_found"))?;
        let runtime_agent_ids = self
            .live_agents
            .iter()
            .filter(|agent| agent.cutex_session_id.as_deref() == Some(cutex_session_id.as_str()))
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        Ok(AgentRuntimeObservation {
            cutex_session_id: cutex_session_id.clone(),
            native_session_id,
            active: record.is_active(),
            cwd: cutex_session_launch_cwd(record).to_string(),
            profile: record.profile.clone().unwrap_or_default(),
            runtime_backend: serde_json::to_value(record.runtime_backend)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            model: record.model_defaults.clone().unwrap_or_default(),
            reasoning: record.reasoning_defaults.clone().unwrap_or_default(),
            permissions: record.permission_defaults.clone().unwrap_or_default(),
            approval_policy: record.approval_policy.clone().unwrap_or_default(),
            sandbox_mode: record.sandbox_mode.clone().unwrap_or_default(),
            groups: record.agent_groups.clone(),
            runtime_generation: record.runtime_generation,
            runtime_agent_ids,
            app_server_runtime: record.app_server_runtime.is_some(),
            agent_bus_endpoint_ids: Vec::new(),
        })
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

    fn rendered(model: &CutexProjectsModel) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn empty_and_error_state_never_falls_back_to_codex_workspaces() {
        let model = CutexProjectsModel::empty_with_failure("not authorized");
        let text = rendered(&model);
        assert!(text.contains("Cutex Projects"));
        assert!(text.contains("No authorized Cutex Projects"));
        assert!(text.contains("not authorized"));
        assert!(!text.contains("Codex Workspaces"));
    }

    #[test]
    fn editor_exposes_only_non_authoritative_presentation_fields() {
        let mut model = CutexProjectsModel::empty_with_failure("test");
        model.failure = None;
        model.view = ProjectView::Editor;
        model.editor = Some(PresentationEditor {
            display_name: "Alpha".to_string(),
            badge_label: "A".to_string(),
            color: ProjectPaletteColor::Cyan,
            field: 0,
        });
        let text = rendered(&model);
        assert!(text.contains("Display name"));
        assert!(text.contains("Badge (1-2 cells)"));
        assert!(text.contains("Palette color"));
        assert!(text.contains("project_id and authority are immutable"));
        assert!(!text.contains("Rotate Director"));
        assert!(!text.contains("Delete Project"));
    }
}
