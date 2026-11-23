# Windows SCM Service Entrypoint Bridge

The Core Service now has a narrow Windows-only bridge from the native Service
Control Manager (SCM) service-main callback to an owned Rust closure.

## Contract

- The bridge accepts exactly one pending service-main closure per process.
- The closure is stored before entering `StartServiceCtrlDispatcherW`.
- The SCM callback takes ownership of the closure and runs it on the service-main
  thread, without holding the global registry mutex.
- The dispatcher call is synchronous; its return value is propagated to the
  caller after the service callback completes.
- Panics from the closure are contained and converted to
  `ERROR_GEN_FAILURE`; no Rust unwind may cross the Win32 callback ABI.
- A second concurrent dispatch attempt fails with
  `ERROR_ALREADY_EXISTS`.

## Deliberate safety boundary

This bridge does not itself enable Windows Service mode. The current Core
Service still starts through its existing process entrypoint and loopback HTTP
runtime. Do not call `dispatch_service` from production startup until all of
the following are implemented and verified together:

1. A trusted service configuration has been loaded through the ACL- and
   handle-validated path.
2. The pinned publisher identity and service executable identity are provisioned.
3. Runtime state and secrets are available to the service virtual account.
4. The authenticated named-pipe server is wired to the service runtime.
5. The SCM worker lifecycle owns startup, wait, and bounded teardown of the
   actual Core Service runtime.
6. Installer, upgrade, rollback, and service-SID/DACL validation pass on Windows.

The bridge is an entrypoint primitive, not a substitute for those preconditions.
