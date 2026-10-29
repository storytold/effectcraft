use super::{invert, mul, mul_vec, rgb_to_xyz};

pub(super) type M3 = [[f64; 3]; 3];

pub(crate) const D65: [f64; 2] = [0.3127, 0.3290];

const ACES_WHITE: [f64; 2] = [0.32168, 0.33767];

const AP0: [[f64; 2]; 3] = [[0.7347, 0.2653], [0.0, 1.0], [0.0001, -0.0770]];

const AP1: [[f64; 2]; 3] = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];

pub(crate) const REC709: [[f64; 2]; 3] = [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]];

pub(crate) const REC2020: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];

pub(crate) const P3: [[f64; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];

pub(super) const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Transfer functions (encoded ↔ linear).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tf {
    Linear,
    Srgb,
    Gamma(f64),
    AcesCct,
    /// ACEScc (Academy S-2014-003), the pure-log grading encoding.
    AcesCc,
    /// SMPTE ST 2084 with 1.0 = 100 cd/m².
    Pq,
}

/// ACEScc: the lowest code value (linear 0 and below) and the end of its toe (linear 2⁻¹⁵).
const ACESCC_MIN: f64 = (-16.0 + 9.72) / 17.52;

const ACESCC_TOE: f64 = (-15.0 + 9.72) / 17.52;

impl Tf {
    pub fn decode(self, v: f64) -> f64 {
        match self {
            Tf::Linear => v,
            Tf::Srgb => {
                let a = v.abs();
                let l = if a <= 0.04045 { a / 12.92 } else { ((a + 0.055) / 1.055).powf(2.4) };
                l.copysign(v)
            }
            Tf::Gamma(g) => v.abs().powf(g).copysign(v),
            Tf::AcesCct => {
                if v <= 0.155_251_141_552_511 {
                    (v - 0.072_905_534_195_835_5) / 10.540_237_741_654_5
                } else if v < (65504f64.log2() + 9.72) / 17.52 {
                    2f64.powf(v * 17.52 - 9.72)
                } else {
                    65504.0
                }
            }
            Tf::AcesCc => {
                if v < ACESCC_TOE {
                    (2f64.powf(v * 17.52 - 9.72) - 2f64.powi(-16)) * 2.0
                } else if v < (65504f64.log2() + 9.72) / 17.52 {
                    2f64.powf(v * 17.52 - 9.72)
                } else {
                    65504.0
                }
            }
            Tf::Pq => {
                let (m1, m2, c1, c2, c3) = pq_consts();
                let e = v.max(0.0).powf(1.0 / m2);
                let l = ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1);
                l * 100.0
            }
        }
    }
    pub fn encode(self, v: f64) -> f64 {
        match self {
            Tf::Linear => v,
            Tf::Srgb => {
                let a = v.abs();
                let e = if a <= 0.003_130_8 { a * 12.92 } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
                e.copysign(v)
            }
            Tf::Gamma(g) => v.abs().powf(1.0 / g).copysign(v),
            Tf::AcesCct => {
                if v <= 0.007_812_5 {
                    10.540_237_741_654_5 * v + 0.072_905_534_195_835_5
                } else {
                    (v.log2() + 9.72) / 17.52
                }
            }
            Tf::AcesCc => {
                if v <= 0.0 {
                    ACESCC_MIN
                } else if v < 2f64.powi(-15) {
                    ((2f64.powi(-16) + v * 0.5).log2() + 9.72) / 17.52
                } else {
                    (v.log2() + 9.72) / 17.52
                }
            }
            Tf::Pq => {
                let (m1, m2, c1, c2, c3) = pq_consts();
                let y = (v / 100.0).max(0.0).powf(m1);
                ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
            }
        }
    }
}

fn pq_consts() -> (f64, f64, f64, f64, f64) {
    (2610.0 / 16384.0, 2523.0 / 4096.0 * 128.0, 3424.0 / 4096.0, 2413.0 / 4096.0 * 32.0, 2392.0 / 4096.0 * 32.0)
}

/// A colour space of the built-in config.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    pub name: &'static str,
    /// Primaries and white (None = CIE XYZ itself).
    pub prims: Option<([[f64; 2]; 3], [f64; 2])>,
    pub tf: Tf,
    /// Passes data through untouched.
    pub raw: bool,
}

pub(crate) const fn sp(name: &'static str, prims: [[f64; 2]; 3], white: [f64; 2], tf: Tf) -> Space {
    Space { name, prims: Some((prims, white)), tf, raw: false }
}

/// The built-in configuration's colour spaces, in popup order.
pub(crate) const ACES2065_1: Space = sp("ACES2065-1", AP0, ACES_WHITE, Tf::Linear);

pub(crate) const ACES_CG: Space = sp("ACEScg", AP1, ACES_WHITE, Tf::Linear);

pub(super) const ACES_CCT: Space = sp("ACEScct", AP1, ACES_WHITE, Tf::AcesCct);

pub const SPACES: &[Space] = &[
    ACES2065_1,
    ACES_CG,
    ACES_CCT,
    sp("Linear Rec.709 (sRGB)", REC709, D65, Tf::Linear),
    sp("sRGB", REC709, D65, Tf::Srgb),
    sp("Gamma 2.4 Rec.709", REC709, D65, Tf::Gamma(2.4)),
    sp("Linear Rec.2020", REC2020, D65, Tf::Linear),
    sp("Gamma 2.4 Rec.2020", REC2020, D65, Tf::Gamma(2.4)),
    sp("Linear P3-D65", P3, D65, Tf::Linear),
    sp("Display P3", P3, D65, Tf::Srgb),
    Space { name: "CIE-XYZ-D65", prims: None, tf: Tf::Linear, raw: false },
    Space { name: "Raw", prims: None, tf: Tf::Linear, raw: true },
];

pub fn space_names() -> Vec<&'static str> {
    SPACES.iter().map(|s| s.name).collect()
}

#[cfg(test)]
pub fn space_by_name(n: &str) -> Option<&'static Space> {
    SPACES.iter().find(|s| s.name.eq_ignore_ascii_case(n))
}

/// Bradford chromatic adaptation from white `src` to white `dst` (xy).
pub fn bradford(src: [f64; 2], dst: [f64; 2]) -> M3 {
    if src == dst {
        return IDENTITY;
    }
    const MB: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let xyz = |c: [f64; 2]| [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]];
    let s = mul_vec(&MB, xyz(src));
    let d = mul_vec(&MB, xyz(dst));
    let diag = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul(&invert(&MB), &mul(&diag, &MB))
}

impl Space {
    /// Linear RGB → XYZ D65 (Bradford-adapted when `adapt`, else the space's own white).
    pub fn to_ref(self, adapt: bool) -> M3 {
        match self.prims {
            None => IDENTITY,
            Some((p, w)) => {
                let m = rgb_to_xyz(p, w);
                if adapt { mul(&bradford(w, D65), &m) } else { m }
            }
        }
    }
}

/// Linear-RGB matrix from space `a` to space `b`.
pub fn matrix(a: &Space, b: &Space) -> M3 {
    mul(&invert(&b.to_ref(true)), &a.to_ref(true))
}

/// A compiled colour-space conversion.
#[derive(Clone, Copy, Debug)]
pub struct Xform {
    pub(super) dec: Tf,
    pub(super) m: [[f32; 3]; 3],
    pub(super) ident: bool,
    pub(super) enc: Tf,
}

impl Xform {
    pub fn new(a: &Space, b: &Space) -> Xform {
        if a.raw || b.raw {
            return Xform { dec: Tf::Linear, m: IDENTITY.map(|r| r.map(|v| v as f32)), ident: true, enc: Tf::Linear };
        }
        let m = matrix(a, b);
        let ident = (0..3).all(|i| (0..3).all(|j| (m[i][j] - IDENTITY[i][j]).abs() < 1e-12));
        Xform { dec: a.tf, m: m.map(|r| r.map(|v| v as f32)), ident, enc: b.tf }
    }
    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let l = c.map(|v| self.dec.decode(v as f64) as f32);
        let l = if self.ident { l } else { [0, 1, 2].map(|i| self.m[i][0] * l[0] + self.m[i][1] * l[1] + self.m[i][2] * l[2]) };
        l.map(|v| self.enc.encode(v as f64) as f32)
    }
}
