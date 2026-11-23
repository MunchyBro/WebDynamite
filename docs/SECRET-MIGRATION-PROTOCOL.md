# Protected secret migration protocol

## Current implementation boundary

Phase 119 adds a verified-write primitive to the machine-DPAPI store. The
existing authenticated IPC `secret.put` operation now reports success only
after the value has been written, decrypted from the destination, and compared
with the supplied value. The decrypted verification buffer is cleared before
returning.

This is a migration safety primitive, not a completed migration. The named-pipe
host is not yet part of the active desktop runtime, the desktop still starts
the interactive Core Service with a per-user Credential Manager API token, and
website environment/database credential flows still use their current HTTP and
Credential Manager paths. Do not delete or rewrite existing credentials based
on the presence of this primitive.

## Required copy-verify-retire sequence

For every source credential, a future migration coordinator must:

1. Read the source value from its current protected source (Windows Credential
   Manager for desktop-owned database credentials, or the existing protected
   source for website environment secrets).
2. Send only the exact, authorized destination identifier and secret name over
   the authenticated named pipe. Never put secret values in logs, command-line
   arguments, temporary files, or migration reports.
3. Require a successful `secret.put` response with `stored=true` and
   `verified=true`. A transport timeout, disconnect, malformed response,
   authorization failure, or store error is an ambiguous outcome: keep the
   source credential and retry idempotently after checking destination status.
4. Only after a verified acknowledgement, retire the source credential.
   Source deletion must be a separate operation and must never happen before
   destination verification.
5. Record only non-secret metadata (source kind, destination key identifier,
   migration version, and completion state). Never persist plaintext or a
   reversible encoding in the journal.
6. Make restart recovery idempotent: a copied-but-not-retired entry is safe to
   copy again, and a retired source must have a durable verified-destination
   completion record.

## Destination protection

The installer must provision the service-owned directory and its protected
DACL before the service accepts secret writes. The current ACL primitive grants
access to Local System, Administrators, and the dedicated service SID; the
future service identity and installation design must match that policy.
Machine DPAPI is encryption at rest, not caller authorization. Keep the
authenticated IPC peer checks and protected directory ACL as separate controls.

## Required migration coverage

The eventual coordinator must cover:
- website environment secrets, preserving website ID and environment name;
- MySQL/MariaDB and PostgreSQL profile passwords from Credential Manager;
- the Core Service API token, only as part of the later service/IPC cutover;
- interrupted copies, destination already-present cases, missing source entries,
  destination ACL/DPAPI failures, and rollback before source retirement.

Do not enable the service runtime or remove the loopback bearer-token path until
all secret classes have been migrated and verified, service restart recovery
works, and end-to-end Windows tests prove that the service can retrieve secrets
without returning them to the desktop.
