//! src/main_components/error.rs - High-precision error domain for nex-box.

use std::fmt;

pub type Result<T> = std::result::Result<T, NexBoxError>;

#[derive(Debug)]
pub enum NexBoxError {
    Io(String, std::io::Error),
    Serialization(serde_json::Error),
    ExecutionFailure(String),
    CommandTimeout(u64),
    #[allow(dead_code)]
    ReflinkUnsupported(String),
    SessionNotFound(String),
    #[allow(dead_code)]
    AtomicCommitConflict(String),
}

impl fmt::Display for NexBoxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(ctx, err) => write!(f, "Filesystem I/O Error: {} ({})", ctx, err),
            Self::Serialization(err) => write!(f, "JSON Serialization Failure: {}", err),
            Self::ExecutionFailure(msg) => write!(f, "Sandbox Execution Error: {}", msg),
            Self::CommandTimeout(ms) => write!(f, "Execution timed out after {}ms", ms),
            Self::ReflinkUnsupported(path) => write!(f, "Reflink/CoW cloning unsupported on mount: {}", path),
            Self::SessionNotFound(id) => write!(f, "Sandbox session '{}' does not exist or expired", id),
            Self::AtomicCommitConflict(detail) => write!(f, "Commit Conflict: {}", detail),
        }
    }
}

impl std::error::Error for NexBoxError {}

impl From<serde_json::Error> for NexBoxError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err)
    }
}