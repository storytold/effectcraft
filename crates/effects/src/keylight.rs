//! Screen Color Key — a full screen-colour keyer with the controls of After Effects' bundled
//! "Keylight (1.2)" (that name is a trademark of its vendor, so the effect is registered as
//! **Screen Color Key**; `lookup("Keylight (1.2)")` finds it through [`KEYLIGHT_ALIASES`]).
//!
//! The method is the textbook *screen difference* key, written from the public description of
//! the controls:
//!
//! * colours are first neutralised by the **Alpha Bias** / **Despill Bias** colours (each channel
//!   scaled so the bias colour becomes grey — mid grey = no change);
//! * the **screen difference** of a pixel is its screen-primary channel minus a balance-weighted
//!   mix of the other two (**Screen Balance**); the raw matte is
//!   `1 − Screen Gain × diff(pixel) / diff(screen colour)`;
//! * the **Screen Matte** is then clipped (Clip Black / White, with Clip Rollback restoring edge
//!   detail), despotted (morphological open/close: Despot Black / White), shrunk or grown and
//!   softened, combined with the **Inside** / **Outside** masks and the source alpha;
//! * the foreground is the source with the screen colour removed in proportion to the
//!   transparency (`(c − (1 − α)·S) / α`), then **despilled** (the primary is limited to the
//!   balanced secondary), with **Replace Method** colouring areas whose alpha was raised by
//!   clipping or the inside mask, and optional foreground and edge colour correction.
//!
//! Views: Source, Source Alpha, Corrected Source, Colour Correction Edges, Screen Matte, Inside
//! Mask, Outside Mask, Combined Matte, Status, Intermediate Result and Final Result.

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, gaussian_blur};
use rayon::prelude::*;

use crate::util::{Plane, gauss_plane, morph_frac, point_in_poly, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

/// Other names the effect answers to in [`crate::lookup`].
pub const KEYLIGHT_ALIASES: &[&str] = &["Keylight (1.2)", "Keylight", "Key Light"];

pub const VIEWS: [&str; 11] = [
    "Source",
    "Source Alpha",
    "Corrected Source",
    "Colour Correction Edges",
    "Screen Matte",
    "Inside Mask",
    "Outside Mask",
    "Combined Matte",
    "Status",
    "Intermediate Result",
    "Final Result",
];

/// Display names of the effect's twirl-down groups (by match id).
pub const GROUPS: &[(&str, &str)] = &[
    ("screenMatte", "Screen Matte"),
    ("insideMask", "Inside Mask"),
    ("outsideMask", "Outside Mask"),
    ("foregroundColourCorrection", "Foreground Colour Correction"),
    ("edgeColourCorrection", "Edge Colour Correction"),
    ("sourceCrops", "Source Crops"),
];

const REPLACE: [&str; 4] = ["None", "Source", "Hard Colour", "Soft Colour"];

fn pct(d: f64) -> (Value, ParamUi) {
    (num(d), slider(0.0, 100.0, 0.0, 100.0, 1))
}

fn mask_popup() -> ParamUi {
    popup(&["None", "Mask 1", "Mask 2", "Mask 3", "Mask 4", "Mask 5", "Mask 6", "Mask 7", "Mask 8", "Mask 9", "Mask 10"])
}

/// Index of the screen's primary channel and the two others (larger-in-screen first).
fn primary(s: [f32; 3]) -> (usize, usize, usize) {
    let pi = if s[1] >= s[0] && s[1] >= s[2] {
        1
    } else if s[2] >= s[0] {
        2
    } else {
        0
    };
    let (a, b) = match pi {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    if s[a] >= s[b] { (pi, a, b) } else { (pi, b, a) }
}

/// Screen difference: primary minus the balance-weighted secondaries.
#[inline]
pub fn screen_diff(c: [f32; 3], pi: usize, o1: usize, o2: usize, bal: f32) -> f32 {
    c[pi] - (bal * c[o1] + (1.0 - bal) * c[o2])
}

/// Scale channels so `bias` becomes neutral (grey keeps everything as is).
#[inline]
fn neutralise(c: [f32; 3], bias: [f32; 3]) -> [f32; 3] {
    let l = luminance(bias[0], bias[1], bias[2]).max(1e-4);
    [0, 1, 2].map(|i| c[i] * l / bias[i].max(1e-4))
}

fn mask_coverage(ctx: &EffectCtx, b: &Buf, idx: u32, softness: f64, invert: bool) -> Option<Plane> {
    let shape = ctx.env.masks.get((idx as usize).checked_sub(1)?)?;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let mut pl = Plane::new(w, h);
    let inv = 1.0 / b.scale.max(1e-9);
    pl.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let lx = (x as f64 + 0.5 - b.offset[0]) * inv;
            let ly = (y as f64 + 0.5 - b.offset[1]) * inv;
            *v = if point_in_poly(&shape.points, lx, ly) != shape.inverted { 1.0 } else { 0.0 };
        }
    });
    let s = softness * b.scale;
    if s > 0.0 {
        pl = gauss_plane(&pl, s * 0.5, s * 0.5);
    }
    if invert {
        pl = pl.map(|v| 1.0 - v);
    }
    Some(pl)
}

/// Saturation / contrast / brightness correction of a straight colour.
fn correct(c: [f32; 3], sat: f32, contrast: f32, bright: f32) -> [f32; 3] {
    let l = luminance(c[0], c[1], c[2]);
    let c = c.map(|v| l + (v - l) * sat);
    c.map(|v| ((v - 0.5) * contrast + 0.5) * bright)
}

/// Colour Suppression choices.
const SUPPRESS: [&str; 7] = ["None", "Red", "Green", "Blue", "Cyan", "Magenta", "Yellow"];
/// Source Crops' X / Y Method choices.
const CROP_METHODS: [&str; 4] = ["Colour", "Repeat", "Reflect", "Wrap"];

/// (colour, balance 0..1, amount 0..1) of a Colour Suppression setting.
fn suppression(ctx: &EffectCtx, kind: &str, bal: &str, amt: &str) -> (u32, f32, f32) {
    (ctx.params.e(kind), (ctx.params.get(bal).map(Value::as_f64).unwrap_or(50.0) / 100.0) as f32, (ctx.params.f(amt) / 100.0) as f32)
}

/// Colour Suppression: remove the excess of a primary (over a Balance-weighted mix of the other
/// two) or of a secondary (the smaller of its two primaries over the third).
fn suppress(mut c: [f32; 3], (kind, bal, amt): (u32, f32, f32)) -> [f32; 3] {
    if kind == 0 || amt <= 0.0 {
        return c;
    }
    match kind {
        1..=3 => {
            let k = kind as usize - 1;
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            let limit = c[a] * bal + c[b] * (1.0 - bal);
            c[k] -= (c[k] - limit).max(0.0) * amt;
        }
        _ => {
            // Cyan = G + B over R, Magenta = R + B over G, Yellow = R + G over B.
            let third = match kind {
                4 => 0,
                5 => 1,
                _ => 2,
            };
            let (a, b) = ((third + 1) % 3, (third + 2) % 3);
            let excess = (c[a].min(c[b]) - c[third]).max(0.0) * amt;
            c[a] -= excess;
            c[b] -= excess;
        }
    }
    c
}

/// (tint offset) of a Colour Balancing wheel: hue (degrees) and saturation (0..1).
fn balance(ctx: &EffectCtx, hue: &str, sat: &str) -> [f32; 3] {
    let s = (ctx.params.f(sat) / 100.0) as f32;
    if s <= 0.0 {
        return [0.0; 3];
    }
    let (r, g, b) = effectcraft_color::hsl_to_rgb((ctx.params.f(hue) / 360.0).rem_euclid(1.0) as f32, 1.0, 0.5);
    let l = luminance(r, g, b);
    [(r - l) * s * 0.5, (g - l) * s * 0.5, (b - l) * s * 0.5]
}

/// Colour Balancing: shift the colour towards the wheel's hue, keeping its luminance.
fn colour_balance(c: [f32; 3], tint: [f32; 3]) -> [f32; 3] {
    if tint == [0.0; 3] {
        return c;
    }
    [c[0] + tint[0], c[1] + tint[1], c[2] + tint[2]].map(|v| v.max(0.0))
}

/// The layer with Source Crops applied: inside the crop the picture, outside it the Edge Colour
/// (X / Y Method Colour) or the kept picture repeated, reflected or wrapped.
fn crop_source(ctx: &EffectCtx, b: &Buf) -> Image {
    let pr = ctx.params;
    let crops = [pr.f("sourceCrops/cropLeft"), pr.f("sourceCrops/cropRight"), pr.f("sourceCrops/cropTop"), pr.f("sourceCrops/cropBottom")].map(|v| v / 100.0);
    if crops.iter().all(|c| *c <= 0.0) {
        return b.img.clone();
    }
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let (x0, x1) = (b.offset[0] + crops[0] * lw, b.offset[0] + (1.0 - crops[1]) * lw);
    let (y0, y1) = (b.offset[1] + crops[2] * lh, b.offset[1] + (1.0 - crops[3]) * lh);
    let methods = [pr.e("sourceCrops/xMethod"), pr.e("sourceCrops/yMethod")];
    let ec = pr.color("sourceCrops/edgeColour");
    let ea = (pr.f("sourceCrops/edgeColourAlpha") / 100.0) as f32;
    if methods == [0, 0] && ea <= 0.0 {
        // Transparent edge colour: the borders are cut from the result instead (keying sees
        // the whole picture).
        return b.img.clone();
    }
    let edge = premul([ec[0], ec[1], ec[2]], ea);
    // Fold a coordinate into [lo, hi) by a method; None = outside with Colour.
    let fold = |v: f64, lo: f64, hi: f64, m: u32| -> Option<f64> {
        let span = (hi - lo).max(1.0);
        if v >= lo && v < hi {
            return Some(v);
        }
        match m {
            1 => Some(v.clamp(lo, hi - 0.5)),
            2 => {
                let t = (v - lo).rem_euclid(2.0 * span);
                Some(lo + if t >= span { 2.0 * span - t - 0.5 } else { t })
            }
            3 => Some(lo + (v - lo).rem_euclid(span)),
            _ => None,
        }
    };
    crate::util::gen_image(b.img.width, b.img.height, |x, y| {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        match (fold(fx, x0, x1, methods[0]), fold(fy, y0, y1, methods[1])) {
            (Some(sx), Some(sy)) => b.img.get_clamped(sx.floor() as i64, sy.floor() as i64),
            _ => edge,
        }
    })
}

fn key_light(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let view = pr.e("view");
    if view == 0 {
        return b;
    }
    let sc = pr.color("screenColour");
    let screen = [sc[0], sc[1], sc[2]];
    let (pi, o1, o2) = primary(screen);
    let gain = pr.f("screenGain") as f32 / 100.0;
    let bal = pr.f("screenBalance") as f32 / 100.0;
    let dbias = pr.color("despillBias");
    let abias = if pr.b("lockBiasesTogether") { dbias } else { pr.color("alphaBias") };
    let (dbias, abias) = ([dbias[0], dbias[1], dbias[2]], [abias[0], abias[1], abias[2]]);
    let s_alpha = neutralise(screen, abias);
    let sd = screen_diff(s_alpha, pi, o1, o2, bal).max(1e-3);
    let cb = pr.f("screenMatte/clipBlack") as f32 / 100.0;
    let cw = pr.f("screenMatte/clipWhite") as f32 / 100.0;
    let rollback = pr.f("screenMatte/clipRollback") * b.scale;
    let preblur = pr.f("screenPreblur") * b.scale;
    let sg = pr.f("screenMatte/screenShrinkGrow") * b.scale;
    let soft = pr.f("screenMatte/screenSoftness") * b.scale;
    let despot_b = pr.f("screenMatte/screenDespotBlack") * b.scale;
    let despot_w = pr.f("screenMatte/screenDespotWhite") * b.scale;

    // Source Crops: the cropped borders take the Edge Colour, or repeat / reflect / wrap the
    // kept picture (X Method / Y Method), before keying.
    let src = crop_source(ctx, &b);
    let blurred;
    let key_src: &Image = if preblur > 0.0 {
        blurred = gaussian_blur(&src, preblur * 0.5, preblur * 0.5, true);
        &blurred
    } else {
        &src
    };
    // Raw screen matte (opacity of the foreground).
    let raw = Plane::from_image(key_src, |px| {
        let (c, _) = unpremul(px);
        let d = screen_diff(neutralise(c, abias), pi, o1, o2, bal);
        (1.0 - gain * d / sd).clamp(0.0, 1.0)
    });
    let clip = |v: f32| ((v - cb) / (cw - cb).max(1e-4)).clamp(0.0, 1.0);
    let mut m = raw.map(clip);
    if rollback > 0.0 {
        // Restore edge detail lost by clipping within `rollback` pixels of the clipped matte.
        let grown = morph_frac(&m, rollback, true);
        let shrunk = morph_frac(&m, rollback, false);
        m = Plane {
            w: m.w,
            h: m.h,
            data: (0..m.data.len())
                .into_par_iter()
                .map(|i| if grown.data[i] > shrunk.data[i] + 1e-4 { m.data[i].max(raw.data[i]).min(grown.data[i]) } else { m.data[i] })
                .collect(),
        };
    }
    if despot_b > 0.0 {
        // Fill small holes: close (dilate then erode).
        m = morph_frac(&morph_frac(&m, despot_b, true), despot_b, false);
    }
    if despot_w > 0.0 {
        // Remove small specks: open (erode then dilate).
        m = morph_frac(&morph_frac(&m, despot_w, false), despot_w, true);
    }
    if sg < 0.0 {
        m = morph_frac(&m, -sg, false);
    } else if sg > 0.0 {
        m = morph_frac(&m, sg, true);
    }
    if soft > 0.0 {
        m = gauss_plane(&m, soft * 0.5, soft * 0.5);
    }
    let screen_matte = m.clone();
    let inside = mask_coverage(ctx, &b, pr.e("insideMask/insideMask"), pr.f("insideMask/insideMaskSoftness"), pr.b("insideMask/invertInsideMask"));
    let outside = mask_coverage(ctx, &b, pr.e("outsideMask/outsideMask"), pr.f("outsideMask/outsideMaskSoftness"), pr.b("outsideMask/invertOutsideMask"));
    let src_alpha_mode = pr.e("insideMask/sourceAlpha");
    let n = src.data.len();
    let combined: Vec<f32> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut a = screen_matte.data[i];
            let sa = src.data[i][3].clamp(0.0, 1.0);
            let ins = inside.as_ref().map_or(0.0, |p| p.data[i]);
            let ins = if src_alpha_mode == 1 { ins.max(sa) } else { ins };
            a = a.max(ins);
            if let Some(o) = &outside {
                a *= 1.0 - o.data[i];
            }
            if src_alpha_mode == 2 {
                a *= sa;
            }
            a.clamp(0.0, 1.0)
        })
        .collect();
    let edges: Option<Vec<f32>> = pr.b("edgeColourCorrection/enableEdgeColourCorrection").then(|| {
        let grow = pr.f("edgeColourCorrection/edgeGrow") * b.scale;
        let hard = (pr.f("edgeColourCorrection/edgeHardness") / 100.0) as f32;
        let esoft = pr.f("edgeColourCorrection/edgeSoftness") * b.scale;
        // Edge band: where the (grown) combined matte is partial.
        let cm = Plane { w: m.w, h: m.h, data: combined.clone() };
        let band = cm.map(|a| if a > 1e-3 && a < 0.999 { 1.0 } else { 0.0 });
        let mut band = if grow > 0.0 { morph_frac(&band, grow, true) } else { band };
        if esoft > 0.0 {
            band = gauss_plane(&band, esoft * 0.5, esoft * 0.5);
        }
        band.data.iter().map(|v| (v * (1.0 + hard * 4.0)).min(1.0)).collect()
    });
    let replace = pr.e("screenMatte/replaceMethod");
    let rc = pr.color("screenMatte/replaceColour");
    let in_replace = pr.e("insideMask/insideReplaceMethod");
    let in_rc = pr.color("insideMask/insideReplaceColour");
    let fg_cc = pr.b("foregroundColourCorrection/enableColourCorrection");
    let (fs, fc, fbr) = (
        pr.f("foregroundColourCorrection/saturation") as f32 / 100.0,
        pr.f("foregroundColourCorrection/contrast") as f32 / 100.0 + 1.0,
        pr.f("foregroundColourCorrection/brightness") as f32 / 100.0 + 1.0,
    );
    let (es, ec, ebr) = (
        pr.f("edgeColourCorrection/edgeSaturation") as f32 / 100.0,
        pr.f("edgeColourCorrection/edgeContrast") as f32 / 100.0 + 1.0,
        pr.f("edgeColourCorrection/edgeBrightness") as f32 / 100.0 + 1.0,
    );
    let unpremultiply = pr.b("unpremultiplyResult");
    let (x_method, y_method) = (pr.e("sourceCrops/xMethod"), pr.e("sourceCrops/yMethod"));
    let edge_alpha = pr.f("sourceCrops/edgeColourAlpha") as f32 / 100.0;
    let fg_sup = suppression(
        ctx,
        "foregroundColourCorrection/colourSuppression",
        "foregroundColourCorrection/suppressionBalance",
        "foregroundColourCorrection/suppressionAmount",
    );
    let fg_bal = balance(ctx, "foregroundColourCorrection/colourBalanceHue", "foregroundColourCorrection/colourBalanceSaturation");
    let edge_sup = suppression(
        ctx,
        "edgeColourCorrection/edgeColourSuppression",
        "edgeColourCorrection/edgeSuppressionBalance",
        "edgeColourCorrection/edgeSuppressionAmount",
    );
    let edge_bal = balance(ctx, "edgeColourCorrection/edgeColourBalanceHue", "edgeColourCorrection/edgeColourBalanceSaturation");
    let crops = [pr.f("sourceCrops/cropLeft"), pr.f("sourceCrops/cropRight"), pr.f("sourceCrops/cropTop"), pr.f("sourceCrops/cropBottom")].map(|v| v / 100.0);
    let (lw, lh) = (ctx.layer_size[0], ctx.layer_size[1]);
    let w = b.img.width as usize;
    let (scale, off) = (b.scale, b.offset);
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, _) = unpremul(src.data[i]);
        let a = combined[i];
        let sm = screen_matte.data[i];
        // Source crops (layer fractions from each edge).
        let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
        let (lx, ly) = ((x - off[0]) / scale / lw.max(1e-9), (y - off[1]) / scale / lh.max(1e-9));
        let cropped = (lx < crops[0] || lx > 1.0 - crops[1] || ly < crops[2] || ly > 1.0 - crops[3]) && edge_alpha <= 0.0 && x_method == 0 && y_method == 0;
        // Screen removal on the raw (unclipped) transparency, then despill.
        let ar = raw.data[i].max(1e-4);
        let mut fg = [0, 1, 2].map(|k| ((c[k] - (1.0 - ar) * screen[k]) / ar).max(0.0));
        let nfg = neutralise(fg, dbias);
        let spill = screen_diff(nfg, pi, o1, o2, bal).max(0.0);
        let l0 = (luminance(dbias[0], dbias[1], dbias[2]) / dbias[pi].max(1e-4)).max(1e-4);
        fg[pi] -= spill / l0;
        let intermediate = fg;
        // Replace colour where the alpha was raised beyond the raw screen matte.
        let raised = (sm - raw.data[i]).max(0.0);
        let raised_in = inside.as_ref().map_or(0.0, |p| (p.data[i] - sm).max(0.0));
        let apply_replace = |fg: [f32; 3], method: u32, rc: [f32; 4], amt: f32| -> [f32; 3] {
            if amt <= 0.0 {
                return fg;
            }
            match method {
                1 => [0, 1, 2].map(|k| fg[k] + (c[k] - fg[k]) * amt),
                2 => [rc[0], rc[1], rc[2]],
                3 => {
                    let l = luminance(c[0], c[1], c[2]) / luminance(rc[0], rc[1], rc[2]).max(1e-4);
                    [0, 1, 2].map(|k| fg[k] + (rc[k] * l - fg[k]) * amt)
                }
                _ => fg,
            }
        };
        fg = apply_replace(fg, replace, rc, raised);
        fg = apply_replace(fg, in_replace, in_rc, raised_in);
        if fg_cc {
            fg = colour_balance(suppress(correct(fg, fs, fc, fbr), fg_sup), fg_bal);
        }
        let edge = edges.as_ref().map_or(0.0, |e| e[i]);
        if edge > 0.0 {
            let ce = colour_balance(suppress(correct(fg, es, ec, ebr), edge_sup), edge_bal);
            fg = [0, 1, 2].map(|k| fg[k] + (ce[k] - fg[k]) * edge);
        }
        let grey = |v: f32| [v, v, v, 1.0];
        *px = match view {
            1 => grey(src.data[i][3]),
            2 => {
                let mut cs = neutralise(c, dbias);
                let s2 = screen_diff(cs, pi, o1, o2, bal).max(0.0);
                cs[pi] -= s2;
                if fg_cc {
                    cs = correct(cs, fs, fc, fbr);
                }
                [cs[0], cs[1], cs[2], 1.0]
            }
            3 => grey(edge),
            4 => grey(sm),
            5 => grey(inside.as_ref().map_or(0.0, |p| p.data[i])),
            6 => grey(outside.as_ref().map_or(0.0, |p| p.data[i])),
            7 => grey(a),
            8 => {
                let v = if a <= 1e-3 {
                    0.0
                } else if a >= 0.999 {
                    1.0
                } else {
                    0.5
                };
                grey(v)
            }
            9 => premul(intermediate, sm),
            _ => {
                if cropped {
                    [0.0; 4]
                } else if unpremultiply {
                    premul(fg, a)
                } else {
                    // Keylight's premultiplied output read as straight colour.
                    premul(fg.map(|v| v * a), a)
                }
            }
        };
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let pp = |id: &'static str, name: &'static str, v: (Value, ParamUi)| p(id, name, v.0, v.1);
    let cc = |id: &'static str, name: &'static str| p(id, name, num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 1));
    vec![EffectSpec {
        id: "ec.keying.keylight",
        name: "Screen Color Key",
        category: "Keying",
        params: vec![
            p("view", "View", Value::Enum(10), popup(&VIEWS)),
            p("unpremultiplyResult", "Unpremultiply Result", Value::Bool(true), ParamUi::Checkbox),
            p("screenColour", "Screen Colour", col(0.0, 1.0, 0.0), ParamUi::Color),
            p("screenGain", "Screen Gain", num(100.0), slider(0.0, 200.0, 0.0, 200.0, 1)),
            p("screenBalance", "Screen Balance", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            p("despillBias", "Despill Bias", col(0.5, 0.5, 0.5), ParamUi::Color),
            p("alphaBias", "Alpha Bias", col(0.5, 0.5, 0.5), ParamUi::Color),
            p("lockBiasesTogether", "Lock Biases Together", Value::Bool(true), ParamUi::Checkbox),
            p("screenPreblur", "Screen Pre-blur", num(0.0), slider(0.0, 100.0, 0.0, 10.0, 1)),
            // Screen Matte
            pp("screenMatte/clipBlack", "Clip Black", pct(0.0)),
            pp("screenMatte/clipWhite", "Clip White", pct(100.0)),
            p("screenMatte/clipRollback", "Clip Rollback", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("screenMatte/screenShrinkGrow", "Screen Shrink/Grow", num(0.0), slider(-100.0, 100.0, -10.0, 10.0, 1)),
            p("screenMatte/screenSoftness", "Screen Softness", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("screenMatte/screenDespotBlack", "Screen Despot Black", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("screenMatte/screenDespotWhite", "Screen Despot White", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("screenMatte/replaceMethod", "Replace Method", Value::Enum(3), popup(&REPLACE)),
            p("screenMatte/replaceColour", "Replace Colour", col(0.5, 0.5, 0.5), ParamUi::Color),
            // Inside Mask
            p("insideMask/insideMask", "Inside Mask", Value::Enum(0), mask_popup()),
            p("insideMask/insideMaskSoftness", "Inside Mask Softness", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("insideMask/invertInsideMask", "Invert", Value::Bool(false), ParamUi::Checkbox),
            p("insideMask/insideReplaceMethod", "Replace Method", Value::Enum(1), popup(&REPLACE)),
            p("insideMask/insideReplaceColour", "Replace Colour", col(0.5, 0.5, 0.5), ParamUi::Color),
            p("insideMask/sourceAlpha", "Source Alpha", Value::Enum(2), popup(&["Ignore", "Add to Inside Mask", "Normal"])),
            // Outside Mask
            p("outsideMask/outsideMask", "Outside Mask", Value::Enum(0), mask_popup()),
            p("outsideMask/outsideMaskSoftness", "Outside Mask Softness", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("outsideMask/invertOutsideMask", "Invert", Value::Bool(false), ParamUi::Checkbox),
            // Foreground Colour Correction
            p("foregroundColourCorrection/enableColourCorrection", "Enable Colour Correction", Value::Bool(false), ParamUi::Checkbox),
            p("foregroundColourCorrection/saturation", "Saturation", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
            cc("foregroundColourCorrection/contrast", "Contrast"),
            cc("foregroundColourCorrection/brightness", "Brightness"),
            p("foregroundColourCorrection/colourSuppression", "Colour Suppression", Value::Enum(0), popup(&SUPPRESS)),
            pp("foregroundColourCorrection/suppressionBalance", "Suppression Balance", pct(50.0)),
            pp("foregroundColourCorrection/suppressionAmount", "Suppression Amount", pct(0.0)),
            p("foregroundColourCorrection/colourBalanceHue", "Colour Balance Hue", num(0.0), ParamUi::Angle),
            pp("foregroundColourCorrection/colourBalanceSaturation", "Colour Balance Saturation", pct(0.0)),
            // Edge Colour Correction
            p("edgeColourCorrection/enableEdgeColourCorrection", "Enable Edge Colour Correction", Value::Bool(false), ParamUi::Checkbox),
            pp("edgeColourCorrection/edgeHardness", "Edge Hardness", pct(0.0)),
            p("edgeColourCorrection/edgeSoftness", "Edge Softness", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("edgeColourCorrection/edgeGrow", "Edge Grow", num(1.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("edgeColourCorrection/edgeSaturation", "Saturation", num(100.0), slider(0.0, 400.0, 0.0, 200.0, 1)),
            cc("edgeColourCorrection/edgeContrast", "Contrast"),
            cc("edgeColourCorrection/edgeBrightness", "Brightness"),
            p("edgeColourCorrection/edgeColourSuppression", "Colour Suppression", Value::Enum(0), popup(&SUPPRESS)),
            pp("edgeColourCorrection/edgeSuppressionBalance", "Suppression Balance", pct(50.0)),
            pp("edgeColourCorrection/edgeSuppressionAmount", "Suppression Amount", pct(0.0)),
            p("edgeColourCorrection/edgeColourBalanceHue", "Colour Balance Hue", num(0.0), ParamUi::Angle),
            pp("edgeColourCorrection/edgeColourBalanceSaturation", "Colour Balance Saturation", pct(0.0)),
            // Source Crops
            p("sourceCrops/xMethod", "X Method", Value::Enum(0), popup(&CROP_METHODS)),
            p("sourceCrops/yMethod", "Y Method", Value::Enum(0), popup(&CROP_METHODS)),
            p("sourceCrops/edgeColour", "Edge Colour", col(0.0, 0.0, 0.0), ParamUi::Color),
            pp("sourceCrops/edgeColourAlpha", "Edge Colour Alpha", pct(0.0)),
            pp("sourceCrops/cropLeft", "Left", pct(0.0)),
            pp("sourceCrops/cropRight", "Right", pct(0.0)),
            pp("sourceCrops/cropTop", "Top", pct(0.0)),
            pp("sourceCrops/cropBottom", "Bottom", pct(0.0)),
        ],
        render: key_light,
        gpu: false,
        float: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, MaskShape, run_fx};

    /// Green screen with a red square (fg) and a half-transparent mix strip.
    fn plate() -> Image {
        crate::util::gen_image(40, 30, |x, y| {
            if (10..20).contains(&x) && (10..20).contains(&y) {
                [0.8, 0.2, 0.1, 1.0]
            } else if (25..30).contains(&x) {
                // 50 % red over green.
                [0.4, 0.6, 0.05, 1.0]
            } else {
                [0.1, 0.9, 0.1, 1.0]
            }
        })
    }

    fn green() -> Value {
        col(0.1, 0.9, 0.1)
    }

    #[test]
    fn keys_the_screen_and_keeps_the_foreground() {
        let out = run_fx("ec.keying.keylight", &[("screenColour", green())], plate(), 0.0, EffectEnv::default());
        let bg = out.img.get(2, 2);
        let fg = out.img.get(15, 15);
        let mix = out.img.get(27, 5);
        assert!(bg[3] < 0.02, "{bg:?}");
        assert!(fg[3] > 0.98 && (fg[0] - 0.8).abs() < 0.05, "{fg:?}");
        assert!(mix[3] > 0.3 && mix[3] < 0.7, "{mix:?}");
        // The recovered edge colour has the green removed (no spill).
        let (c, _) = unpremul(mix);
        assert!(c[1] <= c[0] + 0.05, "{c:?}");
        // Deterministic.
        let again = run_fx("ec.keying.keylight", &[("screenColour", green())], plate(), 0.0, EffectEnv::default());
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn clip_gain_views_and_masks() {
        // Clip White 60 %: the half-transparent strip becomes solid.
        let out = run_fx("ec.keying.keylight", &[("screenColour", green()), ("screenMatte/clipWhite", num(40.0))], plate(), 0.0, EffectEnv::default());
        assert!(out.img.get(27, 5)[3] > 0.99);
        // Screen Matte and Status views are grey-scale and opaque.
        let sm = run_fx("ec.keying.keylight", &[("screenColour", green()), ("view", Value::Enum(4))], plate(), 0.0, EffectEnv::default());
        assert!(sm.img.get(2, 2)[0] < 0.02 && sm.img.get(15, 15)[0] > 0.98 && sm.img.get(2, 2)[3] == 1.0);
        let st = run_fx("ec.keying.keylight", &[("screenColour", green()), ("view", Value::Enum(8))], plate(), 0.0, EffectEnv::default());
        assert_eq!(st.img.get(27, 5)[0], 0.5);
        // Source view is the identity.
        let src = run_fx("ec.keying.keylight", &[("view", Value::Enum(0))], plate(), 0.0, EffectEnv::default());
        assert_eq!(src.img.data, plate().data);
        // Inside mask forces opacity; outside mask forces transparency.
        let sq = |x0: f64, y0: f64, x1: f64, y1: f64| MaskShape {
            name: "m".into(),
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            closed: true,
            inverted: false,
        };
        let masks = [sq(0.0, 0.0, 6.0, 6.0), sq(12.0, 12.0, 18.0, 18.0)];
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let m = run_fx(
            "ec.keying.keylight",
            &[("screenColour", green()), ("insideMask/insideMask", Value::Enum(1)), ("outsideMask/outsideMask", Value::Enum(2))],
            plate(),
            0.0,
            env,
        );
        assert!(m.img.get(2, 2)[3] > 0.99, "{:?}", m.img.get(2, 2));
        assert!(m.img.get(15, 15)[3] < 0.01);
        // Screen gain above 100 % keys more of the strip.
        let g = run_fx("ec.keying.keylight", &[("screenColour", green()), ("screenGain", num(150.0))], plate(), 0.0, EffectEnv::default());
        assert!(g.img.get(27, 5)[3] < out.img.get(27, 5)[3]);
        assert_eq!(crate::lookup("Keylight (1.2)").map(|s| s.id), Some("ec.keying.keylight"));
        // Searches find it by After Effects' name as well as its own.
        let spec = crate::find("ec.keying.keylight").unwrap();
        assert!(crate::name_matches(spec, "keylight") && crate::name_matches(spec, "key light") && !crate::name_matches(spec, "blur"));
    }

    /// Instances saved with the flat layout get their parameters moved into the twirl-downs.
    #[test]
    fn flat_instances_move_into_twirl_downs() {
        use effectcraft_project::build::Ids;
        let spec = crate::find("ec.keying.keylight").unwrap();
        let mut next = 1;
        let mut ids = Ids(&mut next);
        let mut g = crate::instantiate(spec, &mut ids, "Screen Color Key", [10.0, 10.0]);
        // Flatten it the way older versions saved it.
        let mut flat = vec![];
        g.walk_mut(&mut |p| flat.push(p.clone()));
        g.children = flat.into_iter().map(Into::into).collect();
        let clip = g.children.iter_mut().find_map(|c| match c {
            effectcraft_project::Node::Prop(p) if p.match_id == "clipWhite" => Some(p),
            _ => None,
        });
        clip.unwrap().value = num(42.0);
        assert!(crate::migrate::upgrade_instance(spec, &mut g, &mut ids, [10.0, 10.0]));
        assert_eq!(g.sub("screenMatte").map(|s| s.name.as_str()), Some("Screen Matte"));
        assert_eq!(g.sub("screenMatte").and_then(|s| s.get("clipWhite")).map(|p| p.value.clone()), Some(num(42.0)));
        assert_eq!(g.sub("insideMask").and_then(|s| s.get("insideMask")).map(|p| p.name.as_str()), Some("Inside Mask"));
        assert!(g.get("clipWhite").is_none());
    }

    #[test]
    fn colour_suppression_balancing_and_crop_methods() {
        // Suppress Red pulls red down to the balance of green and blue; Cyan the shared G+B.
        assert!((suppress([0.9, 0.3, 0.1], (1, 0.5, 1.0))[0] - 0.2).abs() < 1e-6);
        let c = suppress([0.1, 0.8, 0.6], (4, 0.5, 1.0));
        assert!((c[1] - 0.3).abs() < 1e-6 && (c[2] - 0.1).abs() < 1e-6);
        assert_eq!(suppress([0.9, 0.3, 0.1], (1, 0.5, 0.0)), [0.9, 0.3, 0.1]);
        // Foreground colour correction with suppression / balancing changes the keyed colour.
        let base = [("screenColour", green()), ("foregroundColourCorrection/enableColourCorrection", Value::Bool(true))];
        let run = |extra: &[(&str, Value)]| {
            let mut v: Vec<(&str, Value)> = base.to_vec();
            v.extend_from_slice(extra);
            run_fx("ec.keying.keylight", &v, plate(), 0.0, EffectEnv::default()).img.get(15, 15)
        };
        let plain = run(&[]);
        let sup = run(&[("foregroundColourCorrection/colourSuppression", Value::Enum(1)), ("foregroundColourCorrection/suppressionAmount", num(100.0))]);
        assert!(sup[0] < plain[0] - 0.1, "{sup:?} vs {plain:?}");
        let bal = run(&[("foregroundColourCorrection/colourBalanceHue", num(240.0)), ("foregroundColourCorrection/colourBalanceSaturation", num(100.0))]);
        assert!(bal[2] > plain[2] + 0.05);
        // Source Crops: Colour fills the cropped border; Repeat copies the kept edge.
        let crop = [("screenColour", green()), ("sourceCrops/cropLeft", num(10.0)), ("view", Value::Enum(1))];
        let filled =
            run_fx("ec.keying.keylight", &[crop.as_slice(), &[("sourceCrops/edgeColourAlpha", num(100.0))]].concat(), plate(), 0.0, EffectEnv::default()).img;
        assert_eq!(filled.get(1, 15)[0], 1.0, "edge colour alpha shows in the source alpha view");
        let src = crop_source(
            &crate::EffectCtx {
                params: &crate::Params {
                    values: [("sourceCrops/cropLeft".to_string(), num(25.0)), ("sourceCrops/xMethod".to_string(), Value::Enum(1))].into_iter().collect(),
                },
                time: 0.0,
                layer_size: [40.0, 30.0],
                seed: 0,
                adjustment: false,
                env: Default::default(),
            },
            &Buf { img: plate(), offset: [0.0; 2], scale: 1.0 },
        );
        assert_eq!(src.get(2, 15), plate().get(10, 15), "repeat");
    }
}
