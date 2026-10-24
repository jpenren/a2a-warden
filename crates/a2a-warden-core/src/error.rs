//! Error types for A2A Warden

use thiserror::Error;

/// Core error type for A2A Warden
#[derive(Error, Debug)]
pub enum Error {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Database error
    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),

    /// A2A protocol error
    #[error("Protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Storage error
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// HTTP client error (the warden acting as an A2A client)
    #[error("HTTP client error: {0}")]
    HttpClient(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Validation error
    #[error("Validation error: {0}")]
    Validation(String),

    /// Not found error
    #[error("Not found: {0}")]
    NotFound(String),

    /// Conflict error
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Unauthorized error
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// Forbidden error
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Task in invalid state
    #[error("Task in invalid state: {0}")]
    InvalidState(String),

    /// Approval error
    #[error("Approval error: {0}")]
    Approval(String),

    /// Policy evaluation error
    #[error("Policy error: {0}")]
    Policy(String),

    /// Cryptography error
    #[error("Cryptography error: {0}")]
    Crypto(String),
}

/// Configuration-specific errors
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to load configuration: {0}")]
    Load(#[from] config::ConfigError),

    #[error("Validation failed: {0}")]
    Validation(String),

    #[error("Environment variable error: {0}")]
    EnvVar(#[from] std::env::VarError),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),
}

/// Database-specific errors.
///
/// The core crate does not depend on sqlx or on a concrete backend (AD-2), so
/// backend errors are carried as their `Display` representation.
#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Connection failed: {0}")]
    Connection(String),

    #[error("Migration failed: {0}")]
    Migration(String),

    #[error("Pool error: {0}")]
    Pool(String),

    #[error("Transaction error: {0}")]
    Transaction(String),

    #[error("Backend not supported: {0}")]
    UnsupportedBackend(String),
}

/// A2A Protocol-specific errors
#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("Invalid A2A version: {0}")]
    InvalidVersion(String),

    #[error("Missing required header: {0}")]
    MissingHeader(String),

    #[error("Invalid JSON-RPC request: {0}")]
    InvalidJsonRpc(String),

    #[error("Method not found: {0}")]
    MethodNotFound(String),

    #[error("Invalid params: {0}")]
    InvalidParams(String),

    #[error("Invalid task state transition: {0}")]
    InvalidStateTransition(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Skill not found: {0}")]
    SkillNotFound(String),

    #[error("Agent Card verification failed: {0}")]
    CardVerificationFailed(String),

    #[error("Push notification error: {0}")]
    PushNotification(String),
}

/// Storage trait errors
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Message not found: {0}")]
    MessageNotFound(String),

    #[error("Approval not found: {0}")]
    ApprovalNotFound(String),

    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Push notification config not found: {0}")]
    PushNotificationNotFound(String),

    #[error("Duplicate entry: {0}")]
    Duplicate(String),

    #[error("Constraint violation: {0}")]
    Constraint(String),

    #[error("Query error: {0}")]
    Query(String),
}

/// Result type alias
pub type Result<T> = std::result::Result<T, Error>;

/// Convert any error to internal error
impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Error::Internal(err.to_string())
    }
}

/// Convert string to internal error
impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Internal(s)
    }
}

/// Convert &str to internal error
impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error::Internal(s.to_string())
    }
}

/// Helper trait for adding context to errors
pub trait ErrorContext<T> {
    fn context(self, msg: &str) -> Result<T>;
    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String;
}

impl<T, E> ErrorContext<T> for std::result::Result<T, E>
where
    E: Into<Error>,
{
    fn context(self, msg: &str) -> Result<T> {
        self.map_err(|e| e.into()).map_err(|e| match e {
            Error::Internal(s) => Error::Internal(format!("{}: {}", msg, s)),
            _ => Error::Internal(format!("{}: {}", msg, e)),
        })
    }

    fn with_context<F>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| e.into()).map_err(|e| match e {
            Error::Internal(s) => Error::Internal(format!("{}: {}", f(), s)),
            _ => Error::Internal(format!("{}: {}", f(), e)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_chain() {
        let err: std::result::Result<(), anyhow::Error> = Err(anyhow::anyhow!("root cause"));
        let err: Result<()> = err.context("adding context");
        assert!(err.is_err());
        match err.unwrap_err() {
            Error::Internal(msg) => assert!(msg.contains("adding context")),
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_error_conversion() {
        let err: Error = "test error".into();
        match err {
            Error::Internal(msg) => assert_eq!(msg, "test error"),
            _ => panic!("Wrong error type"),
        }
    }
}
