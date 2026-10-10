//! Distort effects: inverse-mapped warps (each output pixel samples the source).

use effectcraft_geom::{Mat3, vec2};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use rayon::prelude::*;

use crate::util::layer_rect;
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Distort", params, render, gpu: false, float: true }
}

/// Resample through `f(x, y) -> source point` (pixel coordinates, centres at +0.5).
fn remap(src: &Image, repeat: bool, f: impl Fn(f64, f64) -> (f64, f64) + Sync) -> Image {
    let mut out = Image::new(src.width, src.height);
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (sx, sy) = f(x as f64 + 0.5, y as f64 + 0.5);
            *px = if repeat { src.sample_bilinear_clamped(sx, sy) } else { src.sample_bilinear(sx, sy) };
        }
    });
    out
}

fn twirl(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let ang = ctx.params.f("angle").to_radians();
    // Twirl Radius is a percentage of the layer's larger dimension: 50 reaches the edges.
    let r = (ctx.params.f("radius") / 100.0 * b.layer_w(ctx).max(b.layer_h(ctx)) * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    b.img = remap(&b.img, false, |x, y| {
        let (dx, dy) = (x - c.0, y - c.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d >= r {
            return (x, y);
        }
        let t = 1.0 - d / r;
        let a = -ang * t * t;
        let (s, co) = a.sin_cos();
        (c.0 + dx * co - dy * s, c.1 + dx * s + dy * co)
    });
    b
}

fn spherize(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let r = (ctx.params.f("radius") * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    b.img = remap(&b.img, false, |x, y| {
        let (dx, dy) = (x - c.0, y - c.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d >= r || d == 0.0 {
            return (x, y);
        }
        let nd = d / r;
        let k = (nd.asin() / std::f64::consts::FRAC_PI_2) / nd;
        (c.0 + dx * k, c.1 + dy * k)
    });
    b
}

fn bulge(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let rx = (ctx.params.f("hradius") * b.scale).max(1.0);
    let ry = (ctx.params.f("vradius") * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    let h = ctx.params.f("height");
    // Taper Radius: higher values make the sides of the bulge shallower (a more peaked profile).
    let taper = 1.0 + ctx.params.f("taperRadius").max(0.0) / 100.0 * 3.0;
    let pin = ctx.params.b("pinAllEdges");
    let (lx, ly, lw, lh) = layer_rect(ctx, &b);
    let edge = (lw.min(lh) * 0.1).max(1.0);
    b.img = remap(&b.img, false, |x, y| {
        let (dx, dy) = ((x - c.0) / rx, (y - c.1) / ry);
        let d2 = dx * dx + dy * dy;
        if d2 >= 1.0 {
            return (x, y);
        }
        let mut amt = h * (1.0 - d2).powf(taper) * 0.5;
        if pin {
            let ex = ((x - lx - 0.5).min(lx + lw - 0.5 - x) / edge).clamp(0.0, 1.0);
            let ey = ((y - ly - 0.5).min(ly + lh - 0.5 - y) / edge).clamp(0.0, 1.0);
            amt *= ex.min(ey);
        }
        let k = 1.0 - amt;
        (c.0 + dx * rx * k, c.1 + dy * ry * k)
    });
    b
}

/// Wave Warp's wave shapes ([`WAVE_TYPES`] order) at phase `t` (radians), in -1..1. `seed`
/// seeds the noise shapes.
fn wave_shape(kind: u32, t: f64, seed: u32) -> f64 {
    let tau = std::f64::consts::TAU;
    let ph = t.rem_euclid(tau) / tau;
    // A half circle over u in 0..1 (0 at the ends, 1 in the middle).
    let half = |u: f64| (1.0 - (2.0 * u - 1.0).powi(2)).max(0.0).sqrt();
    let noise = |i: f64| crate::util::hash1(i as i64 as u32, 0x5eed, seed) as f64 * 2.0 - 1.0;
    match kind {
        1 => {
            if ph < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        2 => 1.0 - 4.0 * (ph - 0.5).abs(),
        3 => 2.0 * ph - 1.0,
        // Circle: alternating upper and lower half circles.
        4 => {
            if ph < 0.5 {
                half(ph * 2.0)
            } else {
                -half(ph * 2.0 - 1.0)
            }
        }
        // Semicircle: a row of half circles bulging the same way.
        5 => 2.0 * half((ph * 2.0).fract()) - 1.0,
        // Uncircle: the concave gaps between half circles, meeting in cusps.
        6 => 1.0 - 2.0 * half((ph * 2.0).fract()),
        // Noise: a random level per half wave; Smooth Noise eases between the levels.
        7 => noise((t / tau * 2.0).floor()),
        8 => {
            let u = t / tau * 2.0;
            let (i, f) = (u.floor(), u - u.floor());
            let s = f * f * (3.0 - 2.0 * f);
            noise(i) * (1.0 - s) + noise(i + 1.0) * s
        }
        _ => t.sin(),
    }
}

pub(crate) const WAVE_TYPES: [&str; 9] = ["Sine", "Square", "Triangle", "Sawtooth", "Circle", "Semicircle", "Uncircle", "Noise", "Smooth Noise"];
pub(crate) const WAVE_PINNING: [&str; 9] =
    ["None", "All Edges", "Center", "Left Edge", "Top Edge", "Right Edge", "Bottom Edge", "Horizontal Edges", "Vertical Edges"];

/// Wave Warp pinning weight (0 = pinned, 1 = free) at buffer pixel (x, y) of the layer
/// rectangle `(x0, y0, w, h)`.
fn wave_pin(pin: u32, x: f64, y: f64, rect: (f64, f64, f64, f64)) -> f64 {
    let (x0, y0, w, h) = rect;
    // Measured between the outermost pixel centres, so pinned edges stay exactly in place.
    let (u, v) = (((x - x0 - 0.5) / (w - 1.0).max(1.0)).clamp(0.0, 1.0), ((y - y0 - 0.5) / (h - 1.0).max(1.0)).clamp(0.0, 1.0));
    let horiz = (v.min(1.0 - v) * 2.0).clamp(0.0, 1.0);
    let vert = (u.min(1.0 - u) * 2.0).clamp(0.0, 1.0);
    match pin {
        1 => horiz.min(vert),
        2 => (((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0).min(1.0),
        3 => u,
        4 => v,
        5 => 1.0 - u,
        6 => 1.0 - v,
        7 => horiz,
        8 => vert,
        _ => 1.0,
    }
}

fn wave_warp(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let h = ctx.params.f("height") * b.scale;
    let w = (ctx.params.f("width") * b.scale).max(1.0);
    let dir = ctx.params.f("direction").to_radians();
    let speed = ctx.params.f("speed");
    let phase = ctx.params.f("phase").to_radians() + ctx.time * speed * std::f64::consts::TAU;
    let kind = ctx.params.e("waveType");
    let pin = ctx.params.e("pinning");
    let seed = ctx.seed ^ 0x3a7e;
    let (dx, dy) = (dir.sin(), -dir.cos());
    if !ctx.adjustment {
        b.pad(h.abs().ceil() as u32 + 1);
    }
    let (x0, y0, lw, lh) = layer_rect(ctx, &b);
    let rect = (x0, y0, lw.max(1.0), lh.max(1.0));
    b.img = remap(&b.img, false, |x, y| {
        // Waves travel along `dir`; displacement is perpendicular to it.
        let along = x * dx + y * dy;
        let disp = wave_shape(kind, along / w * std::f64::consts::TAU + phase, seed) * h * wave_pin(pin, x, y, rect);
        (x + dy * disp, y - dx * disp)
    });
    b
}

fn ripple(ctx: &EffectCtx, mut b: Buf) -> Buf {
    // Radius is a percentage of the layer: 100 from the layer centre reaches the edge.
    let r = (ctx.params.f("radius") / 100.0 * b.layer_w(ctx).max(b.layer_h(ctx)) * 0.5 * b.scale).max(1.0);
    let asymmetric = ctx.params.e("conversion") == 0;
    let c = b.to_px(ctx.params.v2("center"));
    let w = (ctx.params.f("waveWidth") * b.scale).max(1.0);
    let h = ctx.params.f("waveHeight") * b.scale;
    let phase = ctx.params.f("phase").to_radians() - ctx.time * ctx.params.f("speed") * std::f64::consts::TAU;
    b.img = remap(&b.img, false, |x, y| {
        let (dx, dy) = (x - c.0, y - c.1);
        let d = (dx * dx + dy * dy).sqrt();
        if d >= r || d == 0.0 {
            return (x, y);
        }
        let fall = 1.0 - d / r;
        let arg = d / w * std::f64::consts::TAU + phase;
        let o = arg.sin() * h * fall;
        // Asymmetric ripples also move sideways (a quarter wave out of step).
        let t = if asymmetric { arg.cos() * h * fall * 0.5 } else { 0.0 };
        let (ux, uy) = (dx / d, dy / d);
        (x + ux * o - uy * t, y + uy * o + ux * t)
    });
    b
}

fn mirror(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let a = ctx.params.f("angle").to_radians();
    let n = (a.cos(), a.sin());
    b.img = remap(&b.img, false, |x, y| {
        let d = (x - c.0) * n.0 + (y - c.1) * n.1;
        if d > 0.0 { (x - 2.0 * d * n.0, y - 2.0 * d * n.1) } else { (x, y) }
    });
    b
}

fn offset(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let shift = ctx.params.v2("shift");
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (sx, sy) = b.to_px(shift);
    let orig = b.img.clone();
    let mut o = remap(&b.img, false, |x, y| ((x - (sx - cx)).rem_euclid(w), (y - (sy - cy)).rem_euclid(h)));
    if blend > 0.0 {
        for (p, q) in o.data.iter_mut().zip(&orig.data) {
            for i in 0..4 {
                p[i] += (q[i] - p[i]) * blend;
            }
        }
    }
    b.img = o;
    b
}

fn polar(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let amt = ctx.params.f("interpolation") / 100.0;
    let to_polar = ctx.params.e("conversion") == 1;
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let rmax = cx.min(cy);
    b.img = remap(&b.img, true, |x, y| {
        let (px, py) = if to_polar {
            // output is polar: angle → x, radius → y
            let dx = x - cx;
            let dy = y - cy;
            let a = dx.atan2(-dy).rem_euclid(std::f64::consts::TAU);
            let r = (dx * dx + dy * dy).sqrt() / rmax;
            (a / std::f64::consts::TAU * w, h - r * h)
        } else {
            let a = x / w * std::f64::consts::TAU;
            let r = (h - y) / h * rmax;
            (cx + r * a.sin(), cy - r * a.cos())
        };
        (x + (px - x) * amt, y + (py - y) * amt)
    });
    b
}

/// Transform's shutter: (angle, phase) in degrees and the number of samples, or None when it
/// renders without motion blur. Use Composition's Shutter Angle takes the comp's shutter
/// (only when the comp and layer motion-blur switches are on); otherwise Shutter Angle, centred
/// on the frame.
pub fn transform_shutter(ctx: &EffectCtx) -> Option<(f64, f64, u32)> {
    let (angle, phase, n) = if ctx.params.b("useCompositionShutterAngle") {
        ctx.env.shutter?
    } else {
        let a = ctx.params.f("shutterAngle").clamp(0.0, 720.0);
        (a, -a * 0.5, 16)
    };
    (angle > 0.0).then_some((angle, phase, n.clamp(2, 64)))
}

fn transform_matrix(pr: &crate::Params, b: &Buf) -> Mat3 {
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

/// A self-contained transform whose Shift, Z-Dist and Rotate animate together around Center.
fn motion_curve_matrix(pr: &crate::Params, b: &Buf) -> Mat3 {
    let center = b.to_px(pr.v2("center"));
    let shift = pr.v2("shift");
    let scale = pr.f("zDist") / 100.0;
    Mat3::translate(vec2(shift[0] * b.scale, shift[1] * b.scale))
        * Mat3::translate(vec2(center.0, center.1))
        * Mat3::rotate_deg(pr.f("rotate"))
        * Mat3::scale(vec2(scale, scale))
        * Mat3::translate(vec2(-center.0, -center.1))
}

fn motion_curve_params_finite(pr: &crate::Params) -> bool {
    let center = pr.v2("center");
    let shift = pr.v2("shift");
    center.into_iter().chain(shift).all(f64::is_finite) && pr.f("zDist").is_finite() && pr.f("rotate").is_finite()
}

fn motion_curve_transform(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let shutter = if ctx.params.b("useCompositionShutterAngle") {
        ctx.env.shutter
    } else {
        let raw_angle = ctx.params.f("shutterAngle");
        let raw_samples = ctx.params.f("samples");
        let angle = raw_angle.clamp(0.0, 720.0);
        let samples = if raw_samples.is_finite() { raw_samples.round().clamp(2.0, 64.0) as u32 } else { 16 };
        (angle > 0.0 && angle.is_finite()).then_some((angle, -angle * 0.5, samples))
    };
    if let (Some((angle, phase, samples)), Some(host)) = (shutter, ctx.env.host) {
        if angle.is_finite() && phase.is_finite() && ctx.time.is_finite() && angle > 0.0 {
            let mut accum = Image::new(b.img.width, b.img.height);
            let mut count = 0.0_f32;
            let sample_count = samples.clamp(2, 64);
            for sample in 0..sample_count {
                let t = ctx.time + (phase + angle * sample as f64 / (sample_count - 1) as f64) / 360.0 / ctx.fps();
                let Some(params) = host.params_at(t) else { continue };
                if !motion_curve_params_finite(&params) { continue; }
                let mut image = Image::new(b.img.width, b.img.height);
                effectcraft_raster::composite_warp(
                    &mut image,
                    &b.img,
                    &motion_curve_matrix(&params, &b),
                    &effectcraft_raster::WarpOpts::default(),
                );
                accum.data.iter_mut().zip(&image.data).for_each(|(dst, src)| (0..4).for_each(|c| dst[c] += src[c]));
                count += 1.0;
            }
            if count > 0.0 {
                accum.data.iter_mut().for_each(|pixel| pixel.iter_mut().for_each(|channel| *channel /= count));
                b.img = accum;
                return b;
            }
        }
    }
    if !motion_curve_params_finite(ctx.params) { return b; }
    let mut out = Image::new(b.img.width, b.img.height);
    effectcraft_raster::composite_warp(&mut out, &b.img, &motion_curve_matrix(ctx.params, &b), &effectcraft_raster::WarpOpts::default());
    b.img = out;
    b
}

fn transform(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let anchor = b.to_px(ctx.params.v2("anchor"));
    let pos = b.to_px(ctx.params.v2("position"));
    let uniform = ctx.params.b("uniform");
    let sh = ctx.params.f("scaleHeight");
    let sw = if uniform { sh } else { ctx.params.f("scaleWidth") };
    let skew = ctx.params.f("skew");
    let skew_axis = ctx.params.f("skewAxis");
    let rot = ctx.params.f("rotation");
    let opacity = ctx.params.f("opacity") as f32 / 100.0;
    let sampling = if ctx.params.e("sampling") == 1 { effectcraft_raster::Sampling::Bicubic } else { effectcraft_raster::Sampling::Bilinear };
    // Motion blur: the transform at Samples instants across the shutter, averaged (the
    // transform at other times comes from the host; without one the frame renders sharp).
    if let Some((angle, phase, n)) = transform_shutter(ctx)
        && let Some(host) = ctx.env.host
    {
        let fd = 1.0 / ctx.fps();
        let mut acc = Image::new(b.img.width, b.img.height);
        let mut count = 0.0f32;
        for k in 0..n {
            let t = ctx.time + (phase + angle * k as f64 / (n - 1) as f64) / 360.0 * fd;
            let Some(pr) = host.params_at(t) else { continue };
            let mut one = Image::new(b.img.width, b.img.height);
            let op = pr.f("opacity") as f32 / 100.0;
            effectcraft_raster::composite_warp(
                &mut one,
                &b.img,
                &transform_matrix(&pr, &b),
                &effectcraft_raster::WarpOpts { opacity: op, sampling, ..Default::default() },
            );
            acc.data.iter_mut().zip(&one.data).for_each(|(a, o)| (0..4).for_each(|c| a[c] += o[c]));
            count += 1.0;
        }
        if count > 0.0 {
            acc.data.iter_mut().for_each(|p| p.iter_mut().for_each(|v| *v /= count));
            b.img = acc;
            return b;
        }
    }
    let m = Mat3::translate(vec2(pos.0, pos.1))
        * Mat3::rotate_deg(rot)
        * Mat3::skew_deg(skew, skew_axis)
        * Mat3::scale(vec2(sw / 100.0, sh / 100.0))
        * Mat3::translate(vec2(-anchor.0, -anchor.1));
    let mut out = Image::new(b.img.width, b.img.height);
    effectcraft_raster::composite_warp(&mut out, &b.img, &m, &effectcraft_raster::WarpOpts { opacity, sampling, ..Default::default() });
    b.img = out;
    b
}

fn corner_pin(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (w, h) = (b.layer_w(ctx), b.layer_h(ctx));
    let q = ["ul", "ur", "lr", "ll"].map(|k| {
        let p = b.to_px(ctx.params.v2(k));
        vec2(p.0, p.1)
    });
    let quad = Mat3::square_to_quad(q);
    let o = b.to_px([0.0, 0.0]);
    let m = quad * Mat3::scale(vec2(1.0 / (w * b.scale), 1.0 / (h * b.scale))) * Mat3::translate(vec2(-o.0, -o.1));
    let mut out = Image::new(b.img.width, b.img.height);
    effectcraft_raster::composite_warp(&mut out, &b.img, &m, &Default::default());
    b.img = out;
    b
}

impl Buf {
    fn layer_w(&self, ctx: &EffectCtx) -> f64 {
        ctx.layer_size[0].max(1.0)
    }
    fn layer_h(&self, ctx: &EffectCtx) -> f64 {
        ctx.layer_size[1].max(1.0)
    }
}

pub fn specs() -> Vec<EffectSpec> {
    let pt = |x, y| Value::Vec2([x, y]);
    vec![
        spec(
            "ec.distort.twirl",
            "Twirl",
            vec![
                p("angle", "Angle", num(0.0), ParamUi::Angle),
                p("radius", "Twirl Radius", num(30.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("center", "Twirl Center", pt(0.5, 0.5), ParamUi::Point),
            ],
            twirl,
        ),
        spec(
            "ec.distort.spherize",
            "Spherize",
            vec![p("radius", "Radius", num(0.0), slider(0.0, 2500.0, 0.0, 2500.0, 1)), p("center", "Center of Sphere", pt(0.5, 0.5), ParamUi::Point)],
            spherize,
        ),
        spec(
            "ec.distort.bulge",
            "Bulge",
            vec![
                p("hradius", "Horizontal Radius", num(50.0), slider(0.0, 4000.0, 0.0, 1000.0, 1)),
                p("vradius", "Vertical Radius", num(50.0), slider(0.0, 4000.0, 0.0, 1000.0, 1)),
                p("center", "Bulge Center", pt(0.5, 0.5), ParamUi::Point),
                p("height", "Bulge Height", num(1.0), slider(-4.0, 4.0, -4.0, 4.0, 2)),
                p("taperRadius", "Taper Radius", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("pinAllEdges", "Pin All Edges", Value::Bool(false), ParamUi::Checkbox),
            ],
            bulge,
        ),
        spec(
            "ec.distort.wavewarp",
            "Wave Warp",
            vec![
                p("waveType", "Wave Type", Value::Enum(0), popup(&WAVE_TYPES)),
                p("height", "Wave Height", num(10.0), slider(-1000.0, 1000.0, -100.0, 100.0, 0)),
                p("width", "Wave Width", num(40.0), slider(1.0, 10000.0, 1.0, 500.0, 0)),
                p("direction", "Direction", num(90.0), ParamUi::Angle),
                p("speed", "Wave Speed", num(1.0), slider(-100.0, 100.0, -5.0, 5.0, 1)),
                p("pinning", "Pinning", Value::Enum(0), popup(&WAVE_PINNING)),
                p("phase", "Phase", num(0.0), ParamUi::Angle),
            ],
            wave_warp,
        ),
        spec(
            "ec.distort.ripple",
            "Ripple",
            vec![
                p("radius", "Radius", num(50.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("center", "Center of Ripple", pt(0.5, 0.5), ParamUi::Point),
                p("conversion", "Type of Conversion", Value::Enum(0), popup(&["Asymmetric", "Symmetric"])),
                p("speed", "Wave Speed", num(1.0), slider(-15.0, 15.0, -5.0, 5.0, 1)),
                p("waveWidth", "Wave Width", num(20.0), slider(1.0, 1000.0, 1.0, 100.0, 1)),
                p("waveHeight", "Wave Height", num(10.0), slider(0.0, 400.0, 0.0, 100.0, 1)),
                p("phase", "Ripple Phase", num(0.0), ParamUi::Angle),
            ],
            ripple,
        ),
        spec(
            "ec.distort.mirror",
            "Mirror",
            vec![p("center", "Reflection Center", pt(0.5, 0.5), ParamUi::Point), p("angle", "Reflection Angle", num(0.0), ParamUi::Angle)],
            mirror,
        ),
        spec(
            "ec.distort.offset",
            "Offset",
            vec![p("shift", "Shift Center To", pt(0.5, 0.5), ParamUi::Point), p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1))],
            offset,
        ),
        spec(
            "ec.distort.polar",
            "Polar Coordinates",
            vec![
                p("interpolation", "Interpolation", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("conversion", "Type of Conversion", Value::Enum(0), popup(&["Rect to Polar", "Polar to Rect"])),
            ],
            polar,
        ),
        spec(
            "ec.distort.motiontransformblur",
            "Motion Transform Blur",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("shift", "Shift", pt(0.0, 0.0), ParamUi::Point),
                p("zDist", "Z-Dist (Scale %)", num(100.0), slider(-1000.0, 1000.0, -500.0, 500.0, 1)),
                p("rotate", "Rotate", num(0.0), ParamUi::Angle),
                p("useCompositionShutterAngle", "Use Composition's Shutter Angle", Value::Bool(false), ParamUi::Checkbox),
                p("shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 720.0, 0.0, 360.0, 1)),
                p("samples", "Samples", num(16.0), slider(2.0, 64.0, 2.0, 32.0, 0)),
            ],
            motion_curve_transform,
        ),
        spec(
            "ec.distort.transform",
            "Transform",
            vec![
                p("anchor", "Anchor Point", pt(0.5, 0.5), ParamUi::Point),
                p("position", "Position", pt(0.5, 0.5), ParamUi::Point),
                p("uniform", "Uniform Scale", Value::Bool(true), ParamUi::Checkbox),
                p("scaleHeight", "Scale Height", num(100.0), slider(-10000.0, 10000.0, 0.0, 600.0, 1)),
                p("scaleWidth", "Scale Width", num(100.0), slider(-10000.0, 10000.0, 0.0, 600.0, 1)),
                p("skew", "Skew", num(0.0), slider(-70.0, 70.0, -70.0, 70.0, 1)),
                p("skewAxis", "Skew Axis", num(0.0), ParamUi::Angle),
                p("rotation", "Rotation", num(0.0), ParamUi::Angle),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("useCompositionShutterAngle", "Use Composition's Shutter Angle", Value::Bool(true), ParamUi::Checkbox),
                p("shutterAngle", "Shutter Angle", num(0.0), slider(0.0, 720.0, 0.0, 360.0, 1)),
                p("sampling", "Sampling", Value::Enum(0), popup(&["Bilinear", "Bicubic"])),
            ],
            transform,
        ),
        spec(
            "ec.distort.cornerpin",
            "Corner Pin",
            vec![
                p("ul", "Upper Left", pt(0.0, 0.0), ParamUi::Point),
                p("ur", "Upper Right", pt(1.0, 0.0), ParamUi::Point),
                p("ll", "Lower Left", pt(0.0, 1.0), ParamUi::Point),
                p("lr", "Lower Right", pt(1.0, 1.0), ParamUi::Point),
            ],
            corner_pin,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Params;

    fn img(w: u32, h: u32) -> Image {
        crate::util::gen_image(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, ((x * 7 + y * 3) % 11) as f32 / 11.0, 1.0])
    }

    /// Run with Point defaults scaled to the layer (like `instantiate`).
    fn run(id: &str, set: &[(&str, Value)], im: Image) -> Buf {
        let s = crate::find(id).unwrap();
        let ls = [im.width as f64, im.height as f64];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, ls))).collect() };
        for (k, v) in set {
            assert!(params.values.contains_key(*k), "{id}: unknown param {k}");
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx = EffectCtx { params: &params, time: 0.0, layer_size: ls, seed: 3, adjustment: false, env: Default::default() };
        crate::apply(s, &ctx, Buf { img: im, offset: [0.0, 0.0], scale: 1.0 })
    }

    fn differs(a: Px4, b: Px4) -> bool {
        (0..4).any(|i| (a[i] - b[i]).abs() > 1e-4)
    }
    type Px4 = [f32; 4];

    #[test]
    fn twirl_radius_is_a_percentage_of_the_larger_dimension() {
        // 60×20 layer, radius 50 → 30 px: a pixel 20 px right of the centre is twirled (the old
        // smaller-dimension rule gave 5 px), corners (> 30 px away) are not.
        let im = img(60, 20);
        let o = run("ec.distort.twirl", &[("angle", num(90.0)), ("radius", num(50.0))], im.clone());
        assert!(differs(o.img.get(50, 10), im.get(50, 10)));
        assert_eq!(o.img.get(0, 0), im.get(0, 0));
    }

    #[test]
    fn ripple_radius_reaches_the_edge_at_100_and_conversion_matters() {
        let im = img(40, 40);
        let base = [("radius", num(100.0)), ("waveHeight", num(4.0)), ("waveWidth", num(7.0))];
        let asym = run("ec.distort.ripple", &base, im.clone());
        let mut sym_set = base.to_vec();
        sym_set.push(("conversion", Value::Enum(1)));
        let sym = run("ec.distort.ripple", &sym_set, im.clone());
        assert!(asym.img.data.iter().zip(&sym.img.data).any(|(a, b)| differs(*a, *b)));
        // Corners lie beyond radius 100 % (half the layer) and stay put.
        assert_eq!(asym.img.get(0, 0), im.get(0, 0));
    }

    #[test]
    fn bulge_taper_and_pin_all_edges() {
        let im = img(40, 40);
        let big = [("hradius", num(40.0)), ("vradius", num(40.0)), ("height", num(2.0))];
        let free = run("ec.distort.bulge", &big, im.clone());
        let mut pinned_set = big.to_vec();
        pinned_set.push(("pinAllEdges", Value::Bool(true)));
        let pinned = run("ec.distort.bulge", &pinned_set, im.clone());
        assert!(differs(free.img.get(20, 0), im.get(20, 0)));
        assert!(!differs(pinned.img.get(20, 0), im.get(20, 0)));
        assert!(!differs(pinned.img.get(0, 20), im.get(0, 20)));
        let mut taper_set = big.to_vec();
        taper_set.push(("taperRadius", num(100.0)));
        let tapered = run("ec.distort.bulge", &taper_set, im.clone());
        assert!(differs(tapered.img.get(30, 20), free.img.get(30, 20)));
    }

    #[test]
    fn wave_warp_shapes_stay_in_range_and_pinning_holds_the_edge() {
        for k in 0..WAVE_TYPES.len() as u32 {
            for i in 0..200 {
                let v = wave_shape(k, i as f64 * 0.137 - 5.0, 9);
                assert!((-1.0001..=1.0001).contains(&v), "{} {v}", WAVE_TYPES[k as usize]);
            }
        }
        let im = img(40, 30);
        // Direction 0° → horizontal displacement; Left Edge pinning keeps the left column.
        let set = [("direction", num(0.0)), ("height", num(5.0)), ("pinning", Value::Enum(3))];
        let o = run("ec.distort.wavewarp", &set, im.clone());
        let pad = ((o.img.width - im.width) / 2) as i64;
        for y in 0..30 {
            let (a, b) = (o.img.get(pad, y + pad), im.get(0, y));
            assert!((0..4).all(|i| (a[i] - b[i]).abs() < 0.01), "row {y}: {a:?} vs {b:?}");
        }
        let free = run("ec.distort.wavewarp", &[("direction", num(0.0)), ("height", num(5.0))], im);
        assert!(free.img.data.iter().zip(&o.img.data).any(|(a, b)| differs(*a, *b)));
    }

    #[test]
    fn transform_sampling_bicubic_differs_from_bilinear() {
        let im = img(32, 32);
        let a = run("ec.distort.transform", &[("rotation", num(17.0))], im.clone());
        let b = run("ec.distort.transform", &[("rotation", num(17.0)), ("sampling", Value::Enum(1))], im);
        assert!(a.img.data.iter().zip(&b.img.data).any(|(p, q)| differs(*p, *q)));
    }

    #[test]
    fn transform_shutter_angle_motion_blur() {
        /// Position moves right 200 px per second.
        struct Moving;
        impl crate::EffectHost for Moving {
            fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn params_at(&self, t: f64) -> Option<Params> {
                let s = crate::find("ec.distort.transform").unwrap();
                let mut p = Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [40.0, 20.0]))).collect() };
                p.values.insert("position".into(), Value::Vec2([20.0 + 200.0 * t, 10.0]));
                Some(p)
            }
        }
        let mut sq = Image::new(40, 20);
        for y in 5..15 {
            for x in 15..25 {
                sq.set(x, y, [1.0, 1.0, 1.0, 1.0]);
            }
        }
        let render = |vals: &[(&str, Value)], shutter: Option<(f64, f64, u32)>| {
            let env = crate::EffectEnv { host: Some(&Moving), frame_rate: 10.0, shutter, ..Default::default() };
            crate::run_fx("ec.distort.transform", vals, sq.clone(), 0.0, env).img
        };
        let partial = |i: &Image| i.data.iter().filter(|p| p[3] > 0.02 && p[3] < 0.98).count();
        // Use Composition Shutter Angle with the comp / layer motion blur off: sharp.
        assert_eq!(partial(&render(&[], None)), 0);
        // With the comp shutter (180°): smeared along x over 10 px.
        let comp = render(&[], Some((180.0, -90.0, 16)));
        assert!(partial(&comp) > 50, "{}", partial(&comp));
        // Own Shutter Angle.
        let own = render(&[("useCompositionShutterAngle", Value::Bool(false)), ("shutterAngle", num(360.0))], None);
        assert!(partial(&own) > partial(&comp));
        assert_eq!(partial(&render(&[("useCompositionShutterAngle", Value::Bool(false))], None)), 0, "0 deg: sharp");
    }
}
