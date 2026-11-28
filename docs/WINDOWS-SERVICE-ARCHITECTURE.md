# Windows Core Service architecture

**Status:** SCM service mode is implemented for trusted signed per-machine installations; clean-machine lifecycle acceptance remains outstanding  
**Target:** Windows 10/11 x64  
**Scope:** Machine-startup hosting, service recovery, desktop attachment, and safe installer lifecycle

## Current behavior

A trusted signed per-machine installation registers the Core Service with Windows Service Control Manager (SCM) under the dedicated `NT SERVICE\\WebDynamiteCore` virtual account. The service is configured for automatic machine startup and bounded restart recovery (5 seconds, 15 seconds, then 60 seconds; the failure counter resets after 24 hours). The desktop attaches through the authenticated local bootstrap channel and does not own the service lifetime. Unsigned engineering builds remain desktop-only and can use the interactive child-process mode.

The desktop stores the Core Service API token in the signed-in user's Windows Credential Manager. The Core Service also uses Windows Credential Manager for protected website environment secrets. This is not directly compatible with moving the Core Service to a different Windows service identity.

## Goals and non-goals

### Goals

- Start the Core Service through the Windows Service Control Manager (SCM), including at machine startup.
- Configure bounded SCM recovery for unexpected service termination.
- Keep the desktop UI optional; closing it must not stop hosted websites or scheduled backups.
- Run the Core Service and website child processes without LocalSystem privileges.
- Keep the management API bound to loopback and require authentication.
- Preserve existing website, database, configuration, release, and backup data during install, upgrade, rollback, and uninstall.
- Make install, upgrade, repair, and uninstall idempotent and diagnosable.

### Non-goals

- Do not expose the management API to the LAN or public internet as part of service installation.
- Do not silently change website project permissions or move project files.
- Do not run website processes as LocalSystem.
- Do not remove the current interactive-process mode until the SCM mode has passed upgrade and recovery testing.

## Recommended service identity

Use a dedicated Windows virtual service account, `NT SERVICE\WebDynamiteCore`, rather than LocalSystem or a stored user password.

The installer must create the service with a stable service name and grant only the permissions required by that identity:

- Read and execute the installed Core Service binary and its bundled runtime assets.
- Modify the WebDynamite machine data directory under `%ProgramData%\WebDynamite`.
- Access only explicitly registered website project roots and the runtime files required by those projects.
- Write service logs and diagnostic output.

The installer must not grant broad write access to an entire user profile or drive. Project-root access must be explicit, reviewable, and reversible. If a selected project path cannot be granted safely, the UI must explain the limitation and leave that project unavailable to the service rather than weakening filesystem permissions.

Website child processes inherit the dedicated service identity, not an administrator or LocalSystem token. The service identity is a shared boundary for managed websites; this is not per-website OS isolation. That limitation must be documented.

## Authentication and secret storage

### Management API

Keep the HTTP listener on `127.0.0.1:47821`. Do not treat loopback binding as authentication: every management route must continue to require the bearer token, apart from the intentionally public health endpoint.

The existing user-scoped Credential Manager entry cannot be read by a virtual service account. Before SCM mode is enabled, introduce a machine-level credential handoff with all of these properties:

1. Generate a high-entropy random API token during installation or first service initialization.
2. Protect the token at rest with Windows DPAPI machine scope.
3. Store the protected blob in a dedicated file under the WebDynamite ProgramData directory.
4. Apply a restrictive DACL that grants read access only to the service SID, the interactive users explicitly supported by the installation, and local administrators.
5. Never put the plaintext token in service command-line arguments, installer logs, registry values readable by ordinary users, or environment variables inherited by unrelated processes.
6. The desktop obtains the token through a narrow, authenticated local handoff. Do not make the protected file broadly readable merely to simplify desktop startup.
7. Rotate credentials atomically and preserve a recovery path if an upgrade is interrupted.

The exact desktop-to-service handoff mechanism must be selected and threat-modeled before implementation. A named pipe with an explicit DACL and client identity checks is the preferred candidate. Do not add an unauthenticated token-disclosure endpoint to the existing HTTP API.

### Website and database secrets

The current per-user Credential Manager design for website environment secrets must be migrated before service mode is enabled. The service must own secret reads and writes; the desktop should submit secret values over the authenticated management channel and never need direct access to the service's secret store.

Use a service-owned encrypted secret store with restrictive filesystem ACLs and DPAPI protection. Define backup/restore behavior explicitly: secret material must not be exported as plaintext, and restore must re-protect secrets for the destination machine/service identity. Database profile passwords and website environment secrets must follow the same ownership model.

A migration must preserve existing user-scoped secrets where they can be read by the current desktop identity. If a secret cannot be migrated, surface a clear re-entry prompt; never silently replace it with an empty value.

## Service lifecycle

The SCM integration must:

- Register a real service entry point and report `START_PENDING`, `RUNNING`, `STOP_PENDING`, and terminal states correctly.
- Honor SCM stop requests and use the existing graceful shutdown path to stop managed website processes, tunnels, and the reverse proxy.
- Configure delayed automatic start only if startup dependencies require it; otherwise use automatic start.
- Configure bounded restart actions with a reset period and a non-zero delay. Avoid infinite rapid restart loops.
- Log startup failures and service-control errors to a service-owned log location and Windows Event Log.
- Detect an already-installed service and distinguish it from an unrelated process occupying port 47821.
- Prevent desktop startup from launching a second interactive Core Service when SCM mode is installed.
- Let the desktop attach to an already-running service without terminating it when the UI closes.

The existing `ctrl_c`-only shutdown future is not sufficient for SCM mode. Add a platform abstraction so console Ctrl+C and SCM stop controls both trigger the same graceful shutdown signal.

## Installer and upgrade lifecycle

Use elevated installer hooks for SCM registration and removal. The exact NSIS hook integration must be verified against the Tauri v2 bundler configuration used by this repository before adding scripts.

Installation must be transactional:

1. Validate the service executable and data directory.
2. Create or migrate the protected machine credential and secret store.
3. Apply the service identity ACLs.
4. Register and configure the service.
5. Start the service and verify authenticated health.
6. Report success only after the service is healthy; otherwise roll back service registration and preserve user data.

Upgrade must stop the service cleanly, replace binaries, preserve data and credentials, update service configuration, then restart and verify health. If restart fails, retain diagnostic logs and provide a repair/rollback path. Do not delete or recreate the ProgramData data directory during ordinary upgrades.

Uninstall must stop and unregister the service and remove installed binaries. It must preserve user data by default. Any optional data deletion must be an explicit, separately confirmed action. A failed service stop must not trigger forced deletion of files that may still be in use.

## Implementation sequence and acceptance gates

1. **Security primitives:** implement and test DPAPI blob protection, DACL construction/verification, atomic credential-file replacement, and named-pipe identity checks. Keep these behind Windows-only modules with non-Windows compile stubs where needed.
2. **Secret migration:** move API-token and website/database secret ownership to the service; test migration, missing credentials, rotation, and backup/restore.
3. **SCM entry point:** add service dispatch and a shared graceful-shutdown abstraction; test service state transitions and stop behavior on Windows.
4. **Installer integration:** add elevated install/upgrade/uninstall hooks and idempotent service configuration; test fresh install, upgrade from the detached-process build, repair, uninstall, and interrupted operations.
5. **Desktop attachment:** detect SCM mode, authenticate through the approved handoff, attach without spawning a duplicate, and show actionable errors when the service is stopped or unhealthy.
6. **Recovery and soak tests:** reboot without signing in, sign in/out, close/reopen the desktop, kill the service, simulate startup failure, and verify bounded SCM recovery without data loss or duplicate listeners.

SCM mode is not production-ready until all gates pass on Windows 10 and Windows 11 x64. Linux CI or a successful Rust compile alone cannot validate service installation, Windows ACLs, DPAPI, or SCM recovery.

## Security invariants

- The Core Service never runs as LocalSystem.
- The management listener remains loopback-only and authenticated.
- No plaintext API token or saved website/database password is written to logs or command-line arguments.
- No service installation step grants broad permissions to a user profile, drive root, or arbitrary project tree.
- No ordinary upgrade or uninstall removes user data.
- A failed migration never silently discards a secret or changes a website's runtime identity.
- The detached-process mode remains available until SCM install, migration, upgrade, and recovery tests pass.


## SCM startup and recovery policy

The installer applies and verifies the following policy whenever it installs or repairs the signed Core Service:

- **Startup:** Automatic (machine boot), not demand-start or delayed-start.
- **Identity:** `NT SERVICE\\WebDynamiteCore` with the service SID enabled.
- **Failure actions:** restart after 5 seconds, 15 seconds, and 60 seconds.
- **Reset window:** 24 hours, preventing an unlimited rapid-restart loop.
- **Non-crash failures:** SCM recovery actions are enabled for non-zero service process exits.
- **Upgrade:** the installer stops the identity- and path-verified service before replacing binaries, then re-applies and verifies the policy before starting it.
- **Uninstall:** unregisters the service but preserves ProgramData databases, configuration, and protected secrets.

The installer lifecycle self-test validates the exact policy arguments and rejects mismatched `sc.exe qfailure` output. The Windows Installer workflow also builds the NSIS bundle with this contract enabled.

These checks validate the policy contract and installer build; they do not replace a real Windows VM acceptance run. Before claiming production readiness, test first boot after installation, recovery after an induced service crash, upgrade with existing data and secrets, interrupted upgrade recovery, uninstall, and reinstall on a disposable Windows machine.
