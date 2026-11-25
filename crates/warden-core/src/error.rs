// OWNER: scaffold
//! Crate-wide error type. Every fallible public fn returns [`Result<T>`].

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum WardenError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("http error: {0}")]
    Http(String),

    #[error("forge error ({status}): {msg}")]
    Forge { status: u16, msg: String },

    #[error("git error: {0}")]
    Git(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("engine error: {0}")]
    Engine(String),

    #[error("guardrail error: {0}")]
    Guardrail(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, WardenError>;

impl From<reqwest::Error> for WardenError {
    fn from(e: reqwest::Error) -> Self {
        WardenError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for WardenError {
    fn from(e: serde_json::Error) -> Self {
        WardenError::Other(format!("json: {e}"))
    }
}

impl From<serde_yaml::Error> for WardenError {
    fn from(e: serde_yaml::Error) -> Self {
        WardenError::Config(format!("yaml: {e}"))
    }
}

impl From<sqlx::migrate::MigrateError> for WardenError {
    fn from(e: sqlx::migrate::MigrateError) -> Self {
        WardenError::Other(format!("migration: {e}"))
    }
}

impl From<keyring::Error> for WardenError {
    fn from(e: keyring::Error) -> Self {
        WardenError::Other(format!("keychain: {e}"))
    }
}

impl From<regex::Error> for WardenError {
    fn from(e: regex::Error) -> Self {
        WardenError::Config(format!("regex: {e}"))
    }
}

impl From<globset::Error> for WardenError {
    fn from(e: globset::Error) -> Self {
        WardenError::Config(format!("glob: {e}"))
    }
}

impl From<anyhow::Error> for WardenError {
    fn from(e: anyhow::Error) -> Self {
        WardenError::Other(e.to_string())
    }
}

impl From<tokio::task::JoinError> for WardenError {
    fn from(e: tokio::task::JoinError) -> Self {
        WardenError::Other(format!("task: {e}"))
    }
}

/// Serialized as the display string so Tauri commands can return `Result<T, WardenError>` directly.
impl Serialize for WardenError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_as_string() {
        let e = WardenError::Forge {
            status: 404,
            msg: "no such project".into(),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "\"forge error (404): no such project\""
        );
    }
}
