use cutex::session::model::CutexSessionRecord;
use cutex::session::model::CutexSessionRuntimeBackend;

use super::session_presenter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartSessionMenuAction {
    ResumeAttach,
    Attach,
    Takeover,
    Foreground,
    Online,
    ResumeHere,
    ResumeManaged,
    Edit,
    ChooseAnother,
}

pub(super) struct StartSessionMenuChoice {
    pub action: StartSessionMenuAction,
    pub row: session_presenter::StartSessionMenuRow,
}

pub(super) fn start_session_menu_choices(
    record: &CutexSessionRecord,
    attachable: bool,
    live_native: bool,
) -> Vec<StartSessionMenuChoice> {
    let is_alden = record.runtime_backend == CutexSessionRuntimeBackend::CuteAlden;
    let is_native = record.runtime_backend == CutexSessionRuntimeBackend::HostForeground;
    if is_alden {
        return vec![
            menu_choice(
                StartSessionMenuAction::ResumeAttach,
                None,
                "resume / repair and take over TUI",
            ),
            menu_choice(
                StartSessionMenuAction::Attach,
                Some(attachable),
                "attach existing TUI",
            ),
            menu_choice(
                StartSessionMenuAction::Takeover,
                Some(attachable),
                "takeover existing TUI",
            ),
            menu_choice(
                StartSessionMenuAction::Online,
                None,
                "ensure background runtime and TUI peer are online",
            ),
            menu_choice(StartSessionMenuAction::Edit, None, "edit/manage session"),
            menu_choice(
                StartSessionMenuAction::ChooseAnother,
                None,
                "choose another session",
            ),
        ];
    }

    if is_native {
        return vec![
            menu_choice(
                StartSessionMenuAction::Foreground,
                None,
                if live_native {
                    "open visible TUI for managed app-server"
                } else {
                    "start app-server and open visible TUI"
                },
            ),
            menu_choice(
                StartSessionMenuAction::Online,
                None,
                if live_native {
                    "ensure managed app-server is online"
                } else {
                    "online app-server and request desktop TUI"
                },
            ),
            menu_choice(StartSessionMenuAction::Edit, None, "edit/manage session"),
            menu_choice(
                StartSessionMenuAction::ChooseAnother,
                None,
                "choose another session",
            ),
        ];
    }

    if record.app_server_runtime.is_some() {
        return vec![
            menu_choice(
                StartSessionMenuAction::Online,
                None,
                "ensure manager-owned app-server is online",
            ),
            menu_choice(StartSessionMenuAction::Edit, None, "edit/manage session"),
            menu_choice(
                StartSessionMenuAction::ChooseAnother,
                None,
                "choose another session",
            ),
        ];
    }

    let choices = vec![
        menu_choice(
            StartSessionMenuAction::Online,
            None,
            "online managed runtime",
        ),
        menu_choice(
            StartSessionMenuAction::ResumeHere,
            None,
            "resume foreground using current cwd",
        ),
        menu_choice(
            StartSessionMenuAction::ResumeManaged,
            None,
            &format!("resume foreground using {}", managed_cwd_label(record)),
        ),
        menu_choice(StartSessionMenuAction::Edit, None, "edit/manage session"),
        menu_choice(
            StartSessionMenuAction::ChooseAnother,
            None,
            "choose another session",
        ),
    ];
    choices
}

fn menu_choice(
    action: StartSessionMenuAction,
    enabled_marker: Option<bool>,
    label: &str,
) -> StartSessionMenuChoice {
    StartSessionMenuChoice {
        action,
        row: session_presenter::StartSessionMenuRow {
            enabled_marker,
            label: label.to_string(),
        },
    }
}

fn managed_cwd_label(record: &CutexSessionRecord) -> &'static str {
    if record.managed_cwd.is_some() {
        "managed cwd"
    } else {
        "session cwd"
    }
}

#[cfg(test)]
mod tests {
    use cutex::session::model::CutexSessionRuntimeBackend;

    use super::*;

    fn native_record() -> CutexSessionRecord {
        let mut record = CutexSessionRecord::new_at(
            "cutex.native".to_string(),
            Some("019e-native".to_string()),
            "host-b".to_string(),
            "D:\\Projects\\example-project".to_string(),
            Some("aemeath".to_string()),
            "2026-07-02T00:00:00Z".to_string(),
        )
        .expect("record");
        record.runtime_backend = CutexSessionRuntimeBackend::HostForeground;
        record.managed_cwd = Some("D:\\Projects\\example-project".to_string());
        record
    }

    fn alden_record() -> CutexSessionRecord {
        let mut record = native_record();
        record.runtime_backend = CutexSessionRuntimeBackend::CuteAlden;
        record
    }

    #[test]
    fn alden_menu_never_offers_a_second_local_codex_core() {
        let choices = start_session_menu_choices(&alden_record(), false, false);

        assert_eq!(choices[0].action, StartSessionMenuAction::ResumeAttach);
        assert!(choices[0].row.label.contains("take over"));
        assert!(choices
            .iter()
            .all(|choice| choice.action != StartSessionMenuAction::ResumeHere
                && choice.action != StartSessionMenuAction::ResumeManaged));
    }

    #[test]
    fn live_native_menu_opens_a_remote_tui_without_stopping_the_runtime() {
        let choices = start_session_menu_choices(&native_record(), false, true);

        assert_eq!(choices[0].action, StartSessionMenuAction::Foreground);
        assert!(choices[0].row.label.contains("managed app-server"));
        assert!(choices
            .iter()
            .all(|choice| choice.action != StartSessionMenuAction::ResumeHere
                && choice.action != StartSessionMenuAction::ResumeManaged));
    }

    #[test]
    fn offline_native_menu_starts_the_app_server_then_remote_tui() {
        let choices = start_session_menu_choices(&native_record(), false, false);

        assert_eq!(choices[0].action, StartSessionMenuAction::Foreground);
        assert!(choices[0].row.label.contains("start app-server"));
        assert_eq!(choices[1].action, StartSessionMenuAction::Online);
    }
}
