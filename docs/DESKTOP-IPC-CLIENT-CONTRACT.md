# Authenticated Desktop IPC Client Contract

Status: implementation contract; the HTTP control plane remains active until every gate below is implemented and tested.

## Endpoint and protocol

- Connect only to `\\\\.\\pipe\\WebDynamite.Core.v1`.
- Use the version-1, four-byte big-endian length-prefixed JSON protocol in `src-tauri/src/ipc_protocol.rs`.
- Enforce the 1 MiB frame limit before allocation and use the request-correlated response reader.
- A connection handles one request/response exchange and is then closed. Do not multiplex requests over a connection.
- The client must generate a fresh, unpredictable request ID for every exchange.

## Connection and I/O deadlines

The Windows client must use overlapped I/O for both connection and data transfer. A synchronous `CreateFileW`, `ReadFile`, or `WriteFile` path is not acceptable because a local peer can stall the UI indefinitely.

- Bound pipe discovery/connection to 5 seconds.
- Bound the complete request write to 5 seconds.
- Bound the complete response read to 5 seconds.
- Use a dedicated event per outstanding OVERLAPPED operation; wait only until the remaining absolute deadline.
- On timeout, call `CancelIoEx`, wait for cancellation completion, and close the handle only after the operation is no longer pending.
- Treat broken pipe, partial frame, cancellation, timeout, and malformed response as hard errors. Never retry an operation automatically: secret writes/deletes may not be safely replayable.
- Do not fall back to loopback HTTP for secret operations when IPC fails.

## Authenticate the server before sending a request

The pipe name and ACL are not proof of server identity. Before writing any request bytes, the client must:

1. Obtain the server PID with `GetNamedPipeServerProcessId`.
2. Open and retain that exact process handle with query and synchronize rights; verify the PID from the handle and that the process is still running.
3. Read the process image path and compare its canonical path to the exact installed Core Service executable path.
4. Verify the executable's Authenticode signature and compare the SHA-256 signer thumbprint to a trusted, installer-provisioned pin.
5. Recheck the pipe server PID and retained process liveness immediately before writing.
6. Fail closed if any expected identity value is absent, malformed, inaccessible, or mismatched.

The expected executable path and signer pin must come from a protected installation configuration that an unprivileged user cannot modify. Never accept either value from the pipe peer, an environment variable, a UI argument, or an ordinary user-writable settings file. The release pipeline must establish the actual signing certificate before a production pin can be populated; no placeholder or self-selected thumbprint is acceptable.

## Server-side counterpart

The Core Service must continue to authenticate the desktop client (user SID, interactive session, exact installed desktop executable path, and pinned Authenticode signer) before reading the first request byte. It must revalidate the retained client process immediately before dispatch and apply the operation-level allowlist. The pipe ACL must remain restricted to the service SID, Local System, Administrators, and the explicitly provisioned interactive user SID.

## Integration gates

Do not wire secret commands to this client until all of the following are true:

- The Core Service has a cancellable, bounded accept loop integrated with graceful shutdown.
- The installer provisions the service SID, pipe ACL, protected secret-store root, protected identity configuration, and signer pins.
- The desktop and service binaries are signed by the configured publisher and the installed files cannot be replaced by unprivileged users.
- Windows integration tests cover wrong server PID/path/signer, PID reuse, wrong user/session, stalled connect/read/write, cancellation, malformed frames, and peer disconnect.
- Secret migration is transactional and preserves the previous Credential Manager values until the protected store is verified after restart.

Until those gates pass, the existing loopback HTTP control plane and user-scoped Credential Manager storage remain unchanged. This is intentional: a partially authenticated transport must not silently become the security boundary for production secrets.
