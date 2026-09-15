//! Explicit, paged report browsing; never part of periodic task refresh.
use super::session_tui_terminal::Terminal;
use super::session_tui_view::{render_details, DetailScroll};
use crossterm::event::{Event, KeyCode, KeyEventKind};
use cutex::management::control_plane::{
    HumanManagementTaskQueryRequest, HumanManagementTaskQuerySchema,
};
use cutex::management::task_reports::ReportPage;
use cutex::task_service::{ActionId, AssignmentId, DirectorQuerySelector};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::Paragraph,
};

pub(super) fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    events: &mut super::session_tui::ShellEvents,
    id: String,
) -> anyhow::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut cursors = vec![String::new()];
    let mut current = 0usize;
    let mut pending = true;
    let mut loading = false;
    let mut page: Option<ReportPage> = None;
    let mut text = String::new();
    let scroll = DetailScroll::default();
    loop {
        if pending && !loading {
            pending = false;
            loading = true;
            text = "Loading reports…".into();
            scroll.reset();
            let tx = tx.clone();
            let id = id.clone();
            let before = cursors[current].clone();
            std::thread::spawn(move || {
                let result = (|| -> anyhow::Result<ReportPage> {
                    let request = HumanManagementTaskQueryRequest {
                        schema: HumanManagementTaskQuerySchema::V1,
                        action_id: ActionId::new(format!(
                            "human-reports-{}",
                            uuid::Uuid::new_v4()
                        ))?,
                        selector: DirectorQuerySelector::Assignment {
                            assignment_id: AssignmentId::new(id)?,
                        },
                        reports_before: Some(before),
                    };
                    super::management_control_plane::ManagementControlClient::connect()?
                        .tasks(&request)?
                        .report_page
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Management service does not support report browsing yet"
                            )
                        })
                })();
                let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            });
        }
        if let Ok(result) = rx.try_recv() {
            loading = false;
            match result {
                Ok(value) => {
                    text = value
                        .reports
                        .iter()
                        .map(|r| {
                            let time = chrono::DateTime::parse_from_rfc3339(&r.timestamp)
                                .map(|v| {
                                    v.with_timezone(&chrono::Local)
                                        .format("%m-%d %H:%M:%S")
                                        .to_string()
                                })
                                .unwrap_or_else(|_| r.timestamp.clone());
                            format!("{} · {} · attempt {}\n{}", time, r.kind, r.attempt, r.text)
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    if text.is_empty() {
                        text = "No reports on this page.".into();
                    }
                    page = Some(value);
                }
                Err(error) => {
                    text = error;
                    page = None;
                }
            }
        }
        terminal.draw(|frame| {
            let areas =
                Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(frame.area());
            render_details(
                frame,
                areas[0],
                &format!(" Reports · {id} · page {} ", current + 1),
                &text,
                &scroll,
            );
            frame.render_widget(
                Paragraph::new("↑/↓ PgUp/PgDn scroll · N older · P newer · R reload · Esc back"),
                areas[1],
            );
        })?;
        if let Some(Event::Key(key)) = events.next()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.code == KeyCode::Esc
                || (key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c'))
            {
                return Ok(());
            }
            if !loading && key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Char('n' | 'N') => {
                        if let Some(next) = page.as_ref().and_then(|p| p.next_before.clone()) {
                            cursors.truncate(current + 1);
                            cursors.push(next);
                            current += 1;
                            pending = true;
                        }
                    }
                    KeyCode::Char('p' | 'P') if current > 0 => {
                        current -= 1;
                        pending = true;
                    }
                    KeyCode::Char('r' | 'R') => {
                        pending = true;
                    }
                    _ => {
                        scroll.handle(key);
                    }
                }
            }
        }
    }
}
