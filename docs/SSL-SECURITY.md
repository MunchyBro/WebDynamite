# SSL & Security

WebDynamite delegates public HTTPS certificate issuance and renewal to Caddy. Public domain assignments are rendered in the SSL & Security screen alongside Caddy installation and process status.

## Public certificate prerequisites

- Public DNS must resolve to an address that reaches the hosting machine.
- Inbound TCP ports 80 and 443 must reach Caddy.
- The assigned public hostname must be present in Caddy's active configuration.
- Caddy must be running and able to complete its ACME validation.

DNS resolution or a running Caddy process alone does not prove that a certificate has been issued or that HTTPS is reachable.

## Current visibility limitation

The current SSL & Security screen is a readiness dashboard only. It does not yet inspect Caddy's certificate storage, perform a live TLS handshake, report certificate issuer or expiry dates, or detect failed renewal attempts. Users must verify the public HTTPS endpoint independently until certificate telemetry is implemented.

## Local development domains

Local names such as `*.localhost` do not qualify for ordinary publicly trusted ACME certificates. Local development certificates require a separate explicit trust workflow. Trust local certificate authorities only on devices controlled by the user.

## Key handling

Do not copy private keys into website project folders or source control. Restrict access to WebDynamite's application data directory and protect backups, which may contain sensitive project data.
