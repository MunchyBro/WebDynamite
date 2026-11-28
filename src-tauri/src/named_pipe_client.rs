//! Authenticated, bounded desktop client for the Core Service named pipe.
//!
//! Tauri uses this transport for the signed desktop's API-token bootstrap and
//! narrowly allowlisted secret operations. Callers must pass trusted executable,
//! publisher-pin and service-SID values; this module never reads them from webview
//! input, runtime environment variables, or ordinary user-writable settings.
use crate::ipc_protocol::{read_response_for_request, write_request, IpcRequest, IpcResponse, PROTOCOL_VERSION};
use serde_json::Value;
use std::{path::Path, time::{Duration, Instant}};
use zeroize::Zeroize;

const PIPE_NAME: &str = r"\\.\pipe\WebDynamite.Core.v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::{io, path::PathBuf, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE, ERROR_IO_PENDING, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Security::{CheckTokenMembership, TOKEN_QUERY},
        Security::Authorization::ConvertStringSidToSidW,
        Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING},
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::{GetNamedPipeServerProcessId, WaitNamedPipeW},
            Threading::{CreateEventW, GetProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE},
        },
    };

    struct OwnedHandle(HANDLE);
    unsafe impl Send for OwnedHandle {}
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 as isize != -1 {
                unsafe { CloseHandle(self.0); }
            }
        }
    }

    struct VerifiedServer {
        process: OwnedHandle,
        pid: u32,
    }

    impl VerifiedServer {
        fn verify_still_connected(&self, pipe: HANDLE) -> Result<(), String> {
            let mut current_pid = 0u32;
            let pipe_ok = unsafe { GetNamedPipeServerProcessId(pipe, &mut current_pid) };
            let process_pid = unsafe { GetProcessId(self.process.0) };
            let process_state = unsafe { WaitForSingleObject(self.process.0, 0) };
            if pipe_ok == 0 || current_pid != self.pid || process_pid != self.pid
                || process_state != WAIT_TIMEOUT
            {
                return Err("The authenticated Core Service process is no longer connected.".into());
            }
            Ok(())
        }
    }

    pub(super) fn validate_service_sid(value: &str) -> Result<String, String> {
    let sid = value.trim();
    let parts = sid.split('-').collect::<Vec<_>>();
    if parts.len() != 9 || parts[0] != "S" || parts[1] != "1" || parts[2] != "5" || parts[3] != "80"
        || parts[4..].iter().any(|part| part.is_empty() || part.parse::<u32>().is_err())
    {
        return Err("A trusted installer-provisioned Windows service SID is required.".into());
    }
    Ok(sid.to_string())
}

fn normalize_thumbprint(value: &str) -> Result<String, String> {
        let normalized = value.trim().replace(':', "").replace(' ', "").to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("A trusted installer-provisioned SHA-256 publisher thumbprint is required.".into());
        }
        Ok(normalized)
    }

    fn authenticate_server(pipe: HANDLE, expected_executable: &Path, expected_thumbprint: &str, expected_service_sid: &str) -> Result<VerifiedServer, String> {
        let expected_pin = normalize_thumbprint(expected_thumbprint)?;
        let expected_service_sid = validate_service_sid(expected_service_sid)?;

        let expected_path = expected_executable.canonicalize()
            .map_err(|_| "The installed Core Service executable path is unavailable.".to_string())?;
        let mut pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 || pid == 0 {
            return Err("Could not identify the named-pipe Core Service process.".into());
        }
        let process = unsafe {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid)
        };
        if process.is_null() {
            return Err("Could not open the Core Service process for identity verification.".into());
        }
        let process = OwnedHandle(process);
        if unsafe { GetProcessId(process.0) } != pid {
            return Err("The Core Service process identity changed during verification.".into());
        }

        let mut confirmed_pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut confirmed_pid) } == 0 || confirmed_pid != pid {
            return Err("The Core Service disconnected during identity verification.".into());
        }

        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length) } == 0
            || length == 0 || length as usize > buffer.len()
        {
            return Err("Could not read the Core Service executable path.".into());
        }
        let actual_path = PathBuf::from(String::from_utf16(&buffer[..length as usize])
            .map_err(|_| "The Core Service executable path is invalid.".to_string())?)
            .canonicalize()
            .map_err(|_| "The Core Service executable path cannot be canonicalized.".to_string())?;
        if !actual_path.to_string_lossy().eq_ignore_ascii_case(&expected_path.to_string_lossy()) {
            return Err("The named-pipe server is not the installed WebDynamite Core Service.".into());
        }

        // Require the configured service SID to be enabled in the server token.
        // A matching executable path and publisher alone do not prove SCM service identity.
        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
            return Err("Could not open the Core Service process token for service identity verification.".into());
        }
        let token = OwnedHandle(token);
        let wide_sid = expected_service_sid.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
        let mut expected_sid = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(wide_sid.as_ptr(), &mut expected_sid) } == 0 || expected_sid.is_null() {
            return Err("The installer-provisioned Core Service SID is invalid.".into());
        }
        let mut is_service_member = 0;
        let membership_ok = unsafe { CheckTokenMembership(token.0, expected_sid, &mut is_service_member) };
        unsafe { LocalFree(expected_sid); }
        if membership_ok == 0 || is_service_member == 0 {
            return Err("The named-pipe server is not running with the expected WebDynamite service SID.".into());
        }

        let verifier = verifysign::CodeSignVerifier::new(
            verifysign::VerifyPolicy::new()
                .revocation(verifysign::Revocation::Chain)
                .allow_network(false),
        ).map_err(|_| "Could not initialize Authenticode verification policy.".to_string())?;
        let signature = verifier.verify_file(&actual_path)
            .map_err(|_| "The Core Service executable has no valid trusted Authenticode signature.".to_string())?;
        if signature.sha256_thumbprint().to_ascii_lowercase() != expected_pin {
            return Err("The Core Service executable is signed by an unexpected publisher.".into());
        }

        if unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT {
            return Err("The Core Service exited during identity verification.".into());
        }
        Ok(VerifiedServer { process, pid })
    }

    struct PipeIo {
        pipe: HANDLE,
        deadline: Instant,
    }

    impl PipeIo {
        fn new(pipe: HANDLE, timeout: Duration) -> Self {
            Self { pipe, deadline: Instant::now() + timeout }
        }

        fn set_deadline(&mut self, timeout: Duration) {
            self.deadline = Instant::now() + timeout;
        }

        fn transfer(&self, buffer: *mut u8, length: u32, writing: bool) -> io::Result<usize> {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            if event.is_null() { return Err(io::Error::last_os_error()); }
            let result = (|| {
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                overlapped.hEvent = event;
                let started = unsafe {
                    if writing {
                        WriteFile(self.pipe, buffer.cast_const().cast(), length, ptr::null_mut(), &mut overlapped)
                    } else {
                        ReadFile(self.pipe, buffer.cast(), length, ptr::null_mut(), &mut overlapped)
                    }
                };
                if started == 0 {
                    let error = unsafe { GetLastError() };
                    if error != ERROR_IO_PENDING {
                        return Err(io::Error::from_raw_os_error(error as i32));
                    }
                    let remaining = self.deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        unsafe { CancelIoEx(self.pipe, &overlapped); }
                        let mut transferred = 0u32;
                        unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut transferred, 1); }
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "Named-pipe I/O exceeded its deadline."));
                    }
                    let wait_ms = remaining.as_millis().max(1).min(u32::MAX as u128) as u32;
                    let wait = unsafe { WaitForSingleObject(event, wait_ms) };
                    if wait == WAIT_TIMEOUT {
                        unsafe { CancelIoEx(self.pipe, &overlapped); }
                        let mut transferred = 0u32;
                        unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut transferred, 1); }
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "Named-pipe I/O timed out."));
                    }
                    if wait != WAIT_OBJECT_0 {
                        let wait_error = unsafe { GetLastError() };
                        unsafe { CancelIoEx(self.pipe, &overlapped); }
                        let mut transferred = 0u32;
                        unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut transferred, 1); }
                        return Err(io::Error::from_raw_os_error(wait_error as i32));
                    }
                }
                let mut transferred = 0u32;
                if unsafe { GetOverlappedResult(self.pipe, &overlapped, &mut transferred, 0) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(transferred as usize)
            })();
            unsafe { CloseHandle(event); }
            result
        }
    }

    impl std::io::Read for PipeIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() { return Ok(0); }
            self.transfer(buffer.as_mut_ptr(), buffer.len().min(u32::MAX as usize) as u32, false)
        }
    }

    impl std::io::Write for PipeIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer.is_empty() { return Ok(0); }
            self.transfer(buffer.as_ptr() as *mut u8, buffer.len().min(u32::MAX as usize) as u32, true)
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    fn wipe_sensitive_data(operation: &str, data: &mut Value) {
        let field = match operation {
            "secret.put" => Some("value"),
            "database-credential.put" => Some("password"),
            _ => None,
        };
        if let Some(field) = field {
            if let Some(serde_json::Value::String(value)) = data.get_mut(field) { value.zeroize(); }
        }
    }

    struct SensitiveDataGuard {
        operation: String,
        data: Option<Value>,
    }
    impl SensitiveDataGuard {
        fn new(operation: &str, data: Value) -> Self {
            Self { operation: operation.to_string(), data: Some(data) }
        }
        fn take(&mut self) -> Value { self.data.take().unwrap_or(Value::Null) }
    }
    impl Drop for SensitiveDataGuard {
        fn drop(&mut self) {
            if let Some(data) = self.data.as_mut() { wipe_sensitive_data(&self.operation, data); }
        }
    }

    struct SensitiveRequestGuard {
        operation: String,
        request: IpcRequest,
    }
    impl Drop for SensitiveRequestGuard {
        fn drop(&mut self) { wipe_sensitive_data(&self.operation, &mut self.request.data); }
    }

    pub fn exchange(operation: &str, data: Value, expected_executable: &Path, expected_thumbprint: &str, expected_service_sid: &str) -> Result<IpcResponse, String> {
        // Own a wipe guard before any validation or connection step can fail.
        let mut data = SensitiveDataGuard::new(operation, data);
        if expected_executable.as_os_str().is_empty() {
            return Err("A protected installed Core Service executable path is required.".into());
        }
        normalize_thumbprint(expected_thumbprint)?;
        validate_service_sid(expected_service_sid)?;

        let wide_name: Vec<u16> = super::PIPE_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe { WaitNamedPipeW(wide_name.as_ptr(), super::CONNECT_TIMEOUT.as_millis() as u32) } == 0 {
            return Err(format!("The WebDynamite Core Service pipe is unavailable: {}", io::Error::last_os_error()));
        }
        let pipe = unsafe {
            CreateFileW(
                wide_name.as_ptr(), 0xC0000000, 0, ptr::null(),
                OPEN_EXISTING, FILE_FLAG_OVERLAPPED, ptr::null_mut(),
            )
        };
        if pipe.is_null() || pipe as isize == -1 {
            return Err(format!("Could not open the WebDynamite Core Service pipe: {}", io::Error::last_os_error()));
        }
        let pipe = OwnedHandle(pipe);

        // Authenticate the server before any request bytes can be written.
        let verified = authenticate_server(pipe.0, expected_executable, expected_thumbprint, expected_service_sid)?;
        verified.verify_still_connected(pipe.0)?;

        let request_id = uuid::Uuid::new_v4().simple().to_string();
        let request = SensitiveRequestGuard {
            operation: operation.to_string(),
            request: IpcRequest {
                version: PROTOCOL_VERSION,
                request_id: request_id.clone(),
                operation: operation.to_string(),
                data: data.take(),
            },
        };
        let mut io = PipeIo::new(pipe.0, super::WRITE_TIMEOUT);
        let write_result = write_request(&mut io, &request.request);
        // The frame writer has already wiped its serialized copy; wipe the
        // in-memory request value immediately, even when the write failed.
        wipe_sensitive_data(operation, &mut request.request.data);
        write_result?;
        verified.verify_still_connected(pipe.0)?;
        io.set_deadline(super::READ_TIMEOUT);
        read_response_for_request(&mut io, &request_id)
    }
}

#[cfg(windows)]
pub use windows_impl::exchange;

/// Non-Windows builds intentionally have no fallback transport.
#[cfg(not(windows))]
pub fn exchange(_operation: &str, _data: Value, _expected_executable: &Path, _expected_thumbprint: &str, _expected_service_sid: &str) -> Result<IpcResponse, String> {
    Err("The authenticated WebDynamite named-pipe client is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn service_sid_validation_accepts_only_service_sid_authorities() {
        assert!(super::windows_impl::validate_service_sid("S-1-5-80-1-2-3-4-5").is_ok());
        assert!(super::windows_impl::validate_service_sid(" S-1-5-80-1-2-3-4-5 ").is_ok());
        assert!(super::windows_impl::validate_service_sid("S-1-5-21-1-2-3-1001").is_err());
        assert!(super::windows_impl::validate_service_sid("S-1-5-80-1-2").is_err());
        assert!(super::windows_impl::validate_service_sid("S-1-5-80-x-2-3-4-5").is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_transport_fails_closed_without_http_fallback() {
        let result = super::exchange("secret.status", serde_json::json!({}), std::path::Path::new(""), "");
        assert!(result.unwrap_err().contains("only on Windows"));
    }
}
