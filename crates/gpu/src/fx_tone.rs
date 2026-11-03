//! GPU effects, tonal and colour correction family (kernels in `shaders/fx_tone.wgsl`, every
//! entry point prefixed `fxt_`): one per-pixel pass each (`fxt_point`) with the CPU effect's
//! exact maths.
//!
//! Levels (Individual Controls), Gamma/Pedestal/Gain, Photo Filter, Change Color, Change to
//! Color, Leave Color, Broadcast Colors, Color Balance (HLS), Video Limiter, PS Arbitrary Map,
//! CC Toner, CC Color Offset and CC Kernel, plus Hue/Saturation's colour ranges and Levels'
//! per-channel controls (the parts the base kernels in `kernels.wgsl` leave to the CPU). Auto
//! Levels / Contrast / Color and Equalize measure the frame's histograms on the CPU (the image
//! is read back for that) and apply the correction on the GPU.

use effectcraft_color::rgb_to_hsl;
use effectcraft_effects::EffectCtx;

use crate::context::{Enc, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_tone.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxt_point"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.color.levelsic",
    "ec.color.gammapedestalgain",
    "ec.color.photofilter",
    "ec.color.changecolor",
    "ec.color.changetocolor",
    "ec.color.leavecolor",
    "ec.color.broadcast",
    "ec.color.colorbalancehls",
    "ec.color.videolimiter",
    "ec.color.psarbitrarymap",
    "ec.color.cctoner",
    "ec.color.cccoloroffset",
    "ec.color.cckernel",
    "ec.color.autolevels",
    "ec.color.autocontrast",
    "ec.color.autocolor",
    "ec.color.equalize",
    "ec.color.shadowhighlight",
    "ec.color.cccolorneutralizer",
    "ec.color.colorstabilizer",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.color.shadowhighlight" => shadow_highlight(e, ctx, b),
        _ => point(e, id, ctx, b),
    }
}

/// Invert in the HLS and YIQ channels (the base `pointwise` kernel does RGB and Alpha).
pub(crate) fn invert(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let mut p = op(18);
    p.u[0][1] = ctx.params.e("channel");
    p.f[0][0] = 1.0 - ctx.params.f("blend") as f32 / 100.0;
    run(e, &p, b, None)
}

/// Shadow/Highlight: the amounts (Auto Amounts) and the Black / White Clip ranges are measured
/// on the CPU; the luminance bases are blurred and the image adjusted on the GPU.
fn shadow_highlight(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let src = e.download(&b.img)?;
    let (s_amt, h_amt) = effectcraft_effects::shadow_highlight_amounts(ctx, &src);
    let (bc, wc) = (pr.f("moreOptions/blackClip").clamp(0.0, 49.0) / 100.0, pr.f("moreOptions/whiteClip").clamp(0.0, 49.0) / 100.0);
    let clip0 = (bc > 0.0 || wc > 0.0).then(|| effectcraft_effects::luma_clip_points(&src, bc, wc));
    let s_r = pr.f("moreOptions/shadowRadius").max(0.0) * b.scale / 2.0;
    let h_r = pr.f("moreOptions/highlightRadius").max(0.0) * b.scale / 2.0;
    let (w, h) = (b.img.width, b.img.height);
    let luma = e.image(w, h);
    e.pixels("fxt_point", &op(19), &b.img, None, &luma, None);
    let base_s = gaussian_blur(e, &luma, s_r, s_r, true);
    let base_h = if (h_r - s_r).abs() < 1e-9 { base_s.clone() } else { gaussian_blur(e, &luma, h_r, h_r, true) };
    let bases = e.image(w, h);
    e.pixels("fxt_point", &op(21), &base_s, Some(&base_h), &bases, None);
    let mut p = op(20);
    p.f[0] =
        [s_amt, h_amt, (pr.f("moreOptions/shadowTonalWidth") as f32 / 100.0).max(0.01), (pr.f("moreOptions/highlightTonalWidth") as f32 / 100.0).max(0.01)];
    p.f[1] = [
        pr.f("moreOptions/colorCorrection") as f32 / 100.0,
        pr.f("moreOptions/midtoneContrast") as f32 / 100.0,
        (pr.f("blend") / 100.0).clamp(0.0, 1.0) as f32,
        0.0,
    ];
    let out = e.scratch(w, h);
    e.pixels("fxt_point", &p, &b.img, Some(&bases), &out, None);
    let Some((lo0, hi0)) = clip0 else { return Some(GBuf { img: out, ..b }) };
    let adjusted = e.download(&out)?;
    let (lo1, hi1) = effectcraft_effects::luma_clip_points(&adjusted, bc, wc);
    if (lo0, hi0) == (lo1, hi1) || hi1 - lo1 <= 1e-3 {
        return Some(GBuf { img: out, ..b });
    }
    let mut p = op(22);
    p.f[0] = [lo0, lo1, (hi0 - lo0) / (hi1 - lo1), 0.0];
    run(e, &p, GBuf { img: out, ..b }, None)
}

fn rgb(c: [f32; 4]) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

/// Hue/Saturation with colour ranges (Channel Control); Master and Colorize alone run in the
/// base `pointwise` kernel.
pub(crate) fn hue_saturation(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |id: String, d: f64| pr.get(&id).map(|v| v.as_f64()).unwrap_or(d) as f32;
    let mut data = vec![];
    for (r, _, c) in effectcraft_effects::HUESAT_RANGES {
        let range = [
            f(format!("{r}RangeStart"), c - 15.0),
            f(format!("{r}RangeEnd"), c + 15.0),
            f(format!("{r}StartFalloff"), 30.0).max(0.0),
            f(format!("{r}EndFalloff"), 30.0).max(0.0),
            f(format!("{r}Hue"), 0.0) / 360.0,
            f(format!("{r}Saturation"), 0.0) / 100.0,
            f(format!("{r}Lightness"), 0.0) / 100.0,
        ];
        if range[4] != 0.0 || range[5] != 0.0 || range[6] != 0.0 {
            data.extend(range);
        }
    }
    let g = |k: &str| pr.f(k) as f32;
    let mut p = Params::default();
    p.u[0] = [14, pr.b("colorize") as u32, (data.len() / 7) as u32, 0];
    p.f[0] = [g("hue") / 360.0, g("saturation") / 100.0, g("lightness") / 100.0, 0.0];
    p.f[1] = [g("colorizeHue") / 360.0, g("colorizeSaturation") / 100.0, g("colorizeLightness") / 100.0, 0.0];
    run(e, &p, b, Some(data))
}

/// Levels with Red / Green / Blue / Alpha controls (the RGB controls alone run in the base
/// `pointwise` kernel).
pub(crate) fn levels(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let (cb, cw) = effectcraft_effects::levels_clip(ctx);
    let mut data = vec![
        pr.f("inBlack") as f32,
        pr.f("inWhite") as f32,
        pr.f("gamma").max(0.01) as f32,
        pr.f("outBlack") as f32,
        pr.f("outWhite") as f32,
        cb as u32 as f32,
        cw as u32 as f32,
    ];
    for (v, (cb, cw)) in effectcraft_effects::levels_channel_settings(ctx) {
        data.extend(v);
        data.extend([cb as u32 as f32, cw as u32 as f32]);
    }
    run(e, &op(15), b, Some(data))
}

fn op(code: u32) -> Params {
    let mut p = Params::default();
    p.u[0][0] = code;
    p
}

fn run(e: &mut Enc, p: &Params, b: GBuf, data: Option<Vec<f32>>) -> Option<GBuf> {
    let buf = data.map(|d| e.data(&d));
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxt_point", p, &b.img, None, &out, buf.as_ref());
    Some(GBuf { img: out, ..b })
}

/// color3::parse_map.
fn parse_map(s: &str) -> Vec<f32> {
    let vals: Vec<f32> = s.split(|ch: char| ch == ',' || ch.is_whitespace()).filter(|t| !t.is_empty()).filter_map(|t| t.parse::<f32>().ok()).collect();
    if vals.len() < 2 {
        return (0..256).map(|i| i as f32 / 255.0).collect();
    }
    let max = vals.iter().cloned().fold(0.0f32, f32::max);
    let k = if max > 1.0 { 1.0 / 255.0 } else { 1.0 };
    vals.iter().map(|v| (v * k).clamp(0.0, 1.0)).collect()
}

/// Piecewise-linear lookup in the CPU effect's f64 phase domain. Store the
/// first representable f32 at each boundary, its value and the interval slope.
/// Computing floor/wrap in shader f32 can select the end of the map instead of
/// its beginning at a wrap (a full-scale colour error).
fn arbitrary_map_table(map: &[f32], phase: f64) -> Option<Vec<f32>> {
    let n = map.len();
    if !(2..=4096).contains(&n) || !phase.is_finite() {
        return None;
    }
    let scale = (n - 1) as f64;
    let offset = phase * scale / 255.0;
    if !offset.is_finite() || offset.abs() >= (1u64 << 52) as f64 {
        return None;
    }
    let at = |v: f32| {
        let x = (v as f64 * scale + offset).rem_euclid(n as f64);
        let i = x.floor() as usize;
        let a = *map.get(i).unwrap_or(&0.0);
        let b = *map.get((i + 1).min(n - 1)).unwrap_or(&a);
        (a + (b - a) * (x - x.floor()) as f32, (b - a) * scale as f32)
    };
    let mut points = vec![0.0f32];
    // Search ordered positive f32 encodings using the CPU's actual f64 sum.
    // Merely rounding a mathematical boundary upward misses the half-ulp
    // region where f64 addition itself rounds across a wrap.
    let first = offset.floor() + 1.0;
    for i in 0..=n {
        let boundary = first + i as f64;
        if boundary > scale + offset {
            break;
        }
        let (mut lo, mut hi) = (0u32, 1.0f32.to_bits());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if f32::from_bits(mid) as f64 * scale + offset < boundary {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let start = f32::from_bits(lo);
        if points.last().is_none_or(|last| start > *last) {
            points.push(start);
        }
    }
    if points.last() != Some(&1.0) {
        points.push(1.0);
    }
    let mut table = Vec::with_capacity(points.len() * 3);
    for v in points {
        let (value, slope) = at(v);
        table.extend([v, value, slope]);
    }
    Some(table)
}

fn point(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |k: &str| pr.f(k) as f32;
    let mut p = Params::default();
    let mut data = None;
    match id {
        "ec.color.levelsic" => {
            const IDS: [[&str; 5]; 5] = [
                ["rgb/rgbInBlack", "rgb/rgbInWhite", "rgb/rgbGamma", "rgb/rgbOutBlack", "rgb/rgbOutWhite"],
                ["red/redInBlack", "red/redInWhite", "red/redGamma", "red/redOutBlack", "red/redOutWhite"],
                ["green/greenInBlack", "green/greenInWhite", "green/greenGamma", "green/greenOutBlack", "green/greenOutWhite"],
                ["blue/blueInBlack", "blue/blueInWhite", "blue/blueGamma", "blue/blueOutBlack", "blue/blueOutWhite"],
                ["alpha/alphaInBlack", "alpha/alphaInWhite", "alpha/alphaGamma", "alpha/alphaOutBlack", "alpha/alphaOutWhite"],
            ];
            let lv: Vec<[f32; 5]> = IDS.iter().map(|ids| ids.map(f)).collect();
            let ident = |l: &[f32; 5]| *l == [0.0, 1.0, 1.0, 0.0, 1.0];
            if lv.iter().all(ident) {
                return Some(b);
            }
            let (cb, cw) = effectcraft_effects::levels_clip(ctx);
            let mut d = vec![];
            for l in &lv {
                d.extend(l);
                d.push(ident(l) as u32 as f32);
            }
            p.u[0] = [1, cb as u32, cw as u32, 0];
            data = Some(d);
        }
        "ec.color.gammapedestalgain" => {
            let ch = |n: &str| [f(&format!("{n}Gamma")).max(0.01), f(&format!("{n}Pedestal")), f(&format!("{n}Gain")), 0.0];
            p.u[0][0] = 2;
            p.f[0] = ch("red");
            p.f[1] = ch("green");
            p.f[2] = ch("blue");
            p.f[3][0] = f("blackStretch").max(0.0);
        }
        "ec.color.photofilter" => {
            let fi = pr.e("filter");
            let c = match effectcraft_effects::PHOTO_FILTERS.get(fi as usize) {
                Some((_, rgb)) if fi != effectcraft_effects::PHOTO_FILTER_CUSTOM => rgb.map(|v| v as f32 / 255.0),
                _ => {
                    let c = pr.color("color");
                    [c[0], c[1], c[2]]
                }
            };
            p.u[0] = [3, pr.b("preserveLuminosity") as u32, 0, 0];
            p.f[0] = [c[0], c[1], c[2], f("density") / 100.0];
        }
        "ec.color.changetocolor" => {
            let from = pr.color("from");
            let to = pr.color("to");
            let (fh, fs, fl) = rgb_to_hsl(from[0], from[1], from[2]);
            let (th, ts, tl) = rgb_to_hsl(to[0], to[1], to[2]);
            let change = pr.e("change");
            let flags = matches!(change, 1 | 3) as u32
                | (matches!(change, 2 | 3) as u32) << 1
                | ((pr.e("changeBy") == 1) as u32) << 2
                | (pr.b("viewCorrectionMatte") as u32) << 3;
            p.u[0] = [4, flags, 0, 0];
            p.f[0] = [fh, fs, fl, f("softness") / 100.0];
            p.f[1] = [th, ts, tl, 0.0];
            p.f[2] = [f("toleranceGroup/hue") / 100.0, f("toleranceGroup/lightness") / 100.0, f("toleranceGroup/saturation") / 100.0, 0.0];
        }
        "ec.color.leavecolor" => {
            let amt = f("amount") / 100.0;
            if amt <= 0.0 {
                return Some(b);
            }
            let k = pr.color("color");
            p.u[0] = [5, (pr.e("matchColors") == 1) as u32, 0, 0];
            p.f[0] = [k[0], k[1], k[2], rgb_to_hsl(k[0], k[1], k[2]).0];
            p.f[1] = [amt, f("tolerance") / 100.0, f("edgeSoftness") / 100.0, 0.0];
        }
        "ec.color.changecolor" => {
            let view_mask = pr.e("view") == 1;
            let (dh, dl, ds) = (f("hueTransform") / 360.0, f("lightnessTransform") / 100.0, f("saturationTransform") / 100.0);
            if !view_mask && dh == 0.0 && dl == 0.0 && ds == 0.0 {
                return Some(b);
            }
            let k = pr.color("colorToChange");
            p.u[0] = [6, pr.e("matchColors"), view_mask as u32 | (pr.b("invertColorCorrectionMask") as u32) << 1, 0];
            p.f[0] = [k[0], k[1], k[2], rgb_to_hsl(k[0], k[1], k[2]).0];
            p.f[1] = [dh, dl, ds, 0.0];
            p.f[2] = [f("matchingTolerance") / 100.0, f("matchingSoftness") / 100.0, 0.0, 0.0];
        }
        "ec.color.broadcast" => {
            let pal = pr.e("locale") == 1;
            let (setup, span) = if pal { (0.0, 100.0) } else { (7.5, 92.5) };
            p.u[0] = [7, pal as u32, pr.e("howToMakeSafe"), 0];
            p.f[0][0] = (f("maxSignal") - setup) / span;
        }
        "ec.color.colorbalancehls" => {
            let (dh, dl, ds) = (f("hue") / 360.0, f("lightness") / 100.0, f("saturation") / 100.0);
            if dh == 0.0 && dl == 0.0 && ds == 0.0 {
                return Some(b);
            }
            p.u[0][0] = 8;
            p.f[0] = [dh, dl, ds, 0.0];
        }
        "ec.color.cctoner" => {
            let g = |id: &str| rgb(pr.color(id));
            let stops = match pr.e("tones") {
                0 => vec![g("shadows"), g("highlights")],
                2 => vec![g("shadows"), g("darktones"), g("midtones"), g("brights"), g("highlights")],
                _ => vec![g("shadows"), g("midtones"), g("highlights")],
            };
            p.u[0] = [9, stops.len() as u32 - 1, 0, 0];
            p.f[0][0] = (f("blend") / 100.0).clamp(0.0, 1.0);
            for (i, s) in stops.iter().enumerate() {
                p.f[1 + i] = *s;
            }
        }
        "ec.color.cccoloroffset" => {
            let offs = ["redPhase", "greenPhase", "bluePhase"].map(|id| (pr.f(id) / 360.0).rem_euclid(1.0) as f32);
            if offs.iter().all(|o| *o == 0.0) {
                return Some(b);
            }
            p.u[0] = [10, pr.e("overflow"), 0, 0];
            p.f[0] = [offs[0], offs[1], offs[2], 0.0];
        }
        "ec.color.cckernel" => {
            let k = ["k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8", "k9"].map(f);
            let (scale, offset) = (f("scale"), f("offset"));
            let identity = k.iter().enumerate().all(|(i, v)| if i == 4 { *v == 1.0 } else { *v == 0.0 });
            if identity && scale == 1.0 && offset == 0.0 {
                return Some(b);
            }
            p.u[0] = [11, pr.b("absolute") as u32, 0, 0];
            p.f[0] = [k[0], k[1], k[2], k[3]];
            p.f[1] = [k[4], k[5], k[6], k[7]];
            p.f[2] = [k[8], scale, offset, 0.0];
        }
        "ec.color.psarbitrarymap" => {
            let map = parse_map(pr.s("map"));
            let table = arbitrary_map_table(&map, pr.f("phase"))?;
            p.u[0] = [12, pr.b("applyPhaseMapToAlpha") as u32, (table.len() / 3) as u32, 0];
            data = Some(table);
        }
        "ec.color.videolimiter" => {
            const CLIP_LEVELS: [f32; 5] = [0.9, 0.95, 1.0, 1.05, 1.09];
            let level = CLIP_LEVELS[(pr.e("clipLevel") as usize).min(4)];
            let comp = [0.0, 0.03, 0.05, 0.1, 0.2][(pr.e("compressionBeforeClipping") as usize).min(4)];
            p.u[0] = [13, pr.e("clipMethod"), pr.b("gamutWarning") as u32, 0];
            p.f[0] = [level, level * (1.0 - comp), 0.0, 0.0];
            p.f[1] = rgb(pr.color("gamutWarningColor"));
        }
        "ec.color.colorstabilizer" => {
            // The sample means are measured on the CPU (the frame is read back).
            if ctx.env.host.is_none() {
                return Some(b);
            }
            let img = e.download(&b.img)?;
            let cpu = effectcraft_effects::Buf { img, offset: b.offset, scale: b.scale };
            let Some(maps) = effectcraft_effects::color_stabilizer_maps(ctx, &cpu) else { return Some(b) };
            let mut d = vec![];
            for m in &maps {
                d.push(m.len() as f32);
                for i in 0..5 {
                    let (x, y) = m.get(i).copied().unwrap_or((0.0, 0.0));
                    d.extend([x as f32, y as f32]);
                }
            }
            p.u[0][0] = 24;
            data = Some(d);
        }
        "ec.color.cccolorneutralizer" => {
            let pairs = [("shadowsUnbalance", "shadowsBalance"), ("midtonesUnbalance", "midtonesBalance"), ("highlightsUnbalance", "highlightsBalance")];
            for (i, (u, bal)) in pairs.iter().enumerate() {
                let (u, v) = (pr.color(u), pr.color(bal));
                p.f[i] = [v[0] - u[0], v[1] - u[1], v[2] - u[2], 0.0];
            }
            p.u[0][0] = 23;
            p.f[3] = [f("pinning") / 100.0, f("blendWOriginal") / 100.0, f("contrast") / 100.0, 0.0];
            p.f[4] = [f("darks") / 100.0, f("brights") / 100.0, 0.0, 0.0];
        }
        "ec.color.autolevels" | "ec.color.autocontrast" | "ec.color.autocolor" | "ec.color.equalize" => {
            // The histograms are measured on the CPU.
            let img = e.download(&b.img)?;
            if id == "ec.color.equalize" {
                let amt = (pr.f("amount") / 100.0).clamp(0.0, 1.0) as f32;
                if amt <= 0.0 {
                    return Some(b);
                }
                let style = pr.e("style");
                let tables = effectcraft_effects::equalize_tables(&img, style);
                p.u[0] = [17, style, tables.first().map_or(0, |t| t.len()) as u32, 0];
                p.f[0][0] = amt;
                data = Some(tables.concat());
            } else {
                let kind = match id {
                    "ec.color.autolevels" => 0,
                    "ec.color.autocontrast" => 1,
                    _ => 2,
                };
                let (ranges, gam) = effectcraft_effects::auto_correct_settings(ctx, &img, kind);
                p.u[0][0] = 16;
                p.f[0] = [ranges[0].0, ranges[1].0, ranges[2].0, 0.0];
                p.f[1] = [ranges[0].1, ranges[1].1, ranges[2].1, 0.0];
                p.f[2] = [gam[0], gam[1], gam[2], (pr.f("blend") / 100.0).clamp(0.0, 1.0) as f32];
            }
        }
        _ => return None,
    }
    run(e, &p, b, data)
}
