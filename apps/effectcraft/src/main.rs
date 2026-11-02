//! EffectCraft desktop binary: the crate's thin entry point over the library (`lib.rs`,
//! [`effectcraft::run`]). Android's NativeActivity entry point calls `effectcraft::run_android`
//! instead, from the entry crate that owns the exported C symbol.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

/// Run a normal desktop launch: install logging and the panic hook, parse arguments (`--version` exits before the
/// session log is attached), scan fonts, honor the launch guard's backend choice and open the eframe window.
/// Builds the session, loads settings, plugins, the requested project/media and the optional control server, then
/// reports startup failure through `startup_failed`. Returns the event loop's result.
fn main() -> eframe::Result {
    effectcraft::run()
}
