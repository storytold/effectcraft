//! GPU effects, pixel ports of part C (kernels in `shaders/fx_pixel2.wgsl`, every entry point
//! prefixed `fp2_`): CC Cross Blur, CC Radial Blur, CC Vector Blur, Reduce Interlace Flicker,
//! CC Composite, Color Link, Cineon Converter, HDR Compander, HDR Highlight Compression, Grow
//! Bounds, CC Overbrights, CC Block Load, CC Burn Film, CC Glass, CC HexTile, CC Mr. Smoothie,
//! CC Plastic, Inner/Outer Key, CC Simple Wire Removal and Basic 3D, with the CPU effects'
//! exact steps (padding, box-blur radii, parameter conversions in f64 on the CPU).
//!
//! What only depends on parameters and geometry is prepared on the CPU and shared with the CPU
//! effect: Inner/Outer Key's trimap and Cleanup strokes (`effects::inner_outer_plan`), CC Glass /
//! Plastic's light (`effects::BumpLight`). Color Link's sample statistics (sorting, trimmed
//! means) run on the CPU over the read-back layer (`effects::color_link_sample`) unless a Source
//! Layer is chosen; the colouring runs here.

use effectcraft_color::BlendMode;
use effectcraft_effects::EffectCtx;
use effectcraft_raster::Image;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, box_passes, gaussian_blur};
use crate::ops::mode_id;

/// Compute entry points in `fx_pixel2.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fp2_cross",
    "fp2_box",
    "fp2_radial",
    "fp2_plane",
    "fp2_vector",
    "fp2_point",
    "fp2_block_means",
    "fp2_block_load",
    "fp2_burn",
    "fp2_bump",
    "fp2_hextile",
    "fp2_smoothie",
    "fp2_io_prep",
    "fp2_io_known",
    "fp2_io_est",
    "fp2_io_strokes",
    "fp2_io_final",
    "fp2_wire",
    "fp2_basic3d",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.blur.cccross",
    "ec.blur.ccradial",
    "ec.blur.ccvector",
    "ec.blur.reduceflicker",
    "ec.channel.cccomposite",
    "ec.color.colorlink",
    "ec.utility.cineon",
    "ec.utility.hdrcompander",
    "ec.utility.hdrcompression",
    "ec.utility.growbounds",
    "ec.utility.ccoverbrights",
    "ec.stylize.ccblockload",
    "ec.stylize.ccburnfilm",
    "ec.stylize.ccglass",
    "ec.stylize.cchextile",
    "ec.stylize.ccmrsmoothie",
    "ec.stylize.ccplastic",
    "ec.key.innerouter",
    "ec.key.ccsimplewireremoval",
    "ec.obsolete.basic3d",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.blur.cccross" => cross_blur(e, ctx, b),
        "ec.blur.ccradial" => radial_blur(e, ctx, b),
        "ec.blur.ccvector" => vector_blur(e, ctx, b),
        "ec.blur.reduceflicker" => reduce_flicker(e, ctx, b),
        "ec.utility.growbounds" => grow_bounds(e, ctx, b),
        "ec.stylize.ccblockload" => block_load(e, ctx, b),
        "ec.stylize.ccburnfilm" => burn_film(e, ctx, b),
        "ec.stylize.ccglass" | "ec.stylize.ccplastic" => bump(e, id, ctx, b),
        "ec.stylize.cchextile" => hextile(e, ctx, b),
        "ec.stylize.ccmrsmoothie" => smoothie(e, ctx, b),
        "ec.key.innerouter" => inner_outer(e, ctx, b),
        "ec.key.ccsimplewireremoval" => wire_removal(e, ctx, b),
        "ec.obsolete.basic3d" => basic_3d(e, ctx, b),
        _ => point(e, id, ctx, b),
    }
}

/// A same-size per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, aux, &out, data);
    b.img = out;
    Some(b)
}

/// A one-value plane (x) of `img` (`fp2_plane` kind / property), softened by util::gauss_plane.
fn plane(e: &mut Enc, img: &GpuImage, kind: u32, prop: u32, soft: f64) -> GpuImage {
    let mut p = Params::default();
    p.u[0] = [kind, prop, 0, 0];
    let pl = e.scratch(img.width, img.height);
    e.pixels("fp2_plane", &p, img, None, &pl, None);
    gauss_plane(e, &pl, soft, soft)
}

/// util::gauss_plane on every channel: 3 box passes per axis (edges repeated), each window
/// summed directly (the CPU sums in f64; see `fp2_box`).
pub(crate) fn gauss_plane(e: &mut Enc, img: &GpuImage, sx: f64, sy: f64) -> GpuImage {
    let mut cur = img.clone();
    for (vertical, sigma) in [(0, sx), (1, sy)] {
        if sigma <= 0.05 {
            continue;
        }
        for r in crate::effects::box_radii(sigma, 3) {
            if r == 0 {
                continue;
            }
            let mut p = Params::default();
            p.u[0] = [r as u32, vertical, 0, 0];
            let out = e.scratch(cur.width, cur.height);
            e.pixels("fp2_box", &p, &cur, None, &out, None);
            cur = out;
        }
    }
    cur
}

// ---------------------------------------------------------------- blurs (blur2, blur3)

fn cross_blur(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let rx = (ctx.params.f("radiusX").max(0.0) * b.scale).round() as usize;
    let ry = (ctx.params.f("radiusY").max(0.0) * b.scale).round() as usize;
    if rx == 0 && ry == 0 {
        return Some(b);
    }
    if !ctx.adjustment {
        b.pad(e, (rx.max(ry) * 3) as u32 + 1)?;
    }
    let repeat = ctx.adjustment;
    let hz = box_passes(e, &b.img, &[rx; 3], &[], repeat);
    let vt = box_passes(e, &b.img, &[], &[ry; 3], repeat);
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("transferMode");
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fp2_cross", &p, &hz, Some(&vt), &out, None);
    b.img = out;
    Some(b)
}

fn radial_blur(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amount = ctx.params.f("amount");
    if amount == 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("type");
    p.f[0] = [c.0 as f32, c.1 as f32, ((amount / 100.0).clamp(-1.0, 1.0) * 0.5) as f32, (amount.to_radians() * 0.25) as f32];
    p.f[1][0] = ctx.params.f("quality").clamp(1.0, 100.0) as f32;
    run(e, "fp2_radial", &p, b, None, None)
}

fn vector_blur(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amount = ctx.params.f("amount").max(0.0) * b.scale;
    if amount < 0.5 {
        return Some(b);
    }
    let soft = ctx.params.f("mapSoftness").max(0.0) * 0.5 * b.scale;
    let map = plane(e, &b.img, 0, 0, soft);
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("type");
    p.f[0] = [amount as f32, ctx.params.f("angleOffset").to_radians() as f32, 0.0, 0.0];
    run(e, "fp2_vector", &p, b, Some(&map), None)
}

fn reduce_flicker(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let s = ctx.params.f("softness").max(0.0) * b.scale;
    if s < 0.05 {
        return Some(b);
    }
    b.img = gaussian_blur(e, &b.img, 0.0, s, true);
    Some(b)
}

fn grow_bounds(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let px = (ctx.params.f("pixels") * b.scale).round().max(0.0) as u32;
    if !ctx.adjustment {
        b.pad(e, px)?;
    }
    Some(b)
}

// ---------------------------------------------------------------- per-pixel colour effects

/// channel2::cc_composite's Composite Original modes.
fn composite_mode(m: u32) -> BlendMode {
    match m {
        2 => BlendMode::Add,
        3 => BlendMode::Multiply,
        4 => BlendMode::Screen,
        5 => BlendMode::Overlay,
        6 => BlendMode::SoftLight,
        7 => BlendMode::HardLight,
        8 => BlendMode::Darken,
        9 => BlendMode::Lighten,
        10 => BlendMode::Difference,
        11 => BlendMode::Hue,
        12 => BlendMode::Saturation,
        13 => BlendMode::Color,
        14 => BlendMode::Luminosity,
        15 => BlendMode::StencilAlpha,
        _ => BlendMode::Normal,
    }
}

fn point(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let mut p = Params::default();
    let mut original = None;
    match id {
        "ec.utility.cineon" => {
            // utility::Cineon's offset and knee, in the CPU's f32.
            let black = pr.f("tenBitBlackPoint") as f32;
            let white = pr.f("tenBitWhitePoint") as f32;
            let rolloff = pr.f("highlightRolloff") as f32;
            let offset = |black: f32, white: f32| 10f32.powf((black - white) * 0.002 / 0.6);
            let knee = 1.0 - (rolloff / 100.0).clamp(0.0, 0.99);
            p.u[0] = [0, pr.e("conversionType"), 0, 0];
            p.f[0] = [offset(black, white), knee, white, pr.f("gamma") as f32];
            p.f[1] = [pr.f("internalBlackPoint") as f32, pr.f("internalWhitePoint") as f32, 0.0, 0.0];
            p.f[2] = [offset(95.0, 685.0), knee, 685.0, 1.7];
        }
        "ec.utility.hdrcompander" => {
            p.u[0] = [1, (pr.e("mode") == 1) as u32, 0, 0];
            p.f[0] = [pr.f("gain").max(1e-6) as f32, pr.f("gamma").max(1e-3) as f32, 0.0, 0.0];
        }
        "ec.utility.hdrcompression" => {
            let t = (pr.f("amount") / 100.0).clamp(0.0, 1.0) as f32;
            if t <= 0.0 {
                return Some(b);
            }
            p.u[0][0] = 2;
            p.f[0][0] = 1.0 - 0.5 * t;
        }
        "ec.utility.ccoverbrights" => {
            p.u[0] = [3, pr.e("channel"), 0, 0];
            p.f[0] = pr.color("highlightColor");
        }
        "ec.channel.cccomposite" => {
            if let Some(source) = ctx.env.host.and_then(|h| h.original()) {
                let fitted = effectcraft_effects::util::fit_buffer(source, b.img.width, b.img.height, b.offset, b.scale);
                original = Some(e.g.upload_image(&fitted)?);
            }
            p.u[1][0] = original.is_some() as u32;
            let m = pr.e("compositeOriginal");
            let mode = if m == 1 { BlendMode::Normal } else { composite_mode(m) };
            p.u[0] = [6, (m == 1) as u32, mode_id(mode), pr.b("rgbOnly") as u32];
            p.f[0][0] = (pr.f("opacity") as f32 / 100.0).clamp(0.0, 1.0);
        }
        "ec.color.colorlink" => {
            // The sample statistics run on the CPU (the layer's own pixels are read back).
            let own = if ctx.layer_param("sourceLayer", false).is_none() { Some(e.download(&b.img)?) } else { None };
            let (c, sa) = effectcraft_effects::color_link_sample(ctx, own.as_ref())?;
            let op = (pr.f("opacity") / 100.0).clamp(0.0, 1.0) as f32;
            let stencil = pr.b("stencilOriginalAlpha") as u32;
            if pr.e("sampleSource") >= 6 {
                p.u[0] = [4, stencil, 0, 0];
                p.f[0] = [sa, op, 0.0, 0.0];
            } else {
                p.u[0] = [5, stencil, mode_id(effectcraft_effects::color_link_mode(ctx)), 0];
                p.f[0] = [c[0] * op, c[1] * op, c[2] * op, op];
            }
        }
        _ => return None,
    }
    run(e, "fp2_point", &p, b, original.as_ref(), None)
}

// ---------------------------------------------------------------- CC Block Load (stylize3)

fn block_load(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let c = (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0);
    if c >= 1.0 {
        return Some(b);
    }
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let nlev = (w.max(h).max(2) as f64).log2().ceil() as u32;
    let lp = c * (nlev + 1) as f64;
    let done = lp.floor() as u32;
    let frac = (lp - done as f64) as f32;
    let size = |k: u32| 1usize << (nlev - k.min(nlev));
    let means = |e: &mut Enc, s: usize| {
        let mut p = Params::default();
        p.u[0][0] = s as u32;
        let g = e.scratch(w.div_ceil(s).max(1) as u32, h.div_ceil(s).max(1) as u32);
        e.pixels("fp2_block_means", &p, &b.img, None, &g, None);
        g
    };
    let s_new = size(done);
    let new = means(e, s_new);
    let old = (done > 0).then(|| (size(done - 1), means(e, size(done - 1))));
    let flags = ctx.params.b("scanlines") as u32
        | (ctx.params.b("smoothing") as u32) << 1
        | (ctx.params.b("bilinear") as u32) << 2
        | (old.is_some() as u32) << 3
        | (ctx.params.b("startCleared") as u32) << 4;
    let mut p = Params::default();
    p.u[0] = [s_new as u32, old.as_ref().map_or(1, |o| o.0 as u32), flags, 0];
    p.f[0] = [frac, h.div_ceil(s_new) as f32, 0.0, 0.0];
    let out = e.scratch(w as u32, h as u32);
    e.pixels("fp2_block_load", &p, &new, old.as_ref().map(|o| &o.1), &out, None);
    b.img = out;
    Some(b)
}

// ---------------------------------------------------------------- CC Burn Film (stylize3)

fn burn_film(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let burn = (ctx.params.f("burn") / 100.0).clamp(0.0, 1.0) as f32;
    if burn <= 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let seed = ctx.params.f("randomSeed").max(0.0) as u32;
    let s = b.scale;
    let diag = (ctx.layer_size[0].hypot(ctx.layer_size[1]) * s).max(1.0);
    let feat = (60.0 * s).max(1e-3);
    let mut p = Params::default();
    p.u[0][0] = seed.wrapping_add(17);
    p.f[0] = [c.0 as f32, c.1 as f32, diag as f32, feat as f32];
    p.f[1] = [1.0 - burn * 1.25, 0.12 * (burn * 10.0).min(1.0), 0.0, 0.0];
    run(e, "fp2_burn", &p, b, None, None)
}

// ---------------------------------------------------------------- CC Glass / CC Plastic (stylize3)

fn bump(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let glass = id == "ec.stylize.ccglass";
    // stylize3::height_field.
    let src = crate::fx_key::fitted(e, ctx, &b, effectcraft_effects::bump_layer_id(ctx), true, false)?.unwrap_or_else(|| b.img.clone());
    let soft = ctx.params.f("softness") * b.scale * 0.5;
    let mut hf = plane(e, &src, 1, ctx.params.e("property"), soft);
    let height = ctx.params.f("height") as f32 / 100.0;
    let mut disp = 0.0;
    if glass {
        disp = (ctx.params.f("displacement") * b.scale * 0.25) as f32;
    } else {
        let lo = (ctx.params.f("cutMin") / 100.0) as f32;
        let hi = (ctx.params.f("cutMax") / 100.0) as f32;
        if lo > 0.0 || hi < 1.0 {
            hf = clamp_plane(e, &hf, lo.min(hi), hi.max(lo));
        }
    }
    let l = effectcraft_effects::BumpLight::from(ctx, &geo(&b));
    let mut p = Params::default();
    p.u[0] = [glass as u32, l.point as u32, 0, 0];
    p.f[0] = [height * 25.0, disp, (height != 0.0) as u32 as f32, l.height];
    p.f[1] = [l.intensity, l.ambient, l.diffuse, l.specular];
    p.f[2] = [l.shininess, l.metal, l.pos.0 as f32, l.pos.1 as f32];
    p.f[3] = [l.color[0], l.color[1], l.color[2], 0.0];
    p.f[4] = [l.dir[0], l.dir[1], l.dir[2], 0.0];
    run(e, "fp2_bump", &p, b, Some(&hf), None)
}

/// An empty CPU buffer with `b`'s geometry (for CPU helpers that only read positions).
fn geo(b: &GBuf) -> effectcraft_effects::Buf {
    effectcraft_effects::Buf { img: Image { width: b.img.width, height: b.img.height, data: vec![] }, offset: b.offset, scale: b.scale }
}

/// Clamp a plane (x) into [lo, hi] (Plane::map with f32::clamp).
fn clamp_plane(e: &mut Enc, pl: &GpuImage, lo: f32, hi: f32) -> GpuImage {
    let mut p = Params::default();
    p.u[0] = [3, 0, 0, 0];
    p.f[0] = [lo, hi, 0.0, 0.0];
    let out = e.scratch(pl.width, pl.height);
    e.pixels("fp2_plane", &p, pl, None, &out, None);
    out
}

// ---------------------------------------------------------------- CC HexTile / CC Mr. Smoothie (stylize3)

fn hextile(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let r = (ctx.params.f("radius") * b.scale).max(0.5);
    let c = b.to_px(ctx.params.v2("center"));
    let (sn, cs) = ctx.params.f("rotate").to_radians().sin_cos();
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("render");
    p.f[0] = [c.0 as f32, c.1 as f32, r as f32, (ctx.params.f("smearing") / 100.0).clamp(0.0, 1.0) as f32];
    p.f[1] = [sn as f32, cs as f32, 0.0, 0.0];
    run(e, "fp2_hextile", &p, b, None, None)
}

fn smoothie(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let flow = crate::fx_key::fitted(e, ctx, &b, "flowLayer", true, false)?.unwrap_or_else(|| b.img.clone());
    let sm = ctx.params.f("smoothness") * b.scale * 0.5;
    let pl = plane(e, &flow, 2, ctx.params.e("property"), sm);
    let a = b.to_px(ctx.params.v2("sampleA"));
    let bb = b.to_px(ctx.params.v2("sampleB"));
    let mut p = Params::default();
    p.f[0] = [a.0 as f32, a.1 as f32, bb.0 as f32, bb.1 as f32];
    p.f[1] = [(ctx.params.f("phase") / 360.0) as f32, (ctx.params.e("colorLoop") + 1) as f32, 0.0, 0.0];
    run(e, "fp2_smoothie", &p, b, Some(&pl), None)
}

// ---------------------------------------------------------------- Inner/Outer Key (keying2)

fn inner_outer(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let Some(plan) = effectcraft_effects::inner_outer_plan(ctx, &geo(&b)) else { return Some(b) };
    let (w, h) = (b.img.width, b.img.height);
    let mut tri = Image::new(w, h);
    for (px, &t) in tri.data.iter_mut().zip(&plan.tri.data) {
        px[0] = t;
    }
    let tri = e.g.upload_image(&tri)?;
    // The matte (x, or w of the estimate) and the estimate's colours.
    let (mut matte, est) = if plan.unknown {
        let t = e.scratch(w, h);
        e.pixels("fp2_io_prep", &Params::default(), &b.img, Some(&tri), &t, None);
        let known = |e: &mut Enc, side: u32| {
            let mut p = Params::default();
            p.u[0][0] = side;
            let k = e.scratch(w, h);
            e.pixels("fp2_io_known", &p, &t, None, &k, None);
            gauss_plane(e, &k, plan.sigma, plan.sigma)
        };
        let fg = known(e, 0);
        let bg = known(e, 1);
        let (rows, stride) = e.image_rows(&t);
        let mut p = Params::default();
        p.u[0][0] = stride;
        let est = e.scratch(w, h);
        e.pixels("fp2_io_est", &p, &fg, Some(&bg), &est, Some(&rows));
        (est.clone(), Some(est))
    } else {
        (tri, None)
    };
    let from_w = est.is_some() as u32;
    if !plan.strokes.is_empty() || from_w != 0 {
        let n = plan.strokes.len();
        let mut data = Vec::with_capacity(n + n * (w * h) as usize);
        data.extend(plan.strokes.iter().map(|s| s.0));
        for (_, cov) in &plan.strokes {
            data.extend_from_slice(cov);
        }
        let buf = e.data(&data);
        let mut p = Params::default();
        p.u[0] = [n as u32, from_w, 0, 0];
        let out = e.scratch(w, h);
        e.pixels("fp2_io_strokes", &p, &matte, None, &out, Some(&buf));
        matte = out;
    }
    if plan.thin.abs() > 0.01 {
        let modes = if plan.thin < 0.0 { crate::fx_color::DILATE_X } else { crate::fx_color::ERODE_X };
        matte = crate::fx_color::morph_frac(e, &matte, plan.thin.abs(), modes);
    }
    if plan.feather > 0.05 {
        matte = gauss_plane(e, &matte, plan.feather * 0.5, plan.feather * 0.5);
    }
    let mut p = Params::default();
    p.u[0] = [0, from_w, plan.invert as u32, 0];
    p.f[0] = [plan.threshold, plan.original, 0.0, 0.0];
    match est {
        Some(est) => {
            let (rows, stride) = e.image_rows(&est);
            p.u[0][0] = stride;
            run(e, "fp2_io_final", &p, b, Some(&matte), Some(&rows))
        }
        None => run(e, "fp2_io_final", &p, b, Some(&matte), None),
    }
}

// ---------------------------------------------------------------- CC Simple Wire Removal (keying2)

fn wire_removal(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let half = (ctx.params.f("thickness") * b.scale * 0.5).max(0.0);
    if half <= 0.0 {
        return Some(b);
    }
    // Frame Offset reads another frame from the effect host: that renders on the CPU.
    if ctx.params.e("removalStyle") == 1 && ctx.params.f("frameOffset").round() != 0.0 && ctx.env.host.is_some() {
        return None;
    }
    let slope = (ctx.params.f("slope") / 100.0).clamp(0.0, 1.0) as f32;
    let mirror = (ctx.params.f("mirrorBlend") / 100.0).clamp(0.0, 1.0) as f32;
    let a = b.to_px(ctx.params.v2("pointA"));
    let z = b.to_px(ctx.params.v2("pointB"));
    let (dx, dy) = (z.0 - a.0, z.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return Some(b);
    }
    let (tx, ty) = (dx / len, dy / len);
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("removalStyle");
    p.f[0] = [a.0 as f32, a.1 as f32, -ty as f32, tx as f32];
    p.f[1] = [half as f32, (half * slope as f64) as f32, mirror, len as f32];
    run(e, "fp2_wire", &p, b, None, None)
}

// ---------------------------------------------------------------- Basic 3D (obsolete)

fn basic_3d(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let swivel = ctx.params.f("swivel").to_radians();
    let tilt = ctx.params.f("tilt").to_radians();
    let dist = ctx.params.f("distanceToImage") * b.scale;
    let spec_on = ctx.params.b("specularHighlight");
    let wire = ctx.params.b("preview/drawPreviewWireframe");
    if swivel == 0.0 && tilt == 0.0 && dist == 0.0 && !spec_on && !wire {
        return Some(b);
    }
    let (lx, ly, lw, lh) = (b.offset[0], b.offset[1], ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let (cx, cy) = (lx + lw * 0.5, ly + lh * 0.5);
    let f = lw.max(lh).max(1.0) * 1.4;
    let (cs, ss) = (swivel.cos(), swivel.sin());
    let (ct, st) = (tilt.cos(), tilt.sin());
    let v3 = |v: [f64; 3]| [v[0] as f32, v[1] as f32, v[2] as f32, 0.0];
    let l = [-0.5f64, -0.6, -1.0];
    let k = 1.0 / (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    let mut p = Params::default();
    p.u[0] = [spec_on as u32, wire as u32, 0, 0];
    p.f[0] = [cx as f32, cy as f32, f as f32, dist as f32];
    p.f[1] = v3([cs, 0.0, -ss]);
    p.f[2] = v3([ss * st, ct, cs * st]);
    p.f[3] = v3([ss * ct, -st, cs * ct]);
    p.f[4] = v3(l.map(|v| v * k));
    p.f[5] = [(lw * 0.5) as f32, (lh * 0.5) as f32, 0.0, 0.0];
    run(e, "fp2_basic3d", &p, b, None, None)
}
