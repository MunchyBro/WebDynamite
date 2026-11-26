# SSL & Security

WebDynamite delegates public HTTPS certificate issuance and renewal to Caddy. Public domain assignments are rendered in the SSL & Security screen alongside Caddy installation and process status.

## Public certificate prerequisites

- Public DNS must resolve to an address that reaches the hosting machine.
- Inbound TCP ports 80 and 443 must reach Caddy.
- The assigned public hostname must be present in Caddy's active configuration.
- Caddy must be running and able to complete its ACME validation.

DNS resolution or a running Caddy process alone does not prove that a certificate has been issued or that HTTPS is reachable.

## Live HTTPS verification

The SSL & Security screen provides two on-demand checks for each registered public hostname:

- **Check HTTPS** invokes `curl` with certificate verification enabled, performs a HEAD request, records the HTTP status and elapsed time, and reports whether the TLS handshake and certificate validation succeeded. Redirects are not followed. `curl` must be installed and available on PATH.
- **Certificate details** connects to TCP port 443 using Windows PowerShell/.NET and reports the presented certificate's subject, issuer, validity start/end, thumbprint, and days remaining. The connection deliberately accepts the certificate for inspection, so this operation does **not** establish trust or hostname validity; use **Check HTTPS** for that. Inspection has a bounded connection timeout and is currently Windows-only.

A failed check can indicate DNS, routing, Caddy, remote-server, or certificate trouble; inspect the displayed error and server logs.

Both operations are point-in-time checks, not continuous monitoring. The certificate details view highlights certificates that are expired or have 30 days or less remaining. Each successful metadata inspection is retained in a per-hostname history (latest 30 entries), viewable from **Certificate history** and exportable as CSV. This is an inspection history, not a complete Caddy renewal audit: it records only times a user manually inspects the remote certificate. Automatic expiry monitoring is available as an opt-in scheduler while the Core Service is running. It inspects registered public hostnames on a configurable 1–168 hour interval, raises deduplicated in-app alerts when certificates enter the configured 1–90 day warning window or expire, and records certificate thumbprint changes as renewal notices. Alerts can be acknowledged; the latest 200 are retained. Monitoring is disabled by default and pauses when the Core Service stops. It does not renew certificates, validate trust (use **Check HTTPS**), or send OS/email notifications. A successful HEAD request also does not verify application routes or content.

## Local development domains

Local names such as `*.localhost` do not qualify for ordinary publicly trusted ACME certificates. Local development certificates require a separate explicit trust workflow. Trust local certificate authorities only on devices controlled by the user.

## Key handling

Do not copy private keys into website project folders or source control. Restrict access to WebDynamite's application data directory and protect backups, which may contain sensitive project data.
