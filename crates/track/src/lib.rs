//! Motion tracking, clean-room from textbook methods:
//!
//! - each **track point** has a *feature region* (the pattern to follow) and a *search region*
//!   (where to look for it in the next frame), as in After Effects' Tracker;
//! - the pattern is located by **normalized cross-correlation** (Lewis, "Fast Normalized
//!   Cross-Correlation", 1995) searched **coarse to fine over an image pyramid**;
//! - the integer match is refined to **sub-pixel** precision (and, optionally, the feature's
//!   rotation and scale) by **Lucas–Kanade** Gauss–Newton image alignment (Lucas & Kanade 1981;
//!   Baker & Matthews, "Lucas-Kanade 20 Years On", 2004) with a gain/bias photometric model;
//! - **confidence** is the correlation of the template with the matched region, in percent;
//!   when it drops below a threshold the tracker continues, stops, extrapolates the motion or
//!   adapts the feature (re-samples the template), like AE's Motion Tracker Options.
//!
//! Only the pixels around each search region are converted and pre-processed, so tracking cost is
//! independent of the frame size. Points are tracked in parallel and the coarse correlation search
//! is parallel over candidate rows (rayon).
//!
//! Coordinates are *layer* pixels: the frame image's pixel `(i, j)` covers `[i, i+1) × [j, j+1)`
//! after subtracting the frame's `offset` (see [`Frame`]).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod camtrack;
pub mod face;
pub mod fit;
pub mod klt;
pub mod mask;
pub mod plane;
pub mod roto;
pub mod solve;
pub mod stabilize;
pub mod subspace;

use effectcraft_raster::Image;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub use plane::{Channel, Plane};
pub use solve::{Homography, angle_and_length, unwrap_degrees};

/// What to do when a frame's confidence is below the threshold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfidenceAction {
    /// Keep the match anyway.
    Continue,
    /// Stop the analysis before this frame.
    Stop,
    /// Ignore the match and continue the previous motion.
    Extrapolate,
    /// Keep the match and re-sample the feature from this frame.
    #[default]
    Adapt,
}

impl ConfidenceAction {
    pub const ALL: [ConfidenceAction; 4] = [ConfidenceAction::Continue, ConfidenceAction::Stop, ConfidenceAction::Extrapolate, ConfidenceAction::Adapt];
    pub fn label(self) -> &'static str {
        match self {
            ConfidenceAction::Continue => "Continue Tracking",
            ConfidenceAction::Stop => "Stop Tracking",
            ConfidenceAction::Extrapolate => "Extrapolate Motion",
            ConfidenceAction::Adapt => "Adapt Feature",
        }
    }
    pub fn from_name(s: &str) -> Option<ConfidenceAction> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        ConfidenceAction::ALL.into_iter().find(|a| {
            format!("{a:?}").to_ascii_lowercase() == n
                || a.label().to_ascii_lowercase().replace(' ', "") == n
                || a.label().to_ascii_lowercase().replace(' ', "").starts_with(&n)
        })
    }
}

/// Motion Tracker Options that affect matching.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackOptions {
    pub channel: Channel,
    /// Pre-process blur radius in pixels (0 = off).
    pub blur: f64,
    /// Pre-process edge enhancement.
    pub enhance: bool,
    /// Sub-pixel positioning (else whole-pixel matches).
    pub subpixel: bool,
    /// Re-sample the feature on every frame.
    pub adapt_every_frame: bool,
    /// Confidence threshold in percent for `action`.
    pub threshold: f64,
    pub action: ConfidenceAction,
    /// Also estimate each feature's rotation and scale while matching (follows rotating or
    /// zooming features without adapting).
    pub track_shape: bool,
}

impl Default for TrackOptions {
    fn default() -> Self {
        TrackOptions {
            channel: Channel::Luminance,
            blur: 0.0,
            enhance: false,
            subpixel: true,
            adapt_every_frame: false,
            threshold: 80.0,
            action: ConfidenceAction::Adapt,
            track_shape: true,
        }
    }
}

/// One track point's regions (layer pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointSpec {
    pub center: [f64; 2],
    /// Feature region width and height.
    pub feature_size: [f64; 2],
    /// Search region centre relative to the feature centre.
    pub search_offset: [f64; 2],
    /// Search region width and height.
    pub search_size: [f64; 2],
}

/// A frame to track in: layer pixels map to image pixels by adding `offset`.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub img: &'a Image,
    pub offset: [f64; 2],
}

impl<'a> Frame<'a> {
    pub fn new(img: &'a Image) -> Frame<'a> {
        Frame { img, offset: [0.0; 2] }
    }
}

/// How a point's position on a frame was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Tracked,
    /// Tracked, and the feature was re-sampled from this frame.
    Adapted,
    /// Low confidence: the previous motion was continued.
    Extrapolated,
    /// Low confidence with the Stop action: no result for this frame.
    Stopped,
}

/// A point's result on one frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointResult {
    /// Feature centre (layer pixels).
    pub center: [f64; 2],
    /// Match confidence, 0–100 %.
    pub confidence: f64,
    /// Feature rotation (degrees) and scale (1 = start) relative to the start frame (when
    /// `track_shape` is on).
    pub rotation: f64,
    pub scale: f64,
    pub status: Status,
}

/// A template sampled on a unit grid around the feature centre at one pyramid level.
#[derive(Clone, Debug)]
struct Patch {
    r: [i32; 2],
    ch: usize,
    raw: Vec<f32>,
    /// Zero-mean values and their sum of squares.
    zm: Vec<f32>,
    ss: f64,
}

impl Patch {
    fn sample(plane: &Plane, c: [f64; 2], a: [f64; 2], r: [i32; 2]) -> Patch {
        let ch = plane.ch;
        let mut raw = Vec::with_capacity(((2 * r[0] + 1) * (2 * r[1] + 1)) as usize * ch);
        let mut v = [0.0f32; 3];
        for gy in -r[1]..=r[1] {
            for gx in -r[0]..=r[0] {
                let (ux, uy) = (gx as f64, gy as f64);
                plane.sample(c[0] + a[0] * ux - a[1] * uy, c[1] + a[1] * ux + a[0] * uy, &mut v);
                raw.extend_from_slice(&v[..ch]);
            }
        }
        let mean = raw.iter().map(|x| *x as f64).sum::<f64>() / raw.len().max(1) as f64;
        let zm: Vec<f32> = raw.iter().map(|x| (*x as f64 - mean) as f32).collect();
        let ss = zm.iter().map(|x| (*x as f64) * (*x as f64)).sum();
        Patch { r, ch, raw, zm, ss }
    }
}

/// Normalized cross-correlation of `patch` with `plane` sampled at `c + A u` (A = similarity
/// `[[a0, -a1], [a1, a0]]`). Pure translations share one set of bilinear weights.
fn ncc(plane: &Plane, p: &Patch, c: [f64; 2], a: [f64; 2]) -> f64 {
    if p.ss <= 1e-12 {
        return 0.0;
    }
    let ch = p.ch;
    let (mut s, mut s2, mut st) = (0.0f64, 0.0f64, 0.0f64);
    let identity = is_identity(a);
    let fx = c[0] - 0.5;
    let fy = c[1] - 0.5;
    let (bx, by) = (fx.floor(), fy.floor());
    let inside = bx as i64 - p.r[0] as i64 >= 0
        && by as i64 - p.r[1] as i64 >= 0
        && (bx as i64 + p.r[0] as i64 + 1) < plane.w as i64
        && (by as i64 + p.r[1] as i64 + 1) < plane.h as i64;
    if identity && inside {
        let (tx, ty) = ((fx - bx) as f32, (fy - by) as f32);
        let (w00, w10, w01, w11) = ((1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty);
        let row = plane.w * ch;
        let mut k = 0;
        for gy in -p.r[1]..=p.r[1] {
            let y = (by as i64 + gy as i64) as usize;
            let base = y * row;
            for gx in -p.r[0]..=p.r[0] {
                let x = (bx as i64 + gx as i64) as usize;
                let i00 = base + x * ch;
                for cc in 0..ch {
                    let v =
                        w00 * plane.data[i00 + cc] + w10 * plane.data[i00 + ch + cc] + w01 * plane.data[i00 + row + cc] + w11 * plane.data[i00 + row + ch + cc];
                    let v = v as f64;
                    s += v;
                    s2 += v * v;
                    st += v * p.zm[k] as f64;
                    k += 1;
                }
            }
        }
    } else {
        let mut v = [0.0f32; 3];
        let mut k = 0;
        for gy in -p.r[1]..=p.r[1] {
            for gx in -p.r[0]..=p.r[0] {
                let (ux, uy) = (gx as f64, gy as f64);
                plane.sample(c[0] + a[0] * ux - a[1] * uy, c[1] + a[1] * ux + a[0] * uy, &mut v);
                for cc in 0..ch {
                    let x = v[cc] as f64;
                    s += x;
                    s2 += x * x;
                    st += x * p.zm[k] as f64;
                    k += 1;
                }
            }
        }
    }
    let n = p.zm.len() as f64;
    let var = s2 - s * s / n;
    if var <= 1e-12 {
        return 0.0;
    }
    (st / (var * p.ss).sqrt()).clamp(-1.0, 1.0)
}

fn is_identity(a: [f64; 2]) -> bool {
    (a[0] - 1.0).abs() < 1e-9 && a[1].abs() < 1e-9
}

/// The bilinear taps of every template pixel at `c + A u`. Candidates `c + (i, j)` with integer
/// `(i, j)` give each pixel the same fractional offsets, so one table serves them all and a
/// candidate is integer-indexed lerps (the arithmetic of `Plane::sample`) with one bounds check.
struct Taps {
    /// Offset (in samples) of the top-left tap per template pixel from the one at `base`.
    off: Vec<isize>,
    w: Vec<[f32; 2]>,
    base: isize,
    /// Tap bounding box (x0, y0, x1, y1) in plane pixels at the reference, `+1` taps included.
    bb: [isize; 4],
}

impl Taps {
    fn new(plane: &Plane, p: &Patch, c: [f64; 2], a: [f64; 2]) -> Option<Taps> {
        let n = ((2 * p.r[0] + 1) * (2 * p.r[1] + 1)) as usize;
        let (mut off, mut w) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let (rx, ry) = ((c[0] - 0.5).floor(), (c[1] - 0.5).floor());
        if !(rx.abs() < 1e9 && ry.abs() < 1e9) {
            return None;
        }
        let (rx, ry) = (rx as isize, ry as isize);
        let mut bb = [isize::MAX, isize::MAX, isize::MIN, isize::MIN];
        for gy in -p.r[1]..=p.r[1] {
            for gx in -p.r[0]..=p.r[0] {
                let (ux, uy) = (gx as f64, gy as f64);
                let fx = c[0] + a[0] * ux - a[1] * uy - 0.5;
                let fy = c[1] + a[1] * ux + a[0] * uy - 0.5;
                let (x0, y0) = (fx.floor(), fy.floor());
                if !(x0.abs() < 1e9 && y0.abs() < 1e9) {
                    return None;
                }
                let (dx, dy) = (x0 as isize - rx, y0 as isize - ry);
                bb = [bb[0].min(dx), bb[1].min(dy), bb[2].max(dx + 1), bb[3].max(dy + 1)];
                off.push((dy * plane.w as isize + dx) * plane.ch as isize);
                w.push([(fx - x0) as f32, (fy - y0) as f32]);
            }
        }
        let base = (ry * plane.w as isize + rx) * plane.ch as isize;
        Some(Taps { off, w, base, bb: [bb[0] + rx, bb[1] + ry, bb[2] + rx, bb[3] + ry] })
    }

    /// The correlation at `c + (i, j)`, or `None` when a tap leaves the plane.
    fn ncc(&self, plane: &Plane, p: &Patch, i: isize, j: isize) -> Option<f64> {
        if self.bb[0] + i < 0 || self.bb[1] + j < 0 || self.bb[2] + i >= plane.w as isize || self.bb[3] + j >= plane.h as isize {
            return None;
        }
        let ch = p.ch;
        let row = plane.w * ch;
        let b = self.base + (j * plane.w as isize + i) * ch as isize;
        let d = &plane.data;
        let (mut s, mut s2, mut st) = (0.0f64, 0.0f64, 0.0f64);
        let mut k = 0;
        for (o, t) in self.off.iter().zip(&self.w) {
            let i00 = (b + o) as usize;
            let (i10, i01) = (i00 + ch, i00 + row);
            for cc in 0..ch {
                let top = d[i00 + cc] + (d[i10 + cc] - d[i00 + cc]) * t[0];
                let bot = d[i01 + cc] + (d[i01 + ch + cc] - d[i01 + cc]) * t[0];
                let x = (top + (bot - top) * t[1]) as f64;
                s += x;
                s2 += x * x;
                st += x * p.zm[k] as f64;
                k += 1;
            }
        }
        let n = p.zm.len() as f64;
        let var = s2 - s * s / n;
        if var <= 1e-12 {
            return Some(0.0);
        }
        Some((st / (var * p.ss).sqrt()).clamp(-1.0, 1.0))
    }
}

/// `ncc` of one template at integer offsets from a reference centre, through a [`Taps`] table
/// when the shape is not the identity (the identity has its own fast path in `ncc`).
struct Scorer<'a> {
    plane: &'a Plane,
    p: &'a Patch,
    c: [f64; 2],
    a: [f64; 2],
    taps: Option<Taps>,
}

impl<'a> Scorer<'a> {
    fn new(plane: &'a Plane, p: &'a Patch, c: [f64; 2], a: [f64; 2]) -> Scorer<'a> {
        let taps = if is_identity(a) || p.ss <= 1e-12 { None } else { Taps::new(plane, p, c, a) };
        Scorer { plane, p, c, a, taps }
    }

    /// The correlation at the reference centre plus `(i, j)` pixels.
    fn at(&self, i: i32, j: i32) -> f64 {
        self.taps
            .as_ref()
            .and_then(|t| t.ncc(self.plane, self.p, i as isize, j as isize))
            .unwrap_or_else(|| ncc(self.plane, self.p, [self.c[0] + i as f64, self.c[1] + j as f64], self.a))
    }
}

/// The pre-processed pixels around one point: a pyramid of a crop. Plane coordinates are image
/// pixels minus `origin`, divided by `2^level`.
struct Crop {
    origin: [f64; 2],
    levels: Vec<Plane>,
}

impl Crop {
    /// Crop image pixels `[c - ext, c + ext]` (image pixel coordinates), pre-processed.
    fn new(frame: &Frame, opts: &TrackOptions, c: [f64; 2], ext: [f64; 2], levels: usize) -> Crop {
        let sigma = opts.blur.max(0.0) / 2.0;
        let margin = (sigma * 3.0).ceil() + 3.0 + if opts.enhance { 6.0 } else { 0.0 } + (1 << levels) as f64;
        let x0 = (c[0] - ext[0] - margin).floor();
        let y0 = (c[1] - ext[1] - margin).floor();
        let w = ((2.0 * (ext[0] + margin)).ceil() as usize + 2).max(4);
        let h = ((2.0 * (ext[1] + margin)).ceil() as usize + 2).max(4);
        let mut p = Plane::from_image(frame.img, opts.channel, x0 as i64, y0 as i64, w, h);
        if sigma > 0.05 {
            p = p.gaussian(sigma);
        }
        if opts.enhance {
            p = p.enhance();
        }
        let mut lv = vec![p];
        for _ in 0..levels {
            let d = lv.last().map(Plane::downsample).unwrap_or_default();
            lv.push(d);
        }
        Crop { origin: [x0, y0], levels: lv }
    }
    fn to_level(&self, p: [f64; 2], k: usize) -> [f64; 2] {
        let s = (1u32 << k) as f64;
        [(p[0] - self.origin[0]) / s, (p[1] - self.origin[1]) / s]
    }
    fn level_to_image(&self, p: [f64; 2], k: usize) -> [f64; 2] {
        let s = (1u32 << k) as f64;
        [p[0] * s + self.origin[0], p[1] * s + self.origin[1]]
    }
}

/// Largest distance (in top-level pixels, per axis) between the best coarse match and the
/// motion prediction for which the fine stage searches only ±1 pixel per level.
const SMALL_MOTION: f64 = 1.0;
/// Lowest coarse NCC score of the best match for which the fine stage may search only ±1.
const RELIABLE_SCORE: f64 = 0.8;

/// Per-point tracking state.
#[derive(Clone, Debug)]
struct PointState {
    spec: PointSpec,
    /// Current centre in image pixels.
    center: [f64; 2],
    /// Current similarity `[s cos θ, s sin θ]` relative to the template's frame.
    shape: [f64; 2],
    velocity: [f64; 2],
    templates: Vec<Patch>,
    levels: usize,
}

fn half(v: [f64; 2]) -> [f64; 2] {
    [(v[0] / 2.0).max(1.0), (v[1] / 2.0).max(1.0)]
}

impl PointState {
    fn feature_r(&self) -> [i32; 2] {
        let h = half(self.spec.feature_size);
        [h[0].round().max(2.0) as i32, h[1].round().max(2.0) as i32]
    }
    fn level_r(&self, k: usize) -> [i32; 2] {
        let r = self.feature_r();
        let s = (1 << k) as f64;
        [((r[0] as f64 / s).ceil() as i32).max(2), ((r[1] as f64 / s).ceil() as i32).max(2)]
    }
    /// Search range around the predicted centre (pixels, per axis).
    fn range(&self) -> [f64; 2] {
        let f = half(self.spec.feature_size);
        let s = half(self.spec.search_size);
        [(s[0] - f[0]).max(2.0), (s[1] - f[1]).max(2.0)]
    }
    fn extent(&self) -> f64 {
        let r = self.feature_r();
        let s = (self.shape[0] * self.shape[0] + self.shape[1] * self.shape[1]).sqrt().max(1.0);
        ((r[0] * r[0] + r[1] * r[1]) as f64).sqrt() * s + 2.0
    }

    fn new(spec: PointSpec, frame: &Frame, opts: &TrackOptions) -> PointState {
        let c = [spec.center[0] + frame.offset[0], spec.center[1] + frame.offset[1]];
        let mut st = PointState { spec, center: c, shape: [1.0, 0.0], velocity: [0.0; 2], templates: vec![], levels: 0 };
        let range = st.range();
        let fr = st.feature_r();
        let mut l = 0;
        while l < 4 && range[0].min(range[1]) / (2u32 << l) as f64 >= 3.0 && (fr[0].min(fr[1]) as f64) / (2u32 << l) as f64 >= 3.0 {
            l += 1;
        }
        st.levels = l;
        let e = st.extent();
        let crop = Crop::new(frame, opts, c, [e, e], l);
        st.resample(&crop);
        st
    }

    /// Re-sample the templates from `crop` at the current centre and shape.
    fn resample(&mut self, crop: &Crop) {
        self.templates = (0..=self.levels).map(|k| Patch::sample(&crop.levels[k], crop.to_level(self.center, k), self.shape, self.level_r(k))).collect();
    }

    fn step(&mut self, frame: &Frame, opts: &TrackOptions) -> PointResult {
        let prev = self.center;
        let predicted = [prev[0] + self.velocity[0], prev[1] + self.velocity[1]];
        let sc = [predicted[0] + self.spec.search_offset[0], predicted[1] + self.spec.search_offset[1]];
        let range = self.range();
        let e = self.extent();
        let crop = Crop::new(frame, opts, sc, [range[0] + e, range[1] + e], self.levels);
        let top = self.levels;
        // Coarse: exhaustive correlation over the search range at the top level.
        let s = (1u32 << top) as f64;
        let ctop = crop.to_level(sc, top);
        let (rx, ry) = ((range[0] / s).ceil() as i32, (range[1] / s).ceil() as i32);
        let plane = &crop.levels[top];
        let tpl = &self.templates[top];
        let scorer = Scorer::new(plane, tpl, ctop, self.shape);
        let shape = self.shape;
        let mut cands: Vec<(f64, [f64; 2])> = (-ry..=ry)
            .into_par_iter()
            .flat_map_iter(|j| {
                let scorer = &scorer;
                (-rx..=rx).map(move |i| {
                    let c = [ctop[0] + i as f64, ctop[1] + j as f64];
                    (scorer.at(i, j), c)
                })
            })
            .collect();
        // Best score first; ties (flat or repetitive regions) go to the candidate nearest the
        // prediction so featureless areas don't drift.
        let cp = crop.to_level(predicted, top);
        let d2 = |c: &[f64; 2]| (c[0] - cp[0]).powi(2) + (c[1] - cp[1]).powi(2);
        cands.sort_by(|a, b| if (a.0 - b.0).abs() < 1e-9 { d2(&a.1).total_cmp(&d2(&b.1)) } else { b.0.total_cmp(&a.0) });
        let mut seeds: Vec<[f64; 2]> = vec![];
        for (_, c) in &cands {
            if seeds.iter().all(|q| (q[0] - c[0]).abs() > 1.5 || (q[1] - c[1]).abs() > 1.5) {
                seeds.push(*c);
            }
            if seeds.len() == 3 {
                break;
            }
        }
        // Fine: refine each seed down the pyramid. A good motion prediction plus a reliable coarse
        // match only needs a 3 × 3 neighbourhood per level; otherwise keep 5 × 5.
        let near = cands[0].0 >= RELIABLE_SCORE && seeds.first().is_some_and(|c| (c[0] - cp[0]).abs() <= SMALL_MOTION && (c[1] - cp[1]).abs() <= SMALL_MOTION);
        let rr: i32 = if near { 1 } else { 2 };
        let mut best = (f64::MIN, sc);
        for seed in seeds {
            let mut c = seed;
            let mut score = f64::MIN;
            for k in (0..top).rev() {
                c = [c[0] * 2.0, c[1] * 2.0];
                let (pl, tp) = (&crop.levels[k], &self.templates[k]);
                // Start from the projected match; neighbours must beat it (no drift on ties).
                let scorer = Scorer::new(pl, tp, c, shape);
                let mut lb = (scorer.at(0, 0), c);
                for j in -rr..=rr {
                    for i in -rr..=rr {
                        if i == 0 && j == 0 {
                            continue;
                        }
                        let q = [c[0] + i as f64, c[1] + j as f64];
                        let v = scorer.at(i, j);
                        if v > lb.0 + 1e-9 {
                            lb = (v, q);
                        }
                    }
                }
                (score, c) = lb;
            }
            if top == 0 {
                score = ncc(&crop.levels[0], &self.templates[0], c, shape);
            }
            if score > best.0 + 1e-9 {
                best = (score, crop.level_to_image(c, 0));
            }
        }
        let c0 = crop.to_level(best.1, 0);
        // Sub-pixel (and shape) refinement.
        let (mut c, mut a) = (c0, shape);
        if opts.subpixel {
            let mut grad = Grad::around(&crop.levels[0], c0, shape, self.templates[0].r);
            let mut ok = false;
            if opts.track_shape
                && let Some((cr, ar)) = lk_refine(&crop, &mut grad, &self.templates[0], c0, shape, true)
            {
                (c, a) = (cr, ar);
                ok = true;
            }
            if !ok {
                if let Some((cr, _)) = lk_refine(&crop, &mut grad, &self.templates[0], c0, shape, false) {
                    c = cr;
                } else {
                    c = parabolic(&crop.levels[0], &self.templates[0], c0, shape);
                }
            }
        }
        // A featureless template can't be matched: stay put.
        if self.templates[0].ss < 1e-6 {
            (c, a) = (crop.to_level(predicted, 0), shape);
        }
        let mut conf = ncc(&crop.levels[0], &self.templates[0], c, a);
        // Keep the coarse match if refinement made it worse.
        let coarse = ncc(&crop.levels[0], &self.templates[0], c0, shape);
        if coarse > conf + 0.02 {
            (c, a, conf) = (c0, shape, coarse);
        }
        let confidence = (conf.max(0.0) * 100.0).min(100.0);
        let found = crop.level_to_image(c, 0);
        let low = confidence < opts.threshold;
        let mut status = Status::Tracked;
        if low {
            match opts.action {
                ConfidenceAction::Stop => {
                    return self.result(prev, frame, confidence, Status::Stopped);
                }
                ConfidenceAction::Extrapolate => {
                    self.center = predicted;
                    return self.result(predicted, frame, confidence, Status::Extrapolated);
                }
                ConfidenceAction::Continue => {}
                ConfidenceAction::Adapt => status = Status::Adapted,
            }
        }
        // Only confident matches predict the next search position.
        self.velocity = if low { [0.0; 2] } else { [found[0] - prev[0], found[1] - prev[1]] };
        self.center = found;
        self.shape = a;
        if opts.adapt_every_frame || status == Status::Adapted {
            status = Status::Adapted;
            self.resample(&crop);
        }
        self.result(found, frame, confidence, status)
    }

    fn result(&self, c: [f64; 2], frame: &Frame, confidence: f64, status: Status) -> PointResult {
        PointResult {
            center: [c[0] - frame.offset[0], c[1] - frame.offset[1]],
            confidence,
            rotation: self.shape[1].atan2(self.shape[0]).to_degrees(),
            scale: (self.shape[0] * self.shape[0] + self.shape[1] * self.shape[1]).sqrt(),
            status,
        }
    }
}

/// Sub-pixel peak from 1-D parabola fits of the correlation around `c`.
fn parabolic(plane: &Plane, p: &Patch, c: [f64; 2], a: [f64; 2]) -> [f64; 2] {
    let f = |dx: f64, dy: f64| ncc(plane, p, [c[0] + dx, c[1] + dy], a);
    let m = f(0.0, 0.0);
    let off = |l: f64, r: f64| {
        let d = l - 2.0 * m + r;
        if d.abs() < 1e-12 { 0.0 } else { (0.5 * (l - r) / d).clamp(-0.5, 0.5) }
    };
    [c[0] + off(f(-1.0, 0.0), f(1.0, 0.0)), c[1] + off(f(0.0, -1.0), f(0.0, 1.0))]
}

/// Level-0 gradients of a crop, computed only where `lk_refine` reads them: valid on the pixel
/// rectangle `valid` (x0, y0, x1, y1, inclusive) and zero elsewhere.
struct Grad {
    gx: Plane,
    gy: Plane,
    valid: [isize; 4],
}

impl Grad {
    /// Extra pixels around the first window of taps, for the drift of the refinement. A window that
    /// still runs out is replaced by the full gradients.
    const MARGIN: isize = 6;

    fn full(plane: &Plane) -> Grad {
        let (gx, gy) = plane.gradients();
        Grad { gx, gy, valid: [isize::MIN, isize::MIN, isize::MAX, isize::MAX] }
    }

    fn around(plane: &Plane, c: [f64; 2], a: [f64; 2], r: [i32; 2]) -> Grad {
        let [x0, y0, x1, y1] = Grad::taps(c, a, r);
        let m = Grad::MARGIN;
        let clip = |v: isize, hi: usize| v.clamp(0, (hi as isize - 1).max(0)) as usize;
        let rect = [clip(x0 - m, plane.w), clip(y0 - m, plane.h), clip(x1 + m, plane.w), clip(y1 + m, plane.h)];
        let (gx, gy) = plane.gradients_in(rect);
        Grad { gx, gy, valid: rect.map(|v| v as isize) }
    }

    /// The pixels read by sampling the template window at `c` with shape `a` (bilinear taps).
    fn taps(c: [f64; 2], a: [f64; 2], r: [i32; 2]) -> [isize; 4] {
        let ex = (a[0].abs() * r[0] as f64 + a[1].abs() * r[1] as f64) * (1.0 + 1e-9) + 1e-9;
        let ey = (a[1].abs() * r[0] as f64 + a[0].abs() * r[1] as f64) * (1.0 + 1e-9) + 1e-9;
        let px = |v: f64| v.clamp(-1e9, 1e9).floor() as isize;
        [px(c[0] - ex - 0.5), px(c[1] - ey - 0.5), px(c[0] + ex - 0.5) + 1, px(c[1] + ey - 0.5) + 1]
    }

    fn covers(&self, c: [f64; 2], a: [f64; 2], r: [i32; 2]) -> bool {
        let t = Grad::taps(c, a, r);
        t[0] >= self.valid[0] && t[1] >= self.valid[1] && t[2] <= self.valid[2] && t[3] <= self.valid[3]
    }
}

/// Forward-additive Lucas–Kanade (Gauss–Newton) alignment of the level-0 template with gain and
/// bias: minimise Σ (I(c + A u) − g T(u) − b)² over the centre `c` and, with `shape`, the
/// similarity `A`. Returns `None` when it diverges.
fn lk_refine(crop: &Crop, grad: &mut Grad, p: &Patch, c0: [f64; 2], a0: [f64; 2], shape: bool) -> Option<([f64; 2], [f64; 2])> {
    let plane = &crop.levels[0];
    let ch = p.ch;
    let (mut c, mut a) = (c0, a0);
    let mut g = 1.0f64;
    let mut b;
    // Initial photometric estimate from means/variances.
    {
        let mut v = [0.0f32; 3];
        let (mut si, mut si2, mut st, mut st2, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0);
        let mut k = 0;
        for gy in -p.r[1]..=p.r[1] {
            for gx in -p.r[0]..=p.r[0] {
                let (ux, uy) = (gx as f64, gy as f64);
                plane.sample(c[0] + a[0] * ux - a[1] * uy, c[1] + a[1] * ux + a[0] * uy, &mut v);
                for cc in 0..ch {
                    let (i, t) = (v[cc] as f64, p.raw[k] as f64);
                    si += i;
                    si2 += i * i;
                    st += t;
                    st2 += t * t;
                    n += 1.0;
                    k += 1;
                }
            }
        }
        let (mi, mt) = (si / n, st / n);
        let (vi, vt) = ((si2 / n - mi * mi).max(0.0), (st2 / n - mt * mt).max(0.0));
        if vt > 1e-12 && vi > 1e-12 {
            g = (vi / vt).sqrt();
        }
        b = mi - g * mt;
    }
    let np = if shape { 6 } else { 4 };
    let (mut v, mut vx, mut vy) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
    for _ in 0..30 {
        if !grad.covers(c, a, p.r) {
            *grad = Grad::full(plane);
        }
        let (gxp, gyp) = (&grad.gx, &grad.gy);
        let mut h = [[0.0f64; 6]; 6];
        let mut rhs = [0.0f64; 6];
        let mut k = 0;
        for gy in -p.r[1]..=p.r[1] {
            for gx in -p.r[0]..=p.r[0] {
                let (ux, uy) = (gx as f64, gy as f64);
                let q = [c[0] + a[0] * ux - a[1] * uy, c[1] + a[1] * ux + a[0] * uy];
                plane.sample(q[0], q[1], &mut v);
                gxp.sample(q[0], q[1], &mut vx);
                gyp.sample(q[0], q[1], &mut vy);
                for cc in 0..ch {
                    let t = p.raw[k] as f64;
                    k += 1;
                    let (ix, iy) = (vx[cc] as f64, vy[cc] as f64);
                    let r = v[cc] as f64 - g * t - b;
                    let j: [f64; 6] = if shape { [ix, iy, ix * ux + iy * uy, -ix * uy + iy * ux, -t, -1.0] } else { [ix, iy, -t, -1.0, 0.0, 0.0] };
                    for m in 0..np {
                        rhs[m] -= j[m] * r;
                        for n in m..np {
                            h[m][n] += j[m] * j[n];
                        }
                    }
                }
            }
        }
        for m in 0..np {
            for n in 0..m {
                h[m][n] = h[n][m];
            }
            h[m][m] *= 1.0 + 1e-6;
            h[m][m] += 1e-9;
        }
        let d = if shape {
            solve::solve_linear(h, rhs)?
        } else {
            let hh = [
                [h[0][0], h[0][1], h[0][2], h[0][3]],
                [h[1][0], h[1][1], h[1][2], h[1][3]],
                [h[2][0], h[2][1], h[2][2], h[2][3]],
                [h[3][0], h[3][1], h[3][2], h[3][3]],
            ];
            let x = solve::solve_linear(hh, [rhs[0], rhs[1], rhs[2], rhs[3]])?;
            [x[0], x[1], 0.0, 0.0, x[2], x[3]]
        };
        // Limit a single step (Gauss-Newton overshoot on poorly textured features).
        let step = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let k = if step > 1.0 { 1.0 / step } else { 1.0 };
        c[0] += d[0] * k;
        c[1] += d[1] * k;
        if shape {
            a[0] += d[2] * k;
            a[1] += d[3] * k;
        }
        g += d[4] * k;
        b += d[5] * k;
        if !(c[0].is_finite() && c[1].is_finite() && a[0].is_finite() && a[1].is_finite()) {
            return None;
        }
        if step < 1e-4 && (!shape || (d[2].abs() + d[3].abs()) < 1e-6) {
            break;
        }
    }
    let moved = ((c[0] - c0[0]).powi(2) + (c[1] - c0[1]).powi(2)).sqrt();
    let s0 = (a0[0] * a0[0] + a0[1] * a0[1]).sqrt();
    let s1 = (a[0] * a[0] + a[1] * a[1]).sqrt();
    if moved > 2.5 || !(0.75..=1.33).contains(&(s1 / s0.max(1e-9))) || g <= 0.0 {
        return None;
    }
    Some((c, a))
}

/// Tracks a set of points frame by frame (the points of one tracker).
#[derive(Clone, Debug)]
pub struct Tracker {
    pub opts: TrackOptions,
    points: Vec<PointState>,
}

impl Tracker {
    /// Start tracking `points` as they appear on `frame` (the start frame).
    pub fn new(opts: TrackOptions, points: &[PointSpec], frame: &Frame) -> Tracker {
        let pts = points.par_iter().map(|s| PointState::new(*s, frame, &opts)).collect();
        Tracker { opts, points: pts }
    }

    /// Track every point into the next frame (in either time direction).
    pub fn step(&mut self, frame: &Frame) -> Vec<PointResult> {
        let opts = &self.opts;
        self.points.par_iter_mut().map(|p| p.step(frame, opts)).collect()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests;
