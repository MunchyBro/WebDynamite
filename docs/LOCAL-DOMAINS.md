# Local domain routing

WebDynamite stores local domain assignments in SQLite. Hostnames must end in `.localhost` or `.test`. Static and Node.js websites can be assigned local hostnames.

## Workflow

1. Register a static project with `index.html`, or a Node.js project with an `npm run start` script.
2. Assign a hostname in **Domains**, for example `portfolio.localhost`.
3. Start a Node.js website before exporting its route. WebDynamite allocates a loopback port and supplies it through the `PORT` environment variable (and `HOST=127.0.0.1`). Your app must listen on that environment-provided port.
4. Generate the Caddyfile in **Domains**. The export reports ready routes and Node.js routes waiting for a port.
5. Install Caddy and configure its executable path (the Core Service uses `caddy` from `PATH`, or `CADDY_BIN` when set).\n5. In WebDynamite, use **Validate**, then **Start Caddy**. When domains or website ports change, use **Reload Caddy** to apply the latest generated configuration.

Static routes proxy to the Core Service static-file endpoint. Node.js routes proxy directly to the website's assigned loopback port.

## Current limitations

- Caddy is not bundled, installed, started, reloaded, or supervised by WebDynamite in this phase.
- The Caddyfile is generated but not validated by a Caddy binary.
- WebDynamite does not edit the Windows hosts file, configure DNS, or request administrator privileges.
- `.localhost` names resolve to loopback on supported systems; `.test` may require local DNS/hosts configuration.
- Public domains, LAN exposure, firewall rules, and HTTPS certificate provisioning are not included.
- Node.js applications must honor the `PORT` environment variable. Apps that hard-code their port will not be reachable through the generated route.
- The Core Service management API remains bound to loopback.

Removing a domain deletes only its registry assignment. It does not delete website files or stop a website.
