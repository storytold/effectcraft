//! GPU effects, warp family (kernels in `shaders/fx_warp.wgsl`, every entry point prefixed
//! `fxw_`): Warp, Bezier Warp, CC Bend It, CC Page Turn, Smear, Reshape, Color Emboss and
//! Cartoon, with the CPU effects' exact steps.
//!
//! Scalar set-up runs on the CPU in f64 exactly as the CPU effect does it (Bezier Warp's grid,
//! Smear's moved outline, Reshape's outline correspondence, Bend It's arc geometry) and goes to
//! the kernels as parameters or tables; the kernels do the per-pixel work in f32. Warp inverts
//! its forward map per pixel with the CPU's Newton iterations, relative to the warp centre so
//! f32 keeps sub-pixel precision (strong Fisheye / Twist bends, where the iterations are chaotic
//! near the fold, sample the CPU's f64 inverse map, `effects::warp_inverse_map`); Bend It's arc
//! distance is evaluated in a cancellation-free form so large radii (small bends) keep their
//! precision too.

use effectcraft_effects::{Buf, EffectCtx, Image};

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_warp.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fxw_warp",
    "fxw_remap",
    "fxw_bendit",
    "fxw_pageturn",
    "fxw_smear",
    "fxw_reshape",
    "fxw_coloremboss",
    "fxw_luma",
    "fxw_keep_alpha",
    "fxw_shock",
    "fxw_edges",
    "fxw_cartoon",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.distort.warp",
    "ec.distort.bezierwarp",
    "ec.distort.ccbendit",
    "ec.distort.ccpageturn",
    "ec.distort.smear",
    "ec.distort.reshape",
    "ec.stylize.coloremboss",
    "ec.stylize.cartoon",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.distort.warp" => warp(e, ctx, b),
        "ec.distort.bezierwarp" => bezier_warp(e, ctx, b),
        "ec.distort.ccbendit" => bend_it(e, ctx, b),
        "ec.distort.ccpageturn" => page_turn(e, ctx, b),
        "ec.distort.smear" => smear(e, ctx, b),
        "ec.distort.reshape" => reshape(e, ctx, b),
        "ec.stylize.coloremboss" => color_emboss(e, ctx, b),
        "ec.stylize.cartoon" => cartoon(e, ctx, b),
        _ => None,
    }
}

/// A same-size per-pixel kernel.
fn run(e: &mut Enc, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels(entry, p, src, aux, &out, data);
    out
}

fn f4(v: [f64; 4]) -> [f32; 4] {
    v.map(|x| x as f32)
}

/// The buffer's geometry without its pixels (the CPU set-up functions read only that).
fn geometry(b: &GBuf) -> Buf {
    Buf { img: Image::new(0, 0), offset: b.offset, scale: b.scale }
}

/// Points as f32 pairs.
fn points(v: &mut Vec<f32>, pts: &[[f64; 2]]) {
    v.extend(pts.iter().flat_map(|q| [q[0] as f32, q[1] as f32]));
}

// ---------------------------------------------------------------- Warp (distort3::warp)

fn warp(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let k = ctx.params.f("bend") / 100.0;
    let hd = ctx.params.f("horizontalDistortion") / 100.0;
    let vd = ctx.params.f("verticalDistortion") / 100.0;
    if k == 0.0 && hd == 0.0 && vd == 0.0 {
        return Some(b);
    }
    let style = ctx.params.e("warpStyle");
    let vertical = ctx.params.e("warpAxis") == 1;
    let (hx, hy) = (ctx.layer_size[0] * b.scale * 0.5, ctx.layer_size[1] * b.scale * 0.5);
    if !ctx.adjustment {
        b.pad(e, effectcraft_effects::warp_pad(ctx, b.scale))?;
    }
    // Fisheye / Twist (a crease at the unit circle) bent past 50 % or with Horizontal /
    // Vertical Distortion: near the crease the f32 and f64 solvers can converge on different
    // pixels. The CPU effect's solver supplies the inverse map for the GPU sampler.
    if matches!(style, 11 | 14) && (k.abs() > 0.5 || hd != 0.0 || vd != 0.0) {
        let geo = Buf { img: Image::new(0, 0), offset: b.offset, scale: b.scale };
        let map = e.g.upload_image(&effectcraft_effects::warp_inverse_map(ctx, &geo, b.img.width, b.img.height))?;
        b.img = run(e, "fxw_remap", &Params::default(), &b.img, Some(&map), None);
        return Some(b);
    }
    let c = b.to_px([ctx.layer_size[0] * 0.5, ctx.layer_size[1] * 0.5]);
    let mut p = Params::default();
    p.u[0] = [0, style.min(14), vertical as u32, 0];
    p.f[0] = f4([c.0, c.1, hx.max(1e-6), hy.max(1e-6)]);
    p.f[1] = f4([k, hd, vd, 0.0]);
    b.img = run(e, "fxw_warp", &p, &b.img, None, None);
    Some(b)
}

// ---------------------------------------------------------------- Bezier Warp (distort2::bezier_warp)

fn bezier_warp(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let (n, dest, srcp) = effectcraft_effects::bezier_grid(ctx, b.offset, b.scale);
    crate::fx_distort::grid_warp(e, b, n, n, &dest, &srcp)
}

// ---------------------------------------------------------------- CC Bend It (distort2::bend_it)

fn bend_it(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let mut beta = ctx.params.f("bend").to_radians();
    if beta.abs() < 1e-6 {
        return Some(b);
    }
    let s = b.to_px(ctx.params.v2("start"));
    let end = b.to_px(ctx.params.v2("end"));
    let static_pre = ctx.params.e("renderPrestart") == 1;
    let l = ((end.0 - s.0).powi(2) + (end.1 - s.1).powi(2)).sqrt();
    if l < 1e-6 {
        return Some(b);
    }
    let a = ((end.0 - s.0) / l, (end.1 - s.1) / l);
    let mut n = (-a.1, a.0);
    if beta < 0.0 {
        beta = -beta;
        n = (-n.0, -n.1);
    }
    let rho = l / beta;
    let pe = (rho * beta.sin(), rho - rho * beta.cos());
    let mut p = Params::default();
    p.u[0][1] = static_pre as u32;
    p.f[0] = f4([s.0, s.1, a.0, a.1]);
    p.f[1] = f4([n.0, n.1, rho, beta]);
    p.f[2] = f4([pe.0, pe.1, l, 0.0]);
    p.f[3] = f4([beta.cos(), beta.sin(), -beta.sin(), beta.cos()]);
    b.img = run(e, "fxw_bendit", &p, &b.img, None, None);
    Some(b)
}

// ---------------------------------------------------------------- CC Page Turn (distort2::page_turn)

fn page_turn(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let f = b.to_px(ctx.params.v2("foldPosition"));
    let phi = ctx.params.f("foldDirection").to_radians();
    let r = (ctx.params.f("foldRadius") * b.scale).max(0.5);
    let lam = ctx.params.f("lightDirection").to_radians();
    let render = ctx.params.e("render");
    let bo = ctx.params.f("backPageOpacity") / 100.0;
    let paper = ctx.params.color("paperColor");
    let n = (-phi.sin(), phi.cos());
    let ldot = n.0 * lam.sin() - n.1 * lam.cos();
    let mut p = Params::default();
    p.u[0] = [0, (render != 1) as u32, (render != 2) as u32, 0];
    p.f[0] = f4([f.0, f.1, n.0, n.1]);
    p.f[1] = [r as f32, ldot as f32, bo as f32, 0.0];
    p.f[2] = [paper[0], paper[1], paper[2], 0.0];
    b.img = run(e, "fxw_pageturn", &p, &b.img, None, None);
    Some(b)
}

// ---------------------------------------------------------------- Smear (distort3::smear)

fn smear(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let Some(s) = effectcraft_effects::smear_setup(ctx, &geometry(&b)) else { return Some(b) };
    let mut data = Vec::with_capacity(2 * (s.bound.len() + s.moved.len()));
    points(&mut data, &s.bound);
    points(&mut data, &s.moved);
    let buf = e.data(&data);
    let mut p = Params::default();
    p.u[0] = [0, s.bound.len() as u32, s.moved.len() as u32, 0];
    p.f[0] = f4([s.c[0], s.c[1], s.cs, s.sn]);
    p.f[1] = f4([s.scl, s.off[0], s.off[1], s.pct]);
    p.f[2][0] = (1.0 / s.kexp) as f32;
    b.img = run(e, "fxw_smear", &p, &b.img, None, Some(&buf));
    Some(b)
}

// ---------------------------------------------------------------- Reshape (distort3::reshape)

fn reshape(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let Some(s) = effectcraft_effects::reshape_setup(ctx, &geometry(&b)) else { return Some(b) };
    debug_assert_eq!(s.dest.len(), effectcraft_effects::RESHAPE_POINTS);
    let mut data = Vec::new();
    points(&mut data, &s.dest);
    points(&mut data, &s.disp);
    let nb = s.bound.as_ref().map_or(0, Vec::len);
    if let Some(bd) = &s.bound {
        points(&mut data, bd);
    }
    let buf = e.data(&data);
    let mut p = Params::default();
    p.u[0] = [0, nb as u32, s.smooth as u32, 0];
    p.f[0] = [(s.kexp * 0.5) as f32, s.pct as f32, 0.0, 0.0];
    b.img = run(e, "fxw_reshape", &p, &b.img, None, Some(&buf));
    Some(b)
}

// ---------------------------------------------------------------- Color Emboss (stylize2::color_emboss)

fn color_emboss(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("direction").to_radians();
    let relief = ctx.params.f("relief") * b.scale;
    let mut p = Params::default();
    p.f[0] = [(ang.cos() * relief) as f32, (-ang.sin() * relief) as f32, ctx.params.f("contrast") as f32 / 100.0, ctx.params.f("blend") as f32 / 100.0];
    b.img = run(e, "fxw_coloremboss", &p, &b.img, None, None);
    Some(b)
}

// ---------------------------------------------------------------- Cartoon (stylize2::cartoon)

fn cartoon(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let render = pr.e("render");
    let r = (pr.f("detailRadius") * b.scale).round().max(0.0) as usize;
    let thr = pr.f("detailThreshold") as f32 / 100.0;
    let steps = pr.f("fill/shadingSteps").max(1.0) as f32;
    let smooth = pr.f("fill/shadingSmoothness") as f32 / 100.0;
    let et = pr.f("edge/edgeThreshold") as f32;
    let ew = pr.f("edge/edgeWidth") * b.scale;
    let es = pr.f("edge/edgeSoftness") as f32 / 100.0;
    let eo = pr.f("edge/edgeOpacity") as f32 / 100.0;
    let black = (pr.f("advanced/edgeBlackLevel") as f32 / 100.0).clamp(0.0, 1.0);
    let contrast = ((pr.f("advanced/edgeContrast").clamp(0.0, 0.999) * std::f64::consts::FRAC_PI_2).tan() as f32).min(100.0);
    let none = Params::default();
    // Edge-preserving smoothing: guided filter on each premultiplied colour channel, guided by
    // the luma; alpha stays.
    let mut smoothed = if r > 0 {
        let guide = run(e, "fxw_luma", &none, &b.img, None, None);
        let eps = thr * thr * 0.25 + 1e-5;
        let f = crate::fx_noise::guided_filter(e, &guide, &b.img, r, eps);
        run(e, "fxw_keep_alpha", &none, &b.img, Some(&f), None)
    } else {
        b.img.clone()
    };
    let enh = pr.f("advanced/edgeEnhancement") / 100.0;
    if enh < 0.0 {
        let s = 1.5 * b.scale;
        let blur = gaussian_blur(e, &smoothed, s, s, true);
        let mut p = Params::default();
        p.f[0][0] = (-enh) as f32;
        smoothed = run(e, "fxc_lerp", &p, &smoothed, Some(&blur), None);
    } else if enh > 0.0 {
        let l = run(e, "fxw_luma", &none, &smoothed, None, None);
        let mut p = Params::default();
        p.f[0][0] = (enh * 2.0 * b.scale) as f32;
        smoothed = run(e, "fxw_shock", &p, &smoothed, Some(&l), None);
    }
    let mut p = Params::default();
    let mut edge_k = eo;
    let rows = if render >= 1 {
        let luma = run(e, "fxw_luma", &none, &smoothed, None, None);
        let t1 = et * 0.2;
        let t0 = t1 * (1.0 - es * 0.9);
        let mut ep = Params::default();
        ep.f[0] = [t0, t1.max(t0 + 1e-4), 0.0, 0.0];
        let mut edges = run(e, "fxw_edges", &ep, &luma, None, None);
        if ew > 1.0 {
            edges = crate::fx_color::morph_frac(e, &edges, (ew - 1.0) * 0.5, crate::fx_color::DILATE_X);
        } else {
            edge_k *= ew.max(0.0) as f32;
        }
        Some(e.image_rows(&edges))
    } else {
        None
    };
    p.u[0] = [0, render, rows.as_ref().map_or(0, |r| r.1), rows.is_some() as u32];
    p.f[0] = [steps, smooth, edge_k, black];
    p.f[1][0] = contrast;
    b.img = run(e, "fxw_cartoon", &p, &b.img, Some(&smoothed), rows.as_ref().map(|r| &r.0));
    Some(b)
}
