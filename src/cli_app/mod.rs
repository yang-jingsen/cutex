mod account_store;
mod agent;
mod agent_bus_config;
mod agent_bus_forwarding;
mod agent_bus_runtime;
mod agent_bus_server;
mod agent_cli;
mod agent_context;
mod agent_management;
mod app;
mod app_server_runtime;
mod app_server_state_sync;
mod app_server_user_input;
mod auth;
mod im_cli;
mod launch;
mod launch_command;
mod launch_output;
mod launch_presenter;
mod launch_process;
mod launch_session;
#[cfg(test)]
mod legacy;
mod management;
mod management_archive;
mod management_context;
mod management_focus;
mod management_lifecycle;
mod notify;
mod profile;
mod profile_settings;
mod profile_settings_presenter;
mod prompt;
mod root_wizard;
mod rotation;
mod session;
mod session_archive;
mod session_attach;
mod session_listing;
mod session_management;
mod session_presenter;
mod session_reconcile;
mod session_runtime;
mod session_settings;
mod session_start_menu;
mod session_tui;
mod session_tui_actions;
mod session_tui_cutex_projects;
mod session_tui_dispatch;
mod session_tui_profile_settings;
mod session_tui_projects;
mod session_tui_recent;
mod session_tui_settings;
mod session_tui_workspace;
mod session_tui_workspace_events;
mod session_tui_workspace_loading;
mod session_tui_workspace_render;
mod session_wizard;
mod settings;
#[cfg(test)]
mod test_home;
mod usage;

pub(crate) use app::run;

pub(crate) fn json_process_error(error: &anyhow::Error) -> Option<serde_json::Value> {
    session_archive::json_process_error(error)
}
