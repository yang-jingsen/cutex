//! Shared state for a top-level TUI workspace.
//!
//! Keep this independent of session records: future workspaces can reuse the
//! selection and transient-visibility behavior without coupling to the agent
//! selector's data model.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionTuiWorkspace {
    Agents,
    RecentSessions,
    RetiredSessions,
    CutexProjects,
    Projects,
    Profiles,
    GlobalSettings,
}

impl SessionTuiWorkspace {
    /// Workspaces that are intentionally available in the production selector.
    pub(super) const PRODUCTION: [Self; 7] = [
        Self::Agents,
        Self::RecentSessions,
        Self::RetiredSessions,
        Self::CutexProjects,
        Self::Projects,
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
    fn production_workspace_list_keeps_cutex_projects_separate_from_codex_workspaces() {
        assert_eq!(
            SessionTuiWorkspace::PRODUCTION,
            [
                SessionTuiWorkspace::Agents,
                SessionTuiWorkspace::RecentSessions,
                SessionTuiWorkspace::RetiredSessions,
                SessionTuiWorkspace::CutexProjects,
                SessionTuiWorkspace::Projects,
                SessionTuiWorkspace::Profiles,
                SessionTuiWorkspace::GlobalSettings,
            ]
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
