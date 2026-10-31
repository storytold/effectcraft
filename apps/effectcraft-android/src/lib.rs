//! EffectCraft on Android.
//!
//! The same engine and egui UI as the desktop app, run by eframe's winit shell as a plain
//! [`android.app.NativeActivity`] — no Java, no Kotlin, no Gradle: `cargo apk` packages
//! `libeffectcraft_android.so` into an APK (see [`crate::android`] and `docs/android.md`).
//!
//! What the desktop shell gets from its OS, this shell wires up where Android allows: file
//! dialogs use an in-app browser (no `rfd` on Android), preview audio goes through cpal/AAudio,
//! and settings, auto-saves and the session log live in the app's own internal storage.
//!
//! The shell itself is Android-only; the in-app browser and the preview audio it wires up are
//! platform-neutral, so on every other target they stay in the build and `cargo xtask ci` lints
//! and tests them on the desktop.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
mod android;
pub mod audio_out;
pub mod file_browser;

/// The entry point `android-activity`'s glue calls, by symbol name, on the thread it spawns for
/// the activity: an `extern "Rust"` function, so plain Rust ABI, not `extern "C"`.
///
/// The symbol can only be exported with `#[unsafe(no_mangle)]`; the function body itself holds
/// no unsafe code. This is the workspace's single exception to `unsafe_code = "forbid"`: the
/// crate sets `deny` and allows it here, once (see `Cargo.toml`).
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android::run(app);
}
