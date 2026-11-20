# WebDynamite

**A Windows-first desktop control center for running, managing, and publishing websites from your own PC.**

WebDynamite brings local website hosting, domains, deployment workflows, databases, file management, monitoring, and backups into one desktop application. It is built for developers who want control over the services running on their own machine, with optional ways to expose selected websites to a LAN or the public internet.

> **Project status:** WebDynamite is under active development. The repository contains working desktop and Core Service functionality, but it is not yet a fully hardened commercial hosting platform. Review the limitations and security notes below before exposing a machine to the internet.

## What it includes

- **Website management:** register projects, start/stop/restart supported runtimes, inspect logs, and manage project files. Runtime stdout/stderr is continuously drained and capped at 10 MiB per website process; log views read only the last 256 KiB.
- **Runtime and builds:** static sites, Node.js process profiles, PHP/Python runtime support, and configurable build/deployment profiles. Build commands are limited to two simultaneous builds across the Core Service and have a configurable 30–1,800 second timeout (default 600 seconds); build stdout/stderr is continuously drained but stored up to 10 MiB per build, while build history displays at most the last 256 KiB. The exact runtime adapter depends on the installed runtime and project configuration.
- **Domains and routing:** local domain registry, Caddy configuration integration, DNS diagnostics for public domains, and TLS readiness/verification tools.
- **Public access:** LAN publishing and Cloudflare Quick Tunnel workflows. A global public-exposure switch is off by default; disabling it stops active Quick Tunnels and removes public-domain routes from the generated Caddy configuration while leaving local/LAN routes independently managed. Public availability still depends on DNS, firewall/router rules, tunnel state, and the target machine remaining online.
- **Databases:** managed SQLite databases, import/export and query tools, plus saved PostgreSQL and MySQL/MariaDB connection profiles with read-only query workflows.
- **File manager:** browse website files, search/sort, create folders, rename, upload/download files and folders, and work with ZIP archives.
- **Git and releases:** inspect repository state, create and roll back static-site release snapshots, delete inactive snapshots, and automatically prune old releases using a configurable 1–100 release retention limit (default 10).
- **Backups:** full-system backup/restore workflows, verification, and scheduled retention controls.
- **Operations:** system metrics, diagnostics, SSL/security checks, logs, and application settings. Dynamic runtime startup waits for the application port to become reachable before reporting success.

Features that depend on external programs or network services require those programs, permissions, credentials, and connectivity to be configured on the host.

## Architecture

| Component | Technology | Responsibility |
|---|---|---|
| Desktop application | Tauri 2, React, TypeScript, Vite | Windows UI and native desktop integration |
| Native desktop layer | Rust | Starts and supervises the Core Service; protects the API token with Windows Credential Manager |
| Core Service | Rust, Axum, Tokio | Local management API, website/runtime operations, database and backup workflows |
| Local state | SQLite | Website registry and application metadata |
| Local reverse proxy | Caddy integration | Local domain routing and HTTPS configuration workflows |
| Optional public connectivity | Cloudflare Tunnel | Quick Tunnel publishing without directly opening an inbound router port |

The Core Service currently listens on `127.0.0.1:47821`. It is designed as a local control API, not as an internet-facing management endpoint. Public hosting is globally disabled by default and must be explicitly enabled in the Networking screen; this switch controls public-domain proxy routes and Cloudflare Quick Tunnels, not LAN sharing. The desktop app launches the bundled `webdynamite-core.exe`, waits for its listener, and passes a randomly generated API token through the child-process environment. The token is stored using Windows Credential Manager.

## Requirements

### To run the Windows application

- Windows 10 or Windows 11 (64-bit).
- Microsoft Edge WebView2 Runtime.
- The WebDynamite Windows installer, when a published release is available.

### To build from source

- Node.js 22 or a compatible current LTS release, with npm.
- Rust stable toolchain and the MSVC target.
- Visual Studio Build Tools with the C++ build tools / Windows SDK.
- WebView2 Runtime and the Tauri 2 Windows prerequisites.

Optional tools are needed only for the feature that uses them:

- Caddy for reverse-proxy and local-domain routing.
- Cloudflared for Cloudflare Tunnel publishing.
- Node.js, PHP, Python, Git, and database servers/clients for the matching project or integration.

## Build and run from source

Open PowerShell in the repository root.

1. Install frontend dependencies:

   ```powershell
   npm install
   ```

2. Build the Core Service executable. The desktop bundle expects it at `target/release/webdynamite-core.exe`:

   ```powershell
   cargo build --release --manifest-path core-service/Cargo.toml
   ```

3. Start the desktop application in development mode:

   ```powershell
   npm run tauri dev
   ```

4. Create a production frontend and desktop build:

   ```powershell
   npm run build
   npm run tauri -- build --bundles nsis
   ```

The NSIS installer is produced under the Tauri build output directory, typically `src-tauri/target/release/bundle/nsis/`.

### Run the Core Service separately (development/debugging)

The desktop application normally starts the service for you. To run it independently for API development, open a separate PowerShell window and set a strong, temporary token:

```powershell
$env:WEBDYNAMITE_API_TOKEN = [guid]::NewGuid().ToString() + [guid]::NewGuid().ToString()
cargo run --manifest-path core-service/Cargo.toml
```

The service requires `WEBDYNAMITE_API_TOKEN` to contain at least 32 characters and binds to loopback. For isolated development/test data, set `WEBDYNAMITE_DATA_DIR` to a writable directory before starting it. See [Core Service documentation](core-service/README.md) for API details, storage paths, and security behavior.

## First-run workflow

1. Install and launch WebDynamite.
2. Open **Websites** and register a project folder. Choose the runtime that matches the project.
3. Start the website and inspect its status and logs. For Node.js projects, make sure the project has the expected start script or configure a runtime profile.
4. Use **File Manager** to inspect or update website files.
5. Open **Domains** to register local hostnames and review/export the local routing configuration. Caddy and local host resolution must be configured on the Windows machine for custom local hostnames to work.
6. Use **Networking** to review LAN access. Only enable access when you understand which interfaces and firewall rules will expose the site.
7. For public publishing, configure a real domain and DNS or use the available tunnel workflow. A local website is not automatically public just because it is registered in WebDynamite.
8. Configure databases, backups, monitoring, and security checks as needed.

## Databases

- **SQLite:** create and manage local databases, inspect tables, run supported queries, and import/export data.
- **PostgreSQL:** save a connection profile and use the read-only query and schema/table inspection workflows.
- **MySQL/MariaDB:** save a connection profile and use the read-only query workflow.

External database connections require a reachable database server and valid credentials. Treat saved connection credentials as sensitive. Do not put database passwords in source code, issue reports, screenshots, or logs.

## Backups and restore

WebDynamite includes full-system backup and restore workflows with verification and scheduling/retention controls. Current backup creation requires websites to be stopped to obtain a consistent snapshot. The service enforces limits of 100,000 files and 10 GiB per backup. New backup manifests use SHA-256 integrity checks; legacy version 1 FNV-1a manifests remain supported by the verifier.

A checksum proves that the backed-up files have not changed since the manifest was written; it does **not** encrypt the backup or protect it from an attacker who can replace both the data and manifest. Store backups separately from the machine being protected and restrict filesystem access. Test restores before relying on a backup as your disaster-recovery plan.

## Tests and validation

Run the Core Service tests:

```powershell
cargo test --manifest-path core-service/Cargo.toml
```

Build the frontend:

```powershell
npm run build
```

Build the Windows NSIS installer:

```powershell
cargo build --release --manifest-path core-service/Cargo.toml
npm run tauri -- build --bundles nsis
```

Pull requests run the repository's Windows CI workflows, including Rust tests, frontend checks, and an NSIS installer build.

## Release process

The repository includes a Windows installer workflow at `.github/workflows/windows-installer.yml`.

1. Update the application version consistently in `package.json`, `src-tauri/tauri.conf.json`, and `src-tauri/Cargo.toml`.
2. Ensure the changes are merged into `main` and the required CI checks pass.
3. Create and push a version tag matching the application version, for example `v0.1.0`.
4. The tag-triggered GitHub Actions workflow builds the Windows NSIS installer and publishes a GitHub Release with the installer asset.
5. Verify the release page contains the expected version and downloadable `.exe`.

The current installer is **unsigned**. Windows SmartScreen may show an unknown-publisher warning. Code signing, automatic updates, and a fully tested in-place upgrade/migration path are not yet configured. Do not describe an installer as signed or an update as automatic until those capabilities are implemented and verified.

## Security and deployment cautions

- Keep the Core Service management API on loopback. Do not reverse-proxy or port-forward `127.0.0.1:47821` to the public internet.
- Public websites and the management API are different things: expose only the website ports/routes you intend to publish.
- Review Caddy, Windows Firewall, router, DNS, and tunnel settings before enabling public access.
- Use least-privilege Windows accounts and restrict access to website folders, database files, credentials, and backup directories.
- Keep Windows, runtimes, dependencies, and database servers patched.
- Use HTTPS for public websites and verify the certificate from an external network.
- Keep offline/off-machine backups and periodically test restoration.
- The installer is currently unsigned; only install builds from a repository/release you trust.

## Current limitations

WebDynamite is actively evolving. The existence of a dashboard or configuration screen does not imply every hosting scenario is production-ready. Runtime behavior varies by framework and project. Dynamic runtime startup now requires an HTTP response from the assigned loopback port (2xx–4xx); this is a basic readiness probe, not a configurable application-specific health check. Advanced multi-tenant hosting isolation, hardened sandboxing, signed releases, automatic desktop updates, and some advanced hosting-provider operations remain future work. Review the code and the Core Service documentation before using this on a production or multi-user server.

## Project links

- [Core Service documentation](core-service/README.md)
- [Windows release workflow](.github/workflows/windows-installer.yml)
- [Issues and bug reports](https://github.com/MunchyBro/WebDynamite/issues)
- [Releases](https://github.com/MunchyBro/WebDynamite/releases)

## Who

```powershell
   BrailStudio
   ```
## License

WebDynamite is distributed under the MIT License. See [LICENSE](LICENSE).


### Runtime crash recovery

Node.js, PHP, and Python websites can optionally restart after an unexpected non-zero process exit. This is disabled by default and is configurable in Settings → Hosting. When enabled, WebDynamite waits the configured delay and makes a bounded number of restart attempts (1–10). Manual stops and clean exits are not restarted. A website that remains healthy for five minutes gets a fresh retry budget after a later crash. Recovery waits and startup attempts run independently per website, so one site's restart delay does not pause supervision of other sites. This is process-exit recovery, not continuous HTTP health monitoring or a Windows service recovery guarantee.
