# Trusted Windows service configuration

The Core Service now contains a strict parser/validator for the identity values
that a future installer-managed Windows Service host will need. It is a
validation primitive, **not an activation mechanism**.

## Configuration contract (version 1)

The installer-owned configuration contains exactly these fields:

- `version`: integer, currently `1`.
- `service_sid`: the service's virtual-account SID (`S-1-5-80-...`).
- `interactive_user_sid`: the interactive desktop user's SID.
- `interactive_session_id`: non-zero Windows session ID authorized to connect.
- `desktop_executable`: absolute path to the installed desktop `.exe`.
- `publisher_sha256`: 64-hex-character SHA-256 Authenticode publisher certificate thumbprint.
- `secret_store_root`: absolute, dedicated secret-store directory.
- `data_root`: absolute general application data directory, separate from the secret store.

Unknown fields, unsupported versions, malformed identities, invalid thumbprints,
relative paths, empty/oversized documents, and a shared secret/data root are
rejected. The parser is capped at 16 KiB.

## Trust requirements before loading

The parser does not establish trust in the file. A future SCM entry point must:

1. Use a fixed installer-owned path under the protected ProgramData application directory.
2. Verify the configuration file and parent directory DACL before reading it; only SYSTEM, Administrators, and the service identity may have the intended access.
3. Reject reparse-point redirection and unexpected ownership; do not follow a user-writable path.
4. Parse the bounded bytes with `trusted_service_config::TrustedServiceConfig::parse_and_validate`.
5. Revalidate the desktop publisher and executable identity on each IPC connection, using the configured thumbprint and exact installed path.
6. Fail closed if the signer thumbprint is missing, placeholder, or does not match the installed signed desktop binary.

## Protected file loading

`trusted_service_config_loader::load_trusted_service_config` reads only the fixed path `C:\\ProgramData\\WebDynamite\\service\\service.json`. It opens the file and its containing directory with `FILE_FLAG_OPEN_REPARSE_POINT`, rejects reparse points, verifies a protected DACL on both handles, requires exactly one allow ACE each for SYSTEM, Administrators, and the configured service SID, rejects inherited or other ACE types, and checks that the owner is one of those trusted principals. The file is size-bounded before parsing, and the parsed service SID must match the service identity supplied by the caller. Non-Windows builds fail closed.

The caller must obtain `service_sid` from trusted service identity (not from the file or environment). The installer must provision the fixed directory and file with protected DACLs before this loader can succeed.

## Activation remains disabled

This module is not called by the current interactive Core Service startup.
It does not register a Windows Service, start a pipe listener, or migrate
secrets. The release pipeline currently produces unsigned installers unless
code signing is separately configured. Therefore no production publisher pin
can be provisioned yet.

Do not enable SCM/pipe startup until the installer provisions and protects this
configuration, the desktop and installer are signed, secret migration is
validated, and install/upgrade/repair/uninstall tests pass.
