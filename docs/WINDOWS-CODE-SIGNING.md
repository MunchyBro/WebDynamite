# Windows code-signing identity and release pipeline

## Production release status

Production release signing is implemented as a protected GitHub Actions workflow, but it remains **inactive until the owner configures a real CA-issued Code Signing PFX and its matching publisher pin**. The repository does not contain a certificate, private key, password, or production publisher pin. Never invent or commit any of them.

Pull-request and manual engineering builds remain unsigned and are not production release candidates. A tag beginning with \`v\` enters the protected \`production-release\` environment. The job fails closed before building or publishing unless all three protected secrets are present and valid:

- \`WEBDYNAMITE_SIGNING_PFX_BASE64\`: base64-encoded CA-issued PFX containing the private key.
- \`WEBDYNAMITE_SIGNING_PFX_PASSWORD\`: PFX password.
- \`WEBDYNAMITE_PUBLISHER_SHA256\`: SHA-256 of the signing certificate's DER bytes (64 hexadecimal characters).

Configure these as **environment secrets** on a GitHub environment named \`production-release\`. Protect that environment with required reviewers and restrict deployment branches/tags to reviewed release tags. Do not expose these secrets to pull-request jobs.

## Release sequence

For a version tag, the workflow:

1. Requires all protected signing inputs and validates the PFX, private key, validity interval, explicit Code Signing EKU, Digital Signature Key Usage, trusted certificate chain, online revocation status, and exact publisher pin.
2. Replaces the build-time publisher resource with the validated pin; the committed \`UNCONFIGURED\` sentinel is never accepted for a signed release.
3. Builds the Core Service and desktop executable without bundling.
4. Authenticode-signs the desktop and Core Service executables with SHA-256 and a trusted RFC 3161 timestamp.
5. Builds the NSIS installer from those signed executables, signs the final installer, then verifies Windows Authenticode trust and the exact pinned publisher on all three executables.
6. Generates a SHA-256 checksum file and publishes a GitHub Release only after every signature check succeeds.

The release job has \`contents: write\` permission only for the tagged release. The engineering build job retains read-only permissions and does not receive production secrets. A missing secret, invalid chain, revocation failure, timestamp/signing error, unsigned file, or publisher mismatch stops publication.

## Temporary signing key handling

The signing script decodes the PFX into a temporary file on the ephemeral GitHub-hosted Windows runner, restricts its ACL to the current runner identity, uses it only for Authenticode signing, clears the decoded byte array, and deletes the temporary file in a \`finally\` block. Never print the PFX, password, or decoded bytes. The runner must be treated as disposable.

## Obtain and configure the publisher identity

Obtain a Windows code-signing certificate from a certificate authority trusted by target Windows machines. It must be currently valid, contain its private key, explicitly permit Digital Signature in Key Usage, and include Code Signing EKU \`1.3.6.1.5.5.7.3.3\`.

Calculate SHA-256 over the certificate DER bytes:

\`\`\`powershell
$cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new("publisher.pfx", $password)
$sha = [System.Security.Cryptography.SHA256]::Create()
[Convert]::ToHexString($sha.ComputeHash($cert.RawData)).ToLowerInvariant()
\`\`\`

Store the resulting pin only in the protected \`WEBDYNAMITE_PUBLISHER_SHA256\` environment secret. Do not commit it to the repository. The installer build receives the validated pin as a generated resource so desktop/service mutual publisher verification can be enforced.

## Local validation

Run deterministic contract checks without a production certificate:

\`\`\`powershell
./scripts/windows/validate-signing-certificate.ps1 -SelfTest
./scripts/windows/sign-release-artifacts.ps1 -Mode SelfTest
./scripts/windows/verify-service-binary.ps1 -SelfTest
\`\`\`

A real release run additionally performs full certificate-chain/revocation validation and signature verification. The self-tests do not prove that a production certificate is configured.

## Safety rules

- Never use a self-signed certificate as a production publisher.
- Never bypass the tag workflow's protected environment or signature verification.
- Never publish unsigned engineering artifacts as production releases.
- Never accept a certificate merely because its display name matches; compare the exact DER SHA-256 pin.
- Rotate publisher identity only through a coordinated, reviewed desktop/service trust-pin migration.
