//! EffectCraft desktop app binary. The app itself is the library (`lib.rs`, [`effectcraft::run`]);
//! Android's NativeActivity entry point is the `effectcraft-android` crate.
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    effectcraft::run()
}
