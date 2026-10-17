# Startup launch-guard ownership (#477)

Refs https://github.com/storytold/effectcraft/issues/477.

The investigation reproduced a separate concurrent-launch ownership bug: a second process could remove the live launch's pending marker, and locking then unlinking the marker did not provide a stable ownership boundary. This does not establish recurrence of the original Linux/NVIDIA surface error on current main.

The implementation uses a persistent launch.lock file, acquires ownership before reading or changing recovery state, respects an older version's locked pending marker, and removes the owned pending marker before releasing the guard. A non-owner leaves another launch's marker and preferences alone. WGPU_BACKEND overrides remain authoritative.

Validation on Linux with Rust 1.99.0: native workspace 3,412 passed, 0 failed, 26 ignored; release startup follow-up 40 unit and 3 argument tests passed; formatting, strict release Clippy, layers and assets passed. The offline WASM-only retry checked all 32 crates successfully after checksum-verified reuse of existing local crate archives. Earlier aggregate commands had stopped at a missing offline dependency; the native suite was not rerun for that retry. One unchanged unused-variable WASM warning remains in pointer_wrap.rs.

Actual patched-release initialization failure/recovery and explicit-backend assertions passed. Linux software-rendered startup was exercised; physical NVIDIA hardware and native macOS startup were not tested.
