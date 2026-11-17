# Local domains and LAN networking

WebDynamite supports local hostnames and opt-in LAN URLs for static, Node.js and PHP websites.

## Local domains

1. Register a static project with `index.html`, a Node.js project with an `npm run start` script, or a PHP project with PHP installed on `PATH`.
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
- Current LAN publishing supports static, Node.js, PHP and Python websites.
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


## PHP websites (Phase 9)

1. Install PHP for Windows and make `php.exe` available on the Core Service `PATH`.
2. Register the project folder with runtime **PHP**.
3. Start the website. WebDynamite launches PHP's built-in development server bound to `127.0.0.1` on an allocated per-site port, with the project folder as its document root.
4. Assign a local domain or explicitly enable LAN access as needed; PHP sites use the same Caddy reverse-proxy routing as Node.js sites.
5. Stop the site from WebDynamite when it is no longer needed.

The PHP built-in server is intended for local development and testing, not production-grade public hosting. Do not expose it directly to the internet. PHP must be installed separately; WebDynamite does not bundle the PHP runtime.


## Python websites (Phase 10)

1. Install Python 3 and ensure the `python` command is available on the Core Service `PATH`.
2. Register the project folder with runtime **Python**.
3. Start the website. WebDynamite launches `python -m http.server` bound to `127.0.0.1` on an allocated per-site port, serving the selected project directory.
4. Assign a local domain or explicitly enable LAN access as needed; Python sites use the same Caddy reverse-proxy routing as Node.js and PHP sites.

Python's built-in HTTP server is a simple static file server for development and testing. It does not execute Python web frameworks (such as Django, Flask or FastAPI), and is not a production-grade public hosting server. Python must be installed separately; WebDynamite does not bundle the runtime.


## Public domain publishing (Phase 11)

Public domains are separate from local `.localhost`/`.test` assignments. In **Public Hosting**, select a registered website and assign a public DNS hostname. WebDynamite stores that assignment and includes a hostname-based route in the generated Caddyfile. Caddy can then obtain and renew HTTPS certificates automatically after public DNS resolves to the correct ingress and inbound TCP ports 80 and 443 reach this machine.

- Public publishing is an explicit per-domain action; DNS diagnostics alone never publish a site.
- Static websites proxy to the Core Service static-file endpoint. Node.js, PHP and Python sites proxy to their assigned loopback application port; start the website before expecting its route to serve traffic.
- Use **Start Caddy with public routes** or **Apply & reload Caddy** to apply the generated configuration. A successful config validation does not prove public reachability or certificate issuance.
- Configure DNS A/AAAA records, router forwarding (if behind NAT), Windows Firewall rules and ISP ingress support yourself. Never expose the Core Service management API.
- Removing a public domain removes its generated route on the next Caddy reload; it does not delete website files or domain DNS records.
- Public hosting is not a hardened production deployment environment. Review application security, updates, backups and access controls before exposing any service.
