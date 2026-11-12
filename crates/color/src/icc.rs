//! A reader for matrix/TRC RGB ICC profiles (ICC.1:2001-04 v2 and ICC.1:2010 v4, the public
//! International Color Consortium specification): the red/green/blue colorants (`rXYZ`, `gXYZ`,
//! `bXYZ`), the media white point (`wtpt`), the chromatic adaptation (`chad`), the tone curves
//! (`rTRC`, `gTRC`, `bTRC` as `curv` or `para`) and the description (`desc` / `mluc`).
//!
//! LUT-based profiles ([`parse_profile`], [`LutProfile`]): `A2B0` / `B2A0` tables of type
//! `lut8Type`, `lut16Type`, `lutAToBType` and `lutBToAType` (ICC.1:2010 §10.10–10.12):
//! curves (`curv`, `para`), matrices and multilinear CLUT interpolation, with the PCS in XYZ or
//! Lab (the legacy 16-bit Lab encoding for `lut16Type`). A profile without `B2A0` is inverted
//! numerically. Implemented from the public specification only.
//!
//! Used by View ▸ Simulate Output ▸ My Custom RGB: a matrix/TRC profile becomes primaries, a white
//! point and a curve; a LUT-based profile is baked into a 3D LUT for the viewer ([`Lut3d`]).
//! Non-RGB profiles are rejected.

use crate::space::{bradford, invert, mul_vec};

/// CIE D50 (the ICC profile connection space white), xy.
pub const D50: [f64; 2] = [0.3457, 0.3585];

/// A tone curve of a profile.
#[derive(Clone, Debug, PartialEq)]
pub enum Trc {
    /// `curv` with one entry, or `para` function type 0: y = x^g.
    Gamma(f64),
    /// `curv` table (0..=65535 samples over 0..1).
    Table(Vec<u16>),
    /// `para` function types 1–4: (type, parameters g a b c d e f).
    Parametric(u16, [f64; 7]),
}

impl Trc {
    /// Encoded value (0..1) → linear.
    pub fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Trc::Gamma(g) => x.powf(*g),
            Trc::Table(t) if t.is_empty() => x,
            Trc::Table(t) => {
                let f = x * (t.len() - 1) as f64;
                let i = (f.floor() as usize).min(t.len() - 1);
                let j = (i + 1).min(t.len() - 1);
                let k = f - i as f64;
                (t[i] as f64 * (1.0 - k) + t[j] as f64 * k) / 65535.0
            }
            Trc::Parametric(ty, p) => {
                let [g, a, b, c, d, e, f] = *p;
                let pw = |v: f64| if v > 0.0 { v.powf(g) } else { 0.0 };
                match ty {
                    1 => {
                        if x >= -b / a {
                            pw(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if x >= -b / a {
                            pw(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            pw(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            pw(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
            }
        }
    }

    /// The single gamma that best matches the curve (least squares in log space over mid-tones).
    pub fn fit_gamma(&self) -> f64 {
        if let Trc::Gamma(g) = self {
            return *g;
        }
        let (mut num, mut den) = (0.0, 0.0);
        for i in 4..20 {
            let x = i as f64 / 20.0;
            let y = self.eval(x);
            if y > 1e-6 && y < 1.0 {
                num += x.ln() * y.ln();
                den += x.ln() * x.ln();
            }
        }
        if den > 0.0 { (num / den).clamp(0.1, 10.0) } else { 1.0 }
    }

    /// Whether the curve is the sRGB (IEC 61966-2-1) piecewise curve, within 8-bit precision.
    pub fn is_srgb(&self) -> bool {
        let srgb = |x: f64| if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) };
        !matches!(self, Trc::Gamma(_)) && (0..=32).all(|i| (self.eval(i as f64 / 32.0) - srgb(i as f64 / 32.0)).abs() < 0.004)
    }
}

/// What a matrix/TRC RGB profile says.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbProfile {
    pub description: String,
    /// Red, green, blue chromaticities (CIE xy) under the profile's own white.
    pub primaries: [[f64; 2]; 3],
    /// The white point (CIE xy) of the device.
    pub white: [f64; 2],
    /// Tone curves (red, green, blue).
    pub trc: [Trc; 3],
    /// ICC version major number.
    pub version: u8,
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn s15f16(b: &[u8], o: usize) -> Option<f64> {
    Some(be32(b, o)? as i32 as f64 / 65536.0)
}

fn xy(v: [f64; 3]) -> [f64; 2] {
    let s = v[0] + v[1] + v[2];
    if s.abs() < 1e-12 { D50 } else { [v[0] / s, v[1] / s] }
}

fn xyz_of(c: [f64; 2]) -> [f64; 3] {
    [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]]
}

/// Parse an RGB matrix/TRC ICC profile.
pub fn parse(b: &[u8]) -> Result<RgbProfile, String> {
    if b.len() < 132 || b.get(36..40) != Some(b"acsp") {
        return Err("not an ICC profile".into());
    }
    if b.get(16..20) != Some(b"RGB ") {
        return Err("not an RGB profile (only RGB display profiles can be simulated)".into());
    }
    let version = b[8];
    let n = be32(b, 128).ok_or("truncated tag table")? as usize;
    let mut tags = std::collections::HashMap::new();
    for i in 0..n.min(200) {
        let o = 132 + i * 12;
        let sig = b.get(o..o + 4).ok_or("truncated tag table")?;
        let off = be32(b, o + 4).ok_or("truncated tag table")? as usize;
        let size = be32(b, o + 8).ok_or("truncated tag table")? as usize;
        if off.checked_add(size).is_some_and(|e| e <= b.len()) {
            tags.insert(String::from_utf8_lossy(sig).into_owned(), &b[off..off + size]);
        }
    }
    let xyz_tag = |name: &str| -> Result<[f64; 3], String> {
        let t = tags.get(name).ok_or_else(|| format!("the profile has no {name} tag (only matrix/TRC profiles are supported)"))?;
        if t.get(0..4) != Some(b"XYZ ") {
            return Err(format!("{name}: not an XYZ tag"));
        }
        Ok([s15f16(t, 8).ok_or("bad XYZ")?, s15f16(t, 12).ok_or("bad XYZ")?, s15f16(t, 16).ok_or("bad XYZ")?])
    };
    let trc_tag = |name: &str| -> Result<Trc, String> {
        let t = tags.get(name).ok_or_else(|| format!("the profile has no {name} tag"))?;
        match t.get(0..4) {
            Some(b"curv") => {
                let count = be32(t, 8).ok_or("bad curv")? as usize;
                match count {
                    0 => Ok(Trc::Gamma(1.0)),
                    1 => Ok(Trc::Gamma(be16(t, 12).ok_or("bad curv")? as f64 / 256.0)),
                    _ => Ok(Trc::Table((0..count).map(|i| be16(t, 12 + i * 2)).collect::<Option<Vec<_>>>().ok_or("truncated curv")?)),
                }
            }
            Some(b"para") => {
                let ty = be16(t, 8).ok_or("bad para")?;
                let np = [1, 3, 4, 5, 7].get(ty as usize).copied().ok_or("unknown para function")?;
                let mut p = [0.0; 7];
                for (k, v) in p.iter_mut().enumerate().take(np) {
                    *v = s15f16(t, 12 + k * 4).ok_or("truncated para")?;
                }
                Ok(if ty == 0 { Trc::Gamma(p[0]) } else { Trc::Parametric(ty, p) })
            }
            _ => Err(format!("{name}: unsupported curve type")),
        }
    };
    let col = [xyz_tag("rXYZ")?, xyz_tag("gXYZ")?, xyz_tag("bXYZ")?];
    // The device white: from the chromatic adaptation (v4, and many v2 profiles), else the media
    // white point (v2 profiles that keep it un-adapted), else D50.
    let d50 = xyz_of(D50);
    let chad = tags.get("chad").filter(|t| t.get(0..4) == Some(b"sf32") && t.len() >= 44).map(|t| {
        let v: Vec<f64> = (0..9).map(|i| s15f16(t, 8 + i * 4).unwrap_or(0.0)).collect();
        [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]
    });
    let white = match (chad, xyz_tag("wtpt").ok()) {
        (Some(m), _) => xy(mul_vec(&invert(&m), d50)),
        (None, Some(w)) => xy(w),
        _ => D50,
    };
    // Colorants are adapted to D50 in the profile: undo that to get the device primaries.
    let undo = match chad {
        Some(m) => invert(&m),
        None => bradford(D50, white),
    };
    let primaries = col.map(|c| xy(mul_vec(&undo, c)));
    let trc = [trc_tag("rTRC")?, trc_tag("gTRC")?, trc_tag("bTRC")?];
    Ok(RgbProfile { description: description(tags.get("desc").copied()).unwrap_or_default(), primaries, white, trc, version })
}

fn description(t: Option<&[u8]>) -> Option<String> {
    let t = t?;
    match t.get(0..4)? {
        b"desc" => {
            let n = be32(t, 8)? as usize;
            let s = t.get(12..12 + n)?;
            Some(String::from_utf8_lossy(s).trim_end_matches('\0').to_string())
        }
        b"mluc" => {
            // The first record: UTF-16BE.
            let len = be32(t, 20)? as usize;
            let off = be32(t, 24)? as usize;
            let u: Vec<u16> = t.get(off..off + len)?.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
            Some(String::from_utf16_lossy(&u).trim_end_matches('\0').to_string())
        }
        _ => None,
    }
}

/// Build a minimal v2 matrix/TRC RGB profile (tests, and saving a custom simulation): colorants
/// Bradford-adapted to D50, `wtpt` the device white, gamma curves.
pub fn write_matrix_profile(desc: &str, primaries: [[f64; 2]; 3], white: [f64; 2], gamma: f64) -> Vec<u8> {
    let m = crate::space::rgb_to_xyz(primaries, white);
    let a = bradford(white, D50);
    let col = |j: usize| mul_vec(&a, [m[0][j], m[1][j], m[2][j]]);
    let f = |v: f64| ((v * 65536.0).round() as i32).to_be_bytes();
    let xyz = |v: [f64; 3]| {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        for c in v {
            t.extend(f(c));
        }
        t
    };
    let curv = {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend(1u32.to_be_bytes());
        t.extend(((gamma * 256.0).round() as u16).to_be_bytes());
        t.extend([0, 0]);
        t
    };
    let dsc = {
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend((desc.len() as u32 + 1).to_be_bytes());
        t.extend(desc.as_bytes());
        t.push(0);
        t.extend([0u8; 12 + 67]);
        while !t.len().is_multiple_of(4) {
            t.push(0);
        }
        t
    };
    let w = xyz_of(white);
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", dsc),
        (b"wtpt", xyz(w)),
        (b"rXYZ", xyz(col(0))),
        (b"gXYZ", xyz(col(1))),
        (b"bXYZ", xyz(col(2))),
        (b"rTRC", curv.clone()),
        (b"gTRC", curv.clone()),
        (b"bTRC", curv),
    ];
    let mut table = vec![];
    let mut data = vec![];
    let base = 128 + 4 + tags.len() * 12;
    for (sig, t) in &tags {
        table.extend(*sig);
        table.extend(((base + data.len()) as u32).to_be_bytes());
        table.extend((t.len() as u32).to_be_bytes());
        data.extend(t);
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
    }
    let total = base + data.len();
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    h[8] = 2;
    h[9] = 0x10;
    h[12..16].copy_from_slice(b"mntr");
    h[16..20].copy_from_slice(b"RGB ");
    h[20..24].copy_from_slice(b"XYZ ");
    h[36..40].copy_from_slice(b"acsp");
    for (k, v) in xyz_of(D50).into_iter().enumerate() {
        h[68 + k * 4..72 + k * 4].copy_from_slice(&f(v));
    }
    let mut out = h;
    out.extend((tags.len() as u32).to_be_bytes());
    out.extend(table);
    out.extend(data);
    out
}

// ---------------------------------------------------------------- LUT-based profiles

/// One processing element of a LUT transform. Values between elements are normalised to 0..1
/// (ICC.1:2010 §10.10–10.12: `lut8Type`, `lut16Type`, `lutAToBType`, `lutBToAType`).
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// One curve per channel.
    Curves(Vec<Trc>),
    /// A 3×3 matrix and offsets (`lutAToBType` / `lutBToAType`; `lut8/16Type` without offsets).
    Matrix([[f64; 3]; 3], [f64; 3]),
    Clut(Clut),
}

/// A colour lookup table: `grid[i]` points along input `i`, `outputs` values per point
/// (normalised 0..1), the first input varying slowest (ICC order).
#[derive(Clone, Debug, PartialEq)]
pub struct Clut {
    pub grid: Vec<usize>,
    pub outputs: usize,
    pub data: Vec<f32>,
}

/// Most interpolating inputs [`Clut::eval`] combines (ICC allows 15).
pub const MAX_CLUT_INPUTS: usize = 16;

impl Clut {
    /// Multilinear interpolation at `x` (0..1 per input).
    pub fn eval(&self, x: &[f64]) -> Vec<f64> {
        let n = self.grid.len();
        let mut base = 0usize;
        let mut stride = vec![0usize; n];
        let mut s = self.outputs;
        for i in (0..n).rev() {
            stride[i] = s;
            // Saturating: a table with enough axes to overflow cannot hold its data anyway, and a
            // saturated offset reads nothing (zero) instead of panicking.
            s = s.saturating_mul(self.grid[i].max(1));
        }
        let mut frac = vec![0.0; n];
        let mut step = vec![0usize; n];
        for i in 0..n {
            let g = self.grid[i].max(1);
            let f = x.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0) * (g - 1) as f64;
            let mut k = (f.floor() as usize).min(g.saturating_sub(2));
            frac[i] = if g > 1 { f - k as f64 } else { 0.0 };
            step[i] = if g > 1 { stride[i] } else { 0 };
            // The last grid point is a corner of its own, not an interpolation toward the next.
            if frac[i] >= 1.0 {
                k += 1;
                frac[i] = 0.0;
            }
            base = base.saturating_add(k.saturating_mul(stride[i]));
        }
        let mut out = vec![0.0; self.outputs];
        // Only the axes that interpolate have two corners: a singleton axis, or one sampled on a
        // grid point, contributes its lower corner alone. ICC tables have at most 15 inputs; a
        // table built with more interpolating axes than that evaluates to zero.
        let active: Vec<usize> = (0..n).filter(|&i| step[i] != 0 && frac[i] != 0.0).collect();
        if active.len() > MAX_CLUT_INPUTS {
            return out;
        }
        for corner in 0..(1usize << active.len()) {
            let mut w = 1.0;
            let mut off = base;
            for (bit, &i) in active.iter().enumerate() {
                if corner >> bit & 1 == 1 {
                    w *= frac[i];
                    off = off.saturating_add(step[i]);
                } else {
                    w *= 1.0 - frac[i];
                }
            }
            if w == 0.0 {
                continue;
            }
            for (o, v) in out.iter_mut().enumerate() {
                *v += w * self.data.get(off.saturating_add(o)).copied().unwrap_or(0.0) as f64;
            }
        }
        out
    }
}

/// A LUT transform (`A2B0` device → PCS, or `B2A0` PCS → device).
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub inputs: usize,
    pub outputs: usize,
    pub stages: Vec<Stage>,
    /// `lut16Type`: PCS Lab uses the legacy 16-bit encoding (0xFF00 = L 100).
    pub legacy_lab: bool,
}

impl Lut {
    /// Run the transform on normalised values.
    pub fn eval(&self, x: &[f64]) -> Vec<f64> {
        let mut v: Vec<f64> = x.iter().map(|v| v.clamp(0.0, 1.0)).collect();
        for s in &self.stages {
            v = match s {
                Stage::Curves(c) => v.iter().enumerate().map(|(i, x)| c.get(i).map_or(*x, |t| t.eval(*x))).collect(),
                Stage::Matrix(m, o) if v.len() == 3 => (0..3).map(|i| (m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + o[i]).clamp(0.0, 1.0)).collect(),
                Stage::Matrix(..) => v,
                Stage::Clut(c) => c.eval(&v),
            };
        }
        v
    }
}

/// The profile connection space of a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pcs {
    Xyz,
    Lab,
}

/// D50 white (X, Y, Z) of the PCS.
const D50_XYZ: [f64; 3] = [0.9642, 1.0, 0.8249];

fn lab_to_xyz(l: [f64; 3]) -> [f64; 3] {
    let fy = (l[0] + 16.0) / 116.0;
    let fx = fy + l[1] / 500.0;
    let fz = fy - l[2] / 200.0;
    let inv = |t: f64| if t > 6.0 / 29.0 { t * t * t } else { 3.0 * (6.0f64 / 29.0).powi(2) * (t - 4.0 / 29.0) };
    [D50_XYZ[0] * inv(fx), D50_XYZ[1] * inv(fy), D50_XYZ[2] * inv(fz)]
}

fn xyz_to_lab(x: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| if t > (6.0f64 / 29.0).powi(3) { t.cbrt() } else { t / (3.0 * (6.0f64 / 29.0).powi(2)) + 4.0 / 29.0 };
    let (fx, fy, fz) = (f(x[0] / D50_XYZ[0]), f(x[1] / D50_XYZ[1]), f(x[2] / D50_XYZ[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Normalised PCS values (a LUT's output or input) → CIE XYZ (D50).
pub fn pcs_to_xyz(pcs: Pcs, legacy: bool, n: &[f64]) -> [f64; 3] {
    let g = |i: usize| n.get(i).copied().unwrap_or(0.0);
    match pcs {
        Pcs::Xyz => [0, 1, 2].map(|i| g(i) * 65535.0 / 32768.0),
        Pcs::Lab => {
            let k = if legacy { 65535.0 / 65280.0 } else { 1.0 };
            lab_to_xyz([g(0) * k * 100.0, g(1) * k * 255.0 - 128.0, g(2) * k * 255.0 - 128.0])
        }
    }
}

/// CIE XYZ (D50) → normalised PCS values.
pub fn xyz_to_pcs(pcs: Pcs, legacy: bool, xyz: [f64; 3]) -> [f64; 3] {
    match pcs {
        Pcs::Xyz => xyz.map(|v| (v * 32768.0 / 65535.0).clamp(0.0, 1.0)),
        Pcs::Lab => {
            let l = xyz_to_lab(xyz);
            let k = if legacy { 65280.0 / 65535.0 } else { 1.0 };
            [(l[0] / 100.0 * k).clamp(0.0, 1.0), ((l[1] + 128.0) / 255.0 * k).clamp(0.0, 1.0), ((l[2] + 128.0) / 255.0 * k).clamp(0.0, 1.0)]
        }
    }
}

/// What a LUT-based RGB profile says: its `A2B0` (device → PCS) and, when present, its `B2A0`
/// (PCS → device).
#[derive(Clone, Debug, PartialEq)]
pub struct LutProfile {
    pub description: String,
    pub pcs: Pcs,
    pub a2b: Lut,
    pub b2a: Option<Lut>,
    pub version: u8,
}

impl LutProfile {
    /// Device RGB (0..1) → CIE XYZ (D50, relative to the PCS white).
    pub fn to_xyz(&self, rgb: [f64; 3]) -> [f64; 3] {
        pcs_to_xyz(self.pcs, self.a2b.legacy_lab, &self.a2b.eval(&rgb))
    }

    /// CIE XYZ (D50) → device RGB (0..1): through `B2A0`, or by inverting `A2B0` (Gauss-Newton
    /// from `guess`, clipped to the device cube) when the profile has none.
    pub fn from_xyz(&self, xyz: [f64; 3], guess: [f64; 3]) -> [f64; 3] {
        if let Some(b) = &self.b2a {
            let v = b.eval(&xyz_to_pcs(self.pcs, b.legacy_lab, xyz));
            return [0, 1, 2].map(|i| v.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0));
        }
        self.invert(xyz, guess)
    }

    /// Device RGB whose `A2B0` result is closest to `xyz` (least squares in CIE Lab).
    pub fn invert(&self, xyz: [f64; 3], guess: [f64; 3]) -> [f64; 3] {
        let target = xyz_to_lab(xyz);
        let err = |rgb: [f64; 3]| {
            let l = xyz_to_lab(self.to_xyz(rgb));
            [l[0] - target[0], l[1] - target[1], l[2] - target[2]]
        };
        let norm = |e: [f64; 3]| e[0] * e[0] + e[1] * e[1] + e[2] * e[2];
        let mut x = guess.map(|v| v.clamp(0.0, 1.0));
        let mut e = err(x);
        for _ in 0..24 {
            if norm(e) < 1e-6 {
                break;
            }
            // Finite-difference Jacobian (one-sided, inward at the cube's faces).
            let h = 1e-3;
            let mut j = [[0.0; 3]; 3];
            for c in 0..3 {
                let mut y = x;
                let d = if x[c] + h <= 1.0 { h } else { -h };
                y[c] += d;
                let ey = err(y);
                for r in 0..3 {
                    j[r][c] = (ey[r] - e[r]) / d;
                }
            }
            let det = j[0][0] * (j[1][1] * j[2][2] - j[1][2] * j[2][1]) - j[0][1] * (j[1][0] * j[2][2] - j[1][2] * j[2][0])
                + j[0][2] * (j[1][0] * j[2][1] - j[1][1] * j[2][0]);
            if det.abs() < 1e-12 {
                break;
            }
            let ji = crate::space::invert(&j);
            let dx = crate::space::mul_vec(&ji, e);
            // Damped step: halve until the error drops.
            let mut t = 1.0;
            let mut improved = false;
            for _ in 0..8 {
                let y = [0, 1, 2].map(|i| (x[i] - t * dx[i]).clamp(0.0, 1.0));
                let ey = err(y);
                if norm(ey) < norm(e) {
                    x = y;
                    e = ey;
                    improved = true;
                    break;
                }
                t *= 0.5;
            }
            if !improved {
                break;
            }
        }
        x
    }

    /// Chromaticities and white the profile's device approximately has (what the dialog shows):
    /// the primaries' and white's `A2B0` results (CIE xy, D50-relative), and the gamma that
    /// best fits its neutral ramp.
    pub fn approximate(&self) -> ([[f64; 2]; 3], [f64; 2], f64) {
        let prim = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|c| xy(self.to_xyz(c)));
        let wy = self.to_xyz([1.0; 3])[1].max(1e-9);
        let ramp = Trc::Table((0..=64).map(|i| ((self.to_xyz([i as f64 / 64.0; 3])[1] / wy).clamp(0.0, 1.0) * 65535.0).round() as u16).collect());
        (prim, D50, ramp.fit_gamma())
    }
}

/// A parsed RGB profile: matrix/TRC, or LUT-based (`A2B0`, which takes precedence when both are
/// present, as the ICC specification says).
#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    Matrix(RgbProfile),
    Lut(LutProfile),
}

fn tag_table(b: &[u8]) -> Result<std::collections::HashMap<String, &[u8]>, String> {
    let n = be32(b, 128).ok_or("truncated tag table")? as usize;
    let mut tags = std::collections::HashMap::new();
    for i in 0..n.min(200) {
        let o = 132 + i * 12;
        let sig = b.get(o..o + 4).ok_or("truncated tag table")?;
        let off = be32(b, o + 4).ok_or("truncated tag table")? as usize;
        let size = be32(b, o + 8).ok_or("truncated tag table")? as usize;
        if off.checked_add(size).is_some_and(|e| e <= b.len()) {
            tags.insert(String::from_utf8_lossy(sig).into_owned(), &b[off..off + size]);
        }
    }
    Ok(tags)
}

/// Parse an RGB ICC profile, LUT-based or matrix/TRC.
pub fn parse_profile(b: &[u8]) -> Result<Profile, String> {
    if b.len() < 132 || b.get(36..40) != Some(b"acsp") {
        return Err("not an ICC profile".into());
    }
    if b.get(16..20) != Some(b"RGB ") {
        return Err("not an RGB profile (only RGB display profiles can be simulated)".into());
    }
    let pcs = match b.get(20..24) {
        Some(b"XYZ ") => Pcs::Xyz,
        Some(b"Lab ") => Pcs::Lab,
        _ => return Err("unknown profile connection space".into()),
    };
    let tags = tag_table(b)?;
    let Some(a2b) = tags.get("A2B0") else { return parse(b).map(Profile::Matrix) };
    let a2b = parse_lut(a2b, false).map_err(|e| format!("A2B0: {e}"))?;
    if a2b.inputs != 3 || a2b.outputs != 3 {
        return Err("A2B0 must map 3 device channels to 3 PCS values".into());
    }
    let b2a = match tags.get("B2A0").map(|t| parse_lut(t, true)) {
        Some(Ok(l)) if l.inputs == 3 && l.outputs == 3 => Some(l),
        _ => None,
    };
    Ok(Profile::Lut(LutProfile { description: description(tags.get("desc").copied()).unwrap_or_default(), pcs, a2b, b2a, version: b[8] }))
}

/// A `curv` or `para` element at `o` of `t`; returns it and its padded length.
fn curve_at(t: &[u8], o: usize) -> Result<(Trc, usize), String> {
    match t.get(o..o + 4) {
        Some(b"curv") => {
            let count = be32(t, o + 8).ok_or("bad curv")? as usize;
            let trc = match count {
                0 => Trc::Gamma(1.0),
                1 => Trc::Gamma(be16(t, o + 12).ok_or("bad curv")? as f64 / 256.0),
                _ => Trc::Table((0..count).map(|i| be16(t, o + 12 + i * 2)).collect::<Option<Vec<_>>>().ok_or("truncated curv")?),
            };
            Ok((trc, (12 + count * 2).div_ceil(4) * 4))
        }
        Some(b"para") => {
            let ty = be16(t, o + 8).ok_or("bad para")?;
            let np = [1, 3, 4, 5, 7].get(ty as usize).copied().ok_or("unknown para function")?;
            let mut p = [0.0; 7];
            for (k, v) in p.iter_mut().enumerate().take(np) {
                *v = s15f16(t, o + 12 + k * 4).ok_or("truncated para")?;
            }
            Ok((if ty == 0 { Trc::Gamma(p[0]) } else { Trc::Parametric(ty, p) }, 12 + np * 4))
        }
        _ => Err("unsupported curve element".into()),
    }
}

fn curves_at(t: &[u8], o: usize, n: usize) -> Result<Vec<Trc>, String> {
    let mut v = vec![];
    let mut o = o;
    for _ in 0..n {
        let (c, len) = curve_at(t, o)?;
        v.push(c);
        o += len;
    }
    Ok(v)
}

fn clut_len(grid: &[usize], outputs: usize) -> Option<usize> {
    grid.iter().try_fold(outputs, |a, g| a.checked_mul(*g)).filter(|n| *n <= 1 << 26)
}

/// `lut8Type` / `lut16Type` / `lutAToBType` / `lutBToAType`. `b2a`: the element order of a
/// PCS → device table.
pub fn parse_lut(t: &[u8], b2a: bool) -> Result<Lut, String> {
    let sig = t.get(0..4).ok_or("empty tag")?;
    match sig {
        b"mft1" | b"mft2" => {
            let wide = sig == b"mft2";
            let (i, o, g) = (*t.get(8).ok_or("truncated")? as usize, *t.get(9).ok_or("truncated")? as usize, *t.get(10).ok_or("truncated")? as usize);
            if i == 0 || o == 0 || i > 8 || o > 15 || g < 2 {
                return Err("bad table dimensions".into());
            }
            let mut m = [[0.0; 3]; 3];
            for (k, v) in m.iter_mut().flatten().enumerate() {
                *v = s15f16(t, 12 + k * 4).ok_or("truncated matrix")?;
            }
            let (n_in, n_out, mut p) =
                if wide { (be16(t, 48).ok_or("truncated")? as usize, be16(t, 50).ok_or("truncated")? as usize, 52) } else { (256, 256, 48) };
            if n_in < 2 || n_out < 2 {
                return Err("bad table sizes".into());
            }
            let w = if wide { 2 } else { 1 };
            let read = |p: usize| -> Option<u16> { if wide { be16(t, p) } else { t.get(p).map(|v| *v as u16 * 257) } };
            let table = |n: usize, p: &mut usize| -> Result<Trc, String> {
                let v = (0..n).map(|k| read(*p + k * w)).collect::<Option<Vec<_>>>().ok_or("truncated table")?;
                *p += n * w;
                Ok(Trc::Table(v))
            };
            let ins = (0..i).map(|_| table(n_in, &mut p)).collect::<Result<Vec<_>, _>>()?;
            let grid = vec![g; i];
            let len = clut_len(&grid, o).ok_or("table too large")?;
            let data = (0..len).map(|k| read(p + k * w).map(|v| v as f32 / 65535.0)).collect::<Option<Vec<_>>>().ok_or("truncated table")?;
            p += len * w;
            let outs = (0..o).map(|_| table(n_out, &mut p)).collect::<Result<Vec<_>, _>>()?;
            let mut stages = vec![];
            // The matrix applies only to XYZ input (a PCS → device table); identity is skipped.
            let ident = m == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            if b2a && i == 3 && !ident {
                stages.push(Stage::Matrix(m, [0.0; 3]));
            }
            stages.push(Stage::Curves(ins));
            stages.push(Stage::Clut(Clut { grid, outputs: o, data }));
            stages.push(Stage::Curves(outs));
            Ok(Lut { inputs: i, outputs: o, stages, legacy_lab: wide })
        }
        b"mAB " | b"mBA " => {
            let (i, o) = (*t.get(8).ok_or("truncated")? as usize, *t.get(9).ok_or("truncated")? as usize);
            if i == 0 || o == 0 || i > 15 || o > 15 {
                return Err("bad channel counts".into());
            }
            let off = |k: usize| be32(t, 12 + k * 4).map(|v| v as usize).ok_or("truncated offsets");
            let (ob, om, oc, oclut, oa) = (off(0)?, off(1)?, off(2)?, off(3)?, off(4)?);
            let a_n = if sig == b"mAB " { i } else { o };
            let b_n = if sig == b"mAB " { o } else { i };
            let bc = if ob > 0 { Some(Stage::Curves(curves_at(t, ob, b_n)?)) } else { None };
            let ac = if oa > 0 { Some(Stage::Curves(curves_at(t, oa, a_n)?)) } else { None };
            let mtx = if om > 0 {
                let v = (0..12).map(|k| s15f16(t, om + k * 4)).collect::<Option<Vec<_>>>().ok_or("truncated matrix")?;
                Some(Stage::Matrix([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]], [v[9], v[10], v[11]]))
            } else {
                None
            };
            let mc = if oc > 0 { Some(Stage::Curves(curves_at(t, oc, 3)?)) } else { None };
            let clut = if oclut > 0 {
                let (ci, co) = (i, o);
                let grid: Vec<usize> = (0..ci).map(|k| t.get(oclut + k).map(|v| *v as usize)).collect::<Option<_>>().ok_or("truncated clut")?;
                if grid.iter().any(|g| *g < 2) {
                    return Err("bad grid".into());
                }
                let prec = *t.get(oclut + 16).ok_or("truncated clut")? as usize;
                let len = clut_len(&grid, co).ok_or("table too large")?;
                let d = oclut + 20;
                let data = match prec {
                    1 => (0..len).map(|k| t.get(d + k).map(|v| *v as f32 / 255.0)).collect::<Option<Vec<_>>>(),
                    2 => (0..len).map(|k| be16(t, d + k * 2).map(|v| v as f32 / 65535.0)).collect::<Option<Vec<_>>>(),
                    _ => return Err("bad clut precision".into()),
                }
                .ok_or("truncated clut")?;
                Some(Stage::Clut(Clut { grid, outputs: co, data }))
            } else {
                None
            };
            if bc.is_none() {
                return Err("the B curves are required".into());
            }
            // A → CLUT → M → matrix → B (A2B); B → matrix → M → CLUT → A (B2A).
            let order = if sig == b"mAB " { [ac, clut, mc, mtx, bc] } else { [bc, mtx, mc, clut, ac] };
            Ok(Lut { inputs: i, outputs: o, stages: order.into_iter().flatten().collect(), legacy_lab: false })
        }
        _ => Err(format!("unsupported table type `{}`", String::from_utf8_lossy(sig))),
    }
}

/// The kind of table [`write_lut_profile`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutKind {
    /// `lut8Type` (PCS Lab).
    Lut8,
    /// `lut16Type` (PCS XYZ).
    Lut16,
    /// `lutAToBType` / `lutBToAType` (PCS Lab, v4) with power-curve A curves and a matrix element.
    AToB,
}

/// Build a LUT-based RGB display profile (tests, generated profiles): `A2B0` samples `to_xyz`
/// (device RGB → XYZ D50) on a `grid`³ table; with `from_xyz`, a `B2A0` samples it on the PCS.
pub fn write_lut_profile(
    desc: &str,
    kind: LutKind,
    grid: usize,
    to_xyz: &dyn Fn([f64; 3]) -> [f64; 3],
    from_xyz: Option<&dyn Fn([f64; 3]) -> [f64; 3]>,
) -> Vec<u8> {
    let (pcs, legacy) = match kind {
        LutKind::Lut8 => (Pcs::Lab, false),
        LutKind::Lut16 => (Pcs::Xyz, true),
        LutKind::AToB => (Pcs::Lab, false),
    };
    // The A curves of an AToB table (a power curve) spread the grid evenly over lightness.
    let a_gamma = 0.75;
    let pt = |k: usize| k as f64 / (grid - 1) as f64;
    let mut a2b_vals = vec![];
    let mut b2a_vals = vec![];
    for r in 0..grid {
        for g in 0..grid {
            for b in 0..grid {
                let mut rgb = [pt(r), pt(g), pt(b)];
                if kind == LutKind::AToB {
                    rgb = rgb.map(|v| v.powf(1.0 / a_gamma));
                }
                a2b_vals.extend(xyz_to_pcs(pcs, legacy, to_xyz(rgb)));
                if let Some(f) = from_xyz {
                    // A lut16 B2A0 over XYZ gets cube-root input curves (even steps in lightness).
                    let node = [pt(r), pt(g), pt(b)];
                    let x = pcs_to_xyz(pcs, legacy, &if kind == LutKind::Lut16 { node.map(|v| v * v * v) } else { node });
                    let d = f(x).map(|v| v.clamp(0.0, 1.0));
                    b2a_vals.extend(d);
                }
            }
        }
    }
    let s15 = |v: f64| ((v * 65536.0).round() as i32).to_be_bytes();
    let pad4 = |t: &mut Vec<u8>| {
        while !t.len().is_multiple_of(4) {
            t.push(0);
        }
    };
    let mft = |vals: &[f64], wide: bool, cube_in: bool| -> Vec<u8> {
        let mut t = if wide { b"mft2\0\0\0\0".to_vec() } else { b"mft1\0\0\0\0".to_vec() };
        t.extend([3, 3, grid as u8, 0]);
        for k in 0..9 {
            t.extend(s15(if k % 4 == 0 { 1.0 } else { 0.0 }));
        }
        let n = 256usize;
        if wide {
            t.extend((n as u16).to_be_bytes());
            t.extend((n as u16).to_be_bytes());
        }
        let put = |t: &mut Vec<u8>, v: f64| {
            if wide { t.extend(((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_be_bytes()) } else { t.push((v.clamp(0.0, 1.0) * 255.0).round() as u8) }
        };
        for _ in 0..3 {
            for k in 0..n {
                let x = k as f64 / (n - 1) as f64;
                put(&mut t, if cube_in { x.cbrt() } else { x });
            }
        }
        for v in vals {
            put(&mut t, *v);
        }
        for _ in 0..3 {
            for k in 0..n {
                put(&mut t, k as f64 / (n - 1) as f64);
            }
        }
        pad4(&mut t);
        t
    };
    let curv_identity = || b"curv\0\0\0\0\0\0\0\0".to_vec();
    let para_gamma = |g: f64| {
        let mut t = b"para\0\0\0\0\0\0\0\0".to_vec();
        t.extend(s15(g));
        t
    };
    let mab = |vals: &[f64], a2b: bool| -> Vec<u8> {
        let mut t = if a2b { b"mAB \0\0\0\0".to_vec() } else { b"mBA \0\0\0\0".to_vec() };
        t.extend([3, 3, 0, 0]);
        let offsets_at = t.len();
        t.extend([0u8; 20]);
        // B curves (identity).
        let ob = t.len();
        for _ in 0..3 {
            t.extend(curv_identity());
        }
        // Matrix: identity, zero offsets (exercises the element).
        let om = t.len();
        for k in 0..9 {
            t.extend(s15(if k % 4 == 0 { 1.0 } else { 0.0 }));
        }
        for _ in 0..3 {
            t.extend(s15(0.0));
        }
        // M curves (identity).
        let oc = t.len();
        for _ in 0..3 {
            t.extend(curv_identity());
        }
        let oclut = t.len();
        let mut g = [0u8; 16];
        g[..3].fill(grid as u8);
        t.extend(g);
        t.extend([2, 0, 0, 0]);
        for v in vals {
            t.extend(((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_be_bytes());
        }
        pad4(&mut t);
        let oa = t.len();
        for _ in 0..3 {
            t.extend(para_gamma(if a2b { a_gamma } else { 1.0 }));
        }
        for (k, o) in [ob, om, oc, oclut, oa].into_iter().enumerate() {
            t[offsets_at + k * 4..offsets_at + k * 4 + 4].copy_from_slice(&(o as u32).to_be_bytes());
        }
        t
    };
    let table = |vals: &[f64], a2b: bool| match kind {
        LutKind::Lut8 => mft(vals, false, false),
        LutKind::Lut16 => mft(vals, true, !a2b),
        LutKind::AToB => mab(vals, a2b),
    };
    let dsc = {
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend((desc.len() as u32 + 1).to_be_bytes());
        t.extend(desc.as_bytes());
        t.push(0);
        t.extend([0u8; 12 + 67]);
        pad4(&mut t);
        t
    };
    let mut tags: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"desc", dsc), (b"A2B0", table(&a2b_vals, true))];
    if from_xyz.is_some() {
        tags.push((b"B2A0", table(&b2a_vals, false)));
    }
    let mut tab = vec![];
    let mut data = vec![];
    let base = 128 + 4 + tags.len() * 12;
    for (sig, t) in &tags {
        tab.extend(*sig);
        tab.extend(((base + data.len()) as u32).to_be_bytes());
        tab.extend((t.len() as u32).to_be_bytes());
        data.extend(t);
        pad4(&mut data);
    }
    let total = base + data.len();
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    h[8] = if kind == LutKind::AToB { 4 } else { 2 };
    h[12..16].copy_from_slice(b"mntr");
    h[16..20].copy_from_slice(b"RGB ");
    h[20..24].copy_from_slice(if pcs == Pcs::Lab { b"Lab " } else { b"XYZ " });
    h[36..40].copy_from_slice(b"acsp");
    for (k, v) in D50_XYZ.into_iter().enumerate() {
        h[68 + k * 4..72 + k * 4].copy_from_slice(&s15(v));
    }
    let mut out = h;
    out.extend((tags.len() as u32).to_be_bytes());
    out.extend(tab);
    out.extend(data);
    out
}

/// A baked 3D lookup table: `size`³ RGB points over the unit cube (red varying slowest),
/// evaluated trilinearly.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
}

impl Lut3d {
    /// Sample `f` on a `size`³ grid.
    pub fn bake(size: usize, f: impl Fn([f64; 3]) -> [f64; 3]) -> Lut3d {
        let size = size.max(2);
        let pt = |k: usize| k as f64 / (size - 1) as f64;
        let data = (0..size * size * size)
            .map(|i| {
                let (r, g, b) = (i / (size * size), i / size % size, i % size);
                f([pt(r), pt(g), pt(b)]).map(|v| v as f32)
            })
            .collect();
        Lut3d { size, data }
    }

    pub fn eval(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let mut k = [0usize; 3];
        let mut f = [0f32; 3];
        for i in 0..3 {
            let x = c[i].clamp(0.0, 1.0) * (n - 1) as f32;
            k[i] = (x.floor() as usize).min(n - 2);
            f[i] = x - k[i] as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[(r * n + g) * n + b];
        let mut out = [0f32; 3];
        for corner in 0..8 {
            let (dr, dg, db) = (corner >> 2 & 1, corner >> 1 & 1, corner & 1);
            let w = (if dr == 1 { f[0] } else { 1.0 - f[0] }) * (if dg == 1 { f[1] } else { 1.0 - f[1] }) * (if db == 1 { f[2] } else { 1.0 - f[2] });
            if w == 0.0 {
                continue;
            }
            let v = at(k[0] + dr, k[1] + dg, k[2] + db);
            for i in 0..3 {
                out[i] += w * v[i];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P709: [[f64; 2]; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
    const D65: [f64; 2] = [0.3127, 0.3290];

    #[test]
    fn matrix_profile_round_trips() {
        let b = write_matrix_profile("Test RGB", P709, D65, 2.2);
        let p = parse(&b).unwrap();
        assert_eq!(p.description, "Test RGB");
        assert_eq!(p.version, 2);
        for (a, e) in p.primaries.iter().zip(P709) {
            assert!((a[0] - e[0]).abs() < 2e-4 && (a[1] - e[1]).abs() < 2e-4, "{a:?} vs {e:?}");
        }
        assert!((p.white[0] - D65[0]).abs() < 2e-4 && (p.white[1] - D65[1]).abs() < 2e-4, "{:?}", p.white);
        assert!((p.trc[0].fit_gamma() - 2.2).abs() < 0.01);
        assert!(!p.trc[0].is_srgb());
    }

    #[test]
    fn curves() {
        let srgb = Trc::Parametric(3, [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045, 0.0, 0.0]);
        assert!(srgb.is_srgb());
        assert!((srgb.fit_gamma() - 2.2).abs() < 0.15, "{}", srgb.fit_gamma());
        let table = Trc::Table((0..256).map(|i| ((i as f64 / 255.0).powf(1.8) * 65535.0).round() as u16).collect());
        assert!((table.fit_gamma() - 1.8).abs() < 0.02);
        assert!((Trc::Gamma(2.0).eval(0.5) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"hello").is_err());
        let mut b = write_matrix_profile("x", P709, D65, 2.2);
        b[16..20].copy_from_slice(b"CMYK");
        assert!(parse(&b).unwrap_err().contains("RGB"));
    }

    /// A display: P3 primaries, D65 white, gamma 2.2 → XYZ D50 (and back).
    fn device() -> (impl Fn([f64; 3]) -> [f64; 3], impl Fn([f64; 3]) -> [f64; 3]) {
        let p3 = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
        let m = crate::space::mul(&bradford(D65, D50), &crate::space::rgb_to_xyz(p3, D65));
        let mi = invert(&m);
        (move |c: [f64; 3]| mul_vec(&m, c.map(|v| v.powf(2.2))), move |x: [f64; 3]| mul_vec(&mi, x).map(|v| v.clamp(0.0, 1.0).powf(1.0 / 2.2)))
    }

    fn lab_de(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (a, b) = (xyz_to_lab(a), xyz_to_lab(b));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn lut_profiles_evaluate_their_a2b0() {
        let (to, from) = device();
        for (kind, grid, tol) in [(LutKind::Lut16, 17, 2.5), (LutKind::Lut8, 17, 3.0), (LutKind::AToB, 17, 1.5)] {
            let b = write_lut_profile("LUT Monitor", kind, grid, &to, Some(&from));
            let Profile::Lut(p) = parse_profile(&b).unwrap() else { panic!("{kind:?}: not a LUT profile") };
            assert_eq!(p.description, "LUT Monitor");
            assert!(p.b2a.is_some());
            let mut worst = 0.0f64;
            for k in 0..125 {
                let c = [(k / 25) as f64 / 4.0, (k / 5 % 5) as f64 / 4.0, (k % 5) as f64 / 4.0];
                worst = worst.max(lab_de(p.to_xyz(c), to(c)));
                // PCS → device through B2A0 lands back on mid-gamut colours (a coarse grid
                // over linear XYZ can't follow the gamma curve: checked at its nodes below).
                if p.pcs == Pcs::Lab && c.iter().all(|v| (0.25..=0.75).contains(v)) {
                    let back = p.from_xyz(to(c), [0.5; 3]);
                    assert!(lab_de(to(back), to(c)) < 6.0, "{kind:?} {c:?} → {back:?}");
                }
            }
            assert!(worst < tol, "{kind:?}: ΔE {worst}");
            // At B2A0's grid nodes the table gives what was sampled.
            let b2a = p.b2a.as_ref().unwrap();
            let q = match kind {
                LutKind::Lut8 => 2.0 / 255.0,
                LutKind::Lut16 => 3e-3,
                LutKind::AToB => 1e-3,
            };
            for node in [[0.5, 0.5, 0.5], [0.25, 0.5, 0.75], [1.0, 0.0, 0.5]] {
                // (lut16: through its cube-root input curves)
                let node = if kind == LutKind::Lut16 { node.map(|v: f64| v * v * v) } else { node };
                let want = from(pcs_to_xyz(p.pcs, b2a.legacy_lab, &node));
                let got = b2a.eval(&node);
                assert!((0..3).all(|i| (got[i] - want[i]).abs() < q), "{kind:?} {node:?}: {got:?} vs {want:?}");
            }
            // The matrix reader rejects it; the approximation finds the primaries.
            assert!(parse(&b).is_err());
            let (prim, _, gamma) = p.approximate();
            let red = xy(to([1.0, 0.0, 0.0]));
            assert!((prim[0][0] - red[0]).abs() < 0.01 && (prim[0][1] - red[1]).abs() < 0.01, "{kind:?} {prim:?}");
            assert!((gamma - 2.2).abs() < 0.1, "{kind:?} gamma {gamma}");
        }
    }

    #[test]
    fn a2b0_without_b2a0_is_inverted() {
        let (to, _) = device();
        let b = write_lut_profile("A2B only", LutKind::Lut16, 17, &to, None);
        let Profile::Lut(p) = parse_profile(&b).unwrap() else { panic!() };
        assert!(p.b2a.is_none());
        for c in [[0.2, 0.5, 0.8], [0.9, 0.1, 0.3], [0.5, 0.5, 0.5], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]] {
            let x = to(c);
            let back = p.from_xyz(x, [0.5; 3]);
            assert!(lab_de(p.to_xyz(back), x) < 0.5, "{c:?} → {back:?}");
        }
        // A matrix/TRC profile still parses as one.
        assert!(matches!(parse_profile(&write_matrix_profile("m", P709, D65, 2.2)).unwrap(), Profile::Matrix(_)));
    }

    #[test]
    fn clut_corners_count_only_interpolating_axes() {
        // 64 singleton axes and one interpolating one: two corners, not 2^65.
        let mut grid = vec![1; 64];
        grid.push(2);
        let c = Clut { grid, outputs: 1, data: vec![0.0, 1.0] };
        let mut x = vec![0.5; 64];
        x.push(0.25);
        assert!((c.eval(&x)[0] - 0.25).abs() < 1e-9);
        // On grid points nothing interpolates.
        let c = Clut { grid: vec![2; 40], outputs: 1, data: vec![] };
        assert_eq!(c.eval(&[1.0; 40]), vec![0.0]);
        // More interpolating axes than ICC allows: zero, at once.
        assert_eq!(c.eval(&[0.5; 40]), vec![0.0]);
        // The last grid point of every axis is that table entry, however many axes there are.
        let mut data = vec![0.0; 1 << 17];
        data[(1 << 17) - 1] = 0.75;
        let c = Clut { grid: vec![2; 17], outputs: 1, data };
        assert!((c.eval(&[1.0; 17])[0] - 0.75).abs() < 1e-9);
        // Strides that overflow usize (64 two-point axes) read nothing instead of panicking.
        let c = Clut { grid: vec![2; 64], outputs: 1, data: vec![] };
        assert_eq!(c.eval(&[1.0; 64]), vec![0.0]);
        assert_eq!(c.eval(&[0.5; 64]), vec![0.0]);
    }

    #[test]
    fn clut_and_lut3d_interpolate() {
        // A 2-point identity CLUT on 3 inputs is exact for every point.
        let mut data = vec![];
        for r in 0..2 {
            for g in 0..2 {
                for b in 0..2 {
                    data.extend([r as f32, g as f32, b as f32]);
                }
            }
        }
        let c = Clut { grid: vec![2; 3], outputs: 3, data };
        let v = c.eval(&[0.25, 0.5, 0.75]);
        assert!((v[0] - 0.25).abs() < 1e-6 && (v[1] - 0.5).abs() < 1e-6 && (v[2] - 0.75).abs() < 1e-6);
        let l = Lut3d::bake(5, |c| [c[0] * c[0], c[1], 1.0 - c[2]]);
        let o = l.eval([0.5, 0.3, 0.2]);
        assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.3).abs() < 1e-6 && (o[2] - 0.8).abs() < 1e-6, "{o:?}");
        assert!(parse_lut(b"xxxx\0\0\0\0", false).is_err());
    }
}
