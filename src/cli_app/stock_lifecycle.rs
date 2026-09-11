//! Opt-in stock child ownership inside the existing Management process.
use anyhow::{ensure, Context};
use cutex::agent_management::{StockRuntimeExecutor, StockRuntimeReceipt};
use cutex::app_server::runtime::AppServerRuntimeLayout;
use cutex::launch::command::LaunchCommand;
use cutex::launch::stock::{ExternalInputBinding, StockBundle};
use cutex::session::model::{
    CutexAppServerRuntimeBinding, CutexSessionRecord, LaunchProfileSource,
};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct StockExecutor {
    child: Option<super::stock_publication::GatedChild>,
    publication: Option<(cutex::agent_management::StockPublication, std::fs::File)>,
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
    #[cfg(feature = "stock-launch-test-hook")]
    if std::env::var("CUTEX_STOCK_TEST_GUARDED_NATIVE").as_deref() == Ok("1") {
        // Explicit S7/S6f same-host-UID fixture only. Default builds have no
        // inherited preload/endpoint authority. This is not OS isolation.
        let private = std::path::PathBuf::from(std::env::var("CUTEX_TEST_PRIVATE_HOME")?);
        ensure!(
            private.is_absolute()
                && private == std::path::PathBuf::from(std::env::var("HOME")?)
                && private.join(".cutex-test-private-home").is_file(),
            "guarded native requires exact private fixture HOME"
        );
        let guard = std::path::Path::new(
            "/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s7a/connect-guard-error.so",
        );
        ensure!(
            guard.canonicalize()? == guard
                && cutex::agent_management::file_sha256(guard)?.as_str()
                    == "8fdf96e00b65c625c6c132f157eea03e081ae6ead7bcfc8dc82ad1865e2d9faa",
            "private approved guard changed"
        );
        let ports = std::env::var("S4_TEST_ALLOWED_PORTS")?;
        ensure!(
            !ports.is_empty()
                && ports
                    .split(',')
                    .all(|p| p.parse::<std::num::NonZeroU16>().is_ok()),
            "private owned ports required"
        );
        launch = launch
            .env("LD_PRELOAD", guard.to_str().unwrap())
            .env("S4_TEST_ALLOWED_PORTS", ports);
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
    owner: bool,
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
    if let Some(projection) = &profile.selected_projection {
        launch = launch.args(projection.native_args(owner)?);
    } else if profile.aemeath_auth.is_some() {
        launch = option(launch, "cli_auth_credentials_store", "file")?;
    } else {
        launch = option(
            launch,
            &format!("model_providers.{}", profile.model_provider),
            &profile.provider,
        )?;
    }
    launch = option(launch, "approval_policy", &profile.approval)?;
    launch = option(launch, "sandbox_mode", &profile.sandbox)?;
    if let Some(reasoning) = &profile.reasoning {
        launch = option(launch, "model_reasoning_effort", reasoning)?;
    }
    option(launch, "analytics.enabled", false)
}

fn receiver_profile(sandbox: &str) -> anyhow::Result<&'static str> {
    match sandbox {
        "read-only" => Ok(":read-only"),
        "workspace-write" => Ok(":workspace"),
        "danger-full-access" => Ok(":danger-full-access"),
        _ => anyhow::bail!("unknown receiver permission profile"),
    }
}

/// A neutral, model-free owner. Never retries thread/start after an uncertain
/// response; the provider journals the known ID before adoption and online.
#[cfg(unix)]
pub(super) fn bootstrap_native(
    permit: &cutex::agent_management::BootstrapExecutionPermit<'_>,
    existing: Option<&str>,
) -> Result<String, cutex::agent_management::LifecycleFailure> {
    use cutex::agent_management::LifecycleFailure;
    use cutex::app_server::client::{AppServerClient, AppServerClientOptions, AppServerEndpoint};
    let review = permit.review();
    let mut known = existing.map(str::to_owned);
    let mut spawned = false;
    let mut operation = || -> anyhow::Result<String> {
        ensure!(
            cfg!(target_os = "linux"),
            "private bootstrap requires Linux"
        );
        ensure!(
            cutex::config::paths::host_codex_home_dir()?.canonicalize()? == review.native_home,
            "bootstrap home changed before spawn"
        );
        review
            .configuration
            .validate_auth_home(&review.native_home)?;
        let bundle = StockBundle::load_references(
            &review.native_home,
            &review.bundle_manifest,
            &review.bundle_sha256,
        )?;
        ensure!(
            bundle.version == 3 && bundle.soon_ingress(),
            "bootstrap bundle mismatch"
        );
        let spec = review
            .request
            .operation
            .bootstrap_spec()
            .context("bootstrap operation required")?;
        ensure!(
            cutex::launch::stock::bootstrap_configuration(spec)? == review.configuration,
            "bootstrap config changed before spawn"
        );
        let directory = std::path::PathBuf::from(std::env::var("TMPDIR")?).join(format!(
            "cb-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        let socket = directory.join("native.sock");
        ensure!(
            socket.as_os_str().len() < 104,
            "private bootstrap socket path too long"
        );
        let launch = configured(
            clean_launch(&bundle.executable.path, &review.native_home)?,
            &review.configuration,
            true,
        )?;
        let launch = option(
            launch,
            "default_permissions",
            receiver_profile(&review.configuration.sandbox)?,
        )?
        .arg("--listen")
        .arg(format!("unix://{}", socket.display()));
        let mut command = launch.to_command();
        if let Some(projection) = &review.configuration.selected_projection {
            if let Some(secret) = projection.secret()? {
                secret.apply(&mut command);
            }
        }
        let mut log_options = std::fs::OpenOptions::new();
        log_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            log_options.mode(0o600);
        }
        let log = log_options.open(directory.join("native.stderr.log"))?;
        command
            .current_dir(&spec.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let parent = std::process::id();
            // Owned child only. Creator death before an ID is captured stays
            // uncertain in the journal, and cannot leave a reusable writer.
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() as u32 != parent {
                        return Err(std::io::Error::other("bootstrap creator exited"));
                    }
                    Ok(())
                });
            }
        }
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let _owned = Owned(command.spawn()?);
        spawned = true;
        let client =
            AppServerClient::connect(AppServerClientOptions::new(AppServerEndpoint::UnixSocket {
                socket_path: socket,
            }))?;
        let handle = client.handle();
        let response = if let Some(native) = existing {
            handle.request("thread/resume", serde_json::json!({"threadId":native,"cwd":spec.cwd,"approvalPolicy":review.configuration.approval}))?
        } else {
            handle.request("thread/start", serde_json::json!({"cwd":spec.cwd,"approvalPolicy":review.configuration.approval,"ephemeral":false,"historyMode":"paginated"}))?
        };
        let native = response["thread"]["id"]
            .as_str()
            .context("bootstrap response missing native ID")?
            .to_owned();
        ensure!(
            uuid::Uuid::parse_str(&native)?.to_string() == native,
            "bootstrap malformed native ID"
        );
        ensure!(
            existing.is_none_or(|expected| expected == native),
            "bootstrap resumed wrong native ID"
        );
        #[cfg(feature = "stock-launch-test-hook")]
        if existing.is_none()
            && cutex::agent_management::bootstrap_test_fault(
                "CUTEX_BOOTSTRAP_TEST_PRE_ID_ACTION",
                &review.request.action_id,
            )
        {
            // The request may have created history, but its ID is not in the
            // Cutex journal. Abort the owned creator; no retry is authorized.
            std::process::exit(86);
        }
        known = Some(native.clone());
        // Accepted aa382cdb5 foundation: paginated read(true) awaits persist()
        // and propagates write failures. Metadata-only read is not this ACK.
        let ack = handle.request(
            "thread/read",
            serde_json::json!({"threadId":native,"includeTurns":true}),
        )?;
        ensure!(
            ack["thread"]["id"].as_str() == Some(&native)
                && ack["thread"]["historyMode"].as_str() == Some("paginated")
                && ack["thread"]["turns"].as_array().is_some_and(Vec::is_empty),
            "neutral bootstrap persistence ACK mismatch"
        );
        Ok(native)
    };
    operation().map_err(|error| LifecycleFailure {
        code: "reviewed_native_bootstrap_failed".into(),
        detail: error.to_string(),
        outcome_unknown: spawned || known.is_some(),
        known_native_session_id: known,
    })
}

#[cfg(not(unix))]
pub(super) fn bootstrap_native(
    _permit: &cutex::agent_management::BootstrapExecutionPermit<'_>,
    _existing: Option<&str>,
) -> Result<String, cutex::agent_management::LifecycleFailure> {
    Err(cutex::agent_management::LifecycleFailure::definite(
        "unsupported_bootstrap_platform",
        "private bootstrap requires Linux",
    ))
}

impl StockRuntimeExecutor for StockExecutor {
    fn published_owner_absent(&mut self, receipt: &StockRuntimeReceipt) -> anyhow::Result<bool> {
        let binding = receipt
            .binding
            .as_ref()
            .context("published binding missing")?;
        #[cfg(target_os = "linux")]
        {
            let stat = std::fs::read_to_string(format!("/proc/{}/stat", binding.pid));
            match stat {
                Ok(stat) => {
                    let state = stat
                        .rsplit(')')
                        .next()
                        .context("invalid owned process stat")?
                        .split_whitespace()
                        .next();
                    if state != Some("Z") {
                        let actual = cutex::platform::process::process_started_at(binding.pid)?;
                        let original = chrono::DateTime::parse_from_rfc3339(&binding.started_at)?;
                        if actual.timestamp() == original.timestamp() {
                            return Ok(false);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            // Never signal a reused PID. Also refuse a live endpoint holder.
            match std::os::unix::net::UnixStream::connect(
                binding
                    .endpoint
                    .strip_prefix("unix://")
                    .context("invalid stock endpoint")?,
            ) {
                Ok(_) => anyhow::bail!("published endpoint remains live; ownership ambiguous"),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(error) => return Err(error.into()),
            }
            ensure!(
                receipt.publication.is_some(),
                "publication_missing: cannot recover legacy binding"
            );
            self.publication(receipt)?;
            Ok(true)
        }
        #[cfg(not(target_os = "linux"))]
        anyhow::bail!("stock publication requires Linux")
    }
    fn publication(
        &mut self,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<cutex::agent_management::StockPublication> {
        if let Some((publication, _)) = &self.publication {
            ensure!(
                receipt
                    .publication
                    .as_ref()
                    .is_none_or(|p| p == publication),
                "publication changed"
            );
            return Ok(publication.clone());
        }
        let publication =
            super::stock_publication::lease(&receipt.claim_id, receipt.publication.as_ref())?;
        let result = publication.0.clone();
        self.publication = Some(publication);
        Ok(result)
    }
    fn stop(&mut self, record: &CutexSessionRecord) -> anyhow::Result<()> {
        let Some(binding) = &record.app_server_runtime else {
            ensure!(
                !cutex::session::archive::record_has_runtime_claim(record),
                "stock owner unavailable; cannot prove stop"
            );
            return Ok(());
        };
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.verify_claim.begin");
        super::app_server_runtime::verify_exact_live_runtime_claim(record, binding)?;
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.verify_process.begin");
        verify_stock_process(record, binding)?;
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.interrupt.begin");
        super::app_server_runtime::runtime_manager()
            .interrupt_active_turn(&record.cutex_session_id)?;
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.disconnect.begin");
        super::app_server_runtime::disconnect_runtime(&record.cutex_session_id)?;
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.group.begin");
        stop_group(binding.pid)?;
        #[cfg(feature = "stock-launch-test-hook")]
        cutex::app_server::private_restart_phase("stop.commit.begin");
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
            true,
        )?;
        if bundle.soon_ingress() {
            launch = option(
                launch,
                "default_permissions",
                receiver_profile(&profile.sandbox)?,
            )?;
        }
        #[allow(unused_mut)]
        let mut mcp_env = vec![
            "CUTEX_AGENT_ID",
            "CUTEX_RUNTIME_GENERATION",
            "CUTEX_AGENT_BUS_URL",
            "CUTEX_AGENT_BUS_TOKEN",
        ];
        #[cfg(feature = "stock-launch-test-hook")]
        if std::env::var("CUTEX_STOCK_TEST_GUARDED_NATIVE").as_deref() == Ok("1") {
            mcp_env.extend(["LD_PRELOAD", "S4_TEST_ALLOWED_PORTS"]);
        }
        launch = option(
            launch,
            "mcp_servers.cutex",
            serde_json::json!({"command":bundle.facade.path,"env_vars":mcp_env,"default_tools_approval_mode":"approve"}),
        )?;
        launch = option(
            launch,
            "code_mode.direct_only_tool_namespaces",
            vec!["mcp__cutex"],
        )?;
        if let Some(job) = &receipt.review.job_mcp {
            job.validate(bundle)?;
            // Business MCP tools retain ordinary native approval. Never add
            // this namespace to the privileged control direct-only set.
            launch = option(launch, "mcp_servers.cutex_job", job.config()?)?;
        }
        let mut args = layout.app_server_args();
        if bundle.common_ingress() {
            // The accepted artifact is the direct app-server, not the stock CLI.
            args.remove(0);
            let directory = std::path::PathBuf::from(layout.binding(0, String::new()).runtime_dir);
            let path = ExternalInputBinding::from_review(receipt).stage(&directory)?;
            args.extend([
                "--external-input-binding-file".into(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        launch = launch
            .args(args)
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
        // One gated setsid child, without contacting systemd. It cannot exec
        // native stock until its binding has committed. This is not a cgroup.
        let secret = profile
            .selected_projection
            .as_ref()
            .map(|p| p.secret())
            .transpose()?
            .flatten();
        self.child = Some(super::stock_publication::spawn_with_secret(
            &launch,
            cutex::session::service::cutex_session_launch_cwd(record),
            &log,
            &self
                .publication
                .as_ref()
                .context("publication lease missing")?
                .1,
            secret.as_ref(),
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
        binding.schema_version = if bundle.soon_ingress() {
            "U-0.153.4+provider-item-id-ca580a78-soon-v1"
        } else if bundle.common_ingress() {
            "U-0.153.4+S6-c2aaceb4-external-input-v1"
        } else {
            "stock-0.153.4-app-server-v2"
        }
        .into();
        binding.schema_sha256 = bundle.schema.sha256.as_str().to_string();
        Ok(binding)
    }
    fn connect(
        &mut self,
        record: &CutexSessionRecord,
        receipt: &StockRuntimeReceipt,
    ) -> anyhow::Result<()> {
        let binding = receipt.binding.as_ref().context("stock binding missing")?;
        let bundle = StockBundle::load(&receipt.review.contract)?;
        if bundle.common_ingress() {
            ExternalInputBinding::from_review(receipt)
                .verify(std::path::Path::new(&binding.runtime_dir))?;
        }
        if let Some(child) = &mut self.child {
            child.release()?;
            self.publication.take();
        }
        // Native exec is permitted only after the binding/receipt commit. The
        // existing manager connect below validates the actual native endpoint.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if verify_stock_process(record, binding).is_ok()
                && std::path::Path::new(binding.endpoint.trim_start_matches("unix://")).exists()
            {
                break;
            }
            if let Some(child) = &mut self.child {
                ensure!(
                    !child.try_wait()?,
                    "published stock child exited before readiness"
                );
            }
            ensure!(
                std::time::Instant::now() < until,
                "published stock owner not ready; retry exact action, no fallback"
            );
            std::thread::park_timeout(std::time::Duration::from_millis(10));
        }
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
                    // The coherent CLI requires the receiver's named profile.
                    // A legacy per-resume sandbox override erases that identity
                    // even when its read-only projection appears equivalent.
                    sandbox: (!bundle.soon_ingress()).then(|| cfg.sandbox.clone()),
                    ..Default::default()
                },
                receipt.expected_generation,
                "host",
            )?;
        }
        if bundle.soon_ingress() {
            let status = manager
                .status(&record.cutex_session_id)?
                .context("receiver settings unavailable")?;
            let settings = status.thread_settings.context("receiver settings absent")?;
            ensure!(settings.pointer("/activePermissionProfile/id").and_then(serde_json::Value::as_str)
                == Some(receiver_profile(&receipt.review.configuration.sandbox)?),
                "existing thread active permission profile mismatches reviewed receiver; explicit controller correction required");
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
            options.registration_only = !bundle.common_ingress();
            if !options.registration_only {
                options.external_input_generation = Some(receipt.expected_generation);
            }
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
            child.cleanup()?;
        }
        self.publication.take();
        Ok(())
    }
    fn retain_owner(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = std::thread::Builder::new()
                .name("stock-child-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
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
        binding.schema_sha256 == bundle.schema.sha256.as_str(),
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
    ensure!(!bundle.common_ingress() || bundle.soon_ingress(),
        "U+S6 bundle contains only app-server; this slice has no pinned compatible CLI attach artifact");
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
    let cli = bundle.cli.as_ref().unwrap_or(&bundle.executable);
    let launch = clean_launch(&cli.path, &contract.native_home)?.args([
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
    let mut launch = configured(launch, &ready.review.configuration, false)?;
    if bundle.soon_ingress() {
        launch = option(
            launch,
            "default_permissions",
            receiver_profile(&ready.review.configuration.sandbox)?,
        )?;
    }
    let status = launch.to_command().status()?;
    ensure!(
        status.success(),
        "stock CLI returned unsuccessfully; owner was not restarted"
    );
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod ingress_guard_tests {
    #[test]
    fn coherent_receiver_profiles_are_explicit_and_unknown_is_not_full_access() {
        for (sandbox, expected) in [
            ("read-only", ":read-only"),
            ("workspace-write", ":workspace"),
            ("danger-full-access", ":danger-full-access"),
        ] {
            assert_eq!(super::receiver_profile(sandbox).unwrap(), expected);
        }
        for unknown in ["", "full-access", "managed", "inherit", "unknown"] {
            assert!(super::receiver_profile(unknown).is_err());
        }
    }
    #[test]
    fn marked_generic_restart_stop_fence_preserves_real_owned_child() {
        use cutex::session::model::{CutexSessionRecord, CutexSessionStore};
        use std::io::BufRead;
        let home = crate::cli_app::test_home::IsolatedTestHome::new("s6-stop").unwrap();
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Owned(
            std::process::Command::new("/usr/bin/python3")
                .args([
                    "-c",
                    "import signal; print('READY', flush=True); signal.pause()",
                ])
                .env_clear()
                .env("HOME", home.root())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut ready = String::new();
        std::io::BufReader::new(child.0.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "READY");
        let native = uuid::Uuid::new_v4().to_string();
        let id = format!("cutex.{native}");
        let mut record = CutexSessionRecord::new(
            id.clone(),
            Some(native.clone()),
            "private".into(),
            home.root().display().to_string(),
            None,
        )
        .unwrap();
        record.runtime_pid = Some(child.0.id());
        record.runtime_generation = 7;
        // Missing referenced evidence is deliberately NOT permission to fall back.
        record.explicit_launch = Some(cutex::agent_management::ExplicitLaunchContract {
            version: 1,
            native_id: native,
            native_home: home.root().into(),
            bundle_manifest: home.root().join("missing-bundle.json"),
            bundle_sha256: cutex::role_revision::Sha256::new("a".repeat(64)).unwrap(),
        });
        let mut store = CutexSessionStore::default();
        store.sessions.insert(id.clone(), record.clone());
        cutex::session::store::save_cutex_session_store(&store).unwrap();
        let entry = serde_json::from_value(serde_json::json!({"session_id":id,"display_name":"Formal private Agent",
            "host_id":"private","cwd":home.root(),"profile":null,"groups":[],"registration_class":"persistent",
            "visible":true,"created_at":"private","updated_at":"private"})).unwrap();
        let error =
            crate::cli_app::management_lifecycle::stop_cutex_session_runtime_for_entry_fenced(
                &entry,
                &[],
                false,
                Some((7, true)),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("explicit_stock_launch_required"),
            "{error:#}"
        );
        assert!(child.0.try_wait().unwrap().is_none());
        assert_eq!(
            cutex::session::store::load_cutex_session_store()
                .unwrap()
                .sessions[&id],
            record
        );
    }
}
