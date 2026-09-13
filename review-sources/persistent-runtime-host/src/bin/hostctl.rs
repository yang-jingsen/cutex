#[cfg(any(target_os = "linux", target_os = "windows"))]
use clap::{Parser, Subcommand};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use persistent_runtime_host::*;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::io::Write;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Parser, Debug)]
#[command(
    name = "hostctl",
    version,
    about = "Headless PRH administration client"
)]
struct Args {
    /// Absolute PRH state directory. This client never creates or starts a host.
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Subcommand, Debug)]
enum Command {
    Info,
    Status,
    List,
    Get {
        service_id: String,
    },
    Start {
        service_id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Stop {
        service_id: String,
        #[arg(long)]
        cascade: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Restart {
        service_id: String,
        #[arg(long)]
        cascade: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Reconcile {
        service_id: Option<String>,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Operation {
        operation_id: String,
    },
    Logs {
        service_id: String,
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value_t = 1_000)]
        limit: u32,
    },
    Events {
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value_t = 1_000)]
        replay_limit: u32,
    },
    Register {
        definition: PathBuf,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Update {
        service_id: String,
        definition: PathBuf,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Remove {
        service_id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    Shutdown {
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-install")]
    WindowsInstall {
        #[arg(long)]
        source_dir: PathBuf,
        #[arg(long)]
        release_id: String,
        #[arg(long)]
        source_revision: Option<String>,
        #[arg(long, default_value = DEFAULT_WINDOWS_INSTALL_ROOT, hide = true)]
        install_root: PathBuf,
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME, hide = true)]
        display_name: String,
        #[arg(long, default_value = DEFAULT_WINDOWS_RUN_VALUE_NAME, hide = true)]
        run_value_name: String,
        #[arg(long, hide = true)]
        operator_sid: Option<String>,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-upgrade")]
    WindowsUpgrade {
        #[arg(long)]
        source_dir: PathBuf,
        #[arg(long)]
        release_id: String,
        #[arg(long)]
        source_revision: Option<String>,
        #[arg(long, default_value = DEFAULT_WINDOWS_INSTALL_ROOT, hide = true)]
        install_root: PathBuf,
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME, hide = true)]
        display_name: String,
        #[arg(long, default_value = DEFAULT_WINDOWS_RUN_VALUE_NAME, hide = true)]
        run_value_name: String,
        #[arg(long, hide = true)]
        operator_sid: Option<String>,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-rollback")]
    WindowsRollback {
        #[arg(long, default_value = DEFAULT_WINDOWS_INSTALL_ROOT, hide = true)]
        install_root: PathBuf,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-uninstall")]
    WindowsUninstall {
        #[arg(long, default_value = DEFAULT_WINDOWS_INSTALL_ROOT, hide = true)]
        install_root: PathBuf,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-service-status")]
    WindowsServiceStatus {
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-host-start")]
    WindowsHostStart {
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-host-stop")]
    WindowsHostStop {
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
    },
    #[cfg(target_os = "windows")]
    #[command(name = "windows-host-restart")]
    WindowsHostRestart {
        #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME, hide = true)]
        service_name: String,
    },
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!("hostctl: {error}");
        std::process::exit(1);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let command = match args.command {
        Command::WindowsInstall {
            source_dir,
            release_id,
            source_revision,
            install_root,
            service_name,
            display_name,
            run_value_name,
            operator_sid,
        }
        | Command::WindowsUpgrade {
            source_dir,
            release_id,
            source_revision,
            install_root,
            service_name,
            display_name,
            run_value_name,
            operator_sid,
        } => {
            let state_dir = args
                .state_dir
                .unwrap_or_else(|| PathBuf::from(DEFAULT_WINDOWS_STATE_DIR));
            let manifest = install_or_upgrade_windows(&WindowsInstallOptions {
                source_dir,
                install_root,
                state_dir,
                service_name,
                display_name,
                run_value_name,
                source_revision: source_revision.unwrap_or_else(|| release_id.clone()),
                release_id,
                operator_sid: operator_sid.unwrap_or(current_user_sid_string()?),
            })?;
            print_json(&manifest)?;
            return Ok(());
        }
        Command::WindowsRollback { install_root } => {
            print_json(&rollback_windows_installation(&install_root)?)?;
            return Ok(());
        }
        Command::WindowsUninstall { install_root } => {
            print_json(&uninstall_windows(&install_root)?)?;
            return Ok(());
        }
        Command::WindowsServiceStatus { service_name } => {
            print_json(&query_windows_service(&service_name)?)?;
            return Ok(());
        }
        Command::WindowsHostStart { service_name } => {
            print_json(&start_windows_service(
                &service_name,
                Duration::from_secs(30),
            )?)?;
            return Ok(());
        }
        Command::WindowsHostStop { service_name } => {
            print_json(&stop_windows_service(
                &service_name,
                Duration::from_secs(180),
            )?)?;
            return Ok(());
        }
        Command::WindowsHostRestart { service_name } => {
            print_json(&restart_windows_service(
                &service_name,
                Duration::from_secs(180),
                Duration::from_secs(30),
            )?)?;
            return Ok(());
        }
        command => command,
    };
    #[cfg(not(target_os = "windows"))]
    let command = args.command;

    let state_dir = match args.state_dir {
        Some(path) => path,
        None => default_state_dir()?,
    };
    let paths = RuntimePaths::new(state_dir)?;
    let client = LocalClient::new(paths.socket);
    let request_id = format!("hostctl-{}", uuid::Uuid::new_v4());
    let request = match command {
        Command::Info => Request::GetHostInfo(GetHostInfoParams {}),
        Command::Status => Request::GetHostStatus(GetHostStatusParams {}),
        Command::List => Request::ListServices(ListServicesParams {}),
        Command::Get { service_id } => Request::GetService(GetServiceParams {
            service_id: ServiceId::new(service_id),
        }),
        Command::Start {
            service_id,
            expected_revision,
            idempotency_key,
        } => Request::EnsureRunning(EnsureRunningParams {
            service_id: ServiceId::new(service_id),
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Stop {
            service_id,
            cascade,
            expected_revision,
            idempotency_key,
        } => Request::EnsureStopped(EnsureStoppedParams {
            service_id: ServiceId::new(service_id),
            cascade,
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Restart {
            service_id,
            cascade,
            expected_revision,
            idempotency_key,
        } => Request::Restart(RestartParams {
            service_id: ServiceId::new(service_id),
            cascade,
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Reconcile {
            service_id,
            expected_revision,
            idempotency_key,
        } => Request::Reconcile(ReconcileParams {
            service_id: service_id.map(ServiceId::new),
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Operation { operation_id } => Request::GetOperation(GetOperationParams {
            operation_id: OperationId::new(operation_id),
        }),
        Command::Logs {
            service_id,
            run_id,
            after,
            limit,
        } => Request::ReadLogs(ReadLogsParams {
            service_id: ServiceId::new(service_id),
            run_id: run_id.map(RunId::new),
            after_sequence: after,
            limit,
        }),
        Command::Events {
            after,
            replay_limit,
        } => {
            let envelope = RequestEnvelope::v1(
                request_id,
                Request::SubscribeEvents(SubscribeEventsParams {
                    after_sequence: after,
                    replay_limit,
                }),
            );
            let (response, mut events) = client.subscribe(&envelope)?;
            print_json(&response)?;
            ensure_success(&response)?;
            while let Some(event) = events.next_event()? {
                print_json(&event)?;
            }
            return Ok(());
        }
        Command::Register {
            definition,
            expected_revision,
            idempotency_key,
        } => Request::RegisterService(RegisterServiceParams {
            definition: read_definition(&definition)?,
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Update {
            service_id,
            definition,
            expected_revision,
            idempotency_key,
        } => Request::UpdateService(UpdateServiceParams {
            service_id: ServiceId::new(service_id),
            definition: read_definition(&definition)?,
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Remove {
            service_id,
            expected_revision,
            idempotency_key,
        } => Request::RemoveService(RemoveServiceParams {
            service_id: ServiceId::new(service_id),
            mutation: mutation(idempotency_key, expected_revision),
        }),
        Command::Shutdown { idempotency_key } => Request::ShutdownHost(ShutdownHostParams {
            mutation: mutation(idempotency_key, None),
        }),
        #[cfg(target_os = "windows")]
        Command::WindowsInstall { .. }
        | Command::WindowsUpgrade { .. }
        | Command::WindowsRollback { .. }
        | Command::WindowsUninstall { .. }
        | Command::WindowsServiceStatus { .. }
        | Command::WindowsHostStart { .. }
        | Command::WindowsHostStop { .. }
        | Command::WindowsHostRestart { .. } => {
            unreachable!("Windows lifecycle commands return before protocol dispatch")
        }
    };
    let response = client.call(&RequestEnvelope::v1(request_id, request))?;
    print_json(&response)?;
    ensure_success(&response)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn mutation(idempotency_key: Option<String>, expected_revision: Option<u64>) -> MutationOptions {
    MutationOptions {
        idempotency_key: idempotency_key
            .unwrap_or_else(|| format!("hostctl-{}", uuid::Uuid::new_v4())),
        expected_revision,
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn read_definition(path: &PathBuf) -> Result<ServiceDefinition, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() > persistent_runtime_host::transport::MAX_FRAME_BYTES {
        return Err("service definition exceeds the local API frame limit".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn print_json(value: &impl serde::Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn ensure_success(response: &ResponseEnvelope) -> Result<(), Box<dyn std::error::Error>> {
    match &response.outcome {
        ResponseOutcome::Ok { .. } => Ok(()),
        ResponseOutcome::Error { error } => Err(error.clone().into()),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("hostctl is available only on Linux and Windows");
    std::process::exit(2);
}
