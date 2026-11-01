//! Keying effects: colour, luma, difference, range and screen keyers plus spill suppression and
//! matte clean-up. Written from public behaviour descriptions and the standard colour-difference /
//! blue-screen matting literature (Vlahos; Smith & Blinn, "Blue Screen Matting").

use effectcraft_color::{rgb_to_hsl, srgb_to_linear};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, gaussian_blur};
use rayon::prelude::*;

use crate::color_fx::luma_601;
use crate::util::{Plane, gauss_plane, morph_frac, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, category: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category, params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn view_popup() -> ParamUi {
    popup(&["Final Output", "Source Only", "Matte Only"])
}

/// Apply a keyed matte `m` (1 = keep) to the image according to the view mode.
fn apply_matte(img: &mut Image, m: &Plane, view: u32) {
    if view == 1 {
        return;
    }
    img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(px, &k)| {
        let k = k.clamp(0.0, 1.0);
        if view == 2 {
            let a = px[3] * k;
            *px = [a, a, a, 1.0];
        } else {
            for c in px.iter_mut() {
                *c *= k;
            }
        }
    });
}

/// Edge thin (positive erodes, negative grows) and feather (Gaussian) of a matte, in buffer pixels.
fn thin_feather(m: Plane, thin: f64, feather: f64) -> Plane {
    let m = if thin > 0.0 {
        morph_frac(&m, thin, false)
    } else if thin < 0.0 {
        morph_frac(&m, -thin, true)
    } else {
        m
    };
    if feather > 0.0 { gauss_plane(&m, feather * 0.5, feather * 0.5) } else { m }
}

/// Plane of `f(straight colour, alpha)` over the image.
fn matte_of(img: &Image, f: impl Fn([f32; 3], f32) -> f32 + Sync) -> Plane {
    Plane::from_image(img, |px| {
        let (c, a) = unpremul(px);
        f(c, a)
    })
}

/// Index of the largest component and the other two.
fn primary(c: [f32; 4]) -> (usize, usize, usize) {
    if c[1] >= c[0] && c[1] >= c[2] {
        (1, 0, 2)
    } else if c[2] >= c[0] && c[2] >= c[1] {
        (2, 0, 1)
    } else {
        (0, 1, 2)
    }
}

// ---- Linear Color Key ----

fn hue_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (ha, sa, _) = rgb_to_hsl(a[0], a[1], a[2]);
    let (hb, _, _) = rgb_to_hsl(b[0], b[1], b[2]);
    if sa < 0.02 {
        return 1.0;
    }
    let d = (ha - hb).abs();
    d.min(1.0 - d) * 2.0
}

fn chroma(c: [f32; 3]) -> [f32; 2] {
    let s = c[0] + c[1] + c[2];
    if s <= 1e-6 { [1.0 / 3.0, 1.0 / 3.0] } else { [c[0] / s, c[1] / s] }
}

fn linear_color_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.color("keyColor");
    let key = [k[0], k[1], k[2]];
    let mode = ctx.params.e("matchColors");
    let tol = ctx.params.f("tolerance") as f32 / 100.0;
    let soft = ctx.params.f("softness") as f32 / 100.0;
    let keep = ctx.params.e("keyOperation") == 1;
    let kc = chroma(key);
    let m = matte_of(&b.img, |c, _| {
        let d = match mode {
            1 => hue_dist(c, key),
            2 => {
                let pc = chroma(c);
                ((pc[0] - kc[0]).powi(2) + (pc[1] - kc[1]).powi(2)).sqrt() / std::f32::consts::SQRT_2
            }
            _ => ((c[0] - key[0]).powi(2) + (c[1] - key[1]).powi(2) + (c[2] - key[2]).powi(2)).sqrt() / 3f32.sqrt(),
        };
        let v = if soft > 1e-6 {
            ((d - tol) / soft).clamp(0.0, 1.0)
        } else if d > tol {
            1.0
        } else {
            0.0
        };
        if keep { 1.0 - v } else { v }
    });
    apply_matte(&mut b.img, &m, ctx.params.e("view"));
    b
}

// ---- Color Key (obsolete) ----

fn color_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.color("keyColor");
    let tol = ctx.params.f("colorTolerance") as f32 / 255.0;
    let m = matte_of(&b.img, |c, _| {
        let d = (c[0] - k[0]).abs().max((c[1] - k[1]).abs()).max((c[2] - k[2]).abs());
        if d <= tol { 0.0 } else { 1.0 }
    });
    let m = thin_feather(m, ctx.params.f("edgeThin") * b.scale, ctx.params.f("edgeFeather") * b.scale);
    apply_matte(&mut b.img, &m, 0);
    b
}

// ---- Luma Key ----

/// Luma Key as After Effects 2026 keys a grey ramp (cases 812-814, 719): Key Out Brighter and
/// Darker cut hard at Threshold and ignore Tolerance; Key Out Similar removes, and Dissimilar
/// keeps, a hard band of Threshold ± Tolerance (levels of 255). Edge Thin and Edge Feather
/// then soften the cut.
fn luma_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let kind = ctx.params.e("keyType");
    let thr = ctx.params.f("threshold") as f32 / 255.0;
    let tol = ctx.params.f("tolerance") as f32 / 255.0;
    let m = matte_of(&b.img, |c, _| {
        let l = luma_601([c[0], c[1], c[2]]);
        let keep = match kind {
            0 => l <= thr,
            2 => (l - thr).abs() > tol,
            3 => (l - thr).abs() <= tol,
            _ => l >= thr,
        };
        if keep { 1.0 } else { 0.0 }
    });
    let m = thin_feather(m, ctx.params.f("edgeThin") * b.scale, ctx.params.f("edgeFeather") * b.scale);
    apply_matte(&mut b.img, &m, 0);
    b
}

// ---- Color Difference Key ----

/// Color Difference Key's View options.
pub const COLOR_DIFF_VIEWS: [&str; 9] = [
    "Source",
    "Uncorrected Matte Partial A",
    "Corrected Matte Partial A",
    "Uncorrected Matte Partial B",
    "Corrected Matte Partial B",
    "Uncorrected Matte",
    "Corrected Matte",
    "Final Output",
    "[A, B, Matte] Corrected, Final",
];

/// Levels on a 0..1 matte value: input black / white (0..255), gamma, output black / white.
fn matte_levels(v: f32, in_b: f32, in_w: f32, gamma: f32, out_b: f32, out_w: f32) -> f32 {
    let t = ((v - in_b / 255.0) / ((in_w - in_b) / 255.0).max(1e-4)).clamp(0.0, 1.0);
    let t = t.powf(1.0 / gamma.max(0.01));
    (out_b / 255.0 + t * (out_w - out_b) / 255.0).clamp(0.0, 1.0)
}

/// The key splits the colour difference between the key's primary channel and each of the two
/// other channels into two partial mattes (A: first other channel, B: second); their union is
/// the alpha matte. Each partial and the combined matte have their own levels controls.
fn color_difference_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let view = pr.e("view");
    if view == 0 {
        return b;
    }
    let accurate = pr.e("colorMatchingAccuracy") == 1;
    let lin = |c: [f32; 3]| if accurate { c.map(|v| srgb_to_linear(v.clamp(0.0, 1.0))) } else { c };
    let k = pr.color("keyColor");
    let key = lin([k[0], k[1], k[2]]);
    let (pi, o1, o2) = primary([key[0], key[1], key[2], 1.0]);
    let kd1 = (key[pi] - key[o1]).max(1e-3);
    let kd2 = (key[pi] - key[o2]).max(1e-3);
    let lv = |pre: &str| {
        let g = |s: &str| pr.f(&format!("{pre}{s}")) as f32;
        (g("InBlack"), g("InWhite"), g("Gamma"), g("OutBlack"), g("OutWhite"))
    };
    let pa = lv("partialA");
    let pb = lv("partialB");
    let (mb, mw, mg) = (pr.f("blackLevel") as f32, pr.f("whiteLevel") as f32, pr.f("gamma") as f32);
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let src = b.img.clone();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a0) = unpremul(src.data[i]);
        let c = lin(c);
        let ua = 1.0 - ((c[pi] - c[o1]) / kd1).clamp(0.0, 1.0);
        let ub = 1.0 - ((c[pi] - c[o2]) / kd2).clamp(0.0, 1.0);
        let ca = matte_levels(ua, pa.0, pa.1, pa.2, pa.3, pa.4);
        let cb = matte_levels(ub, pb.0, pb.1, pb.2, pb.3, pb.4);
        let um = ua.max(ub);
        let cm = matte_levels(ca.max(cb), mb, mw, mg, 0.0, 255.0);
        // The four-up view: Partial A, Partial B, Matte (all corrected) and the final output.
        let v = if view == 8 {
            let (x, y) = (i % w.max(1), i / w.max(1));
            match (x * 2 >= w, y * 2 >= h) {
                (false, false) => 2,
                (true, false) => 4,
                (false, true) => 6,
                (true, true) => 7,
            }
        } else {
            view
        };
        let grey = |m: f32| {
            let m = m * a0;
            [m, m, m, 1.0]
        };
        *px = match v {
            1 => grey(ua),
            2 => grey(ca),
            3 => grey(ub),
            4 => grey(cb),
            5 => grey(um),
            6 => grey(cm),
            _ => src.data[i].map(|v| v * cm),
        };
    });
    b
}

// ---- Color Range ----

fn to_lab(c: [f32; 3]) -> [f32; 3] {
    let l = c.map(|v| srgb_to_linear(v.clamp(0.0, 1.0)));
    let x = (0.4124 * l[0] + 0.3576 * l[1] + 0.1805 * l[2]) / 0.95047;
    let y = 0.2126 * l[0] + 0.7152 * l[1] + 0.0722 * l[2];
    let z = (0.0193 * l[0] + 0.1192 * l[1] + 0.9505 * l[2]) / 1.08883;
    let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Colour in the chosen space as three 0..255 components.
fn range_components(space: u32, c: [f32; 3]) -> [f32; 3] {
    match space {
        1 => {
            let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
            let u = -0.147_13 * c[0] - 0.288_86 * c[1] + 0.436 * c[2];
            let v = 0.615 * c[0] - 0.514_99 * c[1] - 0.100_01 * c[2];
            [y * 255.0, u * 255.0 + 128.0, v * 255.0 + 128.0]
        }
        2 => c.map(|v| v * 255.0),
        _ => {
            let lab = to_lab(c);
            [lab[0] * 2.55, lab[1] + 128.0, lab[2] + 128.0]
        }
    }
}

fn color_range(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let space = ctx.params.e("colorSpace");
    let lo = ["minL", "minA", "minB"].map(|i| ctx.params.f(i) as f32);
    let hi = ["maxL", "maxA", "maxB"].map(|i| ctx.params.f(i) as f32);
    let fuzz = ctx.params.f("fuzziness") as f32;
    let m = matte_of(&b.img, |c, _| {
        let v = range_components(space, c);
        let mut out = 0.0f32;
        for i in 0..3 {
            out = out.max(lo[i] - v[i]).max(v[i] - hi[i]);
        }
        if out <= 0.0 {
            0.0
        } else if fuzz > 0.0 {
            (out / fuzz).clamp(0.0, 1.0)
        } else {
            1.0
        }
    });
    apply_matte(&mut b.img, &m, 0);
    b
}

// ---- Extract ----

fn extract(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ch = ctx.params.e("channel");
    let bp = ctx.params.f("blackPoint") as f32;
    let wp = ctx.params.f("whitePoint") as f32;
    let bs = ctx.params.f("blackSoftness") as f32;
    let ws = ctx.params.f("whiteSoftness") as f32;
    let inv = ctx.params.b("invert");
    let m = matte_of(&b.img, |c, a| {
        let v = 255.0
            * match ch {
                1 => c[0],
                2 => c[1],
                3 => c[2],
                4 => a,
                _ => luma_601([c[0], c[1], c[2]]),
            };
        let mut k = 1.0f32;
        if v < bp {
            k = if bs > 0.0 { (1.0 - (bp - v) / bs).clamp(0.0, 1.0) } else { 0.0 };
        } else if v > wp {
            k = if ws > 0.0 { (1.0 - (v - wp) / ws).clamp(0.0, 1.0) } else { 0.0 };
        }
        if inv { 1.0 - k } else { k }
    });
    apply_matte(&mut b.img, &m, 0);
    b
}

// ---- Screen Key (Keylight-style) ----

/// Secondary-channel estimate: `balance` weights the larger of the two secondaries.
#[inline]
fn secondary(c: [f32; 3], o1: usize, o2: usize, bal: f32) -> f32 {
    let (hi, lo) = if c[o1] > c[o2] { (c[o1], c[o2]) } else { (c[o2], c[o1]) };
    bal * hi + (1.0 - bal) * lo
}

fn screen_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let view = ctx.params.e("view");
    if view == 3 {
        return b;
    }
    let sc = ctx.params.color("screenColor");
    let screen = [sc[0], sc[1], sc[2]];
    let (pi, o1, o2) = primary(sc);
    let gain = ctx.params.f("screenGain") as f32 / 100.0;
    let bal = ctx.params.f("screenBalance") as f32 / 100.0;
    let despill = ctx.params.f("despill") as f32 / 100.0;
    let cb = ctx.params.f("clipBlack") as f32 / 100.0;
    let cw = ctx.params.f("clipWhite") as f32 / 100.0;
    let preblur = ctx.params.f("screenPreblur") * b.scale;
    let sg = ctx.params.f("screenShrinkGrow") * b.scale;
    let soft = ctx.params.f("screenSoftness") * b.scale;
    let source_edges = ctx.params.e("edgeColor") == 1;
    let sd = (screen[pi] - secondary(screen, o1, o2, bal)).max(1e-3);

    let blurred;
    let key_src = if preblur > 0.0 {
        blurred = gaussian_blur(&b.img, preblur * 0.5, preblur * 0.5, true);
        &blurred
    } else {
        &b.img
    };
    let mut m = matte_of(key_src, |c, _| {
        let d = c[pi] - secondary(c, o1, o2, bal);
        let t = (d / sd * gain).clamp(0.0, 1.0);
        ((1.0 - t - cb) / (cw - cb).max(1e-3)).clamp(0.0, 1.0)
    });
    if sg < 0.0 {
        m = morph_frac(&m, -sg, false);
    } else if sg > 0.0 {
        m = morph_frac(&m, sg, true);
    }
    if soft > 0.0 {
        m = gauss_plane(&m, soft * 0.5, soft * 0.5);
    }
    b.img.data.par_iter_mut().zip(m.data.par_iter()).for_each(|(px, &k)| {
        let k = k.clamp(0.0, 1.0);
        let (c, a0) = unpremul(*px);
        let alpha = a0 * k;
        match view {
            1 => *px = [alpha, alpha, alpha, 1.0],
            2 => {
                let v = if alpha <= 1e-3 {
                    0.0
                } else if alpha >= 0.999 {
                    1.0
                } else {
                    0.5
                };
                *px = [v, v, v, 1.0];
            }
            _ => {
                let mut fg = if source_edges || k <= 1e-4 {
                    c
                } else {
                    let s = 1.0 - k;
                    [0, 1, 2].map(|i| ((c[i] - s * screen[i]).max(0.0) / k).min(c[i].max(1.0)))
                };
                let spill = (fg[pi] - secondary(fg, o1, o2, bal)).max(0.0);
                fg[pi] -= spill * despill;
                *px = premul(fg, alpha);
            }
        }
    });
    b
}

// ---- Spill suppression ----

/// Faster: limit the key's primary channel to the larger of the other two. Better: remove the
/// part of each pixel's chroma that points along the key colour's chroma (works for any key
/// colour, not only primaries).
fn spill_suppressor(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let k = ctx.params.color("colorToSuppress");
    let amt = ctx.params.f("suppression") as f32 / 100.0;
    if ctx.params.e("colorAccuracy") == 1 {
        let kg = (k[0] + k[1] + k[2]) / 3.0;
        let kc = [k[0] - kg, k[1] - kg, k[2] - kg];
        let kn = (kc[0] * kc[0] + kc[1] * kc[1] + kc[2] * kc[2]).sqrt();
        if kn < 1e-5 {
            return b;
        }
        let dir = kc.map(|v| v / kn);
        b.img.map_straight(|c| {
            let g = (c[0] + c[1] + c[2]) / 3.0;
            let proj = (c[0] - g) * dir[0] + (c[1] - g) * dir[1] + (c[2] - g) * dir[2];
            if proj <= 0.0 {
                return c;
            }
            [0, 1, 2].map(|i| c[i] - proj * dir[i] * amt)
        });
        return b;
    }
    let (pi, o1, o2) = primary(k);
    b.img.map_straight(|mut c| {
        let spill = (c[pi] - c[o1].max(c[o2])).max(0.0);
        c[pi] -= spill * amt;
        c
    });
    b
}

const LUMA_W: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn advanced_spill(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ultra = ctx.params.e("method") == 1;
    let pi = if ultra {
        primary(ctx.params.color("ultraSettings/keyColor")).0
    } else {
        // Dominant key primary: the channel with the largest total excess over the other two.
        let ex = b
            .img
            .data
            .par_iter()
            .map(|px| {
                let (c, a) = unpremul(*px);
                [(c[0] - c[1].max(c[2])).max(0.0) * a, (c[1] - c[0].max(c[2])).max(0.0) * a, (c[2] - c[0].max(c[1])).max(0.0) * a]
            })
            .reduce(|| [0.0; 3], |x, y| [x[0] + y[0], x[1] + y[1], x[2] + y[2]]);
        if ex[1] >= ex[0] && ex[1] >= ex[2] {
            1
        } else if ex[2] >= ex[0] {
            2
        } else {
            0
        }
    };
    let (o1, o2) = match pi {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let amt = ctx.params.f("suppression") as f32 / 100.0;
    let range = ctx.params.f("ultraSettings/spillRange") as f32 / 100.0;
    let luma = ctx.params.f("ultraSettings/lumaCorrection") as f32 / 100.0;
    // Ultra: Tolerance limits suppression to hues near the key colour; Desaturate takes the
    // colour out of suppressed pixels; Spill Color Correction puts the removed spill back as
    // neutral light (so suppressed areas keep their brightness without the cast).
    let key = ctx.params.color("ultraSettings/keyColor");
    let key_hue = effectcraft_color::rgb_to_hsl(key[0], key[1], key[2]).0;
    let tol = if ultra { ctx.params.get("ultraSettings/tolerance").map(Value::as_f64).unwrap_or(100.0) as f32 / 100.0 } else { 1.0 };
    let desat = if ultra { ctx.params.f("ultraSettings/desaturate") as f32 / 100.0 } else { 0.0 };
    let neutral = if ultra { ctx.params.f("ultraSettings/spillColorCorrection") as f32 / 100.0 } else { 0.0 };
    b.img.map_straight(|mut c| {
        let w = if tol < 1.0 {
            let h = effectcraft_color::rgb_to_hsl(c[0], c[1], c[2]).0;
            let d = (h - key_hue).abs();
            let d = d.min(1.0 - d);
            (1.0 - d / (tol * 0.5).max(1e-4)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mx = c[o1].max(c[o2]);
        let avg = 0.5 * (c[o1] + c[o2]);
        let limit = mx + (avg - mx) * range;
        let spill = (c[pi] - limit).max(0.0) * amt * w;
        c[pi] -= spill;
        let back = spill * LUMA_W[pi] * luma + spill * neutral / 3.0;
        let mut c = c.map(|v| v + back);
        if desat > 0.0 && spill > 0.0 {
            let l = effectcraft_color::luminance(c[0], c[1], c[2]);
            let k = (desat * (spill * 4.0).min(1.0)).min(1.0);
            c = c.map(|v| v + (l - v) * k);
        }
        c
    });
    b
}

// ---- Key Cleaner (approximation) ----

fn key_cleaner(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("additionalEdgeRadius") * b.scale).max(0.0);
    let contrast = ctx.params.f("alphaContrast") as f32 / 100.0;
    let strength = ctx.params.f("strength") as f32 / 100.0;
    let mut a = Plane::alpha(&b.img);
    if ctx.params.b("reduceChatter") {
        a = gauss_plane(&a, 1.0, 1.0).map(|v| ((v - 0.5) * 1.5 + 0.5).clamp(0.0, 1.0));
    }
    let est = if r > 0.0 && strength > 0.0 { Some(gaussian_blur(&b.img, r * 0.5, r * 0.5, true)) } else { None };
    b.img.data.par_iter_mut().zip(a.data.par_iter()).enumerate().for_each(|(i, (px, &na))| {
        let (mut c, a0) = unpremul(*px);
        if let Some(e) = &est
            && a0 > 1e-4
            && a0 < 0.999
        {
            let (ec, ea) = unpremul(e.data[i]);
            if ea > 1e-4 {
                let t = strength * (1.0 - a0);
                c = [0, 1, 2].map(|k| c[k] + (ec[k] - c[k]) * t);
            }
        }
        let na = ((na - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
        let na = if a0 <= 1e-6 { 0.0 } else { na };
        *px = premul(c, na);
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let thin = || slider(-5.0, 5.0, -5.0, 5.0, 1);
    let feather = || slider(0.0, 100.0, 0.0, 20.0, 1);
    let b255 = || slider(0.0, 255.0, 0.0, 255.0, 0);
    let gamma = || slider(0.01, 10.0, 0.1, 3.0, 2);
    vec![
        spec(
            "ec.key.linearcolor",
            "Linear Color Key",
            "Keying",
            vec![
                p("view", "View", Value::Enum(0), view_popup()),
                p("keyColor", "Key Color", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("matchColors", "Match colors", Value::Enum(0), popup(&["Using RGB", "Using Hue", "Using Chroma"])),
                p("tolerance", "Matching Tolerance", num(10.0), pct()),
                p("softness", "Matching Softness", num(10.0), pct()),
                p("keyOperation", "Key Operation", Value::Enum(0), popup(&["Key Colors", "Keep Colors"])),
            ],
            linear_color_key,
        ),
        spec(
            "ec.key.colorkey",
            "Color Key",
            "Obsolete",
            vec![
                p("keyColor", "Key Color", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("colorTolerance", "Color Tolerance", num(0.0), b255()),
                p("edgeThin", "Edge Thin", num(0.0), thin()),
                p("edgeFeather", "Edge Feather", num(0.0), feather()),
            ],
            color_key,
        ),
        spec(
            "ec.key.luma",
            "Luma Key",
            "Obsolete",
            vec![
                p("keyType", "Key Type", Value::Enum(1), popup(&["Key Out Brighter", "Key Out Darker", "Key Out Similar", "Key Out Dissimilar"])),
                p("threshold", "Threshold", num(0.0), b255()),
                p("tolerance", "Tolerance", num(0.0), b255()),
                p("edgeThin", "Edge Thin", num(0.0), thin()),
                p("edgeFeather", "Edge Feather", num(0.0), feather()),
            ],
            luma_key,
        ),
        spec(
            "ec.key.colordifference",
            "Color Difference Key",
            "Keying",
            vec![
                p("view", "View", Value::Enum(7), popup(&COLOR_DIFF_VIEWS)),
                p("keyColor", "Key Color", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("colorMatchingAccuracy", "Color Matching Accuracy", Value::Enum(0), popup(&["Faster", "More Accurate"])),
                p("partialAInBlack", "Partial A In Black", num(0.0), b255()),
                p("partialAInWhite", "Partial A In White", num(255.0), b255()),
                p("partialAGamma", "Partial A Gamma", num(1.0), gamma()),
                p("partialAOutBlack", "Partial A Out Black", num(0.0), b255()),
                p("partialAOutWhite", "Partial A Out White", num(255.0), b255()),
                p("partialBInBlack", "Partial B In Black", num(0.0), b255()),
                p("partialBInWhite", "Partial B In White", num(255.0), b255()),
                p("partialBGamma", "Partial B Gamma", num(1.0), gamma()),
                p("partialBOutBlack", "Partial B Out Black", num(0.0), b255()),
                p("partialBOutWhite", "Partial B Out White", num(255.0), b255()),
                p("blackLevel", "Matte In Black", num(0.0), b255()),
                p("whiteLevel", "Matte In White", num(255.0), b255()),
                p("gamma", "Matte Gamma", num(1.0), gamma()),
            ],
            color_difference_key,
        ),
        spec(
            "ec.key.colorrange",
            "Color Range",
            "Keying",
            vec![
                p("fuzziness", "Fuzziness", num(0.0), b255()),
                p("colorSpace", "Color Space", Value::Enum(0), popup(&["Lab", "YUV", "RGB"])),
                p("minL", "Min (L, Y, R)", num(0.0), b255()),
                p("maxL", "Max (L, Y, R)", num(255.0), b255()),
                p("minA", "Min (a, U, G)", num(0.0), b255()),
                p("maxA", "Max (a, U, G)", num(100.0), b255()),
                p("minB", "Min (b, V, B)", num(128.0), b255()),
                p("maxB", "Max (b, V, B)", num(255.0), b255()),
            ],
            color_range,
        ),
        spec(
            "ec.key.extract",
            "Extract",
            "Keying",
            vec![
                p("channel", "Channel", Value::Enum(0), popup(&["Luminance", "Red", "Green", "Blue", "Alpha"])),
                p("blackPoint", "Black Point", num(0.0), b255()),
                p("whitePoint", "White Point", num(255.0), b255()),
                p("blackSoftness", "Black Softness", num(0.0), b255()),
                p("whiteSoftness", "White Softness", num(0.0), b255()),
                p("invert", "Invert", Value::Bool(false), ParamUi::Checkbox),
            ],
            extract,
        ),
        spec(
            "ec.key.screen",
            "Screen Key",
            "Keying",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Final Result", "Screen Matte", "Status", "Source"])),
                p("screenColor", "Screen Colour", col(0.15, 0.75, 0.25), ParamUi::Color),
                p("screenGain", "Screen Gain", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
                p("screenBalance", "Screen Balance", num(50.0), pct()),
                p("despill", "Despill", num(100.0), pct()),
                p("clipBlack", "Clip Black", num(0.0), pct()),
                p("clipWhite", "Clip White", num(100.0), pct()),
                p("screenPreblur", "Screen Pre-blur", num(0.0), slider(0.0, 100.0, 0.0, 10.0, 1)),
                p("screenShrinkGrow", "Screen Shrink/Grow", num(0.0), slider(-50.0, 50.0, -10.0, 10.0, 1)),
                p("screenSoftness", "Screen Softness", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("edgeColor", "Edge Colour", Value::Enum(0), popup(&["Screen Subtract", "Source"])),
            ],
            screen_key,
        ),
        spec(
            "ec.key.spill",
            "Spill Suppressor",
            "Obsolete",
            vec![
                p("colorToSuppress", "Color To Suppress", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("colorAccuracy", "Color Accuracy", Value::Enum(0), popup(&["Faster", "Better"])),
                p("suppression", "Suppression", num(100.0), slider(0.0, 200.0, 0.0, 100.0, 0)),
            ],
            spill_suppressor,
        ),
        spec(
            "ec.key.advancedspill",
            "Advanced Spill Suppressor",
            "Keying",
            vec![
                p("method", "Method", Value::Enum(0), popup(&["Standard", "Ultra"])),
                p("suppression", "Suppression", num(100.0), slider(0.0, 200.0, 0.0, 100.0, 0)),
                p("ultraSettings/keyColor", "Key Color", col(0.0, 1.0, 0.0), ParamUi::Color),
                p("ultraSettings/tolerance", "Tolerance", num(50.0), pct()),
                p("ultraSettings/desaturate", "Desaturate", num(0.0), pct()),
                p("ultraSettings/spillRange", "Spill Range", num(50.0), pct()),
                p("ultraSettings/spillColorCorrection", "Spill Color Correction", num(0.0), pct()),
                p("ultraSettings/lumaCorrection", "Luma Correction", num(0.0), pct()),
            ],
            advanced_spill,
        ),
        spec(
            "ec.key.keycleaner",
            "Key Cleaner",
            "Keying",
            vec![
                p("additionalEdgeRadius", "Additional Edge Radius", num(3.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("reduceChatter", "Reduce Chatter", Value::Bool(false), ParamUi::Checkbox),
                p("alphaContrast", "Alpha Contrast", num(100.0), slider(100.0, 500.0, 100.0, 300.0, 0)),
                p("strength", "Strength", num(100.0), pct()),
            ],
            key_cleaner,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, apply, find};

    /// Left half green screen, right half red foreground.
    fn green_screen() -> Image {
        let mut img = Image::new(16, 8);
        for y in 0..8 {
            for x in 0..16 {
                img.set(x, y, if x < 8 { [0.1, 0.8, 0.15, 1.0] } else { [0.9, 0.2, 0.1, 1.0] });
            }
        }
        img
    }

    fn run(id: &str, over: &[(&str, Value)], img: Image) -> Image {
        let s = find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in over {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: Default::default() };
        apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 }).img
    }

    fn green() -> Value {
        Value::Color([0.1, 0.8, 0.15, 1.0])
    }

    #[test]
    fn linear_color_key_removes_green() {
        let out = run("ec.key.linearcolor", &[("keyColor", green())], green_screen());
        assert!(out.get(2, 2)[3] < 0.01);
        assert!(out.get(12, 2)[3] > 0.99);
    }

    #[test]
    fn color_difference_key_removes_green() {
        let out = run("ec.key.colordifference", &[("keyColor", green())], green_screen());
        assert!(out.get(2, 2)[3] < 0.01, "{:?}", out.get(2, 2));
        assert!(out.get(12, 2)[3] > 0.99);
    }

    #[test]
    fn screen_key_removes_green_and_despills() {
        let mut img = green_screen();
        img.set(12, 4, [0.6, 0.65, 0.5, 1.0]);
        let out = run("ec.key.screen", &[("screenColor", green())], img);
        assert!(out.get(2, 2)[3] < 0.01);
        assert!(out.get(12, 2)[3] > 0.99);
        let px = out.get(12, 4);
        let (c, a) = unpremul(px);
        assert!(a > 0.5 && a < 1.0, "{a}");
        assert!(c[1] <= 0.5 * (c[0] + c[2]) + 1e-3, "{c:?}");
    }

    #[test]
    fn screen_key_matte_view_is_opaque() {
        let out = run("ec.key.screen", &[("screenColor", green()), ("view", Value::Enum(1))], green_screen());
        assert!(out.data.iter().all(|p| p[3] == 1.0));
        assert!(out.get(2, 2)[0] < 0.01 && out.get(12, 2)[0] > 0.99);
    }

    #[test]
    fn color_range_keys_green_in_lab() {
        let out = run("ec.key.colorrange", &[], green_screen());
        assert!(out.get(2, 2)[3] < 0.01);
        assert!(out.get(12, 2)[3] > 0.99);
    }

    #[test]
    fn luma_key_darker() {
        let mut img = Image::new(4, 1);
        img.set(0, 0, [0.1, 0.1, 0.1, 1.0]);
        img.set(1, 0, [0.9, 0.9, 0.9, 1.0]);
        let out = run("ec.key.luma", &[("threshold", num(128.0))], img);
        assert_eq!(out.get(0, 0)[3], 0.0);
        assert_eq!(out.get(1, 0)[3], 1.0);
    }

    /// After Effects 2026 (cases 812-814): Luma Key on a vertical black-to-white ramp (row y
    /// has level (y + 0.5) / 1080 × 255), Threshold 140, Tolerance 20. EffectCraft ramped the
    /// matte over the tolerance (case 719: a 50 % grey kept 80 % of its alpha where AE keys it
    /// out).
    #[test]
    fn luma_key_matches_after_effects() {
        let ramp = || {
            let mut img = Image::new(1, 1080);
            for y in 0..1080 {
                let v = (y as f32 + 0.5) / 1080.0;
                img.set(0, y, [v, v, v, 1.0]);
            }
            img
        };
        let key = |kind: u32, thin: f64, feather: f64| {
            let set =
                [("keyType", Value::Enum(kind)), ("threshold", num(140.0)), ("tolerance", num(20.0)), ("edgeThin", num(thin)), ("edgeFeather", num(feather))];
            let o = run("ec.key.luma", &set, ramp());
            (0..1080).map(|y| o.get(0, y)[3]).collect::<Vec<f32>>()
        };
        // Key Out Darker (AE 2 = EffectCraft 1): keyed up to row 592, kept from 593.
        let d = key(1, 0.0, 0.0);
        assert!(d[..593].iter().all(|a| *a == 0.0) && d[593..].iter().all(|a| *a == 1.0), "{:?}", &d[588..598]);
        // Key Out Similar: AE keys rows 509..=677 (levels 120.3..160.0) and keeps the rest; the
        // rows at the band's ends sit within 0.1 level of 120 / 160, so they may go either way.
        let s = key(2, 0.0, 0.0);
        assert!(
            (0..1080).filter(|y| !(507..=510).contains(y) && !(676..=679).contains(y)).all(|y| (s[y] == 0.0) == (509..=677).contains(&y)),
            "{:?} {:?}",
            &s[505..512],
            &s[674..681]
        );
        // Key Out Brighter and Dissimilar are the complements.
        let b = key(0, 0.0, 0.0);
        assert!(b[595] == 0.0 && b[590] == 1.0);
        let ds = key(3, 0.0, 0.0);
        assert!(ds[600] == 1.0 && ds[400] == 0.0 && ds[700] == 0.0);
        // A 50 % grey at Darker 140 / 20 is keyed out entirely (case 719).
        let o =
            run("ec.key.luma", &[("keyType", Value::Enum(1)), ("threshold", num(140.0)), ("tolerance", num(20.0))], Image::filled(2, 2, [0.5, 0.5, 0.5, 1.0]));
        assert_eq!(o.get(0, 0)[3], 0.0);
        // Edge Thin 1, Edge Feather 2.5 (case 814): AE's alpha rises over rows 591..596.
        let f = key(1, 1.0, 2.5);
        for (y, ae) in [(590, 0.0), (591, 0.026), (592, 0.139), (593, 0.357), (594, 0.635), (595, 0.868), (596, 0.97), (597, 1.0)] {
            assert!((f[y] - ae).abs() < 0.2, "row {y}: {} vs AE {ae}", f[y]);
        }
    }

    /// After Effects 2026: Key Out Darker and Extract's Black Point at 65 / 166 / 24 on pure
    /// red / green / blue keep red and blue and key out green: Rec. 601 luminance (Rec. 709
    /// would key out red and blue and keep green).
    #[test]
    fn luma_key_and_extract_use_rec601_as_after_effects() {
        for (c, thr, kept) in [([1.0, 0.0, 0.0], 65.0, true), ([0.0, 1.0, 0.0], 166.0, false), ([0.0, 0.0, 1.0], 24.0, true)] {
            let img = Image::filled(2, 1, [c[0], c[1], c[2], 1.0]);
            let key = run("ec.key.luma", &[("keyType", Value::Enum(1)), ("threshold", num(thr))], img.clone());
            let ext = run("ec.key.extract", &[("blackPoint", num(thr))], img);
            let want = if kept { 1.0 } else { 0.0 };
            assert_eq!((key.get(0, 0)[3], ext.get(0, 0)[3]), (want, want), "{c:?} at {thr}");
        }
    }

    #[test]
    fn extract_keeps_range() {
        let mut img = Image::new(3, 1);
        img.set(0, 0, [0.05, 0.05, 0.05, 1.0]);
        img.set(1, 0, [0.5, 0.5, 0.5, 1.0]);
        img.set(2, 0, [1.0, 1.0, 1.0, 1.0]);
        let out = run("ec.key.extract", &[("blackPoint", num(50.0)), ("whitePoint", num(200.0))], img);
        assert_eq!(out.get(0, 0)[3], 0.0);
        assert_eq!(out.get(1, 0)[3], 1.0);
        assert_eq!(out.get(2, 0)[3], 0.0);
    }

    #[test]
    fn spill_suppressor_limits_green() {
        let img = Image::filled(2, 2, [0.3, 0.8, 0.4, 1.0]);
        let out = run("ec.key.spill", &[("colorToSuppress", green())], img);
        let px = out.get(0, 0);
        assert!((px[1] - 0.4).abs() < 1e-5 && px[0] == 0.3 && px[2] == 0.4);
    }

    #[test]
    fn advanced_spill_standard_detects_green() {
        let img = Image::filled(2, 2, [0.3, 0.8, 0.3, 1.0]);
        let out = run("ec.key.advancedspill", &[], img);
        assert!((out.get(0, 0)[1] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn color_difference_key_partials_views_and_levels() {
        // Source view passes through.
        let src = run("ec.key.colordifference", &[("keyColor", green()), ("view", Value::Enum(0))], green_screen());
        assert_eq!(src.get(2, 2), green_screen().get(2, 2));
        // Matte views are opaque grey; the corrected matte keys the screen.
        let m = run("ec.key.colordifference", &[("keyColor", green()), ("view", Value::Enum(6))], green_screen());
        assert!(m.get(2, 2)[0] < 0.01 && m.get(12, 2)[0] > 0.99 && m.get(2, 2)[3] == 1.0);
        // Partial A's output white caps that partial (and so the final matte where only A keeps).
        let pa = run("ec.key.colordifference", &[("keyColor", green()), ("view", Value::Enum(2)), ("partialAOutWhite", num(128.0))], green_screen());
        assert!((pa.get(12, 2)[0] - 128.0 / 255.0).abs() < 1e-3, "{:?}", pa.get(12, 2));
        // The four-up view shows a different panel per quadrant.
        let four = run("ec.key.colordifference", &[("keyColor", green()), ("view", Value::Enum(8))], green_screen());
        assert_eq!(four.get(2, 2)[3], 1.0, "matte panel is opaque");
        assert!(four.get(12, 6)[3] > 0.99, "final output keeps the foreground");
    }

    #[test]
    fn spill_suppressor_better_handles_secondary_key_colours() {
        // A yellow cast on a grey pixel: Faster only limits the primary channel, Better removes
        // the yellow chroma along the key direction.
        let img = Image::filled(1, 1, [0.7, 0.7, 0.3, 1.0]);
        let yellow = Value::Color([1.0, 1.0, 0.0, 1.0]);
        let better = run("ec.key.spill", &[("colorToSuppress", yellow), ("colorAccuracy", Value::Enum(1))], img);
        let px = better.get(0, 0);
        assert!((px[0] - px[2]).abs() < 1e-4 && (px[1] - px[2]).abs() < 1e-4, "{px:?}");
    }

    #[test]
    fn advanced_spill_ultra_reads_its_twirl_down() {
        // Ultra with a blue key leaves a green pixel alone.
        let img = Image::filled(1, 1, [0.3, 0.8, 0.3, 1.0]);
        let out = run("ec.key.advancedspill", &[("method", Value::Enum(1)), ("ultraSettings/keyColor", Value::Color([0.0, 0.0, 1.0, 1.0]))], img);
        assert!((out.get(0, 0)[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn key_cleaner_defaults_keep_opaque_pixels() {
        let img = green_screen();
        let out = run("ec.key.keycleaner", &[], img.clone());
        assert_eq!(out.get(12, 2), img.get(12, 2));
    }

    #[test]
    fn advanced_spill_ultra_tolerance_desaturate_and_color_correction() {
        let ultra = |extra: &[(&str, Value)], c: [f32; 4]| {
            let mut v = vec![("method", Value::Enum(1)), ("ultraSettings/keyColor", Value::Color([0.0, 1.0, 0.0, 1.0]))];
            v.extend_from_slice(extra);
            run("ec.key.advancedspill", &v, Image::filled(1, 1, c)).get(0, 0)
        };
        // A yellow-green pixel (hue away from the key) is left alone with a narrow tolerance.
        let yg = [0.6, 0.8, 0.1, 1.0];
        assert!((ultra(&[("ultraSettings/tolerance", num(5.0))], yg)[1] - 0.8).abs() < 1e-5);
        assert!(ultra(&[("ultraSettings/tolerance", num(100.0))], yg)[1] < 0.79);
        // Desaturate greys the suppressed pixel.
        let g = [0.3, 0.8, 0.3, 1.0];
        let d = ultra(&[("ultraSettings/desaturate", num(100.0))], g);
        assert!((d[0] - d[1]).abs() < 0.05, "{d:?}");
        // Spill Color Correction keeps the brightness the suppression removed.
        let plain = ultra(&[], g);
        let cc = ultra(&[("ultraSettings/spillColorCorrection", num(100.0))], g);
        let sum = |p: [f32; 4]| p[0] + p[1] + p[2];
        assert!(sum(cc) > sum(plain) + 0.1 && (sum(cc) - sum(g)).abs() < 1e-4, "{plain:?} {cc:?}");
    }
}
