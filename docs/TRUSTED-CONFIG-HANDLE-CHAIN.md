# Native trusted-config handle opening

Phase 115 adds trusted_config_handle_chain, a Windows-only path-bound opener for the trusted service configuration and its directory ancestry.

## Contract

- Accepts only normalized absolute local drive-letter paths; UNC and device paths are rejected.
- Requires the config file to be strictly below the fixed trusted root.
- Opens the root first, then each descendant directory, then the config file.
- Uses FILE_FLAG_OPEN_REPARSE_POINT for every object and FILE_FLAG_BACKUP_SEMANTICS for directories.
- Uses read-only sharing (no write/delete sharing) so the opened objects cannot be replaced or renamed while the chain is retained.
- Rejects reparse-point attributes and file/directory type mismatches.
- Compares each handle's GetFinalPathNameByHandleW result with GetFullPathNameW lexical normalization of the requested path. This rejects intermediate path redirection as well as a redirected final component.
- Retains every native handle in TrustedConfigHandleChain. Its security_objects() method supplies the file -> parent -> trusted-root handles expected by trusted_config_acl_collector.

## Required caller sequence

1. Open the chain with open_trusted_config_chain.
2. Keep the returned TrustedConfigHandleChain alive.
3. Obtain security_objects() and pass those handle/path pairs to collect_and_validate_from_handles.
4. Read the bounded config bytes from the already-open file handle (never reopen the path).
5. Parse the bytes while the chain remains alive.
6. Drop the chain only after read, parse, and ACL policy validation are complete.

The current module intentionally does not read or parse config bytes and is not wired into service startup, named-pipe activation, or SCM registration. Windows Service mode remains disabled.

## Validation boundary

The path-binding helper and ACL collector are separate layers. The collector verifies owner/DACL metadata from the same handles; the handle-chain opener establishes that those handles resolve to the requested local paths and that no opened component is itself a reparse point. The caller must preserve the chain lifetime through the entire config operation.
