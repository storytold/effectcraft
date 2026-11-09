//! 3D Channel effects: 3D Channel Extract, Cryptomatte, Depth Matte, Depth of Field, EXtractoR,
//! Fog 3D, ID Matte and IDentifier.
//!
//! They read the layer's auxiliary channels ([`AuxChannels`], via
//! [`crate::EffectHost::aux`]): multi-layer OpenEXR footage (depth, object/material IDs,
//! Cryptomatte layers, any named channel) or the compositor's depth / layer-ID / Cryptomatte
//! pass for a precomp of a 3D comp. Without auxiliary channels they pass the layer through
//! (EXtractoR still maps the layer's own R, G, B and A).
//!
//! 3D Layer Instance (Create 3D Layer Instance's own effect, Layer ▸ Create) is not an aux
//! reader: it replaces the layer's pixels with a referenced layer's render — for a 3D model
//! layer, its render on its own through the comp's camera — so 2D effects can be applied to a
//! 3D layer's render.
//!
//! Aux pixels are looked up nearest-neighbour (IDs must not be interpolated) at each buffer
//! pixel's layer position, so render scale and layer padding are respected. Cryptomatte makes its
//! finished matte from the unscaled channels first and box-filters only that below render scale 1
//! ([`crypto_plane`]).

use std::sync::Arc;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::channels3d::{AuxLookup, BACKGROUND_DEPTH, crypto_hash};
use effectcraft_raster::{AuxChannels, Image, Px, gaussian_blur};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, hash1, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "3D Channel", params, render, gpu: false, float: true }
}

/// The effects that read their layer's auxiliary channels ([`crate::EffectHost::aux`]): every
/// 3D Channel effect but 3D Layer Instance.
const READS_AUX: [&str; 8] = [
    "ec.3d.channelextract",
    "ec.3d.cryptomatte",
    "ec.3d.depthmatte",
    "ec.3d.depthoffield",
    "ec.3d.extractor",
    "ec.3d.fog3d",
    "ec.3d.idmatte",
    "ec.3d.identifier",
];

/// The parameters that pick which channels or which Cryptomatte layer an effect reads. They
/// can't be animated (no stopwatch, no keyframes, no expression), as in After Effects: the
/// channels a frame needs are known before it renders, from these values alone (#3271).
const CHANNEL_PARAMS: [(&str, &[&str]); 5] = [
    ("ec.3d.channelextract", &["channel"]),
    ("ec.3d.cryptomatte", &["layer", "layerName"]),
    ("ec.3d.extractor", &["red", "green", "blue", "alpha"]),
    ("ec.3d.idmatte", &["auxChannel"]),
    ("ec.3d.identifier", &["channelType"]),
];

/// Does parameter `param` of effect `effect` pick the channels it reads ([`CHANNEL_PARAMS`])?
pub fn selects_channels(effect: &str, param: &str) -> bool {
    CHANNEL_PARAMS.iter().any(|(e, ps)| *e == effect && ps.contains(&param))
}

/// Does effect `id` read its layer's auxiliary channels ([`crate::EffectHost::aux`])? The media
/// layer then decodes the footage's picture together with its channels (#474).
pub fn reads_aux(id: &str) -> bool {
    READS_AUX.contains(&id)
}

/// What effect `id` reads of its layer's auxiliary channels with `params`, declared before the
/// render (#3271): the lookups it makes on the planes, which the footage resolves against the
/// file's channel names to read only those channels (with the picture's, in one pass). `None`
/// for an effect that doesn't say: all channels are read.
pub fn aux_needs(id: &str, params: &crate::Params) -> Option<Vec<AuxLookup>> {
    use effectcraft_raster::channels3d::{COVERAGE_NAMES, DEPTH_NAMES, MATERIAL_ID_NAMES, OBJECT_ID_NAMES};
    let find = AuxLookup::find;
    // (as `id_channel` picks)
    let ids = |which: u32| if which == 1 { find(&MATERIAL_ID_NAMES) } else { find(&OBJECT_ID_NAMES) };
    Some(match id {
        // (as `channel_extract_at` looks them up, fallbacks included)
        "ec.3d.channelextract" => match params.e("channel") {
            0 => vec![find(&DEPTH_NAMES)],
            1 => vec![find(&OBJECT_ID_NAMES)],
            7 => vec![find(&MATERIAL_ID_NAMES)],
            4 => vec![find(&COVERAGE_NAMES)],
            2 => vec![find(&["UV.U", "U", "uv.x"]), find(&["UV.V", "V", "uv.y"]), find(&["UV.W", "W", "uv.z"])],
            3 => vec![find(&["N.X", "Normal.X", "normals.x"]), find(&["N.Y", "Normal.Y", "normals.y"]), find(&["N.Z", "Normal.Z", "normals.z"])],
            5 => vec![find(&["BG.R", "Background.R"]), find(&["BG.G", "Background.G"]), find(&["BG.B", "Background.B"])],
            _ => vec![find(&["R"]), find(&["G"]), find(&["B"])],
        },
        "ec.3d.depthmatte" | "ec.3d.depthoffield" | "ec.3d.fog3d" => vec![find(&DEPTH_NAMES)],
        // (Use Coverage can be animated, as in After Effects: its channel is read whatever its
        // value, so the need doesn't change over time)
        "ec.3d.idmatte" => vec![ids(params.e("auxChannel")), find(&COVERAGE_NAMES)],
        "ec.3d.identifier" => vec![ids(params.e("channelType"))],
        "ec.3d.cryptomatte" => vec![AuxLookup::Crypto { name: params.s("layerName").to_string(), index: params.e("layer") as usize }],
        // (an empty name reads the layer's own channel)
        "ec.3d.extractor" => {
            // (`(copy)` reads the input's own channel, no footage channel)
            ["red", "green", "blue", "alpha"]
                .iter()
                .map(|k| params.s(k).trim())
                .filter(|n| !n.is_empty() && !is_extractor_copy(n))
                .map(|n| AuxLookup::Name(n.to_string()))
                .collect()
        }
        _ => return None,
    })
}

/// [`aux_needs`] as an [`AuxNeed`]: what effect `id` reads, nothing for an effect that reads no
/// channels (every effect that does declares its lookups; a test holds them to it).
pub fn aux_need(id: &str, params: &crate::Params) -> effectcraft_raster::channels3d::AuxNeed {
    effectcraft_raster::channels3d::AuxNeed::Lookups(aux_needs(id, params).unwrap_or_default())
}

/// How far effect `id` with `params` reaches around a pixel of its layer (layer pixels), for
/// reading only the footage rows a region of interest shows (#3271): `Some(0)` for effects whose
/// pixel depends on that pixel alone, `None` for every other (it may read anywhere).
pub fn pixel_margin(id: &str, params: &crate::Params) -> Option<f64> {
    match id {
        "ec.3d.channelextract" | "ec.3d.depthmatte" | "ec.3d.identifier" | "ec.3d.cryptomatte" | "ec.3d.extractor" => Some(0.0),
        // (its Gradient Layer reads another layer)
        "ec.3d.fog3d" if params.f("layerContribution") <= 0.0 => Some(0.0),
        // (Feather blurs the matte)
        "ec.3d.idmatte" if params.f("feather") <= 0.0 => Some(0.0),
        _ => None,
    }
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

/// The Cryptomatte Layer popup and the layer the effect reads (the slot rules live with
/// [`AuxChannels`], so a declared need resolves the layer as the effect does, #3271).
pub use effectcraft_raster::channels3d::{CRYPTO_LAYERS, crypto_layer_choices, crypto_layer_menu, crypto_layer_name, crypto_layer_named};

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
/// selection's coverage (A), computed in parallel with the rank planes resolved once: the
/// per-pixel reduction the effect's planes ([`crypto_plane`]) are made of.
pub fn crypto_aux_image(a: &AuxChannels, layer: &str, sel: &[u32]) -> Image {
    let planes = a.crypto_rank_planes(layer);
    let mut img = Image::new(a.width, a.height);
    img.data.par_iter_mut().enumerate().for_each_init(IdColorMemo::default, |memo, (i, px)| {
        let c = crypto_colors(&planes, i, memo);
        *px = [c[0], c[1], c[2], crypto_matte(&planes, sel, i)];
    });
    img
}

/// The Colors display's pixel for coverage-weighted ID colours `c` and matte `m`: the colours,
/// the selection brighter, over alpha 1. The CPU effect and [`crypto_plane`] both make it here.
#[inline]
fn crypto_colors_px(c: [f32; 3], m: f32) -> Px {
    let c = c.map(|v| v * (1.0 - 0.5 * m) + 0.5 * m);
    [c[0], c[1], c[2], 1.0]
}

/// What a finished Cryptomatte plane ([`crypto_plane`]) holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CryptoOutput {
    /// The selection's coverage, one value per pixel: what Matte, Preview and Matte Only read.
    Matte,
    /// The Colors display's finished pixel (RGBA, alpha 1).
    Colors,
}

impl CryptoOutput {
    /// What the Cryptomatte effect reads for `display` and Matte Only: the ID colours only for
    /// the Colors display (0) without Matte Only; Matte, Preview and Matte Only need the matte
    /// alone.
    pub fn for_display(display: u32, matte_only: bool) -> Self {
        if display == 0 && !matte_only { Self::Colors } else { Self::Matte }
    }

    /// Values per pixel.
    pub fn channels(self) -> usize {
        match self {
            Self::Matte => 1,
            Self::Colors => 4,
        }
    }
}

/// A finished Cryptomatte output (#3334): the matte, or the Colors display's pixels, made from
/// the unscaled ID and coverage channels at source resolution and only then, below render
/// scale 1, box-filtered down. ID and coverage channels are never resampled: interpolating
/// hashed IDs makes IDs that don't exist, and picking one rank's pixel loses coverage at edges.
#[derive(Clone, Debug, PartialEq)]
pub struct CryptoPlane {
    pub width: u32,
    pub height: u32,
    /// Plane pixels per layer pixel (as [`AuxChannels::scale`]).
    pub scale: f64,
    pub output: CryptoOutput,
    /// [`CryptoOutput::channels`] values per pixel, row by row.
    pub data: Vec<f32>,
}

impl CryptoPlane {
    /// The plane pixel at layer point (`lx`, `ly`), with [`AuxChannels::index_at`]'s arithmetic.
    pub fn index_at(&self, lx: f64, ly: f64) -> Option<usize> {
        let x = (lx * self.scale).floor();
        let y = (ly * self.scale).floor();
        if x < 0.0 || y < 0.0 || x >= self.width as f64 || y >= self.height as f64 {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// The bytes its values take.
    pub fn bytes(&self) -> usize {
        self.data.len().saturating_mul(std::mem::size_of::<f32>())
    }

    /// The effect's pixel for buffer pixel `px` over plane pixel `k` (`None`: outside the aux
    /// image, where the matte is 0 and Colors shows black).
    fn shade(&self, px: Px, k: Option<usize>, display: u32, matte_only: bool, in_alpha: bool) -> Px {
        match self.output {
            CryptoOutput::Colors => {
                let at = k.and_then(|k| k.checked_mul(4)).and_then(|i| self.data.get(i..i.saturating_add(4)));
                at.and_then(|c| <[f32; 4]>::try_from(c).ok()).unwrap_or([0.0, 0.0, 0.0, 1.0])
            }
            CryptoOutput::Matte => {
                let m = k.and_then(|k| self.data.get(k)).copied().unwrap_or(0.0);
                crypto_matte_px(px, m, display, matte_only, in_alpha)
            }
        }
    }
}

/// The effect's pixel for buffer pixel `px` and matte `m` in the displays that read the matte
/// alone (Matte, Preview, and every display with Matte Only).
#[inline]
fn crypto_matte_px(px: Px, m: f32, display: u32, matte_only: bool, in_alpha: bool) -> Px {
    if matte_only {
        return matte_only_px(m, in_alpha);
    }
    match display {
        // Matte: the layer keyed by the selection (each pixel only reads itself, so no copy).
        1 => px.map(|v| v * m),
        // Preview: the layer with the selection tinted.
        _ => [px[0] * (1.0 - 0.5 * m) + 0.5 * m * px[3], px[1] * (1.0 - 0.5 * m) + 0.5 * m * px[3], px[2] * (1.0 - 0.5 * m), px[3]],
    }
}

/// The render scale a Cryptomatte plane is box-filtered to ([`crypto_plane`]): `None` at render
/// scale 1 and above, where the plane stays at source resolution and is looked up
/// nearest-neighbour, as every 3D Channel effect looks up its channels.
pub fn crypto_box_scale(render_scale: f64) -> Option<f64> {
    (render_scale.is_finite() && render_scale > 0.0 && render_scale < 1.0 - 1e-9).then_some(render_scale)
}

/// The most pixels a box-filtered plane may have (a footage item claiming a far larger size
/// than its file would otherwise allocate a huge one); beyond it the plane stays unscaled.
const MAX_BOX_PIXELS: usize = 1 << 27;

/// Cryptomatte layer `layer`'s output for selection `sel` (hashes) at `render_scale` (#3334):
/// [`CryptoPlane`]. The matte (or the Colors pixel) is made per aux pixel from every rank's ID
/// and coverage; below render scale 1 the finished plane is box-filtered to the render's pixel
/// grid (coverage is linear, so the filtered matte is exactly the coverage of the larger pixel;
/// the Colors pixels are coloured first and filtered after).
pub fn crypto_plane(a: &AuxChannels, layer: &str, sel: &[u32], output: CryptoOutput, render_scale: f64) -> CryptoPlane {
    let src = crypto_source_plane(a, layer, sel, output);
    match crypto_box_scale(render_scale) {
        Some(s) => crypto_box(src, s),
        None => src,
    }
}

/// [`crypto_plane`] at source resolution: one value (or pixel) per aux pixel.
fn crypto_source_plane(a: &AuxChannels, layer: &str, sel: &[u32], output: CryptoOutput) -> CryptoPlane {
    let planes = a.crypto_rank_planes(layer);
    let n = (a.width as usize).saturating_mul(a.height as usize);
    let data = match output {
        CryptoOutput::Matte => (0..n).into_par_iter().map(|i| crypto_matte(&planes, sel, i)).collect(),
        CryptoOutput::Colors => {
            let mut d = vec![0.0f32; n.saturating_mul(4)];
            d.par_chunks_mut(4).enumerate().for_each_init(IdColorMemo::default, |memo, (i, px)| {
                let c = crypto_colors_px(crypto_colors(&planes, i, memo), crypto_matte(&planes, sel, i));
                px.copy_from_slice(&c);
            });
            d
        }
    };
    CryptoPlane { width: a.width, height: a.height, scale: a.scale, output, data }
}

/// For each output pixel of one axis of a box filter by `k` layer pixels: the source pixels it
/// covers and how much of each (layer pixel `x` reads source pixel `⌊(x + ½) · src_scale⌋`, the
/// pixel render scale 1 shows there).
fn box_axis(layer_len: usize, src_len: usize, src_scale: f64, k: f64, out_len: usize) -> Vec<Vec<(usize, f64)>> {
    (0..out_len)
        .map(|j| {
            let (x0, x1) = (j as f64 * k, (j as f64 + 1.0) * k);
            let last = (x1.ceil() as usize).min(layer_len);
            (x0.floor() as usize..last)
                .filter_map(|x| {
                    let w = x1.min(x as f64 + 1.0) - x0.max(x as f64);
                    let s = ((x as f64 + 0.5) * src_scale).floor();
                    (w > 0.0 && s >= 0.0 && s < src_len as f64).then_some((s as usize, w))
                })
                .collect()
        })
        .collect()
}

/// `src` (at source resolution) box-filtered to render scale `s`: each pixel of the result is
/// the mean of the render-scale-1 pixels it covers (the layer's pixels outside the aux image
/// count as 0, as the effect shows them; the Colors display's alpha is 1 everywhere).
fn crypto_box(src: CryptoPlane, s: f64) -> CryptoPlane {
    if !(src.scale.is_finite() && src.scale > 0.0) {
        return src;
    }
    // (layer pixels per output pixel; a whole number when it is one within rounding, so a
    // third resolution averages exactly three by three pixels)
    let k = 1.0 / s;
    let k = if (k - k.round()).abs() < 1e-9 * k { k.round() } else { k };
    let (lw, lh) = ((src.width as f64 / src.scale).ceil(), (src.height as f64 / src.scale).ceil());
    let (dw, dh) = ((lw / k * (1.0 - 1e-12)).ceil(), (lh / k * (1.0 - 1e-12)).ceil());
    let limit = (src.width as usize).saturating_mul(src.height as usize).saturating_mul(4).min(MAX_BOX_PIXELS);
    let fits = |v: f64| v.is_finite() && v >= 0.0 && v <= MAX_BOX_PIXELS as f64;
    // (the filter visits every layer pixel: bounded by the source's size too)
    if !(fits(lw) && fits(lh) && fits(dw) && fits(dh)) || (dw * dh) as usize > limit || (lw * lh) as usize > limit {
        return src;
    }
    let (lw, lh, dw, dh) = (lw as usize, lh as usize, dw as usize, dh as usize);
    let ch = src.output.channels();
    let xs = box_axis(lw, src.width as usize, src.scale, k, dw);
    let ys = box_axis(lh, src.height as usize, src.scale, k, dh);
    let (sw, area) = (src.width as usize, k * k);
    let mut data = vec![0.0f32; dw * dh * ch];
    data.par_chunks_mut((dw * ch).max(1)).zip(ys.par_iter()).for_each(|(row, ty)| {
        for (dx, tx) in xs.iter().enumerate() {
            for c in 0..ch {
                let mut sum = 0.0f64;
                for &(sy, wy) in ty {
                    for &(sx, wx) in tx {
                        let v = src.data.get((sy * sw + sx) * ch + c).copied().unwrap_or(0.0);
                        sum += wy * wx * v as f64;
                    }
                }
                if let Some(o) = row.get_mut(dx * ch + c) {
                    *o = (sum / area) as f32;
                }
            }
        }
    });
    if src.output == CryptoOutput::Colors {
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 1.0;
        }
    }
    CryptoPlane { width: dw as u32, height: dh as u32, scale: s, output: src.output, data }
}

/// The hidden switch that keeps Matte Only as projects saved before #3127 render it: the matte in
/// RGB and alpha. Fresh instances have it off (the matte in RGB over alpha 1, as After Effects'
/// Cryptomatte plug-in shows Matte Only); `migrate::legacy_default` turns it on for older saves.
pub const MATTE_ONLY_IN_ALPHA: &str = "matteOnlyInAlpha";

/// A Matte Only pixel for matte `m`: grey `m` over alpha 1, or with `in_alpha` (older
/// projects) `m` in every channel. The GPU shader (`fx_depth.wgsl`, op 7) does the same.
#[inline]
pub fn matte_only_px(m: f32, in_alpha: bool) -> Px {
    if in_alpha { [m; 4] } else { [m, m, m, 1.0] }
}

fn cryptomatte(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(a) = aux(ctx) else { return b };
    let Some(layer) = crypto_layer_named(&a, ctx.params.s("layerName"), ctx.params.e("layer") as usize) else { return b };
    let manifest = a.manifest(&layer);
    let sel = selection_hashes(ctx.params.s("selection"), manifest);
    let display = ctx.params.e("display");
    let matte_only = ctx.params.b("matteOnly");
    let in_alpha = ctx.params.b(MATTE_ONLY_IN_ALPHA);
    // Below render scale 1: the finished matte (or Colors pixels) box-filtered down (#3334),
    // looked up per buffer pixel; the ID channels are never resampled.
    if crypto_box_scale(b.scale).is_some() {
        let plane = crypto_plane(&a, &layer, &sel, CryptoOutput::for_display(display, matte_only), b.scale);
        let idx = crypto_index_map(&b, &plane);
        b.img.data.par_iter_mut().zip(idx.par_iter()).for_each(|(px, &k)| *px = plane.shade(*px, k, display, matte_only, in_alpha));
        return b;
    }
    let idx = index_map(&b, &a);
    let planes = a.crypto_rank_planes(&layer);
    b.img.data.par_iter_mut().zip(idx.par_iter()).for_each_init(IdColorMemo::default, |memo, (px, &k)| {
        let m = k.map_or(0.0, |k| crypto_matte(&planes, &sel, k));
        *px = if display == 0 && !matte_only {
            // Colors: coverage-weighted ID colours (the spec's preview), selection brighter.
            crypto_colors_px(k.map_or([0.0; 3], |k| crypto_colors(&planes, k, memo)), m)
        } else {
            crypto_matte_px(*px, m, display, matte_only, in_alpha)
        };
    });
    b
}

/// The [`CryptoPlane`] pixel for each buffer pixel (None outside the aux image), looked up as
/// [`index_map`] looks up the aux.
fn crypto_index_map(b: &Buf, plane: &CryptoPlane) -> Vec<Option<usize>> {
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let [x, y] = aux_point(b.scale, b.offset, [(i % w) as f64 + 0.5, (i / w) as f64 + 0.5]);
            plane.index_at(x, y)
        })
        .collect()
}

// ---------------------------------------------------------------- EXtractoR

/// EXtractoR's channel name for After Effects' `(copy)`: the effect input's own channel of the
/// same position (red's red ... alpha's alpha), as the earlier effects left it, never a footage
/// channel of some name (an OpenEXR's `A` is the file's alpha, not the input's).
pub const EXTRACTOR_COPY: &str = "(copy)";

/// Whether an EXtractoR channel name is [`EXTRACTOR_COPY`].
pub fn is_extractor_copy(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(EXTRACTOR_COPY)
}

/// The layer's own channel an EXtractoR output channel at position `k` reads when no footage
/// channel of that name exists: `R`/`G`/`B`/`A` (or the long names) by name, [`EXTRACTOR_COPY`]
/// its own position.
pub fn extractor_own_channel(k: usize, name: &str) -> Option<usize> {
    if is_extractor_copy(name) {
        return (k < 4).then_some(k);
    }
    match name.trim().to_ascii_uppercase().as_str() {
        "R" | "RED" => Some(0),
        "G" | "GREEN" => Some(1),
        "B" | "BLUE" => Some(2),
        "A" | "ALPHA" => Some(3),
        _ => None,
    }
}

/// The footage channel an EXtractoR output channel reads by name: none for an empty name or
/// [`EXTRACTOR_COPY`] (the input's own channel).
pub fn extractor_aux_channel<'a>(a: &'a AuxChannels, name: &str) -> Option<&'a [f32]> {
    let n = name.trim();
    if n.is_empty() || is_extractor_copy(n) { None } else { a.get(n) }
}

fn extractor(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let a = aux(ctx);
    let names = [ctx.params.s("red"), ctx.params.s("green"), ctx.params.s("blue"), ctx.params.s("alpha")];
    let (bp, wp) = (ctx.params.f("blackPoint") as f32, ctx.params.f("whitePoint") as f32);
    let unmult = ctx.params.b("unMult");
    let clip = ctx.params.b("clip");
    let idx = a.as_ref().map(|a| index_map(&b, a));
    // Each output channel: an aux channel by name, or the layer's own R/G/B/A (`(copy)`: its
    // own position, never an aux channel).
    let own = |k: usize, n: &str| extractor_own_channel(k, n);
    let src_ch: Vec<Option<&[f32]>> = names.iter().map(|n| a.as_ref().and_then(|a| extractor_aux_channel(a, n))).collect();
    if src_ch.iter().all(Option::is_none) && bp == 0.0 && wp == 1.0 && !unmult && names.iter().enumerate().all(|(k, n)| own(k, n) == Some(k)) {
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
                (None, _) => match own(k, names[k]) {
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

/// A 2D copy of a referenced layer (Create 3D Layer Instance's solid): the layer's pixels
/// replaced with the referenced layer's render, alpha included — the copy is transparent
/// where the referenced layer isn't. A 3D model or primitive layer is rendered on its own
/// through the comp's camera, as After Effects renders 3D layers; another layer is read with
/// its masks and effects. With no layer chosen the effect passes the layer through, as a layer
/// parameter with nothing chosen does.
fn instance_3d(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if let Some(o) = ctx.layer_param("layer", true) {
        b.img = crate::util::fit_layer(ctx, &b, &o, false);
    }
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
                p(MATTE_ONLY_IN_ALPHA, "Matte Only In Alpha", Value::Bool(false), ParamUi::Hidden),
                // A layer named in full (`CryptoMaterial`); empty uses Layer. Last, so the
                // earlier parameters keep their indexes and saved instances, which get it
                // appended, have the same order as new ones.
                text("layerName", "Layer Name", ""),
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
        spec("ec.3d.instance3d", "3D Layer Instance", vec![p("layer", "Layer", Value::Layer(None), ParamUi::Layer)], instance_3d),
    ]
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod aux_need_tests {
    /// #3271: ID Matte's Use Coverage can be animated (as in After Effects): the coverage channel
    /// is read whether it is on or off, so what a frame reads doesn't depend on it.
    #[test]
    fn id_matte_reads_coverage_either_way() {
        let spec = crate::find("ec.3d.idmatte").expect("an effect");
        let mut params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        let off = super::aux_needs("ec.3d.idmatte", &params);
        params.values.insert("useCoverage".into(), effectcraft_keyframe::Value::Bool(true));
        let on = super::aux_needs("ec.3d.idmatte", &params);
        assert_eq!(off, on);
        assert_eq!(on.map(|v| v.len()), Some(2), "the ID channel and the coverage channel");
        assert!(!super::selects_channels("ec.3d.idmatte", "useCoverage"), "animatable");
        assert!(super::selects_channels("ec.3d.idmatte", "auxChannel"), "static");
    }

    /// #3271: every effect that reads its layer's channels says which (else a frame would read
    /// all of them, or none).
    #[test]
    fn every_channel_effect_declares_what_it_reads() {
        for id in super::READS_AUX {
            let spec = crate::find(id).expect("an effect");
            let params = crate::Params { values: spec.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            assert!(super::aux_needs(id, &params).is_some(), "{id}");
        }
    }
}
