# Desktop verification of the Core Service service SID

The desktop named-pipe client verifies the server before writing any IPC request.
The checks include the connected server PID, exact installed executable path,
Authenticode signature and pinned publisher SHA-256. It also opens the server
process token and requires the installer-provisioned WebDynamite service SID to
be enabled in that token.

The expected service SID is supplied only through the trusted release-build
setting `WEBDYNAMITE_SERVICE_SID` (compile-time `option_env!`). It is never
accepted from the webview, user settings, command-line arguments or runtime
environment. It must be a Windows service SID in the `S-1-5-80-...` authority
with five numeric sub-authorities. A missing or malformed value disables
authenticated IPC; there is no HTTP or unauthenticated fallback.

The service SID is an additional identity check, not a replacement for the
publisher pin, exact executable path, named-pipe ACL, or server-side desktop
client authentication. Production IPC remains unavailable until the installer
and trusted release build provide the protected service SID and publisher pin,
and the service-mode runtime is activated only after its remaining lifecycle
and clean-machine validation gates are complete.

The deterministic SID-format tests run on Windows. They do not simulate a real
SCM service token; that requires Windows integration testing with the actual
provisioned service identity.
