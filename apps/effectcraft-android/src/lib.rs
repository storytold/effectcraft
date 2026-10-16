//! Android entry point for EffectCraft.
//!
//! Android's `NativeActivity` loads this library and calls [`android_main`] on a dedicated
//! thread. It opens the shared app ([`effectcraft::run`], the same code the desktop binary
//! runs) and returns when the window closes, ending the activity's main-loop thread.
//!
//! This crate exists because that export must be unmangled, and on edition 2024 the only
//! spelling (`#[unsafe(no_mangle)]`) trips the workspace-forbidden `unsafe_code` lint. It also
//! holds `android_access` (Android builds), the one piece of real unsafe in the project: JNI
//! calls to the activity's JVM to check and request the "All files access" grant, whose safe
//! closures are handed to the shared app as [`effectcraft::SharedStorageAccess`]. The crate
//! therefore carries its own lints (see `Cargo.toml`) instead of the workspace's.
//!
//! Android limitations (v1): no system file picker or message dialogs (rfd has no Android
//! backend), so Save, Open Project, exports, folder picks and media import are served by the
//! in-app browser — app-private storage always, shared storage (`/storage/emulated/0`) too once
//! the user grants "All files access" (`android_access`); settings, plug-ins and crash recovery
//! use app-private storage.

#[cfg(target_os = "android")]
mod android_access;

/// NativeActivity's main-loop thread entry point (required; see the `android-activity` docs).
/// winit pumps the activity's lifecycle and input events inside [`effectcraft::run_android`],
/// which needs the activity's handle to create its event loop; when the activity is destroyed
/// that returns and this returns with it.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    // The "All files access" grant, checked and requested over JNI — the entry crate's unsafe
    // stays here; `run_android` only ever sees plain closures (built before the app moves).
    let shared = android_access::capture(&app).map(|a| effectcraft::SharedStorageAccess {
        granted: Box::new(move || a.shared_storage_granted()),
        ask: Box::new(move || a.ask_shared_storage_grant()),
    });
    if let Err(e) = effectcraft::run_android(app, shared) {
        log::error!("effectcraft exited with an error: {e}");
    }
}
