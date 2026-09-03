use anyhow::anyhow;
use anyhow::Context;

use cutex::agent_bus::service::validate_agent_bus_port;
use cutex::agent_bus::service::DEFAULT_AGENT_BUS_PORT;
use cutex::cli::args::{GlobalCommand, ProxyCommand};
use cutex::config::global_settings::{
    apply_global_config_patch, parse_notify_events, parse_notify_user_message_content,
    parse_rate_limit_mode, ConfigValueUpdate, GlobalConfigPatch,
};
use cutex::config::proxy::*;
use cutex::config::store::load_codez_config;
use cutex::config::store::load_codez_config_checked;
use cutex::config::store::save_codez_config;
use cutex::profiles::inspect::session_config_label;
use cutex::profiles::lookup::find_account;
use cutex::profiles::lookup::resolve_configured_default_profile_name;
use cutex::profiles::store::save_store;
use cutex::ui::format::bool_label;
use cutex::ui::format::optional_u64_label;
use cutex::ui::format::proxy_config_label;

use super::account_store::load_store;
use super::profile_settings_presenter;
use super::prompt::*;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const DEFAULT_NOTIFY_EVENTS: &str =
    "task_completed,thinking_too_long,waiting_approval,connection_error,session_exit,session_started,session_startup_idle,user_message_sent,user_message_dispatched,turn_started,turn_completed,turn_interrupted,turn_failed,approval_requested,approval_resolved,thread_closed,context_compacted,rate_limit_warning,rate_limit_prompt_shown";

pub(crate) fn global(command: GlobalCommand) -> anyhow::Result<()> {
    match command {
        GlobalCommand::Show => cmd_global_show(),
        GlobalCommand::Edit => cmd_global_edit(),
        GlobalCommand::Set {
            docker_use_sudo,
            session_enable,
            default_profile,
            clear_default_profile,
            default_profile_direct_launch,
            proxy_url,
            proxy_no_proxy,
            proxy_force_http_transport,
            proxy_clear,
            notify_idle_timeout,
            notify_composer_idle_timeout,
            notify_approval_timeout,
            notify_startup_idle_timeout,
            notify_events,
            notify_user_message_content,
            notify_user_message_preview_chars,
            rate_limit_threshold_warning_mode,
            rate_limit_model_nudge_mode,
            agent_bus_enable,
            agent_bus_port,
            agent_bus_token,
            agent_message_prefix,
            agent_message_suffix,
        } => cmd_global_set(GlobalSetOptions {
            docker_use_sudo,
            session_enable,
            default_profile,
            clear_default_profile,
            default_profile_direct_launch,
            proxy_url,
            proxy_no_proxy,
            proxy_force_http_transport,
            proxy_clear,
            notify_idle_timeout,
            notify_composer_idle_timeout,
            notify_approval_timeout,
            notify_startup_idle_timeout,
            notify_events,
            notify_user_message_content,
            notify_user_message_preview_chars,
            rate_limit_threshold_warning_mode,
            rate_limit_model_nudge_mode,
            agent_bus_enable,
            agent_bus_port,
            agent_bus_token,
            agent_message_prefix,
            agent_message_suffix,
        }),
    }
}

pub(crate) fn proxy(command: ProxyCommand) -> anyhow::Result<()> {
    match command {
        ProxyCommand::Show { profile } => cmd_proxy_show(profile),
        ProxyCommand::Set {
            url,
            no_proxy,
            force_http_transport,
        } => cmd_proxy_set(url, no_proxy, force_http_transport),
        ProxyCommand::Clear => cmd_proxy_clear(),
        ProxyCommand::SetProfile {
            profile,
            url,
            no_proxy,
            force_http_transport,
        } => cmd_proxy_set_profile(&profile, url, no_proxy, force_http_transport),
        ProxyCommand::DisableProfile { profile } => cmd_proxy_disable_profile(&profile),
        ProxyCommand::ClearProfile { profile } => cmd_proxy_clear_profile(&profile),
    }
}

pub(crate) fn cmd_global_edit() -> anyhow::Result<()> {
    loop {
        let config = load_codez_config_checked()?;
        println!();
        println!("{BOLD}{CYAN}Global Settings Wizard{RESET}");
        println!("{DIM}Boolean rows toggle immediately. Text rows prompt for a new value. Use `-` to clear optional values.{RESET}");
        println!(
            "  1. {} docker_use_sudo                         {}",
            checkbox(config.docker_use_sudo),
            bool_label(config.docker_use_sudo)
        );
        println!(
            "  2. {} managed sessions                        {}",
            checkbox(config.session.enabled),
            session_config_label(&config.session)
        );
        println!(
            "  3. {} direct launch default profile          {}",
            checkbox(config.default_profile_direct_launch),
            bool_label(config.default_profile_direct_launch)
        );
        println!(
            "  4.     default profile                        {}",
            config.default_profile.as_deref().unwrap_or("-")
        );
        println!(
            "  5. {} global proxy enabled                    {}",
            checkbox(config.proxy.as_ref().is_some_and(|proxy| proxy.enabled)),
            proxy_config_label(config.proxy.as_ref())
        );
        println!(
            "  6.     proxy url                              {}",
            config
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.url.as_deref())
                .unwrap_or("-")
        );
        println!(
            "  7.     proxy no_proxy                         {}",
            config
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.no_proxy.as_deref())
                .unwrap_or("-")
        );
        println!(
            "  8. {} proxy force_http                        {}",
            checkbox(
                config
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.force_http_transport)
            ),
            config
                .proxy
                .as_ref()
                .map(|proxy| bool_label(proxy.force_http_transport))
                .unwrap_or("-")
        );
        println!(
            "  9.     notify service url                     {}",
            config.notify_service_url.as_deref().unwrap_or("-")
        );
        println!(
            " 10.     notify service token                   {}",
            if config
                .notify_service_token
                .as_ref()
                .is_some_and(|token| !token.is_empty())
            {
                "(set)"
            } else {
                "-"
            }
        );
        println!(
            " 11.     notify idle timeout                    {}",
            optional_u64_label(config.notify_service_idle_timeout_secs)
        );
        println!(
            " 12.     notify composer idle timeout           {}",
            optional_u64_label(config.notify_service_composer_idle_timeout_secs)
        );
        println!(
            " 13.     notify approval timeout                {}",
            optional_u64_label(config.notify_service_approval_timeout_secs)
        );
        println!(
            " 14.     notify startup idle timeout            {}",
            optional_u64_label(config.notify_service_startup_idle_timeout_secs)
        );
        println!(
            " 15.     notify events                          {}",
            config
                .notify_service_events
                .as_ref()
                .map(|events| events.join(","))
                .unwrap_or_else(|| "-".to_string())
        );
        println!(
            " 16.     notify user message content            {}",
            config
                .notify_service_user_message_content
                .as_deref()
                .unwrap_or("-")
        );
        println!(
            " 17.     notify user message preview chars      {}",
            optional_u64_label(config.notify_service_user_message_preview_chars)
        );
        println!(
            " 18.     rate limit threshold warning mode      {}",
            config
                .rate_limit_threshold_warning_mode
                .as_deref()
                .unwrap_or("-")
        );
        println!(
            " 19.     rate limit model nudge mode            {}",
            config.rate_limit_model_nudge_mode.as_deref().unwrap_or("-")
        );
        println!(
            " 20. {} agent bus enabled                     {}",
            checkbox(config.agent_bus_enabled),
            bool_label(config.agent_bus_enabled)
        );
        println!(
            " 21.     agent bus port                        {}",
            config.agent_bus_port.unwrap_or(DEFAULT_AGENT_BUS_PORT)
        );
        println!(
            " 22.     agent bus token                       {}",
            if config
                .agent_bus_token
                .as_ref()
                .is_some_and(|token| !token.is_empty())
            {
                "(set)"
            } else {
                "-"
            }
        );
        println!(
            " 23.     agent message prefix                  {}",
            config
                .agent_message_prefix_template
                .as_deref()
                .unwrap_or("-")
        );
        println!(
            " 24.     agent message suffix                  {}",
            config
                .agent_message_suffix_template
                .as_deref()
                .unwrap_or("-")
        );
        println!(" 25.     show current settings");

        let Some(choice) = read_wizard_choice(25)? else {
            println!("Done.");
            return Ok(());
        };

        let mut next = config.clone();
        match choice {
            1 => next.docker_use_sudo = !next.docker_use_sudo,
            2 => next.session.enabled = !next.session.enabled,
            3 => next.default_profile_direct_launch = !next.default_profile_direct_launch,
            4 => {
                let store = load_store()?;
                let value = prompt_optional_string(
                    "Default profile name or id",
                    next.default_profile.as_deref(),
                )?;
                next.default_profile = resolve_configured_default_profile_name(&store, value)?;
            }
            5 => {
                if next.proxy.as_ref().is_some_and(|proxy| proxy.enabled) {
                    next.proxy = None;
                } else {
                    let url = prompt_line("Proxy URL", "socks5h://127.0.0.1:7890")?;
                    next.proxy = Some(proxy_config_from_parts(
                        true,
                        Some(url),
                        None,
                        /*force_http_transport*/ true,
                    )?);
                }
            }
            6 => {
                let url = prompt_optional_string(
                    "Proxy URL",
                    next.proxy.as_ref().and_then(|proxy| proxy.url.as_deref()),
                )?;
                next.proxy = url
                    .map(|url| {
                        proxy_config_from_parts(
                            true,
                            Some(url),
                            next.proxy.as_ref().and_then(|proxy| proxy.no_proxy.clone()),
                            next.proxy
                                .as_ref()
                                .map(|proxy| proxy.force_http_transport)
                                .unwrap_or(true),
                        )
                    })
                    .transpose()?;
            }
            7 => {
                let Some(proxy) = next.proxy.as_mut() else {
                    println!("{YELLOW}Enable proxy first.{RESET}");
                    continue;
                };
                proxy.no_proxy =
                    prompt_optional_string("Proxy NO_PROXY", proxy.no_proxy.as_deref())?;
            }
            8 => {
                let Some(proxy) = next.proxy.as_mut() else {
                    println!("{YELLOW}Enable proxy first.{RESET}");
                    continue;
                };
                proxy.force_http_transport = !proxy.force_http_transport;
            }
            9 => {
                next.notify_service_url = prompt_optional_string(
                    "Notify service URL",
                    next.notify_service_url.as_deref(),
                )?;
            }
            10 => {
                next.notify_service_token = prompt_optional_string(
                    "Notify service token",
                    next.notify_service_token.as_deref(),
                )?;
            }
            11 => {
                next.notify_service_idle_timeout_secs = prompt_optional_u64(
                    "Notify idle timeout seconds",
                    next.notify_service_idle_timeout_secs,
                )?;
            }
            12 => {
                next.notify_service_composer_idle_timeout_secs = prompt_optional_u64(
                    "Notify composer idle timeout seconds",
                    next.notify_service_composer_idle_timeout_secs,
                )?;
            }
            13 => {
                next.notify_service_approval_timeout_secs = prompt_optional_u64(
                    "Notify approval timeout seconds",
                    next.notify_service_approval_timeout_secs,
                )?;
            }
            14 => {
                next.notify_service_startup_idle_timeout_secs = prompt_optional_u64(
                    "Notify startup idle timeout seconds",
                    next.notify_service_startup_idle_timeout_secs,
                )?;
            }
            15 => {
                let current = next.notify_service_events.as_deref();
                let events = prompt_optional_csv("Notify event CSV", current)?;
                next.notify_service_events = events;
                if next.notify_service_events.is_none() {
                    println!(
                        "{DIM}Using cute-codex default events: {DEFAULT_NOTIFY_EVENTS}{RESET}"
                    );
                }
            }
            16 => {
                let current = next
                    .notify_service_user_message_content
                    .as_deref()
                    .unwrap_or("-");
                let value = prompt_line(
                    "Notify user message content: none, preview, full (`-` clears)",
                    current,
                )?;
                next.notify_service_user_message_content =
                    parse_notify_user_message_content(&value)?;
            }
            17 => {
                next.notify_service_user_message_preview_chars = prompt_optional_u64(
                    "Notify user message preview chars",
                    next.notify_service_user_message_preview_chars,
                )?;
            }
            18 => {
                let current = next
                    .rate_limit_threshold_warning_mode
                    .as_deref()
                    .unwrap_or("-");
                let value = prompt_line(
                    "Rate limit threshold warning mode: off, daily, always (`-` clears)",
                    current,
                )?;
                next.rate_limit_threshold_warning_mode = parse_rate_limit_mode(&value)?;
            }
            19 => {
                let current = next.rate_limit_model_nudge_mode.as_deref().unwrap_or("-");
                let value = prompt_line(
                    "Rate limit model nudge mode: off, daily, always (`-` clears)",
                    current,
                )?;
                next.rate_limit_model_nudge_mode = parse_rate_limit_mode(&value)?;
            }
            20 => next.agent_bus_enabled = !next.agent_bus_enabled,
            21 => {
                let current = next
                    .agent_bus_port
                    .unwrap_or(DEFAULT_AGENT_BUS_PORT)
                    .to_string();
                let value = prompt_line("Agent bus port", &current)?;
                let port = value
                    .trim()
                    .parse::<u16>()
                    .with_context(|| format!("Invalid agent bus port: {value}"))?;
                validate_agent_bus_port(port)?;
                next.agent_bus_port = Some(port);
            }
            22 => {
                next.agent_bus_token =
                    prompt_optional_string("Agent bus token", next.agent_bus_token.as_deref())?;
            }
            23 => {
                next.agent_message_prefix_template = prompt_optional_string(
                    "Agent message prefix template",
                    next.agent_message_prefix_template.as_deref(),
                )?;
            }
            24 => {
                next.agent_message_suffix_template = prompt_optional_string(
                    "Agent message suffix template",
                    next.agent_message_suffix_template.as_deref(),
                )?;
            }
            25 => {
                profile_settings_presenter::print_global_settings(&config);
                continue;
            }
            _ => unreachable!(),
        }

        save_codez_config(&next)?;
        println!("{GREEN}Saved.{RESET}");
    }
}

pub(crate) struct GlobalSetOptions {
    pub(crate) docker_use_sudo: Option<bool>,
    pub(crate) session_enable: Option<bool>,
    pub(crate) default_profile: Option<String>,
    pub(crate) clear_default_profile: bool,
    pub(crate) default_profile_direct_launch: Option<bool>,
    pub(crate) proxy_url: Option<String>,
    pub(crate) proxy_no_proxy: Option<String>,
    pub(crate) proxy_force_http_transport: Option<bool>,
    pub(crate) proxy_clear: bool,
    pub(crate) notify_idle_timeout: Option<u64>,
    pub(crate) notify_composer_idle_timeout: Option<u64>,
    pub(crate) notify_approval_timeout: Option<u64>,
    pub(crate) notify_startup_idle_timeout: Option<u64>,
    pub(crate) notify_events: Option<String>,
    pub(crate) notify_user_message_content: Option<String>,
    pub(crate) notify_user_message_preview_chars: Option<u64>,
    pub(crate) rate_limit_threshold_warning_mode: Option<String>,
    pub(crate) rate_limit_model_nudge_mode: Option<String>,
    pub(crate) agent_bus_enable: Option<bool>,
    pub(crate) agent_bus_port: Option<u16>,
    pub(crate) agent_bus_token: Option<String>,
    pub(crate) agent_message_prefix: Option<String>,
    pub(crate) agent_message_suffix: Option<String>,
}

pub(crate) fn cmd_global_show() -> anyhow::Result<()> {
    let config = load_codez_config();
    profile_settings_presenter::print_global_settings(&config);
    Ok(())
}

pub(crate) fn cmd_global_set(options: GlobalSetOptions) -> anyhow::Result<()> {
    let GlobalSetOptions {
        docker_use_sudo,
        session_enable,
        default_profile,
        clear_default_profile,
        default_profile_direct_launch,
        proxy_url,
        proxy_no_proxy,
        proxy_force_http_transport,
        proxy_clear,
        notify_idle_timeout,
        notify_composer_idle_timeout,
        notify_approval_timeout,
        notify_startup_idle_timeout,
        notify_events,
        notify_user_message_content,
        notify_user_message_preview_chars,
        rate_limit_threshold_warning_mode,
        rate_limit_model_nudge_mode,
        agent_bus_enable,
        agent_bus_port,
        agent_bus_token,
        agent_message_prefix,
        agent_message_suffix,
    } = options;
    if proxy_no_proxy.is_some() && proxy_url.is_none() {
        anyhow::bail!("--proxy-no-proxy requires --proxy-url");
    }
    if proxy_force_http_transport.is_some() && proxy_url.is_none() {
        anyhow::bail!("--proxy-force-http requires --proxy-url");
    }
    if let Some(port) = agent_bus_port {
        validate_agent_bus_port(port)?;
    }

    if docker_use_sudo.is_none()
        && session_enable.is_none()
        && default_profile.is_none()
        && !clear_default_profile
        && default_profile_direct_launch.is_none()
        && proxy_url.is_none()
        && !proxy_clear
        && notify_idle_timeout.is_none()
        && notify_composer_idle_timeout.is_none()
        && notify_approval_timeout.is_none()
        && notify_startup_idle_timeout.is_none()
        && notify_events.is_none()
        && notify_user_message_content.is_none()
        && notify_user_message_preview_chars.is_none()
        && rate_limit_threshold_warning_mode.is_none()
        && rate_limit_model_nudge_mode.is_none()
        && agent_bus_enable.is_none()
        && agent_bus_port.is_none()
        && agent_bus_token.is_none()
        && agent_message_prefix.is_none()
        && agent_message_suffix.is_none()
    {
        anyhow::bail!(
                    "No changes requested. Provide --docker-use-sudo <BOOL>, --session-enable <BOOL>, --default-profile <PROFILE>, --clear-default-profile, --default-profile-direct-launch <BOOL>, --proxy-url <URL>, --proxy-clear, --notify-idle-timeout <SECS>, --notify-composer-idle-timeout <SECS>, --notify-approval-timeout <SECS>, --notify-startup-idle-timeout <SECS>, --notify-events <CSV>, --notify-user-message-content <MODE>, --notify-user-message-preview-chars <CHARS>, --rate-limit-threshold-warning-mode <MODE>, --rate-limit-model-nudge-mode <MODE>, --agent-bus-enable <BOOL>, --agent-bus-port <PORT>, --agent-bus-token <TOKEN>, --agent-message-prefix <TEMPLATE>, or --agent-message-suffix <TEMPLATE>."
                );
    }

    let mut config = load_codez_config_checked()?;
    let proxy = if proxy_clear {
        ConfigValueUpdate::Clear
    } else if let Some(url) = proxy_url {
        ConfigValueUpdate::Set(proxy_config_from_parts(
            true,
            Some(url),
            proxy_no_proxy,
            proxy_force_http_transport.unwrap_or(true),
        )?)
    } else {
        ConfigValueUpdate::Unchanged
    };
    let notify_events_update =
        requested_optional_update(notify_events.map(|events| parse_notify_events(&events)));
    let notify_message_content_update = requested_optional_update(
        notify_user_message_content
            .map(|content| parse_notify_user_message_content(&content))
            .transpose()?,
    );
    let rate_limit_threshold_update = requested_optional_update(
        rate_limit_threshold_warning_mode
            .map(|mode| parse_rate_limit_mode(&mode))
            .transpose()?,
    );
    let rate_limit_model_nudge_update = requested_optional_update(
        rate_limit_model_nudge_mode
            .map(|mode| parse_rate_limit_mode(&mode))
            .transpose()?,
    );
    let agent_bus_token_update =
        requested_optional_update(agent_bus_token.map(|token| parse_optional_string(&token)));
    let agent_message_prefix_update = requested_optional_update(
        agent_message_prefix.map(|template| parse_optional_string(&template)),
    );
    let agent_message_suffix_update = requested_optional_update(
        agent_message_suffix.map(|template| parse_optional_string(&template)),
    );
    let default_profile_update = if clear_default_profile {
        ConfigValueUpdate::Clear
    } else if let Some(target) = default_profile {
        let store = load_store()?;
        ConfigValueUpdate::Set(
            resolve_configured_default_profile_name(&store, Some(target))?
                .ok_or_else(|| anyhow!("Default profile cannot be empty"))?,
        )
    } else {
        ConfigValueUpdate::Unchanged
    };
    let changed = apply_global_config_patch(
        &mut config,
        &GlobalConfigPatch {
            docker_use_sudo,
            session_enabled: session_enable,
            default_profile: default_profile_update,
            default_profile_direct_launch,
            proxy,
            notify_service_idle_timeout_secs: requested_value_update(notify_idle_timeout),
            notify_service_composer_idle_timeout_secs: requested_value_update(
                notify_composer_idle_timeout,
            ),
            notify_service_approval_timeout_secs: requested_value_update(notify_approval_timeout),
            notify_service_startup_idle_timeout_secs: requested_value_update(
                notify_startup_idle_timeout,
            ),
            notify_service_events: notify_events_update,
            notify_service_user_message_content: notify_message_content_update,
            notify_service_user_message_preview_chars: requested_value_update(
                notify_user_message_preview_chars,
            ),
            rate_limit_threshold_warning_mode: rate_limit_threshold_update,
            rate_limit_model_nudge_mode: rate_limit_model_nudge_update,
            agent_bus_enabled: agent_bus_enable,
            agent_bus_port: requested_value_update(agent_bus_port),
            agent_bus_token: agent_bus_token_update,
            agent_message_prefix_template: agent_message_prefix_update,
            agent_message_suffix_template: agent_message_suffix_update,
            ..GlobalConfigPatch::default()
        },
    )?;

    if changed {
        save_codez_config(&config)?;
        println!("{GREEN}Updated{RESET} global settings");
    } else {
        println!("{YELLOW}No changes{RESET} global settings already match requested values");
    }
    profile_settings_presenter::print_global_settings(&config);

    Ok(())
}

pub(crate) fn cmd_proxy_show(profile: Option<String>) -> anyhow::Result<()> {
    let global_config = load_codez_config();
    if let Some(profile) = profile {
        let store = load_store()?;
        let account = find_account(&store, &profile)?
            .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
        println!("{BOLD}{CYAN}Proxy for profile{RESET} {}", account.name);
        println!(
            "{DIM}profile{RESET} {}",
            proxy_config_label(account.proxy.as_ref())
        );
        println!(
            "{DIM}global{RESET}  {}",
            proxy_config_label(global_config.proxy.as_ref())
        );
        println!(
            "{DIM}effective{RESET} {}",
            proxy_config_label(effective_proxy_config(account, &global_config))
        );
    } else {
        println!("{BOLD}{CYAN}Global Proxy{RESET}");
        println!("{}", proxy_config_label(global_config.proxy.as_ref()));
    }
    Ok(())
}

pub(crate) fn cmd_proxy_set(
    url: String,
    no_proxy: Option<String>,
    force_http_transport: bool,
) -> anyhow::Result<()> {
    let mut config = load_codez_config_checked()?;
    set_global_proxy_config(&mut config, url, no_proxy, force_http_transport)?;
    save_codez_config(&config)?;
    println!(
        "{GREEN}Updated{RESET} global proxy: {}",
        proxy_config_label(config.proxy.as_ref())
    );
    Ok(())
}

pub(crate) fn cmd_proxy_clear() -> anyhow::Result<()> {
    let mut config = load_codez_config_checked()?;
    clear_global_proxy_config(&mut config);
    save_codez_config(&config)?;
    println!("{YELLOW}Cleared{RESET} global proxy");
    Ok(())
}

pub(crate) fn cmd_proxy_set_profile(
    profile: &str,
    url: String,
    no_proxy: Option<String>,
    force_http_transport: bool,
) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.name == profile || account.id == profile)
        .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
    set_account_proxy_config(account, url, no_proxy, force_http_transport)?;
    let name = account.name.clone();
    let label = proxy_config_label(account.proxy.as_ref());
    save_store(&store)?;
    println!("{GREEN}Updated{RESET} proxy for {BOLD}{name}{RESET}: {label}");
    Ok(())
}

pub(crate) fn cmd_proxy_disable_profile(profile: &str) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.name == profile || account.id == profile)
        .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
    disable_account_proxy_config(account)?;
    let name = account.name.clone();
    save_store(&store)?;
    println!("{YELLOW}Disabled{RESET} proxy for {BOLD}{name}{RESET}");
    Ok(())
}

pub(crate) fn cmd_proxy_clear_profile(profile: &str) -> anyhow::Result<()> {
    let mut store = load_store()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.name == profile || account.id == profile)
        .ok_or_else(|| anyhow!("Account not found: {profile}"))?;
    clear_account_proxy_config(account);
    let name = account.name.clone();
    save_store(&store)?;
    println!("{YELLOW}Cleared{RESET} proxy override for {BOLD}{name}{RESET}");
    Ok(())
}

fn requested_value_update<T>(value: Option<T>) -> ConfigValueUpdate<T> {
    value.map_or(ConfigValueUpdate::Unchanged, ConfigValueUpdate::Set)
}

fn requested_optional_update<T>(value: Option<Option<T>>) -> ConfigValueUpdate<T> {
    match value {
        None => ConfigValueUpdate::Unchanged,
        Some(Some(value)) => ConfigValueUpdate::Set(value),
        Some(None) => ConfigValueUpdate::Clear,
    }
}
