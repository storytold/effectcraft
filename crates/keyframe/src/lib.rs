//! Animated values with After Effects keyframe semantics.
//!
//! * **Temporal** interpolation between two keys is linear, Bezier or hold. Bezier eases are given
//!   per dimension as *speed* (value units per second) and *influence* (fraction of the segment's
//!   duration, 0.1 %–100 %), exactly the numbers the Keyframe Velocity dialog shows.
//! * **Spatial** properties (Position, Anchor Point, Point effect params) move along a Bezier motion
//!   path defined by spatial tangents; their temporal ease is a single speed along the path
//!   (pixels/second), evaluated through an arc-length table.
//! * Auto-Bezier and continuous-Bezier keys derive their tangents from their neighbours.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod text_doc;
pub mod value;

use effectcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};
pub use text_doc::{BaselineOption, CharStyle, Composer, Direction, FigureStyle, FigureWidth, Kerning, OpenType, ParaStyle, StyleRun};
pub use value::{FeatherPoint, Gradient, Justify, ShapePath, TextDoc, Value};

/// Temporal interpolation on one side of a keyframe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interp {
    #[default]
    Linear,
    Bezier,
    Hold,
}

impl Interp {
    pub fn label(self) -> &'static str {
        match self {
            Interp::Linear => "Linear",
            Interp::Bezier => "Bezier",
            Interp::Hold => "Hold",
        }
    }
}

/// Temporal ease on one side of a key: speed in units/s and influence 0..1 (fraction of the
/// adjacent segment duration).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ease {
    pub speed: f64,
    pub influence: f64,
}

impl Default for Ease {
    fn default() -> Self {
        Ease { speed: 0.0, influence: 1.0 / 6.0 }
    }
}

impl Ease {
    /// Easy Ease: zero speed, 33.33 % influence.
    pub const EASY: Ease = Ease { speed: 0.0, influence: 1.0 / 3.0 };
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub time: Tick,
    pub value: Value,
    #[serde(default)]
    pub in_interp: Interp,
    #[serde(default)]
    pub out_interp: Interp,
    /// Per-dimension (or one entry for spatial / single-dimension values).
    #[serde(default)]
    pub in_ease: Vec<Ease>,
    #[serde(default)]
    pub out_ease: Vec<Ease>,
    /// Temporal continuous / auto Bezier flags.
    #[serde(default)]
    pub continuous: bool,
    #[serde(default)]
    pub auto_bezier: bool,
    /// Spatial tangents relative to the key value (spatial properties only).
    #[serde(default)]
    pub spatial_in: [f64; 3],
    #[serde(default)]
    pub spatial_out: [f64; 3],
    /// Spatial auto-Bezier: tangents computed from neighbours.
    #[serde(default = "yes")]
    pub spatial_auto: bool,
    #[serde(default)]
    pub spatial_continuous: bool,
    #[serde(default)]
    pub roving: bool,
    /// Keyframe color label: index into the label list (0 = None).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub label: u8,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn yes() -> bool {
    true
}

impl Keyframe {
    pub fn new(time: Tick, value: Value) -> Keyframe {
        Keyframe {
            time,
            value,
            in_interp: Interp::Linear,
            out_interp: Interp::Linear,
            in_ease: vec![],
            out_ease: vec![],
            continuous: false,
            auto_bezier: false,
            spatial_in: [0.0; 3],
            spatial_out: [0.0; 3],
            spatial_auto: true,
            spatial_continuous: false,
            roving: false,
            label: 0,
        }
    }
    pub fn hold(mut self) -> Keyframe {
        self.in_interp = Interp::Hold;
        self.out_interp = Interp::Hold;
        self
    }
    /// Easy Ease on both sides.
    pub fn eased(mut self) -> Keyframe {
        self.in_interp = Interp::Bezier;
        self.out_interp = Interp::Bezier;
        let n = self.value.dims().max(1);
        self.in_ease = vec![Ease::EASY; n];
        self.out_ease = vec![Ease::EASY; n];
        self
    }
    /// Shape of the keyframe icon in the timeline.
    pub fn icon(&self) -> KeyIcon {
        let side = |i: Interp, auto: bool| match i {
            Interp::Hold => KeyHalf::Hold,
            Interp::Linear => KeyHalf::Linear,
            Interp::Bezier if auto => KeyHalf::Auto,
            Interp::Bezier => KeyHalf::Bezier,
        };
        KeyIcon { left: side(self.in_interp, self.auto_bezier), right: side(self.out_interp, self.auto_bezier) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyHalf {
    Linear,
    Bezier,
    Auto,
    Hold,
}

/// Keyframe icon halves (diamond = linear, hourglass = bezier/ease, circle = auto, square = hold).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyIcon {
    pub left: KeyHalf,
    pub right: KeyHalf,
}

fn secs(t: Tick) -> f64 {
    t.0 as f64 / TICKS_PER_SECOND as f64
}

/// Cubic Bezier in 1D.
fn bez(p0: f64, p1: f64, p2: f64, p3: f64, u: f64) -> f64 {
    let v = 1.0 - u;
    v * v * v * p0 + 3.0 * v * v * u * p1 + 3.0 * v * u * u * p2 + u * u * u * p3
}
fn bez_d(p0: f64, p1: f64, p2: f64, p3: f64, u: f64) -> f64 {
    let v = 1.0 - u;
    3.0 * v * v * (p1 - p0) + 6.0 * v * u * (p2 - p1) + 3.0 * u * u * (p3 - p2)
}

/// Solve `bez(x0..x3, u) = x` for u in 0..1 (x curve monotone).
fn solve_u(x0: f64, x1: f64, x2: f64, x3: f64, x: f64) -> f64 {
    if x <= x0 {
        return 0.0;
    }
    if x >= x3 {
        return 1.0;
    }
    if x1 == x3 && x2 == x0 {
        // Full influence on both sides has a flat time derivative at the midpoint.
        // Its exact inverse avoids cancellation and residual-only Newton convergence there.
        let progress = (x - x0) / (x3 - x0);
        return 0.5 + ((progress - 0.5) * 0.25).cbrt();
    }
    let mut u = (x - x0) / (x3 - x0);
    for _ in 0..8 {
        let f = bez(x0, x1, x2, x3, u) - x;
        let d = bez_d(x0, x1, x2, x3, u);
        if d.abs() < 1e-12 {
            break;
        }
        if (f / d).abs() < 1e-12 {
            return u;
        }
        let n = u - f / d;
        if !(0.0..=1.0).contains(&n) {
            break;
        }
        u = n;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) * 0.5;
        if bez(x0, x1, x2, x3, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) * 0.5
}

/// Fraction of progress 0..1 through a 1D Bezier temporal segment whose value goes from 0 to 1,
/// with normalised out/in speeds (speed × duration / Δvalue) and influences.
pub fn ease_progress(out_speed_n: f64, out_inf: f64, in_speed_n: f64, in_inf: f64, x: f64) -> f64 {
    // The time handles may cross. AE keeps both influences; rescaling them changes the ease.
    let (i0, i1) = (out_inf.clamp(0.0, 1.0), in_inf.clamp(0.0, 1.0));
    let u = solve_u(0.0, i0, 1.0 - i1, 1.0, x);
    bez(0.0, out_speed_n * i0, 1.0 - in_speed_n * i1, 1.0, u)
}

/// Whether control point `c` lies on the segment `a`–`b` (within rounding), so a cubic with
/// such handles traces the straight chord.
fn on_chord(a: [f64; 3], c: [f64; 3], b: [f64; 3]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if dd <= 1e-24 {
        return false;
    }
    let t = (v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / dd;
    let off = [v[0] - d[0] * t, v[1] - d[1] * t, v[2] - d[2] * t];
    (0.0..=1.0).contains(&t) && off[0] * off[0] + off[1] * off[1] + off[2] * off[2] <= dd * 1e-18
}

/// Spatial cubic segment with an arc-length table.
struct SpatialSeg {
    p: [[f64; 3]; 4],
    /// Cumulative length at u = i / N.
    table: Vec<f64>,
}

const ARC_N: usize = 96;

/// 5-point Gauss-Legendre nodes and weights on -1..1.
const GAUSS: [(f64, f64); 5] = [
    (0.0, 0.568_888_888_888_888_9),
    (-0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (-0.906_179_845_938_664, 0.236_926_885_056_189_1),
    (0.906_179_845_938_664, 0.236_926_885_056_189_1),
];

impl SpatialSeg {
    fn new(p: [[f64; 3]; 4]) -> SpatialSeg {
        let mut table = Vec::with_capacity(ARC_N + 1);
        table.push(0.0);
        let mut acc = 0.0;
        for i in 1..=ARC_N {
            acc += Self::arc(&p, (i - 1) as f64 / ARC_N as f64, i as f64 / ARC_N as f64);
            table.push(acc);
        }
        SpatialSeg { p, table }
    }
    fn point(p: &[[f64; 3]; 4], u: f64) -> [f64; 3] {
        [bez(p[0][0], p[1][0], p[2][0], p[3][0], u), bez(p[0][1], p[1][1], p[2][1], p[3][1], u), bez(p[0][2], p[1][2], p[2][2], p[3][2], u)]
    }
    /// |dB/du|.
    fn tangent_len(p: &[[f64; 3]; 4], u: f64) -> f64 {
        (0..3).map(|k| bez_d(p[0][k], p[1][k], p[2][k], p[3][k], u).powi(2)).sum::<f64>().sqrt()
    }
    /// Arc length between `a` and `b` (Gauss-Legendre; exact enough on one table cell).
    fn arc(p: &[[f64; 3]; 4], a: f64, b: f64) -> f64 {
        let (m, h) = ((a + b) * 0.5, (b - a) * 0.5);
        GAUSS.iter().map(|(x, w)| w * Self::tangent_len(p, m + h * x)).sum::<f64>() * h
    }
    fn length(&self) -> f64 {
        *self.table.last().unwrap_or(&0.0)
    }
    /// The point at arc length `s`. The table cell's linear guess is refined with Newton steps
    /// on the true arc length, so the position (and the speed graph, its derivative) is smooth
    /// along curved paths instead of piecewise linear per cell (#456).
    fn at_length(&self, s: f64) -> [f64; 3] {
        let len = self.length();
        // A path whose length overflowed (huge coordinates) or is NaN has no arc-length table to
        // walk: `clamp` would panic on a NaN bound.
        if !(len > 0.0 && len.is_finite()) {
            return self.p[0];
        }
        let s = s.clamp(0.0, len);
        let i = self.table.partition_point(|&v| v < s).clamp(1, ARC_N);
        let (Some(&a), Some(&b)) = (self.table.get(i - 1), self.table.get(i)) else { return self.p[0] };
        let (u0, u1) = ((i - 1) as f64 / ARC_N as f64, i as f64 / ARC_N as f64);
        let mut u = u0 + if b > a { (s - a) / (b - a) } else { 0.0 } * (u1 - u0);
        for _ in 0..4 {
            let d = Self::tangent_len(&self.p, u);
            if d <= 1e-12 {
                break;
            }
            let e = a + Self::arc(&self.p, u0, u) - s;
            u = (u - e / d).clamp(u0, u1);
            if e.abs() <= 1e-12 * len.max(1.0) {
                break;
            }
        }
        Self::point(&self.p, u)
    }
}

fn v3(v: &Value) -> [f64; 3] {
    v.as_vec3()
}

/// Auto-Bezier spatial tangents for key `i`, as After Effects computes them (Catmull-Rom): the out
/// tangent is a sixth of the way from the previous key to the next, the in tangent its mirror, and
/// key times play no part. An end key uses its one neighbour, so the path leaves the first key
/// and enters the last one toward the neighbouring key instead of with no tangent.
fn auto_spatial(keys: &[Keyframe], i: usize) -> ([f64; 3], [f64; 3]) {
    if i >= keys.len() || keys.len() < 2 {
        return ([0.0; 3], [0.0; 3]);
    }
    let a = v3(&keys[i.saturating_sub(1)].value);
    let c = v3(&keys[(i + 1).min(keys.len() - 1)].value);
    let tout = [(c[0] - a[0]) / 6.0, (c[1] - a[1]) / 6.0, (c[2] - a[2]) / 6.0];
    // A side facing a key at the same position gets no tangent, so the segment between two
    // identical keys has zero length and the layer holds still (#1675).
    let p = v3(&keys[i].value);
    let tin = if i > 0 && a == p { [0.0; 3] } else { [-tout[0], -tout[1], -tout[2]] };
    let tout = if i + 1 < keys.len() && c == p { [0.0; 3] } else { tout };
    (tin, tout)
}

/// Effective spatial tangents of key `i` (auto or stored).
pub fn spatial_tangents(keys: &[Keyframe], i: usize) -> ([f64; 3], [f64; 3]) {
    let Some(key) = keys.get(i) else { return ([0.0; 3], [0.0; 3]) };
    if key.spatial_auto { auto_spatial(keys, i) } else { (key.spatial_in, key.spatial_out) }
}

/// Spatial tangents of key `i` as motion-path handles: [`spatial_tangents`], with a side that
/// has none but a neighbour (a key whose stored tangents are zero, as on a straight path) pointing
/// where the path leaves the key, toward the segment's next control point, a third of the way to
/// the neighbouring key. After Effects shows Bezier handles there to drag the path into a curve.
pub fn spatial_handles(keys: &[Keyframe], i: usize) -> ([f64; 3], [f64; 3]) {
    let (mut tin, mut tout) = spatial_tangents(keys, i);
    let Some(p) = keys.get(i).map(|k| v3(&k.value)) else { return (tin, tout) };
    // Toward key `j`'s control point on this segment (its in side when `j` follows `i`).
    let toward = |j: usize, after: bool| -> Option<[f64; 3]> {
        let q = v3(&keys.get(j)?.value);
        let (jin, jout) = spatial_tangents(keys, j);
        let t = if after { jin } else { jout };
        let d = [q[0] + t[0] - p[0], q[1] + t[1] - p[1], q[2] + t[2] - p[2]];
        let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let len = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt() / 3.0;
        (dl > 1e-9).then(|| d.map(|v| v * len / dl))
    };
    let none = |t: [f64; 3]| t == [0.0; 3];
    if none(tout)
        && let Some(v) = toward(i + 1, true)
    {
        tout = v;
    }
    if none(tin)
        && let Some(v) = i.checked_sub(1).and_then(|j| toward(j, false))
    {
        tin = v;
    }
    (tin, tout)
}

/// Effective temporal ease of key `i` for dimension `d` (auto-Bezier keys get the neighbour slope).
fn effective_ease(keys: &[Keyframe], i: usize, d: usize, spatial: bool, out: bool) -> Ease {
    let k = &keys[i];
    let list = if out { &k.out_ease } else { &k.in_ease };
    if k.auto_bezier && i > 0 && i + 1 < keys.len() {
        let dt = secs(keys[i + 1].time) - secs(keys[i - 1].time);
        let slope = if spatial {
            let a = v3(&keys[i - 1].value);
            let c = v3(&keys[i + 1].value);
            ((c[0] - a[0]).powi(2) + (c[1] - a[1]).powi(2) + (c[2] - a[2]).powi(2)).sqrt() / dt.max(1e-12)
        } else {
            let a = keys[i - 1].value.components();
            let c = keys[i + 1].value.components();
            (c.get(d).unwrap_or(&0.0) - a.get(d).unwrap_or(&0.0)) / dt.max(1e-12)
        };
        return Ease { speed: slope, influence: 1.0 / 6.0 };
    }
    list.get(d).or(list.first()).copied().unwrap_or_default()
}

/// The ease a key side presents in the Graph Editor and the Keyframe Velocity dialog: stored
/// Bezier eases, auto-Bezier slopes, or the equivalent of a linear side (speed of the straight
/// segment, 33.3 % influence). `None` for hold sides and sides without a neighbour.
pub fn side_ease(keys: &[Keyframe], i: usize, d: usize, spatial: bool, out: bool) -> Option<Ease> {
    let k = keys.get(i)?;
    let j = if out { i + 1 } else { i.checked_sub(1)? };
    let n = keys.get(j)?;
    let interp = if out { k.out_interp } else { k.in_interp };
    if interp == Interp::Hold || (!out && n.out_interp == Interp::Hold) || !k.value.interpolates() {
        return None;
    }
    let is_spatial = spatial && matches!(k.value, Value::Vec2(_) | Value::Vec3(_));
    if interp == Interp::Linear {
        let dur = (secs(k.time) - secs(n.time)).abs().max(1e-12);
        let dv = if is_spatial {
            let (a, b) = if out { (i, j) } else { (j, i) };
            spatial_segment_length(keys, a.min(b))
        } else {
            let a = k.value.components();
            let b = n.value.components();
            let dv = b.get(d).unwrap_or(&0.0) - a.get(d).unwrap_or(&0.0);
            if out { dv } else { -dv }
        };
        return Some(Ease { speed: dv / dur, influence: 1.0 / 3.0 });
    }
    Some(effective_ease(keys, i, if is_spatial { 0 } else { d }, is_spatial, out))
}

/// Arc length of the spatial segment `i` → `i + 1` (motion-path length).
pub fn spatial_segment_length(keys: &[Keyframe], i: usize) -> f64 {
    let Some(next) = i.checked_add(1) else { return 0.0 };
    let (Some(a), Some(b)) = (keys.get(i), keys.get(next)) else { return 0.0 };
    let p0 = v3(&a.value);
    let p3 = v3(&b.value);
    let (_, out_t) = spatial_tangents(keys, i);
    let (in_t, _) = spatial_tangents(keys, next);
    SpatialSeg::new([p0, [p0[0] + out_t[0], p0[1] + out_t[1], p0[2] + out_t[2]], [p3[0] + in_t[0], p3[1] + in_t[1], p3[2] + in_t[2]], p3]).length()
}

/// Re-time roving keyframes: each run of roving keys between two fixed (non-roving) keys gets
/// times proportional to the motion-path length, so the layer moves through them at one
/// constant speed (After Effects' "Rove Across Time"). The first and last keys never rove.
/// Returns true when any key time changed. Key order is preserved.
pub fn retime_roving(keys: &mut [Keyframe], spatial: bool) -> bool {
    let n = keys.len();
    if n < 3 {
        for k in keys.iter_mut() {
            k.roving = false;
        }
        return false;
    }
    keys[0].roving = false;
    keys[n - 1].roving = false;
    if !keys.iter().any(|k| k.roving) {
        return false;
    }
    let mut changed = false;
    let mut a = 0;
    while a < n - 1 {
        let Some(b) = (a + 1..n).find(|&j| !keys[j].roving) else { break };
        if b > a + 1 {
            // Cumulative path length (spatial) or value distance (other) of the run a..=b.
            let mut acc = vec![0.0];
            for i in a..b {
                let len = if spatial && matches!(keys[i].value, Value::Vec2(_) | Value::Vec3(_)) {
                    spatial_segment_length(keys, i)
                } else {
                    let x = keys[i].value.components();
                    let y = keys[i + 1].value.components();
                    x.iter().zip(&y).map(|(p, q)| (q - p) * (q - p)).sum::<f64>().sqrt()
                };
                acc.push(acc.last().unwrap_or(&0.0) + len);
            }
            let total = *acc.last().unwrap_or(&0.0);
            let (t0, t1) = (keys[a].time, keys[b].time);
            for (o, i) in (a + 1..b).enumerate() {
                let f = if total > 1e-9 { acc[o + 1] / total } else { (o + 1) as f64 / (b - a) as f64 };
                let span = i128::from(t1.0) - i128::from(t0.0);
                let mut retimed = i128::from(t0.0) + (span as f64 * f).round() as i128;
                // Keep every roving key strictly between its neighbours: a zero-length segment would otherwise
                // put it on a fixed key's time, and two keys can't share a time.
                let (lo, hi) = (i128::from(keys[i - 1].time.0) + 1, i128::from(t1.0) - (b - i) as i128);
                if lo <= hi {
                    retimed = retimed.clamp(lo, hi);
                }
                let nt = Tick(retimed.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
                if nt != keys[i].time {
                    keys[i].time = nt;
                    changed = true;
                }
                // Roving keys pass speed straight through.
                keys[i].in_interp = Interp::Linear;
                keys[i].out_interp = Interp::Linear;
            }
        }
        a = b;
    }
    changed
}

/// Reverse a run of keys in time within `[first, last]` (Time-Reverse Keyframes): key `i` moves
/// to `first + last - t` and in/out sides swap.
pub fn time_reverse(keys: &mut [Keyframe]) {
    let (Some(first), Some(last)) = (keys.first().map(|k| k.time), keys.last().map(|k| k.time)) else { return };
    for k in keys.iter_mut() {
        // Imported Tick values may span i64's full range; keep the intermediate exact.
        let reversed = i128::from(first.0) + i128::from(last.0) - i128::from(k.time.0);
        k.time = Tick(reversed.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
        std::mem::swap(&mut k.in_interp, &mut k.out_interp);
        std::mem::swap(&mut k.in_ease, &mut k.out_ease);
        std::mem::swap(&mut k.spatial_in, &mut k.spatial_out);
    }
    keys.reverse();
}

/// Evaluate a keyframed property at `t`. `spatial` selects motion-path semantics for 2D/3D values.
pub fn evaluate(keys: &[Keyframe], t: Tick, spatial: bool) -> Option<Value> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some(first.value.clone());
    }
    let last = keys.last()?;
    if t >= last.time {
        return Some(last.value.clone());
    }
    let i = keys.partition_point(|k| k.time <= t) - 1;
    Some(segment_value(keys, i, t, spatial))
}

/// Value inside segment `i` → `i+1` at time `t`.
pub fn segment_value(keys: &[Keyframe], i: usize, t: Tick, spatial: bool) -> Value {
    let a = &keys[i];
    let b = &keys[i + 1];
    if a.out_interp == Interp::Hold || !a.value.interpolates() {
        return a.value.clone();
    }
    let (t0, t1) = (secs(a.time), secs(b.time));
    let dur = (t1 - t0).max(1e-12);
    let x = ((secs(t) - t0) / dur).clamp(0.0, 1.0);
    let lin_out = a.out_interp == Interp::Linear;
    // A Hold in-side only matters for the segment it ends, which the out side above decides:
    // it shapes the incoming segment like a linear side, as in the Graph Editor and Lottie export.
    let lin_in = b.in_interp != Interp::Bezier;
    let is_spatial = spatial && matches!(a.value, Value::Vec2(_) | Value::Vec3(_));
    if is_spatial {
        let p0 = v3(&a.value);
        let p3 = v3(&b.value);
        let (_, out_t) = spatial_tangents(keys, i);
        let (in_t, _) = spatial_tangents(keys, i + 1);
        let seg = SpatialSeg::new([p0, [p0[0] + out_t[0], p0[1] + out_t[1], p0[2] + out_t[2]], [p3[0] + in_t[0], p3[1] + in_t[1], p3[2] + in_t[2]], p3]);
        let len = seg.length();
        let f = if lin_out && lin_in || len <= 1e-12 {
            x
        } else {
            let eo = if lin_out { Ease { speed: len / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i, 0, true, true) };
            let ei = if lin_in { Ease { speed: len / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i + 1, 0, true, false) };
            ease_progress(eo.speed * dur / len, eo.influence, ei.speed * dur / len, ei.influence, x)
        };
        // A straight path (both handles on the chord, as auto-Bezier keys on a line have) is the
        // chord itself: interpolate it exactly instead of through the arc-length table.
        let p = if on_chord(p0, seg.p[1], p3) && on_chord(p0, seg.p[2], p3) {
            [p0[0] + (p3[0] - p0[0]) * f, p0[1] + (p3[1] - p0[1]) * f, p0[2] + (p3[2] - p0[2]) * f]
        } else {
            seg.at_length(f * len)
        };
        return a.value.with_components(&p);
    }
    if lin_out && lin_in {
        return a.value.lerp(&b.value, x);
    }
    let ca = a.value.components();
    let cb = b.value.components();
    if ca.is_empty() || ca.len() != cb.len() {
        // Paths, gradients: ease on dimension 0 as overall progress.
        let eo = if lin_out { Ease { speed: 1.0 / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i, 0, false, true) };
        let ei = if lin_in { Ease { speed: 1.0 / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i + 1, 0, false, false) };
        let f = ease_progress(eo.speed * dur, eo.influence, ei.speed * dur, ei.influence, x);
        return a.value.lerp(&b.value, f);
    }
    let mut out = Vec::with_capacity(ca.len());
    for d in 0..ca.len() {
        let dv = cb[d] - ca[d];
        let eo = if lin_out { Ease { speed: dv / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i, d, false, true) };
        let ei = if lin_in { Ease { speed: dv / dur, influence: 1.0 / 3.0 } } else { effective_ease(keys, i + 1, d, false, false) };
        // Bezier in (time, value) space directly: P1 = (i0, v0 + s0*i0*dur), P2 = (1-i1, v1 - s1*i1*dur).
        let (i0, i1) = (eo.influence.clamp(0.0, 1.0), ei.influence.clamp(0.0, 1.0));
        let u = solve_u(0.0, i0, 1.0 - i1, 1.0, x);
        let v = bez(ca[d], ca[d] + eo.speed * i0 * dur, cb[d] - ei.speed * i1 * dur, cb[d], u);
        out.push(v);
    }
    a.value.with_components(&out)
}

/// Numeric velocity (units/s per dimension) at `t` by central difference.
pub fn velocity(keys: &[Keyframe], t: Tick, spatial: bool) -> Vec<f64> {
    let h = Tick(TICKS_PER_SECOND / 1000);
    let (Some(a), Some(b)) = (evaluate(keys, t - h, spatial), evaluate(keys, t + h, spatial)) else { return vec![] };
    let dt = 2.0 * secs(h);
    a.components().iter().zip(b.components()).map(|(x, y)| (y - x) / dt).collect()
}

/// Speed (magnitude of velocity).
pub fn speed(keys: &[Keyframe], t: Tick, spatial: bool) -> f64 {
    velocity(keys, t, spatial).iter().map(|v| v * v).sum::<f64>().sqrt()
}

/// Insert or replace (same time) a key; keeps keys sorted. Returns its index.
pub fn set_key(keys: &mut Vec<Keyframe>, key: Keyframe) -> usize {
    match keys.binary_search_by(|k| k.time.cmp(&key.time)) {
        Ok(i) => {
            let mut k = key;
            // Keep the existing interpolation when only the value changes.
            let old = &keys[i];
            k.in_interp = old.in_interp;
            k.out_interp = old.out_interp;
            k.in_ease = old.in_ease.clone();
            k.out_ease = old.out_ease.clone();
            k.auto_bezier = old.auto_bezier;
            k.continuous = old.continuous;
            k.spatial_in = old.spatial_in;
            k.spatial_out = old.spatial_out;
            k.spatial_auto = old.spatial_auto;
            k.spatial_continuous = old.spatial_continuous;
            k.roving = old.roving;
            k.label = old.label;
            keys[i] = k;
            i
        }
        Err(i) => {
            keys.insert(i, key);
            i
        }
    }
}

/// Index of the key exactly at `t`.
pub fn key_at(keys: &[Keyframe], t: Tick) -> Option<usize> {
    keys.binary_search_by(|k| k.time.cmp(&t)).ok()
}

/// Apply Easy Ease (in, out or both) to key `i`.
pub fn easy_ease(k: &mut Keyframe, ease_in: bool, ease_out: bool) {
    let n = k.value.dims().max(1);
    if ease_in {
        k.in_interp = Interp::Bezier;
        k.in_ease = vec![Ease::EASY; n];
    }
    if ease_out {
        k.out_interp = Interp::Bezier;
        k.out_ease = vec![Ease::EASY; n];
    }
    k.auto_bezier = false;
}

#[cfg(test)]
mod tests;
