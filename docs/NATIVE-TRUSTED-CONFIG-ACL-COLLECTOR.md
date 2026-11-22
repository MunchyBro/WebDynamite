# Native trusted configuration ACL collection

The trusted_config_acl_collector module reads file attributes, owner SID, DACL
protection state, and ACEs from already-open Windows handles. It fails closed on
reparse points, unexpected object types, absent/null DACLs, invalid SIDs, deny
or unsupported ACE types, unexpected ACE flags, and any access mask other than
the installer contract's exact full-control mask.

## Handle ownership and path binding are caller responsibilities

collect_and_validate_from_handles does not open paths. Its caller must:

1. Resolve the fixed installer-owned trusted root and config path from trusted
   installation state, never from an untrusted request or environment override.
2. Open the config file and every parent directory without following reparse
   points. Open and inspect each component, file -> parent -> trusted root.
3. Bind each handle to its expected canonical path (for example, verify the
   final path reported for the handle) and supply the exact ordered path chain.
4. Keep every handle open while collecting metadata, reading the bounded config
   bytes from the already-open config handle, and parsing the configuration.
5. Close handles only after the security check and config read/parse complete.

The collector deliberately accepts handles rather than reopening paths. It
cannot by itself prove that a caller-supplied path label belongs to a handle,
nor can it make a path-based sequence of opens race-free. Do not call it with
handles or path labels from an untrusted caller.

## ACL contract

For each object, the collector currently accepts only ACCESS_ALLOWED_ACE entries
with the exact FILE_ALL_ACCESS mask. Directory ACEs must have only
OBJECT_INHERIT_ACE and CONTAINER_INHERIT_ACE flags; file ACEs must have no flags.
The policy evaluator then requires exactly one entry for each of Local System,
built-in Administrators, and the configured WebDynamite service SID, and rejects
other principals and duplicate entries. Owners must be Local System or
Administrators, and each DACL must be present and protected.

This is a deliberately strict installer contract, not a general-purpose ACL
normalizer. If the installer changes its SDDL, update the collector and policy
together and add Windows integration tests against descriptors created by the
installer.

## Activation status

This collector is not wired into service startup, the named-pipe listener, or
SCM registration. The existing interactive Core Service remains unchanged.
Windows Service mode must stay disabled until a trusted handle-opening
implementation, signer identity provisioning, secret migration, installer
lifecycle, and end-to-end Windows security tests are complete.
