//! Read-only, Director-authorized Task Service workspace.
//!
//! This module intentionally owns its own small model and refresh worker.  It
//! never reads cwd, collaboration groups, native workspaces, or names to
//! determine task ownership: the Task Service Director query is the source of
//! task state, and its authenticated route enforces the Director authority.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Stdout};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use cutex::agent_bus::client::{
    agent_bus_fetch_agents_if_healthy, agent_bus_submit_task_service_director_action,
};
use cutex::agent_bus::model::AgentBusAgent;
use cutex::agent_management::{effective_presentation, AgentManagementProvider, ProjectId};
use cutex::config::store::load_codez_config;
use cutex::task_service::{
    ActionId, DirectorActionRequest, DirectorActionSchema, DirectorActionStatus,
    DirectorAssignmentView, DirectorAttemptView, DirectorQuerySelector, DirectorSemanticOperation,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};
use tui_input::{Input, InputRequest};
use uuid::Uuid;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const ACTIVITY_LIMIT: usize = 96;

type TaskTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Queued,
    Assigned,
    Running,
    ReviewReady,
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
            Self::Blocked => "blocked",
            Self::Closed => "closed",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Queued | Self::Assigned => Style::new().fg(Color::Cyan),
            Self::Running => Style::new().fg(Color::Green),
            Self::ReviewReady => Style::new().fg(Color::Magenta),
            Self::Blocked => Style::new().fg(Color::Yellow),
            Self::Closed => Style::new().fg(Color::DarkGray),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskProjectPresentation {
    display_name: String,
    badge_label: String,
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
            None => format!("unavailable ({})", self.project_id),
        }
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

fn exact_agent_join(agents: &[AgentBusAgent], assignee_session_id: &str) -> Option<AgentJoin> {
    // `cutex_session_id` is the sole join key. In particular, display names,
    // runtime IDs, thread names, cwd, and groups must never act as fallbacks.
    let mut matches = agents
        .iter()
        .filter(|agent| agent.cutex_session_id.as_deref() == Some(assignee_session_id))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    matches.first().map(|agent| AgentJoin {
        display_name: agent.name.clone(),
        runtime_id: agent.id.clone(),
        availability: "online",
    })
}

fn map_state(
    assignment: &DirectorAssignmentView,
    active: Option<&DirectorAttemptView>,
) -> TaskState {
    if assignment.state == "closed" || assignment.closure_reason.is_some() {
        return TaskState::Closed;
    }
    match active.map(|attempt| attempt.phase.as_str()) {
        Some("blocked") => TaskState::Blocked,
        Some("review_ready") => TaskState::ReviewReady,
        Some("running") => TaskState::Running,
        Some("completed" | "failed" | "cancelled" | "aborted") => TaskState::Closed,
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
                    .unwrap_or_else(|| "unscoped".to_string()),
                project_presentation: assignment
                    .project_id
                    .as_ref()
                    .and_then(|project_id| project_presentations.get(project_id).cloned()),
                task_id: assignment.task_id.as_str().to_string(),
                task_revision: assignment.task_revision.get(),
                assignment_id: assignment.assignment_id.as_str().to_string(),
                assignee_session_id: assignment.assignee_cutex_session_id.as_str().to_string(),
                agent: exact_agent_join(agents, assignment.assignee_cutex_session_id.as_str()),
                state,
                phase: active.map(|attempt| attempt.phase.clone()),
                attempt_number: active.map(|attempt| attempt.attempt_number),
                updated_at,
                activity: merged_activity(active),
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
    receipt: &cutex::task_service::DirectorActionReceipt,
) -> BTreeMap<ProjectId, TaskProjectPresentation> {
    let project_ids = receipt
        .assignments
        .iter()
        .filter_map(|assignment| assignment.project_id.as_ref().cloned())
        .collect::<BTreeSet<_>>();
    let Ok(provider) = AgentManagementProvider::open_default() else {
        return BTreeMap::new();
    };
    let Ok(snapshot) = provider.store().snapshot() else {
        return BTreeMap::new();
    };
    let authoritative_project_ids = snapshot.projects.keys().cloned().collect::<BTreeSet<_>>();
    project_ids
        .into_iter()
        .filter_map(|project_id| {
            exact_project_presentation(
                &project_id,
                &authoritative_project_ids,
                &snapshot.project_presentations,
            )
            .map(|presentation| (project_id, presentation))
        })
        .collect()
}

fn exact_project_presentation(
    project_id: &ProjectId,
    authoritative_project_ids: &BTreeSet<ProjectId>,
    stored_presentations: &BTreeMap<
        ProjectId,
        cutex::agent_management::ProjectPresentationSettings,
    >,
) -> Option<TaskProjectPresentation> {
    authoritative_project_ids.contains(project_id).then(|| {
        let presentation = effective_presentation(project_id, stored_presentations.get(project_id));
        TaskProjectPresentation {
            display_name: presentation.display_name,
            badge_label: presentation.badge_label,
        }
    })
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
    bounded_single_line(text, ACTIVITY_LIMIT)
}

fn bounded_single_line(value: &str, max: usize) -> String {
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > max {
        value = value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>();
        value.push('…');
    }
    value
}

#[derive(Debug, Clone)]
struct TaskModel {
    rows: Vec<TaskRow>,
    selected_assignment_id: Option<String>,
    query: Input,
    show_closed: bool,
    detail: bool,
    loading: bool,
    warning: Option<String>,
    refreshed_at: Option<Instant>,
}

impl Default for TaskModel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected_assignment_id: None,
            query: Input::default(),
            show_closed: false,
            detail: false,
            loading: true,
            warning: None,
            refreshed_at: None,
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
            Ok(action_id) => DirectorActionRequest {
                schema: DirectorActionSchema::V2,
                action_id,
                action: DirectorSemanticOperation::Query {
                    selector: DirectorQuerySelector::All {},
                },
            },
            Err(error) => {
                let _ = sender.send(RefreshResult::Error(format!(
                    "invalid query identifier: {error:?}"
                )));
                return;
            }
        };
        let result = agent_bus_submit_task_service_director_action(&config, &request)
            .map_err(|error| format!("Task Service Director query unavailable: {error:#}"))
            .and_then(|receipt| match receipt.status {
                DirectorActionStatus::CurrentState | DirectorActionStatus::Committed => {
                    Ok(task_rows(
                        &receipt,
                        &agent_bus_fetch_agents_if_healthy(&config),
                        &exact_project_presentations(&receipt),
                    ))
                }
                _ => Err(format!(
                    "Task Service Director query returned {}{}",
                    director_status_label(receipt.status),
                    receipt
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

pub(super) fn run() -> anyhow::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("Tasks workspace requires an interactive terminal");
    }
    enable_raw_mode().context("Failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create Tasks terminal")?;
    let result = run_loop(&mut terminal);
    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

fn run_loop(terminal: &mut TaskTerminal) -> anyhow::Result<()> {
    let (sender, receiver) = mpsc::channel();
    let mut model = TaskModel::default();
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
        terminal.draw(|frame| render(frame, &model))?;
        if !event::poll(EVENT_POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if handle_key(&mut model, &mut cadence, key) {
            return Ok(());
        }
    }
}

/// Returns true when the caller should leave the workspace.
fn handle_key(model: &mut TaskModel, cadence: &mut RefreshCadence, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return true;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('r' | 'R')) {
        cadence.request_now(Instant::now());
        return false;
    }
    if key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('a' | 'A')) {
        model.show_closed = !model.show_closed;
        model.retain_selection();
        return false;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Left => {
            if model.detail {
                model.detail = false;
            } else if !model.query.value().is_empty() {
                model.query.reset();
                model.retain_selection();
            } else {
                return true;
            }
        }
        KeyCode::Up => model.move_selection(-1),
        KeyCode::Down => model.move_selection(1),
        KeyCode::Enter | KeyCode::Right => model.detail = model.selected_row().is_some(),
        KeyCode::Backspace => {
            model.query.handle(InputRequest::DeletePrevChar);
            model.retain_selection();
        }
        KeyCode::Delete => {
            model.query.handle(InputRequest::DeleteNextChar);
            model.retain_selection();
        }
        KeyCode::F(5) => cadence.request_now(Instant::now()),
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
    false
}

fn render(frame: &mut Frame<'_>, model: &TaskModel) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(area);
    let visible_count = model.visible_indices().len();
    let mode = if model.show_closed {
        "all history"
    } else {
        "active"
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Cutex Tasks",
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {visible_count} {mode}")),
            Span::styled("  read-only", Style::new().fg(Color::DarkGray)),
            Span::styled(
                if model.loading { "  refreshing" } else { "" },
                Style::new().fg(Color::Yellow),
            ),
        ])),
        chunks[0],
    );
    render_filter(frame, chunks[1], model);
    render_table(frame, chunks[2], model);
    frame.render_widget(
        Paragraph::new("↑↓ select  Enter inspect  Ctrl-A all/active  F5/Ctrl-R refresh  Esc back")
            .style(Style::new().fg(Color::DarkGray)),
        chunks[3],
    );
    if model.detail {
        render_detail(frame, centered_rect(area, 86, 72), model);
    }
}

fn render_filter(frame: &mut Frame<'_>, area: Rect, model: &TaskModel) {
    let title = if model.show_closed {
        " Filter tasks (all history) "
    } else {
        " Filter tasks (active; Ctrl-A for all) "
    };
    frame.render_widget(
        Paragraph::new(model.query.value()).block(
            Block::bordered()
                .title(title)
                .border_style(Style::new().fg(Color::DarkGray)),
        ),
        area,
    );
    if let Some(warning) = model.warning.as_deref() {
        let warning_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(warning).style(Style::new().fg(Color::Yellow)),
            warning_area,
        );
    }
}

fn render_table(frame: &mut Frame<'_>, area: Rect, model: &TaskModel) {
    let visible = model.visible_indices();
    let wide = area.width >= 100;
    let rows = visible
        .iter()
        .map(|index| task_table_row(&model.rows[*index], wide))
        .collect::<Vec<_>>();
    let header = if wide {
        Row::new(["TASK", "ST", "AGENT", "TRY", "UPDATED", "ACTIVITY"])
    } else {
        Row::new(["TASK", "ST", "AGENT", "TRY", "UPDATED", "ACTIVITY"])
    }
    .style(Style::new().fg(Color::Gray).add_modifier(Modifier::BOLD))
    .bottom_margin(1);
    let widths = if wide {
        vec![
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Length(22),
            Constraint::Length(5),
            Constraint::Length(12),
            Constraint::Min(18),
        ]
    } else {
        vec![
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(16),
            Constraint::Length(4),
            Constraint::Length(9),
            Constraint::Min(10),
        ]
    };
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .highlight_symbol("> ")
        .row_highlight_style(
            Style::new()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = TableState::default().with_selected(model.selected_visible_index());
    frame.render_stateful_widget(table, area, &mut state);
    if visible.is_empty() && area.height > 2 {
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
                y: area.y + 2,
                width: area.width.saturating_sub(4),
                height: 1,
            },
        );
    }
}

fn task_table_row(row: &TaskRow, wide: bool) -> Row<'static> {
    let task = if wide {
        format!("{} r{}", row.task_id, row.task_revision)
    } else {
        bounded_single_line(&row.task_id, 12)
    };
    let agent = if wide {
        row.agent_label()
    } else {
        bounded_single_line(&row.agent_label(), 16)
    };
    Row::new([
        Cell::from(task),
        Cell::from(row.state.label()).style(row.state.style()),
        Cell::from(agent),
        Cell::from(
            row.attempt_number
                .map(|number| number.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        Cell::from(format_age(&row.updated_at, Utc::now())).style(Style::new().fg(Color::Gray)),
        Cell::from(row.activity.clone()).style(Style::new().fg(Color::Gray)),
    ])
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

fn render_detail(frame: &mut Frame<'_>, area: Rect, model: &TaskModel) {
    frame.render_widget(Clear, area);
    let content = model
        .selected_row()
        .map(|row| {
            vec![
                Line::from(vec![
                    Span::styled("Task: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{} r{}", row.task_id, row.task_revision)),
                ]),
                Line::from(vec![
                    Span::styled("Assignment: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(row.assignment_id.clone()),
                ]),
                Line::from(vec![
                    Span::styled("Project: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(row.project_label()),
                ]),
                Line::from(vec![
                    Span::styled("Project ID: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(row.project_id.clone()),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Assignee session: ",
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(row.assignee_session_id.clone()),
                ]),
                Line::from(vec![
                    Span::styled("Agent: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::raw(row.agent_label()),
                ]),
                Line::from(vec![
                    Span::styled("State: ", Style::new().add_modifier(Modifier::BOLD)),
                    Span::styled(row.state.label(), row.state.style()),
                    Span::raw(
                        row.phase
                            .as_deref()
                            .map(|phase| format!(" ({phase})"))
                            .unwrap_or_default(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        "Attempt / updated: ",
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        "{} / {}",
                        row.attempt_number
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        row.updated_at
                    )),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Activity",
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Line::from(row.activity.clone()),
                Line::from(""),
                Line::from(Span::styled(
                    "Read-only Task Service view. Esc closes.",
                    Style::new().fg(Color::DarkGray),
                )),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("Selected task is no longer visible.")]);
    frame.render_widget(
        Paragraph::new(content)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(" Task details ")),
        area,
    );
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - height_percent) / 2),
        Constraint::Percentage(height_percent),
        Constraint::Percentage((100 - height_percent) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width_percent) / 2),
        Constraint::Percentage(width_percent),
        Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::time::Duration;

    fn row(id: &str, state: TaskState, updated_at: &str) -> TaskRow {
        TaskRow {
            project_id: "project-alpha".to_string(),
            project_presentation: Some(TaskProjectPresentation {
                display_name: "Alpha Project".to_string(),
                badge_label: "AP".to_string(),
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
            exact_agent_join(&agents, "cutex.worker")
                .unwrap()
                .display_name,
            "Worker"
        );
        assert!(exact_agent_join(&agents, "Worker").is_none());
        assert!(exact_agent_join(&agents, "runtime-worker").is_none());
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
        let authoritative_ids = BTreeSet::from([alpha.clone()]);
        let stored = BTreeMap::from([(
            alpha.clone(),
            serde_json::from_value(serde_json::json!({
                "display_name": "Core Platform",
                "badge_label": "CP",
                "color": "magenta",
                "revision": 7,
                "updated_at": "2026-01-01T00:00:00Z",
                "updated_by_director_session": "cutex.director"
            }))
            .unwrap(),
        )]);

        let alpha_presentation =
            exact_project_presentation(&alpha, &authoritative_ids, &stored).unwrap();
        assert_eq!(alpha_presentation.display_name, "Core Platform");
        assert_eq!(alpha_presentation.badge_label, "CP");
        assert!(exact_project_presentation(&beta, &authoritative_ids, &stored).is_none());

        let mut alpha_row = row("alpha", TaskState::Running, "2026-01-01T00:00:00Z");
        alpha_row.project_presentation = Some(alpha_presentation);
        let mut beta_row = alpha_row.clone();
        beta_row.project_id = beta.to_string();
        beta_row.project_presentation = None;
        assert!(alpha_row.matches("project-alpha"));
        assert!(!beta_row.matches("project-alpha"));
        assert_eq!(beta_row.project_label(), "unavailable (project-beta)");
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
    }

    #[test]
    fn old_timestamps_remain_visibly_old_without_claiming_progress() {
        assert_eq!(format_age("2020-01-01T00:00:00Z", Utc::now()), "old");
    }
}
