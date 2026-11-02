//! Distort: CC Flo Motion, Liquify and Rolling Shutter Repair.
//!
//! * **CC Flo Motion** — two knots that pull the image toward themselves (positive amount) or
//!   push it away (negative), with a falloff exponent, optional tiling and supersampled
//!   antialiasing.
//! * **Liquify** — brush strokes build a displacement mesh (a grid of layer-space offsets). The
//!   strokes are the effect's data (the hidden `distortionMesh` text, one stroke per line, so it
//!   animates with hold keyframes and is written by the `liquify.stroke` command); replaying
//!   them is deterministic and cached. Tools: warp, turbulence, twirl clockwise / counter-
//!   clockwise, pucker, bloat, shift pixels, reflection, clone, reconstruction and freeze / thaw.
//!   Distortion Percentage scales the mesh and Distortion Mesh Offset moves it.
//! * **Rolling Shutter Repair** — every scanline of a CMOS frame is exposed a little later than
//!   the previous one, so moving content shears. Motion is measured between the neighbouring
//!   frames with pyramidal Lucas–Kanade (`effectcraft-track`) on a grid of points; each pixel is
//!   then resampled from where it was at the frame's mid-exposure time:
//!   `out(p) = in(p + v(p)·rate·(s(p) − ½))`, with `s` the scan position (0…1 along the scan
//!   direction) and `v` the per-frame velocity (Warp: one smooth velocity per scanline band;
//!   Pixel Motion: a dense interpolated field).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::util::{hash1, point_in_poly};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Distort", params, render, gpu: false, float: true }
}

// ---------------------------------------------------------------- CC Flo Motion

fn flo_motion(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let knots = [(ctx.params.v2("knot1"), ctx.params.f("amount1") / 100.0), (ctx.params.v2("knot2"), ctx.params.f("amount2") / 100.0)];
    if knots.iter().all(|k| k.1 == 0.0) {
        return b;
    }
    let tile = ctx.params.b("tileEdges");
    let ss = match ctx.params.e("antialiasing") {
        0 => 1,
        1 => 2,
        _ => 3,
    };
    let falloff = ctx.params.f("falloff").max(0.01);
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let radius = (lw * lw + lh * lh).sqrt() * 0.5;
    let src = b.img.clone();
    let (scale, off) = (b.scale, b.offset);
    let map = |lx: f64, ly: f64| -> (f64, f64) {
        let (mut x, mut y) = (lx, ly);
        for (k, a) in knots {
            if a == 0.0 {
                continue;
            }
            let (dx, dy) = (x - k[0], y - k[1]);
            let r = (dx * dx + dy * dy).sqrt() / radius;
            // Weight 1 at the knot, falling off with distance.
            let w = (1.0 - r.min(1.0)).powf(falloff);
            // Pull: sample closer to the knot (content flows out of it); push: farther.
            let s = (1.0 - a.clamp(-4.0, 0.95) * w).max(0.02);
            x = k[0] + dx * s;
            y = k[1] + dy * s;
        }
        (x, y)
    };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let px = x as f64 + (sx as f64 + 0.5) / ss as f64;
                    let py = y as f64 + (sy as f64 + 0.5) / ss as f64;
                    let (lx, ly) = map((px - off[0]) / scale, (py - off[1]) / scale);
                    let (lx, ly) = if tile { (lx.rem_euclid(lw), ly.rem_euclid(lh)) } else { (lx, ly) };
                    let q = src.sample_bilinear(lx * scale + off[0], ly * scale + off[1]);
                    for c in 0..4 {
                        acc[c] += q[c];
                    }
                }
            }
            let n = (ss * ss) as f32;
            *o = acc.map(|v| v / n);
        }
    });
    b
}

// ---------------------------------------------------------------- Liquify

/// Liquify tools (the stroke text uses these names).
pub const LIQUIFY_TOOLS: [&str; 12] = [
    "warp",
    "turbulence",
    "twirlClockwise",
    "twirlCounterclockwise",
    "pucker",
    "bloat",
    "shiftPixels",
    "reflection",
    "clone",
    "reconstruction",
    "freeze",
    "thaw",
];

/// Display labels of [`LIQUIFY_TOOLS`] (the Tool popup), in the same order.
pub const LIQUIFY_TOOL_LABELS: [&str; 12] = [
    "Warp",
    "Turbulence",
    "Twirl Clockwise",
    "Twirl Counterclockwise",
    "Pucker",
    "Bloat",
    "Shift Pixels",
    "Reflection",
    "Clone",
    "Reconstruction",
    "Freeze",
    "Thaw",
];

/// One recorded Liquify stroke.
#[derive(Clone, Debug, PartialEq)]
pub struct LiquifyStroke {
    pub tool: usize,
    /// Brush size (diameter) and pressure (0–100), turbulent jitter (0–100), layer pixels.
    pub size: f64,
    pub pressure: f64,
    pub jitter: f64,
    /// Clone offset (source − destination).
    pub clone_offset: [f64; 2],
    pub points: Vec<[f64; 2]>,
}

impl LiquifyStroke {
    /// Text form: `tool size pressure jitter cloneX cloneY x,y x,y …`.
    pub fn to_line(&self) -> String {
        let mut s = format!("{} {} {} {} {} {}", LIQUIFY_TOOLS[self.tool], self.size, self.pressure, self.jitter, self.clone_offset[0], self.clone_offset[1]);
        for p in &self.points {
            s.push_str(&format!(" {},{}", p[0], p[1]));
        }
        s
    }
    pub fn parse(line: &str) -> Option<LiquifyStroke> {
        let mut it = line.split_whitespace();
        let name = it.next()?;
        let tool = LIQUIFY_TOOLS.iter().position(|t| t.eq_ignore_ascii_case(name))?;
        let mut f = || -> Option<f64> { it.next()?.parse().ok() };
        let (size, pressure, jitter, cx, cy) = (f()?, f()?, f()?, f()?, f()?);
        let points: Vec<[f64; 2]> = it
            .filter_map(|t| {
                let (a, b) = t.split_once(',')?;
                Some([a.parse().ok()?, b.parse().ok()?])
            })
            .collect();
        (!points.is_empty()).then_some(LiquifyStroke { tool, size, pressure, jitter, clone_offset: [cx, cy], points })
    }
}

/// Parse the `distortionMesh` text into strokes (bad lines are skipped).
pub fn parse_strokes(s: &str) -> Vec<LiquifyStroke> {
    s.lines().filter_map(LiquifyStroke::parse).collect()
}

/// Displacement mesh: offsets (layer pixels) at grid nodes spaced `cell` apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub nx: usize,
    pub ny: usize,
    pub cell: f64,
    pub d: Vec<[f64; 2]>,
    pub freeze: Vec<f64>,
}

impl Mesh {
    fn new(w: f64, h: f64, cell: f64) -> Mesh {
        let nx = (w / cell).ceil() as usize + 1;
        let ny = (h / cell).ceil() as usize + 1;
        Mesh { nx, ny, cell, d: vec![[0.0; 2]; nx * ny], freeze: vec![0.0; nx * ny] }
    }
    /// Bilinear displacement at a layer point (zero outside the mesh).
    pub fn at(&self, x: f64, y: f64) -> [f64; 2] {
        let fx = x / self.cell;
        let fy = y / self.cell;
        if fx < 0.0 || fy < 0.0 || fx > (self.nx - 1) as f64 || fy > (self.ny - 1) as f64 {
            return [0.0; 2];
        }
        let (x0, y0) = ((fx.floor() as usize).min(self.nx - 2), (fy.floor() as usize).min(self.ny - 2));
        let (x1, y1) = ((x0 + 1).min(self.nx - 1), (y0 + 1).min(self.ny - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let g = |i: usize, j: usize| self.d[j * self.nx + i];
        let l = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        l(l(g(x0, y0), g(x1, y0), tx), l(g(x0, y1), g(x1, y1), tx), ty)
    }
}

/// Replay strokes into a displacement mesh over a `w`×`h` layer. `frozen(x, y)` reports
/// mask-frozen points (Freeze Area Mask).
pub fn build_mesh(strokes: &[LiquifyStroke], w: f64, h: f64, frozen: &(dyn Fn(f64, f64) -> bool + Sync)) -> Mesh {
    let cell = (w.max(h) / 128.0).clamp(2.0, 16.0);
    let mut m = Mesh::new(w, h, cell);
    for (i, f) in m.freeze.iter_mut().enumerate() {
        let (x, y) = ((i % m.nx) as f64 * cell, (i / m.nx) as f64 * cell);
        if frozen(x, y) {
            *f = 1.0;
        }
    }
    for (si, s) in strokes.iter().enumerate() {
        let r = (s.size * 0.5).max(1.0);
        let pr = (s.pressure / 100.0).clamp(0.0, 1.0);
        // Dabs every r/4 along the stroke; each dab knows the local motion of the brush.
        let mut dabs: Vec<([f64; 2], [f64; 2])> = vec![];
        if s.points.len() == 1 {
            dabs.push((s.points[0], [0.0; 2]));
        }
        for seg in s.points.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let n = ((len / (r * 0.25)).ceil() as usize).max(1);
            let step = [(b[0] - a[0]) / n as f64, (b[1] - a[1]) / n as f64];
            for k in 1..=n {
                dabs.push(([a[0] + step[0] * k as f64, a[1] + step[1] * k as f64], step));
            }
        }
        for (di, (c, mv)) in dabs.iter().enumerate() {
            let (gx0, gx1) = (((c[0] - r) / cell).floor().max(0.0) as usize, (((c[0] + r) / cell).ceil() as usize).min(m.nx - 1));
            let (gy0, gy1) = (((c[1] - r) / cell).floor().max(0.0) as usize, (((c[1] + r) / cell).ceil() as usize).min(m.ny - 1));
            let prev = m.d.clone();
            for gy in gy0..=gy1 {
                for gx in gx0..=gx1 {
                    let (x, y) = (gx as f64 * cell, gy as f64 * cell);
                    let (dx, dy) = (x - c[0], y - c[1]);
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= r {
                        continue;
                    }
                    let i = gy * m.nx + gx;
                    // Smooth brush falloff.
                    let fall = {
                        let t = 1.0 - dist / r;
                        t * t * (3.0 - 2.0 * t)
                    };
                    let tool = LIQUIFY_TOOLS[s.tool];
                    if tool == "freeze" || tool == "thaw" {
                        let v = pr * fall;
                        m.freeze[i] = if tool == "freeze" { m.freeze[i].max(v) } else { (m.freeze[i] - v).max(0.0) };
                        continue;
                    }
                    let w = pr * fall * (1.0 - m.freeze[i]) * 0.5;
                    if w <= 0.0 {
                        continue;
                    }
                    let d = &mut m.d[i];
                    match tool {
                        "warp" => {
                            d[0] -= mv[0] * w * 2.0;
                            d[1] -= mv[1] * w * 2.0;
                        }
                        "turbulence" => {
                            let j = s.jitter / 100.0 * r * 0.1;
                            let seed = (si as u32).wrapping_mul(7919) ^ di as u32;
                            d[0] += (hash1(gx as u32, gy as u32, seed) as f64 * 2.0 - 1.0) * j * w;
                            d[1] += (hash1(gx as u32, gy as u32, seed ^ 0x55) as f64 * 2.0 - 1.0) * j * w;
                            d[0] -= mv[0] * w;
                            d[1] -= mv[1] * w;
                        }
                        "twirlClockwise" | "twirlCounterclockwise" => {
                            let sgn = if tool == "twirlClockwise" { -1.0 } else { 1.0 };
                            let a = sgn * 0.08 * w;
                            let (sn, cs) = a.sin_cos();
                            d[0] += dx * cs - dy * sn - dx;
                            d[1] += dx * sn + dy * cs - dy;
                        }
                        "pucker" => {
                            d[0] += dx * 0.05 * w;
                            d[1] += dy * 0.05 * w;
                        }
                        "bloat" => {
                            d[0] -= dx * 0.05 * w;
                            d[1] -= dy * 0.05 * w;
                        }
                        "shiftPixels" => {
                            // Perpendicular to the stroke (to its left).
                            d[0] -= mv[1] * w * 2.0;
                            d[1] += mv[0] * w * 2.0;
                        }
                        "reflection" => {
                            // Mirror across the stroke line through the dab.
                            let l = (mv[0] * mv[0] + mv[1] * mv[1]).sqrt();
                            if l > 1e-9 {
                                let n = [-mv[1] / l, mv[0] / l];
                                let k = dx * n[0] + dy * n[1];
                                let tgt = [-2.0 * k * n[0], -2.0 * k * n[1]];
                                d[0] += (tgt[0] - d[0]) * w;
                                d[1] += (tgt[1] - d[1]) * w;
                            }
                        }
                        "clone" => {
                            // Copy the distortion found at the clone source.
                            let sx = x + s.clone_offset[0];
                            let sy = y + s.clone_offset[1];
                            let fx = (sx / cell).round().clamp(0.0, (m.nx - 1) as f64) as usize;
                            let fy = (sy / cell).round().clamp(0.0, (m.ny - 1) as f64) as usize;
                            let src = prev[fy * m.nx + fx];
                            let tgt = [src[0] + s.clone_offset[0], src[1] + s.clone_offset[1]];
                            d[0] += (tgt[0] - d[0]) * w;
                            d[1] += (tgt[1] - d[1]) * w;
                        }
                        // Reconstruction: relax toward no distortion.
                        _ => {
                            d[0] *= 1.0 - w;
                            d[1] *= 1.0 - w;
                        }
                    }
                }
            }
        }
    }
    m
}

type MeshCache = Mutex<HashMap<u64, Arc<Mesh>>>;

/// Liquify's displacement mesh for the instance (replayed strokes, cached; also used by the
/// GPU kernels).
pub fn liquify_mesh(ctx: &EffectCtx) -> Arc<Mesh> {
    use std::hash::{Hash, Hasher};
    let text = ctx.params.s("distortionMesh");
    let mask = ctx.params.f("freezeAreaMask").round() as usize;
    let poly = mask.checked_sub(1).and_then(|i| ctx.env.masks.get(i)).cloned();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    ctx.layer_size[0].to_bits().hash(&mut h);
    ctx.layer_size[1].to_bits().hash(&mut h);
    if let Some(p) = &poly {
        for q in &p.points {
            q[0].to_bits().hash(&mut h);
            q[1].to_bits().hash(&mut h);
        }
        p.inverted.hash(&mut h);
    }
    let key = h.finish();
    static CACHE: OnceLock<MeshCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(m) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return m;
    }
    let strokes = parse_strokes(text);
    let frozen = |x: f64, y: f64| match &poly {
        Some(p) => point_in_poly(&p.points, x, y) != p.inverted,
        None => false,
    };
    let m = Arc::new(build_mesh(&strokes, ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0), &frozen));
    if let Ok(mut c) = cache.lock() {
        if c.len() > 32 {
            c.clear();
        }
        c.insert(key, m.clone());
    }
    m
}

fn liquify(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pct = ctx.params.f("distortionPercentage") / 100.0;
    let show_freeze = ctx.params.b("viewFreezeAreaMask");
    let show_mesh = ctx.params.b("viewMesh");
    if ctx.params.s("distortionMesh").trim().is_empty() && !show_freeze && !show_mesh {
        return b;
    }
    let mesh = liquify_mesh(ctx);
    let off = ctx.params.v2("distortionMeshOffset");
    let mesh_color = ctx.params.color("meshColor");
    let src = b.img.clone();
    let (scale, o) = (b.scale, b.offset);
    let line_w = 0.6 / scale.max(1e-6);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let lx = (x as f64 + 0.5 - o[0]) / scale;
            let ly = (y as f64 + 0.5 - o[1]) / scale;
            let d = mesh.at(lx - off[0], ly - off[1]);
            let (sx, sy) = (lx + d[0] * pct, ly + d[1] * pct);
            let mut c = if d == [0.0; 2] { *px } else { src.sample_bilinear(sx * scale + o[0], sy * scale + o[1]) };
            if show_freeze {
                let fx = ((lx - off[0]) / mesh.cell).round().clamp(0.0, (mesh.nx - 1) as f64) as usize;
                let fy = ((ly - off[1]) / mesh.cell).round().clamp(0.0, (mesh.ny - 1) as f64) as usize;
                let f = mesh.freeze[fy * mesh.nx + fx] as f32;
                if f > 0.0 {
                    c[0] = c[0] * (1.0 - 0.5 * f) + 0.5 * f;
                    c[3] = c[3].max(0.5 * f);
                }
            }
            if show_mesh {
                // Grid lines every 4 mesh cells, displaced with the mesh.
                let gs = mesh.cell * 4.0;
                let (gx, gy) = ((lx + d[0] * pct - off[0]).rem_euclid(gs), (ly + d[1] * pct - off[1]).rem_euclid(gs));
                if gx < line_w || gy < line_w {
                    c = [mesh_color[0], mesh_color[1], mesh_color[2], 1.0];
                }
            }
            *px = c;
        }
    });
    b
}

// ---------------------------------------------------------------- Rolling Shutter Repair

/// Per-pixel velocity field (layer pixels per frame) on a coarse grid, from tracked points.
struct Flow {
    nx: usize,
    ny: usize,
    cell: f64,
    v: Vec<[f64; 2]>,
}

impl Flow {
    fn at(&self, x: f64, y: f64) -> [f64; 2] {
        let fx = (x / self.cell - 0.5).clamp(0.0, (self.nx - 1) as f64);
        let fy = (y / self.cell - 0.5).clamp(0.0, (self.ny - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.nx - 1), (y0 + 1).min(self.ny - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let g = |i: usize, j: usize| self.v[j * self.nx + i];
        let l = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        l(l(g(x0, y0), g(x1, y0), tx), l(g(x0, y1), g(x1, y1), tx), ty)
    }
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Measure motion around the current frame (`cur`) from `prev` to `next` (all in the same
/// buffer grid). `per_row`: Warp method (one velocity per scanline band).
/// `detail` (Pixel Motion Detail, 0..1; 0.2 = the standard grid) scales the number of motion
/// vectors the Pixel Motion method measures: more vectors follow finer local motion.
fn measure(cur: &Image, prev: Option<&Image>, next: Option<&Image>, scale: f64, detailed: bool, per_row: bool, detail: f64) -> Option<Flow> {
    use effectcraft_track::klt::{GrayPyramid, LkOpts, analysis_factor, track};
    let factor = analysis_factor(cur.width, cur.height, if detailed { 640 } else { 320 });
    let pc = GrayPyramid::from_image(cur, [0.0; 2], factor, 4);
    let (w, h) = (pc.width(), pc.height());
    let base = if detailed { 24.0 } else { 12.0 };
    let n = if per_row { base as usize } else { (base * (0.5 + detail.clamp(0.0, 1.0) * 2.5)).round().clamp(4.0, 96.0) as usize };
    let cell_w = w as f64 / n as f64;
    let rows = ((n as f64) * h as f64 / w.max(1) as f64).round().max(2.0) as usize;
    let cell_h = h as f64 / rows as f64;
    let pts: Vec<[f64; 2]> = (0..rows).flat_map(|j| (0..n).map(move |i| [(i as f64 + 0.5) * cell_w, (j as f64 + 0.5) * cell_h])).collect();
    let o = LkOpts { radius: 4, fb_max: 1.0, ..Default::default() };
    let go = |other: Option<&Image>| other.map(|img| track(&pc, &GrayPyramid::from_image(img, [0.0; 2], factor, 4), &pts, None, &o));
    let (tn, tp) = (go(next), go(prev));
    // Velocity in plane pixels per frame.
    let vel: Vec<Option<[f64; 2]>> = (0..pts.len())
        .map(|i| {
            let p = pts[i];
            let fw = tn.as_ref().and_then(|t| t[i]).map(|q| [q[0] - p[0], q[1] - p[1]]);
            let bw = tp.as_ref().and_then(|t| t[i]).map(|q| [p[0] - q[0], p[1] - q[1]]);
            match (fw, bw) {
                (Some(a), Some(b)) => Some([(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]),
                (a, b) => a.or(b),
            }
        })
        .collect();
    let valid: Vec<[f64; 2]> = vel.iter().flatten().copied().collect();
    if valid.is_empty() {
        return None;
    }
    let mut vx: Vec<f64> = valid.iter().map(|v| v[0]).collect();
    let mut vy: Vec<f64> = valid.iter().map(|v| v[1]).collect();
    let global = [median(&mut vx), median(&mut vy)];
    let k = factor as f64 / scale.max(1e-9);
    let mut v = vec![[0.0; 2]; n * rows];
    for j in 0..rows {
        // Row (band) median for Warp; per point (outliers replaced) for Pixel Motion.
        let band: Vec<[f64; 2]> = (0..n).filter_map(|i| vel[j * n + i]).collect();
        let row_med = if band.is_empty() {
            global
        } else {
            let mut a: Vec<f64> = band.iter().map(|v| v[0]).collect();
            let mut b: Vec<f64> = band.iter().map(|v| v[1]).collect();
            [median(&mut a), median(&mut b)]
        };
        for i in 0..n {
            let s = if per_row {
                row_med
            } else {
                match vel[j * n + i] {
                    Some(q) if (q[0] - row_med[0]).abs() + (q[1] - row_med[1]).abs() < 3.0 + 0.5 * (row_med[0].abs() + row_med[1].abs()) => q,
                    _ => row_med,
                }
            };
            v[j * n + i] = [s[0] * k, s[1] * k];
        }
    }
    // Cell size in layer pixels (grid is uniform in plane pixels; assume square plane pixels).
    Some(Flow { nx: n, ny: rows, cell: cell_w * k, v })
}

fn same_grid(nb: Buf, b: &Buf) -> Image {
    if nb.img.width == b.img.width && nb.img.height == b.img.height && nb.offset == b.offset && nb.scale == b.scale {
        return nb.img;
    }
    let k = nb.scale / b.scale.max(1e-9);
    crate::util::gen_image(b.img.width, b.img.height, |x, y| {
        let lx = (x as f64 + 0.5 - b.offset[0]) * k + nb.offset[0];
        let ly = (y as f64 + 0.5 - b.offset[1]) * k + nb.offset[1];
        nb.img.sample_bilinear(lx, ly)
    })
}

fn rolling_shutter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let rate = ctx.params.f("rollingShutterRate") / 100.0;
    if rate == 0.0 {
        return b;
    }
    let Some(host) = ctx.env.host else { return b };
    let fps = ctx.fps();
    let prev = host.self_at(ctx.time - 1.0 / fps, ctx.env.effect_index).map(|nb| same_grid(nb, &b));
    let next = host.self_at(ctx.time + 1.0 / fps, ctx.env.effect_index).map(|nb| same_grid(nb, &b));
    if prev.is_none() && next.is_none() {
        return b;
    }
    let per_row = ctx.params.e("advanced/method") == 0;
    let Some(flow) = measure(
        &b.img,
        prev.as_ref(),
        next.as_ref(),
        1.0,
        ctx.params.b("advanced/detailedAnalysis"),
        per_row,
        ctx.params.f("advanced/pixelMotionDetail") / 100.0,
    ) else {
        return b;
    };
    let dir = ctx.params.e("scanDirection");
    let src = b.img.clone();
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let s = match dir {
                1 => 1.0 - fy / h,
                2 => fx / w,
                3 => 1.0 - fx / w,
                _ => fy / h,
            };
            let tau = rate * (s - 0.5);
            let v = flow.at(fx, fy);
            *px = src.sample_bilinear_clamped(fx + v[0] * tau, fy + v[1] * tau);
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.distort.ccflomotion",
            "CC Flo Motion",
            vec![
                p("knot1", "Knot 1", Value::Vec2([0.25, 0.5]), ParamUi::Point),
                p("amount1", "Amount 1", num(50.0), slider(-400.0, 100.0, -100.0, 100.0, 1)),
                p("knot2", "Knot 2", Value::Vec2([0.75, 0.5]), ParamUi::Point),
                p("amount2", "Amount 2", num(-50.0), slider(-400.0, 100.0, -100.0, 100.0, 1)),
                p("tileEdges", "Tile Edges", Value::Bool(false), ParamUi::Checkbox),
                p("antialiasing", "Antialiasing", Value::Enum(1), popup(&["Low", "Medium", "High"])),
                p("falloff", "Falloff", num(2.0), slider(0.01, 10.0, 0.1, 5.0, 2)),
            ],
            flo_motion,
        ),
        spec(
            "ec.distort.liquify",
            "Liquify",
            vec![
                p("tool", "Tool", Value::Enum(0), popup(&LIQUIFY_TOOL_LABELS)),
                p("brushSize", "Brush Size", num(64.0), slider(1.0, 600.0, 1.0, 600.0, 0)),
                p("brushPressure", "Brush Pressure", num(50.0), slider(1.0, 100.0, 1.0, 100.0, 0)),
                p("freezeAreaMask", "Freeze Area Mask", num(0.0), ParamUi::Mask),
                p("turbulentJitter", "Turbulent Jitter", num(70.0), slider(1.0, 100.0, 1.0, 100.0, 0)),
                p("cloneOffset", "Clone Offset", Value::Vec2([0.0, 0.0]), ParamUi::Hidden),
                p("reconstructionMode", "Reconstruction Mode", Value::Enum(0), popup(&["Revert", "Displace", "Amplitwist", "Affine"])),
                p("viewFreezeAreaMask", "View Freeze Area Mask", Value::Bool(false), ParamUi::Checkbox),
                p("viewMesh", "View Mesh", Value::Bool(false), ParamUi::Checkbox),
                p("meshSize", "Mesh Size", Value::Enum(1), popup(&["Small", "Medium", "Large"])),
                p("meshColor", "Mesh Color", col(0.5, 0.5, 0.5), ParamUi::Color),
                // The strokes (see `LiquifyStroke`); written by the `liquify.stroke` command.
                p("distortionMesh", "Distortion Mesh", Value::Str(String::new()), ParamUi::Hidden),
                p("distortionMeshOffset", "Distortion Mesh Offset", Value::Vec2([0.0, 0.0]), ParamUi::Point),
                p("distortionPercentage", "Distortion Percentage", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
                p("distortionDisplayQuality", "Distortion Display Quality", Value::Enum(1), popup(&["Draft", "Smooth"])),
            ],
            liquify,
        ),
        spec(
            "ec.distort.rollingshutterrepair",
            "Rolling Shutter Repair",
            vec![
                p("rollingShutterRate", "Rolling Shutter Rate", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("scanDirection", "Scan Direction", Value::Enum(0), popup(&["Top → Bottom", "Bottom → Top", "Left → Right", "Right → Left"])),
                p("advanced/method", "Method", Value::Enum(0), popup(&["Warp", "Pixel Motion"])),
                p("advanced/detailedAnalysis", "Detailed Analysis", Value::Bool(false), ParamUi::Checkbox),
                p("advanced/pixelMotionDetail", "Pixel Motion Detail", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            rolling_shutter,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};

    fn grid_img(w: u32, h: u32) -> Image {
        crate::util::gen_image(w, h, |x, y| {
            let v = if x % 8 == 0 || y % 8 == 0 { 1.0 } else { 0.2 };
            [v, v * 0.5, x as f32 / w as f32, 1.0]
        })
    }

    #[test]
    fn flo_motion_pulls_and_pushes() {
        let img = grid_img(64, 64);
        let out = run_fx(
            "ec.distort.ccflomotion",
            &[("knot1", Value::Vec2([32.0, 32.0])), ("amount1", num(80.0)), ("amount2", num(0.0))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        // The knot itself is fixed; pixels near it are magnified (sampled closer to the knot).
        assert!((out.img.get(32, 32)[2] - img.get(32, 32)[2]).abs() < 0.01);
        assert!((out.img.get(40, 32)[2] - img.get(40, 32)[2]).abs() > 0.01);
        assert!(out.img.get(40, 32)[2] < img.get(40, 32)[2]);
        let none = run_fx("ec.distort.ccflomotion", &[("amount1", num(0.0)), ("amount2", num(0.0))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(none.img.data, img.data);
        let again = run_fx(
            "ec.distort.ccflomotion",
            &[("knot1", Value::Vec2([32.0, 32.0])), ("amount1", num(80.0)), ("amount2", num(0.0))],
            img,
            0.0,
            EffectEnv::default(),
        );
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn liquify_strokes_round_trip_and_warp() {
        let s = LiquifyStroke { tool: 0, size: 30.0, pressure: 80.0, jitter: 70.0, clone_offset: [0.0, 0.0], points: vec![[20.0, 32.0], [40.0, 32.0]] };
        assert_eq!(LiquifyStroke::parse(&s.to_line()), Some(s.clone()));
        assert!(LiquifyStroke::parse("nope 1 2 3 4 5 1,2").is_none());
        let img = grid_img(64, 64);
        let out = run_fx("ec.distort.liquify", &[("distortionMesh", Value::Str(s.to_line()))], img.clone(), 0.0, EffectEnv::default());
        assert_ne!(out.img.data, img.data);
        // Warping to the right: a pixel right of the start now shows content from its left.
        let at = |i: &Image| i.get(36, 32)[2];
        assert!(at(&out.img) < at(&img), "{} vs {}", at(&out.img), at(&img));
        // Far away, untouched.
        assert_eq!(out.img.get(5, 5), img.get(5, 5));
        // Distortion Percentage 0 = identity; deterministic.
        let zero = run_fx(
            "ec.distort.liquify",
            &[("distortionMesh", Value::Str(s.to_line())), ("distortionPercentage", num(0.0))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert_eq!(zero.img.data, img.data);
        let again = run_fx("ec.distort.liquify", &[("distortionMesh", Value::Str(s.to_line()))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(out.img.data, again.img.data);
        // #1194: Distortion Mesh Offset is an exposed, editable control (AE's Distortion ▸
        // Offset) and it moves the mesh: the pattern rendered at x renders at x + offset.
        let spec = crate::find("ec.distort.liquify").unwrap();
        assert!(
            spec.params.iter().any(|ps| ps.id == "distortionMeshOffset" && ps.ui == effectcraft_project::ParamUi::Point),
            "offset is shown in Effect Controls"
        );
        let shifted = run_fx(
            "ec.distort.liquify",
            &[("distortionMesh", Value::Str(s.to_line())), ("distortionMeshOffset", Value::Vec2([20.0, 0.0]))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert_ne!(shifted.img.data, out.img.data, "the offset changes the render");
        // The displaced region's centroid moves right by the offset (the mesh is sampled at
        // layer minus offset), and the row far left of the shifted mesh is back to untouched.
        let centroid = |o: &Image| -> f32 {
            let (mut wsum, mut msum) = (0.0f32, 0.0f32);
            for (i, (p, q)) in o.data.iter().zip(img.data.iter()).enumerate() {
                let d: f32 = (0..4).map(|k| (p[k] - q[k]).abs()).sum();
                wsum += d;
                msum += d * (i % 64) as f32;
            }
            if wsum > 1e-6 { msum / wsum } else { 0.0 }
        };
        let (co, cs) = (centroid(&out.img), centroid(&shifted.img));
        assert!((cs - co - 20.0).abs() < 5.0, "centroids {co} -> {cs}");
        assert_eq!(shifted.img.get(5, 32), img.get(5, 32), "left of the shifted mesh: untouched");
        // Reconstruction undoes the warp; freeze protects.
        let mut rec = s.clone();
        rec.tool = 9;
        rec.pressure = 100.0;
        let lines = format!("{}\n{}\n{}\n{}", s.to_line(), rec.to_line(), rec.to_line(), rec.to_line());
        let m1 = build_mesh(&parse_strokes(&s.to_line()), 64.0, 64.0, &|_, _| false);
        let m2 = build_mesh(&parse_strokes(&lines), 64.0, 64.0, &|_, _| false);
        let mag = |m: &Mesh| m.d.iter().map(|d| d[0].abs() + d[1].abs()).sum::<f64>();
        assert!(mag(&m2) < mag(&m1) * 0.5);
        let frozen = build_mesh(&parse_strokes(&s.to_line()), 64.0, 64.0, &|_, _| true);
        assert_eq!(mag(&frozen), 0.0);
        // Every tool changes (or deliberately keeps) the mesh without NaNs.
        for t in 0..LIQUIFY_TOOLS.len() {
            let st = LiquifyStroke { tool: t, clone_offset: [10.0, 0.0], ..s.clone() };
            let m = build_mesh(&[s.clone(), st], 64.0, 64.0, &|_, _| false);
            assert!(m.d.iter().all(|d| d[0].is_finite() && d[1].is_finite()), "{}", LIQUIFY_TOOLS[t]);
        }
    }

    /// A textured frame panning right at 3 px per frame, captured with a rolling shutter: row
    /// `y` is exposed `(y / h − ½)` frames late (rate 100 %).
    fn texture(x: f64, y: f64) -> f32 {
        let a = ((x * 0.31).sin() * (y * 0.23).cos() + (x * 0.07 + y * 0.11).sin()) as f32;
        0.5 + 0.25 * a
    }
    fn rs_frame(f: f64, shutter: bool) -> Image {
        let (w, h) = (96u32, 64u32);
        crate::util::gen_image(w, h, |x, y| {
            let tau = if shutter { (y as f64 + 0.5) / h as f64 - 0.5 } else { 0.0 };
            let shift = 3.0 * (f + tau);
            let v = texture(x as f64 + 0.5 - shift, y as f64 + 0.5);
            [v, v, v, 1.0]
        })
    }
    struct Pan;
    impl EffectHost for Pan {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            Some(Buf { img: rs_frame(t * 24.0, true), offset: [0.0; 2], scale: 1.0 })
        }
    }

    #[test]
    fn rolling_shutter_repair_unskews_a_pan() {
        let host = Pan;
        let env = EffectEnv { host: Some(&host), frame_rate: 24.0, ..Default::default() };
        let t = 5.0 / 24.0;
        let skewed = rs_frame(5.0, true);
        let truth = rs_frame(5.0, false);
        let err = |a: &Image| {
            let mut s = 0.0;
            let mut n = 0.0;
            for y in 4..60 {
                for x in 12..84 {
                    s += (a.get(x, y)[0] - truth.get(x, y)[0]).abs();
                    n += 1.0;
                }
            }
            s / n
        };
        for method in [0u32, 1] {
            let out = run_fx(
                "ec.distort.rollingshutterrepair",
                &[("rollingShutterRate", num(100.0)), ("advanced/method", Value::Enum(method))],
                skewed.clone(),
                t,
                env,
            );
            assert!(err(&out.img) < err(&skewed) * 0.4, "method {method}: {} vs {}", err(&out.img), err(&skewed));
            let again = run_fx(
                "ec.distort.rollingshutterrepair",
                &[("rollingShutterRate", num(100.0)), ("advanced/method", Value::Enum(method))],
                skewed.clone(),
                t,
                env,
            );
            assert_eq!(out.img.data, again.img.data);
        }
        // Pixel Motion Detail changes how many vectors Pixel Motion measures (and still repairs).
        let detail = |d: f64| {
            run_fx(
                "ec.distort.rollingshutterrepair",
                &[("rollingShutterRate", num(100.0)), ("advanced/method", Value::Enum(1)), ("advanced/pixelMotionDetail", num(d))],
                skewed.clone(),
                t,
                env,
            )
            .img
        };
        let (fine, coarse) = (detail(100.0), detail(0.0));
        assert_ne!(fine.data, coarse.data);
        assert!(err(&fine) < err(&skewed) * 0.5 && err(&coarse) < err(&skewed) * 0.5);
        let none = run_fx("ec.distort.rollingshutterrepair", &[], skewed.clone(), t, EffectEnv::default());
        assert_eq!(none.img.data, skewed.data);
    }
}
