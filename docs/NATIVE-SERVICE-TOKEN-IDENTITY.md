# Native Windows Service Token Identity

Phase 134 adds a fail-closed identity source for trusted service configuration.

## Identity acquisition

`service_identity_win::current_service_identity` opens the current process
token with query-only access and reads:

- the token user SID from `TokenUser`;
- enabled service SID groups from `TokenGroups`.

Exactly one enabled SID matching the NT service SID shape
`S-1-5-80-...` is required. Missing, malformed, truncated, or ambiguous
identity data is rejected. Token handles and converted SID buffers are released
through owned cleanup paths.

## Trusted configuration chain

`load_current_service_config` obtains the service SID from that native token
and passes it as the independent expected identity to
`load_trusted_service_config`. The existing loader then validates the
installer-provisioned object chain and ACLs before reading the bounded config
from its already-open handle, and confirms the configured service SID matches
the token-derived SID.

`service_pipe_runtime::start_current_service_pipe_worker` composes this
identity/configuration chain with the authenticated named-pipe worker. It does
not accept the service SID from environment variables or from the config file.

## Activation boundary

This phase adds callable primitives only. It does not invoke the service
dispatcher from process startup, install the service, or switch the current
interactive HTTP runtime into service mode. The installer must still provision
the exact service SID and protected directory ACLs. Signer identity, service
account secret migration, SCM installer lifecycle, and full Windows service
integration tests remain outstanding. Do not treat a normal interactive
process as a service: token acquisition intentionally fails unless exactly one
enabled NT service SID is present.
