//! 3D Channel effects: 3D Channel Extract, Cryptomatte, Depth Matte, Depth of Field, EXtractoR,
//! Fog 3D, ID Matte and IDentifier.
//!
//! They read the layer's auxiliary channels ([`AuxChannels`], via
//! [`crate::EffectHost::aux`]): multi-layer OpenEXR footage (depth, object/material IDs,
//! Cryptomatte layers, any named channel) or the compositor's depth / layer-ID / Cryptomatte
//! pass for a precomp of a 3D comp. Without auxiliary channels they pass the layer through
//! (EXtractoR still maps the layer's own R, G, B and A).
//!
//! Aux pixels are looked up nearest-neighbour (IDs must not be interpolated) at each buffer
//! pixel's layer position, so render scale and layer padding are respected.

use std::sync::Arc;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, crypto_hash};
use effectcraft_raster::{AuxChannels, Image, Px, gaussian_blur};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, hash1, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "3D Channel", params, render, gpu: false, float: true }
}

fn aux(ctx: &EffectCtx) -> Option<Arc<AuxChannels>> {
    ctx.env.host?.aux()
}

/// Aux index for each buffer pixel (None outside the aux image).
fn index_map(b: &Buf, a: &AuxChannels) -> Vec<Option<usize>> {
    index_map_at(b, a, [0.5, 0.5])
}

/// [`index_map`] sampling each pixel at sub-pixel position `at` (0..1).
fn index_map_at(b: &Buf, a: &AuxChannels, at: [f64; 2]) -> Vec<Option<usize>> {
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let inv = 1.0 / b.scale.max(1e-9);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f64 + at[0], (i / w) as f64 + at[1]);
            a.index_at((x - b.offset[0]) * inv, (y - b.offset[1]) * inv)
        })
        .collect()
}

/// A channel sampled into the buffer grid (`fill` outside).
fn plane_of(b: &Buf, idx: &[Option<usize>], ch: &[f32], fill: f32) -> Plane {
    Plane { w: b.img.width as usize, h: b.img.height as usize, data: idx.par_iter().map(|i| i.map_or(fill, |i| ch[i])).collect() }
}

/// Distinct colour for an ID (hash → hue).
pub fn id_color(id: f32) -> [f32; 3] {
    let h = hash1(id.to_bits(), 0x1d, 0x5eed);
    let (r, g, b) = effectcraft_color::hsl_to_rgb(h, 0.75, 0.5);
    if id == 0.0 { [0.0; 3] } else { [r, g, b] }
}

// ---------------------------------------------------------------- 3D Channel Extract

pub const EXTRACT_CHANNELS: [&str; 8] = ["Z-Depth", "Object ID", "Texture UV", "Surface Normals", "Coverage", "Background RGB", "Unclamped RGB", "Material ID"];

fn channel_extract(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    if ctx.params.b("antialias") {
        // Anti-alias: average four sub-pixel samples of the channel.
        let subs = [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]];
        let imgs: Vec<Image> = subs.iter().map(|s| channel_extract_at(ctx, &b, &a, &index_map_at(&b, &a, *s))).collect::<Option<Vec<_>>>().unwrap_or_default();
        if imgs.len() == 4 {
            let mut out = imgs[0].clone();
            out.data.par_iter_mut().enumerate().for_each(|(i, p)| {
                *p = std::array::from_fn(|c| imgs.iter().map(|m| m.data[i][c]).sum::<f32>() * 0.25);
            });
            b.img = out;
        }
        return b;
    }
    let idx = index_map(&b, &a);
    if let Some(i) = channel_extract_at(ctx, &b, &a, &idx) {
        b.img = i;
    }
    b
}

/// 3D Channel Extract through one index map (None when the channel is missing).
fn channel_extract_at(ctx: &EffectCtx, b: &Buf, a: &AuxChannels, idx: &[Option<usize>]) -> Option<Image> {
    let (bp, wp) = (ctx.params.f("blackPoint") as f32, ctx.params.f("whitePoint") as f32);
    // Clamp Output limits the mapped values to 0..1; Invert Depth Map flips them.
    let (clamp, invert) = (ctx.params.b("clampOutput"), ctx.params.b("invertDepthMap"));
    let cl = move |v: f32| if clamp { v.clamp(0.0, 1.0) } else { v };
    let lv = |v: f32| cl((v - bp) / (wp - bp).abs().max(1e-9) * (wp - bp).signum());
    let lv = |v: f32| if wp >= bp { lv(v) } else { 1.0 - cl((v - wp) / (bp - wp).max(1e-9)) };
    let lv = |v: f32| if invert { 1.0 - lv(v) } else { lv(v) };
    let get3 = |names: [&[&str]; 3]| -> Option<[&[f32]; 3]> { Some([a.find(names[0])?, a.find(names[1])?, a.find(names[2])?]) };
    match ctx.params.e("channel") {
        0 => a.depth().map(|z| map_grey(b, idx, |i| lv(z[i]))),
        1 => a.object_id().map(|c| map_grey(b, idx, |i| lv(c[i]))),
        7 => a.material_id().map(|c| map_grey(b, idx, |i| lv(c[i]))),
        4 => a.coverage().map(|c| map_grey(b, idx, |i| lv(c[i]))),
        2 => get3([&["UV.U", "U", "uv.x"], &["UV.V", "V", "uv.y"], &["UV.W", "W", "uv.z"]])
            .or_else(|| Some([a.find(&["UV.U", "U", "uv.x"])?, a.find(&["UV.V", "V", "uv.y"])?, a.find(&["UV.V", "V", "uv.y"])?]))
            .map(|c| map_rgb(b, idx, |i| [lv(c[0][i]), lv(c[1][i]), 0.0])),
        3 => get3([&["N.X", "Normal.X", "normals.x"], &["N.Y", "Normal.Y", "normals.y"], &["N.Z", "Normal.Z", "normals.z"]])
            .map(|c| map_rgb(b, idx, |i| [lv(c[0][i] * 0.5 + 0.5), lv(c[1][i] * 0.5 + 0.5), lv(c[2][i] * 0.5 + 0.5)])),
        5 => get3([&["BG.R", "Background.R"], &["BG.G", "Background.G"], &["BG.B", "Background.B"]])
            .map(|c| map_rgb(b, idx, |i| [lv(c[0][i]), lv(c[1][i]), lv(c[2][i])])),
        _ => get3([&["R"], &["G"], &["B"]]).map(|c| map_rgb(b, idx, |i| [c[0][i], c[1][i], c[2][i]])),
    }
}

fn map_grey(b: &Buf, idx: &[Option<usize>], f: impl Fn(usize) -> f32 + Sync) -> Image {
    map_rgb(b, idx, |i| {
        let v = f(i);
        [v, v, v]
    })
}

fn map_rgb(b: &Buf, idx: &[Option<usize>], f: impl Fn(usize) -> [f32; 3] + Sync) -> Image {
    let mut img = Image::new(b.img.width, b.img.height);
    img.data.par_iter_mut().zip(idx.par_iter()).for_each(|(p, i)| {
        if let Some(i) = i {
            let c = f(*i);
            *p = [c[0], c[1], c[2], 1.0];
        }
    });
    img
}

// ---------------------------------------------------------------- Depth Matte

fn depth_matte(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(z) = a.depth() else { return b };
    let idx = index_map(&b, &a);
    let depth = ctx.params.f("depth") as f32;
    let feather = ctx.params.f("feather").max(0.0) as f32;
    let invert = ctx.params.b("invert");
    // Keeps what lies at or beyond Depth (Invert keeps what is in front), with a soft ramp of
    // `feather` depth units centred on the threshold.
    let zp = plane_of(&b, &idx, z, BACKGROUND_DEPTH);
    b.img.data.par_iter_mut().zip(zp.data.par_iter()).for_each(|(px, &zv)| {
        let k = if feather > 0.0 {
            ((zv - depth) / feather + 0.5).clamp(0.0, 1.0)
        } else if zv >= depth {
            1.0
        } else {
            0.0
        };
        let k = if invert { 1.0 - k } else { k };
        *px = px.map(|v| v * k);
    });
    b
}

// ---------------------------------------------------------------- Depth of Field

/// Blur radius (pixels) for depth `z`.
pub fn dof_radius(z: f32, focal: f32, thickness: f32, max_r: f32, bias: f32) -> f32 {
    let d = ((z - focal).abs() - thickness * 0.5).max(0.0);
    let range = focal.abs().max(1.0);
    let n = (d / range).min(1.0);
    // Bias 50 = linear; higher bias brings blur in sooner.
    let gamma = 2f32.powf((50.0 - bias) / 25.0);
    max_r * n.powf(gamma)
}

fn depth_of_field(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(z) = a.depth() else { return b };
    let max_r = (ctx.params.f("maximumRadius").max(0.0) * b.scale) as f32;
    if max_r <= 0.0 {
        return b;
    }
    let focal = ctx.params.f("focalPlane") as f32;
    let thick = ctx.params.f("focalPlaneThickness").max(0.0) as f32;
    let bias = ctx.params.f("focalBias") as f32;
    let idx = index_map(&b, &a);
    let zp = plane_of(&b, &idx, z, BACKGROUND_DEPTH);
    // A stack of blurs at a few radii, interpolated per pixel by its circle of confusion.
    const LEVELS: usize = 5;
    let stack: Vec<Image> = (0..LEVELS)
        .into_par_iter()
        .map(|k| {
            let r = max_r * k as f32 / (LEVELS - 1) as f32;
            if r < 0.3 { b.img.clone() } else { gaussian_blur(&b.img, (r / 2.0) as f64, (r / 2.0) as f64, true) }
        })
        .collect();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let r = dof_radius(zp.data[i], focal, thick, max_r, bias);
        let f = (r / max_r * (LEVELS - 1) as f32).clamp(0.0, (LEVELS - 1) as f32);
        let k0 = (f.floor() as usize).min(LEVELS - 2);
        let t = f - k0 as f32;
        let (a0, a1) = (stack[k0].data[i], stack[k0 + 1].data[i]);
        *px = [0, 1, 2, 3].map(|c| a0[c] + (a1[c] - a0[c]) * t);
    });
    b
}

// ---------------------------------------------------------------- Fog 3D

fn fog_3d(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(z) = a.depth() else { return b };
    let pr = ctx.params;
    let fc = pr.color("fogColor");
    let (start, end) = (pr.f("fogStartDepth") as f32, pr.f("fogEndDepth") as f32);
    let opacity = (pr.f("fogOpacity") / 100.0) as f32;
    let density = (pr.f("scatteringDensity") / 100.0) as f32;
    let foggy_bg = pr.b("foggyBackground");
    let contrib = (pr.f("layerContribution") / 100.0) as f32;
    let grad = if contrib > 0.0 { ctx.layer_param("gradientLayer", true).map(|o| crate::util::fit_layer(ctx, &b, &o, true)) } else { None };
    let idx = index_map(&b, &a);
    let zp = plane_of(&b, &idx, z, BACKGROUND_DEPTH);
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let zv = zp.data[i];
        let bg = zv >= BACKGROUND_DEPTH * 0.5;
        let mut f = if bg {
            if foggy_bg { 1.0 } else { 0.0 }
        } else {
            let t = ((zv - start) / (end - start).abs().max(1e-6)).clamp(0.0, 1.0);
            // Denser scattering thickens the fog sooner.
            1.0 - (1.0 - t).powf(1.0 + density * 3.0)
        };
        if let Some(g) = &grad {
            let (gc, ga) = unpremul(g.data[i]);
            let l = effectcraft_color::luminance(gc[0], gc[1], gc[2]) * ga;
            f *= 1.0 - contrib + contrib * l;
        }
        let f = (f * opacity).clamp(0.0, 1.0);
        let (c, al) = unpremul(*px);
        let al2 = if bg && foggy_bg { al + (1.0 - al) * f } else { al };
        let out = [0, 1, 2].map(|k| c[k] + (fc[k] - c[k]) * f);
        *px = premul(out, al2);
    });
    b
}

// ---------------------------------------------------------------- ID Matte / IDentifier

fn id_channel(a: &AuxChannels, which: u32) -> Option<&[f32]> {
    if which == 1 { a.material_id() } else { a.object_id() }
}

fn id_matte(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(ids) = id_channel(&a, ctx.params.e("auxChannel")) else { return b };
    let idx = index_map(&b, &a);
    let sel = ctx.params.f("idSelection").round() as f32;
    let cov = if ctx.params.b("useCoverage") { a.coverage() } else { None };
    let mut m = Plane {
        w: b.img.width as usize,
        h: b.img.height as usize,
        data: idx.par_iter().map(|i| i.map_or(0.0, |i| if ids[i].round() == sel { cov.map_or(1.0, |c| c[i].clamp(0.0, 1.0)) } else { 0.0 })).collect(),
    };
    let feather = ctx.params.f("feather").max(0.0) * b.scale;
    if feather > 0.0 {
        m = gauss_plane(&m, feather * 0.5, feather * 0.5);
    }
    let invert = ctx.params.b("invert");
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(px, &k)| {
        let k = if invert { 1.0 - k } else { k };
        *px = px.map(|v| v * k);
    });
    b
}

fn identifier(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(ids) = id_channel(&a, ctx.params.e("channelType")) else { return b };
    let idx = index_map(&b, &a);
    let sel = ctx.params.f("id").round() as f32;
    let display = ctx.params.e("display");
    let max_id = ids.iter().cloned().fold(1.0f32, f32::max);
    let src = b.img.clone();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let Some(k) = idx[i] else {
            if display != 2 {
                *px = [0.0; 4];
            }
            return;
        };
        let id = ids[k].round();
        let hit = id == sel;
        *px = match display {
            // Colors: every ID in its own colour, the selected one highlighted white.
            0 => {
                let c = if hit && sel != 0.0 { [1.0; 3] } else { id_color(id) };
                [c[0], c[1], c[2], 1.0]
            }
            1 => {
                let v = if hit { 1.0 } else { 0.0 };
                [v, v, v, 1.0]
            }
            2 => {
                let s = src.data[i];
                if hit { s } else { [0.0; 4] }
            }
            _ => {
                let v = id / max_id;
                [v, v, v, 1.0]
            }
        };
    });
    b
}

// ---------------------------------------------------------------- Cryptomatte

/// Does `name` match a selection pattern (`*` wildcards, case-sensitive like the spec)?
fn wild(pat: &str, name: &str) -> bool {
    match pat.split_once('*') {
        None => pat == name,
        Some((pre, rest)) => {
            let Some(tail) = name.strip_prefix(pre) else { return false };
            if rest.is_empty() {
                return true;
            }
            (0..=tail.len()).filter(|&k| tail.is_char_boundary(k)).any(|k| wild(rest, &tail[k..]))
        }
    }
}

/// Split a selection at its commas. A name in double or single quotes stays whole, commas
/// included (Blender allows `Green, Glossy`), and a backslash takes the next character
/// literally (`Green\, Glossy`, `\"`). Unquoted text is trimmed; the quotes are dropped.
fn selection_names(sel: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = sel.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', _) => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            (c, Some(q)) if c == q => quote = None,
            ('"' | '\'', None) if cur.trim().is_empty() => {
                cur.clear();
                quote = Some(c);
            }
            (',', None) => out.push(std::mem::take(&mut cur).trim().to_string()),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// Parse a selection: comma-separated names (optionally quoted, see [`selection_names`]) or
/// `<hex>` hashes / wildcards.
pub fn selection_hashes(sel: &str, manifest: Option<&[(String, u32)]>) -> Vec<u32> {
    let mut out = vec![];
    for t in selection_names(sel) {
        let t = t.as_str();
        if t.is_empty() {
            continue;
        }
        if t.contains('*') {
            if let Some(m) = manifest {
                out.extend(m.iter().filter(|(n, _)| wild(t, n)).map(|(_, h)| *h));
            }
            continue;
        }
        if let Some(hex) = t.strip_prefix("<").and_then(|r| r.strip_suffix(">"))
            && let Ok(h) = u32::from_str_radix(hex, 16)
        {
            out.push(h);
            continue;
        }
        out.push(manifest.and_then(|m| m.iter().find(|(n, _)| n == t).map(|(_, h)| *h)).unwrap_or_else(|| crypto_hash(t)));
    }
    out
}

pub const CRYPTO_LAYERS: [&str; 3] = ["CryptoObject", "CryptoMaterial", "CryptoAsset"];

/// Rank planes as [`AuxChannels::crypto_rank_planes`] returns them.
type RankPlanes<'a> = [(&'a [f32], &'a [f32])];

/// The selection's coverage at aux pixel `i`: the summed coverage of the selected IDs over all
/// ranks (the Cryptomatte spec's matte), clamped to 0..1.
#[inline]
fn crypto_matte(planes: &RankPlanes, sel: &[u32], i: usize) -> f32 {
    planes
        .iter()
        .filter_map(|(id, c)| Some((*id.get(i)?, *c.get(i)?)))
        .filter(|(id, c)| *c > 0.0 && sel.contains(&id.to_bits()))
        .map(|(_, c)| c)
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

/// The coverage-weighted ID colours at aux pixel `i` (the spec's preview).
#[inline]
fn crypto_colors(planes: &RankPlanes, i: usize) -> [f32; 3] {
    let mut c = [0.0f32; 3];
    for (id, cov) in planes {
        let (Some(&id), Some(&cov)) = (id.get(i), cov.get(i)) else { continue };
        let k = id_color(id);
        for j in 0..3 {
            c[j] += k[j] * cov;
        }
    }
    c
}

/// Per aux pixel of Cryptomatte layer `layer`: the coverage-weighted ID colours (RGB) and the
/// selection's coverage (A), computed in parallel with the rank planes resolved once. The GPU
/// effect uploads this image and samples it per layer pixel.
pub fn crypto_aux_image(a: &AuxChannels, layer: &str, sel: &[u32]) -> Image {
    let planes = a.crypto_rank_planes(layer);
    let mut img = Image::new(a.width, a.height);
    img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let c = crypto_colors(&planes, i);
        *px = [c[0], c[1], c[2], crypto_matte(&planes, sel, i)];
    });
    img
}

fn cryptomatte(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let layers = a.crypto_layers();
    if layers.is_empty() {
        return b;
    }
    let want = CRYPTO_LAYERS[(ctx.params.e("layer") as usize).min(2)];
    let layer = layers.iter().find(|l| l.as_str() == want || l.ends_with(want)).unwrap_or(&layers[0]).clone();
    let manifest = a.manifests.iter().find(|(l, _)| *l == layer).map(|(_, m)| m.as_slice());
    let sel = selection_hashes(ctx.params.s("selection"), manifest);
    let idx = index_map(&b, &a);
    let display = ctx.params.e("display");
    let matte_only = ctx.params.b("matteOnly");
    let planes = a.crypto_rank_planes(&layer);
    let src = b.img.clone();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let m = idx[i].map_or(0.0, |k| crypto_matte(&planes, &sel, k));
        if matte_only {
            *px = [m, m, m, m];
            return;
        }
        *px = match display {
            // Colors: coverage-weighted ID colours (the spec's preview), selection brighter.
            0 => {
                let c = idx[i].map_or([0.0; 3], |k| crypto_colors(&planes, k));
                let c = c.map(|v| v * (1.0 - 0.5 * m) + 0.5 * m);
                [c[0], c[1], c[2], 1.0]
            }
            // Matte: the layer keyed by the selection.
            1 => src.data[i].map(|v| v * m),
            // Preview: the layer with the selection tinted.
            _ => {
                let s = src.data[i];
                [s[0] * (1.0 - 0.5 * m) + 0.5 * m * s[3], s[1] * (1.0 - 0.5 * m) + 0.5 * m * s[3], s[2] * (1.0 - 0.5 * m), s[3]]
            }
        };
    });
    b
}

// ---------------------------------------------------------------- EXtractoR

fn extractor(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let a = aux(ctx);
    let names = [ctx.params.s("red"), ctx.params.s("green"), ctx.params.s("blue"), ctx.params.s("alpha")];
    let (bp, wp) = (ctx.params.f("blackPoint") as f32, ctx.params.f("whitePoint") as f32);
    let unmult = ctx.params.b("unMult");
    let clip = ctx.params.b("clip");
    let idx = a.as_ref().map(|a| index_map(&b, a));
    // Each output channel: an aux channel by name, or the layer's own R/G/B/A.
    let own = |n: &str| match n.trim().to_ascii_uppercase().as_str() {
        "R" | "RED" => Some(0),
        "G" | "GREEN" => Some(1),
        "B" | "BLUE" => Some(2),
        "A" | "ALPHA" => Some(3),
        "" => None,
        _ => None,
    };
    let src_ch: Vec<Option<&[f32]>> = names.iter().map(|n| a.as_ref().and_then(|a| if n.trim().is_empty() { None } else { a.get(n.trim()) })).collect();
    if src_ch.iter().all(Option::is_none) && bp == 0.0 && wp == 1.0 && !unmult && names.iter().enumerate().all(|(k, n)| own(n) == Some(k)) {
        return b;
    }
    let src = b.img.clone();
    let lv = |v: f32| {
        let o = (v - bp) / (wp - bp).abs().max(1e-9) * (wp - bp).signum();
        if clip { o.clamp(0.0, 1.0) } else { o }
    };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (sc, sa) = unpremul(src.data[i]);
        let straight = [sc[0], sc[1], sc[2], sa];
        let mut o: Px = [0.0; 4];
        for k in 0..4 {
            o[k] = match (src_ch[k], idx.as_ref().and_then(|m| m[i])) {
                (Some(ch), Some(j)) => ch[j],
                (Some(_), None) => 0.0,
                (None, _) => match own(names[k]) {
                    Some(c) => straight[c],
                    None => {
                        if k == 3 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                },
            };
            if k < 3 {
                o[k] = lv(o[k]);
            }
        }
        let al = o[3].clamp(0.0, 1.0);
        let c = if unmult && al > 1e-6 { [o[0] / al, o[1] / al, o[2] / al] } else { [o[0], o[1], o[2]] };
        *px = premul(c, al);
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let text = |id: &'static str, name: &'static str, d: &str| p(id, name, Value::Str(d.into()), ParamUi::Text);
    vec![
        spec(
            "ec.3d.channelextract",
            "3D Channel Extract",
            vec![
                p("channel", "3D Channel", Value::Enum(0), popup(&EXTRACT_CHANNELS)),
                p("blackPoint", "Black Point", num(0.0), slider(-1.0e6, 1.0e6, -10000.0, 10000.0, 2)),
                p("whitePoint", "White Point", num(10000.0), slider(-1.0e6, 1.0e6, -10000.0, 10000.0, 2)),
                p("clampOutput", "Clamp Output", Value::Bool(true), ParamUi::Checkbox),
                p("invertDepthMap", "Invert Depth Map", Value::Bool(false), ParamUi::Checkbox),
                p("antialias", "Anti-alias", Value::Bool(false), ParamUi::Checkbox),
            ],
            channel_extract,
        ),
        spec(
            "ec.3d.cryptomatte",
            "Cryptomatte",
            vec![
                p("layer", "Layer", Value::Enum(0), popup(&CRYPTO_LAYERS)),
                text("selection", "Selection", ""),
                p("display", "Display", Value::Enum(2), popup(&["Colors", "Matte", "Preview"])),
                p("matteOnly", "Matte Only", Value::Bool(false), ParamUi::Checkbox),
            ],
            cryptomatte,
        ),
        spec(
            "ec.3d.depthmatte",
            "Depth Matte",
            vec![
                p("depth", "Depth", num(0.0), slider(-1.0e6, 1.0e6, -10000.0, 10000.0, 2)),
                p("feather", "Feather", num(0.0), slider(0.0, 1.0e6, 0.0, 1000.0, 2)),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
            ],
            depth_matte,
        ),
        spec(
            "ec.3d.depthoffield",
            "Depth of Field",
            vec![
                p("focalPlane", "Focal Plane", num(0.0), slider(-1.0e6, 1.0e6, 0.0, 10000.0, 2)),
                p("maximumRadius", "Maximum Radius", num(0.0), slider(0.0, 500.0, 0.0, 50.0, 2)),
                p("focalPlaneThickness", "Focal Plane Thickness", num(0.0), slider(0.0, 1.0e6, 0.0, 1000.0, 2)),
                p("focalBias", "Focal Bias", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 2)),
            ],
            depth_of_field,
        ),
        spec(
            "ec.3d.extractor",
            "EXtractoR",
            vec![
                text("red", "Red", "R"),
                text("green", "Green", "G"),
                text("blue", "Blue", "B"),
                text("alpha", "Alpha", "A"),
                p("blackPoint", "Black Point", num(0.0), slider(-1.0e6, 1.0e6, -1.0, 2.0, 3)),
                p("whitePoint", "White Point", num(1.0), slider(-1.0e6, 1.0e6, -1.0, 2.0, 3)),
                p("unMult", "UnMult", Value::Bool(false), ParamUi::Checkbox),
                p("clip", "Clip", Value::Bool(false), ParamUi::Checkbox),
            ],
            extractor,
        ),
        spec(
            "ec.3d.fog3d",
            "Fog 3D",
            vec![
                p("fogColor", "Fog Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("fogStartDepth", "Fog Start Depth", num(0.0), slider(-1.0e6, 1.0e6, 0.0, 10000.0, 2)),
                p("fogEndDepth", "Fog End Depth", num(3000.0), slider(-1.0e6, 1.0e6, 0.0, 10000.0, 2)),
                p("fogOpacity", "Fog Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("scatteringDensity", "Scattering Density", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("foggyBackground", "Foggy Background", Value::Bool(true), ParamUi::Checkbox),
                p("gradientLayer", "Gradient Layer", Value::Layer(None), ParamUi::Layer),
                p("layerContribution", "Layer Contribution", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            fog_3d,
        ),
        spec(
            "ec.3d.idmatte",
            "ID Matte",
            vec![
                p("auxChannel", "Aux. Channel", Value::Enum(0), popup(&["Object ID", "Material ID"])),
                p("idSelection", "ID Selection", num(0.0), slider(0.0, 65535.0, 0.0, 100.0, 0)),
                p("feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
                p("useCoverage", "Use Coverage", Value::Bool(false), ParamUi::Checkbox),
            ],
            id_matte,
        ),
        spec(
            "ec.3d.identifier",
            "IDentifier",
            vec![
                p("channelType", "Channel Type", Value::Enum(0), popup(&["Object ID", "Material ID"])),
                p("display", "Display", Value::Enum(0), popup(&["Colors", "Luma Matte", "Alpha Matte", "Raw"])),
                p("id", "ID", num(0.0), slider(0.0, 65535.0, 0.0, 100.0, 0)),
            ],
            identifier,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};
    use effectcraft_raster::channels3d::crypto_float;

    /// A 20×10 layer: left half object 1 at depth 100, right half object 2 at depth 500; the
    /// top-right corner is background. Cryptomatte: "left" / "right".
    struct AuxHost(Arc<AuxChannels>);
    impl EffectHost for AuxHost {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn aux(&self) -> Option<Arc<AuxChannels>> {
            Some(self.0.clone())
        }
    }

    fn scene() -> AuxHost {
        let (w, h) = (20usize, 10usize);
        let mut a = AuxChannels::new(w as u32, h as u32, 1.0);
        let (mut z, mut id, mut r, mut g) = (vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h]);
        let (hl, hr) = (crypto_hash("left"), crypto_hash("right"));
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if x >= 15 && y < 3 {
                    z[i] = BACKGROUND_DEPTH;
                } else if x < 10 {
                    z[i] = 100.0;
                    id[i] = 1.0;
                    r[i] = crypto_float(hl);
                    g[i] = 1.0;
                } else {
                    z[i] = 500.0;
                    id[i] = 2.0;
                    r[i] = crypto_float(hr);
                    g[i] = 1.0;
                }
            }
        }
        a.channels = vec![("Z".into(), z), ("ObjectID".into(), id), ("CryptoObject00.R".into(), r), ("CryptoObject00.G".into(), g)];
        a.manifests = vec![("CryptoObject".into(), vec![("left".into(), hl), ("right".into(), hr)])];
        AuxHost(Arc::new(a))
    }

    fn img() -> Image {
        Image::filled(20, 10, [0.5, 0.5, 0.5, 1.0])
    }

    fn run(id: &str, vals: &[(&str, Value)], host: &AuxHost) -> Image {
        run_fx(id, vals, img(), 0.0, EffectEnv { host: Some(host), ..Default::default() }).img
    }

    #[test]
    fn passes_through_without_aux_channels() {
        for s in specs() {
            let out = run_fx(s.id, &[], img(), 0.0, EffectEnv::default());
            assert_eq!(out.img.data, img().data, "{}", s.id);
        }
    }

    #[test]
    fn depth_matte_thresholds() {
        let h = scene();
        let o = run("ec.3d.depthmatte", &[("depth", num(300.0))], &h);
        assert_eq!(o.get(2, 5)[3], 0.0);
        assert_eq!(o.get(12, 5)[3], 1.0);
        let inv = run("ec.3d.depthmatte", &[("depth", num(300.0)), ("invert", Value::Bool(true))], &h);
        assert_eq!(inv.get(2, 5)[3], 1.0);
        assert_eq!(inv.get(12, 5)[3], 0.0);
        // Feather: a ramp of 800 units centred on 300 → depth 100 at 0.25, 500 at 0.75.
        let f = run("ec.3d.depthmatte", &[("depth", num(300.0)), ("feather", num(800.0))], &h);
        assert!((f.get(2, 5)[3] - 0.25).abs() < 1e-5 && (f.get(12, 5)[3] - 0.75).abs() < 1e-5);
    }

    #[test]
    fn channel_extract_maps_depth_and_ids() {
        let h = scene();
        let o = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(1000.0))], &h);
        assert!((o.get(2, 5)[0] - 0.1).abs() < 1e-6 && (o.get(12, 5)[0] - 0.5).abs() < 1e-6);
        assert_eq!(o.get(17, 1)[0], 1.0);
        let ids = run("ec.3d.channelextract", &[("channel", Value::Enum(1)), ("whitePoint", num(2.0))], &h);
        assert_eq!(ids.get(2, 5)[0], 0.5);
        // Invert Depth Map flips the ramp; without Clamp Output values run past 1.
        let inv = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(1000.0)), ("invertDepthMap", Value::Bool(true))], &h);
        assert!((inv.get(2, 5)[0] - 0.9).abs() < 1e-6);
        let raw = run("ec.3d.channelextract", &[("blackPoint", num(0.0)), ("whitePoint", num(100.0)), ("clampOutput", Value::Bool(false))], &h);
        assert!(raw.get(12, 5)[0] > 1.5);
    }

    #[test]
    fn id_matte_and_identifier() {
        let h = scene();
        let o = run("ec.3d.idmatte", &[("idSelection", num(2.0))], &h);
        assert_eq!(o.get(2, 5)[3], 0.0);
        assert_eq!(o.get(12, 5)[3], 1.0);
        let l = run("ec.3d.identifier", &[("display", Value::Enum(1)), ("id", num(1.0))], &h);
        assert_eq!(l.get(2, 5)[0], 1.0);
        assert_eq!(l.get(12, 5)[0], 0.0);
        let c = run("ec.3d.identifier", &[], &h);
        assert_ne!(c.get(2, 5), c.get(12, 5));
    }

    /// The parallel aux image (rank planes resolved once) is bit-identical to the per-pixel
    /// reduction over `crypto_ranks` it replaces.
    #[test]
    fn crypto_aux_image_matches_the_per_pixel_reduction() {
        let (w, h) = (7u32, 5u32);
        let n = (w * h) as usize;
        let mut a = AuxChannels::new(w, h, 1.0);
        let ids = [crypto_hash("cube"), crypto_hash("sphere"), crypto_hash("ring")].map(effectcraft_raster::channels3d::crypto_float);
        for (k, ch) in ["00.R", "00.G", "00.B", "00.A", "01.R", "01.G", "01.B", "01.A"].iter().enumerate() {
            let v: Vec<f32> = (0..n).map(|i| if k % 2 == 0 { ids[(i + k) % 3] } else { ((i * 7 + k * 3) % 11) as f32 / 10.0 }).collect();
            a.channels.push((format!("CryptoObject{ch}"), v));
        }
        let sel = vec![crypto_hash("sphere"), crypto_hash("ring")];
        let img = crypto_aux_image(&a, "CryptoObject", &sel);
        for i in 0..n {
            let ranks = a.crypto_ranks("CryptoObject", i);
            let m: f32 = ranks.iter().filter(|(id, c)| *c > 0.0 && sel.contains(&id.to_bits())).map(|(_, c)| *c).sum::<f32>().clamp(0.0, 1.0);
            let mut c = [0.0f32; 3];
            for (id, cov) in &ranks {
                let k = id_color(*id);
                for j in 0..3 {
                    c[j] += k[j] * cov;
                }
            }
            let want = [c[0], c[1], c[2], m];
            assert_eq!(img.data[i].map(f32::to_bits), want.map(f32::to_bits), "pixel {i}");
        }
    }

    /// The lookup before #473, kept as the reference: the rank channels formatted and searched
    /// by name for every pixel.
    fn ranks_by_name(a: &AuxChannels, layer: &str, i: usize) -> Vec<(f32, f32)> {
        let mut out = vec![];
        for k in 0.. {
            let get = |c: &str| a.get(&format!("{layer}{k:02}.{c}"));
            let (Some(r), Some(g)) = (get("R"), get("G")) else { break };
            out.push((r[i], g[i]));
            if let (Some(b), Some(al)) = (get("B"), get("A")) {
                out.push((b[i], al[i]));
            }
        }
        out
    }

    /// The reduction before #473 over those ranks: (ID colours, selection coverage).
    fn reduce_by_name(a: &AuxChannels, layer: &str, sel: &[u32], i: usize) -> ([f32; 3], f32) {
        let ranks = ranks_by_name(a, layer, i);
        let m: f32 = ranks.iter().filter(|(id, c)| *c > 0.0 && sel.contains(&id.to_bits())).map(|(_, c)| *c).sum::<f32>().clamp(0.0, 1.0);
        let mut c = [0.0f32; 3];
        for (id, cov) in &ranks {
            let k = id_color(*id);
            for j in 0..3 {
                c[j] += k[j] * cov;
            }
        }
        (c, m)
    }

    /// A 6×2 Cryptomatte layer with awkward values: NaN, ±inf, negative and -0.0 coverage, NaN
    /// and -0.0 IDs; rank 01 has no B/A pair, rank 02 is missing, so rank 03 (filled with values
    /// that would change every pixel) must be ignored as before.
    fn edge_aux(with_rank_after_gap: bool) -> AuxChannels {
        let (w, h) = (6u32, 2u32);
        let nan_payload = f32::from_bits(0x7fc0_1234);
        let (hs, hc) = (crypto_float(crypto_hash("sphere")), crypto_float(crypto_hash("cube")));
        let cov = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, -0.0, 0.0, 0.25, 1.0, 2.0, f32::MIN_POSITIVE, nan_payload, 0.5];
        let ids = [hs, f32::NAN, hc, -0.0, 0.0, nan_payload, hs, hc, f32::NAN, hs, hc, -0.0];
        let rot = |v: &[f32; 12], k: usize| -> Vec<f32> { (0..12).map(|i| v[(i + k) % 12]).collect() };
        let mut a = AuxChannels::new(w, h, 1.0);
        let mut push = |n: &str, v: Vec<f32>| a.channels.push((format!("CryptoObject{n}"), v));
        push("00.R", rot(&ids, 0));
        push("00.G", rot(&cov, 0));
        push("00.B", rot(&ids, 3));
        push("00.A", rot(&cov, 5));
        push("01.R", rot(&ids, 7));
        push("01.G", rot(&cov, 2));
        if with_rank_after_gap {
            push("03.R", vec![hs; 12]);
            push("03.G", vec![1.0; 12]);
            push("03.B", vec![hc; 12]);
            push("03.A", vec![1.0; 12]);
        }
        a
    }

    fn edge_selection() -> Vec<u32> {
        vec![crypto_hash("sphere"), f32::NAN.to_bits(), 0x7fc0_1234, (-0.0f32).to_bits()]
    }

    /// Edge values give bit-identical results to the lookup by name, the missing B/A pair is
    /// skipped and a rank after a gap is ignored.
    #[test]
    fn crypto_aux_image_is_bit_exact_on_edge_values() {
        let sel = edge_selection();
        let a = edge_aux(true);
        assert_eq!(a.crypto_rank_planes("CryptoObject").len(), 3, "00.R/G, 00.B/A, 01.R/G; not 03 after the gap");
        let img = crypto_aux_image(&a, "CryptoObject", &sel);
        let without_gap_rank = crypto_aux_image(&edge_aux(false), "CryptoObject", &sel);
        for i in 0..12 {
            let (c, m) = reduce_by_name(&a, "CryptoObject", &sel, i);
            let want = [c[0], c[1], c[2], m];
            assert_eq!(img.data[i].map(f32::to_bits), want.map(f32::to_bits), "pixel {i}");
            assert_eq!(img.data[i].map(f32::to_bits), without_gap_rank.data[i].map(f32::to_bits), "pixel {i}: the rank after the gap was used");
        }
    }

    /// The CPU effect on a 20×10 layer over the 6×2 edge aux: pixels inside the aux area match
    /// the lookup by name bit for bit in every display mode, pixels outside it (no aux index)
    /// get an empty matte and no ID colour.
    #[test]
    fn cryptomatte_cpu_path_is_bit_exact_inside_and_outside_the_aux_area() {
        let a = edge_aux(true);
        let host = AuxHost(Arc::new(a.clone()));
        let names = "sphere, <7fc00000>, <7fc01234>, <80000000>";
        assert_eq!(selection_hashes(names, None), edge_selection());
        let src = img().data[0];
        for (display, matte_only) in [(0, false), (1, false), (2, false), (0, true)] {
            let out = run(
                "ec.3d.cryptomatte",
                &[("selection", Value::Str(names.into())), ("display", Value::Enum(display)), ("matteOnly", Value::Bool(matte_only))],
                &host,
            );
            let expect = |c: [f32; 3], m: f32| -> [f32; 4] {
                if matte_only {
                    return [m; 4];
                }
                match display {
                    0 => {
                        let c = c.map(|v| v * (1.0 - 0.5 * m) + 0.5 * m);
                        [c[0], c[1], c[2], 1.0]
                    }
                    1 => src.map(|v| v * m),
                    _ => [src[0] * (1.0 - 0.5 * m) + 0.5 * m * src[3], src[1] * (1.0 - 0.5 * m) + 0.5 * m * src[3], src[2] * (1.0 - 0.5 * m), src[3]],
                }
            };
            for (x, y) in [(0u32, 0u32), (1, 0), (5, 0), (0, 1), (3, 1), (5, 1)] {
                let (c, m) = reduce_by_name(&a, "CryptoObject", &edge_selection(), (y * 6 + x) as usize);
                assert_eq!(
                    out.get(x as i64, y as i64).map(f32::to_bits),
                    expect(c, m).map(f32::to_bits),
                    "display {display} matteOnly {matte_only} at ({x}, {y})"
                );
            }
            for (x, y) in [(6u32, 0u32), (19, 0), (0, 2), (12, 9)] {
                assert_eq!(
                    out.get(x as i64, y as i64).map(f32::to_bits),
                    expect([0.0; 3], 0.0).map(f32::to_bits),
                    "display {display} matteOnly {matte_only} outside the aux at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn cryptomatte_selects_by_name_and_wildcard() {
        let h = scene();
        let m = run("ec.3d.cryptomatte", &[("selection", Value::Str("right".into())), ("matteOnly", Value::Bool(true))], &h);
        assert_eq!(m.get(2, 5)[3], 0.0);
        assert_eq!(m.get(12, 5)[3], 1.0);
        let w = run("ec.3d.cryptomatte", &[("selection", Value::Str("l*".into())), ("display", Value::Enum(1))], &h);
        assert_eq!(w.get(2, 5)[3], 1.0);
        assert_eq!(w.get(12, 5)[3], 0.0);
        assert!(wild("*ight", "right") && wild("r*t", "right") && !wild("x*", "right"));
        let hex = format!("<{:08x}>", crypto_hash("left"));
        assert_eq!(selection_hashes(&hex, None), vec![crypto_hash("left")]);
    }

    /// #473: a name with a comma in it (Blender allows `Green, Glossy`) is selected when it is
    /// quoted or its comma escaped, instead of being cut into two names that match nothing.
    #[test]
    fn cryptomatte_selection_keeps_quoted_names_with_commas_whole() {
        let manifest = vec![("Green, Glossy".to_string(), 7), ("Green".to_string(), 1), ("Glossy".to_string(), 2), ("ring".to_string(), 3)];
        let m = Some(manifest.as_slice());
        assert_eq!(selection_hashes(r#""Green, Glossy""#, m), vec![7]);
        assert_eq!(selection_hashes("'Green, Glossy', ring", m), vec![7, 3]);
        assert_eq!(selection_hashes(r"Green\, Glossy", m), vec![7]);
        assert_eq!(selection_hashes(r#"ring , "Green, Glossy""#, m), vec![3, 7]);
        // Without a manifest the whole name is hashed.
        assert_eq!(selection_hashes(r#""Green, Glossy""#, None), vec![crypto_hash("Green, Glossy")]);
        // Plain lists, apostrophes inside names and an unclosed quote still parse.
        assert_eq!(selection_hashes("Green, Glossy", m), vec![1, 2]);
        assert_eq!(selection_names("Bob's mat, ring"), vec!["Bob's mat", "ring"]);
        assert_eq!(selection_names(r#""Green, Glossy"#), vec!["Green, Glossy"]);
        assert_eq!(selection_names(r#"say \"hi\""#), vec![r#"say "hi""#]);
    }

    #[test]
    fn fog_and_depth_of_field_follow_depth() {
        let h = scene();
        let f = run("ec.3d.fog3d", &[("fogColor", col(1.0, 0.0, 0.0)), ("fogEndDepth", num(1000.0)), ("scatteringDensity", num(0.0))], &h);
        // Linear fog: 10 % at depth 100, 50 % at 500, full on the background.
        assert!((f.get(2, 5)[0] - 0.55).abs() < 1e-4, "{:?}", f.get(2, 5));
        assert!((f.get(12, 5)[0] - 0.75).abs() < 1e-4);
        assert!((f.get(17, 1)[0] - 1.0).abs() < 1e-4 && f.get(17, 1)[1] < 1e-4);
        assert!((dof_radius(100.0, 100.0, 0.0, 10.0, 50.0)).abs() < 1e-6);
        assert!((dof_radius(200.0, 100.0, 0.0, 10.0, 50.0) - 10.0).abs() < 1e-6);
        assert_eq!(dof_radius(140.0, 100.0, 100.0, 10.0, 50.0), 0.0);
        // A sharp edge at the far object blurs when focused near.
        let mut pic = Image::new(20, 10);
        for y in 0..10 {
            for x in 0..20 {
                let v = if x % 2 == 0 { 1.0 } else { 0.0 };
                pic.set(x, y, [v, v, v, 1.0]);
            }
        }
        let env = EffectEnv { host: Some(&h), ..Default::default() };
        let d = run_fx("ec.3d.depthoffield", &[("focalPlane", num(100.0)), ("maximumRadius", num(4.0))], pic.clone(), 0.0, env).img;
        assert_eq!(d.get(4, 5), pic.get(4, 5));
        assert!((d.get(12, 5)[0] - 0.5).abs() < 0.3 && d.get(12, 5) != pic.get(12, 5));
    }

    #[test]
    fn extractor_maps_named_channels() {
        let h = scene();
        let o = run("ec.3d.extractor", &[("red", Value::Str("Z".into())), ("whitePoint", num(1000.0))], &h);
        assert!((o.get(2, 5)[0] - 0.1).abs() < 1e-6);
        assert!((o.get(2, 5)[1] - 0.0005).abs() < 1e-6);
        // Without aux: the layer's own channels can be swapped.
        let s = run_fx("ec.3d.extractor", &[("red", Value::Str("A".into()))], Image::filled(2, 2, [0.2, 0.3, 0.4, 1.0]), 0.0, EffectEnv::default());
        assert_eq!(s.img.get(0, 0), [1.0, 0.3, 0.4, 1.0]);
    }

    #[test]
    fn channel_extract_antialias_averages_sub_pixels() {
        // Depth at twice the layer resolution, alternating 0 / 1000 per aux column.
        let mut a = AuxChannels::new(40, 20, 2.0);
        a.channels = vec![("Z".into(), (0..800).map(|i| if i % 2 == 0 { 0.0 } else { 1000.0 }).collect())];
        let host = AuxHost(Arc::new(a));
        let vals = [("blackPoint", num(0.0)), ("whitePoint", num(1000.0))];
        let hard = run("ec.3d.channelextract", &vals, &host);
        let v = hard.get(5, 5)[0];
        assert!(v == 0.0 || v == 1.0, "{v}");
        let soft = run("ec.3d.channelextract", &[vals.as_slice(), &[("antialias", Value::Bool(true))]].concat(), &host);
        assert!((soft.get(5, 5)[0] - 0.5).abs() < 1e-5, "{:?}", soft.get(5, 5));
    }
}
