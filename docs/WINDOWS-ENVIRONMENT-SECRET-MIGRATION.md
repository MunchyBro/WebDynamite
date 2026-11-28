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

On startup, the signed desktop enumerates website environment metadata through
the authenticated loopback API and migrates each secret from the current
desktop user's `WebDynamite / website-env:<website-id>:<name>` Credential
Manager entry.

For each secret:

1. The desktop asks the authenticated service whether the DPAPI copy already
   exists. This check returns a boolean only, never the secret value.
2. If the protected copy exists, it is authoritative; the desktop removes any
   stale legacy credential without overwriting the newer protected value.
3. Otherwise, the desktop reads the legacy credential and sends it over the
   authenticated, publisher-pinned named pipe using `secret.put`.
4. The service writes the value to the machine-DPAPI store and verifies a
   read-back before acknowledging success.
5. Only after the verified acknowledgement does the desktop delete the legacy
   Credential Manager entry.

The operation is idempotent. If migration stops partway through, completed
entries remain in the DPAPI store and the next startup resumes safely. If a
legacy value is missing and no protected copy exists, migration reports the
specific website secret as requiring attention; it does not invent a value or
delete any source credential. A failure to remove a legacy entry leaves the
verified DPAPI copy intact and is reported for follow-up.

Engineering builds without trusted IPC pins do not run this migration and
continue using the legacy interactive Credential Manager path. The SCM
runtime's protected store is used only after trusted installer configuration
has been validated by the service host.