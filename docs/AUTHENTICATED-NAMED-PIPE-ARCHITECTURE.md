# Authenticated desktop-to-service named-pipe architecture

## Status and scope

This document defines the security contract for replacing the current desktop-readable
Core Service bearer token with a local Windows IPC boundary. It is an architecture
milestone, not an enabled transport. The current loopback HTTP API and per-user-session
Core Service startup remain unchanged until implementation and migration acceptance
gates pass.

The intended service identity is the virtual service account
`NT SERVICE\\WebDynamiteCore`, not LocalSystem. The desktop remains an
unprivileged interactive process. The service owns machine-DPAPI secret storage.

## Security goals

- Only the installed WebDynamite desktop, running as the expected interactive user,
  may establish an authenticated management session.
- A desktop client must not receive a machine-DPAPI decryption capability, the service
  API token, or raw secret-file access.
- Website and database credentials are submitted to the service over IPC and remain
  service-owned. The service injects them into website processes when required.
- Every request is bounded, versioned, authorized, and auditable without logging
  credentials or secret values.
- A local process that merely knows the pipe name, can connect to loopback, or runs
  under another account cannot gain management access.

## Named-pipe endpoint and ACL

Use a single documented local endpoint, initially
`\\\\.\\pipe\\WebDynamite.Core.v1`. The name is not a secret.

The service creates the pipe with an explicit protected security descriptor.
The DACL must grant:
- Full control to the service SID `NT SERVICE\\WebDynamiteCore`.
- Only the minimum connect/read/write rights to the intended interactive user SID.
- No access to Everyone, Anonymous, or broad Users groups.

The service SID is configured as a restricted service SID and the service runs with
the minimum required privileges. Pipe instances use remote-client rejection where
supported. The server must create a fresh instance for each client and enforce a
small maximum number of concurrent clients.

The installer must resolve and validate the actual service SID and interactive user
SID; it must never accept an arbitrary SID or pipe ACL supplied by the desktop.
Pipe creation failure is fatal to the IPC host, not a reason to fall back to an
unauthenticated endpoint.

## Client identity verification

A successful pipe open is necessary but not sufficient. For every connection the
server must:
1. Obtain the connected client PID from the pipe handle.
2. Open the process with query-limited rights and obtain its full executable path.
3. Open the process token and verify its user SID is the expected interactive user.
4. Verify the executable resides in the canonical, administrator-protected WebDynamite
   installation directory and has a valid Authenticode signature from the configured
   WebDynamite publisher.
5. Reject inaccessible, exited, ambiguous, unsigned, or unexpected clients. Do not
   fall back to PID-only, path-only, filename-only, or publisher-name-only checks.

The verifier keeps the opened process handle for the connection, checks the pipe PID again after opening it, confirms the process is still alive, and compares the process token SID/session with the pipe session. It requires the exact canonical desktop executable path and a valid Authenticode signature whose SHA-256 leaf-certificate thumbprint matches a 64-character configured pin. The signer pin is mandatory; missing, malformed, unsigned, or mismatched signatures fail closed.

The installer/release pipeline must establish a stable publisher identity and signed
desktop binaries before this verification is enabled in production. The pin must be supplied from trusted installer/build configuration, not from the connecting desktop process. The installation directory and executable must be writable only by trusted installers and administrators. Development builds
may use an explicit development-only mode that cannot be enabled by an installed
production service.

The server must account for PID reuse: retain and verify the process handle for the
lifetime of the connection, re-check liveness before authorizing sensitive operations,
and close all handles on disconnect.

## Framing and protocol

Use a length-prefixed binary frame with a fixed maximum (initial proposal: 1 MiB).
The payload is UTF-8 JSON with:
- protocol version,
- request ID,
- operation name,
- operation-specific data.

Reject zero-length, oversized, malformed, unknown-version, duplicate-field where
ambiguous, and trailing-data frames. Apply per-operation payload limits lower than
the global maximum. Each connection has read/write deadlines, bounded queues, and
a maximum number of in-flight requests. Never deserialize unbounded allocations.

The first exchange is a version/capability negotiation. The server returns only
non-sensitive capabilities and a fresh random session identifier. There is no
"get token", "read arbitrary file", "decrypt", or "export all secrets" operation.

Every operation is independently authorized by the server. Initial secret operations:
- `secret.put`: accept a bounded credential value, validate the owning resource and
  logical key, then write it using the service-owned protected secret store.
- `secret.delete`: delete only a credential bound to an existing resource/key.
- `secret.status`: return presence/metadata only; never return the secret value.

Later website lifecycle and configuration operations may be proxied through this
channel, but must have explicit schemas and authorization checks. Do not expose a
generic filesystem path, shell command, arbitrary environment dump, or unrestricted
HTTP proxy operation.

Responses include the matching request ID, protocol version, success/error code, and
bounded data. Errors must not disclose secret contents, DPAPI ciphertext, filesystem
ACL details, or internal tokens. Secret values must be redacted from tracing and crash
diagnostics.

## Secret ownership and migration

The service is the only runtime reader of machine-DPAPI secret files. The desktop
sends new or changed values through the authenticated pipe and discards temporary
buffers as soon as practical. The service validates resource ownership and stores
secrets under opaque generated identifiers, not caller-chosen paths.

The service must not return a secret after storing it. Existing website environment
and database credentials currently stored in the interactive user's Windows
Credential Manager remain supported until a transactional migration exists.

Migration requirements:
- Inventory all credential namespaces and references before moving data.
- Copy each credential into the service store, verify decryption and ownership,
  then update the database reference.
- Keep the old entry until the new reference is committed and validated.
- Make migration idempotent and recoverable after interruption.
- Never log or include values in migration reports.
- Provide an explicit rollback policy; do not silently delete the only valid copy.
- Remove desktop token retrieval and user-scoped secret access only after all callers
  have moved and upgrade tests prove old installations recover safely.

## Threat model and non-goals

This boundary is designed to prevent other local accounts, remote clients, random
local processes, and clients outside the signed installation from using the service.
It does not protect secrets from a fully compromised administrator, kernel, service
process, or the same user after that user has control of the verified signed desktop
process. Website child processes inherit the service account identity and are not
isolated from one another by Windows account boundaries in this milestone.

DPAPI machine scope is encryption, not caller authorization. The directory DACL,
service identity, pipe ACL, client identity verification, and operation authorization
are all required. A pipe name, random nonce, or loopback-only TCP binding is not
authentication by itself.

## Implementation sequence and release gates

1. Add protocol types, strict frame codec, size/deadline limits, and portable tests.
2. Add Windows pipe creation with explicit DACL, remote-client rejection, and safe
   handle lifecycle; test allowed and denied SIDs on Windows CI.
3. Add client PID/token/path/publisher verification and negative tests for unsigned,
   wrong-path, wrong-user, and disconnected clients.
4. Add only the narrow secret put/delete/status operations and service-side ownership
   checks. Prove that no API returns secret plaintext.
5. Add a desktop client that uses the pipe; do not ship a token-fetch command or
   machine-DPAPI decrypt endpoint.
6. Migrate existing Credential Manager secrets transactionally with crash/restart tests.
7. Integrate SCM service lifecycle and installer ACL provisioning. Keep the current
   interactive process mode until install, upgrade, repair, rollback, and uninstall
   tests pass on clean and upgraded Windows systems.

Production activation is blocked until signed desktop verification, service SID
provisioning, least-privilege pipe ACL tests, secret migration, and installer recovery
tests all pass. Until then, this document changes no runtime behavior.
