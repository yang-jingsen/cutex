//! Shared state for a top-level TUI workspace.
//!
//! Keep this independent of session records: future workspaces can reuse the
//! selection and transient-visibility behavior without coupling to the agent
//! selector's data model.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionTuiWorkspace {
    Agents,
    RecentSessions,
    RetiredSessions,
    CutexProjects,
    Projects,
    Tasks,
    Profiles,
    GlobalSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimaryPanel {
    Agents,
    Projects,
    Tasks,
    Recent,
}

impl PrimaryPanel {
    pub(super) const ALL: [Self; 4] = [Self::Agents, Self::Recent, Self::Projects, Self::Tasks];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Agents => "Managed",
            Self::Projects => "Cutex Projects",
            Self::Tasks => "Tasks",
            Self::Recent => "Recent",
        }
    }

    pub(super) fn shortcut(self) -> &'static str {
        match self {
            Self::Agents => "Alt+M",
            Self::Projects => "Alt+P",
            Self::Tasks => "Alt+T",
            Self::Recent => "Alt+R",
        }
    }

    pub(super) fn adjacent(self, forward: bool) -> Option<Self> {
        let index = Self::ALL.iter().position(|panel| *panel == self)?;
        let next = if forward {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
        Self::ALL.get(next).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrimaryPanelOutcome {
    Exit,
    Switch(PrimaryPanel),
}

pub(super) fn primary_panel_tabs(active: PrimaryPanel) -> ratatui::text::Line<'static> {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};

    let mut spans = Vec::new();
    for (index, panel) in PrimaryPanel::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" | ", Style::new().fg(Color::DarkGray)));
        }
        let style = if panel == active {
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(
            format!(" {} {} ", panel.label(), panel.shortcut()),
            style,
        ));
    }
    Line::from(spans)
}

/// Resolve only the frozen top-level workspace shortcuts. These shortcuts are
/// handled before a workspace's local input mapping, so Alt-modified text can
/// never leak into a filter or editor.
pub(super) fn primary_panel_shortcut(key: KeyEvent) -> Option<PrimaryPanel> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        || !key.modifiers.contains(KeyModifiers::ALT)
        || key.modifiers.contains(KeyModifiers::CONTROL)
    {
        return None;
    }
    match key.code {
        KeyCode::Char('m' | 'M') => Some(PrimaryPanel::Agents),
        KeyCode::Char('r' | 'R') => Some(PrimaryPanel::Recent),
        KeyCode::Char('p' | 'P') => Some(PrimaryPanel::Projects),
        KeyCode::Char('t' | 'T') => Some(PrimaryPanel::Tasks),
        _ => None,
    }
}

impl SessionTuiWorkspace {
    /// Workspaces that are intentionally available in the production selector.
    pub(super) const PRODUCTION: [Self; 8] = [
        Self::Agents,
        Self::RecentSessions,
        Self::RetiredSessions,
        Self::CutexProjects,
        Self::Projects,
        Self::Tasks,
        Self::Profiles,
        Self::GlobalSettings,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WorkspaceSelection<T> {
    selected: Option<T>,
    transiently_visible: Option<T>,
}

impl<T> Default for WorkspaceSelection<T> {
    fn default() -> Self {
        Self {
            selected: None,
            transiently_visible: None,
        }
    }
}

impl<T: PartialEq> WorkspaceSelection<T> {
    pub(super) fn selected(&self) -> Option<&T> {
        self.selected.as_ref()
    }

    pub(super) fn select(&mut self, target: Option<T>) {
        self.selected = target;
    }

    pub(super) fn is_selected(&self, target: &T) -> bool {
        self.selected.as_ref() == Some(target)
    }

    pub(super) fn mark_transiently_visible(&mut self, target: Option<T>) {
        self.transiently_visible = target;
    }

    pub(super) fn is_transiently_visible(&self, target: &T) -> bool {
        self.transiently_visible.as_ref() == Some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_workspace_list_keeps_cutex_projects_tasks_and_codex_workspaces_separate() {
        assert_eq!(
            SessionTuiWorkspace::PRODUCTION,
            [
                SessionTuiWorkspace::Agents,
                SessionTuiWorkspace::RecentSessions,
                SessionTuiWorkspace::RetiredSessions,
                SessionTuiWorkspace::CutexProjects,
                SessionTuiWorkspace::Projects,
                SessionTuiWorkspace::Tasks,
                SessionTuiWorkspace::Profiles,
                SessionTuiWorkspace::GlobalSettings,
            ]
        );
    }

    #[test]
    fn primary_panels_have_the_frozen_managed_recent_projects_tasks_order() {
        assert_eq!(
            PrimaryPanel::ALL,
            [
                PrimaryPanel::Agents,
                PrimaryPanel::Recent,
                PrimaryPanel::Projects,
                PrimaryPanel::Tasks,
            ]
        );
        assert_eq!(PrimaryPanel::Agents.label(), "Managed");
        assert_eq!(PrimaryPanel::Projects.label(), "Cutex Projects");
        assert_eq!(
            PrimaryPanel::Agents.adjacent(true),
            Some(PrimaryPanel::Recent)
        );
        assert_eq!(PrimaryPanel::Agents.adjacent(false), None);
        assert_eq!(PrimaryPanel::Tasks.adjacent(true), None);
        assert_eq!(
            PrimaryPanel::Tasks.adjacent(false),
            Some(PrimaryPanel::Projects)
        );
    }

    #[test]
    fn only_alt_m_r_p_t_select_top_level_workspaces() {
        for (character, expected) in [
            ('m', PrimaryPanel::Agents),
            ('r', PrimaryPanel::Recent),
            ('p', PrimaryPanel::Projects),
            ('t', PrimaryPanel::Tasks),
        ] {
            assert_eq!(
                primary_panel_shortcut(KeyEvent::new(KeyCode::Char(character), KeyModifiers::ALT,)),
                Some(expected)
            );
        }
        assert_eq!(
            primary_panel_shortcut(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            primary_panel_shortcut(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn selection_keeps_recently_changed_items_visible_without_changing_selection() {
        let mut state = WorkspaceSelection::default();
        state.select(Some("alpha"));
        state.mark_transiently_visible(Some("beta"));

        assert!(state.is_selected(&"alpha"));
        assert!(state.is_transiently_visible(&"beta"));
        assert!(!state.is_selected(&"beta"));
    }
}
