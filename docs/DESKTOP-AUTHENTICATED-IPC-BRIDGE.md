# Desktop authenticated named-pipe bridge

## What this phase wires

The Tauri desktop backend now exposes three narrow commands backed by the
existing Windows named-pipe client:

- `ipc_secret_status` sends `secret.status`;
- `ipc_secret_put` sends `secret.put`;
- `ipc_secret_delete` sends `secret.delete`.

The bridge uses the bundled Core Service executable path and a publisher
SHA-256 pin compiled into the trusted release build through
`WEBDYNAMITE_PUBLISHER_SHA256`. The pin is not accepted from webview input,
ordinary settings, environment variables, or command-line arguments. Missing
or malformed build-time pin configuration fails closed. The named-pipe client
verifies the connected server process path and Authenticode publisher before
sending request bytes, then uses bounded overlapped I/O and request-ID-bound
responses.

Secret values are passed only in the authenticated IPC request body. The
bridge does not log, persist, echo, or return secret values. The service
dispatcher still authorizes only the existing narrow write/delete/status
operations; no secret-read or generic RPC operation is added.

## Important activation boundary

This is a desktop transport bridge, not the service cutover. The current
Core Service process still starts in interactive loopback-HTTP mode, and its
named-pipe accept loop is not yet activated by trusted service configuration
or the Windows Service Control Manager. Therefore these IPC commands will fail
closed until the later service-host activation work supplies the matching
server endpoint and trusted signer configuration.

The existing website, database, and settings commands continue to use the
current loopback HTTP API. No credentials are copied or deleted in this phase.
The existing Credential Manager API token and database credentials remain
untouched. The migration coordinator must still implement copy -> verified
write acknowledgement -> durable completion record -> source retirement,
with restart recovery and end-to-end Windows tests.

## Build configuration

A release build that intends to enable the IPC bridge must inject
`WEBDYNAMITE_PUBLISHER_SHA256` at compile time with the exact 64-character
SHA-256 Authenticode publisher pin validated by the release signing workflow.
Do not populate it from user-controlled configuration. Builds without the
pin intentionally compile with IPC disabled at runtime.

## Next gates

1. Provision and validate trusted service configuration and the exact
   publisher pin source.
2. Activate the authenticated named-pipe server loop only under the validated
   Windows Service identity and protected service data layout.
3. Add migration coordination for website environment secrets and database
   profile credentials; retain sources until destination verification.
4. Keep loopback HTTP available until every required command has an IPC
   equivalent and Windows end-to-end migration and rollback tests pass.
