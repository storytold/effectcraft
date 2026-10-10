//! TextBox is a linked Rust effect plug-in; no text-layer-specific rendering.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use effectcraft_effects::plugin::{EffectPlugin, PluginFrame, PluginManifest, PluginParams, register_plugin};
use effectcraft_effects::{Buf, Image};
use rayon::prelude::*;
use std::sync::{Arc, OnceLock};

pub const ID: &str = "org.effectcraft.text-box";
const MAX_PIXELS: usize = 16_777_216; // 256 MiB of RGBA f32, checked before allocation.
const MANIFEST: &str = include_str!("../manifest.json");

pub struct TextBox(PluginManifest);
impl TextBox {
    pub fn new() -> Result<Self, String> {
        let manifest: PluginManifest = serde_json::from_str(MANIFEST).map_err(|e| e.to_string())?;
        manifest.validate()?;
        Ok(Self(manifest))
    }
}

pub fn register() -> Result<(), String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED.get_or_init(|| register_plugin(Arc::new(TextBox::new()?)).map(|_| ())).clone()
}

/// Half-open pixel-edge bounds; any finite positive alpha counts, including antialiasing.
/// Blank lines and spaces have no pixels and therefore do not enlarge these bounds.
pub fn alpha_bounds(img: &Image) -> Option<[f64; 4]> {
    if img.width == 0 || img.height == 0 || (img.width as usize).checked_mul(img.height as usize) != Some(img.data.len()) {
        return None;
    }
    let w = img.width as usize;
    let mut bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (i, px) in img.data.iter().enumerate() {
        if px[3].is_finite() && px[3] > 0.0 {
            let (x, y) = ((i % w) as f64, (i / w) as f64);
            bounds[0] = bounds[0].min(x);
            bounds[1] = bounds[1].min(y);
            bounds[2] = bounds[2].max(x + 1.0);
            bounds[3] = bounds[3].max(y + 1.0);
        }
    }
    bounds[0].is_finite().then_some(bounds)
}

fn scalar(params: &PluginParams, id: &str, min: f64, max: f64) -> Result<f64, String> {
    let v = params.f(id);
    if !v.is_finite() {
        return Err(format!("non-finite TextBox parameter: {id}"));
    }
    Ok(v.clamp(min, max))
}

fn color(params: &PluginParams, id: &str) -> Result<[f64; 4], String> {
    let c = params.color(id);
    if c.iter().any(|v| !v.is_finite()) {
        return Err(format!("non-finite TextBox color: {id}"));
    }
    Ok(c.map(|v| v.clamp(0.0, 1.0)))
}

/// Signed distance to a rounded rectangle, with radius clamped to half its shorter edge.
fn distance(x: f64, y: f64, rect: [f64; 4], radius: f64) -> f64 {
    let hx = (rect[2] - rect[0]) * 0.5;
    let hy = (rect[3] - rect[1]) * 0.5;
    let r = radius.min(hx.min(hy)).max(0.0);
    let qx = (x - (rect[0] + hx)).abs() - hx + r;
    let qy = (y - (rect[1] + hy)).abs() - hy + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn over(front: [f32; 4], back: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - front[3].clamp(0.0, 1.0);
    std::array::from_fn(|i| front[i] + back[i] * k)
}
fn premult(c: [f64; 4], coverage: f64) -> [f32; 4] {
    let a = (c[3] * coverage) as f32;
    [c[0] as f32 * a, c[1] as f32 * a, c[2] as f32 * a, a]
}

impl EffectPlugin for TextBox {
    fn manifest(&self) -> &PluginManifest {
        &self.0
    }
    fn source(&self) -> String {
        "linked Rust plug-in: examples/plugins/text-box".into()
    }
    fn render(&self, _frame: &mut PluginFrame, _params: &PluginParams, _time: f64) -> Result<(), String> {
        Err("TextBox requires the Rust render_buffer host extension".into())
    }
    fn render_buffer(&self, input: &Buf, params: &PluginParams, _time: f64) -> Result<Buf, String> {
        let count = (input.img.width as usize).checked_mul(input.img.height as usize);
        if count != Some(input.img.data.len()) || input.img.data.len() > MAX_PIXELS {
            return Err("invalid or oversized TextBox input buffer".into());
        }
        let s = input.scale;
        if !s.is_finite() || s <= 0.0 || input.offset.iter().any(|v| !v.is_finite()) {
            return Err("invalid TextBox input coordinates".into());
        }
        let Some(b) = alpha_bounds(&input.img) else {
            return Ok(input.clone());
        };
        let px = scalar(params, "paddingX", -10000.0, 10000.0)?;
        let py = scalar(params, "paddingY", -10000.0, 10000.0)?;
        let sides = ["paddingLeft", "paddingTop", "paddingRight", "paddingBottom"].map(|id| scalar(params, id, -10000.0, 10000.0));
        let [left, top, right, bottom] = sides;
        let (left, top, right, bottom) = (left?, top?, right?, bottom?);
        let ox = scalar(params, "offsetX", -10000.0, 10000.0)? * s;
        let oy = scalar(params, "offsetY", -10000.0, 10000.0)? * s;
        let rect = [b[0] - (px + left) * s + ox, b[1] - (py + top) * s + oy, b[2] + (px + right) * s + ox, b[3] + (py + bottom) * s + oy];
        let radius = scalar(params, "cornerRadius", 0.0, 10000.0)? * s;
        let stroke_width = scalar(params, "strokeWidth", 0.0, 10000.0)? * s;
        let mut fill = color(params, "fillColor")?;
        fill[3] *= scalar(params, "fillOpacity", 0.0, 100.0)? / 100.0;
        let stroke = color(params, "strokeColor")?;
        let matte = scalar(params, "matte", 0.0, 1.0)? != 0.0;
        if rect.iter().chain([radius, stroke_width].iter()).any(|v| !v.is_finite()) {
            return Err("TextBox geometry is not finite".into());
        }
        // Negative padding can collapse either axis. An empty shape has no fill, stroke
        // or matte coverage; never let the SDF turn crossed edges into an inverted box.
        if rect[2] <= rect[0] || rect[3] <= rect[1] {
            let mut out = input.clone();
            if matte {
                out.img.data.fill([0.0; 4]);
            }
            return Ok(out);
        }
        if !matte && fill[3] == 0.0 && (stroke_width == 0.0 || stroke[3] == 0.0) {
            return Ok(input.clone());
        }
        let margin = stroke_width * 0.5 + 0.5;
        let x0 = (rect[0] - margin).floor().min(0.0);
        let y0 = (rect[1] - margin).floor().min(0.0);
        let x1 = (rect[2] + margin).ceil().max(input.img.width as f64);
        let y1 = (rect[3] + margin).ceil().max(input.img.height as f64);
        let wf = x1 - x0;
        let hf = y1 - y0;
        if [x0, y0, x1, y1, wf, hf, radius, stroke_width].iter().any(|v| !v.is_finite())
            || wf < 1.0
            || hf < 1.0
            || wf > MAX_PIXELS as f64
            || hf > MAX_PIXELS as f64
        {
            return Err("TextBox geometry exceeds supported buffer size".into());
        }
        let (w, h) = (wf as usize, hf as usize);
        let n = w.checked_mul(h).filter(|n| *n <= MAX_PIXELS).ok_or("TextBox output exceeds 16M pixels (256 MiB)")?;
        let mut data = Vec::new();
        data.try_reserve_exact(n).map_err(|e| format!("TextBox allocation failed: {e}"))?;
        data.resize(n, [0.0; 4]);
        let iw = input.img.width as usize;
        let ih = input.img.height as usize;
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let sy = y as i64 + y0 as i64;
            for (x, out) in row.iter_mut().enumerate() {
                let sx = x as i64 + x0 as i64;
                let src = if sx >= 0 && sy >= 0 && (sx as usize) < iw && (sy as usize) < ih {
                    input.img.data.get((sy as usize).saturating_mul(iw).saturating_add(sx as usize)).copied().unwrap_or([0.0; 4])
                } else {
                    [0.0; 4]
                };
                let (cx, cy) = (sx as f64 + 0.5, sy as f64 + 0.5);
                // Avoid distance/coverage/compositing work outside the box's footprint.
                if cx < rect[0] - margin || cx > rect[2] + margin || cy < rect[1] - margin || cy > rect[3] + margin {
                    *out = if matte { [0.0; 4] } else { src };
                    continue;
                }
                let d = distance(cx, cy, rect, radius);
                // The matte is geometric coverage, independent of fill/stroke alpha.
                // Apply it to every premultiplied source channel to preserve edge colors.
                let coverage = (0.5 - d).clamp(0.0, 1.0);
                let src = if matte { src.map(|v| v * coverage as f32) } else { src };
                let bg = premult(fill, coverage);
                let line = if stroke_width > 0.0 { (0.5 + stroke_width * 0.5 - d.abs()).clamp(0.0, 1.0) } else { 0.0 };
                let bg = over(premult(stroke, line), bg);
                *out = over(src, bg);
            }
        });
        Ok(Buf { img: Image { width: w as u32, height: h as u32, data }, offset: [input.offset[0] - x0, input.offset[1] - y0], scale: s })
    }
}

#[cfg(test)]
mod tests;
