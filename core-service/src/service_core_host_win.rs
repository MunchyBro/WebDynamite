//! Full Core Service Windows SCM host.
//!
//! Service mode is fail-closed: the current process token supplies the service
//! SID, installer-owned configuration is ACL-validated, and the API token is
//! loaded only from the machine-DPAPI secret store. No environment or command
//! line secret provisioning is accepted here.

#[cfg(windows)]
mod windows_impl {
    use std::{
        path::Path,
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
        time::Duration,
    };
    use zeroize::{Zeroize, Zeroizing};
    use windows_sys::Win32::Foundation::{ERROR_GEN_FAILURE, ERROR_INVALID_DATA};

    const SERVICE_NAME: &str = "WebDynamiteCore";
    const TRUSTED_ROOT: &str = r"C:\ProgramData\WebDynamite";
    const CONFIG_PATH: &str = r"C:\ProgramData\WebDynamite\config\service.json";
    const API_TOKEN_NAME: &str = "api-token";

    struct CoreRuntimeWorker {
        shutdown: Option<tokio::sync::oneshot::Sender<()>>,
        thread: Option<JoinHandle<Result<(), String>>>,
        pipe_worker: Option<crate::service_pipe_runtime::AuthenticatedSecretPipeWorker>,
    }

    /// Enters SCM dispatch for the full HTTP/API Core Service, not merely the
    /// authenticated secret-pipe worker. Installer provisioning must create the
    /// trusted config, service SID ACLs and DPAPI-protected api-token first.
    pub fn run_core_service_scm() -> Result<(), u32> {
        let slot = Arc::new(Mutex::new(None::<CoreRuntimeWorker>));
        let start_slot = Arc::clone(&slot);
        let stop_slot = Arc::clone(&slot);

        crate::service_entry_win::dispatch_service_lifecycle(
            SERVICE_NAME,
            move |lifecycle| {
                let config = crate::service_identity_win::load_current_service_config(
                    Path::new(CONFIG_PATH),
                    Path::new(TRUSTED_ROOT),
                ).map_err(|_| ERROR_INVALID_DATA)?;
                // Start the authenticated desktop secret channel under the same
                // SCM lifecycle. The worker validates the desktop process identity
                // from trusted installer configuration before reading any request.
                let runtime_secret_store_root = config.secret_store_root.clone();
                let mut pipe_worker = crate::service_pipe_runtime::start_authenticated_secret_pipe_worker(
                    config.clone(),
                    Arc::clone(&lifecycle),
                    1_000,
                ).map_err(|_| ERROR_INVALID_DATA)?;

                let mut token_bytes = match crate::secret_store::read_secret(
                    &config.secret_store_root,
                    API_TOKEN_NAME,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let _ = pipe_worker.stop();
                        return Err(ERROR_INVALID_DATA);
                    }
                };
                if std::str::from_utf8(&token_bytes).is_err() {
                    token_bytes.zeroize();
                    let _ = pipe_worker.stop();
                    return Err(ERROR_INVALID_DATA);
                }
                let token_text = match String::from_utf8(std::mem::take(&mut token_bytes)) {
                    Ok(value) => value,
                    Err(_) => {
                        let _ = pipe_worker.stop();
                        return Err(ERROR_INVALID_DATA);
                    }
                };
                let mut token = Zeroizing::new(token_text);
                if token.trim().len() < 32 {
                    let _ = pipe_worker.stop();
                    return Err(ERROR_INVALID_DATA);
                }

                let data_root = config.data_root;
                let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
                let runtime_lifecycle = Arc::clone(&lifecycle);
                let runtime_thread = match thread::Builder::new()
                    .name("webdynamite-core-runtime".into())
                    .spawn(move || {
                        let runtime = tokio::runtime::Builder::new_multi_thread()
                            .enable_all()
                            .build()
                            .map_err(|_| "Could not create the Core Service Tokio runtime.".to_string())?;
                        let runtime_token = std::mem::take(&mut *token);
                        let result = runtime.block_on(crate::run_core_service_at(runtime_token, data_root, Some(runtime_secret_store_root), async {
                            let _ = shutdown_rx.await;
                        })).map_err(|error| error.to_string());
                        runtime_lifecycle.request_stop();
                        result
                    }) {
                    Ok(worker) => worker,
                    Err(_) => {
                        let _ = pipe_worker.stop();
                        return Err(ERROR_GEN_FAILURE);
                    }
                };

                let mut guard = match start_slot.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        let _ = pipe_worker.stop();
                        let _ = runtime_thread.join();
                        return Err(ERROR_GEN_FAILURE);
                    }
                };
                if guard.is_some() {
                    let _ = shutdown_tx.send(());
                    let _ = pipe_worker.stop();
                    let _ = runtime_thread.join();
                    return Err(ERROR_GEN_FAILURE);
                }
                *guard = Some(CoreRuntimeWorker {
                    shutdown: Some(shutdown_tx),
                    thread: Some(runtime_thread),
                    pipe_worker: Some(pipe_worker),
                });
                Ok(())
            },
            |lifecycle| {
                while !lifecycle.stop_requested() {
                    thread::sleep(Duration::from_millis(50));
                }
            },
            move || {
                let worker = stop_slot.lock().map_err(|_| ERROR_GEN_FAILURE)?.take();
                let Some(mut worker) = worker else { return Ok(()); };
                if let Some(shutdown) = worker.shutdown.take() {
                    let _ = shutdown.send(());
                }
                let pipe_result = worker.pipe_worker.take()
                    .map(|mut pipe| pipe.stop())
                    .unwrap_or(Ok(()));
                let runtime_result = match worker.thread.take() {
                    Some(handle) => match handle.join() {
                        Ok(Ok(())) => Ok(()),
                        Ok(Err(_)) | Err(_) => Err(ERROR_GEN_FAILURE),
                    },
                    None => Ok(()),
                };
                if pipe_result.is_err() || runtime_result.is_err() {
                    Err(ERROR_GEN_FAILURE)
                } else {
                    Ok(())
                }
            },
        )
    }
}

#[cfg(windows)]
pub use windows_impl::run_core_service_scm;

#[cfg(not(windows))]
pub fn run_core_service_scm() -> Result<(), u32> {
    Err(31) // ERROR_GEN_FAILURE: SCM hosting is Windows-only.
}
