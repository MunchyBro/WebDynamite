# Windows Core Service registration (Phase 204)

scripts/windows/manage-core-service.ps1 provides a narrow, elevated service registration lifecycle primitive. It is intentionally not wired into NSIS yet.

## Register

After the signed Core Service executable has been installed at its final path:

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\manage-core-service.ps1 -Mode Install -ServiceBinaryPath "C:\Program Files\WebDynamite\webdynamite-core.exe"

The script registers WebDynamiteCore as NT SERVICE\WebDynamiteCore, enables its unrestricted service SID, sets demand-start mode, and verifies the registered account and SID type. Re-running Install is idempotent only when both the service identity and executable path match; it refuses to overwrite a conflicting service.

## Verify and remove

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\manage-core-service.ps1 -Mode Verify
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\manage-core-service.ps1 -Mode Remove

Remove stops and waits for the service before unregistering it. It never deletes ProgramData, website data, configuration, or secrets.

## Deliberate safety boundary

Install does not start the service. Startup remains blocked until a separate, reviewed provisioning step creates and ACL-protects the trusted service config, provisions the machine-DPAPI API token, and verifies the pinned desktop identity. The script does not generate or expose secrets, migrate Credential Manager entries, modify existing data ACLs, or configure NSIS hooks. Production signing, rollback/upgrade behavior, and end-to-end Windows installation tests remain required before this primitive can be enabled in a release installer.
