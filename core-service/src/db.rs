use rusqlite::{Connection, Result};

fn has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in columns {
        if name? == column { return Ok(true); }
    }
    Ok(false)
}

pub fn initialize(connection: &Connection) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS websites (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            project_path TEXT NOT NULL,
            runtime TEXT NOT NULL CHECK (runtime IN ('static', 'node', 'php', 'python')),
            status TEXT NOT NULL DEFAULT 'stopped' CHECK (status IN ('stopped', 'starting', 'running', 'stopping', 'error')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_websites_name ON websites(name);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);
        "
    )?;

    // Version 2 adds per-project launch configuration without discarding existing rows.
    if !has_column(connection, "websites", "start_command")? {
        connection.execute("ALTER TABLE websites ADD COLUMN start_command TEXT", [])?;
    }
    if !has_column(connection, "websites", "port")? {
        connection.execute("ALTER TABLE websites ADD COLUMN port INTEGER", [])?;
    }
    connection.execute("INSERT OR IGNORE INTO schema_migrations(version) VALUES (2)", [])?;
    // Version 3 introduces explicit domain-to-website assignments.
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS domains (
            id TEXT PRIMARY KEY NOT NULL,
            website_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
            hostname TEXT NOT NULL COLLATE NOCASE UNIQUE,
            access_mode TEXT NOT NULL DEFAULT 'local' CHECK (access_mode IN ('local')),
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_domains_website ON domains(website_id);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES (3);"
    )?;
    // Version 4 adds opt-in LAN publishing. Existing websites remain private by default.
    if !has_column(connection, "websites", "lan_enabled")? {
        connection.execute("ALTER TABLE websites ADD COLUMN lan_enabled INTEGER NOT NULL DEFAULT 0 CHECK (lan_enabled IN (0,1))", [])?;
    }
    if !has_column(connection, "websites", "lan_port")? {
        connection.execute("ALTER TABLE websites ADD COLUMN lan_port INTEGER", [])?;
    }
    connection.execute("INSERT OR IGNORE INTO schema_migrations(version) VALUES (4)", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_schema_and_is_idempotent() {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        initialize(&connection).unwrap();
        let version: i64 = connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 4);
        assert!(has_column(&connection, "websites", "start_command").unwrap());
        assert!(has_column(&connection, "websites", "port").unwrap());
        let domains: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='domains'", [], |row| row.get(0)).unwrap();
        assert_eq!(domains, 1);
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM websites", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}
