//! RGB colour spaces for colour management: the project working space, footage interpretation,
//! output and the display.
//!
//! Each space is defined by its published primaries and white point and its transfer function:
//!
//! * **sRGB IEC 61966-2-1** — Rec. 709 primaries, D65, the piecewise sRGB curve.
//! * **Rec. 709** — ITU-R BT.709-6 primaries, D65, the BT.1886 display EOTF (gamma 2.4).
//! * **Rec. 2020** — ITU-R BT.2020-2 primaries, D65, BT.1886 (gamma 2.4).
//! * **Display P3** — SMPTE EG 432-1 (DCI-P3) primaries with D65 and the sRGB curve.
//! * **ACEScg** — AP1 primaries (Academy S-2014-004), the ACES white (x 0.32168, y 0.33767),
//!   linear.
//! * **ACES2065-1** — AP0 primaries (SMPTE ST 2065-1), the ACES white, linear.
//! * **Rec. 2100 PQ** — BT.2020 primaries with the SMPTE ST 2084 / ITU-R BT.2100 perceptual
//!   quantizer; linear 1.0 is the HDR reference white of ITU-R BT.2408 (203 cd/m²).
//! * **Rec. 2100 HLG** — BT.2020 primaries with the ITU-R BT.2100 hybrid log-gamma OETF;
//!   linear 1.0 is the BT.2408 reference white (75% HLG signal).
//!
//! RGB → XYZ matrices are derived from the primaries (the standard construction from chromaticity
//! coordinates: columns of xyY → XYZ scaled so that RGB (1, 1, 1) maps to the white point), and
//! spaces whose white is not D65 (ACES) are adapted to D65 with the Bradford transform, so
//! conversions between any two spaces are exact up to `f64` rounding. Transfer functions extend
//! to over-range and negative values (32 bpc) by mirroring around zero.

use serde::{Deserialize, Serialize};

/// An RGB colour space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorSpace {
    /// sRGB IEC 61966-2-1.
    Srgb,
    /// HDTV Rec. 709 (gamma 2.4).
    Rec709,
    /// UHDTV Rec. 2020 (gamma 2.4).
    Rec2020,
    /// Display P3.
    DisplayP3,
    /// ACEScg (AP1, linear).
    AcesCg,
    /// ACES2065-1 (AP0, linear).
    Aces2065,
    /// Rec. 2100 PQ (BT.2020 primaries, SMPTE ST 2084).
    Rec2100Pq,
    /// Rec. 2100 HLG (BT.2020 primaries, hybrid log-gamma).
    Rec2100Hlg,
}

/// CIE xy chromaticity of D65.
const D65: [f64; 2] = [0.3127, 0.3290];
/// CIE xy chromaticity of the ACES white point.
pub const ACES_WHITE: [f64; 2] = [0.32168, 0.33767];
const AP0: [[f64; 2]; 3] = [[0.7347, 0.2653], [0.0, 1.0], [0.0001, -0.0770]];
const AP1: [[f64; 2]; 3] = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];
const BT2020: [[f64; 2]; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];

/// SMPTE ST 2084 constants.
const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;
/// Linear 1.0 in cd/m² (ITU-R BT.2408 HDR reference white).
pub const PQ_REFERENCE_WHITE: f64 = 203.0;
/// ITU-R BT.2100 HLG constants.
const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;
/// Scene light (E, 0..1) of linear 1.0: the BT.2408 reference white at a 75% HLG signal.
pub const HLG_REFERENCE: f64 = 0.264_962_56;

/// SMPTE ST 2084 inverse EOTF: absolute luminance / 10 000 cd/m² → signal.
pub fn pq_encode(y: f64) -> f64 {
    let y = y.max(0.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

/// SMPTE ST 2084 EOTF: signal → absolute luminance / 10 000 cd/m².
pub fn pq_decode(e: f64) -> f64 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

/// ITU-R BT.2100 HLG OETF: scene light E (0..1) → signal.
pub fn hlg_encode(e: f64) -> f64 {
    let e = e.max(0.0);
    if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { HLG_A * (12.0 * e - HLG_B).ln() + HLG_C }
}

/// ITU-R BT.2100 HLG inverse OETF: signal → scene light E.
pub fn hlg_decode(v: f64) -> f64 {
    let v = v.max(0.0);
    if v <= 0.5 { v * v / 3.0 } else { (((v - HLG_C) / HLG_A).exp() + HLG_B) / 12.0 }
}

/// Transfer curves (see [`ColorSpace::curve`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Curve {
    Linear,
    Srgb,
    /// BT.1886 gamma 2.4.
    Gamma24,
    Pq,
    Hlg,
}

impl Curve {
    /// Encoded value → linear light.
    pub fn decode(self, v: f32) -> f32 {
        let a = v.abs();
        let l = match self {
            Curve::Linear => a,
            Curve::Srgb => crate::srgb_to_linear(a),
            Curve::Gamma24 => a.powf(2.4),
            Curve::Pq => (pq_decode(a as f64) * 10_000.0 / PQ_REFERENCE_WHITE) as f32,
            Curve::Hlg => (hlg_decode(a as f64) / HLG_REFERENCE) as f32,
        };
        l.copysign(v)
    }

    /// Linear light → encoded value.
    pub fn encode(self, v: f32) -> f32 {
        let a = v.abs();
        let e = match self {
            Curve::Linear => a,
            Curve::Srgb => crate::linear_to_srgb(a),
            Curve::Gamma24 => a.powf(1.0 / 2.4),
            Curve::Pq => pq_encode(a as f64 * PQ_REFERENCE_WHITE / 10_000.0) as f32,
            Curve::Hlg => hlg_encode(a as f64 * HLG_REFERENCE) as f32,
        };
        e.copysign(v)
    }
}

impl ColorSpace {
    /// Every space (footage interpretation, output).
    pub const ALL: [ColorSpace; 8] = [
        ColorSpace::Srgb,
        ColorSpace::Rec709,
        ColorSpace::Rec2020,
        ColorSpace::DisplayP3,
        ColorSpace::AcesCg,
        ColorSpace::Aces2065,
        ColorSpace::Rec2100Pq,
        ColorSpace::Rec2100Hlg,
    ];
    /// Spaces a project can work in (Project Settings ▸ Color ▸ Working Space).
    pub const WORKING: [ColorSpace; 6] =
        [ColorSpace::Srgb, ColorSpace::Rec709, ColorSpace::Rec2020, ColorSpace::DisplayP3, ColorSpace::AcesCg, ColorSpace::Aces2065];

    /// Display name (as in Project Settings ▸ Color ▸ Working Space).
    pub fn label(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "sRGB IEC61966-2.1",
            ColorSpace::Rec709 => "HDTV (Rec. 709)",
            ColorSpace::Rec2020 => "Rec. 2020",
            ColorSpace::DisplayP3 => "Display P3",
            ColorSpace::AcesCg => "ACEScg",
            ColorSpace::Aces2065 => "ACES2065-1",
            ColorSpace::Rec2100Pq => "Rec. 2100 PQ",
            ColorSpace::Rec2100Hlg => "Rec. 2100 HLG",
        }
    }

    /// Short identifier used by commands and files (`srgb`, `rec709`, `rec2020`, `p3`, `acescg`,
    /// `aces2065`, `rec2100pq`, `rec2100hlg`).
    pub fn id(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "srgb",
            ColorSpace::Rec709 => "rec709",
            ColorSpace::Rec2020 => "rec2020",
            ColorSpace::DisplayP3 => "p3",
            ColorSpace::AcesCg => "acescg",
            ColorSpace::Aces2065 => "aces2065",
            ColorSpace::Rec2100Pq => "rec2100pq",
            ColorSpace::Rec2100Hlg => "rec2100hlg",
        }
    }

    /// Parse an id or label (case-insensitive, loose).
    pub fn parse(s: &str) -> Option<ColorSpace> {
        let k: String = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        match k.as_str() {
            "srgb" | "srgbiec6196621" | "srgbiec61966" | "iec6196621" => Some(ColorSpace::Srgb),
            "rec709" | "bt709" | "hdtvrec709" | "hdtv" | "709" => Some(ColorSpace::Rec709),
            "rec2020" | "bt2020" | "2020" | "uhdtv" => Some(ColorSpace::Rec2020),
            "p3" | "displayp3" | "p3d65" | "dcip3d65" => Some(ColorSpace::DisplayP3),
            "acescg" | "ap1" => Some(ColorSpace::AcesCg),
            "aces2065" | "aces20651" | "aces" | "ap0" => Some(ColorSpace::Aces2065),
            "rec2100pq" | "bt2100pq" | "pq" | "st2084" | "hdr10" => Some(ColorSpace::Rec2100Pq),
            "rec2100hlg" | "bt2100hlg" | "hlg" => Some(ColorSpace::Rec2100Hlg),
            _ => None,
        }
    }

    /// Red, green and blue primaries (CIE xy).
    pub fn primaries(self) -> [[f64; 2]; 3] {
        match self {
            ColorSpace::Srgb | ColorSpace::Rec709 => [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]],
            ColorSpace::Rec2020 | ColorSpace::Rec2100Pq | ColorSpace::Rec2100Hlg => BT2020,
            ColorSpace::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060]],
            ColorSpace::AcesCg => AP1,
            ColorSpace::Aces2065 => AP0,
        }
    }

    /// White point (CIE xy).
    pub fn white(self) -> [f64; 2] {
        match self {
            ColorSpace::AcesCg | ColorSpace::Aces2065 => ACES_WHITE,
            _ => D65,
        }
    }

    /// The transfer curve.
    pub fn curve(self) -> Curve {
        match self {
            ColorSpace::Srgb | ColorSpace::DisplayP3 => Curve::Srgb,
            ColorSpace::Rec709 | ColorSpace::Rec2020 => Curve::Gamma24,
            ColorSpace::AcesCg | ColorSpace::Aces2065 => Curve::Linear,
            ColorSpace::Rec2100Pq => Curve::Pq,
            ColorSpace::Rec2100Hlg => Curve::Hlg,
        }
    }

    /// The space is linear light by definition (ACES).
    pub fn is_linear(self) -> bool {
        self.curve() == Curve::Linear
    }

    /// A high-dynamic-range encoding (Rec. 2100 PQ / HLG).
    pub fn is_hdr(self) -> bool {
        matches!(self.curve(), Curve::Pq | Curve::Hlg)
    }

    /// Linear RGB → CIE XYZ D65 (Y of white = 1; non-D65 spaces Bradford-adapted).
    pub fn to_xyz(self) -> [[f64; 3]; 3] {
        let w = self.white();
        let m = rgb_to_xyz(self.primaries(), w);
        if w == D65 { m } else { mul(&bradford(w, D65), &m) }
    }

    /// Encoded value → linear light.
    pub fn decode(self, v: f32) -> f32 {
        self.curve().decode(v)
    }

    /// Linear light → encoded value.
    pub fn encode(self, v: f32) -> f32 {
        self.curve().encode(v)
    }
}

/// Bradford chromatic adaptation from white `src` to white `dst` (CIE xy).
pub fn bradford(src: [f64; 2], dst: [f64; 2]) -> [[f64; 3]; 3] {
    const MB: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let xyz = |c: [f64; 2]| [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]];
    let s = mul_vec(&MB, xyz(src));
    let d = mul_vec(&MB, xyz(dst));
    let diag = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul(&invert(&MB), &mul(&diag, &MB))
}

/// The standard RGB → XYZ matrix from primaries and white point.
pub fn rgb_to_xyz(p: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
    let xyz = |c: [f64; 2]| [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]];
    let (r, g, b) = (xyz(p[0]), xyz(p[1]), xyz(p[2]));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let w = xyz(white);
    let s = mul_vec(&invert(&m), w);
    let mut o = m;
    for row in &mut o {
        for j in 0..3 {
            row[j] *= s[j];
        }
    }
    o
}

/// 3×3 matrix inverse (the matrices here are always well conditioned).
pub fn invert(m: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    let k = 1.0 / det;
    [
        [c(1, 1, 2, 2) * k, -c(0, 1, 2, 2) * k, c(0, 1, 1, 2) * k],
        [-c(1, 0, 2, 2) * k, c(0, 0, 2, 2) * k, -c(0, 0, 1, 2) * k],
        [c(1, 0, 2, 1) * k, -c(0, 0, 2, 1) * k, c(0, 0, 1, 1) * k],
    ]
}

pub fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn mul_vec(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

/// A colour conversion between two spaces, each either encoded (with its transfer curve) or
/// linear. Applied to straight (un-premultiplied) RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conversion {
    /// Decode the input with this space's curve first.
    pub decode: Option<ColorSpace>,
    /// Linear RGB matrix (None = identity primaries).
    pub matrix: Option<[[f32; 3]; 3]>,
    /// Encode the output with this space's curve last.
    pub encode: Option<ColorSpace>,
}

impl Conversion {
    /// From `from` (linear when `from_linear`) to `to` (linear when `to_linear`). `None` when
    /// the conversion is the identity.
    pub fn new(from: ColorSpace, from_linear: bool, to: ColorSpace, to_linear: bool) -> Option<Conversion> {
        let same_primaries = from.primaries() == to.primaries();
        let same_curve = from_linear == to_linear && (from_linear || from.curve() == to.curve());
        if same_primaries && same_curve {
            return None;
        }
        let matrix = (!same_primaries).then(|| {
            let m = mul(&invert(&to.to_xyz()), &from.to_xyz());
            m.map(|r| r.map(|v| v as f32))
        });
        // Same primaries, both encoded with different curves (sRGB ↔ Rec. 709), or a matrix in
        // between: go through linear.
        let decode = (!from_linear).then_some(from);
        let encode = (!to_linear).then_some(to);
        Some(Conversion { decode, matrix, encode })
    }

    /// Decode only (encoded → linear in the same space).
    pub fn linearize(space: ColorSpace) -> Conversion {
        Conversion { decode: Some(space), matrix: None, encode: None }
    }

    /// Encode only (linear → encoded in the same space).
    pub fn delinearize(space: ColorSpace) -> Conversion {
        Conversion { decode: None, matrix: None, encode: Some(space) }
    }

    #[inline]
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let mut c = c;
        if let Some(s) = self.decode {
            c = c.map(|v| s.decode(v));
        }
        if let Some(m) = &self.matrix {
            c = [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2]);
        }
        if let Some(s) = self.encode {
            c = c.map(|v| s.encode(v));
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_matrix_matches_iec() {
        // IEC 61966-2-1 (rounded to 4 places).
        let m = ColorSpace::Srgb.to_xyz();
        let want = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - want[i][j]).abs() < 2e-4, "{i}{j}: {} vs {}", m[i][j], want[i][j]);
            }
        }
        // Rec. 2020 luminance row (BT.2020: 0.2627, 0.6780, 0.0593).
        let y = ColorSpace::Rec2020.to_xyz()[1];
        assert!((y[0] - 0.2627).abs() < 1e-4 && (y[1] - 0.6780).abs() < 1e-4 && (y[2] - 0.0593).abs() < 1e-4, "{y:?}");
    }

    #[test]
    fn round_trips() {
        let samples = [[0.2f32, 0.5, 0.8], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.03, 0.9, 0.4], [1.5, 0.2, 2.0]];
        for a in ColorSpace::ALL {
            for b in ColorSpace::ALL {
                for (al, bl) in [(false, false), (true, false), (false, true), (true, true)] {
                    let fwd = Conversion::new(a, al, b, bl);
                    let back = Conversion::new(b, bl, a, al);
                    assert_eq!(fwd.is_none(), back.is_none());
                    for c in samples {
                        // HDR signals are 0..1.
                        if a.is_hdr() && !al && c.iter().any(|v| *v > 1.0) {
                            continue;
                        }
                        let mid = fwd.map(|f| f.apply(c)).unwrap_or(c);
                        let o = back.map(|f| f.apply(mid)).unwrap_or(mid);
                        for i in 0..3 {
                            // Pure power curves amplify f32 rounding near zero; PQ signals near 1.0
                            // stand for ~50× reference white.
                            let tol = if a.is_hdr() || b.is_hdr() { 1e-2 } else { 2e-3 };
                            assert!((o[i] - c[i]).abs() < tol, "{a:?}{al}→{b:?}{bl}: {c:?} → {mid:?} → {o:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn white_is_preserved_and_gamut_grows() {
        let c = Conversion::new(ColorSpace::Srgb, false, ColorSpace::Rec2020, false).unwrap();
        let w = c.apply([1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-4), "{w:?}");
        // Pure sRGB red sits inside Rec. 2020: less saturated there.
        let r = c.apply([1.0, 0.0, 0.0]);
        assert!(r[0] < 1.0 && r[1] > 0.0 && r[2] > 0.0, "{r:?}");
        // sRGB ↔ Rec. 709 share primaries: only the curve changes.
        let k = Conversion::new(ColorSpace::Srgb, false, ColorSpace::Rec709, false).unwrap();
        assert!(k.matrix.is_none());
        assert!(Conversion::new(ColorSpace::Srgb, true, ColorSpace::Rec709, true).is_none());
    }

    #[test]
    fn transfer_extends_over_range() {
        let s = ColorSpace::Srgb;
        assert!((s.decode(s.encode(4.0)) - 4.0).abs() < 1e-4);
        assert!((s.decode(s.encode(-0.25)) + 0.25).abs() < 1e-5);
        assert!((ColorSpace::Rec709.decode(0.5) - 0.5f32.powf(2.4)).abs() < 1e-6);
        assert_eq!(ColorSpace::parse("sRGB IEC61966-2.1"), Some(ColorSpace::Srgb));
        assert_eq!(ColorSpace::parse("Display P3"), Some(ColorSpace::DisplayP3));
    }
    #[test]
    fn aces_and_rec2100_spaces() {
        // SMPTE ST 2065-1 AP0 → XYZ (ACES white, unadapted).
        let m = rgb_to_xyz(ColorSpace::Aces2065.primaries(), ACES_WHITE);
        let want = [[0.952_552_4, 0.0, 0.000_093_68], [0.343_966_4, 0.728_166_1, -0.072_132_5], [0.0, 0.0, 1.008_825_2]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - want[i][j]).abs() < 1e-5, "{i}{j}: {}", m[i][j]);
            }
        }
        // ACEScg white → sRGB white; ACEScg red sits outside sRGB (negative green/blue).
        let c = Conversion::new(ColorSpace::AcesCg, true, ColorSpace::Srgb, true).unwrap();
        let w = c.apply([1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
        let r = c.apply([1.0, 0.0, 0.0]);
        assert!((r[0] - 1.705).abs() < 3e-3 && r[1] < 0.0 && r[2] < 0.0, "{r:?}");
        assert!(ColorSpace::AcesCg.is_linear() && !ColorSpace::Srgb.is_linear());
        // PQ: the BT.2408 reference white (1.0 = 203 cd/m²) encodes to ≈ 0.58; 10 000 cd/m² to 1.
        let pq = ColorSpace::Rec2100Pq;
        assert!((pq.encode(1.0) - 0.5807).abs() < 1e-3, "{}", pq.encode(1.0));
        assert!((pq.encode(10_000.0 / 203.0) - 1.0).abs() < 1e-4);
        assert!((pq_encode(100.0 / 10_000.0) - 0.508).abs() < 1e-3);
        // HLG: reference white at 75%; the OETF is continuous at 1/12.
        let hlg = ColorSpace::Rec2100Hlg;
        assert!((hlg.encode(1.0) - 0.75).abs() < 1e-5, "{}", hlg.encode(1.0));
        assert!((hlg_encode(1.0 / 12.0) - 0.5).abs() < 1e-9 && (hlg_encode(1.0) - 1.0).abs() < 1e-6);
        for v in [0.0f32, 0.01, 0.18, 1.0, 3.0] {
            assert!((pq.decode(pq.encode(v)) - v).abs() < 1e-3 * v.max(1.0), "pq {v}");
            assert!((hlg.decode(hlg.encode(v)) - v).abs() < 1e-3 * v.max(1.0), "hlg {v}");
        }
        assert!(pq.is_hdr() && hlg.is_hdr() && !ColorSpace::Rec2020.is_hdr());
        // Same primaries as Rec. 2020: only the curve changes.
        assert!(Conversion::new(ColorSpace::Rec2020, false, ColorSpace::Rec2100Pq, false).unwrap().matrix.is_none());
        for cs in ColorSpace::ALL {
            assert_eq!(ColorSpace::parse(cs.id()), Some(cs));
            assert_eq!(ColorSpace::parse(cs.label()), Some(cs), "{}", cs.label());
        }
    }
}
