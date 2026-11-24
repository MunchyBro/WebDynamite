# WebDynamite Core Service

The Core Service is a separate process from the Tauri desktop UI. It owns the internal SQLite database and exposes a loopback-only HTTP management API.

## Current capabilities

- Binds only to `127.0.0.1:47821`.
- Stores the internal database under `%PROGRAMDATA%\\WebDynamite\\database\\webdynamite.sqlite3` by default.
- Supports `WEBDYNAMITE_DATA_DIR` for development and test environments.
- Requires `WEBDYNAMITE_API_TOKEN` with at least 32 characters.
- Maintains a website registry in SQLite with repeatable schema migrations.
- Supports static project registration and serving, and Node.js `npm run start` process launch.
- Supports website start, stop, restart and bounded log retrieval.
- Keeps hosted static content behind a path-canonicalization check to prevent directory traversal.

## Development

PowerShell:

```powershell
$env:WEBDYNAMITE_API_TOKEN = "replace-with-a-random-secret-at-least-32-characters"
cargo run --manifest-path core-service/Cargo.toml
```

The Windows desktop app generates a random API token on first launch and stores it in Windows Credential Manager under the current Windows account. It passes the token to the Core Service only through the child-process environment. Do not commit tokens or write them to ordinary logs.

## Desktop lifecycle and packaging

The Tauri desktop app starts the bundled `webdynamite-core.exe` when no authenticated instance is already listening, waits up to 12 seconds for its loopback listener, and then detaches from the child process. Closing the desktop window does **not** stop the Core Service; a later desktop launch reuses the existing process only after authenticating with the stored API token. If port 47821 is occupied by a process that cannot authenticate as this installation, startup fails rather than silently connecting to it.

This is still an interactive-session process, **not** a Windows Service Control Manager (SCM) service. It does not provide machine-startup execution before sign-in or SCM crash/reboot recovery. The SCM dispatcher, lifecycle controller, trusted configuration loader, and named-pipe authentication code in this repository are security/integration primitives; they are not connected to the production Core Service startup path. Do not register or activate SCM mode until protected machine credential migration, service runtime composition, signed-binary provisioning, transactional installer lifecycle, and Windows install/upgrade/recovery tests are complete.

The Windows bundle includes the Core Service as an application resource. Build it before launching the desktop app in development:

```powershell
cargo build --release --manifest-path core-service/Cargo.toml
npm run tauri dev
```

The CI workflow builds this executable before checking the Tauri backend. The installer workflow currently produces unsigned engineering artifacts only. Tagged releases are deliberately blocked until the production signing and release gates documented in [Windows code signing](../docs/WINDOWS-CODE-SIGNING.md) are complete.

## Monitoring

The authenticated `GET /api/v1/monitoring/metrics` endpoint returns host CPU, memory, disk, network counters and OS uptime. Each response includes `collected_at_unix_ms`, a Unix epoch timestamp in milliseconds identifying when that sample was collected. Memory and disk sizes are bytes; network counters are cumulative since boot or interface reset.

## API

Health (no token):

```http
GET http://127.0.0.1:47821/api/v1/health
```

All `/api/v1/websites` endpoints require `Authorization: Bearer <token>`.

| Method | Endpoint | Purpose |
|---|---|---|
| GET | `/api/v1/websites` | List registered websites |
| POST | `/api/v1/websites` | Register a project |
| GET | `/api/v1/websites/{id}` | Get a website |
| DELETE | `/api/v1/websites/{id}` | Stop and remove registry record |
| POST | `/api/v1/websites/{id}/start` | Start a static or Node.js website |
| POST | `/api/v1/websites/{id}/stop` | Stop a managed Node.js process or static route |
| POST | `/api/v1/websites/{id}/restart` | Restart a website |
| GET | `/api/v1/websites/{id}/logs` | Read the latest process log content |

Create request:

```json
{
  "name": "Portfolio",
  "project_path": "C:\\\\Sites\\\\portfolio",
  "runtime": "static"
}
```

Supported start adapters in this batch:

- **Static:** requires `index.html`; served from `http://127.0.0.1:47821/hosted/{id}/index.html`.
- **Node.js:** runs `npm run start` in the project directory. Ensure the package defines a `start` script. Output is stored under the service logs directory.

PHP and Python can be registered for forward compatibility but their start adapters are not implemented yet. Node.js websites currently use the project's default start script; configurable commands, port allocation, reverse-proxy routing and continuous crash reconciliation remain future work. Static content is currently served through the Core Service's local port, not through custom domains.

## Security notes

- The API listens on loopback only; it is not a public management endpoint.
- Project paths must be absolute and exist at registration and start time.
- Static file requests are canonicalized and must remain inside the registered project directory.
- Public exposure, custom domains, HTTPS and firewall changes are not enabled by this service.
- A production desktop installer must provision the API token securely.

## Tests

```powershell
cargo test --manifest-path core-service/Cargo.toml
```
