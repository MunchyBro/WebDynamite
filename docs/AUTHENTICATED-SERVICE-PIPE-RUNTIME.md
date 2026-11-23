# Authenticated Service Pipe Runtime Composition

Phase 133 adds a service-owned runtime wrapper around the existing authenticated,
bounded named-pipe accept loop and the protected secret IPC dispatcher.

## Runtime ownership

`service_pipe_runtime::start_authenticated_secret_pipe_worker`:

- accepts a `TrustedServiceConfig` that has already been loaded through
  `load_trusted_service_config` using a service SID obtained independently
  from the running service token;
- revalidates the strict config shape before starting;
- starts a dedicated named worker thread;
- creates no listener until the existing overlapped host has authenticated the
  interactive user's SID, session, exact desktop executable, and pinned
  Authenticode publisher;
- dispatches only the existing authorized secret operations;
- requests lifecycle stop if the worker encounters a fatal runtime error.

`wait_for_service_stop` blocks the SCM service-main thread while the worker
runs independently. The returned `AuthenticatedSecretPipeWorker` owns the
cancellation signal and thread join. Its `stop` method requests cancellation
and joins after the overlapped accept timeout; the timeout is clamped to
100 ms through 10 seconds.

## Intended lifecycle composition

The SCM host can compose these pieces in its existing start/wait/stop hooks:

1. **Start:** obtain trusted service identity from the service token, load the
   ACL-validated config, then start the pipe worker and retain it in
   service-owned state.
2. **Wait:** call `wait_for_service_stop` with the lifecycle supplied by
   `run_service_main`.
3. **Stop:** take the worker from service-owned state and call `stop`.

The worker is not detached. Startup failure must roll back any partially
initialized state. A fatal pipe worker error requests service stop so SCM does
not remain in Running while IPC is unavailable.

## Deliberate activation boundary

This phase does **not** call `dispatch_service_lifecycle` from `main`, change
the current interactive HTTP startup path, or install/enable the Windows
service. It also does not invent the trusted service SID source. The caller
must still obtain that SID from native service-token context, and the installer
must provision the service identity, protected config/data/secret roots,
publisher identity, and desktop session policy before this runtime is used.

Production activation remains blocked on independently verified service-token
identity acquisition, signer provisioning, service-account secret migration,
service SID/DACL and installer lifecycle validation, and end-to-end Windows
testing. Do not populate trusted configuration from ordinary environment
variables or user-writable files.
