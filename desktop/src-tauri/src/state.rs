//! Process-wide state: one `Db`, and the session of whoever is signed in.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use realinvoice_core::{seed, Db, User};
use serde::{Deserialize, Serialize};

/// Name of this billing node. Hardcoded until the Settings pane exists.
pub const NODE_NAME: &str = "POS-01";

/// The desktop app's own database file, inside Tauri's app config dir. Deliberately
/// separate from anything the other runtimes use: the console must keep billing when
/// every other machine is unreachable.
pub const DB_FILE_NAME: &str = "db.sqlite";

/// Who is signed in, for as long as this process runs.
///
/// The token is checked on every command so a caller must present something it was given
/// rather than assert it is signed in. There is no expiry and nothing is persisted: close
/// the app and the session is gone, which is the correct lifetime for a till that should
/// not stay signed in after hours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user: User,
}

/// The shared connection and the current session.
pub struct AppState {
    db: Mutex<Db>,
    db_path: PathBuf,
    session: Mutex<Option<Session>>,
}

impl AppState {
    /// Opens the database at `path`, running migrations, and seeds demo data if it is
    /// empty.
    ///
    /// No account is created here. An installation with no users is *unconfigured*, and
    /// the app asks the person in front of it to create one. Nothing is generated,
    /// printed to a terminal, or hardcoded — the first password only ever exists as the
    /// hash they cause to be written.
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let mut db = Db::open(&path).map_err(|e| e.to_string())?;
        seed::seed_if_empty(&mut db).map_err(|e| e.to_string())?;

        Ok(Self { db: Mutex::new(db), db_path: path, session: Mutex::new(None) })
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

    /// The current session, or `None` when nobody is signed in.
    pub fn session(&self) -> Option<Session> {
        self.session.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Starts a session.
    pub fn begin_session(&self, session: Session) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session);
    }

    /// Ends the session. Nothing about the signed-in user survives this.
    pub fn end_session(&self) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}
