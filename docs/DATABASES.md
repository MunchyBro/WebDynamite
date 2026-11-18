# Managed databases

WebDynamite currently manages SQLite database files in its application data directory and can save connection profiles for an existing local PostgreSQL server. The internal registry database is separate and is not exposed through the managed database API.

## SQLite: create, test, import, and export

Open **Databases** to create a managed SQLite database. Use **Test** to run SQLite's integrity check, **Export** to download a database file, or **Import SQLite file** to validate and register an existing database under a new name.

Import and export are limited to 16 MiB per database. Export checkpoints the WAL and refuses the operation if SQLite reports a busy checkpoint. Import validates the SQLite header, runs `PRAGMA integrity_check`, checkpoints the WAL, and normalizes the imported file to DELETE journal mode. Imports never overwrite existing databases.

## SQLite read-only SQL console

Select a managed SQLite database and enter a SQL statement in **SQLite query console**. The Core Service opens the selected file with SQLite's read-only flag and also rejects statements SQLite identifies as write-capable.

Response limits:
- SQL input: 64 KiB
- Result columns: 100
- Returned rows: 200
- Approximate serialized result budget: 1 MiB
- Text and BLOB cell previews: 8 KiB per cell

BLOB previews are returned as Base64 objects. Text and BLOB previews may be shortened. This console is for inspection, not writes or administrative changes.

## PostgreSQL: local connection profiles

The **Local PostgreSQL connections** section saves connection metadata for a PostgreSQL server already installed and running on this PC. The initial integration intentionally permits only `localhost`, `127.0.0.1`, and `::1`; remote hosts are rejected because this initial client uses PostgreSQL's NoTLS mode.

A profile stores its name, host, port, database name, and username. **Test** asks for the password at test time and connects to the server to report the database, authenticated user, and server version. The password is sent only for that request and is not written to the WebDynamite database or profile.

Removing a profile removes only its saved connection metadata. It does not stop PostgreSQL, delete a database, or modify server data.

## PostgreSQL read-only SQL console

Choose a saved local PostgreSQL profile in **PostgreSQL read-only query console** and run one SQL statement. The password is requested for every execution and is never stored. Queries run inside a PostgreSQL read-only transaction, with a five-second statement timeout. PostgreSQL extended-protocol preparation rejects multi-statement input.

Response limits:
- SQL input: 64 KiB
- Result columns: 100
- Returned rows: 200 (streamed; excess rows are not buffered)
- Approximate serialized result budget: 1 MiB
- Text and bytea previews: 8 KiB per cell

The console is for inspection, not writes or administrative changes. Unsupported PostgreSQL data types are identified in the result instead of being decoded. The existing loopback-only NoTLS restriction still applies.

## Not yet implemented

- PostgreSQL server installation, initialization, process lifecycle, and data-directory management.
- PostgreSQL server installation, initialization, process lifecycle, and data-directory management; database/user provisioning.
- Remote PostgreSQL TLS connections and protected persistent credential storage.
- MySQL/MariaDB support.
- Application connection-string injection.
