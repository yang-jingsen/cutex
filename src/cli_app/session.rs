use cutex::cli::args::{
    SessionCommand, SessionCwdCommand, SessionDefaultsCommand, SessionGroupsCommand,
    SessionListArgs, SessionProfileCommand, SessionQuickCommand,
};
use cutex::session::model::{
    CutexSessionQuickActionMode, CutexSessionRecord, CutexSessionUserAction,
};
use cutex::session::projection::CutexSessionListFilter;

pub(crate) fn run_command(command: SessionCommand) -> anyhow::Result<()> {
    match command {
        SessionCommand::Wizard { list } => cmd_session_wizard(&list),
        SessionCommand::List { list } => cmd_session_list(&list),
        SessionCommand::Show { id } => cmd_session_show(&id),
        SessionCommand::Retired { json } => super::session_archive::cmd_session_retired(json),
        SessionCommand::Retire { id, reason, json } => {
            super::session_archive::cmd_session_retire(&id, reason.as_deref(), json)
        }
        SessionCommand::Restore { id, json } => {
            super::session_archive::cmd_session_restore(&id, json)
        }
        SessionCommand::Adopt {
            id,
            name,
            cwd,
            current_cwd,
            groups,
            expose_to_im,
            pin,
        } => cmd_session_adopt(&id, name, cwd, current_cwd, groups, expose_to_im, pin),
        SessionCommand::Expose { id, name, groups } => {
            cmd_session_expose(&id, name.as_deref(), groups)
        }
        SessionCommand::Hide { id } => cmd_session_hide(&id),
        SessionCommand::Unmanage { id } => cmd_session_unmanage(&id),
        SessionCommand::Quick { command } => cmd_session_quick(command),
        SessionCommand::Groups { command } => cmd_session_groups(command),
        SessionCommand::Profile { command } => cmd_session_profile(command),
        SessionCommand::Defaults { command } => cmd_session_defaults(command),
        SessionCommand::Cwd { command } => cmd_session_cwd(command),
        SessionCommand::Online { id, profile } => {
            cmd_session_online_with_profile(&id, profile.as_deref(), true).map(|_| ())
        }
        SessionCommand::Foreground { id, profile } => {
            cmd_session_foreground_with_profile(&id, profile.as_deref())
        }
        SessionCommand::Offline { id, force } => {
            cmd_session_lifecycle_action(&id, "session.offline", force)
        }
        SessionCommand::Close { id, force } => {
            cmd_session_lifecycle_action(&id, "session.close", force)
        }
        SessionCommand::Attach { name, takeover } => {
            super::session_attach::cmd_session_attach(&name, takeover)
        }
        SessionCommand::Takeover { id } => cmd_session_takeover(&id),
        SessionCommand::DuplicateCheck { id, json } => cmd_session_duplicate_check(&id, json),
    }
}

pub(crate) fn start_wizard(list: &SessionListArgs) -> anyhow::Result<()> {
    super::session_wizard::cmd_start_wizard(list)
}

pub(crate) fn cmd_session_wizard(list: &SessionListArgs) -> anyhow::Result<()> {
    super::session_wizard::cmd_session_wizard(list)
}

pub(crate) fn record_cutex_session_user_action(
    id: &str,
    action: CutexSessionUserAction,
) -> anyhow::Result<()> {
    super::session_runtime::record_cutex_session_user_action(id, action)
}

pub(crate) fn cmd_session_takeover(id: &str) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_takeover(id)
}

pub(crate) fn cmd_session_resume_alden(id: &str) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_resume_alden(id)
}

pub(crate) fn cmd_session_resume_alden_with_profile(
    id: &str,
    launch_profile: Option<&str>,
) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_resume_alden_with_profile(id, launch_profile)
}

pub(crate) fn cmd_session_duplicate_check(id: &str, json: bool) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_duplicate_check(id, json)
}

pub(crate) fn retire_session(id: &str) -> anyhow::Result<()> {
    super::session_archive::retire(id, None).map(|_| ())
}

pub(crate) fn restore_session(id: &str) -> anyhow::Result<()> {
    super::session_archive::restore(id).map(|_| ())
}

pub(crate) fn cmd_session_lifecycle_action(
    id: &str,
    action_type: &str,
    force: bool,
) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_lifecycle_action(id, action_type, force)
}

pub(crate) fn cmd_session_online_with_profile(
    id: &str,
    launch_profile: Option<&str>,
    open_visible_terminal: bool,
) -> anyhow::Result<serde_json::Value> {
    super::session_runtime::cmd_session_online_with_profile(
        id,
        launch_profile,
        open_visible_terminal,
    )
}

pub(crate) fn cmd_session_close_and_restart_with_profile(
    id: &str,
    launch_profile: Option<&str>,
    open_visible_terminal: bool,
) -> anyhow::Result<serde_json::Value> {
    super::session_runtime::cmd_session_close_and_restart_with_profile(
        id,
        launch_profile,
        open_visible_terminal,
    )
}

pub(crate) fn cmd_session_close_and_wait(id: &str) -> anyhow::Result<serde_json::Value> {
    super::session_runtime::cmd_session_close_and_wait(id)
}

pub(crate) fn cmd_session_close_and_wait_quiet(id: &str) -> anyhow::Result<serde_json::Value> {
    super::session_runtime::cmd_session_close_and_wait_quiet(id)
}

pub(crate) fn cmd_session_foreground(id: &str) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_foreground(id)
}

pub(crate) fn cmd_session_foreground_with_profile(
    id: &str,
    launch_profile: Option<&str>,
) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_foreground_with_profile(id, launch_profile)
}

pub(crate) fn cmd_session_resume_foreground(
    record: &CutexSessionRecord,
    cwd_override: Option<&str>,
) -> anyhow::Result<()> {
    super::session_runtime::cmd_session_resume_foreground(record, cwd_override)
}

pub(crate) fn cmd_session_list(list: &SessionListArgs) -> anyhow::Result<()> {
    super::session_listing::cmd_session_list(list)
}

pub(crate) fn cutex_session_list_filter_from_args(
    list: &SessionListArgs,
) -> CutexSessionListFilter {
    super::session_listing::cutex_session_list_filter_from_args(list)
}

pub(crate) fn cmd_session_show(id: &str) -> anyhow::Result<()> {
    super::session_management::cmd_session_show(id)
}

pub(crate) fn cmd_session_adopt(
    id: &str,
    name: Option<String>,
    cwd: Option<String>,
    current_cwd: bool,
    groups: Vec<String>,
    expose_to_im: bool,
    pin: bool,
) -> anyhow::Result<()> {
    super::session_management::cmd_session_adopt(
        id,
        name,
        cwd,
        current_cwd,
        groups,
        expose_to_im,
        pin,
    )
}

pub(crate) fn cmd_session_expose(
    id: &str,
    name: Option<&str>,
    groups: Vec<String>,
) -> anyhow::Result<()> {
    super::session_management::cmd_session_expose(id, name, groups)
}

pub(crate) fn cmd_session_hide(id: &str) -> anyhow::Result<()> {
    super::session_management::cmd_session_hide(id)
}

pub(crate) fn cmd_session_unmanage(id: &str) -> anyhow::Result<()> {
    super::session_management::cmd_session_unmanage(id)
}

pub(crate) fn cmd_session_quick(command: SessionQuickCommand) -> anyhow::Result<()> {
    super::session_management::cmd_session_quick(command)
}

pub(crate) fn cmd_session_quick_set(
    id: &str,
    mode: CutexSessionQuickActionMode,
) -> anyhow::Result<()> {
    super::session_management::cmd_session_quick_set(id, mode)
}

pub(crate) fn cmd_session_groups(command: SessionGroupsCommand) -> anyhow::Result<()> {
    super::session_management::cmd_session_groups(command)
}

pub(crate) fn cmd_session_profile(command: SessionProfileCommand) -> anyhow::Result<()> {
    super::session_management::cmd_session_profile(command)
}

pub(crate) fn cmd_session_defaults(command: SessionDefaultsCommand) -> anyhow::Result<()> {
    super::session_settings::cmd_session_defaults(command)
}

pub(crate) fn cmd_session_defaults_edit(id: &str) -> anyhow::Result<()> {
    super::session_settings::cmd_session_defaults_edit(id)
}

pub(crate) fn cmd_session_cwd(command: SessionCwdCommand) -> anyhow::Result<()> {
    super::session_settings::cmd_session_cwd(command)
}
