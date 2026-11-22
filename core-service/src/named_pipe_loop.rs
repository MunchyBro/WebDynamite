//! Cancellable orchestration for bounded named-pipe accept operations.
//!
//! The service lifecycle owns the cancellation flag. The supplied step must
//! perform one bounded accept/connection operation and return TimedOut when no
//! client arrived; it must never block indefinitely. This module does not start
//! automatically and does not invent service identity/configuration.
use std::{sync::atomic::{AtomicBool, Ordering}, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptLoopStep {
    /// One connection was handled (successfully or with a protocol-level reply).
    ConnectionHandled,
    /// The bounded accept window elapsed without a client.
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptLoopExit {
    /// Shutdown was requested between bounded accept operations.
    Cancelled,
}

const MAX_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Runs bounded accept steps until cancellation or a fatal handler error.
///
/// Cancellation is observed between steps. Therefore `accept_step` MUST honor
/// its supplied timeout and clean up any pending OVERLAPPED operation before
/// returning. The interval is clamped to 1ms..=10s to bound shutdown latency.
pub fn run_cancellable_accept_loop<F>(
    cancellation: &AtomicBool,
    poll_interval: Duration,
    mut accept_step: F,
) -> Result<AcceptLoopExit, String>
where
    F: FnMut(u32) -> Result<AcceptLoopStep, String>,
{
    let interval = poll_interval.max(Duration::from_millis(1)).min(MAX_POLL_INTERVAL);
    let timeout_ms = interval.as_millis().max(1).min(u32::MAX as u128) as u32;

    while !cancellation.load(Ordering::Acquire) {
        match accept_step(timeout_ms)? {
            AcceptLoopStep::ConnectionHandled | AcceptLoopStep::TimedOut => {}
        }
    }

    Ok(AcceptLoopExit::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn exits_after_cancellation_without_an_extra_accept() {
        let cancellation = AtomicBool::new(false);
        let mut calls = 0;
        let result = run_cancellable_accept_loop(&cancellation, Duration::from_millis(250), |timeout| {
            assert_eq!(timeout, 250);
            calls += 1;
            cancellation.store(true, Ordering::Release);
            Ok(AcceptLoopStep::TimedOut)
        }).unwrap();
        assert_eq!(result, AcceptLoopExit::Cancelled);
        assert_eq!(calls, 1);
    }

    #[test]
    fn propagates_fatal_accept_errors() {
        let cancellation = AtomicBool::new(false);
        let result = run_cancellable_accept_loop(&cancellation, Duration::from_millis(1), |_| {
            Err("pipe security descriptor rejected".into())
        });
        assert_eq!(result.unwrap_err(), "pipe security descriptor rejected");
    }

    #[test]
    fn clamps_poll_interval_to_a_bounded_windows_wait() {
        let cancellation = AtomicBool::new(false);
        let mut observed = 0;
        let _ = run_cancellable_accept_loop(&cancellation, Duration::from_secs(60), |timeout| {
            observed = timeout;
            cancellation.store(true, Ordering::Release);
            Ok(AcceptLoopStep::ConnectionHandled)
        }).unwrap();
        assert_eq!(observed, 10_000);
    }
}
