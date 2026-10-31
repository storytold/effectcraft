//! # effectcraft-plugin (L3)
//!
//! Loads **WebAssembly effect plug-ins** (plug-in API v1, see `docs/plugins.md`) into the effect
//! registry ([`effectcraft_effects::plugin`]). A plug-in is a `.wasm` (or `.wat`) module with
//! no imports that exports:
//!
//! | export | signature | |
//! |---|---|---|
//! | `memory` | memory | linear memory shared with the host |
//! | `ec_api_version` | `() -> i32` | must return 1 |
//! | `ec_manifest_ptr`, `ec_manifest_len` | `() -> i32` | a UTF-8 JSON [`PluginManifest`] in memory |
//! | `ec_alloc` | `(bytes: i32) -> i32` | a buffer of at least `bytes` bytes, 8-byte aligned; it may reuse the previous one |
//! | `ec_render` | `(pixels: i32, width: i32, height: i32, params: i32, nparams: i32, time: f64, scale: f64) -> i32` | process the pixels in place; 0 = ok |
//!
//! For each render the host asks `ec_alloc` for one buffer holding the parameters (`nparams`
//! little-endian `f64`s) followed by the pixels (`width × height × 4` little-endian `f32`s,
//! premultiplied RGBA, row-major), calls `ec_render`, and reads the pixels back.
//!
//! Modules run in the [wasmi](https://github.com/wasmi-labs/wasmi) interpreter (pure Rust,
//! MIT/Apache-2.0) with deterministic floats, no imports (no files, network or clock) and a
//! fuel limit per frame, so a plug-in can't escape its sandbox, hang the app, or render
//! differently on another machine. The runtime is the `wasm` feature (on for the desktop app and
//! the CLI); without it [`load_wasm`] says plug-ins are unavailable.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use effectcraft_effects::EffectSpec;
pub use effectcraft_effects::plugin::{EffectPlugin, PLUGIN_API_VERSION, PluginFrame, PluginManifest, PluginParams, register_plugin};

#[cfg(feature = "wasm")]
mod wasm;

/// Whether this build can load WebAssembly plug-ins.
pub fn wasm_available() -> bool {
    cfg!(feature = "wasm")
}

/// Load and register a WebAssembly plug-in (`bytes` = a `.wasm` binary or `.wat` text;
/// `source` = where it came from, for listings). Returns the registered effect.
pub fn load_wasm(bytes: &[u8], source: &str) -> Result<&'static EffectSpec, String> {
    #[cfg(feature = "wasm")]
    {
        let p = wasm::WasmPlugin::new(bytes, source)?;
        register_plugin(std::sync::Arc::new(p))
    }
    #[cfg(not(feature = "wasm"))]
    {
        let _ = (bytes, source);
        Err("WebAssembly plug-ins are not available in this build".into())
    }
}

/// [`load_wasm`] for the engine's plug-in loader hook: `{id, name, category, version, api}`.
pub fn loader(bytes: &[u8], source: &str) -> Result<serde_json::Value, String> {
    let spec = load_wasm(bytes, source)?;
    let m = effectcraft_effects::plugin::plugin(spec.id).map(|p| p.manifest().clone());
    Ok(serde_json::json!({
        "id": spec.id,
        "name": spec.name,
        "category": spec.category,
        "version": m.as_ref().map(|m| m.version.clone()).unwrap_or_default(),
        "api": PLUGIN_API_VERSION,
        "params": spec.params.len(),
    }))
}

/// Render a module's effect without registering it (the Plug-in Library's preview): default
/// parameters, `pixels` (premultiplied RGBA, `width × height`) at each of `times` (seconds).
/// Returns the manifest as JSON and one frame per time.
pub fn preview(bytes: &[u8], width: u32, height: u32, pixels: &[[f32; 4]], times: &[f64]) -> Result<(serde_json::Value, Vec<Vec<[f32; 4]>>), String> {
    #[cfg(feature = "wasm")]
    {
        use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginParamKind, PluginParams};
        let p = wasm::WasmPlugin::new(bytes, "preview")?;
        let m = p.manifest();
        let mut flat = Vec::with_capacity(m.flat_len());
        for param in &m.params {
            match &param.kind {
                PluginParamKind::Slider { default, .. } | PluginParamKind::Angle { default } => flat.push(*default),
                PluginParamKind::Checkbox { default } => flat.push(if *default { 1.0 } else { 0.0 }),
                PluginParamKind::Popup { default, .. } => flat.push(*default as f64),
                // Points default to fractions of the layer: the frame is the layer.
                PluginParamKind::Point { default } => flat.extend([default[0] * f64::from(width), default[1] * f64::from(height)]),
                PluginParamKind::Color { default } => flat.extend(default.iter().copied()),
            }
        }
        let params = PluginParams { manifest: m, flat: &flat };
        let mut frames = Vec::with_capacity(times.len());
        for &t in times {
            let mut px = pixels.to_vec();
            let mut frame = PluginFrame { width, height, pixels: &mut px, scale: 1.0 };
            p.render(&mut frame, &params, t)?;
            frames.push(px);
        }
        let manifest = serde_json::to_value(m).map_err(|e| e.to_string())?;
        Ok((manifest, frames))
    }
    #[cfg(not(feature = "wasm"))]
    {
        let _ = (bytes, width, height, pixels, times);
        Err("WebAssembly plug-ins are not available in this build".into())
    }
}

// Two attributes, so clippy sees `cfg(test)` and treats the module as test code.
#[cfg(test)]
#[cfg(feature = "wasm")]
mod tests;
