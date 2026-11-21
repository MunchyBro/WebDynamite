//! Authenticated one-connection named-pipe host primitive.
//!
//! The caller supplies trusted installer/service configuration. The peer is
//! authenticated before the first byte is read, and its retained process handle
//! is revalidated immediately before dispatch. A service loop and SCM shutdown
//! integration are intentionally separate follow-up work.

#[cfg(windows)]
mod windows_impl {
    use std::{io::{self, Read, Write}, path::Path};
    use windows_sys::Win32::{
        Foundation::{GetLastError, HANDLE, ERROR_PIPE_CONNECTED},
        System::IO::{ReadFile, WriteFile},
        System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe},
    };

    use crate::{
        client_identity::verify_connected_client,
        ipc_protocol::{read_request, write_response, IpcRequest, IpcResponse},
        named_pipe::create_server_pipe,
    };

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
            let response = handler(request.clone());
            if response.request_id != request.request_id {
                return Err("IPC handler returned a response for a different request.".into());
            }
            write_response(&mut io, &response)
        })();

        unsafe { DisconnectNamedPipe(handle); }
        result
    }
}

#[cfg(windows)]
pub use windows_impl::serve_one_authenticated_connection;

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
