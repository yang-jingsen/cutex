//! Placeholder for the upcoming Job list, using the same primary navigation.
use super::{
    session_tui::ShellEvents,
    session_tui_layout as theme,
    session_tui_workspace::{primary_panel_shortcut, PrimaryPanel, PrimaryPanelOutcome},
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};
use std::io::Stdout;
use super::session_tui_terminal::Terminal;

pub(super) fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    events: &mut ShellEvents,
) -> anyhow::Result<PrimaryPanelOutcome> {
    loop {
        terminal.draw(|frame| {
            let areas = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(2),
            ])
            .split(frame.area());
            frame.render_widget(
                Paragraph::new(theme::tabs(PrimaryPanel::Jobs, frame.area().width)),
                areas[0],
            );
            frame.render_widget(
                Paragraph::new(theme::heading("Cutex", "Jobs")),
                areas[1],
            );
            frame.render_widget(
                Paragraph::new("The Job list will be available here in a future update.")
                    .wrap(Wrap { trim: true })
                    .block(
                        Block::bordered()
                            .title(" Jobs ")
                            .border_style(Style::new().fg(theme::muted())),
                    ),
                areas[2],
            );
            frame.render_widget(
                Paragraph::new("Preview · Job list not connected")
                    .style(Style::new().fg(theme::muted())),
                areas[3],
            );
            frame.render_widget(
                Paragraph::new(ratatui::text::Line::from(super::session_tui::footer_hints(
                    &[
                        ("←/→", "panels"),
                        ("Alt+6", "settings"),
                        ("Esc", "agents"),
                        ("Ctrl+C", "exit"),
                    ],
                )))
                .wrap(Wrap { trim: true }),
                areas[4],
            );
        })?;
        if let Some(Event::Key(key)) = events.next()? {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c' | 'C'))
            {
                return Ok(PrimaryPanelOutcome::Exit);
            }
            if let Some(panel) = primary_panel_shortcut(key).filter(|p| *p != PrimaryPanel::Jobs) {
                return Ok(PrimaryPanelOutcome::Switch(panel));
            }
            if key.modifiers.is_empty() {
                match key.code {
                    KeyCode::Left => return Ok(PrimaryPanelOutcome::Switch(PrimaryPanel::Tasks)),
                    KeyCode::Right => {
                        return Ok(PrimaryPanelOutcome::Switch(PrimaryPanel::Settings))
                    }
                    KeyCode::Esc => return Ok(PrimaryPanelOutcome::Switch(PrimaryPanel::Agents)),
                    _ => {}
                }
            }
        }
    }
}
