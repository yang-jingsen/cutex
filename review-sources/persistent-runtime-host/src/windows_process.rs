#![cfg(target_os = "windows")]

use crate::model::{
    HealthState, ProcessIdentity, ReadinessProbe, RunId, ServiceDefinition, ServiceId,
    ShutdownPolicy, StopOutcome,
};
use crate::process_backend::{BackendEvent, ProcessBackend, ProcessBackendError};
use crate::protocol::LogStream;
use crate::windows_support::{
    build_windows_command_line, encode_windows_environment, merge_windows_environment,
    WindowsLaunchGate, WINDOWS_INHERITED_HANDLE_ROLES,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetHandleInformation, GetLastError, SetHandleInformation, FILETIME, HANDLE,
    HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Foundation::{GENERIC_READ, TRUE};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpCrackUrl, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    URL_COMPONENTS, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_INTERNET_SCHEME_HTTPS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, GetProcessTimes,
    InitializeProcThreadAttributeList, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

const FORCE_EXIT_CODE: u32 = 0x5052_4801;

#[derive(Clone, Debug)]
pub struct WindowsProcessConfig {
    pub readiness_deadline: Duration,
    pub descendant_cleanup_timeout: Duration,
    pub event_queue_capacity: usize,
}

impl Default for WindowsProcessConfig {
    fn default() -> Self {
        Self {
            readiness_deadline: Duration::from_secs(15),
            descendant_cleanup_timeout: Duration::from_secs(3),
            event_queue_capacity: 1_024,
        }
    }
}

pub struct WindowsProcessBackend {
    config: WindowsProcessConfig,
    shared: Arc<Shared>,
}

struct Shared {
    event_sender: SyncSender<BackendEvent>,
    processes: Mutex<BTreeMap<(ServiceId, RunId), Arc<ProcessRecord>>>,
}

struct ProcessRecord {
    process_id: u32,
    job: OwnedHandle,
    containment_failure: Mutex<Option<String>>,
    exit: Mutex<Option<ExitRecord>>,
    exit_changed: Condvar,
}

#[derive(Clone, Copy)]
struct ExitRecord {
    job_empty: bool,
    forced_descendant_cleanup: bool,
}

impl WindowsProcessBackend {
    pub fn new(
        config: WindowsProcessConfig,
    ) -> Result<(Self, Receiver<BackendEvent>), ProcessBackendError> {
        let (event_sender, event_receiver) = mpsc::sync_channel(config.event_queue_capacity.max(1));
        Ok((
            Self {
                config,
                shared: Arc::new(Shared {
                    event_sender,
                    processes: Mutex::new(BTreeMap::new()),
                }),
            },
            event_receiver,
        ))
    }

    fn wait_until_ready(
        &self,
        definition: &ServiceDefinition,
        record: &ProcessRecord,
    ) -> Result<(), ProcessBackendError> {
        let Some(probe) = &definition.readiness_probe else {
            return Ok(());
        };
        let deadline = Instant::now() + self.config.readiness_deadline;
        loop {
            if let Some(message) = record
                .containment_failure
                .lock()
                .expect("containment failure lock poisoned")
                .clone()
            {
                return Err(ProcessBackendError::new(message));
            }
            if record
                .exit
                .lock()
                .expect("process exit lock poisoned")
                .is_some()
            {
                return Err(ProcessBackendError::new(
                    "service exited before its readiness probe succeeded",
                ));
            }
            if probe_once(probe) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ProcessBackendError::new(format!(
                    "service readiness did not succeed within {} milliseconds",
                    self.config.readiness_deadline.as_millis()
                )));
            }
            thread::sleep(probe_interval(probe).min(Duration::from_millis(250)));
        }
    }

    fn start_health_monitor(
        &self,
        definition: &ServiceDefinition,
        service_id: ServiceId,
        run_id: RunId,
        record: Arc<ProcessRecord>,
    ) -> Result<(), ProcessBackendError> {
        let Some(probe) = definition.readiness_probe.clone() else {
            return Ok(());
        };
        let sender = self.shared.event_sender.clone();
        thread::Builder::new()
            .name("prh-win-health".to_owned())
            .spawn(move || loop {
                thread::sleep(probe_interval(&probe));
                if record
                    .exit
                    .lock()
                    .expect("process exit lock poisoned")
                    .is_some()
                {
                    break;
                }
                let health = if probe_once(&probe) {
                    HealthState::Healthy
                } else {
                    HealthState::Unhealthy
                };
                match sender.try_send(BackendEvent::Health {
                    service_id: service_id.clone(),
                    run_id: run_id.clone(),
                    health,
                }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
            })
            .map(|_| ())
            .map_err(|error| {
                ProcessBackendError::new(format!("failed to start Windows health monitor: {error}"))
            })
    }

    fn terminate_failed_start(&self, record: &ProcessRecord) {
        let _ = terminate_job(&record.job);
        let _ = wait_for_clean_exit(record, self.config.descendant_cleanup_timeout);
    }
}

impl ProcessBackend for WindowsProcessBackend {
    fn name(&self) -> &'static str {
        "windows_job_object"
    }

    fn start(
        &self,
        definition: &ServiceDefinition,
        run_id: &RunId,
    ) -> Result<ProcessIdentity, ProcessBackendError> {
        validate_windows_paths(definition)?;
        if self
            .shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .keys()
            .any(|(service_id, _)| service_id == &definition.id)
        {
            return Err(ProcessBackendError::new(
                "a prior occurrence is still tracked or awaiting Job cleanup",
            )
            .retryable());
        }

        let job = create_kill_on_close_job()?;
        let (stdout_reader, stdout_writer) = create_output_pipe()?;
        let (stderr_reader, stderr_writer) = create_output_pipe()?;
        let stdin = open_inheritable_null_input()?;
        let inherited: [HANDLE; WINDOWS_INHERITED_HANDLE_ROLES.len()] =
            [stdin.raw(), stdout_writer.raw(), stderr_writer.raw()];
        let attributes = AttributeList::for_handle_allowlist(&inherited)?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin.raw();
        startup.StartupInfo.hStdOutput = stdout_writer.raw();
        startup.StartupInfo.hStdError = stderr_writer.raw();
        startup.lpAttributeList = attributes.raw();

        let application = wide_string(&definition.executable);
        let mut command_line = wide_string(&build_windows_command_line(
            &definition.executable,
            &definition.arguments,
        ));
        let working_directory = wide_string(&definition.working_directory);
        let environment_entries = controlled_windows_environment(&definition.environment);
        let environment = encode_windows_environment(&environment_entries);
        let mut process = PROCESS_INFORMATION::default();
        let mut gate = WindowsLaunchGate::new();
        // SAFETY: all buffers, handles, the initialized attribute list, and
        // structures remain live through CreateProcessW. lpApplicationName is
        // explicit, so command-line parsing cannot substitute another image.
        if unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                TRUE,
                CREATE_SUSPENDED
                    | CREATE_NEW_PROCESS_GROUP
                    | CREATE_UNICODE_ENVIRONMENT
                    | EXTENDED_STARTUPINFO_PRESENT,
                environment.as_ptr().cast(),
                working_directory.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        } == 0
        {
            return Err(backend_last_error(
                "failed to directly create suspended process",
            ));
        }
        let process_handle = OwnedHandle::new(process.hProcess).map_err(process_error)?;
        let thread_handle = OwnedHandle::new(process.hThread).map_err(process_error)?;
        gate.process_created_suspended()
            .map_err(ProcessBackendError::new)?;
        // Parent copies are no longer needed; only the allowlisted child copies
        // remain after successful CreateProcessW.
        drop(stdin);
        drop(stdout_writer);
        drop(stderr_writer);

        // SAFETY: the process is still suspended, and both handles are valid.
        // Failure is fatal; there is no naked-process fallback.
        if unsafe { AssignProcessToJobObject(job.raw(), process_handle.raw()) } == 0 {
            let assignment_error = backend_last_error(
                "failed to assign suspended process to its per-occurrence Job; refusing naked fallback",
            );
            if let Err(cleanup_error) = terminate_suspended_process(&process_handle) {
                return Err(ProcessBackendError::new(format!(
                    "{}; suspended-process cleanup also failed: {}",
                    assignment_error.message, cleanup_error.message
                )));
            }
            return Err(assignment_error);
        }
        gate.assigned_to_job().map_err(ProcessBackendError::new)?;
        let birth_marker = process_birth_marker(&process_handle)?;
        let key = (definition.id.clone(), run_id.clone());
        let record = Arc::new(ProcessRecord {
            process_id: process.dwProcessId,
            job,
            containment_failure: Mutex::new(None),
            exit: Mutex::new(None),
            exit_changed: Condvar::new(),
        });
        self.shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .insert(key.clone(), record.clone());

        spawn_reaper(
            process_handle,
            self.shared.clone(),
            key,
            record.clone(),
            self.config.descendant_cleanup_timeout,
        )?;
        if let Err(error) = spawn_output_drain(
            stdout_reader,
            self.shared.event_sender.clone(),
            definition.id.clone(),
            run_id.clone(),
            LogStream::Stdout,
        ) {
            self.terminate_failed_start(&record);
            return Err(error);
        }
        if let Err(error) = spawn_output_drain(
            stderr_reader,
            self.shared.event_sender.clone(),
            definition.id.clone(),
            run_id.clone(),
            LogStream::Stderr,
        ) {
            self.terminate_failed_start(&record);
            return Err(error);
        }
        gate.authorize_resume().map_err(ProcessBackendError::new)?;
        // SAFETY: the thread handle belongs to the still-suspended process and
        // the launch gate proves Job assignment completed first.
        if unsafe { ResumeThread(thread_handle.raw()) } == u32::MAX {
            let error = backend_last_error("failed to resume Job-contained process");
            self.terminate_failed_start(&record);
            return Err(error);
        }
        drop(thread_handle);

        if let Err(error) = self.wait_until_ready(definition, &record) {
            self.terminate_failed_start(&record);
            return Err(error);
        }
        if let Err(error) = self.start_health_monitor(
            definition,
            definition.id.clone(),
            run_id.clone(),
            record.clone(),
        ) {
            self.terminate_failed_start(&record);
            return Err(error);
        }
        Ok(ProcessIdentity {
            pid: process.dwProcessId,
            birth_marker,
        })
    }

    fn stop(
        &self,
        service_id: &ServiceId,
        run_id: &RunId,
        shutdown_policy: &ShutdownPolicy,
    ) -> Result<StopOutcome, ProcessBackendError> {
        let record = self
            .shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .get(&(service_id.clone(), run_id.clone()))
            .cloned();
        let Some(record) = record else {
            return Ok(StopOutcome::Graceful);
        };

        // CREATE_NEW_PROCESS_GROUP leaves CTRL+BREAK enabled. This is a best
        // effort graceful signal when host and target share a console; a GUI
        // host or non-console target may reject it and then takes the bounded
        // forced path below.
        let graceful_signal_sent =
            unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, record.process_id) } != 0;
        if graceful_signal_sent {
            if let Some(exit) = wait_for_clean_exit(
                &record,
                Duration::from_millis(shutdown_policy.graceful_timeout_ms),
            ) {
                return Ok(if exit.forced_descendant_cleanup {
                    StopOutcome::Forced
                } else {
                    StopOutcome::Graceful
                });
            }
        }

        terminate_job(&record.job)?;
        if wait_for_clean_exit(
            &record,
            Duration::from_millis(shutdown_policy.force_kill_timeout_ms),
        )
        .is_some()
        {
            Ok(StopOutcome::Forced)
        } else {
            Ok(StopOutcome::TimedOut)
        }
    }
}

impl Drop for WindowsProcessBackend {
    fn drop(&mut self) {
        let records = self
            .shared
            .processes
            .lock()
            .expect("process map lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for record in records {
            let _ = terminate_job(&record.job);
        }
    }
}

fn validate_windows_paths(definition: &ServiceDefinition) -> Result<(), ProcessBackendError> {
    if !Path::new(&definition.executable).is_absolute() {
        return Err(ProcessBackendError::new(
            "Windows executable must be an absolute native path",
        ));
    }
    if !Path::new(&definition.working_directory).is_absolute() {
        return Err(ProcessBackendError::new(
            "Windows working directory must be an absolute native path",
        ));
    }
    Ok(())
}

fn create_kill_on_close_job() -> Result<OwnedHandle, ProcessBackendError> {
    let job =
        OwnedHandle::new(unsafe { CreateJobObjectW(null(), null()) }).map_err(process_error)?;
    make_non_inheritable(&job)?;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(backend_last_error(
            "failed to enforce JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE",
        ));
    }
    Ok(job)
}

fn create_output_pipe() -> Result<(File, OwnedHandle), ProcessBackendError> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: TRUE,
    };
    let mut read_handle = null_mut();
    let mut write_handle = null_mut();
    if unsafe { CreatePipe(&mut read_handle, &mut write_handle, &attributes, 64 * 1024) } == 0 {
        return Err(backend_last_error(
            "failed to create redirected output pipe",
        ));
    }
    let read_handle = OwnedHandle::new(read_handle).map_err(process_error)?;
    let write_handle = OwnedHandle::new(write_handle).map_err(process_error)?;
    make_non_inheritable(&read_handle)?;
    verify_inheritable(&write_handle)?;
    let raw = read_handle.into_raw();
    let reader = unsafe { File::from_raw_handle(raw as RawHandle) };
    Ok((reader, write_handle))
}

fn open_inheritable_null_input() -> Result<OwnedHandle, ProcessBackendError> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: TRUE,
    };
    let null_name = wide_string("NUL");
    let handle = unsafe {
        CreateFileW(
            null_name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    let handle = OwnedHandle::new(handle).map_err(process_error)?;
    verify_inheritable(&handle)?;
    Ok(handle)
}

fn make_non_inheritable(handle: &OwnedHandle) -> Result<(), ProcessBackendError> {
    if unsafe { SetHandleInformation(handle.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(backend_last_error("failed to clear handle inheritance"));
    }
    let mut flags = 0;
    if unsafe { GetHandleInformation(handle.raw(), &mut flags) } == 0 {
        return Err(backend_last_error("failed to verify handle inheritance"));
    }
    if flags & HANDLE_FLAG_INHERIT != 0 {
        return Err(ProcessBackendError::new(
            "handle remained inheritable after inheritance was cleared",
        ));
    }
    Ok(())
}

fn verify_inheritable(handle: &OwnedHandle) -> Result<(), ProcessBackendError> {
    let mut flags = 0;
    if unsafe { GetHandleInformation(handle.raw(), &mut flags) } == 0 {
        return Err(backend_last_error(
            "failed to inspect intended child handle",
        ));
    }
    if flags & HANDLE_FLAG_INHERIT == 0 {
        return Err(ProcessBackendError::new(
            "intended child stdio handle is not inheritable",
        ));
    }
    Ok(())
}

struct AttributeList {
    storage: Vec<usize>,
    pointer: *mut core::ffi::c_void,
}

impl AttributeList {
    fn for_handle_allowlist(handles: &[HANDLE]) -> Result<Self, ProcessBackendError> {
        let mut bytes = 0_usize;
        unsafe {
            let _ = InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(backend_last_error("failed to size process attribute list"));
        }
        let mut storage = vec![0_usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        let pointer = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(pointer, 1, 0, &mut bytes) } == 0 {
            return Err(backend_last_error(
                "failed to initialize process attribute list",
            ));
        }
        let list = Self { storage, pointer };
        if unsafe {
            UpdateProcThreadAttribute(
                list.pointer,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(backend_last_error(
                "failed to install explicit inherited-handle allowlist",
            ));
        }
        Ok(list)
    }

    fn raw(&self) -> *mut core::ffi::c_void {
        debug_assert!(!self.storage.is_empty());
        self.pointer
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer);
        }
    }
}

fn spawn_output_drain(
    mut reader: File,
    sender: SyncSender<BackendEvent>,
    service_id: ServiceId,
    run_id: RunId,
    stream: LogStream,
) -> Result<(), ProcessBackendError> {
    thread::Builder::new()
        .name("prh-win-output".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; 8 * 1024];
            let mut dropped_bytes = 0_u64;
            let mut dropped_chunks = 0_u64;
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        if dropped_chunks > 0 {
                            let _ = sender.send(dropped_output_event(
                                &service_id,
                                &run_id,
                                dropped_chunks,
                                dropped_bytes,
                            ));
                        }
                        break;
                    }
                    Ok(length) => {
                        if dropped_chunks > 0 {
                            match sender.try_send(dropped_output_event(
                                &service_id,
                                &run_id,
                                dropped_chunks,
                                dropped_bytes,
                            )) {
                                Ok(()) => {
                                    dropped_chunks = 0;
                                    dropped_bytes = 0;
                                }
                                Err(TrySendError::Full(_)) => {}
                                Err(TrySendError::Disconnected(_)) => break,
                            }
                        }
                        let event = BackendEvent::Output {
                            service_id: service_id.clone(),
                            run_id: run_id.clone(),
                            stream,
                            bytes: buffer[..length].to_vec(),
                        };
                        match sender.try_send(event) {
                            Ok(()) => {}
                            Err(TrySendError::Full(BackendEvent::Output { bytes, .. })) => {
                                dropped_chunks = dropped_chunks.saturating_add(1);
                                dropped_bytes = dropped_bytes.saturating_add(bytes.len() as u64);
                            }
                            Err(TrySendError::Full(_)) => unreachable!("sent output event"),
                            Err(TrySendError::Disconnected(_)) => break,
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = sender.send(BackendEvent::LogFailure {
                            service_id,
                            run_id,
                            message: error.to_string(),
                        });
                        break;
                    }
                }
            }
        })
        .map(|_| ())
        .map_err(|error| {
            ProcessBackendError::new(format!("failed to start Windows output drain: {error}"))
        })
}

fn dropped_output_event(
    service_id: &ServiceId,
    run_id: &RunId,
    chunks: u64,
    bytes: u64,
) -> BackendEvent {
    BackendEvent::LogFailure {
        service_id: service_id.clone(),
        run_id: run_id.clone(),
        message: format!(
            "bounded process-output queue dropped {chunks} chunks ({bytes} bytes); service draining continued"
        ),
    }
}

fn spawn_reaper(
    process: OwnedHandle,
    shared: Arc<Shared>,
    key: (ServiceId, RunId),
    record: Arc<ProcessRecord>,
    cleanup_timeout: Duration,
) -> Result<(), ProcessBackendError> {
    let cleanup_shared = shared.clone();
    let cleanup_key = key.clone();
    let cleanup_record = record.clone();
    thread::Builder::new()
        .name("prh-win-reaper".to_owned())
        .spawn(move || {
            let _ = unsafe { WaitForSingleObject(process.raw(), u32::MAX) };
            let mut raw_exit_code = 0;
            let exit_code = (unsafe { GetExitCodeProcess(process.raw(), &mut raw_exit_code) } != 0)
                .then_some(raw_exit_code as i32);
            let active = active_job_processes(&record.job);
            let forced_descendant_cleanup = active.is_ok_and(|count| count > 0);
            if forced_descendant_cleanup {
                let _ = terminate_job(&record.job);
            }
            let deadline = Instant::now() + cleanup_timeout;
            let mut job_empty = wait_until_job_empty(&record.job, deadline).unwrap_or(false);
            {
                let mut exit = record.exit.lock().expect("process exit lock poisoned");
                *exit = Some(ExitRecord {
                    job_empty,
                    forced_descendant_cleanup,
                });
                record.exit_changed.notify_all();
            }
            if !job_empty {
                let message = format!(
                    "per-occurrence Job for process {} survived the bounded cleanup deadline",
                    record.process_id
                );
                *record
                    .containment_failure
                    .lock()
                    .expect("containment failure lock poisoned") = Some(message.clone());
                let _ = shared.event_sender.send(BackendEvent::ContainmentFailure {
                    service_id: key.0.clone(),
                    run_id: key.1.clone(),
                    message,
                });
                while !job_empty {
                    let _ = terminate_job(&record.job);
                    thread::sleep(Duration::from_millis(100));
                    job_empty = active_job_processes(&record.job).is_ok_and(|count| count == 0);
                }
                let mut exit = record.exit.lock().expect("process exit lock poisoned");
                *exit = Some(ExitRecord {
                    job_empty: true,
                    forced_descendant_cleanup,
                });
                record.exit_changed.notify_all();
            }
            shared
                .processes
                .lock()
                .expect("process map lock poisoned")
                .remove(&key);
            let _ = shared.event_sender.send(BackendEvent::Exit {
                service_id: key.0,
                run_id: key.1,
                exit_code,
            });
        })
        .map(|_| ())
        .map_err(|error| {
            let _ = terminate_job(&cleanup_record.job);
            while active_job_processes(&cleanup_record.job).is_ok_and(|count| count > 0) {
                let _ = terminate_job(&cleanup_record.job);
                thread::sleep(Duration::from_millis(10));
            }
            cleanup_shared
                .processes
                .lock()
                .expect("process map lock poisoned")
                .remove(&cleanup_key);
            ProcessBackendError::new(format!(
                "failed to start required Windows child reaper: {error}; Job was terminated fail-closed"
            ))
        })
}

fn active_job_processes(job: &OwnedHandle) -> Result<u32, ProcessBackendError> {
    let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    if unsafe {
        QueryInformationJobObject(
            job.raw(),
            JobObjectBasicAccountingInformation,
            (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            null_mut(),
        )
    } == 0
    {
        return Err(backend_last_error("failed to query per-occurrence Job"));
    }
    Ok(accounting.ActiveProcesses)
}

fn terminate_job(job: &OwnedHandle) -> Result<(), ProcessBackendError> {
    if matches!(active_job_processes(job), Ok(0)) {
        return Ok(());
    }
    if unsafe { TerminateJobObject(job.raw(), FORCE_EXIT_CODE) } == 0 {
        return Err(backend_last_error("failed to terminate per-occurrence Job"));
    }
    Ok(())
}

fn wait_until_job_empty(job: &OwnedHandle, deadline: Instant) -> Result<bool, ProcessBackendError> {
    loop {
        if active_job_processes(job)? == 0 {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_clean_exit(record: &ProcessRecord, timeout: Duration) -> Option<ExitRecord> {
    let exit = record.exit.lock().expect("process exit lock poisoned");
    let (exit, _) = record
        .exit_changed
        .wait_timeout_while(exit, timeout, |exit| {
            exit.is_none_or(|exit| !exit.job_empty)
        })
        .expect("process exit lock poisoned while waiting for Job cleanup");
    exit.filter(|exit| exit.job_empty)
}

fn terminate_suspended_process(process: &OwnedHandle) -> Result<(), ProcessBackendError> {
    if unsafe { WaitForSingleObject(process.raw(), 0) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    if unsafe { TerminateProcess(process.raw(), FORCE_EXIT_CODE) } == 0 {
        let error = backend_last_error("failed to terminate the unassigned suspended process");
        if unsafe { WaitForSingleObject(process.raw(), 0) } != WAIT_OBJECT_0 {
            return Err(error);
        }
    }
    if unsafe { WaitForSingleObject(process.raw(), 5_000) } == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(ProcessBackendError::new(
            "unassigned suspended process did not terminate within 5000 milliseconds",
        ))
    }
}

fn process_birth_marker(process: &OwnedHandle) -> Result<String, ProcessBackendError> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process.raw(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(backend_last_error("failed to read process creation time"));
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Ok(format!("filetime-{ticks}"))
}

fn controlled_windows_environment(additions: &BTreeMap<String, String>) -> Vec<(String, String)> {
    const BASE_NAMES: &[&str] = &[
        "SystemRoot",
        "WINDIR",
        "ComSpec",
        "Path",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "USERNAME",
        "USERDOMAIN",
        "COMPUTERNAME",
        "LANG",
        "TZ",
    ];
    let base = BASE_NAMES.iter().filter_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| ((*name).to_owned(), value))
    });
    merge_windows_environment(base, additions)
}

fn probe_interval(probe: &ReadinessProbe) -> Duration {
    let milliseconds = match probe {
        ReadinessProbe::Tcp { interval_ms, .. } | ReadinessProbe::Http { interval_ms, .. } => {
            *interval_ms
        }
    };
    Duration::from_millis(milliseconds)
}

fn probe_once(probe: &ReadinessProbe) -> bool {
    match probe {
        ReadinessProbe::Tcp {
            host,
            port,
            timeout_ms,
            ..
        } => (host.as_str(), *port)
            .to_socket_addrs()
            .ok()
            .into_iter()
            .flatten()
            .any(|address| {
                TcpStream::connect_timeout(&address, Duration::from_millis(*timeout_ms)).is_ok()
            }),
        ReadinessProbe::Http {
            url,
            timeout_ms,
            expected_status_min,
            expected_status_max,
            ..
        } => winhttp_status(url, *timeout_ms).is_some_and(|status| {
            status >= u32::from(*expected_status_min) && status <= u32::from(*expected_status_max)
        }),
    }
}

fn winhttp_status(url: &str, timeout_ms: u64) -> Option<u32> {
    let url = wide_string(url);
    let mut components = URL_COMPONENTS {
        dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
        dwSchemeLength: u32::MAX,
        dwHostNameLength: u32::MAX,
        dwUrlPathLength: u32::MAX,
        dwExtraInfoLength: u32::MAX,
        ..Default::default()
    };
    if unsafe {
        WinHttpCrackUrl(
            url.as_ptr(),
            u32::try_from(url.len().saturating_sub(1)).ok()?,
            0,
            &mut components,
        )
    } == 0
    {
        return None;
    }
    let host = nul_terminated_component(components.lpszHostName, components.dwHostNameLength);
    let mut object = component(components.lpszUrlPath, components.dwUrlPathLength);
    object.extend(component(
        components.lpszExtraInfo,
        components.dwExtraInfoLength,
    ));
    if object.is_empty() {
        object.push('/' as u16);
    }
    object.push(0);
    let agent = wide_string("PersistentRuntimeHost/0.1");
    let get = wide_string("GET");
    let session = WinHttpOwned::new(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        )
    })?;
    let timeout = i32::try_from(timeout_ms.clamp(1, i32::MAX as u64)).ok()?;
    if unsafe { WinHttpSetTimeouts(session.raw(), timeout, timeout, timeout, timeout) } == 0 {
        return None;
    }
    let connection = WinHttpOwned::new(unsafe {
        WinHttpConnect(session.raw(), host.as_ptr(), components.nPort, 0)
    })?;
    let flags = if components.nScheme == WINHTTP_INTERNET_SCHEME_HTTPS {
        WINHTTP_FLAG_SECURE
    } else {
        0
    };
    let request = WinHttpOwned::new(unsafe {
        WinHttpOpenRequest(
            connection.raw(),
            get.as_ptr(),
            object.as_ptr(),
            null(),
            null(),
            null(),
            flags,
        )
    })?;
    if unsafe { WinHttpSendRequest(request.raw(), null(), 0, null(), 0, 0, 0) } == 0
        || unsafe { WinHttpReceiveResponse(request.raw(), null_mut()) } == 0
    {
        return None;
    }
    let mut status = 0_u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    if unsafe {
        WinHttpQueryHeaders(
            request.raw(),
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut bytes,
            null_mut(),
        )
    } == 0
    {
        None
    } else {
        Some(status)
    }
}

fn component(pointer: *const u16, length: u32) -> Vec<u16> {
    if pointer.is_null() || length == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(pointer, length as usize) }.to_vec()
    }
}

fn nul_terminated_component(pointer: *const u16, length: u32) -> Vec<u16> {
    let mut value = component(pointer, length);
    value.push(0);
    value
}

fn wide_string(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

fn backend_last_error(context: &str) -> ProcessBackendError {
    let error = std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32);
    ProcessBackendError::new(format!("{context}: {error}"))
}

fn process_error(error: std::io::Error) -> ProcessBackendError {
    ProcessBackendError::new(error.to_string())
}

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE) -> std::io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(std::io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ))
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn into_raw(self) -> HANDLE {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct WinHttpOwned(*mut core::ffi::c_void);

impl WinHttpOwned {
    fn new(handle: *mut core::ffi::c_void) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }

    fn raw(&self) -> *mut core::ffi::c_void {
        self.0
    }
}

impl Drop for WinHttpOwned {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}
