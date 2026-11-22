# Trusted service configuration file loading

Phase 112 adds a Windows handle-based bounded reader for the installer-owned
service configuration.

- Opens the file with `FILE_FLAG_OPEN_REPARSE_POINT`, so the final path component
  is not followed through a reparse point.
- Inspects attributes on the opened handle and rejects directories and reparse
  points.
- Reads at most 16 KiB plus one byte and rejects empty, oversized, or changing
  files.
- The parser remains a separate step:
  `TrustedServiceConfig::parse_and_validate`.

## Important trust boundary

This reader is a safe file-opening primitive, not the complete trust decision.
The future SCM entry point must use a fixed path below the installer-owned
ProgramData directory and independently verify the file and every parent
directory's owner and protected DACL before accepting the bytes. The loader
does not repair ACLs, establish ownership, activate SCM mode, or start IPC.
Do not pass a user-selected path or environment-variable path to it.

Service activation remains disabled pending ACL/owner verification, signed
desktop and installer identity provisioning, secret migration, and installer
lifecycle validation.
