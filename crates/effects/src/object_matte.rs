//! Object Matte (Effect ▸ Matte): isolate one object from a click, and follow it through the
//! shot (After Effects 26.2's Object Matte, from its public description: pick an object with a
//! point, optionally a box around it, and get a matte that tracks it).
//!
//! The prompt is the **Object Point** (layer pixels) and, with **Use Box**, a box around the
//! object. On the **Reference Frame** it is segmented by the same engine as Roto Brush: the
//! trained model chosen in Settings ▸ Roto Brush (MobileSAM, `effectcraft_segment`) when one is
//! installed, the classical graph cut otherwise. The model gets the point as a foreground point
//! and the box as its box prompt; the graph cut gets the point as a foreground stroke and the
//! box's edges (without a box, the layer's border) as background strokes. With **Track Object** the
//! matte is then carried frame by frame up to **Frames Before** / **Frames After** away (optical
//! flow, re-segmented by the model prompted with the carried matte); without it every frame is
//! segmented on its own from the prompt (useful when the point is animated).
//!
//! Like Roto Brush, the segmentation is derived data in Roto Brush's content-keyed cache
//! ([`crate::roto::cached`]): the chain seed holds the engine-maintained Input Key (what the
//! frames are made of), the layer size, the frame rate, the mode and the model, and every
//! frame's key the prompt, so edits upstream or to the prompt recompute it and undo never sees a
//! stale matte. The engine fills that cache in a background job (`objectMatte.propagate`,
//! outward from the reference frame over [`job_frames`]) and **Freeze** (`objectMatte.freeze`)
//! stores the final mattes in the hidden **Frozen** parameter ([`FrozenCache`], as Roto Brush
//! does): they are used, without segmenting, while [`frozen_key`] still matches. **Feather**,
//! **Contrast** and **Shift Edge** shape the matte, **Invert** keeps everything but the object
//! and **View ▸ Matte** shows the matte in grey.

use std::sync::Arc;

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use effectcraft_segment::MaskModel;
use effectcraft_track::roto::refine::MatteParams;
use effectcraft_track::roto::{FrameSeg, RotoData, SegOpts, Stroke, StrokeKind, rle};
use rayon::prelude::*;

use crate::roto::{self, Chain, FROZEN, FrozenCache, INPUT_KEY};
use crate::warp_stab::param;
use crate::{Buf, EffectCtx, EffectSpec, Params, num, p, popup, slider};

/// The effect's id.
pub const ID: &str = "ec.matte.objectmatte";

/// Display names of the instance's parameter groups.
pub const GROUPS: &[(&str, &str)] = &[("objectTracking", "Tracking"), ("objectMatte", "Matte")];

pub const VIEWS: [&str; 2] = ["Final Output", "Matte"];

/// Most frames a matte is carried from the reference frame each way.
pub const MAX_REACH: f64 = 600.0;
/// Default reach each way (frames).
pub const DEFAULT_REACH: f64 = 30.0;

/// Radius (layer pixels) of the foreground stroke the Object Point becomes.
const POINT_RADIUS: f64 = 3.0;
/// Radius (layer pixels) of the background strokes along the box / the layer's border.
const EDGE_RADIUS: f64 = 2.0;

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "Object Matte",
        category: "Matte",
        params: vec![
            p("objectPoint", "Object Point", Value::Vec2([0.5, 0.5]), ParamUi::Point),
            p("useBox", "Use Box", Value::Bool(false), ParamUi::Checkbox),
            p("boxTopLeft", "Box Top Left", Value::Vec2([0.25, 0.25]), ParamUi::Point),
            p("boxBottomRight", "Box Bottom Right", Value::Vec2([0.75, 0.75]), ParamUi::Point),
            p("objectTracking/trackObject", "Track Object", Value::Bool(true), ParamUi::Checkbox),
            p("objectTracking/referenceFrame", "Reference Frame", num(0.0), ParamUi::Number),
            p("objectTracking/framesBefore", "Frames Before", num(DEFAULT_REACH), slider(0.0, MAX_REACH, 0.0, 120.0, 0)),
            p("objectTracking/framesAfter", "Frames After", num(DEFAULT_REACH), slider(0.0, MAX_REACH, 0.0, 120.0, 0)),
            p("objectMatte/feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("objectMatte/contrast", "Contrast", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("objectMatte/shiftEdge", "Shift Edge", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
            p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
            p("view", "View", Value::Enum(0), popup(&VIEWS)),
            p(INPUT_KEY, "Input Key", Value::Str(String::new()), ParamUi::Hidden),
            p(FROZEN, "Frozen", Value::Str(String::new()), ParamUi::Hidden),
        ],
        render,
        gpu: false,
        float: true,
    }]
}

fn f(params: &Params, id: &str) -> f64 {
    param(params, id).map(Value::as_f64).unwrap_or(0.0)
}
fn b(params: &Params, id: &str) -> bool {
    param(params, id).map(Value::as_bool).unwrap_or(false)
}
fn v2(params: &Params, id: &str) -> Option<[f64; 2]> {
    let v = param(params, id)?.as_vec2();
    v.iter().all(|c| c.is_finite()).then_some(v)
}
fn s<'a>(params: &'a Params, id: &str) -> &'a str {
    match param(params, id) {
        Some(Value::Str(s)) => s,
        _ => "",
    }
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// A frame count parameter (finite, clamped to `lo..=hi`).
fn frames(params: &Params, id: &str, lo: f64, hi: f64) -> i64 {
    let v = f(params, id);
    if v.is_finite() { v.round().clamp(lo, hi) as i64 } else { 0 }
}

/// The reference frame (layer frame).
pub fn reference_frame(params: &Params) -> i64 {
    frames(params, "objectTracking/referenceFrame", -1e9, 1e9)
}

/// Frames Before / After (finite, clamped to `0..=MAX_REACH`).
pub fn reach(params: &Params) -> [i64; 2] {
    [frames(params, "objectTracking/framesBefore", 0.0, MAX_REACH), frames(params, "objectTracking/framesAfter", 0.0, MAX_REACH)]
}

/// Whether the matte is carried from the reference frame (else every frame is segmented on its
/// own).
pub fn tracks(params: &Params) -> bool {
    b(params, "objectTracking/trackObject")
}

/// The prompt as Roto Brush strokes on layer frame `frame`: the Object Point as a foreground
/// stroke; the box as a box stroke (the model's box prompt) with a model, its edges as
/// background strokes for the graph cut without one; without a box, the layer's border when
/// there is no model (the graph cut needs background samples; a model does not, and a border
/// stroke would cut objects touching the frame edge). `None` when the point is unusable.
pub fn prompt_strokes(params: &Params, layer_size: [f64; 2], frame: i64, with_model: bool) -> Option<Vec<Stroke>> {
    let [w, h] = layer_size;
    if !(w.is_finite() && h.is_finite() && w >= 1.0 && h >= 1.0) {
        return None;
    }
    let pt = v2(params, "objectPoint")?;
    let inside = |q: [f64; 2]| q[0] >= 0.0 && q[1] >= 0.0 && q[0] <= w && q[1] <= h;
    if !inside(pt) {
        return None;
    }
    let mut out = vec![Stroke { kind: StrokeKind::Fg, frame, radius: POINT_RADIUS, points: vec![pt] }];
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
    let boxed = b(params, "useBox").then(|| Some((v2(params, "boxTopLeft")?, v2(params, "boxBottomRight")?))).flatten();
    match boxed {
        Some((a, c)) => {
            let (x0, x1) = (a[0].min(c[0]).clamp(0.0, w), a[0].max(c[0]).clamp(0.0, w));
            let (y0, y1) = (a[1].min(c[1]).clamp(0.0, h), a[1].max(c[1]).clamp(0.0, h));
            // The point must lie inside the box, clear of its edges, for the box to mean anything.
            let m = POINT_RADIUS + EDGE_RADIUS;
            if pt[0] - m <= x0 || pt[0] + m >= x1 || pt[1] - m <= y0 || pt[1] + m >= y1 {
                return None;
            }
            out.push(if with_model {
                // The model takes the box itself as its box prompt.
                Stroke { kind: StrokeKind::Box, frame, radius: 0.0, points: vec![[x0, y0], [x1, y1]] }
            } else {
                Stroke { kind: StrokeKind::Bg, frame, radius: EDGE_RADIUS, points: rect(x0, y0, x1, y1) }
            });
        }
        None if !with_model => {
            let i = EDGE_RADIUS;
            if w <= 4.0 * i || h <= 4.0 * i {
                return None;
            }
            out.push(Stroke { kind: StrokeKind::Bg, frame, radius: EDGE_RADIUS, points: rect(i, i, w - i, h - i) });
        }
        None => {}
    }
    Some(out)
}

/// The segmentation data the matte of layer frame `f` comes from: with Track Object, the prompt
/// on the reference frame and the span it is carried over; otherwise the prompt on `f` alone.
pub fn roto_data(params: &Params, layer_size: [f64; 2], f: i64, with_model: bool) -> Option<RotoData> {
    if tracks(params) {
        let base = reference_frame(params);
        let [before, after] = reach(params);
        let strokes = prompt_strokes(params, layer_size, base, with_model)?;
        Some(RotoData { base: Some(base), span: [base.saturating_sub(before), base.saturating_add(after)], strokes })
    } else {
        Some(RotoData { base: Some(f), span: [f, f], strokes: prompt_strokes(params, layer_size, f, with_model)? })
    }
}

/// Seed of the chain keys: what the frames are (the Input Key), the layer size, the frame rate,
/// the mode and the model. The prompt is in each frame's key.
pub fn seed(params: &Params, layer_size: [f64; 2], fps: f64, model: Option<&dyn MaskModel>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, ID.as_bytes());
    fnv(&mut h, s(params, INPUT_KEY).as_bytes());
    for v in [layer_size[0], layer_size[1], fps] {
        fnv(&mut h, &v.to_bits().to_le_bytes());
    }
    fnv(&mut h, &[tracks(params) as u8]);
    fnv(&mut h, model.map_or(effectcraft_segment::CLASSICAL, |m| m.info().id).as_bytes());
    h
}

/// The layer frames a background job computes, outward from the reference frame: the tracked
/// span (Frames Before / After around it), clipped to `limits` (the layer's frames). Without
/// Track Object, the same frames, each segmented on its own.
pub fn job_frames(params: &Params, limits: [i64; 2]) -> Vec<i64> {
    let base = reference_frame(params);
    let [before, after] = reach(params);
    let (lo, hi) = (base.saturating_sub(before).max(limits[0]), base.saturating_add(after).min(limits[1]));
    if base < limits[0] || base > limits[1] {
        return vec![];
    }
    let mut v = vec![base];
    v.extend(base.saturating_add(1)..=hi);
    v.extend((lo..base).rev());
    v
}

/// The segmentation chain the matte of layer frame `f` comes from (`None`: no usable prompt).
pub fn chain(params: &Params, layer_size: [f64; 2], fps: f64, f: i64, scale: f64, model: Option<Arc<dyn MaskModel>>) -> Option<Chain> {
    let data = roto_data(params, layer_size, f, model.is_some())?;
    let keys = data.chain_keys(seed(params, layer_size, fps, model.as_deref()));
    Some(Chain { data: Arc::new(data), keys, opts: seg_opts(scale), refine_radius: 0.0, model })
}

/// Frame `f`'s segmentation at full resolution, computing what is missing (the background
/// job's step). `None` outside the tracked span, without a prompt, or when cancelled.
pub fn ensure(
    params: &Params,
    layer_size: [f64; 2],
    fps: f64,
    f: i64,
    model: Option<Arc<dyn MaskModel>>,
    frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>,
    cancel: &dyn Fn() -> bool,
) -> Option<Arc<FrameSeg>> {
    let ch = chain(params, layer_size, fps, f, 1.0, model)?;
    if !ch.data.in_span(f) {
        return None;
    }
    ch.ensure(f, frame, cancel)
}

/// The chain key of each of `frames` (what a computed segmentation is cached under).
pub fn frame_keys(params: &Params, layer_size: [f64; 2], fps: f64, frames: &[i64], model: Option<Arc<dyn MaskModel>>) -> Vec<(i64, u64)> {
    if tracks(params) {
        let Some(ch) = chain(params, layer_size, fps, 0, 1.0, model) else { return vec![] };
        return frames.iter().filter_map(|f| Some((*f, *ch.keys.get(f)?))).collect();
    }
    frames.iter().filter_map(|f| Some((*f, *chain(params, layer_size, fps, *f, 1.0, model.clone())?.keys.get(f)?))).collect()
}

/// What frozen mattes depend on: what the frames are (the seed: Input Key, size, rate, mode,
/// model), the prompt, the tracked span and the matte settings.
pub fn frozen_key(params: &Params, layer_size: [f64; 2], fps: f64, model: Option<&dyn MaskModel>) -> String {
    let mut h = seed(params, layer_size, fps, model);
    let base = reference_frame(params);
    if let Some(st) = prompt_strokes(params, layer_size, base, model.is_some()) {
        fnv(&mut h, serde_json::to_string(&st).unwrap_or_default().as_bytes());
    }
    for v in [base, reach(params)[0], reach(params)[1]] {
        fnv(&mut h, &v.to_le_bytes());
    }
    fnv(&mut h, format!("{:?}", matte_params(params)).as_bytes());
    format!("{h:016x}")
}

/// Whether the instance is frozen (has frozen mattes).
pub fn is_frozen(params: &Params) -> bool {
    !s(params, FROZEN).is_empty()
}

/// Frame `f`'s frozen matte on a `w`×`h` grid, when the instance is frozen, the mattes still
/// match ([`frozen_key`]) and `f` is one of them.
pub fn frozen_alpha(params: &Params, layer_size: [f64; 2], fps: f64, f: i64, w: usize, h: usize, model: Option<&dyn MaskModel>) -> Option<Vec<f32>> {
    let j = s(params, FROZEN);
    if j.is_empty() {
        return None;
    }
    let fc = frozen_cache(j)?;
    if fc.key != frozen_key(params, layer_size, fps, model) {
        return None;
    }
    let fr = fc.frames.get(&f)?;
    let [fw, fh] = fc.size;
    let expected = fw.checked_mul(fh)?;
    if fw == 0 || fh == 0 || expected > rle::MAX_PLANE_BYTES {
        return None;
    }
    let a = rle::decode_bounded(&fr.a, expected)?;
    if a.len() != expected {
        return None;
    }
    let af: Vec<f32> = a.iter().map(|v| *v as f32 / 255.0).collect();
    if fw == w && fh == h {
        return Some(af);
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = ((x as f64 + 0.5) * fw as f64 / w as f64, (y as f64 + 0.5) * fh as f64 / h as f64);
            out[y * w + x] = effectcraft_track::roto::matting::sample(&af, fw, fh, sx, sy);
        }
    }
    Some(out)
}

/// The parsed Frozen parameter (parsed once per content).
fn frozen_cache(j: &str) -> Option<Arc<FrozenCache>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static C: OnceLock<Mutex<HashMap<u64, Arc<FrozenCache>>>> = OnceLock::new();
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, j.as_bytes());
    let c = C.get_or_init(Default::default);
    if let Some(d) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&h) {
        return Some(d.clone());
    }
    let d = Arc::new(FrozenCache::from_json(j)?);
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 8 {
        m.clear();
    }
    m.insert(h, d.clone());
    Some(d)
}

/// Segmentation settings at `scale` (Roto Brush's defaults).
fn seg_opts(scale: f64) -> SegOpts {
    SegOpts { scale, search_radius: 15.0, motion_threshold: 10.0, motion_damping: 20.0, best: false }
}

/// Matte settings: Feather, Contrast, Shift Edge (no Refine Edge band).
pub fn matte_params(params: &Params) -> MatteParams {
    let num = |id: &str| {
        let v = f(params, id);
        if v.is_finite() { v } else { 0.0 }
    };
    MatteParams {
        rb_feather: num("objectMatte/feather").clamp(0.0, 100.0),
        rb_contrast: num("objectMatte/contrast").clamp(0.0, 100.0),
        rb_shift: num("objectMatte/shiftEdge").clamp(-100.0, 100.0),
        decontaminate: false,
        ..MatteParams::default()
    }
}

/// What a frame's matte is: the object's alpha on the layer grid, or why there is none.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameMatte {
    /// The object's alpha (0–1) on the frame's layer grid.
    Alpha(Vec<f32>),
    /// The prompt is unusable (point outside the layer, outside the box…): the layer passes.
    NoPrompt,
    /// The frame is beyond the tracked range, or its matte could not be computed.
    Outside,
}

/// The matte of layer frame `f` on the layer grid of `img` (the effect's input on that frame at
/// `scale`), computing what is missing. `frame(g)`: the input on frame g on the same grid.
#[allow(clippy::too_many_arguments)]
pub fn frame_matte(
    params: &Params,
    layer_size: [f64; 2],
    fps: f64,
    f: i64,
    scale: f64,
    img: &Image,
    model: Option<Arc<dyn MaskModel>>,
    frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>,
    cancel: &dyn Fn() -> bool,
) -> FrameMatte {
    let Some(chain) = chain(params, layer_size, fps, f, scale, model) else { return FrameMatte::NoPrompt };
    if !chain.data.in_span(f) {
        return FrameMatte::Outside;
    }
    let Some(seg) = chain.ensure(f, frame, cancel) else { return FrameMatte::Outside };
    let (w, h) = (img.width as usize, img.height as usize);
    let seg = if seg.w == w && seg.h == h { (*seg).clone() } else { roto::resample(&seg, w, h) };
    FrameMatte::Alpha(roto::final_alpha(img, &seg, None, None, &matte_params(params), 0.0, scale))
}

fn render(ctx: &EffectCtx, buf: Buf) -> Buf {
    if ctx.adjustment {
        return buf;
    }
    let fps = ctx.fps();
    let f = roto::frame_of(ctx.time, fps);
    let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    let img = Arc::new(roto::layer_grid(&buf, ctx.layer_size));
    let (w, h) = (img.width as usize, img.height as usize);
    let host = ctx.env.host;
    let effects = ctx.env.effect_index;
    let layer_size = ctx.layer_size;
    let me = img.clone();
    let mut frame = |g: i64| -> Option<Arc<Image>> {
        if g == f {
            return Some(me.clone());
        }
        let other = host?.self_at(g as f64 / fps, effects)?;
        let im = roto::layer_grid(&other, layer_size);
        (im.width as usize == w && im.height as usize == h).then(|| Arc::new(im))
    };
    let model = roto::installed_model();
    // Frozen mattes are used as they are (no segmentation).
    if let Some(a) = frozen_alpha(ctx.params, layer_size, fps, f, w, h, model.as_deref()) {
        return compose(&buf, Some(&a), w, h, b(ctx.params, "invert"), param(ctx.params, "view").map(Value::as_enum).unwrap_or(0) == 1);
    }
    let alpha = match frame_matte(ctx.params, layer_size, fps, f, sc, &img, model, &mut frame, &|| false) {
        FrameMatte::Alpha(a) if a.len() == w * h => Some(a),
        FrameMatte::NoPrompt => return buf,
        _ => None,
    };
    compose(&buf, alpha.as_deref(), w, h, b(ctx.params, "invert"), param(ctx.params, "view").map(Value::as_enum).unwrap_or(0) == 1)
}

/// The output: the input keyed by the matte (`None`: no object, all zero), or the matte itself.
fn compose(buf: &Buf, alpha: Option<&[f32]>, w: usize, h: usize, invert: bool, view_matte: bool) -> Buf {
    let (ox, oy) = (buf.offset[0].round() as i64, buf.offset[1].round() as i64);
    let mut out = Buf { img: Image::new(buf.img.width, buf.img.height), offset: buf.offset, scale: buf.scale };
    out.img.rows_mut().for_each(|(by, row)| {
        for (bx, px) in row.iter_mut().enumerate() {
            let (x, y) = (bx as i64 - ox, by as i64 - oy);
            let a = if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                alpha.and_then(|a| a.get(y as usize * w + x as usize)).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let a = if invert { 1.0 - a } else { a }.clamp(0.0, 1.0);
            let src = buf.img.get(bx as i64, by as i64);
            *px = if view_matte {
                let v = a * src[3];
                [v, v, v, 1.0]
            } else {
                [src[0] * a, src[1] * a, src[2] * a, src[3] * a]
            };
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels};

    /// A red disk of radius 15 moving 2 px a frame right over a textured blue background
    /// (100×80, 25 fps), plus a still green square in the corner.
    fn frame_at(fi: i64) -> Image {
        let c = [40.0 + 2.0 * fi as f64, 40.0];
        let mut img = Image::new(100, 80);
        for y in 0..80 {
            for x in 0..100 {
                let d = ((x as f64 + 0.5 - c[0]).hypot(y as f64 + 0.5 - c[1])) - 15.0;
                let a = (0.5 - d).clamp(0.0, 1.0) as f32;
                let tex = ((x * 7 + y * 13) % 9) as f32 / 60.0;
                let bg = if x >= 80 && y >= 60 { [0.1, 0.8, 0.2] } else { [0.1, 0.4 + tex, 0.7] };
                let fg = [0.85 + tex, 0.2, 0.1];
                img.set(x, y, [fg[0] * a + bg[0] * (1.0 - a), fg[1] * a + bg[1] * (1.0 - a), fg[2] * a + bg[2] * (1.0 - a), 1.0]);
            }
        }
        img
    }
    fn frame_of_time(t: f64) -> Image {
        frame_at((t * 25.0 + 1e-6).floor() as i64)
    }

    struct Seq;
    impl EffectHost for Seq {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            Some(Buf { img: frame_of_time(t), offset: [0.0; 2], scale: 1.0 })
        }
    }

    fn params(vals: &[(&str, Value)]) -> Params {
        let s = &specs()[0];
        let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [100.0, 80.0]))).collect() };
        for (k, v) in vals {
            p.values.insert(k.to_string(), v.clone());
        }
        p
    }

    fn run(p: &Params, fi: i64) -> Image {
        let host = Seq;
        let t = fi as f64 / 25.0 + 0.001;
        let ctx = EffectCtx {
            params: p,
            time: t,
            layer_size: [100.0, 80.0],
            seed: 1,
            adjustment: false,
            env: EffectEnv { host: Some(&host), frame_rate: 25.0, ..Default::default() },
        };
        crate::apply(&specs()[0], &ctx, Buf { img: frame_of_time(t), offset: [0.0; 2], scale: 1.0 }).img
    }

    /// Pixels whose alpha disagrees with the disk centred at `cx` (1 px of slack at the edge).
    fn wrong(out: &Image, cx: f64) -> usize {
        let mut n = 0;
        for y in 0..80u32 {
            for x in 0..100u32 {
                let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - 40.0);
                let a = out.data[(y * 100 + x) as usize][3];
                if (d < 14.0 && a < 0.5) || (d > 16.0 && a > 0.5) {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn a_click_isolates_the_object_and_tracks_it() {
        let p = params(&[("objectPoint", Value::Vec2([40.0, 40.0]))]);
        // The reference frame: the disk, nothing else.
        let out = run(&p, 0);
        assert!(wrong(&out, 40.0) < 20, "{} wrong pixels on the reference frame", wrong(&out, 40.0));
        // Six frames on (carried through frames 1–5), the disk has moved 12 px and the matte with it.
        let out = run(&p, 6);
        assert!(wrong(&out, 52.0) < 30, "{} wrong pixels on frame 6", wrong(&out, 52.0));
        // Beyond Frames After: no matte.
        let p2 = params(&[("objectPoint", Value::Vec2([40.0, 40.0])), ("objectTracking/framesAfter", Value::Scalar(3.0))]);
        assert!(run(&p2, 6).data.iter().all(|px| px[3] == 0.0));
        // Invert keeps everything but the disk; View ▸ Matte shows it in grey, opaque.
        let inv = run(&params(&[("objectPoint", Value::Vec2([40.0, 40.0])), ("invert", Value::Bool(true))]), 0);
        assert!(inv.data[40 * 100 + 40][3] < 0.5 && inv.data[5 * 100 + 95][3] > 0.5);
        let m = run(&params(&[("objectPoint", Value::Vec2([40.0, 40.0])), ("view", Value::Enum(1))]), 0);
        assert!(m.data[40 * 100 + 40][0] > 0.5 && m.data[5 * 100 + 95][0] < 0.5 && m.data.iter().all(|px| px[3] == 1.0));
    }

    #[test]
    fn without_tracking_every_frame_follows_the_prompt() {
        // The point animated onto the disk at frame 9: frame 9 is segmented from it alone.
        let p = params(&[("objectPoint", Value::Vec2([58.0, 40.0])), ("objectTracking/trackObject", Value::Bool(false))]);
        let out = run(&p, 9);
        assert!(wrong(&out, 58.0) < 20, "{} wrong pixels", wrong(&out, 58.0));
    }

    #[test]
    fn a_box_and_bad_prompts() {
        // A box around the green square, the point in it: the square, not the disk.
        let p = params(&[
            ("objectPoint", Value::Vec2([90.0, 70.0])),
            ("useBox", Value::Bool(true)),
            ("boxTopLeft", Value::Vec2([74.0, 54.0])),
            ("boxBottomRight", Value::Vec2([100.0, 80.0])),
        ]);
        let out = run(&p, 0);
        assert!(out.data[70 * 100 + 90][3] > 0.5 && out.data[40 * 100 + 40][3] < 0.5);
        // Unusable prompts pass the layer through instead of failing: a point off the layer, a
        // non-finite point, a point outside its box.
        let src = frame_at(0);
        for vals in [
            vec![("objectPoint", Value::Vec2([-5.0, 40.0]))],
            vec![("objectPoint", Value::Vec2([f64::NAN, 40.0]))],
            vec![
                ("objectPoint", Value::Vec2([40.0, 40.0])),
                ("useBox", Value::Bool(true)),
                ("boxTopLeft", Value::Vec2([60.0, 10.0])),
                ("boxBottomRight", Value::Vec2([90.0, 30.0])),
            ],
        ] {
            let p = params(&vals);
            assert_eq!(run(&p, 0).data, src.data, "{vals:?}");
        }
        // A huge reference frame or reach doesn't overflow.
        let p = params(&[("objectTracking/referenceFrame", Value::Scalar(1e300)), ("objectTracking/framesBefore", Value::Scalar(f64::INFINITY))]);
        assert!(run(&p, 0).data.iter().all(|px| px[3] == 0.0));
    }

    /// A stand-in model (the weights are an optional download): a disk around the first
    /// foreground point, radius 15, so the test sees the model's answer in the matte.
    #[derive(Default)]
    struct Stub {
        calls: std::sync::atomic::AtomicUsize,
        /// The box prompt of the last call.
        bbox: std::sync::Mutex<Option<[f32; 4]>>,
    }
    impl MaskModel for Stub {
        fn info(&self) -> &'static effectcraft_segment::ModelInfo {
            &effectcraft_segment::MOBILE_SAM
        }
        fn segment(&self, _: &[[f32; 3]], w: usize, h: usize, prompt: &effectcraft_segment::Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *self.bbox.lock().unwrap() = prompt.bbox;
            let Some(([cx, cy], _)) = prompt.points.iter().find(|(_, fg)| *fg) else { return Err("no point".into()) };
            Ok((0..w * h).map(|i| (((i % w) as f32 + 0.5 - cx).hypot((i / w) as f32 + 0.5 - cy) < 15.0) as u8 as f32).collect())
        }
    }

    #[test]
    fn segments_with_the_installed_model_without_a_border_stroke() {
        let p = params(&[("objectPoint", Value::Vec2([40.0, 40.0]))]);
        // With a model, no border stroke: an object touching the frame edge stays whole.
        assert_eq!(prompt_strokes(&p, [100.0, 80.0], 0, true).map(|s| s.len()), Some(1));
        assert_eq!(prompt_strokes(&p, [100.0, 80.0], 0, false).map(|s| s.len()), Some(2));
        // The model's answer is part of the cache key, and drives the matte.
        let stub = Arc::new(Stub::default());
        assert_ne!(seed(&p, [100.0, 80.0], 25.0, Some(stub.as_ref())), seed(&p, [100.0, 80.0], 25.0, None));
        let img = frame_at(0);
        let mut frame = |g: i64| Some(Arc::new(frame_at(g)));
        let FrameMatte::Alpha(a) = frame_matte(&p, [100.0, 80.0], 25.0, 0, 1.0, &img, Some(stub.clone()), &mut frame, &|| false) else { panic!("no matte") };
        assert!(a[40 * 100 + 40] > 0.5 && a[5 * 100 + 5] < 0.5);
        assert_eq!(stub.calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(*stub.bbox.lock().unwrap(), None);
    }

    #[test]
    fn the_box_reaches_the_model_as_a_box_prompt() {
        let p = params(&[
            ("objectPoint", Value::Vec2([90.0, 70.0])),
            ("useBox", Value::Bool(true)),
            ("boxTopLeft", Value::Vec2([100.0, 80.0])),
            ("boxBottomRight", Value::Vec2([74.0, 54.0])),
        ]);
        // With a model: the point and a box stroke (corners ordered); without: the box's edges
        // as a background stroke, as before.
        let with = prompt_strokes(&p, [100.0, 80.0], 0, true).unwrap();
        assert_eq!(with.iter().map(|s| s.kind).collect::<Vec<_>>(), [StrokeKind::Fg, StrokeKind::Box]);
        assert_eq!(with[1].points, [[74.0, 54.0], [100.0, 80.0]]);
        let without = prompt_strokes(&p, [100.0, 80.0], 0, false).unwrap();
        assert_eq!(without.iter().map(|s| s.kind).collect::<Vec<_>>(), [StrokeKind::Fg, StrokeKind::Bg]);
        // The model is called with the box itself (matte pixels at scale 1) and only one
        // foreground point: no background points along the edges.
        let stub = Arc::new(Stub::default());
        let img = frame_at(0);
        let mut frame = |g: i64| Some(Arc::new(frame_at(g)));
        let FrameMatte::Alpha(a) = frame_matte(&p, [100.0, 80.0], 25.0, 0, 1.0, &img, Some(stub.clone()), &mut frame, &|| false) else { panic!("no matte") };
        assert_eq!(*stub.bbox.lock().unwrap(), Some([74.0, 54.0, 100.0, 80.0]));
        assert!(a[70 * 100 + 90] > 0.5 && a[40 * 100 + 40] < 0.5);
        // At half resolution the box is scaled with the frame (another Input Key, so the
        // full-resolution result above is not reused).
        let mut p = p;
        p.values.insert(INPUT_KEY.into(), Value::Str("half".into()));
        let half = effectcraft_raster::resample(&img, 50, 40);
        let mut frame = |g: i64| Some(Arc::new(effectcraft_raster::resample(&frame_at(g), 50, 40)));
        let _ = frame_matte(&p, [100.0, 80.0], 25.0, 0, 0.5, &half, Some(stub.clone()), &mut frame, &|| false);
        assert_eq!(*stub.bbox.lock().unwrap(), Some([37.0, 27.0, 50.0, 40.0]));
    }

    #[test]
    fn job_frames_go_outward_from_the_reference_frame_inside_the_layer() {
        let p = params(&[
            ("objectTracking/referenceFrame", Value::Scalar(5.0)),
            ("objectTracking/framesBefore", Value::Scalar(2.0)),
            ("objectTracking/framesAfter", Value::Scalar(3.0)),
        ]);
        assert_eq!(job_frames(&p, [0, 100]), [5, 6, 7, 8, 4, 3]);
        assert_eq!(job_frames(&p, [4, 6]), [5, 6, 4]);
        assert!(job_frames(&p, [10, 20]).is_empty());
        let huge = params(&[("objectTracking/referenceFrame", Value::Scalar(1e300)), ("objectTracking/framesAfter", Value::Scalar(f64::INFINITY))]);
        assert!(job_frames(&huge, [0, 100]).is_empty());
    }

    /// Frozen mattes are used as stored while their key matches (no segmentation: a stored
    /// pattern the segmentation could never produce comes out), and ignored once it doesn't.
    #[test]
    fn frozen_mattes_are_used_while_their_key_matches() {
        let mut p = params(&[("objectPoint", Value::Vec2([40.0, 40.0])), ("objectTracking/framesAfter", Value::Scalar(3.0))]);
        p.values.insert(INPUT_KEY.into(), Value::Str("frozen-test".into()));
        // Left half opaque on frame 2.
        let alpha: Vec<f32> = (0..100 * 80).map(|i| ((i % 100) < 50) as u8 as f32).collect();
        let mut fc = FrozenCache { key: frozen_key(&p, [100.0, 80.0], 25.0, None), size: [100, 80], ..Default::default() };
        fc.frames.insert(2, roto::freeze_frame(&alpha, &[]));
        p.values.insert(FROZEN.into(), Value::Str(fc.to_json()));
        assert!(is_frozen(&p));
        let out = run(&p, 2);
        assert!(out.data[40 * 100 + 10][3] > 0.99 && out.data[40 * 100 + 90][3] < 0.01, "the frozen matte is used");
        // At half resolution too.
        assert_eq!(frozen_alpha(&p, [100.0, 80.0], 25.0, 2, 50, 40, None).map(|a| (a[20 * 50 + 5], a[20 * 50 + 45])), Some((1.0, 0.0)));
        // A frame without a frozen matte is computed (the disk).
        assert!(wrong(&run(&p, 1), 42.0) < 30);
        // Another prompt: the frozen mattes no longer apply.
        p.values.insert("objectPoint".into(), Value::Vec2([41.0, 40.0]));
        assert_eq!(frozen_alpha(&p, [100.0, 80.0], 25.0, 2, 100, 80, None), None);
        assert!(wrong(&run(&p, 2), 44.0) < 30);
        // Damaged frozen data is ignored, not a crash.
        p.values.insert(FROZEN.into(), Value::Str("{\"key\": 3, \"frames\": [".into()));
        assert_eq!(frozen_alpha(&p, [100.0, 80.0], 25.0, 2, 100, 80, None), None);
        let mut fc2 = fc.clone();
        fc2.size = [usize::MAX, 2];
        p.values.insert("objectPoint".into(), Value::Vec2([40.0, 40.0]));
        p.values.insert(FROZEN.into(), Value::Str(fc2.to_json()));
        assert_eq!(frozen_alpha(&p, [100.0, 80.0], 25.0, 2, 100, 80, None), None);
    }
}
