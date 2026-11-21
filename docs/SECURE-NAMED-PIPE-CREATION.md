# Secure named-pipe creation primitive

This change adds a Windows-only first-instance named-pipe creation primitive. It uses the fixed local pipe name `\\.\\pipe\\WebDynamiteCore-v1`, duplex byte-stream mode, remote-client rejection, one server instance, 64 KiB buffers, and a five-second default timeout. A protected DACL grants full access to Local System, Built-in Administrators, and the dedicated WebDynamite service SID; the explicitly supplied interactive user's SID receives generic read/write access only. The handle is RAII-managed.

The supplied interactive SID is shape-checked before it is interpolated into the SDDL. The accepted account SID authorities are `S-1-5-21` (local/domain accounts) and `S-1-12-1` (Microsoft account SIDs); service and built-in group SID authorities are rejected.

## Security boundary

This primitive is not wired into the running service. It does not accept connections, authenticate the connected process, inspect its token, verify executable path or publisher, dispatch protocol messages, or grant secret operations. The pipe DACL limits who can open the pipe but is not a substitute for process identity verification. A later host must verify the connected process before reading or dispatching requests. The service must be installed under the documented virtual service account before this primitive is used in production.
