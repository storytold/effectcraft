//! GPU effects, keying, matte and channel family (kernels in `shaders/fx_key.wgsl`, every entry
//! point prefixed `fxk_`): the CPU effects' exact steps with the pixel loops as compute kernels.
//!
//! The keyers (Color Key, Luma Key, Color Range, Extract, Difference Matte, Color Difference
//! Key, Screen Key) build their matte in a plane pass, run the CPU's edge thin (morphology) and
//! feather (Gaussian) on it, and apply it in a second pass; spill suppression, Unmult and the
//! channel effects are one per-pixel pass (`fxk_point`). Simple / Matte Choker and Key Cleaner
//! work on the alpha plane and put it back with `util::set_alpha` (straight colour kept, the
//! transparent pixels' colour from a small blur). Advanced Spill Suppressor's Standard method
//! finds the dominant key primary with a GPU sum reduction (`fxk_reduce`). Other layers (layer
//! parameters) are resampled into the buffer like `util::fit_layer` (`fxk_fit`).

use effectcraft_color::rgb_to_hsl;
use effectcraft_effects::EffectCtx;
use effectcraft_effects::util::{SRC_ORDER, Src};

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, box_passes, gaussian_blur};
use crate::fx_color::{DILATE_X, ERODE_X, morph_frac};

/// Compute entry points in `fx_key.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxk_point", "fxk_fit", "fxk_reduce"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.key.colorkey",
    "ec.key.luma",
    "ec.key.colorrange",
    "ec.key.extract",
    "ec.key.differencematte",
    "ec.key.colordifference",
    "ec.key.screen",
    "ec.key.spill",
    "ec.key.advancedspill",
    "ec.key.keycleaner",
    "ec.key.unmult",
    "ec.matte.simplechoker",
    "ec.matte.mattechoker",
    "ec.matte.refinesoft",
    "ec.matte.refinehard",
    "ec.channel.setmatte",
    "ec.channel.setchannels",
    "ec.channel.shiftchannels",
    "ec.channel.removecolormatting",
    "ec.channel.arithmetic",
    "ec.channel.solidcomposite",
    "ec.channel.combiner",
    "ec.channel.blend",
    "ec.channel.calculations",
    "ec.channel.compoundarithmetic",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.key.colorkey" | "ec.key.luma" | "ec.key.colorrange" | "ec.key.extract" => matte_key(e, id, ctx, b),
        "ec.key.differencematte" => difference_matte(e, ctx, b),
        "ec.key.screen" => screen_key(e, ctx, b),
        "ec.key.advancedspill" => advanced_spill(e, ctx, b),
        "ec.key.keycleaner" => key_cleaner(e, ctx, b),
        "ec.matte.simplechoker" => simple_choker(e, ctx, b),
        "ec.matte.mattechoker" => matte_choker(e, ctx, b),
        "ec.matte.refinesoft" => refine(e, ctx, b, false),
        "ec.matte.refinehard" => refine(e, ctx, b, true),
        "ec.channel.setchannels" => set_channels(e, ctx, b),
        _ => point(e, id, ctx, b),
    }
}

// ---------------------------------------------------------------- helpers

/// One `fxk_point` pass over `src`'s size.
fn run(e: &mut Enc, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels("fxk_point", p, src, aux, &out, data);
    out
}

fn op(code: u32) -> Params {
    let mut p = Params::default();
    p.u[0][0] = code;
    p
}

fn src_index(s: Src) -> u32 {
    SRC_ORDER.iter().position(|x| *x == s).unwrap_or(10) as u32
}

/// Layer parameter `id` resampled into the buffer's pixel grid (`util::fit_layer`):
/// `Some(None)` when no layer is chosen (the effect then uses its own layer).
pub(crate) fn fitted(e: &mut Enc, ctx: &EffectCtx, b: &GBuf, id: &str, masks_and_effects: bool, stretch: bool) -> Option<Option<GpuImage>> {
    let Some(o) = ctx.layer_param(id, masks_and_effects) else { return Some(None) };
    let (w, h) = (b.img.width, b.img.height);
    if o.buf.img.is_empty() {
        return Some(Some(e.image(w, h)));
    }
    let ls = ctx.layer_size;
    let os = o.size;
    let (sx, sy) = if stretch && ls[0] > 0.0 && ls[1] > 0.0 { (os[0] / ls[0], os[1] / ls[1]) } else { (1.0, 1.0) };
    let (dx, dy) = if stretch { (0.0, 0.0) } else { ((os[0] - ls[0]) * 0.5, (os[1] - ls[1]) * 0.5) };
    let inv = 1.0 / b.scale.max(1e-9);
    // Per column / row: the other layer's buffer coordinate.
    let mut data = Vec::with_capacity((w + h) as usize);
    data.extend((0..w).map(|x| (((x as f64 + 0.5 - b.offset[0]) * inv * sx + dx) * o.buf.scale + o.buf.offset[0]) as f32));
    data.extend((0..h).map(|y| (((y as f64 + 0.5 - b.offset[1]) * inv * sy + dy) * o.buf.scale + o.buf.offset[1]) as f32));
    let src = e.g.upload_image(&o.buf.img)?;
    let buf = e.data(&data);
    let out = e.scratch(w, h);
    e.pixels("fxk_fit", &Params::default(), &src, None, &out, Some(&buf));
    Some(Some(out))
}

/// `util::layer_or_self`.
fn layer_or_self(e: &mut Enc, ctx: &EffectCtx, b: &GBuf, id: &str, stretch: bool) -> Option<GpuImage> {
    Some(fitted(e, ctx, b, id, true, stretch)?.unwrap_or_else(|| b.img.clone()))
}

/// keying::thin_feather on a matte plane (x channel).
fn thin_feather(e: &mut Enc, m: GpuImage, thin: f64, feather: f64) -> GpuImage {
    let m = if thin > 0.0 {
        morph_frac(e, &m, thin, ERODE_X)
    } else if thin < 0.0 {
        morph_frac(e, &m, -thin, DILATE_X)
    } else {
        m
    };
    if feather > 0.0 { gaussian_blur(e, &m, feather * 0.5, feather * 0.5, true) } else { m }
}

/// keying::apply_matte (view 0 final, 1 source, 2 matte).
fn apply_matte(e: &mut Enc, b: GBuf, m: &GpuImage, view: u32) -> GBuf {
    if view == 1 {
        return b;
    }
    let mut p = op(5);
    p.u[0][1] = view;
    let img = run(e, &p, &b.img, Some(m), None);
    GBuf { img, ..b }
}

/// keying::primary.
fn primary(c: [f32; 4]) -> [u32; 3] {
    if c[1] >= c[0] && c[1] >= c[2] {
        [1, 0, 2]
    } else if c[2] >= c[0] && c[2] >= c[1] {
        [2, 0, 1]
    } else {
        [0, 1, 2]
    }
}

/// util::set_alpha with the new alpha in `na`'s x channel.
fn set_alpha(e: &mut Enc, img: &GpuImage, na: &GpuImage) -> GpuImage {
    let fill = gaussian_blur(e, img, 3.0, 3.0, true);
    let (rows, row) = e.image_rows(&fill);
    let mut p = op(27);
    p.u[3][0] = row;
    run(e, &p, img, Some(na), Some(&rows))
}

// ---------------------------------------------------------------- keyers

fn matte_key(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |k: &str| pr.f(k) as f32;
    let mut p = Params::default();
    let mut thin = 0.0;
    let mut feather = 0.0;
    match id {
        "ec.key.colorkey" => {
            let k = pr.color("keyColor");
            p.u[0][0] = 1;
            p.f[0] = [k[0], k[1], k[2], f("colorTolerance") / 255.0];
            thin = pr.f("edgeThin") * b.scale;
            feather = pr.f("edgeFeather") * b.scale;
        }
        "ec.key.luma" => {
            p.u[0] = [2, pr.e("keyType"), 0, 0];
            p.f[0] = [f("threshold") / 255.0, f("tolerance") / 255.0, 0.0, 0.0];
            thin = pr.f("edgeThin") * b.scale;
            feather = pr.f("edgeFeather") * b.scale;
        }
        "ec.key.colorrange" => {
            p.u[0] = [3, pr.e("colorSpace"), 0, 0];
            p.f[0] = [f("minL"), f("minA"), f("minB"), 0.0];
            p.f[1] = [f("maxL"), f("maxA"), f("maxB"), 0.0];
            p.f[2][0] = f("fuzziness");
        }
        _ => {
            p.u[0] = [4, pr.e("channel"), pr.b("invert") as u32, 0];
            p.f[0] = [f("blackPoint"), f("whitePoint"), f("blackSoftness"), f("whiteSoftness")];
        }
    }
    let m = run(e, &p, &b.img, None, None);
    let m = thin_feather(e, m, thin, feather);
    Some(apply_matte(e, b, &m, 0))
}

fn difference_matte(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let view = pr.e("view");
    if view == 1 {
        return Some(b);
    }
    let blur = pr.f("blurBeforeDifference") * b.scale;
    let other = layer_or_self(e, ctx, &b, "differenceLayer", pr.e("ifLayerSizesDiffer") == 1)?;
    let (src, other) = if blur > 0.05 {
        (gaussian_blur(e, &b.img, blur * 0.5, blur * 0.5, true), gaussian_blur(e, &other, blur * 0.5, blur * 0.5, true))
    } else {
        (b.img.clone(), other)
    };
    let mut p = op(6);
    p.f[0] = [(pr.f("matchingTolerance") / 100.0) as f32, (pr.f("matchingSoftness") / 100.0) as f32, 0.0, 0.0];
    let m = run(e, &p, &src, Some(&other), None);
    Some(apply_matte(e, b, &m, if view == 2 { 3 } else { 4 }))
}

/// keying::secondary's screen-key matte and composite.
fn screen_key(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let view = pr.e("view");
    if view == 3 {
        return Some(b);
    }
    let sc = pr.color("screenColor");
    let ix = primary(sc);
    let (pi, o1, o2) = (ix[0] as usize, ix[1] as usize, ix[2] as usize);
    let gain = pr.f("screenGain") as f32 / 100.0;
    let bal = pr.f("screenBalance") as f32 / 100.0;
    let cb = pr.f("clipBlack") as f32 / 100.0;
    let cw = pr.f("clipWhite") as f32 / 100.0;
    let preblur = pr.f("screenPreblur") * b.scale;
    let sg = pr.f("screenShrinkGrow") * b.scale;
    let soft = pr.f("screenSoftness") * b.scale;
    let (hi, lo) = if sc[o1] > sc[o2] { (sc[o1], sc[o2]) } else { (sc[o2], sc[o1]) };
    let sd = (sc[pi] - (bal * hi + (1.0 - bal) * lo)).max(1e-3);
    let key_src = if preblur > 0.0 { gaussian_blur(e, &b.img, preblur * 0.5, preblur * 0.5, true) } else { b.img.clone() };
    let mut p = op(22);
    p.u[1] = [ix[0], ix[1], ix[2], 0];
    p.f[0] = [sd, gain, bal, cb];
    p.f[1][0] = cw;
    let mut m = run(e, &p, &key_src, None, None);
    if sg < 0.0 {
        m = morph_frac(e, &m, -sg, ERODE_X);
    } else if sg > 0.0 {
        m = morph_frac(e, &m, sg, DILATE_X);
    }
    if soft > 0.0 {
        m = gaussian_blur(e, &m, soft * 0.5, soft * 0.5, true);
    }
    let mut p = op(23);
    p.u[0] = [23, view, (pr.e("edgeColor") == 1) as u32, 0];
    p.u[1] = [ix[0], ix[1], ix[2], 0];
    p.f[0] = [sc[0], sc[1], sc[2], bal];
    p.f[1][0] = pr.f("despill") as f32 / 100.0;
    let img = run(e, &p, &b.img, Some(&m), None);
    Some(GBuf { img, ..b })
}

fn advanced_spill(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let ultra = pr.e("method") == 1;
    let key = pr.color("ultraSettings/keyColor");
    let mut p = op(8);
    let sums = if ultra {
        let ix = primary(key);
        p.u[1] = [ix[0], ix[1], ix[2], 0];
        None
    } else {
        // Standard: the dominant key primary over the whole buffer (sum reduction).
        p.u[1][3] = 1;
        let mut cur = b.img.clone();
        let mut first = true;
        while first || cur.width > 1 || cur.height > 1 {
            let out = e.scratch(cur.width.div_ceil(16), cur.height.div_ceil(16));
            let mut rp = Params::default();
            rp.u[0][0] = first as u32;
            e.pixels("fxk_reduce", &rp, &cur, None, &out, None);
            cur = out;
            first = false;
        }
        Some(cur)
    };
    let tol = if ultra { pr.get("ultraSettings/tolerance").map(|v| v.as_f64()).unwrap_or(100.0) as f32 / 100.0 } else { 1.0 };
    let f = |k: &str| pr.f(k) as f32 / 100.0;
    let (desat, neutral) = if ultra { (f("ultraSettings/desaturate"), f("ultraSettings/spillColorCorrection")) } else { (0.0, 0.0) };
    p.f[0] = [f("suppression"), f("ultraSettings/spillRange"), f("ultraSettings/lumaCorrection"), tol];
    p.f[1] = [desat, neutral, rgb_to_hsl(key[0], key[1], key[2]).0, 0.0];
    let img = run(e, &p, &b.img, sums.as_ref(), None);
    Some(GBuf { img, ..b })
}

fn key_cleaner(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let r = (pr.f("additionalEdgeRadius") * b.scale).max(0.0);
    let strength = pr.f("strength") as f32 / 100.0;
    let chatter = pr.b("reduceChatter");
    let mut a = run(e, &op(24), &b.img, None, None);
    if chatter {
        a = gaussian_blur(e, &a, 1.0, 1.0, true);
    }
    let est = (r > 0.0 && strength > 0.0).then(|| gaussian_blur(e, &b.img, r * 0.5, r * 0.5, true));
    let rows = est.as_ref().map(|img| e.image_rows(img));
    let mut p = op(25);
    p.u[0][1] = chatter as u32;
    p.u[3] = [rows.as_ref().map_or(0, |r| r.1), rows.is_some() as u32, 0, 0];
    p.f[0] = [pr.f("alphaContrast") as f32 / 100.0, strength, 0.0, 0.0];
    let img = run(e, &p, &b.img, Some(&a), rows.as_ref().map(|r| &r.0));
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- mattes

fn simple_choker(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let c = ctx.params.f("chokeMatte") * b.scale;
    if c.abs() > 1e-6 {
        if c < 0.0 && !ctx.adjustment {
            b.pad(e, (-c).ceil() as u32 + 1)?;
        }
        let a = run(e, &op(24), &b.img, None, None);
        let na = if c > 0.0 { morph_frac(e, &a, c, ERODE_X) } else { morph_frac(e, &a, -c, DILATE_X) };
        b.img = set_alpha(e, &b.img, &na);
    }
    if ctx.params.e("view") == 1 {
        b.img = run(e, &op(28), &b.img, None, None);
    }
    Some(b)
}

fn matte_choker(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let g1 = pr.f("geometricSoftness1") * b.scale;
    let c1 = pr.f("choke1") as f32;
    let s1 = pr.f("grayLevelSoftness1") as f32 / 100.0;
    let g2 = pr.f("geometricSoftness2") * b.scale;
    let c2 = pr.f("choke2") as f32;
    let s2 = pr.f("grayLevelSoftness2") as f32 / 100.0;
    let it = pr.f("iterations").round().clamp(1.0, 100.0) as usize;
    if (c1 < 0.0 || c2 < 0.0 || g1 > 0.0 || g2 > 0.0) && !ctx.adjustment {
        b.pad(e, ((g1 + g2) * 1.5 * it as f64).ceil() as u32 + 1)?;
    }
    let mut a = run(e, &op(24), &b.img, None, None);
    let stage = |e: &mut Enc, a: GpuImage, geo: f64, choke: f32, gray: f32| -> GpuImage {
        let a = if geo > 0.0 { gaussian_blur(e, &a, geo * 0.5, geo * 0.5, true) } else { a };
        let t = 0.5 + choke / 255.0 * 0.5;
        let w = gray.max(1e-3);
        if (t - 0.5).abs() < 1e-6 && (w - 1.0).abs() < 1e-6 {
            return a;
        }
        let mut p = op(26);
        p.f[0] = [t, w, 0.0, 0.0];
        run(e, &p, &a, None, None)
    };
    for _ in 0..it {
        a = stage(e, a, g1, c1, s1);
        a = stage(e, a, g2, c2, s2);
    }
    b.img = set_alpha(e, &b.img, &a);
    Some(b)
}

/// matte::refine (Refine Soft / Hard Matte) for one frame: Reduce Chatter, the matte motion
/// blur (they read neighbouring frames) and Invert run on the CPU.
fn refine(e: &mut Enc, ctx: &EffectCtx, b: GBuf, hard: bool) -> Option<GBuf> {
    let pr = ctx.params;
    // Invert gives alpha to the empty areas, whose colour then comes from a blur of nearly
    // transparent pixels (util::set_alpha): ill-conditioned, so it renders on the CPU.
    if pr.f("reduceChatter") > 0.0 || pr.b("useMotionBlur") || pr.b("invert") {
        return None;
    }
    let dk = |id: &str| if hard { id.to_string() } else { format!("decontamination/{id}") };
    let r = (pr.f("radius") * b.scale).round().max(1.0) as usize;
    let smooth = pr.f("smooth") as f32 / 100.0;
    let feather = pr.f("feather") / 100.0 * r as f64 * 0.5;
    let choke = (pr.f("choke") - if hard { 0.0 } else { pr.f("shiftEdge") }).clamp(-100.0, 100.0) as f32 / 100.0 * 0.5;
    let decon = pr.b("decontaminate");
    let decon_amt = pr.f(&dk("decontaminationAmount")) as f32 / 100.0;
    let extend = pr.b(&dk("extendWhereSmoothed"));
    let decon_r = r as f64 + pr.f(&dk("increaseDecontaminationRadius")).max(0.0) * b.scale;
    let view_map = pr.b(&dk("viewDecontaminationMap"));
    let contrast = if hard { pr.f("alphaContrast") as f32 / 100.0 } else { 1.0 + pr.f("contrast").max(0.0) as f32 / 100.0 * 3.0 };
    let edge_details = hard || pr.b("calculateEdgeDetails");
    let view_edges = !hard && pr.b("viewEdgeRegion");
    let (w, h) = (b.img.width, b.img.height);

    // (luminance guide, alpha, guide × alpha, guide²); the edge band from the alpha's range.
    let pack = run(e, &op(29), &b.img, None, None);
    let hi = morph_frac(e, &pack, r as f64, [2, 1, 2, 2]);
    let lo = morph_frac(e, &pack, r as f64, [2, 0, 2, 2]);
    let band = run(e, &op(32), &hi, Some(&lo), None);
    let mut na = if edge_details {
        let eps = 1e-4 + smooth * smooth * 0.05;
        let means = box_passes(e, &pack, &[r], &[r], true);
        let mut p = op(30);
        p.f[0][0] = eps;
        let coef = run(e, &p, &means, None, None);
        let coef = box_passes(e, &coef, &[r], &[r], true);
        let (rows, row) = e.image_rows(&band);
        let mut p = op(31);
        p.u[3][0] = row;
        run(e, &p, &coef, Some(&pack), Some(&rows))
    } else {
        let a = run(e, &op(24), &b.img, None, None);
        if smooth > 0.0 {
            let s = smooth as f64 * r as f64 * 0.5;
            gaussian_blur(e, &a, s, s, true)
        } else {
            a
        }
    };
    if feather > 0.0 {
        na = gaussian_blur(e, &na, feather, feather, true);
    }
    let mut p = op(33);
    p.u[0][1] = pr.b("invert") as u32;
    p.f[0] = [choke, contrast, 0.0, 0.0];
    let na = run(e, &p, &na, None, None);
    if view_edges {
        let img = run(e, &op(34), &na, Some(&band), None);
        return Some(GBuf { img, ..b });
    }
    let est = (decon && decon_amt > 0.0 && !view_map).then(|| gaussian_blur(e, &b.img, decon_r * 0.5, decon_r * 0.5, true));
    let rows = est.as_ref().map(|img| e.image_rows(img));
    let mut p = op(35);
    p.u[0] = [35, view_map as u32, (decon && (view_map || decon_amt > 0.0)) as u32, extend as u32];
    p.u[3] = [rows.as_ref().map_or(0, |r| r.1), rows.is_some() as u32, 0, 0];
    p.f[0][0] = decon_amt;
    let img = run(e, &p, &b.img, Some(&na), rows.as_ref().map(|r| &r.0));
    if view_map {
        return Some(GBuf { img, ..b });
    }
    debug_assert_eq!((img.width, img.height), (w, h));
    let img = set_alpha(e, &img, &na);
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- channel

const SET_CHANNEL_LAYERS: [&str; 4] = ["sourceLayer1", "sourceLayer2", "sourceLayer3", "sourceLayer4"];

/// Shift Channels' Take … From menu, in the order of `effects`' `SHIFT_ORDER`: the two must agree,
/// or the source depends on whether the effect renders on the GPU.
const SHIFT_ORDER: [Src; 11] =
    [Src::Alpha, Src::Red, Src::Green, Src::Blue, Src::Luminance, Src::Hue, Src::Lightness, Src::Saturation, Src::Full, Src::Off, Src::Half];

const MATTE_ORDER: [Src; 10] = [Src::Red, Src::Green, Src::Blue, Src::Alpha, Src::Luminance, Src::Hue, Src::Lightness, Src::Saturation, Src::Full, Src::Off];

fn set_channels(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let s = ["setRedTo", "setGreenTo", "setBlueTo", "setAlphaTo"].map(|id| src_index(effectcraft_effects::util::src_at(pr.e(id))));
    let stretch = pr.b("stretchLayersToFit");
    let mut layers = vec![];
    for id in SET_CHANNEL_LAYERS {
        layers.push(fitted(e, ctx, &b, id, true, stretch)?);
    }
    if layers.iter().all(Option::is_none) {
        return route(e, b, s);
    }
    let mut vals = e.image(b.img.width, b.img.height);
    for (k, layer) in layers.iter().enumerate() {
        let mut p = op(20);
        p.u[0] = [20, k as u32, s[k], 0];
        vals = run(e, &p, &vals, Some(layer.as_ref().unwrap_or(&b.img)), None);
    }
    let img = run(e, &op(21), &vals, None, None);
    Some(GBuf { img, ..b })
}

/// channel::route.
fn route(e: &mut Enc, b: GBuf, s: [u32; 4]) -> Option<GBuf> {
    if s == [0, 1, 2, 3] {
        return Some(b);
    }
    let mut p = op(11);
    p.u[1] = s;
    let img = run(e, &p, &b.img, None, None);
    Some(GBuf { img, ..b })
}

fn point(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let f = |k: &str| pr.f(k) as f32;
    let mut p = Params::default();
    let mut aux = None;
    match id {
        "ec.key.spill" => {
            let k = pr.color("colorToSuppress");
            let amt = f("suppression") / 100.0;
            if pr.e("colorAccuracy") == 1 {
                let kg = (k[0] + k[1] + k[2]) / 3.0;
                let kc = [k[0] - kg, k[1] - kg, k[2] - kg];
                let kn = (kc[0] * kc[0] + kc[1] * kc[1] + kc[2] * kc[2]).sqrt();
                if kn < 1e-5 {
                    return Some(b);
                }
                p.u[0] = [7, 1, 0, 0];
                p.f[0] = [kc[0] / kn, kc[1] / kn, kc[2] / kn, amt];
            } else {
                let ix = primary(k);
                p.u[0] = [7, 0, 0, 0];
                p.u[1] = [ix[0], ix[1], ix[2], 0];
                p.f[0][3] = amt;
            }
        }
        "ec.key.colordifference" => {
            let view = pr.e("view");
            if view == 0 {
                return Some(b);
            }
            let accurate = pr.e("colorMatchingAccuracy") == 1;
            let lin = |c: [f32; 3]| if accurate { c.map(|v| effectcraft_color::srgb_to_linear(v.clamp(0.0, 1.0))) } else { c };
            let k = pr.color("keyColor");
            let key = lin([k[0], k[1], k[2]]);
            let ix = primary([key[0], key[1], key[2], 1.0]);
            let (pi, o1, o2) = (ix[0] as usize, ix[1] as usize, ix[2] as usize);
            let lv = |pre: &str| ["InBlack", "InWhite", "Gamma", "OutBlack", "OutWhite"].map(|s| f(&format!("{pre}{s}")));
            let (pa, pb) = (lv("partialA"), lv("partialB"));
            p.u[0] = [9, view, accurate as u32, 0];
            p.u[1] = [ix[0], ix[1], ix[2], 0];
            p.f[0] = [(key[pi] - key[o1]).max(1e-3), (key[pi] - key[o2]).max(1e-3), f("blackLevel"), f("whiteLevel")];
            p.f[1] = [pa[0], pa[1], pa[2], pa[3]];
            p.f[2] = [pa[4], pb[4], f("gamma"), 0.0];
            p.f[3] = [pb[0], pb[1], pb[2], pb[3]];
        }
        "ec.key.unmult" => {
            let white = pr.e("backgroundColor") == 1;
            let level = if white { pr.f("whiteLevel") } else { pr.f("blackLevel") }.clamp(0.0, 100.0) as f32 / 100.0;
            p.u[0] = [10, white as u32, pr.b("removeColorMatting") as u32, pr.b("clipHdrResults") as u32];
            p.f[0] = [level, pr.f("softness").clamp(0.0, 100.0) as f32 / 100.0, 0.0, 0.0];
        }
        "ec.channel.shiftchannels" => {
            let s = ["takeRedFrom", "takeGreenFrom", "takeBlueFrom", "takeAlphaFrom"]
                .map(|id| src_index(SHIFT_ORDER[(pr.e(id) as usize).min(SHIFT_ORDER.len() - 1)]));
            return route(e, b, s);
        }
        "ec.channel.removecolormatting" => {
            p.u[0] = [12, pr.b("clipHdr") as u32, 0, 0];
            p.f[0] = pr.color("backgroundColor");
        }
        "ec.channel.arithmetic" => {
            p.u[0] = [13, pr.e("operator"), pr.b("clip") as u32, 0];
            p.f[0] = [f("redValue") / 255.0, f("greenValue") / 255.0, f("blueValue") / 255.0, 0.0];
        }
        "ec.channel.solidcomposite" => {
            const MODES: [effectcraft_color::BlendMode; 12] = {
                use effectcraft_color::BlendMode::*;
                [Normal, Add, Multiply, Screen, Overlay, SoftLight, HardLight, Darken, Lighten, Difference, Color, Luminosity]
            };
            let c = pr.color("color");
            let opa = (f("opacity") / 100.0 * c[3]).clamp(0.0, 1.0);
            p.u[0] = [14, crate::ops::mode_id(MODES[(pr.e("blendingMode") as usize).min(11)]), 0, 0];
            p.f[0] = [c[0] * opa, c[1] * opa, c[2] * opa, opa];
            p.f[1][0] = f("sourceOpacity") / 100.0;
        }
        "ec.channel.setmatte" => {
            let src = MATTE_ORDER[(pr.e("takeMatteFrom") as usize).min(9)];
            aux = fitted(e, ctx, &b, "takeMatteFromLayer", true, pr.b("stretchMatteToFit"))?;
            let pre_ok = !matches!(src, Src::Alpha | Src::Full | Src::Off);
            let flags = pr.b("invertMatte") as u32
                | (pr.b("compositeMatteWithOriginal") as u32) << 1
                | ((pr.b("premultiplyMatteLayer") && pre_ok) as u32) << 2
                | (aux.is_none() as u32) << 3
                | ((src == Src::Alpha) as u32) << 4;
            p.u[0] = [15, src_index(src), flags, 0];
        }
        "ec.channel.combiner" => {
            if pr.b("useSecondLayer") {
                aux = Some(layer_or_self(e, ctx, &b, "sourceLayer", true)?);
            }
            let flags = pr.b("invert") as u32 | (pr.b("solidAlpha") as u32) << 1 | (aux.is_some() as u32) << 2;
            p.u[0] = [16, pr.e("from"), pr.e("to"), flags];
        }
        "ec.channel.blend" => {
            let orig = (f("blendWithOriginal") / 100.0).clamp(0.0, 1.0);
            if orig >= 1.0 {
                return Some(b);
            }
            aux = Some(layer_or_self(e, ctx, &b, "blendWithLayer", pr.e("ifLayerSizesDiffer") == 1)?);
            p.u[0] = [17, pr.e("mode"), 0, 0];
            p.f[0][0] = orig;
        }
        "ec.channel.calculations" => {
            const MODES: [effectcraft_color::BlendMode; 22] = {
                use effectcraft_color::BlendMode::*;
                [
                    Normal,
                    Darken,
                    Multiply,
                    ColorBurn,
                    LinearBurn,
                    Add,
                    Lighten,
                    Screen,
                    ColorDodge,
                    LinearDodge,
                    Overlay,
                    SoftLight,
                    HardLight,
                    LinearLight,
                    VividLight,
                    PinLight,
                    HardMix,
                    Difference,
                    Exclusion,
                    Hue,
                    Saturation,
                    Luminosity,
                ]
            };
            if pr.get("secondLayer").and_then(|v| v.as_layer()).is_some() {
                aux = Some(layer_or_self(e, ctx, &b, "secondLayer", pr.b("stretchSecondLayerToFit"))?);
            }
            let flags =
                pr.b("invertInput") as u32 | (pr.b("invertSecondLayer") as u32) << 1 | (pr.b("preserveTransparency") as u32) << 2 | (aux.is_some() as u32) << 3;
            let mode = MODES[(pr.e("blendingMode") as usize).min(MODES.len() - 1)];
            p.u[0] = [18, pr.e("inputChannel"), pr.e("secondLayerChannel"), crate::ops::mode_id(mode)];
            p.u[1][0] = flags;
            p.f[0][0] = (f("secondLayerOpacity") / 100.0).clamp(0.0, 1.0);
        }
        "ec.channel.compoundarithmetic" => {
            let orig = (f("blendWithOriginal") / 100.0).clamp(0.0, 1.0);
            if orig >= 1.0 {
                return Some(b);
            }
            aux = Some(layer_or_self(e, ctx, &b, "secondSource", pr.b("stretchSecondSourceToFit"))?);
            p.u[0] = [19, pr.e("operator"), pr.e("operateOnChannels"), pr.e("overflowBehavior")];
            p.f[0][0] = orig;
        }
        _ => return None,
    }
    let img = run(e, &p, &b.img, aux.as_ref(), None);
    Some(GBuf { img, ..b })
}
