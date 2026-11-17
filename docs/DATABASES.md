# Managed databases

WebDynamite currently manages SQLite database files in its application data directory. The internal registry database is separate and is not exposed through the managed database API.

## Create, test, import, and export

Open **Databases** to create a managed SQLite database. Use **Test** to run SQLite's integrity check, **Export** to download a database file, or **Import SQLite file** to validate and register an existing database under a new name.

Import and export are limited to 16 MiB per database. Export checkpoints the WAL and refuses the operation if SQLite reports a busy checkpoint. Import validates the SQLite header, runs `PRAGMA integrity_check`, checkpoints the WAL, and normalizes the imported file to DELETE journal mode. Imports never overwrite existing databases.

## Read-only SQL console

Select a managed database and enter a SQL statement in **SQLite query console**. The Core Service opens the selected file with SQLite's read-only flag and also rejects statements SQLite identifies as write-capable. This console is intended for inspecting tables and data; it is not an administrative SQL editor.

Response limits:
- SQL input: 64 KiB
- Result columns: 100
- Returned rows: 200
- Approximate serialized result budget: 1 MiB
- Text and BLOB cell previews: 8 KiB per cell

BLOB previews are returned as Base64 objects. Text and BLOB previews may be shortened. Results are marked as truncated when row or response limits are reached. Use your application's own SQLite connection for writes or larger exports.

## Limitations

- PostgreSQL and MySQL/MariaDB instance management are not yet implemented.
- The SQL console does not support write statements, multi-statement scripts, query parameters, or transaction editing.
- Application connection-string injection and user/role management are not implemented.
