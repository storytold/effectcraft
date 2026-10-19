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

/// Influence of a linear key side in a segment whose other side is eased: the side keeps the
/// segment's average speed with a 16.67 % influence, as After Effects shows it in the Keyframe
/// Velocity dialog and evaluates it (time-remap read-backs: a bezier-out (speed 0, 50 %) into a
/// linear-in from 3.5 to 0.5 over 2 s gives 3.290 at a quarter and 2.657 at half of the segment).
pub const LINEAR_SIDE_INFLUENCE: f64 = 1.0 / 6.0;

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
    ([-tout[0], -tout[1], -tout[2]], tout)
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
/// segment, [`LINEAR_SIDE_INFLUENCE`]). `None` for hold sides and sides without a neighbour.
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
        return Some(Ease { speed: dv / dur, influence: LINEAR_SIDE_INFLUENCE });
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

/// Evaluate keyed 3D Orientation at `t` as After Effects does: the temporal ease gives the
/// progress through each segment, and the rotation moves along the shortest arc between the two
/// keys ([`effectcraft_geom::orientation_slerp`]) instead of blending each angle. Speeds are
/// degrees per second along that arc. Values other than 3D vectors evaluate as [`evaluate`].
pub fn evaluate_orientation(keys: &[Keyframe], t: Tick) -> Option<Value> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some(first.value.clone());
    }
    let last = keys.last()?;
    if t >= last.time {
        return Some(last.value.clone());
    }
    let i = keys.partition_point(|k| k.time <= t).checked_sub(1)?;
    let (a, b) = (keys.get(i)?, keys.get(i + 1)?);
    let (Value::Vec3(va), Value::Vec3(vb)) = (&a.value, &b.value) else { return Some(segment_value(keys, i, t, false)) };
    if a.out_interp == Interp::Hold {
        return Some(a.value.clone());
    }
    let (t0, t1) = (secs(a.time), secs(b.time));
    let dur = (t1 - t0).max(1e-12);
    let x = ((secs(t) - t0) / dur).clamp(0.0, 1.0);
    // On a key: its own value, as stored.
    if x <= 0.0 {
        return Some(a.value.clone());
    }
    let arc = effectcraft_geom::orientation_arc_deg((*va).into(), (*vb).into());
    let lin_out = a.out_interp == Interp::Linear;
    let lin_in = b.in_interp != Interp::Bezier;
    let f = if lin_out && lin_in || arc <= 1e-9 {
        x
    } else {
        // Auto-Bezier keys take the arc speed between their neighbours.
        let side = |j: usize, out: bool| -> Ease {
            // A linear side keeps the segment's average speed at the same influence as in
            // `segment_value` (and a missing key, unreachable here, is treated as one).
            let Some(k) = keys.get(j) else { return Ease { speed: arc / dur, influence: LINEAR_SIDE_INFLUENCE } };
            let linear = if out { lin_out } else { lin_in };
            if linear {
                return Ease { speed: arc / dur, influence: LINEAR_SIDE_INFLUENCE };
            }
            if k.auto_bezier && j > 0 && j + 1 < keys.len() {
                let span = (secs(keys[j + 1].time) - secs(keys[j - 1].time)).max(1e-12);
                let (Value::Vec3(p), Value::Vec3(n)) = (&keys[j - 1].value, &keys[j + 1].value) else { return Ease { speed: arc / dur, influence: 1.0 / 6.0 } };
                return Ease { speed: effectcraft_geom::orientation_arc_deg((*p).into(), (*n).into()) / span, influence: 1.0 / 6.0 };
            }
            let list = if out { &k.out_ease } else { &k.in_ease };
            list.first().copied().unwrap_or_default()
        };
        let (eo, ei) = (side(i, true), side(i + 1, false));
        ease_progress(eo.speed * dur / arc, eo.influence, ei.speed * dur / arc, ei.influence, x)
    };
    let f = if f.is_finite() { f } else { x };
    let o = effectcraft_geom::orientation_slerp((*va).into(), (*vb).into(), f);
    Some(Value::Vec3([o.x, o.y, o.z]))
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
            let eo = if lin_out { Ease { speed: len / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i, 0, true, true) };
            let ei = if lin_in { Ease { speed: len / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i + 1, 0, true, false) };
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
        let eo = if lin_out { Ease { speed: 1.0 / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i, 0, false, true) };
        let ei = if lin_in { Ease { speed: 1.0 / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i + 1, 0, false, false) };
        let f = ease_progress(eo.speed * dur, eo.influence, ei.speed * dur, ei.influence, x);
        return a.value.lerp(&b.value, f);
    }
    let mut out = Vec::with_capacity(ca.len());
    for d in 0..ca.len() {
        let dv = cb[d] - ca[d];
        let eo = if lin_out { Ease { speed: dv / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i, d, false, true) };
        let ei = if lin_in { Ease { speed: dv / dur, influence: LINEAR_SIDE_INFLUENCE } } else { effective_ease(keys, i + 1, d, false, false) };
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
mod tests {
    use super::*;

    fn s(x: f64) -> Tick {
        Tick::from_seconds_f64(x)
    }

    #[test]
    fn missing_spatial_keys_have_no_tangents_or_length() {
        let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0]))];
        for i in [1, usize::MAX] {
            assert_eq!(spatial_tangents(&keys, i), ([0.0; 3], [0.0; 3]));
            assert_eq!(spatial_segment_length(&keys, i), 0.0);
        }
        assert_eq!(spatial_tangents(&[], 0), ([0.0; 3], [0.0; 3]));
        assert_eq!(spatial_segment_length(&[], usize::MAX), 0.0);
    }

    #[test]
    fn time_reverse_handles_deserialized_tick_extremes() {
        // Tick's public serde representation can contain values beyond from_seconds_f64's cap.
        for (first, last) in [(i64::MIN, i64::MIN + 10), (i64::MAX - 10, i64::MAX), (i64::MIN, i64::MAX)] {
            let mut keys = vec![Keyframe::new(Tick(first), Value::Scalar(1.0)), Keyframe::new(Tick(last), Value::Scalar(2.0))];
            time_reverse(&mut keys);
            assert_eq!(keys[0].time, Tick(first));
            assert_eq!(keys[0].value, Value::Scalar(2.0));
            assert_eq!(keys[1].time, Tick(last));
            assert_eq!(keys[1].value, Value::Scalar(1.0));
            time_reverse(&mut keys);
            assert_eq!(keys[0].value, Value::Scalar(1.0));
        }
    }

    #[test]
    fn roving_handles_deserialized_tick_extremes() {
        let mut keys = vec![
            Keyframe::new(Tick(i64::MIN), Value::Vec2([0.0, 0.0])),
            Keyframe::new(Tick(10), Value::Vec2([50.0, 0.0])),
            Keyframe::new(Tick(i64::MAX), Value::Vec2([100.0, 0.0])),
        ];
        for key in &mut keys {
            key.spatial_auto = false;
        }
        keys[1].roving = true;
        assert!(retime_roving(&mut keys, true));
        assert_eq!(keys[1].time, Tick::ZERO);
    }

    #[test]
    fn replacing_key_preserves_spatial_metadata() {
        let mut key = Keyframe::new(s(1.0), Value::Vec2([20.0, 30.0]));
        key.roving = true;
        key.spatial_continuous = true;
        let mut keys = vec![key];
        set_key(&mut keys, Keyframe::new(s(1.0), Value::Vec2([40.0, 50.0])));
        assert!(keys[0].roving);
        assert!(keys[0].spatial_continuous);
        assert_eq!(keys[0].value, Value::Vec2([40.0, 50.0]));
    }

    #[test]
    fn temporal_ease_matches_live_ae_with_overlapping_influences() {
        // Original Rotation probe via AEsync 2.0.4, AE 26.3x87, 2026-10-05.
        // AE retains each handle's requested influence even when their sum exceeds 100%.
        for (from, to, out_speed, out_inf, in_speed, in_inf, samples) in [
            (0.0, 100.0, 0.0, 0.8, 0.0, 0.8, [(0.1, 0.59243939036327), (0.25, 4.76438030608764), (0.4, 18.4284394271562), (0.75, 95.2356196939123)]),
            (0.0, 100.0, 20.0, 0.8, 80.0, 0.6, [(0.1, 2.25754942621577), (0.25, 6.99841889412931), (0.4, 14.9334636959052), (0.75, 73.8196785378389)]),
            (100.0, 0.0, -20.0, 0.6, -80.0, 0.8, [(0.1, 97.6395466334594), (0.25, 91.597048571292), (0.4, 74.5367957474338), (0.75, 22.3789127753126)]),
        ] {
            let mut a = Keyframe::new(s(0.0), Value::Scalar(from));
            let mut b = Keyframe::new(s(1.0), Value::Scalar(to));
            a.out_interp = Interp::Bezier;
            b.in_interp = Interp::Bezier;
            a.out_ease = vec![Ease { speed: out_speed, influence: out_inf }];
            b.in_ease = vec![Ease { speed: in_speed, influence: in_inf }];
            for (time, expected) in samples {
                let value = evaluate(&[a.clone(), b.clone()], s(time), false).unwrap().as_f64();
                assert!((value - expected).abs() < 1e-7, "{time}: {value} != {expected}");
                let normalized = ease_progress(out_speed / (to - from), out_inf, in_speed / (to - from), in_inf, time);
                assert!((from + normalized * (to - from) - expected).abs() < 1e-7);
            }
        }
    }

    #[test]
    fn maximum_influence_resolves_flat_midpoint_time_curve() {
        for time in [0.5 - 1e-9, 0.5 - 1e-12, 0.5, 0.5 + 1e-12, 0.5 + 1e-9] {
            // At 100%/100%, x = 0.5 + 4*(u - 0.5)^3 exactly.
            let u = 0.5 + ((time - 0.5) / 4.0_f64).cbrt();
            let expected = bez(0.0, 0.0, 1.0, 1.0, u);
            let actual = ease_progress(0.0, 1.0, 0.0, 1.0, time);
            assert!((actual - expected).abs() < 1e-10, "{time}: {actual} != {expected}");
        }
    }

    /// Keyed Orientation moves along the shortest arc, eased in time, as After Effects 2026 reads
    /// it back (cases 802 and 803: keys [0,0,0] at 0 s and [60,80,40] at 2 s).
    #[test]
    fn keyed_orientation_matches_after_effects() {
        let close = |v: Value, e: [f64; 3]| {
            let v = v.as_vec3();
            assert!((0..3).all(|i| (v[i] - e[i]).abs() < 2e-3), "{v:?} vs {e:?}");
        };
        let lin = vec![Keyframe::new(s(0.0), Value::Vec3([0.0; 3])), Keyframe::new(s(2.0), Value::Vec3([60.0, 80.0, 40.0]))];
        close(evaluate_orientation(&lin, s(0.5)).unwrap(), [16.9201, 16.0688, 16.0629]);
        close(evaluate_orientation(&lin, s(1.0)).unwrap(), [30.6186, 35.7617, 28.4317]);
        close(evaluate_orientation(&lin, s(2.0)).unwrap(), [60.0, 80.0, 40.0]);
        // A middle key reads back as stored (not as another angle triple of the same rotation).
        let three = vec![
            Keyframe::new(s(0.0), Value::Vec3([0.0; 3])),
            Keyframe::new(s(1.0), Value::Vec3([0.0, 120.0, 0.0])),
            Keyframe::new(s(2.0), Value::Vec3([0.0; 3])),
        ];
        assert_eq!(evaluate_orientation(&three, s(1.0)), Some(Value::Vec3([0.0, 120.0, 0.0])));
        // Easy ease (AE's preset: speed 0, influence 33 %) on both keys.
        let ease = |k: Keyframe| {
            let mut k = k.eased();
            k.in_ease = vec![Ease { speed: 0.0, influence: 0.33 }];
            k.out_ease = vec![Ease { speed: 0.0, influence: 0.33 }];
            k
        };
        let eased: Vec<Keyframe> = lin.into_iter().map(ease).collect();
        close(evaluate_orientation(&eased, s(0.5)).unwrap(), [11.0974, 9.5750, 10.5916]);
        close(evaluate_orientation(&eased, s(1.0)).unwrap(), [30.6186, 35.7617, 28.4317]);
        close(evaluate_orientation(&eased, s(1.5)).unwrap(), [47.6915, 65.7590, 40.3338]);
        // Hold keys and other value types.
        let held = vec![Keyframe::new(s(0.0), Value::Vec3([1.0, 2.0, 3.0])).hold(), Keyframe::new(s(1.0), Value::Vec3([0.0; 3]))];
        close(evaluate_orientation(&held, s(0.5)).unwrap(), [1.0, 2.0, 3.0]);
        let scalar = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(1.0), Value::Scalar(10.0))];
        assert_eq!(evaluate_orientation(&scalar, s(0.5)), Some(Value::Scalar(5.0)));
        assert_eq!(evaluate_orientation(&[], s(0.5)), None);
    }

    /// M13.15: agents set gradients as plain JSON, not only the tagged form `get` returns.
    #[test]
    fn gradients_coerce_from_plain_json() {
        let g = Value::Gradient(Gradient::default());
        let plain = serde_json::json!({"colors": [[0, [0, 0.5, 1, 1]], [1, "#ffff00"]], "opacities": [[0, 1], [1, 0.5]]});
        let Some(Value::Gradient(a)) = g.coerce_json(&plain) else { panic!("object form") };
        assert_eq!(a.colors, vec![(0.0, [0.0, 0.5, 1.0, 1.0]), (1.0, [1.0, 1.0, 0.0, 1.0])]);
        assert_eq!(a.opacities, vec![(0.0, 1.0), (1.0, 0.5)]);
        let Some(Value::Gradient(b)) = g.coerce_json(&serde_json::json!(["#ff0000", "#00ff00", "#0000ff"])) else { panic!("list form") };
        assert_eq!(b.colors.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0.0, 0.5, 1.0]);
        assert_eq!(b.opacities.len(), 3);
        // The tagged form (what prop.get returns) still round-trips.
        assert_eq!(g.coerce_json(&g.to_json()), Some(g.clone()));
        assert_eq!(g.coerce_json(&serde_json::json!(5)), None);
    }

    /// #612: the gradient editor samples a new stop from its own track only. `sample` multiplies
    /// in the opacity stops, so feeding its alpha into a colour stop would apply 50% twice (25%).
    #[test]
    fn gradient_tracks_sample_separately() {
        let g = Gradient { colors: vec![(0.0, [1.0, 0.0, 0.0, 0.8]), (1.0, [0.0, 0.0, 1.0, 0.4])], opacities: vec![(0.0, 0.5), (1.0, 0.5)] };
        let c = g.color_at(0.5);
        assert!((c[0] - 0.5).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6, "{c:?}");
        assert!((c[3] - 0.6).abs() < 1e-6, "the colour track's own alpha, without opacity: {c:?}");
        assert!((g.opacity_at(0.5) - 0.5).abs() < 1e-6);
        assert!((g.sample(0.5)[3] - 0.3).abs() < 1e-6, "sample still applies opacity once");
        // A stop added at 50% from the opacity track keeps 50%: sampled again, it is still 50%.
        let mut with_stop = g.clone();
        with_stop.opacities.insert(1, (0.5, g.opacity_at(0.5)));
        assert!((with_stop.opacity_at(0.5) - 0.5).abs() < 1e-6);
        assert!((with_stop.sample(0.5)[3] - 0.3).abs() < 1e-6, "adding a stop doesn't change the look");
        // Outside the stops, and with none: the ends hold, and an empty track is opaque white.
        assert_eq!(g.color_at(-1.0), [1.0, 0.0, 0.0, 0.8]);
        assert_eq!(g.color_at(2.0), [0.0, 0.0, 1.0, 0.4]);
        let empty = Gradient { colors: vec![], opacities: vec![] };
        assert_eq!((empty.color_at(0.3), empty.opacity_at(0.3)), ([1.0; 4], 1.0));
    }

    /// #612: the editor saves the typed form, so one colour stop (which the plain JSON parser
    /// rejects) and each stop's RGBA come back exactly.
    #[test]
    fn typed_gradient_round_trips_one_stop_and_rgba() {
        let one = Value::Gradient(Gradient { colors: vec![(0.25, [0.1, 0.2, 0.3, 0.7])], opacities: vec![(0.0, 1.0), (0.6, 0.35)] });
        let cur = Value::Gradient(Gradient::default());
        assert_eq!(cur.coerce_json(&one.to_json()), Some(one.clone()));
        assert_eq!(cur.coerce_json(&serde_json::json!({"colors": [[0.25, [0.1, 0.2, 0.3, 0.7]]]})), None, "plain JSON needs two colours");
    }

    #[test]
    fn linear_and_hold() {
        let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0))];
        assert_eq!(evaluate(&keys, s(1.0), false), Some(Value::Scalar(50.0)));
        assert_eq!(evaluate(&keys, s(-1.0), false), Some(Value::Scalar(0.0)));
        assert_eq!(evaluate(&keys, s(3.0), false), Some(Value::Scalar(100.0)));
        let held = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)).hold(), Keyframe::new(s(2.0), Value::Scalar(100.0))];
        assert_eq!(evaluate(&held, s(1.999), false), Some(Value::Scalar(0.0)));
    }

    #[test]
    fn hold_key_keeps_its_incoming_segment_linear() {
        // Hold on the second key (in and out, as KeyframeInterpolationType.HOLD sets it) holds
        // what comes after it, not the linear motion into it.
        let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0)).hold()];
        assert_eq!(evaluate(&keys, s(0.5), false), Some(Value::Scalar(25.0)));
        let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])), Keyframe::new(s(2.0), Value::Vec2([100.0, 0.0])).hold()];
        let p = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
        assert!((p[0] - 25.0).abs() < 0.01, "{p:?}");
    }

    #[test]
    fn easy_ease_is_symmetric_and_slow_at_ends() {
        let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)).eased(), Keyframe::new(s(1.0), Value::Scalar(100.0)).eased()];
        let mid = evaluate(&keys, s(0.5), false).unwrap().as_f64();
        assert!((mid - 50.0).abs() < 1e-6, "{mid}");
        let early = evaluate(&keys, s(0.1), false).unwrap().as_f64();
        assert!(early < 10.0 && early > 0.0, "{early}");
        let v0 = velocity(&keys, s(0.01), false)[0];
        let vm = velocity(&keys, s(0.5), false)[0];
        assert!(v0 < vm);
    }

    #[test]
    fn bezier_with_linear_speed_matches_linear() {
        let mut a = Keyframe::new(s(0.0), Value::Scalar(0.0));
        let mut b = Keyframe::new(s(2.0), Value::Scalar(100.0));
        a.out_interp = Interp::Bezier;
        b.in_interp = Interp::Bezier;
        a.out_ease = vec![Ease { speed: 50.0, influence: 1.0 / 3.0 }];
        b.in_ease = vec![Ease { speed: 50.0, influence: 1.0 / 3.0 }];
        let keys = vec![a, b];
        for i in 0..=20 {
            let t = i as f64 * 0.1;
            let v = evaluate(&keys, s(t), false).unwrap().as_f64();
            assert!((v - 50.0 * t).abs() < 1e-6, "{t} {v}");
        }
    }

    /// A linear side next to an eased one keeps the segment's average speed at 16.67 %
    /// influence. After Effects time-remap read-backs (a square at x = 50 + 455 * remap): out of
    /// a key at 2 s (3.5, speed 0, influence 50 %) into a linear key at 4 s (0.5) the remap is
    /// 3.290 at 2.5 s and 2.657 at 3 s (33.3 % gave 3.308 and 2.629).
    #[test]
    fn a_linear_side_next_to_an_eased_side_matches_after_effects() {
        let mut a = Keyframe::new(s(2.0), Value::Scalar(3.5));
        a.out_interp = Interp::Bezier;
        a.out_ease = vec![Ease { speed: 0.0, influence: 0.5 }];
        let keys = vec![a, Keyframe::new(s(4.0), Value::Scalar(0.5))];
        for (t, ae) in [(2.5, 3.290), (3.0, 2.657)] {
            let v = evaluate(&keys, s(t), false).unwrap().as_f64();
            // AE's square edge is read to the pixel: 1 px = 1 / 455 of remap.
            assert!((v - ae).abs() < 2.0 / 455.0, "{t}: {v} vs {ae}");
        }
        // The mirror case: a linear out into an eased in.
        let mut b = Keyframe::new(s(4.0), Value::Scalar(0.5));
        b.in_interp = Interp::Bezier;
        b.in_ease = vec![Ease { speed: 0.0, influence: 0.5 }];
        let mirror = vec![Keyframe::new(s(2.0), Value::Scalar(3.5)), b];
        let v = evaluate(&mirror, s(3.5), false).unwrap().as_f64();
        assert!((v - (4.0 - 3.290)).abs() < 2.0 / 455.0, "{v}");
        assert_eq!(side_ease(&keys, 1, 0, false, false).map(|e| e.influence), Some(LINEAR_SIDE_INFLUENCE));
    }

    /// Keyed Orientation shapes a linear side next to an eased one as a scalar property does
    /// (`LINEAR_SIDE_INFLUENCE`): about one axis the shortest arc is the angle itself, so the
    /// orientation follows the same curve as a scalar with the same keys (CodeRabbit on #1876).
    #[test]
    fn orientation_linear_side_next_to_an_eased_side_matches_a_scalar() {
        let ease = vec![Ease { speed: 0.0, influence: 0.5 }];
        let (mut a, mut sa) = (Keyframe::new(s(2.0), Value::Vec3([0.0, 0.0, 0.0])), Keyframe::new(s(2.0), Value::Scalar(0.0)));
        for k in [&mut a, &mut sa] {
            k.out_interp = Interp::Bezier;
            k.out_ease = ease.clone();
        }
        let orient = vec![a, Keyframe::new(s(4.0), Value::Vec3([0.0, 0.0, 90.0]))];
        let scalar = vec![sa, Keyframe::new(s(4.0), Value::Scalar(90.0))];
        // The mirror: a linear out into an eased in.
        let (mut b, mut sb) = (Keyframe::new(s(4.0), Value::Vec3([0.0, 0.0, 90.0])), Keyframe::new(s(4.0), Value::Scalar(90.0)));
        for k in [&mut b, &mut sb] {
            k.in_interp = Interp::Bezier;
            k.in_ease = ease.clone();
        }
        let orient_m = vec![Keyframe::new(s(2.0), Value::Vec3([0.0, 0.0, 0.0])), b];
        let scalar_m = vec![Keyframe::new(s(2.0), Value::Scalar(0.0)), sb];
        for (o, sc) in [(&orient, &scalar), (&orient_m, &scalar_m)] {
            for t in [2.25, 2.5, 3.0, 3.5, 3.75] {
                let Some(Value::Vec3(v)) = evaluate_orientation(o, s(t)) else { panic!("orientation at {t}") };
                let want = evaluate(sc, s(t), false).unwrap().as_f64();
                assert!((v[2] - want).abs() < 1e-6 && v[0].abs() < 1e-9 && v[1].abs() < 1e-9, "{t}: {v:?} vs {want}");
            }
        }
    }

    #[test]
    fn spatial_linear_constant_speed() {
        let keys = vec![Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])), Keyframe::new(s(1.0), Value::Vec2([300.0, 400.0]))];
        let p = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
        assert!((p[0] - 150.0).abs() < 0.5 && (p[1] - 200.0).abs() < 0.5, "{p:?}");
    }

    /// #456: along a curved spatial path with linear (constant-speed) keys the speed graph is
    /// flat, not a sawtooth from a piecewise-linear arc-length table.
    #[test]
    fn spatial_curved_path_has_smooth_constant_speed() {
        let mut a = Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0]));
        let mut b = Keyframe::new(s(2.0), Value::Vec2([400.0, 0.0]));
        a.spatial_auto = false;
        b.spatial_auto = false;
        a.spatial_out = [50.0, -300.0, 0.0];
        b.spatial_in = [80.0, 250.0, 0.0];
        let keys = vec![a, b];
        let len = spatial_segment_length(&keys, 0);
        let speeds: Vec<f64> = (1..400).map(|i| speed(&keys, s(i as f64 * 0.005), true)).collect();
        for v in &speeds {
            assert!((v - len / 2.0).abs() < 1e-3 * len, "speed {v} vs {}", len / 2.0);
        }
    }

    #[test]
    fn spatial_auto_bezier_passes_through_keys() {
        let keys = vec![
            Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])),
            Keyframe::new(s(1.0), Value::Vec2([100.0, 100.0])),
            Keyframe::new(s(2.0), Value::Vec2([200.0, 0.0])),
        ];
        let p = evaluate(&keys, s(1.0), true).unwrap().as_vec2();
        assert_eq!(p, [100.0, 100.0]);
        // Curved: the midpoint of the first segment lies off the straight line.
        let m = evaluate(&keys, s(0.5), true).unwrap().as_vec2();
        assert!((m[0] - m[1]).abs() > 1.0, "{m:?}");
    }

    #[test]
    fn straight_auto_bezier_path_is_exact() {
        // Auto-Bezier ends now have handles along the line; the chord is still hit exactly.
        let keys = vec![Keyframe::new(s(0.0), Value::Vec2([40.0, 50.0])), Keyframe::new(s(2.0), Value::Vec2([120.0, 50.0]))];
        assert_eq!(segment_value(&keys, 0, s(1.0), true), Value::Vec2([80.0, 50.0]));
    }

    /// #880: a motion path whose arc length overflows (huge coordinates, as the Position field
    /// accepts) or is NaN evaluates without panicking: scrubbing over it crashed the app.
    #[test]
    fn overflowing_or_nan_motion_paths_evaluate() {
        let paths = [
            [[-1.7e308, 0.0], [1.7e308, 0.0], [-1.7e308, 5.0]],
            [[0.0, 0.0], [f64::NAN, 10.0], [20.0, 0.0]],
            [[f64::INFINITY, 0.0], [0.0, f64::NEG_INFINITY], [20.0, 0.0]],
        ];
        for path in paths {
            let mut keys: Vec<Keyframe> = path.iter().enumerate().map(|(i, &v)| Keyframe::new(s(i as f64 * 2.0), Value::Vec2(v))).collect();
            for eased in [false, true] {
                if eased {
                    keys = keys.into_iter().map(Keyframe::eased).collect();
                }
                for f in 0..=40 {
                    let t = s(f as f64 * 0.1);
                    assert!(evaluate(&keys, t, true).is_some());
                    let _ = speed(&keys, t, true);
                }
            }
        }
    }

    #[test]
    fn spatial_auto_bezier_tangents_follow_after_effects() {
        // Measured on After Effects 26.3 with original paths (docs/fidelity/README.md): each
        // auto-Bezier key's out tangent is (next - previous) / 6 and its in tangent the mirror,
        // whatever the key times. End keys use their one neighbour: (neighbour - key) / 6.
        let p = [[40.0, 300.0, 0.0], [260.0, 80.0, 20.0], [700.0, 420.0, -40.0], [1100.0, 150.0, 0.0]];
        let mut keys: Vec<Keyframe> = [0.0, 0.2, 1.5, 1.8].iter().zip(p).map(|(&t, v)| Keyframe::new(s(t), Value::Vec3(v))).collect();
        let sixth = |a: [f64; 3], b: [f64; 3]| [(b[0] - a[0]) / 6.0, (b[1] - a[1]) / 6.0, (b[2] - a[2]) / 6.0];
        let neg = |v: [f64; 3]| [-v[0], -v[1], -v[2]];
        let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
        let want = [sixth(p[0], p[1]), sixth(p[0], p[2]), sixth(p[1], p[3]), sixth(p[2], p[3])];
        for (i, out) in want.into_iter().enumerate() {
            let (tin, tout) = spatial_tangents(&keys, i);
            assert!(close(tout, out) && close(tin, neg(out)), "key {i}: {tin:?} {tout:?}");
        }
        // Moving a key in time leaves every tangent as it was.
        keys[1].time = s(1.2);
        assert!(close(spatial_tangents(&keys, 1).1, sixth(p[0], p[2])));
        // A key between two coincident neighbours, and a lone key, have none.
        let still = [[5.0, 5.0], [9.0, 1.0], [5.0, 5.0]].iter().enumerate().map(|(i, &v)| Keyframe::new(s(i as f64), Value::Vec2(v))).collect::<Vec<_>>();
        assert_eq!(spatial_tangents(&still, 1), ([0.0; 3], [0.0; 3]));
        assert_eq!(spatial_tangents(&still[..1], 0), ([0.0; 3], [0.0; 3]));
    }

    #[test]
    fn set_key_replaces_same_time() {
        let mut keys = vec![];
        set_key(&mut keys, Keyframe::new(s(1.0), Value::Scalar(1.0)).eased());
        set_key(&mut keys, Keyframe::new(s(0.0), Value::Scalar(0.0)));
        set_key(&mut keys, Keyframe::new(s(1.0), Value::Scalar(5.0)));
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[1].value, Value::Scalar(5.0));
        assert_eq!(keys[1].in_interp, Interp::Bezier);
    }

    #[test]
    fn colors_and_paths_interpolate() {
        let keys = vec![Keyframe::new(s(0.0), Value::Color([0.0, 0.0, 0.0, 1.0])), Keyframe::new(s(1.0), Value::Color([1.0, 0.5, 0.0, 1.0]))];
        assert_eq!(evaluate(&keys, s(0.5), false).unwrap().as_color(), [0.5, 0.25, 0.0, 1.0]);
        let a = ShapePath::rect([0.0, 0.0], 10.0, 10.0);
        let b = ShapePath::rect([0.0, 0.0], 20.0, 20.0);
        let keys = vec![Keyframe::new(s(0.0), Value::Path(a)), Keyframe::new(s(1.0), Value::Path(b))];
        let v = evaluate(&keys, s(0.5), false).unwrap();
        assert_eq!(v.as_path().unwrap().vertices[0], [-7.5, -7.5]);
    }

    #[test]
    fn roving_keys_get_constant_speed_times() {
        let mut keys = vec![
            Keyframe::new(s(0.0), Value::Vec2([0.0, 0.0])),
            Keyframe::new(s(0.2), Value::Vec2([300.0, 0.0])),
            Keyframe::new(s(2.0), Value::Vec2([400.0, 0.0])),
        ];
        for k in &mut keys {
            k.spatial_auto = false;
        }
        keys[1].roving = true;
        assert!(retime_roving(&mut keys, true));
        // 300 of 400 px → 75 % of the 2 s span.
        assert!((keys[1].time.seconds() - 1.5).abs() < 1e-6, "{}", keys[1].time.seconds());
        let v0 = speed(&keys, s(0.5), true);
        let v1 = speed(&keys, s(1.8), true);
        assert!((v0 - 200.0).abs() < 1.0 && (v1 - 200.0).abs() < 1.0, "{v0} {v1}");
        // First/last keys can't rove.
        keys[0].roving = true;
        retime_roving(&mut keys, true);
        assert!(!keys[0].roving);
    }

    #[test]
    fn roving_keys_never_share_a_time_with_a_fixed_key() {
        // Zero-length last segment (issue #532) and zero-length first segment.
        for xs in [[0.0, 40.0, 120.0, 120.0], [0.0, 0.0, 40.0, 120.0]] {
            let mut keys: Vec<Keyframe> = xs.iter().enumerate().map(|(i, x)| Keyframe::new(s(i as f64), Value::Vec2([*x, 64.0]))).collect();
            for k in &mut keys {
                k.spatial_auto = false;
            }
            keys[1].roving = true;
            keys[2].roving = true;
            retime_roving(&mut keys, true);
            assert!(keys.windows(2).all(|w| w[0].time < w[1].time), "{xs:?}: {:?}", keys.iter().map(|k| k.time).collect::<Vec<_>>());
        }
    }

    #[test]
    fn side_ease_reports_linear_and_eased_sides() {
        let keys = vec![Keyframe::new(s(0.0), Value::Scalar(0.0)), Keyframe::new(s(2.0), Value::Scalar(100.0)).eased()];
        let out = side_ease(&keys, 0, 0, false, true).unwrap();
        assert!((out.speed - 50.0).abs() < 1e-9);
        let inn = side_ease(&keys, 1, 0, false, false).unwrap();
        assert_eq!(inn, Ease::EASY);
        assert!(side_ease(&keys, 0, 0, false, false).is_none());
    }

    #[test]
    fn time_reverse_mirrors_keys() {
        let mut keys =
            vec![Keyframe::new(s(1.0), Value::Scalar(0.0)).eased(), Keyframe::new(s(2.0), Value::Scalar(5.0)), Keyframe::new(s(4.0), Value::Scalar(10.0))];
        keys[0].out_interp = Interp::Hold;
        time_reverse(&mut keys);
        assert_eq!(keys.iter().map(|k| k.time.seconds().round() as i64).collect::<Vec<_>>(), vec![1, 3, 4]);
        assert_eq!(keys[0].value, Value::Scalar(10.0));
        assert_eq!(keys[2].in_interp, Interp::Hold);
    }

    proptest::proptest! {
        #[test]
        fn eased_values_stay_in_range(t in 0.0f64..1.0, i0 in 0.01f64..1.0, i1 in 0.01f64..1.0) {
            let mut a = Keyframe::new(s(0.0), Value::Scalar(0.0)).eased();
            let mut b = Keyframe::new(s(1.0), Value::Scalar(10.0)).eased();
            a.out_ease = vec![Ease { speed: 0.0, influence: i0 }];
            b.in_ease = vec![Ease { speed: 0.0, influence: i1 }];
            let v = evaluate(&[a, b], s(t), false).unwrap().as_f64();
            proptest::prop_assert!((-1e-9..=10.0 + 1e-9).contains(&v));
        }
    }
}
