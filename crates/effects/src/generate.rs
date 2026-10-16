//! Generate and Noise effects that synthesise images.

use effectcraft_color::{BlendMode, blend_pixel};
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Px;
use rayon::prelude::*;

use crate::{Buf, EffectCtx, EffectSpec, col, num, p, popup, slider};

/// Checkerboard / Grid "Size From" options.
pub(crate) const SIZE_FROM: [&str; 3] = ["Corner Point", "Width Slider", "Width & Height Sliders"];

fn spec(id: &'static str, name: &'static str, category: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category, params, render, gpu: false, float: true }
}

/// Blend a generated straight colour `g` (with alpha `ga`) over the original pixel.
fn put(px: &mut Px, g: [f32; 3], ga: f32, blend_orig: f32) {
    let g2 = [g[0] * ga, g[1] * ga, g[2] * ga, ga];
    let k = 1.0 - blend_orig;
    for c in 0..4 {
        px[c] = px[c] * blend_orig + g2[c] * k;
    }
}

fn gradient_ramp(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let s = b.to_px(ctx.params.v2("start"));
    let e = b.to_px(ctx.params.v2("end"));
    let c0 = ctx.params.color("startColor");
    let c1 = ctx.params.color("endColor");
    let radial = ctx.params.e("shape") == 1;
    let blend = ctx.params.f("blend") as f32 / 100.0;
    let scatter = ctx.params.f("scatter") as f32 / 512.0;
    let (dx, dy) = (e.0 - s.0, e.1 - s.1);
    let len2 = (dx * dx + dy * dy).max(1e-9);
    let seed = ctx.seed;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (vx, vy) = (x as f64 + 0.5 - s.0, y as f64 + 0.5 - s.1);
            let mut t = if radial { ((vx * vx + vy * vy) / len2).sqrt() } else { (vx * dx + vy * dy) / len2 } as f32;
            if scatter > 0.0 {
                t += (effectcraft_raster::hash_noise(x as u32, y as u32, seed) - 0.5) * scatter;
            }
            let t = t.clamp(0.0, 1.0);
            let c = [c0[0] + (c1[0] - c0[0]) * t, c0[1] + (c1[1] - c0[1]) * t, c0[2] + (c1[2] - c0[2]) * t];
            // The ramp fills only where the layer has alpha (#1063), so it keeps the source alpha.
            let a = px[3];
            put(px, c, a, blend);
        }
    });
    b
}

/// Blending Mode options of the shape generators (Checkerboard, Grid, Circle): "None" shows the
/// generated pattern only; the others composite it onto the layer like layer blending modes.
pub(crate) const GEN_BLEND_MODES: [&str; 20] = [
    "None",
    "Normal",
    "Add",
    "Multiply",
    "Screen",
    "Overlay",
    "Soft Light",
    "Hard Light",
    "Color Dodge",
    "Color Burn",
    "Darken",
    "Lighten",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
    "Stencil Alpha",
    "Silhouette Alpha",
];

pub fn gen_mode(i: u32) -> Option<BlendMode> {
    Some(match i {
        0 => return None,
        2 => BlendMode::Add,
        3 => BlendMode::Multiply,
        4 => BlendMode::Screen,
        5 => BlendMode::Overlay,
        6 => BlendMode::SoftLight,
        7 => BlendMode::HardLight,
        8 => BlendMode::ColorDodge,
        9 => BlendMode::ColorBurn,
        10 => BlendMode::Darken,
        11 => BlendMode::Lighten,
        12 => BlendMode::Difference,
        13 => BlendMode::Exclusion,
        14 => BlendMode::Hue,
        15 => BlendMode::Saturation,
        16 => BlendMode::Color,
        17 => BlendMode::Luminosity,
        18 => BlendMode::StencilAlpha,
        19 => BlendMode::SilhouetteAlpha,
        _ => BlendMode::Normal,
    })
}

/// Composite a generated premultiplied pixel `g` onto the original `o` with blending mode
/// index `mode` (into [`GEN_BLEND_MODES`]).
#[inline]
pub(crate) fn gen_blend(o: Px, g: Px, mode: u32) -> Px {
    match gen_mode(mode) {
        None => g,
        Some(m) => blend_pixel(m, o, g, 0.5),
    }
}

fn four_color(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pts: Vec<((f64, f64), [f32; 4])> =
        (1..=4).map(|i| (b.to_px(ctx.params.v2(&format!("positionsColors/point{i}"))), ctx.params.color(&format!("positionsColors/color{i}")))).collect();
    let blend = ctx.params.f("blend").max(1.0);
    let jitter = (ctx.params.f("jitter") / 100.0).clamp(0.0, 1.0) as f32;
    let op = ctx.params.f("opacity") as f32 / 100.0;
    let mode = ctx.params.e("blendingMode");
    let seed = ctx.seed;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let mut acc = [0.0f64; 3];
            let mut wsum = 0.0;
            for ((qx, qy), c) in &pts {
                let d2 = (x as f64 + 0.5 - qx).powi(2) + (y as f64 + 0.5 - qy).powi(2);
                let w = 1.0 / (d2 / (blend * 100.0) + 1e-6).powf(1.5);
                for i in 0..3 {
                    acc[i] += c[i] as f64 * w;
                }
                wsum += w;
            }
            let mut c = acc.map(|v| (v / wsum) as f32);
            if jitter > 0.0 {
                // Dither against banding: a few code values of noise.
                let n = (effectcraft_raster::hash_noise(x as u32, y as u32, seed) - 0.5) * jitter * (8.0 / 255.0);
                c = c.map(|v| v + n);
            }
            *px = gen_blend(*px, [c[0] * op, c[1] * op, c[2] * op, op], mode);
        }
    });
    b
}

/// Cell size of Checkerboard / Grid from Size From (Corner Point, Width Slider, Width &
/// Height Sliders), in pixels.
fn cell_size(ctx: &EffectCtx, b: &Buf, anchor: (f64, f64)) -> (f64, f64) {
    let (w, h) = match ctx.params.e("sizeFrom") {
        0 => {
            let c = b.to_px(ctx.params.v2("corner"));
            ((c.0 - anchor.0).abs(), (c.1 - anchor.1).abs())
        }
        1 => (ctx.params.f("width") * b.scale, ctx.params.f("width") * b.scale),
        _ => (ctx.params.f("width") * b.scale, ctx.params.f("height") * b.scale),
    };
    (w.max(1.0), h.max(1.0))
}

fn checkerboard(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let anchor = b.to_px(ctx.params.v2("anchor"));
    let (w, h) = cell_size(ctx, &b, anchor);
    // Feather softens the transitions; 0 keeps a half-pixel antialiased edge.
    let fw = (ctx.params.f("feather/featherWidth") * b.scale).max(1.0);
    let fh = (ctx.params.f("feather/featherHeight") * b.scale).max(1.0);
    let c = ctx.params.color("color");
    let op = ctx.params.f("opacity") as f32 / 100.0;
    let mode = ctx.params.e("blendingMode");
    // Signed distance (in feather units) to the nearest cell edge, positive in even cells.
    let axis = |p: f64, size: f64, f: f64| -> f64 {
        let u = p / size;
        let fr = u - u.floor();
        let d = fr.min(1.0 - fr) * size;
        let s = if (u.floor() as i64).rem_euclid(2) == 0 { 1.0 } else { -1.0 };
        s * (d / (f * 0.5)).min(1.0)
    };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let sx = axis(x as f64 + 0.5 - anchor.0, w, fw);
            let sy = axis(y as f64 + 0.5 - anchor.1, h, fh);
            let cov = ((0.5 + 0.5 * sx * sy) as f32).clamp(0.0, 1.0) * op;
            *px = gen_blend(*px, [c[0] * cov, c[1] * cov, c[2] * cov, cov], mode);
        }
    });
    b
}

fn grid(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let anchor = b.to_px(ctx.params.v2("anchor"));
    let (w, h) = cell_size(ctx, &b, anchor);
    let border = (ctx.params.f("border") * b.scale).max(0.0);
    let fw = (ctx.params.f("feather/featherWidth") * b.scale).max(1.0);
    let fh = (ctx.params.f("feather/featherHeight") * b.scale).max(1.0);
    let invert = ctx.params.b("invertGrid");
    let c = ctx.params.color("color");
    let op = ctx.params.f("opacity") as f32 / 100.0;
    let mode = ctx.params.e("blendingMode");
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let gx = (x as f64 + 0.5 - anchor.0).rem_euclid(w);
            let gy = (y as f64 + 0.5 - anchor.1).rem_euclid(h);
            let (dx, dy) = (gx.min(w - gx), gy.min(h - gy));
            let cx = ((border / 2.0 - dx) / fw + 0.5).clamp(0.0, 1.0);
            let cy = ((border / 2.0 - dy) / fh + 0.5).clamp(0.0, 1.0);
            let mut cov = if border <= 0.0 { 0.0 } else { cx.max(cy) as f32 };
            if invert {
                cov = 1.0 - cov;
            }
            let a = cov * op;
            *px = gen_blend(*px, [c[0] * a, c[1] * a, c[2] * a, a], mode);
        }
    });
    b
}

/// Circle Edge options.
pub(crate) const CIRCLE_EDGES: [&str; 5] = ["None", "Edge Radius", "Thickness", "Thickness * Radius", "Thickness & Feather * Radius"];

fn circle(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = b.to_px(ctx.params.v2("center"));
    let r = ctx.params.f("radius") * b.scale;
    let thick = ctx.params.f("thickness").max(0.0);
    let mut f_out = ctx.params.f("feather/feather").max(0.0) * b.scale;
    let mut f_in = ctx.params.f("feather/featherInner").max(0.0) * b.scale;
    // Inner radius of the ring (None = solid disk).
    let inner = match ctx.params.e("edge") {
        1 => Some(ctx.params.f("edgeRadius") * b.scale),
        2 => Some(r - thick * b.scale),
        3 => Some(r - thick / 100.0 * r),
        4 => {
            f_out = ctx.params.f("feather/feather").max(0.0) / 100.0 * r;
            f_in = ctx.params.f("feather/featherInner").max(0.0) / 100.0 * r;
            Some(r - thick / 100.0 * r)
        }
        _ => None,
    };
    let (ro, ri) = match inner {
        Some(i) => (r.max(i), Some(r.min(i).max(0.0))),
        None => (r, None),
    };
    let (f_out, f_in) = (f_out.max(0.5), f_in.max(0.5));
    let color = ctx.params.color("color");
    let op = ctx.params.f("opacity") as f32 / 100.0;
    let invert = ctx.params.b("invert");
    let mode = ctx.params.e("blendingMode");
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let d = ((x as f64 + 0.5 - c.0).powi(2) + (y as f64 + 0.5 - c.1).powi(2)).sqrt();
            let mut cov = ((ro - d) / f_out + 0.5).clamp(0.0, 1.0) as f32;
            if let Some(ri) = ri {
                cov *= ((d - ri) / f_in + 0.5).clamp(0.0, 1.0) as f32;
            }
            if invert {
                cov = 1.0 - cov;
            }
            let a = cov * op;
            *px = gen_blend(*px, [color[0] * a, color[1] * a, color[2] * a, a], mode);
        }
    });
    b
}

// ---- value noise / fBm (our own lattice noise with quintic fade) ----

#[inline]
pub(crate) fn lattice(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    let mut h =
        (ix as u32).wrapping_mul(0x27d4_eb2d) ^ (iy as u32).wrapping_mul(0x1656_67b1) ^ (iz as u32).wrapping_mul(0x9e37_79b9) ^ seed.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

#[inline]
pub(crate) fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// 3D value noise in 0..1 (z = evolution).
pub fn value_noise(x: f32, y: f32, z: f32, seed: u32) -> f32 {
    let (x0, y0, z0) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (fade(x - x0), fade(y - y0), fade(z - z0));
    let (ix, iy, iz) = (x0 as i32, y0 as i32, z0 as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx: i32, dy: i32, dz: i32| lattice(ix + dx, iy + dy, iz + dz, seed);
    let a = l(l(c(0, 0, 0), c(1, 0, 0), fx), l(c(0, 1, 0), c(1, 1, 0), fx), fy);
    let b = l(l(c(0, 0, 1), c(1, 0, 1), fx), l(c(0, 1, 1), c(1, 1, 1), fx), fy);
    l(a, b, fz)
}

// Fractal Noise lives in `noise3` (with Turbulent Noise).

pub fn specs() -> Vec<EffectSpec> {
    let pt = |x, y| Value::Vec2([x, y]);
    vec![
        spec(
            "ec.generate.gradientramp",
            "Gradient Ramp",
            "Generate",
            vec![
                p("start", "Start of Ramp", pt(0.5, 0.0), ParamUi::Point),
                p("startColor", "Start Color", col(0.0, 0.0, 0.0), ParamUi::Color),
                p("end", "End of Ramp", pt(0.5, 1.0), ParamUi::Point),
                p("endColor", "End Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("shape", "Ramp Shape", Value::Enum(0), popup(&["Linear Ramp", "Radial Ramp"])),
                p("scatter", "Ramp Scatter", num(0.0), slider(0.0, 512.0, 0.0, 512.0, 1)),
                p("blend", "Blend With Original", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            gradient_ramp,
        ),
        spec(
            "ec.generate.fourcolor",
            "4-Color Gradient",
            "Generate",
            vec![
                p("positionsColors/point1", "Point 1", pt(0.1, 0.1), ParamUi::Point),
                p("positionsColors/color1", "Color 1", col(1.0, 1.0, 0.0), ParamUi::Color),
                p("positionsColors/point2", "Point 2", pt(0.9, 0.1), ParamUi::Point),
                p("positionsColors/color2", "Color 2", col(0.0, 1.0, 0.0), ParamUi::Color),
                p("positionsColors/point3", "Point 3", pt(0.1, 0.9), ParamUi::Point),
                p("positionsColors/color3", "Color 3", col(1.0, 0.0, 1.0), ParamUi::Color),
                p("positionsColors/point4", "Point 4", pt(0.9, 0.9), ParamUi::Point),
                p("positionsColors/color4", "Color 4", col(0.0, 0.0, 1.0), ParamUi::Color),
                p("blend", "Blend", num(100.0), slider(1.0, 1000.0, 1.0, 1000.0, 1)),
                p("jitter", "Jitter", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&GEN_BLEND_MODES[..18])),
            ],
            four_color,
        ),
        spec(
            "ec.generate.checkerboard",
            "Checkerboard",
            "Generate",
            vec![
                p("anchor", "Anchor", pt(0.0, 0.0), ParamUi::Point),
                p("sizeFrom", "Size From", Value::Enum(1), popup(&SIZE_FROM)),
                p("corner", "Corner", pt(0.1, 0.1), ParamUi::Point),
                p("width", "Width", num(64.0), slider(1.0, 4000.0, 1.0, 400.0, 1)),
                p("height", "Height", num(64.0), slider(1.0, 4000.0, 1.0, 400.0, 1)),
                p("feather/featherWidth", "Width", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 1)),
                p("feather/featherHeight", "Height", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 1)),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&GEN_BLEND_MODES)),
            ],
            checkerboard,
        ),
        spec(
            "ec.generate.grid",
            "Grid",
            "Generate",
            vec![
                p("anchor", "Anchor", pt(0.0, 0.0), ParamUi::Point),
                p("sizeFrom", "Size From", Value::Enum(1), popup(&SIZE_FROM)),
                p("corner", "Corner", pt(0.1, 0.1), ParamUi::Point),
                p("width", "Width", num(64.0), slider(1.0, 4000.0, 1.0, 400.0, 1)),
                p("height", "Height", num(64.0), slider(1.0, 4000.0, 1.0, 400.0, 1)),
                p("border", "Border", num(2.0), slider(0.0, 400.0, 0.0, 40.0, 1)),
                p("feather/featherWidth", "Width", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 1)),
                p("feather/featherHeight", "Height", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 1)),
                p("invertGrid", "Invert Grid", Value::Bool(false), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&GEN_BLEND_MODES)),
            ],
            grid,
        ),
        spec(
            "ec.generate.circle",
            "Circle",
            "Generate",
            vec![
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("radius", "Radius", num(75.0), slider(0.0, 4000.0, 0.0, 1000.0, 1)),
                p("edge", "Edge", Value::Enum(0), popup(&CIRCLE_EDGES)),
                p("edgeRadius", "Edge Radius", num(0.0), slider(0.0, 4000.0, 0.0, 1000.0, 1)),
                p("thickness", "Thickness", num(10.0), slider(0.0, 4000.0, 0.0, 100.0, 1)),
                p("feather/feather", "Feather Outer Edge", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("feather/featherInner", "Feather Inner Edge", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
                p("invert", "Invert Circle", Value::Bool(false), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("opacity", "Opacity", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("blendingMode", "Blending Mode", Value::Enum(0), popup(&GEN_BLEND_MODES)),
            ],
            circle,
        ),
    ]
}

#[cfg(test)]
mod generator_tests {
    use super::*;
    use crate::{EffectEnv, run_fx};
    use effectcraft_raster::Image;

    fn red() -> Image {
        Image::filled(32, 32, [1.0, 0.0, 0.0, 1.0])
    }

    #[test]
    fn gradient_ramp_stays_inside_the_layer_alpha() {
        // #1063: a shape's transparent pixels stay transparent; the ramp fills only the shape.
        let mut img = Image::filled(40, 24, [1.0, 0.0, 0.0, 1.0]);
        for (i, p) in img.data.iter_mut().enumerate() {
            let a = if i % 40 < 10 { 0.0 } else { 1.0 };
            *p = p.map(|c| c * a);
        }
        for blend in [0.0, 50.0] {
            let o = run_fx("ec.generate.gradientramp", &[("blend", num(blend))], img.clone(), 0.0, EffectEnv::default());
            for (src, out) in img.data.iter().zip(&o.img.data) {
                assert!((src[3] - out[3]).abs() < 1e-6, "blend {blend}: alpha {} became {}", src[3], out[3]);
                assert!(out[..3].iter().all(|&c| c <= out[3] + 1e-6), "blend {blend}: premultiplied colour above alpha");
            }
            assert!(o.img.data.iter().any(|p| p[3] > 0.99), "blend {blend}: the ramp is drawn inside the shape");
        }
    }

    #[test]
    fn checkerboard_blending_none_shows_pattern_only_and_size_from_width() {
        let set = [("width", num(8.0)), ("height", num(2.0))];
        // None (default): the off squares are transparent; Width Slider makes 8×8 squares.
        let o = run_fx("ec.generate.checkerboard", &set, red(), 0.0, EffectEnv::default());
        assert!(o.img.get(3, 3)[3] > 0.99 && o.img.get(3, 3)[1] > 0.99, "{:?}", o.img.get(3, 3));
        assert!(o.img.get(11, 3)[3] < 0.01);
        assert!(o.img.get(3, 6)[3] > 0.99, "height ignored with Width Slider");
        // Width & Height Sliders: 8×2 cells; Normal keeps the layer under the off squares.
        let o = run_fx(
            "ec.generate.checkerboard",
            &[set[0].clone(), set[1].clone(), ("sizeFrom", Value::Enum(2)), ("blendingMode", Value::Enum(1))],
            red(),
            0.0,
            EffectEnv::default(),
        );
        assert!(o.img.get(3, 2)[0] > 0.99 && o.img.get(3, 2)[1] < 0.01, "second row of cells is off");
        // Corner Point: cell size from the anchor-corner rectangle.
        let o = run_fx("ec.generate.checkerboard", &[("sizeFrom", Value::Enum(0)), ("corner", Value::Vec2([4.0, 4.0]))], red(), 0.0, EffectEnv::default());
        assert!(o.img.get(1, 1)[3] > 0.99 && o.img.get(5, 1)[3] < 0.01);
    }

    #[test]
    fn grid_invert_and_stencil() {
        let set = [("width", num(10.0)), ("border", num(2.0))];
        let o = run_fx("ec.generate.grid", &set, red(), 0.0, EffectEnv::default());
        assert!(o.img.get(0, 5)[3] > 0.9, "line at the anchor");
        assert!(o.img.get(5, 5)[3] < 0.01, "cell interior is empty with None");
        let o = run_fx("ec.generate.grid", &[set[0].clone(), set[1].clone(), ("invertGrid", Value::Bool(true))], red(), 0.0, EffectEnv::default());
        assert!(o.img.get(5, 5)[3] > 0.99 && o.img.get(0, 5)[3] < 0.1);
        // Stencil Alpha: the layer shows only through the grid lines.
        let o = run_fx("ec.generate.grid", &[set[0].clone(), set[1].clone(), ("blendingMode", Value::Enum(18))], red(), 0.0, EffectEnv::default());
        assert!(o.img.get(0, 5)[0] > 0.9 && o.img.get(5, 5)[3] < 0.01);
    }

    #[test]
    fn circle_edge_makes_a_ring() {
        let c = ("center", Value::Vec2([16.0, 16.0]));
        let disk = run_fx("ec.generate.circle", &[c.clone(), ("radius", num(10.0))], red(), 0.0, EffectEnv::default());
        assert!(disk.img.get(16, 16)[3] > 0.99 && disk.img.get(30, 16)[3] < 0.01);
        let ring = run_fx(
            "ec.generate.circle",
            &[c.clone(), ("radius", num(10.0)), ("edge", Value::Enum(2)), ("thickness", num(3.0))],
            red(),
            0.0,
            EffectEnv::default(),
        );
        assert!(ring.img.get(16, 16)[3] < 0.01, "hole in the middle");
        assert!(ring.img.get(24, 16)[3] > 0.99, "{:?}", ring.img.get(24, 16));
        let er =
            run_fx("ec.generate.circle", &[c, ("radius", num(10.0)), ("edge", Value::Enum(1)), ("edgeRadius", num(5.0))], red(), 0.0, EffectEnv::default());
        assert!(er.img.get(16, 16)[3] < 0.01 && er.img.get(23, 16)[3] > 0.99);
    }

    #[test]
    fn four_color_blending_mode_and_opacity() {
        let pts: Vec<(&str, Value)> = (1..=4)
            .map(|i| match i {
                1 => ("positionsColors/point1", Value::Vec2([0.0, 0.0])),
                2 => ("positionsColors/point2", Value::Vec2([32.0, 0.0])),
                3 => ("positionsColors/point3", Value::Vec2([0.0, 32.0])),
                _ => ("positionsColors/point4", Value::Vec2([32.0, 32.0])),
            })
            .collect();
        let mut half = pts.clone();
        half.push(("opacity", num(50.0)));
        // None: the gradient alone at half opacity.
        let o = run_fx("ec.generate.fourcolor", &half, red(), 0.0, EffectEnv::default());
        assert!((o.img.get(16, 16)[3] - 0.5).abs() < 1e-3);
        // Normal: over the opaque layer.
        half.push(("blendingMode", Value::Enum(1)));
        let o = run_fx("ec.generate.fourcolor", &half, red(), 0.0, EffectEnv::default());
        assert!((o.img.get(16, 16)[3] - 1.0).abs() < 1e-3);
    }
}
