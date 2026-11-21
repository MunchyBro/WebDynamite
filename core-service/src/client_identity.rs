//! Windows named-pipe peer verification. This module deliberately fails closed:
//! callers must supply the expected interactive SID, session, exact installed
//! desktop executable path, and pinned Authenticode signer SHA-256 thumbprint.

#[cfg(windows)]
mod windows_impl {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::{Path, PathBuf}, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree, HANDLE, STILL_ACTIVE},
        Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenSessionId, TokenUser},
        Security::Authorization::ConvertSidToStringSidW,
        System::{
            Pipes::{GetNamedPipeClientProcessId, GetNamedPipeClientSessionId},
            Threading::{GetExitCodeProcess, GetProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    };

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
    }

    /// Keeps the exact process object opened during verification alive while the
    /// pipe connection is authorized. Re-check before each sensitive dispatch.
    pub struct VerifiedClient {
        process: OwnedHandle,
        pid: u32,
        session_id: u32,
        executable: PathBuf,
        user_sid: String,
        publisher_sha256: String,
    }

    impl VerifiedClient {
        pub fn pid(&self) -> u32 { self.pid }
        pub fn session_id(&self) -> u32 { self.session_id }
        pub fn executable(&self) -> &Path { &self.executable }
        pub fn user_sid(&self) -> &str { &self.user_sid }
        pub fn publisher_sha256(&self) -> &str { &self.publisher_sha256 }

        pub fn verify_still_connected(&self, pipe: HANDLE) -> Result<(), String> {
            let mut pipe_pid = 0u32;
            let mut pipe_session = 0u32;
            let pid_ok = unsafe { GetNamedPipeClientProcessId(pipe, &mut pipe_pid) };
            let session_ok = unsafe { GetNamedPipeClientSessionId(pipe, &mut pipe_session) };
            let mut exit_code = 0u32;
            let alive = unsafe { GetExitCodeProcess(self.process.0, &mut exit_code) };
            let handle_pid = unsafe { GetProcessId(self.process.0) };
            if pid_ok == 0 || session_ok == 0 || alive == 0
                || pipe_pid != self.pid || handle_pid != self.pid
                || pipe_session != self.session_id || exit_code != STILL_ACTIVE
            {
                return Err("The verified desktop client is no longer connected.".into());
            }
            Ok(())
        }
    }

    fn token_sid(token: HANDLE) -> Result<String, String> {
        let mut required = 0u32;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut required);
        }
        if required == 0 || required > 64 * 1024 {
            return Err("Could not determine the desktop process user token.".into());
        }
        // u64 storage gives the TOKEN_USER structure its required alignment.
        let mut storage = vec![0u64; (required as usize + 7) / 8];
        let ok = unsafe {
            GetTokenInformation(
                token, TokenUser, storage.as_mut_ptr().cast(),
                (storage.len() * 8) as u32, &mut required,
            )
        };
        if ok == 0 { return Err("Could not read the desktop process user SID.".into()); }
        let user = unsafe { &*(storage.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_text = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_text) } == 0 {
            return Err("Could not format the desktop process user SID.".into());
        }
        let mut length = 0usize;
        unsafe {
            while *sid_text.add(length) != 0 { length += 1; }
        }
        let result = String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
            .map_err(|_| "The desktop process user SID is malformed.".to_string());
        unsafe { LocalFree(sid_text.cast()); }
        result
    }

    fn token_session(token: HANDLE) -> Result<u32, String> {
        let mut session_id = 0u32;
        let mut returned = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token, TokenSessionId, (&mut session_id as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32, &mut returned,
            )
        };
        if ok == 0 || returned < std::mem::size_of::<u32>() as u32 {
            return Err("Could not read the desktop process session ID.".into());
        }
        Ok(session_id)
    }

    fn process_image_path(process: HANDLE) -> Result<PathBuf, String> {
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length)
        };
        if ok == 0 || length == 0 || length as usize > buffer.len() {
            return Err("Could not read the desktop process executable path.".into());
        }
        Ok(PathBuf::from(String::from_utf16(&buffer[..length as usize])
            .map_err(|_| "The desktop process executable path is invalid.".to_string())?))
    }

    fn normalize_thumbprint(value: &str) -> Result<String, String> {
        let normalized = value.trim().replace(':', "").replace(' ', "").to_ascii_lowercase();
        if normalized.len() != 64 || !normalized.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("A pinned publisher SHA-256 thumbprint is required.".into());
        }
        Ok(normalized)
    }

    pub fn verify_connected_client(
        pipe: HANDLE,
        expected_user_sid: &str,
        expected_session_id: u32,
        expected_executable: &Path,
        expected_publisher_sha256: &str,
    ) -> Result<VerifiedClient, String> {
        let expected_thumbprint = normalize_thumbprint(expected_publisher_sha256)?;
        let expected_path = expected_executable.canonicalize()
            .map_err(|_| "The installed WebDynamite desktop executable is unavailable.".to_string())?;

        let mut pid = 0u32;
        if unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) } == 0 || pid == 0 {
            return Err("Could not identify the named-pipe client process.".into());
        }
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err("Could not open the named-pipe client process for verification.".into());
        }
        let process = OwnedHandle(process);
        if unsafe { GetProcessId(process.0) } != pid {
            return Err("The named-pipe client process identity changed during verification.".into());
        }

        // Re-read the pipe PID after opening the process handle to reject a client
        // that disconnected or changed while the process object was being acquired.
        let mut confirmed_pid = 0u32;
        if unsafe { GetNamedPipeClientProcessId(pipe, &mut confirmed_pid) } == 0 || confirmed_pid != pid {
            return Err("The named-pipe client disconnected during verification.".into());
        }

        let mut token = ptr::null_mut();
        if unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
            return Err("Could not open the named-pipe client access token.".into());
        }
        let token = OwnedHandle(token);
        let actual_sid = token_sid(token.0)?;
        if !actual_sid.eq_ignore_ascii_case(expected_user_sid) {
            return Err("The named-pipe client is running under an unexpected Windows user.".into());
        }
        let actual_session = token_session(token.0)?;
        let mut pipe_session = 0u32;
        if unsafe { GetNamedPipeClientSessionId(pipe, &mut pipe_session) } == 0
            || actual_session != expected_session_id || pipe_session != expected_session_id
        {
            return Err("The named-pipe client is running in an unexpected Windows session.".into());
        }

        let actual_path = process_image_path(process.0)?.canonicalize()
            .map_err(|_| "The named-pipe client executable path cannot be canonicalized.".to_string())?;
        if !actual_path.to_string_lossy().eq_ignore_ascii_case(&expected_path.to_string_lossy()) {
            return Err("The named-pipe client is not the installed WebDynamite desktop executable.".into());
        }

        // The installer must protect this exact executable path from modification
        // by unprivileged users. Signature verification is path-based by design.
        let signature = verifysign::verify_file(&actual_path.to_string_lossy())
            .map_err(|_| "The WebDynamite desktop executable has no valid trusted Authenticode signature.".to_string())?;
        let signer = signature.sha256_thumbprint().to_ascii_lowercase();
        if signer != expected_thumbprint {
            return Err("The WebDynamite desktop executable is signed by an unexpected publisher.".into());
        }

        let mut exit_code = 0u32;
        if unsafe { GetExitCodeProcess(process.0, &mut exit_code) } == 0 || exit_code != STILL_ACTIVE {
            return Err("The named-pipe client exited during verification.".into());
        }

        Ok(VerifiedClient {
            process, pid, session_id: actual_session, executable: actual_path,
            user_sid: actual_sid, publisher_sha256: signer,
        })
    }
}

#[cfg(windows)]
pub use windows_impl::{verify_connected_client, VerifiedClient};

#[cfg(not(windows))]
pub struct VerifiedClient;

#[cfg(not(windows))]
pub fn verify_connected_client(
    _pipe: *mut std::ffi::c_void,
    _expected_user_sid: &str,
    _expected_session_id: u32,
    _expected_executable: &std::path::Path,
    _expected_publisher_sha256: &str,
) -> Result<VerifiedClient, String> {
    Err("Desktop client identity verification is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn publisher_thumbprints_must_be_sha256_hex() {
        #[cfg(windows)]
        {
            assert!(super::windows_impl::normalize_thumbprint(&"a".repeat(64)).is_ok());
            assert!(super::windows_impl::normalize_thumbprint(&"g".repeat(64)).is_err());
            assert!(super::windows_impl::normalize_thumbprint(&"a".repeat(40)).is_err());
        }
    }
}
