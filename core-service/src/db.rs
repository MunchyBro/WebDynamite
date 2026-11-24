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
    connection.execute_batch("CREATE TABLE IF NOT EXISTS public_domains (
        id TEXT PRIMARY KEY NOT NULL,
        website_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
        hostname TEXT NOT NULL COLLATE NOCASE UNIQUE,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE INDEX IF NOT EXISTS idx_public_domains_website ON public_domains(website_id);
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (5);")?;
    // Version 6 stores metadata for user-managed SQLite databases separately from the internal registry.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS managed_databases (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL COLLATE NOCASE UNIQUE,
        engine TEXT NOT NULL CHECK (engine IN ('sqlite')),
        file_path TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (6);")?;
    // Version 7 adds per-website build profiles and immutable build history.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS build_profiles (
        id TEXT PRIMARY KEY NOT NULL,
        website_id TEXT NOT NULL UNIQUE REFERENCES websites(id) ON DELETE CASCADE,
        program TEXT NOT NULL CHECK (program IN ('node','python','php','cargo','dotnet')),
        args_json TEXT NOT NULL,
        output_dir TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE TABLE IF NOT EXISTS build_runs (
        id TEXT PRIMARY KEY NOT NULL,
        website_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
        program TEXT NOT NULL,
        status TEXT NOT NULL CHECK (status IN ('success','failed')),
        exit_code INTEGER,
        duration_ms INTEGER NOT NULL,
        logs TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    CREATE INDEX IF NOT EXISTS idx_build_runs_website_created ON build_runs(website_id,created_at DESC);
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (7);")?;
    // Version 8 adds immutable release snapshots for static-site rollback.
    if !has_column(connection, "releases", "active")? {
        connection.execute_batch("CREATE TABLE IF NOT EXISTS releases (
            id TEXT PRIMARY KEY NOT NULL,
            website_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
            release_path TEXT NOT NULL UNIQUE,
            source TEXT NOT NULL DEFAULT 'build',
            commit_hash TEXT,
            active INTEGER NOT NULL DEFAULT 0 CHECK (active IN (0,1)),
            file_count INTEGER NOT NULL DEFAULT 0,
            size_bytes INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX IF NOT EXISTS idx_releases_website_created ON releases(website_id,created_at DESC);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_releases_one_active ON releases(website_id) WHERE active=1;")?;
    }
    connection.execute("INSERT OR IGNORE INTO schema_migrations(version) VALUES (8)", [])?;
    // Version 9 stores verified full-system backup inventory.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS backups (
        id TEXT PRIMARY KEY NOT NULL,
        backup_path TEXT NOT NULL UNIQUE,
        checksum TEXT NOT NULL,
        file_count INTEGER NOT NULL,
        size_bytes INTEGER NOT NULL,
        website_count INTEGER NOT NULL,
        database_count INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_backups_created ON backups(created_at DESC);
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (9);")?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY NOT NULL,
        value_json TEXT NOT NULL,
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (10);")?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS runtime_profiles (\n        id TEXT PRIMARY KEY NOT NULL,\n        website_id TEXT NOT NULL UNIQUE REFERENCES websites(id) ON DELETE CASCADE,\n        program TEXT NOT NULL,\n        args_json TEXT NOT NULL,\n        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP\n    );\n    INSERT OR IGNORE INTO schema_migrations(version) VALUES (11);
    CREATE TABLE IF NOT EXISTS backup_schedule (
        id INTEGER PRIMARY KEY CHECK (id=1),
        enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
        interval_hours INTEGER NOT NULL DEFAULT 24 CHECK (interval_hours BETWEEN 1 AND 720),
        retention_count INTEGER NOT NULL DEFAULT 7 CHECK (retention_count BETWEEN 1 AND 100),
        next_run_at INTEGER,
        last_run_at INTEGER,
        last_status TEXT,
        last_error TEXT
    );
    INSERT OR IGNORE INTO backup_schedule(id) VALUES (1);
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (12);")?;
    // Version 13 stores local PostgreSQL connection metadata; passwords are never persisted.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS postgres_connections (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL COLLATE NOCASE UNIQUE,
        host TEXT NOT NULL CHECK (host IN ('localhost','127.0.0.1','::1')),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        database_name TEXT NOT NULL,
        username TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (13);
    -- Version 14 stores local MySQL/MariaDB connection metadata; passwords are never persisted.
    CREATE TABLE IF NOT EXISTS mysql_connections (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL COLLATE NOCASE UNIQUE,
        host TEXT NOT NULL CHECK (host IN ('localhost','127.0.0.1','::1')),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        database_name TEXT NOT NULL,
        username TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
    );
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (14);")?;
    // Version 16 stores environment metadata; secret values remain in Windows Credential Manager.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS website_environment (
        website_id TEXT NOT NULL REFERENCES websites(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        value TEXT NOT NULL DEFAULT '',
        is_secret INTEGER NOT NULL DEFAULT 0 CHECK (is_secret IN (0,1)),
        updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
        PRIMARY KEY (website_id,name)
    );
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (16);")?;
    // Version 15 keeps a bounded audit trail of TLS certificate inspections.
    connection.execute_batch("CREATE TABLE IF NOT EXISTS tls_certificate_history (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        hostname TEXT NOT NULL COLLATE NOCASE,
        subject TEXT NOT NULL,
        issuer TEXT NOT NULL,
        not_before TEXT NOT NULL,
        not_after TEXT NOT NULL,
        thumbprint TEXT NOT NULL,
        checked_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_tls_history_host_checked ON tls_certificate_history(hostname, checked_at DESC);
    INSERT OR IGNORE INTO schema_migrations(version) VALUES (15);")?;
    // Version 17 adds an optional HTTP readiness path to custom runtime profiles.
    if !has_column(connection, "runtime_profiles", "health_path")? {
        connection.execute_batch("ALTER TABLE runtime_profiles ADD COLUMN health_path TEXT NOT NULL DEFAULT '/';")?;
    }
    connection.execute("INSERT OR IGNORE INTO schema_migrations(version) VALUES (17)", [])?;
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
        assert_eq!(version, 17);
        assert!(has_column(&connection, "websites", "start_command").unwrap());
        assert!(has_column(&connection, "websites", "port").unwrap());
        assert!(has_column(&connection, "websites", "lan_enabled").unwrap());
        assert!(has_column(&connection, "websites", "lan_port").unwrap());
        assert!(has_column(&connection, "runtime_profiles", "health_path").unwrap());
        let domains: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='domains'", [], |row| row.get(0)).unwrap();
        assert_eq!(domains, 1);
        let profiles: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='build_profiles'", [], |row| row.get(0)).unwrap();
        assert_eq!(profiles, 1);
        let runs: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='build_runs'", [], |row| row.get(0)).unwrap();
        assert_eq!(runs, 1);
        let releases: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='releases'", [], |row| row.get(0)).unwrap();
        assert_eq!(releases, 1);
        assert!(has_column(&connection, "releases", "file_count").unwrap());
        assert!(has_column(&connection, "releases", "size_bytes").unwrap());
        let backups: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='backups'", [], |row| row.get(0)).unwrap();
        assert_eq!(backups, 1);
        let settings: i64 = connection.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='settings'", [], |row| row.get(0)).unwrap();
        assert_eq!(settings, 1);
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM websites", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}
