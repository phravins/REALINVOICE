//! Embedded migrations, run on startup.
//!
//! Migrations are compiled into the binary so a fresh machine needs nothing but the
//! executable. Applied names are recorded in `schema_migrations`; running twice is a
//! no-op, and each migration runs in its own transaction.

use rusqlite::Connection;

use crate::error::Result;

/// Ordered list of `(name, sql)`. Append only — never edit an applied migration.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    ("002_users", include_str!("../migrations/002_users.sql")),
    ("003_settings", include_str!("../migrations/003_settings.sql")),
];

/// Applies every migration that hasn't run yet against `conn`.
pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         -- Another console (or the future sync worker) holding the write lock should
         -- make this one wait its turn, not fail the sale.
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS schema_migrations (
             name       TEXT PRIMARY KEY,
             applied_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;

    for (name, sql) in MIGRATIONS {
        let already_applied: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM schema_migrations WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )?;
        if already_applied {
            continue;
        }

        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute("INSERT INTO schema_migrations (name) VALUES (?1)", [name])?;
        tx.commit()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_create_every_table_and_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        run_migrations(&mut conn).unwrap();

        for table in
            ["customers", "items", "invoices", "invoice_lines", "sync_queue", "users", "settings"]
        {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "missing table {table}");
        }

        let applied: i64 =
            conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| row.get(0)).unwrap();
        assert_eq!(applied, MIGRATIONS.len() as i64);
    }
}
