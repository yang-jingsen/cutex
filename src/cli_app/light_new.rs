//! Human new-agent bootstrap: persist an empty native thread, then adopt it.
use anyhow::{ensure, Context};
use cutex::app_server::client::{AppServerClient, AppServerClientOptions, AppServerEndpoint};
use cutex::launch::local_deployment::LocalDeployment;
use cutex::launch::stock::StockBundle;

pub(super) fn create(
    formal_name: &str,
    cwd: &str,
) -> anyhow::Result<cutex::agent_management::HumanAdoptResult> {
    ensure!(
        !formal_name.trim().is_empty()
            && formal_name.trim() == formal_name
            && formal_name.chars().count() <= 128
            && !formal_name.chars().any(char::is_control),
        "valid agent name required"
    );
    let cwd = std::path::Path::new(cwd).canonicalize()?;
    ensure!(cwd.is_dir(), "agent cwd must be a directory");
    let native = create_native(&cwd, &cutex::launch::stock::local_configuration()?, |_| {})?;
    super::management_control_plane::ManagementControlClient::connect()?
        .adopt_saved_native(&cutex::agent_management::HumanAdoptRequest {
            action_id: cutex::agent_management::AgentActionId::new(format!("human-new-{native}"))?,
            native_id: native.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            formal_name: formal_name.into(),
        })
        .with_context(|| format!("Saved native thread {native} exists; retry Adopt for this ID"))
}

/// Persist an empty native thread without a model turn or management recursion.
/// Report its identity immediately so typed callers can journal late failures.
pub(super) fn create_native(
    cwd: &std::path::Path,
    configuration: &cutex::launch::stock::StockConfiguration,
    mut captured: impl FnMut(&str),
) -> anyhow::Result<String> {
    let deployment = LocalDeployment::selected()?.context("No local runtime installed")?;
    let bundle: StockBundle = serde_json::from_slice(&std::fs::read(&deployment.bundle_manifest)?)?;
    StockBundle::load_references(
        bundle
            .shared_config
            .path
            .parent()
            .context("bundle home missing")?,
        &deployment.bundle_manifest,
        &cutex::agent_management::file_sha256(&deployment.bundle_manifest)?,
    )?;
    configuration.validate_auth_home(&deployment.native_home)?;
    let directory = std::env::temp_dir().join(format!(
        "cn-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    #[cfg(not(unix))]
    anyhow::bail!("new local runtime requires Linux");
    let socket = directory.join("native.sock");
    use super::stock_lifecycle::{clean_launch, configured, option, receiver_profile};
    let launch = configured(
        clean_launch(&bundle.executable.path, &deployment.native_home)?,
        &configuration,
        true,
        None,
    )?;
    let launch = option(
        launch,
        "default_permissions",
        receiver_profile(&configuration.sandbox)?,
    )?
    .arg("--listen")
    .arg(format!("unix://{}", socket.display()));
    let mut command = launch.to_command();
    if let Some(projection) = &configuration.selected_projection {
        if let Some(secret) = projection.secret()? {
            secret.apply(&mut command);
        }
    }
    command
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(directory.join("native.stderr.log"))?);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        let parent = std::process::id();
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::other("native creator exited"));
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
    let owned = Owned(command.spawn()?);
    let client =
        AppServerClient::connect(AppServerClientOptions::new(AppServerEndpoint::UnixSocket {
            socket_path: socket,
        }))?;
    let handle = client.handle();
    let response = handle.request("thread/start", serde_json::json!({"cwd":cwd,"approvalPolicy":configuration.approval,"ephemeral":false,"historyMode":"paginated"})).context("Native creation response unconfirmed; inspect Recent before creating another agent")?;
    let native = response["thread"]["id"]
        .as_str()
        .context("native thread ID missing")?
        .to_string();
    ensure!(
        uuid::Uuid::parse_str(&native)?.to_string() == native,
        "invalid native ID"
    );
    captured(&native);
    // Once the native ID is known, always report it on later failure so that a
    // saved thread can be adopted without generating a second identity.
    let operation = || -> anyhow::Result<_> {
        let ack = handle.request(
            "thread/read",
            serde_json::json!({"threadId":native,"includeTurns":true}),
        )?;
        ensure!(
            ack["thread"]["id"].as_str() == Some(&native)
                && ack["thread"]["historyMode"].as_str() == Some("paginated")
                && ack["thread"]["turns"].as_array().is_some_and(Vec::is_empty),
            "empty native thread was not persisted"
        );
        Ok(())
    };
    operation().with_context(|| {
        format!("Native thread {native} created; recover/adopt this ID before trying new again")
    })?;
    drop(client);
    drop(owned);
    Ok(native)
}

/// Foreground form shared by the main selector's New action.
pub(super) fn wizard() -> anyhow::Result<Option<cutex::agent_management::HumanAdoptResult>> {
    let name = super::prompt::prompt_line("New agent name (empty cancels)", "")?;
    if name.is_empty() {
        return Ok(None);
    }
    let cwd = super::prompt::prompt_line(
        "Working directory",
        &std::env::current_dir()?.to_string_lossy(),
    )?;
    let result = create(&name, &cwd)?;
    if let Some(error) = &result.error {
        anyhow::bail!("Agent created, but import incomplete: {error}");
    }
    Ok(Some(result))
}

pub(super) fn adopt_saved(
    native: &str,
    name: &str,
    cwd: &str,
) -> anyhow::Result<cutex::agent_management::HumanAdoptResult> {
    use cutex::catalog::CatalogEndpoint;
    let launch = super::session_native_workflow::NativeLaunch {
        cwd: cwd.into(),
        native_home: LocalDeployment::source_home()?,
        profile: None,
        model: None,
    };
    let source = launch.endpoint()?.request(
        "thread/read",
        serde_json::json!({"threadId":native,"includeTurns":false}),
    )?;
    let cwd = source
        .pointer("/thread/cwd")
        .and_then(serde_json::Value::as_str)
        .context("saved thread cwd missing")?;
    let result = super::management_control_plane::ManagementControlClient::connect()?
        .adopt_saved_native(&cutex::agent_management::HumanAdoptRequest {
            action_id: cutex::agent_management::AgentActionId::new(format!(
                "human-adopt-{native}"
            ))?,
            native_id: native.into(),
            cwd: cwd.into(),
            formal_name: name.into(),
        })?;
    if let Some(error) = &result.error {
        anyhow::bail!("Agent adopted, but roster import incomplete: {error}. Retry the same Adopt");
    }
    Ok(result)
}
