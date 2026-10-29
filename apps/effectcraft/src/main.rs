//! EffectCraft desktop app binary. The app itself is the library (`lib.rs`, [`effectcraft::run`]);
//! Android's NativeActivity entry point is the `effectcraft-android` crate.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

/// Run a normal desktop launch: install logging and the panic hook, parse arguments (`--version` exits before the
/// session log is attached), scan fonts, honor the launch guard's backend choice and open the eframe window.
/// Builds the session, loads settings, plugins, the requested project/media and the optional control server, then
/// reports startup failure through `startup_failed`. Returns the event loop's result.
fn main() -> eframe::Result {
    effectcraft::run()
}
