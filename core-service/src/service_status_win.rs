//! Windows-only adapter for publishing the platform-neutral SCM status snapshot.
use crate::service_status::ServiceStatusSnapshot;
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_INVALID_HANDLE},
    System::Services::{SetServiceStatus, SERVICE_STATUS, SERVICE_STATUS_HANDLE},
};

/// Publish one status snapshot through a handle returned by
/// RegisterServiceCtrlHandlerExW.
///
/// # Safety
/// The caller must pass a live status handle owned by the current registered
/// service instance. The handle must remain valid for the duration of this call.
pub unsafe fn publish_service_status(
    handle: SERVICE_STATUS_HANDLE,
    snapshot: &ServiceStatusSnapshot,
) -> Result<(), u32> {
    if handle.is_null() {
        return Err(ERROR_INVALID_HANDLE);
    }

    let native = SERVICE_STATUS {
        dwServiceType: snapshot.service_type,
        dwCurrentState: snapshot.current_state,
        dwControlsAccepted: snapshot.controls_accepted,
        dwWin32ExitCode: snapshot.win32_exit_code,
        dwServiceSpecificExitCode: snapshot.service_specific_exit_code,
        dwCheckPoint: snapshot.checkpoint,
        dwWaitHint: snapshot.wait_hint_ms,
    };

    if SetServiceStatus(handle, &native) == 0 {
        Err(GetLastError())
    } else {
        Ok(())
    }
}
