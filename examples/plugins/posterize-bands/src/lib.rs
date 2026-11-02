//! Posterize Bands: an example Visualab Effects effect plug-in (plug-in API v1, see
//! `docs/plugins.md`). It quantizes each pixel's luminance into `levels` bands and mixes a tint
//! colour in by `mix` percent, optionally only above a `threshold`.
//!
//! Build: `cargo build --release --target wasm32-unknown-unknown`, then load
//! `target/wasm32-unknown-unknown/release/posterize_bands.wasm` with Effect ▸ Load Effect Plug-in
//! (or `effect.plugins.load {"path": …}`), or drop it in the settings folder's `Plug-ins`.

use std::cell::RefCell;

/// The manifest: id, name, Effects & Presets category, parameters (flattened in this order:
/// levels, mix, tint r g b a, invert).
const MANIFEST: &str = r#"{
  "api": 1,
  "id": "org.effectcraft.example.posterize-bands",
  "name": "Posterize Bands",
  "category": "Stylize",
  "version": "1.0.0",
  "author": "Visualab Effects contributors",
  "description": "Quantizes luminance into tinted bands.",
  "params": [
    {"id": "levels", "name": "Levels", "type": "slider", "default": 4, "min": 2, "max": 64, "sliderMax": 16, "decimals": 0},
    {"id": "mix", "name": "Tint Amount", "type": "slider", "default": 50, "min": 0, "max": 100, "decimals": 1},
    {"id": "tint", "name": "Tint", "type": "color", "default": [1.0, 0.55, 0.2, 1.0]},
    {"id": "invert", "name": "Invert Bands", "type": "checkbox", "default": false}
  ]
}"#;

thread_local! {
    static BUF: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_api_version() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_manifest_ptr() -> i32 {
    MANIFEST.as_ptr() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ec_manifest_len() -> i32 {
    MANIFEST.len() as i32
}

/// One reusable, 8-byte-aligned buffer for the parameters and pixels.
#[unsafe(no_mangle)]
pub extern "C" fn ec_alloc(bytes: i32) -> i32 {
    BUF.with(|b| {
        let mut b = b.borrow_mut();
        let words = (bytes.max(0) as usize).div_ceil(8);
        if b.len() < words {
            b.resize(words, 0);
        }
        b.as_mut_ptr() as i32
    })
}

/// # Safety
/// The host passes pointers into the buffer `ec_alloc` returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ec_render(pixels: i32, width: i32, height: i32, params: i32, nparams: i32, _time: f64, _scale: f64) -> i32 {
    if nparams < 7 {
        return 1;
    }
    let p = unsafe { std::slice::from_raw_parts(params as *const f64, nparams as usize) };
    let px = unsafe { std::slice::from_raw_parts_mut(pixels as *mut [f32; 4], (width.max(0) * height.max(0)) as usize) };
    let levels = p[0].round().max(2.0) as f32;
    let mix = (p[1] / 100.0).clamp(0.0, 1.0) as f32;
    let tint = [p[2] as f32, p[3] as f32, p[4] as f32];
    let invert = p[6] != 0.0;
    for c in px.iter_mut() {
        let a = c[3];
        if a <= 0.0 {
            continue;
        }
        // Un-premultiply, band the luminance, re-premultiply.
        let (r, g, b) = (c[0] / a, c[1] / a, c[2] / a);
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let mut band = (y * levels).floor().min(levels - 1.0) / (levels - 1.0);
        if invert {
            band = 1.0 - band;
        }
        for (i, ch) in [r, g, b].into_iter().enumerate() {
            let banded = if y > 0.0 { ch * band / y.max(1e-6) } else { band };
            let v = banded * (1.0 - mix) + band * tint[i] * mix;
            c[i] = v.max(0.0) * a;
        }
    }
    0
}
