//! Color Correction effects, batch 3: Change Color, Video Limiter, CC Color Neutralizer,
//! PS Arbitrary Map and Lumetri Color (Basic Correction, Creative, Curves, Color Wheels,
//! Vignette).

use effectcraft_color::{hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, layer_rect, lerp, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Color Correction", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn bipolar() -> ParamUi {
    slider(-100.0, 100.0, -100.0, 100.0, 1)
}

/// Apply `f(straight colour) -> straight colour` to every pixel, keeping alpha.
fn map_colour(b: &mut Buf, f: impl Fn(usize, [f32; 3]) -> [f32; 3] + Sync) {
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        *px = premul(f(i, c), a);
    });
}

// ---------------------------------------------------------------- Change Color

/// Similarity mask (1 = matches) of colour `c` to `key`.
#[inline]
fn colour_match(c: [f32; 3], key: [f32; 3], using: u32, tol: f32, soft: f32) -> f32 {
    let d = match using {
        0 => ((c[0] - key[0]).powi(2) + (c[1] - key[1]).powi(2) + (c[2] - key[2]).powi(2)).sqrt() / 3f32.sqrt(),
        1 => {
            let (h1, s1, _) = rgb_to_hsl(c[0], c[1], c[2]);
            let (h2, _, _) = rgb_to_hsl(key[0], key[1], key[2]);
            let dh = (h1 - h2).abs();
            let dh = dh.min(1.0 - dh) * 2.0;
            // Greys have no reliable hue.
            if s1 < 0.02 { 1.0 } else { dh }
        }
        _ => {
            let chroma = |c: [f32; 3]| {
                let s = (c[0] + c[1] + c[2]).max(1e-6);
                [c[0] / s, c[1] / s]
            };
            let (a, k) = (chroma(c), chroma(key));
            ((a[0] - k[0]).powi(2) + (a[1] - k[1]).powi(2)).sqrt() * 1.5
        }
    };
    if d <= tol {
        1.0
    } else if soft <= 1e-6 {
        0.0
    } else {
        (1.0 - (d - tol) / soft).clamp(0.0, 1.0)
    }
}

fn change_color(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let view_mask = ctx.params.e("view") == 1;
    let dh = (ctx.params.f("hueTransform") / 360.0) as f32;
    let dl = (ctx.params.f("lightnessTransform") / 100.0) as f32;
    let ds = (ctx.params.f("saturationTransform") / 100.0) as f32;
    let k = ctx.params.color("colorToChange");
    let key = [k[0], k[1], k[2]];
    let tol = (ctx.params.f("matchingTolerance") / 100.0) as f32;
    let soft = (ctx.params.f("matchingSoftness") / 100.0) as f32;
    let using = ctx.params.e("matchColors");
    let invert = ctx.params.b("invertColorCorrectionMask");
    if !view_mask && dh == 0.0 && dl == 0.0 && ds == 0.0 {
        return b;
    }
    map_colour(&mut b, |_, c| {
        let mut m = colour_match(c, key, using, tol, soft);
        if invert {
            m = 1.0 - m;
        }
        if view_mask {
            return [m; 3];
        }
        if m <= 0.0 {
            return c;
        }
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let h2 = (h + dh).rem_euclid(1.0);
        let s2 = (s + ds * if ds > 0.0 { 1.0 - s } else { s }).clamp(0.0, 1.0);
        let l2 = (l + dl * if dl > 0.0 { 1.0 - l } else { l }).clamp(0.0, 1.0);
        let (r, g, bb) = hsl_to_rgb(h2, s2, l2);
        [lerp(c[0], r, m), lerp(c[1], g, m), lerp(c[2], bb, m)]
    });
    b
}

// ---------------------------------------------------------------- Video Limiter

const CLIP_LEVELS: [f32; 5] = [0.9, 0.95, 1.0, 1.05, 1.09];

/// Soft knee: values above `start` approach `limit` smoothly.
#[inline]
fn knee(v: f32, start: f32, limit: f32) -> f32 {
    if v <= start || limit <= start {
        return v.min(limit);
    }
    let r = limit - start;
    start + r * (1.0 - (-(v - start) / r).exp())
}

fn video_limiter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let level = CLIP_LEVELS[(ctx.params.e("clipLevel") as usize).min(CLIP_LEVELS.len() - 1)];
    let method = ctx.params.e("clipMethod");
    let comp = [0.0, 0.03, 0.05, 0.1, 0.2][(ctx.params.e("compressionBeforeClipping") as usize).min(4)];
    let warn = ctx.params.b("gamutWarning");
    let wc = ctx.params.color("gamutWarningColor");
    let start = level * (1.0 - comp);
    map_colour(&mut b, |_, c| {
        let y = luminance(c[0], c[1], c[2]);
        let mut o = c;
        // Luma: compress luminance, keep chroma offsets.
        if method != 1 {
            let y2 = knee(y.max(0.0), start, level);
            o = [o[0] - y + y2, o[1] - y + y2, o[2] - y + y2];
        }
        // Chroma: scale chroma towards luma until every channel is within 0..level.
        if method != 0 {
            let yl = luminance(o[0], o[1], o[2]);
            let mut t = 1.0f32;
            for &v in &o {
                if v > level && v - yl > 1e-6 {
                    t = t.min((level - yl) / (v - yl));
                }
                if v < 0.0 && yl - v > 1e-6 {
                    t = t.min(yl / (yl - v));
                }
            }
            let t = t.clamp(0.0, 1.0);
            o = [yl + (o[0] - yl) * t, yl + (o[1] - yl) * t, yl + (o[2] - yl) * t];
        }
        let o = o.map(|v| v.clamp(0.0, level));
        if warn && (c.iter().zip(o.iter()).any(|(a, b)| (a - b).abs() > 1e-4)) {
            return [wc[0], wc[1], wc[2]];
        }
        o
    });
    b
}

// ---------------------------------------------------------------- CC Color Neutralizer

fn color_neutralizer(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pairs = [("shadowsUnbalance", "shadowsBalance"), ("midtonesUnbalance", "midtonesBalance"), ("highlightsUnbalance", "highlightsBalance")];
    let off: Vec<[f32; 3]> = pairs
        .iter()
        .map(|(u, bal)| {
            let (u, v) = (ctx.params.color(u), ctx.params.color(bal));
            [v[0] - u[0], v[1] - u[1], v[2] - u[2]]
        })
        .collect();
    let pin = (ctx.params.f("pinning") / 100.0) as f32;
    let orig = (ctx.params.f("blendWOriginal") / 100.0) as f32;
    let contrast = (ctx.params.f("contrast") / 100.0) as f32;
    let darks = (ctx.params.f("darks") / 100.0) as f32;
    let brights = (ctx.params.f("brights") / 100.0) as f32;
    map_colour(&mut b, |_, c| {
        let l = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0);
        let ws = 1.0 - smoothstep(0.0, 0.5, l);
        let wh = smoothstep(0.5, 1.0, l);
        let wm = 1.0 - ws - wh;
        // Pinning keeps pure black and pure white fixed.
        let keep = 1.0 - pin * (1.0 - 4.0 * l * (1.0 - l)).clamp(0.0, 1.0);
        let mut o = [0.0; 3];
        for k in 0..3 {
            let d = off[0][k] * ws + off[1][k] * wm + off[2][k] * wh;
            let mut v = c[k] + d * keep;
            v = 0.5 + (v - 0.5) * (1.0 + contrast);
            v += darks * 0.25 * (1.0 - l).powi(2) + brights * 0.25 * l * l;
            o[k] = v.max(0.0);
        }
        [lerp(o[0], c[0], orig), lerp(o[1], c[1], orig), lerp(o[2], c[2], orig)]
    });
    b
}

// ---------------------------------------------------------------- PS Arbitrary Map

/// Parse a map string of 256 comma/space separated values (0..255, or 0..1 floats). An empty or
/// malformed map is the identity.
fn parse_map(s: &str) -> Vec<f32> {
    let vals: Vec<f32> = s.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|t| !t.is_empty()).filter_map(|t| t.parse::<f32>().ok()).collect();
    if vals.len() < 2 {
        return (0..256).map(|i| i as f32 / 255.0).collect();
    }
    let max = vals.iter().cloned().fold(0.0f32, f32::max);
    let k = if max > 1.0 { 1.0 / 255.0 } else { 1.0 };
    vals.iter().map(|v| (v * k).clamp(0.0, 1.0)).collect()
}

fn arbitrary_map(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let map = parse_map(ctx.params.s("map"));
    let phase = ctx.params.f("phase");
    let alpha = ctx.params.b("applyPhaseMapToAlpha");
    let n = map.len();
    let lookup = |v: f32| -> f32 {
        let x = (v.clamp(0.0, 1.0) as f64 * (n - 1) as f64 + phase * (n - 1) as f64 / 255.0).rem_euclid(n as f64);
        let i0 = x.floor() as usize % n;
        let i1 = if i0 + 1 < n { i0 + 1 } else { i0 };
        let t = (x - x.floor()) as f32;
        lerp(map[i0], map[i1], t)
    };
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        let o = c.map(lookup);
        let na = if alpha { lookup(a) } else { a };
        *px = premul(o, na);
    });
    b
}

// ---------------------------------------------------------------- Lumetri Color

/// Smooth tone-curve adjustment: offsets for shadows/midtones/highlights (each -1..1), with the
/// end points fixed.
#[inline]
fn tone_curve(x: f32, s: f32, m: f32, h: f32) -> f32 {
    if s == 0.0 && m == 0.0 && h == 0.0 {
        return x;
    }
    let t = x.clamp(0.0, 1.0);
    let bump = |c: f32| {
        let d = (t - c) / 0.25;
        (1.0 - d * d).max(0.0).powi(2)
    };
    x + 0.25 * (s * bump(0.25) + m * bump(0.5) + h * bump(0.75))
}

/// The colour part of a wheel/tint colour (offset from its own grey), 0 for neutral colours.
#[inline]
fn tint_offset(c: [f32; 4]) -> [f32; 3] {
    let l = (c[0] + c[1] + c[2]) / 3.0;
    [c[0] - l, c[1] - l, c[2] - l]
}

/// Control points `x,y x,y …` (0..1, whitespace or `;` separated), sorted by x.
fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let mut pts: Vec<(f32, f32)> = s
        .split(|c: char| c.is_whitespace() || c == ';')
        .filter_map(|t| {
            let (x, y) = t.split_once(',')?;
            let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
            (x.is_finite() && y.is_finite()).then_some((x, y))
        })
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
    pts
}

/// A Lumetri hue / saturation curve: control points `x,y` in 0..1 where y = 0.5 is neutral,
/// interpolated with a cardinal (Catmull-Rom) spline through the points. Hue curves wrap
/// around (x = 0 and x = 1 are the same hue); the others are flat beyond their end points.
/// Tabulated as offsets `y − 0.5`.
#[derive(Clone, Debug)]
pub struct OffsetCurve {
    lut: Vec<f32>,
}

impl OffsetCurve {
    const N: usize = 512;

    /// `None` for an empty or flat-neutral curve.
    pub fn parse(s: &str, periodic: bool) -> Option<OffsetCurve> {
        let pts = parse_points(s);
        if pts.is_empty() || pts.iter().all(|p| (p.1 - 0.5).abs() < 1e-6) {
            return None;
        }
        let mut ext: Vec<(f32, f32)> = vec![];
        if periodic {
            let wrapped: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (x.rem_euclid(1.0), y)).collect();
            let mut w = wrapped;
            w.sort_by(|a, b| a.0.total_cmp(&b.0));
            for k in -1..=1 {
                ext.extend(w.iter().map(|&(x, y)| (x + k as f32, y)));
            }
        } else {
            let (x0, y0) = pts[0];
            let (x1, y1) = pts[pts.len() - 1];
            ext.push((x0.min(0.0) - 2.0, y0));
            ext.push((x0.min(0.0) - 1.0, y0));
            ext.extend(pts.iter().copied());
            ext.push((x1.max(1.0) + 1.0, y1));
            ext.push((x1.max(1.0) + 2.0, y1));
        }
        let n = ext.len();
        // Tangents: the mean of the neighbouring slopes, flat at local extrema and next to flat
        // segments, and at most three times either slope (Fritsch-Carlson) so the curve never
        // overshoots its points, even beside a short steep segment.
        let slope = |k: usize| (ext[k + 1].1 - ext[k].1) / (ext[k + 1].0 - ext[k].0).max(1e-6);
        let m: Vec<f32> = (0..n)
            .map(|k| {
                if k == 0 || k == n - 1 {
                    return 0.0;
                }
                let (a, b) = (slope(k - 1), slope(k));
                if a * b <= 0.0 {
                    0.0
                } else {
                    let lim = 3.0 * a.abs().min(b.abs());
                    ((a + b) * 0.5).clamp(-lim, lim)
                }
            })
            .collect();
        let eval = |x: f32| -> f32 {
            let k = ext.partition_point(|p| p.0 <= x).saturating_sub(1).min(n - 2);
            let (xa, ya) = ext[k];
            let (xb, yb) = ext[k + 1];
            let h = (xb - xa).max(1e-6);
            let t = ((x - xa) / h).clamp(0.0, 1.0);
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * ya + (t3 - 2.0 * t2 + t) * h * m[k] + (-2.0 * t3 + 3.0 * t2) * yb + (t3 - t2) * h * m[k + 1]
        };
        let single = pts.len() == 1;
        let lut = (0..=Self::N).map(|i| if single { pts[0].1 - 0.5 } else { eval(i as f32 / Self::N as f32) - 0.5 }).collect();
        Some(OffsetCurve { lut })
    }

    /// Offset from neutral at `x` (clamped to 0..1).
    pub fn at(&self, x: f32) -> f32 {
        let f = x.clamp(0.0, 1.0) * Self::N as f32;
        let i = (f as usize).min(Self::N - 1);
        let t = f - i as f32;
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t
    }
}

/// Lumetri's built-in creative looks (our own formulas; Custom loads a `.cube` file).
pub const LUMETRI_LOOKS: [&str; 8] =
    ["None", "Custom", "EC Warm Film", "EC Cool Night", "EC Bleach Bypass", "EC Teal & Orange", "EC Faded Matte", "EC Monochrome"];

fn builtin_look(look: u32, c: [f32; 3]) -> [f32; 3] {
    let l = luminance(c[0], c[1], c[2]);
    match look {
        2 => [c[0] * 1.06 + 0.015, c[1] * 1.01 + 0.01, c[2] * 0.88],
        3 => [c[0] * 0.82, c[1] * 0.92, c[2] * 1.08 + 0.02].map(|v| v * 0.92),
        4 => {
            // Silver retained: half the saturation, a luminance overlay for contrast.
            let d = c.map(|v| l + (v - l) * 0.45);
            let o = |v: f32| if l < 0.5 { 2.0 * v * l } else { 1.0 - 2.0 * (1.0 - v) * (1.0 - l) };
            d.map(|v| v * 0.5 + o(v) * 0.5)
        }
        5 => {
            let t = (l - 0.5).clamp(-0.5, 0.5);
            [c[0] + 0.18 * t, c[1] + 0.03 * t, c[2] - 0.16 * t]
        }
        6 => c.map(|v| 0.08 + v * 0.84).map(|v| l + (v - l) * 0.8),
        7 => [l; 3],
        _ => c,
    }
}

/// HSL Secondary's Show Mask styles.
const HSL_MASK_STYLES: [&str; 3] = ["Color/Gray", "Color/Black", "White/Black"];

/// HSL Secondary key: membership (0..1) of an HSL colour (h, s, l in 0..1).
struct HslKey {
    hue: Option<(f32, f32, f32)>,
    sat: Option<(f32, f32, f32)>,
    luma: Option<(f32, f32, f32)>,
}

impl HslKey {
    fn from(ctx: &EffectCtx) -> HslKey {
        let pr = ctx.params;
        let k = |id: &str| pr.f(&format!("hslSecondary/key/{id}")) as f32;
        let on = |id: &str| pr.get(&format!("hslSecondary/key/{id}")).is_none_or(|v| v.as_bool());
        let band = |lo: &str, hi: &str, soft: &str| (k(lo) / 100.0, k(hi) / 100.0, k(soft) / 100.0);
        HslKey {
            hue: on("hueEnabled").then(|| (k("hueCenter"), k("hueRange"), k("hueSoftness"))),
            sat: on("saturationEnabled").then(|| band("saturationLow", "saturationHigh", "saturationSoftness")),
            luma: on("lightnessEnabled").then(|| band("lightnessLow", "lightnessHigh", "lightnessSoftness")),
        }
    }
    fn band(v: f32, (lo, hi, soft): (f32, f32, f32)) -> f32 {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let d = if v < lo {
            lo - v
        } else if v > hi {
            v - hi
        } else {
            0.0
        };
        if d <= 0.0 {
            1.0
        } else if soft > 0.0 {
            (1.0 - d / soft).max(0.0)
        } else {
            0.0
        }
    }
    fn weight(&self, h: f32, s: f32, l: f32) -> f32 {
        let mut w = 1.0;
        if let Some((centre, range, soft)) = self.hue
            && range < 180.0
        {
            let d = ((h * 360.0 - centre).rem_euclid(360.0)).min((centre - h * 360.0).rem_euclid(360.0));
            let hw = if d <= range {
                1.0
            } else if soft > 0.0 {
                (1.0 - (d - range) / soft).max(0.0)
            } else {
                0.0
            };
            // Greys have no hue: they belong to a hue key only as far as they are saturated.
            w *= hw * (s * 20.0).min(1.0);
        }
        if let Some(b) = self.sat {
            w *= Self::band(s, b);
        }
        if let Some(b) = self.luma {
            w *= Self::band(l, b);
        }
        w
    }
}

/// Lumetri Color: Basic Correction (input LUT, white balance, HDR range, tone, saturation),
/// Creative (look, adjustments), Curves (tone sliders, RGB curves, hue/saturation curves),
/// Color Wheels, HSL Secondary (key, refine, correction), Vignette, in that order.
fn lumetri(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let hdr = pr.b("highDynamicRange");
    let hdr_white = if hdr { (pr.get("basicCorrection/tone/hdrWhite").map(Value::as_f64).unwrap_or(100.0) / 100.0).max(0.01) as f32 } else { 1.0 };
    let hdr_spec = if hdr { (pr.f("basicCorrection/tone/hdrSpecular") / 100.0) as f32 } else { 0.0 };
    let input_lut = if pr.e("basicCorrection/inputLut") == 1 { crate::utility::load_lut(pr.s("basicCorrection/inputLutFile")) } else { None };
    let look = pr.e("creative/look");
    let look_lut = if look == 1 { crate::utility::load_lut(pr.s("creative/lookFile")) } else { None };
    let temp = (pr.f("basicCorrection/whiteBalance/temperature") / 100.0) as f32;
    let tint = (pr.f("basicCorrection/whiteBalance/tint") / 100.0) as f32;
    let expo = 2f32.powf(pr.f("basicCorrection/tone/exposure") as f32);
    let contrast = (pr.f("basicCorrection/tone/contrast") / 100.0) as f32;
    let (hi, sh, wh, bl) = (
        (pr.f("basicCorrection/tone/highlights") / 100.0) as f32,
        (pr.f("basicCorrection/tone/shadows") / 100.0) as f32,
        (pr.f("basicCorrection/tone/whites") / 100.0) as f32,
        (pr.f("basicCorrection/tone/blacks") / 100.0) as f32,
    );
    let sat = (pr.f("basicCorrection/saturation") / 100.0) as f32;
    let intensity = (pr.f("creative/lookIntensity") / 100.0) as f32;
    let faded = (pr.f("creative/adjustments/fadedFilm") / 100.0) as f32;
    let vib = (pr.f("creative/adjustments/vibrance") / 100.0) as f32;
    let csat = (pr.f("creative/adjustments/creativeSaturation") / 100.0) as f32;
    let st = tint_offset(pr.color("creative/adjustments/shadowTint"));
    let ht = tint_offset(pr.color("creative/adjustments/highlightTint"));
    let tbal = (pr.f("creative/adjustments/tintBalance") / 100.0) as f32;
    let curve = |k: &str| {
        [
            (pr.f(&format!("curves/{k}Shadows")) / 100.0) as f32,
            (pr.f(&format!("curves/{k}Midtones")) / 100.0) as f32,
            (pr.f(&format!("curves/{k}Highlights")) / 100.0) as f32,
        ]
    };
    let (cm, cr, cg, cb) = (curve("curveMaster"), curve("curveRed"), curve("curveGreen"), curve("curveBlue"));
    let rgb_curves =
        ["curves/rgbCurves/master", "curves/rgbCurves/red", "curves/rgbCurves/green", "curves/rgbCurves/blue"].map(|id| crate::color2::Curve::parse(pr.s(id)));
    let hs = |id: &str, periodic: bool| OffsetCurve::parse(pr.s(&format!("curves/hueSaturationCurves/{id}")), periodic);
    let (hue_sat, hue_hue, hue_luma, luma_sat, sat_sat) =
        (hs("hueVsSat", true), hs("hueVsHue", true), hs("hueVsLuma", true), hs("lumaVsSat", false), hs("satVsSat", false));
    let any_hs = hue_sat.is_some() || hue_hue.is_some() || hue_luma.is_some() || luma_sat.is_some() || sat_sat.is_some();
    let ws = tint_offset(pr.color("colorWheels/shadowsWheel"));
    let wm = tint_offset(pr.color("colorWheels/midtonesWheel"));
    let wh_ = tint_offset(pr.color("colorWheels/highlightsWheel"));
    let vig = pr.f("vignette/vignetteAmount") as f32;
    let vmid = (pr.f("vignette/vignetteMidpoint") / 100.0) as f32;
    let vround = (pr.f("vignette/vignetteRoundness") / 100.0) as f32;
    let vfeather = (pr.f("vignette/vignetteFeather") / 100.0) as f32;
    let sharpen = (pr.f("creative/adjustments/sharpen") / 100.0) as f32;
    // HSL Secondary.
    let sec = |id: &str, d: f64| pr.get(&format!("hslSecondary/{id}")).map(Value::as_f64).unwrap_or(d) as f32;
    let s_temp = sec("correction/temperature", 0.0) / 100.0;
    let s_tint = sec("correction/tint", 0.0) / 100.0;
    let s_contrast = sec("correction/contrast", 0.0) / 100.0;
    let s_sharpen = sec("correction/sharpen", 0.0) / 100.0;
    let s_sat = sec("correction/saturation", 100.0) / 100.0;
    let s_wheel = pr.get("hslSecondary/correction/correctionWheel").map(|v| tint_offset(v.as_color())).unwrap_or([0.0; 3]);
    let show_mask = pr.b("hslSecondary/key/showMask");
    let sec_active = show_mask || s_temp != 0.0 || s_tint != 0.0 || s_contrast != 0.0 || s_sharpen != 0.0 || s_sat != 1.0 || s_wheel != [0.0; 3];
    let (lx, ly, lw, lh) = layer_rect(ctx, &b);
    let w = b.img.width as usize;
    let (cx, cy) = (lx + lw * 0.5, ly + lh * 0.5);
    map_colour(&mut b, |_, c| {
        let mut c = c;
        if let Some(l) = &input_lut {
            c = l.apply(c);
        }
        // Basic Correction: white balance, exposure, contrast, tone, saturation. With High
        // Dynamic Range the tone controls work on a range whose white is HDR White.
        if hdr_white != 1.0 {
            c = c.map(|v| v / hdr_white);
        }
        if temp != 0.0 || tint != 0.0 {
            c = [c[0] * (1.0 + 0.25 * temp), c[1] * (1.0 - 0.25 * tint), c[2] * (1.0 - 0.25 * temp)];
        }
        if expo != 1.0 {
            c = c.map(|v| v * expo);
        }
        if contrast != 0.0 {
            c = c.map(|v| 0.5 + (v - 0.5) * (1.0 + contrast));
        }
        if hi != 0.0 || sh != 0.0 || wh != 0.0 || bl != 0.0 {
            let l = luminance(c[0], c[1], c[2]).max(0.0);
            let lt = l.min(1.0);
            let d = 0.25 * hi * smoothstep(0.5, 1.0, lt) + 0.25 * sh * (1.0 - smoothstep(0.0, 0.5, lt)) + 0.2 * wh * lt * lt + 0.2 * bl * (1.0 - lt).powi(2);
            c = c.map(|v| v + d);
        }
        if hdr_spec != 0.0 {
            // HDR Specular scales what lies above white.
            c = c.map(|v| if v > 1.0 { 1.0 + (v - 1.0) * (1.0 + hdr_spec).max(0.0) } else { v });
        }
        if hdr_white != 1.0 {
            c = c.map(|v| v * hdr_white);
        }
        let l = luminance(c[0], c[1], c[2]);
        if sat != 1.0 {
            c = c.map(|v| l + (v - l) * sat);
        }
        // Creative: the look, then the adjustments, all at Intensity.
        let has_look = look >= 2 || look_lut.is_some();
        if intensity != 0.0 && (has_look || faded != 0.0 || vib != 0.0 || csat != 1.0 || st != [0.0; 3] || ht != [0.0; 3]) {
            let mut d = match &look_lut {
                Some(lut) => lut.apply(c),
                None => builtin_look(look, c),
            };
            if faded != 0.0 {
                d = d.map(|v| v * (1.0 - 0.25 * faded) + 0.12 * faded);
            }
            let l = luminance(d[0], d[1], d[2]);
            if vib != 0.0 {
                let mx = d[0].max(d[1]).max(d[2]);
                let mn = d[0].min(d[1]).min(d[2]);
                let s = if mx > 1e-6 { (mx - mn) / mx } else { 0.0 };
                let k = 1.0 + vib * (1.0 - s.clamp(0.0, 1.0));
                d = d.map(|v| l + (v - l) * k);
            }
            if csat != 1.0 {
                d = d.map(|v| l + (v - l) * csat);
            }
            let pivot = (0.5 + 0.25 * tbal).clamp(0.05, 0.95);
            let lt = l.clamp(0.0, 1.0);
            let wsh = 1.0 - smoothstep(0.0, pivot, lt);
            let whi = smoothstep(pivot, 1.0, lt);
            for k in 0..3 {
                d[k] += 0.5 * (st[k] * wsh + ht[k] * whi);
            }
            c = [lerp(c[0], d[0], intensity), lerp(c[1], d[1], intensity), lerp(c[2], d[2], intensity)];
        }
        // Curves: tone sliders, RGB curves (master, then each channel), hue/saturation curves.
        c = c.map(|v| tone_curve(v, cm[0], cm[1], cm[2]));
        c = [tone_curve(c[0], cr[0], cr[1], cr[2]), tone_curve(c[1], cg[0], cg[1], cg[2]), tone_curve(c[2], cb[0], cb[1], cb[2])];
        if let Some(m) = &rgb_curves[0] {
            c = c.map(|v| m.eval(v));
        }
        for k in 0..3 {
            if let Some(cv) = &rgb_curves[k + 1] {
                c[k] = cv.eval(c[k]);
            }
        }
        if any_hs {
            let (h, s, l) = rgb_to_hsl(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0));
            let chroma = (s * 20.0).min(1.0);
            let off = |cv: &Option<OffsetCurve>, x: f32| cv.as_ref().map(|cv| cv.at(x)).unwrap_or(0.0);
            // y = 1 doubles saturation, y = 0 removes it.
            let mut ms = (1.0 + 2.0 * off(&hue_sat, h) * chroma) * (1.0 + 2.0 * off(&luma_sat, l)) * (1.0 + 2.0 * off(&sat_sat, s));
            ms = ms.max(0.0);
            let dh = off(&hue_hue, h) * chroma;
            let dl = off(&hue_luma, h) * chroma;
            let luma = luminance(c[0], c[1], c[2]);
            if dh != 0.0 {
                let (r, g, b2) = hsl_to_rgb((h + dh).rem_euclid(1.0), s, l);
                // Keep values outside 0..1 (HDR) by moving only the in-range part.
                let base = [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)];
                c = [c[0] + r - base[0], c[1] + g - base[1], c[2] + b2 - base[2]];
            }
            if ms != 1.0 {
                c = c.map(|v| luma + (v - luma) * ms);
            }
            if dl != 0.0 {
                c = c.map(|v| v + dl);
            }
        }
        // Color wheels.
        if ws != [0.0; 3] || wm != [0.0; 3] || wh_ != [0.0; 3] {
            let lt = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0);
            let a = 1.0 - smoothstep(0.0, 0.5, lt);
            let z = smoothstep(0.5, 1.0, lt);
            let m = 1.0 - a - z;
            for k in 0..3 {
                c[k] += 0.5 * (ws[k] * a + wm[k] * m + wh_[k] * z);
            }
        }
        c
    });
    // HSL Secondary: key the graded colour, refine the matte, apply the correction through it.
    let mask = sec_active.then(|| {
        let key = HslKey::from(ctx);
        let mut m = Plane::from_image(&b.img, |px| {
            let (c, a) = unpremul(px);
            if a <= 0.0 {
                return 0.0;
            }
            let (h, s, l) = rgb_to_hsl(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0));
            key.weight(h, s, l)
        });
        let denoise = sec("refine/denoise", 0.0) / 100.0;
        if denoise > 0.0 {
            // Smooth speckle, then restore the matte's edge contrast.
            let s = (denoise * 3.0) as f64 * b.scale;
            m = gauss_plane(&m, s, s);
            let k = 1.0 + denoise * 2.0;
            m.data.iter_mut().for_each(|v| *v = ((*v - 0.5) * k + 0.5).clamp(0.0, 1.0));
        }
        let blur = sec("refine/blur", 0.0) / 100.0;
        if blur > 0.0 {
            let s = (blur * 10.0) as f64 * b.scale;
            m = gauss_plane(&m, s, s);
        }
        if pr.b("hslSecondary/key/invertMask") {
            m.data.iter_mut().for_each(|v| *v = 1.0 - *v);
        }
        m
    });
    let style = pr.e("hslSecondary/key/maskStyle");
    map_colour(&mut b, |i, c| {
        let mut c = c;
        if let Some(m) = &mask {
            let k = m.data[i];
            if show_mask {
                return match style {
                    0 => {
                        let g = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) * 0.2 + 0.4;
                        [lerp(g, c[0], k), lerp(g, c[1], k), lerp(g, c[2], k)]
                    }
                    1 => c.map(|v| v * k),
                    _ => [k; 3],
                };
            }
            if k > 0.0 {
                let mut d = c;
                if s_temp != 0.0 || s_tint != 0.0 {
                    d = [d[0] * (1.0 + 0.25 * s_temp), d[1] * (1.0 - 0.25 * s_tint), d[2] * (1.0 - 0.25 * s_temp)];
                }
                if s_contrast != 0.0 {
                    d = d.map(|v| 0.5 + (v - 0.5) * (1.0 + s_contrast));
                }
                let l = luminance(d[0], d[1], d[2]);
                if s_sat != 1.0 {
                    d = d.map(|v| l + (v - l) * s_sat);
                }
                for j in 0..3 {
                    d[j] += 0.5 * s_wheel[j];
                }
                c = [lerp(c[0], d[0], k), lerp(c[1], d[1], k), lerp(c[2], d[2], k)];
            }
        }
        // Vignette.
        if vig != 0.0 {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let nx = ((x - cx) / (lw * 0.5).max(1e-6)) as f32;
            let ny = ((y - cy) / (lh * 0.5).max(1e-6)) as f32;
            // Roundness: 0 follows the frame's aspect, +1 a circle, -1 squarer.
            let aspect = (lw / lh.max(1e-6)) as f32;
            let (nx, ny) = if vround > 0.0 { (nx * lerp(1.0, aspect.max(1.0), vround), ny * lerp(1.0, (1.0 / aspect).max(1.0), vround)) } else { (nx, ny) };
            let pw = 2.0 + (-vround).max(0.0) * 6.0;
            let r = (nx.abs().powf(pw) + ny.abs().powf(pw)).powf(1.0 / pw) / std::f32::consts::SQRT_2;
            let start = vmid * 0.9;
            let f = smoothstep(start, start + 0.05 + vfeather * (1.05 - start), r);
            let k = if vig < 0.0 { 1.0 + vig * 0.2 * f } else { 1.0 };
            c = c.map(|v| if vig > 0.0 { v + (1.0 - v) * (vig * 0.2 * f).min(1.0) } else { v * k.max(0.0) });
        }
        c.map(|v| v.max(0.0))
    });
    let sec_sharpen = if show_mask { 0.0 } else { s_sharpen };
    if sharpen != 0.0 || (sec_sharpen != 0.0 && mask.is_some()) {
        let luma = Plane::luma(&b.img);
        let blur = gauss_plane(&luma, 1.0 * b.scale, 1.0 * b.scale);
        b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
            let (c, a) = unpremul(*px);
            if a <= 0.0 {
                return;
            }
            let amount = sharpen + sec_sharpen * mask.as_ref().map(|m| m.data[i]).unwrap_or(0.0);
            let d = (luma.data[i] - blur.data[i]) * amount * 2.0;
            *px = premul(c.map(|v| (v + d).max(0.0)), a);
        });
    }
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let grey = || col(0.5, 0.5, 0.5);
    let mut lum = vec![
        p("highDynamicRange", "High Dynamic Range", Value::Bool(false), ParamUi::Checkbox),
        p("basicCorrection/inputLut", "Input LUT", Value::Enum(0), popup(&["None", "Custom"])),
        // A .cube file path (or the file's text) for Input LUT = Custom.
        p("basicCorrection/inputLutFile", "Input LUT File", Value::Str(String::new()), ParamUi::Text),
        p("basicCorrection/whiteBalance/temperature", "Temperature", num(0.0), bipolar()),
        p("basicCorrection/whiteBalance/tint", "Tint", num(0.0), bipolar()),
        p("basicCorrection/tone/exposure", "Exposure", num(0.0), slider(-5.0, 5.0, -5.0, 5.0, 2)),
        p("basicCorrection/tone/contrast", "Contrast", num(0.0), bipolar()),
        p("basicCorrection/tone/highlights", "Highlights", num(0.0), bipolar()),
        p("basicCorrection/tone/shadows", "Shadows", num(0.0), bipolar()),
        p("basicCorrection/tone/whites", "Whites", num(0.0), bipolar()),
        p("basicCorrection/tone/blacks", "Blacks", num(0.0), bipolar()),
        p("basicCorrection/tone/hdrWhite", "HDR White", num(100.0), slider(100.0, 10000.0, 100.0, 4000.0, 0)),
        p("basicCorrection/tone/hdrSpecular", "HDR Specular", num(0.0), bipolar()),
        p("basicCorrection/saturation", "Saturation", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
        p("creative/look", "Look", Value::Enum(0), popup(&LUMETRI_LOOKS)),
        p("creative/lookFile", "Look File", Value::Str(String::new()), ParamUi::Text),
        p("creative/lookIntensity", "Intensity", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
        p("creative/adjustments/fadedFilm", "Faded Film", num(0.0), pct()),
        p("creative/adjustments/sharpen", "Sharpen", num(0.0), bipolar()),
        p("creative/adjustments/vibrance", "Vibrance", num(0.0), bipolar()),
        p("creative/adjustments/creativeSaturation", "Saturation", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
        p("creative/adjustments/shadowTint", "Shadow Tint", grey(), ParamUi::Color),
        p("creative/adjustments/highlightTint", "Highlight Tint", grey(), ParamUi::Color),
        p("creative/adjustments/tintBalance", "Tint Balance", num(0.0), bipolar()),
    ];
    const CURVES: [(&str, &str); 12] = [
        ("curves/curveMasterShadows", "RGB Curve Shadows"),
        ("curves/curveMasterMidtones", "RGB Curve Midtones"),
        ("curves/curveMasterHighlights", "RGB Curve Highlights"),
        ("curves/curveRedShadows", "Red Curve Shadows"),
        ("curves/curveRedMidtones", "Red Curve Midtones"),
        ("curves/curveRedHighlights", "Red Curve Highlights"),
        ("curves/curveGreenShadows", "Green Curve Shadows"),
        ("curves/curveGreenMidtones", "Green Curve Midtones"),
        ("curves/curveGreenHighlights", "Green Curve Highlights"),
        ("curves/curveBlueShadows", "Blue Curve Shadows"),
        ("curves/curveBlueMidtones", "Blue Curve Midtones"),
        ("curves/curveBlueHighlights", "Blue Curve Highlights"),
    ];
    for (id, name) in CURVES {
        lum.push(p(id, name, num(0.0), bipolar()));
    }
    // Curve graphs: control points "x,y x,y …" in 0..1 (like Curves). RGB curves default to
    // the identity diagonal; hue/saturation curves are flat at y = 0.5 (no change) and empty
    // means flat.
    let pts = |id: &'static str, name: &'static str, d: &str| p(id, name, Value::Str(d.into()), ParamUi::Hidden);
    lum.extend([
        pts("curves/rgbCurves/master", "RGB Curve", "0,0 1,1"),
        pts("curves/rgbCurves/red", "Red Curve", "0,0 1,1"),
        pts("curves/rgbCurves/green", "Green Curve", "0,0 1,1"),
        pts("curves/rgbCurves/blue", "Blue Curve", "0,0 1,1"),
        pts("curves/hueSaturationCurves/hueVsSat", "Hue vs Sat", ""),
        pts("curves/hueSaturationCurves/hueVsHue", "Hue vs Hue", ""),
        pts("curves/hueSaturationCurves/hueVsLuma", "Hue vs Luma", ""),
        pts("curves/hueSaturationCurves/lumaVsSat", "Luma vs Sat", ""),
        pts("curves/hueSaturationCurves/satVsSat", "Sat vs Sat", ""),
    ]);
    lum.extend([
        p("colorWheels/shadowsWheel", "Shadows", grey(), ParamUi::Color),
        p("colorWheels/midtonesWheel", "Midtones", grey(), ParamUi::Color),
        p("colorWheels/highlightsWheel", "Highlights", grey(), ParamUi::Color),
        // HSL Secondary: key (hue centre / range / softness in degrees, saturation and
        // lightness bands in %), refine, correction.
        p("hslSecondary/key/hueEnabled", "H", Value::Bool(true), ParamUi::Checkbox),
        p("hslSecondary/key/hueCenter", "Hue Center", num(0.0), ParamUi::Angle),
        p("hslSecondary/key/hueRange", "Hue Range", num(180.0), slider(0.0, 180.0, 0.0, 180.0, 1)),
        p("hslSecondary/key/hueSoftness", "Hue Softness", num(0.0), slider(0.0, 180.0, 0.0, 90.0, 1)),
        p("hslSecondary/key/saturationEnabled", "S", Value::Bool(true), ParamUi::Checkbox),
        p("hslSecondary/key/saturationLow", "Saturation Low", num(0.0), pct()),
        p("hslSecondary/key/saturationHigh", "Saturation High", num(100.0), pct()),
        p("hslSecondary/key/saturationSoftness", "Saturation Softness", num(0.0), pct()),
        p("hslSecondary/key/lightnessEnabled", "L", Value::Bool(true), ParamUi::Checkbox),
        p("hslSecondary/key/lightnessLow", "Lightness Low", num(0.0), pct()),
        p("hslSecondary/key/lightnessHigh", "Lightness High", num(100.0), pct()),
        p("hslSecondary/key/lightnessSoftness", "Lightness Softness", num(0.0), pct()),
        p("hslSecondary/key/showMask", "Show Mask", Value::Bool(false), ParamUi::Checkbox),
        p("hslSecondary/key/maskStyle", "Mask Style", Value::Enum(0), popup(&HSL_MASK_STYLES)),
        p("hslSecondary/key/invertMask", "Invert Mask", Value::Bool(false), ParamUi::Checkbox),
        p("hslSecondary/refine/denoise", "Denoise", num(0.0), pct()),
        p("hslSecondary/refine/blur", "Blur", num(0.0), pct()),
        p("hslSecondary/correction/correctionWheel", "Correction", grey(), ParamUi::Color),
        p("hslSecondary/correction/temperature", "Temperature", num(0.0), bipolar()),
        p("hslSecondary/correction/tint", "Tint", num(0.0), bipolar()),
        p("hslSecondary/correction/contrast", "Contrast", num(0.0), bipolar()),
        p("hslSecondary/correction/sharpen", "Sharpen", num(0.0), bipolar()),
        p("hslSecondary/correction/saturation", "Saturation", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
        p("vignette/vignetteAmount", "Amount", num(0.0), slider(-5.0, 5.0, -5.0, 5.0, 2)),
        p("vignette/vignetteMidpoint", "Midpoint", num(50.0), pct()),
        p("vignette/vignetteRoundness", "Roundness", num(0.0), bipolar()),
        p("vignette/vignetteFeather", "Feather", num(50.0), pct()),
    ]);
    vec![
        spec(
            "ec.color.changecolor",
            "Change Color",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Corrected Layer", "Color Correction Mask"])),
                p("hueTransform", "Hue Transform", num(0.0), slider(-360.0, 360.0, -180.0, 180.0, 1)),
                p("lightnessTransform", "Lightness Transform", num(0.0), bipolar()),
                p("saturationTransform", "Saturation Transform", num(0.0), bipolar()),
                p("colorToChange", "Color To Change", col(0.8, 0.2, 0.2), ParamUi::Color),
                p("matchingTolerance", "Matching Tolerance", num(15.0), pct()),
                p("matchingSoftness", "Matching Softness", num(0.0), pct()),
                p("matchColors", "Match Colors", Value::Enum(1), popup(&["Using RGB", "Using Hue", "Using Chroma"])),
                p("invertColorCorrectionMask", "Invert Color Correction Mask", Value::Bool(false), ParamUi::Checkbox),
            ],
            change_color,
        ),
        spec(
            "ec.color.videolimiter",
            "Video Limiter",
            vec![
                p("clipLevel", "Clip Level", Value::Enum(2), popup(&["90%", "95%", "100%", "105%", "109%"])),
                p("clipMethod", "Clip Method", Value::Enum(2), popup(&["Luma", "Chroma", "Smart Limit"])),
                p("compressionBeforeClipping", "Compression before Clipping", Value::Enum(2), popup(&["None", "3%", "5%", "10%", "20%"])),
                p("gamutWarning", "Gamut Warning", Value::Bool(false), ParamUi::Checkbox),
                p("gamutWarningColor", "Gamut Warning Color", col(1.0, 0.0, 0.0), ParamUi::Color),
            ],
            video_limiter,
        ),
        spec(
            "ec.color.cccolorneutralizer",
            "CC Color Neutralizer",
            vec![
                p("shadowsUnbalance", "Shadows Unbalance", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("shadowsBalance", "Shadows Balance", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("midtonesUnbalance", "Midtones Unbalance", grey(), ParamUi::Color),
                p("midtonesBalance", "Midtones Balance", grey(), ParamUi::Color),
                p("highlightsUnbalance", "Highlights Unbalance", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("highlightsBalance", "Highlights Balance", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("pinning", "Pinning", num(0.0), pct()),
                p("blendWOriginal", "Blend w. Original", num(0.0), pct()),
                p("darks", "Darks", num(0.0), bipolar()),
                p("brights", "Brights", num(0.0), bipolar()),
                p("contrast", "Contrast", num(0.0), bipolar()),
            ],
            color_neutralizer,
        ),
        spec(
            "ec.color.psarbitrarymap",
            "PS Arbitrary Map",
            vec![
                p("phase", "Phase", num(0.0), slider(-1000.0, 1000.0, 0.0, 255.0, 0)),
                p("applyPhaseMapToAlpha", "Apply Phase Map To Alpha", Value::Bool(false), ParamUi::Checkbox),
                p("map", "Map", Value::Str(String::new()), ParamUi::Hidden),
            ],
            arbitrary_map,
        ),
        spec("ec.color.lumetri", "Lumetri Color", lum, lumetri),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};
    use effectcraft_raster::Image;

    fn ramp() -> Image {
        let mut img = Image::new(16, 8);
        for y in 0..8 {
            for x in 0..16 {
                let a = if y < 7 { 1.0 } else { 0.5 };
                img.set(x, y, [x as f32 / 16.0 * a, (y as f32 / 8.0) * a, 0.3 * a, a]);
            }
        }
        img
    }

    fn close(a: &Image, b: &Image, tol: f32) -> bool {
        a.data.iter().zip(b.data.iter()).all(|(p, q)| p.iter().zip(q.iter()).all(|(x, y)| (x - y).abs() <= tol))
    }

    fn run(id: &str, vals: &[(&str, Value)], img: Image) -> Image {
        run_fx(id, vals, img, 0.0, EffectEnv::default()).img
    }

    #[test]
    fn change_color_zero_is_identity_and_hue_shift_moves_red() {
        let img = ramp();
        assert!(close(&run("ec.color.changecolor", &[], img.clone()), &img, 0.0));
        let red = Image::filled(4, 4, [0.8, 0.2, 0.2, 1.0]);
        let out = run("ec.color.changecolor", &[("hueTransform", num(120.0))], red.clone());
        let p = out.get(1, 1);
        assert!(p[1] > p[0] && p[1] > p[2], "{p:?}");
        let blue = Image::filled(4, 4, [0.1, 0.2, 0.9, 1.0]);
        assert!(close(&run("ec.color.changecolor", &[("hueTransform", num(120.0))], blue.clone()), &blue, 1e-6));
        let mask = run("ec.color.changecolor", &[("view", Value::Enum(1))], red);
        assert!((mask.get(0, 0)[0] - 1.0).abs() < 1e-6);
        assert_eq!(out.data, run("ec.color.changecolor", &[("hueTransform", num(120.0))], Image::filled(4, 4, [0.8, 0.2, 0.2, 1.0])).data);
    }

    #[test]
    fn video_limiter_keeps_legal_and_limits_hot() {
        let mid = Image::filled(4, 4, [0.4, 0.5, 0.3, 1.0]);
        assert!(close(&run("ec.color.videolimiter", &[], mid.clone()), &mid, 1e-6));
        let hot = Image::filled(4, 4, [1.6, 1.2, 0.9, 1.0]);
        let out = run("ec.color.videolimiter", &[], hot);
        assert!(out.data.iter().all(|p| p[0] <= 1.0 + 1e-6 && p[1] <= 1.0 + 1e-6));
        let warn = run("ec.color.videolimiter", &[("gamutWarning", Value::Bool(true))], Image::filled(2, 2, [1.6, 1.2, 0.9, 1.0]));
        assert_eq!(warn.get(0, 0), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn color_neutralizer_defaults_identity_and_removes_cast() {
        let img = ramp();
        assert!(close(&run("ec.color.cccolorneutralizer", &[], img.clone()), &img, 1e-6));
        let cast = Image::filled(4, 4, [0.6, 0.5, 0.4, 1.0]);
        let out = run("ec.color.cccolorneutralizer", &[("midtonesUnbalance", col(0.6, 0.5, 0.4)), ("midtonesBalance", col(0.5, 0.5, 0.5))], cast);
        let p = out.get(0, 0);
        assert!((p[0] - p[2]).abs() < 0.1, "{p:?}");
    }

    #[test]
    fn arbitrary_map_identity_and_inverting_map() {
        let img = ramp();
        assert!(close(&run("ec.color.psarbitrarymap", &[], img.clone()), &img, 1e-5));
        let inv: Vec<String> = (0..256).map(|i| (255 - i).to_string()).collect();
        let out = run("ec.color.psarbitrarymap", &[("map", Value::Str(inv.join(",")))], Image::filled(2, 2, [0.25, 0.5, 1.0, 1.0]));
        let p = out.get(0, 0);
        assert!((p[0] - 0.75).abs() < 0.01 && p[2].abs() < 0.01, "{p:?}");
    }

    #[test]
    fn lumetri_defaults_identity_and_controls_act() {
        let img = ramp();
        assert!(close(&run("ec.color.lumetri", &[], img.clone()), &img, 1e-5));
        let g = Image::filled(8, 8, [0.4, 0.4, 0.4, 1.0]);
        let e = run("ec.color.lumetri", &[("basicCorrection/tone/exposure", num(1.0))], g.clone());
        assert!((e.get(0, 0)[0] - 0.8).abs() < 1e-5);
        let warm = run("ec.color.lumetri", &[("basicCorrection/whiteBalance/temperature", num(50.0))], g.clone());
        assert!(warm.get(0, 0)[0] > warm.get(0, 0)[2]);
        let bw = run("ec.color.lumetri", &[("basicCorrection/saturation", num(0.0))], img.clone());
        let p = bw.get(5, 3);
        assert!((p[0] - p[1]).abs() < 1e-5 && (p[1] - p[2]).abs() < 1e-5);
        let v = run("ec.color.lumetri", &[("vignette/vignetteAmount", num(-3.0))], g.clone());
        assert!(v.get(0, 0)[0] < v.get(4, 4)[0]);
        let c = run("ec.color.lumetri", &[("curves/curveMasterMidtones", num(50.0))], g.clone());
        assert!(c.get(0, 0)[0] > 0.4);
        assert_eq!(c.data, run("ec.color.lumetri", &[("curves/curveMasterMidtones", num(50.0))], g).data);
    }

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    /// Red, green and grey swatches side by side.
    fn swatches() -> Image {
        let mut img = Image::new(3, 1);
        img.set(0, 0, [0.8, 0.1, 0.1, 1.0]);
        img.set(1, 0, [0.1, 0.8, 0.1, 1.0]);
        img.set(2, 0, [0.5, 0.5, 0.5, 1.0]);
        img
    }

    #[test]
    fn lumetri_rgb_curves_per_channel() {
        let g = Image::filled(2, 2, [0.5, 0.5, 0.5, 1.0]);
        let m = run("ec.color.lumetri", &[("curves/rgbCurves/master", s("0,0 0.5,0.75 1,1"))], g.clone()).get(0, 0);
        assert!((m[0] - 0.75).abs() < 1e-3 && (m[2] - 0.75).abs() < 1e-3);
        let r = run("ec.color.lumetri", &[("curves/rgbCurves/red", s("0,0 0.5,0.25 1,1"))], g).get(0, 0);
        assert!((r[0] - 0.25).abs() < 1e-3 && (r[1] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn lumetri_hue_saturation_curves_target_hues() {
        let img = swatches();
        // Hue vs Sat: desaturate reds only (a dip to 0 at hue 0, neutral from 0.2 to 0.8).
        let out = run("ec.color.lumetri", &[("curves/hueSaturationCurves/hueVsSat", s("0,0 0.2,0.5 0.8,0.5"))], img.clone());
        let red = out.get(0, 0);
        assert!((red[0] - red[1]).abs() < 0.02, "red desaturated: {red:?}");
        assert!(close_px(out.get(1, 0), img.get(1, 0), 1e-3), "green untouched");
        assert!(close_px(out.get(2, 0), img.get(2, 0), 1e-4), "grey untouched");
        // Hue vs Hue: shift reds by +1/6 turn (y = 0.5 + 60/360) towards yellow.
        let out = run("ec.color.lumetri", &[("curves/hueSaturationCurves/hueVsHue", s("0,0.6667 0.2,0.5 0.8,0.5"))], img.clone());
        let p = out.get(0, 0);
        assert!(p[1] > 0.6 && p[0] > 0.6 && p[2] < 0.2, "red became yellow: {p:?}");
        // Hue vs Luma brightens greens only.
        let out = run("ec.color.lumetri", &[("curves/hueSaturationCurves/hueVsLuma", s("0.333,0.7 0.1,0.5 0.6,0.5"))], img.clone());
        assert!(out.get(1, 0)[1] > 0.85 && close_px(out.get(0, 0), img.get(0, 0), 1e-3));
        // Luma vs Sat / Sat vs Sat: y = 0 everywhere removes all saturation.
        for id in ["lumaVsSat", "satVsSat"] {
            let out = run("ec.color.lumetri", &[(&format!("curves/hueSaturationCurves/{id}"), s("0,0 1,0"))], img.clone());
            let p = out.get(0, 0);
            assert!((p[0] - p[1]).abs() < 1e-3 && (p[1] - p[2]).abs() < 1e-3, "{id}: {p:?}");
        }
    }

    #[test]
    fn hue_saturation_curve_stays_between_its_points() {
        // #1656: a point dragged to the bottom right next to another one. The steep short
        // segment used to pull the long one beside it far outside the box (|offset| > 0.5).
        // The third pair sits across the hue wrap (0.98 and 0.005 are neighbours).
        let cases = [("0.3,0.4 0.31,0 0.7,0.6", true), ("0.1,0.3 0.3,0.5 0.31,1 0.7,0.5", false), ("0.5,0.6 0.98,0.4 0.005,0", true)];
        for (pts, periodic) in cases {
            let c = OffsetCurve::parse(pts, periodic).unwrap();
            for i in 0..=1000 {
                let x = i as f32 / 1000.0;
                let v = c.at(x);
                assert!((-0.5..=0.5).contains(&v), "{pts} at {x}: {v}");
            }
            for p in parse_points(pts) {
                assert!((c.at(p.0) - (p.1 - 0.5)).abs() < 0.02, "{pts}: passes through {p:?}");
            }
            if periodic {
                assert!((c.at(0.0) - c.at(1.0)).abs() < 1e-4, "{pts}: no seam at the wrap");
            }
        }
    }

    fn close_px(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
    }

    #[test]
    fn lumetri_hsl_secondary_keys_and_corrects() {
        let img = swatches();
        let key_red = [("hslSecondary/key/hueCenter", num(0.0)), ("hslSecondary/key/hueRange", num(20.0)), ("hslSecondary/key/hueSoftness", num(10.0))];
        let mut vals: Vec<(&str, Value)> = key_red.to_vec();
        vals.push(("hslSecondary/correction/saturation", num(0.0)));
        let out = run("ec.color.lumetri", &vals, img.clone());
        let r = out.get(0, 0);
        assert!((r[0] - r[1]).abs() < 1e-3, "red desaturated through the key: {r:?}");
        assert!(close_px(out.get(1, 0), img.get(1, 0), 1e-5) && close_px(out.get(2, 0), img.get(2, 0), 1e-5));
        // Show Mask (White/Black) shows the key; Invert Mask flips it.
        let mut vals: Vec<(&str, Value)> = key_red.to_vec();
        vals.extend([("hslSecondary/key/showMask", Value::Bool(true)), ("hslSecondary/key/maskStyle", Value::Enum(2))]);
        let m = run("ec.color.lumetri", &vals, img.clone());
        assert_eq!([m.get(0, 0)[0], m.get(1, 0)[0], m.get(2, 0)[0]], [1.0, 0.0, 0.0]);
        vals.push(("hslSecondary/key/invertMask", Value::Bool(true)));
        let m = run("ec.color.lumetri", &vals, img.clone());
        assert_eq!([m.get(0, 0)[0], m.get(1, 0)[0], m.get(2, 0)[0]], [0.0, 1.0, 1.0]);
        // Lightness band keys only the bright grey; Blur softens the matte across pixels.
        let vals = [
            ("hslSecondary/key/lightnessLow", num(40.0)),
            ("hslSecondary/key/lightnessHigh", num(60.0)),
            ("hslSecondary/key/showMask", Value::Bool(true)),
            ("hslSecondary/key/maskStyle", Value::Enum(2)),
        ];
        let m = run("ec.color.lumetri", &vals, img.clone());
        assert_eq!(m.get(2, 0)[0], 1.0);
        let mut wide = Image::new(32, 1);
        for x in 0..32 {
            wide.set(x, 0, if x < 16 { [0.8, 0.1, 0.1, 1.0] } else { [0.1, 0.8, 0.1, 1.0] });
        }
        let mut vals: Vec<(&str, Value)> = key_red.to_vec();
        vals.extend([
            ("hslSecondary/key/showMask", Value::Bool(true)),
            ("hslSecondary/key/maskStyle", Value::Enum(2)),
            ("hslSecondary/refine/blur", num(50.0)),
        ]);
        let m = run("ec.color.lumetri", &vals, wide);
        let v = m.get(16, 0)[0];
        assert!(v > 0.05 && v < 0.95, "blurred matte edge {v}");
    }

    #[test]
    fn lumetri_hdr_range_and_looks() {
        // HDR White 200 nits: the tone controls see 1.6 as 0.8, so Highlights act on it.
        let bright = Image::filled(2, 2, [1.6, 1.6, 1.6, 1.0]);
        let sdr = run("ec.color.lumetri", &[("basicCorrection/tone/highlights", num(-100.0))], bright.clone()).get(0, 0)[0];
        let hdr = run(
            "ec.color.lumetri",
            &[("highDynamicRange", Value::Bool(true)), ("basicCorrection/tone/hdrWhite", num(200.0)), ("basicCorrection/tone/highlights", num(-100.0))],
            bright.clone(),
        )
        .get(0, 0)[0];
        assert!(hdr < 1.6 - 0.05 && sdr < 1.6, "{sdr} {hdr}");
        let spec = run("ec.color.lumetri", &[("highDynamicRange", Value::Bool(true)), ("basicCorrection/tone/hdrSpecular", num(100.0))], bright).get(0, 0)[0];
        assert!((spec - 2.2).abs() < 1e-4, "{spec}");
        // Built-in looks change the image at Intensity; 0 % leaves it.
        let img = ramp();
        let mono = run("ec.color.lumetri", &[("creative/look", Value::Enum(7))], img.clone());
        let p = mono.get(9, 3);
        assert!((p[0] - p[1]).abs() < 1e-5 && (p[1] - p[2]).abs() < 1e-5);
        let none = run("ec.color.lumetri", &[("creative/look", Value::Enum(7)), ("creative/lookIntensity", num(0.0))], img.clone());
        assert!(close(&none, &img, 1e-5));
        // A Custom look from inline .cube text (swap red and blue).
        let mut cube = String::from("LUT_3D_SIZE 2\n");
        for bb in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    cube += &format!("{bb} {g} {r}\n");
                }
            }
        }
        let g = Image::filled(1, 1, [0.9, 0.2, 0.1, 1.0]);
        let o = run("ec.color.lumetri", &[("creative/look", Value::Enum(1)), ("creative/lookFile", Value::Str(cube.clone()))], g.clone()).get(0, 0);
        assert!((o[0] - 0.1).abs() < 1e-3 && (o[2] - 0.9).abs() < 1e-3, "{o:?}");
        let o = run("ec.color.lumetri", &[("basicCorrection/inputLut", Value::Enum(1)), ("basicCorrection/inputLutFile", Value::Str(cube))], g).get(0, 0);
        assert!((o[0] - 0.1).abs() < 1e-3, "{o:?}");
    }
}
