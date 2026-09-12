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
    /// The one-time password for the account seeded on this launch, if this was a first
    /// run. Held in memory only and cleared once somebody signs in.
    first_run_password: Mutex<Option<String>>,
}

impl AppState {
    /// Opens the database at `path`, running migrations, seeds demo data if it is empty,
    /// and creates the default owner account if no users exist yet.
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let mut db = Db::open(&path).map_err(|e| e.to_string())?;
        seed::seed_if_empty(&mut db).map_err(|e| e.to_string())?;

        let first_run_password =
            match seed::seed_owner_if_empty(&mut db).map_err(|e| e.to_string())? {
                Some((owner, password)) => {
                    // Printed once, here, and never written to disk. After this the only copy
                    // is the bcrypt hash in the users table.
                    println!("\n================ RealInvoice first run ================");
                    println!("  A sign-in account has been created on this machine:");
                    println!("      username: {}", owner.username);
                    println!("      password: {password}");
                    println!("  Write it down — it is not stored anywhere in the clear");
                    println!("  and will not be shown again after someone signs in.");
                    println!("=======================================================\n");
                    Some(password)
                }
                None => None,
            };

        Ok(Self {
            db: Mutex::new(db),
            db_path: path,
            session: Mutex::new(None),
            first_run_password: Mutex::new(first_run_password),
        })
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

    /// Starts a session. Clears the first-run password: somebody has signed in, so the
    /// bootstrap credential has served its purpose.
    pub fn begin_session(&self, session: Session) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session);
        *self.first_run_password.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Ends the session. Nothing about the signed-in user survives this.
    pub fn end_session(&self) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// The one-time password from this launch, while it is still unused.
    pub fn first_run_password(&self) -> Option<String> {
        self.first_run_password.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}
