# Local domains and LAN networking

WebDynamite supports local hostnames and opt-in LAN URLs for static and Node.js websites.

## Local domains

1. Register a static project with `index.html`, or a Node.js project with an `npm run start` script.
2. Assign a hostname in **Domains**, for example `portfolio.localhost`.
3. Start a Node.js website before applying its route. WebDynamite supplies `PORT` and `HOST=127.0.0.1`; the app must honor `process.env.PORT`.
4. Install Caddy and make it available on `PATH`, or set `CADDY_BIN` for the Core Service.
5. Use **Validate** and **Start Caddy** in the Domains screen. Reload Caddy after changing domain assignments or website ports.

Static routes proxy to the Core Service static-file endpoint. Node.js routes proxy directly to their loopback application port.

## LAN publishing

LAN publishing is disabled by default for every website.

1. Open **Networking** after connecting the desktop app to the Core Service.
2. Enable LAN access for a static or Node.js website.
3. WebDynamite discovers the private IPv4 address on the default network route and allocates a unique port.
4. If Caddy is already running, WebDynamite reloads its generated configuration. Otherwise, start Caddy from **Networking** or **Domains**.
5. On Windows, allow that TCP port through Windows Defender Firewall for the **Private** profile only. Do not create a router port-forward for LAN-only sharing.
6. Open the displayed `http://PRIVATE-IP:PORT` URL from another device on the same private network.

LAN routes bind Caddy to the detected private IPv4 address rather than a wildcard interface. Website application processes remain bound to loopback. The Core Service management listener remains bound to `127.0.0.1:47821` and is not directly exposed to the LAN.

## Security and limitations

- LAN access is an explicit per-website setting and is off by default.
- Disabling LAN access removes the route on the next Caddy reload; the assigned port is retained for convenient re-enabling.
- This feature does not configure Windows Firewall automatically and does not change router settings.
- It does not enable internet/public access. Do not forward the LAN ports on your router.
- LAN URLs are based on the active private IPv4 address and can change when the PC changes networks.
- Current LAN publishing supports static and Node.js websites. PHP and Python adapters are not available yet.
- Public domain verification, public DNS diagnostics, tunnels and HTTPS provisioning remain separate work.

Removing a local domain deletes only its registry assignment. It does not delete website files or stop a website.


## Public domain diagnostics (Phase 8)

The **Public Hosting** screen can resolve a public hostname using the machine's configured DNS resolver and display its IPv4/IPv6 answers. Optionally enter the expected public IPv4 address to check whether the domain's A records include it.

This is a diagnostic only:
- It does not publish a website or change Caddy configuration.
- It does not open Windows Firewall, modify router settings, or enable port forwarding.
- A successful DNS lookup does not prove domain ownership or that inbound traffic reaches this PC.
- HTTPS is not considered ready until the domain reaches the public ingress and Caddy successfully completes its ACME challenge.
- Never expose the Core Service management API to the public internet.
