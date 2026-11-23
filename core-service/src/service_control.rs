//! Deterministic translation of raw SCM control codes into lifecycle actions.
//!
//! The eventual RegisterServiceCtrlHandlerExW callback should do no blocking
//! work: call this translator, publish the returned status snapshot when
//! present, and signal the service worker to perform bounded shutdown.

use crate::{
    service_lifecycle::{ServiceControl, ServiceLifecycle},
    service_status::ServiceStatusSnapshot,
};

pub const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;
pub const SERVICE_CONTROL_INTERROGATE: u32 = 0x0000_0004;
pub const SERVICE_CONTROL_SHUTDOWN: u32 = 0x0000_0005;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScmControlOutcome {
    /// False means the control code is not supported and must not be acknowledged.
    pub accepted: bool,
    /// Present when the callback should publish a fresh status snapshot.
    pub status: Option<ServiceStatusSnapshot>,
    /// Set for STOP/SHUTDOWN; the callback should only signal shutdown, never
    /// synchronously join workers or terminate child processes.
    pub signal_shutdown: bool,
}

pub fn translate_scm_control(
    lifecycle: &ServiceLifecycle,
    control_code: u32,
) -> ScmControlOutcome {
    let control = match control_code {
        SERVICE_CONTROL_STOP => ServiceControl::Stop,
        SERVICE_CONTROL_SHUTDOWN => ServiceControl::Shutdown,
        SERVICE_CONTROL_INTERROGATE => ServiceControl::Interrogate,
        _ => ServiceControl::Unsupported,
    };
    let result = lifecycle.handle_control(control);
    ScmControlOutcome {
        accepted: result.accepted,
        status: result.report_status.then(|| ServiceStatusSnapshot::from_state(result.state)),
        signal_shutdown: matches!(control, ServiceControl::Stop | ServiceControl::Shutdown)
            && result.accepted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_lifecycle::ServiceState;
    use std::sync::Arc;

    #[test]
    fn stop_and_shutdown_request_stop_and_return_pending_status() {
        for code in [SERVICE_CONTROL_STOP, SERVICE_CONTROL_SHUTDOWN] {
            let lifecycle = ServiceLifecycle::new();
            lifecycle.mark_running().unwrap();
            let outcome = translate_scm_control(&lifecycle, code);
            assert!(outcome.accepted);
            assert!(outcome.signal_shutdown);
            let status = outcome.status.unwrap();
            assert_eq!(status.current_state, crate::service_status::SERVICE_STOP_PENDING);
            assert_eq!(lifecycle.state(), ServiceState::StopPending);
        }
    }

    #[test]
    fn interrogate_returns_status_without_mutating_lifecycle() {
        let lifecycle = ServiceLifecycle::new();
        lifecycle.mark_running().unwrap();
        let outcome = translate_scm_control(&lifecycle, SERVICE_CONTROL_INTERROGATE);
        assert!(outcome.accepted);
        assert!(!outcome.signal_shutdown);
        assert_eq!(outcome.status.unwrap().current_state, crate::service_status::SERVICE_RUNNING);
        assert_eq!(lifecycle.state(), ServiceState::Running);
    }

    #[test]
    fn unsupported_controls_are_not_acknowledged() {
        let lifecycle = ServiceLifecycle::new();
        let outcome = translate_scm_control(&lifecycle, 0xFFFF_FFFF);
        assert!(!outcome.accepted);
        assert!(!outcome.signal_shutdown);
        assert!(outcome.status.is_none());
        assert_eq!(lifecycle.state(), ServiceState::StartPending);
    }

    #[test]
    fn repeated_stop_is_idempotent_and_does_not_claim_new_status() {
        let lifecycle = ServiceLifecycle::new();
        let first = translate_scm_control(&lifecycle, SERVICE_CONTROL_STOP);
        let second = translate_scm_control(&lifecycle, SERVICE_CONTROL_STOP);
        assert!(first.signal_shutdown);
        assert!(first.status.is_some());
        assert!(second.accepted);
        assert!(second.signal_shutdown);
        assert!(second.status.is_none());
        assert_eq!(lifecycle.state(), ServiceState::StopPending);
    }

    #[test]
    fn control_translation_is_safe_to_share_between_callback_threads() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        lifecycle.mark_running().unwrap();
        let threads = (0..8).map(|_| {
            let lifecycle = Arc::clone(&lifecycle);
            std::thread::spawn(move || translate_scm_control(&lifecycle, SERVICE_CONTROL_STOP))
        }).collect::<Vec<_>>();
        let outcomes = threads.into_iter().map(|thread| thread.join().unwrap()).collect::<Vec<_>>();
        assert!(outcomes.iter().all(|outcome| outcome.accepted && outcome.signal_shutdown));
        assert_eq!(outcomes.iter().filter(|outcome| outcome.status.is_some()).count(), 1);
        assert_eq!(lifecycle.state(), ServiceState::StopPending);
    }
}
