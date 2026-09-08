//! Bounded shell geometry/theme. Tasks deliberately retains its legacy renderer.
use super::session_tui_workspace::PrimaryPanel;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(super) const BRAND: Color = Color::Magenta;
pub(super) const TEXT: Color = Color::White;
pub(super) const MUTED: Color = Color::DarkGray;
pub(super) const FOCUS: Color = Color::Cyan;
pub(super) const SELECTION: Color = Color::Blue;
pub(super) const SUCCESS: Color = Color::Green;
pub(super) const WARNING: Color = Color::Yellow;
pub(super) const ERROR: Color = Color::Red;

/// List inner >=72, Inspector inner >=38, two borders each and one gap.
/// Right target 32%, capped at 56 inner cells; list retains primary space.
pub(super) fn inspector_panes(area: Rect, visible: bool) -> Option<(Rect, Rect)> {
    if !visible || area.width < 115 {
        return None;
    }
    let right = (area.width.saturating_mul(32) / 100)
        .clamp(40, 58)
        .min(area.width - 75);
    let left = area.width - right - 1;
    Some((
        Rect {
            width: left,
            ..area
        },
        Rect {
            x: area.x + left + 1,
            width: right,
            ..area
        },
    ))
}

pub(super) fn tabs(active: PrimaryPanel, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::styled("CUTEX", Style::new().fg(BRAND).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    if width < 42 {
        spans.push(Span::styled(
            active.label().to_owned(),
            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
        ));
    } else {
        for panel in PrimaryPanel::ALL {
            let label = if panel == PrimaryPanel::Projects {
                "Projects"
            } else {
                panel.label()
            };
            spans.push(Span::styled(
                format!(" {label} "),
                if panel == active {
                    Style::new()
                        .fg(TEXT)
                        .bg(SELECTION)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(MUTED)
                },
            ));
        }
        if width >= 72 {
            spans.push(Span::styled(
                "  Global Settings [Alt+S]",
                Style::new().fg(FOCUS),
            ));
        }
    }
    Line::from(spans)
}
