//! An OpenFX (OFX 1.4) image-effect host for EffectCraft.
//!
//! This crate loads native OFX plug-in bundles (RE:Vision ReelSmart Motion Blur, Boris FX
//! Sapphire, the free OFX collections, ...) and registers every plug-in as an ordinary
//! EffectCraft effect through the plug-in API v2 (`effectcraft_effects::plugin`). It is a CPU
//! host: float premultiplied RGBA images (byte / short / RGB / alpha plug-ins are converted),
//! no GPU or OpenGL, no overlay interacts. See `docs/ofx.md` for usage and how it fits together.
//!
//! **This crate runs native code in-process, without a sandbox**, unlike EffectCraft's own
//! plug-ins, which are sandboxed WebAssembly. It is opt-in: only hosts built with the `openfx`
//! feature (`effectcraft-host/openfx`) link it. It needs `unsafe` to speak the C ABI, so it does
//! not opt into the workspace's `unsafe_code = "forbid"` lint; every host callback is wrapped in
//! `catch_unwind`, null-checks its pointers and validates handles by magic number. On wasm32 the
//! crate is empty.
//!
//! Modules: [`ffi`] (the C ABI), [`props`] (property sets), [`params`] (parameter storage and the
//! mapping to EffectCraft parameters), [`clips`] (clips, images and pixel conversion),
//! [`instance`] (effect handles, geometry, per-render context, actions), [`effect`] (the
//! `EffectPlugin` implementation), [`suites`] (the host suites), [`loader`] (bundle discovery
//! and registration) and [`blocklist`] (skips plug-ins that crashed the app while loading).

// Native code: nothing to host on the web build.
#![cfg(not(target_arch = "wasm32"))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blocklist;
pub mod clips;
pub mod effect;
pub mod ffi;
pub mod instance;
pub mod loader;
pub mod params;
pub mod props;
pub mod suites;

use std::path::PathBuf;

pub use blocklist::set_state_dir;
pub use loader::{default_search_paths, load_path};

/// One OFX plug-in that was registered as an effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedEffect {
    /// The EffectCraft effect id: the OFX identifier lower-cased (`ofx.` is prefixed if it
    /// would start with `ec.`).
    pub id: String,
    /// Display name (the plug-in's label).
    pub name: String,
    /// Effects & Presets category: the first component of the plug-in's grouping.
    pub category: String,
    /// The OFX plug-in identifier.
    pub ofx_id: String,
    /// The OFX context the effect runs in: `filter`, `general`, `transition` or `generator`.
    pub context: String,
    /// The bundle folder (or binary) it came from.
    pub bundle: String,
}

/// The outcome of [`load_path`].
#[derive(Clone, Debug, Default)]
pub struct LoadReport {
    pub loaded: Vec<LoadedEffect>,
    pub errors: Vec<String>,
}

/// Engine hook: [`load_path`] as `{"loaded": [{id, name, category, ofxId, context, bundle}], "errors": [...]}`.
/// `Err` only when nothing could be loaded and there were errors.
pub fn loader(path: &str) -> Result<serde_json::Value, String> {
    let report = load_path(&PathBuf::from(path));
    if report.loaded.is_empty() && !report.errors.is_empty() {
        return Err(report.errors.join("; "));
    }
    let loaded: Vec<serde_json::Value> = report
        .loaded
        .iter()
        .map(|e| serde_json::json!({"id": e.id, "name": e.name, "category": e.category, "ofxId": e.ofx_id, "context": e.context, "bundle": e.bundle}))
        .collect();
    Ok(serde_json::json!({"loaded": loaded, "errors": report.errors}))
}
