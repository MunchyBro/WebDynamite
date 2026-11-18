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

## PostgreSQL read-only query console

Select a saved local PostgreSQL profile in **PostgreSQL read-only query console**. Enter one row-returning SQL statement and run it. The Core Service opens a fresh connection using a password requested for that query, then executes inside a PostgreSQL read-only transaction. It applies a 5-second statement timeout and a 10-second idle transaction timeout. Only one statement is accepted; multi-statement scripts and write-capable operations are not supported.

Results are limited to 200 rows, 8 KiB per text value, and approximately 1 MiB of serialized cell data. A row is represented as JSON so PostgreSQL values can be displayed without requiring a client-side driver for every PostgreSQL type. Results may be truncated. The password is request-only and is never stored in a profile.

The read-only transaction is an additional database guard, not a sandbox for arbitrary SQL functions. Only run queries you trust.

## Not yet implemented

- PostgreSQL server installation, initialization, process lifecycle, and data-directory management.
- PostgreSQL query console and database/user provisioning.
- Remote PostgreSQL TLS connections and protected persistent credential storage.
- MySQL/MariaDB support.
- Application connection-string injection.
