//! Read-only, Human-authenticated Task Service workspace.
//!
//! This module intentionally owns its own small model and refresh worker.  It
//! never reads cwd, collaboration groups, native workspaces, or names to
//! determine task ownership: the Task Service Director query is the source of
//! task state, and the Management route preserves the exact current Director
//! seat plus Primary Director project-authority scope.

use ratatui::widgets::Wrap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Stdout;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::event::{ Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cutex::agent_bus::client::agent_bus_fetch_agents_if_healthy;
use cutex::agent_bus::model::AgentBusAgent;
use cutex::agent_management::{ProjectId, ProjectPaletteColor};
use cutex::config::store::load_codez_config;
use cutex::management::control_plane::{
    HumanManagementTaskQueryRequest, HumanManagementTaskQueryResponse,
    HumanManagementTaskQuerySchema,
};
use cutex::task_service::{
    ActionId, DirectorActionStatus, DirectorAssignmentView, DirectorAttemptView,
    DirectorQuerySelector,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use super::session_tui_terminal::Terminal;
use tui_input::{Input, InputRequest};
use uuid::Uuid;

use super::management_control_plane::ManagementControlClient;
use super::session_tui::footer_hints;
use super::session_tui_input::{self as input_policy, Command};
use super::session_tui_view::{self as views, DetailScroll};
use super::session_tui_workspace::{primary_panel_shortcut, PrimaryPanel, PrimaryPanelOutcome};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

type TaskTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Queued,
    Assigned,
    Running,
    ReviewReady,
    RetryPending,
    Blocked,
    Closed,
}

impl TaskState {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::ReviewReady => "review",
            Self::RetryPending => "retry pending",
            Self::Blocked => "blocked",
            Self::Closed => "closed",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Queued | Self::Assigned | Self::RetryPending => Style::new().fg(crate::cli_app::session_tui_layout::focus()),
            Self::Running => Style::new().fg(crate::cli_app::session_tui_layout::success()),
            Self::ReviewReady => Style::new().fg(crate::cli_app::session_tui_layout::accent()),
            Self::Blocked => Style::new().fg(crate::cli_app::session_tui_layout::warning()),
            Self::Closed => Style::new().fg(crate::cli_app::session_tui_layout::muted()),
        }
    }

    fn is_closed(self) -> bool {
        matches!(self, Self::Closed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentJoin {
    display_name: String,
    runtime_id: String,
    availability: &'static str,
    activity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskProjectPresentation {
    display_name: String,
    badge_label: String,
    color: ProjectPaletteColor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskRow {
    project_id: String,
    project_presentation: Option<TaskProjectPresentation>,
    task_id: String,
    task_revision: u64,
    assignment_id: String,
    assignee_session_id: String,
    agent: Option<AgentJoin>,
    state: TaskState,
    phase: Option<String>,
    attempt_number: Option<u64>,
    updated_at: String,
    activity: String,
    status_summary: Option<String>,
    result_reference: Option<String>,
    last_output: Option<String>,
    last_tool_call: Option<String>,
}

impl TaskRow {
    fn selection_key(&self) -> &str {
        &self.assignment_id
    }

    fn agent_label(&self) -> String {
        match &self.agent {
            Some(agent) => format!("{} ({})", agent.display_name, agent.availability),
            None => format!("unavailable ({})", self.assignee_session_id),
        }
    }

    fn project_label(&self) -> String {
        match &self.project_presentation {
            Some(presentation) => format!(
                "{} {} ({})",
                presentation.badge_label, presentation.display_name, self.project_id
            ),
            None if self.project_id == "-" => "-".to_string(),
            None => format!("unavailable ({})", self.project_id),
        }
    }

    fn agent_activity_label(&self) -> &str {
        self.agent
            .as_ref()
            .map(|agent| agent.activity.as_str())
            .unwrap_or("-")
    }

    fn matches(&self, filter: &str) -> bool {
        let filter = filter.trim().to_ascii_lowercase();
        if filter.is_empty() {
            return true;
        }
        [
            self.task_id.as_str(),
            self.assignment_id.as_str(),
            self.assignee_session_id.as_str(),
            self.project_id.as_str(),
            self.state.label(),
            self.phase.as_deref().unwrap_or(""),
            self.activity.as_str(),
        ]
        .into_iter()
        .chain(self.project_presentation.iter().flat_map(|presentation| {
            [
                presentation.display_name.as_str(),
                presentation.badge_label.as_str(),
            ]
        }))
        .chain(self.agent.iter().flat_map(|agent| {
            [
                agent.display_name.as_str(),
                agent.runtime_id.as_str(),
                agent.availability,
            ]
        }))
        .any(|value| value.to_ascii_lowercase().contains(&filter))
    }
}

fn exact_agent_join(
    agents: &[AgentBusAgent],
    activities: &HashMap<String, super::session_tui::SelectorActivity>,
    assignee_session_id: &str,
    now: DateTime<Utc>,
) -> Option<AgentJoin> {
    // `cutex_session_id` is the sole join key. In particular, display names,
    // runtime IDs, thread names, cwd, and groups must never act as fallbacks.
    let mut matches = agents
        .iter()
        .filter(|agent| agent.cutex_session_id.as_deref() == Some(assignee_session_id))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    (matches.len() == 1).then(|| {
        let agent = matches[0];
        AgentJoin {
            display_name: agent.name.clone(),
            runtime_id: agent.id.clone(),
            availability: "online",
            activity: super::session_tui::format_selector_activity(
                activities.get(assignee_session_id),
                now,
            ),
        }
    })
}

fn map_state(
    assignment: &DirectorAssignmentView,
    active: Option<&DirectorAttemptView>,
) -> TaskState {
    if assignment.state == "closed" || assignment.closure_reason.is_some() {
        return TaskState::Closed;
    }
    if assignment.state == "retry_pending" { return TaskState::RetryPending; }
    match active.map(|attempt| attempt.phase.as_str()) {
        Some("blocked") => TaskState::Blocked,
        Some("review_ready") => TaskState::ReviewReady,
        Some("running") => TaskState::Running,
        Some("completed" | "failed" | "cancelled" | "aborted") => TaskState::Assigned,
        _ if assignment.state == "awaiting_ack" => TaskState::Queued,
        _ => TaskState::Assigned,
    }
}

fn latest_attempt(assignment: &DirectorAssignmentView) -> Option<&DirectorAttemptView> {
    assignment
        .attempts
        .iter()
        .max_by_key(|attempt| attempt.attempt_number)
}

fn task_rows(
    receipt: &cutex::task_service::DirectorActionReceipt,
    agents: &[AgentBusAgent],
    project_presentations: &BTreeMap<ProjectId, TaskProjectPresentation>,
    activities: &HashMap<String, super::session_tui::SelectorActivity>,
    now: DateTime<Utc>,
) -> Vec<TaskRow> {
    let mut rows = receipt
        .assignments
        .iter()
        .map(|assignment| {
            let active = assignment
                .active_attempt_number
                .and_then(|number| {
                    assignment
                        .attempts
                        .iter()
                        .find(|attempt| attempt.attempt_number == number)
                })
                .or_else(|| latest_attempt(assignment));
            let state = map_state(assignment, active);
            let updated_at = active
                .map(|attempt| attempt.updated_at.clone())
                .or_else(|| assignment.closed_at.clone())
                .or_else(|| assignment.acknowledged_at.clone())
                .unwrap_or_else(|| assignment.created_at.clone());
            TaskRow {
                project_id: assignment
                    .project_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "-".to_string()),
                project_presentation: assignment
                    .project_id
                    .as_ref()
                    .and_then(|project_id| project_presentations.get(project_id).cloned()),
                task_id: assignment.task_id.as_str().to_string(),
                task_revision: assignment.task_revision.get(),
                assignment_id: assignment.assignment_id.as_str().to_string(),
                assignee_session_id: assignment.assignee_cutex_session_id.as_str().to_string(),
                agent: exact_agent_join(
                    agents,
                    activities,
                    assignment.assignee_cutex_session_id.as_str(),
                    now,
                ),
                state,
                phase: active.map(|attempt| attempt.phase.clone()),
                attempt_number: active.map(|attempt| attempt.attempt_number),
                updated_at,
                activity: merged_activity(active),
                status_summary: active.and_then(|attempt| attempt.latest_status_summary.clone()),
                result_reference: active.and_then(|attempt| attempt.result_reference.clone()),
                last_output: active
                    .and_then(|attempt| attempt.last_output.as_ref())
                    .map(|output| output.display_text.clone()),
                last_tool_call: active
                    .and_then(|attempt| attempt.last_tool_call.as_ref())
                    .map(|tool| tool.display_text.clone()),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        left.state
            .is_closed()
            .cmp(&right.state.is_closed())
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.assignment_id.cmp(&right.assignment_id))
    });
    rows
}

/// Resolve display metadata only after the Director query has scoped task
/// records. Each lookup is an exact canonical `ProjectId` map lookup; names,
/// badges, cwd, groups, and native workspaces are never used as keys.
fn exact_project_presentations(
    response: &HumanManagementTaskQueryResponse,
) -> BTreeMap<ProjectId, TaskProjectPresentation> {
    let receipt_project_ids = response
        .receipt
        .assignments
        .iter()
        .filter_map(|assignment| assignment.project_id.as_ref().cloned())
        .collect::<BTreeSet<_>>();
    let authoritative_project_ids = response
        .project_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    receipt_project_ids
        .into_iter()
        .filter_map(|project_id| {
            authoritative_project_ids
                .contains(&project_id)
                .then(|| response.project_presentations.get(&project_id))
                .flatten()
                .map(|presentation| {
                    (
                        project_id,
                        TaskProjectPresentation {
                            display_name: presentation.display_name.clone(),
                            badge_label: presentation.badge_label.clone(),
                            color: presentation.color,
                        },
                    )
                })
        })
        .collect()
}

fn merged_activity(attempt: Option<&DirectorAttemptView>) -> String {
    let Some(attempt) = attempt else {
        return "-".to_string();
    };
    let mut values = Vec::new();
    if let Some(summary) = attempt.latest_status_summary.as_deref() {
        values.push(summary);
    }
    if let Some(output) = attempt.last_output.as_ref() {
        values.push(output.display_text.as_str());
    }
    if let Some(tool) = attempt.last_tool_call.as_ref() {
        values.push(tool.display_text.as_str());
    }
    if let Some(result) = attempt.result_reference.as_deref() {
        values.push(result);
    }
    let text = values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or("-");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn bounded_single_line(value: &str, max: usize) -> String {
    views::clipped(&value.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

#[derive(Debug, Clone)]
pub(super) struct TaskModel {
    rows: Vec<TaskRow>,
    selected_assignment_id: Option<String>,
    query: Input,
    filter_focused: bool,
    show_closed: bool,
    detail: bool,
    detail_scroll: DetailScroll,
    loading: bool,
    warning: Option<String>,
    refreshed_at: Option<Instant>,
    pub(super) open_settings_requested: bool,
}

impl Default for TaskModel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected_assignment_id: None,
            query: Input::default(),
            filter_focused: false,
            show_closed: false,
            detail: false,
            detail_scroll: DetailScroll::default(),
            loading: true,
            warning: None,
            refreshed_at: None,
            open_settings_requested: false,
        }
    }
}

impl TaskModel {
    fn visible_indices(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                (self.show_closed || !row.state.is_closed()) && row.matches(self.query.value())
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn selected_visible_index(&self) -> Option<usize> {
        let visible = self.visible_indices();
        self.selected_assignment_id
            .as_ref()
            .and_then(|id| {
                visible
                    .iter()
                    .position(|index| self.rows[*index].assignment_id == *id)
            })
            .or_else(|| (!visible.is_empty()).then_some(0))
    }

    fn selected_row(&self) -> Option<&TaskRow> {
        let visible = self.visible_indices();
        self.selected_visible_index()
            .and_then(|index| self.rows.get(visible[index]))
    }

    fn retain_selection(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.selected_assignment_id = None;
        } else if !visible.iter().any(|index| {
            self.selected_assignment_id.as_deref() == Some(self.rows[*index].selection_key())
        }) {
            self.selected_assignment_id = Some(self.rows[visible[0]].assignment_id.clone());
        }
    }

    fn replace_rows(&mut self, rows: Vec<TaskRow>, now: Instant) {
        self.rows = rows;
        self.loading = false;
        self.warning = None;
        self.refreshed_at = Some(now);
        self.retain_selection();
    }

    fn set_error(&mut self, error: String) {
        self.loading = false;
        self.warning = Some(bounded_single_line(&error, 180));
        self.retain_selection();
    }

    fn move_selection(&mut self, direction: isize) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.selected_assignment_id = None;
            return;
        }
        let current = self.selected_visible_index().unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(visible.len() - 1)
        } else {
            (current + 1) % visible.len()
        };
        self.selected_assignment_id = Some(self.rows[visible[next]].assignment_id.clone());
    }
}

#[derive(Debug, Clone)]
struct RefreshCadence {
    interval: Duration,
    next_due: Instant,
}

impl RefreshCadence {
    fn new(now: Instant) -> Self {
        Self {
            interval: REFRESH_INTERVAL,
            next_due: now,
        }
    }

    fn due(&mut self, now: Instant) -> bool {
        if now < self.next_due {
            return false;
        }
        self.next_due = now + self.interval;
        true
    }

    fn request_now(&mut self, now: Instant) {
        self.next_due = now;
    }
}

enum RefreshResult {
    Rows(Vec<TaskRow>),
    Error(String),
}

fn spawn_refresh(sender: mpsc::Sender<RefreshResult>) {
    thread::spawn(move || {
        let config = load_codez_config();
        let request = match ActionId::new(format!("tasks-query-{}", Uuid::new_v4())) {
            Ok(action_id) => HumanManagementTaskQueryRequest {
                schema: HumanManagementTaskQuerySchema::V1,
                action_id,
                selector: DirectorQuerySelector::All {},
            },
            Err(error) => {
                let _ = sender.send(RefreshResult::Error(format!(
                    "invalid query identifier: {error:?}"
                )));
                return;
            }
        };
        let result = ManagementControlClient::connect()
            .and_then(|client| client.tasks(&request))
            .map_err(|error| format!("Management Task query unavailable: {error:#}"))
            .and_then(|response| match response.receipt.status {
                DirectorActionStatus::CurrentState | DirectorActionStatus::Committed => {
                    let activities = super::session_tui::current_activity_by_durable_session()
                        .unwrap_or_default();
                    Ok(task_rows(
                        &response.receipt,
                        &agent_bus_fetch_agents_if_healthy(&config),
                        &exact_project_presentations(&response),
                        &activities,
                        Utc::now(),
                    ))
                }
                _ => Err(format!(
                    "Task Service Director query returned {}{}",
                    director_status_label(response.receipt.status),
                    response
                        .receipt
                        .code
                        .as_deref()
                        .map(|code| format!(" ({code})"))
                        .unwrap_or_default()
                )),
            });
        let _ = sender.send(match result {
            Ok(rows) => RefreshResult::Rows(rows),
            Err(error) => RefreshResult::Error(error),
        });
    });
}

fn director_status_label(status: DirectorActionStatus) -> &'static str {
    match status {
        DirectorActionStatus::Committed => "committed",
        DirectorActionStatus::CurrentState => "current_state",
        DirectorActionStatus::Conflict => "conflict",
        DirectorActionStatus::NoWrite => "no_write",
        DirectorActionStatus::ResponseUncertain => "response_uncertain",
    }
}

pub(super) fn run(
    terminal: &mut TaskTerminal,
    events: &mut super::session_tui::ShellEvents,
    previous_model: Option<TaskModel>,
) -> anyhow::Result<(PrimaryPanelOutcome, TaskModel)> {
    let mut model = previous_model.unwrap_or_default();
    let outcome = run_loop(terminal, events, &mut model)?;
    Ok((outcome, model))
}

fn run_loop(
    terminal: &mut TaskTerminal,
    events: &mut super::session_tui::ShellEvents,
    model: &mut TaskModel,
) -> anyhow::Result<PrimaryPanelOutcome> {
    let (sender, receiver) = mpsc::channel();
    let mut cadence = RefreshCadence::new(Instant::now());
    let mut request_in_flight = false;
    loop {
        let now = Instant::now();
        if !request_in_flight && cadence.due(now) {
            model.loading = true;
            spawn_refresh(sender.clone());
            request_in_flight = true;
        }
        match receiver.try_recv() {
            Ok(RefreshResult::Rows(rows)) => {
                model.replace_rows(rows, now);
                request_in_flight = false;
            }
            Ok(RefreshResult::Error(error)) => {
                model.set_error(error);
                request_in_flight = false;
            }
            Err(TryRecvError::Disconnected) => {
                model.set_error("Tasks refresh worker stopped".to_string());
                request_in_flight = false;
            }
            Err(TryRecvError::Empty) => {}
        }
        terminal.draw(|frame| render(frame, model))?;
        if let Some(event) = events.next()? {
            if let Some(outcome) = handle_event(model, &mut cadence, event) { return Ok(outcome); }
        }
    }
}

fn handle_event(model: &mut TaskModel, cadence: &mut RefreshCadence, event: Event) -> Option<PrimaryPanelOutcome> {
    match event {
        Event::Paste(text) if model.filter_focused => {
            input_policy::paste(&mut model.query, &text);
            model.retain_selection();
            None
        }
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => handle_key(model, cadence, key),
        _ => None,
    }
}

fn handle_key(
    model: &mut TaskModel,
    cadence: &mut RefreshCadence,
    key: KeyEvent,
) -> Option<PrimaryPanelOutcome> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(PrimaryPanelOutcome::Exit);
    }
    if input_policy::resolve(key) == Some(Command::Settings) {
        model.open_settings_requested = true;
        return Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents));
    }
    if let Some(panel) = primary_panel_shortcut(key) {
        return (panel != PrimaryPanel::Tasks).then_some(PrimaryPanelOutcome::Switch(panel));
    }
    if key.modifiers == KeyModifiers::NONE && key.code == KeyCode::F(5) {
        cadence.request_now(Instant::now());
        return None;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('r' | 'R')) {
        cadence.request_now(Instant::now());
        return None;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('a' | 'A')) {
        model.show_closed = !model.show_closed;
        model.retain_selection();
        return None;
    }
    if input_policy::resolve(key) == Some(Command::Inspect) {
        model.filter_focused = false;
        model.detail = model.selected_row().is_some();
        model.detail_scroll.reset();
        return None;
    }
    if model.filter_focused {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {
                model.filter_focused = false
            }
            KeyCode::Backspace => {
                model.query.handle(InputRequest::DeletePrevChar);
                model.retain_selection();
            }
            KeyCode::Delete => {
                model.query.handle(InputRequest::DeleteNextChar);
                model.retain_selection();
            }
            KeyCode::Left => {
                model.query.handle(InputRequest::GoToPrevChar);
            }
            KeyCode::Right => {
                model.query.handle(InputRequest::GoToNextChar);
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
        }
        return None;
    }
    if model.detail && model.detail_scroll.handle(key) {
        return None;
    }
    match key.code {
        KeyCode::Esc => {
            if model.detail {
                model.detail = false;
            } else if !model.query.value().is_empty() {
                model.query.reset();
                model.retain_selection();
            } else {
                return Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents));
            }
        }
        KeyCode::Left if model.detail => model.detail = false,
        KeyCode::Right if model.detail => {}
        KeyCode::Left => {
            return PrimaryPanel::Tasks
                .adjacent(false)
                .map(PrimaryPanelOutcome::Switch)
        }
        KeyCode::Right => return PrimaryPanel::Tasks.adjacent(true).map(PrimaryPanelOutcome::Switch),
        KeyCode::Tab if model.detail => model.detail = false,
        KeyCode::Tab => {
            model.detail = model.selected_row().is_some();
            model.detail_scroll.reset();
        }
        KeyCode::BackTab => model.detail = false,
        KeyCode::Up => model.move_selection(-1),
        KeyCode::Down => model.move_selection(1),
        KeyCode::Enter => {
            model.detail = model.selected_row().is_some();
            model.detail_scroll.reset();
        }
        KeyCode::Char('/') => { model.detail = false; model.filter_focused = true; },
        _ => {}
    }
    None
}

fn render(frame: &mut Frame<'_>, model: &TaskModel) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .split(area);
    let visible_count = model.visible_indices().len();
    let mode = if model.show_closed {
        "all history"
    } else {
        "active"
    };
    frame.render_widget(
        Paragraph::new(crate::cli_app::session_tui_layout::tabs(
            PrimaryPanel::Tasks,
            area.width,
        )),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Cutex", Style::new().fg(crate::cli_app::session_tui_layout::focus()).add_modifier(Modifier::BOLD)),
            Span::styled(" Tasks", Style::new().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw(format!("  {visible_count} {mode}")),
            Span::styled("  read-only", Style::new().fg(Color::DarkGray)),
        ])),
        chunks[1],
    );
    let panes = crate::cli_app::session_tui_layout::list_details(chunks[2], true);
    render_filter(frame, panes.filter, model);
    render_table(frame, panes.list, model);
    let wide_inspector = panes.details.is_some();
    if let Some(details) = panes.details {
        render_detail(frame, details, model, model.detail);
    }
    let footer = if model.filter_focused {
        footer_hints(&[
            ("Type", "filter"),
            ("Enter/Esc", "finish"),
            ("Ctrl+A", "history"),
        ])
    } else if model.detail {
        footer_hints(&[
            ("↑/↓ PgUp/PgDn", "scroll"),
            ("F5", "refresh"),
            ("Esc/Tab", "close"),
        ])
    } else {
        footer_hints(&[
            ("↑/↓", "select"),
            ("Enter/Alt+I", "inspect"),
            ("←/→", "panels"),
            ("/", "filter"),
            ("Ctrl+A", "history"),
            ("F5", "refresh"),
            ("Esc", "back"),
        ])
    };
    frame.render_widget(
        Paragraph::new(Line::from(footer)).wrap(Wrap { trim: true }).style(Style::new().fg(Color::DarkGray)),
        chunks[4],
    );
    frame.render_widget(Paragraph::new(model.warning.as_deref().unwrap_or("Ready")).style(Style::new().fg(if model.warning.is_some() { crate::cli_app::session_tui_layout::warning() } else { crate::cli_app::session_tui_layout::muted() })), chunks[3]);
    if model.detail && !wide_inspector {
        render_detail(frame, chunks[2], model, true);
    }
}

fn render_filter(frame: &mut Frame<'_>, area: Rect, model: &TaskModel) {
    let title = if model.show_closed { " Filter tasks · all history [/] " } else { " Filter tasks · active [/] " };
    input_policy::render_input(frame, area, &model.query, title, model.filter_focused);
}

fn render_table(frame: &mut Frame<'_>, area: Rect, model: &TaskModel) {
    let visible = model.visible_indices();
    let columns = task_columns(area.width.saturating_sub(2));
    let selected = model.selected_visible_index();
    let rows = visible
        .iter()
        .enumerate()
        .map(|(visible_index, index)| {
            task_table_row(
                &model.rows[*index],
                selected == Some(visible_index),
                &columns,
            )
        })
        .collect::<Vec<_>>();
    let header = Row::new(columns.iter().map(|(column, _)| column.label()))
        .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let table = Table::new(
        rows,
        columns.iter().map(|(_, width)| Constraint::Length(*width)),
    )
    .header(header)
    .block(Block::bordered().title(if model.show_closed {
        " Cutex Tasks + history "
    } else {
        " Cutex Tasks "
    }))
    .column_spacing(1)
    .highlight_symbol("> ")
    // Row styles carry selection so semantic status and badge colors survive.
    .row_highlight_style(Style::new());
    let mut state = TableState::default().with_selected(model.selected_visible_index());
    frame.render_stateful_widget(table, area, &mut state);
    if visible.is_empty() && area.height > 3 {
        let message = if model.warning.is_some() {
            "Task data is unavailable; press F5 to retry."
        } else if model.query.value().is_empty() {
            "No active tasks in the authenticated Director project scope."
        } else {
            "No tasks match this filter."
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::new().fg(Color::DarkGray)),
            Rect {
                x: area.x + 2,
                y: area.y + 3,
                width: area.width.saturating_sub(4),
                height: 1,
            },
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskColumn {
    Task,
    State,
    Agent,
    AgentActivity,
    Attempt,
    Updated,
    Activity,
}

impl TaskColumn {
    fn label(self) -> &'static str {
        match self {
            Self::Task => "TASK",
            Self::State => "STATE",
            Self::Agent => "AGENT",
            Self::AgentActivity => "AGENT ACT",
            Self::Attempt => "TRY",
            Self::Updated => "UPDATED",
            Self::Activity => "ACTIVITY",
        }
    }
}

fn task_columns(width: u16) -> Vec<(TaskColumn, u16)> {
    // Two cells are reserved for the selection marker and one between columns.
    if width >= 110 {
        return vec![
            (TaskColumn::Task, 26),
            (TaskColumn::State, 8),
            (TaskColumn::Agent, 18),
            (TaskColumn::AgentActivity, 10),
            (TaskColumn::Attempt, 4),
            (TaskColumn::Updated, 8),
            (TaskColumn::Activity, width - 82),
        ];
    }
    if width >= 78 {
        return vec![
            (TaskColumn::Task, 20),
            (TaskColumn::State, 8),
            (TaskColumn::Agent, 14),
            (TaskColumn::AgentActivity, 9),
            (TaskColumn::Attempt, 3),
            (TaskColumn::Updated, 7),
            (TaskColumn::Activity, width - 69),
        ];
    }
    if width >= 52 {
        return vec![
            (TaskColumn::Task, width - 24),
            (TaskColumn::State, 8),
            (TaskColumn::Attempt, 4),
            (TaskColumn::Updated, 7),
        ];
    }
    if width < 38 {
        if width < 14 {
            return vec![(TaskColumn::Task, width.saturating_sub(2).max(1))];
        }
        return vec![
            (TaskColumn::Task, width.saturating_sub(11)),
            (TaskColumn::State, 8),
        ];
    }
    vec![
        (TaskColumn::Task, width - 19),
        (TaskColumn::State, 8),
        (TaskColumn::Updated, 7),
    ]
}

fn task_name_line(row: &TaskRow, width: usize) -> Line<'static> {
    if width < 6 {
        return Line::from(views::clipped(&row.task_id, width));
    }
    let badge = row.project_presentation.as_ref().map_or_else(
        || Span::raw("    "),
        |project| {
            let label = views::clipped(&project.badge_label, 2);
            Span::styled(
                format!(
                    " {}{} ",
                    label,
                    " ".repeat(
                        2usize
                            .saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str(),))
                    )
                ),
                super::session_tui_cutex_projects::project_badge_style(project.color),
            )
        },
    );
    let revision = format!(" r{}", row.task_revision);
    let show_revision = width >= 24;
    let text_width = width.saturating_sub(5);
    let text = if show_revision {
        views::clipped(&format!("{}{}", row.task_id, revision), text_width)
    } else {
        views::clipped(&row.task_id, text_width)
    };
    Line::from(vec![
        badge,
        Span::raw(" "),
        Span::styled(text, Style::new().add_modifier(Modifier::BOLD)),
    ])
}

fn task_table_row(row: &TaskRow, selected: bool, columns: &[(TaskColumn, u16)]) -> Row<'static> {
    Row::new(columns.iter().map(|(column, width)| {
        let width = usize::from(*width);
        match column {
            TaskColumn::Task => Cell::from(task_name_line(row, width)),
            TaskColumn::State => {
                Cell::from(views::clipped(row.state.label(), width)).style(row.state.style())
            }
            TaskColumn::Agent => Cell::from(views::clipped(&row.agent_label(), width)),
            TaskColumn::AgentActivity => {
                Cell::from(views::clipped(row.agent_activity_label(), width))
                    .style(Style::new().fg(Color::Gray))
            }
            TaskColumn::Attempt => Cell::from(
                row.attempt_number
                    .map(|number| number.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            ),
            TaskColumn::Updated => Cell::from(views::clipped(
                &format_age(&row.updated_at, Utc::now()),
                width,
            ))
            .style(Style::new().fg(Color::Gray)),
            TaskColumn::Activity => {
                Cell::from(views::clipped(&row.activity, width)).style(Style::new().fg(Color::Gray))
            }
        }
    }))
    .style(if selected {
        Style::new()
            .bg(crate::cli_app::session_tui_layout::selection())
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    })
}

fn format_age(value: &str, now: DateTime<Utc>) -> String {
    let Ok(value) = DateTime::parse_from_rfc3339(value) else {
        return bounded_single_line(value, 10);
    };
    let seconds = now
        .signed_duration_since(value.with_timezone(&Utc))
        .num_seconds()
        .max(0);
    match seconds {
        0..=4 => "now".to_string(),
        5..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        86_400..=604_799 => format!("{}d", seconds / 86_400),
        // Do not imply that an old non-terminal task is making progress. The
        // detail view preserves the exact timestamp; the compact table keeps
        // a stable, unambiguous age indicator even in a narrow terminal.
        _ => "old".to_string(),
    }
}

fn render_detail(frame: &mut Frame<'_>, area: Rect, model: &TaskModel, focused: bool) {
    let content = model
        .selected_row()
        .map(|row| {
            vec![
                Line::styled(row.task_id.clone(), Style::new().fg(crate::cli_app::session_tui_layout::focus()).add_modifier(Modifier::BOLD)),
                Line::from(vec![detail_label("State"), Span::styled(row.state.label(), row.state.style())]),
                detail_field("Task updated", row.updated_at.clone()),
                detail_field("Agent", row.agent_label()),
                detail_field("Attempt", row.attempt_number.map(|v| v.to_string()).unwrap_or_else(|| "-".into())),
                detail_field("Project", row.project_label()),
                Line::default(),
                detail_field("Status summary", row.status_summary.clone().unwrap_or_else(|| "-".into())),
                detail_field("Result ref", row.result_reference.clone().unwrap_or_else(|| "-".into())),
                Line::default(),
                detail_field("Task activity", row.activity.clone()),
                detail_field("Agent activity (last observed)", row.agent_activity_label().into()),
                detail_field("Last output", row.last_output.clone().unwrap_or_else(|| "-".into())),
                detail_field("Last tool", row.last_tool_call.clone().unwrap_or_else(|| "-".into())),
                Line::default(),
                Line::styled("Technical identifiers", Style::new().fg(crate::cli_app::session_tui_layout::focus())),
                detail_field("Revision", row.task_revision.to_string()),
                detail_field("Assignment", row.assignment_id.clone()),
                detail_field("Project ID", row.project_id.clone()),
                detail_field("Assignee", row.assignee_session_id.clone()),
                detail_field("Phase", row.phase.clone().unwrap_or_else(|| "-".into())),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("Selected task is no longer visible.")]);
    views::render_entity_details(frame, area, "Task Details", content, &model.detail_scroll, focused);
}

fn detail_label(label: &str) -> Span<'static> {
    Span::styled(
        format!("{label}: "),
        Style::new().fg(crate::cli_app::session_tui_layout::focus()).add_modifier(Modifier::BOLD),
    )
}

fn detail_field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![detail_label(label), Span::raw(value)])
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_filter_handles_paste_only_while_focused() {
        let mut model = TaskModel::default();
        let mut cadence = RefreshCadence::new(Instant::now());
        handle_event(&mut model, &mut cadence, Event::Paste("ignored".into()));
        assert_eq!(model.query.value(), "");
        model.filter_focused = true;
        handle_event(&mut model, &mut cadence, Event::Paste("中文 task".into()));
        assert_eq!(model.query.value(), "中文 task");
    }

    #[test]
    fn pro_review_task_filter_leaves_details() {
        let mut model = TaskModel { rows: vec![row("one", TaskState::Running, "2026-01-01T00:00:00Z")], selected_assignment_id: Some("one".into()), detail: true, ..Default::default() };
        let mut cadence = RefreshCadence::new(Instant::now());
        handle_key(&mut model, &mut cadence, KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(model.filter_focused);
        assert!(!model.detail);
        handle_key(&mut model, &mut cadence, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(model.query.value(), "x");
    }

    #[test]
    fn ui_contract_b2_legacy_navigation_keeps_task_local_state() {
        let mut model = TaskModel::default();
        let mut cadence = RefreshCadence::new(Instant::now());
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(model.query.value(), "x");
        let query = model.query.clone();
        assert_eq!(
            handle_key(
                &mut model,
                &mut cadence,
                KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT)
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents))
        );
        assert_eq!(model.query.value(), query.value());
        assert!(model.filter_focused); // frozen legacy behavior, not B1 focus policy
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!model.filter_focused);
        assert_eq!(model.query.value(), "x");
    }
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::time::Duration;

    fn buffer_text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row(id: &str, state: TaskState, updated_at: &str) -> TaskRow {
        TaskRow {
            project_id: "project-alpha".to_string(),
            project_presentation: Some(TaskProjectPresentation {
                display_name: "Alpha Project".to_string(),
                badge_label: "AP".to_string(),
                color: ProjectPaletteColor::Magenta,
            }),
            task_id: format!("task-{id}"),
            task_revision: 1,
            assignment_id: id.to_string(),
            assignee_session_id: "cutex.worker".to_string(),
            agent: None,
            state,
            phase: None,
            attempt_number: Some(1),
            updated_at: updated_at.to_string(),
            activity: "old, bounded activity".to_string(),
            status_summary: Some("Task status summary".to_string()),
            result_reference: Some("/private/result.json".to_string()),
            last_output: Some("bounded output projection".to_string()),
            last_tool_call: Some("bounded tool projection".to_string()),
        }
    }

    #[test]
    fn exact_agent_join_never_falls_back_to_name_or_runtime_id() {
        let agents = vec![AgentBusAgent {
            id: "runtime-worker".to_string(),
            name: "Worker".to_string(),
            base_name: None,
            thread_name: None,
            path_key: None,
            session_id: None,
            cutex_session_id: Some("cutex.worker".to_string()),
            profile: "default".to_string(),
            cwd: "/tmp".to_string(),
            pid: 1,
            host_id: None,
            groups: vec![],
            registration_class: Default::default(),
            last_seen_epoch_secs: 1,
        }];
        assert_eq!(
            exact_agent_join(&agents, &HashMap::new(), "cutex.worker", Utc::now())
                .unwrap()
                .display_name,
            "Worker"
        );
        assert!(exact_agent_join(&agents, &HashMap::new(), "Worker", Utc::now()).is_none());
        assert!(exact_agent_join(&agents, &HashMap::new(), "runtime-worker", Utc::now()).is_none());
        let mut ambiguous = agents.clone();
        let mut duplicate = agents[0].clone();
        duplicate.id = "newer-looking-runtime".into();
        ambiguous.push(duplicate);
        assert!(
            exact_agent_join(&ambiguous, &HashMap::new(), "cutex.worker", Utc::now()).is_none(),
            "multiple current occurrences fail closed instead of guessing"
        );
    }

    #[test]
    fn wide_inspector_keeps_task_updated_separate_from_current_agent_activity() {
        let mut task = row("activity", TaskState::Running, "2026-01-02T03:04:05Z");
        task.agent = Some(AgentJoin {
            display_name: "Worker 中文".into(),
            runtime_id: "runtime-current".into(),
            availability: "online",
            activity: " EDIT  2s ".into(),
        });
        task.activity = "Task semantic progress remains a different fact".into();
        task.status_summary = Some("Validated current assignment boundary".into());
        task.result_reference = Some("/private/result/report.json".into());
        let model = TaskModel {
            rows: vec![task],
            selected_assignment_id: Some("activity".into()),
            loading: false,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(180, 32)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let output = buffer_text(terminal.backend().buffer());
        for expected in [
            "AGENT ACT",
            "EDIT  2s",
            "Task Details",
            "Task updated",
            "Agent activity (last observed)",
            "Task semantic progress",
            "Validated current assignment",
            "/private/result/report.json",
        ] {
            assert!(output.contains(expected), "missing {expected:?}\n{output}");
        }
    }

    #[test]
    fn states_cover_queued_running_review_blocked_and_closed() {
        let mut assignment = serde_json::from_value::<DirectorAssignmentView>(serde_json::json!({"assignment_id":"a-1","task_id":"t-1","task_revision":1,"assignee_cutex_session_id":"cutex.worker","state":"active","created_at":"2026-01-01T00:00:00Z","attempts":[]})).unwrap();
        assert_eq!(map_state(&assignment, None), TaskState::Assigned);
        assignment.state = "awaiting_ack".to_string();
        assert_eq!(map_state(&assignment, None), TaskState::Queued);
        for (phase, expected) in [
            ("running", TaskState::Running),
            ("review_ready", TaskState::ReviewReady),
            ("blocked", TaskState::Blocked),
        ] {
            let attempt = serde_json::from_value::<DirectorAttemptView>(serde_json::json!({"attempt_number":1,"phase":phase,"started_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"})).unwrap();
            assert_eq!(map_state(&assignment, Some(&attempt)), expected);
        }
        assignment.state = "retry_pending".to_string();
        for phase in ["failed", "aborted"] {
            let attempt = serde_json::from_value::<DirectorAttemptView>(serde_json::json!({"attempt_number":1,"phase":phase,"started_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"})).unwrap();
            assert_eq!(map_state(&assignment, Some(&attempt)), TaskState::RetryPending);
        }
        assignment.state = "closed".to_string();
        assert_eq!(map_state(&assignment, None), TaskState::Closed);
    }

    #[test]
    fn active_toggle_filter_and_selection_are_deterministic() {
        let mut model = TaskModel {
            rows: vec![
                row("closed", TaskState::Closed, "2020-01-01T00:00:00Z"),
                row("run", TaskState::Running, "2026-01-01T00:00:00Z"),
            ],
            selected_assignment_id: Some("run".to_string()),
            ..Default::default()
        };
        assert_eq!(model.visible_indices(), vec![1]);
        model.show_closed = true;
        model.retain_selection();
        assert_eq!(model.selected_assignment_id.as_deref(), Some("run"));
        model.query = Input::new("closed".to_string());
        model.retain_selection();
        assert_eq!(model.selected_assignment_id.as_deref(), Some("closed"));
    }

    #[test]
    fn project_presentation_uses_only_exact_authoritative_project_ids() {
        let alpha = ProjectId::new("project-alpha").unwrap();
        let beta = ProjectId::new("project-beta").unwrap();
        let response: HumanManagementTaskQueryResponse = serde_json::from_value(serde_json::json!({
            "schema": "cutex/human-management-task-query/v1",
            "director_seat_occupant": "cutex.director",
            "director_seat_epoch": 2,
            "project_ids": ["project-alpha"],
            "project_presentations": {
                "project-alpha": {
                    "display_name": "Core Platform", "badge_label": "CP", "color": "magenta",
                    "revision": 7, "stored": true
                }
            },
            "receipt": {
                "schema": "cutex/task-service-director-receipt/v1",
                "action_id": "management-query-1", "operation": "query",
                "status": "current_state",
                "assignments": [
                    {"project_id":"project-alpha","assignment_id":"a-1","task_id":"t-1","task_revision":1,"assignee_cutex_session_id":"cutex.worker","state":"active","created_at":"2026-01-01T00:00:00Z","attempts":[]},
                    {"project_id":"project-beta","assignment_id":"a-2","task_id":"t-2","task_revision":1,"assignee_cutex_session_id":"cutex.worker","state":"active","created_at":"2026-01-01T00:00:00Z","attempts":[]}
                ]
            }
        })).unwrap();
        let presentations = exact_project_presentations(&response);
        let alpha_presentation = presentations.get(&alpha).cloned().unwrap();
        assert_eq!(alpha_presentation.display_name, "Core Platform");
        assert_eq!(alpha_presentation.badge_label, "CP");
        assert_eq!(alpha_presentation.color, ProjectPaletteColor::Magenta);
        assert!(!presentations.contains_key(&beta));

        let mut alpha_row = row("alpha", TaskState::Running, "2026-01-01T00:00:00Z");
        alpha_row.project_presentation = Some(alpha_presentation);
        let mut beta_row = alpha_row.clone();
        beta_row.project_id = beta.to_string();
        beta_row.project_presentation = None;
        assert!(alpha_row.matches("project-alpha"));
        assert!(!beta_row.matches("project-alpha"));
        assert_eq!(beta_row.project_label(), "unavailable (project-beta)");
        beta_row.project_id = "-".to_string();
        assert_eq!(beta_row.project_label(), "-");
    }

    #[test]
    fn responsive_columns_fit_and_keep_primary_task_first() {
        for width in [10, 20, 30, 38, 52, 77, 78, 109, 110, 160] {
            let columns = task_columns(width);
            let used = columns.iter().map(|(_, width)| *width).sum::<u16>()
                + columns.len().saturating_sub(1) as u16
                + 2;
            assert!(used <= width, "{width}: {columns:?}");
            assert_eq!(columns[0].0, TaskColumn::Task);
            if width >= 14 {
                assert!(columns
                    .iter()
                    .any(|(column, _)| *column == TaskColumn::State));
            }
        }
    }

    #[test]
    fn configured_badge_survives_selection_and_unicode_at_all_layouts() {
        let mut task = row(
            "任务-非常长的标识",
            TaskState::Blocked,
            "2026-01-01T00:00:00Z",
        );
        task.task_id = "任务-非常长的标识-abcdef0123456789".to_string();
        let model = TaskModel {
            rows: vec![task],
            selected_assignment_id: Some("任务-非常长的标识".to_string()),
            ..Default::default()
        };
        for width in [38, 58, 88, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 18)).unwrap();
            terminal.draw(|frame| render(frame, &model)).unwrap();
            let buffer = terminal.backend().buffer();
            let output = buffer_text(buffer);
            assert!(output.contains("AP"), "width={width}\n{output}");
            assert!(
                output.contains('任') && output.contains('务'),
                "width={width}\n{output}"
            );
            // Border y=5, header y=6, margin y=7, selected row y=8; badge
            // begins after the bordered table's "> " selection marker.
            assert_eq!(buffer[(3, 8)].bg, Color::LightMagenta);
            assert_eq!(buffer[(3, 8)].fg, Color::Black);
            assert_eq!(
                buffer[(12, 8)].bg,
                crate::cli_app::session_tui_layout::selection()
            );
        }
    }

    #[test]
    fn full_frame_reuses_global_shell_and_frames_filter_and_task_list() {
        let mut blocked = row("shell-blocked", TaskState::Blocked, "2026-09-09T00:58:03Z");
        blocked.activity = "Awaiting explicit Human decision".to_string();
        let mut review = row(
            "shell-review",
            TaskState::ReviewReady,
            "2026-09-09T00:42:03Z",
        );
        review.activity = "Review evidence prepared".to_string();
        let mut running = row("shell", TaskState::Running, "2026-09-09T01:02:03Z");
        running.agent = Some(AgentJoin {
            display_name: "Worker".into(),
            runtime_id: "runtime-current".into(),
            availability: "online",
            activity: " CMD 37s ".into(),
        });
        let model = TaskModel {
            rows: vec![running, blocked, review],
            selected_assignment_id: Some("shell".to_string()),
            loading: false,
            ..Default::default()
        };
        for width in [72, 120, 180] {
            let mut terminal = Terminal::new(TestBackend::new(width, 18)).unwrap();
            terminal.draw(|frame| render(frame, &model)).unwrap();
            let output = buffer_text(terminal.backend().buffer());
            assert!(output.contains("CUTEX"), "width={width}\n{output}");
            assert!(output.contains("Agents"), "width={width}\n{output}");
            assert!(output.contains("Sessions"), "width={width}\n{output}");
            assert!(output.contains("Projects"), "width={width}\n{output}");
            assert!(output.contains("Tasks"), "width={width}\n{output}");
            assert!(
                output.contains("Settings"),
                "width={width}\n{output}"
            );
            assert!(
                output.contains("Filter tasks · active"),
                "width={width}\n{output}"
            );
            assert!(output.contains("Cutex Tasks"), "width={width}\n{output}");
            assert!(
                output.contains('┌') && output.contains('└'),
                "width={width}\n{output}"
            );
            if width == 180 {
                if let Some(path) = std::env::var_os("CUTEX_TASKS_FRAME_CAPTURE") {
                    std::fs::write(path, output.trim_end()).expect("write full-frame evidence");
                }
            }
        }

        let mut narrow = Terminal::new(TestBackend::new(38, 14)).unwrap();
        narrow.draw(|frame| render(frame, &model)).unwrap();
        let narrow_output = buffer_text(narrow.backend().buffer());
        assert!(narrow_output.contains("CUTEX"));
        assert!(narrow_output.contains("Tasks"));
        assert!(!narrow_output.contains("Global Settings"));
        assert!(narrow_output.contains('┌') && narrow_output.contains('└'));
    }

    #[test]
    fn filter_and_detail_modes_preserve_the_shared_shell() {
        let mut model = TaskModel {
            rows: vec![row("modes", TaskState::Running, "2026-09-09T01:02:03Z")],
            selected_assignment_id: Some("modes".to_string()),
            loading: false,
            filter_focused: true,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(120, 22)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let filter = buffer_text(terminal.backend().buffer());
        assert!(filter.contains("CUTEX"));
        assert!(filter.contains("Settings"));
        assert!(filter.contains("Filter tasks · active"));

        model.filter_focused = false;
        model.detail = true;
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let detail = buffer_text(terminal.backend().buffer());
        assert!(detail.contains("CUTEX"));
        assert!(detail.contains("Settings"));
        assert!(detail.contains("Task Details"));

        handle_key(
            &mut model,
            &mut RefreshCadence::new(Instant::now()),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!model.detail);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let list = buffer_text(terminal.backend().buffer());
        assert!(list.contains("CUTEX"));
        assert!(list.contains("Settings"));
        assert!(list.contains("Cutex Tasks"));
    }

    #[test]
    fn unset_badge_reserves_alignment_slot() {
        let with_badge = row("badge", TaskState::Running, "2026-01-01T00:00:00Z");
        let mut without_badge = row("blank", TaskState::Running, "2026-01-01T00:00:00Z");
        without_badge.project_presentation = None;
        let columns = task_columns(80);
        let first = task_table_row(&with_badge, false, &columns);
        let second = task_table_row(&without_badge, false, &columns);
        let mut terminal = Terminal::new(TestBackend::new(80, 4)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Table::new(
                        vec![first, second],
                        columns.iter().map(|(_, width)| Constraint::Length(*width)),
                    )
                    .column_spacing(1),
                    frame.area(),
                )
            })
            .unwrap();
        let output = buffer_text(terminal.backend().buffer());
        assert!(output.contains(" AP  task-badge"));
        assert!(output.contains("     task-blank"));
    }

    #[test]
    fn detail_scroll_reaches_long_exact_identifiers_and_activity_tail() {
        let mut task = row("detail", TaskState::ReviewReady, "2026-09-09T01:02:03Z");
        task.assignment_id = format!("assignment-{}-TAIL", "甲乙丙丁".repeat(24));
        task.assignee_session_id = format!("cutex.{}-SESSION-END", "worker".repeat(20));
        task.activity = format!("/very/long/{}/result.json", "目录/".repeat(8));
        let mut model = TaskModel {
            selected_assignment_id: Some(task.assignment_id.clone()),
            rows: vec![task],
            detail: true,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(74, 14)).unwrap();
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let first = buffer_text(terminal.backend().buffer());
        assert!(first.contains("Task Details"));
        assert!(first.contains("Task updated"));
        handle_key(
            &mut model,
            &mut RefreshCadence::new(Instant::now()),
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        );
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let middle = buffer_text(terminal.backend().buffer());
        let mut visited = middle;
        for _ in 0..40 {
            handle_key(&mut model, &mut RefreshCadence::new(Instant::now()), KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            terminal.draw(|frame| render(frame, &model)).unwrap();
            visited.push_str(&buffer_text(terminal.backend().buffer()));
        }
        assert!(visited.contains("/very/long/") && visited.contains("lt.json"), "{visited}");
        handle_key(
            &mut model,
            &mut RefreshCadence::new(Instant::now()),
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
        );
        terminal.draw(|frame| render(frame, &model)).unwrap();
        let last = buffer_text(terminal.backend().buffer());
        assert!(visited.contains("-SESSION") && visited.contains("-END"), "{visited}");
        assert!(last.contains("Phase:"), "{last}");
        assert_eq!(
            model.selected_assignment_id.as_deref(),
            Some(model.rows[0].assignment_id.as_str())
        );
    }

    #[test]
    fn cadence_is_testable_without_wall_clock() {
        let start = Instant::now();
        let mut cadence = RefreshCadence::new(start);
        assert!(cadence.due(start));
        assert!(!cadence.due(start + Duration::from_millis(999)));
        assert!(cadence.due(start + Duration::from_secs(1)));
    }

    #[test]
    fn task_list_arrows_switch_tabs_and_detail_arrows_stay_local() {
        let now = Instant::now();
        let mut cadence = RefreshCadence::new(now);
        let mut model = TaskModel {
            rows: vec![
                row("one", TaskState::Running, "2026-01-01T00:00:00Z"),
                row("two", TaskState::Blocked, "2026-01-02T00:00:00Z"),
            ],
            selected_assignment_id: Some("one".to_string()),
            ..Default::default()
        };

        assert_eq!(
            handle_key(
                &mut model,
                &mut cadence,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Jobs))
        );
        assert!(!model.detail);
        assert_eq!(model.selected_assignment_id.as_deref(), Some("one"));
        assert_eq!(
            handle_key(
                &mut model,
                &mut cadence,
                KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Projects))
        );
        assert!(!model.detail);
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(model.selected_assignment_id.as_deref(), Some("two"));
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert!(model.detail);
        let selected = model.selected_assignment_id.clone();
        assert_eq!(
            handle_key(
                &mut model,
                &mut cadence,
                KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            ),
            None
        );
        assert!(model.detail);
        assert_eq!(model.selected_assignment_id, selected);
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!model.detail);
        handle_key(
            &mut model,
            &mut cadence,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(model.detail);
    }

    #[test]
    fn alt_shortcuts_preserve_task_selection_filter_and_detail_view() {
        let now = Instant::now();
        let mut cadence = RefreshCadence::new(now);
        let mut model = TaskModel {
            rows: vec![row("one", TaskState::Running, "2026-01-01T00:00:00Z")],
            selected_assignment_id: Some("one".to_string()),
            query: Input::new("running".to_string()),
            detail: true,
            ..Default::default()
        };

        assert_eq!(
            handle_key(
                &mut model,
                &mut cadence,
                KeyEvent::new(KeyCode::Char('3'), KeyModifiers::ALT),
            ),
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Projects))
        );
        assert_eq!(model.selected_assignment_id.as_deref(), Some("one"));
        assert_eq!(model.query.value(), "running");
        assert!(model.detail);
    }

    #[test]
    fn global_settings_shortcut_uses_the_real_parent_workspace_route() {
        let mut model = TaskModel::default();
        let outcome = handle_key(
            &mut model,
            &mut RefreshCadence::new(Instant::now()),
            KeyEvent::new(KeyCode::Char('6'), KeyModifiers::ALT),
        );
        assert_eq!(
            outcome,
            Some(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents))
        );
        assert!(model.open_settings_requested);
    }

    #[test]
    fn narrow_wide_empty_and_old_data_render() {
        for width in [58, 120] {
            let backend = TestBackend::new(width, 18);
            let mut terminal = Terminal::new(backend).unwrap();
            let model = TaskModel {
                rows: vec![row("old", TaskState::Running, "2020-01-01T00:00:00Z")],
                selected_assignment_id: Some("old".to_string()),
                ..Default::default()
            };
            terminal.draw(|frame| render(frame, &model)).unwrap();
            let output = format!("{:?}", terminal.backend().buffer());
            assert!(output.contains("TASK"));
            assert!(output.contains("old"));
        }
        let backend = TestBackend::new(58, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &TaskModel {
                        warning: Some("unavailable".to_string()),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("unavailable"));

        let mut terminal = Terminal::new(TestBackend::new(38, 9)).unwrap();
        let model = TaskModel {
            rows: vec![row("resize", TaskState::Running, "2020-01-01T00:00:00Z")],
            selected_assignment_id: Some("resize".to_string()),
            ..Default::default()
        };
        terminal.draw(|frame| render(frame, &model)).unwrap();
        terminal.backend_mut().resize(120, 24);
        terminal.draw(|frame| render(frame, &model)).unwrap();
        assert!(format!("{:?}", terminal.backend().buffer()).contains("resize"));
    }

    #[test]
    fn old_timestamps_remain_visibly_old_without_claiming_progress() {
        assert_eq!(format_age("2020-01-01T00:00:00Z", Utc::now()), "old");
    }
}
