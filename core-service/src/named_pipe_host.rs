//! Authenticated one-connection named-pipe host primitive.
//!
//! The SCM-owned runtime supplies trusted installer/service configuration.
//! The peer is authenticated before the first byte is read, and its retained
//! process handle is revalidated immediately before dispatch. Each accepted
//! connection is bounded and the service runtime owns cancellation/shutdown.

#[cfg(windows)]
mod windows_impl {
    use std::{io::{self, Read, Write}, path::Path};
    use windows_sys::Win32::{
        Foundation::{GetLastError, HANDLE, ERROR_PIPE_CONNECTED},
        Storage::FileSystem::{ReadFile, WriteFile},
        System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe},
    };

    use zeroize::Zeroize;

    use crate::{
        client_identity::verify_connected_client,
        ipc_authorization::authorize_request,
        ipc_protocol::{read_request, write_response, IpcRequest, IpcResponse, PROTOCOL_VERSION},
        named_pipe::{create_server_pipe, create_overlapped_server_pipe},
        named_pipe_accept::connect_with_timeout,
        named_pipe_io::OverlappedPipeIo,
    };

    fn wipe_sensitive_response(operation: &str, response: &mut IpcResponse) {
        let field = match operation {
            "service.auth-token" => Some("token"),
            "database-credential.get" => Some("password"),
            _ => None,
        };
        if let Some(field) = field {
            if let Some(serde_json::Value::String(value)) = response.data.as_mut().and_then(|data| data.get_mut(field)) {
                value.zeroize();
            }
        }
    }

    fn rejected_request_response(mut request: IpcRequest, code: impl ToString) -> IpcResponse {
        let field = match request.operation.as_str() {
            "secret.put" => Some("value"),
            "database-credential.put" => Some("password"),
            _ => None,
        };
        if let Some(field) = field {
            if let Some(serde_json::Value::String(value)) = request.data.get_mut(field) { value.zeroize(); }
        }
        IpcResponse {
            version: PROTOCOL_VERSION,
            request_id: request.request_id.clone(),
            ok: false,
            data: None,
            error_code: Some(code.to_string()),
        }
    }

    struct PipeIo(HANDLE);

    impl Read for PipeIo {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if buffer.is_empty() { return Ok(0); }
            let requested = buffer.len().min(u32::MAX as usize) as u32;
            let mut read = 0u32;
            let ok = unsafe {
                ReadFile(self.0, buffer.as_mut_ptr().cast(), requested, &mut read, std::ptr::null_mut())
            };
            if ok == 0 { return Err(io::Error::last_os_error()); }
            Ok(read as usize)
        }
    }

    impl Write for PipeIo {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer.is_empty() { return Ok(0); }
            let requested = buffer.len().min(u32::MAX as usize) as u32;
            let mut written = 0u32;
            let ok = unsafe {
                WriteFile(self.0, buffer.as_ptr().cast(), requested, &mut written, std::ptr::null_mut())
            };
            if ok == 0 { return Err(io::Error::last_os_error()); }
            Ok(written as usize)
        }

        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }

    /// Accepts and handles exactly one authenticated local client.
    ///
    /// The handler is never called until the OS peer has passed SID, session,
    /// executable-path and pinned Authenticode checks. It receives no pipe handle
    /// and cannot bypass framing or identity checks here.
    ///
    /// ConnectNamedPipe is currently synchronous; a cancellable overlapped accept
    /// loop and service shutdown coordination are intentionally separate work.
    pub fn serve_one_authenticated_connection<F>(
        service_sid: &str,
        client_user_sid: &str,
        expected_session_id: u32,
        expected_executable: &Path,
        expected_publisher_sha256: &str,
        handler: F,
    ) -> Result<(), String>
    where
        F: FnOnce(IpcRequest) -> IpcResponse,
    {
        let pipe = create_server_pipe(service_sid, client_user_sid)?;
        let handle = pipe.as_raw_handle();
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
        if connected == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_CONNECTED {
                return Err(format!("Could not accept WebDynamite desktop connection (Windows error {error})."));
            }
        }

        let result = (|| {
            let verified = verify_connected_client(
                handle, client_user_sid, expected_session_id,
                expected_executable, expected_publisher_sha256,
            )?;

            // No request bytes are consumed before the peer identity is verified.
            let mut io = PipeIo(handle);
            let request = read_request(&mut io)?;

            // Recheck the same process object and pipe peer immediately before
            // handing the decoded request to application authorization/dispatch.
            verified.verify_still_connected(handle)?;
            let request_id = request.request_id.clone();
            let operation = request.operation.clone();
            let mut response = match authorize_request(&request.operation, &request.data) {
                Ok(()) => handler(request),
                Err(code) => rejected_request_response(request, code),
            };
            if response.request_id != request_id {
                wipe_sensitive_response(&operation, &mut response);
                return Err("IPC handler returned a response for a different request.".into());
            }
            let write_result = write_response(&mut io, &response);
            wipe_sensitive_response(&operation, &mut response);
            write_result
        })();

        unsafe { DisconnectNamedPipe(handle); }
        result
    }
    /// Serves one authenticated request over a pipe created for overlapped I/O.
    /// Both connection acceptance and every framed read/write have bounded waits.
    pub fn serve_one_authenticated_overlapped_connection<F>(
        service_sid: &str,
        client_user_sid: &str,
        expected_session_id: u32,
        expected_executable: &Path,
        expected_publisher_sha256: &str,
        timeout_ms: u32,
        handler: F,
    ) -> Result<(), String>
    where
        F: FnOnce(IpcRequest) -> IpcResponse,
    {
        let pipe = create_overlapped_server_pipe(service_sid, client_user_sid)?;
        let handle = pipe.as_raw_handle();
        connect_with_timeout(handle, timeout_ms)?;

        let result = (|| {
            let verified = verify_connected_client(
                handle, client_user_sid, expected_session_id,
                expected_executable, expected_publisher_sha256,
            )?;

            // Authentication completes before the first request byte is read.
            let mut io = OverlappedPipeIo::new(handle, timeout_ms)?;
            let request = read_request(&mut io)?;

            // Revalidate the same retained process and pipe peer before dispatch.
            verified.verify_still_connected(handle)?;
            let request_id = request.request_id.clone();
            let operation = request.operation.clone();
            let mut response = match authorize_request(&request.operation, &request.data) {
                Ok(()) => handler(request),
                Err(code) => rejected_request_response(request, code),
            };
            if response.request_id != request_id {
                wipe_sensitive_response(&operation, &mut response);
                return Err("IPC handler returned a response for a different request.".into());
            }
            let write_result = write_response(&mut io, &response);
            wipe_sensitive_response(&operation, &mut response);
            write_result
        })();

        unsafe { DisconnectNamedPipe(handle); }
        result
    }

}

#[cfg(windows)]
pub use windows_impl::{serve_one_authenticated_connection, serve_one_authenticated_overlapped_connection};

#[cfg(not(windows))]
pub fn serve_one_authenticated_connection<F>(
    _service_sid: &str,
    _client_user_sid: &str,
    _expected_session_id: u32,
    _expected_executable: &std::path::Path,
    _expected_publisher_sha256: &str,
    _handler: F,
) -> Result<(), String>
where
    F: FnOnce(crate::ipc_protocol::IpcRequest) -> crate::ipc_protocol::IpcResponse,
{
    Err("The WebDynamite named-pipe host is available only on Windows.".into())
}

#[cfg(not(windows))]
pub fn serve_one_authenticated_overlapped_connection<F>(
    _service_sid: &str,
    _client_user_sid: &str,
    _expected_session_id: u32,
    _expected_executable: &std::path::Path,
    _expected_publisher_sha256: &str,
    _timeout_ms: u32,
    _handler: F,
) -> Result<(), String>
where
    F: FnOnce(crate::ipc_protocol::IpcRequest) -> crate::ipc_protocol::IpcResponse,
{
    Err("The WebDynamite overlapped named-pipe host is available only on Windows.".into())
}

/// Repeatedly accepts authenticated desktop connections until cancellation.
///
/// Each iteration uses the existing bounded overlapped one-connection host.
/// Cancellation is observed between iterations, so the timeout also bounds
/// normal shutdown observation. Trusted installer configuration and SCM wiring
/// remain separate activation gates.
#[cfg(windows)]
pub fn serve_authenticated_overlapped_loop<F>(
    service_sid: &str,
    client_user_sid: &str,
    expected_session_id: u32,
    expected_executable: &std::path::Path,
    expected_publisher_sha256: &str,
    timeout_ms: u32,
    cancellation: &std::sync::atomic::AtomicBool,
    handler: F,
) -> Result<(), String>
where
    F: Fn(crate::ipc_protocol::IpcRequest) -> crate::ipc_protocol::IpcResponse,
{
    use std::time::Duration;
    use crate::named_pipe_loop::{run_cancellable_accept_loop, AcceptLoopStep};
    const ACCEPT_TIMEOUT: &str = "Timed out waiting for a WebDynamite desktop pipe connection.";

    run_cancellable_accept_loop(cancellation, Duration::from_millis(timeout_ms.max(1) as u64), |bounded_timeout| {
        match serve_one_authenticated_overlapped_connection(
            service_sid, client_user_sid, expected_session_id,
            expected_executable, expected_publisher_sha256, bounded_timeout,
            |request| handler(request),
        ) {
            Ok(()) => Ok(AcceptLoopStep::ConnectionHandled),
            Err(error) if error == ACCEPT_TIMEOUT => Ok(AcceptLoopStep::TimedOut),
            Err(error) => Err(error),
        }
    })?;
    Ok(())
}

#[cfg(not(windows))]
pub fn serve_authenticated_overlapped_loop<F>(
    _service_sid: &str,
    _client_user_sid: &str,
    _expected_session_id: u32,
    _expected_executable: &std::path::Path,
    _expected_publisher_sha256: &str,
    _timeout_ms: u32,
    _cancellation: &std::sync::atomic::AtomicBool,
    _handler: F,
) -> Result<(), String>
where
    F: Fn(crate::ipc_protocol::IpcRequest) -> crate::ipc_protocol::IpcResponse,
{
    Err("The authenticated named-pipe accept loop is available only on Windows.".into())
}

/// Serves one authenticated desktop connection using the protected secret
/// dispatcher. The installer must provision `secret_store_root` with its
/// restrictive service-owned DACL before this entry point is used.
///
/// This is deliberately a one-connection primitive; callers still own the
/// cancellable accept loop and service lifecycle.
pub fn serve_one_authenticated_secret_connection(
    secret_store_root: &std::path::Path,
    service_sid: &str,
    client_user_sid: &str,
    expected_session_id: u32,
    expected_executable: &std::path::Path,
    expected_publisher_sha256: &str,
) -> Result<(), String> {
    let root = secret_store_root.to_path_buf();
    serve_one_authenticated_connection(
        service_sid,
        client_user_sid,
        expected_session_id,
        expected_executable,
        expected_publisher_sha256,
        move |mut request| crate::secret_ipc::dispatch_secret_request(&root, &mut request),
    )
}


/// Serves one authenticated secret operation over bounded overlapped I/O.
/// The secret-store root must already have its restrictive installer-provisioned DACL.
pub fn serve_one_authenticated_overlapped_secret_connection(
    secret_store_root: &std::path::Path,
    service_sid: &str,
    client_user_sid: &str,
    expected_session_id: u32,
    expected_executable: &std::path::Path,
    expected_publisher_sha256: &str,
    timeout_ms: u32,
) -> Result<(), String> {
    let root = secret_store_root.to_path_buf();
    serve_one_authenticated_overlapped_connection(
        service_sid,
        client_user_sid,
        expected_session_id,
        expected_executable,
        expected_publisher_sha256,
        timeout_ms,
        move |mut request| crate::secret_ipc::dispatch_secret_request(&root, &mut request),
    )
}
