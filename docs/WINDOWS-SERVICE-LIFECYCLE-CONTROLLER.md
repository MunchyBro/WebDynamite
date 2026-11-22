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
