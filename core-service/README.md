# WebDynamite Core Service

The Core Service is a separate process from the Tauri desktop UI. It owns the internal SQLite database and exposes a loopback-only HTTP management API.

## Current foundation capabilities

- Binds only to `127.0.0.1:47821`.
- Keeps the internal database under `%PROGRAMDATA%\WebDynamite\database\webdynamite.sqlite3` by default.
- Supports `WEBDYNAMITE_DATA_DIR` for development and test environments.
- Requires a configured bearer token (`WEBDYNAMITE_API_TOKEN`) of at least 32 characters.
- Provides a public process health endpoint and authenticated website registry endpoints.
- Uses parameterized SQL statements and validates website names, absolute project paths, and supported runtime identifiers.

## Run in a development terminal

PowerShell:

```powershell
$env:WEBDYNAMITE_API_TOKEN = "replace-with-a-random-secret-at-least-32-characters"
cargo run --manifest-path core-service/Cargo.toml
```

The token above is a development configuration. Production provisioning must generate and protect the token using Windows DPAPI or Credential Manager; it must not be committed to source control or written to ordinary logs.

## API

Health (no token):

```http
GET http://127.0.0.1:47821/api/v1/health
```

All website endpoints require `Authorization: Bearer <token>`:

- `GET /api/v1/websites`
- `POST /api/v1/websites`
- `GET /api/v1/websites/{id}`
- `DELETE /api/v1/websites/{id}`

Example create request:

```json
{
  "name": "Portfolio",
  "project_path": "C:\\Sites\\portfolio",
  "runtime": "static"
}
```

New records begin in the `stopped` state. Process start/stop/restart endpoints will be added with the website runtime supervisor in Phase 2.

## Verification

Run:

```powershell
cargo test --manifest-path core-service/Cargo.toml
```

The current test suite verifies that the initial schema migration is repeatable.
