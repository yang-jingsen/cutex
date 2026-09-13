#![cfg(target_os = "windows")]

use std::io;
use std::path::Path;
use std::ptr::null_mut;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, IsValidSid, SetFileSecurityW, TokenOwner, TokenUser,
    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

static OPERATOR_SID: OnceLock<String> = OnceLock::new();

pub(crate) struct OwnerSecurityDescriptor {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl OwnerSecurityDescriptor {
    pub(crate) fn new(inherit_to_children: bool) -> io::Result<Self> {
        let owner_sid = current_user_sid_string()?;
        let operator_sid = configured_operator_sid().unwrap_or_else(|| owner_sid.clone());
        let inheritance = if inherit_to_children { "OICI" } else { "" };
        let rights = if inherit_to_children { "FA" } else { "GA" };
        let sddl = wide_string(&format!(
            "O:{owner_sid}D:P(A;{inheritance};{rights};;;SY)(A;{inheritance};{rights};;;{operator_sid})"
        ));
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Self { descriptor })
    }

    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }

    fn raw(&self) -> PSECURITY_DESCRIPTOR {
        self.descriptor
    }
}

impl Drop for OwnerSecurityDescriptor {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe {
                let _ = LocalFree(self.descriptor);
            }
        }
    }
}

pub(crate) fn secure_path_for_current_user(
    path: &Path,
    inherit_to_children: bool,
) -> io::Result<()> {
    verify_path_owned_by_current_user(path)?;
    let descriptor = OwnerSecurityDescriptor::new(inherit_to_children)?;
    let path = wide_path(path);
    if unsafe {
        SetFileSecurityW(
            path.as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.raw(),
        )
    } == 0
    {
        Err(last_error())
    } else {
        Ok(())
    }
}

fn verify_path_owned_by_current_user(path: &Path) -> io::Result<()> {
    let path = wide_path(path);
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = OwnedLocal(descriptor);
    let token_buffer = current_user_token_buffer()?;
    let token_user = unsafe { &*(token_buffer.as_ptr().cast::<TOKEN_USER>()) };
    let token_owner_buffer = current_token_owner_buffer()?;
    let token_owner = unsafe { &*(token_owner_buffer.as_ptr().cast::<TOKEN_OWNER>()) };
    // Elevated administrator tokens commonly create files with their default
    // owner group rather than the token user. Accept only those two token-bound
    // identities, then `secure_path_for_current_user` replaces the owner with
    // the user SID while applying the protected DACL.
    let configured_operator = configured_operator_sid()
        .map(|sid| ParsedSid::new(&sid))
        .transpose()?;
    let owned_by_operator = !owner.is_null()
        && configured_operator
            .as_ref()
            .is_some_and(|sid| unsafe { EqualSid(owner, sid.raw()) } != 0);
    // In service mode LocalSystem is the process token and becomes the owner
    // when it reapplies the protected operator+SYSTEM descriptor. A later
    // elevated upgrade by that same configured operator must accept that one
    // additional trusted owner; foreground mode does not broaden its check.
    let owned_by_system = if configured_operator.is_some() && !owner.is_null() {
        let system = ParsedSid::new("S-1-5-18")?;
        (unsafe { EqualSid(owner, system.raw()) }) != 0
    } else {
        false
    };
    if !owner.is_null()
        && (unsafe { EqualSid(owner, token_user.User.Sid) } != 0
            || unsafe { EqualSid(owner, token_owner.Owner) } != 0
            || owned_by_operator
            || owned_by_system)
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "PRH local state path is owned by another Windows account",
        ))
    }
}

/// Fixes the non-SYSTEM principal used by all subsequently created Windows
/// state objects and named pipes in this process. Service mode calls this once
/// before touching its state directory; foreground mode retains the current
/// token user behavior.
pub fn configure_operator_sid(sid: &str) -> io::Result<()> {
    let canonical = canonical_sid_string(sid)?;
    if let Some(existing) = OPERATOR_SID.get() {
        if existing == &canonical {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the PRH Windows operator SID was already configured differently",
        ));
    }
    OPERATOR_SID.set(canonical).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the PRH Windows operator SID was already configured",
        )
    })
}

pub fn current_user_sid_string() -> io::Result<String> {
    let buffer = current_user_token_buffer()?;
    let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    sid_to_string(token_user.User.Sid)
}

fn configured_operator_sid() -> Option<String> {
    OPERATOR_SID.get().cloned()
}

pub(crate) fn refuse_reparse_point(path: &Path, description: &str) -> io::Result<()> {
    let path = wide_path(path);
    let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
    if attributes == u32::MAX {
        return Err(last_error());
    }
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{description} must not be a Windows reparse point"),
        ))
    } else {
        Ok(())
    }
}

fn sid_to_string(sid: PSID) -> io::Result<String> {
    let mut sid_text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 {
        return Err(last_error());
    }
    let length = wide_pointer_length(sid_text);
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    unsafe {
        let _ = LocalFree(sid_text.cast());
    }
    sid
}

fn canonical_sid_string(value: &str) -> io::Result<String> {
    if value.is_empty() || value.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "operator SID must be a non-empty Windows SID string",
        ));
    }
    let parsed = ParsedSid::new(value)?;
    sid_to_string(parsed.raw())
}

fn current_user_token_buffer() -> io::Result<Vec<usize>> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = OwnedHandle::new(token)?;
    let mut required = 0;
    unsafe {
        let _ = GetTokenInformation(token.raw(), TokenUser, null_mut(), 0, &mut required);
    }
    if required == 0 {
        return Err(last_error());
    }
    let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(buffer)
}

fn current_token_owner_buffer() -> io::Result<Vec<usize>> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = OwnedHandle::new(token)?;
    let mut required = 0;
    unsafe {
        let _ = GetTokenInformation(token.raw(), TokenOwner, null_mut(), 0, &mut required);
    }
    if required == 0 {
        return Err(last_error());
    }
    let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenOwner,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(last_error());
    }
    Ok(buffer)
}

fn wide_pointer_length(pointer: *const u16) -> usize {
    let mut length = 0;
    unsafe {
        while *pointer.add(length) != 0 {
            length += 1;
        }
    }
    length
}

fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn last_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

struct OwnedHandle(HANDLE);

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

struct OwnedLocal(PSECURITY_DESCRIPTOR);

impl Drop for OwnedLocal {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = LocalFree(self.0);
            }
        }
    }
}

struct ParsedSid(PSID);

impl ParsedSid {
    fn new(value: &str) -> io::Result<Self> {
        let value = wide_string(value);
        let mut sid = null_mut();
        if unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut sid) } == 0 {
            return Err(last_error());
        }
        if unsafe { IsValidSid(sid) } == 0 {
            unsafe {
                let _ = LocalFree(sid.cast());
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "operator SID is not valid",
            ));
        }
        Ok(Self(sid))
    }

    fn raw(&self) -> PSID {
        self.0
    }
}

impl Drop for ParsedSid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = LocalFree(self.0.cast());
            }
        }
    }
}
