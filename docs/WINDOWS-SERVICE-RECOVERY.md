# Windows Core Service startup and recovery

A signed per-machine WebDynamite installation registers `WebDynamiteCore` under the dedicated `NT SERVICE\\WebDynamiteCore` virtual service account. It is not a LocalSystem service and the management API remains bound to loopback.

## Startup policy

The installer configures automatic machine startup. The Core Service can therefore start before an interactive desktop session and continue hosting websites after the UI closes or a user signs out.

## Bounded failure recovery

SCM recovery actions are configured as follows:

| Failure | SCM action |
|---|---:|
| First failure | Restart after 5 seconds |
| Second failure | Restart after 15 seconds |
| Subsequent failure | Restart after 60 seconds |
| Failure counter reset | 24 hours |

Recovery actions are also enabled for non-zero service process exits. This is a bounded retry policy, not an unlimited restart loop. A persistently failing service will stop restarting after the configured actions are exhausted until the reset period or a manual service start.

## Installer behavior

Install and repair re-apply the startup and recovery policy idempotently. Upgrade stops the exact registered service only after verifying its service identity and executable path, then re-applies the policy after replacing files. Uninstall unregisters the service while preserving the database, configuration, and machine-protected secrets under ProgramData.

Unsigned engineering installers remain desktop-only and do not register or activate the privileged SCM service.

## Validation and production gate

The Windows Installer workflow runs deterministic PowerShell self-tests for policy generation and output verification. It also creates a uniquely named disposable Windows service on the hosted Windows runner, applies the real SCM startup/recovery configuration, queries it back, and removes the test service before building the NSIS installer. A passing CI run proves the script contract, real SCM configuration/query round-trip, and bundle build; it does not prove that a real machine starts after reboot or recovers from an actual crash.

Before production readiness, run disposable-VM tests for first install and reboot, induced service crash and recovery, upgrade with existing data/secrets, interrupted upgrade, uninstall, and reinstall. Verify that websites and backups remain available while the desktop is closed, and that no user data is removed by default.