# SSL & Security

WebDynamite delegates public HTTPS certificate issuance and renewal to Caddy. Public domain assignments are rendered in the SSL & Security screen alongside Caddy installation and process status.

## Public certificate prerequisites

- Public DNS must resolve to an address that reaches the hosting machine.
- Inbound TCP ports 80 and 443 must reach Caddy.
- The assigned public hostname must be present in Caddy's active configuration.
- Caddy must be running and able to complete its ACME validation.

DNS resolution or a running Caddy process alone does not prove that a certificate has been issued or that HTTPS is reachable.

## Live HTTPS verification

The SSL & Security screen can run an on-demand HTTPS check for each registered public hostname. It invokes `curl` with certificate verification enabled, performs a HEAD request, records the HTTP status and elapsed time, and reports whether the TLS handshake and certificate validation succeeded. Redirects are not followed. `curl` must be installed and available on PATH. A failed check can indicate DNS, routing, Caddy, remote-server, or certificate trouble; inspect the displayed error and server logs.

This is a point-in-time probe, not continuous monitoring. It does not currently report certificate issuer, exact expiry date, renewal history, or automatically alert on future failures. A successful HEAD request also does not verify application routes or content.

## Local development domains

Local names such as `*.localhost` do not qualify for ordinary publicly trusted ACME certificates. Local development certificates require a separate explicit trust workflow. Trust local certificate authorities only on devices controlled by the user.

## Key handling

Do not copy private keys into website project folders or source control. Restrict access to WebDynamite's application data directory and protect backups, which may contain sensitive project data.
