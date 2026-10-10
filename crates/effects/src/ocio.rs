//! OCIO-style colour transforms (Color Correction ▸ OCIO CDL / Color Space / Display / File / Look
//! Transform), Color Profile Converter (Utility) and Color Stabilizer (Color Correction).
//!
//! The OCIO effects run on a **built-in minimal configuration** written from published
//! standards, not from OpenColorIO's source:
//!
//! * colour spaces are defined by primaries, white point and transfer function: ACES2065-1 (AP0)
//!   and ACEScg / ACEScct (AP1) from SMPTE ST 2065-1 and Academy S-2014-004 / S-2016-001, sRGB
//!   (IEC 61966-2-1), Rec. 709 / Rec. 2020 with the BT.1886 gamma 2.4 EOTF, Display P3 (SMPTE
//!   EG 432-1 primaries, D65, sRGB curve), their linear variants and CIE XYZ D65;
//! * the reference space is CIE XYZ D65; spaces with another white point (the ACES white,
//!   x 0.32168 y 0.33767) are adapted with the Bradford chromatic adaptation transform, as in the
//!   published ACES configurations;
//! * displays (sRGB, Rec.1886 Rec.709, Display P3, Rec.2100-PQ via SMPTE ST 2084) and views
//!   (Standard = clip-free encode, Tone Mapped = extended Reinhard roll-off, Raw);
//! * looks are ASC CDLs applied in ACEScct;
//! * CDL maths follows the ASC CDL v1.2 specification (slope, offset, power, then saturation with
//!   Rec. 709 luma weights); the "No Clamp" style mirrors power around zero;
//! * File Transform reads `.cube` (Adobe/Resolve), `.3dl` (Autodesk Lustre/Flame integer
//!   lattices), `.csp` (cineSpace, with its per-channel pre-LUT shaper), Sony Imageworks
//!   `.spi1d` / `.spi3d` / `.spimtx` and ASC `.cc` / `.ccc` / `.cdl` XML, with nearest, trilinear
//!   or tetrahedral interpolation and an inverse direction (exact for 1D/CDL/matrices, iterative
//!   for 3D lattices);
//! * Configuration ▸ Custom reads a `.ocio` file instead ([`crate::ocio_config`]); what it can't
//!   apply is reported by [`warning`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_color::ColorSpace;
use effectcraft_color::space::{invert, mul, mul_vec, rgb_to_xyz};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::unpremul;
use crate::utility::{Lut, parse_cube};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

type M3 = [[f64; 3]; 3];

pub(crate) const D65: [f64; 2] = [0.3127, 0.3290];
const ACES_WHITE: [f64; 2] = [0.32168, 0.33767];
const AP0: [[f64; 2]; 3] = [[0.7347, 0.2653], [0.0, 1.0], [0.0001, -0.0770]];
const AP1: [[f64; 2]; 3] = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];
pub(crate) const REC709: [[f64; 2]; 3] = [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]];
pub(crate) const REC2020: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
pub(crate) const P3: [[f64; 2]; 3] = [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]];
const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

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
const ACES_CCT: Space = sp("ACEScct", AP1, ACES_WHITE, Tf::AcesCct);

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
    dec: Tf,
    m: [[f32; 3]; 3],
    ident: bool,
    enc: Tf,
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

// ---------------------------------------------------------------- ASC CDL

/// An ASC Colour Decision List correction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cdl {
    pub slope: [f64; 3],
    pub offset: [f64; 3],
    pub power: [f64; 3],
    pub sat: f64,
}

impl Default for Cdl {
    fn default() -> Self {
        Cdl { slope: [1.0; 3], offset: [0.0; 3], power: [1.0; 3], sat: 1.0 }
    }
}

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

impl Cdl {
    /// Forward CDL. `clamp` = the v1.2 style (results clamped to 0..1 after SOP and saturation).
    pub fn apply(&self, c: [f64; 3], clamp: bool) -> [f64; 3] {
        let mut o = [0.0; 3];
        for i in 0..3 {
            let v = c[i] * self.slope[i] + self.offset[i];
            o[i] = if clamp {
                v.clamp(0.0, 1.0).powf(self.power[i])
            } else if v >= 0.0 {
                v.powf(self.power[i])
            } else {
                v
            };
        }
        let l = o[0] * LUMA[0] + o[1] * LUMA[1] + o[2] * LUMA[2];
        let o = o.map(|v| l + self.sat * (v - l));
        if clamp { o.map(|v| v.clamp(0.0, 1.0)) } else { o }
    }
    /// Exact inverse of [`Cdl::apply`] (inside the clamped range).
    pub fn invert(&self, c: [f64; 3], clamp: bool) -> [f64; 3] {
        let c = if clamp { c.map(|v| v.clamp(0.0, 1.0)) } else { c };
        let l = c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
        let s = if self.sat.abs() < 1e-9 { 1e-9 } else { self.sat };
        let o = c.map(|v| l + (v - l) / s);
        let mut r = [0.0; 3];
        for i in 0..3 {
            let v = if clamp { o[i].clamp(0.0, 1.0) } else { o[i] };
            let pw = self.power[i].max(1e-9);
            let u = if v >= 0.0 { v.powf(1.0 / pw) } else { v };
            let sl = if self.slope[i].abs() < 1e-9 { 1e-9 } else { self.slope[i] };
            r[i] = (u - self.offset[i]) / sl;
        }
        if clamp { r.map(|v| v.clamp(0.0, 1.0)) } else { r }
    }
}

fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = xml.find(&format!("<{name}"))?;
    let gt = open + xml[open..].find('>')?;
    if xml[..gt].ends_with('/') {
        return Some("");
    }
    let close = xml[gt..].find(&format!("</{name}>"))? + gt;
    Some(&xml[gt + 1..close])
}

fn nums3(s: &str) -> Option<[f64; 3]> {
    let v: Vec<f64> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    (v.len() >= 3).then(|| [v[0], v[1], v[2]])
}

fn parse_cc(block: &str) -> Cdl {
    let mut c = Cdl::default();
    if let Some(s) = tag(block, "Slope").and_then(nums3) {
        c.slope = s;
    }
    if let Some(s) = tag(block, "Offset").and_then(nums3) {
        c.offset = s;
    }
    if let Some(s) = tag(block, "Power").and_then(nums3) {
        c.power = s;
    }
    if let Some(s) = tag(block, "Saturation").and_then(|t| t.trim().parse().ok()) {
        c.sat = s;
    }
    c
}

/// Parse ASC CDL XML (`.cc`, `.ccc`, `.cdl`): every `<ColorCorrection>` with its `id`.
pub fn parse_cdl_xml(xml: &str) -> Vec<(String, Cdl)> {
    let mut out = vec![];
    let mut rest = xml;
    while let Some(i) = rest.find("<ColorCorrection") {
        let after = &rest[i..];
        let head_end = after.find('>').unwrap_or(after.len());
        let head = &after[..head_end];
        let id = head
            .find("id=")
            .and_then(|k| {
                let q = head[k + 3..].chars().next()?;
                let s = &head[k + 4..];
                s.find(q).map(|e| s[..e].to_string())
            })
            .unwrap_or_default();
        let end = after.find("</ColorCorrection>").map(|e| e + "</ColorCorrection>".len()).unwrap_or(after.len());
        out.push((id, parse_cc(&after[..end])));
        rest = &after[end..];
    }
    out
}

// ---------------------------------------------------------------- LUT files

/// A loaded transform file.
#[derive(Clone, Debug)]
pub enum FileXform {
    /// A lattice/1D LUT with an optional per-channel piecewise-linear shaper (cineSpace pre-LUT).
    Lut { lut: Lut, shaper: Option<[Vec<(f32, f32)>; 3]> },
    /// CDL corrections by id (in file order).
    Cdl(Vec<(String, Cdl)>),
    /// A matrix with offset (Sony Imageworks `.spimtx`), as a MatrixTransform's.
    Matrix { m: [f64; 16], offset: [f64; 4] },
}

/// Parse a Sony Imageworks `.spimtx` matrix: three rows of four numbers, three matrix columns
/// and an offset in 16-bit code values (÷ 65535).
pub fn parse_spimtx(text: &str) -> Option<FileXform> {
    let v: Vec<f64> = text.split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let mut m = [0.0; 16];
    let mut offset = [0.0; 4];
    for (r, row) in v.as_chunks::<4>().0.iter().enumerate() {
        m[r * 4..r * 4 + 3].copy_from_slice(&row[..3]);
        offset[r] = row[3] / 65535.0;
    }
    m[15] = 1.0;
    Some(FileXform::Matrix { m, offset })
}

/// Parse a Sony Imageworks `.spi1d` 1D LUT: `From <min> <max>`, `Length <n>`, `Components
/// <1 | 3>`, then `{` the values `}`.
pub fn parse_spi1d(text: &str) -> Option<Lut> {
    let (mut from, mut len, mut comps) = ([0.0f32, 1.0], 0usize, 1usize);
    let mut values: Vec<f32> = vec![];
    let mut body = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if body {
            if line.starts_with('}') {
                break;
            }
            for t in line.split_whitespace() {
                values.push(t.parse().ok()?);
            }
            continue;
        }
        let mut t = line.split_whitespace();
        match t.next()? {
            "{" => body = true,
            "From" => from = [t.next()?.parse().ok()?, t.next()?.parse().ok()?],
            "Length" => len = t.next()?.parse().ok()?,
            "Components" => comps = t.next()?.parse().ok()?,
            _ => {}
        }
    }
    // A 1D table is interpolated by index, so cap it like the lattices (2²⁴ entries).
    if !(2..=1 << 24).contains(&len)
        || !matches!(comps, 1 | 3)
        || values.len() != len.checked_mul(comps)?
        || from[1] <= from[0]
        || from.iter().any(|v| !v.is_finite())
    {
        return None;
    }
    let d1 = values.chunks_exact(comps).map(|c| if comps == 3 { [c[0], c[1], c[2]] } else { [c[0]; 3] }).collect();
    Some(Lut { n1: len, d1, n3: 0, d3: vec![], dmin: [from[0]; 3], dmax: [from[1]; 3] })
}

/// Parse a Sony Imageworks `.spi3d` lattice: `SPILUT 1.0`, `3 3`, the sizes, then one
/// `r g b  R G B` line per lattice point (indices, then the colour).
pub fn parse_spi3d(text: &str) -> Option<Lut> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next()?.starts_with("SPILUT") {
        return None;
    }
    lines.next()?;
    let size: Vec<usize> = lines.next()?.split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let n = *size.first()?;
    if size.len() != 3 || size.iter().any(|s| *s != n) || !(2..=129).contains(&n) {
        return None;
    }
    let mut d3 = vec![[0.0f32; 3]; n * n * n];
    let mut seen = 0usize;
    for line in lines {
        let v: Vec<&str> = line.split_whitespace().collect();
        let [r, g, b, cr, cg, cb] = v.as_slice() else { return None };
        let (r, g, b): (usize, usize, usize) = (r.parse().ok()?, g.parse().ok()?, b.parse().ok()?);
        if r >= n || g >= n || b >= n {
            return None;
        }
        // Our lattice is red fastest.
        *d3.get_mut(r + g * n + b * n * n)? = [cr.parse().ok()?, cg.parse().ok()?, cb.parse().ok()?];
        seen += 1;
    }
    (seen == n * n * n).then_some(Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] })
}

/// Parse an Autodesk `.3dl` lattice (integers, blue fastest; an optional input-mesh line).
pub fn parse_3dl(text: &str) -> Option<Lut> {
    let mut mesh: Option<Vec<f64>> = None;
    let mut rows: Vec<[f64; 3]> = vec![];
    let mut out_bits: Option<u32> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with('<') {
            continue;
        }
        if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
            // "Mesh <in bits> <out bits>" (Lustre header); other keywords are ignored.
            let t: Vec<&str> = l.split_whitespace().collect();
            if t[0].eq_ignore_ascii_case("mesh") && t.len() >= 3 {
                out_bits = t[2].parse().ok();
            }
            continue;
        }
        let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if v.len() > 3 && mesh.is_none() && rows.is_empty() {
            mesh = Some(v);
        } else if v.len() == 3 {
            rows.push([v[0], v[1], v[2]]);
        } else {
            return None;
        }
    }
    let is_cube = |k: usize| {
        let n = (k as f64).cbrt().round() as usize;
        n * n * n == k
    };
    // A 3-entry input mesh (3³ lattice) looks like a data row: tell them apart by the count.
    if mesh.is_none() && !is_cube(rows.len()) && rows.len() == 28 {
        mesh = Some(rows.remove(0).to_vec());
    }
    let n = match &mesh {
        Some(m) => m.len(),
        None => (rows.len() as f64).cbrt().round() as usize,
    };
    if n < 2 || rows.len() != n * n * n {
        return None;
    }
    let maxv = rows.iter().flat_map(|r| r.iter()).fold(0.0f64, |m, &v| m.max(v));
    let out_max = match out_bits {
        Some(b) if b > 0 && b < 32 => (1u64 << b) as f64 - 1.0,
        _ => [255.0, 1023.0, 4095.0, 65535.0].into_iter().find(|&m| maxv <= m).unwrap_or(maxv.max(1.0)),
    };
    // .3dl rows: red slowest, blue fastest. Our lattice is red fastest.
    let mut d3 = vec![[0.0f32; 3]; n * n * n];
    for (k, r) in rows.iter().enumerate() {
        let (ri, gi, bi) = (k / (n * n), (k / n) % n, k % n);
        d3[ri + gi * n + bi * n * n] = r.map(|v| (v / out_max) as f32);
    }
    Some(Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] })
}

/// Parse a cineSpace `.csp` LUT (1D or 3D, with its three pre-LUT shapers).
pub fn parse_csp(text: &str) -> Option<(Lut, [Vec<(f32, f32)>; 3])> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next()?.starts_with("CSPLUTV100") {
        return None;
    }
    let kind = lines.next()?;
    let mut lines = lines.peekable();
    if lines.peek().is_some_and(|l| l.starts_with("BEGIN METADATA")) {
        for l in lines.by_ref() {
            if l.starts_with("END METADATA") {
                break;
            }
        }
    }
    let nums = |l: &str| -> Option<Vec<f32>> { l.split_whitespace().map(|t| t.parse().ok()).collect() };
    let mut shaper: [Vec<(f32, f32)>; 3] = Default::default();
    for ch in shaper.iter_mut() {
        let n: usize = lines.next()?.parse().ok()?;
        let ins = nums(lines.next()?)?;
        let outs = nums(lines.next()?)?;
        if ins.len() != n || outs.len() != n || n < 2 {
            return None;
        }
        *ch = ins.into_iter().zip(outs).collect();
    }
    if kind.starts_with("3D") {
        let sz = nums(lines.next()?)?;
        if sz.len() != 3 || sz[0] != sz[1] || sz[1] != sz[2] {
            return None;
        }
        let n = sz[0] as usize;
        let mut d3 = Vec::with_capacity(n * n * n);
        for _ in 0..n * n * n {
            let v = nums(lines.next()?)?;
            if v.len() != 3 {
                return None;
            }
            d3.push([v[0], v[1], v[2]]);
        }
        (n >= 2).then_some((Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] }, shaper))
    } else {
        let n: usize = lines.next()?.parse().ok()?;
        let mut d1 = Vec::with_capacity(n);
        for _ in 0..n {
            let v = nums(lines.next()?)?;
            if v.len() != 3 {
                return None;
            }
            d1.push([v[0], v[1], v[2]]);
        }
        (n >= 2).then_some((Lut { n1: n, d1, n3: 0, d3: vec![], dmin: [0.0; 3], dmax: [1.0; 3] }, shaper))
    }
}

fn shape(curve: &[(f32, f32)], x: f32) -> f32 {
    if curve.len() < 2 {
        return x;
    }
    let i = curve.partition_point(|p| p.0 < x).clamp(1, curve.len() - 1);
    let (a, b) = (curve[i - 1], curve[i]);
    let t = ((x - a.0) / (b.0 - a.0).max(1e-12)).clamp(0.0, 1.0);
    a.1 + (b.1 - a.1) * t
}

fn shape_inv(curve: &[(f32, f32)], y: f32) -> f32 {
    let flipped: Vec<(f32, f32)> = curve.iter().map(|&(a, b)| (b, a)).collect();
    shape(&flipped, y)
}

/// Parse transform text by its format (detected from `ext` or the content).
pub fn parse_file(text: &str, ext: &str) -> Option<FileXform> {
    let ext = ext.to_ascii_lowercase();
    if ext == "cc" || ext == "ccc" || ext == "cdl" || text.contains("<ColorCorrection") {
        let v = parse_cdl_xml(text);
        return (!v.is_empty()).then_some(FileXform::Cdl(v));
    }
    if ext == "csp" || text.trim_start().starts_with("CSPLUTV100") {
        return parse_csp(text).map(|(lut, s)| FileXform::Lut { lut, shaper: Some(s) });
    }
    if ext == "spimtx" {
        return parse_spimtx(text);
    }
    if ext == "spi1d" || text.trim_start().starts_with("Version") {
        return parse_spi1d(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    if ext == "spi3d" || text.trim_start().starts_with("SPILUT") {
        return parse_spi3d(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    if ext == "cube" || text.contains("LUT_3D_SIZE") || text.contains("LUT_1D_SIZE") {
        return parse_cube(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    parse_3dl(text).map(|lut| FileXform::Lut { lut, shaper: None })
}

type FileCache = Mutex<HashMap<String, Option<Arc<FileXform>>>>;

/// Load (and cache) a transform from a file path or inline content.
pub fn load_file(src: &str) -> Option<Arc<FileXform>> {
    let src_t = src.trim();
    if src_t.is_empty() {
        return None;
    }
    static CACHE: OnceLock<FileCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock()
        && let Some(v) = m.get(src)
    {
        return v.clone();
    }
    let inline = src.contains('\n');
    let parsed = if inline {
        parse_file(src, "")
    } else {
        let ext = std::path::Path::new(src_t).extension().and_then(|e| e.to_str()).unwrap_or("");
        std::fs::read_to_string(src_t).ok().and_then(|t| parse_file(&t, ext))
    }
    .map(Arc::new);
    if let Ok(mut m) = cache.lock() {
        m.insert(src.to_string(), parsed.clone());
    }
    parsed
}

impl FileXform {
    pub fn apply(&self, c: [f32; 3], interp: u32, inverse: bool, ccc_id: &str) -> [f32; 3] {
        match self {
            FileXform::Cdl(list) => {
                let cdl = list.iter().find(|(id, _)| !ccc_id.is_empty() && id == ccc_id).or(list.first()).map(|(_, c)| *c).unwrap_or_default();
                let c64 = c.map(|v| v as f64);
                let o = if inverse { cdl.invert(c64, true) } else { cdl.apply(c64, true) };
                o.map(|v| v as f32)
            }
            FileXform::Matrix { m, offset } => crate::ocio_config::matrix_apply(m, offset, c.map(|v| v as f64), inverse).map(|v| v as f32),
            FileXform::Lut { lut, shaper } => {
                let fwd = |c: [f32; 3]| {
                    let c = match shaper {
                        Some(s) => [shape(&s[0], c[0]), shape(&s[1], c[1]), shape(&s[2], c[2])],
                        None => c,
                    };
                    lut.apply_interp(c, interp)
                };
                if !inverse {
                    return fwd(c);
                }
                if lut.n3 == 0 {
                    // 1D: invert each channel exactly through the (monotone) tables.
                    let mut x = c;
                    if lut.n1 > 1 {
                        x = [0, 1, 2].map(|i| {
                            let curve: Vec<(f32, f32)> =
                                (0..lut.n1).map(|k| (lut.d1[k][i], lut.dmin[i] + (lut.dmax[i] - lut.dmin[i]) * k as f32 / (lut.n1 - 1) as f32)).collect();
                            shape(&curve, c[i])
                        });
                    }
                    if let Some(s) = shaper {
                        x = [shape_inv(&s[0], x[0]), shape_inv(&s[1], x[1]), shape_inv(&s[2], x[2])];
                    }
                    return x;
                }
                // 3D: damped fixed-point (Newton with an identity Jacobian) from the target.
                let mut x = c;
                for _ in 0..40 {
                    let y = fwd(x);
                    let e = [c[0] - y[0], c[1] - y[1], c[2] - y[2]];
                    if e.iter().all(|v| v.abs() < 1e-6) {
                        break;
                    }
                    x = [x[0] + e[0], x[1] + e[1], x[2] + e[2]];
                }
                x
            }
        }
    }
}

// ---------------------------------------------------------------- displays, views and looks

/// Displays of the built-in config: (name, display primaries/white, encoding).
pub const DISPLAYS: &[(&str, [[f64; 2]; 3], Tf)] =
    &[("sRGB", REC709, Tf::Srgb), ("Rec.1886 Rec.709", REC709, Tf::Gamma(2.4)), ("Display P3", P3, Tf::Srgb), ("Rec.2100-PQ", REC2020, Tf::Pq)];

pub const VIEWS: &[&str] = &["Standard", "Tone Mapped", "Raw"];

/// Configuration popup: the built-in config, or a custom `.ocio` file (Config File: a path or
/// the config's text; spaces chosen by name, see [`crate::ocio_config`]).
pub const CONFIGS: &[&str] = &["Built-in (EffectCraft minimal)", "Custom (.ocio file)"];

/// Built-in looks: CDLs applied in ACEScct.
pub fn looks() -> Vec<(&'static str, Cdl)> {
    let grey = 0.413_588_402_492_442_1; // ACEScct of 0.18
    let c = |slope: [f64; 3], offset: [f64; 3], sat: f64| Cdl { slope, offset, power: [1.0; 3], sat };
    vec![
        ("Warm", c([1.04, 1.0, 0.94], [0.0; 3], 1.0)),
        ("Cool", c([0.95, 1.0, 1.06], [0.0; 3], 1.0)),
        ("High Contrast", c([1.25; 3], [grey * (1.0 - 1.25); 3], 1.0)),
        ("Low Contrast", c([0.8; 3], [grey * (1.0 - 0.8); 3], 1.0)),
        ("Desaturated", c([1.0; 3], [0.0; 3], 0.6)),
        ("Vivid", c([1.0; 3], [0.0; 3], 1.3)),
    ]
}

/// Extended Reinhard roll-off (white point 16): maps 0..∞ into 0..1, ≈ identity near 0.
fn tonemap(v: f64) -> f64 {
    let w2 = 256.0;
    if v <= 0.0 { v } else { v * (1.0 + v / w2) / (1.0 + v) }
}

fn tonemap_inv(y: f64) -> f64 {
    if y <= 0.0 {
        return y;
    }
    // Solve v² / w² + v (1 − y) − y = 0.
    let (a, b, c) = (1.0 / 256.0, 1.0 - y, -y);
    (-b + (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
}

/// Display transform of an input-space colour (`inverse`: display code values → input space).
pub fn display_xform(input: &Space, display: usize, view: usize, inverse: bool) -> impl Fn([f32; 3]) -> [f32; 3] + Sync {
    let (_, prims, tf) = DISPLAYS[display.min(DISPLAYS.len() - 1)];
    let disp = Space { name: "display", prims: Some((prims, D65)), tf: Tf::Linear, raw: false };
    let raw = view == 2 || input.raw;
    let m_in = Xform::new(&Space { tf: Tf::Linear, ..*input }, &disp);
    let m_out = Xform::new(&disp, &Space { tf: Tf::Linear, ..*input });
    let in_tf = input.tf;
    move |c: [f32; 3]| {
        if raw {
            return c;
        }
        if !inverse {
            let lin = c.map(|v| in_tf.decode(v as f64) as f32);
            let d = m_in.apply(lin);
            d.map(|v| {
                let v = v as f64;
                let v = if view == 1 { tonemap(v) } else { v };
                tf.encode(v) as f32
            })
        } else {
            let d = c.map(|v| {
                let l = tf.decode(v as f64);
                (if view == 1 { tonemap_inv(l) } else { l }) as f32
            });
            m_out.apply(d).map(|v| in_tf.encode(v as f64) as f32)
        }
    }
}

// ---------------------------------------------------------------- effects

fn spec(id: &'static str, name: &'static str, cat: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: cat, params, render, gpu: false, float: true }
}

fn space_param(id: &'static str, name: &'static str, default: u32) -> crate::ParamSpec {
    p(id, name, Value::Enum(default), popup(&space_names()))
}

fn direction() -> crate::ParamSpec {
    p("direction", "Direction", Value::Enum(0), popup(&["Forward", "Inverse"]))
}

fn map_px(b: &mut Buf, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        let o = f(c);
        *px = [o[0] * a, o[1] * a, o[2] * a, a];
    });
}

fn space_at(i: u32) -> &'static Space {
    &SPACES[(i as usize).min(SPACES.len() - 1)]
}

/// The effects that take Configuration ▸ Custom (a `.ocio` file).
pub(crate) const CUSTOM_CONFIG_FX: [&str; 2] = ["ec.color.ociocolorspace", "ec.color.ociodisplay"];

/// Effect `id`'s transform under Configuration ▸ Custom: the config's conversion from the source
/// space to the destination space (Color Space Transform) or through a display's view (Display
/// Transform), see [`crate::ocio_config`]. `Ok(None)` with the built-in configuration; an error
/// (the pixels pass through) when the config can't be read or has no such colour space.
fn custom_xf(id: &str, pr: &crate::Params) -> Result<Option<crate::ocio_config::Xf>, String> {
    use crate::ocio_config::Xf;
    if !CUSTOM_CONFIG_FX.contains(&id) || pr.e("config") != 1 {
        return Ok(None);
    }
    let cfg = crate::ocio_config::load_config(pr.s("configFile")).map_err(|e| format!("Can't read the OCIO config: {e}"))?;
    // A space by its typed name, else by the built-in popup's name.
    let space = |name: &str, popup: u32| {
        let name = if name.trim().is_empty() { space_at(popup).name } else { name.trim() };
        cfg.space(name).ok_or_else(|| format!("The OCIO config has no color space `{name}`"))
    };
    let a = space(pr.s("sourceName"), pr.e("source"))?;
    let inverse = pr.e("direction") == 1;
    if id == "ec.color.ociodisplay" {
        let (dn, vn) = display_view_names(pr);
        let xf = cfg.display_path(a, dn, vn);
        return Ok(Some(if inverse { Xf::Inverse(Box::new(xf)) } else { xf }));
    }
    let z = space(pr.s("destinationName"), pr.e("destination"))?;
    // Inverse: the conversion the other way (OCIO's inverse ColorSpaceTransform).
    Ok(Some(if inverse { cfg.path(z, a) } else { cfg.path(a, z) }))
}

/// The display and view a Display Transform names: the typed names, else the popups'.
fn display_view_names(pr: &crate::Params) -> (&str, &str) {
    let display = DISPLAYS.get(pr.e("display") as usize).map_or("", |d| d.0);
    let view = VIEWS.get(pr.e("view") as usize).copied().unwrap_or("");
    let typed = |k: &str, popup| if pr.s(k).trim().is_empty() { popup } else { pr.s(k).trim() };
    (typed("displayName", display), typed("viewName", view))
}

/// Effect Controls' warning for effect `id`: a custom OCIO configuration that can't be read, has
/// no such colour space, display or view, or uses transforms EffectCraft can't apply (colours
/// pass through those steps, #410). `None` when the effect renders as asked.
pub fn warning(id: &str, pr: &crate::Params) -> Option<String> {
    let xf = match custom_xf(id, pr) {
        Ok(x) => x?,
        Err(e) => return Some(format!("{e}: the colors pass through.")),
    };
    let mut notes = vec![];
    if id == "ec.color.ociodisplay"
        && let Ok(cfg) = crate::ocio_config::load_config(pr.s("configFile"))
    {
        let (dn, vn) = display_view_names(pr);
        if let Some((d, v)) = cfg.view(dn, vn)
            && (!d.eq_ignore_ascii_case(dn) || !v.name.eq_ignore_ascii_case(vn))
        {
            notes.push(format!("The OCIO config has no display `{dn}` with a view `{vn}`: showing {d} / {}.", v.name));
        }
    }
    let missing = xf.unsupported_list();
    if !missing.is_empty() {
        notes.push(format!("Not supported, the colors pass through: {}.", missing.join(", ")));
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// Configuration ▸ Custom: render through the config's transform. `false` with the built-in
/// configuration (the caller renders it).
fn custom_render(id: &str, ctx: &EffectCtx, b: &mut Buf) -> bool {
    match custom_xf(id, ctx.params) {
        Ok(None) => false,
        Ok(Some(xf)) => {
            map_px(b, |c| xf.apply(c.map(|v| v as f64), false).map(|v| v as f32));
            true
        }
        // A config that can't be read, or an unknown space: the pixels pass through.
        Err(_) => true,
    }
}

fn ocio_cst(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if custom_render("ec.color.ociocolorspace", ctx, &mut b) {
        return b;
    }
    let (a, z) = (space_at(ctx.params.e("source")), space_at(ctx.params.e("destination")));
    let (a, z) = if ctx.params.e("direction") == 1 { (z, a) } else { (a, z) };
    if a.name == z.name {
        return b;
    }
    let x = Xform::new(a, z);
    map_px(&mut b, |c| x.apply(c));
    b
}

fn cdl_from_params(pr: &crate::Params) -> Cdl {
    Cdl {
        slope: [pr.f("slopeRed"), pr.f("slopeGreen"), pr.f("slopeBlue")],
        offset: [pr.f("offsetRed"), pr.f("offsetGreen"), pr.f("offsetBlue")],
        power: [pr.f("powerRed").max(0.0), pr.f("powerGreen").max(0.0), pr.f("powerBlue").max(0.0)],
        sat: pr.f("saturation").max(0.0),
    }
}

fn ocio_cdl(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let cdl = cdl_from_params(ctx.params);
    if cdl == Cdl::default() {
        return b;
    }
    let clamp = ctx.params.e("style") == 0;
    let inv = ctx.params.e("direction") == 1;
    map_px(&mut b, |c| {
        let c = c.map(|v| v as f64);
        let o = if inv { cdl.invert(c, clamp) } else { cdl.apply(c, clamp) };
        o.map(|v| v as f32)
    });
    b
}

fn ocio_display(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if custom_render("ec.color.ociodisplay", ctx, &mut b) {
        return b;
    }
    let input = space_at(ctx.params.e("source"));
    let f = display_xform(input, ctx.params.e("display") as usize, ctx.params.e("view") as usize, ctx.params.e("direction") == 1);
    map_px(&mut b, f);
    b
}

fn ocio_file(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(x) = load_file(ctx.params.s("file")) else { return b };
    let interp = match ctx.params.e("interpolation") {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    let inv = ctx.params.e("direction") == 1;
    let id = ctx.params.s("cccId").to_string();
    map_px(&mut b, |c| x.apply(c, interp, inv, &id));
    b
}

/// Parse a looks string: comma/space separated names, `-Name` = inverse.
pub fn parse_looks(s: &str) -> Vec<(Cdl, bool)> {
    let all = looks();
    s.split([',', ':'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let (inv, n) = match t.strip_prefix('-') {
                Some(r) => (true, r.trim()),
                None => (false, t.trim_start_matches('+').trim()),
            };
            all.iter().find(|(name, _)| name.eq_ignore_ascii_case(n)).map(|(_, c)| (*c, inv))
        })
        .collect()
}

fn ocio_look(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let mut list = parse_looks(ctx.params.s("looks"));
    let pick = ctx.params.e("look");
    if pick > 0
        && let Some((_, c)) = looks().get(pick as usize - 1)
    {
        list.insert(0, (*c, false));
    }
    if list.is_empty() {
        return b;
    }
    let (src, dst) = (space_at(ctx.params.e("source")), space_at(ctx.params.e("destination")));
    let inv = ctx.params.e("direction") == 1;
    let cct = &ACES_CCT;
    let (a, z) = if inv { (dst, src) } else { (src, dst) };
    let to_p = Xform::new(a, cct);
    let from_p = Xform::new(cct, z);
    if inv {
        list.reverse();
    }
    map_px(&mut b, |c| {
        let mut v = to_p.apply(c).map(|x| x as f64);
        for (cdl, li) in &list {
            v = if *li != inv { cdl.invert(v, false) } else { cdl.apply(v, false) };
        }
        from_p.apply(v.map(|x| x as f32))
    });
    b
}

// ---- Color Profile Converter (Utility)

pub const PROFILES: &[&str] = &["Project Working Space", "sRGB IEC61966-2.1", "HDTV (Rec. 709)", "Rec. 2020", "Display P3", "ACEScg", "ACES2065-1"];

fn profile(ctx: &EffectCtx, i: u32) -> (Space, bool) {
    let ws = |cs: ColorSpace| match cs {
        ColorSpace::Srgb => sp("sRGB", REC709, D65, Tf::Srgb),
        ColorSpace::Rec709 => sp("Rec.709", REC709, D65, Tf::Gamma(2.4)),
        ColorSpace::Rec2020 => sp("Rec.2020", REC2020, D65, Tf::Gamma(2.4)),
        ColorSpace::DisplayP3 => sp("Display P3", P3, D65, Tf::Srgb),
        ColorSpace::AcesCg => ACES_CG,
        ColorSpace::Aces2065 => ACES2065_1,
        // HDR encodings are output spaces, never a working space.
        ColorSpace::Rec2100Pq | ColorSpace::Rec2100Hlg => sp("Rec.2020", REC2020, D65, Tf::Gamma(2.4)),
    };
    match i {
        0 => (ws(ctx.env.working_space.unwrap_or(ColorSpace::Srgb)), ctx.env.working_linear),
        1 => (ws(ColorSpace::Srgb), false),
        2 => (ws(ColorSpace::Rec709), false),
        3 => (ws(ColorSpace::Rec2020), false),
        4 => (ws(ColorSpace::DisplayP3), false),
        5 => (ACES_CG, true),
        _ => (ACES2065_1, true),
    }
}

/// Gamut compression for the perceptual intent: pull out-of-gamut (negative) colours toward
/// their luminance just enough to bring every channel to ≥ 0, rolling off smoothly.
fn compress_gamut(c: [f64; 3], luma: [f64; 3]) -> [f64; 3] {
    let l = c[0] * luma[0] + c[1] * luma[1] + c[2] * luma[2];
    if l <= 0.0 {
        return c.map(|v| v.max(0.0));
    }
    let mn = c.iter().cloned().fold(f64::INFINITY, f64::min);
    if mn >= 0.0 {
        return c;
    }
    // Fraction of the way toward grey that reaches zero on the most negative channel.
    let t = (l / (l - mn)).clamp(0.0, 1.0);
    c.map(|v| l + (v - l) * t)
}

fn profile_converter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (inp, in_lin_default) = profile(ctx, ctx.params.e("inputProfile"));
    let (out, out_lin_default) = profile(ctx, ctx.params.e("outputProfile"));
    let in_lin = ctx.params.b("linearizeInputProfile") || in_lin_default && ctx.params.e("inputProfile") == 0;
    let out_lin = ctx.params.b("linearizeOutputProfile") || out_lin_default && ctx.params.e("outputProfile") == 0;
    let intent = ctx.params.e("intent");
    let dec = if in_lin { Tf::Linear } else { inp.tf };
    let enc = if out_lin { Tf::Linear } else { out.tf };
    // Absolute colorimetric keeps the source white (no adaptation); the others adapt.
    let m = if intent == 3 { mul(&invert(&out.to_ref(false)), &inp.to_ref(false)) } else { matrix(&inp, &out) };
    let identity = dec == enc && (0..3).all(|i| (0..3).all(|j| (m[i][j] - IDENTITY[i][j]).abs() < 1e-9));
    if identity {
        return b;
    }
    let out_luma = out.to_ref(true)[1];
    map_px(&mut b, |c| {
        let l = c.map(|v| dec.decode(v as f64));
        let mut o = mul_vec(&m, l);
        match intent {
            0 => o = compress_gamut(o, out_luma),
            2 => o = o.map(|v| v.max(0.0)),
            _ => {}
        }
        o.map(|v| enc.encode(v) as f32)
    });
    b
}

// ---- Colour programs (the GPU compositor's colour-management kernels)

/// One per-pixel step of a colour transform, in the order the CPU effect applies them (see
/// [`color_program`]).
#[derive(Clone, Debug)]
pub enum ColorOp {
    /// Transfer function, encoded → linear.
    Decode(Tf),
    /// Transfer function, linear → encoded.
    Encode(Tf),
    /// Linear-RGB matrix.
    Matrix([[f32; 3]; 3]),
    /// ASC CDL (forward or exact inverse; `clamp` = the v1.2 style).
    Cdl { cdl: Cdl, clamp: bool, inverse: bool },
    /// The Tone Mapped view's roll-off (or its inverse).
    Tonemap { inverse: bool },
    /// Color Profile Converter's perceptual gamut compression toward luminance `luma`.
    Gamut([f64; 3]),
    /// Negative channels to 0 (the saturation intent).
    Max0,
    /// A LUT (0 nearest, 1 trilinear, 2 tetrahedral) with an optional per-channel shaper, forward
    /// or inverse (as [`FileXform::apply`]).
    Lut { lut: Arc<Lut>, shaper: Option<Arc<[Vec<(f32, f32)>; 3]>>, interp: u32, inverse: bool },
    /// `m × (c + pre) + post` (a custom config's MatrixTransform and RangeTransform).
    Affine { m: [[f32; 3]; 3], pre: [f32; 3], post: [f32; 3] },
    /// `max(c, 0) ^ p` per channel (ExponentTransform).
    Pow([f32; 3]),
    /// LogTransform / LogAffineTransform, forward or inverse (as `ocio_config::Xf::apply`).
    LogAffine { base: f32, log_slope: [f32; 3], log_offset: [f32; 3], lin_slope: [f32; 3], lin_offset: [f32; 3], inverse: bool },
    /// ExponentWithLinearTransform, forward (decode) or inverse (as
    /// [`crate::ocio_config::moncurve`]).
    MonCurve { gamma: [f32; 3], offset: [f32; 3], inverse: bool, mirror: bool },
    /// Clamp each channel to `lo..=hi` (a clamping RangeTransform; ±`f32::MAX` = unbounded).
    Clamp { lo: [f32; 3], hi: [f32; 3] },
}

/// The ops of a matrix-and-offset transform (MatrixTransform, `.spimtx`), forward or inverse
/// (as [`crate::ocio_config::matrix_apply`]).
fn matrix_ops(m: &[f64; 16], offset: &[f64; 4], inverse: bool, ops: &mut Vec<ColorOp>) {
    if !inverse {
        let m = [0, 1, 2].map(|r| [m[r * 4] as f32, m[r * 4 + 1] as f32, m[r * 4 + 2] as f32]);
        ops.push(ColorOp::Affine { m, pre: [0.0; 3], post: [offset[0] as f32, offset[1] as f32, offset[2] as f32] });
    } else if let Some(inv) = crate::ocio_config::mat3_inverse(m) {
        let m = inv.map(|r| r.map(|v| v as f32));
        ops.push(ColorOp::Affine { m, pre: [-offset[0] as f32, -offset[1] as f32, -offset[2] as f32], post: [0.0; 3] });
    }
}

/// The ops of a LUT / CDL / matrix file transform (as [`FileXform::apply`]).
fn file_ops(x: &FileXform, interp: u32, inverse: bool, ccc_id: &str, ops: &mut Vec<ColorOp>) {
    match x {
        FileXform::Cdl(list) => {
            let cdl = list.iter().find(|(i, _)| !ccc_id.is_empty() && i == ccc_id).or(list.first()).map(|(_, c)| *c).unwrap_or_default();
            ops.push(ColorOp::Cdl { cdl, clamp: true, inverse });
        }
        FileXform::Lut { lut, shaper } => {
            ops.push(ColorOp::Lut { lut: Arc::new(lut.clone()), shaper: shaper.clone().map(Arc::new), interp, inverse });
        }
        FileXform::Matrix { m, offset } => matrix_ops(m, offset, inverse, ops),
    }
}

/// The ops of a custom config's transform `x` (as `Xf::apply`).
fn config_xf_ops(x: &crate::ocio_config::Xf, inverse: bool, ops: &mut Vec<ColorOp>) {
    use crate::ocio_config::Xf;
    let f3 = |v: &[f64; 3]| v.map(|x| x as f32);
    match x {
        Xf::Inverse(x) => config_xf_ops(x, !inverse, ops),
        Xf::Matrix { m, offset } => matrix_ops(m, offset, inverse, ops),
        Xf::File { xf: Some(x), interp, ccc, .. } => file_ops(x, *interp, inverse, ccc, ops),
        Xf::ExponentLinear { gamma, offset, mirror } => ops.push(ColorOp::MonCurve { gamma: f3(gamma), offset: f3(offset), inverse, mirror: *mirror }),
        // A BuiltinTransform's curve: forward decodes.
        Xf::Curve(tf) => ops.push(if inverse { ColorOp::Encode(*tf) } else { ColorOp::Decode(*tf) }),
        Xf::Named(_) => {}
        Xf::File { xf: None, .. } | Xf::Unsupported(_) => {}
        Xf::Exponent(e) => ops.push(ColorOp::Pow([0, 1, 2].map(|k| (if inverse { 1.0 / e[k].max(1e-9) } else { e[k] }) as f32))),
        Xf::Log { base } => {
            ops.push(ColorOp::LogAffine { base: *base as f32, log_slope: [1.0; 3], log_offset: [0.0; 3], lin_slope: [1.0; 3], lin_offset: [0.0; 3], inverse })
        }
        Xf::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset } => ops.push(ColorOp::LogAffine {
            base: *base as f32,
            log_slope: f3(log_slope),
            log_offset: f3(log_offset),
            lin_slope: f3(lin_slope),
            lin_offset: f3(lin_offset),
            inverse,
        }),
        Xf::Cdl(cdl) => ops.push(ColorOp::Cdl { cdl: *cdl, clamp: true, inverse }),
        Xf::Range(r) => {
            let (k, off, lo, hi) = r.parts(inverse);
            let k = k as f32;
            ops.push(ColorOp::Affine { m: [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]], pre: [0.0; 3], post: [off as f32; 3] });
            if lo.is_finite() || hi.is_finite() {
                let bound = |v: f64| (v as f32).clamp(-f32::MAX, f32::MAX);
                ops.push(ColorOp::Clamp { lo: [bound(lo); 3], hi: [bound(hi); 3] });
            }
        }
        Xf::Group(list) => {
            if inverse {
                list.iter().rev().for_each(|x| config_xf_ops(x, true, ops));
            } else {
                list.iter().for_each(|x| config_xf_ops(x, false, ops));
            }
        }
    }
}

/// How a colour program meets premultiplied pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Straight {
    /// `unpremul` (colour 0 below alpha 1e-6); fully transparent pixels untouched (OCIO effects).
    Unpremul,
    /// Colour ÷ alpha; fully transparent pixels untouched (`Image::map_straight`, Apply Color LUT).
    Divide,
}

fn xform_ops(x: &Xform, ops: &mut Vec<ColorOp>) {
    if x.dec != Tf::Linear {
        ops.push(ColorOp::Decode(x.dec));
    }
    if !x.ident {
        ops.push(ColorOp::Matrix(x.m));
    }
    if x.enc != Tf::Linear {
        ops.push(ColorOp::Encode(x.enc));
    }
}

/// Effect `id`'s per-pixel transform as a list of [`ColorOp`]s that reproduces the CPU effect
/// (empty = the layer passes through; custom OCIO configs compile their transforms too). `None`
/// for another effect.
pub fn color_program(id: &str, ctx: &EffectCtx) -> Option<(Vec<ColorOp>, Straight)> {
    let pr = ctx.params;
    let mut ops = vec![];
    // Configuration ▸ Custom compiles the config's transforms (a config that can't be read, or
    // names unknown spaces, passes the pixels through).
    match custom_xf(id, pr) {
        Ok(None) => {}
        Ok(Some(xf)) => {
            config_xf_ops(&xf, false, &mut ops);
            return Some((ops, Straight::Unpremul));
        }
        Err(_) => return Some((ops, Straight::Unpremul)),
    }
    match id {
        "ec.utility.applylut" => {
            if let Some(lut) = crate::utility::load_lut(pr.s("lut")) {
                ops.push(ColorOp::Lut { lut, shaper: None, interp: 2, inverse: false });
            }
            return Some((ops, Straight::Divide));
        }
        "ec.color.ociocolorspace" => {
            let (a, z) = (space_at(pr.e("source")), space_at(pr.e("destination")));
            let (a, z) = if pr.e("direction") == 1 { (z, a) } else { (a, z) };
            if a.name != z.name {
                xform_ops(&Xform::new(a, z), &mut ops);
            }
        }
        "ec.color.ociocdl" => {
            let cdl = cdl_from_params(pr);
            if cdl != Cdl::default() {
                ops.push(ColorOp::Cdl { cdl, clamp: pr.e("style") == 0, inverse: pr.e("direction") == 1 });
            }
        }
        "ec.color.ociodisplay" => {
            let input = space_at(pr.e("source"));
            let (display, view, inverse) = (pr.e("display") as usize, pr.e("view") as usize, pr.e("direction") == 1);
            let (_, prims, tf) = DISPLAYS[display.min(DISPLAYS.len() - 1)];
            if view != 2 && !input.raw {
                let disp = Space { name: "display", prims: Some((prims, D65)), tf: Tf::Linear, raw: false };
                let lin = Space { tf: Tf::Linear, ..*input };
                if !inverse {
                    ops.push(ColorOp::Decode(input.tf));
                    xform_ops(&Xform::new(&lin, &disp), &mut ops);
                    if view == 1 {
                        ops.push(ColorOp::Tonemap { inverse: false });
                    }
                    ops.push(ColorOp::Encode(tf));
                } else {
                    ops.push(ColorOp::Decode(tf));
                    if view == 1 {
                        ops.push(ColorOp::Tonemap { inverse: true });
                    }
                    xform_ops(&Xform::new(&disp, &lin), &mut ops);
                    ops.push(ColorOp::Encode(input.tf));
                }
            }
        }
        "ec.color.ociofile" => {
            if let Some(x) = load_file(pr.s("file")) {
                let interp = match pr.e("interpolation") {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                file_ops(&x, interp, pr.e("direction") == 1, pr.s("cccId"), &mut ops);
            }
        }
        "ec.color.ociolook" => {
            let mut list = parse_looks(pr.s("looks"));
            let pick = pr.e("look");
            if pick > 0
                && let Some((_, c)) = looks().get(pick as usize - 1)
            {
                list.insert(0, (*c, false));
            }
            if !list.is_empty() {
                let (src, dst) = (space_at(pr.e("source")), space_at(pr.e("destination")));
                let inv = pr.e("direction") == 1;
                let cct = &ACES_CCT;
                let (a, z) = if inv { (dst, src) } else { (src, dst) };
                if inv {
                    list.reverse();
                }
                xform_ops(&Xform::new(a, cct), &mut ops);
                for (cdl, li) in &list {
                    ops.push(ColorOp::Cdl { cdl: *cdl, clamp: false, inverse: *li != inv });
                }
                xform_ops(&Xform::new(cct, z), &mut ops);
            }
        }
        "ec.utility.colorprofileconverter" => {
            let (inp, in_lin_default) = profile(ctx, pr.e("inputProfile"));
            let (out, out_lin_default) = profile(ctx, pr.e("outputProfile"));
            let in_lin = pr.b("linearizeInputProfile") || in_lin_default && pr.e("inputProfile") == 0;
            let out_lin = pr.b("linearizeOutputProfile") || out_lin_default && pr.e("outputProfile") == 0;
            let intent = pr.e("intent");
            let dec = if in_lin { Tf::Linear } else { inp.tf };
            let enc = if out_lin { Tf::Linear } else { out.tf };
            let m = if intent == 3 { mul(&invert(&out.to_ref(false)), &inp.to_ref(false)) } else { matrix(&inp, &out) };
            let identity = dec == enc && (0..3).all(|i| (0..3).all(|j| (m[i][j] - IDENTITY[i][j]).abs() < 1e-9));
            if !identity {
                ops.push(ColorOp::Decode(dec));
                ops.push(ColorOp::Matrix(m.map(|r| r.map(|v| v as f32))));
                match intent {
                    0 => ops.push(ColorOp::Gamut(out.to_ref(true)[1])),
                    2 => ops.push(ColorOp::Max0),
                    _ => {}
                }
                ops.push(ColorOp::Encode(enc));
            }
        }
        _ => return None,
    }
    Some((ops, Straight::Unpremul))
}

// ---- Color Stabilizer

/// Mean straight colour of a square (half-size `r` layer pixels) around layer point `pt`.
fn sample_mean(b: &Buf, pt: [f64; 2], r: f64) -> [f64; 3] {
    let (cx, cy) = b.to_px(pt);
    let rr = (r * b.scale).max(0.5);
    let (x0, x1) = ((cx - rr).floor().max(0.0) as i64, ((cx + rr).ceil() as i64).min(b.img.width as i64));
    let (y0, y1) = ((cy - rr).floor().max(0.0) as i64, ((cy + rr).ceil() as i64).min(b.img.height as i64));
    let mut acc = [0.0f64; 4];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = b.img.get(x, y);
            for k in 0..4 {
                acc[k] += p[k] as f64;
            }
        }
    }
    if acc[3] <= 1e-9 {
        return [0.0; 3];
    }
    [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]]
}

/// Monotone piecewise-quadratic curve through (0,0), three samples and (1,1) (linear between
/// them is enough for a stabiliser and stays monotone).
fn curve3(xs: [f64; 3], ys: [f64; 3], v: f64) -> f64 {
    let mut pts = vec![(0.0, 0.0)];
    let mut pairs: Vec<(f64, f64)> = xs.iter().cloned().zip(ys.iter().cloned()).collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (x, y) in pairs {
        if pts.last().is_some_and(|l| x > l.0 + 1e-4) && x < 1.0 - 1e-4 {
            pts.push((x, y));
        }
    }
    pts.push((1.0, 1.0));
    let i = pts.partition_point(|p| p.0 < v).clamp(1, pts.len() - 1);
    let (a, c) = (pts[i - 1], pts[i]);
    a.1 + (c.1 - a.1) * (v - a.0) / (c.0 - a.0).max(1e-9)
}

/// Color Stabilizer's correction of buffer `b` as one piecewise-linear map per channel (points
/// with increasing x; the end segments extend), the same maps [`color_stabilizer`] applies.
/// `None` without a host or reference frame (the layer passes through). (The GPU effect.)
pub fn color_stabilizer_maps(ctx: &EffectCtx, b: &Buf) -> Option<[Vec<(f64, f64)>; 3]> {
    let reference = ctx.env.host?.self_at(ctx.params.f("referenceFrame"), ctx.env.effect_index)?;
    let r = ctx.params.f("sampleSize").max(0.5);
    let pts = [ctx.params.v2("blackPoint"), ctx.params.v2("midPoint"), ctx.params.v2("whitePoint")];
    let cur = pts.map(|pt| sample_mean(b, pt, r));
    let refv = pts.map(|pt| sample_mean(&reference, pt, r));
    let mode = ctx.params.e("stabilize");
    Some([0, 1, 2].map(|k| {
        let offset = |c0: f64, r0: f64| vec![(0.0, r0 - c0), (1.0, 1.0 + r0 - c0)];
        match mode {
            0 => offset(cur[0][k], refv[0][k]),
            1 => {
                let (c0, c1, r0, r1) = (cur[0][k], cur[2][k], refv[0][k], refv[2][k]);
                if (c1 - c0).abs() < 1e-6 {
                    offset(c0, r0)
                } else if c0 < c1 {
                    vec![(c0, r0), (c1, r1)]
                } else {
                    vec![(c1, r1), (c0, r0)]
                }
            }
            _ => {
                let mut v = vec![(0.0, 0.0)];
                let mut pairs: Vec<(f64, f64)> = (0..3).map(|i| (cur[i][k], refv[i][k])).collect();
                pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (x, y) in pairs {
                    if v.last().is_some_and(|l| x > l.0 + 1e-4) && x < 1.0 - 1e-4 {
                        v.push((x, y));
                    }
                }
                v.push((1.0, 1.0));
                v
            }
        }
    }))
}

fn color_stabilizer(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(host) = ctx.env.host else { return b };
    let Some(reference) = host.self_at(ctx.params.f("referenceFrame"), ctx.env.effect_index) else { return b };
    let r = ctx.params.f("sampleSize").max(0.5);
    let pts = [ctx.params.v2("blackPoint"), ctx.params.v2("midPoint"), ctx.params.v2("whitePoint")];
    let cur = pts.map(|pt| sample_mean(&b, pt, r));
    let refv = pts.map(|pt| sample_mean(&reference, pt, r));
    let mode = ctx.params.e("stabilize");
    map_px(&mut b, |c| {
        [0, 1, 2].map(|k| {
            let v = c[k] as f64;
            let o = match mode {
                // Brightness: one sample (black point) — offset each channel.
                0 => v + refv[0][k] - cur[0][k],
                // Levels: black and white samples — linear map per channel.
                1 => {
                    let (c0, c1, r0, r1) = (cur[0][k], cur[2][k], refv[0][k], refv[2][k]);
                    if (c1 - c0).abs() < 1e-6 { v + r0 - c0 } else { r0 + (v - c0) * (r1 - r0) / (c1 - c0) }
                }
                // Curves: three samples — a curve through them per channel.
                _ => curve3([cur[0][k], cur[1][k], cur[2][k]], [refv[0][k], refv[1][k], refv[2][k]], v),
            };
            o as f32
        })
    });
    b
}

fn sl(lo: f64, hi: f64, smin: f64, smax: f64) -> ParamUi {
    slider(lo, hi, smin, smax, 3)
}

pub fn specs() -> Vec<EffectSpec> {
    let cc = "Color Correction";
    let srgb = SPACES.iter().position(|s| s.name == "sRGB").unwrap_or(0) as u32;
    let lin709 = SPACES.iter().position(|s| s.name == "Linear Rec.709 (sRGB)").unwrap_or(0) as u32;
    let acescg = SPACES.iter().position(|s| s.name == "ACEScg").unwrap_or(0) as u32;
    let mut look_opts = vec!["None"];
    look_opts.extend(looks().iter().map(|(n, _)| *n));
    vec![
        spec(
            "ec.color.ociocdl",
            "OCIO CDL Transform",
            cc,
            vec![
                p("slopeRed", "Slope Red", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("slopeGreen", "Slope Green", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("slopeBlue", "Slope Blue", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("offsetRed", "Offset Red", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("offsetGreen", "Offset Green", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("offsetBlue", "Offset Blue", num(0.0), sl(-100.0, 100.0, -1.0, 1.0)),
                p("powerRed", "Power Red", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("powerGreen", "Power Green", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("powerBlue", "Power Blue", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("saturation", "Saturation", num(1.0), sl(0.0, 100.0, 0.0, 4.0)),
                p("style", "Style", Value::Enum(0), popup(&["ASC CDL v1.2", "No Clamp"])),
                direction(),
            ],
            ocio_cdl,
        ),
        spec(
            "ec.color.ociocolorspace",
            "OCIO Color Space Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(CONFIGS)),
                p("configFile", "Config File (.ocio)", Value::Str(String::new()), ParamUi::Text),
                space_param("source", "Source", lin709),
                p("sourceName", "Source (config name)", Value::Str(String::new()), ParamUi::Text),
                space_param("destination", "Destination", srgb),
                p("destinationName", "Destination (config name)", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_cst,
        ),
        spec(
            "ec.color.ociodisplay",
            "OCIO Display Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(CONFIGS)),
                p("configFile", "Config File (.ocio)", Value::Str(String::new()), ParamUi::Text),
                space_param("source", "Input Color Space", lin709),
                p("sourceName", "Source (config name)", Value::Str(String::new()), ParamUi::Text),
                p("display", "Display Device", Value::Enum(0), popup(&DISPLAYS.iter().map(|d| d.0).collect::<Vec<_>>())),
                p("displayName", "Display (config name)", Value::Str(String::new()), ParamUi::Text),
                p("view", "View Transform", Value::Enum(0), popup(VIEWS)),
                p("viewName", "View (config name)", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_display,
        ),
        // `file`: a path to a .cube/.3dl/.csp/.cc/.ccc/.cdl file, or the file's text itself.
        spec(
            "ec.color.ociofile",
            "OCIO File Transform",
            cc,
            vec![
                p("file", "File", Value::Str(String::new()), ParamUi::Text),
                p("cccId", "CCC ID", Value::Str(String::new()), ParamUi::Text),
                p("interpolation", "Interpolation", Value::Enum(3), popup(&["Nearest", "Linear", "Tetrahedral", "Best"])),
                direction(),
            ],
            ocio_file,
        ),
        spec(
            "ec.color.ociolook",
            "OCIO Look Transform",
            cc,
            vec![
                p("config", "Configuration", Value::Enum(0), popup(&["Built-in (EffectCraft minimal)"])),
                space_param("source", "Source", acescg),
                space_param("destination", "Destination", acescg),
                p("look", "Look", Value::Enum(0), popup(&look_opts)),
                p("looks", "Looks", Value::Str(String::new()), ParamUi::Text),
                direction(),
            ],
            ocio_look,
        ),
        spec(
            "ec.color.colorstabilizer",
            "Color Stabilizer",
            cc,
            vec![
                // The pivot frame (After Effects' Set Frame button stores it).
                p("referenceFrame", "Reference Frame (s)", num(0.0), slider(-100000.0, 100000.0, 0.0, 60.0, 2)),
                p("stabilize", "Stabilize", Value::Enum(0), popup(&["Brightness", "Levels", "Curves"])),
                p("blackPoint", "Black Point", Value::Vec2([0.25, 0.75]), ParamUi::Point),
                p("midPoint", "Mid Point", Value::Vec2([0.5, 0.5]), ParamUi::Point),
                p("whitePoint", "White Point", Value::Vec2([0.75, 0.25]), ParamUi::Point),
                p("sampleSize", "Sample Size", num(5.0), slider(1.0, 100.0, 1.0, 50.0, 0)),
            ],
            color_stabilizer,
        ),
        spec(
            "ec.utility.colorprofileconverter",
            "Color Profile Converter",
            "Utility",
            vec![
                p("inputProfile", "Input Profile", Value::Enum(0), popup(PROFILES)),
                p("linearizeInputProfile", "Linearize Input Profile", Value::Bool(false), ParamUi::Checkbox),
                p("outputProfile", "Output Profile", Value::Enum(1), popup(PROFILES)),
                p("linearizeOutputProfile", "Linearize Output Profile", Value::Bool(false), ParamUi::Checkbox),
                p("intent", "Intent", Value::Enum(1), popup(&["Perceptual", "Relative Colorimetric", "Saturation", "Absolute Colorimetric"])),
                // Our built-in profiles share a zero black point and none is scene-referred, so
                // these two are kept for compatibility and do not change the result.
                p("useBlackPointCompensation", "Use Black Point Compensation", Value::Bool(true), ParamUi::Checkbox),
                p("sceneRefCompensation", "Scene-ref. Profile Compensation", Value::Enum(2), popup(&["On", "Off", "Use Project Setting"])),
            ],
            profile_converter,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, Image, LayerPixels, run_fx};

    fn close(a: M3, b: M3, tol: f64) {
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() < tol, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn aces_matrices_match_published_values() {
        let ap0 = space_by_name("ACES2065-1").unwrap();
        let ap1 = space_by_name("ACEScg").unwrap();
        // Academy S-2014-004 AP0 → AP1.
        close(
            matrix(ap0, ap1),
            [
                [1.451_439_316_1, -0.236_510_746_9, -0.214_928_569_3],
                [-0.076_553_773_4, 1.176_229_699_8, -0.099_675_926_4],
                [0.008_316_148_4, -0.006_032_449_8, 0.997_716_301_4],
            ],
            2e-4,
        );
        // ACEScg → linear Rec.709 with Bradford adaptation (published ACES config value).
        let lin = space_by_name("Linear Rec.709 (sRGB)").unwrap();
        close(matrix(ap1, lin), [[1.705_05, -0.621_79, -0.083_26], [-0.130_26, 1.140_80, -0.010_55], [-0.024_00, -0.128_97, 1.152_97]], 2e-3);
        // White maps to white.
        let w = mul_vec(&matrix(ap1, lin), [1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
    }

    #[test]
    fn transfer_functions_round_trip() {
        for tf in [Tf::Srgb, Tf::Gamma(2.4), Tf::AcesCct, Tf::Pq] {
            for v in [0.0, 0.001, 0.18, 0.5, 1.0, 4.0] {
                let r = tf.decode(tf.encode(v));
                assert!((r - v).abs() < 1e-6 * v.max(1.0), "{tf:?} {v} {r}");
            }
        }
        // ACEScct of 18 % grey (S-2016-001).
        assert!((Tf::AcesCct.encode(0.18) - 0.413_588).abs() < 1e-5);
        // PQ: 100 nits (1.0) encodes to ≈ 0.508.
        assert!((Tf::Pq.encode(1.0) - 0.508).abs() < 1e-3);
    }

    #[test]
    fn color_space_transform_effect_round_trips() {
        let img = Image::filled(4, 4, [0.2, 0.5, 0.7, 1.0]);
        let srgb = SPACES.iter().position(|s| s.name == "sRGB").unwrap() as u32;
        let cg = SPACES.iter().position(|s| s.name == "ACEScg").unwrap() as u32;
        let a = run_fx("ec.color.ociocolorspace", &[("source", Value::Enum(srgb)), ("destination", Value::Enum(cg))], img.clone(), 0.0, EffectEnv::default());
        assert_ne!(a.img.data, img.data);
        let b = run_fx("ec.color.ociocolorspace", &[("source", Value::Enum(cg)), ("destination", Value::Enum(srgb))], a.img.clone(), 0.0, EffectEnv::default());
        let c = run_fx(
            "ec.color.ociocolorspace",
            &[("source", Value::Enum(srgb)), ("destination", Value::Enum(cg)), ("direction", Value::Enum(1))],
            a.img,
            0.0,
            EffectEnv::default(),
        );
        for (x, y) in b.img.data.iter().zip(&img.data).chain(c.img.data.iter().zip(&img.data)) {
            assert!((0..4).all(|k| (x[k] - y[k]).abs() < 1e-4), "{x:?} {y:?}");
        }
    }

    #[test]
    fn cdl_math() {
        let cdl = Cdl { slope: [1.2, 1.0, 0.8], offset: [0.05, 0.0, -0.02], power: [1.1, 1.0, 0.9], sat: 0.8 };
        let c = [0.3, 0.5, 0.7];
        // Hand computation of the v1.2 formula.
        let sop = [(0.3f64 * 1.2 + 0.05).powf(1.1), 0.5, (0.7f64 * 0.8 - 0.02).powf(0.9)];
        let l = sop[0] * 0.2126 + sop[1] * 0.7152 + sop[2] * 0.0722;
        let want = sop.map(|v| l + 0.8 * (v - l));
        let got = cdl.apply(c, true);
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 1e-12);
        }
        let back = cdl.invert(got, true);
        for k in 0..3 {
            assert!((back[k] - c[k]).abs() < 1e-9);
        }
        // v1.2 clamps, No Clamp keeps negatives.
        let neg = Cdl { offset: [-0.5; 3], ..Cdl::default() };
        assert_eq!(neg.apply([0.2; 3], true), [0.0; 3]);
        assert!((neg.apply([0.2; 3], false)[0] + 0.3).abs() < 1e-12);
        let img = Image::filled(2, 2, [0.3, 0.5, 0.7, 1.0]);
        let out = run_fx("ec.color.ociocdl", &[("slopeRed", num(1.2)), ("offsetRed", num(0.05)), ("powerRed", num(1.1))], img, 0.0, EffectEnv::default());
        let r = (0.3f64 * 1.2 + 0.05).powf(1.1);
        assert!((out.img.get(0, 0)[0] as f64 - r).abs() < 1e-5);
    }

    fn identity_lattice(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<[f32; 3]> {
        let mut v = vec![];
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let s = (n - 1) as f32;
                    v.push(f([r as f32 / s, g as f32 / s, b as f32 / s]));
                }
            }
        }
        v
    }

    /// Sony Imageworks `.spi1d`, `.spi3d` and `.spimtx` (Blender's configs use them, #410), and
    /// malformed ones rejected without panicking.
    #[test]
    fn spi_files_parse() {
        let spi1d = "Version 1\nFrom 0.0 2.0\nLength 3\nComponents 1\n{\n0.0\n0.25\n1.0\n}\n";
        let x = parse_file(spi1d, "spi1d").unwrap();
        let o = x.apply([1.0, 0.5, 2.0], 1, false, "");
        assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.125).abs() < 1e-6 && (o[2] - 1.0).abs() < 1e-6, "{o:?}");
        let back = x.apply(o, 1, true, "");
        assert!((back[0] - 1.0).abs() < 1e-5 && (back[2] - 2.0).abs() < 1e-5, "{back:?}");
        let three = parse_spi1d("Version 1\nFrom 0 1\nLength 2\nComponents 3\n{\n0 0 0\n1 0.5 2\n}\n").unwrap();
        assert_eq!(three.d1, vec![[0.0; 3], [1.0, 0.5, 2.0]]);
        let mut spi3d = String::from("SPILUT 1.0\n3 3\n2 2 2\n");
        for (r, g, b) in (0..8).map(|i| (i & 1, (i >> 1) & 1, i >> 2)) {
            spi3d += &format!("{r} {g} {b} {} {} {}\n", r as f32 * 0.5, g, b);
        }
        let x = parse_file(&spi3d, "spi3d").unwrap();
        let o = x.apply([1.0, 0.5, 0.25], 1, false, "");
        assert!((o[0] - 0.5).abs() < 1e-6 && (o[1] - 0.5).abs() < 1e-6 && (o[2] - 0.25).abs() < 1e-6, "{o:?}");
        let m = parse_file("2 0 0 6553.5\n0 1 0 0\n0 0 1 0\n", "spimtx").unwrap();
        let o = m.apply([0.25, 0.5, 1.0], 1, false, "");
        assert!((o[0] - 0.6).abs() < 1e-6 && o[1] == 0.5 && o[2] == 1.0, "{o:?}");
        assert!((m.apply(o, 1, true, "")[0] - 0.25).abs() < 1e-6);
        for bad in [
            "Version 1\nFrom 0 1\nLength 99999999999\nComponents 1\n{\n0\n}\n",
            "Version 1\nFrom 1 0\nLength 2\nComponents 1\n{\n0\n1\n}\n",
            "Version 1\nFrom 0 1\nLength 3\nComponents 1\n{\n0\n1\n}\n",
            "Version 1\nFrom 0 1\nLength 2\nComponents 2\n{\n0 0\n1 1\n}\n",
        ] {
            assert!(parse_spi1d(bad).is_none(), "{bad}");
        }
        assert!(parse_spi3d("SPILUT 1.0\n3 3\n4000 4000 4000\n").is_none());
        assert!(parse_spi3d("SPILUT 1.0\n3 3\n2 2 2\n9 0 0 1 1 1\n").is_none());
        assert!(parse_spi3d("SPILUT 1.0\n3 3\n2 2 2\n0 0 0 1 1 1\n").is_none(), "incomplete lattice");
        assert!(parse_spimtx("1 0 0\n0 1 0\n").is_none());
        assert!(parse_spimtx("NaN 0 0 0 0 1 0 0 0 0 1 0").is_none());
    }

    #[test]
    fn lut_files_parse_and_interpolate() {
        // .cube with a non-linear lattice: trilinear between lattice points.
        let f = |c: [f32; 3]| [c[0] * c[0], c[1], 1.0 - c[2]];
        let mut cube = String::from("LUT_3D_SIZE 3\n");
        for r in identity_lattice(3, f) {
            cube += &format!("{} {} {}\n", r[0], r[1], r[2]);
        }
        let Some(FileXform::Lut { lut, .. }) = parse_file(&cube, "cube") else { panic!() };
        // Trilinear between 0.5 (0.25) and 1.0 (1.0) at 0.75 → 0.625.
        let o = lut.apply_interp([0.75, 0.3, 0.2], 1);
        assert!((o[0] - 0.625).abs() < 1e-6 && (o[1] - 0.3).abs() < 1e-6 && (o[2] - 0.8).abs() < 1e-6, "{o:?}");
        // Nearest snaps.
        assert_eq!(lut.apply_interp([0.74, 0.0, 0.0], 0)[0], 0.25);
        // .3dl (blue fastest, 10-bit integers, mesh line).
        let mut tdl = String::from("0 511 1023\n");
        for r in 0..3 {
            for g in 0..3 {
                for b in 0..3 {
                    let v = [r, g, b].map(|i| i as f64 * 511.5);
                    tdl += &format!("{} {} {}\n", v[0].round(), v[1].round(), (1023.0 - v[2]).round());
                }
            }
        }
        let Some(FileXform::Lut { lut, .. }) = parse_file(&tdl, "3dl") else { panic!() };
        let o = lut.apply_interp([0.25, 0.5, 0.25], 1);
        assert!((o[0] - 0.25).abs() < 2e-3 && (o[1] - 0.5).abs() < 2e-3 && (o[2] - 0.75).abs() < 2e-3, "{o:?}");
        // .csp with a squaring pre-LUT on red and an identity 2³ lattice.
        let mut csp = String::from("CSPLUTV100\n3D\n\n3\n0 0.5 1\n0 0.25 1\n2\n0 1\n0 1\n2\n0 1\n0 1\n\n2 2 2\n");
        for r in identity_lattice(2, |c| c) {
            csp += &format!("{} {} {}\n", r[0], r[1], r[2]);
        }
        let x = parse_file(&csp, "csp").unwrap();
        let o = x.apply([0.5, 0.4, 0.6], 2, false, "");
        assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.4).abs() < 1e-6, "{o:?}");
        let inv = x.apply(o, 2, true, "");
        assert!((inv[0] - 0.5).abs() < 1e-4, "{inv:?}");
        // CDL collection: pick by id.
        let ccc = r#"<ColorCorrectionCollection><ColorCorrection id="a"><SOPNode><Slope>2 2 2</Slope><Offset>0 0 0</Offset><Power>1 1 1</Power></SOPNode></ColorCorrection>
            <ColorCorrection id='b'><SOPNode><Slope>0.5 0.5 0.5</Slope></SOPNode><SatNode><Saturation>1</Saturation></SatNode></ColorCorrection></ColorCorrectionCollection>"#;
        let x = parse_file(ccc, "ccc").unwrap();
        assert!((x.apply([0.4; 3], 2, false, "b")[0] - 0.2).abs() < 1e-6);
        assert!((x.apply([0.4; 3], 2, false, "a")[0] - 0.8).abs() < 1e-6);
        assert!((x.apply([0.4; 3], 2, false, "")[0] - 0.8).abs() < 1e-6);
        // Effect with inline content; inverse of a 3D lattice.
        let img = Image::filled(2, 2, [0.6, 0.3, 0.2, 1.0]);
        let out = run_fx("ec.color.ociofile", &[("file", Value::Str(cube.clone()))], img.clone(), 0.0, EffectEnv::default());
        let back = run_fx("ec.color.ociofile", &[("file", Value::Str(cube)), ("direction", Value::Enum(1))], out.img, 0.0, EffectEnv::default());
        assert!((back.img.get(0, 0)[0] - 0.6).abs() < 1e-3, "{:?}", back.img.get(0, 0));
        assert!(parse_file("garbage", "3dl").is_none());
    }

    #[test]
    fn display_and_look_transforms() {
        // Linear 0.18 through the sRGB display / Standard view = sRGB encoding of 0.18.
        let img = Image::filled(2, 2, [0.18, 0.18, 0.18, 1.0]);
        let out = run_fx("ec.color.ociodisplay", &[], img.clone(), 0.0, EffectEnv::default());
        assert!((out.img.get(0, 0)[0] as f64 - Tf::Srgb.encode(0.18)).abs() < 1e-5);
        let back = run_fx("ec.color.ociodisplay", &[("direction", Value::Enum(1))], out.img, 0.0, EffectEnv::default());
        assert!((back.img.get(0, 0)[0] - 0.18).abs() < 1e-5);
        // Tone mapped view keeps highlights below 1.
        let hot = Image::filled(1, 1, [8.0, 8.0, 8.0, 1.0]);
        let tm = run_fx("ec.color.ociodisplay", &[("view", Value::Enum(1))], hot, 0.0, EffectEnv::default());
        assert!(tm.img.get(0, 0)[0] < 1.0);
        assert!((tonemap_inv(tonemap(3.0)) - 3.0).abs() < 1e-9);
        // Looks: Warm raises red over blue; its inverse undoes it.
        let grey = Image::filled(1, 1, [0.18, 0.18, 0.18, 1.0]);
        let w = run_fx("ec.color.ociolook", &[("looks", Value::Str("Warm".into()))], grey.clone(), 0.0, EffectEnv::default());
        let px = w.img.get(0, 0);
        assert!(px[0] > px[2]);
        let u = run_fx("ec.color.ociolook", &[("looks", Value::Str("-Warm".into()))], w.img, 0.0, EffectEnv::default());
        assert!((u.img.get(0, 0)[0] - 0.18).abs() < 1e-4 && (u.img.get(0, 0)[2] - 0.18).abs() < 1e-4);
        assert_eq!(parse_looks("Warm, -Cool, nope").len(), 2);
    }

    #[test]
    fn profile_converter_converts_and_round_trips() {
        let img = Image::filled(2, 2, [0.8, 0.4, 0.1, 1.0]);
        // sRGB → Rec. 2020 (relative), then back.
        let a = run_fx(
            "ec.utility.colorprofileconverter",
            &[("inputProfile", Value::Enum(1)), ("outputProfile", Value::Enum(3))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert_ne!(a.img.data, img.data);
        let b = run_fx(
            "ec.utility.colorprofileconverter",
            &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1))],
            a.img,
            0.0,
            EffectEnv::default(),
        );
        assert!((0..3).all(|k| (b.img.get(0, 0)[k] - img.get(0, 0)[k]).abs() < 1e-4));
        // Same profile both sides: identity. Working space defaults to sRGB.
        let c = run_fx(
            "ec.utility.colorprofileconverter",
            &[("inputProfile", Value::Enum(0)), ("outputProfile", Value::Enum(1))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert_eq!(c.img.data, img.data);
        // Perceptual pulls out-of-gamut colours inside: saturated Rec.2020 green into sRGB.
        let g = Image::filled(1, 1, [0.0, 1.0, 0.0, 1.0]);
        let rel = run_fx(
            "ec.utility.colorprofileconverter",
            &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1))],
            g.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert!(rel.img.get(0, 0)[0] < 0.0);
        let per = run_fx(
            "ec.utility.colorprofileconverter",
            &[("inputProfile", Value::Enum(3)), ("outputProfile", Value::Enum(1)), ("intent", Value::Enum(0))],
            g,
            0.0,
            EffectEnv::default(),
        );
        assert!(per.img.get(0, 0).iter().all(|v| *v >= -1e-6));
    }

    struct RefHost(Image);
    impl EffectHost for RefHost {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, _: f64, _: usize) -> Option<Buf> {
            Some(Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 })
        }
    }

    #[test]
    fn color_stabilizer_matches_the_reference_frame() {
        let mk = |k: f32, o: f32| {
            let mut img = Image::new(40, 40);
            for y in 0..40 {
                for x in 0..40 {
                    let v = (x as f32 / 40.0) * k + o;
                    img.set(x, y, [v, v * 0.9, v * 0.8, 1.0]);
                }
            }
            img
        };
        let reference = mk(1.0, 0.0);
        let flicker = mk(0.8, 0.1);
        let host = RefHost(reference.clone());
        let env = EffectEnv { host: Some(&host), ..Default::default() };
        let pts = [("blackPoint", Value::Vec2([5.0, 20.0])), ("midPoint", Value::Vec2([20.0, 20.0])), ("whitePoint", Value::Vec2([35.0, 20.0]))];
        let mut vals: Vec<(&str, Value)> = pts.to_vec();
        vals.push(("stabilize", Value::Enum(1)));
        vals.push(("sampleSize", num(1.0)));
        let out = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, env);
        for x in [5i64, 20, 35] {
            let (o, r) = (out.img.get(x, 20), reference.get(x, 20));
            assert!((o[0] - r[0]).abs() < 0.01, "x {x}: {o:?} vs {r:?}");
        }
        // Deterministic, and a no-op without a host.
        let again = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, env);
        assert_eq!(out.img.data, again.img.data);
        let none = run_fx("ec.color.colorstabilizer", &vals, flicker.clone(), 0.0, EffectEnv::default());
        assert_eq!(none.img.data, flicker.data);
        // Brightness mode corrects an offset at the black point.
        let lifted = mk(1.0, 0.1);
        let mut vals2: Vec<(&str, Value)> = pts.to_vec();
        vals2.push(("sampleSize", num(1.0)));
        let b = run_fx("ec.color.colorstabilizer", &vals2, lifted, 0.0, env);
        assert!((b.img.get(5, 20)[0] - reference.get(5, 20)[0]).abs() < 0.01);
    }
}
