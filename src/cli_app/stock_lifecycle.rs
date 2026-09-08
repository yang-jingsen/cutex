//! Opt-in stock child ownership inside the existing Management process.
use anyhow::{ensure, Context};
use cutex::agent_management::{StockRuntimeExecutor, StockRuntimeReceipt};
use cutex::app_server::runtime::AppServerRuntimeLayout;
use cutex::launch::command::LaunchCommand;
use cutex::launch::stock::{StockBundle, STOCK_SCHEMA_SHA256};
use cutex::session::model::{
    CutexAppServerRuntimeBinding, CutexSessionRecord, LaunchProfileSource,
};
use std::process::Child;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct StockExecutor {
    child: Option<Child>,
}
impl Drop for StockExecutor {
    fn drop(&mut self) {
        let _ = self.cleanup_owned();
    }
}

fn clean_launch(
    program: &std::path::Path,
    home: &std::path::Path,
) -> anyhow::Result<LaunchCommand> {
    let mut launch = LaunchCommand::new(
        program
            .to_str()
            .context("stock executable path must be UTF-8")?,
    );
    // Enumerate names only to remove inherited values, including fork aliases.
    for (key, _) in std::env::vars_os() {
        launch =
            launch.env_remove(key.into_string().map_err(|_| {
                anyhow::anyhow!("non-UTF8 environment key cannot be safely scrubbed")
            })?);
    }
    for key in ["HOME", "TMPDIR"] {
        launch = launch.env(
            key,
            std::env::var(key).with_context(|| format!("private {key} required"))?,
        );
    }
    Ok(launch
        .env(
            "CODEX_HOME",
            home.to_str().context("native home must be UTF-8")?,
        )
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("TERM", "xterm-256color"))
}
fn option(
    launch: LaunchCommand,
    key: &str,
    value: impl serde::Serialize,
) -> anyhow::Result<LaunchCommand> {
    // TOML inline values; no credentials are accepted in this plan.
    let value = toml::Value::try_from(value)?;
    Ok(launch.arg("-c").arg(format!("{key}={value}")))
}

fn configured(
    mut launch: LaunchCommand,
    profile: &cutex::launch::stock::StockConfiguration,
) -> anyhow::Result<LaunchCommand> {
    launch = option(launch, "model", &profile.model)?;
    launch = option(launch, "model_provider", &profile.model_provider)?;
    ensure!(
        profile
            .model_provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
        "unsupported stock provider key"
    );
    launch = option(
        launch,
        &format!("model_providers.{}", profile.model_provider),
        &profile.provider,
    )?;
    launch = option(launch, "approval_policy", &profile.approval)?;
    launch = option(launch, "sandbox_mode", &profile.sandbox)?;
    if let Some(reasoning) = &profile.reasoning {
        launch = option(launch, "model_reasoning_effort", reasoning)?;
    }
    option(launch, "analytics.enabled", false)
}

impl StockRuntimeExecutor for StockExecutor {
    fn stop(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()> {
        let Some(binding) = &record.app_server_runtime else {
            ensure!(
                !cutex::session::archive::record_has_runtime_claim(record),
                "stock owner unavailable; cannot prove stop"
            );
            return Ok(());
        };
        super::app_server_runtime::verify_exact_live_runtime_claim(record, binding)?;
        verify_stock_process(record, binding)?;
        super::app_server_runtime::runtime_manager()
            .interrupt_active_turn(&record.cutex_session_id)?;
        super::app_server_runtime::disconnect_runtime(&record.cutex_session_id)?;
        stop_group(binding.pid)?;
        let path = cutex::session::store::cutex_sessions_path()?;
        cutex::agent_management::commit_stock_runtime_stop(&path, record)
    }
    fn spawn(
        &mut self,
        record: &CutexSessionRecord,
        bundle: &StockBundle,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<CutexAppServerRuntimeBinding> {
        let config = cutex::config::store::load_codez_config_checked()?;
        ensure!(
            config.agent_bus_enabled,
            "existing private Agent Bus required; no automatic service start"
        );
        let token = config
            .agent_bus_token
            .as_ref()
            .filter(|s| !s.is_empty())
            .context("private authenticated Agent Bus required")?;
        let layout = AppServerRuntimeLayout::prepare_stock(&record.cutex_session_id)?;
        let profile = &receipt.review.configuration;
        let mut launch = configured(
            clean_launch(
                &bundle.executable.path,
                &receipt.review.contract.native_home,
            )?,
            profile,
        )?;
        launch = option(
            launch,
            "mcp_servers.cutex",
            serde_json::json!({"command":bundle.facade.path,"env_vars":["CUTEX_AGENT_ID","CUTEX_RUNTIME_GENERATION","CUTEX_AGENT_BUS_URL","CUTEX_AGENT_BUS_TOKEN"],"default_tools_approval_mode":"approve"}),
        )?;
        launch = option(
            launch,
            "code_mode.direct_only_tool_namespaces",
            vec!["mcp__cutex"],
        )?;
        launch = launch
            .args(layout.app_server_args())
            .env("CUTEX_AGENT_ID", &receipt.runtime_agent_id)
            .env(
                "CUTEX_RUNTIME_GENERATION",
                receipt.expected_generation.to_string(),
            )
            .env(
                "CUTEX_AGENT_BUS_URL",
                cutex::agent_bus::service::agent_bus_base_url(
                    cutex::agent_bus::service::agent_bus_port(&config),
                ),
            )
            .env("CUTEX_AGENT_BUS_TOKEN", token);
        let log = std::path::PathBuf::from(layout.binding(0, String::new()).runtime_dir)
            .join("stock.stderr.log");
        // Existing detached child/setsid adapter, without contacting systemd.
        // This private subset claims process-group ownership, not a cgroup.
        self.child = Some(cutex::runtime::lifecycle::spawn_detached_session_launch(
            &launch,
            cutex::session::service::cutex_session_launch_cwd(record),
            &log,
        )?);
        let child = self.child.as_mut().expect("spawned owned child");
        let mut binding = layout.binding(
            child.id(),
            cutex::platform::process::process_started_at(child.id())?.to_rfc3339(),
        );
        binding.launched_profile = Some(profile.profile_name.clone());
        binding.launch_profile_source = Some(if profile.inherited {
            LaunchProfileSource::GlobalDefault
        } else {
            LaunchProfileSource::SessionConfigured
        });
        binding.schema_version = "stock-0.153.4-app-server-v2".into();
        binding.schema_sha256 = STOCK_SCHEMA_SHA256.into();
        super::management_lifecycle::wait_for_app_server_endpoint(&layout, child, &log)?;
        Ok(binding)
    }
    fn connect(
        &mut self,
        record: &CutexSessionRecord,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<()> {
        let binding = receipt.binding.as_ref().context("stock binding missing")?;
        verify_stock_process(record, binding)?;
        let manager = super::app_server_runtime::runtime_manager();
        if let Some(status) = manager
            .status(&record.cutex_session_id)?
            .filter(|s| s.connected)
        {
            ensure!(
                status.thread_id == receipt.review.contract.native_id
                    && status.runtime_generation == receipt.expected_generation,
                "stock manager occurrence mismatch"
            );
        } else {
            let cfg = &receipt.review.configuration;
            manager.connect_binding(
                &record.cutex_session_id,
                binding,
                cutex::app_server::commands::ThreadResumeParams {
                    thread_id: receipt.review.contract.native_id.clone(),
                    model: Some(cfg.model.clone()),
                    model_provider: Some(cfg.model_provider.clone()),
                    cwd: Some(cutex::session::service::cutex_session_launch_cwd(record).into()),
                    approval_policy: Some(serde_json::json!(cfg.approval)),
                    sandbox: Some(cfg.sandbox.clone()),
                    ..Default::default()
                },
                receipt.expected_generation,
                "host",
            )?;
        }
        if manager
            .agent_bus_bridge_status(&record.cutex_session_id)?
            .is_none()
        {
            let mut registration = super::app_server_runtime::runtime_agent_registration(
                record,
                binding,
                &receipt.runtime_agent_id,
            )?;
            // Stock registration carries the reviewed configuration unchanged;
            // it must not derive new collaboration groups from a cwd hash.
            registration.groups = record.agent_groups.clone();
            registration.path_key = None;
            registration.name = receipt.review.subject.formal_name.clone();
            registration.base_name = Some(receipt.review.subject.formal_name.clone());
            let mut options = cutex::app_server::bus_bridge::AppServerAgentBusBridgeOptions::new(
                registration,
                &receipt.review.contract.native_id,
            )
            .with_cutex_session_id(&record.cutex_session_id);
            options.registration_only = true;
            #[allow(unused_mut)]
            let mut config = cutex::config::store::load_codez_config_checked()?;
            #[cfg(feature = "stock-launch-test-hook")]
            {
                static DENIED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if std::env::var("CUTEX_STOCK_TEST_REGISTER_DENY_ACTION")
                    .ok()
                    .as_deref()
                    == Some(receipt.action_id.as_str())
                    && !DENIED.swap(true, std::sync::atomic::Ordering::SeqCst)
                {
                    // Exercise the real private provider's authentication denial,
                    // not a successful fake registration or authority override.
                    config.agent_bus_token = Some("s4-deliberately-invalid-fixture-token".into());
                }
            }
            manager.start_agent_bus_bridge(
                &record.cutex_session_id,
                Arc::new(cutex::agent_bus::client::AgentBusHttpClient::from_config(
                    &config,
                )),
                options,
            )?;
        }
        #[cfg(feature = "stock-launch-test-hook")]
        {
            static INJECTED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if std::env::var("CUTEX_STOCK_TEST_LOST_READY_ACTION")
                .ok()
                .as_deref()
                == Some(receipt.action_id.as_str())
                && !INJECTED.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                anyhow::bail!("private test injected lost readiness after real registration");
            }
        }
        Ok(())
    }
    fn cleanup_owned(&mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.child.take() {
            if child.try_wait()?.is_none() {
                #[cfg(target_os = "linux")]
                {
                    ensure!(
                        unsafe { libc::getpgid(child.id() as i32) } == child.id() as i32,
                        "owned child group changed"
                    );
                    ensure!(
                        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) } == 0,
                        "owned child cleanup failed"
                    );
                }
                #[cfg(not(target_os = "linux"))]
                child.kill()?;
            }
            child.wait()?;
        }
        Ok(())
    }
    fn retain_owner(&mut self) {
        if let Some(child) = self.child.take() {
            super::management_lifecycle::spawn_detached_child_reaper(
                child,
                "private stock owner".into(),
            );
        }
    }
}

fn verify_stock_process(
    record: &CutexSessionRecord,
    binding: &CutexAppServerRuntimeBinding,
) -> anyhow::Result<()> {
    let contract = record
        .explicit_launch
        .as_ref()
        .context("stock marker missing")?;
    let bundle = StockBundle::load(contract)?;
    ensure!(
        binding.schema_sha256 == STOCK_SCHEMA_SHA256,
        "stock binding schema mismatch"
    );
    #[cfg(target_os = "linux")]
    {
        ensure!(
            std::fs::read_link(format!("/proc/{}/exe", binding.pid))? == bundle.executable.path,
            "stock process executable mismatch"
        );
        let actual = cutex::platform::process::process_started_at(binding.pid)?;
        let expected = chrono::DateTime::parse_from_rfc3339(&binding.started_at)?;
        ensure!(
            actual.timestamp() == expected.timestamp(),
            "stock PID generation changed"
        );
        ensure!(
            unsafe { libc::getpgid(binding.pid as i32) } == binding.pid as i32,
            "stock process group mismatch"
        );
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bundle;
        anyhow::bail!("stock subset requires Linux")
    }
}
fn stop_group(pid: u32) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        ensure!(
            pid > 1 && pid <= i32::MAX as u32 && unsafe { libc::getpgid(pid as i32) } == pid as i32,
            "cannot prove owned stock process group"
        );
        ensure!(
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) } == 0,
            "owned stock group stop failed"
        );
        let result = cutex::platform::process::terminate_process_and_wait(pid, true)?;
        ensure!(result.stopped, "owned stock child stop not proven");
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        anyhow::bail!("stock subset requires Linux")
    }
}

pub(super) fn request(path: &std::path::Path, management_url: &str) -> anyhow::Result<()> {
    let url = url::Url::parse(management_url)?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.path() == "/",
        "explicit local Management URL required"
    );
    let request: cutex::agent_management::ExplicitLaunchRequest =
        serde_json::from_slice(&std::fs::read(path)?)?;
    let config = cutex::config::store::load_codez_config_checked()?;
    let token = cutex::management::service::management_root_credential(&config, None)?;
    let result = cutex::management::remote::management_http_json_with_timeout(
        management_url,
        "POST",
        "/v2/agent-management/explicit-launch",
        Some(token),
        Some(&serde_json::to_vec(&request)?),
        std::time::Duration::from_secs(60),
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub(super) fn attach(id: &str) -> anyhow::Result<()> {
    let store = cutex::session::store::load_cutex_session_store()?;
    let record = store
        .sessions
        .get(id)
        .context("exact stock durable ID required")?;
    ensure!(!record.is_retired(), "archived stock Agent cannot attach");
    let binding = record
        .app_server_runtime
        .as_ref()
        .context("stock runtime is offline")?;
    let contract = record
        .explicit_launch
        .as_ref()
        .context("stock activation missing")?;
    let bundle = StockBundle::load(contract)?;
    verify_stock_process(record, binding)?;
    super::app_server_runtime::verify_exact_live_runtime_claim(record, binding)?;
    ensure!(
        record.app_server_launch_claim_id.is_none(),
        "stock readiness unresolved; replay launch action"
    );
    let ready = store
        .explicit_launch_receipts
        .values()
        .find_map(|r| match r {
            cutex::agent_management::ExplicitLaunchActionReceipt::Runtime(r)
                if r.stage == cutex::agent_management::StockRuntimeStage::Ready
                    && r.binding.as_ref() == Some(binding)
                    && r.expected_generation == record.runtime_generation
                    && record.current_runtime_agent_id.as_deref() == Some(&r.runtime_agent_id) =>
            {
                Some(r)
            }
            _ => None,
        })
        .context("stock ready receipt missing")?;
    // Remote CLI config must describe the running occurrence, not silently
    // substitute local OpenAI defaults or a newly selected durable profile.
    let launch = clean_launch(&bundle.executable.path, &contract.native_home)?.args([
        "resume",
        "--remote",
        &binding.endpoint,
        &contract.native_id,
        "--no-alt-screen",
        "--cd",
        cutex::session::service::cutex_session_launch_cwd(record),
        "-c",
        "tui.resume_cwd=\"current\"",
    ]);
    let launch = configured(launch, &ready.review.configuration)?;
    let status = launch.to_command().status()?;
    ensure!(
        status.success(),
        "stock CLI returned unsuccessfully; owner was not restarted"
    );
    Ok(())
}
