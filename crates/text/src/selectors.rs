//! Text selector maths: how much each character / word / line is affected by an animator.
//!
//! Pure functions shared by the renderer and tests. Selection values are fractions (1 = fully
//! selected; negative values invert the animated property's effect, as with a negative Amount).
//! Behaviour follows After Effects' public documentation of the Range, Wiggly and Expression
//! selectors; the formulas themselves are our own.
//!
//! * **Range**: Start / End / Offset over `n` units, Shape (Square with Smoothness, Ramp Up /
//!   Down, Triangle, Round, Smooth), Ease High / Low, Amount, Randomize Order.
//! * **Wiggly**: smooth seeded noise in time (Wiggles/Second, Temporal Phase) and across units
//!   (Correlation, Spatial Phase), mapped to Min..Max Amount, per dimension unless Lock Dimensions.
//! * **Modes** combine each selector with the selectors above it.

/// Range selector Shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Square,
    RampUp,
    RampDown,
    Triangle,
    Round,
    Smooth,
}

impl Shape {
    pub fn from_index(i: u32) -> Shape {
        match i {
            1 => Shape::RampUp,
            2 => Shape::RampDown,
            3 => Shape::Triangle,
            4 => Shape::Round,
            5 => Shape::Smooth,
            _ => Shape::Square,
        }
    }
}

/// How a selector combines with the selectors above it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Add,
    Subtract,
    Intersect,
    Min,
    Max,
    Difference,
}

impl Mode {
    pub fn from_index(i: u32) -> Mode {
        match i {
            1 => Mode::Subtract,
            2 => Mode::Intersect,
            3 => Mode::Min,
            4 => Mode::Max,
            5 => Mode::Difference,
            _ => Mode::Add,
        }
    }
    /// Selection before the first selector: nothing for Add / Max / Difference, everything for
    /// Subtract / Intersect / Min (so a lone Subtract selector inverts its range).
    pub fn initial(self) -> f64 {
        match self {
            Mode::Subtract | Mode::Intersect | Mode::Min => 1.0,
            Mode::Add | Mode::Max | Mode::Difference => 0.0,
        }
    }
    /// Combine the selection so far (`acc`) with this selector's value `v`.
    pub fn combine(self, acc: f64, v: f64) -> f64 {
        let r = match self {
            Mode::Add => acc + v,
            // Subtracting more than is selected leaves nothing, not a negative selection (After
            // Effects 2026: a Subtract selector over unselected characters leaves their
            // Tracking at 0, EXPORTER read-back t8).
            // Intended for every animator property, not only Tracking: selectors combine into one
            // selection per character for the whole animator (`render::text::animator_selection`),
            // which then scales each of its properties, as in After Effects, where selectors
            // belong to the animator. The read-back observed that selection through Tracking;
            // Opacity, Position and the rest were not measured separately.
            Mode::Subtract if acc >= 0.0 => (acc - v).max(0.0),
            Mode::Subtract => acc - v,
            Mode::Intersect => acc * v,
            Mode::Min => acc.min(v),
            Mode::Max => acc.max(v),
            Mode::Difference => (acc - v).abs(),
        };
        r.clamp(-1.0, 1.0)
    }
}

/// What a selector counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BasedOn {
    #[default]
    Characters,
    CharactersExcludingSpaces,
    Words,
    Lines,
}

impl BasedOn {
    pub fn from_index(i: u32) -> BasedOn {
        match i {
            1 => BasedOn::CharactersExcludingSpaces,
            2 => BasedOn::Words,
            3 => BasedOn::Lines,
            _ => BasedOn::Characters,
        }
    }
}

/// Evaluated Range Selector parameters (fractions of the unit count, not percent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub start: f64,
    pub end: f64,
    pub offset: f64,
    /// -1..1.
    pub amount: f64,
    pub shape: Shape,
    /// 0..1 (Square only): width of the soft edge in units.
    pub smoothness: f64,
    /// -1..1.
    pub ease_high: f64,
    pub ease_low: f64,
}

impl Default for Range {
    fn default() -> Self {
        Range { start: 0.0, end: 1.0, offset: 0.0, amount: 1.0, shape: Shape::Square, smoothness: 1.0, ease_high: 0.0, ease_low: 0.0 }
    }
}

/// Shape profile over the range (`f` = 0 at Start, 1 at End).
pub fn shape_value(shape: Shape, f: f64) -> f64 {
    match shape {
        Shape::Square => {
            if (0.0..=1.0).contains(&f) {
                1.0
            } else {
                0.0
            }
        }
        // Ramps hold their end values outside the range (cascades animate Offset across it).
        Shape::RampUp => f.clamp(0.0, 1.0),
        Shape::RampDown => 1.0 - f.clamp(0.0, 1.0),
        _ if !(0.0..=1.0).contains(&f) => 0.0,
        Shape::Triangle => 1.0 - (2.0 * f - 1.0).abs(),
        Shape::Round => (1.0 - (2.0 * f - 1.0).powi(2)).max(0.0).sqrt(),
        Shape::Smooth => {
            let t = 1.0 - (2.0 * f - 1.0).abs();
            t * t * (3.0 - 2.0 * t)
        }
    }
}

/// Ease High / Low remap of a selection value `v` (0..1): positive values slow the change near
/// the fully selected (high) or unselected (low) end, negative values make it more abrupt. A cubic
/// Bezier `y(t)` with control values `(1 - low)/3` and `1 - (1 - high)/3`; monotonic for -1..1.
pub fn ease(v: f64, high: f64, low: f64) -> f64 {
    if high == 0.0 && low == 0.0 {
        return v;
    }
    let t = v.clamp(0.0, 1.0);
    let p1 = (1.0 - low.clamp(-1.0, 1.0)) / 3.0;
    let p2 = 1.0 - (1.0 - high.clamp(-1.0, 1.0)) / 3.0;
    let u = 1.0 - t;
    3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
}

/// Selection of unit `i` of `n` (0-based, after any random reordering) by a range selector.
pub fn range_value(r: &Range, i: usize, n: usize) -> f64 {
    let n = n.max(1) as f64;
    let (mut s, mut e) = (r.start, r.end);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    s += r.offset;
    e += r.offset;
    let unit = 1.0 / n;
    let c = (i as f64 + 0.5) * unit;
    let v = match r.shape {
        Shape::Square => {
            let w = r.smoothness.clamp(0.0, 1.0) * unit;
            let edge = |d: f64| {
                if w > 1e-12 {
                    (d / w + 0.5).clamp(0.0, 1.0)
                } else if d >= 0.0 {
                    1.0
                } else {
                    0.0
                }
            };
            if e - s <= 1e-12 { 0.0 } else { edge(c - s).min(edge(e - c)) }
        }
        shape => {
            if e - s <= 1e-12 {
                // Degenerate range: ramps step at the point, others select nothing.
                match shape {
                    Shape::RampUp => f64::from(u8::from(c >= s)),
                    Shape::RampDown => f64::from(u8::from(c < s)),
                    _ => 0.0,
                }
            } else {
                ease(shape_value(shape, (c - s) / (e - s)), r.ease_high, r.ease_low)
            }
        }
    };
    v * r.amount
}

/// SplitMix64 step (deterministic hashing for seeds and noise lattices).
pub fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash3(a: u64, b: u64, c: u64) -> u64 {
    splitmix(splitmix(splitmix(a) ^ b.wrapping_mul(0x632B_E59B_D9B4_E019)) ^ c.wrapping_mul(0x8CB9_2BA7_2F3D_8DD7))
}

/// Randomize Order: a seeded permutation `p` with `p[i]` = the position unit `i` takes.
pub fn random_order(n: usize, seed: u64) -> Vec<usize> {
    let mut p: Vec<usize> = (0..n).collect();
    let mut s = splitmix(seed ^ 0x5EED);
    for i in (1..n).rev() {
        s = splitmix(s);
        let j = (s % (i as u64 + 1)) as usize;
        p.swap(i, j);
    }
    p
}

/// Smooth 1D value noise in -1..1 (quintic interpolation of seeded lattice values).
pub fn noise1(x: f64, seed: u64, channel: u64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let lat = |k: f64| {
        let h = hash3(seed, channel, k as i64 as u64);
        (h >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let a = lat(i);
    let b = lat(i + 1.0);
    let t = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    a + (b - a) * t
}

/// Evaluated Wiggly Selector parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wiggly {
    /// -1..1.
    pub max: f64,
    pub min: f64,
    pub wiggles_per_second: f64,
    /// 0..1: 1 = every unit moves together.
    pub correlation: f64,
    /// Degrees (one cycle = 360).
    pub temporal_phase: f64,
    pub spatial_phase: f64,
    pub lock_dimensions: bool,
    pub seed: u64,
}

impl Default for Wiggly {
    fn default() -> Self {
        Wiggly { max: 1.0, min: -1.0, wiggles_per_second: 2.0, correlation: 0.5, temporal_phase: 0.0, spatial_phase: 0.0, lock_dimensions: false, seed: 0 }
    }
}

/// Wiggly selection of unit `i` at layer time `t` seconds for dimension `dim` (0..3).
pub fn wiggly_value(w: &Wiggly, t: f64, i: usize, dim: usize) -> f64 {
    let ch = if w.lock_dimensions { 0 } else { dim as u64 };
    let tau = t * w.wiggles_per_second + w.temporal_phase / 360.0;
    let shared = noise1(tau, w.seed, ch * 2);
    // Each unit's own channel; Spatial Phase slides the pattern along the units.
    let along = i as f64 * 0.61 + w.spatial_phase / 360.0;
    let own = 0.5 * noise1(tau + along, w.seed ^ 0xA5A5, ch * 2 + 1) + 0.5 * noise1(tau * 1.37 + along * 1.7, w.seed ^ (i as u64 + 1).wrapping_mul(0x9E37), ch);
    let c = w.correlation.clamp(0.0, 1.0);
    let v = (c * shared + (1.0 - c) * own.clamp(-1.0, 1.0)).clamp(-1.0, 1.0);
    w.min + (v + 1.0) * 0.5 * (w.max - w.min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: f64, end: f64, shape: Shape) -> Range {
        Range { start, end, shape, ..Default::default() }
    }

    #[test]
    fn square_partial_coverage_and_smoothness() {
        // 4 units, range 0..0.375: units 0 fully, 1 half (smoothness 100%), 2 none.
        let rg = r(0.0, 0.375, Shape::Square);
        assert!((range_value(&rg, 0, 4) - 1.0).abs() < 1e-9);
        assert!((range_value(&rg, 1, 4) - 0.5).abs() < 1e-9);
        assert_eq!(range_value(&rg, 2, 4), 0.0);
        // Smoothness 0: hard edge at the unit centre.
        let hard = Range { smoothness: 0.0, ..rg };
        assert_eq!(range_value(&hard, 1, 4), 1.0);
        let hard2 = Range { end: 0.3, ..hard };
        assert_eq!(range_value(&hard2, 1, 4), 0.0);
        // Everything selected by default.
        assert!((0..7).all(|i| (range_value(&Range::default(), i, 7) - 1.0).abs() < 1e-9));
    }

    #[test]
    fn shapes_profiles() {
        let n = 5;
        let up: Vec<f64> = (0..n).map(|i| range_value(&r(0.0, 1.0, Shape::RampUp), i, n)).collect();
        assert!(up.windows(2).all(|w| w[1] > w[0]));
        assert!((up[0] - 0.1).abs() < 1e-9 && (up[4] - 0.9).abs() < 1e-9);
        let down: Vec<f64> = (0..n).map(|i| range_value(&r(0.0, 1.0, Shape::RampDown), i, n)).collect();
        assert!((down[0] - 0.9).abs() < 1e-9);
        let tri: Vec<f64> = (0..n).map(|i| range_value(&r(0.0, 1.0, Shape::Triangle), i, n)).collect();
        assert!((tri[2] - 1.0).abs() < 1e-9 && (tri[0] - tri[4]).abs() < 1e-9 && tri[0] < 0.5);
        let round = range_value(&r(0.0, 1.0, Shape::Round), 0, n);
        assert!(round > tri[0], "round bulges above triangle");
        let smooth = range_value(&r(0.0, 1.0, Shape::Smooth), 0, n);
        assert!(smooth < tri[0], "smooth eases below triangle");
        // Ramp Up holds after the end, ramp down before the start; others are 0 outside.
        assert_eq!(range_value(&r(0.0, 0.2, Shape::RampUp), 4, n), 1.0);
        assert_eq!(range_value(&r(0.8, 1.0, Shape::RampDown), 0, n), 1.0);
        assert_eq!(range_value(&r(0.0, 0.2, Shape::Triangle), 4, n), 0.0);
        // Offset moves the range.
        let off = Range { offset: 0.6, ..r(0.0, 0.2, Shape::Square) };
        assert_eq!(range_value(&off, 0, n), 0.0);
        assert!((range_value(&off, 3, n) - 1.0).abs() < 1e-9);
        // Amount scales (and may invert).
        let neg = Range { amount: -0.5, ..Range::default() };
        assert!((range_value(&neg, 2, n) + 0.5).abs() < 1e-9);
    }

    #[test]
    fn ease_high_low() {
        assert_eq!(ease(0.3, 0.0, 0.0), 0.3);
        assert!((ease(0.0, 1.0, 1.0)).abs() < 1e-12 && (ease(1.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        // Ease Low 100% flattens the start, Ease High 100% the top.
        assert!(ease(0.1, 0.0, 1.0) < 0.1);
        assert!(ease(0.9, 1.0, 0.0) > 0.9);
        // Negative makes it abrupt.
        assert!(ease(0.1, 0.0, -1.0) > 0.1);
        // Monotonic.
        let v: Vec<f64> = (0..=20).map(|i| ease(i as f64 / 20.0, -1.0, 1.0)).collect();
        assert!(v.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn modes_combine() {
        assert_eq!(Mode::Add.combine(0.5, 0.75), 1.0);
        assert_eq!(Mode::Subtract.combine(1.0, 0.25), 0.75);
        assert_eq!(Mode::Intersect.combine(0.5, 0.5), 0.25);
        assert_eq!(Mode::Min.combine(0.3, 0.6), 0.3);
        assert_eq!(Mode::Max.combine(0.3, 0.6), 0.6);
        assert_eq!(Mode::Difference.combine(0.3, 1.0), 0.7);
        // A lone Subtract inverts.
        assert_eq!(Mode::Subtract.combine(Mode::Subtract.initial(), 1.0), 0.0);
        assert_eq!(Mode::Subtract.combine(Mode::Subtract.initial(), 0.0), 1.0);
        // Subtracting from what is not selected leaves 0, not a negative selection (AE t8).
        assert_eq!(Mode::Subtract.combine(0.0, 1.0), 0.0);
        assert_eq!(Mode::Subtract.combine(0.25, 1.0), 0.0);
        assert_eq!(Mode::from_index(5), Mode::Difference);
    }

    #[test]
    fn randomize_is_a_seeded_permutation() {
        let a = random_order(20, 7);
        let mut s = a.clone();
        s.sort_unstable();
        assert_eq!(s, (0..20).collect::<Vec<_>>());
        assert_eq!(a, random_order(20, 7));
        assert_ne!(a, random_order(20, 8));
        assert_ne!(a, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn wiggly_is_deterministic_smooth_and_bounded() {
        let w = Wiggly { seed: 3, ..Default::default() };
        let a = wiggly_value(&w, 1.234, 5, 0);
        assert_eq!(a, wiggly_value(&w, 1.234, 5, 0));
        for i in 0..50 {
            let v = wiggly_value(&w, i as f64 * 0.137, i % 7, i % 3);
            assert!((-1.0..=1.0).contains(&v));
        }
        // Smooth in time.
        let d = (wiggly_value(&w, 1.0, 2, 0) - wiggly_value(&w, 1.001, 2, 0)).abs();
        assert!(d < 0.02, "{d}");
        // Correlation 100%: all units identical; 0%: they differ.
        let c1 = Wiggly { correlation: 1.0, ..w };
        assert_eq!(wiggly_value(&c1, 0.7, 0, 0), wiggly_value(&c1, 0.7, 9, 0));
        let c0 = Wiggly { correlation: 0.0, ..w };
        assert_ne!(wiggly_value(&c0, 0.7, 0, 0), wiggly_value(&c0, 0.7, 9, 0));
        // Lock Dimensions: same value in every dimension.
        let lk = Wiggly { lock_dimensions: true, ..w };
        assert_eq!(wiggly_value(&lk, 0.7, 3, 0), wiggly_value(&lk, 0.7, 3, 1));
        assert_ne!(wiggly_value(&w, 0.7, 3, 0), wiggly_value(&w, 0.7, 3, 1));
        // Min/Max map the range.
        let pos = Wiggly { min: 0.0, max: 1.0, ..w };
        assert!((0..30).all(|i| wiggly_value(&pos, i as f64 * 0.3, i, 0) >= 0.0));
        // Changes over time.
        assert_ne!(wiggly_value(&w, 0.0, 1, 0), wiggly_value(&w, 0.8, 1, 0));
    }
}
