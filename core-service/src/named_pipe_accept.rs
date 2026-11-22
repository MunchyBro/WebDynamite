//! Bounded overlapped connection acceptance for the protected local pipe.
//!
//! This primitive is intentionally not wired into the service startup loop yet.
//! It owns the OVERLAPPED/event lifetime through completion or cancellation.

#[cfg(windows)]
pub fn connect_with_timeout(pipe: windows_sys::Win32::Foundation::HANDLE, timeout_ms: u32) -> Result<(), String> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::ConnectNamedPipe,
            Threading::{CreateEventW, WaitForSingleObject},
        },
    };

    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(format!("Could not create named-pipe accept event (Windows error {}).", unsafe { GetLastError() }));
    }

    // Keep the event alive until the kernel has completed or cancelled this I/O.
    let result = (|| {
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event;
        let connected = unsafe { ConnectNamedPipe(pipe, &mut overlapped) };
        if connected != 0 {
            return Ok(());
        }

        let error = unsafe { GetLastError() };
        if error == ERROR_PIPE_CONNECTED {
            return Ok(());
        }
        if error != ERROR_IO_PENDING {
            return Err(format!("Could not begin overlapped pipe accept (Windows error {error})."));
        }

        let wait = unsafe { WaitForSingleObject(event, timeout_ms) };
        if wait == WAIT_TIMEOUT {
            unsafe { CancelIoEx(pipe, &overlapped); }
            let mut transferred = 0u32;
            // A cancelled OVERLAPPED must not be dropped until completion is observed.
            unsafe { GetOverlappedResult(pipe, &overlapped, &mut transferred, 1); }
            return Err("Timed out waiting for a WebDynamite desktop pipe connection.".into());
        }
        if wait != WAIT_OBJECT_0 {
            let wait_error = unsafe { GetLastError() };
            unsafe { CancelIoEx(pipe, &overlapped); }
            let mut transferred = 0u32;
            unsafe { GetOverlappedResult(pipe, &overlapped, &mut transferred, 1); }
            return Err(format!("Waiting for a WebDynamite pipe connection failed (Windows error {wait_error})."));
        }

        let mut transferred = 0u32;
        if unsafe { GetOverlappedResult(pipe, &overlapped, &mut transferred, 0) } == 0 {
            let completion_error = unsafe { GetLastError() };
            return Err(format!("Overlapped pipe accept did not complete (Windows error {completion_error})."));
        }
        Ok(())
    })();

    unsafe { CloseHandle(event); }
    result
}

#[cfg(not(windows))]
pub fn connect_with_timeout(_pipe: isize, _timeout_ms: u32) -> Result<(), String> {
    Err("Overlapped named-pipe acceptance is available only on Windows.".into())
}
