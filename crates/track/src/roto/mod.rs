//! Roto Brush segmentation and propagation, clean-room from published methods:
//!
//! - **Interactive graph-cut segmentation** (Y. Boykov and M.-P. Jolly, "Interactive Graph Cuts
//!   for Optimal Boundary & Region Segmentation of Objects in N-D Images", ICCV 2001): the user's
//!   foreground / background strokes are hard constraints, every other pixel gets a data cost
//!   from colour models and neighbouring pixels a contrast-sensitive smoothness cost; the
//!   minimum s–t cut ([`maxflow`], Boykov–Kolmogorov) is the segmentation.
//! - **Iterated colour models** (C. Rother, V. Kolmogorov and A. Blake, "GrabCut", SIGGRAPH
//!   2004): Gaussian mixtures ([`gmm`]) are fitted to the strokes, then re-fitted to the
//!   segmentation and the cut repeated; the 8-connected n-link weight is
//!   `γ/dist · exp(−β‖Iₚ − I_q‖²)` with `β = 1 / (2⟨‖Iₚ − I_q‖²⟩)`.
//! - **Coarse to fine**: large frames are segmented at a reduced size, then re-cut at full
//!   resolution in a narrow band around the upsampled boundary (Lombaert et al., "A Multilevel
//!   Banded Graph Cuts Method for Fast Image Segmentation", ICCV 2005).
//! - **Propagation** (in the spirit of X. Bai et al., "Video SnapCut", SIGGRAPH 2009): the
//!   previous frame's matte is warped to the next frame by optical flow
//!   (`effectcraft_raster::flow::block_flow`), and the frame re-segmented by graph cut only in a
//!   band of **Search Radius** pixels around the warped boundary, with the previous frame's
//!   colour models and the warped matte as a shape prior. Correction strokes on a frame re-cut
//!   the whole frame with the warped matte as a soft prior, and propagation restarts from there.
//!
//! - **Trained models** (optional, swappable: `effectcraft_segment::MaskModel`, e.g. MobileSAM):
//!   [`segment_with`] / [`propagate_with`] prompt the model with points along the strokes, or with
//!   the warped matte (its box, points deep inside it and the matte itself), and its foreground
//!   probability becomes a strong prior for the same graph cut, so strokes stay hard constraints
//!   and edges still snap to colour. A model that fails, or whose mask disagrees with the flow, is
//!   ignored for that frame.
//!
//! Mattes are in layer pixels at a given scale (1 = full resolution). Strokes are stored in
//! layer pixels ([`RotoData`], serde) and keyed per frame so that each frame's result is a
//! deterministic function of its *chain*: the base frame's strokes, then every frame's strokes
//! on the way out to it ([`RotoData::chain_keys`]).

pub mod gmm;
pub mod matting;
pub mod maxflow;
pub mod refine;
pub mod rle;

use std::collections::BTreeMap;

use effectcraft_raster::Image;
use effectcraft_segment::{MaskModel, Prompt};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub use gmm::Gmm;
use maxflow::{Graph, Segment};

/// What a stroke paints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StrokeKind {
    /// Roto Brush (green): foreground.
    Fg,
    /// Roto Brush with Alt (red): background.
    Bg,
    /// Refine Edge tool: mark a band for edge matting.
    Refine,
    /// Refine Edge tool with Alt: remove from the band.
    RefineErase,
    /// A box around the object (Object Matte): two opposite corners in `points`. Trained models
    /// get it as their box prompt; the graph cut ignores it.
    Box,
}

impl StrokeKind {
    pub fn from_name(s: &str) -> Option<StrokeKind> {
        Some(match s.to_ascii_lowercase().as_str() {
            "fg" | "foreground" => StrokeKind::Fg,
            "bg" | "background" => StrokeKind::Bg,
            "refine" | "refineedge" => StrokeKind::Refine,
            "refineerase" | "refine-erase" | "erase" => StrokeKind::RefineErase,
            _ => return None,
        })
    }
    pub fn is_segmentation(self) -> bool {
        matches!(self, StrokeKind::Fg | StrokeKind::Bg)
    }
}

/// One stroke: a polyline swept by a disc of `radius` layer pixels, on layer frame `frame`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub kind: StrokeKind,
    pub frame: i64,
    pub radius: f64,
    pub points: Vec<[f64; 2]>,
}

/// A Roto Brush instance's user data: strokes, the base frame and the segmentation span
/// (inclusive layer frames).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RotoData {
    pub base: Option<i64>,
    pub span: [i64; 2],
    pub strokes: Vec<Stroke>,
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

impl RotoData {
    pub fn from_json(s: &str) -> RotoData {
        if s.trim().is_empty() {
            return RotoData::default();
        }
        serde_json::from_str(s).unwrap_or_default()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
    pub fn is_empty(&self) -> bool {
        self.base.is_none() || !self.strokes.iter().any(|s| s.kind.is_segmentation())
    }
    pub fn strokes_at(&self, frame: i64) -> Vec<&Stroke> {
        self.strokes.iter().filter(|s| s.frame == frame).collect()
    }
    /// Frames that carry strokes.
    pub fn stroke_frames(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.strokes.iter().map(|s| s.frame).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    pub fn in_span(&self, f: i64) -> bool {
        self.base.is_some() && f >= self.span[0] && f <= self.span[1]
    }
    /// Add a stroke: the first segmentation stroke sets the base frame and a span of
    /// `default_span` frames each side (clamped to `limits`); strokes outside the span extend it.
    pub fn add(&mut self, s: Stroke, default_span: i64, limits: [i64; 2]) {
        let f = s.frame;
        match self.base {
            None if s.kind.is_segmentation() => {
                self.base = Some(f);
                self.span = [(f - default_span).max(limits[0]), (f + default_span).min(limits[1]).max(f)];
            }
            Some(_) => {
                self.span[0] = self.span[0].min(f);
                self.span[1] = self.span[1].max(f);
            }
            None => {}
        }
        self.strokes.push(s);
    }
    /// Largest Refine Edge stroke radius (layer pixels).
    pub fn refine_radius(&self) -> f64 {
        self.strokes.iter().filter(|s| matches!(s.kind, StrokeKind::Refine)).map(|s| s.radius).fold(0.0, f64::max)
    }
    /// Content key of every frame in the span: the base frame's from `seed` (what the frames
    /// and settings are) and its strokes, every other frame's from its neighbour towards the
    /// base and its own strokes.
    pub fn chain_keys(&self, seed: u64) -> BTreeMap<i64, u64> {
        let mut out = BTreeMap::new();
        let Some(base) = self.base else { return out };
        let key = |prev: u64, f: i64| {
            let mut h = prev ^ 0x9e37_79b9_7f4a_7c15;
            fnv(&mut h, &f.to_le_bytes());
            for s in self.strokes.iter().filter(|s| s.frame == f) {
                fnv(&mut h, serde_json::to_string(s).unwrap_or_default().as_bytes());
            }
            h
        };
        let k0 = key(seed, base);
        out.insert(base, k0);
        let mut k = k0;
        for f in base + 1..=self.span[1] {
            k = key(k, f);
            out.insert(f, k);
        }
        k = k0;
        for f in (self.span[0]..base).rev() {
            k = key(k, f);
            out.insert(f, k);
        }
        out
    }
    /// The neighbour a frame is propagated from (None for the base frame and outside the span).
    pub fn source_of(&self, f: i64) -> Option<i64> {
        let base = self.base?;
        if !self.in_span(f) || f == base {
            return None;
        }
        Some(if f > base { f - 1 } else { f + 1 })
    }
}

/// Segmentation settings (the effect's Roto Brush parameters, in frame pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegOpts {
    /// Pixels of the matte per layer pixel.
    pub scale: f64,
    /// Search Radius (layer pixels).
    pub search_radius: f64,
    /// Motion Threshold / Motion Damping, percent.
    pub motion_threshold: f64,
    pub motion_damping: f64,
    /// Quality ▸ Best.
    pub best: bool,
}

impl Default for SegOpts {
    fn default() -> Self {
        SegOpts { scale: 1.0, search_radius: 15.0, motion_threshold: 10.0, motion_damping: 20.0, best: false }
    }
}

/// One frame's segmentation: the binary matte, the Refine Edge band and the colour models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSeg {
    pub w: usize,
    pub h: usize,
    pub matte: Vec<u8>,
    pub refine: Vec<u8>,
    pub fg: Gmm,
    pub bg: Gmm,
}

impl FrameSeg {
    pub fn empty(w: usize, h: usize) -> FrameSeg {
        FrameSeg { w, h, matte: vec![0; w * h], refine: vec![0; w * h], ..Default::default() }
    }
    pub fn area(&self) -> usize {
        self.matte.iter().filter(|v| **v != 0).count()
    }
    /// Centroid of the matte (pixels), if any.
    pub fn centroid(&self) -> Option<[f64; 2]> {
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for (i, v) in self.matte.iter().enumerate() {
            if *v != 0 {
                sx += (i % self.w) as f64 + 0.5;
                sy += (i / self.w) as f64 + 0.5;
                n += 1.0;
            }
        }
        (n > 0.0).then(|| [sx / n, sy / n])
    }

    /// A compact binary form (to ship a segmentation between engine instances, e.g. from a Web
    /// Worker to the page): `ECSG`, width, height (u32 LE), the colour models as JSON (length
    /// prefixed), then the matte and the Refine Edge band run-length encoded (u16 LE run, byte).
    pub fn to_bytes(&self) -> Vec<u8> {
        let models = serde_json::to_vec(&(&self.fg, &self.bg)).unwrap_or_default();
        let mut out = Vec::with_capacity(16 + models.len() + 64);
        out.extend_from_slice(b"ECSG");
        out.extend_from_slice(&(self.w as u32).to_le_bytes());
        out.extend_from_slice(&(self.h as u32).to_le_bytes());
        out.extend_from_slice(&(models.len() as u32).to_le_bytes());
        out.extend_from_slice(&models);
        for plane in [&self.matte, &self.refine] {
            let mut i = 0;
            while i < plane.len() {
                let v = plane[i];
                let mut n = 1;
                while i + n < plane.len() && plane[i + n] == v && n < u16::MAX as usize {
                    n += 1;
                }
                out.extend_from_slice(&(n as u16).to_le_bytes());
                out.push(v);
                i += n;
            }
        }
        out
    }

    /// Inverse of [`FrameSeg::to_bytes`] (`None` for malformed input).
    pub fn from_bytes(b: &[u8]) -> Option<FrameSeg> {
        if b.len() < 16 || &b[..4] != b"ECSG" {
            return None;
        }
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let (w, h, ml) = (u(4), u(8), u(12));
        let n = w.checked_mul(h)?;
        let models = b.get(16..16 + ml)?;
        let (fg, bg): (Gmm, Gmm) = serde_json::from_slice(models).ok()?;
        let mut rest = &b[16 + ml..];
        let mut plane = || -> Option<Vec<u8>> {
            let mut v = Vec::with_capacity(n);
            while v.len() < n {
                if rest.len() < 3 {
                    return None;
                }
                let run = u16::from_le_bytes([rest[0], rest[1]]) as usize;
                v.resize(v.len() + run, rest[2]);
                rest = &rest[3..];
            }
            (v.len() == n).then_some(v)
        };
        let matte = plane()?;
        let refine = plane()?;
        Some(FrameSeg { w, h, matte, refine, fg, bg })
    }
}

/// Straight RGB of an image (transparent pixels are black).
pub fn rgb_of(img: &Image) -> Vec<[f32; 3]> {
    img.data.par_iter().map(|p| if p[3] > 1e-6 { [p[0] / p[3], p[1] / p[3], p[2] / p[3]] } else { [0.0; 3] }).collect()
}

/// Pixels covered by `strokes` (of the given kinds) at `scale`.
pub fn rasterize(strokes: &[&Stroke], kinds: &[StrokeKind], scale: f64, w: usize, h: usize) -> Vec<bool> {
    let mut m = vec![false; w * h];
    for s in strokes.iter().filter(|s| kinds.contains(&s.kind)) {
        let r = (s.radius * scale).max(0.5);
        let pts: Vec<[f64; 2]> = s.points.iter().map(|p| [p[0] * scale, p[1] * scale]).collect();
        let segs: Vec<([f64; 2], [f64; 2])> = if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|w| (w[0], w[1])).collect() };
        for (a, b) in segs {
            let x0 = (a[0].min(b[0]) - r).floor().max(0.0) as usize;
            let x1 = ((a[0].max(b[0]) + r).ceil().max(0.0) as usize).min(w);
            let y0 = (a[1].min(b[1]) - r).floor().max(0.0) as usize;
            let y1 = ((a[1].max(b[1]) + r).ceil().max(0.0) as usize).min(h);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = [x as f64 + 0.5, y as f64 + 0.5];
                    let t = if l2 > 0.0 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
                    if (p[0] - q[0]).hypot(p[1] - q[1]) <= r {
                        m[y * w + x] = true;
                    }
                }
            }
        }
    }
    m
}

/// Hard constraints: 1 = foreground, 2 = background, 0 = free.
const HARD_FG: u8 = 1;
const HARD_BG: u8 = 2;

/// Graph-cut energy weights.
const GAMMA: f64 = 50.0;
const PRIOR: f64 = 1.0;
/// A trained model's matte as a prior: it outweighs the colour models.
const MODEL_PRIOR: f64 = 4.0;
/// Fixed-point scale of the energies.
const K: f64 = 16.0;
const INF: i64 = 1 << 40;

const NB: [(i64, i64, bool); 8] = [(1, 0, true), (0, 1, true), (1, 1, true), (-1, 1, true), (-1, 0, false), (0, -1, false), (-1, -1, false), (1, -1, false)];

fn beta(img: &[[f32; 3]], free: &[bool], w: usize, h: usize) -> f64 {
    let (mut s, mut n) = (0.0f64, 0.0f64);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !free[i] {
                continue;
            }
            let c = img[i];
            for (dx, dy) in [(1usize, 0usize), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < w && ny < h {
                    let q = img[ny * w + nx];
                    s += ((c[0] - q[0]).powi(2) + (c[1] - q[1]).powi(2) + (c[2] - q[2]).powi(2)) as f64;
                    n += 1.0;
                }
            }
        }
    }
    if s <= 1e-12 { 1.0 } else { n / (2.0 * s) }
}

/// One min-cut over the `free` pixels; others keep `labels`. `prior`: P(foreground) per pixel
/// and how much it weighs against the colour models.
fn cut(img: &[[f32; 3]], w: usize, h: usize, labels: &mut [u8], free: &[bool], hard: &[u8], fg: &Gmm, bg: &Gmm, prior: Option<(&[f32], f64)>) {
    let mut idx = vec![u32::MAX; w * h];
    let mut nodes = vec![];
    for i in 0..w * h {
        if free[i] {
            idx[i] = nodes.len() as u32;
            nodes.push(i);
        }
    }
    if nodes.is_empty() {
        return;
    }
    let b = beta(img, free, w, h);
    let costs: Vec<(f64, f64)> = nodes
        .par_iter()
        .map(|&i| {
            let c = img[i];
            let (mut cf, mut cb) = (fg.cost(c), bg.cost(c));
            if let Some((p, wt)) = prior {
                let pf = (p[i] as f64).clamp(0.02, 0.98);
                cf += wt * -pf.ln();
                cb += wt * -(1.0 - pf).ln();
            }
            (cf, cb)
        })
        .collect();
    let mut g = Graph::new(nodes.len(), nodes.len() * 4);
    for (n, &i) in nodes.iter().enumerate() {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let (cf, cb) = costs[n];
        let m = cf.min(cb);
        let (mut src, mut snk) = (((cb - m) * K).round() as i64, ((cf - m) * K).round() as i64);
        match hard[i] {
            HARD_FG => (src, snk) = (INF, 0),
            HARD_BG => (src, snk) = (0, INF),
            _ => {}
        }
        g.add_tweights(n, src, snk);
        let c = img[i];
        for (dx, dy, fwd) in NB {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let j = ny as usize * w + nx as usize;
            let q = img[j];
            let d2 = ((c[0] - q[0]).powi(2) + (c[1] - q[1]).powi(2) + (c[2] - q[2]).powi(2)) as f64;
            let dist = if dx != 0 && dy != 0 { std::f64::consts::SQRT_2 } else { 1.0 };
            let wt = ((GAMMA / dist * (-b * d2).exp()) * K).round() as i32;
            if wt <= 0 {
                continue;
            }
            if idx[j] == u32::MAX {
                // Fixed neighbour: disagreeing with it costs the edge.
                if labels[j] != 0 {
                    g.add_tweights(n, wt as i64, 0);
                } else {
                    g.add_tweights(n, 0, wt as i64);
                }
            } else if fwd {
                g.add_edge(n, idx[j] as usize, wt, wt);
            }
        }
    }
    g.maxflow();
    for (n, &i) in nodes.iter().enumerate() {
        labels[i] = (g.segment(n) == Segment::Source) as u8;
    }
}

/// Colour samples of the pixels where `sel` holds.
fn samples(img: &[[f32; 3]], sel: impl Fn(usize) -> bool) -> Vec<[f32; 3]> {
    img.iter().enumerate().filter(|(i, _)| sel(*i)).map(|(_, c)| *c).collect()
}

const COMPONENTS: usize = 5;
const MAX_SAMPLES: usize = 24_000;

fn fit_models(img: &[[f32; 3]], labels: &[u8]) -> (Gmm, Gmm) {
    let fg = samples(img, |i| labels[i] != 0);
    let bg = samples(img, |i| labels[i] == 0);
    (Gmm::fit(&fg, COMPONENTS, MAX_SAMPLES), Gmm::fit(&bg, COMPONENTS, MAX_SAMPLES))
}

/// Keep the foreground components (8-connected) that touch `seeds`; fill small background
/// holes that touch neither the frame edge nor a background seed.
fn clean(labels: &mut [u8], seeds: &[bool], bg_seeds: &[bool], w: usize, h: usize) {
    let n = w * h;
    let mut comp = vec![u32::MAX; n];
    let mut stack = vec![];
    let mut fg_area = 0usize;
    for start in 0..n {
        if comp[start] != u32::MAX {
            continue;
        }
        let lab = labels[start];
        let id = start as u32;
        comp[start] = id;
        stack.push(start);
        let mut members = vec![];
        let (mut seeded, mut edge, mut bgs) = (false, false, false);
        while let Some(i) = stack.pop() {
            members.push(i);
            seeded |= seeds[i];
            bgs |= bg_seeds[i];
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            edge |= x == 0 || y == 0 || x == w as i64 - 1 || y == h as i64 - 1;
            for (dx, dy, _) in NB {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if comp[j] == u32::MAX && labels[j] == lab {
                    comp[j] = id;
                    stack.push(j);
                }
            }
        }
        if lab != 0 {
            if seeded {
                fg_area += members.len();
            } else {
                for i in members {
                    labels[i] = 0;
                }
            }
        } else if !edge && !bgs {
            // Remember holes as negative candidates (resolved below once fg_area is known).
            for i in members {
                comp[i] = u32::MAX - 1;
            }
        }
    }
    // Fill holes smaller than 2 % of the foreground.
    let mut seen = vec![false; n];
    for start in 0..n {
        if comp[start] != u32::MAX - 1 || seen[start] || labels[start] != 0 {
            continue;
        }
        let mut members = vec![start];
        seen[start] = true;
        let mut k = 0;
        while k < members.len() {
            let i = members[k];
            k += 1;
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for (dx, dy, _) in NB {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if !seen[j] && comp[j] == u32::MAX - 1 && labels[j] == 0 {
                    seen[j] = true;
                    members.push(j);
                }
            }
        }
        // A hole is enclosed by foreground that survived the component filter.
        let enclosed = members.iter().all(|&i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            NB.iter().all(|(dx, dy, _)| {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    return false;
                }
                let j = ny as usize * w + nx as usize;
                labels[j] != 0 || comp[j] == u32::MAX - 1
            })
        });
        if enclosed && (members.len() as f64) < 0.02 * fg_area as f64 {
            for i in members {
                labels[i] = 1;
            }
        }
    }
}

fn downsample_rgb(img: &[[f32; 3]], w: usize, h: usize, f: usize) -> (Vec<[f32; 3]>, usize, usize) {
    let (cw, ch) = (w.div_ceil(f), h.div_ceil(f));
    let out = (0..cw * ch)
        .into_par_iter()
        .map(|i| {
            let (cx, cy) = (i % cw, i / cw);
            let mut s = [0.0f32; 3];
            let mut n = 0.0;
            for y in cy * f..((cy + 1) * f).min(h) {
                for x in cx * f..((cx + 1) * f).min(w) {
                    let c = img[y * w + x];
                    s[0] += c[0];
                    s[1] += c[1];
                    s[2] += c[2];
                    n += 1.0;
                }
            }
            [s[0] / n, s[1] / n, s[2] / n]
        })
        .collect();
    (out, cw, ch)
}

/// What a frame is segmented with besides its strokes: the matte propagated from its neighbour.
pub struct Prior {
    /// Warped neighbour matte, soft (0–1).
    pub soft: Vec<f32>,
    /// The neighbour's colour models.
    pub fg: Gmm,
    pub bg: Gmm,
    /// Warped Refine Edge band.
    pub refine: Vec<u8>,
    /// How much the prior weighs against the colour models.
    pub weight: f64,
}

/// Segment a frame from its strokes (and a propagated prior): GrabCut iterations on a reduced
/// copy, then a full-resolution cut in a band around the boundary.
pub fn segment(img: &Image, strokes: &[&Stroke], prior: Option<&Prior>, opts: &SegOpts, refine_radius: f64) -> FrameSeg {
    let (w, h) = (img.width as usize, img.height as usize);
    if w == 0 || h == 0 {
        return FrameSeg::empty(w, h);
    }
    let rgb = rgb_of(img);
    let fgs = rasterize(strokes, &[StrokeKind::Fg], opts.scale, w, h);
    let bgs = rasterize(strokes, &[StrokeKind::Bg], opts.scale, w, h);
    let hard: Vec<u8> = fgs
        .iter()
        .zip(&bgs)
        .map(|(f, b)| {
            if *b {
                HARD_BG
            } else if *f {
                HARD_FG
            } else {
                0
            }
        })
        .collect();
    // A warped matte is uncertain by about the search radius: blur it. A model's matte is not.
    let soft_prior: Option<Vec<f32>> =
        prior.map(|p| if p.weight > PRIOR { p.soft.clone() } else { matting::gauss(&p.soft, w, h, (opts.search_radius * opts.scale / 4.0).max(1.0)) });
    let pw = prior.map_or(PRIOR, |p| p.weight);
    // Initial colour models: the strokes (and the propagated matte).
    let (fg0, bg0) = match prior {
        Some(p) => {
            let fgc = samples(&rgb, |i| (hard[i] == HARD_FG) || (hard[i] == 0 && p.soft[i] >= 0.5));
            let bgc = samples(&rgb, |i| (hard[i] == HARD_BG) || (hard[i] == 0 && p.soft[i] < 0.5));
            let f = Gmm::fit(&fgc, COMPONENTS, MAX_SAMPLES);
            let b = Gmm::fit(&bgc, COMPONENTS, MAX_SAMPLES);
            (if f.is_empty() { p.fg.clone() } else { f }, if b.is_empty() { p.bg.clone() } else { b })
        }
        None => {
            let fgc = samples(&rgb, |i| hard[i] == HARD_FG);
            let mut bgc = samples(&rgb, |i| hard[i] == HARD_BG);
            if bgc.is_empty() {
                // No background strokes: what is far from the foreground strokes.
                let d = matting::distance(&fgs, w, h);
                let r = strokes.iter().filter(|s| s.kind == StrokeKind::Fg).map(|s| s.radius).fold(1.0, f64::max) * opts.scale;
                let far = (4.0 * r).max(0.25 * w.max(h) as f64) as f32;
                bgc = samples(&rgb, |i| d[i] > far);
                if bgc.is_empty() {
                    bgc = samples(&rgb, |i| hard[i] != HARD_FG);
                }
            }
            (Gmm::fit(&fgc, COMPONENTS, MAX_SAMPLES), Gmm::fit(&bgc, COMPONENTS, MAX_SAMPLES))
        }
    };
    let iters = if opts.best { 4 } else { 3 };
    let work = if opts.best { 960 } else { 480 };
    let f = w.max(h).div_ceil(work).max(1);
    let mut labels: Vec<u8> = match &soft_prior {
        Some(p) => p.iter().map(|v| (*v >= 0.5) as u8).collect(),
        None => fgs.iter().map(|v| *v as u8).collect(),
    };
    for (l, hd) in labels.iter_mut().zip(&hard) {
        if *hd != 0 {
            *l = (*hd == HARD_FG) as u8;
        }
    }
    let (mut fgm, mut bgm) = (fg0, bg0);
    if f > 1 {
        let (crgb, cw, ch) = downsample_rgb(&rgb, w, h, f);
        let mut chard = vec![0u8; cw * ch];
        let mut cprior = soft_prior.as_ref().map(|_| vec![0.0f32; cw * ch]);
        for cy in 0..ch {
            for cx in 0..cw {
                let (mut anyf, mut anyb, mut s, mut n) = (false, false, 0.0f32, 0.0f32);
                for y in cy * f..((cy + 1) * f).min(h) {
                    for x in cx * f..((cx + 1) * f).min(w) {
                        let i = y * w + x;
                        anyf |= hard[i] == HARD_FG;
                        anyb |= hard[i] == HARD_BG;
                        if let Some(p) = &soft_prior {
                            s += p[i];
                        }
                        n += 1.0;
                    }
                }
                chard[cy * cw + cx] = match (anyf, anyb) {
                    (true, false) => HARD_FG,
                    (false, true) => HARD_BG,
                    _ => 0,
                };
                if let Some(cp) = &mut cprior {
                    cp[cy * cw + cx] = s / n;
                }
            }
        }
        let mut cl: Vec<u8> = match &cprior {
            Some(p) => p.iter().map(|v| (*v >= 0.5) as u8).collect(),
            None => chard.iter().map(|v| (*v == HARD_FG) as u8).collect(),
        };
        let free = vec![true; cw * ch];
        for it in 0..iters {
            cut(&crgb, cw, ch, &mut cl, &free, &chard, &fgm, &bgm, cprior.as_deref().map(|c| (c, pw)));
            if it + 1 < iters {
                let (a, b) = fit_models(&crgb, &cl);
                if !a.is_empty() && !b.is_empty() {
                    (fgm, bgm) = (a, b);
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                labels[i] = if hard[i] != 0 { (hard[i] == HARD_FG) as u8 } else { cl[(y / f) * cw + x / f] };
            }
        }
        let (a, b) = fit_models(&rgb, &labels);
        if !a.is_empty() && !b.is_empty() {
            (fgm, bgm) = (a, b);
        }
        // Full-resolution band.
        let sd = matting::signed_distance(&labels, w, h);
        let band = f as f32 + 1.5;
        let free: Vec<bool> = sd.iter().map(|d| d.abs() <= band).collect();
        cut(&rgb, w, h, &mut labels, &free, &hard, &fgm, &bgm, soft_prior.as_deref().map(|c| (c, pw)));
    } else {
        let free = vec![true; w * h];
        for it in 0..iters {
            cut(&rgb, w, h, &mut labels, &free, &hard, &fgm, &bgm, soft_prior.as_deref().map(|c| (c, pw)));
            if it + 1 < iters {
                let (a, b) = fit_models(&rgb, &labels);
                if !a.is_empty() && !b.is_empty() {
                    (fgm, bgm) = (a, b);
                }
            }
        }
    }
    // Keep what the strokes (or the prior's core) select.
    let mut seeds = fgs.clone();
    if let Some(p) = &soft_prior {
        let core = core_of(p, w, h, opts.search_radius * opts.scale);
        for (s, c) in seeds.iter_mut().zip(core) {
            *s |= c;
        }
    }
    clean(&mut labels, &seeds, &bgs, w, h);
    finish(img, &rgb, labels, strokes, prior.map(|p| p.refine.as_slice()), opts, refine_radius)
}

/// Pixels deep inside a soft matte (more than `r` from its edge), else the matte itself.
fn core_of(soft: &[f32], w: usize, h: usize, r: f64) -> Vec<bool> {
    let bin: Vec<u8> = soft.iter().map(|v| (*v >= 0.5) as u8).collect();
    let sd = matting::signed_distance(&bin, w, h);
    let core: Vec<bool> = sd.iter().map(|d| *d as f64 > r).collect();
    if core.iter().any(|c| *c) { core } else { bin.iter().map(|v| *v != 0).collect() }
}

/// Final models and Refine Edge band of a segmented frame.
fn finish(_img: &Image, rgb: &[[f32; 3]], labels: Vec<u8>, strokes: &[&Stroke], warped_refine: Option<&[u8]>, opts: &SegOpts, refine_radius: f64) -> FrameSeg {
    let (w, h) = (_img.width as usize, _img.height as usize);
    let (fg, bg) = fit_models(rgb, &labels);
    let add = rasterize(strokes, &[StrokeKind::Refine], opts.scale, w, h);
    let erase = rasterize(strokes, &[StrokeKind::RefineErase], opts.scale, w, h);
    let near: Option<Vec<f32>> = warped_refine.filter(|r| r.iter().any(|v| *v != 0)).map(|_| matting::signed_distance(&labels, w, h));
    let rr = (refine_radius * opts.scale + 2.0) as f32;
    let refine: Vec<u8> = (0..w * h)
        .map(|i| {
            let carried = match (warped_refine, &near) {
                (Some(r), Some(sd)) => r[i] != 0 && sd[i].abs() <= rr,
                _ => false,
            };
            ((carried || add[i]) && !erase[i]) as u8
        })
        .collect();
    FrameSeg { w, h, matte: labels, refine, fg, bg }
}

/// Warp the previous frame's segmentation onto the next frame by optical flow (next → prev),
/// with Motion Threshold / Damping applied to the vectors.
pub fn warp_prior(prev_img: &Image, prev: &FrameSeg, next_img: &Image, opts: &SegOpts) -> Prior {
    let (w, h) = (next_img.width as usize, next_img.height as usize);
    let flow = effectcraft_raster::flow::block_flow(next_img, prev_img, 8, 4);
    let thr = (opts.motion_threshold / 100.0 * opts.search_radius * opts.scale).max(0.0);
    let damp = (1.0 - opts.motion_damping / 100.0).clamp(0.0, 1.0);
    let src: Vec<f32> = prev.matte.iter().map(|v| *v as f32).collect();
    let pw = prev.w.min(w);
    let ph = prev.h.min(h);
    let res: Vec<(f32, u8)> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let v = flow.at(x, y);
            let (mut vx, mut vy) = (v[0] as f64, v[1] as f64);
            if vx.hypot(vy) < thr {
                vx *= damp;
                vy *= damp;
            }
            let (sx, sy) = (x + vx, y + vy);
            let soft = if pw == prev.w && ph == prev.h { matting::sample(&src, prev.w, prev.h, sx, sy) } else { 0.0 };
            let (ix, iy) = (sx.floor().clamp(0.0, (prev.w.max(1) - 1) as f64) as usize, sy.floor().clamp(0.0, (prev.h.max(1) - 1) as f64) as usize);
            let r = prev.refine.get(iy * prev.w + ix).copied().unwrap_or(0);
            (soft, r)
        })
        .collect();
    Prior { soft: res.iter().map(|r| r.0).collect(), fg: prev.fg.clone(), bg: prev.bg.clone(), refine: res.iter().map(|r| r.1).collect(), weight: PRIOR }
}

/// Propagate the previous frame's segmentation to the next frame: warp it, then re-cut in a
/// band of Search Radius pixels around the warped boundary. Strokes on the next frame re-cut
/// the whole frame with the warped matte as a soft prior instead.
pub fn propagate(prev_img: &Image, prev: &FrameSeg, next_img: &Image, strokes: &[&Stroke], opts: &SegOpts, refine_radius: f64) -> FrameSeg {
    let (w, h) = (next_img.width as usize, next_img.height as usize);
    let prior = warp_prior(prev_img, prev, next_img, opts);
    if strokes.iter().any(|s| s.kind.is_segmentation()) {
        return segment(next_img, strokes, Some(&prior), opts, refine_radius);
    }
    let rgb = rgb_of(next_img);
    let mut labels: Vec<u8> = prior.soft.iter().map(|v| (*v >= 0.5) as u8).collect();
    let sd = matting::signed_distance(&labels, w, h);
    let r = (opts.search_radius * opts.scale).max(2.0) as f32;
    let free: Vec<bool> = sd.iter().map(|d| d.abs() <= r).collect();
    let hard = vec![0u8; w * h];
    let soft = matting::gauss(&prior.soft, w, h, (r as f64 / 4.0).max(1.0));
    let (fg, bg) = if prev.fg.is_empty() || prev.bg.is_empty() { fit_models(&rgb, &labels) } else { (prev.fg.clone(), prev.bg.clone()) };
    cut(&rgb, w, h, &mut labels, &free, &hard, &fg, &bg, Some((&soft, PRIOR)));
    let seeds = core_of(&prior.soft, w, h, r as f64);
    clean(&mut labels, &seeds, &vec![false; w * h], w, h);
    finish(next_img, &rgb, labels, strokes, Some(&prior.refine), opts, refine_radius)
}

/// Points spread along a stroke (matte pixels), about one per stroke width, at most `max`.
fn points_along(s: &Stroke, scale: f64, max: usize) -> Vec<[f32; 2]> {
    let pts: Vec<[f64; 2]> = s.points.iter().map(|p| [p[0] * scale, p[1] * scale]).collect();
    let len: f64 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
    let step = (2.0 * s.radius * scale).max(len / max.max(1) as f64).max(1.0);
    let mut out = pts.first().map(|p| vec![[p[0] as f32, p[1] as f32]]).unwrap_or_default();
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let d = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        let mut t = step - carry;
        while t <= d && out.len() < max {
            let f = t / d.max(1e-9);
            out.push([(w[0][0] + (w[1][0] - w[0][0]) * f) as f32, (w[0][1] + (w[1][1] - w[0][1]) * f) as f32]);
            t += step;
        }
        carry = (carry + d) % step;
    }
    out
}

/// A model's prompt from the strokes: points along the foreground and background strokes, and
/// the box of a [`StrokeKind::Box`] stroke (matte pixels).
pub fn stroke_prompt(strokes: &[&Stroke], scale: f64) -> Prompt {
    let mut points = vec![];
    for (kind, fg) in [(StrokeKind::Fg, true), (StrokeKind::Bg, false)] {
        let ss: Vec<&&Stroke> = strokes.iter().filter(|s| s.kind == kind).collect();
        let per = (16 / ss.len().max(1)).max(1);
        for s in ss {
            points.extend(points_along(s, scale, per).into_iter().map(|p| (p, fg)));
        }
    }
    let bbox = strokes.iter().filter(|s| s.kind == StrokeKind::Box).find_map(|s| box_of(s, scale));
    Prompt { points, bbox, ..Default::default() }
}

/// The box `[x0, y0, x1, y1]` (matte pixels) of a box stroke: its first two points as opposite
/// corners; `None` when they are missing, not finite or the box is empty.
fn box_of(s: &Stroke, scale: f64) -> Option<[f32; 4]> {
    let (a, c) = (s.points.first()?, s.points.get(1)?);
    let v = [a[0].min(c[0]), a[1].min(c[1]), a[0].max(c[0]), a[1].max(c[1])].map(|v| (v * scale) as f32);
    (v.iter().all(|x| x.is_finite()) && v[2] > v[0] && v[3] > v[1]).then_some(v)
}

/// Add a prior matte to a prompt: its box (a little larger; unless the prompt has one), up to
/// four points spread through its core, and the matte itself.
fn add_prior_prompt(prompt: &mut Prompt, soft: &[f32], w: usize, h: usize) -> bool {
    let bin: Vec<u8> = soft.iter().map(|v| (*v >= 0.5) as u8).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, v) in bin.iter().enumerate() {
        if *v != 0 {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x0 > x1 {
        return false;
    }
    let (mx, my) = (((x1 - x0) as f32 * 0.05).max(4.0), ((y1 - y0) as f32 * 0.05).max(4.0));
    // A box the user drew on this frame wins over the prior's.
    if prompt.bbox.is_none() {
        prompt.bbox = Some([(x0 as f32 - mx).max(0.0), (y0 as f32 - my).max(0.0), (x1 as f32 + 1.0 + mx).min(w as f32), (y1 as f32 + 1.0 + my).min(h as f32)]);
    }
    // Farthest-point picks among the core (deep inside), starting from the deepest pixel.
    let sd = matting::signed_distance(&bin, w, h);
    let deep = sd.iter().copied().fold(0.0f32, f32::max);
    let core: Vec<usize> = (0..w * h).filter(|i| sd[*i] >= (deep * 0.5).max(1.0)).step_by(((w * h) / 20_000).max(1)).collect();
    let mut picks: Vec<usize> = core.iter().copied().max_by(|a, b| sd[*a].total_cmp(&sd[*b])).into_iter().collect();
    while picks.len() < 4 {
        let far = core.iter().copied().max_by_key(|c| picks.iter().map(|p| (c % w).abs_diff(p % w).pow(2) + (c / w).abs_diff(p / w).pow(2)).min().unwrap_or(0));
        match far {
            Some(f) if !picks.contains(&f) => picks.push(f),
            _ => break,
        }
    }
    prompt.points.extend(picks.into_iter().map(|i| ([(i % w) as f32 + 0.5, (i / w) as f32 + 0.5], true)));
    prompt.mask = Some(soft.to_vec());
    true
}

/// [`segment`] with a trained model (if any): its matte for the strokes (and the propagated
/// prior) becomes a strong prior for the cut.
pub fn segment_with(img: &Image, strokes: &[&Stroke], prior: Option<&Prior>, opts: &SegOpts, refine_radius: f64, model: Option<&dyn MaskModel>) -> FrameSeg {
    let (w, h) = (img.width as usize, img.height as usize);
    let Some(m) = model.filter(|_| w > 0 && h > 0) else { return segment(img, strokes, prior, opts, refine_radius) };
    let mut prompt = stroke_prompt(strokes, opts.scale);
    if let Some(p) = prior {
        add_prior_prompt(&mut prompt, &p.soft, w, h);
    }
    if !prompt.points.iter().any(|(_, fg)| *fg) {
        return segment(img, strokes, prior, opts, refine_radius);
    }
    match m.segment(&rgb_of(img), w, h, &prompt) {
        Ok(soft) if soft.len() == w * h => {
            let (fg, bg) = prior.map_or_else(Default::default, |p| (p.fg.clone(), p.bg.clone()));
            let refine = prior.map_or_else(|| vec![0; w * h], |p| p.refine.clone());
            segment(img, strokes, Some(&Prior { soft, fg, bg, refine, weight: MODEL_PRIOR }), opts, refine_radius)
        }
        _ => segment(img, strokes, prior, opts, refine_radius),
    }
}

/// [`propagate`] with a trained model (if any): the model, prompted with the flow-warped matte,
/// re-segments the frame; when it disagrees with the flow (IoU under 0.5) the classical result
/// stands.
pub fn propagate_with(
    prev_img: &Image,
    prev: &FrameSeg,
    next_img: &Image,
    strokes: &[&Stroke],
    opts: &SegOpts,
    refine_radius: f64,
    model: Option<&dyn MaskModel>,
) -> FrameSeg {
    let (w, h) = (next_img.width as usize, next_img.height as usize);
    let Some(m) = model.filter(|_| w > 0 && h > 0) else { return propagate(prev_img, prev, next_img, strokes, opts, refine_radius) };
    let prior = warp_prior(prev_img, prev, next_img, opts);
    let mut prompt = stroke_prompt(strokes, opts.scale);
    if !add_prior_prompt(&mut prompt, &prior.soft, w, h) {
        return propagate(prev_img, prev, next_img, strokes, opts, refine_radius);
    }
    match m.segment(&rgb_of(next_img), w, h, &prompt) {
        Ok(soft) if soft.len() == w * h => {
            let a: Vec<u8> = soft.iter().map(|v| (*v >= 0.5) as u8).collect();
            let b: Vec<u8> = prior.soft.iter().map(|v| (*v >= 0.5) as u8).collect();
            if iou(&a, &b) < 0.5 {
                return propagate(prev_img, prev, next_img, strokes, opts, refine_radius);
            }
            let p = Prior { soft, weight: MODEL_PRIOR, ..prior };
            segment(next_img, strokes, Some(&p), opts, refine_radius)
        }
        _ => propagate(prev_img, prev, next_img, strokes, opts, refine_radius),
    }
}

/// Intersection over union of two binary mattes.
pub fn iou(a: &[u8], b: &[u8]) -> f64 {
    let (mut i, mut u) = (0usize, 0usize);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (*x != 0, *y != 0);
        i += (x && y) as usize;
        u += (x || y) as usize;
    }
    if u == 0 { 1.0 } else { i as f64 / u as f64 }
}

#[cfg(test)]
mod tests;
