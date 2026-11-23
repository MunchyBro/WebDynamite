//! Windows SCM service-main orchestration primitive.
//!
//! This composes handler registration, lifecycle transitions, status
//! publication, runtime startup, stop waiting, and bounded runtime teardown.
//! The application must provide the runtime hooks; this module does not enable
//! SCM mode or choose a service identity/configuration policy.
use crate::{
    service_handler_win::register_service_control_handler,
    service_lifecycle::{ServiceLifecycle, ServiceState},
    service_status::ServiceStatusSnapshot,
    service_status_win::publish_service_status,
};
use std::sync::Arc;
use windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE;

/// Execute one service instance on the SCM-provided service-main thread.
///
/// `start_runtime` must return only after startup is complete or has failed.
/// `wait_for_stop` must block without busy-spinning until the handler requests
/// shutdown. `stop_runtime` must perform bounded cleanup and must not be run
/// from the SCM control-handler callback.
///
/// The registered handler owner remains alive across all hooks, ensuring the
/// SCM callback context and status handle stay valid.
pub fn run_service_main<Start, Wait, Stop>(
    service_name: &str,
    start_runtime: Start,
    wait_for_stop: Wait,
    stop_runtime: Stop,
) -> Result<(), u32>
where
    Start: FnOnce() -> Result<(), u32>,
    Wait: FnOnce(Arc<ServiceLifecycle>),
    Stop: FnOnce() -> Result<(), u32>,
{
    let lifecycle = Arc::new(ServiceLifecycle::new());
    let handler = register_service_control_handler(service_name, Arc::clone(&lifecycle))?;
    let handle = handler.status_handle();

    publish(handle, ServiceStatusSnapshot::from_state(ServiceState::StartPending))?;

    if let Err(start_error) = start_runtime() {
        lifecycle.request_stop();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        stopped.win32_exit_code = start_error;
        publish(handle, stopped)?;
        return Err(start_error);
    }

    if lifecycle.stop_requested() {
        let _ = stop_runtime();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        publish(handle, ServiceStatusSnapshot::from_state(ServiceState::Stopped))?;
        return Ok(());
    }

    lifecycle.mark_running().map_err(|_| ERROR_INVALID_HANDLE)?;
    publish(handle, ServiceStatusSnapshot::from_state(ServiceState::Running))?;

    wait_for_stop(Arc::clone(&lifecycle));
    if !lifecycle.stop_requested() {
        lifecycle.request_stop();
    }

    // STOP/SHUTDOWN callback normally publishes STOP_PENDING. Publish again
    // here to cover a host-initiated shutdown path and make teardown explicit.
    publish(handle, ServiceStatusSnapshot::from_state(ServiceState::StopPending))?;

    let stop_result = stop_runtime();
    lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
    let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
    if let Err(error) = stop_result {
        stopped.win32_exit_code = error;
    }
    publish(handle, stopped)?;
    stop_result
}

fn publish(
    handle: windows_sys::Win32::System::Services::SERVICE_STATUS_HANDLE,
    snapshot: ServiceStatusSnapshot,
) -> Result<(), u32> {
    // SAFETY: the registered handler owner remains in scope for this entire
    // function and owns the status handle returned by the SCM.
    unsafe { publish_service_status(handle, &snapshot) }
}
