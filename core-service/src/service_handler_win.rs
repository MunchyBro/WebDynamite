//! Native Windows SCM control-handler registration and callback.
//!
//! The registered context is owned by `RegisteredServiceHandler` and must
//! outlive the service dispatcher. The callback performs only an atomic
//! lifecycle transition and a bounded SetServiceStatus call; worker cleanup is
//! deliberately left to the service-main thread.
use crate::{
    service_control::{translate_scm_control, ScmControlOutcome},
    service_lifecycle::ServiceLifecycle,
    service_status_win::publish_service_status,
};
use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_CALL_NOT_IMPLEMENTED, ERROR_GEN_FAILURE,
        ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    },
    System::Services::{
        RegisterServiceCtrlHandlerExW, SERVICE_STATUS_HANDLE,
    },
};

struct HandlerContext {
    lifecycle: Arc<ServiceLifecycle>,
    status_handle: AtomicPtr<std::ffi::c_void>,
}

/// Owns the callback context and registered SCM status handle.
///
/// Keep this value alive until the service-main callback is ready to return.
/// The SCM may invoke the handler on a different thread while it is alive.
pub struct RegisteredServiceHandler {
    _context: Box<HandlerContext>,
    handle: SERVICE_STATUS_HANDLE,
}

impl RegisteredServiceHandler {
    pub fn status_handle(&self) -> SERVICE_STATUS_HANDLE {
        self.handle
    }
}

/// Register the extended SCM control handler for one service instance.
pub fn register_service_control_handler(
    service_name: &str,
    lifecycle: Arc<ServiceLifecycle>,
) -> Result<RegisteredServiceHandler, u32> {
    if service_name.is_empty() || service_name.encode_utf16().any(|unit| unit == 0) {
        return Err(ERROR_INVALID_PARAMETER);
    }

    let mut context = Box::new(HandlerContext {
        lifecycle,
        status_handle: AtomicPtr::new(ptr::null_mut()),
    });
    let context_ptr = (&mut *context) as *mut HandlerContext as *mut std::ffi::c_void;
    let name: Vec<u16> = OsStr::new(service_name).encode_wide().chain(Some(0)).collect();

    // SAFETY: name is NUL-terminated and remains alive for this call. The
    // boxed context has a stable address and is returned to the caller on
    // success, which must retain it until service-main exits.
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(
            name.as_ptr(),
            Some(service_control_handler),
            context_ptr,
        )
    };
    if handle.is_null() {
        return Err(unsafe { GetLastError() });
    }

    context.status_handle.store(handle, Ordering::Release);
    Ok(RegisteredServiceHandler { _context: context, handle })
}

unsafe extern "system" fn service_control_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut std::ffi::c_void,
    context: *mut std::ffi::c_void,
) -> u32 {
    if context.is_null() {
        return ERROR_INVALID_PARAMETER;
    }

    // No panic may cross the Win32 callback ABI boundary.
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: SCM receives the pointer to the Box retained by
        // RegisteredServiceHandler; its owner must keep it alive through the
        // service-main callback.
        let context = unsafe { &*(context as *const HandlerContext) };
        let outcome = translate_scm_control(&context.lifecycle, control);
        handle_outcome(context, outcome)
    }))
    .unwrap_or(ERROR_GEN_FAILURE)
}

fn handle_outcome(context: &HandlerContext, outcome: ScmControlOutcome) -> u32 {
    if !outcome.accepted {
        return ERROR_CALL_NOT_IMPLEMENTED;
    }

    if let Some(snapshot) = outcome.status {
        let handle = context.status_handle.load(Ordering::Acquire);
        if handle.is_null() {
            return ERROR_INVALID_HANDLE;
        }
        // SAFETY: the registered handler owns this live status handle until
        // its context is dropped after service-main returns.
        if let Err(error) = unsafe { publish_service_status(handle, &snapshot) } {
            return error;
        }
    }

    // STOP/SHUTDOWN only transition the lifecycle state. The service-main
    // worker observes stop_requested() and performs bounded cleanup there.
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        service_control::{SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_STOP},
        service_lifecycle::ServiceState,
    };

    #[test]
    fn callback_outcome_requests_worker_shutdown_without_blocking() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        lifecycle.mark_running().unwrap();
        let context = HandlerContext {
            lifecycle: Arc::clone(&lifecycle),
            status_handle: AtomicPtr::new(ptr::null_mut()),
        };
        let outcome = translate_scm_control(&lifecycle, SERVICE_CONTROL_STOP);
        // A null handle is reported as an error rather than falsely
        // acknowledging a status transition that could not be published.
        assert_eq!(handle_outcome(&context, outcome), ERROR_INVALID_HANDLE);
        assert_eq!(lifecycle.state(), ServiceState::StopPending);
    }

    #[test]
    fn interrogate_without_status_handle_fails_closed() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let context = HandlerContext {
            lifecycle: Arc::clone(&lifecycle),
            status_handle: AtomicPtr::new(ptr::null_mut()),
        };
        let outcome = translate_scm_control(&lifecycle, SERVICE_CONTROL_INTERROGATE);
        assert_eq!(handle_outcome(&context, outcome), ERROR_INVALID_HANDLE);
        assert_eq!(lifecycle.state(), ServiceState::StartPending);
    }
}
