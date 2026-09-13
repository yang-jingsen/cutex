#![cfg(target_os = "windows")]

use crate::windows_security::OwnerSecurityDescriptor;
use std::io::{self, Read, Write};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_BROKEN_PIPE,
    ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, FALSE, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, TRUE,
    WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, SetNamedPipeHandleState, WaitNamedPipeW,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Overlapped, byte-mode named-pipe connection used by the unchanged v1 JSON
/// framing. Every I/O operation has an independently cancellable deadline.
pub struct WindowsPipeStream {
    handle: OwnedHandle,
    read_timeout_ms: AtomicU32,
    write_timeout_ms: AtomicU32,
}

unsafe impl Send for WindowsPipeStream {}

impl WindowsPipeStream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let name = wide_path(path);
        let deadline = Instant::now() + DEFAULT_IO_TIMEOUT;
        loop {
            // SAFETY: all pointers are valid for the duration of the call; the
            // resulting handle is validated and owned below.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | security_identification_flag(),
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                let stream = Self::from_handle(OwnedHandle::new(handle)?);
                let mode = PIPE_READMODE_BYTE;
                // SAFETY: the connected pipe handle and mode pointer are valid.
                if unsafe { SetNamedPipeHandleState(handle, &mode, null(), null()) } == 0 {
                    return Err(last_error());
                }
                return Ok(stream);
            }
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_BUSY {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for the PRH named pipe",
                ));
            }
            // SAFETY: the pipe name is NUL-terminated and remains live.
            if unsafe { WaitNamedPipeW(name.as_ptr(), duration_millis(remaining)) } == 0 {
                let wait_error = unsafe { GetLastError() };
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out waiting for the PRH named pipe",
                    ));
                }
                if wait_error != ERROR_PIPE_BUSY {
                    return Err(io::Error::from_raw_os_error(wait_error as i32));
                }
            }
        }
    }

    fn from_handle(handle: OwnedHandle) -> Self {
        Self {
            handle,
            read_timeout_ms: AtomicU32::new(duration_millis(DEFAULT_IO_TIMEOUT)),
            write_timeout_ms: AtomicU32::new(duration_millis(DEFAULT_IO_TIMEOUT)),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = null_mut();
        // SAFETY: source and target are the current process, the output pointer
        // is valid, and inheritance is deliberately disabled.
        if unsafe {
            DuplicateHandle(
                process,
                self.handle.raw(),
                process,
                &mut duplicate,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Self {
            handle: OwnedHandle::new(duplicate)?,
            read_timeout_ms: AtomicU32::new(self.read_timeout_ms.load(Ordering::Relaxed)),
            write_timeout_ms: AtomicU32::new(self.write_timeout_ms.load(Ordering::Relaxed)),
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout_ms
            .store(optional_duration_millis(timeout), Ordering::Relaxed);
        Ok(())
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout_ms
            .store(optional_duration_millis(timeout), Ordering::Relaxed);
        Ok(())
    }
}

impl Read for WindowsPipeStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = u32::try_from(buffer.len().min(u32::MAX as usize)).unwrap_or(u32::MAX);
        match overlapped_io(
            self.handle.raw(),
            self.read_timeout_ms.load(Ordering::Relaxed),
            |overlapped| unsafe {
                ReadFile(
                    self.handle.raw(),
                    buffer.as_mut_ptr(),
                    length,
                    null_mut(),
                    overlapped,
                )
            },
        ) {
            Ok(bytes) => Ok(bytes as usize),
            Err(error)
                if matches!(
                    error.raw_os_error().map(|value| value as u32),
                    Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA)
                ) =>
            {
                Ok(0)
            }
            Err(error) => Err(error),
        }
    }
}

impl Write for WindowsPipeStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = u32::try_from(buffer.len().min(u32::MAX as usize)).unwrap_or(u32::MAX);
        overlapped_io(
            self.handle.raw(),
            self.write_timeout_ms.load(Ordering::Relaxed),
            |overlapped| unsafe {
                WriteFile(
                    self.handle.raw(),
                    buffer.as_ptr(),
                    length,
                    null_mut(),
                    overlapped,
                )
            },
        )
        .map(|bytes| bytes as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct WindowsPipeListener {
    name: Vec<u16>,
    security: OwnerSecurityDescriptor,
}

impl WindowsPipeListener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        Ok(Self {
            name: wide_path(path),
            security: OwnerSecurityDescriptor::new(false)?,
        })
    }

    /// Wait for one connection while periodically consulting the host's stop
    /// predicate. `None` means the host requested shutdown before a client
    /// connected.
    pub fn accept_while(
        &self,
        mut keep_waiting: impl FnMut() -> bool,
    ) -> io::Result<Option<WindowsPipeStream>> {
        let attributes = self.security.attributes();
        // SAFETY: the name and security descriptor remain live while the pipe
        // instance is created. The handle is validated immediately.
        let pipe = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                &attributes,
            )
        };
        let pipe = OwnedHandle::new(pipe)?;
        let event = OwnedHandle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        // SAFETY: pipe and OVERLAPPED stay live until connection completion or
        // cancellation below.
        let connected = unsafe { ConnectNamedPipe(pipe.raw(), &mut overlapped) };
        if connected == 0 {
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => return Ok(Some(WindowsPipeStream::from_handle(pipe))),
                ERROR_IO_PENDING => {}
                code => return Err(io::Error::from_raw_os_error(code as i32)),
            }
        } else {
            return Ok(Some(WindowsPipeStream::from_handle(pipe)));
        }

        loop {
            let wait = unsafe { WaitForSingleObject(event.raw(), 50) };
            if wait == WAIT_OBJECT_0 {
                let mut transferred = 0;
                if unsafe { GetOverlappedResult(pipe.raw(), &overlapped, &mut transferred, FALSE) }
                    == 0
                {
                    let error = unsafe { GetLastError() };
                    if error != ERROR_PIPE_CONNECTED {
                        return Err(io::Error::from_raw_os_error(error as i32));
                    }
                }
                return Ok(Some(WindowsPipeStream::from_handle(pipe)));
            }
            if wait == WAIT_FAILED {
                let error = last_error();
                cancel_overlapped(pipe.raw(), &overlapped);
                return Err(error);
            }
            if wait != WAIT_TIMEOUT {
                cancel_overlapped(pipe.raw(), &overlapped);
                return Err(io::Error::other("unexpected named-pipe wait result"));
            }
            if !keep_waiting() {
                cancel_overlapped(pipe.raw(), &overlapped);
                return Ok(None);
            }
        }
    }
}

fn overlapped_io(
    handle: HANDLE,
    timeout_ms: u32,
    begin: impl FnOnce(*mut OVERLAPPED) -> i32,
) -> io::Result<u32> {
    let event = OwnedHandle::new(unsafe { CreateEventW(null(), 1, 0, null()) })?;
    let mut overlapped = OVERLAPPED {
        hEvent: event.raw(),
        ..Default::default()
    };
    let started = begin(&mut overlapped);
    if started == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }
    let wait = unsafe { WaitForSingleObject(event.raw(), timeout_ms) };
    if wait == WAIT_TIMEOUT {
        cancel_overlapped(handle, &overlapped);
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "named-pipe I/O timed out",
        ));
    }
    if wait == WAIT_FAILED {
        let error = last_error();
        cancel_overlapped(handle, &overlapped);
        return Err(error);
    }
    if wait != WAIT_OBJECT_0 {
        cancel_overlapped(handle, &overlapped);
        return Err(io::Error::other("unexpected named-pipe I/O wait result"));
    }
    let mut transferred = 0;
    if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, FALSE) } == 0 {
        let error = unsafe { GetLastError() };
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    Ok(transferred)
}

fn cancel_overlapped(handle: HANDLE, overlapped: &OVERLAPPED) {
    // SAFETY: the operation belongs to this handle and OVERLAPPED. Waiting for
    // cancellation completion keeps the stack OVERLAPPED live until the kernel
    // no longer references it.
    unsafe {
        let _ = CancelIoEx(handle, overlapped);
        let mut ignored = 0;
        if GetOverlappedResult(handle, overlapped, &mut ignored, TRUE) == 0 {
            debug_assert!(matches!(
                GetLastError(),
                ERROR_OPERATION_ABORTED | ERROR_BROKEN_PIPE
            ));
        }
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn optional_duration_millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(u32::MAX, duration_millis)
}

fn duration_millis(duration: Duration) -> u32 {
    duration.as_millis().clamp(1, u128::from(u32::MAX - 1)) as u32
}

fn security_identification_flag() -> u32 {
    SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn new(handle: HANDLE) -> io::Result<Self> {
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            Err(last_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
