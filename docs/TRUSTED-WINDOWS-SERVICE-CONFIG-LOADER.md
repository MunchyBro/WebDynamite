# Trusted Windows service configuration loader

Phase 116 adds a bounded loader primitive. It is not called by the interactive
Core Service startup and does not register or activate a Windows Service.

## Required trust order

The caller supplies the fixed installer-owned config path and trusted root,
plus the expected service SID obtained independently from trusted SCM/service
token context. The SID must never be sourced from the JSON document or an
ordinary environment variable.

The loader then:

1. Opens the config file and every ancestor through the trusted root using
   `trusted_config_handle_chain`, retaining all handles.
2. Collects owner/DACL metadata from those same handles and validates the
   complete chain against the independently supplied service SID.
3. Reads no more than 16 KiB plus one detection byte from the already-open
   config file handle. It never reopens the path.
4. Parses the bytes with the strict versioned config parser.
5. Requires the parsed service SID to match the independently trusted SID.
6. Drops the handle chain only after ACL validation, bounded reading, and
   parsing have completed.

Any open, metadata, ACL, read, size, parse, or identity mismatch fails closed.

## Important boundary

This is a callable loading primitive only. It does not decide the trusted
installation paths, obtain the service SID from the SCM, wire into startup,
start the named-pipe host, or enable SCM service mode. The caller must derive
all trust inputs from the installed service context. Keep interactive startup
unchanged until signer provisioning, secret migration, installer lifecycle,
and end-to-end Windows validation are complete.
