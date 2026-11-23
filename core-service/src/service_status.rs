//! Pure Windows SCM SERVICE_STATUS projection for the lifecycle controller.
//!
//! Keeping this mapping independent of Win32 calls makes the status contract
//! deterministic and testable on every CI platform. The future dispatcher must
//! pass these snapshots to SetServiceStatus and advance checkpoints while doing
//! bounded startup/shutdown work.

use crate::service_lifecycle::ServiceState;

/// Win32 SERVICE_* constants from winsvc.h.
pub const SERVICE_WIN32_OWN_PROCESS: u32 = 0x0000_0010;
pub const SERVICE_ACCEPT_STOP: u32 = 0x0000_0001;
pub const SERVICE_ACCEPT_SHUTDOWN: u32 = 0x0000_0004;

pub const SERVICE_STOPPED: u32 = 0x0000_0001;
pub const SERVICE_START_PENDING: u32 = 0x0000_0002;
pub const SERVICE_STOP_PENDING: u32 = 0x0000_0003;
pub const SERVICE_RUNNING: u32 = 0x0000_0004;

/// Layout-compatible with Win32 SERVICE_STATUS (all fields are DWORD).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ServiceStatusSnapshot {
    pub service_type: u32,
    pub current_state: u32,
    pub controls_accepted: u32,
    pub win32_exit_code: u32,
    pub service_specific_exit_code: u32,
    pub checkpoint: u32,
    pub wait_hint_ms: u32,
}

impl ServiceStatusSnapshot {
    /// Construct a status update from the authoritative lifecycle state.
    /// Pending states use a non-zero checkpoint and bounded 30s wait hint;
    /// running is the only state that accepts STOP and SHUTDOWN controls.
    pub const fn from_state(state: ServiceState) -> Self {
        let (current_state, controls_accepted, checkpoint, wait_hint_ms) = match state {
            ServiceState::StartPending => (SERVICE_START_PENDING, 0, 1, 30_000),
            ServiceState::Running => (
                SERVICE_RUNNING,
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN,
                0,
                0,
            ),
            ServiceState::StopPending => (SERVICE_STOP_PENDING, 0, 1, 30_000),
            ServiceState::Stopped => (SERVICE_STOPPED, 0, 0, 0),
        };
        Self {
            service_type: SERVICE_WIN32_OWN_PROCESS,
            current_state,
            controls_accepted,
            win32_exit_code: 0,
            service_specific_exit_code: 0,
            checkpoint,
            wait_hint_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_states_publish_checkpoints_and_wait_hints() {
        for state in [ServiceState::StartPending, ServiceState::StopPending] {
            let status = ServiceStatusSnapshot::from_state(state);
            assert_eq!(status.service_type, SERVICE_WIN32_OWN_PROCESS);
            assert_eq!(status.controls_accepted, 0);
            assert_eq!(status.checkpoint, 1);
            assert_eq!(status.wait_hint_ms, 30_000);
        }
    }

    #[test]
    fn only_running_accepts_stop_and_shutdown() {
        let status = ServiceStatusSnapshot::from_state(ServiceState::Running);
        assert_eq!(status.current_state, SERVICE_RUNNING);
        assert_eq!(
            status.controls_accepted,
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        );
        assert_eq!(status.checkpoint, 0);
        assert_eq!(status.wait_hint_ms, 0);
    }

    #[test]
    fn stopped_is_terminal_status_without_controls_or_pending_fields() {
        let status = ServiceStatusSnapshot::from_state(ServiceState::Stopped);
        assert_eq!(status.current_state, SERVICE_STOPPED);
        assert_eq!(status.controls_accepted, 0);
        assert_eq!(status.checkpoint, 0);
        assert_eq!(status.wait_hint_ms, 0);
    }
}
