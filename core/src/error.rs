use thiserror::Error;

/// Everything the core crate can fail with. Kept small and `Serialize`-friendly at the
/// boundary so callers (Tauri commands, later the sync worker) can surface it verbatim.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
