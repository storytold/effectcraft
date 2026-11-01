//! Shape-layer path operators (After Effects "Add ▸" path operations).
//!
//! Every operator maps a list of paths to a list of paths of the same length (one entry per shape
//! above the operator), except [`merge`], which combines them into one. Paths are in the space of
//! the shape group the operator lives in.

use std::f64::consts::PI;

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, PathEl, PathSeg, Point, Vec2};

use crate::fit::{end_tangent, start_tangent, unit};
use crate::{FillRule, Join};

const ACC: f64 = 1e-3;

// ------------------------------------------------------------------------------------------------
// Subpath helpers

/// One subpath as a list of segments.
#[derive(Clone, Debug)]
struct Sub {
    segs: Vec<PathSeg>,
    closed: bool,
}

fn degenerate(s: &PathSeg) -> bool {
    let c = s.to_cubic();
    let d = |a: Point, b: Point| a.distance(b) < 1e-9;
    d(c.p0, c.p1) && d(c.p0, c.p2) && d(c.p0, c.p3)
}

fn subs(p: &BezPath) -> Vec<Sub> {
    p.subpaths()
        .filter_map(|els| {
            let closed = matches!(els.last(), Some(PathEl::ClosePath));
            let bp = BezPath::from_vec(els.to_vec());
            let segs: Vec<PathSeg> = bp.segments().filter(|s| !degenerate(s)).collect();
            (!segs.is_empty()).then_some(Sub { segs, closed })
        })
        .collect()
}

fn push_seg(out: &mut BezPath, seg: PathSeg) {
    match seg {
        PathSeg::Line(l) => out.line_to(l.p1),
        PathSeg::Quad(q) => out.quad_to(q.p1, q.p2),
        PathSeg::Cubic(c) => out.curve_to(c.p1, c.p2, c.p3),
    }
}

fn seg_start_tan(s: &PathSeg) -> Vec2 {
    start_tangent(&s.to_cubic())
}

fn seg_end_tan(s: &PathSeg) -> Vec2 {
    end_tangent(&s.to_cubic())
}

/// Unit tangent of a segment at parameter `t` (robust at retracted handles).
fn seg_tan_at(s: &PathSeg, t: f64) -> Vec2 {
    let c = s.to_cubic();
    if t <= 1e-9 {
        return start_tangent(&c);
    }
    if t >= 1.0 - 1e-9 {
        return end_tangent(&c);
    }
    unit(c.deriv().eval(t).to_vec2()).unwrap_or_else(|| start_tangent(&c))
}

fn perp(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

fn last_point(p: &BezPath) -> Option<Point> {
    p.elements().last().and_then(|e| e.end_point())
}

/// Reverse the direction of every subpath (the Path Direction toggle).
pub fn reverse(paths: &[BezPath]) -> Vec<BezPath> {
    paths.iter().map(|p| p.reverse_subpaths()).collect()
}

/// Total arc length of a path.
pub fn length(p: &BezPath) -> f64 {
    p.segments().map(|s| s.arclen(ACC)).sum()
}

/// Number of on-curve vertices of a path (closing duplicates excluded).
pub fn vertex_count(p: &BezPath) -> usize {
    subs(p).iter().map(|s| s.segs.len() + usize::from(!s.closed)).sum()
}

// ------------------------------------------------------------------------------------------------
// Trim Paths

/// A path measured for trimming.
struct Measured {
    segs: Vec<PathSeg>,
    lens: Vec<f64>,
    total: f64,
}

impl Measured {
    fn new(p: &BezPath) -> Self {
        let segs: Vec<PathSeg> = p.segments().filter(|s| !degenerate(s)).collect();
        let lens: Vec<f64> = segs.iter().map(|s| s.arclen(ACC)).collect();
        let total = lens.iter().sum();
        Measured { segs, lens, total }
    }

    /// Append the portion between absolute lengths `sa..sb`, continuing the current subpath when
    /// the piece starts where `out` ends (e.g. across the start point of a closed path).
    fn append(&self, sa: f64, sb: f64, out: &mut BezPath) {
        let mut acc = 0.0;
        for (seg, &len) in self.segs.iter().zip(&self.lens) {
            let (s0, s1) = (acc, acc + len);
            acc = s1;
            if s1 <= sa || s0 >= sb || len <= 0.0 {
                continue;
            }
            let t0 = if sa > s0 { seg.inv_arclen(sa - s0, ACC) } else { 0.0 };
            let t1 = if sb < s1 { seg.inv_arclen(sb - s0, ACC) } else { 1.0 };
            if t1 <= t0 {
                continue;
            }
            let piece = seg.subsegment(t0..t1);
            if last_point(out).is_none_or(|q| q.distance(piece.start()) > 1e-6) {
                out.move_to(piece.start());
            }
            push_seg(out, piece);
        }
    }
}

/// Fractional intervals kept by a trim, or `None` when nothing is trimmed.
fn trim_intervals(start: f64, end: f64, offset_deg: f64) -> Option<Vec<(f64, f64)>> {
    let (mut s, mut e) = (start / 100.0, end / 100.0);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    let (s, e) = (s.clamp(0.0, 1.0), e.clamp(0.0, 1.0));
    if e - s >= 1.0 - 1e-9 {
        return None;
    }
    if e - s <= 1e-9 {
        return Some(vec![]);
    }
    let off = (offset_deg / 360.0).rem_euclid(1.0);
    let (s, e) = (s + off, e + off);
    Some(if e <= 1.0 {
        vec![(s, e)]
    } else if s >= 1.0 {
        vec![(s - 1.0, e - 1.0)]
    } else {
        vec![(s, 1.0), (0.0, e - 1.0)]
    })
}

/// Trim Paths, "Trim Multiple Shapes: Simultaneously": every path is trimmed by the same
/// fractions of its own length at the same time. `start`/`end` in percent, `offset` in degrees.
/// Paths trimmed away entirely come back empty (the list keeps its length).
pub fn trim(paths: &[BezPath], start: f64, end: f64, offset_deg: f64) -> Vec<BezPath> {
    let Some(iv) = trim_intervals(start, end, offset_deg) else { return paths.to_vec() };
    paths
        .iter()
        .map(|p| {
            let m = Measured::new(p);
            let mut out = BezPath::new();
            if m.total > 0.0 {
                for &(a, b) in &iv {
                    m.append(a * m.total, b * m.total, &mut out);
                }
            }
            out
        })
        .collect()
}

/// Trim Paths, "Trim Multiple Shapes: Individually": the paths are laid end to end in stacking
/// order and trimmed as one continuous length, so they draw on one after another.
pub fn trim_individually(paths: &[BezPath], start: f64, end: f64, offset_deg: f64) -> Vec<BezPath> {
    let Some(iv) = trim_intervals(start, end, offset_deg) else { return paths.to_vec() };
    let ms: Vec<Measured> = paths.iter().map(Measured::new).collect();
    let total: f64 = ms.iter().map(|m| m.total).sum();
    let mut base = 0.0;
    ms.iter()
        .map(|m| {
            let mut out = BezPath::new();
            for &(a, b) in &iv {
                let (ga, gb) = (a * total - base, b * total - base);
                let (la, lb) = (ga.max(0.0), gb.min(m.total));
                if lb > la {
                    m.append(la, lb, &mut out);
                }
            }
            base += m.total;
            out
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Pucker & Bloat

/// Pucker & Bloat (`amount` in percent, −100..100). Positive (bloat) moves the vertices towards
/// the path's centre (the mean of its vertices) and pushes the tangent handles away from it, so
/// segments bulge outwards; negative (pucker) does the opposite, giving spiky shapes.
pub fn pucker_bloat(paths: &[BezPath], amount: f64) -> Vec<BezPath> {
    let a = amount / 100.0;
    if a == 0.0 {
        return paths.to_vec();
    }
    paths
        .iter()
        .map(|p| {
            let verts: Vec<Point> = p.elements().iter().filter_map(|e| if matches!(e, PathEl::ClosePath) { None } else { e.end_point() }).collect();
            if verts.is_empty() {
                return p.clone();
            }
            let sum = verts.iter().fold(Vec2::ZERO, |s, q| s + q.to_vec2());
            let c = (sum / verts.len() as f64).to_point();
            let mv = |q: Point| q + (c - q) * a;
            let mvh = |q: Point| q + (q - c) * a;
            let mut out = BezPath::new();
            let mut prev = Point::ZERO;
            let mut start = Point::ZERO;
            for el in p.elements() {
                match *el {
                    PathEl::MoveTo(q) => {
                        out.move_to(mv(q));
                        prev = q;
                        start = q;
                    }
                    PathEl::LineTo(q) => {
                        out.curve_to(mvh(prev), mvh(q), mv(q));
                        prev = q;
                    }
                    PathEl::QuadTo(x, q) => {
                        let c1 = prev + (x - prev) * (2.0 / 3.0);
                        let c2 = q + (x - q) * (2.0 / 3.0);
                        out.curve_to(mvh(c1), mvh(c2), mv(q));
                        prev = q;
                    }
                    PathEl::CurveTo(x, y, q) => {
                        out.curve_to(mvh(x), mvh(y), mv(q));
                        prev = q;
                    }
                    PathEl::ClosePath => {
                        // The implicit closing line becomes a curve too.
                        if prev.distance(start) > 1e-9 {
                            out.curve_to(mvh(prev), mvh(start), mv(start));
                        }
                        out.close_path();
                        prev = start;
                    }
                }
            }
            out
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Round Corners

/// Round Corners: every corner vertex (a tangent discontinuity, e.g. between two lines) is
/// replaced by a circular arc that starts and ends `radius` along the adjacent segments (clamped
/// to half of each segment). Smooth vertices and the ends of open paths are kept.
pub fn round_corners(paths: &[BezPath], radius: f64) -> Vec<BezPath> {
    if radius <= 0.0 || !radius.is_finite() {
        return paths.to_vec();
    }
    paths
        .iter()
        .map(|p| {
            let mut out = BezPath::new();
            for sub in subs(p) {
                round_sub(&sub, radius, &mut out);
            }
            out
        })
        .collect()
}

fn round_sub(sub: &Sub, r: f64, out: &mut BezPath) {
    let segs = &sub.segs;
    let n = segs.len();
    let lens: Vec<f64> = segs.iter().map(|s| s.arclen(ACC)).collect();
    // cut[i]: distance cut away on both sides of vertex i (the start of segment i).
    let mut cut = vec![0.0; n];
    let mut phi = vec![0.0; n];
    for i in 0..n {
        if !sub.closed && i == 0 {
            continue;
        }
        let prev = (i + n - 1) % n;
        let (tin, tout) = (seg_end_tan(&segs[prev]), seg_start_tan(&segs[i]));
        let turn = tin.cross(tout).atan2(tin.dot(tout)).abs();
        if !(1e-3..=PI - 1e-3).contains(&turn) {
            continue;
        }
        cut[i] = r.min(lens[prev] / 2.0).min(lens[i] / 2.0);
        phi[i] = turn;
    }
    // Trimmed segments with the original tangents at their cut ends.
    let trimmed: Vec<(PathSeg, Vec2, Vec2)> = (0..n)
        .map(|j| {
            let a = cut[j];
            let b = if sub.closed || j + 1 < n { cut[(j + 1) % n] } else { 0.0 };
            if a <= 0.0 && b <= 0.0 {
                return (segs[j], seg_start_tan(&segs[j]), seg_end_tan(&segs[j]));
            }
            let t0 = if a > 0.0 { segs[j].inv_arclen(a, ACC) } else { 0.0 };
            let t1 = if b > 0.0 { segs[j].inv_arclen((lens[j] - b).max(0.0), ACC) } else { 1.0 }.max(t0);
            (segs[j].subsegment(t0..t1), seg_tan_at(&segs[j], t0), seg_tan_at(&segs[j], t1))
        })
        .collect();
    let arc = |out: &mut BezPath, from: &(PathSeg, Vec2, Vec2), to: &(PathSeg, Vec2, Vec2), corner: Point, phi: f64| {
        let (p, q) = (from.0.end(), to.0.start());
        // Circular arc through the two tangent points: radius = d / tan(φ/2), handle = 4/3·tan(φ/4)·R.
        let k = 4.0 / 3.0 * (phi / 4.0).tan() / (phi / 2.0).tan();
        let (hp, hq) = (p.distance(corner) * k, q.distance(corner) * k);
        out.curve_to(p + from.2 * hp, q - to.1 * hq, q);
    };
    let piece = |out: &mut BezPath, t: &(PathSeg, Vec2, Vec2)| {
        if !degenerate(&t.0) {
            push_seg(out, t.0);
        }
    };
    out.move_to(trimmed[0].0.start());
    piece(out, &trimmed[0]);
    for j in 1..n {
        if cut[j] > 0.0 {
            arc(out, &trimmed[j - 1], &trimmed[j], segs[j].start(), phi[j]);
        }
        piece(out, &trimmed[j]);
    }
    if sub.closed {
        if cut[0] > 0.0 {
            arc(out, &trimmed[n - 1], &trimmed[0], segs[0].start(), phi[0]);
        }
        out.close_path();
    }
}

// ------------------------------------------------------------------------------------------------
// Offset Paths

/// Offset Paths: grows (positive `amount`) or shrinks (negative) every path with the given line
/// join. `copies` copies are made, copy *i* (0-based) offset by `amount · (i + 1 + copy_offset)`,
/// all returned in the same compound path.
pub fn offset(paths: &[BezPath], amount: f64, join: Join, miter_limit: f64, copies: f64, copy_offset: f64) -> Vec<BezPath> {
    let n = copies.round().clamp(1.0, 1000.0) as usize;
    paths
        .iter()
        .map(|p| {
            let mut out = BezPath::new();
            for i in 0..n {
                let d = amount * (i as f64 + 1.0 + copy_offset);
                out.extend(crate::offset::offset_path(p, d, join, miter_limit).elements().iter().copied());
            }
            out
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Twist

/// Twist: rotates the paths about `center` by an angle that grows linearly with the distance
/// from the centre, from 0 at the centre to `angle_deg` at the farthest point, so the middle twists
/// most sharply. Segments are subdivided and their handles mapped through the exact derivative of
/// the warp, which keeps the result smooth.
pub fn twist(paths: &[BezPath], angle_deg: f64, center: [f64; 2]) -> Vec<BezPath> {
    let c = Point::new(center[0], center[1]);
    let mut rmax: f64 = 0.0;
    for p in paths {
        for el in p.elements() {
            if let Some(q) = el.end_point() {
                rmax = rmax.max(q.distance(c));
            }
        }
    }
    let a = angle_deg.to_radians();
    if a == 0.0 || rmax <= 1e-9 || !a.is_finite() {
        return paths.to_vec();
    }
    let k = a / rmax;
    let rot = |v: Vec2, th: f64| {
        let (s, co) = th.sin_cos();
        Vec2::new(v.x * co - v.y * s, v.x * s + v.y * co)
    };
    let map = |p: Point| {
        let v = p - c;
        c + rot(v, k * v.hypot())
    };
    // Derivative of the warp at `p` applied to `dv`.
    let jac = |p: Point, dv: Vec2| {
        let v = p - c;
        let r = v.hypot();
        let mut w = dv;
        if r > 1e-12 {
            w += perp(v) * (k * v.dot(dv) / r);
        }
        rot(w, k * r)
    };
    paths
        .iter()
        .map(|p| {
            let mut out = BezPath::new();
            for sub in subs(p) {
                out.move_to(map(sub.segs[0].start()));
                for seg in &sub.segs {
                    let cb = seg.to_cubic();
                    let len = seg.arclen(ACC);
                    let m = ((a.abs() * len / rmax) / 0.15 + len / 50.0).ceil().clamp(1.0, 64.0) as usize;
                    for i in 0..m {
                        let q = cb.subsegment(i as f64 / m as f64..(i + 1) as f64 / m as f64);
                        let (p0, p3) = (map(q.p0), map(q.p3));
                        out.curve_to(p0 + jac(q.p0, q.p1 - q.p0), p3 + jac(q.p3, q.p2 - q.p3), p3);
                    }
                }
                if sub.closed {
                    out.close_path();
                }
            }
            out
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Resampling (Zig Zag, Wiggle Paths)

#[derive(Clone, Copy, Debug)]
struct Sample {
    p: Point,
    tan: Vec2,
    /// Length of the piece before / after this sample.
    len_prev: f64,
    len_next: f64,
}

/// Split every segment into `m` pieces of equal arc length. Original vertices get the average of
/// their incoming and outgoing tangents (so a corner's normal is its bisector).
fn resample(sub: &Sub, m: usize) -> Vec<Sample> {
    let segs = &sub.segs;
    let n = segs.len();
    let lens: Vec<f64> = segs.iter().map(|s| s.arclen(ACC)).collect();
    let mut out = Vec::with_capacity(n * m + 1);
    for j in 0..n {
        let piece = lens[j] / m as f64;
        for k in 0..m {
            if k == 0 {
                let tout = seg_start_tan(&segs[j]);
                let prev = if sub.closed || j > 0 { Some((j + n - 1) % n) } else { None };
                let tan = match prev {
                    Some(pj) => unit(seg_end_tan(&segs[pj]) + tout).unwrap_or(tout),
                    None => tout,
                };
                let len_prev = prev.map_or(0.0, |pj| lens[pj] / m as f64);
                out.push(Sample { p: segs[j].start(), tan, len_prev, len_next: piece });
            } else {
                let t = segs[j].inv_arclen(piece * k as f64, ACC);
                out.push(Sample { p: segs[j].eval(t), tan: seg_tan_at(&segs[j], t), len_prev: piece, len_next: piece });
            }
        }
    }
    if !sub.closed {
        let last = &segs[n - 1];
        out.push(Sample { p: last.end(), tan: seg_end_tan(last), len_prev: lens[n - 1] / m as f64, len_next: 0.0 });
    }
    out
}

/// Emit displaced samples as corner (polyline) or smooth (tangent-aligned cubic) points.
/// `handle` is the handle length as a fraction of the adjacent piece length.
fn emit(samples: &[Sample], disp: &[Vec2], closed: bool, smooth: bool, handle: f64, out: &mut BezPath) {
    let n = samples.len();
    if n == 0 {
        return;
    }
    let pt = |i: usize| samples[i].p + disp[i];
    out.move_to(pt(0));
    let count = if closed { n } else { n - 1 };
    for i in 0..count {
        let j = (i + 1) % n;
        if smooth {
            let (a, b) = (&samples[i], &samples[j]);
            out.curve_to(pt(i) + a.tan * (a.len_next * handle), pt(j) - b.tan * (b.len_prev * handle), pt(j));
        } else {
            out.line_to(pt(j));
        }
    }
    if closed {
        out.close_path();
    }
}

/// Zig Zag: each segment gets `ridges` ridges — it is split into `ridges + 1` equal pieces and the
/// points (original vertices included) are pushed alternately `size` to either side of the path.
/// `smooth` makes rounded waves instead of sharp zig zags. A closed path with `n` segments ends up
/// with `n · (ridges + 1)` vertices (open: one more).
pub fn zigzag(paths: &[BezPath], size: f64, ridges: f64, smooth: bool) -> Vec<BezPath> {
    let m = ridges.round().clamp(0.0, 1000.0) as usize + 1;
    paths
        .iter()
        .map(|p| {
            let mut out = BezPath::new();
            for sub in subs(p) {
                let s = resample(&sub, m);
                let disp: Vec<Vec2> = s.iter().enumerate().map(|(i, x)| perp(x.tan) * if i % 2 == 0 { size } else { -size }).collect();
                emit(&s, &disp, sub.closed, smooth, 0.36, &mut out);
            }
            out
        })
        .collect()
}

/// Wiggle Paths parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WiggleParams {
    /// Maximum displacement (px).
    pub size: f64,
    /// Subdivisions per segment.
    pub detail: f64,
    /// Smooth (true) or corner points.
    pub smooth: bool,
    /// Wiggles per second.
    pub speed: f64,
    /// 0..100 %: how much neighbouring points move together.
    pub correlation: f64,
    /// Temporal phase in degrees (360° = one wiggle period).
    pub phase_deg: f64,
    /// Spatial phase in degrees: slides each subpath's pattern of displacements along its points
    /// (360° = once around the subpath).
    pub spatial_phase_deg: f64,
    pub seed: f64,
}

impl Default for WiggleParams {
    fn default() -> Self {
        WiggleParams { size: 10.0, detail: 10.0, smooth: true, speed: 2.0, correlation: 50.0, phase_deg: 0.0, spatial_phase_deg: 0.0, seed: 0.0 }
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Deterministic hash → [−1, 1].
fn hash(seed: u64, idx: u64, t: i64) -> f64 {
    let h = splitmix(splitmix(splitmix(seed) ^ idx) ^ t as u64);
    (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
}

/// Smooth 1D value noise in [−1, 1] (cubic Hermite through hashed lattice values).
fn noise(seed: u64, idx: u64, t: f64) -> f64 {
    let i = t.floor();
    let f = t - i;
    let i = i as i64;
    let p = |k: i64| hash(seed, idx, i + k);
    // Catmull-Rom through four lattice values keeps the motion C1-continuous.
    let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
    let v = 0.5 * (2.0 * p1 + (-p0 + p2) * f + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f * f * f);
    v.clamp(-1.0, 1.0)
}

/// Catmull-Rom interpolation between `p1` (f = 0) and `p2` (f = 1).
fn catmull_rom(p0: f64, p1: f64, p2: f64, p3: f64, f: f64) -> f64 {
    0.5 * (2.0 * p1 + (-p0 + p2) * f + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f * f * f)
}

/// Wiggle Paths: subdivides the paths (`detail` points per segment) and displaces every point by
/// smooth random noise of up to `size` pixels that evolves over `time_s` seconds at `speed`
/// wiggles per second. The spatial phase moves each point's own share of the noise along the
/// subpath, and a fraction of a point is interpolated between its neighbors. Deterministic for a
/// given seed and time.
pub fn wiggle(paths: &[BezPath], w: &WiggleParams, time_s: f64) -> Vec<BezPath> {
    if w.size == 0.0 {
        return paths.to_vec();
    }
    let m = w.detail.round().clamp(0.0, 1000.0) as usize + 1;
    let seed = w.seed.round() as i64 as u64;
    let t = time_s * w.speed + w.phase_deg / 360.0;
    let corr = (w.correlation / 100.0).clamp(0.0, 1.0);
    let shared = Vec2::new(noise(seed, u64::MAX, t), noise(seed, u64::MAX - 1, t));
    let turns = if w.spatial_phase_deg.is_finite() { (w.spatial_phase_deg / 360.0).rem_euclid(1.0) } else { 0.0 };
    let mut idx: u64 = 0;
    paths
        .iter()
        .map(|p| {
            let mut out = BezPath::new();
            for sub in subs(p) {
                let s = resample(&sub, m);
                let n = s.len();
                let first = idx;
                let own = |j: usize| {
                    let i = first + j as u64;
                    Vec2::new(noise(seed, i * 2, t + i as f64 * 0.37), noise(seed, i * 2 + 1, t + i as f64 * 0.61))
                };
                let along = |j: usize| {
                    if turns == 0.0 {
                        return own(j);
                    }
                    let u = (j as f64 + turns * n as f64).rem_euclid(n as f64);
                    let k = u.floor();
                    let f = u - k;
                    let k = k as usize % n;
                    if f == 0.0 {
                        return own(k);
                    }
                    let (p0, p1, p2, p3) = (own((k + n - 1) % n), own(k), own((k + 1) % n), own((k + 2) % n));
                    Vec2::new(catmull_rom(p0.x, p1.x, p2.x, p3.x, f).clamp(-1.0, 1.0), catmull_rom(p0.y, p1.y, p2.y, p3.y, f).clamp(-1.0, 1.0))
                };
                let disp: Vec<Vec2> = (0..n).map(|j| (shared * corr + along(j) * (1.0 - corr)) * w.size).collect();
                idx += n as u64;
                emit(&s, &disp, sub.closed, w.smooth, 1.0 / 3.0, &mut out);
            }
            out
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Merge Paths

/// Merge Paths modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MergeMode {
    /// Combine into one compound path (no boolean).
    #[default]
    Merge,
    /// Union.
    Add,
    /// The first path minus all the others.
    Subtract,
    /// Area covered by every path.
    Intersect,
    /// Area covered by an odd number of paths.
    Exclude,
}

impl MergeMode {
    pub fn from_index(i: u32) -> MergeMode {
        match i {
            1 => MergeMode::Add,
            2 => MergeMode::Subtract,
            3 => MergeMode::Intersect,
            4 => MergeMode::Exclude,
            _ => MergeMode::Merge,
        }
    }
}

/// Merge Paths: combine the paths (in stacking order, each filled non-zero) into one path.
pub fn merge(paths: &[BezPath], mode: MergeMode) -> BezPath {
    if mode == MergeMode::Merge {
        let mut out = BezPath::new();
        for p in paths {
            out.extend(p.elements().iter().copied());
        }
        return out;
    }
    let inputs: Vec<(BezPath, FillRule)> = paths.iter().filter(|p| !p.elements().is_empty()).map(|p| (p.clone(), FillRule::NonZero)).collect();
    if inputs.is_empty() {
        return BezPath::new();
    }
    match mode {
        MergeMode::Add => crate::boolean::boolean_n(&inputs, |m| m.iter().any(|&b| b)),
        MergeMode::Subtract => crate::boolean::boolean_n(&inputs, |m| m[0] && !m[1..].iter().any(|&b| b)),
        MergeMode::Intersect => crate::boolean::boolean_n(&inputs, |m| m.iter().all(|&b| b)),
        MergeMode::Exclude => crate::boolean::boolean_n(&inputs, |m| m.iter().filter(|&&b| b).count() % 2 == 1),
        // Handled above (no boolean operation).
        MergeMode::Merge => crate::boolean::boolean_n(&inputs, |m| m.iter().any(|&b| b)),
    }
}

/// Signed area helper used by tests and callers: unsigned non-zero filled area of a path list.
pub fn area(paths: &[BezPath]) -> f64 {
    let mut all = BezPath::new();
    for p in paths {
        all.extend(p.elements().iter().copied());
    }
    crate::boolean::filled_area(&all, FillRule::NonZero)
}

#[cfg(test)]
mod tests;
