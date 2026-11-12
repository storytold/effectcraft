//! Color Correction effects.

use effectcraft_color::{hsl_to_rgb, luminance, rgb_to_hsl};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;

use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Color Correction", params, render, gpu: false, float: true }
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Rec. 601 luminance on the working-space values (sRGB working space, not linearised: red
/// 76/255, green 150/255, blue 29/255), as After Effects 2026 renders Tint, Tritone, Luma Key and
/// Extract. Tritone, case 718: a red solid with Blend With Original 25 gives (132, 23, 46), from
/// luminance 0.299; Rec. 709's 0.213 gave (113, 16, 49); on a grey ramp both weightings agree
/// (case 815). Luma Key and Extract: Key Out Darker / Extract Black Point at 65, 166 and 24 on
/// pure red, green and blue keep red (601: 76, 709: 54) and blue (29 / 18) and key out green
/// (150 / 182). The GPU kernels (`pointwise` op 1, `fx_color` op 4, `fxk_key_luma`) mirror it.
pub(crate) fn luma_601(c: [f32; 3]) -> f32 {
    LUMA_601[0] * c[0] + LUMA_601[1] * c[1] + LUMA_601[2] * c[2]
}

/// The Rec. 601 luma weights of `luma_601`. The WGSL kernels repeat them as literals; the GPU
/// test `shader_constants_match_the_cpu` checks that they are these.
pub const LUMA_601: [f32; 3] = [0.299, 0.587, 0.114];

fn tint(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let black = ctx.params.color("black");
    let white = ctx.params.color("white");
    let amt = ctx.params.f("amount") as f32 / 100.0;
    b.img.map_straight(|c| {
        let l = luma_601(c).clamp(0.0, 1.0);
        let t = [black[0] + (white[0] - black[0]) * l, black[1] + (white[1] - black[1]) * l, black[2] + (white[2] - black[2]) * l];
        mix(c, t, amt)
    });
    b
}

fn tritone(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let hi = ctx.params.color("highlights");
    let mid = ctx.params.color("midtones");
    let sh = ctx.params.color("shadows");
    let blend = 1.0 - ctx.params.f("blend") as f32 / 100.0;
    b.img.map_straight(|c| {
        let l = luma_601(c).clamp(0.0, 1.0);
        let t = if l < 0.5 {
            mix([sh[0], sh[1], sh[2]], [mid[0], mid[1], mid[2]], l * 2.0)
        } else {
            mix([mid[0], mid[1], mid[2]], [hi[0], hi[1], hi[2]], (l - 0.5) * 2.0)
        };
        mix(c, t, blend)
    });
    b
}

/// Brightness & Contrast without Use Legacy: pin black and white, then a mild S-curve.
///
/// Fitted to After Effects non-legacy grey ramps (#681, kasper9n oracles). Brightness is a
/// power toward white (`1-(1-v)^e`) or black (`v^e`); contrast uses `g = 1 + 0.53 c` instead of
/// the old `2^(2c)` (that crushed 0.25 to ~8/255 at Contrast 100; AE is ~40/255). Values above 1
/// pass through. The GPU kernel mirrors it.
///
/// Negative Brightness and Contrast are the inverse curves of the positive ones (After Effects
/// 2026 ramps: Brightness −40 is Brightness +40 inverted within 2.6/255, Contrast −50 likewise),
/// so they use the reciprocal exponents: Brightness −40 was 20/255 off AE, now 6.2.
pub fn bc_modern(v: f32, b: f32, c: f32) -> f32 {
    if v >= 1.0 {
        return v;
    }
    let mut v = v.max(0.0);
    if b != 0.0 {
        let e = 2f32.powf(1.24 * b.abs());
        v = 1.0 - (1.0 - v).powf(if b > 0.0 { e } else { 1.0 / e });
    }
    if c != 0.0 {
        let g = (1.0 + 0.53 * c.abs()).max(0.05);
        let g = if c > 0.0 { g } else { 1.0 / g };
        v = if v < 0.5 { 0.5 * (2.0 * v).powf(g) } else { 1.0 - 0.5 * (2.0 - 2.0 * v).max(0.0).powf(g) };
    }
    v
}

fn brightness_contrast(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let br = ctx.params.f("brightness") as f32 / 100.0;
    let ct = ctx.params.f("contrast") as f32 / 100.0;
    if br == 0.0 && ct == 0.0 {
        return b;
    }
    if ctx.params.b("useLegacy") {
        let k = if ct >= 0.0 { 1.0 / (1.0 - ct * 0.99) } else { 1.0 + ct };
        b.img.map_straight(|c| c.map(|v| ((v + br - 0.5) * k + 0.5).max(0.0)));
    } else {
        b.img.map_straight(|c| c.map(|v| bc_modern(v, br, ct)));
    }
    b
}

/// Hue/Saturation's colour ranges (Channel Control entries after Master): id prefix, display
/// name and centre hue in degrees.
pub const HUESAT_RANGES: [(&str, &str, f64); 6] = [
    ("reds", "Reds", 0.0),
    ("yellows", "Yellows", 60.0),
    ("greens", "Greens", 120.0),
    ("cyans", "Cyans", 180.0),
    ("blues", "Blues", 240.0),
    ("magentas", "Magentas", 300.0),
];

/// Hue/Saturation's Channel Control popup.
pub const HUESAT_CHANNELS: [&str; 7] = ["Master", "Reds", "Yellows", "Greens", "Cyans", "Blues", "Magentas"];

/// One colour range of Hue/Saturation: the hues it covers fully (`start..end`, degrees, may
/// wrap), the fall-off widths either side, and its hue / saturation / lightness adjustments.
#[derive(Clone, Copy, Debug)]
struct HueRange {
    start: f32,
    end: f32,
    fall_start: f32,
    fall_end: f32,
    hue: f32,
    sat: f32,
    light: f32,
}

impl HueRange {
    /// Membership of hue `h` (degrees): 1 inside the range, ramping to 0 across the fall-offs.
    fn weight(&self, h: f32) -> f32 {
        let width = (self.end - self.start).rem_euclid(360.0);
        let d = (h - self.start).rem_euclid(360.0);
        if d <= width {
            return 1.0;
        }
        let after = d - width;
        let before = 360.0 - d;
        let wa = if self.fall_end > 0.0 { 1.0 - after / self.fall_end } else { 0.0 };
        let wb = if self.fall_start > 0.0 { 1.0 - before / self.fall_start } else { 0.0 };
        wa.max(wb).clamp(0.0, 1.0)
    }
}

/// The colour ranges with non-zero adjustments.
fn huesat_ranges(ctx: &EffectCtx) -> Vec<HueRange> {
    let f = |id: String, d: f64| ctx.params.get(&id).map(Value::as_f64).unwrap_or(d) as f32;
    HUESAT_RANGES
        .iter()
        .map(|(r, _, c)| HueRange {
            start: f(format!("{r}RangeStart"), c - 15.0),
            end: f(format!("{r}RangeEnd"), c + 15.0),
            fall_start: f(format!("{r}StartFalloff"), 30.0).max(0.0),
            fall_end: f(format!("{r}EndFalloff"), 30.0).max(0.0),
            hue: f(format!("{r}Hue"), 0.0) / 360.0,
            sat: f(format!("{r}Saturation"), 0.0) / 100.0,
            light: f(format!("{r}Lightness"), 0.0) / 100.0,
        })
        .filter(|r| r.hue != 0.0 || r.sat != 0.0 || r.light != 0.0)
        .collect()
}

/// Whether no colour range of Hue/Saturation adjusts anything (the GPU kernel does Master and
/// Colorize only).
pub fn huesat_ranges_identity(ctx: &EffectCtx) -> bool {
    huesat_ranges(ctx).is_empty()
}

fn adjust_sat(s: f32, sat: f32) -> f32 {
    if sat >= 0.0 { s + (1.0 - s) * sat * s.min(1.0) } else { s * (1.0 + sat) }
}

fn adjust_light(l: f32, light: f32) -> f32 {
    if light >= 0.0 { l + (1.0 - l) * light } else { l * (1.0 + light) }
}

/// Hue/Saturation. Master adjusts every pixel; each colour range (Reds … Magentas, chosen in
/// Channel Control) adjusts pixels whose original hue lies in its range, fading across the
/// fall-offs (greys, having no hue, belong to no range). Colorize replaces hue and
/// saturation.
fn hue_saturation(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let hue = ctx.params.f("hue") as f32 / 360.0;
    let sat = ctx.params.f("saturation") as f32 / 100.0;
    let light = ctx.params.f("lightness") as f32 / 100.0;
    let colorize = ctx.params.b("colorize");
    let ch = ctx.params.f("colorizeHue") as f32 / 360.0;
    let cs = ctx.params.f("colorizeSaturation") as f32 / 100.0;
    let cl = ctx.params.f("colorizeLightness") as f32 / 100.0;
    let ranges = huesat_ranges(ctx);
    b.img.map_straight(|c| {
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let (h, s, l) = if colorize {
            (ch, cs, (l + cl * if cl > 0.0 { 1.0 - l } else { l }).clamp(0.0, 1.0))
        } else {
            let mut s2 = adjust_sat(s, sat);
            let mut l2 = adjust_light(l, light);
            let mut h2 = h + hue;
            for r in &ranges {
                let w = r.weight(h * 360.0) * (s * 20.0).min(1.0);
                if w > 0.0 {
                    h2 += r.hue * w;
                    s2 = adjust_sat(s2.clamp(0.0, 1.0), r.sat * w);
                    l2 = adjust_light(l2, r.light * w);
                }
            }
            (h2.rem_euclid(1.0), s2.clamp(0.0, 1.0), l2)
        };
        let (r, g, bl) = hsl_to_rgb(h, s, l);
        [r, g, bl]
    });
    b
}

/// Options of Levels' Clip To Output Black / White.
pub const LEVELS_CLIP: [&str; 3] = ["Off", "On", "Off for 32 bpc Color"];

/// Whether Levels clips below Input Black / above Input White. "Off for 32 bpc Color" clips
/// in 8 and 16 bpc projects and not in 32 bpc ones (After Effects 2026, Levels (Individual
/// Controls) in a 32 bpc project: values below Input Black end below Output Black). The hidden
/// `noClip` switch of projects saved before the two popups existed turns both off.
pub fn levels_clip(ctx: &EffectCtx) -> (bool, bool) {
    if ctx.params.b("noClip") {
        return (false, false);
    }
    let on = |id: &str| levels_clip_on(ctx, id);
    (on("clipToOutputBlack"), on("clipToOutputWhite"))
}

/// One Clip To Output popup: 0 Off, 1 On, 2 Off for 32 bpc Color (missing: clip).
pub fn levels_clip_on(ctx: &EffectCtx, id: &str) -> bool {
    match ctx.params.get(id).map(|v| v.as_enum()) {
        Some(0) => false,
        Some(2) => !ctx.env.float_depth,
        _ => true,
    }
}

/// Levels' gamma on the normalised input `t`: unclipped values below Input Black extrapolate
/// (the curve mirrored through 0) instead of stopping at Output Black.
pub fn levels_gamma(t: f32, g: f32) -> f32 {
    if t >= 0.0 { t.powf(1.0 / g) } else { -(-t).powf(1.0 / g) }
}

/// Levels' Channel popup.
pub const LEVELS_CHANNELS: [&str; 5] = ["RGB", "Red", "Green", "Blue", "Alpha"];

/// Spec-id prefixes of Levels' per-channel controls (`redInBlack`, …; the RGB controls have
/// plain ids).
pub const LEVELS_CHANNEL_PREFIX: [&str; 4] = ["red", "green", "blue", "alpha"];

/// Levels' five controls (input black, input white, gamma, output black, output white) for
/// Channel popup index `ch` (0 = RGB).
pub fn levels_channel_ids(ch: usize) -> [String; 5] {
    if ch == 0 || ch > 4 {
        return ["inBlack", "inWhite", "gamma", "outBlack", "outWhite"].map(String::from);
    }
    let pre = LEVELS_CHANNEL_PREFIX[ch - 1];
    ["InBlack", "InWhite", "Gamma", "OutBlack", "OutWhite"].map(|s| format!("{pre}{s}"))
}

fn level(v: f32, [ib, iw, g, ob, ow]: [f32; 5], (clip_b, clip_w): (bool, bool)) -> f32 {
    let mut t = (v - ib) / (iw - ib).max(1e-6);
    if clip_b {
        t = t.max(0.0);
    }
    if clip_w {
        t = t.min(1.0);
    }
    ob + (ow - ob) * levels_gamma(t, g)
}

const LEVELS_IDENTITY: [f32; 5] = [0.0, 1.0, 1.0, 0.0, 1.0];

/// Levels: the RGB controls map every colour channel, then the Red / Green / Blue controls
/// their own channel and the Alpha controls alpha. All five sets are kept; the Channel popup
/// only picks the set Effect Controls shows.
fn levels(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ib = ctx.params.f("inBlack") as f32;
    let iw = ctx.params.f("inWhite") as f32;
    let g = ctx.params.f("gamma").max(0.01) as f32;
    let ob = ctx.params.f("outBlack") as f32;
    let ow = ctx.params.f("outWhite") as f32;
    let clip = levels_clip(ctx);
    b.img.map_straight(|c| c.map(|v| level(v, [ib, iw, g, ob, ow], clip)));
    let chans = levels_channel_settings(ctx);
    let ident = |l: &([f32; 5], (bool, bool))| l.0 == LEVELS_IDENTITY;
    if chans.iter().all(ident) {
        return b;
    }
    use rayon::prelude::*;
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = crate::util::unpremul(*px);
        let mut o = c;
        for i in 0..3 {
            if !ident(&chans[i]) {
                o[i] = level(c[i], chans[i].0, chans[i].1);
            }
        }
        let na = if ident(&chans[3]) { a } else { level(a, chans[3].0, chans[3].1).clamp(0.0, 1.0) };
        *px = crate::util::premul(o, na);
    });
    b
}

/// Levels' Red, Green, Blue and Alpha settings and clip switches (identity for projects saved
/// before the Channel popup).
pub fn levels_channel_settings(ctx: &EffectCtx) -> [([f32; 5], (bool, bool)); 4] {
    std::array::from_fn(|i| {
        let ids = levels_channel_ids(i + 1);
        let v: [f32; 5] = std::array::from_fn(|k| ctx.params.get(&ids[k]).map(|v| v.as_f64() as f32).unwrap_or(LEVELS_IDENTITY[k]));
        let pre = LEVELS_CHANNEL_PREFIX[i];
        let on = |id: String| levels_clip_on(ctx, &id);
        let clip = if ctx.params.b("noClip") { (false, false) } else { (on(format!("{pre}ClipToOutputBlack")), on(format!("{pre}ClipToOutputWhite"))) };
        ([v[0], v[1], v[2].max(0.01), v[3], v[4]], clip)
    })
}

/// Whether Levels' per-channel controls are all at their defaults (the GPU kernel handles
/// only the RGB controls).
pub fn levels_channels_identity(ctx: &EffectCtx) -> bool {
    levels_channel_settings(ctx).iter().all(|l| l.0 == LEVELS_IDENTITY)
}

/// Exposure's per-channel (gain, offset, gamma) for R, G, B: the Master group's values, or the
/// Red / Green / Blue groups' with Channels = Individual Channels.
pub fn exposure_settings(ctx: &EffectCtx) -> [(f32, f32, f32); 3] {
    let f = |k: &str| ctx.params.f(k) as f32;
    let one = |e: &str, o: &str, g: &str| (2f32.powf(f(e)), f(o), f(g).max(0.01));
    if ctx.params.e("channels") == 1 {
        [
            one("red/redExposure", "red/redOffset", "red/redGamma"),
            one("green/greenExposure", "green/greenOffset", "green/greenGamma"),
            one("blue/blueExposure", "blue/blueOffset", "blue/blueGamma"),
        ]
    } else {
        [one("master/exposure", "master/offset", "master/gamma"); 3]
    }
}

/// The power curve Exposure's own linear-light conversion uses (Bypass Linear Light Conversion
/// off). After Effects decodes with a pure 2.2 power, not the piecewise sRGB curve: Offset 0.0005
/// on black measures 8/255 there, which 2.2 predicts (8.06) and sRGB does not (1.65).
/// `kernels.wgsl` repeats it as a literal; the GPU test `shader_constants_match_the_cpu` checks it.
pub const EXPOSURE_LINEAR_GAMMA: f32 = 2.2;

/// Exposure: per channel, `(value · 2^exposure + offset)^(1 / gamma)`, in linear light unless
/// Bypass Linear Light Conversion is on. The conversion is the effect's own pure 2.2 power curve
/// (see [`EXPOSURE_LINEAR_GAMMA`]), applied whatever the project's working space, also when it
/// is already linear (Linearize Working Space), as before. Negative values (32 bpc, or a
/// negative Offset) go through the curves mirrored, so a negative Offset darkens smoothly past
/// black and the default settings leave every value unchanged.
fn exposure(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = exposure_settings(ctx);
    let bypass = ctx.params.b("bypassLinearLight");
    b.img.map_straight(|c| {
        [0, 1, 2].map(|i| {
            let (e, off, g) = s[i];
            if bypass {
                return mirror_pow(c[i] * e + off, 1.0 / g);
            }
            let lin = mirror_pow(c[i], EXPOSURE_LINEAR_GAMMA);
            let o = mirror_pow(lin * e + off, 1.0 / g);
            mirror_pow(o, 1.0 / EXPOSURE_LINEAR_GAMMA)
        })
    });
    b
}

/// `|v|^p` with `v`'s sign: Exposure adjusts negative values as if they were positive.
pub fn mirror_pow(v: f32, p: f32) -> f32 {
    v.abs().powf(p).copysign(v)
}

/// Black & White: a pixel's gray is its smallest channel plus its chroma (largest − smallest
/// channel) times the colour sliders' weight for its hue. A neutral pixel keeps its value, a pure
/// hue becomes its slider's percentage (Reds 40 % turns pure red into 40 % gray), and the gray
/// never moves further from the smallest channel than the pixel's own chroma times the weight.
/// It used to offset the luminance by the HSL saturation, which is near 1 for any tinted
/// near-black pixel: the faint colour noise of compressed footage's shadows came out as bright
/// and black blocks (#1463), and 32-bpc overbrights divided by zero.
/// Black & White's Tint: the tint colour's hue and saturation at the gray's luminosity, as a
/// Color blend does (luminosity with weights 0.3 / 0.59 / 0.11, out-of-range channels pulled
/// back toward the gray). After Effects 2026 with Tint Color [1, 0.5, 0]: gray 0.1 (a red solid
/// at Reds 10, case 717) → (43, 21, 0), gray 0.6 (yellow at defaults, case 825) → (255, 129, 2),
/// white → white; the old hue / saturation / lightness mix gave (51, 26, 0) for the first.
/// The luminosity weights of `bw_tint` (a Color blend's). `fx_color.wgsl` repeats them; the GPU
/// test `shader_constants_match_the_cpu` checks that they are these.
pub const BW_TINT_LUMA: [f32; 3] = [0.3, 0.59, 0.11];

pub fn bw_tint(tint: [f32; 3], gray: f32) -> [f32; 3] {
    let lum = |c: [f32; 3]| BW_TINT_LUMA[0] * c[0] + BW_TINT_LUMA[1] * c[1] + BW_TINT_LUMA[2] * c[2];
    if !gray.is_finite() || gray <= 0.0 {
        return [0.0; 3];
    }
    if gray >= 1.0 {
        return [gray; 3];
    }
    let d = gray - lum(tint);
    let c = tint.map(|v| v + d);
    let l = lum(c);
    let (n, x) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    let mut o = c;
    if n < 0.0 && l - n > 1e-9 {
        o = o.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 && x - l > 1e-9 {
        o = o.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    o
}

fn black_white(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let w = [ctx.params.f("reds"), ctx.params.f("yellows"), ctx.params.f("greens"), ctx.params.f("cyans"), ctx.params.f("blues"), ctx.params.f("magentas")]
        .map(|v| v as f32 / 100.0);
    let tint_on = ctx.params.b("tint");
    let tc = ctx.params.color("tintColor");
    b.img.map_straight(|c| {
        let (h, _, _) = rgb_to_hsl(c[0], c[1], c[2]);
        // Interpolate the six hue weights around the colour wheel.
        let hp = h * 6.0;
        let i = hp.floor() as usize % 6;
        let f = hp - hp.floor();
        let wt = w[i] + (w[(i + 1) % 6] - w[i]) * f;
        let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
        let gray = (min + (max - min) * wt).max(0.0);
        if tint_on { bw_tint([tc[0], tc[1], tc[2]], gray) } else { [gray, gray, gray] }
    });
    b
}

/// Whether Fill uses masks (Fill Mask or All Masks): the GPU kernel fills alpha only.
pub fn fill_uses_masks(ctx: &EffectCtx) -> bool {
    !ctx.env.masks.is_empty() && (ctx.params.b("allMasks") || ctx.params.f("fillMask") >= 1.0)
}

/// Coverage (0..1, 2×2 supersampled) of the closed polygons `polys` (buffer px) on a `w`×`h`
/// grid: the union of the shapes, each inverted when its mask is.
pub(crate) fn polys_coverage(w: usize, h: usize, polys: &[(Vec<[f64; 2]>, bool)]) -> crate::util::Plane {
    use rayon::prelude::*;
    let mut out = crate::util::Plane::new(w, h);
    out.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let mut hit = 0.0;
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (px, py) = (x as f64 + sx, y as f64 + sy);
                if polys.iter().any(|(p, inv)| crate::util::point_in_poly(p, px, py) != *inv) {
                    hit += 0.25;
                }
            }
            *v = hit;
        }
    });
    out
}

/// Fill: paints the layer's pixels with Color. With Fill Mask (or All Masks) only the pixels
/// inside that mask (or any mask) are painted, with the mask edge feathered horizontally /
/// vertically; Invert paints outside instead. Without a mask, Invert fills the transparent
/// areas.
fn fill(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = ctx.params.color("color");
    let invert = ctx.params.b("invert");
    let opacity = ctx.params.f("opacity") as f32 / 100.0;
    if fill_uses_masks(ctx) {
        let all = ctx.params.b("allMasks");
        let idx = ctx.params.f("fillMask").round() as usize;
        let polys: Vec<(Vec<[f64; 2]>, bool)> = ctx
            .env
            .masks
            .iter()
            .enumerate()
            .filter(|(i, m)| (all || i + 1 == idx) && m.points.len() >= 3)
            .map(|(_, m)| (m.points.iter().map(|q| [q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1]]).collect(), m.inverted))
            .collect();
        let (w, h) = (b.img.width as usize, b.img.height as usize);
        let mut cov = polys_coverage(w, h, &polys);
        let (fh, fv) = (ctx.params.f("horizontalFeather").max(0.0) * b.scale, ctx.params.f("verticalFeather").max(0.0) * b.scale);
        if fh > 0.0 || fv > 0.0 {
            cov = crate::util::gauss_plane(&cov, fh * 0.5, fv * 0.5);
        }
        use rayon::prelude::*;
        b.img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &m)| {
            let m = if invert { 1.0 - m } else { m } * opacity;
            let a = px[3];
            for i in 0..3 {
                px[i] += (c[i] * a - px[i]) * m;
            }
        });
        return b;
    }
    // Without masks the layer becomes the fill colour, and Opacity is the layer's opacity: After
    // Effects 2026 renders Fill (0, 0.5, 1) at Opacity 50 % as that colour at half alpha, with
    // nothing of the original showing through (it is not a mix with the original).
    b.img.data.iter_mut().for_each(|px| {
        let a = if invert { 1.0 - px[3] } else { px[3] } * opacity;
        *px = [c[0] * a, c[1] * a, c[2] * a, a];
    });
    b
}

/// Change to Color: pixels whose hue, lightness and saturation are each within their tolerance
/// of From are changed (Softness feathers the match). Change picks the components that change;
/// Change By either sets them to To's (Setting To Color) or shifts them by To − From
/// (Transforming To Color). View Correction Matte shows the match as grey.
fn change_to_color(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let from = ctx.params.color("from");
    let to = ctx.params.color("to");
    let tol = ["toleranceGroup/hue", "toleranceGroup/lightness", "toleranceGroup/saturation"].map(|k| ctx.params.f(k) as f32 / 100.0);
    let soft = ctx.params.f("softness") as f32 / 100.0;
    let change = ctx.params.e("change");
    let (ch_l, ch_s) = (matches!(change, 1 | 3), matches!(change, 2 | 3));
    let transform = ctx.params.e("changeBy") == 1;
    let matte = ctx.params.b("viewCorrectionMatte");
    let (fh, fs, fl) = rgb_to_hsl(from[0], from[1], from[2]);
    let (th, ts, tl) = rgb_to_hsl(to[0], to[1], to[2]);
    b.img.map_straight(|c| {
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let d = [((h - fh + 0.5).rem_euclid(1.0) - 0.5).abs() * 2.0, (l - fl).abs(), (s - fs).abs()];
        let mut k = 1.0f32;
        for i in 0..3 {
            let over = d[i] - tol[i];
            if over > 0.0 {
                k = k.min(if soft <= 1e-6 { 0.0 } else { (1.0 - over / soft).clamp(0.0, 1.0) });
            }
        }
        if matte {
            return [k; 3];
        }
        if k <= 0.0 {
            return c;
        }
        let (nh, ns, nl) = if transform {
            let dh = ((th - fh + 0.5).rem_euclid(1.0) - 0.5) * k;
            ((h + dh).rem_euclid(1.0), if ch_s { s + (ts - fs) * k } else { s }, if ch_l { l + (tl - fl) * k } else { l })
        } else {
            let dh = ((th - h + 0.5).rem_euclid(1.0) - 0.5) * k;
            ((h + dh).rem_euclid(1.0), if ch_s { s + (ts - s) * k } else { s }, if ch_l { l + (tl - l) * k } else { l })
        };
        let (r, g, bl) = hsl_to_rgb(nh, ns.clamp(0.0, 1.0), nl.clamp(0.0, 1.0));
        [r, g, bl]
    });
    b
}

/// Leave Color: decolours pixels unlike Color To Leave. Tolerance 0 keeps only exact matches,
/// 100 keeps everything; Edge Softness feathers the boundary; Match Colors compares RGB
/// distance or hue only.
fn leave_color(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let keep = ctx.params.color("color");
    let amt = ctx.params.f("amount") as f32 / 100.0;
    let tol = ctx.params.f("tolerance") as f32 / 100.0;
    let soft = ctx.params.f("edgeSoftness") as f32 / 100.0;
    let by_hue = ctx.params.e("matchColors") == 1;
    if amt <= 0.0 {
        return b;
    }
    let (kh, _, _) = rgb_to_hsl(keep[0], keep[1], keep[2]);
    b.img.map_straight(|c| {
        let d = if by_hue {
            let (h, s, _) = rgb_to_hsl(c[0], c[1], c[2]);
            // Greys have no hue: they never match a hue.
            if s < 0.02 { 1.0 } else { ((h - kh + 0.5).rem_euclid(1.0) - 0.5).abs() * 2.0 }
        } else {
            ((c[0] - keep[0]).powi(2) + (c[1] - keep[1]).powi(2) + (c[2] - keep[2]).powi(2)).sqrt() / 3f32.sqrt()
        };
        let keepk = if tol >= 1.0 || d <= tol {
            1.0
        } else if soft <= 1e-6 {
            0.0
        } else {
            (1.0 - (d - tol) / soft).clamp(0.0, 1.0)
        };
        let g = luminance(c[0], c[1], c[2]);
        mix(c, [g, g, g], amt * (1.0 - keepk))
    });
    b
}

/// Photo Filter's Filter menu: the conventional camera filters (Wratten-style warming /
/// cooling conversion and light-balancing filters) and plain colours, as 8-bit sRGB values
/// chosen for EffectCraft; the last entry, Custom, uses the Color parameter.
pub const PHOTO_FILTERS: [(&str, [u8; 3]); 21] = [
    ("Warming Filter (85)", [240, 140, 30]),
    ("Warming Filter (LBA)", [245, 155, 25]),
    ("Warming Filter (81)", [240, 180, 40]),
    ("Cooling Filter (80)", [20, 110, 250]),
    ("Cooling Filter (LBB)", [20, 95, 245]),
    ("Cooling Filter (82)", [30, 180, 250]),
    ("Red", [230, 30, 30]),
    ("Orange", [240, 130, 30]),
    ("Yellow", [245, 225, 35]),
    ("Green", [30, 200, 30]),
    ("Cyan", [30, 200, 230]),
    ("Blue", [30, 55, 230]),
    ("Violet", [150, 30, 230]),
    ("Magenta", [225, 30, 225]),
    ("Sepia", [170, 120, 55]),
    ("Deep Red", [250, 0, 0]),
    ("Deep Blue", [0, 40, 200]),
    ("Deep Emerald", [0, 135, 0]),
    ("Deep Yellow", [255, 210, 0]),
    ("Underwater", [0, 190, 175]),
    ("Custom", [0, 0, 0]),
];
/// Index of Photo Filter's Custom entry.
pub const PHOTO_FILTER_CUSTOM: u32 = PHOTO_FILTERS.len() as u32 - 1;

fn photo_filter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let f = ctx.params.e("filter");
    let c = match PHOTO_FILTERS.get(f as usize) {
        Some((_, rgb)) if f != PHOTO_FILTER_CUSTOM => rgb.map(|v| v as f32 / 255.0),
        _ => {
            let c = ctx.params.color("color");
            [c[0], c[1], c[2]]
        }
    };
    let d = ctx.params.f("density") as f32 / 100.0;
    let keep = ctx.params.b("preserveLuminosity");
    b.img.map_straight(|x| {
        let f = [x[0] * c[0], x[1] * c[1], x[2] * c[2]];
        let mut o = mix(x, f, d);
        if keep {
            let l0 = luminance(x[0], x[1], x[2]);
            let l1 = luminance(o[0], o[1], o[2]).max(1e-6);
            o = o.map(|v| v * l0 / l1);
        }
        o
    });
    b
}

fn vibrance(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let vib = ctx.params.f("vibrance") as f32 / 100.0;
    let sat = ctx.params.f("saturation") as f32 / 100.0;
    b.img.map_straight(|c| {
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let s = (s * (1.0 + sat) + vib * (1.0 - s) * s.min(0.5)).clamp(0.0, 1.0);
        let (r, g, bl) = hsl_to_rgb(h, s, l);
        [r, g, bl]
    });
    b
}

fn color_balance(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let g = |k: &str| ctx.params.f(k) as f32 / 100.0;
    let sh = [g("shadowRed"), g("shadowGreen"), g("shadowBlue")];
    let md = [g("midRed"), g("midGreen"), g("midBlue")];
    let hl = [g("hiRed"), g("hiGreen"), g("hiBlue")];
    let keep_luma = ctx.params.b("preserveLuminosity");
    b.img.map_straight(|c| {
        let l = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0);
        let ws = (1.0 - l * 2.0).clamp(0.0, 1.0);
        let wh = (l * 2.0 - 1.0).clamp(0.0, 1.0);
        let wm = 1.0 - ws - wh;
        let mut o = c;
        for i in 0..3 {
            o[i] = (c[i] + (sh[i] * ws + md[i] * wm + hl[i] * wh) * 0.5).max(0.0);
        }
        if keep_luma {
            // Preserve Luminosity: keep the pixel's luminance, change only its colour.
            let d = luminance(c[0], c[1], c[2]) - luminance(o[0], o[1], o[2]);
            o = o.map(|v| (v + d).max(0.0));
        }
        o
    });
    b
}

fn channel_mixer(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let g = |k: &str| ctx.params.f(k) as f32 / 100.0;
    let m = [[g("rr"), g("rg"), g("rb"), g("rc")], [g("gr"), g("gg"), g("gb"), g("gc")], [g("br"), g("bg"), g("bb"), g("bc")]];
    let mono = ctx.params.b("monochrome");
    b.img.map_straight(|c| {
        let row = |r: [f32; 4]| (c[0] * r[0] + c[1] * r[1] + c[2] * r[2] + r[3]).max(0.0);
        if mono {
            let v = row(m[0]);
            [v, v, v]
        } else {
            [row(m[0]), row(m[1]), row(m[2])]
        }
    });
    b
}

fn gamma_pg(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ch =
        |n: &str| (ctx.params.f(&format!("{n}Gamma")).max(0.01) as f32, ctx.params.f(&format!("{n}Pedestal")) as f32, ctx.params.f(&format!("{n}Gain")) as f32);
    let r = ch("red");
    let g = ch("green");
    let bl = ch("blue");
    let stretch = ctx.params.f("blackStretch").max(0.0) as f32;
    b.img.map_straight(|c| {
        // Black Stretch lifts the low values of every channel (a toe that fades out by mid grey).
        let toe = |v: f32| if stretch > 0.0 && v > 0.0 && v < 1.0 { v + stretch * 0.25 * v * (1.0 - v).powi(4) } else { v };
        let c = c.map(toe);
        let f = |v: f32, (gm, pd, gn): (f32, f32, f32)| (pd + (gn - pd) * v.max(0.0).powf(gm)).max(0.0);
        [f(c[0], r), f(c[1], g), f(c[2], bl)]
    });
    b
}

/// Colorama's Get Phase From options.
pub const COLORAMA_PHASE: [&str; 10] = ["Intensity", "Red", "Green", "Blue", "Hue", "Lightness", "Saturation", "Value", "Alpha", "Zero"];

/// The input phase (0..1) of a straight colour + alpha for Get Phase From option `mode`.
fn colorama_phase(c: [f32; 3], a: f32, mode: u32) -> f32 {
    let v = match mode {
        1 => c[0],
        2 => c[1],
        3 => c[2],
        4 => rgb_to_hsl(c[0], c[1], c[2]).0,
        5 => rgb_to_hsl(c[0], c[1], c[2]).2,
        6 => rgb_to_hsl(c[0], c[1], c[2]).1,
        7 => c[0].max(c[1]).max(c[2]),
        8 => a,
        9 => 0.0,
        _ => luminance(c[0], c[1], c[2]),
    };
    v.clamp(0.0, 1.0)
}

/// Colorama's Output Cycle presets (our own palettes; Custom uses the Palette parameter).
pub const COLORAMA_PRESETS: [&str; 9] = ["Hue Cycle", "Ramp Grayscale", "Fire", "Ice", "Ramp Red", "Ramp Green", "Ramp Blue", "Pastel Cycle", "Custom"];
/// Colorama's Modify options: which channels of the pixel take the palette colour.
const COLORAMA_MODIFY: [&str; 12] =
    ["All", "Red", "Green", "Blue", "Hue", "Lightness", "Saturation", "Value", "Red & Green", "Green & Blue", "Blue & Red", "None"];

/// An output cycle: colour stops around the wheel (position 0..1, straight RGBA), cyclic.
#[derive(Clone, Debug, PartialEq)]
pub struct ColoramaPalette {
    pub stops: Vec<(f32, [f32; 4])>,
}

impl ColoramaPalette {
    /// Built-in palette `i` of [`COLORAMA_PRESETS`].
    pub fn preset(i: u32) -> ColoramaPalette {
        let s = |v: &[(f32, [f32; 3])]| ColoramaPalette { stops: v.iter().map(|(p, c)| (*p, [c[0], c[1], c[2], 1.0])).collect() };
        match i {
            1 => s(&[(0.0, [0.0; 3]), (0.999, [1.0; 3])]),
            2 => s(&[(0.0, [0.0, 0.0, 0.0]), (0.3, [0.7, 0.05, 0.0]), (0.6, [1.0, 0.55, 0.0]), (0.85, [1.0, 0.95, 0.4]), (0.999, [1.0, 1.0, 1.0])]),
            3 => s(&[(0.0, [0.0, 0.0, 0.1]), (0.4, [0.1, 0.3, 0.8]), (0.75, [0.5, 0.85, 1.0]), (0.999, [1.0, 1.0, 1.0])]),
            4 => s(&[(0.0, [0.0; 3]), (0.999, [1.0, 0.0, 0.0])]),
            5 => s(&[(0.0, [0.0; 3]), (0.999, [0.0, 1.0, 0.0])]),
            6 => s(&[(0.0, [0.0; 3]), (0.999, [0.0, 0.0, 1.0])]),
            7 => s(&[(0.0, [1.0, 0.7, 0.7]), (1.0 / 3.0, [0.7, 1.0, 0.7]), (2.0 / 3.0, [0.7, 0.7, 1.0])]),
            _ => s(&[
                (0.0, [1.0, 0.0, 0.0]),
                (1.0 / 6.0, [1.0, 1.0, 0.0]),
                (2.0 / 6.0, [0.0, 1.0, 0.0]),
                (3.0 / 6.0, [0.0, 1.0, 1.0]),
                (4.0 / 6.0, [0.0, 0.0, 1.0]),
                (5.0 / 6.0, [1.0, 0.0, 1.0]),
            ]),
        }
    }

    /// Parse "pos:r,g,b\[,a\] pos:r,g,b …" (positions 0..1, channels 0..1). `None` without stops.
    pub fn parse(s: &str) -> Option<ColoramaPalette> {
        let mut stops: Vec<(f32, [f32; 4])> = s
            .split_whitespace()
            .filter_map(|t| {
                let (pos, c) = t.split_once(':')?;
                let v: Vec<f32> = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() >= 3).then(|| (pos.trim().parse::<f32>().ok().map(|p| p.rem_euclid(1.0)), [v[0], v[1], v[2], v.get(3).copied().unwrap_or(1.0)]))
            })
            .filter_map(|(p, c)| p.map(|p| (p, c)))
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        (!stops.is_empty()).then_some(ColoramaPalette { stops })
    }

    /// The colour at phase `ph` (0..1). Interpolate Palette blends between neighbouring stops
    /// (around the wheel); off, each phase takes the stop at or before it.
    pub fn eval(&self, ph: f32, interpolate: bool) -> [f32; 4] {
        let n = self.stops.len();
        let ph = ph.rem_euclid(1.0);
        let k = self.stops.iter().rposition(|s| s.0 <= ph).unwrap_or(n - 1);
        let (p0, c0) = self.stops[k];
        if !interpolate || n == 1 {
            return c0;
        }
        let (p1, c1) = self.stops[(k + 1) % n];
        let span = (p1 - p0).rem_euclid(1.0).max(1e-6);
        let t = ((ph - p0).rem_euclid(1.0) / span).clamp(0.0, 1.0);
        std::array::from_fn(|i| c0[i] + (c1[i] - c0[i]) * t)
    }
}

/// How well colour `c` matches `key` (1 = matches) for Pixel Selection's Matching Mode
/// (1 RGB, 2 Hue, 3 Chroma), with Matching Tolerance / Softness (0..1).
fn colorama_match(c: [f32; 3], key: [f32; 3], mode: u32, tol: f32, soft: f32) -> f32 {
    let d = match mode {
        2 => {
            let (h1, s1, _) = rgb_to_hsl(c[0], c[1], c[2]);
            let (h2, _, _) = rgb_to_hsl(key[0], key[1], key[2]);
            let dh = (h1 - h2).abs();
            dh.min(1.0 - dh) * 2.0 + (1.0 - s1.min(1.0)) * 0.5
        }
        3 => {
            // Chroma: the colour without its brightness.
            let n = |c: [f32; 3]| {
                let s = c[0] + c[1] + c[2];
                if s > 1e-5 { [c[0] / s, c[1] / s, c[2] / s] } else { [1.0 / 3.0; 3] }
            };
            let (a, b) = (n(c), n(key));
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * 1.5
        }
        _ => ((c[0] - key[0]).powi(2) + (c[1] - key[1]).powi(2) + (c[2] - key[2]).powi(2)).sqrt() / 3f32.sqrt(),
    };
    if d <= tol {
        1.0
    } else if soft > 0.0 {
        (1.0 - (d - tol) / soft).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Colorama: a phase taken from the pixel (Get Phase From, optionally combined with an Add
/// Phase layer), shifted and repeated around the Output Cycle, picks a palette colour, which
/// replaces the channels Modify names. Pixel Selection limits it to colours near Matching
/// Color; Masking to where a mask layer allows; Blend With Original mixes the result back.
fn colorama(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let shift = pr.f("inputPhase/phaseShift") as f32 / 360.0;
    let mode = pr.e("inputPhase/mode");
    let cycles = pr.f("outputCycle/cycles").max(0.0) as f32;
    let smooth = pr.get("outputCycle/interpolatePalette").is_none_or(|v| v.as_bool());
    let preset = pr.e("outputCycle/usePresetPalette");
    let palette = if preset as usize == COLORAMA_PRESETS.len() - 1 { ColoramaPalette::parse(pr.s("outputCycle/palette")) } else { None }
        .unwrap_or_else(|| ColoramaPalette::preset(preset));
    let blend = 1.0 - pr.f("blend") as f32 / 100.0;
    // Add Phase.
    let add = ctx.layer_param("inputPhase/addPhase", true).map(|o| crate::util::fit_layer(ctx, &b, &o, true));
    let add_from = pr.e("inputPhase/addPhaseFrom");
    let add_mode = pr.e("inputPhase/addMode");
    // Modify.
    let modify = pr.e("modify/modify");
    let modify_alpha = pr.b("modify/modifyAlpha");
    let change_empty = pr.b("modify/changeEmptyPixels");
    // Pixel Selection.
    let match_mode = pr.e("pixelSelection/matchingMode");
    let key = pr.color("pixelSelection/matchingColor");
    let tol = (pr.f("pixelSelection/matchingTolerance") / 100.0) as f32;
    let soft = (pr.f("pixelSelection/matchingSoftness") / 100.0) as f32;
    // Masking.
    let mask_mode = pr.e("masking/maskingMode");
    let mask = (mask_mode != 0).then(|| ctx.layer_param("masking/maskLayer", true).map(|o| crate::util::fit_layer(ctx, &b, &o, true))).flatten();
    let composite = pr.get("masking/compositeOverLayer").is_none_or(|v| v.as_bool());
    use rayon::prelude::*;
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let a = px[3];
        if a <= 0.0 && !change_empty {
            return;
        }
        let c = if a > 0.0 { [px[0] / a, px[1] / a, px[2] / a] } else { [0.0; 3] };
        let mut p = colorama_phase(c, a, mode);
        if let Some(img) = &add {
            let (ac, aa) = crate::util::unpremul(img.data[i]);
            let q = colorama_phase(ac, aa, add_from);
            p = match add_mode {
                1 => (p + q).min(1.0),
                2 => (p + q) * 0.5,
                3 => p + q - p * q,
                _ => (p + q).rem_euclid(1.0),
            };
        }
        let ph = (p * cycles + shift).rem_euclid(1.0);
        let pc = palette.eval(ph, smooth);
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let (ph_, ps, pl) = rgb_to_hsl(pc[0], pc[1], pc[2]);
        let hl = |h: f32, s: f32, l: f32| {
            let (r, g, bb) = hsl_to_rgb(h, s, l);
            [r, g, bb]
        };
        let new: [f32; 3] = match modify {
            1 => [pc[0], c[1], c[2]],
            2 => [c[0], pc[1], c[2]],
            3 => [c[0], c[1], pc[2]],
            4 => hl(ph_, s, l),
            5 => hl(h, s, pl),
            6 => hl(h, ps, l),
            7 => {
                // Value: scale the colour to the palette colour's maximum.
                let (m, pm) = (c[0].max(c[1]).max(c[2]), pc[0].max(pc[1]).max(pc[2]));
                if m > 1e-6 { c.map(|v| v * pm / m) } else { [pm; 3] }
            }
            8 => [pc[0], pc[1], c[2]],
            9 => [c[0], pc[1], pc[2]],
            10 => [pc[0], c[1], pc[2]],
            11 => c,
            _ => [pc[0], pc[1], pc[2]],
        };
        let new_a = if modify_alpha {
            pc[3]
        } else if a > 0.0 {
            a
        } else {
            1.0
        };
        // Where the effect applies: Pixel Selection × Masking.
        let mut w = if match_mode == 0 { 1.0 } else { colorama_match(c, [key[0], key[1], key[2]], match_mode, tol, soft) };
        if let Some(m) = &mask {
            let (mc, ma) = crate::util::unpremul(m.data[i]);
            let lum = luminance(mc[0], mc[1], mc[2]) * ma;
            w *= match mask_mode {
                1 => lum,
                2 => 1.0 - lum,
                3 => ma,
                _ => 1.0 - ma,
            }
            .clamp(0.0, 1.0);
        }
        let k = w * blend;
        let o = mix(c, new, k);
        let oa = a + (new_a - a) * k;
        // Composite Over Layer off: only the affected pixels remain.
        let oa = if composite { oa } else { oa * w };
        *px = [o[0] * oa, o[1] * oa, o[2] * oa, oa.clamp(0.0, 1.0)];
    });
    b
}

/// Levels' Red / Green / Blue / Alpha controls ("Red Input Black", …), in that order.
fn levels_channel_params() -> Vec<crate::ParamSpec> {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let names = ["Input Black", "Input White", "Gamma", "Output Black", "Output White"];
    let mut v = vec![];
    for (i, pre) in LEVELS_CHANNEL_PREFIX.iter().enumerate() {
        let label = LEVELS_CHANNELS[i + 1];
        let ids = levels_channel_ids(i + 1);
        for k in 0..5 {
            let ui = if k == 2 { slider(0.1, 10.0, 0.1, 3.0, 2) } else { slider(-1.0, 2.0, 0.0, 1.0, 3) };
            v.push(p(leak(ids[k].clone()), leak(format!("{label} {}", names[k])), num(LEVELS_IDENTITY[k] as f64), ui));
        }
        v.push(p(leak(format!("{pre}ClipToOutputBlack")), leak(format!("{label} Clip To Output Black")), Value::Enum(2), popup(&LEVELS_CLIP)));
        v.push(p(leak(format!("{pre}ClipToOutputWhite")), leak(format!("{label} Clip To Output White")), Value::Enum(2), popup(&LEVELS_CLIP)));
    }
    v
}

pub fn specs() -> Vec<EffectSpec> {
    let pct = || slider(-100.0, 100.0, -100.0, 100.0, 1);
    let mut gpg = vec![p("blackStretch", "Black Stretch", num(0.0), slider(0.0, 4.0, 0.0, 4.0, 2))];
    for (n, label) in [("red", "Red"), ("green", "Green"), ("blue", "Blue")] {
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        gpg.push(p(leak(format!("{n}Gamma")), leak(format!("{label} Gamma")), num(1.0), slider(0.1, 10.0, 0.1, 4.0, 2)));
        gpg.push(p(leak(format!("{n}Pedestal")), leak(format!("{label} Pedestal")), num(0.0), slider(-2.0, 2.0, -1.0, 1.0, 2)));
        gpg.push(p(leak(format!("{n}Gain")), leak(format!("{label} Gain")), num(1.0), slider(0.0, 4.0, 0.0, 2.0, 2)));
    }
    vec![
        spec(
            "ec.color.tint",
            "Tint",
            vec![
                p("black", "Map Black To", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("white", "Map White To", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("amount", "Amount to Tint", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            tint,
        ),
        spec(
            "ec.color.tritone",
            "Tritone",
            vec![
                p("highlights", "Highlights", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("midtones", "Midtones", col(0.5, 0.4, 0.3), ParamUi::Color),
                p("shadows", "Shadows", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            tritone,
        ),
        spec(
            "ec.color.brightnesscontrast",
            "Brightness & Contrast",
            vec![
                p("brightness", "Brightness", num(0.0), slider(-150.0, 150.0, -150.0, 150.0, 1)),
                p("contrast", "Contrast", num(0.0), pct()),
                p("useLegacy", "Use Legacy", Value::Bool(false), ParamUi::Checkbox),
            ],
            brightness_contrast,
        ),
        spec(
            "ec.color.huesaturation",
            "Hue/Saturation",
            {
                let mut v = vec![
                    p("channelControl", "Channel Control", Value::Enum(0), popup(&HUESAT_CHANNELS)),
                    p("hue", "Master Hue", num(0.0), ParamUi::Angle),
                    p("saturation", "Master Saturation", num(0.0), pct()),
                    p("lightness", "Master Lightness", num(0.0), pct()),
                ];
                let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
                for (r, name, c) in HUESAT_RANGES {
                    let single = name.trim_end_matches('s');
                    v.push(p(leak(format!("{r}Hue")), leak(format!("{single} Hue")), num(0.0), ParamUi::Angle));
                    v.push(p(leak(format!("{r}Saturation")), leak(format!("{single} Saturation")), num(0.0), pct()));
                    v.push(p(leak(format!("{r}Lightness")), leak(format!("{single} Lightness")), num(0.0), pct()));
                    v.push(p(leak(format!("{r}RangeStart")), leak(format!("{single} Range Start")), num(c - 15.0), slider(-360.0, 720.0, -180.0, 360.0, 0)));
                    v.push(p(leak(format!("{r}RangeEnd")), leak(format!("{single} Range End")), num(c + 15.0), slider(-360.0, 720.0, -180.0, 360.0, 0)));
                    v.push(p(leak(format!("{r}StartFalloff")), leak(format!("{single} Start Falloff")), num(30.0), slider(0.0, 180.0, 0.0, 90.0, 0)));
                    v.push(p(leak(format!("{r}EndFalloff")), leak(format!("{single} End Falloff")), num(30.0), slider(0.0, 180.0, 0.0, 90.0, 0)));
                }
                v.extend([
                    p("colorize", "Colorize", Value::Bool(false), ParamUi::Checkbox),
                    p("colorizeHue", "Colorize Hue", num(0.0), ParamUi::Angle),
                    p("colorizeSaturation", "Colorize Saturation", num(25.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                    p("colorizeLightness", "Colorize Lightness", num(0.0), pct()),
                ]);
                v
            },
            hue_saturation,
        ),
        spec(
            "ec.color.levels",
            "Levels",
            vec![
                p("channel", "Channel", Value::Enum(0), popup(&LEVELS_CHANNELS)),
                p("inBlack", "Input Black", num(0.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("inWhite", "Input White", num(1.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("gamma", "Gamma", num(1.0), slider(0.1, 10.0, 0.1, 3.0, 2)),
                p("outBlack", "Output Black", num(0.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("outWhite", "Output White", num(1.0), slider(-1.0, 2.0, 0.0, 1.0, 3)),
                p("clipToOutputBlack", "Clip To Output Black", Value::Enum(2), popup(&LEVELS_CLIP)),
                p("clipToOutputWhite", "Clip To Output White", Value::Enum(2), popup(&LEVELS_CLIP)),
                // Projects saved before the two popups: "Don't Clip".
                p("noClip", "Don't Clip", Value::Bool(false), ParamUi::Hidden),
            ]
            .into_iter()
            .chain(levels_channel_params())
            .collect(),
            levels,
        ),
        spec(
            "ec.color.exposure",
            "Exposure",
            {
                let mut v = vec![p("channels", "Channels", Value::Enum(0), popup(&["Master", "Individual Channels"]))];
                // After Effects' ranges (valid, slider, decimals). Gamma values below 0.1 from
                // older projects or Photoshop imports (0.01..9.99) are kept and still render.
                let group = |ids: [&'static str; 3]| {
                    [
                        p(ids[0], "Exposure", num(0.0), slider(-100.0, 100.0, -4.0, 4.0, 2)),
                        p(ids[1], "Offset", num(0.0), slider(-2.0, 2.0, -0.5, 0.5, 4)),
                        p(ids[2], "Gamma Correction", num(1.0), slider(0.1, 10.0, 0.1, 10.0, 2)),
                    ]
                };
                v.extend(group(["master/exposure", "master/offset", "master/gamma"]));
                v.extend(group(["red/redExposure", "red/redOffset", "red/redGamma"]));
                v.extend(group(["green/greenExposure", "green/greenOffset", "green/greenGamma"]));
                v.extend(group(["blue/blueExposure", "blue/blueOffset", "blue/blueGamma"]));
                v.push(p("bypassLinearLight", "Bypass Linear Light Conversion", Value::Bool(false), ParamUi::Checkbox));
                v
            },
            exposure,
        ),
        spec(
            "ec.color.blackwhite",
            "Black & White",
            vec![
                p("reds", "Reds", num(40.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("yellows", "Yellows", num(60.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("greens", "Greens", num(40.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("cyans", "Cyans", num(60.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("blues", "Blues", num(20.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("magentas", "Magentas", num(80.0), slider(-200.0, 300.0, -200.0, 300.0, 0)),
                p("tint", "Tint", Value::Bool(false), ParamUi::Checkbox),
                p("tintColor", "Tint Color", col(0.88, 0.76, 0.6), ParamUi::Color),
            ],
            black_white,
        ),
        EffectSpec {
            id: "ec.generate.fill",
            name: "Fill",
            category: "Generate",
            params: vec![
                p("fillMask", "Fill Mask", num(0.0), ParamUi::Mask),
                p("allMasks", "All Masks", Value::Bool(false), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
                p("horizontalFeather", "Horizontal Feather", num(0.0), slider(0.0, 5000.0, 0.0, 100.0, 1)),
                p("verticalFeather", "Vertical Feather", num(0.0), slider(0.0, 5000.0, 0.0, 100.0, 1)),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            render: fill,
            gpu: false,
            float: true,
        },
        spec(
            "ec.color.changetocolor",
            "Change to Color",
            vec![
                p("from", "From", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("to", "To", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("change", "Change", Value::Enum(0), popup(&["Hue", "Hue & Lightness", "Hue & Saturation", "Hue, Lightness & Saturation"])),
                p("changeBy", "Change By", Value::Enum(0), popup(&["Setting To Color", "Transforming To Color"])),
                p("toleranceGroup/hue", "Hue", num(5.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("toleranceGroup/lightness", "Lightness", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("toleranceGroup/saturation", "Saturation", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("softness", "Softness", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("viewCorrectionMatte", "View Correction Matte", Value::Bool(false), ParamUi::Checkbox),
            ],
            change_to_color,
        ),
        spec(
            "ec.color.leavecolor",
            "Leave Color",
            vec![
                p("amount", "Amount to Decolor", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("color", "Color To Leave", col(1.0, 0.0, 0.0), ParamUi::Color),
                p("tolerance", "Tolerance", num(15.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("edgeSoftness", "Edge Softness", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("matchColors", "Match Colors", Value::Enum(0), popup(&["Using RGB", "Using Hue"])),
            ],
            leave_color,
        ),
        spec(
            "ec.color.photofilter",
            "Photo Filter",
            vec![
                p("filter", "Filter", Value::Enum(0), popup(&PHOTO_FILTERS.map(|f| f.0))),
                p("color", "Color", col(0.93, 0.54, 0.09), ParamUi::Color),
                p("density", "Density", num(25.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("preserveLuminosity", "Preserve Luminosity", Value::Bool(true), ParamUi::Checkbox),
            ],
            photo_filter,
        ),
        spec("ec.color.vibrance", "Vibrance", vec![p("vibrance", "Vibrance", num(0.0), pct()), p("saturation", "Saturation", num(0.0), pct())], vibrance),
        spec(
            "ec.color.colorbalance",
            "Color Balance",
            ["shadowRed", "shadowGreen", "shadowBlue", "midRed", "midGreen", "midBlue", "hiRed", "hiGreen", "hiBlue"]
                .iter()
                .zip([
                    "Shadow Red Balance",
                    "Shadow Green Balance",
                    "Shadow Blue Balance",
                    "Midtone Red Balance",
                    "Midtone Green Balance",
                    "Midtone Blue Balance",
                    "Highlight Red Balance",
                    "Highlight Green Balance",
                    "Highlight Blue Balance",
                ])
                .map(|(id, n)| p(id, n, num(0.0), pct()))
                .chain([p("preserveLuminosity", "Preserve Luminosity", Value::Bool(false), ParamUi::Checkbox)])
                .collect(),
            color_balance,
        ),
        spec(
            "ec.color.channelmixer",
            "Channel Mixer",
            {
                let ids = ["rr", "rg", "rb", "rc", "gr", "gg", "gb", "gc", "br", "bg", "bb", "bc"];
                let names = [
                    "Red-Red",
                    "Red-Green",
                    "Red-Blue",
                    "Red-Const",
                    "Green-Red",
                    "Green-Green",
                    "Green-Blue",
                    "Green-Const",
                    "Blue-Red",
                    "Blue-Green",
                    "Blue-Blue",
                    "Blue-Const",
                ];
                let mut v: Vec<_> = ids
                    .iter()
                    .zip(names)
                    .map(|(id, n)| p(id, n, num(if matches!(*id, "rr" | "gg" | "bb") { 100.0 } else { 0.0 }), slider(-200.0, 200.0, -200.0, 200.0, 0)))
                    .collect();
                v.push(p("monochrome", "Monochrome", Value::Bool(false), ParamUi::Checkbox));
                v
            },
            channel_mixer,
        ),
        spec("ec.color.gammapedestalgain", "Gamma/Pedestal/Gain", gpg, gamma_pg),
        spec(
            "ec.color.colorama",
            "Colorama",
            vec![
                p("inputPhase/mode", "Get Phase From", Value::Enum(0), popup(&COLORAMA_PHASE)),
                p("inputPhase/addPhase", "Add Phase", Value::Layer(None), ParamUi::Layer),
                p("inputPhase/addPhaseFrom", "Add Phase From", Value::Enum(0), popup(&COLORAMA_PHASE)),
                p("inputPhase/addMode", "Add Mode", Value::Enum(0), popup(&["Wrap", "Clamp", "Average", "Screen"])),
                p("inputPhase/phaseShift", "Phase Shift", num(0.0), ParamUi::Angle),
                p("outputCycle/usePresetPalette", "Use Preset Palette", Value::Enum(0), popup(&COLORAMA_PRESETS)),
                // Use Preset Palette = Custom: "position:r,g,b[,a] …" around the wheel.
                p(
                    "outputCycle/palette",
                    "Output Cycle",
                    Value::Str("0:1,0,0 0.1667:1,1,0 0.3333:0,1,0 0.5:0,1,1 0.6667:0,0,1 0.8333:1,0,1".into()),
                    ParamUi::Text,
                ),
                p("outputCycle/cycles", "Cycle Repetitions", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 1)),
                p("outputCycle/interpolatePalette", "Interpolate Palette", Value::Bool(true), ParamUi::Checkbox),
                p("modify/modify", "Modify", Value::Enum(0), popup(&COLORAMA_MODIFY)),
                p("modify/modifyAlpha", "Modify Alpha", Value::Bool(false), ParamUi::Checkbox),
                p("modify/changeEmptyPixels", "Change Empty Pixels", Value::Bool(false), ParamUi::Checkbox),
                p("pixelSelection/matchingColor", "Matching Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("pixelSelection/matchingTolerance", "Matching Tolerance", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("pixelSelection/matchingSoftness", "Matching Softness", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("pixelSelection/matchingMode", "Matching Mode", Value::Enum(0), popup(&["Off", "RGB", "Hue", "Chroma"])),
                p("masking/maskLayer", "Mask Layer", Value::Layer(None), ParamUi::Layer),
                p("masking/maskingMode", "Masking Mode", Value::Enum(0), popup(&["Off", "Luminance", "Inverted Luminance", "Alpha", "Inverted Alpha"])),
                p("masking/compositeOverLayer", "Composite Over Layer", Value::Bool(true), ParamUi::Checkbox),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            colorama,
        ),
    ]
}

#[cfg(test)]
mod tests;
