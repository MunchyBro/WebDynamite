# Managed databases

WebDynamite currently manages SQLite database files in its application data directory. The internal WebDynamite registry is separate and is never exposed as a user-managed database.

## Create and test

Open **Databases**, enter a name using letters, numbers, underscores, or hyphens (1–48 characters), and choose **Create SQLite database**. Use **Test** to run SQLite's integrity check.

## Export

Choose **Export** beside a managed database. WebDynamite checkpoints the SQLite WAL before reading the database file. If SQLite reports active readers preventing a complete checkpoint, the export is refused so the resulting file is not silently incomplete.

Exports are limited to 16 MiB. The downloaded file uses the `.sqlite3` extension.

## Import

Choose **Import SQLite file**, select an existing SQLite database file, and provide a new managed database name. WebDynamite checks the SQLite file header, opens it, runs `PRAGMA integrity_check`, checkpoints its WAL, and normalizes it to DELETE journal mode before registering it.

Imports are limited to 16 MiB. Import always creates a new managed database; it does not overwrite an existing database. The destination name must be unique. Invalid or corrupt files are rejected and temporary import files are cleaned up.

## Limitations

- PostgreSQL and MySQL/MariaDB instance management are not yet implemented.
- Import/export are local file operations; they do not synchronize data to a remote host.
- There is no SQL query console or automated application connection-string injection yet.
- Export/import size limits are 16 MiB per database.
