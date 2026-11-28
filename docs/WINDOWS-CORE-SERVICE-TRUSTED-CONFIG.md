# Windows Core Service trusted desktop session binding

The trusted service configuration pins the interactive user's Windows SID, the
installed desktop executable path, and the Authenticode publisher certificate
SHA-256 thumbprint. The session field has two modes:

- A nonzero `interactive_session_id` pins one exact Windows logon session.
  This is useful only for tightly controlled, short-lived test environments.
- `interactive_session_id: 0` is the installer default. Windows assigns
  session IDs dynamically and they can change after sign-out, sign-in, or
  reboot. In this mode the service still requires the pipe client's token
  session to be nonzero and to exactly match the session reported by the
  named pipe. The configured user SID, exact executable path, and pinned
  Authenticode signer must also match.

Do not write the installer process's current session ID into persistent
configuration. Doing so can make the service reject the legitimate desktop
client after the next logon.

This setting does not permit an arbitrary user or executable: only the
configured user SID and the pinned, signed desktop binary are accepted. The
session-zero check prevents service/session-0 processes from being treated as
interactive desktop clients.
