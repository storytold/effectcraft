//! GPU effects, stylize and distort family (kernels in `shaders/fx_stylize.wgsl`, every entry point prefixed
//! `fxs_`): the CPU effects' exact steps with the pixel loops as compute kernels.
//!
//! Parameter conversions, padding, output geometry and anything the CPU computes in f64 with a
//! floor or a wrap (tile indices, wrapped coordinates, brush cells) are computed here exactly as
//! the CPU does and uploaded as per-column / per-row / per-cell tables; the kernels do the
//! per-pixel work. Glow's other Glow Operations / Arbitrary Map and Transform's shutter-angle
//! motion blur extend the base kernels in `effects.rs` (see [`glow_extra`], [`transform_blur`]).

use std::f64::consts::{FRAC_PI_2, TAU};

use effectcraft_color::BlendMode;
use effectcraft_effects::EffectCtx;
use effectcraft_geom::{Mat3, vec2};
use effectcraft_raster::Sampling;

use crate::context::{Enc, Params};
use crate::effects::{GBuf, gaussian_blur};
use crate::ops;

/// Compute entry points in `fx_stylize.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fxs_point",
    "fxs_warp",
    "fxs_table",
    "fxs_griddler",
    "fxs_motiontile",
    "fxs_repetile",
    "fxs_magnify",
    "fxs_scatter",
    "fxs_brush",
    "fxs_roughen",
    "fxs_tex_luma",
    "fxs_texturize",
    "fxs_glow_map",
    "fxs_glow_op",
    "fxs_add",
    "fxs_div",
    "fxs_flomotion",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.stylize.posterize",
    "ec.stylize.threshold",
    "ec.stylize.ccthreshold",
    "ec.stylize.ccthresholdrgb",
    "ec.stylize.strobe",
    "ec.stylize.ccvignette",
    "ec.stylize.scatter",
    "ec.stylize.brushstrokes",
    "ec.stylize.roughenedges",
    "ec.stylize.texturize",
    "ec.stylize.motiontile",
    "ec.stylize.cckaleida",
    "ec.stylize.ccrepetile",
    "ec.distort.mirror",
    "ec.distort.offset",
    "ec.distort.polar",
    "ec.distort.spherize",
    "ec.distort.cornerpin",
    "ec.distort.opticscompensation",
    "ec.distort.magnify",
    "ec.distort.ccslant",
    "ec.distort.ccsmear",
    "ec.distort.ccsplit",
    "ec.distort.ccsplit2",
    "ec.distort.cctiler",
    "ec.distort.ccgriddler",
    "ec.distort.liquify",
    "ec.distort.twirllegacy",
    "ec.distort.ccripplepulse",
    "ec.distort.ccpowerpin",
    "ec.distort.ccflomotion",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.stylize.posterize" => posterize(e, ctx, b),
        "ec.stylize.threshold" => threshold(e, ctx, b),
        "ec.stylize.ccthreshold" => cc_threshold(e, ctx, b),
        "ec.stylize.ccthresholdrgb" => cc_threshold_rgb(e, ctx, b),
        "ec.stylize.strobe" => strobe(e, ctx, b),
        "ec.stylize.ccvignette" => vignette(e, ctx, b),
        "ec.stylize.scatter" => scatter(e, ctx, b),
        "ec.stylize.brushstrokes" => brush_strokes(e, ctx, b),
        "ec.stylize.roughenedges" => roughen_edges(e, ctx, b),
        "ec.stylize.texturize" => texturize(e, ctx, b),
        "ec.stylize.motiontile" => motion_tile(e, ctx, b),
        "ec.stylize.cckaleida" => kaleida(e, ctx, b),
        "ec.stylize.ccrepetile" => repetile(e, ctx, b),
        "ec.distort.mirror" => mirror(e, ctx, b),
        "ec.distort.offset" => offset(e, ctx, b),
        "ec.distort.polar" => polar(e, ctx, b),
        "ec.distort.spherize" => spherize(e, ctx, b),
        "ec.distort.cornerpin" => corner_pin(e, ctx, b),
        "ec.distort.opticscompensation" => optics_compensation(e, ctx, b),
        "ec.distort.magnify" => magnify(e, ctx, b),
        "ec.distort.ccslant" => cc_slant(e, ctx, b),
        "ec.distort.ccsmear" => cc_smear(e, ctx, b),
        "ec.distort.ccsplit" => {
            let s = ctx.params.f("split") / 100.0;
            cc_split(e, ctx, b, s, s)
        }
        "ec.distort.ccsplit2" => cc_split(e, ctx, b, ctx.params.f("split1") / 100.0, ctx.params.f("split2") / 100.0),
        "ec.distort.cctiler" => cc_tiler(e, ctx, b),
        "ec.distort.ccgriddler" => cc_griddler(e, ctx, b),
        "ec.distort.liquify" => liquify(e, ctx, b),
        "ec.distort.twirllegacy" => twirl_legacy(e, ctx, b),
        "ec.distort.ccripplepulse" => ripple_pulse(e, ctx, b),
        "ec.distort.ccpowerpin" => power_pin(e, ctx, b),
        "ec.distort.ccflomotion" => flo_motion(e, ctx, b),
        _ => None,
    }
}

/// Run a same-size per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, None, &out, data);
    b.img = out;
    Some(b)
}

/// util::layer_rect.
fn layer_rect(ctx: &EffectCtx, b: &GBuf) -> (f64, f64, f64, f64) {
    effectcraft_effects::util::layer_rect_at(ctx, b.offset, b.scale)
}

// ---------------------------------------------------------------- per-pixel (fxs_point)

fn point(e: &mut Enc, mode: u32, f: impl FnOnce(&mut Params), b: GBuf) -> Option<GBuf> {
    let mut p = Params::default();
    p.u[0][0] = mode;
    f(&mut p);
    run(e, "fxs_point", &p, b, None)
}

fn posterize(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let lv = ctx.params.f("level").round().clamp(2.0, 255.0) as f32 - 1.0;
    point(e, 1, |p| p.f[0][0] = lv, b)
}

fn threshold(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let t = ctx.params.f("level") as f32 / 255.0;
    point(e, 2, |p| p.f[0][0] = t, b)
}

fn cc_threshold(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let t = ctx.params.f("threshold") as f32 / 255.0;
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let (ch, inv) = (ctx.params.e("channel"), ctx.params.b("invert") as u32);
    point(
        e,
        3,
        |p| {
            p.u[0][1] = ch;
            p.u[0][2] = inv;
            p.f[0] = [t, blend, 0.0, 0.0];
        },
        b,
    )
}

fn cc_threshold_rgb(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let t = [ctx.params.f("redThreshold"), ctx.params.f("greenThreshold"), ctx.params.f("blueThreshold")].map(|v| v as f32 / 255.0);
    let inv = [ctx.params.b("invertRed"), ctx.params.b("invertGreen"), ctx.params.b("invertBlue")].map(u32::from);
    let blend = ctx.params.f("blend") as f32 / 100.0;
    point(
        e,
        4,
        |p| {
            p.u[0] = [4, inv[0], inv[1], inv[2]];
            p.f[0] = [t[0], t[1], t[2], blend];
        },
        b,
    )
}

fn strobe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let on = effectcraft_effects::strobe_on(
        ctx.time,
        ctx.params.f("strobeDuration"),
        ctx.params.f("strobePeriod"),
        ctx.params.f("randomStrobeProbability") / 100.0,
        ctx.seed,
    );
    if !on {
        return Some(b);
    }
    let blend = (ctx.params.f("blendWithOriginal") as f32 / 100.0).clamp(0.0, 1.0);
    if ctx.params.e("strobe") == 1 {
        // Image::scale_alpha.
        if (blend - 1.0).abs() < 1e-7 {
            return Some(b);
        }
        return point(e, 6, |p| p.f[0][0] = blend, b);
    }
    let s = ctx.params.color("strobeColor");
    let op = ctx.params.e("strobeOperator");
    point(
        e,
        5,
        |p| {
            p.u[0][1] = op;
            p.f[0] = [s[0], s[1], s[2], blend];
        },
        b,
    )
}

fn vignette(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amt = ctx.params.f("amount") as f32 / 100.0;
    let fov = ctx.params.f("angleOfView").clamp(0.0, 179.0).to_radians();
    if amt == 0.0 || fov <= 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let pin = ctx.params.f("pinHighlights") as f32 / 100.0;
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let half_diag = (lw * lw + lh * lh).sqrt() * 0.5;
    let dist = half_diag.max(1.0) / (fov * 0.5).tan();
    point(
        e,
        7,
        |p| {
            p.f[0] = [c.0 as f32, c.1 as f32, dist as f32, amt];
            p.f[1][0] = pin;
        },
        b,
    )
}

// ---------------------------------------------------------------- warps (fxs_warp)

fn warp(e: &mut Enc, mode: u32, f: impl FnOnce(&mut Params), b: GBuf, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let mut p = Params::default();
    p.u[0][0] = mode;
    f(&mut p);
    run(e, "fxs_warp", &p, b, data)
}

fn mirror(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = b.to_px(ctx.params.v2("center"));
    let a = ctx.params.f("angle").to_radians();
    warp(e, 1, |p| p.f[0] = [c.0 as f32, c.1 as f32, a.cos() as f32, a.sin() as f32], b, None)
}

fn spherize(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let r = (ctx.params.f("radius") * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    warp(e, 2, |p| p.f[0] = [c.0 as f32, c.1 as f32, r as f32, 0.0], b, None)
}

fn polar(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amt = ctx.params.f("interpolation") / 100.0;
    let to_polar = ctx.params.e("conversion") == 1;
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let rmax = cx.min(cy);
    warp(
        e,
        3,
        |p| {
            p.u[0][1] = to_polar as u32;
            p.f[0] = [w as f32, h as f32, cx as f32, cy as f32];
            p.f[1] = [rmax as f32, amt as f32, 0.0, 0.0];
        },
        b,
        None,
    )
}

fn cc_slant(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let k = ctx.params.f("slant") / 100.0;
    let height = ctx.params.f("height") / 100.0;
    if k == 0.0 && (height - 1.0).abs() < 1e-12 {
        return Some(b);
    }
    let stretch = ctx.params.b("stretching");
    let xs = if stretch { 1.0 / (1.0 + k * k).sqrt() } else { 1.0 };
    let hinv = if height.abs() < 1e-6 { 1e6 } else { 1.0 / height };
    if !ctx.adjustment {
        let lean = (k * ctx.layer_size[1] * b.scale * height.max(1.0)).abs();
        let grow = ((height - 1.0).max(0.0) * ctx.layer_size[1] * b.scale).max(lean);
        b.pad(e, grow.ceil().min(4096.0) as u32)?;
    }
    let fl = b.to_px(ctx.params.v2("floor"));
    warp(
        e,
        4,
        |p| {
            p.f[0] = [fl.0 as f32, fl.1 as f32, hinv as f32, k as f32];
            p.f[1][0] = xs as f32;
        },
        b,
        None,
    )
}

fn cc_smear(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let reach = ctx.params.f("reach") / 100.0;
    if !ctx.adjustment {
        b.pad(e, effectcraft_effects::cc_smear_pad(ctx, [b.img.width, b.img.height], b.offset, b.scale))?;
    }
    let from = b.to_px(ctx.params.v2("from"));
    let to = b.to_px(ctx.params.v2("to"));
    let radius = (ctx.params.f("radius") * b.scale).max(0.5);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let l2 = dx * dx + dy * dy;
    if l2 < 1e-12 || reach == 0.0 {
        return Some(b);
    }
    warp(
        e,
        5,
        |p| {
            p.f[0] = [from.0 as f32, from.1 as f32, dx as f32, dy as f32];
            p.f[1] = [l2 as f32, reach as f32, radius as f32, 0.0];
        },
        b,
        None,
    )
}

/// distort3::split_impl (CC Split / CC Split 2).
fn cc_split(e: &mut Enc, ctx: &EffectCtx, b: GBuf, s1: f64, s2: f64) -> Option<GBuf> {
    if s1 == 0.0 && s2 == 0.0 {
        return Some(b);
    }
    let a = b.to_px(ctx.params.v2("pointA"));
    let c = b.to_px(ctx.params.v2("pointB"));
    let (dx, dy) = (c.0 - a.0, c.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return Some(b);
    }
    let (ux, uy) = (dx / len, dy / len);
    warp(
        e,
        6,
        |p| {
            p.f[0] = [a.0 as f32, a.1 as f32, ux as f32, uy as f32];
            p.f[1] = [len as f32, s1 as f32, s2 as f32, (len * 0.5) as f32];
        },
        b,
        None,
    )
}

fn optics_compensation(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let fov = ctx.params.f("fieldOfView").clamp(0.0, 179.0).to_radians();
    if fov < 1e-6 {
        return Some(b);
    }
    let reverse = ctx.params.b("reverseLensDistortion");
    let (lx, ly, lw, lh) = layer_rect(ctx, &b);
    let r_ref = match ctx.params.e("fovOrientation") {
        1 => lh * 0.5,
        2 => (lw * lw + lh * lh).sqrt() * 0.5,
        _ => lw * 0.5,
    }
    .max(1.0);
    let half = fov * 0.5;
    let f = r_ref / half.tan();
    let optimal = ctx.params.b("optimalPixels");
    let resize = if optimal { 0 } else { ctx.params.e("resize") };
    let corners = [(lx, ly), (lx + lw, ly), (lx, ly + lh), (lx + lw, ly + lh)];
    if reverse && resize > 0 && !ctx.adjustment {
        let c = b.to_px(ctx.params.v2("viewCenter"));
        let rc = corners.iter().map(|p| ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()).fold(0.0, f64::max);
        let th = rc / r_ref * half;
        let cap = lw.max(lh) * [0.0, 0.5, 1.5, 3.5][resize.min(3) as usize];
        let grow = if th < FRAC_PI_2 - 1e-3 { (f * th.tan() - rc).max(0.0) } else { cap };
        b.pad(e, grow.min(cap).min(4096.0).ceil() as u32)?;
    }
    let c = b.to_px(ctx.params.v2("viewCenter"));
    let fit = if optimal {
        let rc = corners.iter().map(|p| ((p.0 - c.0).powi(2) + (p.1 - c.1).powi(2)).sqrt()).fold(1.0, f64::max);
        if reverse {
            let th = (rc / r_ref * half).min(FRAC_PI_2 - 1e-3);
            (f * th.tan() / rc).max(1.0)
        } else {
            ((rc / f).atan() / half * r_ref / rc).min(1.0)
        }
    } else {
        1.0
    };
    warp(
        e,
        7,
        |p| {
            p.u[0][1] = reverse as u32;
            p.f[0] = [c.0 as f32, c.1 as f32, fit as f32, f as f32];
            p.f[1] = [r_ref as f32, half as f32, 0.0, 0.0];
        },
        b,
        None,
    )
}

fn kaleida(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = b.to_px(ctx.params.v2("center"));
    let size = ctx.params.f("size").max(1.0) / 100.0;
    let mode = ctx.params.e("mirroring");
    let rot = ctx.params.f("rotation").to_radians();
    let n = [6.0, 8.0, 4.0, 12.0, 3.0][mode.min(4) as usize];
    let seg = TAU / n;
    warp(
        e,
        8,
        |p| {
            p.u[0][1] = (mode != 3) as u32;
            p.f[0] = [c.0 as f32, c.1 as f32, size as f32, rot as f32];
            p.f[1][0] = seg as f32;
        },
        b,
        None,
    )
}

fn liquify(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    // View Freeze Area Mask / View Mesh overlays render on the CPU (catalog CPU_ONLY_CONTROLS).
    if ctx.params.b("viewFreezeAreaMask") || ctx.params.b("viewMesh") {
        return None;
    }
    if ctx.params.s("distortionMesh").trim().is_empty() {
        return Some(b);
    }
    let pct = ctx.params.f("distortionPercentage") / 100.0;
    let mesh = effectcraft_effects::liquify_mesh(ctx);
    if mesh.nx < 2 || mesh.ny < 2 {
        return None;
    }
    let off = ctx.params.v2("distortionMeshOffset");
    let data: Vec<f32> = mesh.d.iter().flat_map(|d| [d[0] as f32, d[1] as f32]).collect();
    let buf = e.data(&data);
    let (scale, o) = (b.scale, b.offset);
    warp(
        e,
        9,
        |p| {
            p.u[0] = [9, mesh.nx as u32, mesh.ny as u32, 0];
            p.f[0] = [mesh.cell as f32, off[0] as f32, off[1] as f32, pct as f32];
            p.f[1] = [scale as f32, o[0] as f32, o[1] as f32, 0.0];
        },
        b,
        Some(&buf),
    )
}

fn twirl_legacy(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("angle").to_radians();
    if ang == 0.0 {
        return Some(b);
    }
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let r = (ctx.params.f("radius") / 100.0 * lw.min(lh) * 0.5).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    warp(e, 10, |p| p.f[0] = [c.0 as f32, c.1 as f32, r as f32, ang as f32], b, None)
}

fn ripple_pulse(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let level = ctx.params.f("pulseLevel") / 100.0;
    let amp = ctx.params.f("amplitude") / 100.0;
    if level == 0.0 || amp == 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let span = ctx.params.f("timeSpan").max(0.01);
    let size = ctx.layer_size[0].max(ctx.layer_size[1]) * b.scale;
    let speed = size * 0.5 / span;
    let lambda = (size * 0.06).max(2.0);
    let height = level * amp * lambda * 0.5;
    let front = speed * ctx.time.max(0.0);
    let bump = ctx.params.b("renderBump");
    warp(
        e,
        11,
        |p| {
            p.u[0][1] = bump as u32;
            p.f[0] = [c.0 as f32, c.1 as f32, front as f32, speed as f32];
            p.f[1] = [span as f32, lambda as f32, height as f32, 0.0];
        },
        b,
        None,
    )
}

fn power_pin(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    if ctx.params.b("unstretch") {
        return None; // Unstretch (reverse pinning) renders on the CPU (catalog CPU_ONLY_CONTROLS).
    }
    let persp = (ctx.params.f("perspective") / 100.0).clamp(0.0, 1.0);
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let ex = [ctx.params.f("expandTop"), ctx.params.f("expandLeft"), ctx.params.f("expandRight"), ctx.params.f("expandBottom")];
    let (x0, y0) = (-ex[1] / 100.0 * lw, -ex[0] / 100.0 * lh);
    let (x1, y1) = (lw + ex[2] / 100.0 * lw, lh + ex[3] / 100.0 * lh);
    let corners = [ctx.params.v2("topLeft"), ctx.params.v2("topRight"), ctx.params.v2("bottomRight"), ctx.params.v2("bottomLeft")];
    if !ctx.adjustment {
        let mut need = 0.0f64;
        for c in &corners {
            let (px, py) = b.to_px(*c);
            need = need.max(-px).max(-py).max(px - b.img.width as f64).max(py - b.img.height as f64);
        }
        if need > 0.0 {
            b.pad(e, need.ceil().min(4096.0) as u32 + 1)?;
        }
    }
    let q = corners.map(|c| {
        let p = b.to_px(c);
        vec2(p.0, p.1)
    });
    let Some(inv) = Mat3::square_to_quad(q).inverse() else { return Some(b) };
    let r = inv.0.map(|r| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0]);
    let (sc, off) = (b.scale, b.offset);
    warp(
        e,
        12,
        |p| {
            p.f[2] = r[0];
            p.f[3] = r[1];
            p.f[4] = r[2];
            p.f[5] = [x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32];
            p.f[6] = [sc as f32, off[0] as f32, off[1] as f32, 0.0];
            if persp < 1.0 {
                let rel = |i: usize| [(q[i].x - q[0].x) as f32, (q[i].y - q[0].y) as f32];
                let (q1, q2, q3) = (rel(1), rel(2), rel(3));
                p.f[7] = [q[0].x as f32, q[0].y as f32, persp as f32, 1.0];
                p.f[8] = [q1[0], q1[1], q2[0], q2[1]];
                p.f[9] = [q3[0], q3[1], 0.0, 0.0];
            }
        },
        b,
        None,
    )
}

fn flo_motion(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let knots = [(ctx.params.v2("knot1"), ctx.params.f("amount1") / 100.0), (ctx.params.v2("knot2"), ctx.params.f("amount2") / 100.0)];
    if knots.iter().all(|k| k.1 == 0.0) {
        return Some(b);
    }
    let ss = match ctx.params.e("antialiasing") {
        0 => 1,
        1 => 2,
        _ => 3,
    };
    let falloff = ctx.params.f("falloff").max(0.01);
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let radius = (lw * lw + lh * lh).sqrt() * 0.5;
    let mut p = Params::default();
    p.u[0] = [ss, ctx.params.b("tileEdges") as u32, 0, 0];
    for (i, (k, a)) in knots.iter().enumerate() {
        p.f[i] = [k[0] as f32, k[1] as f32, *a as f32, 0.0];
    }
    p.f[2] = [falloff as f32, radius as f32, lw as f32, lh as f32];
    p.f[3] = [b.scale as f32, b.offset[0] as f32, b.offset[1] as f32, 0.0];
    run(e, "fxs_flomotion", &p, b, None)
}

// ---------------------------------------------------------------- separable tables

/// Per-column `fx(x + 0.5)` then per-row `fy(y + 0.5)`, `k` values each.
fn axis_table<const K: usize>(w: u32, h: u32, fx: impl Fn(f64) -> [f64; K], fy: impl Fn(f64) -> [f64; K]) -> Vec<f32> {
    let mut v = Vec::with_capacity((w + h) as usize * K);
    v.extend((0..w).flat_map(|x| fx(x as f64 + 0.5).map(|t| t as f32)));
    v.extend((0..h).flat_map(|y| fy(y as f64 + 0.5).map(|t| t as f32)));
    v
}

fn table(e: &mut Enc, b: GBuf, data: &[f32], repeat: bool, blend: f32) -> Option<GBuf> {
    let mut p = Params::default();
    p.u[0] = [b.img.width, repeat as u32, 0, 0];
    p.f[0][0] = blend;
    let buf = e.data(data);
    run(e, "fxs_table", &p, b, Some(&buf))
}

fn offset(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let shift = ctx.params.v2("shift");
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (sx, sy) = b.to_px(shift);
    let data = axis_table(b.img.width, b.img.height, |x| [(x - (sx - cx)).rem_euclid(w)], |y| [(y - (sy - cy)).rem_euclid(h)]);
    table(e, b, &data, false, blend.max(0.0))
}

fn cc_tiler(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let s = (ctx.params.f("scale") / 100.0).max(0.001);
    if !ctx.adjustment {
        b.pad(e, effectcraft_effects::util::layer_cover_pad(ctx, [b.img.width, b.img.height], b.offset, b.scale))?;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let blend = (ctx.params.f("blendWithOriginal") / 100.0) as f32;
    let (x0, y0, w, h) = layer_rect(ctx, &b);
    let (w, h) = (w.max(1.0), h.max(1.0));
    let data = axis_table(b.img.width, b.img.height, |x| [x0 + (c.0 + (x - c.0) / s - x0).rem_euclid(w)], |y| [y0 + (c.1 + (y - c.1) / s - y0).rem_euclid(h)]);
    table(e, b, &data, true, blend)
}

fn cc_griddler(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let hs = (ctx.params.f("horizontalScale") / 100.0).max(0.001);
    let vs = (ctx.params.f("verticalScale") / 100.0).max(0.001);
    let rot = ctx.params.f("rotation").to_radians();
    let cut = ctx.params.b("cutTiles");
    let (x0, y0, lw, _) = layer_rect(ctx, &b);
    let t = (ctx.params.f("tileSize") / 100.0 * lw).max(1.0);
    if (hs - 1.0).abs() < 1e-9 && (vs - 1.0).abs() < 1e-9 && rot.abs() < 1e-9 && !cut {
        return Some(b);
    }
    let (sr, cr) = rot.sin_cos();
    let centre = |p: f64, o: f64| {
        let tc = o + (((p - o) / t).floor() + 0.5) * t;
        [tc, p - tc]
    };
    let data = axis_table(b.img.width, b.img.height, |x| centre(x, x0), |y| centre(y, y0));
    let mut p = Params::default();
    p.u[0] = [b.img.width, cut as u32, 0, 0];
    p.f[0] = [cr as f32, sr as f32, hs as f32, vs as f32];
    p.f[1][0] = (0.45 * t) as f32;
    let buf = e.data(&data);
    run(e, "fxs_griddler", &p, b, Some(&buf))
}

fn motion_tile(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let s = b.scale;
    let (lw, lh) = (ctx.layer_size[0], ctx.layer_size[1]);
    if lw <= 0.0 || lh <= 0.0 {
        return Some(b);
    }
    let tc = ctx.params.v2("tileCenter");
    let tw = (lw * ctx.params.f("tileWidth") / 100.0).max(0.01);
    let th = (lh * ctx.params.f("tileHeight") / 100.0).max(0.01);
    let mirror = ctx.params.b("mirrorEdges");
    let phase = ctx.params.f("phase") / 360.0;
    let hshift = ctx.params.b("horizontalPhaseShift");
    let (nw, nh, noff) = effectcraft_effects::motion_tile_frame(ctx, b.offset, (b.img.width, b.img.height), s);
    if !e.g.fits(nw, nh) {
        return None;
    }
    let split = |t: f64| [t.floor(), t - t.floor()];
    let data = axis_table(nw, nh, |x| split(((x - noff[0]) / s - tc[0]) / tw + 0.5), |y| split(((y - noff[1]) / s - tc[1]) / th + 0.5));
    let mut p = Params::default();
    p.u[0] = [nw, hshift as u32, mirror as u32, 0];
    p.f[0] = [phase as f32, (lw * s) as f32, (lh * s) as f32, 0.0];
    p.f[1] = [b.offset[0] as f32, b.offset[1] as f32, 0.0, 0.0];
    let buf = e.data(&data);
    let out = e.scratch(nw, nh);
    e.pixels("fxs_motiontile", &p, &b.img, None, &out, Some(&buf));
    Some(GBuf { img: out, offset: noff, scale: s })
}

fn repetile(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let s = b.scale;
    let (r, l, d, u) = (
        ctx.params.f("expandRight").max(0.0) * s,
        ctx.params.f("expandLeft").max(0.0) * s,
        ctx.params.f("expandDown").max(0.0) * s,
        ctx.params.f("expandUp").max(0.0) * s,
    );
    if ctx.adjustment || r + l + d + u < 0.5 {
        return Some(b);
    }
    let (x0, y0, w, h) = layer_rect(ctx, &b);
    if w < 1.0 || h < 1.0 {
        return Some(b);
    }
    let (nw, nh) = ((w + l + r).round().max(1.0) as u32, (h + u + d).round().max(1.0) as u32);
    if !e.g.fits(nw, nh) {
        return None;
    }
    // stylize2::tile_coord.
    let tile = |v: f64, size: f64| {
        let i = (v / size).floor();
        [i, v - i * size]
    };
    let data = axis_table(nw, nh, |x| tile(x - l, w), |y| tile(y - u, h));
    let mut p = Params::default();
    p.u[0] = [nw, ctx.params.e("tiling"), 0, 0];
    p.f[0] = [x0 as f32, y0 as f32, w as f32, h as f32];
    let buf = e.data(&data);
    let out = e.scratch(nw, nh);
    e.pixels("fxs_repetile", &p, &b.img, None, &out, Some(&buf));
    Some(GBuf { img: out, offset: [l, u], scale: s })
}

fn magnify(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let square = ctx.params.e("shape") == 1;
    let c = b.to_px(ctx.params.v2("center"));
    let mag = (ctx.params.f("magnification") / 100.0).max(0.01);
    let link = ctx.params.e("link");
    let mut size = ctx.params.f("size") * b.scale;
    let mut feather = ctx.params.f("feather") * b.scale;
    if link >= 1 {
        size *= mag;
    }
    if link == 2 {
        feather *= mag;
    }
    let op = (ctx.params.f("opacity") / 100.0) as f32;
    let mode = ctx.params.e("blendingMode") as usize;
    let blend = effectcraft_effects::MAGNIFY_MODES.get(mode.wrapping_sub(1)).copied();
    if ctx.params.b("resizeLayer") && link == 0 && !ctx.adjustment {
        let (lx, ly, lw, lh) = layer_rect(ctx, &b);
        let reach = size.max(0.0);
        let over = [lx - (c.0 - reach), (c.0 + reach) - (lx + lw), ly - (c.1 - reach), (c.1 + reach) - (ly + lh)].into_iter().fold(0.0, f64::max);
        b.pad(e, over.min(4096.0).ceil() as u32)?;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let axis = |p: f64, c: f64| {
        let d = p - c;
        let sp = c + d / mag;
        [d, sp, sp.floor() + 0.5]
    };
    let data = axis_table(b.img.width, b.img.height, |x| axis(x, c.0), |y| axis(y, c.1));
    let (kind, mode_id) = match blend {
        None => (0, 0),
        Some(BlendMode::Normal) => (1, 0),
        Some(m) => (2, ops::mode_id(m)),
    };
    let mut p = Params::default();
    p.u[0] = [b.img.width, square as u32, ctx.params.e("scaling"), kind];
    p.u[1] = [mode_id, ctx.seed ^ 0x3a61, 0, 0];
    p.f[0] = [size as f32, feather as f32, op, mag as f32];
    p.f[1][0] = (size - feather) as f32;
    let buf = e.data(&data);
    run(e, "fxs_magnify", &p, b, Some(&buf))
}

// ---------------------------------------------------------------- stylize2.rs

fn scatter(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amt = ctx.params.f("amount") * b.scale;
    if amt <= 0.0 {
        return Some(b);
    }
    let seed = if ctx.params.b("randomizeEveryFrame") { ctx.seed ^ ((ctx.time * 1000.0) as u32).wrapping_mul(0x9e37_79b9) } else { ctx.seed };
    let mut p = Params::default();
    p.u[0] = [ctx.params.e("grain"), seed, 0, 0];
    p.f[0][0] = amt as f32;
    run(e, "fxs_scatter", &p, b, None)
}

fn brush_strokes(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("strokeAngle");
    let size = (ctx.params.f("brushSize") * b.scale).max(0.5);
    let len = ctx.params.f("strokeLength") * b.scale;
    let density = ctx.params.f("strokeDensity") as f32;
    let rnd = ctx.params.f("strokeRandomness");
    let blend = ctx.params.f("blendWithOriginal") as f32 / 100.0;
    let seed = ctx.seed ^ 0xb705;
    let cell = (size * 2.0).max(1.0);
    let (w, h) = (b.img.width, b.img.height);
    let cell_of = |i: u32| (i as f64 / cell).floor() as i64 as u32;
    let (ncx, ncy) = (cell_of(w.saturating_sub(1)) + 1, cell_of(h.saturating_sub(1)) + 1);
    if (ncx as u64 * ncy as u64 * 6 + (w + h) as u64) >= 1 << 24 {
        return None;
    }
    let mut data: Vec<f32> = (0..w).map(|x| cell_of(x) as f32).chain((0..h).map(|y| cell_of(y) as f32)).collect();
    data.reserve((ncx * ncy * 6) as usize);
    for cy in 0..ncy {
        for cx in 0..ncx {
            let hh = |k: u32| effectcraft_raster::hash_noise(cx.wrapping_add(k.wrapping_mul(7919)), cy, seed) as f64;
            let a = (ang + (hh(0) - 0.5) * rnd * 60.0).to_radians();
            let l = len * (0.5 + hh(1));
            let (dx, dy) = (a.sin(), -a.cos());
            let jit = (hh(2) - 0.5) * size;
            let n = l.ceil().clamp(1.0, 32.0);
            let m = if (hh(3) as f32) < density.min(1.0) { 1.0 } else { 0.0 };
            data.extend([dx as f32, dy as f32, l as f32, jit as f32, n as f32, m]);
        }
    }
    let mut p = Params::default();
    p.u[0] = [w, ncx, ctx.params.e("paintSurface"), h];
    p.f[0][0] = blend;
    let buf = e.data(&data);
    run(e, "fxs_brush", &p, b, Some(&buf))
}

fn roughen_edges(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let kind = ctx.params.e("edgeType");
    let ec = ctx.params.color("edgeColor");
    let border = ctx.params.f("border") * b.scale;
    let sharp = ctx.params.f("edgeSharpness").max(0.01) as f32;
    let infl = ctx.params.f("fractalInfluence") as f32;
    let scale = (ctx.params.f("scale") * b.scale * 0.1).max(0.5) as f32;
    let (ox, oy) = b.to_px(ctx.params.v2("offset"));
    let oct = ctx.params.f("complexity") as f32;
    let evo = ctx.params.f("evolution") as f32 / 360.0;
    let st = ctx.params.f("stretch") as f32;
    let (sx, sy) = (1.0 + st.max(0.0), 1.0 + (-st).max(0.0));
    let cycle = ctx.params.b("evolutionOptions/cycleEvolution").then(|| ctx.params.f("evolutionOptions/cycle").round().max(1.0) as f32);
    let (evo, evo_w, c) = match cycle {
        Some(c) => {
            let ev = evo.rem_euclid(c);
            (ev, ev / c, c)
        }
        None => (evo, 0.0, 0.0),
    };
    let (fm, im, sm, colored) = match kind {
        1 => (1.0, 1.0, 1.0, true),
        2 => (1.0, 1.0, 3.0, false),
        3 => (3.0, 1.0, 1.5, false),
        4 => (1.5, 1.5, 1.0, false),
        5 => (1.5, 1.5, 1.0, true),
        6 => (0.6, 1.2, 2.5, false),
        7 => (0.6, 1.2, 2.5, true),
        _ => (1.0, 1.0, 1.0, false),
    };
    let extra = if kind == 4 || kind == 5 { 1.0 } else { 0.0 };
    let seed = ctx.seed ^ 0x40f1 ^ (ctx.params.f("evolutionOptions/randomSeed").round() as i64 as u32).wrapping_mul(0x9e37_79b9);
    // util::gauss_plane of the alpha (edges repeated); the kernel reads .w.
    let d = if border > 0.05 { gaussian_blur(e, &b.img, border * 0.5, border * 0.5, true) } else { b.img.clone() };
    let mut p = Params::default();
    p.u[0] = [seed, cycle.is_some() as u32, colored as u32, 0];
    p.f[0] = [ox as f32, oy as f32, scale * sx, scale * sy];
    p.f[1] = [fm, evo, c, evo_w];
    p.f[2] = [oct + extra, infl, im, sharp];
    p.f[3] = [ec[0], ec[1], ec[2], sm];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxs_roughen", &p, &b.img, Some(&d), &out, None);
    b.img = out;
    Some(b)
}

fn texturize(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("lightDirection").to_radians();
    let k = ctx.params.f("textureContrast") as f32;
    if k == 0.0 {
        return Some(b);
    }
    let Some(tex) = ctx.layer_param("textureLayer", true) else { return Some(b) };
    if tex.buf.img.is_empty() {
        return None;
    }
    // transition::place_layer, per column / row in the texture buffer's pixels.
    let (ls, os) = (ctx.layer_size, tex.size);
    let inv = 1.0 / b.scale.max(1e-9);
    let mode = ctx.params.e("texturePlacement");
    let place = |v: f64, k: usize| {
        let p = (v - b.offset[k]) * inv;
        let q = match mode {
            0 if os[0] >= 1.0 && os[1] >= 1.0 => p.rem_euclid(os[k]),
            1 => p + (os[k] - ls[k]) * 0.5,
            _ if ls[0] > 0.0 && ls[1] > 0.0 => p * os[k] / ls[k],
            _ => p,
        };
        [q * tex.buf.scale + tex.buf.offset[k]]
    };
    let data = axis_table(b.img.width, b.img.height, |x| place(x, 0), |y| place(y, 1));
    let timg = e.g.upload_image(&tex.buf.img)?;
    let mut p = Params::default();
    p.u[0][0] = b.img.width;
    let buf = e.data(&data);
    let luma = e.image(b.img.width, b.img.height);
    e.pixels("fxs_tex_luma", &p, &b.img, Some(&timg), &luma, Some(&buf));
    let s = 0.7 * b.scale;
    let hgt = gaussian_blur(e, &luma, s, s, true);
    let mut p = Params::default();
    p.f[0] = [ang.cos() as f32, -ang.sin() as f32, k, 0.0];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("fxs_texturize", &p, &b.img, Some(&hgt), &out, None);
    b.img = out;
    Some(b)
}

// ---------------------------------------------------------------- distort.rs

fn corner_pin(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let (w, h) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let q = ["ul", "ur", "lr", "ll"].map(|k| {
        let p = b.to_px(ctx.params.v2(k));
        vec2(p.0, p.1)
    });
    let quad = Mat3::square_to_quad(q);
    let o = b.to_px([0.0, 0.0]);
    let m = quad * Mat3::scale(vec2(1.0 / (w * b.scale), 1.0 / (h * b.scale))) * Mat3::translate(vec2(-o.0, -o.1));
    let empty = e.image(b.img.width, b.img.height);
    b.img = ops::warp(e, &empty, &b.img, &m, Sampling::Bilinear, BlendMode::Normal, 1.0, 0, None);
    Some(b)
}

// ---------------------------------------------------------------- Glow / Transform extras

/// Glow settings the base kernel in `effects.rs` leaves to the CPU (another Glow Operation or
/// the Arbitrary Map) and [`glow`] implements.
pub(crate) fn glow_extra(ctx: &EffectCtx) -> bool {
    ctx.params.e("colors") == 2 || effectcraft_effects::glow_operation(ctx) != BlendMode::Add
}

fn dims_xy(dim: u32) -> (f64, f64) {
    match dim {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    }
}

/// misc::glow with any Glow Operation and the Arbitrary Map.
pub(crate) fn glow(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let thr = ctx.params.f("threshold") as f32 / 100.0;
    let radius = ctx.params.f("radius") * b.scale;
    let intensity = ctx.params.f("intensity") as f32;
    let colors = ctx.params.e("colors");
    let (kx, ky) = dims_xy(ctx.params.e("glowDimensions"));
    if !ctx.adjustment {
        b.pad(e, (radius * 1.5).ceil() as u32 + 2)?;
    }
    let mut p = Params::default();
    p.u[0] = [(colors == 1) as u32, (ctx.params.e("based") == 0) as u32, ctx.params.e("colorLooping"), 0];
    p.f[0] = [thr, ctx.params.f("colorLoops") as f32, (ctx.params.f("colorPhase") / 360.0) as f32, ctx.params.f("abMidpoint") as f32 / 100.0];
    let bright = e.image(b.img.width, b.img.height);
    if colors == 2 {
        // Arbitrary Map: Curves tables per channel (see effects::curves).
        let mut parts = ctx.params.s("arbitraryMap").split('|');
        let curves: Vec<Option<effectcraft_effects::Curve>> = (0..3).map(|_| parts.next().and_then(effectcraft_effects::Curve::parse)).collect();
        let mut data = vec![0.0f32];
        let mut offs = [-1.0f32; 3];
        for (i, c) in curves.iter().enumerate() {
            let Some(c) = c else { continue };
            let (xs, ys, ms, lut) = c.tables();
            offs[i] = data.len() as f32;
            data.push(xs.len() as f32);
            data.extend_from_slice(xs);
            data.extend_from_slice(ys);
            data.extend_from_slice(ms);
            data.extend_from_slice(lut);
        }
        p.f[1] = [offs[0], offs[1], offs[2], 0.0];
        let buf = e.data(&data);
        e.pixels("fxs_glow_map", &p, &b.img, None, &bright, Some(&buf));
    } else {
        p.f[1] = ctx.params.color("colorA");
        p.f[2] = ctx.params.color("colorB");
        e.pixels("glow_bright", &p, &b.img, None, &bright, None);
    }
    let s = (radius / 2.0).max(0.5);
    let blurred = gaussian_blur(e, &bright, s * kx, s * ky, false);
    let operation = ctx.params.e("operation");
    let glow_op = effectcraft_effects::glow_operation(ctx);
    let mut p = Params::default();
    p.f[0][0] = intensity;
    let out = e.scratch(b.img.width, b.img.height);
    if operation == 0 && glow_op != BlendMode::Add {
        p.u[0][0] = ops::mode_id(glow_op);
        e.pixels("fxs_glow_op", &p, &b.img, Some(&blurred), &out, None);
    } else {
        p.u[0][0] = operation.min(2);
        e.pixels("glow_combine", &p, &b.img, Some(&blurred), &out, None);
    }
    b.img = out;
    Some(b)
}

/// Transform renders with motion blur (a shutter and a host to read the parameters at other
/// times): [`transform`] implements it, the base kernel in `effects.rs` the sharp case.
pub(crate) fn transform_blur(ctx: &EffectCtx) -> bool {
    ctx.env.host.is_some() && effectcraft_effects::transform_shutter(ctx).is_some()
}

/// distort::transform_matrix.
fn transform_matrix(pr: &effectcraft_effects::Params, b: &GBuf) -> Mat3 {
    let anchor = b.to_px(pr.v2("anchor"));
    let pos = b.to_px(pr.v2("position"));
    let sh = pr.f("scaleHeight");
    let sw = if pr.b("uniform") { sh } else { pr.f("scaleWidth") };
    Mat3::translate(vec2(pos.0, pos.1))
        * Mat3::rotate_deg(pr.f("rotation"))
        * Mat3::skew_deg(pr.f("skew"), pr.f("skewAxis"))
        * Mat3::scale(vec2(sw / 100.0, sh / 100.0))
        * Mat3::translate(vec2(-anchor.0, -anchor.1))
}

/// distort::transform's motion blur: the transform at Samples instants across the shutter,
/// averaged.
pub(crate) fn transform(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let (angle, phase, n) = effectcraft_effects::transform_shutter(ctx)?;
    let host = ctx.env.host?;
    let sampling = if ctx.params.e("sampling") == 1 { Sampling::Bicubic } else { Sampling::Bilinear };
    let fd = 1.0 / ctx.fps();
    let (w, h) = (b.img.width, b.img.height);
    let mut acc = e.image(w, h);
    let mut count = 0u32;
    for k in 0..n {
        let t = ctx.time + (phase + angle * k as f64 / (n - 1) as f64) / 360.0 * fd;
        let Some(pr) = host.params_at(t) else { continue };
        let op = pr.f("opacity") as f32 / 100.0;
        let empty = e.image(w, h);
        let one = ops::warp(e, &empty, &b.img, &transform_matrix(&pr, &b), sampling, BlendMode::Normal, op, 0, None);
        let sum = e.image(w, h);
        e.pixels("fxs_add", &Params::default(), &acc, Some(&one), &sum, None);
        acc = sum;
        count += 1;
    }
    if count == 0 {
        // The CPU renders the frame sharp.
        return None;
    }
    let mut p = Params::default();
    p.f[0][0] = count as f32;
    let out = e.scratch(w, h);
    e.pixels("fxs_div", &p, &acc, None, &out, None);
    b.img = out;
    Some(b)
}
