//! GPU effects, colour family (kernels in `shaders/fx_color.wgsl`): the CPU effects' exact
//! steps with the pixel loops as compute kernels.
//!
//! Colour Balance, Vibrance, Black & White, Tritone, Colorama, Channel Mixer, Selective Color
//! and Linear Color Key are one per-pixel pass (`fxc_point`); Lumetri Color is a per-pixel pass
//! plus, with Sharpen, a luminance plane blurred like `util::gauss_plane`; Key Light runs its
//! matte pipeline (pre-blur, clip, rollback, despot, shrink / grow, softness, edge band) as
//! plane passes. Key Light with an Inside / Outside mask, or Hard Colour replacement after
//! Screen Softness, and Colorama with Interpolate Palette off run on the CPU.

use effectcraft_color::{luminance, rgb_to_hsl};
use effectcraft_effects::EffectCtx;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_color.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fxc_point",
    "fxc_lumetri",
    "fxc_luma",
    "fxc_sharpen",
    "fxc_morph",
    "fxc_lerp",
    "fxc_kl_raw",
    "fxc_kl_rollback",
    "fxc_kl_pack",
    "fxc_kl_band",
    "fxc_kl_edge",
    "fxc_kl_final",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.color.colorbalance",
    "ec.color.vibrance",
    "ec.color.lumetri",
    "ec.color.blackwhite",
    "ec.color.tritone",
    "ec.color.colorama",
    "ec.color.channelmixer",
    "ec.color.selectivecolor",
    "ec.key.linearcolor",
    "ec.keying.keylight",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.color.lumetri" => lumetri(e, ctx, b),
        "ec.keying.keylight" => key_light(e, ctx, b),
        _ => point(e, id, ctx, b),
    }
}

fn rgb(c: [f32; 4]) -> [f32; 4] {
    [c[0], c[1], c[2], 0.0]
}

/// Same selective colour ids as `color2::SC_IDS`.
const SC_IDS: [[&str; 4]; 9] = [
    ["details/reds/redsCyan", "details/reds/redsMagenta", "details/reds/redsYellow", "details/reds/redsBlack"],
    ["details/yellows/yellowsCyan", "details/yellows/yellowsMagenta", "details/yellows/yellowsYellow", "details/yellows/yellowsBlack"],
    ["details/greens/greensCyan", "details/greens/greensMagenta", "details/greens/greensYellow", "details/greens/greensBlack"],
    ["details/cyans/cyansCyan", "details/cyans/cyansMagenta", "details/cyans/cyansYellow", "details/cyans/cyansBlack"],
    ["details/blues/bluesCyan", "details/blues/bluesMagenta", "details/blues/bluesYellow", "details/blues/bluesBlack"],
    ["details/magentas/magentasCyan", "details/magentas/magentasMagenta", "details/magentas/magentasYellow", "details/magentas/magentasBlack"],
    ["details/whites/whitesCyan", "details/whites/whitesMagenta", "details/whites/whitesYellow", "details/whites/whitesBlack"],
    ["details/neutrals/neutralsCyan", "details/neutrals/neutralsMagenta", "details/neutrals/neutralsYellow", "details/neutrals/neutralsBlack"],
    ["details/blacks/blacksCyan", "details/blacks/blacksMagenta", "details/blacks/blacksYellow", "details/blacks/blacksBlack"],
];

fn point(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |k: &str| pr.f(k) as f32;
    let mut p = Params::default();
    let mut data = None;
    match id {
        "ec.color.colorbalance" => {
            let g = |k: &str| f(k) / 100.0;
            p.u[0] = [1, pr.b("preserveLuminosity") as u32, 0, 0];
            p.f[0] = [g("shadowRed"), g("shadowGreen"), g("shadowBlue"), 0.0];
            p.f[1] = [g("midRed"), g("midGreen"), g("midBlue"), 0.0];
            p.f[2] = [g("hiRed"), g("hiGreen"), g("hiBlue"), 0.0];
        }
        "ec.color.vibrance" => {
            p.u[0][0] = 2;
            p.f[0] = [f("vibrance") / 100.0, f("saturation") / 100.0, 0.0, 0.0];
        }
        "ec.color.blackwhite" => {
            let w = ["reds", "yellows", "greens", "cyans", "blues", "magentas"].map(|k| f(k) / 100.0);
            let tc = pr.color("tintColor");
            p.u[0] = [3, pr.b("tint") as u32, 0, 0];
            p.f[0] = [w[0], w[1], w[2], w[3]];
            p.f[1] = [w[4], w[5], 0.0, 0.0];
            p.f[2] = [tc[0], tc[1], tc[2], 0.0];
        }
        "ec.color.tritone" => {
            p.u[0][0] = 4;
            p.f[0] = rgb(pr.color("highlights"));
            p.f[1] = rgb(pr.color("midtones"));
            p.f[2] = rgb(pr.color("shadows"));
            p.f[3][0] = 1.0 - f("blend") / 100.0;
        }
        "ec.color.colorama" => {
            let smooth = pr.get("outputCycle/interpolatePalette").is_none_or(|v| v.as_bool());
            // The posterized palette is discontinuous: phases on a step (common with 8 bpc
            // input) can round to the other side in the GPU's float maths, so it runs on the CPU.
            if !smooth {
                return None;
            }
            p.u[0] = [5, pr.e("inputPhase/mode"), smooth as u32, 0];
            p.f[0] = [pr.f("outputCycle/cycles").max(0.0) as f32, f("inputPhase/phaseShift") / 360.0, 1.0 - f("blend") / 100.0, 0.0];
        }
        "ec.color.channelmixer" => {
            let g = |k: &str| f(k) / 100.0;
            p.u[0] = [6, pr.b("monochrome") as u32, 0, 0];
            p.f[0] = [g("rr"), g("rg"), g("rb"), g("rc")];
            p.f[1] = [g("gr"), g("gg"), g("gb"), g("gc")];
            p.f[2] = [g("br"), g("bg"), g("bb"), g("bc")];
        }
        "ec.color.selectivecolor" => {
            let adj: Vec<f32> = SC_IDS.iter().flat_map(|ids| ids.map(|k| f(k) / 100.0)).collect();
            if adj.iter().all(|v| *v == 0.0) {
                return Some(b);
            }
            p.u[0] = [7, (pr.e("method") == 0) as u32, 0, 0];
            data = Some(e.data(&adj));
        }
        "ec.key.linearcolor" => {
            let view = pr.e("view");
            if view == 1 {
                return Some(b);
            }
            let k = pr.color("keyColor");
            let s = k[0] + k[1] + k[2];
            let kc = if s <= 1e-6 { [1.0 / 3.0, 1.0 / 3.0] } else { [k[0] / s, k[1] / s] };
            let (kh, _, _) = rgb_to_hsl(k[0], k[1], k[2]);
            p.u[0] = [8, pr.e("matchColors"), (pr.e("keyOperation") == 1) as u32, view];
            p.f[0] = [k[0], k[1], k[2], f("tolerance") / 100.0];
            p.f[1] = [f("softness") / 100.0, kc[0], kc[1], kh];
        }
        _ => return None,
    }
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxc_point", &p, &b.img, None, &out, data.as_ref());
    Some(GBuf { img: out, ..b })
}

/// color3::tint_offset.
fn tint_offset(c: [f32; 4]) -> [f32; 3] {
    let l = (c[0] + c[1] + c[2]) / 3.0;
    [c[0] - l, c[1] - l, c[2] - l]
}

fn lumetri(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |k: &str| (pr.f(k) / 100.0) as f32;
    let mut d: Vec<f32> = vec![
        f("basicCorrection/whiteBalance/temperature"),
        f("basicCorrection/whiteBalance/tint"),
        2f32.powf(pr.f("basicCorrection/tone/exposure") as f32),
        f("basicCorrection/tone/contrast"),
        f("basicCorrection/tone/highlights"),
        f("basicCorrection/tone/shadows"),
        f("basicCorrection/tone/whites"),
        f("basicCorrection/tone/blacks"),
        f("basicCorrection/saturation"),
        f("creative/lookIntensity"),
        f("creative/adjustments/fadedFilm"),
        f("creative/adjustments/vibrance"),
        f("creative/adjustments/creativeSaturation"),
        (0.5 + 0.25 * f("creative/adjustments/tintBalance")).clamp(0.05, 0.95),
    ];
    d.extend(tint_offset(pr.color("creative/adjustments/shadowTint")));
    d.extend(tint_offset(pr.color("creative/adjustments/highlightTint")));
    for c in ["Master", "Red", "Green", "Blue"] {
        for part in ["Shadows", "Midtones", "Highlights"] {
            d.push(f(&format!("curves/curve{c}{part}")));
        }
    }
    for w in ["shadowsWheel", "midtonesWheel", "highlightsWheel"] {
        d.extend(tint_offset(pr.color(&format!("colorWheels/{w}"))));
    }
    let (lx, ly) = (b.offset[0], b.offset[1]);
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    d.extend([
        pr.f("vignette/vignetteAmount") as f32,
        f("vignette/vignetteMidpoint") * 0.9,
        f("vignette/vignetteRoundness"),
        f("vignette/vignetteFeather"),
        (lx + lw * 0.5) as f32,
        (ly + lh * 0.5) as f32,
        lw as f32,
        lh as f32,
        (lw / lh.max(1e-6)) as f32,
    ]);
    debug_assert_eq!(d.len(), 50);
    // Input LUT and Look (data[50..53]: look, input LUT offset, look LUT offset; tables after).
    let look = pr.e("creative/look");
    let input_lut = if pr.e("basicCorrection/inputLut") == 1 { effectcraft_effects::load_lut(pr.s("basicCorrection/inputLutFile")) } else { None };
    let look_lut = if look == 1 { effectcraft_effects::load_lut(pr.s("creative/lookFile")) } else { None };
    d.extend([look as f32, -1.0, -1.0]);
    if let Some(l) = &input_lut {
        d[51] = crate::fx_lut::push_lut(&mut d, l);
    }
    if let Some(l) = &look_lut {
        d[52] = crate::fx_lut::push_lut(&mut d, l);
    }
    let buf = e.data(&d);
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxc_lumetri", &Params::default(), &b.img, None, &out, Some(&buf));
    let sharpen = f("creative/adjustments/sharpen");
    if sharpen == 0.0 {
        return Some(GBuf { img: out, ..b });
    }
    let luma = e.image(out.width, out.height);
    e.pixels("fxc_luma", &Params::default(), &out, None, &luma, None);
    let blurred = gaussian_blur(e, &luma, b.scale, b.scale, true);
    let mut p = Params::default();
    p.f[0][0] = sharpen * 2.0;
    let sharp = e.image(out.width, out.height);
    e.pixels("fxc_sharpen", &p, &out, Some(&blurred), &sharp, None);
    Some(GBuf { img: sharp, ..b })
}

// ---------------------------------------------------------------- planes

/// util::morph_plane with the same radius on both axes; `modes` = per-channel operation
/// (0 erode, 1 dilate, 2 keep).
fn morph(e: &mut Enc, img: &GpuImage, r: u32, modes: [u32; 4]) -> GpuImage {
    if r == 0 {
        return img.clone();
    }
    let mut cur = img.clone();
    for vertical in [0, 1] {
        let mut p = Params::default();
        p.u[0] = [r, vertical, 0, 0];
        p.u[1] = modes;
        let out = e.scratch(cur.width, cur.height);
        e.pixels("fxc_morph", &p, &cur, None, &out, None);
        cur = out;
    }
    cur
}

/// util::morph_frac (blend between the two nearest integer radii).
pub(crate) fn morph_frac(e: &mut Enc, img: &GpuImage, radius: f64, modes: [u32; 4]) -> GpuImage {
    if radius <= 0.0 {
        return img.clone();
    }
    let r0 = radius.floor() as u32;
    let t = (radius - r0 as f64) as f32;
    let a = morph(e, img, r0, modes);
    if t < 1e-4 {
        return a;
    }
    let b = morph(e, img, r0 + 1, modes);
    let mut p = Params::default();
    p.f[0][0] = t;
    let out = e.scratch(img.width, img.height);
    e.pixels("fxc_lerp", &p, &a, Some(&b), &out, None);
    out
}

pub(crate) const ERODE_X: [u32; 4] = [0, 2, 2, 2];
pub(crate) const DILATE_X: [u32; 4] = [1, 2, 2, 2];

// ---------------------------------------------------------------- Key Light

/// keylight::primary.
fn primary(s: [f32; 3]) -> [u32; 3] {
    let pi: usize = if s[1] >= s[0] && s[1] >= s[2] {
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
    let ix = if s[a] >= s[b] { [pi, a, b] } else { [pi, b, a] };
    ix.map(|i| i as u32)
}

fn neutralise(c: [f32; 3], bias: [f32; 3]) -> [f32; 3] {
    let l = luminance(bias[0], bias[1], bias[2]).max(1e-4);
    [0, 1, 2].map(|i| c[i] * l / bias[i].max(1e-4))
}

fn key_light(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let view = pr.e("view");
    if view == 0 {
        return Some(b);
    }
    // Inside / Outside masks (polygon coverage) run on the CPU.
    let has_mask = |id: &str| pr.e(id).checked_sub(1).is_some_and(|i| ctx.env.masks.get(i as usize).is_some());
    if has_mask("insideMask/insideMask") || has_mask("outsideMask/outsideMask") {
        return None;
    }
    let sc = pr.color("screenColour");
    let screen = [sc[0], sc[1], sc[2]];
    let ix = primary(screen);
    let (pi, o1, o2) = (ix[0] as usize, ix[1] as usize, ix[2] as usize);
    let gain = pr.f("screenGain") as f32 / 100.0;
    let bal = pr.f("screenBalance") as f32 / 100.0;
    let dbias = pr.color("despillBias");
    let abias = if pr.b("lockBiasesTogether") { dbias } else { pr.color("alphaBias") };
    let (dbias, abias) = ([dbias[0], dbias[1], dbias[2]], [abias[0], abias[1], abias[2]]);
    let s_alpha = neutralise(screen, abias);
    let sd = (s_alpha[pi] - (bal * s_alpha[o1] + (1.0 - bal) * s_alpha[o2])).max(1e-3);
    let cb = pr.f("screenMatte/clipBlack") as f32 / 100.0;
    let cw = pr.f("screenMatte/clipWhite") as f32 / 100.0;
    let rollback = pr.f("screenMatte/clipRollback") * b.scale;
    let preblur = pr.f("screenPreblur") * b.scale;
    let sg = pr.f("screenMatte/screenShrinkGrow") * b.scale;
    let soft = pr.f("screenMatte/screenSoftness") * b.scale;
    let despot_b = pr.f("screenMatte/screenDespotBlack") * b.scale;
    let despot_w = pr.f("screenMatte/screenDespotWhite") * b.scale;
    // Hard Colour replaces wherever the screen matte exceeds the raw matte by any amount; after
    // Screen Softness's blur that compares float-rounded sums, so it runs on the CPU.
    if pr.e("screenMatte/replaceMethod") == 2 && soft > 0.0 {
        return None;
    }
    let (w, h) = (b.img.width, b.img.height);

    let key_src = if preblur > 0.0 { gaussian_blur(e, &b.img, preblur * 0.5, preblur * 0.5, true) } else { b.img.clone() };
    let mut p = Params::default();
    p.u[0] = [ix[0], ix[1], ix[2], 0];
    p.f[0] = [abias[0], abias[1], abias[2], sd];
    p.f[1] = [gain, bal, cb, cw];
    let raw = e.image(w, h);
    e.pixels("fxc_kl_raw", &p, &key_src, None, &raw, None);
    let mut m = raw.clone();
    if rollback > 0.0 {
        let sg_img = morph_frac(e, &m, rollback, [0, 1, 2, 2]);
        let out = e.scratch(w, h);
        e.pixels("fxc_kl_rollback", &Params::default(), &sg_img, Some(&raw), &out, None);
        m = out;
    }
    if despot_b > 0.0 {
        let d = morph_frac(e, &m, despot_b, DILATE_X);
        m = morph_frac(e, &d, despot_b, ERODE_X);
    }
    if despot_w > 0.0 {
        let d = morph_frac(e, &m, despot_w, ERODE_X);
        m = morph_frac(e, &d, despot_w, DILATE_X);
    }
    if sg < 0.0 {
        m = morph_frac(e, &m, -sg, ERODE_X);
    } else if sg > 0.0 {
        m = morph_frac(e, &m, sg, DILATE_X);
    }
    if soft > 0.0 {
        m = gaussian_blur(e, &m, soft * 0.5, soft * 0.5, true);
    }
    let mut planes = e.image(w, h);
    e.pixels("fxc_kl_pack", &Params::default(), &m, Some(&raw), &planes, None);
    let src_alpha_mode = pr.e("insideMask/sourceAlpha");
    if pr.b("edgeColourCorrection/enableEdgeColourCorrection") {
        let grow = pr.f("edgeColourCorrection/edgeGrow") * b.scale;
        let hard = (pr.f("edgeColourCorrection/edgeHardness") / 100.0) as f32;
        let esoft = pr.f("edgeColourCorrection/edgeSoftness") * b.scale;
        let mut p = Params::default();
        p.u[0][0] = src_alpha_mode;
        let mut band = e.image(w, h);
        e.pixels("fxc_kl_band", &p, &b.img, Some(&planes), &band, None);
        if grow > 0.0 {
            band = morph_frac(e, &band, grow, DILATE_X);
        }
        if esoft > 0.0 {
            band = gaussian_blur(e, &band, esoft * 0.5, esoft * 0.5, true);
        }
        let mut p = Params::default();
        p.f[0][0] = hard;
        let out = e.scratch(w, h);
        e.pixels("fxc_kl_edge", &p, &planes, Some(&band), &out, None);
        planes = out;
    }
    let pc = |k: &str| pr.f(k) as f32 / 100.0;
    let l0 = (luminance(dbias[0], dbias[1], dbias[2]) / dbias[pi].max(1e-4)).max(1e-4);
    let mut p = Params::default();
    p.u[0] = [ix[0], ix[1], ix[2], view];
    p.u[1] = [
        pr.e("screenMatte/replaceMethod"),
        pr.b("foregroundColourCorrection/enableColourCorrection") as u32,
        pr.b("unpremultiplyResult") as u32,
        src_alpha_mode,
    ];
    p.f[0] = [screen[0], screen[1], screen[2], bal];
    p.f[1] = [dbias[0], dbias[1], dbias[2], l0];
    p.f[2] = pr.color("screenMatte/replaceColour");
    p.f[3] =
        [pc("foregroundColourCorrection/saturation"), pc("foregroundColourCorrection/contrast") + 1.0, pc("foregroundColourCorrection/brightness") + 1.0, 0.0];
    p.f[4] = [pc("edgeColourCorrection/edgeSaturation"), pc("edgeColourCorrection/edgeContrast") + 1.0, pc("edgeColourCorrection/edgeBrightness") + 1.0, 0.0];
    p.f[5] = ["sourceCrops/cropLeft", "sourceCrops/cropRight", "sourceCrops/cropTop", "sourceCrops/cropBottom"].map(|k| (pr.f(k) / 100.0) as f32);
    p.f[6] = [b.offset[0] as f32, b.offset[1] as f32, b.scale as f32, 0.0];
    p.f[7] = [ctx.layer_size[0].max(1e-9) as f32, ctx.layer_size[1].max(1e-9) as f32, 0.0, 0.0];
    let out = e.scratch(w, h);
    e.pixels("fxc_kl_final", &p, &b.img, Some(&planes), &out, None);
    Some(GBuf { img: out, ..b })
}
