# Local IPC protocol v1

The Core Service contains a transport-independent framing codec in
`core-service/src/ipc_protocol.rs`. It is a protocol foundation only; it does
not create a Windows named pipe, authenticate a client, or change the current
loopback HTTP service.

## Frame format

Each message is:
1. A four-byte unsigned big-endian payload length.
2. Exactly that many bytes of UTF-8 JSON.

The payload must be between 1 byte and 1 MiB. The reader rejects incomplete
frames, oversized lengths, malformed JSON, unknown struct fields, duplicate
struct fields, unsupported protocol versions, and invalid request metadata.
Callers must apply transport-level read/write deadlines and must not dispatch
any request until the Windows pipe peer has passed OS identity verification.

## Request

```json
{
  "version": 1,
  "request_id": "req-01",
  "operation": "secret.put",
  "data": {
    "resource": "website-1",
    "key": "DATABASE_URL",
    "value": "example-secret"
  }
}
```

Request IDs are 1-64 ASCII letters, digits, hyphens, or underscores.
Operation names are 1-64 lowercase ASCII letters, digits, dots, hyphens,
or underscores. Operation-specific schemas and authorization remain the
responsibility of the dispatcher; this codec deliberately does not define a
generic operation executor.

## Response

A response contains `version`, the matching `request_id`, and `ok`.
Successful responses may include `data`; failed responses must include an
`error_code`. The codec rejects contradictory success/error metadata and
bounds the serialized response to the same 1 MiB maximum.

## Security boundaries

- Framing validation is not authentication or authorization.
- Never accept a connection based on the pipe name alone.
- Never add a token retrieval, arbitrary file read, generic shell, or generic
  HTTP proxy operation.
- Secret operations must be narrow, resource-bound, and must never return
  secret plaintext after storage.
- The named-pipe host must enforce connection deadlines, bounded concurrency,
  and explicit DACLs before this codec is wired into production.
