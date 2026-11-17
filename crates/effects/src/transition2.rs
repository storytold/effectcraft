//! Transition effects, batch 2: Glass Wipe, Luminance Image Wipe, Serrated Wipe, Line Sweep,
//! Twist Wipe and Warp Dissolve.
//!
//! Written from the public descriptions of these transitions (gradient-driven wipes with glass
//! refraction or warping, jagged "jaws" opening, staggered line sweeps, a ribbon twist that
//! reveals a back side); the maths is our own. Completion 0 leaves the layer untouched;
//! completion 100 leaves it fully transparent (or showing the revealed / back-side layer).

use effectcraft_color::rgb_to_hsl;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::{Plane, fit_layer, gauss_plane, layer_or_self, lerp4, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Transition", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

fn pt(x: f64, y: f64) -> Value {
    Value::Vec2([x, y])
}

fn completion(ctx: &EffectCtx) -> f32 {
    (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0) as f32
}

/// The layer in layer parameter `id` fitted to `b`, if one is chosen.
fn reveal(ctx: &EffectCtx, b: &Buf, id: &str) -> Option<Image> {
    ctx.layer_param(id, true).map(|o| fit_layer(ctx, b, &o, false))
}

const GRAD_PROPS: [&str; 7] = ["Red", "Green", "Blue", "Alpha", "Luminance", "Lightness", "Saturation"];

fn prop_plane(img: &Image, prop: u32) -> Plane {
    Plane::from_image(img, |p| {
        let (c, a) = unpremul(p);
        match prop {
            0 => c[0] * a,
            1 => c[1] * a,
            2 => c[2] * a,
            3 => a,
            4 => effectcraft_color::luminance(c[0], c[1], c[2]) * a,
            5 => rgb_to_hsl(c[0], c[1], c[2]).2 * a,
            _ => rgb_to_hsl(c[0], c[1], c[2]).1 * a,
        }
    })
}

/// Fraction of a pixel with gradient value `g` that has transitioned, for a soft band of
/// half-width `w`: none at completion 0, all at completion 1.
#[inline]
fn wipe_t(g: f32, c: f32, w: f32) -> f32 {
    let w = w.max(1e-3);
    let edge = c * (1.0 + 2.0 * w) - w;
    1.0 - smoothstep(edge - w, edge + w, g)
}

#[inline]
fn grad(pl: &Plane, x: usize, y: usize) -> (f32, f32) {
    let (xi, yi) = (x as i64, y as i64);
    ((pl.get_clamped(xi + 1, yi) - pl.get_clamped(xi - 1, yi)) * 0.5, (pl.get_clamped(xi, yi + 1) - pl.get_clamped(xi, yi - 1)) * 0.5)
}

// ------------------------------------------------------------------ Glass Wipe

fn glass_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx);
    if c <= 0.0 {
        return b;
    }
    let gimg = layer_or_self(ctx, &b, "gradientLayer", true, false);
    let mut g = prop_plane(&gimg, 4);
    let soft = (ctx.params.f("softness") / 100.0) as f32;
    let sm = ctx.params.f("softness") * b.scale * 0.2;
    if sm > 0.05 {
        g = gauss_plane(&g, sm, sm);
    }
    let disp = (ctx.params.f("displacementAmount") * b.scale * 4.0) as f32;
    let rev = reveal(ctx, &b, "layerToReveal");
    let src = b.img.clone();
    let w = soft * 0.5 + 0.01;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let t = wipe_t(g.get(x, y), c, w);
            let band = 4.0 * t * (1.0 - t);
            let (gx, gy) = grad(&g, x, y);
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (ox, oy) = ((gx * disp * band) as f64 * 10.0, (gy * disp * band) as f64 * 10.0);
            let a = if band > 1e-4 { src.sample_bilinear(fx + ox, fy + oy) } else { src.data[y * src.width as usize + x] };
            let r = match &rev {
                Some(r) if band > 1e-4 => r.sample_bilinear(fx - ox, fy - oy),
                Some(r) => r.data[y * r.width as usize + x],
                None => [0.0; 4],
            };
            *px = lerp4(a, r, t);
        }
    });
    b
}

// ------------------------------------------------------------------ Luminance Image Wipe

fn image_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx);
    if c <= 0.0 {
        return b;
    }
    let gimg = layer_or_self(ctx, &b, "layer", true, false);
    let mut g = prop_plane(&gimg, ctx.params.e("property"));
    let bl = ctx.params.f("blur") * b.scale * 0.5;
    if bl > 0.05 {
        g = gauss_plane(&g, bl, bl);
    }
    if ctx.params.b("inverseGradient") {
        g = g.map(|v| 1.0 - v);
    }
    let mut w = (ctx.params.f("borderSoftness") / 100.0) as f32 * 0.5;
    if ctx.params.b("autoSoftness") {
        w = w.max(0.02);
    }
    b.img.data.par_iter_mut().zip(g.data.par_iter()).for_each(|(px, &gv)| {
        let k = 1.0 - wipe_t(gv, c, w);
        *px = px.map(|v| v * k);
    });
    b
}

// ------------------------------------------------------------------ Serrated Wipe

fn jaws(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx) as f64;
    if c <= 0.0 {
        return b;
    }
    let s = b.scale;
    let (cx, cy) = b.to_px(ctx.params.v2("center"));
    let ang = ctx.params.f("direction").to_radians();
    let (sn, cs) = ang.sin_cos();
    let width = (ctx.params.f("width") * s).max(1.0);
    let amp = ctx.params.f("height") / 100.0 * width * 0.5;
    let shape = ctx.params.e("shape");
    // Far enough at completion 1 that both halves (teeth included) have left the buffer.
    let (bw, bh) = (b.img.width as f64, b.img.height as f64);
    let vmax = [(0.0, 0.0), (bw, 0.0), (0.0, bh), (bw, bh)].iter().map(|&(x, y)| (-(x - cx) * sn + (y - cy) * cs).abs()).fold(0.0, f64::max);
    let d = c * (vmax + 2.0 * amp + 2.0);
    // Tooth profile in [-amp, amp] along the cut.
    let profile = move |u: f64| -> f64 {
        let ph = (u / width).rem_euclid(1.0);
        match shape {
            0 => amp * (1.0 - 4.0 * (ph - 0.5).abs()),
            1 => {
                // RoboJaw: square teeth with sloped flanks.
                let t = (1.0 - 4.0 * (ph - 0.5).abs()) * 2.0;
                amp * t.clamp(-1.0, 1.0)
            }
            2 => {
                if ph < 0.5 {
                    amp
                } else {
                    -amp
                }
            }
            _ => amp * (ph * std::f64::consts::TAU).sin(),
        }
    };
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            // u along the cut, v across it.
            let (u, v) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let f = profile(u);
            let back = |vs: f64| {
                let (sx, sy) = (u * cs - vs * sn + cx, u * sn + vs * cs + cy);
                src.sample_bilinear(sx, sy)
            };
            *px = if v + d < f {
                back(v + d)
            } else if v - d >= f {
                back(v - d)
            } else {
                [0.0; 4]
            };
        }
    });
    b
}

// ------------------------------------------------------------------ Line Sweep

fn line_sweep(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx) as f64;
    if c <= 0.0 {
        return b;
    }
    let s = b.scale;
    let ang = ctx.params.f("direction").to_radians();
    let (sn, cs) = ang.sin_cos();
    let thick = (ctx.params.f("thickness") * s).max(1.0);
    let slant = ctx.params.f("slant") * s;
    let flip = ctx.params.b("flipDirection");
    // Projected extents of the buffer along the sweep (u) and across it (v).
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let proj = |x: f64, y: f64| (x * cs + y * sn, -x * sn + y * cs);
    let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (x, y) in corners {
        let (u, v) = proj(x, y);
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
    }
    let nstr = ((v1 - v0) / thick).ceil().max(1.0);
    let total = (u1 - u0) + slant.abs() * nstr + 1.0;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (u, v) = proj(x as f64 + 0.5, y as f64 + 0.5);
            let k = ((v - v0) / thick).floor();
            let k = if flip { nstr - 1.0 - k } else { k };
            let stagger = if slant >= 0.0 { k } else { nstr - 1.0 - k };
            let front = u0 + c * total - slant.abs() * stagger;
            if u < front {
                *px = [0.0; 4];
            }
        }
    });
    b
}

// ------------------------------------------------------------------ Twist Wipe

fn twister(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx) as f64;
    if c <= 0.0 {
        return b;
    }
    let (cx, cy) = b.to_px(ctx.params.v2("center"));
    let ang = ctx.params.f("axis").to_radians();
    let (sn, cs) = ang.sin_cos();
    let shading = ctx.params.b("shading");
    let half = (b.img.width as f64).hypot(b.img.height as f64) * 0.5;
    let src = b.img.clone();
    let back = reveal(ctx, &b, "backside");
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            // u along the twist axis, v across it.
            let (u, v) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let s01 = ((u / half) * 0.5 + 0.5).clamp(0.0, 1.0);
            let th = std::f64::consts::PI * (c * 2.0 - s01).clamp(0.0, 1.0);
            let k = th.cos();
            if k.abs() < 1e-4 {
                *px = [0.0; 4];
                continue;
            }
            let vs = v / k;
            let (sx, sy) = (u * cs - vs * sn + cx, u * sn + vs * cs + cy);
            let mut o: Px = if k > 0.0 {
                src.sample_bilinear(sx, sy)
            } else {
                // Back side: the layer itself is seen from behind (mirrored across the axis); a
                // Backside layer is mapped so it reads upright when fully flipped.
                match &back {
                    Some(bk) => bk.sample_bilinear(u * cs + vs * sn + cx, u * sn - vs * cs + cy),
                    None => src.sample_bilinear(sx, sy),
                }
            };
            if shading {
                let l = (0.4 + 0.6 * k.abs()) as f32;
                o = [o[0] * l, o[1] * l, o[2] * l, o[3]];
            }
            *px = o;
        }
    });
    b
}

// ------------------------------------------------------------------ Warp Dissolve

fn warpomatic(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = completion(ctx);
    if c <= 0.0 {
        return b;
    }
    let rimg = layer_or_self(ctx, &b, "reactorLayer", true, false);
    let mut m = prop_plane(&rimg, 4);
    let sm = ctx.params.f("smoothness") * b.scale * 0.5;
    if sm > 0.05 {
        m = gauss_plane(&m, sm, sm);
    }
    m = match ctx.params.e("reactor") {
        1 => {
            // Contrast differences: distance from the local mean.
            let mean = gauss_plane(&m, 8.0 * b.scale, 8.0 * b.scale);
            m.zip_map(&mean, |a, b| ((a - b).abs() * 4.0).min(1.0))
        }
        2 => {
            // Local differences: gradient magnitude.
            let mut o = Plane::new(m.w, m.h);
            o.data.par_chunks_mut(m.w.max(1)).enumerate().for_each(|(y, row)| {
                for (x, v) in row.iter_mut().enumerate() {
                    let (gx, gy) = grad(&m, x, y);
                    *v = ((gx * gx + gy * gy).sqrt() * 8.0).min(1.0);
                }
            });
            o
        }
        _ => m,
    };
    let amt = (ctx.params.f("warpAmount") * b.scale) as f32 * 20.0;
    let dir = ctx.params.e("warpDirection");
    let span = (ctx.params.f("blendSpan") / 100.0) as f32 * 0.5;
    let rev = reveal(ctx, &b, "layerToReveal");
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let q = wipe_t(m.get(x, y), c, span);
            let (gx, gy) = grad(&m, x, y);
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let (ox, oy) = ((gx * amt * q) as f64, (gy * amt * q) as f64);
            let (ix, iy) = ((gx * amt * (1.0 - q)) as f64, (gy * amt * (1.0 - q)) as f64);
            let (ix, iy) = match dir {
                1 => (-ix, -iy),
                2 => (-iy, ix),
                _ => (ix, iy),
            };
            let out = if q < 1.0 { src.sample_bilinear(fx + ox, fy + oy) } else { [0.0; 4] };
            let inc = match &rev {
                Some(r) if q > 0.0 => r.sample_bilinear(fx + ix, fy + iy),
                _ => [0.0; 4],
            };
            *px = lerp4(out, inc, q);
        }
    });
    b
}

// ------------------------------------------------------------------ specs

pub fn specs() -> Vec<EffectSpec> {
    let done = || p("completion", "Completion", num(0.0), pct());
    vec![
        spec(
            "ec.transition.ccglasswipe",
            "Glass Wipe",
            vec![
                done(),
                p("layerToReveal", "Layer to Reveal", Value::Layer(None), ParamUi::Layer),
                p("gradientLayer", "Gradient Layer", Value::Layer(None), ParamUi::Layer),
                p("softness", "Softness", num(10.0), pct()),
                p("displacementAmount", "Displacement Amount", num(10.0), slider(-100.0, 100.0, -100.0, 100.0, 1)),
            ],
            glass_wipe,
        ),
        spec(
            "ec.transition.ccimagewipe",
            "Luminance Image Wipe",
            vec![
                done(),
                p("borderSoftness", "Border Softness", num(0.0), pct()),
                p("autoSoftness", "Auto Softness", Value::Bool(true), ParamUi::Checkbox),
                p("layer", "Layer", Value::Layer(None), ParamUi::Layer),
                p("property", "Property", Value::Enum(4), popup(&GRAD_PROPS)),
                p("blur", "Blur", num(0.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("inverseGradient", "Inverse Gradient", Value::Bool(false), ParamUi::Checkbox),
            ],
            image_wipe,
        ),
        spec(
            "ec.transition.ccjaws",
            "Serrated Wipe",
            vec![
                done(),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("direction", "Direction", num(0.0), ParamUi::Angle),
                p("height", "Height", num(50.0), pct()),
                p("width", "Width", num(50.0), slider(1.0, 1000.0, 1.0, 200.0, 1)),
                p("shape", "Shape", Value::Enum(0), popup(&["Spikes", "RoboJaw", "Block", "Waves"])),
            ],
            jaws,
        ),
        spec(
            "ec.transition.cclinesweep",
            "Line Sweep",
            vec![
                done(),
                p("direction", "Direction", num(0.0), ParamUi::Angle),
                p("thickness", "Thickness", num(20.0), slider(1.0, 500.0, 1.0, 100.0, 1)),
                p("slant", "Slant", num(20.0), slider(-500.0, 500.0, -100.0, 100.0, 1)),
                p("flipDirection", "Flip Direction", Value::Bool(false), ParamUi::Checkbox),
            ],
            line_sweep,
        ),
        spec(
            "ec.transition.cctwister",
            "Twist Wipe",
            vec![
                done(),
                p("backside", "Backside", Value::Layer(None), ParamUi::Layer),
                p("shading", "Shading", Value::Bool(true), ParamUi::Checkbox),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("axis", "Axis", num(45.0), ParamUi::Angle),
            ],
            twister,
        ),
        spec(
            "ec.transition.ccwarpomatic",
            "Warp Dissolve",
            vec![
                done(),
                p("layerToReveal", "Layer to Reveal", Value::Layer(None), ParamUi::Layer),
                p("reactorLayer", "Reactor Layer", Value::Layer(None), ParamUi::Layer),
                p("reactor", "Reactor", Value::Enum(0), popup(&["Brightness", "Contrast Differences", "Local Differences"])),
                p("smoothness", "Smoothness", num(10.0), slider(0.0, 200.0, 0.0, 100.0, 1)),
                p("warpAmount", "Warp Amount", num(50.0), slider(-1000.0, 1000.0, -200.0, 200.0, 1)),
                p("warpDirection", "Warp Direction", Value::Enum(0), popup(&["Joint", "Opposing", "Twisting"])),
                p("blendSpan", "Blend Span", num(10.0), pct()),
            ],
            warpomatic,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, run_fx};

    fn ramp(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set(x, y, [x as f32 / w as f32, y as f32 / h as f32, 0.3, 1.0]);
            }
        }
        img
    }

    fn run(id: &str, vals: &[(&str, Value)], img: Image) -> Buf {
        run_fx(id, vals, img, 0.0, EffectEnv::default())
    }

    const ALL: [&str; 6] = [
        "ec.transition.ccglasswipe",
        "ec.transition.ccimagewipe",
        "ec.transition.ccjaws",
        "ec.transition.cclinesweep",
        "ec.transition.cctwister",
        "ec.transition.ccwarpomatic",
    ];

    fn centre(id: &str) -> Vec<(&'static str, Value)> {
        match id {
            "ec.transition.ccjaws" => vec![("center", pt(20.0, 16.0)), ("width", num(10.0))],
            "ec.transition.cctwister" => vec![("center", pt(20.0, 16.0))],
            _ => vec![],
        }
    }

    #[test]
    fn zero_completion_is_identity() {
        let img = ramp(40, 32);
        for id in ALL {
            let o = run(id, &centre(id), img.clone());
            assert_eq!(o.img.data, img.data, "{id}");
        }
    }

    #[test]
    fn full_completion_is_transparent() {
        let img = ramp(40, 32);
        for id in ALL {
            if id == "ec.transition.cctwister" {
                continue;
            }
            let mut v = centre(id);
            v.push(("completion", num(100.0)));
            let o = run(id, &v, img.clone());
            assert!(o.img.data.iter().all(|p| p[3].abs() < 1e-5), "{id}");
        }
    }

    #[test]
    fn midway_deterministic_and_partial() {
        let img = ramp(40, 32);
        for id in ALL {
            let mut v = centre(id);
            v.push(("completion", num(50.0)));
            let a = run(id, &v, img.clone());
            let b = run(id, &v, img.clone());
            assert_eq!(a.img.data, b.img.data, "{id}");
            let opaque = a.img.data.iter().filter(|p| p[3] > 0.99).count();
            let clear = a.img.data.iter().filter(|p| p[3] < 0.01).count();
            if id != "ec.transition.cctwister" {
                assert!(opaque > 0 && clear > 0, "{id}: {opaque} {clear}");
            }
            assert_ne!(a.img.data, img.data, "{id}");
        }
    }

    #[test]
    fn twister_full_shows_mirrored_back() {
        let img = ramp(40, 32);
        let o = run(
            "ec.transition.cctwister",
            &[("completion", num(100.0)), ("shading", Value::Bool(false)), ("axis", num(0.0)), ("center", pt(20.0, 16.0))],
            img.clone(),
        );
        // Axis horizontal through the centre: the back is the source flipped vertically.
        for (x, y) in [(5usize, 4usize), (30, 20), (12, 27)] {
            let got = o.img.data[y * 40 + x];
            let want = img.data[(31 - y) * 40 + x];
            assert!((got[1] - want[1]).abs() < 1e-3 && (got[0] - want[0]).abs() < 1e-3, "{x},{y} {got:?} {want:?}");
        }
    }

    #[test]
    fn line_sweep_flip_reverses_stagger() {
        let img = ramp(40, 32);
        let a = run("ec.transition.cclinesweep", &[("completion", num(40.0))], img.clone());
        let b = run("ec.transition.cclinesweep", &[("completion", num(40.0)), ("flipDirection", Value::Bool(true))], img.clone());
        let cleared = |o: &Buf, y: usize| (0..40).filter(|&x| o.img.data[y * 40 + x][3] < 0.01).count();
        // Unflipped: lower stripes trail; flipped: upper stripes trail.
        assert!(cleared(&a, 2) > cleared(&a, 30));
        assert!(cleared(&b, 2) < cleared(&b, 30));
    }

    #[test]
    fn image_wipe_follows_gradient() {
        let img = ramp(40, 32);
        let o = run("ec.transition.ccimagewipe", &[("completion", num(50.0)), ("property", Value::Enum(0))], img.clone());
        // Red ramps left→right: dark (left) side goes first.
        assert!(o.img.data[16 * 40 + 2][3] < 0.01);
        assert!(o.img.data[16 * 40 + 37][3] > 0.99);
    }
}
