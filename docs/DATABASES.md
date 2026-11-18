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

BLOB previews are returned as Base64 objects. Text and BLOB previews may be shortened. Results can be exported as UTF-8 CSV; exports include only the bounded rows returned by the query API and guard text cells that could be interpreted as spreadsheet formulas. This console is for inspection, not writes or administrative changes.

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

The console is for inspection, not writes or administrative changes. Unsupported PostgreSQL data types are identified in the result instead of being decoded. The existing loopback-only NoTLS restriction still applies. Query results can be exported from the desktop UI as UTF-8 CSV (with a BOM for spreadsheet compatibility). CSV cells are quoted and embedded quotes are escaped; text cells beginning with spreadsheet formula characters are prefixed to reduce formula-injection risk. Exports contain only rows/columns returned by the bounded query response.

### Schema browser shortcuts

The PostgreSQL console includes metadata shortcuts to list non-system schemas, list tables in a schema, and inspect a table's columns (ordinal position, type, nullability, and default). These shortcuts query `information_schema` through the same read-only endpoint and use SQL-escaped schema/table values. They do not modify database objects. **Preview rows** builds a quoted-identifier `SELECT * ... LIMIT 200` for the selected schema and table, then runs it through the same read-only query endpoint. The preview is capped by the query console response limits and does not change data.

## MySQL / MariaDB: local connection profiles

The **Local MySQL / MariaDB connections** section stores connection metadata for an existing MySQL or MariaDB server on this PC. Profiles accept only `localhost`, `127.0.0.1`, or `::1` and default to port `3306`. The database name may be left blank to connect without selecting a default database. The connection test reports the selected database (if any), authenticated account, and server version. The password is requested for each test and is never persisted. The initial integration disables TLS because it is restricted to loopback hosts; remote database connections are not supported.

Removing a profile deletes only its saved metadata, not the database server or its data. MySQL/MariaDB SQL console and schema management are not yet included.

## Not yet implemented

- PostgreSQL server installation, initialization, process lifecycle, and data-directory management.
- PostgreSQL server installation, initialization, process lifecycle, and data-directory management; database/user provisioning.
- Remote PostgreSQL TLS connections and protected persistent credential storage.
- MySQL/MariaDB support.
- Application connection-string injection.


MySQL/MariaDB connection profiles are currently limited to loopback hosts and non-TLS connections; passwords are entered only when testing a profile and are not stored.
