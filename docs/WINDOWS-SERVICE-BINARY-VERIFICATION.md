# Windows service binary signature preflight (Phase 136)

## Purpose

This read-only preflight verifies the executable currently registered with the
Windows Service Control Manager for \`WebDynamiteCore\` before a future installer
or service activation step trusts it.

The script checks all of the following:

- The service exists and exposes a parseable executable image path.
- The executable is located below \`%ProgramFiles%\\WebDynamite\`.
- The executable exists and no existing path component is a reparse point.
- Windows reports a valid, trusted Authenticode signature.
- The signer's certificate DER SHA-256 exactly matches the publisher pin
  supplied by the caller.

The pin must come from protected installer/service configuration provisioned
from the real production code-signing certificate. Do not hard-code a sample
pin, accept a user-writable setting, or fall back to an environment variable.

## Usage

From an elevated PowerShell process, after the service and signed binary have
been installed by a separately reviewed step:

    ./scripts/windows/verify-service-binary.ps1 -ExpectedPublisherSha256 "<64-character SHA-256 certificate pin>"

Run deterministic parser and pin-validation tests in CI:

    ./scripts/windows/verify-service-binary.ps1 -SelfTest

## Safety boundary

This command only queries SCM metadata and verifies the executable on disk. It
does not register, start, stop, reconfigure, repair, or remove a service. It
does not write certificates, keys, configuration, or registry values.

The current release pipeline does not yet sign the desktop executable, Core
Service, or NSIS installer, and the NSIS bundle does not yet register the SCM
service. Consequently this preflight is not wired into release installation
and is not a production activation gate. A missing service, unsigned binary,
untrusted signature, path outside the installation root, reparse-point path,
or publisher-pin mismatch fails closed.

This is a verification primitive only. Production SCM activation remains
blocked until the signing pipeline, protected pin provisioning, service-owned
credential migration, authenticated desktop handoff, transactional installer
lifecycle, and end-to-end Windows install/upgrade/recovery tests are complete.
