# Windows service installation provisioning (Phase 120)

This phase adds a narrow, repeatable provisioning step for the future Windows
Service installation. It prepares the ProgramData directory layout and applies
the same protected three-principal DACL required by
\`core-service/src/secret_store_acl.rs\` and
\`core-service/src/trusted_config_security.rs\`.

## Managed layout

- \`%ProgramData%\\WebDynamite\`
- \`%ProgramData%\\WebDynamite\\Config\`
- \`%ProgramData%\\WebDynamite\\Data\`
- \`%ProgramData%\\WebDynamite\\Secrets\`

Each directory receives a protected DACL with exactly three full-control
principals: Local System, built-in Administrators, and the
\`NT SERVICE\\WebDynamiteCore\` service SID. Object/container inheritance is
enabled so a subsequently-created trusted config file can inherit the contract.
The script rejects reparse-point path components and requires an elevated
process.

## Use

Run from an elevated Windows installer process:

\`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\provision-service-layout.ps1 -Mode Prepare\`

Verify without changing ACLs:

\`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\provision-service-layout.ps1 -Mode Verify\`

CI exercises the script's deterministic contract self-test:

\`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\\windows\\provision-service-layout.ps1 -Mode SelfTest\`

## Safety boundary

This script remains a directory/ACL provisioning primitive: it does not itself
register, start, stop, upgrade, repair, or uninstall a Windows service. The
integrated NSIS lifecycle orchestrator calls it only after registering the
exact virtual service identity. Prepare creates missing managed directories
and applies the protected DACL to those directories; it does not recursively
rewrite ACLs on existing files or descendants. The orchestrator preserves
existing data and machine secrets during repair, upgrade, and uninstall.

The service SID must resolve as \`NT SERVICE\\WebDynamiteCore\`. If the service
identity is not provisioned on the target machine, the script fails closed
rather than substituting a user or LocalSystem identity.

## Installed service identity preflight (Phase 135)

After the Windows service has been installed by a separately reviewed installer
step, run the additional read-only identity check from an elevated process:

`powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\windows\provision-service-layout.ps1 -Mode VerifyService`

This verifies that the registered `WebDynamiteCore` service uses the exact
virtual service account `NT SERVICE\WebDynamiteCore`, and that SCM reports its
service SID type as `UNRESTRICTED`. It fails closed for missing services,
LocalSystem, another logon account, or a disabled/restricted service SID.
The command does not create, start, stop, reconfigure, or delete the service.

This remains a read-only identity acceptance check. The integrated NSIS
installer now registers the service, provisions the protected directory
layout, and verifies that layout before trusted config creation and service
startup. The check does not itself mutate SCM state. The interactive Core
Service runtime remains unchanged.


## NSIS lifecycle integration (Phase 150)

The per-machine NSIS installer now invokes the Core Service lifecycle through
Tauri's supported installer hooks:

- **Before install/upgrade:** stop an existing, identity-verified service so
  Windows can safely replace its executable. A first install is a no-op here.
- **After install/upgrade:** validate the installed desktop and Core Service
  signatures; register the demand-start service; prepare and verify protected
  ProgramData ACLs; create the machine-DPAPI API token only when it is absent;
  atomically provision and verify the trusted desktop/service configuration;
  start the service; and wait for the local API TCP listener.
- **Before uninstall:** stop and unregister the service before its executable
  is removed. The ProgramData database, configuration, and machine secrets are
  deliberately preserved.

The service runs only when both binaries have valid Authenticode signatures.
An unsigned engineering installer remains installable as a desktop-only build
and explicitly skips privileged service registration/activation. Invalid or
untrusted signatures fail closed. Production release publication remains
blocked until the real publisher certificate and signing pipeline are
configured.

The installer uses the per-machine NSIS mode because service registration,
machine DPAPI, and protected ProgramData ACL provisioning require elevation.
The lifecycle script is embedded into the NSIS installer at build time, so
pre-install and uninstall hooks do not depend on files already having been
copied into the application directory.

CI runs deterministic PowerShell contract self-tests and builds the NSIS
bundle. It does **not** substitute for a real signed Windows install/upgrade/
repair/uninstall test on a disposable Windows machine. Before production
release, exercise first install, upgrade with an existing token, repair,
service startup failure, uninstall, and reinstall while verifying that user
data and secrets survive. The local TCP listener check confirms startup only;
it is not an authenticated API end-to-end test.


### Publisher pin requirement

A valid Authenticode chain alone is not sufficient to activate the privileged
service. The installed resource file
`resources/webdynamite-publisher.sha256` must contain the exact nonzero
64-hex SHA-256 hash of the expected publisher certificate's DER bytes. The
repository ships only an `UNCONFIGURED` sentinel. Engineering builds remain
unsigned and skip service activation; a future protected signing workflow must
replace the sentinel with the value from the protected
`WEBDYNAMITE_PUBLISHER_SHA256` secret before packaging. Signed binaries with
a missing, malformed, or mismatched pin fail closed. Never commit or guess a
production publisher pin.
