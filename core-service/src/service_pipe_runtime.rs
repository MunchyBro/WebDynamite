//! Service-owned authenticated named-pipe runtime composition.
//!
//! The caller must obtain `TrustedServiceConfig` through the trusted loader
//! using a service SID obtained independently from the running service token.
//! This module never reads identity values from environment variables or
//! activates SCM service mode by itself.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    service_lifecycle::ServiceLifecycle,
    trusted_service_config::TrustedServiceConfig,
};

const MIN_ACCEPT_TIMEOUT_MS: u32 = 100;
const MAX_ACCEPT_TIMEOUT_MS: u32 = 10_000;

/// Owns the bounded authenticated pipe worker and its cancellation signal.
/// Keep this value alive for the entire SCM service instance.
pub struct AuthenticatedSecretPipeWorker {
    cancellation: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
}

impl AuthenticatedSecretPipeWorker {
    /// Requests cancellation and joins the worker. The overlapped accept/read
    /// timeout bounds how long the worker can take to observe this request.
    pub fn stop(&mut self) -> Result<(), String> {
        self.cancellation.store(true, Ordering::Release);
        match self.thread.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| "Authenticated named-pipe worker panicked.".to_string())?,
            None => Ok(()),
        }
    }
}

impl Drop for AuthenticatedSecretPipeWorker {
    fn drop(&mut self) {
        self.cancellation.store(true, Ordering::Release);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

/// Blocks the SCM service-main thread until STOP/SHUTDOWN changes lifecycle state.
/// The pipe worker runs independently and is cancelled by its owner in teardown.
pub fn wait_for_service_stop(lifecycle: Arc<ServiceLifecycle>) {
    while !lifecycle.stop_requested() {
        thread::sleep(Duration::from_millis(50));
    }
}

/// Starts the authenticated secret IPC loop using an already-loaded, trusted
/// installer configuration. Config loading and independent service-SID
/// verification must happen before this function is called.
#[cfg(windows)]
pub fn start_authenticated_secret_pipe_worker(
    config: TrustedServiceConfig,
    lifecycle: Arc<ServiceLifecycle>,
    timeout_ms: u32,
) -> Result<AuthenticatedSecretPipeWorker, String> {
    config.validate()?;
    let service_sid = config.service_sid.clone();
    let client_user_sid = config.interactive_user_sid.clone();
    let session_id = config.interactive_session_id;
    let desktop_executable = config.desktop_executable.clone();
    let publisher_sha256 = config.publisher_sha256.clone();
    let secret_store_root = config.secret_store_root.clone();
    let cancellation = Arc::new(AtomicBool::new(false));
    let worker_cancellation = Arc::clone(&cancellation);

    let thread = thread::Builder::new()
        .name("webdynamite-authenticated-pipe".into())
        .spawn(move || {
            let result = crate::named_pipe_host::serve_authenticated_overlapped_loop(
                &service_sid,
                &client_user_sid,
                session_id,
                &desktop_executable,
                &publisher_sha256,
                timeout_ms.clamp(MIN_ACCEPT_TIMEOUT_MS, MAX_ACCEPT_TIMEOUT_MS),
                &worker_cancellation,
                move |request| {
                    crate::secret_ipc::dispatch_secret_request(&secret_store_root, &request)
                },
            );
            if result.is_err() {
                // A fatal worker failure must not leave SCM reporting Running.
                lifecycle.request_stop();
            }
            result
        })
        .map_err(|_| "Could not start the authenticated named-pipe worker thread.".to_string())?;

    Ok(AuthenticatedSecretPipeWorker {
        cancellation,
        thread: Some(thread),
    })
}

#[cfg(not(windows))]
pub fn start_authenticated_secret_pipe_worker(
    _config: TrustedServiceConfig,
    _lifecycle: Arc<ServiceLifecycle>,
    _timeout_ms: u32,
) -> Result<AuthenticatedSecretPipeWorker, String> {
    Err("The authenticated service pipe runtime is available only on Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_lifecycle::ServiceState;

    #[test]
    fn service_wait_observes_stop_without_owning_pipe_worker() {
        let lifecycle = Arc::new(ServiceLifecycle::new());
        lifecycle.mark_running().unwrap();
        let waiter = Arc::clone(&lifecycle);
        let thread = std::thread::spawn(move || wait_for_service_stop(waiter));
        lifecycle.request_stop();
        thread.join().unwrap();
        assert_eq!(lifecycle.state(), ServiceState::StopPending);
    }

    #[cfg(not(windows))]
    #[test]
    fn runtime_fails_closed_off_windows() {
        let config = TrustedServiceConfig {
            version: 1,
            service_sid: "S-1-5-80-1-2-3-4-5".into(),
            interactive_user_sid: "S-1-5-21-1-2-3-1001".into(),
            interactive_session_id: 1,
            desktop_executable: std::path::PathBuf::from("/opt/WebDynamite.exe"),
            publisher_sha256: "a".repeat(64),
            secret_store_root: std::path::PathBuf::from("/var/lib/webdynamite/secrets"),
            data_root: std::path::PathBuf::from("/var/lib/webdynamite/data"),
        };
        let result = start_authenticated_secret_pipe_worker(
            config,
            Arc::new(ServiceLifecycle::new()),
            1_000,
        );
        assert!(matches!(result, Err(ref message) if message.contains("only on Windows")));
    }
}
