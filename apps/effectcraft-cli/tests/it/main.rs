//! The crate's integration tests, one binary (each module is a former `tests/<name>.rs`).

mod cli;
mod qa_template;

/// Nextest archives relocate binaries; plain Cargo keeps the compile-time path valid.
fn binary() -> std::ffi::OsString {
    std::env::var_os("NEXTEST_BIN_EXE_effectcraft_cli").unwrap_or_else(|| env!("CARGO_BIN_EXE_effectcraft-cli").into())
}
