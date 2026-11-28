# Windows desktop-to-service IPC bootstrap

The signed Windows desktop build attaches to the installed `WebDynamiteCore`
SCM service through the authenticated named pipe
 `\\.\pipe\WebDynamite.Core.v1` before it makes authenticated API
requests.

## Startup contract

1. The desktop binary is built with both
   `WEBDYNAMITE_PUBLISHER_SHA256` and `WEBDYNAMITE_SERVICE_SID`. These are
   compile-time inputs from the protected release build, never webview input,
   runtime environment overrides, or user settings.
2. The desktop opens the named pipe and verifies the server process handle,
   exact installed Core Service executable path, enabled virtual service SID,
   and Authenticode publisher SHA-256 pin before sending a request.
3. The service independently verifies the desktop process user SID, live
   nonzero logon session, exact installed desktop executable path, and pinned
   Authenticode publisher before dispatching the request.
4. The narrowly allowlisted `service.auth-token` operation reads the existing
   machine-DPAPI `api-token` secret and returns it only to that authenticated
   desktop peer. The desktop validates the token length and refreshes its
   Windows Credential Manager cache before using the existing loopback HTTP API.
5. Serialized IPC response bytes and the service-side response token copy are
   zeroized after transmission. The API token is never placed on a command line
   or logged.

The protocol does not expose website secret reads, arbitrary commands,
filesystem access, or generic RPC. The narrowly scoped `secret.put`,
`secret.delete` and `secret.status` operations are separately
allowlisted.

## Database profile credentials

Trusted desktop builds use the installer-protected machine-DPAPI store for
MySQL/MariaDB and PostgreSQL profile passwords. The authenticated pipe allows
only profile-scoped `database-credential.put`, `database-credential.get`,
and `database-credential.delete` operations. Engines are restricted to
`mysql` or `postgres`; profile identifiers are validated and included in a
domain-separated SHA-256 storage-key derivation.

On first access to an existing profile, the desktop reads its legacy per-user
Windows Credential Manager entry, transfers it over the authenticated pipe,
and performs a fresh service-side DPAPI read-back. The old credential is
retired only after the read-back matches. If transfer, decryption, or
verification fails, the original credential remains available for retry.
Subsequent saves and deletes target the service-owned store. Engineering builds
without trusted IPC pins retain their existing per-user Credential Manager
behavior.

The get operation returns a profile password only to the already authenticated,
publisher-pinned desktop process. Serialized IPC request and response copies
are zeroized after use; the existing desktop UI/API flow continues to receive
the password as before.

## Fail-closed behavior

A build with neither trusted compile-time pin uses the existing interactive
engineering startup path. A build with both pins uses the installed SCM service
only: if the service cannot be authenticated or is not listening, startup fails
instead of spawning a second interactive Core Service. A partial or malformed
pin configuration is rejected; it never silently downgrades to engineering
mode.

The current CI artifacts are unsigned engineering artifacts. This change does
not create a production signing certificate or publisher pin. Production
release builds remain blocked until the protected signing workflow supplies the
CA-trusted publisher SHA-256 pin and the exact installed service SID as trusted
compile-time inputs.

## Scope

This milestone bootstraps the API token over authenticated named-pipe IPC. The
application's existing website-management API remains on loopback HTTP with
bearer authentication. Moving the complete application command surface to
named-pipe RPC is a separate protocol expansion and must retain per-operation
authorization, bounded framing, and peer revalidation.
