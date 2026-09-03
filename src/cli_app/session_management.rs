use anyhow::{anyhow, Context};

use cutex::agent_bus::model::AgentGroupUpdateMode;
use cutex::cli::args::{SessionGroupsCommand, SessionProfileCommand, SessionQuickCommand};
use cutex::platform::host::current_host_name;
use cutex::session::model::CutexSessionQuickActionMode;
use cutex::session::projection::runtime_backend_short_label;
use cutex::session::service::{
    adopt_cutex_session, cutex_session_key_for_user_id, expose_cutex_session, hide_cutex_session,
    normalize_cutex_session_managed_cwd_path, persist_cutex_session_store_and_im_record,
    set_cutex_session_profile_by_key, set_cutex_session_quick_action, unmanage_cutex_session,
    update_cutex_session_groups, CutexSessionAdoptOptions, CutexSessionEnsureSeed,
};
use cutex::session::store::load_cutex_session_store;
use cutex::ui::format::{bool_label, compact_home_path};

use super::agent_bus_runtime;
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";

pub(crate) fn cmd_session_show(id: &str) -> anyhow::Result<()> {
    super::session_reconcile::mirror_im_registry_into_cutex_session_store(
        &cutex::im::registry::load_im_registry()?,
    )?;
    let store = load_cutex_session_store()?;
    let key = cutex_session_key_for_user_id(&store, id)
        .ok_or_else(|| anyhow!("cutex session is not known: {id}"))?;
    let record = store
        .sessions
        .get(&key)
        .ok_or_else(|| anyhow!("cutex session disappeared while showing: {key}"))?;
    println!("{}", serde_json::to_string_pretty(record)?);
    Ok(())
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
    if current_cwd && cwd.is_some() {
        anyhow::bail!("Use either --cwd or --current-cwd, not both");
    }
    let managed_cwd = if current_cwd {
        Some(
            std::env::current_dir()
                .context("Failed to determine current directory")?
                .display()
                .to_string(),
        )
    } else {
        cwd.map(|path| normalize_cutex_session_managed_cwd_path(&path))
            .transpose()?
    };
    let mut store = load_cutex_session_store()?;
    let outcome = adopt_cutex_session(
        &mut store,
        id,
        cutex_session_ensure_seed(),
        CutexSessionAdoptOptions {
            display_name: name.as_deref(),
            managed_cwd,
            groups,
            expose_to_im,
            pin,
        },
    )?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    let _ = agent_bus_runtime::maybe_patch_live_agent_groups(
        &outcome.session_id,
        &outcome.groups,
        AgentGroupUpdateMode::Set,
    );
    println!(
        "{GREEN}Adopted{RESET} {BOLD}{}{RESET} as a managed cutex session",
        outcome.display_name
    );
    println!(
        "  {DIM}session{RESET} {}  {DIM}backend{RESET} {}  {DIM}im{RESET} {}",
        outcome.session_id,
        runtime_backend_short_label(outcome.runtime_backend),
        bool_label(outcome.im_visible)
    );
    println!(
        "  {DIM}launch cwd{RESET} {}",
        compact_home_path(&outcome.launch_cwd)
    );
    Ok(())
}

fn cutex_session_ensure_seed() -> CutexSessionEnsureSeed {
    let cwd = std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| ".".to_string());
    CutexSessionEnsureSeed {
        host_id: current_host_name(),
        cwd,
        profile: None,
    }
}

pub(crate) fn cmd_session_expose(
    id: &str,
    name: Option<&str>,
    groups: Vec<String>,
) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let outcome = expose_cutex_session(&mut store, id, cutex_session_ensure_seed(), name, groups)?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    let _ = agent_bus_runtime::maybe_patch_live_agent_groups(
        &outcome.session_id,
        &outcome.groups,
        AgentGroupUpdateMode::Set,
    );
    println!(
        "{GREEN}Exposed{RESET} cutex session {BOLD}{}{RESET}",
        outcome.session_id
    );
    Ok(())
}

pub(crate) fn cmd_session_hide(id: &str) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let outcome = hide_cutex_session(&mut store, id)?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    println!(
        "{YELLOW}Hid{RESET} cutex session {BOLD}{}{RESET}",
        outcome.session_id
    );
    Ok(())
}

pub(crate) fn cmd_session_unmanage(id: &str) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let outcome = unmanage_cutex_session(&mut store, id)?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    println!(
        "{YELLOW}Unmanaged{RESET} cutex session {BOLD}{}{RESET}; history and any running runtime were left untouched",
        outcome.session_id
    );
    Ok(())
}

pub(crate) fn cmd_session_quick(command: SessionQuickCommand) -> anyhow::Result<()> {
    let (id, mode) = match command {
        SessionQuickCommand::Pin { id } => (id, CutexSessionQuickActionMode::Pinned),
        SessionQuickCommand::Hide { id } => (id, CutexSessionQuickActionMode::Hidden),
        SessionQuickCommand::Auto { id } => (id, CutexSessionQuickActionMode::Auto),
    };
    cmd_session_quick_set(&id, mode)
}

pub(crate) fn cmd_session_quick_set(
    id: &str,
    mode: CutexSessionQuickActionMode,
) -> anyhow::Result<()> {
    let mut store = load_cutex_session_store()?;
    let outcome = set_cutex_session_quick_action(&mut store, id, mode)?;
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    println!(
        "{GREEN}Updated{RESET} quick action for {BOLD}{session_id}{RESET}: {}",
        mode.label(),
        session_id = outcome.session_id
    );
    Ok(())
}

pub(crate) fn cmd_session_groups(command: SessionGroupsCommand) -> anyhow::Result<()> {
    let (id, groups, mode) = match command {
        SessionGroupsCommand::Set { id, groups } => (id, groups, AgentGroupUpdateMode::Set),
        SessionGroupsCommand::Add { id, groups } => (id, groups, AgentGroupUpdateMode::Add),
        SessionGroupsCommand::Remove { id, groups } => (id, groups, AgentGroupUpdateMode::Remove),
    };
    let mut store = load_cutex_session_store()?;
    let outcome = update_cutex_session_groups(&mut store, &id, groups, mode)?;
    let groups_label = outcome.groups.join(",");
    persist_cutex_session_store_and_im_record(&store, &outcome.key)?;
    let _ = agent_bus_runtime::maybe_patch_live_agent_groups(
        &outcome.session_id,
        &outcome.groups,
        AgentGroupUpdateMode::Set,
    );
    println!(
        "{GREEN}Updated{RESET} cutex session groups for {BOLD}{session_id}{RESET}: {groups_label}",
        session_id = outcome.session_id
    );
    Ok(())
}

pub(crate) fn cmd_session_profile(command: SessionProfileCommand) -> anyhow::Result<()> {
    let (id, requested_profile) = match command {
        SessionProfileCommand::Set { id, profile } => (id, Some(profile)),
        SessionProfileCommand::Clear { id } => (id, None),
    };
    let mut store = load_cutex_session_store()?;
    let key = cutex_session_key_for_user_id(&store, &id)
        .ok_or_else(|| anyhow!("cutex session is not known: {id}"))?;
    // Resolve the target record before touching the local profile catalog. A
    // direct CLI invocation must never stamp this host's profile name onto a
    // remote session.
    let record = store
        .sessions
        .get(&key)
        .ok_or_else(|| anyhow!("cutex session disappeared while updating profile: {key}"))?;
    let current_host = current_host_name();
    if !cutex::runtime::lifecycle::cutex_session_host_is_local(&record.host_id, &current_host) {
        anyhow::bail!(
            "remote_runtime_manager_required: session host_id={} current_host={} cutex_session_id={}",
            record.host_id,
            current_host,
            record.cutex_session_id
        );
    }
    let profile = requested_profile
        .as_deref()
        .map(super::launch::resolve_launch_profile_override)
        .transpose()?
        .map(|resolved| resolved.account.name);
    let outcome = set_cutex_session_profile_by_key(&mut store, &key, profile.clone())?;
    persist_cutex_session_store_and_im_record(&store, &key)?;
    match profile {
        Some(profile) => println!("{GREEN}Configured{RESET} profile {BOLD}{profile}{RESET} for {}", outcome.session_id),
        None => println!("{GREEN}Cleared{RESET} configured profile for {}; next launch follows the global default", outcome.session_id),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_app::test_home::IsolatedTestHome;
    use cutex::profiles::materialize::materialized_account_files;
    use cutex::profiles::model::{AccountsStore, CliKind, RuntimeConfig, StoredAccount};
    use cutex::profiles::store::save_store;
    use cutex::session::model::CutexSessionRecord;
    use cutex::session::store::save_cutex_session_store;
    use std::fs;

    fn profile(name: &str) -> StoredAccount {
        StoredAccount {
            id: format!("{name}-id"),
            name: name.to_string(),
            email: None,
            plan_type: None,
            source: Some("test".to_string()),
            runtime: RuntimeConfig::Host,
            proxy: None,
            session: None,
            cli_kind: CliKind::Codex,
            default_cli_args: Vec::new(),
            agent_name: None,
            last_used_at: None,
        }
    }

    fn save_resolvable_profile(account: &StoredAccount) {
        save_store(&AccountsStore {
            version: 3,
            accounts: vec![account.clone()],
            active_account_id: Some(account.id.clone()),
        })
        .expect("save profile store");
        let files = materialized_account_files(account).expect("profile files");
        fs::create_dir_all(files.auth_path.parent().expect("profile parent"))
            .expect("create profile directory");
        fs::write(&files.auth_path, "{}\n").expect("write profile auth");
        fs::write(&files.config_path, "model = \"test\"\n").expect("write profile config");
    }

    fn save_session(id: &str, host_id: String) {
        let record = CutexSessionRecord::new_at(
            id.to_string(),
            Some("thread-profile-command".to_string()),
            host_id,
            "/tmp/profile-command".to_string(),
            None,
            "2026-08-15T00:00:00Z".to_string(),
        )
        .expect("session record");
        let mut store = load_cutex_session_store().expect("load session store");
        store.sessions.insert(id.to_string(), record);
        save_cutex_session_store(&store).expect("save session store");
    }

    #[test]
    fn profile_command_uses_real_store_for_set_clear_and_rejects_unknown_or_remote_targets() {
        let _home = IsolatedTestHome::new("csm").expect("create isolated HOME");
        let local_id = "cutex.profile-command-local";
        let remote_id = "cutex.profile-command-remote";
        let account = profile("alpha");
        save_resolvable_profile(&account);
        let local_host = current_host_name();
        save_session(local_id, local_host.clone());
        save_session(remote_id, format!("remote-{local_host}"));

        cmd_session_profile(SessionProfileCommand::Set {
            id: local_id.to_string(),
            profile: "alpha".to_string(),
        })
        .expect("set configured profile");
        assert_eq!(
            load_cutex_session_store().expect("load store").sessions[local_id]
                .profile
                .as_deref(),
            Some("alpha")
        );

        cmd_session_profile(SessionProfileCommand::Clear {
            id: local_id.to_string(),
        })
        .expect("clear configured profile");
        assert_eq!(
            load_cutex_session_store().expect("reload store").sessions[local_id].profile,
            None
        );

        let before_unknown = load_cutex_session_store()
            .expect("load before unknown profile")
            .sessions[local_id]
            .clone();
        assert!(cmd_session_profile(SessionProfileCommand::Set {
            id: local_id.to_string(),
            profile: "missing".to_string(),
        })
        .is_err());
        assert_eq!(
            load_cutex_session_store()
                .expect("reload after unknown")
                .sessions[local_id],
            before_unknown
        );

        let before_remote = load_cutex_session_store()
            .expect("load before remote")
            .sessions[remote_id]
            .clone();
        assert!(cmd_session_profile(SessionProfileCommand::Set {
            id: remote_id.to_string(),
            profile: "alpha".to_string(),
        })
        .is_err());
        assert_eq!(
            load_cutex_session_store()
                .expect("reload after remote")
                .sessions[remote_id],
            before_remote
        );
    }
}
