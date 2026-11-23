//! Process-wide bridge from the native SCM service-main callback to Rust-owned work.
//!
//! The SCM callback ABI has no user-data parameter. This module therefore
//! installs exactly one pending, owned closure before entering the dispatcher.
//! The closure is taken and run on the SCM service-main thread; it is never
//! executed while the registry mutex is held.
//!
//! This is an entrypoint primitive only. It does not choose runtime
//! configuration, create a service identity, or enable service mode itself.

use crate::service_dispatch_win::start_service_dispatcher;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Mutex, OnceLock},
};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_GEN_FAILURE};

type ServiceMainWork = Box<dyn FnOnce() -> Result<(), u32> + Send + 'static>;

static PENDING_SERVICE_MAIN: OnceLock<Mutex<Option<ServiceMainWork>>> = OnceLock::new();
static SERVICE_MAIN_RESULT: OnceLock<Mutex<Option<Result<(), u32>>>> = OnceLock::new();

fn pending_slot() -> &'static Mutex<Option<ServiceMainWork>> {
    PENDING_SERVICE_MAIN.get_or_init(|| Mutex::new(None))
}

fn result_slot() -> &'static Mutex<Option<Result<(), u32>>> {
    SERVICE_MAIN_RESULT.get_or_init(|| Mutex::new(None))
}

/// Enter the SCM dispatcher and run one owned service-main closure.
///
/// The closure runs on the SCM-created service-main thread. The dispatcher
/// call is synchronous, so its result is returned after the service callback
/// has completed. Only one dispatch may be active in a process.
pub fn dispatch_service<F>(service_name: &str, service_main: F) -> Result<(), u32>
where
    F: FnOnce() -> Result<(), u32> + Send + 'static,
{
    {
        let mut pending = pending_slot().lock().map_err(|_| ERROR_GEN_FAILURE)?;
        if pending.is_some() {
            return Err(ERROR_ALREADY_EXISTS);
        }
        *pending = Some(Box::new(service_main));
    }
    *result_slot().lock().map_err(|_| ERROR_GEN_FAILURE)? = None;

    if let Err(error) = start_service_dispatcher(service_name, Some(service_main_trampoline)) {
        if let Ok(mut pending) = pending_slot().lock() {
            *pending = None;
        }
        if let Ok(mut result) = result_slot().lock() {
            *result = None;
        }
        return Err(error);
    }

    result_slot()
        .lock()
        .map_err(|_| ERROR_GEN_FAILURE)?
        .take()
        .unwrap_or(Err(ERROR_GEN_FAILURE))
}

unsafe extern "system" fn service_main_trampoline(
    _argc: u32,
    _argv: *mut *mut u16,
) {
    let work = pending_slot().lock().ok().and_then(|mut pending| pending.take());
    let result = match work {
        Some(work) => invoke_service_main(work),
        None => Err(ERROR_GEN_FAILURE),
    };

    if let Ok(mut slot) = result_slot().lock() {
        *slot = Some(result);
    }
}

fn invoke_service_main(work: ServiceMainWork) -> Result<(), u32> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or(Err(ERROR_GEN_FAILURE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_main_work_returns_its_result() {
        assert_eq!(invoke_service_main(Box::new(|| Ok(()))), Ok(()));
        assert_eq!(invoke_service_main(Box::new(|| Err(1066))), Err(1066));
    }

    #[test]
    fn service_main_panic_is_contained_as_win32_failure() {
        let result = invoke_service_main(Box::new(|| -> Result<(), u32> {
            panic!("service-main callback panic");
        }));
        assert_eq!(result, Err(ERROR_GEN_FAILURE));
    }
}

/// Compose the native dispatcher bridge with the existing SCM lifecycle runner.
///
/// This wires the service-main callback, control-handler registration, status
/// publication, and startup/wait/teardown orchestration into one callable
/// entrypoint. The supplied hooks still own the runtime implementation; this
/// function deliberately does not construct production runtime state or enable
/// service mode by itself.
pub fn dispatch_service_lifecycle<Start, Wait, Stop>(
    service_name: &str,
    start_runtime: Start,
    wait_for_stop: Wait,
    stop_runtime: Stop,
) -> Result<(), u32>
where
    Start: FnOnce(std::sync::Arc<crate::service_lifecycle::ServiceLifecycle>) -> Result<(), u32>
        + Send
        + 'static,
    Wait: FnOnce(std::sync::Arc<crate::service_lifecycle::ServiceLifecycle>) + Send + 'static,
    Stop: FnOnce() -> Result<(), u32> + Send + 'static,
{
    let owned_name = service_name.to_owned();
    dispatch_service(&owned_name, move || {
        crate::service_main_win::run_service_main(
            &owned_name,
            start_runtime,
            wait_for_stop,
            stop_runtime,
        )
    })
}
