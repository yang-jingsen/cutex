#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
use clap::Parser;
#[cfg(target_os = "windows")]
use persistent_runtime_host::*;
#[cfg(target_os = "windows")]
use std::collections::BTreeMap;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::ptr::{null, null_mut};
#[cfg(target_os = "windows")]
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(target_os = "windows")]
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "windows")]
use std::time::Duration;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFYICONDATAW,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, KillTimer, LoadIconW, MessageBoxW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    TrackPopupMenu, TranslateMessage, HMENU, IDI_APPLICATION, IDYES, MB_DEFBUTTON2, MB_ICONERROR,
    MB_ICONWARNING, MB_OK, MB_YESNO, MF_DISABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MSG, SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU,
    WM_DESTROY, WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

#[cfg(target_os = "windows")]
const TRAY_CALLBACK: u32 = WM_APP + 1;
#[cfg(target_os = "windows")]
const WORKER_COMPLETE: u32 = WM_APP + 2;
#[cfg(target_os = "windows")]
const TRAY_ICON_ID: u32 = 1;
#[cfg(target_os = "windows")]
const REFRESH_TIMER_ID: usize = 1;
#[cfg(target_os = "windows")]
const COMMAND_REFRESH: u32 = 100;
#[cfg(target_os = "windows")]
const COMMAND_CLEAR_ALERTS: u32 = 101;
#[cfg(target_os = "windows")]
const COMMAND_EXIT: u32 = 102;
#[cfg(target_os = "windows")]
const COMMAND_HOST_START: u32 = 103;
#[cfg(target_os = "windows")]
const COMMAND_HOST_STOP: u32 = 104;
#[cfg(target_os = "windows")]
const COMMAND_HOST_RESTART: u32 = 105;
#[cfg(target_os = "windows")]
const DYNAMIC_COMMAND_START: u32 = 1_000;

#[cfg(target_os = "windows")]
#[derive(Parser, Debug)]
#[command(name = "prh-tray", version, about = "Native PRH Windows tray client")]
struct Args {
    /// Absolute PRH state directory. The tray never starts a missing host.
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Exact installed SCM service this tray may query or explicitly control.
    #[arg(long, default_value = DEFAULT_WINDOWS_SERVICE_NAME)]
    service_name: String,
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
enum TrayAction {
    Start(String),
    Stop(String),
    Restart(String),
    OpenLogs(String),
}

#[cfg(target_os = "windows")]
enum WorkerResult {
    Refresh(RefreshResult),
    Mutation(Result<(), String>),
    HostControl(Result<(), String>),
    Logs(Result<PathBuf, String>),
}

#[cfg(target_os = "windows")]
struct RefreshResult {
    service: Result<Option<WindowsServiceStatus>, String>,
    host: Result<HostStatus, String>,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
enum HostControl {
    Start,
    Stop,
    Restart,
}

#[cfg(target_os = "windows")]
struct TrayState {
    hwnd: usize,
    endpoint: PathBuf,
    state_dir: PathBuf,
    service_name: String,
    service_status: Option<WindowsServiceStatus>,
    service_query_error: Option<String>,
    reachability_error: Option<String>,
    projection: Option<TrayProjection>,
    dismissed_alert: Option<String>,
    last_error: Option<String>,
    actions: BTreeMap<u32, TrayAction>,
    sender: Sender<WorkerResult>,
    receiver: Receiver<WorkerResult>,
    refresh_in_flight: bool,
    taskbar_created: u32,
}

#[cfg(target_os = "windows")]
static TRAY_STATE: OnceLock<Mutex<TrayState>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn main() {
    if let Err(error) = run(Args::parse()) {
        show_error_dialog(&format!("PRH tray could not start:\n\n{error}"));
        std::process::exit(1);
    }
}

#[cfg(target_os = "windows")]
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = args.state_dir.map_or_else(default_state_dir, Ok)?;
    let paths = RuntimePaths::new(state_dir.clone())?;
    let (sender, receiver) = mpsc::channel();
    let taskbar_message = wide("TaskbarCreated");
    let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_message.as_ptr()) };
    if taskbar_created == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    TRAY_STATE
        .set(Mutex::new(TrayState {
            hwnd: 0,
            endpoint: paths.socket,
            state_dir,
            service_name: args.service_name,
            service_status: None,
            service_query_error: None,
            reachability_error: None,
            projection: None,
            dismissed_alert: None,
            last_error: None,
            actions: BTreeMap::new(),
            sender,
            receiver,
            refresh_in_flight: false,
            taskbar_created,
        }))
        .map_err(|_| "tray state was already initialized")?;

    let class_name = wide("PersistentRuntimeHostTrayWindow");
    let window_name = wide("Persistent Runtime Host");
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let icon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    let window_class = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hIcon: icon,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&window_class) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        )
    };
    if hwnd.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    state().lock().expect("tray state lock poisoned").hwnd = hwnd as usize;
    add_tray_icon(hwnd)?;
    if unsafe { SetTimer(hwnd, REFRESH_TIMER_ID, 5_000, None) } == 0 {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    request_refresh(hwnd);

    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        if result == 0 {
            break;
        }
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    let taskbar_created = TRAY_STATE
        .get()
        .and_then(|state| state.lock().ok().map(|state| state.taskbar_created));
    if taskbar_created == Some(message) {
        let _ = add_tray_icon(hwnd);
        update_tray_tip(hwnd);
        return 0;
    }
    match message {
        TRAY_CALLBACK => {
            let event = lparam as u32;
            if matches!(event, WM_RBUTTONUP | WM_CONTEXTMENU | WM_LBUTTONDBLCLK) {
                show_menu(hwnd);
            }
            0
        }
        WORKER_COMPLETE => {
            drain_worker_results(hwnd);
            0
        }
        WM_TIMER if wparam == REFRESH_TIMER_ID => {
            request_refresh(hwnd);
            0
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(hwnd, REFRESH_TIMER_ID);
            }
            remove_tray_icon(hwnd);
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(target_os = "windows")]
fn request_refresh(hwnd: HWND) {
    let (endpoint, service_name, sender) = {
        let mut state = state().lock().expect("tray state lock poisoned");
        if state.refresh_in_flight {
            return;
        }
        state.refresh_in_flight = true;
        (
            state.endpoint.clone(),
            state.service_name.clone(),
            state.sender.clone(),
        )
    };
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let result = RefreshResult {
            service: query_windows_service(&service_name).map_err(|error| error.to_string()),
            host: read_host_status(&endpoint),
        };
        let _ = sender.send(WorkerResult::Refresh(result));
        unsafe {
            let _ = PostMessageW(hwnd as HWND, WORKER_COMPLETE, 0, 0);
        }
    });
}

#[cfg(target_os = "windows")]
fn read_host_status(endpoint: &Path) -> Result<HostStatus, String> {
    let response = LocalClient::new(endpoint).call(&RequestEnvelope::v1(
        format!("prh-tray-refresh-{}", uuid::Uuid::new_v4()),
        Request::GetHostStatus(GetHostStatusParams {}),
    ));
    match response.map_err(|error| error.to_string())?.outcome {
        ResponseOutcome::Ok { response } => match *response {
            Response::GetHostStatus(status) => Ok(status),
            _ => Err("host returned the wrong response to GetHostStatus".to_owned()),
        },
        ResponseOutcome::Error { error } => Err(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn drain_worker_results(hwnd: HWND) {
    loop {
        let result = {
            let state = state().lock().expect("tray state lock poisoned");
            state.receiver.try_recv()
        };
        match result {
            Ok(WorkerResult::Refresh(result)) => {
                let mut state = state().lock().expect("tray state lock poisoned");
                state.refresh_in_flight = false;
                match result.service {
                    Ok(status) => {
                        state.service_status = status;
                        state.service_query_error = None;
                    }
                    Err(error) => {
                        state.service_status = None;
                        state.service_query_error = Some(error);
                    }
                }
                match result.host {
                    Ok(status) => {
                        let projection = project_tray_status(&status);
                        if projection.alert_fingerprint.is_none() {
                            // A cleared condition ends the prior dismissal so
                            // an equivalent future recurrence is visible.
                            state.dismissed_alert = None;
                        }
                        state.projection = Some(projection);
                        state.reachability_error = None;
                    }
                    Err(error) => {
                        state.projection = None;
                        state.reachability_error = Some(error);
                    }
                }
            }
            Ok(WorkerResult::Mutation(result)) => {
                if let Err(error) = result {
                    state().lock().expect("tray state lock poisoned").last_error = Some(error);
                }
                request_refresh(hwnd);
            }
            Ok(WorkerResult::HostControl(result)) => {
                if let Err(error) = result {
                    state().lock().expect("tray state lock poisoned").last_error = Some(error);
                }
                request_refresh(hwnd);
            }
            Ok(WorkerResult::Logs(result)) => match result {
                Ok(path) => {
                    if let Err(error) = open_file(&path) {
                        state().lock().expect("tray state lock poisoned").last_error = Some(error);
                    }
                }
                Err(error) => {
                    state().lock().expect("tray state lock poisoned").last_error = Some(error);
                }
            },
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
        }
    }
    update_tray_tip(hwnd);
}

#[cfg(target_os = "windows")]
fn show_menu(hwnd: HWND) {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return;
    }
    let _guard = MenuGuard(menu);
    let mut next_id = DYNAMIC_COMMAND_START;
    let mut actions = BTreeMap::new();
    {
        let state = state().lock().expect("tray state lock poisoned");
        append_text(
            menu,
            MF_STRING | MF_DISABLED,
            0,
            &host_service_label(&state),
        );
        if let Some(projection) = &state.projection {
            append_text(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                &format!("API: {}", projection.host_label),
            );
        }
        if let Some(error) = &state.service_query_error {
            append_text(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                &format!("SCM error: {}", compact(error, 100)),
            );
        }
        if let Some(error) = &state.reachability_error {
            append_text(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                &format!("API unreachable: {}", compact(error, 100)),
            );
        }
        if let Some(error) = &state.last_error {
            append_text(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                &format!("Error: {}", compact(error, 100)),
            );
        }
        append_separator(menu);
        let installed = state.service_status.is_some();
        let running = state
            .service_status
            .as_ref()
            .is_some_and(WindowsServiceStatus::is_running);
        let stopped = state
            .service_status
            .as_ref()
            .is_some_and(WindowsServiceStatus::is_stopped);
        append_text(
            menu,
            MF_STRING | if installed && stopped { 0 } else { MF_GRAYED },
            COMMAND_HOST_START as usize,
            "Start host service…",
        );
        append_text(
            menu,
            MF_STRING | if installed && !stopped { 0 } else { MF_GRAYED },
            COMMAND_HOST_STOP as usize,
            "Stop host service…",
        );
        append_text(
            menu,
            MF_STRING | if installed && running { 0 } else { MF_GRAYED },
            COMMAND_HOST_RESTART as usize,
            "Restart host service…",
        );
        append_separator(menu);
        if let Some(projection) = &state.projection {
            let alerts_visible = projection.alerts_visible(state.dismissed_alert.as_deref());
            for service in &projection.services {
                let submenu = unsafe { CreatePopupMenu() };
                if submenu.is_null() {
                    continue;
                }
                let attention = if alerts_visible && service.needs_attention {
                    "! "
                } else {
                    ""
                };
                append_text(
                    submenu,
                    MF_STRING | MF_DISABLED,
                    0,
                    &format!("Status: {}", service.status),
                );
                append_service_action(
                    submenu,
                    &mut actions,
                    &mut next_id,
                    "Start",
                    service.can_start,
                    TrayAction::Start(service.service_id.clone()),
                );
                append_service_action(
                    submenu,
                    &mut actions,
                    &mut next_id,
                    "Stop",
                    service.can_stop,
                    TrayAction::Stop(service.service_id.clone()),
                );
                append_service_action(
                    submenu,
                    &mut actions,
                    &mut next_id,
                    "Restart",
                    service.can_restart,
                    TrayAction::Restart(service.service_id.clone()),
                );
                append_service_action(
                    submenu,
                    &mut actions,
                    &mut next_id,
                    "Open logs",
                    true,
                    TrayAction::OpenLogs(service.service_id.clone()),
                );
                append_text(
                    menu,
                    MF_STRING | MF_POPUP,
                    submenu as usize,
                    &format!("{attention}{}", service.label),
                );
            }
        } else {
            append_text(
                menu,
                MF_STRING | MF_DISABLED,
                0,
                "No service status available",
            );
        }
        append_separator(menu);
        append_text(menu, MF_STRING, COMMAND_REFRESH as usize, "Refresh");
        let clear_enabled = state.last_error.is_some()
            || state.service_query_error.is_some()
            || state.reachability_error.is_some()
            || state.projection.as_ref().is_some_and(|projection| {
                projection.alerts_visible(state.dismissed_alert.as_deref())
            });
        append_text(
            menu,
            MF_STRING | if clear_enabled { 0 } else { MF_GRAYED },
            COMMAND_CLEAR_ALERTS as usize,
            "Clear errors and alerts",
        );
        append_separator(menu);
        append_text(menu, MF_STRING, COMMAND_EXIT as usize, "Exit tray");
    }
    state().lock().expect("tray state lock poisoned").actions = actions;
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return;
    }
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    let command = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        )
    } as u32;
    unsafe {
        let _ = PostMessageW(hwnd, WM_NULL, 0, 0);
    }
    if command != 0 {
        execute_menu_command(hwnd, command);
    }
}

#[cfg(target_os = "windows")]
fn execute_menu_command(hwnd: HWND, command: u32) {
    match command {
        COMMAND_REFRESH => request_refresh(hwnd),
        COMMAND_CLEAR_ALERTS => {
            let mut state = state().lock().expect("tray state lock poisoned");
            state.dismissed_alert = state
                .projection
                .as_ref()
                .and_then(|projection| projection.alert_fingerprint.clone());
            state.last_error = None;
            state.service_query_error = None;
            state.reachability_error = None;
            drop(state);
            update_tray_tip(hwnd);
        }
        COMMAND_HOST_START => confirm_and_spawn_host_control(hwnd, HostControl::Start),
        COMMAND_HOST_STOP => confirm_and_spawn_host_control(hwnd, HostControl::Stop),
        COMMAND_HOST_RESTART => confirm_and_spawn_host_control(hwnd, HostControl::Restart),
        COMMAND_EXIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {
            let action = state()
                .lock()
                .expect("tray state lock poisoned")
                .actions
                .get(&command)
                .cloned();
            if let Some(action) = action {
                spawn_action(hwnd, action);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn host_service_label(state: &TrayState) -> String {
    match (&state.service_status, state.projection.is_some()) {
        (Some(status), true) if status.is_running() => {
            format!(
                "Host service: running (PID {}, API reachable)",
                status.process_id
            )
        }
        (Some(status), false) if status.is_running() => {
            format!(
                "Host service: running (PID {}, API unreachable)",
                status.process_id
            )
        }
        (Some(status), reachable) => format!(
            "Host service: {} ({})",
            status.state,
            if reachable {
                "API reachable"
            } else {
                "API unavailable"
            }
        ),
        (None, _) if state.service_query_error.is_some() => {
            "Host service: SCM query failed".to_owned()
        }
        (None, _) => "Host service: not installed".to_owned(),
    }
}

#[cfg(target_os = "windows")]
fn confirm_and_spawn_host_control(hwnd: HWND, control: HostControl) {
    let verb = match control {
        HostControl::Start => "start",
        HostControl::Stop => "stop",
        HostControl::Restart => "restart",
    };
    let message = wide(&format!(
        "Do you want to {verb} the Persistent Runtime Host Windows service?\n\nThis affects every supervised service."
    ));
    let title = wide("Confirm host service control");
    let confirmed = unsafe {
        MessageBoxW(
            hwnd,
            message.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        )
    } == IDYES;
    if confirmed {
        spawn_host_control(hwnd, control);
    }
}

#[cfg(target_os = "windows")]
fn spawn_host_control(hwnd: HWND, control: HostControl) {
    let (service_name, sender) = {
        let state = state().lock().expect("tray state lock poisoned");
        (state.service_name.clone(), state.sender.clone())
    };
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let result = match control {
            HostControl::Start => start_windows_service(&service_name, Duration::from_secs(30)),
            HostControl::Stop => stop_windows_service(&service_name, Duration::from_secs(180)),
            HostControl::Restart => restart_windows_service(
                &service_name,
                Duration::from_secs(180),
                Duration::from_secs(30),
            ),
        }
        .map(|_| ())
        .map_err(|error| error.to_string());
        let _ = sender.send(WorkerResult::HostControl(result));
        unsafe {
            let _ = PostMessageW(hwnd as HWND, WORKER_COMPLETE, 0, 0);
        }
    });
}

#[cfg(target_os = "windows")]
fn spawn_action(hwnd: HWND, action: TrayAction) {
    let (endpoint, state_dir, sender) = {
        let state = state().lock().expect("tray state lock poisoned");
        (
            state.endpoint.clone(),
            state.state_dir.clone(),
            state.sender.clone(),
        )
    };
    let hwnd = hwnd as usize;
    std::thread::spawn(move || {
        let result = match action {
            TrayAction::Start(service_id) => WorkerResult::Mutation(run_mutation(
                &endpoint,
                Request::EnsureRunning(EnsureRunningParams {
                    service_id: ServiceId::new(service_id),
                    mutation: tray_mutation(),
                }),
            )),
            TrayAction::Stop(service_id) => WorkerResult::Mutation(run_mutation(
                &endpoint,
                Request::EnsureStopped(EnsureStoppedParams {
                    service_id: ServiceId::new(service_id),
                    cascade: false,
                    mutation: tray_mutation(),
                }),
            )),
            TrayAction::Restart(service_id) => WorkerResult::Mutation(run_mutation(
                &endpoint,
                Request::Restart(RestartParams {
                    service_id: ServiceId::new(service_id),
                    cascade: false,
                    mutation: tray_mutation(),
                }),
            )),
            TrayAction::OpenLogs(service_id) => {
                WorkerResult::Logs(write_log_view(&endpoint, &state_dir, &service_id))
            }
        };
        let _ = sender.send(result);
        unsafe {
            let _ = PostMessageW(hwnd as HWND, WORKER_COMPLETE, 0, 0);
        }
    });
}

#[cfg(target_os = "windows")]
fn run_mutation(endpoint: &Path, request: Request) -> Result<(), String> {
    let response = LocalClient::new(endpoint)
        .call(&RequestEnvelope::v1(
            format!("prh-tray-action-{}", uuid::Uuid::new_v4()),
            request,
        ))
        .map_err(|error| error.to_string())?;
    match response.outcome {
        ResponseOutcome::Ok { .. } => Ok(()),
        ResponseOutcome::Error { error } => Err(error.to_string()),
    }
}

#[cfg(target_os = "windows")]
fn tray_mutation() -> MutationOptions {
    MutationOptions::new(format!("prh-tray-{}", uuid::Uuid::new_v4()))
}

#[cfg(target_os = "windows")]
fn write_log_view(endpoint: &Path, state_dir: &Path, service_id: &str) -> Result<PathBuf, String> {
    let response = LocalClient::new(endpoint)
        .call(&RequestEnvelope::v1(
            format!("prh-tray-logs-{}", uuid::Uuid::new_v4()),
            Request::ReadLogs(ReadLogsParams {
                service_id: ServiceId::new(service_id),
                run_id: None,
                after_sequence: None,
                limit: 1_000,
            }),
        ))
        .map_err(|error| error.to_string())?;
    let page = match response.outcome {
        ResponseOutcome::Ok { response } => match *response {
            Response::ReadLogs(page) => page,
            _ => return Err("host returned the wrong response to ReadLogs".to_owned()),
        },
        ResponseOutcome::Error { error } => return Err(error.to_string()),
    };
    let directory = state_dir.join("tray-log-view");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{service_id}.log"));
    let mut view = String::new();
    view.push_str(&format!(
        "Persistent Runtime Host logs: {service_id}\r\n\r\n"
    ));
    for entry in page.entries {
        let encoding = match entry.encoding {
            LogEncoding::Utf8 => "utf8",
            LogEncoding::Base64 => "base64",
        };
        view.push_str(&format!(
            "[{}] [{}] [{}] {}\r\n",
            entry.sequence,
            format!("{:?}", entry.stream).to_lowercase(),
            encoding,
            entry.data
        ));
    }
    std::fs::write(&path, view).map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(target_os = "windows")]
fn open_file(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let operation = wide("open");
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(format!(
            "Windows could not open the log view (ShellExecute result {result})"
        ))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn add_tray_icon(hwnd: HWND) -> std::io::Result<()> {
    let mut data = tray_icon_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK;
    data.hIcon = unsafe { LoadIconW(null_mut(), IDI_APPLICATION) };
    set_tip(&mut data, &effective_tooltip());
    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn update_tray_tip(hwnd: HWND) {
    let mut data = tray_icon_data(hwnd);
    data.uFlags = NIF_TIP;
    set_tip(&mut data, &effective_tooltip());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

#[cfg(target_os = "windows")]
fn remove_tray_icon(hwnd: HWND) {
    let data = tray_icon_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

#[cfg(target_os = "windows")]
fn tray_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    }
}

#[cfg(target_os = "windows")]
fn effective_tooltip() -> String {
    let state = state().lock().expect("tray state lock poisoned");
    if state.last_error.is_some() || state.service_query_error.is_some() {
        return "PRH: connection or action error".to_owned();
    }
    if state
        .service_status
        .as_ref()
        .is_none_or(|status| !status.is_running())
    {
        return state.service_status.as_ref().map_or_else(
            || "PRH: host service not installed".to_owned(),
            |status| format!("PRH: host service {}", status.state),
        );
    }
    let Some(projection) = &state.projection else {
        return "PRH: host running, API unreachable".to_owned();
    };
    if projection.alerts_visible(state.dismissed_alert.as_deref()) {
        projection.tooltip.clone()
    } else {
        let running = projection
            .services
            .iter()
            .filter(|service| service.status.starts_with("running"))
            .count();
        format!(
            "PRH: {running}/{} services running",
            projection.services.len()
        )
    }
}

#[cfg(target_os = "windows")]
fn set_tip(data: &mut NOTIFYICONDATAW, tip: &str) {
    let encoded = tip.encode_utf16().take(data.szTip.len() - 1);
    for (destination, value) in data.szTip.iter_mut().zip(encoded) {
        *destination = value;
    }
}

#[cfg(target_os = "windows")]
fn append_service_action(
    menu: HMENU,
    actions: &mut BTreeMap<u32, TrayAction>,
    next_id: &mut u32,
    label: &str,
    enabled: bool,
    action: TrayAction,
) {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    actions.insert(id, action);
    append_text(
        menu,
        MF_STRING | if enabled { 0 } else { MF_GRAYED },
        id as usize,
        label,
    );
}

#[cfg(target_os = "windows")]
fn append_text(menu: HMENU, flags: u32, id: usize, text: &str) {
    let text = wide(text);
    unsafe {
        let _ = AppendMenuW(menu, flags, id, text.as_ptr());
    }
}

#[cfg(target_os = "windows")]
fn append_separator(menu: HMENU) {
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, null());
    }
}

#[cfg(target_os = "windows")]
fn compact(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push('…');
    }
    result
}

#[cfg(target_os = "windows")]
fn state() -> &'static Mutex<TrayState> {
    TRAY_STATE.get().expect("tray state initialized")
}

#[cfg(target_os = "windows")]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(target_os = "windows")]
fn show_error_dialog(message: &str) {
    let message = wide(message);
    let title = wide("Persistent Runtime Host");
    unsafe {
        let _ = MessageBoxW(
            null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(target_os = "windows")]
struct MenuGuard(HMENU);

#[cfg(target_os = "windows")]
impl Drop for MenuGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("prh-tray is available only on Windows");
    std::process::exit(2);
}
