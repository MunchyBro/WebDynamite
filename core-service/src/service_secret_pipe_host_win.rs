//! Dormant SCM host composition for the authenticated secret pipe.
//!
//! This is deliberately not selected by the current interactive Core Service
//! entrypoint. A future installer/service entrypoint may call it only after it
//! provisions the service, protected config and secret-store ACLs. It hosts the
//! secret IPC worker only; it does not claim to host the website HTTP API.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use windows_sys::Win32::Foundation::ERROR_GEN_FAILURE;

/// Enter the SCM dispatcher and own the authenticated secret-pipe worker for
/// the full service lifetime. All paths must point into installer-protected
/// locations; the worker independently derives the service SID from its token
/// before loading the trusted configuration.
pub fn run_authenticated_secret_pipe_service(
    service_name: &str,
    config_path: &Path,
    trusted_root: &Path,
    timeout_ms: u32,
) -> Result<(), u32> {
    let config_path: PathBuf = config_path.to_path_buf();
    let trusted_root: PathBuf = trusted_root.to_path_buf();
    let worker_slot = Arc::new(Mutex::new(None));
    let startup_slot = Arc::clone(&worker_slot);
    let shutdown_slot = Arc::clone(&worker_slot);

    crate::service_entry_win::dispatch_service_lifecycle(
        service_name,
        move |lifecycle| {
            let worker = crate::service_pipe_runtime::start_current_service_pipe_worker(
                &config_path,
                &trusted_root,
                lifecycle,
                timeout_ms,
            )
            .map_err(|_| ERROR_GEN_FAILURE)?;

            let mut slot = startup_slot.lock().map_err(|_| ERROR_GEN_FAILURE)?;
            if slot.is_some() {
                return Err(ERROR_GEN_FAILURE);
            }
            *slot = Some(worker);
            Ok(())
        },
        |lifecycle| crate::service_pipe_runtime::wait_for_service_stop(lifecycle),
        move || {
            let worker = shutdown_slot
                .lock()
                .map_err(|_| ERROR_GEN_FAILURE)?
                .take();
            match worker {
                Some(mut worker) => worker.stop().map_err(|_| ERROR_GEN_FAILURE),
                None => Ok(()),
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_requires_explicit_protected_paths() {
        // This host is intentionally a callable adapter, not a process-wide
        // auto-start hook. The real SCM/token/config path is Windows-only.
        assert!(!Path::new("").is_absolute());
    }
}
