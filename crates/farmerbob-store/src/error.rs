//! Error types for the store.

use thiserror::Error;

/// Errors raised by [`Store`] operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A database error occurred.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// An unrecognized state name was found in the database.
    #[error("unknown state: {0}")]
    UnknownState(String),

    /// State data is malformed or missing.
    #[error("invalid state data: {0}")]
    InvalidStateData(String),

    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),
}
