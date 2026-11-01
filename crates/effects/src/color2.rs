//! Color Correction, batch 2: Curves, Levels (Individual Controls), automatic corrections
//! (Auto Levels / Contrast / Color, Equalize), Shadow/Highlight, Selective Color, Color Balance
//! (HLS), Color Link, Broadcast Colors and the CC toners/offsets/kernel.

use effectcraft_color::{BlendMode, blend_pixel, hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, lerp3, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Color Correction", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(-100.0, 100.0, -100.0, 100.0, 1)
}

/// Map straight colour + alpha of every pixel (row-parallel).
fn map_ca(img: &mut Image, f: impl Fn([f32; 3], f32) -> ([f32; 3], f32) + Sync) {
    img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 && px[0] == 0.0 && px[1] == 0.0 && px[2] == 0.0 {
            // Fully transparent: only alpha transforms can change it.
            let (_, na) = f([0.0; 3], 0.0);
            if na > 0.0 {
                *px = [0.0, 0.0, 0.0, na.clamp(0.0, 1.0)];
            }
            return;
        }
        let (nc, na) = f(c, a);
        *px = premul(nc, na.clamp(0.0, 1.0));
    });
}

// ---------------------------------------------------------------------------------------------
// Curves

/// A tone curve through control points, interpolated with a monotone cubic (Fritsch–Carlson
/// 1980) Hermite spline and extended linearly outside the first/last point.
#[derive(Clone, Debug)]
pub struct Curve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
    lut: Vec<f32>,
}

const LUT_N: usize = 1024;

impl Curve {
    /// Parse "x,y x,y …" (separators: whitespace or ';'). Returns None for an identity curve or
    /// fewer than two valid points.
    pub fn parse(s: &str) -> Option<Curve> {
        Self::parse_with(s, false)
    }

    /// [`Curve::parse`] interpolated as After Effects' Curves effect does: a natural cubic spline
    /// through the points, clamped to 0..1 (AE 2026: RGB points (0,0) (125,199) (255,255) and
    /// Red (0,0) (128,44) (255,255) map every grey step exactly as the natural spline does).
    pub fn parse_natural(s: &str) -> Option<Curve> {
        Self::parse_with(s, true)
    }

    fn parse_with(s: &str, natural: bool) -> Option<Curve> {
        let mut pts: Vec<(f32, f32)> = s
            .split(|c: char| c.is_whitespace() || c == ';')
            .filter(|t| !t.is_empty())
            .filter_map(|t| {
                let mut it = t.split(',');
                let x = it.next()?.trim().parse::<f32>().ok()?;
                let y = it.next()?.trim().parse::<f32>().ok()?;
                (x.is_finite() && y.is_finite()).then_some((x, y))
            })
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
        if pts.len() < 2 || pts.iter().all(|(x, y)| (x - y).abs() < 1e-6) {
            return None;
        }
        let xs: Vec<f32> = pts.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = pts.iter().map(|p| p.1).collect();
        let n = xs.len();
        let d: Vec<f32> = (0..n - 1).map(|k| (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k])).collect();
        if natural {
            let ms = natural_slopes(&xs, &d);
            let mut c = Curve { xs, ys, ms, lut: Vec::new() };
            c.lut = (0..=LUT_N).map(|i| c.eval_exact(i as f32 / LUT_N as f32).clamp(0.0, 1.0)).collect();
            return Some(c);
        }
        let mut ms = vec![0.0f32; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            ms[k] = if d[k - 1] * d[k] <= 0.0 { 0.0 } else { (d[k - 1] + d[k]) * 0.5 };
        }
        for k in 0..n - 1 {
            if d[k].abs() < 1e-12 {
                ms[k] = 0.0;
                ms[k + 1] = 0.0;
            } else {
                let a = ms[k] / d[k];
                let b = ms[k + 1] / d[k];
                let s = a * a + b * b;
                if s > 9.0 {
                    let t = 3.0 / s.sqrt();
                    ms[k] = t * a * d[k];
                    ms[k + 1] = t * b * d[k];
                }
            }
        }
        let mut c = Curve { xs, ys, ms, lut: Vec::new() };
        c.lut = (0..=LUT_N).map(|i| c.eval_exact(i as f32 / LUT_N as f32)).collect();
        Some(c)
    }

    fn eval_exact(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0] + (x - self.xs[0]) * self.ms[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1] + (x - self.xs[n - 1]) * self.ms[n - 1];
        }
        let k = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n - 2);
        let h = self.xs[k + 1] - self.xs[k];
        let t = (x - self.xs[k]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * self.ys[k] + h10 * h * self.ms[k] + h01 * self.ys[k + 1] + h11 * h * self.ms[k + 1]
    }

    /// Control points (x, y), Hermite tangents and the 1025-entry table over 0..1 that
    /// [`Curve::eval`] interpolates (the GPU path uploads them).
    pub fn tables(&self) -> (&[f32], &[f32], &[f32], &[f32]) {
        (&self.xs, &self.ys, &self.ms, &self.lut)
    }

    pub fn eval(&self, x: f32) -> f32 {
        if !(0.0..=1.0).contains(&x) {
            return self.eval_exact(x);
        }
        let f = x * LUT_N as f32;
        let i = (f as usize).min(LUT_N - 1);
        let t = f - i as f32;
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * t
    }
}

/// Knot slopes of the natural cubic spline (zero curvature at both ends) through points with
/// abscissae `xs` and segment slopes `d`; as Hermite tangents they reproduce the spline exactly.
fn natural_slopes(xs: &[f32], d: &[f32]) -> Vec<f32> {
    let n = xs.len();
    if n < 3 {
        return vec![d.first().copied().unwrap_or(0.0); n];
    }
    let h: Vec<f64> = (0..n - 1).map(|k| f64::from(xs[k + 1] - xs[k])).collect();
    let dd: Vec<f64> = d.iter().map(|&v| f64::from(v)).collect();
    // Second derivatives M[1..n-1] (M[0] = M[n-1] = 0): tridiagonal system, Thomas algorithm.
    let m = n - 2;
    let (mut a, mut b, mut c, mut r) = (vec![0.0; m], vec![0.0; m], vec![0.0; m], vec![0.0; m]);
    for i in 0..m {
        let k = i + 1;
        a[i] = h[k - 1];
        b[i] = 2.0 * (h[k - 1] + h[k]);
        c[i] = h[k];
        r[i] = 6.0 * (dd[k] - dd[k - 1]);
    }
    for i in 1..m {
        let w = a[i] / b[i - 1];
        b[i] -= w * c[i - 1];
        r[i] -= w * r[i - 1];
    }
    let mut sec = vec![0.0f64; n];
    for i in (0..m).rev() {
        let next = if i + 1 < m { sec[i + 2] } else { 0.0 };
        sec[i + 1] = (r[i] - c[i] * next) / b[i];
    }
    let mut ms: Vec<f32> = (0..n - 1).map(|k| (dd[k] - h[k] * (2.0 * sec[k] + sec[k + 1]) / 6.0) as f32).collect();
    ms.push((dd[n - 2] + h[n - 2] * (sec[n - 2] + 2.0 * sec[n - 1]) / 6.0) as f32);
    ms
}

fn ev(c: &Option<Curve>, v: f32) -> f32 {
    c.as_ref().map(|c| c.eval(v)).unwrap_or(v)
}

/// Curves. Each channel's curve is a hidden string parameter (`rgb`, `red`, `green`, `blue`,
/// `alpha`) holding control points as `"x,y x,y …"` in 0..1 (whitespace or `;` separated,
/// sorted on load); the default `"0,0 1,1"` is the identity. Colour channels go through the
/// master RGB curve first, then their own curve.
fn curves(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let [m, r, g, bl, a] = ["rgb", "red", "green", "blue", "alpha"].map(|id| Curve::parse_natural(ctx.params.s(id)));
    if m.is_none() && r.is_none() && g.is_none() && bl.is_none() && a.is_none() {
        return b;
    }
    let chans = [r, g, bl];
    map_ca(&mut b.img, |c, al| {
        let o = [0, 1, 2].map(|i| ev(&chans[i], ev(&m, c[i])));
        (o, ev(&a, al))
    });
    b
}

// ---------------------------------------------------------------------------------------------
// Levels (Individual Controls)

/// Levels (Individual Controls) parameter ids: each channel's controls sit in its own
/// twirl-down group (RGB, Red, Green, Blue, Alpha).
const LV_IDS: [[&str; 5]; 5] = [
    ["rgb/rgbInBlack", "rgb/rgbInWhite", "rgb/rgbGamma", "rgb/rgbOutBlack", "rgb/rgbOutWhite"],
    ["red/redInBlack", "red/redInWhite", "red/redGamma", "red/redOutBlack", "red/redOutWhite"],
    ["green/greenInBlack", "green/greenInWhite", "green/greenGamma", "green/greenOutBlack", "green/greenOutWhite"],
    ["blue/blueInBlack", "blue/blueInWhite", "blue/blueGamma", "blue/blueOutBlack", "blue/blueOutWhite"],
    ["alpha/alphaInBlack", "alpha/alphaInWhite", "alpha/alphaGamma", "alpha/alphaOutBlack", "alpha/alphaOutWhite"],
];
const LV_NAMES: [[&str; 5]; 5] = [
    ["Input Black", "Input White", "Gamma", "Output Black", "Output White"],
    ["Red Input Black", "Red Input White", "Red Gamma", "Red Output Black", "Red Output White"],
    ["Green Input Black", "Green Input White", "Green Gamma", "Green Output Black", "Green Output White"],
    ["Blue Input Black", "Blue Input White", "Blue Gamma", "Blue Output Black", "Blue Output White"],
    ["Alpha Input Black", "Alpha Input White", "Alpha Gamma", "Alpha Output Black", "Alpha Output White"],
];

#[derive(Clone, Copy)]
struct Lv {
    ib: f32,
    iw: f32,
    g: f32,
    ob: f32,
    ow: f32,
}

impl Lv {
    fn identity(&self) -> bool {
        self.ib == 0.0 && self.iw == 1.0 && self.g == 1.0 && self.ob == 0.0 && self.ow == 1.0
    }
    fn apply(&self, v: f32, (clip_b, clip_w): (bool, bool)) -> f32 {
        if self.identity() {
            return v;
        }
        let mut t = (v - self.ib) / (self.iw - self.ib).max(1e-6);
        if clip_b {
            t = t.max(0.0);
        }
        if clip_w {
            t = t.min(1.0);
        }
        self.ob + (self.ow - self.ob) * crate::color_fx::levels_gamma(t, self.g.max(0.01))
    }
}

fn levels_ic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let lv = LV_IDS.map(|ids| Lv {
        ib: ctx.params.f(ids[0]) as f32,
        iw: ctx.params.f(ids[1]) as f32,
        g: ctx.params.f(ids[2]) as f32,
        ob: ctx.params.f(ids[3]) as f32,
        ow: ctx.params.f(ids[4]) as f32,
    });
    if lv.iter().all(Lv::identity) {
        return b;
    }
    let clip = crate::color_fx::levels_clip(ctx);
    // Each channel's own controls first, then the RGB controls: After Effects 2026 with RGB
    // (0.1, 0.9, gamma 1.5, 0.05, 0.95) and Green Input White 0.8 maps grey 32/255 to green
    // 52/255 (RGB first would give 45).
    map_ca(&mut b.img, |c, a| ([0, 1, 2].map(|i| lv[0].apply(lv[i + 1].apply(c[i], clip), clip)), lv[4].apply(a, clip).clamp(0.0, 1.0)));
    b
}

// ---------------------------------------------------------------------------------------------
// Histograms and automatic corrections

const BINS: usize = 1024;

/// Alpha-weighted histograms of straight R, G, B and luminance.
fn histograms(img: &Image) -> [Vec<f64>; 4] {
    img.data
        .par_chunks(4096)
        .map(|chunk| {
            let mut h: [Vec<f64>; 4] = std::array::from_fn(|_| vec![0.0; BINS]);
            for &px in chunk {
                let (c, a) = unpremul(px);
                if a <= 1e-4 {
                    continue;
                }
                let bin = |v: f32| ((v.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize).min(BINS - 1);
                for k in 0..3 {
                    h[k][bin(c[k])] += a as f64;
                }
                h[3][bin(luminance(c[0], c[1], c[2]))] += a as f64;
            }
            h
        })
        .reduce(
            || std::array::from_fn(|_| vec![0.0; BINS]),
            |mut a, b| {
                for k in 0..4 {
                    for i in 0..BINS {
                        a[k][i] += b[k][i];
                    }
                }
                a
            },
        )
}

/// (low, high) values clipping `black`/`white` fractions of the histogram.
fn clip_points(h: &[f64], black: f64, white: f64) -> (f32, f32) {
    let total: f64 = h.iter().sum();
    if total <= 0.0 {
        return (0.0, 1.0);
    }
    let mut acc = 0.0;
    let mut lo = 0;
    for (i, v) in h.iter().enumerate() {
        acc += v;
        if acc > total * black {
            lo = i;
            break;
        }
    }
    acc = 0.0;
    let mut hi = BINS - 1;
    for (i, v) in h.iter().enumerate().rev() {
        acc += v;
        if acc > total * white {
            hi = i;
            break;
        }
    }
    let (lo, hi) = (lo as f32 / (BINS - 1) as f32, hi as f32 / (BINS - 1) as f32);
    if hi - lo < 1e-3 { (0.0, 1.0) } else { (lo, hi) }
}

/// Alpha-weighted mean (0..1) of a histogram.
fn hist_mean(h: &[f64]) -> f64 {
    let (mut s, mut w) = (0.0, 0.0);
    for (i, v) in h.iter().enumerate() {
        s += v * i as f64;
        w += v;
    }
    if w > 0.0 { s / w / (BINS - 1) as f64 } else { 0.5 }
}

/// The histograms the automatic corrections analyse: the current frame's, or with Temporal
/// Smoothing (seconds) the sum over the frames within that range on either side. Scene Detect
/// stops at a frame whose brightness jumps (a cut).
fn smoothed_histograms(ctx: &EffectCtx, img: &Image) -> [Vec<f64>; 4] {
    let mut h = histograms(img);
    let secs = ctx.params.f("temporalSmoothing");
    let Some(host) = ctx.env.host.filter(|_| secs > 0.0) else { return h };
    let fps = ctx.fps();
    let n = (secs * fps).round().clamp(0.0, 240.0) as i64;
    let scene = ctx.params.b("sceneDetect");
    let m0 = hist_mean(&h[3]);
    for dir in [-1i64, 1] {
        for k in 1..=n {
            let Some(nb) = host.self_at(ctx.time + (dir * k) as f64 / fps, ctx.env.effect_index) else { break };
            let hn = histograms(&nb.img);
            if scene && (hist_mean(&hn[3]) - m0).abs() > 0.15 {
                break;
            }
            for c in 0..4 {
                for i in 0..BINS {
                    h[c][i] += hn[c][i];
                }
            }
        }
    }
    h
}

fn stretch(v: f32, lo: f32, hi: f32) -> f32 {
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// 0 = Auto Levels (per channel), 1 = Auto Contrast (shared), 2 = Auto Color (per channel + neutral mids).
fn auto_correct(ctx: &EffectCtx, mut b: Buf, kind: u32) -> Buf {
    let blend = (ctx.params.f("blend") / 100.0).clamp(0.0, 1.0) as f32;
    let (ranges, gam) = auto_correct_settings(ctx, &b.img, kind);
    map_ca(&mut b.img, |c, a| {
        let o = [0, 1, 2].map(|k| stretch(c[k], ranges[k].0, ranges[k].1).powf(gam[k]));
        (lerp3(o, c, blend), a)
    });
    b
}

/// What Auto Levels (`kind` 0), Auto Contrast (1) and Auto Color (2) measure on `img`: each
/// channel's input range (low, high) and gamma. (The GPU effects apply them.)
pub fn auto_correct_settings(ctx: &EffectCtx, img: &Image, kind: u32) -> ([(f32, f32); 3], [f32; 3]) {
    let black = ctx.params.f("blackClip").clamp(0.0, 49.0) / 100.0;
    let white = ctx.params.f("whiteClip").clamp(0.0, 49.0) / 100.0;
    let h = smoothed_histograms(ctx, img);
    let ranges: [(f32, f32); 3] = if kind == 1 {
        let mut combined = vec![0.0; BINS];
        for k in 0..3 {
            for i in 0..BINS {
                combined[i] += h[k][i];
            }
        }
        let r = clip_points(&combined, black, white);
        [r; 3]
    } else {
        [0, 1, 2].map(|k| clip_points(&h[k], black, white))
    };
    // Auto Color: per-channel gamma that moves each channel's mean to the common mean.
    let mut gam = [1.0f32; 3];
    if kind == 2 && ctx.params.b("snapNeutralMidtones") {
        let mut sums = [0.0f64; 4];
        for px in &img.data {
            let (c, a) = unpremul(*px);
            if a > 1e-4 {
                for k in 0..3 {
                    sums[k] += (stretch(c[k], ranges[k].0, ranges[k].1) * a) as f64;
                }
                sums[3] += a as f64;
            }
        }
        if sums[3] > 0.0 {
            let means = [0, 1, 2].map(|k| (sums[k] / sums[3]) as f32);
            let target = (means[0] + means[1] + means[2]) / 3.0;
            for k in 0..3 {
                if means[k] > 1e-3 && means[k] < 0.999 && target > 1e-3 && target < 0.999 {
                    gam[k] = (target.ln() / means[k].ln()).clamp(0.2, 5.0);
                }
            }
        }
    }
    (ranges, gam)
}

fn auto_levels(ctx: &EffectCtx, b: Buf) -> Buf {
    auto_correct(ctx, b, 0)
}
fn auto_contrast(ctx: &EffectCtx, b: Buf) -> Buf {
    auto_correct(ctx, b, 1)
}
fn auto_color(ctx: &EffectCtx, b: Buf) -> Buf {
    auto_correct(ctx, b, 2)
}

fn cdf(h: &[f64]) -> Vec<f32> {
    let total: f64 = h.iter().sum::<f64>().max(1e-12);
    let mut acc = 0.0;
    h.iter()
        .map(|v| {
            acc += v;
            (acc / total) as f32
        })
        .collect()
}

/// Equalize's lookup tables (`BINS` entries each) for `img`: the R, G and B cumulative
/// histograms (Style RGB), the luminance one (Brightness) or the combined one (Photoshop Style).
pub fn equalize_tables(img: &Image, style: u32) -> Vec<Vec<f32>> {
    let h = histograms(img);
    match style {
        1 => vec![cdf(&h[3])],
        2 => {
            let mut combined = vec![0.0; BINS];
            for k in 0..3 {
                for i in 0..BINS {
                    combined[i] += h[k][i];
                }
            }
            vec![cdf(&combined)]
        }
        _ => vec![cdf(&h[0]), cdf(&h[1]), cdf(&h[2])],
    }
}

fn equalize(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amt = (ctx.params.f("amount") / 100.0).clamp(0.0, 1.0) as f32;
    if amt <= 0.0 {
        return b;
    }
    let style = ctx.params.e("style");
    let t = equalize_tables(&b.img, style);
    let look = |c: &Vec<f32>, v: f32| c[((v.clamp(0.0, 1.0) * (BINS - 1) as f32).round() as usize).min(BINS - 1)];
    match style {
        1 => {
            map_ca(&mut b.img, |col, a| {
                let l = luminance(col[0], col[1], col[2]);
                let nl = look(&t[0], l);
                let o = if l > 1e-5 { col.map(|v| v * nl / l) } else { [nl; 3] };
                (lerp3(col, o, amt), a)
            });
        }
        2 => {
            map_ca(&mut b.img, |col, a| (lerp3(col, col.map(|v| look(&t[0], v)), amt), a));
        }
        _ => {
            map_ca(&mut b.img, |col, a| (lerp3(col, [0, 1, 2].map(|k| look(&t[k], col[k])), amt), a));
        }
    }
    b
}

// ---------------------------------------------------------------------------------------------
// Shadow/Highlight

/// Shadow/Highlight's shadow and highlight amounts for `img` (Auto Amounts measures them).
pub fn shadow_highlight_amounts(ctx: &EffectCtx, img: &Image) -> (f32, f32) {
    let (mut s_amt, mut h_amt) = (ctx.params.f("shadowAmount") as f32 / 100.0, ctx.params.f("highlightAmount") as f32 / 100.0);
    if ctx.params.b("autoAmounts") {
        // Temporal Smoothing / Scene Detect average the analysis over neighbouring frames.
        let mean = if ctx.params.f("temporalSmoothing") > 0.0 { hist_mean(&smoothed_histograms(ctx, img)[3]) } else { hist_mean(&histograms(img)[3]) } as f32;
        s_amt = ((0.6 - mean) * 1.5).clamp(0.0, 1.0) * 0.8 + 0.1;
        h_amt = ((mean - 0.6) * 1.5).clamp(0.0, 1.0) * 0.8;
    }
    (s_amt, h_amt)
}

/// The luminance values clipping `black` / `white` fractions of `img`'s alpha-weighted
/// luminance histogram (Shadow/Highlight's Black / White Clip).
pub fn luma_clip_points(img: &Image, black: f64, white: f64) -> (f32, f32) {
    clip_points(&histograms(img)[3], black, white)
}

fn shadow_highlight(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (s_amt, h_amt) = shadow_highlight_amounts(ctx, &b.img);
    let luma = Plane::luma(&b.img);
    let (bc, wc) = (ctx.params.f("moreOptions/blackClip").clamp(0.0, 49.0) / 100.0, ctx.params.f("moreOptions/whiteClip").clamp(0.0, 49.0) / 100.0);
    let clip0 = (bc > 0.0 || wc > 0.0).then(|| luma_clip_points(&b.img, bc, wc));
    let s_tw = (ctx.params.f("moreOptions/shadowTonalWidth") as f32 / 100.0).max(0.01);
    let h_tw = (ctx.params.f("moreOptions/highlightTonalWidth") as f32 / 100.0).max(0.01);
    let s_r = ctx.params.f("moreOptions/shadowRadius").max(0.0) * b.scale / 2.0;
    let h_r = ctx.params.f("moreOptions/highlightRadius").max(0.0) * b.scale / 2.0;
    let cc = ctx.params.f("moreOptions/colorCorrection") as f32 / 100.0;
    let mc = ctx.params.f("moreOptions/midtoneContrast") as f32 / 100.0;
    let blend = (ctx.params.f("blend") / 100.0).clamp(0.0, 1.0) as f32;
    let base_s = gauss_plane(&luma, s_r, s_r);
    let base_h = if (h_r - s_r).abs() < 1e-9 { base_s.clone() } else { gauss_plane(&luma, h_r, h_r) };
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        let ws = (1.0 - base_s.data[i] / s_tw).clamp(0.0, 1.0).powi(2);
        let wh = ((base_h.data[i] - (1.0 - h_tw)) / h_tw).clamp(0.0, 1.0).powi(2);
        let gain = (1.0 + s_amt * ws * 2.0) * (1.0 - h_amt * wh * 0.5);
        let l = luminance(c[0], c[1], c[2]);
        let nl = l * gain;
        let sat = gain.max(1e-3).powf(cc);
        let mut o = c.map(|v| nl + (v - l) * sat);
        if mc != 0.0 {
            let shift = (0.5 + (nl - 0.5) * (1.0 + mc)) - nl;
            o = o.map(|v| v + shift);
        }
        let o = lerp3(o.map(|v| v.max(0.0)), c, blend);
        *px = premul(o, a);
    });
    // Black / White Clip: the adjusted image's extremes (ignoring those fractions of pixels)
    // are mapped back onto the original's, so lifting shadows or recovering highlights doesn't
    // flatten the ends of the tonal range.
    if let Some((lo0, hi0)) = clip0 {
        let (lo1, hi1) = luma_clip_points(&b.img, bc, wc);
        if (lo0, hi0) != (lo1, hi1) && hi1 - lo1 > 1e-3 {
            let k = (hi0 - lo0) / (hi1 - lo1);
            map_ca(&mut b.img, |c, a| (c.map(|v| (lo0 + (v - lo1) * k).max(0.0)), a));
        }
    }
    b
}

// ---------------------------------------------------------------------------------------------
// Selective Color

const SC_IDS: [[&str; 4]; 9] = [
    ["redsCyan", "redsMagenta", "redsYellow", "redsBlack"],
    ["yellowsCyan", "yellowsMagenta", "yellowsYellow", "yellowsBlack"],
    ["greensCyan", "greensMagenta", "greensYellow", "greensBlack"],
    ["cyansCyan", "cyansMagenta", "cyansYellow", "cyansBlack"],
    ["bluesCyan", "bluesMagenta", "bluesYellow", "bluesBlack"],
    ["magentasCyan", "magentasMagenta", "magentasYellow", "magentasBlack"],
    ["whitesCyan", "whitesMagenta", "whitesYellow", "whitesBlack"],
    ["neutralsCyan", "neutralsMagenta", "neutralsYellow", "neutralsBlack"],
    ["blacksCyan", "blacksMagenta", "blacksYellow", "blacksBlack"],
];
/// Selective Color's Colors popup (the range whose controls Effect Controls shows) and the
/// Details twirl-down group of each range.
pub const SC_COLORS: [&str; 9] = ["Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas", "Whites", "Neutrals", "Blacks"];
pub const SC_GROUPS: [&str; 9] = ["reds", "yellows", "greens", "cyans", "blues", "magentas", "whites", "neutrals", "blacks"];

/// Spec id of range `r`'s control `leaf` (`details/reds/redsCyan`).
fn sc_param_id(r: usize, leaf: &str) -> String {
    format!("details/{}/{leaf}", SC_GROUPS[r])
}

const SC_NAMES: [[&str; 4]; 9] = [
    ["Reds Cyan", "Reds Magenta", "Reds Yellow", "Reds Black"],
    ["Yellows Cyan", "Yellows Magenta", "Yellows Yellow", "Yellows Black"],
    ["Greens Cyan", "Greens Magenta", "Greens Yellow", "Greens Black"],
    ["Cyans Cyan", "Cyans Magenta", "Cyans Yellow", "Cyans Black"],
    ["Blues Cyan", "Blues Magenta", "Blues Yellow", "Blues Black"],
    ["Magentas Cyan", "Magentas Magenta", "Magentas Yellow", "Magentas Black"],
    ["Whites Cyan", "Whites Magenta", "Whites Yellow", "Whites Black"],
    ["Neutrals Cyan", "Neutrals Magenta", "Neutrals Yellow", "Neutrals Black"],
    ["Blacks Cyan", "Blacks Magenta", "Blacks Yellow", "Blacks Black"],
];

/// How much a colour belongs to each Selective Color range.
fn sc_weights(c: [f32; 3]) -> [f32; 9] {
    let c = c.map(|v| v.clamp(0.0, 1.0));
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[b].total_cmp(&c[a]));
    let (mx, md, mn) = (c[idx[0]], c[idx[1]], c[idx[2]]);
    let mut w = [0.0f32; 9];
    // Primaries: weight = max − mid when that primary is the largest component.
    w[[0, 2, 4][idx[0]]] = mx - md;
    // Secondaries: weight = mid − min when the complementary primary is the smallest.
    w[[3, 5, 1][idx[2]]] = md - mn;
    w[6] = ((mn - 0.5) * 2.0).max(0.0);
    w[8] = ((0.5 - mx) * 2.0).max(0.0);
    w[7] = (1.0 - ((mx - 0.5).abs() + (mn - 0.5).abs())).clamp(0.0, 1.0);
    w
}

fn selective_color(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let adj: [[f32; 4]; 9] = std::array::from_fn(|r| SC_IDS[r].map(|id| ctx.params.f(&sc_param_id(r, id)) as f32 / 100.0));
    if adj.iter().all(|r| r.iter().all(|v| *v == 0.0)) {
        return b;
    }
    let relative = ctx.params.e("method") == 0;
    map_ca(&mut b.img, |c, a| {
        let w = sc_weights(c);
        let mut o = c;
        for (r, wr) in w.iter().enumerate() {
            if *wr <= 0.0 {
                continue;
            }
            let k = adj[r][3];
            for i in 0..3 {
                let ink = 1.0 - c[i];
                let delta = adj[r][i] + k;
                let delta = if relative { delta * ink.max(0.0) } else { delta };
                o[i] -= wr * delta;
            }
        }
        (o.map(|v| v.max(0.0)), a)
    });
    b
}

// ---------------------------------------------------------------------------------------------
// Color Balance (HLS), Color Link, Broadcast Colors

fn color_balance_hls(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let dh = (ctx.params.f("hue") / 360.0) as f32;
    let dl = (ctx.params.f("lightness") / 100.0) as f32;
    let ds = (ctx.params.f("saturation") / 100.0) as f32;
    if dh == 0.0 && dl == 0.0 && ds == 0.0 {
        return b;
    }
    b.img.map_straight(|c| {
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let l = if dl > 0.0 { l + (1.0 - l) * dl } else { l + l * dl };
        let s = if ds > 0.0 { s + (1.0 - s) * ds } else { s + s * ds };
        let (r, g, bb) = hsl_to_rgb(h + dh, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
        [r, g, bb]
    });
    b
}

const LINK_MODES: [(&str, BlendMode); 13] = [
    ("Normal", BlendMode::Normal),
    ("Add", BlendMode::Add),
    ("Multiply", BlendMode::Multiply),
    ("Screen", BlendMode::Screen),
    ("Overlay", BlendMode::Overlay),
    ("Soft Light", BlendMode::SoftLight),
    ("Hard Light", BlendMode::HardLight),
    ("Darken", BlendMode::Darken),
    ("Lighten", BlendMode::Lighten),
    ("Difference", BlendMode::Difference),
    ("Hue", BlendMode::Hue),
    ("Color", BlendMode::Color),
    ("Luminosity", BlendMode::Luminosity),
];

/// Color Link's Sample options.
pub const LINK_SAMPLES: [&str; 10] =
    ["Average", "Median", "Brightest", "Darkest", "Max RGB", "Min RGB", "Average Alpha", "Median Alpha", "Max Alpha", "Min Alpha"];

/// Element of sorted `v` at fraction `q`.
fn quantile(v: &[f32], q: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v[((q * (v.len() - 1) as f64).round() as usize).min(v.len() - 1)]
}

/// Mean of `v` without the fraction `clip` at each end.
fn trimmed_mean(v: &mut [f32], clip: f64) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f32::total_cmp);
    let cut = (v.len() as f64 * clip) as usize;
    let s = &v[cut..v.len() - cut];
    if s.is_empty() { quantile(v, 0.5) } else { (s.iter().map(|x| *x as f64).sum::<f64>() / s.len() as f64) as f32 }
}

/// Representative colour (RGB) and alpha of an image for a Sample option (see [`LINK_SAMPLES`]),
/// ignoring the fraction `clip` of pixels at each extreme. Fully transparent pixels are skipped
/// for the colour samples.
fn sample_color(img: &Image, how: u32, clip: f64) -> ([f32; 3], f32) {
    let clip = clip.clamp(0.0, 0.49);
    let alpha_mode = how >= 6;
    let mut ch: [Vec<f32>; 3] = Default::default();
    let mut lum: Vec<(f32, [f32; 3])> = vec![];
    let mut alphas: Vec<f32> = Vec::with_capacity(img.data.len());
    let mut wsum = [0.0f64; 4];
    for px in &img.data {
        let (c, a) = unpremul(*px);
        alphas.push(a);
        if alpha_mode || a <= 1e-3 {
            continue;
        }
        for k in 0..3 {
            ch[k].push(c[k]);
        }
        lum.push((luminance(c[0], c[1], c[2]), c));
        for k in 0..3 {
            wsum[k] += (c[k] * a) as f64;
        }
        wsum[3] += a as f64;
    }
    if alpha_mode {
        alphas.sort_by(f32::total_cmp);
        let a = match how {
            7 => quantile(&alphas, 0.5),
            8 => quantile(&alphas, 1.0 - clip),
            9 => quantile(&alphas, clip),
            _ => trimmed_mean(&mut alphas, clip),
        };
        return ([0.0; 3], a);
    }
    if lum.is_empty() {
        return ([0.0; 3], 1.0);
    }
    let c = match how {
        1 => ch.each_mut().map(|v| {
            v.sort_by(f32::total_cmp);
            quantile(v, 0.5)
        }),
        2 | 3 => {
            lum.sort_by(|a, b| a.0.total_cmp(&b.0));
            let q = if how == 2 { 1.0 - clip } else { clip };
            lum[((q * (lum.len() - 1) as f64).round() as usize).min(lum.len() - 1)].1
        }
        4 | 5 => ch.each_mut().map(|v| {
            v.sort_by(f32::total_cmp);
            quantile(v, if how == 4 { 1.0 - clip } else { clip })
        }),
        // Plain Average is alpha-weighted.
        0 if clip <= 0.0 => [0, 1, 2].map(|k| (wsum[k] / wsum[3]) as f32),
        _ => ch.each_mut().map(|v| trimmed_mean(v, clip)),
    };
    (c, 1.0)
}

/// Color Link: colours the layer with a colour sampled from the Source Layer (its source, no
/// masks or effects) or, with no source layer, from the layer itself.
/// Color Link's sampled colour (RGB) and alpha: from the Source Layer, or from `img` (the
/// layer's own pixels; `None` = the caller has not read them back) when no source is chosen.
/// `None` when the own pixels are needed but not given.
pub fn color_link_sample(ctx: &EffectCtx, img: Option<&Image>) -> Option<([f32; 3], f32)> {
    let how = ctx.params.e("sampleSource");
    let clip = ctx.params.f("clip") / 100.0;
    Some(match ctx.layer_param("sourceLayer", false) {
        Some(lp) => sample_color(&lp.buf.img, how, clip),
        None => sample_color(img?, how, clip),
    })
}

/// Color Link's Blending Mode.
pub fn color_link_mode(ctx: &EffectCtx) -> BlendMode {
    LINK_MODES[(ctx.params.e("blendingMode") as usize).min(LINK_MODES.len() - 1)].1
}

fn color_link(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let how = ctx.params.e("sampleSource");
    let Some((c, sa)) = color_link_sample(ctx, Some(&b.img)) else { return b };
    let op = (ctx.params.f("opacity") / 100.0).clamp(0.0, 1.0) as f32;
    let stencil = ctx.params.b("stencilOriginalAlpha");
    if how >= 6 {
        // Alpha samples set the layer's alpha (blending modes don't apply).
        b.img.data.par_iter_mut().for_each(|px| {
            let (c0, a0) = unpremul(*px);
            let mut na = a0 + (sa - a0) * op;
            if stencil {
                na = na.min(a0);
            }
            *px = premul(c0, na.clamp(0.0, 1.0));
        });
        return b;
    }
    let mode = color_link_mode(ctx);
    let src = [c[0] * op, c[1] * op, c[2] * op, op];
    b.img.data.par_iter_mut().for_each(|px| {
        let a0 = px[3];
        let mut o = blend_pixel(mode, *px, src, 0.5);
        o[3] = o[3].clamp(0.0, 1.0);
        if stencil {
            let (sc, _) = unpremul(o);
            o = premul(sc, a0);
        }
        *px = o;
    });
    b
}

fn broadcast_colors(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pal = ctx.params.e("locale") == 1;
    let how = ctx.params.e("howToMakeSafe");
    let limit = ctx.params.f("maxSignal") as f32;
    let (setup, span) = if pal { (0.0, 100.0) } else { (7.5, 92.5) };
    let chroma = |c: [f32; 3]| -> (f32, f32) {
        let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
        let ch = if pal {
            let (u, v) = (0.492 * (c[2] - y), 0.877 * (c[0] - y));
            (u * u + v * v).sqrt()
        } else {
            let i = 0.596 * c[0] - 0.274 * c[1] - 0.322 * c[2];
            let q = 0.211 * c[0] - 0.523 * c[1] + 0.312 * c[2];
            (i * i + q * q).sqrt()
        };
        (y, ch)
    };
    let max_norm = (limit - setup) / span;
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        let (y, ch) = chroma(c);
        let unsafe_ = y + ch > max_norm + 1e-5;
        match how {
            2 => {
                if unsafe_ {
                    *px = [0.0; 4];
                }
            }
            3 => {
                if !unsafe_ {
                    *px = [0.0; 4];
                }
            }
            1 => {
                if unsafe_ {
                    let target = (max_norm - y).max(0.0);
                    let s = if ch > 1e-6 { target / ch } else { 1.0 };
                    let mut o = c.map(|v| y + (v - y) * s);
                    if y > max_norm {
                        o = o.map(|v| v * max_norm / y);
                    }
                    *px = premul(o, a);
                }
            }
            _ => {
                if unsafe_ {
                    let k = max_norm / (y + ch);
                    *px = premul(c.map(|v| v * k), a);
                }
            }
        }
    });
    b
}

// ---------------------------------------------------------------------------------------------
// CC Toner, CC Color Offset, CC Kernel

fn cc_toner(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let g = |id: &str| {
        let c = ctx.params.color(id);
        [c[0], c[1], c[2]]
    };
    let stops: Vec<[f32; 3]> = match ctx.params.e("tones") {
        0 => vec![g("shadows"), g("highlights")],
        2 => vec![g("shadows"), g("darktones"), g("midtones"), g("brights"), g("highlights")],
        _ => vec![g("shadows"), g("midtones"), g("highlights")],
    };
    let blend = (ctx.params.f("blend") / 100.0).clamp(0.0, 1.0) as f32;
    let n = stops.len() - 1;
    b.img.map_straight(|c| {
        let t = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) * n as f32;
        let k = (t.floor() as usize).min(n - 1);
        let tone = lerp3(stops[k], stops[k + 1], t - k as f32);
        lerp3(tone, c, blend)
    });
    b
}

fn cc_color_offset(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let offs = ["redPhase", "greenPhase", "bluePhase"].map(|id| (ctx.params.f(id) / 360.0).rem_euclid(1.0) as f32);
    if offs.iter().all(|o| *o == 0.0) {
        return b;
    }
    let overflow = ctx.params.e("overflow");
    b.img.map_straight(|c| {
        [0, 1, 2].map(|k| {
            if offs[k] == 0.0 {
                return c[k];
            }
            let x = c[k] + offs[k];
            if x <= 1.0 {
                return x;
            }
            match overflow {
                1 => (2.0 - x).max(0.0),
                2 => {
                    if x - 1.0 < 0.5 {
                        0.0
                    } else {
                        1.0
                    }
                }
                _ => x.rem_euclid(1.0),
            }
        })
    });
    b
}

const K_IDS: [&str; 9] = ["k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"];
const K_NAMES: [&str; 9] = ["Top Left", "Top", "Top Right", "Left", "Center", "Right", "Bottom Left", "Bottom", "Bottom Right"];

fn cc_kernel(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = K_IDS.map(|id| ctx.params.f(id) as f32);
    let scale = ctx.params.f("scale") as f32;
    let offset = ctx.params.f("offset") as f32;
    let abs = ctx.params.b("absolute");
    let identity = k.iter().enumerate().all(|(i, v)| if i == 4 { *v == 1.0 } else { *v == 0.0 });
    if identity && scale == 1.0 && offset == 0.0 {
        return b;
    }
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = px[3];
            if a <= 0.0 {
                continue;
            }
            let mut acc = [0.0f32; 3];
            for j in 0..3 {
                for i in 0..3 {
                    let w = k[j * 3 + i];
                    if w == 0.0 {
                        continue;
                    }
                    let q = src.get_clamped(x as i64 + i as i64 - 1, y as i64 + j as i64 - 1);
                    let (c, _) = unpremul(q);
                    for ch in 0..3 {
                        acc[ch] += c[ch] * w;
                    }
                }
            }
            let o = acc.map(|v| {
                let v = v * scale + offset;
                (if abs { v.abs() } else { v }).max(0.0)
            });
            *px = premul(o, a);
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let mut lv_params = Vec::new();
    for (ids, names) in LV_IDS.iter().zip(LV_NAMES.iter()) {
        lv_params.push(p(ids[0], names[0], num(0.0), slider(-1.0, 2.0, 0.0, 1.0, 3)));
        lv_params.push(p(ids[1], names[1], num(1.0), slider(-1.0, 2.0, 0.0, 1.0, 3)));
        lv_params.push(p(ids[2], names[2], num(1.0), slider(0.1, 10.0, 0.1, 3.0, 2)));
        lv_params.push(p(ids[3], names[3], num(0.0), slider(-1.0, 2.0, 0.0, 1.0, 3)));
        lv_params.push(p(ids[4], names[4], num(1.0), slider(-1.0, 2.0, 0.0, 1.0, 3)));
    }
    lv_params.push(p("clipToOutputBlack", "Clip To Output Black", Value::Enum(2), popup(&crate::color_fx::LEVELS_CLIP)));
    lv_params.push(p("clipToOutputWhite", "Clip To Output White", Value::Enum(2), popup(&crate::color_fx::LEVELS_CLIP)));
    let mut sc_params = vec![p("method", "Method", Value::Enum(0), popup(&["Relative", "Absolute"])), p("colors", "Colors", Value::Enum(0), popup(&SC_COLORS))];
    // Details: every range's Cyan / Magenta / Yellow / Black in its own twirl-down.
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    for (r, (ids, names)) in SC_IDS.iter().zip(SC_NAMES.iter()).enumerate() {
        for k in 0..4 {
            sc_params.push(p(leak(sc_param_id(r, ids[k])), names[k], num(0.0), pct()));
        }
    }
    let clip_params = |extra: bool| {
        let mut v = vec![
            p("temporalSmoothing", "Temporal Smoothing (seconds)", num(0.0), slider(0.0, 4.0, 0.0, 4.0, 2)),
            p("sceneDetect", "Scene Detect", Value::Bool(false), ParamUi::Checkbox),
            p("blackClip", "Black Clip", num(0.1), slider(0.0, 49.0, 0.0, 10.0, 2)),
            p("whiteClip", "White Clip", num(0.1), slider(0.0, 49.0, 0.0, 10.0, 2)),
            p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
        ];
        if extra {
            v.insert(4, p("snapNeutralMidtones", "Snap Neutral Midtones", Value::Bool(true), ParamUi::Checkbox));
        }
        v
    };
    let mut k_params: Vec<crate::ParamSpec> =
        K_IDS.iter().zip(K_NAMES.iter()).map(|(id, n)| p(id, n, num(if *id == "k5" { 1.0 } else { 0.0 }), slider(-100.0, 100.0, -10.0, 10.0, 2))).collect();
    k_params.push(p("scale", "Scale", num(1.0), slider(-100.0, 100.0, 0.0, 4.0, 3)));
    k_params.push(p("offset", "Offset", num(0.0), slider(-1.0, 1.0, -1.0, 1.0, 3)));
    k_params.push(p("absolute", "Absolute Value", Value::Bool(false), ParamUi::Checkbox));
    let curve = |id: &'static str, name: &'static str| p(id, name, Value::Str("0,0 1,1".into()), ParamUi::Hidden);
    vec![
        spec(
            "ec.color.curves",
            "Curves",
            vec![
                // The channel the graph edits (the Effect Controls graph's Channel popup).
                p("channel", "Channel", Value::Enum(0), ParamUi::Hidden),
                curve("rgb", "RGB"),
                curve("red", "Red"),
                curve("green", "Green"),
                curve("blue", "Blue"),
                curve("alpha", "Alpha"),
            ],
            curves,
        ),
        spec("ec.color.levelsic", "Levels (Individual Controls)", lv_params, levels_ic),
        spec("ec.color.autolevels", "Auto Levels", clip_params(false), auto_levels),
        spec("ec.color.autocontrast", "Auto Contrast", clip_params(false), auto_contrast),
        spec("ec.color.autocolor", "Auto Color", clip_params(true), auto_color),
        spec(
            "ec.color.equalize",
            "Equalize",
            vec![
                p("style", "Equalize", Value::Enum(0), popup(&["RGB", "Brightness", "Photoshop Style"])),
                p("amount", "Amount to Equalize", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            equalize,
        ),
        spec(
            "ec.color.shadowhighlight",
            "Shadow/Highlight",
            vec![
                p("autoAmounts", "Auto Amounts", Value::Bool(true), ParamUi::Checkbox),
                p("shadowAmount", "Shadow Amount", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("highlightAmount", "Highlight Amount", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("temporalSmoothing", "Temporal Smoothing (seconds)", num(0.0), slider(0.0, 4.0, 0.0, 4.0, 2)),
                p("sceneDetect", "Scene Detect", Value::Bool(false), ParamUi::Checkbox),
                p("moreOptions/shadowTonalWidth", "Shadow Tonal Width", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("moreOptions/shadowRadius", "Shadow Radius", num(30.0), slider(0.0, 2500.0, 0.0, 100.0, 0)),
                p("moreOptions/highlightTonalWidth", "Highlight Tonal Width", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("moreOptions/highlightRadius", "Highlight Radius", num(30.0), slider(0.0, 2500.0, 0.0, 100.0, 0)),
                p("moreOptions/colorCorrection", "Color Correction", num(20.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
                p("moreOptions/midtoneContrast", "Midtone Contrast", num(0.0), pct()),
                p("moreOptions/blackClip", "Black Clip", num(0.01), slider(0.0, 49.0, 0.0, 10.0, 2)),
                p("moreOptions/whiteClip", "White Clip", num(0.01), slider(0.0, 49.0, 0.0, 10.0, 2)),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            shadow_highlight,
        ),
        spec("ec.color.selectivecolor", "Selective Color", sc_params, selective_color),
        spec(
            "ec.color.colorbalancehls",
            "Color Balance (HLS)",
            vec![p("hue", "Hue", num(0.0), ParamUi::Angle), p("lightness", "Lightness", num(0.0), pct()), p("saturation", "Saturation", num(0.0), pct())],
            color_balance_hls,
        ),
        spec(
            "ec.color.colorlink",
            "Color Link",
            vec![
                p("sourceLayer", "Source Layer", Value::Layer(None), ParamUi::Layer),
                // A chosen source layer is sampled without its masks and effects.
                p("sourceLayerSource", "Source Layer Source", Value::Enum(0), ParamUi::Hidden),
                p("sampleSource", "Sample", Value::Enum(0), popup(&LINK_SAMPLES)),
                p("clip", "Clip", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("stencilOriginalAlpha", "Stencil Original Alpha", Value::Bool(true), ParamUi::Checkbox),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&LINK_MODES.map(|m| m.0))),
            ],
            color_link,
        ),
        spec(
            "ec.color.broadcast",
            "Broadcast Colors",
            vec![
                p("locale", "Broadcast Locale", Value::Enum(0), popup(&["NTSC", "PAL"])),
                p(
                    "howToMakeSafe",
                    "How To Make Color Safe",
                    Value::Enum(0),
                    popup(&["Reduce Luminance", "Reduce Saturation", "Key Out Unsafe", "Key Out Safe"]),
                ),
                p("maxSignal", "Maximum Signal Amplitude (IRE)", num(110.0), slider(90.0, 120.0, 90.0, 120.0, 0)),
            ],
            broadcast_colors,
        ),
        spec(
            "ec.color.cctoner",
            "CC Toner",
            vec![
                p("tones", "Tones", Value::Enum(1), popup(&["Duotone", "Tritone", "Pentone"])),
                p("highlights", "Highlights", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("brights", "Brights", col(0.86, 0.78, 0.64), ParamUi::Color),
                p("midtones", "Midtones", col(0.55, 0.43, 0.28), ParamUi::Color),
                p("darktones", "Darktones", col(0.26, 0.18, 0.1), ParamUi::Color),
                p("shadows", "Shadows", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("blend", "Blend w. Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            cc_toner,
        ),
        spec(
            "ec.color.cccoloroffset",
            "CC Color Offset",
            vec![
                p("redPhase", "Red Phase", num(0.0), ParamUi::Angle),
                p("greenPhase", "Green Phase", num(0.0), ParamUi::Angle),
                p("bluePhase", "Blue Phase", num(0.0), ParamUi::Angle),
                p("overflow", "Overflow", Value::Enum(0), popup(&["Wrap", "Solarize", "Polarize"])),
            ],
            cc_color_offset,
        ),
        spec("ec.color.cckernel", "CC Kernel", k_params, cc_kernel),
    ]
}

#[cfg(test)]
mod tests;
