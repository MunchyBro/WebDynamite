# Trusted config security policy

Phase 113 adds a fail-closed policy evaluator for the security metadata that
must be collected for the trusted service configuration and its directory
chain.

## Required evidence

The evaluator requires one snapshot for the config file, followed by every
parent directory through the fixed installer-owned root, inclusive. It rejects:

- Missing, reordered, or out-of-root path snapshots.
- Relative paths and paths containing parent traversal components.
- Reparse points and incorrect file/directory object types.
- Owners other than Local System (`S-1-5-18`) or built-in Administrators
  (`S-1-5-32-544`).
- Missing or unprotected DACLs.
- Any ACE principal other than SYSTEM, Administrators, and the configured
  WebDynamite service SID; duplicate entries or missing required principals.

The future Windows collector must reject deny ACEs, object-specific/unknown ACE
types, inherited or otherwise unsupported ACE semantics rather than reducing
them to a SID list. The policy's `allowed_ace_sids` input represents only ACEs
that the collector has already established are supported allow ACEs.

## Critical implementation boundary

This is a policy evaluator, not a Windows security metadata collector. Its
snapshot values are not trustworthy if supplied by the desktop, a config file,
or another untrusted caller. A future SCM host must collect metadata directly
from Windows handles opened without following reparse points, cover every
directory from the file to the fixed trusted root, and keep the handles alive
through config loading to avoid path replacement races.

This phase does not call the evaluator during startup, activate SCM mode, or
enable the named-pipe listener. The existing interactive process mode remains
the only active mode until native handle-based collection, signer provisioning,
secret migration, and installer lifecycle validation are complete.
