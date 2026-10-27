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
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let [x, y] = aux_point(b.scale, b.offset, [(i % w) as f64 + at[0], (i / w) as f64 + at[1]]);
            a.index_at(x, y)
        })
        .collect()
}

/// The layer point where 3D Channel effects look up the aux for buffer position `p` (pixel
/// `i` + sub-pixel offset) of a buffer with `scale` and `offset`. The Cryptomatte pick calls
/// it too, so the two can't pick different aux pixels.
#[inline]
pub fn aux_point(scale: f64, offset: [f64; 2], p: [f64; 2]) -> [f64; 2] {
    let inv = 1.0 / scale.max(1e-9);
    [(p[0] - offset[0]) * inv, (p[1] - offset[1]) * inv]
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
pub(crate) fn selection_names(sel: &str) -> Vec<String> {
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

/// Parse a selection: comma-separated names (optionally quoted, see `selection_names`) or
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

/// [`id_color`] of the last ID seen on each rank plane, per worker. Neighbouring pixels mostly
/// carry the same IDs, so this skips the hash and the HSL conversion for most of them. Keyed by the
/// ID's bits, so it returns exactly what [`id_color`] would.
///
/// Keep the per-pixel loop below as it is: with NaN coverage, which NaN payload ends up in a
/// channel depends on the order the compiler adds the ranks in, and a row-wise loop let it reorder
/// them (`crypto_aux_image_is_bit_exact_on_edge_values` catches that).
#[derive(Default)]
struct IdColorMemo(Vec<(u32, [f32; 3])>);

impl IdColorMemo {
    #[inline]
    fn get(&mut self, rank: usize, id: f32) -> [f32; 3] {
        let bits = id.to_bits();
        match self.0.get_mut(rank) {
            Some(e) if e.0 == bits => e.1,
            Some(e) => {
                *e = (bits, id_color(id));
                e.1
            }
            None => {
                let c = id_color(id);
                self.0.resize(rank + 1, (bits, c));
                c
            }
        }
    }
}

/// The coverage-weighted ID colours at aux pixel `i` (the spec's preview).
#[inline]
fn crypto_colors(planes: &RankPlanes, i: usize, memo: &mut IdColorMemo) -> [f32; 3] {
    let mut c = [0.0f32; 3];
    for (rank, (id, cov)) in planes.iter().enumerate() {
        let (Some(&id), Some(&cov)) = (id.get(i), cov.get(i)) else { continue };
        let k = memo.get(rank, id);
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
    crypto_aux_image_with(a, layer, sel, CryptoAux::ColorsAndMatte)
}

/// What [`crypto_aux_image_with`] computes per aux pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryptoAux {
    /// The coverage-weighted ID colours (RGB) and the selection's coverage (A).
    ColorsAndMatte,
    /// Only the selection's coverage (A); the RGB stays 0.
    MatteOnly,
}

impl CryptoAux {
    /// What the Cryptomatte effect reads for `display` and Matte Only: the ID colours are only
    /// read by the Colors display (0) without Matte Only; Matte, Preview and Matte Only need the
    /// matte alone.
    pub fn for_display(display: u32, matte_only: bool) -> Self {
        if display == 0 && !matte_only { Self::ColorsAndMatte } else { Self::MatteOnly }
    }
}

/// [`crypto_aux_image`]; with [`CryptoAux::MatteOnly`] the RGB stays 0 and only the matte (A) is
/// computed, for the displays that never read the ID colours.
pub fn crypto_aux_image_with(a: &AuxChannels, layer: &str, sel: &[u32], what: CryptoAux) -> Image {
    let planes = a.crypto_rank_planes(layer);
    let mut img = Image::new(a.width, a.height);
    img.data.par_iter_mut().enumerate().for_each_init(IdColorMemo::default, |memo, (i, px)| {
        let c = match what {
            CryptoAux::ColorsAndMatte => crypto_colors(&planes, i, memo),
            CryptoAux::MatteOnly => [0.0; 3],
        };
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
    b.img.data.par_iter_mut().zip(idx.par_iter()).for_each_init(IdColorMemo::default, |memo, (px, &k)| {
        let m = k.map_or(0.0, |k| crypto_matte(&planes, &sel, k));
        if matte_only {
            *px = [m, m, m, m];
            return;
        }
        *px = match display {
            // Colors: coverage-weighted ID colours (the spec's preview), selection brighter.
            0 => {
                let c = k.map_or([0.0; 3], |k| crypto_colors(&planes, k, memo));
                let c = c.map(|v| v * (1.0 - 0.5 * m) + 0.5 * m);
                [c[0], c[1], c[2], 1.0]
            }
            // Matte: the layer keyed by the selection (each pixel only reads itself, so no copy).
            1 => px.map(|v| v * m),
            // Preview: the layer with the selection tinted.
            _ => {
                let s = *px;
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
mod tests;
