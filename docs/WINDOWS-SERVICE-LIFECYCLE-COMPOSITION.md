# Windows SCM Lifecycle Composition

The Windows-only `dispatch_service_lifecycle` entrypoint composes three
existing primitives:

1. `StartServiceCtrlDispatcherW` registration through the dispatcher bridge.
2. SCM service-main callback execution on the SCM-provided thread.
3. Control-handler registration, status publication, and lifecycle-managed
   startup, stop-wait, and teardown through `run_service_main`.

The dispatcher and callback each own a stable service-name string for the
synchronous dispatcher lifetime. The service runtime hooks are moved into the
callback and execute without holding the global callback registry lock.

## What this does not activate

This is a composition layer only. It does not select trusted configuration,
create or validate the service virtual account, migrate secrets, or start the
Core Service HTTP/named-pipe runtime. Production service-mode activation remains
blocked until those security and installer prerequisites are implemented and
verified on Windows. Existing desktop-launched Core Service behavior is
unchanged.
