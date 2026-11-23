//! Narrow Windows Service Control Manager (SCM) API boundary.
//!
//! Provides native dispatcher, control-handler registration, and status
//! publication primitives. It intentionally does not start the Core Service,
//! load trusted config, create the named-pipe host, or alter interactive startup.
//! Those actions remain gated on installer provisioning, signer identity,
//! and migration/end-to-end validation.

#[cfg(windows)]
mod windows_impl {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::{
        Foundation::GetLastError,
        System::Services::{
            RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
            LPHANDLER_FUNCTION_EX, LPSERVICE_MAIN_FUNCTIONW, SERVICE_ACCEPT_SHUTDOWN,
            SERVICE_ACCEPT_STOP, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STOPPED,
            SERVICE_STOP_PENDING, SERVICE_STATUS, SERVICE_STATUS_HANDLE,
            SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
        },
    };

    use crate::service_lifecycle::ServiceState;

    /// Enters the SCM dispatcher table for this process.
    /// The service name and callback must match the installer registration.
    /// This function is never called by the current interactive main.
    pub fn dispatch_service_table(
        service_name: &str,
        service_main: LPSERVICE_MAIN_FUNCTIONW,
    ) -> Result<(), String> {
        if service_name.is_empty() || service_name.contains('\0') || service_main.is_none() {
            return Err("A valid service name and service-main callback are required.".into());
        }

        let mut name = OsStr::new(service_name).encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let mut table = [
            SERVICE_TABLE_ENTRYW {
                lpServiceName: name.as_mut_ptr(),
                lpServiceProc: service_main,
            },
            SERVICE_TABLE_ENTRYW {
                lpServiceName: ptr::null_mut(),
                lpServiceProc: None,
            },
        ];

        let ok = unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            return Err(format!("Windows SCM dispatcher could not start (error {error})."));
        }
        Ok(())
    }

    /// Registers the extended control handler for the named service.
    pub fn register_control_handler(
        service_name: &str,
        handler: LPHANDLER_FUNCTION_EX,
        context: *mut std::ffi::c_void,
    ) -> Result<SERVICE_STATUS_HANDLE, String> {
        if service_name.is_empty() || service_name.contains('\0') || handler.is_none() {
            return Err("A valid service name and control-handler callback are required.".into());
        }
        let name = OsStr::new(service_name).encode_wide().chain(Some(0)).collect::<Vec<_>>();
        let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), handler, context) };
        if handle == 0 {
            let error = unsafe { GetLastError() };
            return Err(format!("Windows SCM control-handler registration failed (error {error})."));
        }
        Ok(handle)
    }

    /// Publishes one lifecycle snapshot to SCM.
    /// STOP and SHUTDOWN are accepted only while Running.
    /// Pending states publish bounded wait hints and nonzero checkpoints.
    pub fn publish_service_status(
        handle: SERVICE_STATUS_HANDLE,
        state: ServiceState,
        checkpoint: u32,
        wait_hint_ms: u32,
        win32_exit_code: u32,
    ) -> Result<(), String> {
        if handle == 0 {
            return Err("Cannot publish SCM status through an invalid service handle.".into());
        }

        let (current_state, controls_accepted, checkpoint, wait_hint) = match state {
            ServiceState::StartPending => (
                SERVICE_START_PENDING,
                0,
                checkpoint.max(1),
                wait_hint_ms.clamp(1_000, 60_000),
            ),
            ServiceState::Running => (
                SERVICE_RUNNING,
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
                0,
                0,
            ),
            ServiceState::StopPending => (
                SERVICE_STOP_PENDING,
                0,
                checkpoint.max(1),
                wait_hint_ms.clamp(1_000, 60_000),
            ),
            ServiceState::Stopped => (SERVICE_STOPPED, 0, 0, 0),
        };

        let mut status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: current_state,
            dwControlsAccepted: controls_accepted,
            dwWin32ExitCode: win32_exit_code,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: checkpoint,
            dwWaitHint: wait_hint,
        };
        let ok = unsafe { SetServiceStatus(handle, &mut status) };
        if ok == 0 {
            let error = unsafe { GetLastError() };
            return Err(format!("Windows SCM rejected a service status update (error {error})."));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows_impl::{dispatch_service_table, publish_service_status, register_control_handler};

#[cfg(not(windows))]
pub fn dispatch_service_table(
    _service_name: &str,
    _service_main: Option<unsafe extern "system" fn(u32, *mut *mut u16)>,
) -> Result<(), String> {
    Err("Windows SCM dispatch is available only on Windows.".into())
}

#[cfg(not(windows))]
pub fn register_control_handler(
    _service_name: &str,
    _handler: Option<unsafe extern "system" fn(u32, u32, *mut std::ffi::c_void, *mut std::ffi::c_void) -> u32>,
    _context: *mut std::ffi::c_void,
) -> Result<isize, String> {
    Err("Windows SCM control-handler registration is available only on Windows.".into())
}

#[cfg(not(windows))]
pub fn publish_service_status(
    _handle: isize,
    _state: crate::service_lifecycle::ServiceState,
    _checkpoint: u32,
    _wait_hint_ms: u32,
    _win32_exit_code: u32,
) -> Result<(), String> {
    Err("Windows SCM status publication is available only on Windows.".into())
}
