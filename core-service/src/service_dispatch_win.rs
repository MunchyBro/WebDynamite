//! Windows SCM dispatcher entry-point wrapper.
//!
//! This module deliberately does not provide or register a service-main
//! callback. It only constructs the native service table and enters the SCM
//! dispatcher when an explicit caller supplies its own callback.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::{
    Foundation::GetLastError,
    System::Services::{
        StartServiceCtrlDispatcherW, LPSERVICE_MAIN_FUNCTIONW, SERVICE_TABLE_ENTRYW,
    },
};

/// Enter the Windows Service Control Manager dispatcher.
///
/// The dispatcher call is synchronous and returns only after the service has
/// stopped or the SCM connection fails. The supplied callback must be a valid
/// process-lifetime service-main function and must obey the SCM callback rules.
pub fn start_service_dispatcher(
    service_name: &str,
    service_main: LPSERVICE_MAIN_FUNCTIONW,
) -> Result<(), u32> {
    if service_name.is_empty() || service_name.encode_utf16().any(|unit| unit == 0) {
        return Err(87); // ERROR_INVALID_PARAMETER
    }
    let Some(service_main) = service_main else {
        return Err(87);
    };

    let mut name: Vec<u16> = OsStr::new(service_name).encode_wide().chain(Some(0)).collect();
    let mut table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];

    // SAFETY: the table and UTF-16 service name remain alive and unmodified for
    // the entire synchronous call. The callback is a non-null function pointer.
    let result = unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) };
    if result == 0 {
        Err(unsafe { GetLastError() })
    } else {
        Ok(())
    }
}
