# Local domain routing

WebDynamite stores local domain assignments in its SQLite registry. The current implementation accepts hostnames ending in `.localhost` or `.test` and only assigns static websites. `.localhost` names are intended for loopback development; `.test` names may require a local DNS/hosts entry depending on the operating system and resolver.

## Current workflow

1. Register and start a static website containing `index.html`.
2. Open **Domains**, choose that website, and assign a hostname such as `portfolio.localhost`.
3. Generate the Caddyfile export from the Domains screen.
4. Install and run Caddy separately, then load the exported configuration using your own Caddy service setup.

The generated route rewrites the request path to the Core Service's static hosting endpoint and proxies to `127.0.0.1:47821`.

## Important limitations

- This phase generates and validates configuration only. It does not install, start, reload, or supervise Caddy.
- It does not edit the Windows hosts file or request administrator privileges.
- It does not open firewall ports or enable LAN/public exposure.
- Node.js domain routing is unavailable until per-site port allocation and runtime health checks are implemented.
- Public domains and HTTPS certificate provisioning are not included in this local-only phase.
- The Core Service management API remains bound to loopback and must not be exposed through a public proxy.

Removing a domain deletes only its registry assignment; it does not delete website files or stop the website.
