//! The crate-wide error type and `Result` alias.

use thiserror::Error;

/// Everything that can go wrong inside `repomon-core`.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("git error: {0}")]
    Git(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("agent runtime error: {0}")]
    Agent(String),

    #[error("{0}")]
    Other(String),
}

/// The crate-wide `Result`.
pub type Result<T> = std::result::Result<T, Error>;
