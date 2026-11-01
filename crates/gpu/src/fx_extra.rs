//! GPU effects, shapes and bevels (kernels in `shaders/fx_extra.wgsl`, every entry point
//! prefixed `fxe_`): Circle, Ellipse, Iris Wipe, Bevel Alpha, Bevel Edges and Gaussian Blur
//! (Legacy), with the CPU effects' exact steps. Radii, feathers, light directions and Iris
//! Wipe's polygon are computed on the CPU in f64 as the CPU effect does; the kernels work
//! relative to the shape's centre.

use effectcraft_effects::EffectCtx;

use crate::context::{Enc, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_extra.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxe_circle", "fxe_ellipse", "fxe_iris", "fxe_alpha", "fxe_bevel_alpha", "fxe_bevel_edges"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.generate.circle",
    "ec.generate.ellipse",
    "ec.transition.iriswipe",
    "ec.perspective.bevelalpha",
    "ec.perspective.beveledges",
    "ec.obsolete.gaussianlegacy",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.generate.circle" => circle(e, ctx, b),
        "ec.generate.ellipse" => ellipse(e, ctx, b),
        "ec.transition.iriswipe" => iris_wipe(e, ctx, b),
        "ec.perspective.bevelalpha" => bevel_alpha(e, ctx, b),
        "ec.perspective.beveledges" => bevel_edges(e, ctx, b),
        "ec.obsolete.gaussianlegacy" => gaussian_legacy(e, ctx, b),
        _ => None,
    }
}

/// A same-size per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, aux: Option<&crate::context::GpuImage>, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, aux, &out, data);
    b.img = out;
    Some(b)
}

fn f4(v: [f64; 4]) -> [f32; 4] {
    v.map(|x| x as f32)
}

// ---------------------------------------------------------------- Circle (generate::circle)

fn circle(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let c = b.to_px(pr.v2("center"));
    let r = pr.f("radius") * b.scale;
    let thick = pr.f("thickness").max(0.0);
    let mut f_out = pr.f("feather/feather").max(0.0) * b.scale;
    let mut f_in = pr.f("feather/featherInner").max(0.0) * b.scale;
    let inner = match pr.e("edge") {
        1 => Some(pr.f("edgeRadius") * b.scale),
        2 => Some(r - thick * b.scale),
        3 => Some(r - thick / 100.0 * r),
        4 => {
            f_out = pr.f("feather/feather").max(0.0) / 100.0 * r;
            f_in = pr.f("feather/featherInner").max(0.0) / 100.0 * r;
            Some(r - thick / 100.0 * r)
        }
        _ => None,
    };
    let (ro, ri) = match inner {
        Some(i) => (r.max(i), Some(r.min(i).max(0.0))),
        None => (r, None),
    };
    let (f_out, f_in) = (f_out.max(0.5), f_in.max(0.5));
    let color = pr.color("color");
    let mut p = Params::default();
    p.u[0] = [crate::fx_generate::gen_mode(pr.e("blendingMode")), ri.is_some() as u32, pr.b("invert") as u32, 0];
    p.f[0] = f4([c.0, c.1, ro, ri.unwrap_or(0.0)]);
    p.f[1] = [f_out as f32, f_in as f32, pr.f("opacity") as f32 / 100.0, 0.0];
    p.f[2] = [color[0], color[1], color[2], 0.0];
    run(e, "fxe_circle", &p, b, None, None)
}

// ---------------------------------------------------------------- Ellipse (generate2::ellipse)

fn ellipse(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let c = b.to_px(pr.v2("center"));
    let rx = (pr.f("width") * b.scale * 0.5).max(0.5);
    let ry = (pr.f("height") * b.scale * 0.5).max(0.5);
    let half = (pr.f("thickness") * b.scale * 0.5).max(0.25);
    let soft = (pr.f("softness") / 100.0).clamp(0.0, 1.0);
    let (inside, outside) = (pr.color("insideColor"), pr.color("outsideColor"));
    let mut p = Params::default();
    p.u[0][1] = pr.b("compositeOnOriginal") as u32;
    p.f[0] = f4([c.0, c.1, rx, ry]);
    p.f[1] = [half as f32, soft as f32, 0.0, 0.0];
    p.f[2] = [inside[0], inside[1], inside[2], 0.0];
    p.f[3] = [outside[0], outside[1], outside[2], 0.0];
    run(e, "fxe_ellipse", &p, b, None, None)
}

// ---------------------------------------------------------------- Iris Wipe (transition::iris_wipe)

fn iris_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let outer = pr.f("outerRadius").max(0.0) * b.scale;
    if outer <= 0.0 {
        return Some(b);
    }
    let n = pr.f("points").round().clamp(3.0, 64.0) as usize;
    let star = pr.b("useInnerRadius");
    let inner = if star { pr.f("innerRadius").max(0.0) * b.scale } else { outer };
    let rot = pr.f("rotation").to_radians();
    let c = b.to_px(pr.v2("center"));
    let m = if star { n * 2 } else { n };
    let step = std::f64::consts::TAU / m as f64;
    let verts: Vec<f32> = (0..m)
        .flat_map(|i| {
            let r = if star && i % 2 == 1 { inner } else { outer };
            let a = rot + i as f64 * step;
            [(r * a.sin()) as f32, (-r * a.cos()) as f32]
        })
        .collect();
    let buf = e.data(&verts);
    let mut p = Params::default();
    p.u[0][1] = m as u32;
    p.f[0] = f4([c.0, c.1, rot, step]);
    p.f[1] = f4([pr.f("feather") * b.scale, outer.min(inner), outer, 0.0]);
    run(e, "fxe_iris", &p, b, None, Some(&buf))
}

// ---------------------------------------------------------------- Bevels (perspective.rs)

/// perspective::light_dir.
fn light_dir(angle_deg: f64, elevation_deg: f64) -> [f64; 3] {
    let (a, el) = (angle_deg.to_radians(), elevation_deg.to_radians());
    let v = [a.sin() * el.cos(), -a.cos() * el.cos(), el.sin()];
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    v.map(|x| x / l)
}

fn bevel_alpha(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let t = pr.f("edgeThickness") * b.scale;
    let inten = pr.f("lightIntensity") as f32;
    if t <= 0.0 || inten == 0.0 {
        return Some(b);
    }
    let light = pr.color("lightColor");
    let l = light_dir(pr.f("lightAngle"), 45.0);
    // The alpha plane's Gaussian (util::gauss_plane: box passes, edges repeated); kernels read x.
    let alpha = e.scratch(b.img.width, b.img.height);
    e.pixels("fxe_alpha", &Params::default(), &b.img, None, &alpha, None);
    let h = gaussian_blur(e, &alpha, t * 0.5, t * 0.5, true);
    let mut p = Params::default();
    p.f[0] = f4([l[0], l[1], l[2], t * 1.25]);
    p.f[1] = [light[0], light[1], light[2], inten];
    run(e, "fxe_bevel_alpha", &p, b, Some(&h), None)
}

fn bevel_edges(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let (x0, y0, w, h) = (b.offset[0], b.offset[1], ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let t = pr.f("edgeThickness").clamp(0.0, 0.5) * w.min(h);
    let inten = pr.f("lightIntensity") as f32;
    if t <= 0.0 || inten == 0.0 {
        return Some(b);
    }
    let light = pr.color("lightColor");
    let l = light_dir(pr.f("lightAngle"), 45.0);
    let mut p = Params::default();
    p.f[0] = f4([l[0], l[1], l[2], t]);
    p.f[1] = [light[0], light[1], light[2], inten];
    p.f[2] = f4([x0, y0, w, h]);
    run(e, "fxe_bevel_edges", &p, b, None, None)
}

// ---------------------------------------------------------------- Gaussian Blur (Legacy) (obsolete.rs)

fn gaussian_legacy(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let s = ctx.params.f("blurriness").max(0.0) * 0.5 * b.scale;
    if s <= 0.0 {
        return Some(b);
    }
    let (kx, ky) = match ctx.params.e("blurDimensions") {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    };
    // Small sigmas use the CPU's sampled kernel (#1939).
    if effectcraft_raster::gaussian_is_exact(s * kx, s * ky) {
        return None;
    }
    if !ctx.adjustment {
        b.pad(e, (s * 3.0).ceil() as u32)?;
    }
    b.img = gaussian_blur(e, &b.img, s * kx, s * ky, ctx.adjustment);
    Some(b)
}
