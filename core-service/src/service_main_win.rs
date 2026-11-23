//! Windows SCM service-main orchestration primitive.
//!
//! The native adapter registers the SCM handler and publishes status. The
//! lifecycle runner is kept independent of Win32 calls so its failure and race
//! behavior can be unit-tested without starting a real Windows service.
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
/// The startup hook receives the shared lifecycle so it can observe a STOP
/// arriving during startup. The wait hook must block without busy-spinning and
/// observe the lifecycle stop request. Runtime teardown always runs on this
/// thread, never inside the SCM control-handler callback.
pub fn run_service_main<Start, Wait, Stop>(
    service_name: &str,
    start_runtime: Start,
    wait_for_stop: Wait,
    stop_runtime: Stop,
) -> Result<(), u32>
where
    Start: FnOnce(Arc<ServiceLifecycle>) -> Result<(), u32>,
    Wait: FnOnce(Arc<ServiceLifecycle>),
    Stop: FnOnce() -> Result<(), u32>,
{
    let lifecycle = Arc::new(ServiceLifecycle::new());
    let handler = register_service_control_handler(service_name, Arc::clone(&lifecycle))?;
    let handle = handler.status_handle();

    run_lifecycle(
        lifecycle,
        start_runtime,
        wait_for_stop,
        stop_runtime,
        |snapshot| unsafe { publish_service_status(handle, &snapshot) },
    )
}

/// Platform-independent lifecycle orchestration with an injected status sink.
/// Kept private to the Windows adapter; tests exercise all cleanup branches
/// with a deterministic publisher and no SCM dependency.
fn run_lifecycle<Start, Wait, Stop, Publish>(
    lifecycle: Arc<ServiceLifecycle>,
    start_runtime: Start,
    wait_for_stop: Wait,
    stop_runtime: Stop,
    mut publish: Publish,
) -> Result<(), u32>
where
    Start: FnOnce(Arc<ServiceLifecycle>) -> Result<(), u32>,
    Wait: FnOnce(Arc<ServiceLifecycle>),
    Stop: FnOnce() -> Result<(), u32>,
    Publish: FnMut(ServiceStatusSnapshot) -> Result<(), u32>,
{
    publish(ServiceStatusSnapshot::from_state(ServiceState::StartPending))?;

    if let Err(start_error) = start_runtime(Arc::clone(&lifecycle)) {
        lifecycle.request_stop();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        stopped.win32_exit_code = start_error;
        let _ = publish(stopped);
        return Err(start_error);
    }

    if lifecycle.stop_requested() {
        let stop_result = stop_runtime();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        if let Err(error) = stop_result {
            stopped.win32_exit_code = error;
        }
        let status_result = publish(stopped);
        return stop_result.and(status_result);
    }

    if lifecycle.mark_running().is_err() {
        // STOP can race the final startup check. Runtime has started, so clean
        // it up even though the Running transition is no longer legal.
        lifecycle.request_stop();
        let stop_result = stop_runtime();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        if let Err(error) = stop_result {
            stopped.win32_exit_code = error;
        }
        let _ = publish(stopped);
        return Err(ERROR_INVALID_HANDLE);
    }

    if let Err(status_error) = publish(ServiceStatusSnapshot::from_state(ServiceState::Running)) {
        lifecycle.request_stop();
        let stop_result = stop_runtime();
        lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
        let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        if let Err(error) = stop_result {
            stopped.win32_exit_code = error;
        }
        let _ = publish(stopped);
        return Err(status_error);
    }

    wait_for_stop(Arc::clone(&lifecycle));
    if !lifecycle.stop_requested() {
        lifecycle.request_stop();
    }

    // A status failure must never prevent worker/child-process teardown.
    let pending_error = publish(ServiceStatusSnapshot::from_state(ServiceState::StopPending)).err();
    let stop_result = stop_runtime();
    lifecycle.mark_stopped().map_err(|_| ERROR_INVALID_HANDLE)?;
    let mut stopped = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
    if let Err(error) = stop_result {
        stopped.win32_exit_code = error;
    }
    let stopped_error = publish(stopped).err();

    if let Some(error) = pending_error.or(stopped_error) {
        return Err(error);
    }
    stop_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};

    fn publisher(
        states: Arc<Mutex<Vec<ServiceState>>>,
        fail_on: Option<ServiceState>,
    ) -> impl FnMut(ServiceStatusSnapshot) -> Result<(), u32> {
        move |snapshot| {
            let state = match snapshot.current_state {
                2 => ServiceState::StartPending,
                4 => ServiceState::Running,
                3 => ServiceState::StopPending,
                1 => ServiceState::Stopped,
                _ => return Err(87),
            };
            states.lock().unwrap().push(state);
            if Some(state) == fail_on { Err(5) } else { Ok(()) }
        }
    }

    #[test]
    fn normal_lifecycle_starts_waits_and_stops() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let stops = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::clone(&stops);
        let result = run_lifecycle(
            Arc::clone(&lifecycle),
            |_| Ok(()),
            |lifecycle| { lifecycle.request_stop(); },
            move || { stop_count.fetch_add(1, Ordering::SeqCst); Ok(()) },
            publisher(Arc::clone(&states), None),
        );
        assert!(result.is_ok());
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
        assert_eq!(*states.lock().unwrap(), vec![
            ServiceState::StartPending, ServiceState::Running,
            ServiceState::StopPending, ServiceState::Stopped,
        ]);
    }

    #[test]
    fn startup_failure_does_not_call_stop_hook_but_reports_stopped() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let result = run_lifecycle(
            Arc::clone(&lifecycle), |_| Err(1066), |_| panic!("wait must not run"),
            || panic!("runtime did not finish starting"),
            publisher(Arc::clone(&states), None),
        );
        assert_eq!(result, Err(1066));
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
        assert_eq!(states.lock().unwrap().last(), Some(&ServiceState::Stopped));
    }

    #[test]
    fn stop_during_startup_stops_runtime_without_running_transition() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let stops = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::clone(&stops);
        let result = run_lifecycle(
            Arc::clone(&lifecycle),
            |lifecycle| { lifecycle.request_stop(); Ok(()) },
            |_| panic!("wait must not run"),
            move || { stop_count.fetch_add(1, Ordering::SeqCst); Ok(()) },
            publisher(Arc::clone(&states), None),
        );
        assert!(result.is_ok());
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(!states.lock().unwrap().contains(&ServiceState::Running));
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
    }

    #[test]
    fn failed_running_status_still_tears_down_runtime() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let stops = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::clone(&stops);
        let result = run_lifecycle(
            Arc::clone(&lifecycle), |_| Ok(()), |_| panic!("wait must not run"),
            move || { stop_count.fetch_add(1, Ordering::SeqCst); Ok(()) },
            publisher(Arc::clone(&states), Some(ServiceState::Running)),
        );
        assert_eq!(result, Err(5));
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
    }

    #[test]
    fn failed_stop_pending_status_does_not_skip_teardown() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let stops = Arc::new(AtomicUsize::new(0));
        let stop_count = Arc::clone(&stops);
        let result = run_lifecycle(
            Arc::clone(&lifecycle), |_| Ok(()),
            |lifecycle| { lifecycle.request_stop(); },
            move || { stop_count.fetch_add(1, Ordering::SeqCst); Ok(()) },
            publisher(Arc::clone(&states), Some(ServiceState::StopPending)),
        );
        assert_eq!(result, Err(5));
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
        assert_eq!(states.lock().unwrap().last(), Some(&ServiceState::Stopped));
    }

    #[test]
    fn stop_hook_error_is_reported_after_final_status() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        let states = Arc::new(Mutex::new(Vec::new()));
        let result = run_lifecycle(
            Arc::clone(&lifecycle), |_| Ok(()),
            |lifecycle| { lifecycle.request_stop(); },
            || Err(1066),
            publisher(Arc::clone(&states), None),
        );
        assert_eq!(result, Err(1066));
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
        assert_eq!(states.lock().unwrap().last(), Some(&ServiceState::Stopped));
    }
}
