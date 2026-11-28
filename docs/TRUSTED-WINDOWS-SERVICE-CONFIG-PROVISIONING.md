# Trusted Windows service configuration provisioning

This milestone adds the installer-side writer for the strict version-1
configuration consumed by the SCM Core Service host. It replaces hand-authored
JSON with a fail-closed provisioning operation that derives identity values
from the installed Windows service and the actual signed application binaries.

## Provisioning contract

Run from an elevated installer process in the installing user's interactive
session, after the service has been registered and the protected ProgramData
layout has been prepared:

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\provision-trusted-service-config.ps1 -Mode Prepare -DesktopExecutable "C:\Program Files\WebDynamite\WebDynamite.exe" -ServiceBinary "C:\Program Files\WebDynamite\resources\webdynamite-core.exe"

The script:

- Resolves the registered NT SERVICE\WebDynamiteCore SID independently.
- Requires both installed executables to have valid Authenticode signatures
  from the same publisher and pins the SHA-256 hash of the signing certificate.
- Derives the installing user's SID and non-zero interactive session ID from
  the elevated installer process; it does not accept these values from UI input.
- Uses the fixed ProgramData roots and rejects reparse-point path components.
- Writes a bounded version-1 JSON document to a same-directory temporary file,
  applies a protected DACL containing exactly SYSTEM, Administrators, and the
  service SID, flushes it, and atomically replaces the destination.
- Verifies the resulting config and restores the previous file if post-write
  verification fails. Unexpected existing ACLs are never overwritten.

Read-only verification:

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\provision-trusted-service-config.ps1 -Mode Verify

CI runs deterministic contract tests without requiring a certificate,
service registration, or elevation:

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\provision-trusted-service-config.ps1 -Mode SelfTest

## Deliberate activation boundary

Prepare does not start the Windows service, change its startup type, or enable
the desktop named-pipe client. The current NSIS installer remains an unsigned
engineering build, and the desktop application still uses its interactive
Core Service startup path. Production activation must wait until the signed
installer invokes this writer, the desktop reads trusted client connection
metadata safely, the named-pipe client is wired into Tauri, and
install/upgrade/repair/uninstall plus reboot tests pass on Windows.

No API token or other secret is placed in this configuration file. The API
token remains in the machine-DPAPI store.
