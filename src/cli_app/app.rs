use clap::Parser;
use cutex::agent_bus::identity::{merge_agent_groups, normalize_agent_groups};
use cutex::cli::args::{Cli, CommandKind, SessionListArgs, SessionListSort};

use super::{
    agent, auth, launch, launch_output::LaunchOutput, management, notify, profile, root_wizard,
    session, session_tui, settings, usage,
};

pub(crate) fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if session_archive_requests_json(&cli) {
        super::app_server_runtime::suppress_background_diagnostics();
    }
    if should_open_session_tui(&cli) {
        return session_tui::run();
    }
    root_wizard::set_codez_codex_home()?;

    match cli.command {
        Some(CommandKind::List) => profile::list()?,
        Some(CommandKind::Current) => profile::current()?,
        Some(CommandKind::Use { target }) => profile::use_profile(&target)?,
        Some(CommandKind::Run {
            profile,
            host,
            agent,
            groups,
            docker_image,
            docker_user_name,
            codex_args,
        }) => {
            let output = LaunchOutput::for_child_args(&codex_args);
            launch::print_build(output);
            let agent_groups = merge_agent_groups(cli.groups, groups);
            launch::run_profile(
                &profile,
                codex_args,
                output,
                cli.host || host,
                cli.agent || agent || !agent_groups.is_empty(),
                agent_groups,
                docker_image,
                docker_user_name,
            )?
        }
        Some(CommandKind::Start { list }) => {
            launch::print_build(LaunchOutput::Human);
            session::start_wizard(&list)?
        }
        Some(CommandKind::Tui) => unreachable!("TUI command is handled before runtime setup"),
        Some(CommandKind::Add {
            from_auth,
            from_config,
            docker_image,
            docker_user_name,
            name,
            cli,
        }) => auth::add(
            &from_auth,
            from_config.as_deref(),
            docker_image,
            docker_user_name,
            &name,
            &cli,
        )?,
        Some(CommandKind::Login {
            name,
            cli,
            api_key,
            base_url,
            provider,
        }) => auth::login(
            name.as_deref(),
            cli.as_deref(),
            api_key.as_deref(),
            base_url.as_deref(),
            provider.as_deref(),
        )?,
        Some(CommandKind::Rename { target, name }) => profile::rename(&target, &name)?,
        Some(CommandKind::Remove { target }) => profile::remove(&target)?,
        Some(CommandKind::Annotate {
            target,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
        }) => profile::annotate(
            &target,
            source,
            clear_source,
            plan,
            clear_plan,
            email,
            clear_email,
        )?,
        Some(CommandKind::Runtime {
            target,
            host,
            docker_image,
            docker_user_name,
        }) => profile::runtime(&target, host, docker_image, docker_user_name)?,
        Some(CommandKind::Profile { command }) => profile::run_command(command)?,
        Some(CommandKind::Global { command }) => settings::global(command)?,
        Some(CommandKind::Usage {
            period,
            group_by,
            since,
            until,
            last,
            reset_window,
            json,
        }) => usage::run(
            period,
            group_by,
            since.as_deref(),
            until.as_deref(),
            last.as_deref(),
            reset_window,
            json,
        )?,
        Some(CommandKind::Proxy { command }) => settings::proxy(command)?,
        Some(CommandKind::Session { command }) => session::run_command(command)?,
        Some(CommandKind::Notify { command }) => notify::run_command(command)?,
        Some(CommandKind::Im { command }) => agent::im(command)?,
        Some(CommandKind::Management { command }) => management::run_command(command)?,
        Some(CommandKind::Agent { command }) => agent::run_command(command)?,
        Some(CommandKind::Wizard) => launch::wizard()?,
        None => {
            let output = LaunchOutput::for_child_args(&cli.codex_args);
            launch::print_build(output);
            let agent_groups = normalize_agent_groups(cli.groups);
            launch::quick_run(
                cli.codex_args,
                output,
                cli.quick,
                cli.host,
                cli.agent || !agent_groups.is_empty(),
                agent_groups,
            )?
        }
    }

    Ok(())
}

fn session_archive_requests_json(cli: &Cli) -> bool {
    matches!(
        cli.command.as_ref(),
        Some(CommandKind::Session {
            command: cutex::cli::args::SessionCommand::Retire { json: true, .. }
                | cutex::cli::args::SessionCommand::Restore { json: true, .. }
        })
    )
}

fn should_open_session_tui(cli: &Cli) -> bool {
    match cli.command.as_ref() {
        Some(CommandKind::Tui) => true,
        Some(CommandKind::Start { list }) => session_list_args_are_default(list),
        None => {
            !cli.quick
                && !cli.host
                && !cli.agent
                && normalize_agent_groups(cli.groups.clone()).is_empty()
                && cli.codex_args.is_empty()
        }
        Some(_) => false,
    }
}

fn session_list_args_are_default(list: &SessionListArgs) -> bool {
    let SessionListArgs {
        all,
        offline,
        one_shot,
        host,
        alden,
        attachable,
        projects,
        groups,
        sort,
    } = list;
    !*all
        && !*offline
        && !*one_shot
        && !*host
        && !*alden
        && !*attachable
        && projects.is_empty()
        && groups.is_empty()
        && *sort == SessionListSort::Status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("CLI should parse")
    }

    fn assert_non_default(update: impl FnOnce(&mut SessionListArgs)) {
        let mut list = SessionListArgs::default();
        update(&mut list);
        assert!(!session_list_args_are_default(&list));
    }

    #[test]
    fn default_root_tui_and_bare_start_open_the_session_tui() {
        assert!(should_open_session_tui(&parsed(&["cutex"])));
        assert!(should_open_session_tui(&parsed(&["cutex", "tui"])));
        assert!(should_open_session_tui(&parsed(&["cutex", "start"])));
        assert!(should_open_session_tui(&parsed(&[
            "cutex", "start", "--sort", "status",
        ])));
    }

    #[test]
    fn every_non_default_start_list_field_keeps_the_compatibility_picker() {
        assert!(session_list_args_are_default(&SessionListArgs::default()));
        assert_non_default(|list| list.all = true);
        assert_non_default(|list| list.offline = true);
        assert_non_default(|list| list.one_shot = true);
        assert_non_default(|list| list.host = true);
        assert_non_default(|list| list.alden = true);
        assert_non_default(|list| list.attachable = true);
        assert_non_default(|list| list.projects.push("waveline".to_string()));
        assert_non_default(|list| list.groups.push("waveline".to_string()));
        assert_non_default(|list| list.sort = SessionListSort::Recent);
    }

    #[test]
    fn plain_launch_intent_never_opens_the_tui() {
        for invocation in [
            &["cutex", "--quick"][..],
            &["cutex", "--host"][..],
            &["cutex", "--agent"][..],
            &["cutex", "--group", "waveline"][..],
            &["cutex", "--", "--version"][..],
        ] {
            assert!(!should_open_session_tui(&parsed(invocation)));
        }
    }

    #[test]
    fn json_archive_commands_suppress_unrelated_background_diagnostics() {
        assert!(session_archive_requests_json(&parsed(&[
            "cutex",
            "session",
            "retire",
            "session-1",
            "--json"
        ])));
        assert!(session_archive_requests_json(&parsed(&[
            "cutex",
            "session",
            "restore",
            "session-1",
            "--json"
        ])));
        assert!(!session_archive_requests_json(&parsed(&[
            "cutex",
            "session",
            "retire",
            "session-1"
        ])));
    }

    #[test]
    fn filtered_start_and_explicit_wizards_remain_fallback_routes() {
        assert!(!should_open_session_tui(&parsed(&[
            "cutex",
            "start",
            "--attachable",
        ])));
        assert!(!should_open_session_tui(&parsed(&[
            "cutex", "start", "--sort", "recent",
        ])));
        assert!(!should_open_session_tui(&parsed(&["cutex", "wizard",])));
        assert!(!should_open_session_tui(&parsed(&["cutex", "config",])));
        assert!(!should_open_session_tui(&parsed(&[
            "cutex", "session", "wizard",
        ])));
    }
}
