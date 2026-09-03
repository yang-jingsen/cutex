use cutex::cli::args::{DesktopNotifyCommand, NotifyCommand};
use cutex::config::store::load_codez_config;
use cutex::notify::desktop::*;
use cutex::notify::service::*;
use cutex::ui::format::bool_label;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

pub(crate) fn run_command(command: NotifyCommand) -> anyhow::Result<()> {
    match command {
        NotifyCommand::Desktop { command } => desktop(command),
    }
}

fn desktop(command: DesktopNotifyCommand) -> anyhow::Result<()> {
    match command {
        DesktopNotifyCommand::Enable { port } => {
            let config = ensure_desktop_notify_config(true, port)?;
            ensure_desktop_notify_bridge_running(&config)?;
            println!(
                "{GREEN}Enabled{RESET} desktop notifications on {}",
                desktop_notify_bridge_url(desktop_notify_port(&config))
            );
            Ok(())
        }
        DesktopNotifyCommand::Disable => {
            disable_desktop_notify_config()?;
            println!("{YELLOW}Disabled{RESET} desktop notifications.");
            Ok(())
        }
        DesktopNotifyCommand::Start { port } => {
            let config =
                ensure_desktop_notify_config(load_codez_config().desktop_notify_enabled, port)?;
            ensure_desktop_notify_bridge_running(&config)?;
            println!(
                "{GREEN}Running{RESET} desktop notification bridge on {}",
                desktop_notify_bridge_url(desktop_notify_port(&config))
            );
            Ok(())
        }
        DesktopNotifyCommand::Status => {
            let config = load_codez_config();
            let port = desktop_notify_port(&config);
            let healthy =
                desktop_notify_bridge_healthy(port, config.desktop_notify_token.as_deref());
            println!("{BOLD}{CYAN}Desktop Notify Bridge{RESET}");
            println!(
                "{DIM}enabled{RESET} {}",
                bool_label(config.desktop_notify_enabled)
            );
            println!("{DIM}port{RESET} {port}");
            println!(
                "{DIM}token{RESET} {}",
                if config
                    .desktop_notify_token
                    .as_ref()
                    .is_some_and(|token| !token.is_empty())
                {
                    "(set)"
                } else {
                    "-"
                }
            );
            println!(
                "{DIM}health{RESET} {}",
                if healthy { "healthy" } else { "not running" }
            );
            println!(
                "{DIM}external_forward{RESET} {}",
                if config
                    .notify_service_url
                    .as_ref()
                    .is_some_and(|url| !url.is_empty())
                {
                    "configured"
                } else {
                    "-"
                }
            );
            Ok(())
        }
        DesktopNotifyCommand::Serve { port, token } => {
            let mut config = load_codez_config();
            if let Some(port) = port {
                config.desktop_notify_port = Some(port);
            }
            if let Some(token) = token {
                config.desktop_notify_token = Some(token);
            }
            run_desktop_notify_bridge(config)
        }
        DesktopNotifyCommand::Test { message } => {
            let message = message.unwrap_or_else(|| "cutex desktop notification test".to_string());
            send_native_desktop_notification("cutex desktop notify", &message)?;
            println!("{GREEN}Sent{RESET} test desktop notification.");
            Ok(())
        }
        DesktopNotifyCommand::InstallUbuntu { port } => {
            let install = install_ubuntu_desktop_notify_service(port)?;
            println!(
                "{GREEN}Installed{RESET} Ubuntu desktop notification service on {}",
                install.bridge_url
            );
            Ok(())
        }
        DesktopNotifyCommand::UninstallUbuntu => {
            uninstall_ubuntu_desktop_notify_service()?;
            println!("{YELLOW}Uninstalled{RESET} Ubuntu desktop notification service.");
            Ok(())
        }
    }
}
