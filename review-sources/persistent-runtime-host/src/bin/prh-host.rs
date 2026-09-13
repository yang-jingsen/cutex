#[cfg(any(target_os = "linux", target_os = "windows"))]
use clap::Parser;
#[cfg(target_os = "linux")]
use persistent_runtime_host::linux_process::{LinuxProcessBackend, LinuxProcessConfig};
#[cfg(target_os = "windows")]
use persistent_runtime_host::windows_process::{WindowsProcessBackend, WindowsProcessConfig};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use persistent_runtime_host::*;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::Arc;
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Clone, Parser, Debug)]
#[command(name = "prh-host", version, about = "Persistent Runtime Host")]
struct Args {
    /// Absolute directory identifying the local endpoint, registry, lock, and logs.
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Maximum bytes in each per-service JSONL log generation.
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    log_max_bytes: u64,

    /// Total log generations retained per service, including the active file.
    #[arg(long, default_value_t = 4)]
    log_files: usize,

    /// In-memory log entries retained per service for live subscribers.
    #[arg(long, default_value_t = 4096)]
    memory_log_entries: usize,

    /// In-memory log bytes retained per service.
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    memory_log_bytes: usize,

    /// Run under the Windows Service Control Manager. Foreground remains the default.
    #[cfg(target_os = "windows")]
    #[arg(long)]
    service: bool,

    /// Exact SCM service name used to register the control handler.
    #[cfg(target_os = "windows")]
    #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME)]
    service_name: String,

    /// Install-time operator SID granted access to service state and the local pipe.
    #[cfg(target_os = "windows")]
    #[arg(long, requires = "service")]
    operator_sid: Option<String>,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    let args = Args::parse();
    #[cfg(target_os = "windows")]
    let result = if args.service {
        run_windows_service(args)
    } else {
        run_foreground(args)
    };
    #[cfg(not(target_os = "windows"))]
    let result = run_foreground(args);
    if let Err(error) = result {
        eprintln!("prh-host: {error}");
        std::process::exit(1);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_foreground(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let terminate = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    {
        signal_hook::flag::register(signal_hook::consts::SIGTERM, terminate.clone())?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, terminate.clone())?;
    }
    #[cfg(target_os = "windows")]
    let _console_handler = WindowsConsoleHandler::install(&terminate)?;
    run_host(args, terminate, true, || {})
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_host(
    args: Args,
    terminate: Arc<AtomicBool>,
    foreground_diagnostics: bool,
    ready: impl FnOnce(),
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    if let Some(operator_sid) = &args.operator_sid {
        configure_operator_sid(operator_sid)?;
    }
    let state_dir = match args.state_dir {
        Some(path) => path,
        None => default_state_dir()?,
    };
    let paths = RuntimePaths::new(state_dir)?;
    let _instance = SingleInstanceGuard::acquire(&paths)?;
    let file_logs = Arc::new(RotatingFileLogs::open(
        &paths.logs,
        FileLogConfig {
            max_bytes_per_file: args.log_max_bytes,
            max_files_per_service: args.log_files,
        },
    )?);
    let registry = Arc::new(FileRegistry::open(&paths.registry)?);
    #[cfg(target_os = "linux")]
    let sentinel = {
        let current_executable = std::env::current_exe()?;
        let executable_directory = current_executable
            .parent()
            .ok_or("host executable has no parent")?;
        let sentinel = executable_directory.join("prh-linux-sentinel");
        if !sentinel.is_file() {
            return Err(format!(
                "required containment companion is missing: {}",
                sentinel.display()
            )
            .into());
        }
        sentinel
    };
    #[cfg(target_os = "linux")]
    let (process_backend, backend_events) =
        LinuxProcessBackend::new(LinuxProcessConfig::new(sentinel))?;
    #[cfg(target_os = "windows")]
    let (process_backend, backend_events) =
        WindowsProcessBackend::new(WindowsProcessConfig::default())?;
    let host = Arc::new(HostController::with_process_backend(
        FakeHostConfig {
            host_instance_id: uuid::Uuid::new_v4().to_string(),
            log_buffer: LogBufferConfig {
                max_entries_per_service: args.memory_log_entries,
                max_bytes_per_service: args.memory_log_bytes,
            },
            event_history_capacity: 4_096,
            event_subscriber_capacity: 256,
        },
        registry,
        Arc::new(process_backend),
    )?);
    seed_file_log_sequences(&host, &file_logs)?;
    let event_pump = spawn_backend_event_pump(host.clone(), backend_events, file_logs.clone());
    if foreground_diagnostics {
        eprintln!("prh-host: listening on {}", paths.socket.display());
    }
    ready();
    let server_result = run_local_server(&paths.socket, host.clone(), file_logs, terminate);
    if host.host_phase() == HostPhase::Running {
        let _ = host.handle(RequestEnvelope::v1(
            "host-server-exit",
            Request::ShutdownHost(ShutdownHostParams {
                mutation: MutationOptions::new("host-server-exit"),
            }),
        ));
    }
    let _ = event_pump.join();
    server_result?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("prh-host is available only on Linux and Windows");
    std::process::exit(2);
}

#[cfg(target_os = "windows")]
static WINDOWS_TERMINATE: OnceLock<Arc<AtomicBool>> = OnceLock::new();

#[cfg(target_os = "windows")]
static WINDOWS_SERVICE_ARGS: OnceLock<std::sync::Mutex<Option<Args>>> = OnceLock::new();

#[cfg(target_os = "windows")]
static WINDOWS_SERVICE_STATUS_HANDLE: AtomicPtr<std::ffi::c_void> =
    AtomicPtr::new(std::ptr::null_mut());

#[cfg(target_os = "windows")]
static WINDOWS_SERVICE_STATE: AtomicU32 = AtomicU32::new(0);

#[cfg(target_os = "windows")]
static WINDOWS_SERVICE_CHECKPOINT: AtomicU32 = AtomicU32::new(1);

#[cfg(target_os = "windows")]
fn run_windows_service(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    use windows_sys::Win32::System::Services::{StartServiceCtrlDispatcherW, SERVICE_TABLE_ENTRYW};

    if args.state_dir.is_none() {
        return Err("Windows service mode requires --state-dir".into());
    }
    if args.operator_sid.is_none() {
        return Err("Windows service mode requires --operator-sid".into());
    }
    persistent_runtime_host::windows_service::validate_service_name(&args.service_name)?;
    WINDOWS_SERVICE_ARGS
        .set(std::sync::Mutex::new(Some(args.clone())))
        .map_err(|_| "Windows service arguments were already initialized")?;
    let mut service_name = args
        .service_name
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_mut_ptr(),
            lpServiceProc: Some(windows_service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_service_main(
    _argument_count: u32,
    _arguments: *mut windows_sys::core::PWSTR,
) {
    use windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR;
    use windows_sys::Win32::System::Services::RegisterServiceCtrlHandlerExW;

    let args = WINDOWS_SERVICE_ARGS
        .get()
        .and_then(|args| args.lock().ok().and_then(|mut args| args.take()));
    let Some(args) = args else {
        return;
    };
    let service_name = args
        .service_name
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(
            service_name.as_ptr(),
            Some(windows_service_control),
            std::ptr::null(),
        )
    };
    if handle.is_null() {
        return;
    }
    WINDOWS_SERVICE_STATUS_HANDLE.store(handle, Ordering::SeqCst);
    let terminate = Arc::new(AtomicBool::new(false));
    let _ = WINDOWS_TERMINATE.set(terminate.clone());
    let _ = report_windows_service_status(
        windows_sys::Win32::System::Services::SERVICE_START_PENDING,
        0,
        30_000,
        0,
        0,
    );
    let status_updates_done = Arc::new(AtomicBool::new(false));
    let status_thread_done = status_updates_done.clone();
    let status_thread_terminate = terminate.clone();
    let status_thread = std::thread::spawn(move || {
        use windows_sys::Win32::System::Services::SERVICE_STOP_PENDING;
        while !status_thread_done.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if status_thread_terminate.load(Ordering::SeqCst)
                && !status_thread_done.load(Ordering::SeqCst)
            {
                let _ = report_windows_service_status(SERVICE_STOP_PENDING, 0, 180_000, 0, 0);
            }
        }
    });
    let result = run_host(args, terminate, false, || {
        let _ = report_windows_service_status(
            windows_sys::Win32::System::Services::SERVICE_RUNNING,
            windows_sys::Win32::System::Services::SERVICE_ACCEPT_STOP
                | windows_sys::Win32::System::Services::SERVICE_ACCEPT_SHUTDOWN
                | windows_sys::Win32::System::Services::SERVICE_ACCEPT_PRESHUTDOWN,
            0,
            0,
            0,
        );
    });
    status_updates_done.store(true, Ordering::SeqCst);
    let _ = status_thread.join();
    let (exit_code, service_exit_code) = if result.is_ok() {
        (0, 0)
    } else {
        (ERROR_SERVICE_SPECIFIC_ERROR, 1)
    };
    if let Err(error) = result {
        eprintln!("prh-host service: {error}");
    }
    let _ = report_windows_service_status(
        windows_sys::Win32::System::Services::SERVICE_STOPPED,
        0,
        0,
        exit_code,
        service_exit_code,
    );
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_service_control(
    control: u32,
    _event_type: u32,
    _event_data: *mut std::ffi::c_void,
    _context: *mut std::ffi::c_void,
) -> u32 {
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::System::Services::{
        SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_PRESHUTDOWN, SERVICE_CONTROL_SHUTDOWN,
        SERVICE_CONTROL_STOP, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    };
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN | SERVICE_CONTROL_PRESHUTDOWN => {
            if let Some(flag) = WINDOWS_TERMINATE.get() {
                flag.store(true, Ordering::SeqCst);
            }
            if WINDOWS_SERVICE_STATE.load(Ordering::SeqCst) != SERVICE_STOPPED {
                let _ = report_windows_service_status(SERVICE_STOP_PENDING, 0, 180_000, 0, 0);
            }
        }
        SERVICE_CONTROL_INTERROGATE => {
            let state = WINDOWS_SERVICE_STATE.load(Ordering::SeqCst);
            if state != 0 {
                let _ = report_windows_service_status(state, controls_for_state(state), 0, 0, 0);
            }
        }
        _ => {}
    }
    NO_ERROR
}

#[cfg(target_os = "windows")]
fn controls_for_state(state: u32) -> u32 {
    use windows_sys::Win32::System::Services::{
        SERVICE_ACCEPT_PRESHUTDOWN, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP, SERVICE_RUNNING,
    };
    if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_PRESHUTDOWN
    } else {
        0
    }
}

#[cfg(target_os = "windows")]
fn report_windows_service_status(
    state: u32,
    controls: u32,
    wait_hint: u32,
    win32_exit_code: u32,
    service_exit_code: u32,
) -> std::io::Result<()> {
    use windows_sys::Win32::System::Services::{
        SetServiceStatus, SERVICE_RUNNING, SERVICE_STATUS, SERVICE_STOPPED,
        SERVICE_WIN32_OWN_PROCESS,
    };
    let handle = WINDOWS_SERVICE_STATUS_HANDLE.load(Ordering::SeqCst);
    if handle.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Windows service status handle is unavailable",
        ));
    }
    let checkpoint = if matches!(state, SERVICE_RUNNING | SERVICE_STOPPED) {
        0
    } else {
        WINDOWS_SERVICE_CHECKPOINT.fetch_add(1, Ordering::SeqCst)
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls,
        dwWin32ExitCode: win32_exit_code,
        dwServiceSpecificExitCode: service_exit_code,
        dwCheckPoint: checkpoint,
        dwWaitHint: wait_hint,
    };
    if unsafe { SetServiceStatus(handle, &status) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        WINDOWS_SERVICE_STATE.store(state, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(target_os = "windows")]
struct WindowsConsoleHandler;

#[cfg(target_os = "windows")]
impl WindowsConsoleHandler {
    fn install(flag: &Arc<AtomicBool>) -> std::io::Result<Self> {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        WINDOWS_TERMINATE.set(flag.clone()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Windows console handler was already initialized",
            )
        })?;
        if unsafe { SetConsoleCtrlHandler(Some(windows_console_event), 1) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsConsoleHandler {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        unsafe {
            let _ = SetConsoleCtrlHandler(Some(windows_console_event), 0);
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn windows_console_event(event: u32) -> i32 {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    if matches!(
        event,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        if let Some(flag) = WINDOWS_TERMINATE.get() {
            flag.store(true, Ordering::Relaxed);
        }
        1
    } else {
        0
    }
}
