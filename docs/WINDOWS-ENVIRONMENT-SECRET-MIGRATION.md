# Windows website environment secret migration

Trusted signed desktop builds use the installed Core Service's
installer-provisioned machine-DPAPI secret store for website environment
secrets. The SCM service receives the protected secret-store root from its
validated installer configuration; the interactive engineering runtime keeps
the legacy per-user Windows Credential Manager path.

## Runtime storage

- Website secret filenames use the same versioned, website-scoped SHA-256
  derivation as authenticated IPC. Website IDs and variable names are not
  exposed in the protected directory listing.
- Secret values are stored only through the existing DPAPI-protected secret
  store. The website environment database continues to hold an empty value for
  secret rows and never receives plaintext secret values.
- Runtime process creation reads the DPAPI value inside the Core Service and
  supplies it to the child process environment. Secret bytes are not returned
  by the website environment listing API.
- Single and batch writes verify DPAPI read-back. Batch changes snapshot
  previous protected values and attempt rollback if the database transaction
  fails. Delete batches similarly restore prior values if the database commit
  fails.

## Legacy migration

On startup, the SCM Core Service migrates legacy entries from its own
Windows Credential Manager logon identity into the machine-DPAPI store. This
runs inside the service process because the service account's Credential
Manager is not accessible to the interactive desktop user's logon.

For each secret:

1. The service checks whether a DPAPI copy already exists and can be decrypted.
2. If the protected copy exists, it is authoritative; the service removes any
   stale legacy credential without overwriting the newer protected value.
3. Otherwise, the service reads its own legacy Credential Manager entry and
   writes the value to the machine-DPAPI store.
4. The service verifies a read-back before deleting the legacy entry.

The operation is idempotent. If migration stops partway through, completed
entries remain in the DPAPI store and the next service startup resumes safely.
If a legacy value is missing and no protected copy exists, the service reports
the specific website secret as requiring attention; it does not invent a value
or delete any source credential. A failure to remove a legacy entry leaves the
verified DPAPI copy intact and is reported for follow-up.

Engineering builds without trusted SCM configuration do not run this migration
and continue using the interactive Credential Manager path. The SCM runtime's
protected store is used only after trusted installer configuration has been
validated by the service host.