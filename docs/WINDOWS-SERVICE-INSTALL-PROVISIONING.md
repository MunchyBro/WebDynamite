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

This is a provisioning primitive, not the complete installer lifecycle. It
does not register, start, stop, upgrade, repair, or uninstall a Windows
service; it does not delete existing data; and it does not change the current
interactive Core Service startup. Prepare only creates missing managed
directories and updates their DACLs. It does not recursively rewrite ACLs on
existing files or descendants. It must not be wired into a production release
until the SCM dispatcher, service identity, signed binaries, rollback policy,
and end-to-end Windows installer tests are complete.

The service SID must resolve as \`NT SERVICE\\WebDynamiteCore\`. If the service
identity is not provisioned on the target machine, the script fails closed
rather than substituting a user or LocalSystem identity.
