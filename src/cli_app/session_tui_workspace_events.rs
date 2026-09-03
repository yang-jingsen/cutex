use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkspaceEvent {
    Up,
    Down,
    First,
    Last,
    Insert(char),
    Backspace,
    Delete,
    ClearInput,
    OpenActions,
    OpenSettings,
    Activate,
    Back,
    Escape,
    Exit,
}

pub(super) fn workspace_event_from_key(
    key: KeyEvent,
    enhanced_keyboard: bool,
) -> Option<WorkspaceEvent> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(WorkspaceEvent::Exit);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('u' | 'U'))
    {
        return Some(WorkspaceEvent::ClearInput);
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        return enhanced_keyboard.then_some(WorkspaceEvent::OpenActions);
    }
    match key.code {
        KeyCode::Up => Some(WorkspaceEvent::Up),
        KeyCode::Down => Some(WorkspaceEvent::Down),
        KeyCode::Home => Some(WorkspaceEvent::First),
        KeyCode::End => Some(WorkspaceEvent::Last),
        KeyCode::Right => Some(WorkspaceEvent::OpenActions),
        KeyCode::Left => Some(WorkspaceEvent::Back),
        KeyCode::Tab => Some(WorkspaceEvent::OpenSettings),
        KeyCode::Enter => Some(WorkspaceEvent::Activate),
        KeyCode::Char(character) => Some(WorkspaceEvent::Insert(character)),
        KeyCode::Backspace => Some(WorkspaceEvent::Backspace),
        KeyCode::Delete => Some(WorkspaceEvent::Delete),
        KeyCode::Esc => Some(WorkspaceEvent::Escape),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_shared_workspace_keys_without_exposing_view_specific_behavior() {
        assert_eq!(
            workspace_event_from_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), false),
            Some(WorkspaceEvent::OpenSettings)
        );
        assert_eq!(
            workspace_event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), false),
            None
        );
        assert_eq!(
            workspace_event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), true),
            Some(WorkspaceEvent::OpenActions)
        );
    }
}
