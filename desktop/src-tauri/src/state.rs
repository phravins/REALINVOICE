//! Process-wide state: one `Db`, opened at startup and shared by every command.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use realinvoice_core::{seed, Db};

/// Name of this billing node. Hardcoded until the Settings pane exists.
pub const NODE_NAME: &str = "POS-01";

/// The desktop app's own database file, inside Tauri's app config dir. Deliberately
/// separate from anything the other runtimes use: the console must keep billing when
/// every other machine is unreachable.
pub const DB_FILE_NAME: &str = "db.sqlite";

/// The shared connection. One `Db` for the process lifetime, reused by every command
/// rather than reopened per call.
pub struct AppState {
    db: Mutex<Db>,
    db_path: PathBuf,
}

impl AppState {
    /// Opens the database at `path`, running migrations, and seeds demo data if it is
    /// empty so a fresh install has a customer to search for.
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let mut db = Db::open(&path).map_err(|e| e.to_string())?;
        seed::seed_if_empty(&mut db).map_err(|e| e.to_string())?;
        Ok(Self { db: Mutex::new(db), db_path: path })
    }

    /// Locks the shared connection. The lock is poisoned only if a command panicked
    /// mid-write; SQLite has already rolled that transaction back, so recovering the
    /// guard is safe and keeps the counter billing.
    pub fn db(&self) -> MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Where this node's SQLite file lives. Shown in the status bar.
    pub fn db_path(&self) -> &std::path::Path {
        &self.db_path
    }
}
