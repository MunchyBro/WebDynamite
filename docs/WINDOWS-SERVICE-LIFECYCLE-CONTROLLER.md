# Windows Service lifecycle controller

Phase 117 introduces a host-neutral, atomic service lifecycle state machine.
It is intended for a future SCM dispatcher, but does not register a service,
call StartServiceCtrlDispatcher, or enable Windows Service mode.

## State transitions

`StartPending -> Running -> StopPending -> Stopped`

A stop or shutdown request may also move directly from StartPending to
StopPending. Transitions are monotonic: startup cannot resurrect a service once
shutdown begins, repeated stop requests are idempotent, and Stopped is final.

## Control mapping

- STOP and SHUTDOWN request StopPending and ask the SCM host to publish status
  when the state changes.
- INTERROGATE requests a fresh status report without changing state.
- Unsupported controls are explicitly rejected by this contract.
- The host marks Stopped only after workers and child processes have completed
  shutdown.

The atomic state is also the cancellation signal for bounded workers. A future
pipe accept loop can poll `stop_requested()` between bounded accept/I/O steps;
it must still cancel and reap any pending OVERLAPPED operation before returning.

## Activation boundary

This module is a tested state-machine primitive only. It does not create SCM
handles, publish SERVICE_STATUS, handle preshutdown notifications, implement
shutdown deadlines, or start the named-pipe listener. Do not activate it in
production until trusted config loading is wired to a trusted service identity,
publisher signing is provisioned, secret migration is complete, and installer
install/upgrade/rollback/repair/uninstall tests pass. The current interactive
Core Service startup remains unchanged.


## SCM status projection (Phase 122 foundation)

The `service_status` module maps the host-neutral lifecycle state to a
C-layout-compatible seven-DWORD `SERVICE_STATUS` snapshot. StartPending and
StopPending publish checkpoint 1 with a 30-second wait hint; Running alone
accepts STOP and SHUTDOWN; Stopped publishes no accepted controls or pending
fields. The projection is platform-independent and unit-tested.

This is not yet an SCM dispatcher and does not call
`StartServiceCtrlDispatcherW`, `RegisterServiceCtrlHandlerExW`, or
`SetServiceStatus`. The interactive HTTP process and named-pipe activation
remain unchanged. A subsequent integration must publish these snapshots from
a real dispatcher and connect stop notifications to bounded worker cancellation
before service mode can be enabled.


## SCM control callback contract (Phase 122)

The `service_control` module translates raw STOP (1), INTERROGATE (4), and
SHUTDOWN (5) control codes into lifecycle transitions and optional status
snapshots. Unknown controls are rejected. Duplicate STOP/SHUTDOWN requests are
idempotent; only the first transition asks for a new pending status snapshot.
The callback contract explicitly signals shutdown rather than joining workers,
waiting on I/O, or killing child processes inside the SCM callback thread.

The translator is platform-independent and concurrency-tested. It is not yet
registered with `RegisterServiceCtrlHandlerExW`; the actual callback must
remain non-blocking and the service worker must perform bounded cleanup before
publishing SERVICE_STOPPED.


## Native SCM status publisher (Phase 123)

The Windows-only `service_status_win` adapter converts the tested seven-DWORD
snapshot into the SDK `SERVICE_STATUS` structure and publishes it through
`SetServiceStatus`. It rejects a null status handle and returns the native
Win32 error when publication fails. The adapter is intentionally a narrow
primitive: it does not acquire/register a service status handle, start an SCM
dispatcher, or enable service mode. A future service-main/handler integration
must own the handle lifetime and publish every lifecycle transition from the
registered service context.


## Native SCM dispatcher entry (Phase 124)

The Windows-only `service_dispatch_win` module provides a narrow wrapper around
`StartServiceCtrlDispatcherW`. It validates the service name, builds the required
UTF-16 service table plus its terminating sentinel, and keeps the name buffer alive
for the full synchronous dispatcher call. The caller supplies a static service-main
callback. Native failures are returned as Win32 error codes.

This is a dispatcher entry primitive, not service activation: no service-main
callback is registered by the application yet, no control handler is installed,
and the existing interactive HTTP startup remains the only active runtime path.
The next integration must implement service-main registration, status-handle
ownership, non-blocking control handling, and bounded worker shutdown before any
SCM mode switch is enabled.


## Native SCM control-handler registration (Phase 125)

The Windows-only `service_handler_win` module registers an extended control
handler with `RegisterServiceCtrlHandlerExW`. It owns a stable callback context
containing the shared lifecycle controller and the status handle returned by
the SCM. Keep the returned `RegisteredServiceHandler` alive for the complete
service-main lifetime.

The callback translates STOP, SHUTDOWN, and INTERROGATE using the existing
platform-neutral control contract. When a status snapshot is requested, it
publishes that snapshot with the native status adapter. Unsupported controls
return `ERROR_CALL_NOT_IMPLEMENTED`; null/invalid status handles and native
publication errors are propagated. Panics are contained at the FFI boundary.

The callback never joins workers, waits for I/O, or terminates child processes.
STOP/SHUTDOWN only transition the shared lifecycle state; service-main must
observe that state and perform bounded cleanup on its own worker thread.

This phase supplies the registration and callback primitive, not a complete
service-main implementation. The dispatcher still has no production callback,
the named-pipe runtime is not started in SCM mode, and interactive HTTP startup
remains unchanged. Do not enable service mode until service-main orchestration,
trusted identity/config validation, credential migration, signing, and
installer lifecycle validation are complete.


## Testable service-main lifecycle orchestration (Phase 128)

The Windows SCM adapter now delegates lifecycle sequencing to a status-sink
runner. The native entry point supplies the real `SetServiceStatus` publisher;
unit tests inject a deterministic publisher instead. This keeps the service
state transitions and teardown guarantees testable without connecting to the
SCM or enabling service mode.

The startup hook receives the shared lifecycle controller, allowing it to
observe a STOP/SHUTDOWN received while startup is in progress. Tests cover
normal start/stop, startup failure, stop during startup, RUNNING publication
failure, STOP_PENDING publication failure, and runtime teardown errors. A
failed RUNNING/STOP_PENDING report must not skip runtime teardown.
