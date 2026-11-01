//! Warp Stabilizer (Effect ▸ Distort ▸ Warp Stabilizer).
//!
//! The analysis (feature tracking and per-frame motion fits) runs as a background job in the
//! engine (`warp.analyze`) and is stored, as JSON, in the instance's hidden **Analysis**
//! parameter ([`effectcraft_track::stabilize::WarpAnalysis`]); **Analysis Key** records what
//! the analysis was made from (the layer's source, in/out points and time mapping) so edits that
//! change the frames invalidate it. Rendering derives the stabilization plan from the analysis
//! and the Stabilization / Borders / Advanced settings ([`effectcraft_track::stabilize::plan`],
//! cached per analysis and settings) and warps each frame by its corrective transform:
//!
//! - **Stabilize Only** shows the moving frame edges, **Stabilize, Crop** crops to the region
//!   valid on every frame, **Stabilize, Crop, Auto-scale** scales that region back up to the
//!   frame (up to Maximum Scale, minus the Action-safe Margin), Additional Scale scales on top;
//! - **Stabilize, Synthesize Edges** fills the uncovered borders from neighbouring frames
//!   (within Synthesis Input Range), mapped through the analysed frame-to-frame motion and read
//!   with [`crate::EffectHost::self_at`]; Synthesis Edge Feather blends the seam and Synthesis
//!   Edge Cropping ignores bad edge pixels of the input frames;
//! - **Show Track Points** draws the analysed background features.
//!
//! *Subspace Warp* warps each frame with a content-preserving mesh fitted to the analysis'
//! subspace-smoothed feature trajectories ([`effectcraft_track::subspace`]); analyses made
//! before trajectories were stored fall back to a perspective warp. *Rolling Shutter Ripple*
//! picks the mesh: Automatic Reduction (12 × 8, stiffer) or Enhanced Reduction (twice the rows,
//! softer), which follows the row-wise wobble of rolling-shutter shake more closely.
//!
//! Without an analysis (or with one made for another layer size) the input passes through.

use std::collections::HashMap;
use std::hash::Hasher;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use effectcraft_track::Homography;
use effectcraft_track::stabilize::{Framing, Method, Plan, Ripple, StabResult, StabSettings, WarpAnalysis, plan};
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, Params, num, p, popup, slider};

/// The effect's id.
pub const ID: &str = "ec.distort.warpstabilizer";

/// Hidden parameters: the stored analysis (JSON) and its key (see the module docs).
pub const ANALYSIS: &str = "analysis";
pub const ANALYSIS_KEY: &str = "analysisKey";

/// Display names of the instance's parameter groups (by match id).
pub const GROUPS: &[(&str, &str)] = &[
    ("stabilization", "Stabilization"),
    ("borders", "Borders"),
    ("autoScale", "Auto-scale"),
    ("advanced", "Advanced"),
    ("edgeCropping", "Synthesis Edge Cropping"),
];

pub const RESULTS: [&str; 2] = ["Smooth Motion", "No Motion"];
pub const METHODS: [&str; 4] = ["Position", "Position, Scale, Rotation", "Perspective", "Subspace Warp"];
pub const FRAMINGS: [&str; 4] = ["Stabilize Only", "Stabilize, Crop", "Stabilize, Crop, Auto-scale", "Stabilize, Synthesize Edges"];

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "Warp Stabilizer",
        category: "Distort",
        params: vec![
            p("stabilization/result", "Result", Value::Enum(0), popup(&RESULTS)),
            p("stabilization/smoothness", "Smoothness", num(50.0), slider(0.0, 1000.0, 0.0, 100.0, 0)),
            p("stabilization/method", "Method", Value::Enum(3), popup(&METHODS)),
            p("stabilization/preserveScale", "Preserve Scale", Value::Bool(false), ParamUi::Checkbox),
            p("borders/framing", "Framing", Value::Enum(2), popup(&FRAMINGS)),
            p("borders/autoScale/maximumScale", "Maximum Scale", num(150.0), slider(100.0, 1000.0, 100.0, 200.0, 0)),
            p("borders/autoScale/actionSafeMargin", "Action-safe Margin", num(0.0), slider(0.0, 45.0, 0.0, 20.0, 0)),
            p("borders/additionalScale", "Additional Scale", num(100.0), slider(1.0, 1000.0, 50.0, 150.0, 0)),
            p("advanced/detailedAnalysis", "Detailed Analysis", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/rollingShutterRipple", "Rolling Shutter Ripple", Value::Enum(0), popup(&["Automatic Reduction", "Enhanced Reduction"])),
            p("advanced/cropLessSmoothMore", "Crop Less <-> Smooth More", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("advanced/synthesizeInputRange", "Synthesis Input Range (seconds)", num(1.0), slider(0.0, 10.0, 0.0, 5.0, 2)),
            p("advanced/synthesizeEdgeFeather", "Synthesis Edge Feather", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("advanced/edgeCropping/left", "Left", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 0)),
            p("advanced/edgeCropping/top", "Top", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 0)),
            p("advanced/edgeCropping/right", "Right", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 0)),
            p("advanced/edgeCropping/bottom", "Bottom", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 0)),
            p("advanced/hideWarningBanner", "Hide Warning Banner", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/showTrackPoints", "Show Track Points", Value::Bool(false), ParamUi::Checkbox),
            p("advanced/trackPointSize", "Track Point Size", num(100.0), slider(0.0, 1000.0, 0.0, 200.0, 0)),
            p(ANALYSIS, "Analysis", Value::Str(String::new()), ParamUi::Hidden),
            p(ANALYSIS_KEY, "Analysis Key", Value::Str(String::new()), ParamUi::Hidden),
        ],
        render,
        gpu: false,
        float: true,
    }]
}

/// A parameter by its spec id (`stabilization/smoothness`), from either flattened instance
/// params (nested groups become `#n/` prefixes, see [`crate::flatten_params`]) or plain ids.
pub fn param<'a>(params: &'a Params, id: &str) -> Option<&'a Value> {
    if let Some(v) = params.get(id) {
        return Some(v);
    }
    let mut pre = String::new();
    let segs: Vec<&str> = id.split('/').collect();
    let (last, groups) = segs.split_last()?;
    for g in groups {
        pre = params.group(&pre, g)?;
    }
    params.get(&format!("{pre}{last}"))
}

fn f(params: &Params, id: &str) -> f64 {
    param(params, id).map(Value::as_f64).unwrap_or(0.0)
}
fn e(params: &Params, id: &str) -> u32 {
    param(params, id).map(Value::as_enum).unwrap_or(0)
}
fn b(params: &Params, id: &str) -> bool {
    param(params, id).map(Value::as_bool).unwrap_or(false)
}
fn s<'a>(params: &'a Params, id: &str) -> &'a str {
    match param(params, id) {
        Some(Value::Str(s)) => s,
        _ => "",
    }
}

/// Stabilization settings from the instance's parameters (`fps`: the analysis' frame rate).
pub fn settings(params: &Params, fps: f64) -> StabSettings {
    StabSettings {
        result: if e(params, "stabilization/result") == 1 { StabResult::NoMotion } else { StabResult::SmoothMotion },
        smoothness: f(params, "stabilization/smoothness"),
        method: match e(params, "stabilization/method") {
            0 => Method::Position,
            1 => Method::Similarity,
            2 => Method::Perspective,
            _ => Method::SubspaceWarp,
        },
        preserve_scale: b(params, "stabilization/preserveScale"),
        framing: match e(params, "borders/framing") {
            0 => Framing::StabilizeOnly,
            1 => Framing::StabilizeCrop,
            3 => Framing::SynthesizeEdges,
            _ => Framing::StabilizeCropAutoScale,
        },
        max_scale: f(params, "borders/autoScale/maximumScale"),
        action_safe: f(params, "borders/autoScale/actionSafeMargin"),
        additional_scale: f(params, "borders/additionalScale"),
        // AE's 0…100 slider (50 = balanced) onto the planner's −100…100.
        crop_less_smooth_more: (f(params, "advanced/cropLessSmoothMore") - 50.0) * 2.0,
        fps,
        ripple: if e(params, "advanced/rollingShutterRipple") == 1 { Ripple::Enhanced } else { Ripple::Automatic },
    }
}

struct KeyHasher(u64);
impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn settings_key(s: &StabSettings) -> u64 {
    let mut h = KeyHasher(0x9e37_79b9_7f4a_7c15);
    for v in [s.smoothness, s.max_scale, s.action_safe, s.additional_scale, s.crop_less_smooth_more, s.fps] {
        h.write_u64(v.to_bits());
    }
    h.write(format!("{:?}{:?}{:?}{:?}{:?}", s.result, s.method, s.preserve_scale, s.framing, s.ripple).as_bytes());
    h.finish()
}

type PlanCache = Mutex<HashMap<(u64, u64), Arc<Plan>>>;

/// Parsed analyses and their content keys; cache hits compare source text without rehashing.
fn analysis_cached(json: &str) -> Option<(u64, Arc<WarpAnalysis>)> {
    static C: crate::analysis_cache::AnalysisCache<WarpAnalysis> = crate::analysis_cache::AnalysisCache::new();
    C.get(json, WarpAnalysis::from_json)
}

fn plans() -> &'static PlanCache {
    static C: OnceLock<PlanCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn plan_cached(akey: u64, a: &WarpAnalysis, s: &StabSettings) -> Arc<Plan> {
    let key = (akey, settings_key(s));
    let c = plans();
    if let Some(p) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return p.clone();
    }
    let p = Arc::new(plan(a, s));
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 32 {
        m.clear();
    }
    m.insert(key, p.clone());
    p
}

/// Validate and cache the default stabilization plan after analysis. Use the serialized
/// analysis so the cached plan has exactly the same inputs as the first render, including
/// any floating-point round trip through JSON. Other settings still get their own plan.
pub fn prepare_default_plan(json: &str) -> bool {
    let Some((key, a)) = analysis_cached(json) else { return false };
    if a.is_empty() {
        return false;
    }
    let fps = if a.frame_duration > 0.0 { 1.0 / a.frame_duration } else { 30.0 };
    let p = plan_cached(key, &a, &StabSettings { fps, ..Default::default() });
    p.warps.len() == a.frames.len() && p.warps.iter().all(|h| h.0.iter().flatten().all(|v| v.is_finite()))
}

/// The stored analysis of an instance (`None` when not analysed).
pub fn analysis(params: &Params) -> Option<Arc<WarpAnalysis>> {
    let j = s(params, ANALYSIS);
    if j.is_empty() {
        return None;
    }
    analysis_cached(j).map(|(_, a)| a).filter(|a| !a.is_empty())
}

/// The stabilization of an instance at layer time `time`: (frame index, plan).
pub fn plan_at(params: &Params, time: f64) -> Option<(usize, Arc<WarpAnalysis>, Arc<Plan>)> {
    let j = s(params, ANALYSIS);
    if j.is_empty() {
        return None;
    }
    let (key, a) = analysis_cached(j)?;
    let k = a.frame_at(time)?;
    let fps = if a.frame_duration > 0.0 { 1.0 / a.frame_duration } else { 30.0 };
    let pl = plan_cached(key, &a, &settings(params, fps));
    (k < pl.warps.len()).then_some((k, a, pl))
}

/// What `warp.status` reports of a stabilization plan (no meshes): small, so another engine
/// instance (a browser job worker) can compute it and send it over.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSummary {
    /// [`Plan::warps`] (row-major 3 × 3 per frame).
    pub warps: Vec<[[f64; 3]; 3]>,
    pub auto_scale: f64,
    pub crop: Option<[f64; 4]>,
    pub valid_fraction: f64,
}

impl PlanSummary {
    fn of(p: &Plan) -> PlanSummary {
        PlanSummary { warps: p.warps.iter().map(|h| h.0).collect(), auto_scale: p.auto_scale, crop: p.crop, valid_fraction: p.valid_fraction }
    }
}

type SummaryCache = Mutex<HashMap<(u64, u64), Arc<PlanSummary>>>;

fn summaries() -> &'static SummaryCache {
    static C: OnceLock<SummaryCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The key of an instance's plan (analysis, settings), with the analysis (`None` when not
/// analysed). Cheap once the analysis is parsed: no plan is computed.
fn plan_key(params: &Params) -> Option<((u64, u64), Arc<WarpAnalysis>, StabSettings)> {
    let j = s(params, ANALYSIS);
    if j.is_empty() {
        return None;
    }
    let (akey, a) = analysis_cached(j)?;
    let fps = if a.frame_duration > 0.0 { 1.0 / a.frame_duration } else { 30.0 };
    let st = settings(params, fps);
    Some(((akey, settings_key(&st)), a, st))
}

/// The key a [`PlanSummary`] of this instance is stored under ([`store_summary`]).
pub fn summary_key(params: &Params) -> Option<[u64; 2]> {
    plan_key(params).map(|(k, _, _)| [k.0, k.1])
}

/// Keep a summary computed elsewhere (a job worker), so [`cached_summary_at`] finds it.
pub fn store_summary(key: [u64; 2], summary: PlanSummary) {
    keep_summary((key[0], key[1]), Arc::new(summary));
}

fn keep_summary(key: (u64, u64), summary: Arc<PlanSummary>) {
    let mut m = summaries().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if m.len() > 32 {
        m.clear();
    }
    m.insert(key, summary);
}

/// The plan summary of an instance (its key and the summary), computing the plan when it isn't
/// cached (seconds for a long Subspace Warp clip). `None` when not analysed.
pub fn summary(params: &Params) -> Option<([u64; 2], Arc<PlanSummary>)> {
    let (key, a, st) = plan_key(params)?;
    let cached = summaries().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).cloned();
    let sum = match cached {
        Some(s) => s,
        None => {
            let s = Arc::new(PlanSummary::of(&plan_cached(key.0, &a, &st)));
            keep_summary(key, s.clone());
            s
        }
    };
    Some(([key.0, key.1], sum))
}

/// The plan summary of an instance at layer time `time`: (frame index, summary). Computes the
/// plan when it isn't cached ([`summary`]).
pub fn summary_at(params: &Params, time: f64) -> Option<(usize, Arc<PlanSummary>)> {
    if let Some(r) = cached_summary_at(params, time) {
        return Some(r);
    }
    let (_, sum) = summary(params)?;
    let k = analysis(params)?.frame_at(time)?;
    (k < sum.warps.len()).then_some((k, sum))
}

/// [`summary_at`] without computing anything: from a stored summary or a cached plan; `None`
/// when neither exists yet (or the instance is not analysed).
pub fn cached_summary_at(params: &Params, time: f64) -> Option<(usize, Arc<PlanSummary>)> {
    let (key, a, _) = plan_key(params)?;
    let k = a.frame_at(time)?;
    let sum = summaries().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key).cloned();
    let sum = match sum {
        Some(s) => s,
        None => {
            let p = plans().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()?;
            let s = Arc::new(PlanSummary::of(&p));
            keep_summary(key, s.clone());
            s
        }
    };
    (k < sum.warps.len()).then_some((k, sum))
}

/// Motion from frame `k` to frame `j` (scene points of frame k → frame j), from the analysed
/// frame-to-frame homographies.
pub fn motion_between(a: &WarpAnalysis, k: usize, j: usize) -> Homography {
    let mut m = Homography::IDENTITY;
    if j > k {
        for i in k + 1..=j {
            m = a.frames[i].motion(Method::Perspective).then_after(&m);
        }
    } else {
        for i in (j + 1..=k).rev() {
            let inv = a.frames[i].motion(Method::Perspective).inverse().unwrap_or(Homography::IDENTITY);
            m = inv.then_after(&m);
        }
    }
    m
}

fn inside(r: &[f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

/// Distance from `p` to the nearest edge of `r` (positive inside).
fn inset(r: &[f64; 4], p: [f64; 2]) -> f64 {
    (p[0] - r[0]).min(r[2] - p[0]).min(p[1] - r[1]).min(r[3] - p[1])
}

fn lerp_px(a: Px, b: Px, t: f32) -> Px {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

fn render(ctx: &EffectCtx, buf: Buf) -> Buf {
    let Some((k, a, pl)) = plan_at(ctx.params, ctx.time) else { return buf };
    let size = a.size;
    if (size[0] - ctx.layer_size[0]).abs() > 0.5 || (size[1] - ctx.layer_size[1]).abs() > 0.5 || ctx.adjustment {
        return buf;
    }
    let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    let Some(inv) = pl.warps[k].inverse() else { return buf };
    // Output point → source point of this frame (a mesh lookup for Subspace Warp).
    let mesh = pl.meshes.get(k);
    let finv = pl.framing.inverse().unwrap_or(Homography::IDENTITY);
    let src_of = |p: [f64; 2]| match mesh {
        Some(m) => m.sample(finv.apply(p)),
        None => inv.apply(p),
    };
    let (ow, oh) = (((size[0] * sc).ceil() as u32).max(1), ((size[1] * sc).ceil() as u32).max(1));
    let synth = matches!(settings(ctx.params, 30.0).framing, Framing::SynthesizeEdges);
    let crop = [
        f(ctx.params, "advanced/edgeCropping/left").max(0.0),
        f(ctx.params, "advanced/edgeCropping/top").max(0.0),
        size[0] - f(ctx.params, "advanced/edgeCropping/right").max(0.0),
        size[1] - f(ctx.params, "advanced/edgeCropping/bottom").max(0.0),
    ];
    let valid = if synth { crop } else { [0.0, 0.0, size[0], size[1]] };
    let feather = if synth { f(ctx.params, "advanced/synthesizeEdgeFeather").max(0.0) } else { 0.0 };
    let src = &buf;
    // Inside the frame, edge pixels extend to the frame's edge; outside it is transparent.
    let sample = |b: &Buf, q: [f64; 2]| {
        if q[0] < 0.0 || q[1] < 0.0 || q[0] > size[0] || q[1] > size[1] {
            return [0.0; 4];
        }
        b.img.sample_bilinear_clamped(q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1])
    };
    let mut out = Image::new(ow, oh);
    // Source position (frame k layer pixels) of every output pixel, and how much of the pixel
    // still needs synthesizing (0 = none).
    let need_m = Mutex::new(if synth { vec![0.0f32; (ow * oh) as usize] } else { vec![] });
    out.rows_mut().for_each(|(y, row)| {
        let mut local = vec![];
        for (x, px) in row.iter_mut().enumerate() {
            let p = [(x as f64 + 0.5) / sc, (y as f64 + 0.5) / sc];
            if let Some(c) = &pl.crop
                && !inside(c, p)
            {
                continue;
            }
            let q = src_of(p);
            if !synth {
                *px = sample(src, q);
                continue;
            }
            let d = inset(&valid, q);
            if d >= feather.max(0.5) {
                *px = sample(src, q);
            } else {
                let w = if d <= 0.0 {
                    1.0
                } else if feather > 0.0 {
                    (1.0 - d / feather) as f32
                } else {
                    1.0
                };
                if d > 0.0 {
                    *px = sample(src, q);
                }
                local.push((x, w));
            }
        }
        if !local.is_empty() {
            let mut n = need_m.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (x, w) in local {
                n[y * ow as usize + x] = w;
            }
        }
    });
    let need = need_m.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    if synth && need.iter().any(|w| *w > 0.0) {
        synthesize(ctx, &a, k, &SynthesisGrid { src_of: &src_of, crop: &crop, scale: sc }, &mut out, &need);
    }
    if b(ctx.params, "advanced/showTrackPoints") {
        let r = (2.5 * f(ctx.params, "advanced/trackPointSize") / 100.0 * sc).max(0.5);
        for q in &a.frames[k].points {
            let Some(o) = pl.output_of(k, [q[0] as f64, q[1] as f64]) else { continue };
            dot(&mut out, [o[0] * sc, o[1] * sc], r, [0.25, 0.75, 1.0, 1.0]);
        }
    }
    Buf { img: out, offset: [0.0; 2], scale: sc }
}

/// Maps an output sample into the uncropped source grid shared by neighbouring frames.
struct SynthesisGrid<'a> {
    src_of: &'a (dyn Fn([f64; 2]) -> [f64; 2] + Sync),
    crop: &'a [f64; 4],
    scale: f64,
}

/// Fill the pixels flagged in `need` from neighbouring frames, nearest first.
fn synthesize(ctx: &EffectCtx, a: &WarpAnalysis, k: usize, grid: &SynthesisGrid<'_>, out: &mut Image, need: &[f32]) {
    let SynthesisGrid { src_of, crop, scale: sc } = *grid;
    let Some(host) = ctx.env.host else { return };
    let n = a.frames.len();
    let range = ((f(ctx.params, "advanced/synthesizeInputRange").max(0.0) / a.frame_duration.max(1e-6)).round() as usize).min(60);
    let ow = out.width as usize;
    let mut filled = vec![false; need.len()];
    let todo: Vec<usize> = (0..need.len()).filter(|i| need[*i] > 0.0).collect();
    let mut left = todo.len();
    for d in 1..=range {
        if left == 0 {
            break;
        }
        for j in [k as i64 + d as i64, k as i64 - d as i64] {
            if j < 0 || j as usize >= n || left == 0 {
                continue;
            }
            let j = j as usize;
            let Some(nb) = host.self_at(a.start + j as f64 * a.frame_duration, ctx.env.effect_index) else { continue };
            let m = motion_between(a, k, j);
            let hits: Vec<(usize, Px)> = todo
                .par_iter()
                .filter(|i| !filled[**i])
                .filter_map(|i| {
                    let (x, y) = (i % ow, i / ow);
                    let q = src_of([(x as f64 + 0.5) / sc, (y as f64 + 0.5) / sc]);
                    let qj = m.apply(q);
                    inside(crop, qj).then(|| (*i, nb.img.sample_bilinear_clamped(qj[0] * nb.scale + nb.offset[0], qj[1] * nb.scale + nb.offset[1])))
                })
                .collect();
            for (i, px) in hits {
                filled[i] = true;
                left -= 1;
                out.data[i] = lerp_px(out.data[i], px, need[i]);
            }
        }
    }
}

fn dot(img: &mut Image, c: [f64; 2], r: f64, col: Px) {
    let (x0, x1) = ((c[0] - r - 1.0).floor().max(0.0) as i64, (c[0] + r + 1.0).ceil() as i64);
    let (y0, y1) = ((c[1] - r - 1.0).floor().max(0.0) as i64, (c[1] + r + 1.0).ceil() as i64);
    for y in y0..y1.min(img.height as i64) {
        for x in x0..x1.min(img.width as i64) {
            let d = ((x as f64 + 0.5 - c[0]).hypot(y as f64 + 0.5 - c[1]) - r).clamp(-0.5, 0.5);
            let cov = (0.5 - d) as f32;
            if cov > 0.0 {
                let i = img.idx(x as u32, y as u32);
                img.data[i] = lerp_px(img.data[i], col, cov);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EffectEnv;
    use crate::analysis_cache::fnv;
    use effectcraft_track::stabilize::FrameMotion;

    fn analysis_json(n: usize, w: f64, h: f64, shift: impl Fn(usize) -> [f64; 2]) -> String {
        let mut a = WarpAnalysis { version: 1, start: 0.0, frame_duration: 1.0 / 25.0, size: [w, h], detailed: false, frames: vec![], tracks: vec![] };
        for k in 0..n {
            let mut fm = FrameMotion::default();
            if k > 0 {
                let (p, q) = (shift(k - 1), shift(k));
                let d = [q[0] - p[0], q[1] - p[1]];
                fm.t = d;
                fm.s = [1.0, 0.0, d[0], d[1]];
                fm.h = [1.0, 0.0, d[0], 0.0, 1.0, d[1], 0.0, 0.0];
            }
            a.frames.push(fm);
        }
        a.to_json()
    }

    fn params(json: &str, framing: u32, result: u32) -> Params {
        let s = &specs()[0];
        let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        p.values.insert(ANALYSIS.into(), Value::Str(json.into()));
        p.values.insert("borders/framing".into(), Value::Enum(framing));
        p.values.insert("stabilization/result".into(), Value::Enum(result));
        p
    }

    /// A white dot at `c` on black.
    fn frame(c: [f64; 2]) -> Image {
        let mut im = Image::filled(64, 48, [0.0, 0.0, 0.0, 1.0]);
        dot(&mut im, c, 3.0, [1.0; 4]);
        im
    }

    fn centroid(im: &Image) -> [f64; 2] {
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        for y in 0..im.height {
            for x in 0..im.width {
                let v = im.get(x as i64, y as i64)[0] as f64;
                sx += v * (x as f64 + 0.5);
                sy += v * (y as f64 + 0.5);
                sw += v;
            }
        }
        [sx / sw, sy / sw]
    }

    #[test]
    fn no_motion_locks_the_scene_and_unanalysed_passes_through() {
        let shift = |k: usize| [((k * 7) % 5) as f64 - 2.0, ((k * 3) % 4) as f64 - 1.5];
        let json = analysis_json(10, 64.0, 48.0, shift);
        let ps = params(&json, 0, 1);
        let mut cs = vec![];
        for k in 0..10 {
            let d = shift(k);
            let img = frame([32.0 + d[0], 24.0 + d[1]]);
            let ctx = EffectCtx { params: &ps, time: k as f64 / 25.0, layer_size: [64.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() };
            let out = render(&ctx, Buf { img, offset: [0.0; 2], scale: 1.0 });
            cs.push(centroid(&out.img));
        }
        for c in &cs {
            assert!((c[0] - cs[0][0]).abs() < 0.05 && (c[1] - cs[0][1]).abs() < 0.05, "{cs:?}");
        }
        // Not analysed: untouched.
        let ps = params("", 0, 1);
        let img = frame([10.0, 10.0]);
        let ctx = EffectCtx { params: &ps, time: 0.0, layer_size: [64.0, 48.0], seed: 1, adjustment: false, env: EffectEnv::default() };
        assert_eq!(render(&ctx, Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 }).img.data, img.data);
    }

    #[test]
    fn prepared_default_plan_preserves_first_render_and_is_reused() {
        let json = analysis_json(25, 64.0, 48.0, |k| [((k * 7) % 5) as f64 - 2.0, k as f64 * 0.1]);
        let mut a = WarpAnalysis::from_json(&json).unwrap();
        a.start = 0.012345678901234567;
        a.tracks = (0..24)
            .map(|id| effectcraft_track::camtrack::Track2D {
                id,
                start: 0,
                pts: (0..25).map(|k| [8.0 + (id % 6) as f32 * 8.0 + ((k * 7) % 5) as f32, 8.0 + (id / 6) as f32 * 8.0 + k as f32 * 0.1]).collect(),
            })
            .collect();
        let json = a.to_json();
        let ps = params(&json, 2, 0);
        let ctx = EffectCtx { params: &ps, time: a.start, layer_size: a.size, seed: 1, adjustment: false, env: EffectEnv::default() };
        let input = Buf { img: frame([30.0, 22.0]), offset: [0.0; 2], scale: 1.0 };
        // Original flow: validate, discard the plan, then render from the stored analysis.
        let _discarded = plan(&a, &StabSettings { fps: 25.0, ..Default::default() });
        let before = render(&ctx, input.clone());
        let key = (fnv(json.as_bytes()), settings_key(&settings(&ps, 25.0)));
        plans().lock().unwrap().remove(&key);
        assert!(prepare_default_plan(&json));
        let prepared = plans().lock().unwrap().get(&key).unwrap().clone();
        assert!(!prepared.meshes.is_empty(), "exercise the Subspace Warp plan");
        let after = render(&ctx, input);
        let (_, _, rendered) = plan_at(&ps, a.start).unwrap();
        assert!(Arc::ptr_eq(&prepared, &rendered));
        assert_eq!(before.offset, after.offset);
        assert_eq!(before.scale.to_bits(), after.scale.to_bits());
        for (a, b) in before.img.data.iter().flatten().zip(after.img.data.iter().flatten()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert!(!prepare_default_plan("invalid"));
        assert!(!prepare_default_plan(&WarpAnalysis::default().to_json()));
    }

    #[test]
    fn changing_analysis_changes_the_plan_key() {
        let json = analysis_json(10, 64.0, 48.0, |k| [k as f64 * 0.1, 0.0]);
        let mut ps = params(&json, 2, 0);
        let before = summary_key(&ps).unwrap();
        assert_eq!(before[0], fnv(json.as_bytes()));
        let mut a = WarpAnalysis::from_json(&json).unwrap();
        a.start = 1.0;
        ps.values.insert(ANALYSIS.into(), Value::Str(a.to_json()));
        let after = summary_key(&ps).unwrap();
        assert_ne!(before[0], after[0]);
        assert_eq!(before[1], after[1]);
        assert_eq!(*analysis(&ps).unwrap(), a);
        ps.values.insert(ANALYSIS.into(), Value::Str(json));
        assert_eq!(summary_key(&ps).unwrap(), before);
        ps.values.insert(ANALYSIS.into(), Value::Str(String::new()));
        assert!(analysis(&ps).is_none());
    }

    #[test]
    fn nested_param_lookup() {
        let mut p = Params::default();
        p.values.insert("#2/@match".into(), Value::Str("borders".into()));
        p.values.insert("#1/@match".into(), Value::Str("stabilization".into()));
        p.values.insert("#2/#1/@match".into(), Value::Str("autoScale".into()));
        p.values.insert("#2/#1/maximumScale".into(), num(120.0));
        p.values.insert("#1/smoothness".into(), num(12.0));
        assert_eq!(f(&p, "borders/autoScale/maximumScale"), 120.0);
        assert_eq!(f(&p, "stabilization/smoothness"), 12.0);
    }
}
