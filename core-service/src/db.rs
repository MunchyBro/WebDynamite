use rusqlite::{Connection, Result};

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

        let version: i64 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);

        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM websites", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
