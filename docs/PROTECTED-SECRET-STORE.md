# Protected secret-file store

The Core Service now has a reusable file-store primitive for the planned Windows
Service-owned credential store.

## Provisioning contract

1. The installer creates the secret-store directory beneath a trusted,
   administrator-controlled parent.
2. Before writing any secret, the installer applies
   `secret_store_acl::secure_empty_directory` with the dedicated
   `NT SERVICE\\WebDynamiteCore` service SID.
3. The service account and administrators are the only principals granted
   access by that protected DACL. The directory's inheritable ACEs are intended
   to flow to files created directly inside it.
4. The service must keep the directory protected and must not accept paths or
   secret names from an untrusted caller.

The store module does not create the root, repair its ACL, inspect an existing
DACL, migrate Credential Manager entries, or change the current runtime secret
backend. Callers must satisfy the provisioning contract before using it.

## File format and write behavior

- Files use the `.wds` extension and a versioned `WDS1` envelope.
- Secret bytes are protected with machine-scoped Windows DPAPI before any
  filesystem write. Plaintext secret bytes are never written to disk.
- Secret names are restricted to 1-64 ASCII letters, digits, hyphens, and
  underscores; path separators and traversal strings are rejected.
- Each write creates a unique temporary file in the same protected directory,
  writes the encrypted envelope, flushes the file, then atomically replaces the
  destination. Windows replacement uses `MoveFileExW` with replace-existing
  and write-through flags.
- Reads enforce a bounded file size and reject unknown/truncated envelopes
  before asking DPAPI to decrypt.
- Deletes are idempotent when the named file is already absent.

## Security and integration boundaries

Machine-scoped DPAPI alone does not authorize a caller. Confidentiality and
access control depend on the protected directory DACL and the service's
identity. The desktop must not receive a machine-DPAPI decryption capability
or an unauthenticated secret-file API.

This milestone only adds the storage primitive. Existing website environment
secrets and the Core Service API token remain in their current storage paths
until the authenticated desktop-to-service handoff and migration are
implemented and tested. The file store is not yet called by production
request handlers or installer hooks.
