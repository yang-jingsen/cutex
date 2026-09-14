//! Shared shell geometry and colors for every primary panel.
use super::session_tui_workspace::PrimaryPanel;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub(super) const BRAND: Color = Color::Rgb(247, 179, 205);
pub(super) const ACCENT: Color = Color::Rgb(224, 142, 178);
pub(super) const TEXT: Color = Color::White;
pub(super) const MUTED: Color = Color::DarkGray;
pub(super) const FOCUS: Color = Color::Rgb(116, 186, 195);
pub(super) const SELECTION: Color = Color::Rgb(38, 52, 79);
pub(super) const SUCCESS: Color = Color::Green;
pub(super) const WARNING: Color = Color::Yellow;
pub(super) const ERROR: Color = Color::Red;

/// Keep the list readable in normal windows, but give all growth beyond its
/// useful column width to details. Includes borders and the selection marker.
pub(super) const LIST_PANE_MAX_WIDTH: u16 = 130;

pub(super) fn inspector_panes(area: Rect, visible: bool) -> Option<(Rect, Rect)> {
    if !visible || area.width < 115 {
        return None;
    }
    let available = area.width - 1;
    let left = ((u32::from(available) * 62 / 100) as u16)
        .max(74)
        .min(LIST_PANE_MAX_WIDTH)
        .min(available - 40);
    let right = available - left;
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

/// Explicit styles prevent stale colors when a shorter heading replaces another.
pub(super) fn heading(prefix: &str, title: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(prefix.to_owned(), Style::new().fg(FOCUS).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {title}"), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
    ])
}

pub(super) fn tabs(active: PrimaryPanel, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::styled("CUTEX", Style::new().fg(BRAND).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    if width < 60 {
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
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_windows_give_all_extra_space_to_details() {
        let (list, detail) = inspector_panes(Rect::new(3, 2, 240, 30), true).unwrap();
        let (wider_list, wider_detail) = inspector_panes(Rect::new(3, 2, 320, 30), true).unwrap();
        assert_eq!(list.width, LIST_PANE_MAX_WIDTH);
        assert_eq!(wider_list, list);
        assert_eq!(wider_detail.x, detail.x);
        assert_eq!(wider_detail.width, detail.width + 80);
        assert_eq!(wider_detail.right(), 323);
    }

    #[test]
    fn split_preserves_minimums_gap_and_narrow_fallback() {
        for width in 0..=500 {
            let area = Rect::new(3, 2, width, 20);
            assert!(inspector_panes(area, false).is_none());
            if width < 115 {
                assert!(inspector_panes(area, true).is_none());
            } else {
                let (list, detail) = inspector_panes(area, true).unwrap();
                assert!(list.width >= 74 && list.width <= LIST_PANE_MAX_WIDTH);
                assert!(detail.width >= 40);
                assert_eq!(list.right() + 1, detail.x);
                assert_eq!(detail.right(), area.right());
            }
        }
    }
}
