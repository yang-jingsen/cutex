#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::io;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_SERVICE_ALREADY_RUNNING,
    ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
use windows_sys::Win32::System::Services::*;

pub const DEFAULT_WINDOWS_SERVICE_NAME: &str = "PersistentRuntimeHost";
pub const DEFAULT_WINDOWS_SERVICE_DISPLAY_NAME: &str = "Persistent Runtime Host";
pub const DEFAULT_WINDOWS_RUN_VALUE_NAME: &str = "PersistentRuntimeHostTray";
pub const DEFAULT_WINDOWS_INSTALL_ROOT: &str = r"D:\Programs\persistent-runtime-host";
pub const DEFAULT_WINDOWS_STATE_DIR: &str = r"C:\ProgramData\PersistentRuntimeHost\state-v1";

const OPERATOR_SERVICE_ACCESS: u32 =
    SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP | SERVICE_INTERROGATE;
const DELETE_ACCESS: u32 = 0x0001_0000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WindowsServiceStatus {
    pub service_name: String,
    pub state: String,
    pub state_code: u32,
    pub process_id: u32,
}

impl WindowsServiceStatus {
    pub fn is_running(&self) -> bool {
        self.state_code == SERVICE_RUNNING
    }

    pub fn is_stopped(&self) -> bool {
        self.state_code == SERVICE_STOPPED
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ServiceConfiguration<'a> {
    pub service_name: &'a str,
    pub display_name: &'a str,
    pub binary_path: &'a str,
    pub operator_sid: &'a str,
}

pub fn query_windows_service(service_name: &str) -> io::Result<Option<WindowsServiceStatus>> {
    validate_service_name(service_name)?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT)?;
    let service = match ServiceHandle::open(&manager, service_name, SERVICE_QUERY_STATUS) {
        Ok(service) => service,
        Err(error) if raw_error(&error) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => return Ok(None),
        Err(error) => return Err(context("open Windows service", error)),
    };
    query_status(&service).map(Some)
}

pub fn start_windows_service(
    service_name: &str,
    timeout: Duration,
) -> io::Result<WindowsServiceStatus> {
    validate_service_name(service_name)?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT)?;
    let service =
        ServiceHandle::open(&manager, service_name, SERVICE_START | SERVICE_QUERY_STATUS)?;
    let status = query_status(&service)?;
    if status.is_running() {
        return Ok(status);
    }
    if status.state_code != SERVICE_START_PENDING
        && unsafe { StartServiceW(service.raw(), 0, null()) } == 0
    {
        let error = last_error();
        if raw_error(&error) != Some(ERROR_SERVICE_ALREADY_RUNNING) {
            return Err(context("start Windows service", error));
        }
    }
    wait_for_state(&service, service_name, SERVICE_RUNNING, timeout)
}

pub fn stop_windows_service(
    service_name: &str,
    timeout: Duration,
) -> io::Result<WindowsServiceStatus> {
    validate_service_name(service_name)?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT)?;
    let service = ServiceHandle::open(&manager, service_name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let status = query_status(&service)?;
    if status.is_stopped() {
        return Ok(status);
    }
    if status.state_code != SERVICE_STOP_PENDING {
        let mut ignored = SERVICE_STATUS::default();
        if unsafe { ControlService(service.raw(), SERVICE_CONTROL_STOP, &mut ignored) } == 0 {
            let error = last_error();
            if raw_error(&error) != Some(ERROR_SERVICE_NOT_ACTIVE) {
                return Err(context("stop Windows service", error));
            }
        }
    }
    wait_for_state(&service, service_name, SERVICE_STOPPED, timeout)
}

pub fn restart_windows_service(
    service_name: &str,
    stop_timeout: Duration,
    start_timeout: Duration,
) -> io::Result<WindowsServiceStatus> {
    stop_windows_service(service_name, stop_timeout)?;
    start_windows_service(service_name, start_timeout)
}

pub(crate) fn ensure_service(configuration: &ServiceConfiguration<'_>) -> io::Result<bool> {
    validate_service_name(configuration.service_name)?;
    reject_nul(configuration.display_name, "service display name")?;
    reject_nul(configuration.binary_path, "service binary path")?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
    let existing = ServiceHandle::open(&manager, configuration.service_name, SERVICE_ALL_ACCESS);
    let (service, created) = match existing {
        Ok(service) => (service, false),
        Err(error) if raw_error(&error) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => {
            let name = wide(configuration.service_name);
            let display_name = wide(configuration.display_name);
            let binary_path = wide(configuration.binary_path);
            let local_system = wide("LocalSystem");
            let handle = unsafe {
                CreateServiceW(
                    manager.raw(),
                    name.as_ptr(),
                    display_name.as_ptr(),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    binary_path.as_ptr(),
                    null(),
                    null_mut(),
                    null(),
                    local_system.as_ptr(),
                    null(),
                )
            };
            (ServiceHandle::new(handle, "create Windows service")?, true)
        }
        Err(error) => return Err(context("open Windows service for configuration", error)),
    };

    if !created {
        let binary_path = wide(configuration.binary_path);
        let display_name = wide(configuration.display_name);
        let local_system = wide("LocalSystem");
        if unsafe {
            ChangeServiceConfigW(
                service.raw(),
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                binary_path.as_ptr(),
                null(),
                null_mut(),
                null(),
                local_system.as_ptr(),
                null(),
                display_name.as_ptr(),
            )
        } == 0
        {
            return Err(context(
                "update Windows service configuration",
                last_error(),
            ));
        }
    }

    configure_description(&service)?;
    configure_recovery(&service)?;
    configure_preshutdown(&service)?;
    configure_operator_access(&service, configuration.operator_sid)?;
    Ok(created)
}

pub(crate) fn service_binary_path(service_name: &str) -> io::Result<Option<String>> {
    validate_service_name(service_name)?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT)?;
    let service = match ServiceHandle::open(&manager, service_name, SERVICE_QUERY_CONFIG) {
        Ok(service) => service,
        Err(error) if raw_error(&error) == Some(ERROR_SERVICE_DOES_NOT_EXIST) => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut required = 0;
    unsafe {
        let _ = QueryServiceConfigW(service.raw(), null_mut(), 0, &mut required);
    }
    if required == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(context("size Windows service configuration", last_error()));
    }
    let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; words];
    let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe { QueryServiceConfigW(service.raw(), config, required, &mut required) } == 0 {
        return Err(context("query Windows service configuration", last_error()));
    }
    let pointer = unsafe { (*config).lpBinaryPathName };
    if pointer.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows service has no binary path",
        ));
    }
    Ok(Some(unsafe { wide_pointer_to_string(pointer) }?))
}

pub(crate) fn delete_service(service_name: &str, timeout: Duration) -> io::Result<()> {
    validate_service_name(service_name)?;
    if query_windows_service(service_name)?.is_none() {
        return Ok(());
    }
    stop_windows_service(service_name, timeout)?;
    let manager = ServiceHandle::manager(SC_MANAGER_CONNECT)?;
    let service =
        ServiceHandle::open(&manager, service_name, DELETE_ACCESS | SERVICE_QUERY_STATUS)?;
    if unsafe { DeleteService(service.raw()) } == 0 {
        return Err(context("delete Windows service", last_error()));
    }
    drop(service);
    let deadline = Instant::now() + timeout;
    loop {
        if query_windows_service(service_name)?.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Windows service {service_name} remained delete-pending"),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn configure_description(service: &ServiceHandle) -> io::Result<()> {
    let mut text =
        wide("Supervises persistent local background services for the installed operator.");
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: text.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.raw(),
            SERVICE_CONFIG_DESCRIPTION,
            (&description as *const SERVICE_DESCRIPTIONW).cast(),
        )
    } == 0
    {
        Err(context("set Windows service description", last_error()))
    } else {
        Ok(())
    }
}

fn configure_recovery(service: &ServiceHandle) -> io::Result<()> {
    let mut actions = [
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 1_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 2_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5_000,
        },
    ];
    let recovery = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: null_mut(),
        lpCommand: null_mut(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.raw(),
            SERVICE_CONFIG_FAILURE_ACTIONS,
            (&recovery as *const SERVICE_FAILURE_ACTIONSW).cast(),
        )
    } == 0
    {
        return Err(context(
            "set Windows service recovery actions",
            last_error(),
        ));
    }
    let failure_flag = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.raw(),
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&failure_flag as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        )
    } == 0
    {
        return Err(context(
            "enable Windows service recovery actions",
            last_error(),
        ));
    }
    let delayed = SERVICE_DELAYED_AUTO_START_INFO {
        fDelayedAutostart: 1,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.raw(),
            SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
            (&delayed as *const SERVICE_DELAYED_AUTO_START_INFO).cast(),
        )
    } == 0
    {
        return Err(context("set delayed Windows service start", last_error()));
    }
    Ok(())
}

fn configure_preshutdown(service: &ServiceHandle) -> io::Result<()> {
    let info = SERVICE_PRESHUTDOWN_INFO {
        dwPreshutdownTimeout: 180_000,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service.raw(),
            SERVICE_CONFIG_PRESHUTDOWN_INFO,
            (&info as *const SERVICE_PRESHUTDOWN_INFO).cast(),
        )
    } == 0
    {
        Err(context(
            "set Windows service preshutdown timeout",
            last_error(),
        ))
    } else {
        Ok(())
    }
}

fn configure_operator_access(service: &ServiceHandle, operator_sid: &str) -> io::Result<()> {
    crate::windows_security::configure_operator_sid(operator_sid)?;
    let sddl =
        format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x{OPERATOR_SERVICE_ACCESS:08X};;;{operator_sid})");
    let encoded = wide(&sddl);
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            encoded.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(context(
            "build Windows service security descriptor",
            last_error(),
        ));
    }
    let descriptor = LocalDescriptor(descriptor);
    if unsafe { SetServiceObjectSecurity(service.raw(), DACL_SECURITY_INFORMATION, descriptor.0) }
        == 0
    {
        Err(context("set Windows service operator access", last_error()))
    } else {
        Ok(())
    }
}

fn wait_for_state(
    service: &ServiceHandle,
    service_name: &str,
    target: u32,
    timeout: Duration,
) -> io::Result<WindowsServiceStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = query_status(service)?;
        if status.state_code == target {
            return Ok(status);
        }
        if target == SERVICE_RUNNING && status.state_code == SERVICE_STOPPED {
            return Err(io::Error::other(format!(
                "Windows service {service_name} stopped while it was starting (exit code {})",
                query_raw_status(service)?.dwWin32ExitCode
            )));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for Windows service {service_name} to enter {} (current {})",
                    state_name(target),
                    status.state
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn query_status(service: &ServiceHandle) -> io::Result<WindowsServiceStatus> {
    let status = query_raw_status(service)?;
    Ok(WindowsServiceStatus {
        service_name: service.name.clone(),
        state: state_name(status.dwCurrentState).to_owned(),
        state_code: status.dwCurrentState,
        process_id: status.dwProcessId,
    })
}

fn query_raw_status(service: &ServiceHandle) -> io::Result<SERVICE_STATUS_PROCESS> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut required = 0;
    if unsafe {
        QueryServiceStatusEx(
            service.raw(),
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut required,
        )
    } == 0
    {
        Err(context("query Windows service status", last_error()))
    } else {
        Ok(status)
    }
}

fn state_name(state: u32) -> &'static str {
    match state {
        SERVICE_STOPPED => "stopped",
        SERVICE_START_PENDING => "start-pending",
        SERVICE_STOP_PENDING => "stop-pending",
        SERVICE_RUNNING => "running",
        SERVICE_CONTINUE_PENDING => "continue-pending",
        SERVICE_PAUSE_PENDING => "pause-pending",
        SERVICE_PAUSED => "paused",
        _ => "unknown",
    }
}

#[doc(hidden)]
pub fn validate_service_name(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 192
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows service name must contain only ASCII letters, digits, dot, underscore, or hyphen",
        ))
    } else {
        Ok(())
    }
}

fn reject_nul(value: &str, label: &str) -> io::Result<()> {
    if value.contains('\0') {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} contains a NUL character"),
        ))
    } else {
        Ok(())
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe fn wide_pointer_to_string(pointer: *const u16) -> io::Result<String> {
    let mut length = 0;
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn raw_error(error: &io::Error) -> Option<u32> {
    error.raw_os_error().map(|value| value as u32)
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

fn context(action: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("failed to {action}: {error}"))
}

struct ServiceHandle {
    handle: SC_HANDLE,
    name: String,
}

impl ServiceHandle {
    fn manager(access: u32) -> io::Result<Self> {
        let handle = unsafe { OpenSCManagerW(null(), null(), access) };
        Self::new(handle, "open local Windows Service Control Manager").map(|mut handle| {
            handle.name = "Service Control Manager".to_owned();
            handle
        })
    }

    fn open(manager: &Self, service_name: &str, access: u32) -> io::Result<Self> {
        let name = wide(service_name);
        let handle = unsafe { OpenServiceW(manager.raw(), name.as_ptr(), access) };
        Self::new(handle, "open Windows service").map(|mut handle| {
            handle.name = service_name.to_owned();
            handle
        })
    }

    fn new(handle: SC_HANDLE, _action: &str) -> io::Result<Self> {
        if handle.is_null() {
            Err(last_error())
        } else {
            Ok(Self {
                handle,
                name: String::new(),
            })
        }
    }

    fn raw(&self) -> SC_HANDLE {
        self.handle
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                let _ = CloseServiceHandle(self.handle);
            }
        }
    }
}

struct LocalDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for LocalDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
}
