//! src/main_components/python_bindings.rs - Native PyO3 bridge exposing nex-box to Python.

#[cfg(feature = "python")]
use std::path::PathBuf;
#[cfg(feature = "python")]
use pyo3::exceptions::PyRuntimeError;
#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
use super::diff::FileChangeKind;
#[cfg(feature = "python")]
use super::error::NexBoxError;
#[cfg(feature = "python")]
use super::sandbox::{ExecutionPolicy, SandboxSession};

#[cfg(feature = "python")]
#[pyclass(name = "WorkspaceDiff")]
#[derive(Clone)]
pub struct PyWorkspaceDiff {
    #[pyo3(get)]
    pub files_created: Vec<String>,
    #[pyo3(get)]
    pub files_modified: Vec<String>,
    #[pyo3(get)]
    pub files_deleted: Vec<String>,
    #[pyo3(get)]
    pub elapsed_micros: u128,
}

#[cfg(feature = "python")]
#[pymethods]
impl PyWorkspaceDiff {
    pub fn __repr__(&self) -> String {
        format!(
            "WorkspaceDiff(created={}, modified={}, deleted={}, elapsed_micros={})",
            self.files_created.len(),
            self.files_modified.len(),
            self.files_deleted.len(),
            self.elapsed_micros
        )
    }
}

#[cfg(feature = "python")]
#[pyclass(name = "SandboxOutcome")]
pub struct PySandboxOutcome {
    #[pyo3(get)]
    pub session_id: String,
    #[pyo3(get)]
    pub exit_code: i32,
    #[pyo3(get)]
    pub duration_ms: u64,
    #[pyo3(get)]
    pub files_cloned: usize,
    #[pyo3(get)]
    pub clone_duration_micros: u128,
    #[pyo3(get)]
    pub committed: bool,
    #[pyo3(get)]
    pub stdout: String,
    #[pyo3(get)]
    pub stderr: String,
    #[pyo3(get)]
    pub diff: PyWorkspaceDiff,
}

#[cfg(feature = "python")]
#[pymethods]
impl PySandboxOutcome {
    pub fn __repr__(&self) -> String {
        format!(
            "SandboxOutcome(exit_code={}, duration_ms={}ms, committed={}, files_cloned={})",
            self.exit_code, self.duration_ms, self.committed, self.files_cloned
        )
    }

    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Executes a command inside an ephemeral zero-copy CoW sandbox.
///
/// Parameters:
/// - command: list of strings (e.g. ["pytest", "tests/"])
/// - cwd: directory to clone (None = current directory)
/// - commit: bool, automatically commit changes to host if exit_code == 0 (default: False)
/// - timeout_ms: int, timeout in milliseconds (default: 60000)
/// - allow_net: bool, allow outbound network access (default: True)
/// - preserve: bool, keep scratch directory for debugging (default: False)
#[cfg(feature = "python")]
#[pyfunction]
#[pyo3(signature = (command, cwd=None, commit=false, timeout_ms=60000, allow_net=true, preserve=false))]
pub fn run(
    command: Vec<String>,
    cwd: Option<String>,
    commit: bool,
    timeout_ms: u64,
    allow_net: bool,
    preserve: bool,
) -> PyResult<PySandboxOutcome> {
    if command.is_empty() {
        return Err(PyRuntimeError::new_err("Command list cannot be empty"));
    }

    let workspace_root = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .canonicalize()
        .map_err(|e| PyRuntimeError::new_err(format!("Workspace canonicalization failed: {}", e)))?;

    let policy = ExecutionPolicy {
        timeout_ms,
        allow_network: allow_net,
        auto_commit: commit,
        preserve_scratch: preserve,
        json_telemetry: true,
    };

    let session = SandboxSession::initialize(&workspace_root, policy)
        .map_err(|e| PyRuntimeError::new_err(format!("Sandbox initialization error: {}", e)))?;

    let outcome = session
        .execute(&command)
        .map_err(|e| match e {
            NexBoxError::CommandTimeout(ms) => {
                PyRuntimeError::new_err(format!("Execution timed out after {}ms", ms))
            }
            other => PyRuntimeError::new_err(format!("Execution failure: {}", other)),
        })?;

    let mut created = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();

    for delta in &outcome.diff.deltas {
        let path_str = delta.relative_path.to_string_lossy().to_string();
        match delta.change_kind {
            FileChangeKind::Created => created.push(path_str),
            FileChangeKind::Modified => modified.push(path_str),
            FileChangeKind::Deleted => deleted.push(path_str),
        }
    }

    let diff = PyWorkspaceDiff {
        files_created: created,
        files_modified: modified,
        files_deleted: deleted,
        elapsed_micros: outcome.diff.elapsed_micros,
    };

    Ok(PySandboxOutcome {
        session_id: outcome.session_id,
        exit_code: outcome.exit_code,
        duration_ms: outcome.duration_ms,
        files_cloned: outcome.files_cloned,
        clone_duration_micros: outcome.clone_duration_micros,
        committed: outcome.committed,
        stdout: outcome.stdout_capture,
        stderr: outcome.stderr_capture,
        diff,
    })
}