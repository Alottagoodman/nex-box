//! src/lib.rs - Root library entrypoint exporting main components and PyO3 C-extension.

pub mod main_components;

#[cfg(feature = "python")]
use pyo3::prelude::*;

#[cfg(feature = "python")]
use main_components::python_bindings::{run, PySandboxOutcome, PyWorkspaceDiff};

/// Python native C-extension module entrypoint
#[cfg(feature = "python")]
#[pymodule]
fn nex_box(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(run, m)?)?;
    m.add_class::<PySandboxOutcome>()?;
    m.add_class::<PyWorkspaceDiff>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}