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


## System monitoring (Phase 12)

The Monitoring page and Overview health panel read live host metrics from the authenticated Core Service endpoint `/api/v1/monitoring/metrics`.

- CPU usage is sampled over a short interval; memory reports used and total physical memory.
- Disk usage aggregates the total and available space of detected mounted volumes.
- Network received/transmitted values are cumulative interface counters since boot or interface reset; they are not instantaneous bandwidth speeds.
- The desktop refreshes metrics every five seconds while Overview or Monitoring is open.
- The endpoint is protected by the same bearer-token middleware as the other management APIs and remains bound to the Core Service loopback listener.


## Managed SQLite databases (Phase 13)

Open **Databases** to create a separate SQLite database for an application. WebDynamite stores the database file under its managed data directory and keeps only its metadata (name, engine and file path) in the internal registry. Use **Test** to run SQLite's integrity check.

- Database names are limited to letters, numbers, underscores and hyphens.
- Deleting a managed database permanently removes its database file and associated SQLite WAL/SHM sidecars. Export or back it up before deletion.
- The internal WebDynamite registry database is not listed as an application database.
- This phase supports SQLite file creation, listing, size reporting, integrity testing and deletion only. PostgreSQL/MySQL discovery, protected credentials, database users/permissions, SQL console, import/export and website connection injection are not implemented yet.


## Website File Manager (Phase 14)

The desktop **File Manager** browses files under a registered website's project folder. It can list directories, create UTF-8 text files, open/edit/save UTF-8 text files up to 1 MiB, and delete files or empty directories.

- File operations are authenticated through the Core Service API.
- Paths must be relative to the registered website root; absolute paths and parent-directory traversal are rejected.
- Existing targets are canonicalized and checked to remain inside the website root. Symbolic links are shown but not followed or edited.
- The website root itself cannot be deleted. Non-empty directories cannot be deleted.
- Binary files and text files larger than 1 MiB are not opened in the built-in editor. Use an external editor for those files.
- This phase does not provide upload/download, drag-and-drop, archive extraction, permission editing, or multi-file operations.


## Automated static deployment (Phase 19)

The **Deployments** screen includes **Build & deploy** for static websites with a saved build profile.

1. Configure the executable, separate arguments and expected output folder.
2. Save the profile. The executable must be one of the supported programs; arguments are passed directly without a shell.
3. Select **Build & deploy** and confirm. WebDynamite runs the build, records its output and exit status, verifies that the configured output directory exists inside the website root, and creates an immutable release snapshot.
4. The new release is activated only after the build and snapshot succeed. The previous release remains in history and can be reactivated from **Git & Releases**.

If the build fails, the active release is not changed. If release creation fails (for example, because output contains a symlink or sensitive file), the deployment is not activated. Inspect the build history for command output.

This workflow currently activates static-site releases only. Node.js, PHP and Python build commands can still be run separately, but automated release activation for those runtimes is not yet supported. Deployment is local to this PC; it does not upload artifacts to a remote server or configure hosting infrastructure.


## PHP and Python framework launchers (Phase 20)

When a website registered as **PHP** is started, WebDynamite detects a Laravel project when both `artisan` and `public/index.php` exist and runs Laravel's development server bound to `127.0.0.1` on the allocated per-site port. Other PHP projects continue to use PHP's built-in server.

For websites registered as **Python**, the launcher checks the project root in this order:

1. `manage.py` — starts Django with `manage.py runserver 127.0.0.1:PORT`.
2. `main.py`, `app.py`, then `wsgi.py` — detects a `FastAPI(` application and starts `uvicorn MODULE:app`, or detects a `Flask(` application and starts Flask's CLI.
3. If no supported framework signature is found, it retains the Python static-file server.

Install each framework and its server dependency in the Python environment selected in Settings before starting the site (Django, Flask, or FastAPI plus Uvicorn). Laravel requires PHP and its Composer-installed project dependencies. All detected servers bind to loopback only; Caddy continues to provide the configured domain routing.

Detection is intentionally conservative and uses conventional entry points and an application variable named `app`. Projects with factory patterns, nonstandard modules/variable names, virtual-environment-specific interpreters, ASGI alternatives, or custom startup arguments still need configurable launch profiles in a later phase. These development servers are not a substitute for hardened production process managers.


## Custom runtime launch profiles (Phase 21)

A custom launch profile overrides automatic runtime detection for a non-static website. Configure an executable and one argument per line in **Runtime Profiles**. Arguments are passed directly without a command shell. The process working directory is the project folder and it receives `PORT` and `HOST=127.0.0.1` environment variables. Use `{port}` and `{project}` placeholders in arguments. Save or remove profiles only while the website is stopped. Removing a profile restores the built-in runtime launcher. Profiles are included in full-system backups.

**Security:** a custom executable runs with the same operating-system permissions as WebDynamite. This feature does not sandbox processes or prevent a user-selected program from binding to other interfaces; use trusted executables and configure them to listen only on loopback.


## Runtime startup reliability (Hosting phase 1)

Dynamic runtime startup probes the configured readiness path for up to 15 seconds before reporting the website as running. The readiness wait yields to the asynchronous Core Service runtime between probes, so one slow website startup does not occupy a Tokio worker for the entire wait. If startup exits or times out, WebDynamite records the website as `error` (or `stopped` for a clean early exit) and attempts to terminate the complete process tree on Windows before returning. Cleanup failures are included in the API error instead of being silently ignored. A successful readiness response confirms only that the configured local HTTP path is responding; it does not verify application correctness or public reachability.


## Caddy startup verification and logs

Starting Caddy is not treated as success merely because Windows created a process. WebDynamite waits up to five seconds for Caddy's local admin TCP endpoint at `127.0.0.1:2019` and reports a startup error if the process exits or the endpoint never becomes ready. The proxy status response includes the process state, local admin readiness and the log-file path. Caddy stdout and stderr are appended to `logs/caddy.log` so startup and runtime errors remain available for diagnosis. This check proves only local Caddy admin-socket readiness; it does not prove a website route, public DNS, inbound connectivity or HTTPS issuance.
