//! Host-neutral Windows Service lifecycle state machine.
//!
//! This is deliberately not an SCM dispatcher. It gives a future Windows
//! service host a small, race-safe contract for reporting lifecycle changes
//! and cancelling bounded workers without activating service mode today.
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceState {
    StartPending = 0,
    Running = 1,
    StopPending = 2,
    Stopped = 3,
}

impl ServiceState {
    fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::StartPending,
            1 => Self::Running,
            2 => Self::StopPending,
            _ => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceControl {
    Stop,
    Shutdown,
    Interrogate,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlResult {
    pub state: ServiceState,
    /// The SCM host should publish a fresh SERVICE_STATUS snapshot.
    pub report_status: bool,
    /// False means the control is not supported by this lifecycle contract.
    pub accepted: bool,
}

#[derive(Debug)]
pub struct ServiceLifecycle {
    state: AtomicU8,
}

impl Default for ServiceLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceLifecycle {
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(ServiceState::StartPending as u8) }
    }

    pub fn state(&self) -> ServiceState {
        ServiceState::from_raw(self.state.load(Ordering::Acquire))
    }

    /// Marks startup complete. Cannot resurrect a service after stop begins.
    pub fn mark_running(&self) -> Result<(), ServiceState> {
        self.state
            .compare_exchange(
                ServiceState::StartPending as u8,
                ServiceState::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(ServiceState::from_raw)
    }

    /// Requests shutdown. Repeated STOP/SHUTDOWN controls are harmless.
    /// The state itself is the cancellation signal for bounded worker loops.
    pub fn request_stop(&self) -> bool {
        loop {
            let current = self.state.load(Ordering::Acquire);
            match ServiceState::from_raw(current) {
                ServiceState::StartPending | ServiceState::Running => {
                    if self.state.compare_exchange(
                        current,
                        ServiceState::StopPending as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ).is_ok() {
                        return true;
                    }
                }
                ServiceState::StopPending | ServiceState::Stopped => return false,
            }
        }
    }

    /// Completes shutdown only after the host has stopped its workers/resources.
    pub fn mark_stopped(&self) -> Result<(), ServiceState> {
        self.state
            .compare_exchange(
                ServiceState::StopPending as u8,
                ServiceState::Stopped as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(ServiceState::from_raw)
    }

    pub fn stop_requested(&self) -> bool {
        matches!(self.state(), ServiceState::StopPending | ServiceState::Stopped)
    }

    /// Maps SCM control notifications to deterministic state transitions.
    /// Interrogate requests a status report without changing state.
    pub fn handle_control(&self, control: ServiceControl) -> ControlResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let changed = self.request_stop();
                ControlResult { state: self.state(), report_status: changed, accepted: true }
            }
            ServiceControl::Interrogate => ControlResult {
                state: self.state(),
                report_status: true,
                accepted: true,
            },
            ServiceControl::Unsupported => ControlResult {
                state: self.state(),
                report_status: false,
                accepted: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_start_stop_sequence_is_monotonic() {
        let lifecycle = ServiceLifecycle::new();
        assert_eq!(lifecycle.state(), ServiceState::StartPending);
        assert!(!lifecycle.stop_requested());
        lifecycle.mark_running().unwrap();
        assert_eq!(lifecycle.state(), ServiceState::Running);
        assert!(lifecycle.request_stop());
        assert_eq!(lifecycle.state(), ServiceState::StopPending);
        assert!(lifecycle.stop_requested());
        lifecycle.mark_stopped().unwrap();
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
        assert!(lifecycle.stop_requested());
    }

    #[test]
    fn stop_is_idempotent_and_start_cannot_resurrect() {
        let lifecycle = ServiceLifecycle::new();
        assert!(lifecycle.request_stop());
        assert!(!lifecycle.request_stop());
        assert_eq!(lifecycle.mark_running(), Err(ServiceState::StopPending));
        lifecycle.mark_stopped().unwrap();
        assert_eq!(lifecycle.mark_running(), Err(ServiceState::Stopped));
    }

    #[test]
    fn interrogate_reports_without_changing_state() {
        let lifecycle = ServiceLifecycle::new();
        let result = lifecycle.handle_control(ServiceControl::Interrogate);
        assert!(result.accepted);
        assert!(result.report_status);
        assert_eq!(result.state, ServiceState::StartPending);
        assert_eq!(lifecycle.state(), ServiceState::StartPending);
    }

    #[test]
    fn stop_and_shutdown_controls_share_the_same_transition() {
        let lifecycle = ServiceLifecycle::new();
        let stop = lifecycle.handle_control(ServiceControl::Stop);
        assert!(stop.accepted && stop.report_status);
        assert_eq!(stop.state, ServiceState::StopPending);
        let shutdown = lifecycle.handle_control(ServiceControl::Shutdown);
        assert!(shutdown.accepted);
        assert!(!shutdown.report_status);
        assert_eq!(shutdown.state, ServiceState::StopPending);
    }

    #[test]
    fn unsupported_controls_are_not_acknowledged() {
        let lifecycle = ServiceLifecycle::new();
        let result = lifecycle.handle_control(ServiceControl::Unsupported);
        assert!(!result.accepted);
        assert!(!result.report_status);
        assert_eq!(result.state, ServiceState::StartPending);
    }

    #[test]
    fn stopped_state_cannot_be_reopened() {
        let lifecycle = ServiceLifecycle::new();
        lifecycle.request_stop();
        lifecycle.mark_stopped().unwrap();
        assert!(!lifecycle.request_stop());
        assert_eq!(lifecycle.state(), ServiceState::Stopped);
    }
}
