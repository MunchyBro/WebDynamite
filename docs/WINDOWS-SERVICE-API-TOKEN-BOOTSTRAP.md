# One-time Windows Core Service API-token bootstrap

The Core Service SCM host reads the fixed api-token entry from the machine-DPAPI secret store. This helper creates that entry without placing the token in an environment variable, command line, log, or plaintext file.

## Preconditions

1. Install/register WebDynamiteCore as NT SERVICE\WebDynamiteCore with an unrestricted service SID.
2. Run the existing elevated service-layout provisioning step and verify the exact protected DACL on C:\ProgramData\WebDynamite\Secrets.
3. Provision the trusted service configuration and verify its file/parent ACL chain before attempting SCM startup.
4. Ensure the service is stopped.

## Create the token

From an elevated PowerShell prompt, after the preconditions above:

    & "C:\Program Files\WebDynamite\webdynamite-core.exe" --provision-api-token
    if ($LASTEXITCODE -ne 0) { throw "Core Service API-token provisioning failed." }

The executable generates 256 bits of random token material, stores it using machine-scoped DPAPI, and verifies a decrypt/read-back comparison before reporting success. The token itself is never printed. Creation is atomic and refuses to replace an existing api-token.wds; rerunning the command cannot silently rotate a live service credential.

The protected store directory must already exist. The command does not create directories, repair ACLs, register/start the service, create the trusted JSON configuration, or migrate existing Credential Manager values.

## Current limitation

This is a manual bootstrap primitive, not an NSIS installer action. A complete installer still needs a signed/pinned desktop executable, trusted config generation with correct interactive user/session identity, installer rollback/repair handling, and end-to-end Windows service startup tests. Do not start the SCM service until those prerequisites are complete.