//! Utility effects: bounds growth, HDR inspection/compression, Cineon log conversion and colour
//! lookup tables (.cube).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::{premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Utility", params, render, gpu: false, float: true }
}

fn grow_bounds(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let px = (ctx.params.f("pixels") * b.scale).round().max(0.0) as u32;
    if !ctx.adjustment {
        b.pad(px);
    }
    b
}

/// Visualise out-of-range values: in-range shown as dim grey, > 1 in the highlight colour,
/// < 0 in its complement.
fn overbrights(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ch = ctx.params.e("channel");
    let hc = ctx.params.color("highlightColor");
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        let v = match ch {
            1 => c[0],
            2 => c[1],
            3 => c[2],
            4 => a,
            5 => luminance(c[0], c[1], c[2]),
            _ => c[0].max(c[1]).max(c[2]),
        };
        let o = if v > 1.0 {
            [hc[0], hc[1], hc[2]]
        } else if v < 0.0 {
            [1.0 - hc[0], 1.0 - hc[1], 1.0 - hc[2]]
        } else {
            let g = v * 0.35;
            [g, g, g]
        };
        *px = premul(o, a);
    });
    b
}

/// Soft-knee, hue-preserving compression of the brightest channel into [0, 1).
fn hdr_compress_value(m: f32, knee: f32) -> f32 {
    if m <= knee {
        return m;
    }
    let w = 1.0 - knee;
    knee + w * (m - knee) / ((m - knee) + w)
}

fn hdr_compression(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let t = (ctx.params.f("amount") / 100.0).clamp(0.0, 1.0) as f32;
    if t <= 0.0 {
        return b;
    }
    let knee = 1.0 - 0.5 * t;
    b.img.map_straight(|c| {
        let m = c[0].max(c[1]).max(c[2]);
        if m <= knee {
            return c;
        }
        let k = hdr_compress_value(m, knee) / m;
        c.map(|v| v * k)
    });
    b
}

// ---- Cineon (Kodak printing-density log encoding: 0.002 density per code value, negative gamma 0.6) ----

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cineon {
    pub black: f32,
    pub white: f32,
    pub ib: f32,
    pub iw: f32,
    pub gamma: f32,
    pub rolloff: f32,
}

impl Cineon {
    fn offset(&self) -> f32 {
        10f32.powf((self.black - self.white) * 0.002 / 0.6)
    }
    fn knee(&self) -> f32 {
        1.0 - (self.rolloff / 100.0).clamp(0.0, 0.99)
    }
    /// Normalized code value (code / 1023) → linear, raised to `1.7 / gamma`
    /// ([`effectcraft_project::cineon_log_to_lin`], which Interpret Footage ▸ Cineon Settings
    /// also uses with `1 / gamma`).
    pub fn log_to_lin(&self, v: f32) -> f32 {
        let lin = effectcraft_project::cineon_log_to_lin(v, self.black, self.white, 1.7, self.gamma, self.rolloff);
        self.ib + (self.iw - self.ib) * lin
    }
    /// Linear → normalised code value.
    pub fn lin_to_log(&self, v: f32) -> f32 {
        let off = self.offset();
        let d = self.iw - self.ib;
        let mut lin = (v - self.ib) / if d.abs() < 1e-6 { 1e-6 } else { d };
        let k = self.knee();
        if k < 1.0 && lin > k {
            let w = 1.0 - k;
            lin = k - w * (1.0 - (lin - k) / w).max(1e-6).ln();
        }
        lin = lin.max(0.0).powf(self.gamma.max(0.01) / 1.7);
        let code = self.white + (lin * (1.0 - off) + off).max(1e-9).log10() * 0.6 / 0.002;
        code / 1023.0
    }
}

fn cineon(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = Cineon {
        black: ctx.params.f("tenBitBlackPoint") as f32,
        white: ctx.params.f("tenBitWhitePoint") as f32,
        ib: ctx.params.f("internalBlackPoint") as f32,
        iw: ctx.params.f("internalWhitePoint") as f32,
        gamma: ctx.params.f("gamma") as f32,
        rolloff: ctx.params.f("highlightRolloff") as f32,
    };
    let std = Cineon { black: 95.0, white: 685.0, ib: 0.0, iw: 1.0, gamma: 1.7, rolloff: c.rolloff };
    let kind = ctx.params.e("conversionType");
    b.img.map_straight(|v| {
        v.map(|x| match kind {
            0 => c.lin_to_log(x),
            1 => c.log_to_lin(x),
            _ => std.lin_to_log(c.log_to_lin(x)),
        })
    });
    b
}

// ---- Colour LUTs (.cube) ----

#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub n1: usize,
    pub d1: Vec<[f32; 3]>,
    pub n3: usize,
    pub d3: Vec<[f32; 3]>,
    pub dmin: [f32; 3],
    pub dmax: [f32; 3],
}

/// Parse `.cube` text (TITLE, LUT_1D_SIZE, LUT_3D_SIZE, DOMAIN_MIN/MAX, data red-fastest).
pub fn parse_cube(text: &str) -> Option<Lut> {
    let mut lut = Lut { n1: 0, d1: vec![], n3: 0, d3: vec![], dmin: [0.0; 3], dmax: [1.0; 3] };
    let mut rows: Vec<[f32; 3]> = Vec::new();
    let three = |it: &mut std::str::SplitWhitespace| -> Option<[f32; 3]> {
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c = it.next()?.parse().ok()?;
        Some([a, b, c])
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.clone().next()?;
        match key {
            "TITLE" => continue,
            "LUT_1D_SIZE" => {
                it.next();
                lut.n1 = it.next()?.parse().ok()?;
            }
            "LUT_3D_SIZE" => {
                it.next();
                lut.n3 = it.next()?.parse().ok()?;
            }
            "DOMAIN_MIN" => {
                it.next();
                lut.dmin = three(&mut it)?;
            }
            "DOMAIN_MAX" => {
                it.next();
                lut.dmax = three(&mut it)?;
            }
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                it.next();
                let lo: f32 = it.next()?.parse().ok()?;
                let hi: f32 = it.next()?.parse().ok()?;
                lut.dmin = [lo; 3];
                lut.dmax = [hi; 3];
            }
            k if k.starts_with(|c: char| c.is_ascii_alphabetic()) => continue,
            _ => rows.push(three(&mut it)?),
        }
    }
    let need1 = lut.n1;
    let need3 = lut.n3 * lut.n3 * lut.n3;
    if (lut.n1 == 0 && lut.n3 == 0) || lut.n1 == 1 || lut.n3 == 1 || rows.len() != need1 + need3 || lut.n3 > 256 {
        return None;
    }
    lut.d1 = rows[..need1].to_vec();
    lut.d3 = rows[need1..].to_vec();
    Some(lut)
}

impl Lut {
    fn norm(&self, c: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| ((c[i] - self.dmin[i]) / (self.dmax[i] - self.dmin[i]).max(1e-9)).clamp(0.0, 1.0))
    }
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let mut c = c;
        if self.n1 > 1 {
            let t = self.norm(c);
            c = [0, 1, 2].map(|i| {
                let x = t[i] * (self.n1 - 1) as f32;
                let i0 = (x.floor() as usize).min(self.n1 - 2);
                let f = x - i0 as f32;
                self.d1[i0][i] + (self.d1[i0 + 1][i] - self.d1[i0][i]) * f
            });
        }
        if self.n3 > 1 {
            c = self.tetra(self.norm(c));
        }
        c
    }
    /// Apply with a choice of 3D interpolation: 0 nearest, 1 trilinear, otherwise tetrahedral.
    pub fn apply_interp(&self, c: [f32; 3], interp: u32) -> [f32; 3] {
        let mut c = c;
        if self.n1 > 1 {
            let t = self.norm(c);
            c = [0, 1, 2].map(|i| {
                let x = t[i] * (self.n1 - 1) as f32;
                if interp == 0 {
                    return self.d1[(x.round() as usize).min(self.n1 - 1)][i];
                }
                let i0 = (x.floor() as usize).min(self.n1 - 2);
                let f = x - i0 as f32;
                self.d1[i0][i] + (self.d1[i0 + 1][i] - self.d1[i0][i]) * f
            });
        }
        if self.n3 > 1 {
            let t = self.norm(c);
            c = match interp {
                0 => self.nearest(t),
                1 => self.trilinear(t),
                _ => self.tetra(t),
            };
        }
        c
    }
    fn nearest(&self, t: [f32; 3]) -> [f32; 3] {
        let n = self.n3;
        let s = (n - 1) as f32;
        let i = |x: f32| ((x * s).round() as usize).min(n - 1);
        self.d3[i(t[0]) + i(t[1]) * n + i(t[2]) * n * n]
    }
    /// Trilinear interpolation on the lattice (red fastest).
    pub fn trilinear(&self, t: [f32; 3]) -> [f32; 3] {
        let n = self.n3;
        let s = (n - 1) as f32;
        let idx = |x: f32| {
            let v = x * s;
            let i = (v.floor() as usize).min(n - 2);
            (i, v - i as f32)
        };
        let (r, fr) = idx(t[0]);
        let (g, fg) = idx(t[1]);
        let (b, fb) = idx(t[2]);
        let at = |dr: usize, dg: usize, db: usize| self.d3[(r + dr) + (g + dg) * n + (b + db) * n * n];
        let l = |a: [f32; 3], b: [f32; 3], f: f32| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f];
        let c00 = l(at(0, 0, 0), at(1, 0, 0), fr);
        let c10 = l(at(0, 1, 0), at(1, 1, 0), fr);
        let c01 = l(at(0, 0, 1), at(1, 0, 1), fr);
        let c11 = l(at(0, 1, 1), at(1, 1, 1), fr);
        l(l(c00, c10, fg), l(c01, c11, fg), fb)
    }
    fn tetra(&self, t: [f32; 3]) -> [f32; 3] {
        let n = self.n3;
        let s = (n - 1) as f32;
        let idx = |x: f32| {
            let v = x * s;
            let i = (v.floor() as usize).min(n - 2);
            (i, v - i as f32)
        };
        let (r, fr) = idx(t[0]);
        let (g, fg) = idx(t[1]);
        let (b, fb) = idx(t[2]);
        let at = |dr: usize, dg: usize, db: usize| self.d3[(r + dr) + (g + dg) * n + (b + db) * n * n];
        let c000 = at(0, 0, 0);
        let c111 = at(1, 1, 1);
        let (w1, w2, w3, p1, p2) = if fr > fg {
            if fg > fb {
                (fr, fg, fb, at(1, 0, 0), at(1, 1, 0))
            } else if fr > fb {
                (fr, fb, fg, at(1, 0, 0), at(1, 0, 1))
            } else {
                (fb, fr, fg, at(0, 0, 1), at(1, 0, 1))
            }
        } else if fb > fg {
            (fb, fg, fr, at(0, 0, 1), at(0, 1, 1))
        } else if fb > fr {
            (fg, fb, fr, at(0, 1, 0), at(0, 1, 1))
        } else {
            (fg, fr, fb, at(0, 1, 0), at(1, 1, 0))
        };
        // c = c000 + w1 (p1 - c000) + w2 (p2 - p1) + w3 (c111 - p2), w1 ≥ w2 ≥ w3.
        [0, 1, 2].map(|i| c000[i] + w1 * (p1[i] - c000[i]) + w2 * (p2[i] - p1[i]) + w3 * (c111[i] - p2[i]))
    }
}

type LutCache = Mutex<HashMap<String, Option<Arc<Lut>>>>;

/// Load (and cache) a LUT from inline `.cube` text or a file path.
pub fn load_lut(src: &str) -> Option<Arc<Lut>> {
    if src.trim().is_empty() {
        return None;
    }
    static CACHE: OnceLock<LutCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock()
        && let Some(v) = m.get(src)
    {
        return v.clone();
    }
    let inline = src.contains('\n') || src.contains("LUT_");
    let parsed = if inline { parse_cube(src) } else { std::fs::read_to_string(src.trim()).ok().and_then(|t| parse_cube(&t)) }.map(Arc::new);
    if let Ok(mut m) = cache.lock() {
        m.insert(src.to_string(), parsed.clone());
    }
    parsed
}

fn apply_lut(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(lut) = load_lut(ctx.params.s("lut")) else {
        return b;
    };
    b.img.map_straight(|c| lut.apply(c));
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec("ec.utility.growbounds", "Grow Bounds", vec![p("pixels", "Pixels", num(0.0), slider(0.0, 4000.0, 0.0, 500.0, 0))], grow_bounds),
        spec(
            "ec.utility.ccoverbrights",
            "CC Overbrights",
            vec![
                p("channel", "Channel", Value::Enum(0), popup(&["Max RGB", "Red", "Green", "Blue", "Alpha", "Luminance"])),
                p("highlightColor", "Highlight Color", col(1.0, 0.0, 0.0), ParamUi::Color),
            ],
            overbrights,
        ),
        spec(
            "ec.utility.hdrcompression",
            "HDR Highlight Compression",
            vec![p("amount", "Amount", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1))],
            hdr_compression,
        ),
        spec(
            "ec.utility.cineon",
            "Cineon Converter",
            vec![
                p("conversionType", "Conversion Type", Value::Enum(1), popup(&["Linear to Log", "Log to Linear", "Log to Log"])),
                p("tenBitBlackPoint", "10 Bit Black Point", num(95.0), slider(0.0, 1023.0, 0.0, 1023.0, 0)),
                p("internalBlackPoint", "Internal Black Point", num(0.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("tenBitWhitePoint", "10 Bit White Point", num(685.0), slider(0.0, 1023.0, 0.0, 1023.0, 0)),
                p("internalWhitePoint", "Internal White Point", num(1.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("gamma", "Gamma", num(1.7), slider(0.1, 5.0, 0.1, 5.0, 2)),
                p("highlightRolloff", "Highlight Rolloff", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            ],
            cineon,
        ),
        // `lut`: a path to a `.cube` file (Effect Controls' Choose…, asked for when the effect is
        // applied, as in After Effects), or the `.cube` text itself (commands/automation).
        spec("ec.utility.applylut", "Apply Color LUT", vec![p("lut", "LUT", Value::Str(String::new()), crate::file_ui(&["cube"]))], apply_lut),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Image, Params};

    fn identity_cube(n: usize) -> String {
        let mut s = format!("TITLE \"id\"\n# comment\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |v: usize| v as f32 / (n - 1) as f32;
                    s += &format!("{} {} {}\n", f(r), f(g), f(b));
                }
            }
        }
        s
    }

    fn run(id: &str, vals: &[(&str, Value)], img: Image) -> Buf {
        let s = crate::find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in vals {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn test_img() -> Image {
        let mut img = Image::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                let a = 0.5 + (x as f32) / 16.0;
                img.set(x, y, premul([x as f32 / 7.0, y as f32 / 7.0, ((x + y) % 5) as f32 / 4.0], a));
            }
        }
        img
    }

    #[test]
    fn identity_lut_is_identity() {
        for n in [2, 17] {
            let lut = parse_cube(&identity_cube(n)).unwrap();
            for c in [[0.1, 0.5, 0.9], [0.33, 0.77, 0.0], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.42, 0.42, 0.9]] {
                let o = lut.apply(c);
                for i in 0..3 {
                    assert!((o[i] - c[i]).abs() < 1e-5, "n={n} {c:?} -> {o:?}");
                }
            }
            let img = test_img();
            let out = run("ec.utility.applylut", &[("lut", Value::Str(identity_cube(n)))], img.clone());
            for (a, b) in out.img.data.iter().zip(img.data.iter()) {
                for c in 0..4 {
                    assert!((a[c] - b[c]).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn swap_lut_swaps_channels() {
        let mut s = String::from("LUT_3D_SIZE 2\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    s += &format!("{b} {g} {r}\n");
                }
            }
        }
        let lut = parse_cube(&s).unwrap();
        let o = lut.apply([0.2, 0.5, 0.8]);
        assert!((o[0] - 0.8).abs() < 1e-5 && (o[1] - 0.5).abs() < 1e-5 && (o[2] - 0.2).abs() < 1e-5, "{o:?}");
    }

    #[test]
    fn bad_cube_is_passthrough() {
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n1 1 1\n").is_none());
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 zz\n").is_none());
        let img = test_img();
        let out = run("ec.utility.applylut", &[("lut", Value::Str("LUT_3D_SIZE 3\n0 0 0\n".into()))], img.clone());
        assert_eq!(out.img, img);
        let out = run("ec.utility.applylut", &[("lut", Value::Str("/nonexistent/path.cube".into()))], img.clone());
        assert_eq!(out.img, img);
    }

    #[test]
    fn one_d_lut_with_domain() {
        let lut = parse_cube("LUT_1D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\n1 1 1\n0 0 0\n").unwrap();
        let o = lut.apply([0.5, 1.0, 2.0]);
        assert!((o[0] - 0.75).abs() < 1e-5 && (o[1] - 0.5).abs() < 1e-5 && o[2].abs() < 1e-5, "{o:?}");
    }

    #[test]
    fn cineon_round_trip() {
        let c = Cineon { black: 95.0, white: 685.0, ib: 0.0, iw: 1.0, gamma: 1.7, rolloff: 20.0 };
        for code in [100.0f32, 200.0, 350.0, 500.0, 600.0] {
            let v = code / 1023.0;
            let back = c.lin_to_log(c.log_to_lin(v));
            assert!((back - v).abs() < 1e-3, "{code}: {back}");
        }
        assert!(c.log_to_lin(95.0 / 1023.0).abs() < 1e-4);
    }

    /// Interpret Footage ▸ Cineon Settings and the Cineon Converter's Log to Linear give the same
    /// values for the same points and Highlight Rolloff when the converter's Gamma is 1.7 times
    /// Cineon Settings' Gamma: the converter's default 1.7 and Cineon Settings' Gamma 1 both give
    /// linear light (code 400 is 0.10251).
    #[test]
    fn cineon_converter_matches_interpret_footage() {
        let codes = [0.0f32, 95.0, 250.0, 400.0, 560.0, 685.0, 800.0, 1023.0];
        for (gamma, rolloff) in [(1.0, 20.0), (1.0, 0.0), (1.7, 50.0), (2.4, 50.0)] {
            let mut img = Image::new(codes.len() as u32, 1);
            for (x, code) in codes.iter().enumerate() {
                let v = code / 1023.0;
                img.set(x as u32, 0, [v, v, v, 1.0]);
            }
            let vals = [("conversionType", Value::Enum(1)), ("gamma", num(gamma * 1.7)), ("highlightRolloff", num(rolloff))];
            let out = run("ec.utility.cineon", &vals, img);
            let settings = effectcraft_project::CineonSettings { log_conversion: true, gamma: gamma as f32, rolloff: rolloff as f32, ..Default::default() };
            for (x, code) in codes.iter().enumerate() {
                let footage = settings.convert(code / 1023.0);
                let effect = out.img.data[x][0];
                assert!((footage - effect).abs() < 1e-6, "gamma {gamma}, rolloff {rolloff}, code {code}: {footage} vs {effect}");
            }
        }
        let linear = effectcraft_project::CineonSettings { gamma: 1.0, rolloff: 0.0, ..Default::default() };
        assert!((linear.convert(400.0 / 1023.0) - 0.10251).abs() < 1e-4);
    }

    /// The Cineon Converter gives the same values as before Cineon Settings shared its
    /// log-to-linear function, at the default Gamma (1.7) and at Gamma 2.4, for all three
    /// conversion types.
    #[test]
    fn cineon_converter_output_is_unchanged() {
        fn log_to_lin(v: f32, gamma: f32, rolloff: f32, black: f32, white: f32) -> f32 {
            let off = 10f32.powf((black - white) * 0.002 / 0.6);
            let mut lin = (10f32.powf((v * 1023.0 - white) * 0.002 / 0.6) - off) / (1.0 - off).max(1e-6);
            lin = lin.max(0.0).powf(1.7 / gamma.max(0.01));
            let k = 1.0 - (rolloff / 100.0).clamp(0.0, 0.99);
            if k < 1.0 && lin > k {
                let w = 1.0 - k;
                lin = k + w * (1.0 - (-(lin - k) / w).exp());
            }
            lin
        }
        fn lin_to_log(v: f32, gamma: f32, rolloff: f32, black: f32, white: f32) -> f32 {
            let off = 10f32.powf((black - white) * 0.002 / 0.6);
            let mut lin = v;
            let k = 1.0 - (rolloff / 100.0).clamp(0.0, 0.99);
            if k < 1.0 && lin > k {
                let w = 1.0 - k;
                lin = k - w * (1.0 - (lin - k) / w).max(1e-6).ln();
            }
            lin = lin.max(0.0).powf(gamma.max(0.01) / 1.7);
            (white + (lin * (1.0 - off) + off).max(1e-9).log10() * 0.6 / 0.002) / 1023.0
        }
        let codes = [0.0f32, 95.0, 250.0, 400.0, 560.0, 685.0, 800.0, 1023.0];
        for gamma in [None, Some(2.4)] {
            for kind in 0..3 {
                let mut img = Image::new(codes.len() as u32, 1);
                for (x, code) in codes.iter().enumerate() {
                    let v = code / 1023.0;
                    img.set(x as u32, 0, [v, v, v, 1.0]);
                }
                let mut vals = vec![("conversionType", Value::Enum(kind))];
                if let Some(g) = gamma {
                    vals.push(("gamma", num(g)));
                }
                let out = run("ec.utility.cineon", &vals, img);
                let g = gamma.unwrap_or(1.7) as f32;
                for (x, code) in codes.iter().enumerate() {
                    let v = code / 1023.0;
                    let want = match kind {
                        0 => lin_to_log(v, g, 20.0, 95.0, 685.0),
                        1 => log_to_lin(v, g, 20.0, 95.0, 685.0),
                        _ => lin_to_log(log_to_lin(v, g, 20.0, 95.0, 685.0), 1.7, 20.0, 95.0, 685.0),
                    };
                    let got = out.img.data[x][0];
                    assert!((want - got).abs() < 1e-6, "gamma {g}, type {kind}, code {code}: {want} vs {got}");
                }
            }
        }
    }

    #[test]
    fn hdr_compression_behaviour() {
        let mut img = Image::new(2, 1);
        img.set(0, 0, [0.2, 0.3, 0.4, 1.0]);
        img.set(1, 0, [4.0, 2.0, 1.0, 1.0]);
        let out = run("ec.utility.hdrcompression", &[], img);
        assert_eq!(out.img.data[0], [0.2, 0.3, 0.4, 1.0]);
        let o = out.img.data[1];
        assert!(o[0] <= 1.0 && o[0] > 0.5 && (o[0] / o[1] - 2.0).abs() < 1e-4, "{o:?}");
    }

    #[test]
    fn grow_bounds_pads() {
        let out = run("ec.utility.growbounds", &[("pixels", num(5.0))], test_img());
        assert_eq!((out.img.width, out.img.height), (18, 18));
        assert_eq!(out.offset, [5.0, 5.0]);
    }
}
