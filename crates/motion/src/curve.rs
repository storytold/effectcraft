//! Motion channels (one curve per scalar channel) ↔ EffectCraft keyframes.
//!
//! A Motion keypoint holds its time, value, interpolation and Bezier handles: input and output
//! tangents as (time offset in seconds, value offset). An EffectCraft keyframe side holds a
//! temporal ease, speed (units/s) and influence (fraction of the segment), which describe the
//! same cubic: a handle of length `dt` and rise `dv` is influence `dt / segment` and speed
//! `dv / dt`.
//!
//! Motion animates every channel on its own curve; EffectCraft animates a vector property with
//! one list of keyframes. Channels are merged on the union of their key times; a channel without
//! a key at one of those times gets the value its own curve has there.

use effectcraft_keyframe::{Ease, Interp, Keyframe, Value};
use effectcraft_project::Property;
use effectcraft_time::{FrameRate, Tick};
use roxmltree::Node;

use crate::xml::{attr_f64, child, child_f64, child_text, children};

/// Keypoints read per curve at most.
const MAX_KEYS: usize = 100_000;

/// Times closer than this (seconds) are the same key time.
const SAME_TIME: f64 = 1e-7;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MKey {
    /// Seconds (layer time).
    pub t: f64,
    pub v: f64,
    /// Motion's `interpolation=` code (`None` = Bezier).
    pub code: Option<u32>,
    /// Input handle (time offset in seconds, ≤ 0; value offset).
    pub tin: Option<(f64, f64)>,
    /// Output handle (time offset in seconds, ≥ 0; value offset).
    pub tout: Option<(f64, f64)>,
}

impl MKey {
    pub fn interp(&self) -> Interp {
        match self.code {
            Some(0) => Interp::Hold,
            Some(1) => Interp::Linear,
            _ => Interp::Bezier,
        }
    }
}

/// One scalar channel: a constant value, or a curve.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Channel {
    pub value: f64,
    pub keys: Vec<MKey>,
}

impl Channel {
    pub fn constant(value: f64) -> Channel {
        Channel { value, keys: vec![] }
    }

    pub fn animated(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The channel's value at `t` seconds.
    pub fn eval(&self, t: f64) -> f64 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else { return self.value };
        if t <= first.t {
            return first.v;
        }
        if t >= last.t {
            return last.v;
        }
        let i = self.keys.partition_point(|k| k.t <= t).saturating_sub(1);
        let (Some(a), Some(b)) = (self.keys.get(i), self.keys.get(i + 1)) else { return last.v };
        let d = b.t - a.t;
        if d <= SAME_TIME {
            return b.v;
        }
        match a.interp() {
            Interp::Hold => a.v,
            Interp::Linear => a.v + (b.v - a.v) * (t - a.t) / d,
            Interp::Bezier => {
                let (Some((ot, ov)), Some((it, iv))) = (a.tout, b.tin) else { return a.v + (b.v - a.v) * (t - a.t) / d };
                // Handles are clamped inside the segment so the time curve stays monotonic.
                let x1 = a.t + ot.clamp(0.0, d);
                let x2 = b.t + it.clamp(-d, 0.0);
                let (y1, y2) = (a.v + ov, b.v + iv);
                let bez = |p0: f64, p1: f64, p2: f64, p3: f64, u: f64| {
                    let m = 1.0 - u;
                    m * m * m * p0 + 3.0 * m * m * u * p1 + 3.0 * m * u * u * p2 + u * u * u * p3
                };
                let (mut lo, mut hi) = (0.0f64, 1.0f64);
                for _ in 0..48 {
                    let mid = 0.5 * (lo + hi);
                    if bez(a.t, x1, x2, b.t, mid) < t {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                bez(a.v, y1, y2, b.v, 0.5 * (lo + hi))
            }
        }
    }
}

/// Read a channel from a `parameter` element: its `value=` (or `default=`, or `fallback`), and
/// its `curve` keypoints when animated.
pub(crate) fn read_channel(p: Node, rate: FrameRate, fallback: f64, warn: &mut dyn FnMut(String)) -> Channel {
    let mut ch = Channel::constant(attr_f64(p, "value").or_else(|| attr_f64(p, "default")).unwrap_or(fallback));
    let Some(curve) = child(p, "curve") else { return ch };
    if let Some(v) = attr_f64(curve, "value") {
        ch.value = v;
    }
    let mut frames = false;
    for kp in children(curve, "keypoint").take(MAX_KEYS) {
        let Some(ts) = child_text(kp, "time") else { continue };
        let Some(t) = crate::parse_time(ts, rate) else { continue };
        frames |= ts.split_whitespace().nth(1).is_none();
        let Some(v) = child_f64(kp, "value") else { continue };
        let code = kp.attribute("interpolation").and_then(|c| c.trim().parse::<u32>().ok());
        if let Some(c) = code
            && !matches!(c, 0 | 1 | 6 | 7 | 8)
        {
            let what = match c {
                13 => "exponential".to_string(),
                14 => "logarithmic".to_string(),
                other => format!("type {other}"),
            };
            warn(format!("{what} keyframe interpolation is approximated with Bezier"));
        }
        let pair = |a: &str, b: &str| child_f64(kp, a).zip(child_f64(kp, b));
        // Old frame-based files give handle times in frames too.
        let scale = if frames { rate.den as f64 / rate.num.max(1) as f64 } else { 1.0 };
        let tin = pair("inputTangentTime", "inputTangentValue").map(|(a, b)| (a * scale, b));
        let tout = pair("outputTangentTime", "outputTangentValue").map(|(a, b)| (a * scale, b));
        ch.keys.push(MKey { t: t.seconds(), v, code, tin, tout });
    }
    ch.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
    ch.keys.dedup_by(|b, a| (b.t - a.t).abs() < SAME_TIME);
    if let Some(k) = ch.keys.first() {
        ch.value = k.v;
    }
    ch
}

/// A channel with its mapping to EffectCraft units: `ec = scale · motion + offset`.
#[derive(Clone, Copy)]
pub(crate) struct Mapped<'a> {
    pub ch: &'a Channel,
    pub scale: f64,
    pub offset: f64,
}

impl Mapped<'_> {
    fn at(&self, t: f64) -> f64 {
        self.scale * self.ch.eval(t) + self.offset
    }
    fn key_at(&self, t: f64) -> Option<(usize, &MKey)> {
        // Keys are sorted by time (`read_channel`); a binary search keeps `merge` O(n log n).
        let i = self.ch.keys.partition_point(|k| k.t < t - SAME_TIME);
        self.ch.keys.get(i).filter(|k| (k.t - t).abs() < SAME_TIME).map(|k| (i, k))
    }
    /// The interpolation of the channel segment containing `t` (when it has no key there).
    fn segment_interp(&self, t: f64) -> Interp {
        let i = self.ch.keys.partition_point(|k| k.t <= t);
        match i.checked_sub(1).and_then(|j| self.ch.keys.get(j)) {
            Some(k) if k.interp() == Interp::Hold => Interp::Hold,
            _ => Interp::Linear,
        }
    }
}

/// EffectCraft static value and keyframes of channels merged into one property value.
/// `make` builds the property value from per-dimension values; `spatial` collapses the eases
/// into one (Position / Anchor Point ease along the motion path).
pub(crate) fn merge(chs: &[Mapped], make: &dyn Fn(&[f64]) -> Value, spatial: bool, warn: &mut dyn FnMut(String)) -> (Value, Vec<Keyframe>) {
    let static_v: Vec<f64> = chs.iter().map(|m| m.scale * m.ch.value + m.offset).collect();
    let mut times: Vec<f64> = chs.iter().flat_map(|m| m.ch.keys.iter().map(|k| k.t)).collect();
    if times.is_empty() {
        return (make(&static_v), vec![]);
    }
    times.sort_by(f64::total_cmp);
    times.dedup_by(|b, a| (*b - *a).abs() < SAME_TIME);
    let n = times.len();
    let dims = chs.len();
    if chs.iter().filter(|m| m.ch.animated()).any(|m| m.ch.keys.len() != n) {
        warn("channels keyframed at different times were merged onto common keyframes".into());
    }
    // Values and per-dimension outgoing interpolation at each union time.
    let vals: Vec<Vec<f64>> = times.iter().map(|&t| chs.iter().map(|m| m.at(t)).collect()).collect();
    let outs: Vec<Vec<Interp>> = times
        .iter()
        .map(|&t| {
            chs.iter()
                .map(|m| match m.key_at(t) {
                    Some((_, k)) => k.interp(),
                    None if m.ch.animated() => m.segment_interp(t),
                    // A constant channel goes along with whatever the others do.
                    None => Interp::Linear,
                })
                .collect()
        })
        .collect();
    let combine = |sides: &[Interp], what: &mut dyn FnMut(String)| -> Interp {
        if sides.iter().all(|s| *s == Interp::Hold) {
            Interp::Hold
        } else if sides.contains(&Interp::Hold) && chs.iter().filter(|m| m.ch.animated()).count() > 1 {
            what("hold and interpolated channels on one keyframe: the hold channels interpolate".into());
            if sides.contains(&Interp::Bezier) { Interp::Bezier } else { Interp::Linear }
        } else if sides.contains(&Interp::Bezier) {
            Interp::Bezier
        } else {
            Interp::Linear
        }
    };
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let (Some(&t), Some(v)) = (times.get(i), vals.get(i)) else { continue };
        let mut k = Keyframe::new(Tick::from_seconds_f64(t), make(v));
        k.spatial_auto = false;
        let out_i = outs.get(i).map(|o| combine(o, warn)).unwrap_or_default();
        let in_i = i.checked_sub(1).and_then(|j| outs.get(j)).map(|o| combine(o, warn)).unwrap_or(out_i);
        k.out_interp = out_i;
        k.in_interp = if in_i == Interp::Hold { Interp::Linear } else { in_i };
        // Ease of one side of dimension `d`: from the Motion handle when the channel has a key
        // here, else the slope of the adjacent segment (which reproduces a straight line).
        let side_ease = |d: usize, out: bool| -> Ease {
            let Some(m) = chs.get(d) else { return Ease::default() };
            let j = if out { i + 1 } else { i.wrapping_sub(1) };
            let (Some(&tj), Some(vj)) = (times.get(j), vals.get(j).and_then(|v| v.get(d))) else { return Ease::default() };
            let vi = v.get(d).copied().unwrap_or(0.0);
            let seg = (tj - t).abs().max(SAME_TIME);
            let slope = if out { (vj - vi) / seg } else { (vi - vj) / seg };
            // The handle counts when its segment is Bezier (for the input side, the segment the
            // previous key starts).
            let handle = m.key_at(t).and_then(|(ci, k)| {
                if out {
                    (k.interp() == Interp::Bezier).then_some(k.tout).flatten()
                } else {
                    let prev = ci.checked_sub(1).and_then(|p| m.ch.keys.get(p))?;
                    (prev.interp() == Interp::Bezier).then_some(k.tin).flatten()
                }
            });
            match handle {
                Some((dt, dv)) if dt.abs() > 1e-9 => Ease { speed: m.scale * dv / dt, influence: (dt.abs() / seg).clamp(0.01, 1.0) },
                _ => Ease { speed: slope, influence: 1.0 / 3.0 },
            }
        };
        let eases = |out: bool| -> Vec<Ease> {
            let per: Vec<Ease> = (0..dims).map(|d| side_ease(d, out)).collect();
            if spatial {
                let speed = per.iter().map(|e| e.speed * e.speed).sum::<f64>().sqrt();
                let influence = per.iter().map(|e| e.influence).sum::<f64>() / per.len().max(1) as f64;
                vec![Ease { speed, influence }]
            } else {
                per
            }
        };
        if k.out_interp == Interp::Bezier && i + 1 < n {
            k.out_ease = eases(true);
        }
        if k.in_interp == Interp::Bezier && i > 0 {
            k.in_ease = eases(false);
        }
        keys.push(k);
    }
    let first = keys.first().map(|k| k.value.clone()).unwrap_or_else(|| make(&static_v));
    (first, keys)
}

/// One dimension of an EffectCraft property as a Motion channel: `motion = (ec − offset) / scale`.
pub(crate) fn channel_of(p: &Property, d: usize, scale: f64, offset: f64) -> Channel {
    let s = if scale.abs() < 1e-12 { 1.0 } else { scale };
    let comp = |v: &Value| v.components().get(d).copied().unwrap_or(0.0);
    let to_m = |x: f64| (x - offset) / s;
    let mut ch = Channel::constant(to_m(comp(&p.value)));
    let keys = &p.keys;
    let spatial = p.spatial;
    for (i, k) in keys.iter().enumerate() {
        let t = k.time.seconds();
        let v = comp(&k.value);
        let code = match k.out_interp {
            Interp::Hold => Some(0),
            Interp::Linear => Some(1),
            Interp::Bezier => None,
        };
        // Per-dimension share of a spatial (path) speed.
        let share = |a: &Keyframe, b: &Keyframe| -> f64 {
            if !spatial {
                return 1.0;
            }
            let (ca, cb) = (a.value.components(), b.value.components());
            let len = ca.iter().zip(&cb).map(|(x, y)| (y - x) * (y - x)).sum::<f64>().sqrt();
            let dd = cb.get(d).copied().unwrap_or(0.0) - ca.get(d).copied().unwrap_or(0.0);
            if len > 1e-12 { dd / len } else { 0.0 }
        };
        let ease = |list: &[Ease]| -> Ease {
            let i = if spatial { 0 } else { d };
            list.get(i).or_else(|| list.first()).copied().unwrap_or_default()
        };
        let tout = keys.get(i + 1).map(|b| {
            let seg = (b.time.seconds() - t).max(1e-9);
            let dv = comp(&b.value) - v;
            match k.out_interp {
                Interp::Bezier => {
                    let e = ease(&k.out_ease);
                    let dt = e.influence.clamp(0.0, 1.0) * seg;
                    (dt, e.speed * share(k, b) * dt / s)
                }
                Interp::Linear => (seg / 3.0, dv / 3.0 / s),
                Interp::Hold => (seg / 3.0, 0.0),
            }
        });
        let tin = i.checked_sub(1).and_then(|j| keys.get(j)).map(|a| {
            let seg = (t - a.time.seconds()).max(1e-9);
            let dv = v - comp(&a.value);
            match (a.out_interp, k.in_interp) {
                (Interp::Hold, _) => (-seg / 3.0, 0.0),
                (_, Interp::Bezier) => {
                    let e = ease(&k.in_ease);
                    let dt = -e.influence.clamp(0.0, 1.0) * seg;
                    (dt, e.speed * share(a, k) * dt / s)
                }
                _ => (-seg / 3.0, -dv / 3.0 / s),
            }
        });
        ch.keys.push(MKey { t, v: to_m(v), code, tin, tout });
    }
    if let Some(k) = ch.keys.first() {
        ch.value = k.v;
    }
    ch
}
