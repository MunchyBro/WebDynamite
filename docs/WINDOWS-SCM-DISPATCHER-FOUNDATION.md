# Windows SCM dispatcher foundation

This phase adds a small native boundary around the Windows Service Control
Manager (SCM) APIs:

- Enter an SCM service table through `StartServiceCtrlDispatcherW`.
- Register an extended service control handler.
- Publish lifecycle snapshots through `SetServiceStatus`.
- Map the existing host-neutral lifecycle states to SCM states.
- Accept STOP and SHUTDOWN only while Running; publish no accepted controls
  while StartPending or StopPending.
- Clamp pending wait hints to 1–60 seconds and force nonzero checkpoints.

The Core Service executable does **not** call this dispatcher yet. Its existing
interactive loopback-HTTP startup remains unchanged. This is intentional:
registering a service entry point before trusted configuration, service-token
identity validation, worker supervision, stop coordination, installer
registration, and rollback are complete could strand a partially initialized
service or break existing desktop startup.

## Not included

- No SCM service registration, start, stop, upgrade, repair, rollback, or
  uninstall commands.
- No service-mode switch in `main`.
- No activation of the named-pipe server loop.
- No loading of service configuration from an untrusted path or environment.
- No Credential Manager migration or source credential deletion.
- No claim that a production Windows Service is ready.

## Next integration gates

1. Implement a service-main coordinator that obtains service identity from
   the SCM service token, loads the installer-provisioned config through the
   trusted handle/ACL chain, and compares the configured service SID.
2. Bind STOP/SHUTDOWN controls to the existing lifecycle cancellation signal
   and bounded worker shutdown; publish STOP_PENDING checkpoints while draining.
3. Initialize protected data paths and the authenticated named-pipe loop only
   after trusted config, signer, and DACL checks pass.
4. Add installer-managed service registration and Windows VM tests for install,
   start, status, stop, recovery, upgrade, rollback, repair, and uninstall.
5. Keep interactive startup as the default until all service-mode E2E gates pass.
