//! Secure first-instance creation primitive for the local desktop/service pipe.
//! This does not connect clients or authenticate the connected process.

#[cfg(windows)]
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};

pub const PIPE_NAME: &str = r"\\.\pipe\WebDynamite.Core.v1";
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_DEFAULT_TIMEOUT_MS: u32 = 5_000;

fn valid_service_sid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    parts.len() == 9 && parts[0] == "S" && parts[1] == "1" && parts[2] == "5" && parts[3] == "80"
        && parts[4..].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_user_sid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    let recognized_user_authority = (parts.get(2) == Some(&"5") && parts.get(3) == Some(&"21"))
        || (parts.get(2) == Some(&"12") && parts.get(3) == Some(&"1"));
    parts.len() == 8 && parts[0] == "S" && parts[1] == "1" && recognized_user_authority
        && parts[4..].iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(windows)]
pub struct NamedPipeHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl NamedPipeHandle {
    pub fn as_raw_handle(&self) -> windows_sys::Win32::Foundation::HANDLE { self.0 }
}

#[cfg(windows)]
impl Drop for NamedPipeHandle {
    fn drop(&mut self) { unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0); } }
}

/// Creates the protected first instance. The caller must still authenticate
/// the connected process before reading or dispatching any request.
#[cfg(windows)]
pub fn create_server_pipe(service_sid: &str, client_user_sid: &str) -> Result<NamedPipeHandle, String> {
    create_server_pipe_with_mode(service_sid, client_user_sid, false)
}

/// Creates the same protected first-instance pipe with overlapped I/O enabled.
/// Every operation on this handle must supply its own OVERLAPPED structure.
#[cfg(windows)]
pub fn create_overlapped_server_pipe(service_sid: &str, client_user_sid: &str) -> Result<NamedPipeHandle, String> {
    create_server_pipe_with_mode(service_sid, client_user_sid, true)
}

#[cfg(windows)]
fn create_server_pipe_with_mode(service_sid: &str, client_user_sid: &str, overlapped: bool) -> Result<NamedPipeHandle, String> {
    use windows_sys::Win32::{
        Foundation::{GetLastError, LocalFree, INVALID_HANDLE_VALUE},
        Security::{Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES},
        Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
        System::Pipes::{CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT},
    };
    if !valid_service_sid(service_sid) { return Err("A valid WebDynamite service SID is required.".into()); }
    if !valid_user_sid(client_user_sid) { return Err("A valid interactive user SID is required.".into()); }

    let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{service_sid})(A;;GRGW;;;{client_user_sid})");
    let wide_sddl = OsStr::new(&sddl).encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let mut descriptor = ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(wide_sddl.as_ptr(), 1, &mut descriptor, ptr::null_mut())
    };
    if converted == 0 {
        return Err(format!("Could not build named-pipe security descriptor (Windows error {}).", unsafe { GetLastError() }));
    }

    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let wide_name = OsStr::new(PIPE_NAME).encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let handle = unsafe {
        CreateNamedPipeW(
            wide_name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | if overlapped { windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED } else { 0 },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1, PIPE_BUFFER_BYTES, PIPE_BUFFER_BYTES, PIPE_DEFAULT_TIMEOUT_MS, &mut attributes,
        )
    };
    let create_error = if handle == INVALID_HANDLE_VALUE { Some(unsafe { GetLastError() }) } else { None };
    unsafe { LocalFree(descriptor); }
    if let Some(error) = create_error {
        return Err(format!("Could not create protected WebDynamite named pipe (Windows error {error})."));
    }
    Ok(NamedPipeHandle(handle))
}

#[cfg(not(windows))]
pub fn create_server_pipe(_service_sid: &str, _client_user_sid: &str) -> Result<(), String> {
    Err("The WebDynamite named-pipe server is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::{valid_service_sid, valid_user_sid, PIPE_NAME};

    #[test]
    fn pipe_endpoint_matches_the_desktop_ipc_contract() {
        assert_eq!(PIPE_NAME, r"\\.\pipe\WebDynamite.Core.v1");
    }

    #[test]
    fn validates_service_and_interactive_user_sid_shapes() {
        assert!(valid_service_sid("S-1-5-80-123-456-789-101112-131415"));
        assert!(!valid_service_sid("S-1-5-21-123-456-789-1001"));
        assert!(valid_user_sid("S-1-5-21-123-456-789-1001"));
        assert!(valid_user_sid("S-1-12-1-123-456-789-1001"));
        assert!(!valid_user_sid("S-1-5-80-123-456-789-101112-131415"));
        assert!(!valid_user_sid("S-1-5-21-123-x-789-1001"));
        assert!(!valid_user_sid("S-1-5-21-123-456-789-1001;OICI"));
        assert!(!valid_user_sid("S-1-5-32-544"));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_reports_unsupported_platform() {
        assert!(super::create_server_pipe(
            "S-1-5-80-123-456-789-101112-131415", "S-1-5-21-123-456-789-1001"
        ).unwrap_err().contains("only on Windows"));
    }

    #[cfg(windows)]
    #[test]
    fn creates_and_closes_protected_first_pipe_instance() {
        let pipe = super::create_server_pipe(
            "S-1-5-80-123-456-789-101112-131415", "S-1-5-21-123-456-789-1001"
        ).unwrap();
        assert!(!pipe.as_raw_handle().is_null());
        drop(pipe);
    }
}
