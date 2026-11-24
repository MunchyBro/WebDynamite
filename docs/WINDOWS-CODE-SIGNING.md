# Windows code-signing identity

## Purpose and current boundary

The desktop and Core Service authenticate each other using an Authenticode
publisher SHA-256 pin. The pin must be derived from the real certificate that
will sign both executables; never invent a pin or use a development/self-signed
certificate as a production publisher identity.

This phase adds certificate preflight validation. It does **not** sign binaries,
make an unsigned installer trusted, or enable Windows Service mode. The release
workflow remains unsigned until the signing pipeline is implemented and tested.

## Current release safety gate

**Tagged Windows releases are deliberately blocked.** The Windows Installer
workflow fails immediately for a `v*` tag because it cannot yet prove that the
desktop executable, Core Service, and final NSIS installer are signed by the
trusted production publisher. Pull-request and manual builds remain available
for engineering validation, but their installer artifacts are not production
release candidates.

Do not bypass this gate or distribute an unsigned artifact. Re-enable release
publication only in a reviewed change that:
1. Retrieves the production PFX and password from protected GitHub Actions
   secrets in a protected release environment.
2. Signs the desktop executable and Core Service before packaging.
3. Signs the final NSIS installer and applies a trusted timestamp.
4. Verifies every signature with Windows Authenticode and checks the exact
   publisher certificate DER SHA-256 pin.
5. Publishes only after all verification steps succeed.

The workflow has read-only repository permissions because it no longer
publishes GitHub Releases. The PFX must never be exposed to pull-request builds.

## Required publisher identity

Obtain a Windows code-signing certificate from a certificate authority trusted
by the Windows machines that will install WebDynamite. The certificate must:
- be currently valid and include its private key in the PFX bundle;
- explicitly permit Digital Signature in the Key Usage extension;
- explicitly include the Code Signing EKU (1.3.6.1.5.5.7.3.3);
- be suitable for Authenticode signing and timestamped releases;
- be protected as a high-value signing credential.

Do not generate a self-signed certificate and describe it as a trusted publisher.
A self-signed certificate is suitable only for isolated development experiments.

## Calculate the publisher pin

The application expects SHA-256 of the signer's certificate DER bytes (not the
usual SHA-1 certificate thumbprint). On a machine with the certificate, run:

    $cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new("publisher.pfx", $password)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    [Convert]::ToHexString($sha.ComputeHash($cert.RawData)).ToLowerInvariant()

Compare this 64-character value with the pin passed to the desktop and service
identity verifiers. Keep the same publisher identity for both signed binaries.

## Preflight validation

Run the repository script from PowerShell:

    $env:WEBDYNAMITE_SIGNING_PFX_BASE64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes("publisher.pfx"))
    $env:WEBDYNAMITE_SIGNING_PFX_PASSWORD = "<retrieve securely; do not save in shell history>"
    $env:WEBDYNAMITE_PUBLISHER_SHA256 = "<64-character SHA-256 certificate pin>"
    ./scripts/windows/validate-signing-certificate.ps1

The script loads the PFX using an ephemeral key-storage context, checks the
private key, validity interval, explicit Code Signing EKU, explicit Digital
Signature Key Usage, a trusted Windows Code Signing chain with online
revocation checking, and the expected SHA-256 pin. Chain construction fails
closed when Windows cannot establish trust or verify revocation; it is not
silently skipped when offline. The script does not write the certificate or
private key to disk. Its -SelfTest mode checks strict pin parsing and positive,
negative, and missing Key Usage cases without requiring a real certificate.
The self-test also verifies strict 64-character hexadecimal pin parsing and
normalization of uppercase, colon-separated pins with surrounding whitespace.
It creates ephemeral certificates to verify that a certificate's matching DER
SHA-256 publisher pin is accepted and a mismatched pin is rejected; a certificate with a private key is accepted and a public-only
certificate is rejected. It verifies that currently valid validity dates are
accepted, expired and not-yet-valid certificates are rejected, the Code Signing EKU is accepted,
a different EKU is rejected, and a missing EKU is rejected. It similarly
checks positive, negative, and missing Key Usage cases.
These deterministic tests do not replace an integration check with a real
CA-issued PFX; the full certificate-chain and online-revocation check runs only
when protected signing inputs are supplied to the non-self-test invocation.

## CI secret names for the later signing-pipeline phase

When the signing workflow is implemented, store these as protected GitHub
Actions secrets (not repository files or variables):
- WEBDYNAMITE_SIGNING_PFX_BASE64
- WEBDYNAMITE_SIGNING_PFX_PASSWORD
- WEBDYNAMITE_PUBLISHER_SHA256

Restrict access to release workflows/environments. Prefer environment
protection and required reviewers for production signing. Never print secret
values, upload PFX files as artifacts, or expose them to untrusted pull-request
workflows. The later pipeline must sign the desktop executable and Core Service
before packaging, sign the final NSIS installer, timestamp all signatures, and
verify every signature and publisher pin before publishing a release.

## Fail-closed requirements before service activation

- The desktop executable and Core Service must both have valid trusted
  Authenticode signatures from the pinned publisher.
- The configured pin must be provisioned from a protected installer/service
  configuration source, not user-writable settings or environment variables.
- The installer must protect executable paths and publisher configuration.
- Rotation must support a planned overlap or coordinated upgrade; never silently
  accept an arbitrary new signer.
- Missing, malformed, expired, untrusted, or mismatched signing identity must
  prevent authenticated IPC and Windows Service activation.
