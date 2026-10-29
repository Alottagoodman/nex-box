//! src/main_components/error.rs - High-precision error domain for nex-box.

use std::fmt;

pub type Result<T> = std::result::Result<T, NexBoxError>;

#[derive(Debug)]
pub enum NexBoxError {
    Io(String, std::io::Error),
    Serialization(serde_json::Error),
    ExecutionFailure(String),
    CommandTimeout(u64),
    CrossDeviceBoundary { workspace_dev: u64, session_dev: u64 },
    ReflinkUnsupported(String),
    SessionNotFound(String),
    OverlayFsMountFailed(String),
    OverlayFsUnmountFailed(String),
    AtomicCommitConflict(String),
}

impl fmt::Display for NexBoxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(ctx, err) => write!(f, "Filesystem I/O Error: {} ({})", ctx, err),
            Self::Serialization(err) => write!(f, "JSON Serialization Failure: {}", err),
            Self::ExecutionFailure(msg) => write!(f, "Sandbox Execution Error: {}", msg),
            Self::CommandTimeout(ms) => write!(f, "Execution timed out after {}ms (SIGKILL process group dispatched)", ms),
            Self::CrossDeviceBoundary { workspace_dev, session_dev } => write!(
                f,
                "Cross-device link violation: workspace st_dev ({}) != session st_dev ({}). Reflinks cannot cross mount points.",
                workspace_dev, session_dev
            ),
            Self::ReflinkUnsupported(path) => write!(f, "Reflink/CoW cloning unsupported on mount: {}", path),
            Self::SessionNotFound(id) => write!(f, "Sandbox session '{}' does not exist or expired", id),
            Self::OverlayFsMountFailed(detail) => write!(f, "OverlayFS mount initialization failed: {}", detail),
            Self::OverlayFsUnmountFailed(detail) => write!(f, "OverlayFS teardown cleanup failed: {}", detail),
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