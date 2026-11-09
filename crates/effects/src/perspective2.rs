//! Perspective effects, batch 2: Environment Map (reflection mapping of an environment layer
//! onto the layer's luminance/alpha relief) and 3D Glasses (stereo pairs and anaglyphs from a
//! left and right view layer).

use std::f64::consts::PI;

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use crate::util::{Plane, fit_layer, gauss_plane, premul, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, LayerPixels, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Perspective", params, render, gpu: false, float: true }
}

// ---------------------------------------------------------------- Environment Map

/// Environment direction (x right, y down, z towards the viewer) → normalised map coordinates.
fn env_uv(mapping: u32, r: [f64; 3]) -> (f64, f64) {
    match mapping {
        1 => {
            // Probe (mirror ball).
            let m = 2.0 * (r[0] * r[0] + r[1] * r[1] + (r[2] + 1.0).powi(2)).sqrt().max(1e-9);
            (0.5 + r[0] / m, 0.5 + r[1] / m)
        }
        2 => {
            // Vertical cross cube map: 3 faces wide, 4 tall.
            let (ax, ay, az) = (r[0].abs(), r[1].abs(), r[2].abs());
            let (col, row, u, v) = if ax >= ay && ax >= az {
                if r[0] > 0.0 { (2.0, 1.0, -r[2] / ax, r[1] / ax) } else { (0.0, 1.0, r[2] / ax, r[1] / ax) }
            } else if ay >= az {
                if r[1] < 0.0 { (1.0, 0.0, r[0] / ay, -r[2] / ay) } else { (1.0, 2.0, r[0] / ay, r[2] / ay) }
            } else if r[2] > 0.0 {
                (1.0, 1.0, r[0] / az, r[1] / az)
            } else {
                (1.0, 3.0, r[0] / az, -r[1] / az)
            };
            ((col + (u + 1.0) * 0.5) / 3.0, (row + (v + 1.0) * 0.5) / 4.0)
        }
        _ => {
            // Spherical (equirectangular).
            let lon = r[0].atan2(r[2]);
            let lat = r[1].clamp(-1.0, 1.0).asin();
            (0.5 + lon / (2.0 * PI), 0.5 + lat / PI)
        }
    }
}

fn cc_environment(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let env = ctx.layer_param("environment", true).unwrap_or_else(|| LayerPixels { buf: b.clone(), size: ctx.layer_size });
    let env_img = if ctx.params.b("filterEnvironment") {
        let s = 2.0 * env.buf.scale;
        effectcraft_raster::gaussian_blur(&env.buf.img, s, s, true)
    } else {
        env.buf.img.clone()
    };
    let mapping = ctx.params.e("mapping");
    let height = ctx.params.f("height") / 100.0 * 20.0;
    // Relief: luminance inside, rounded off by the alpha edge.
    let relief = Plane::from_image(&b.img, |px| {
        let (c, a) = unpremul(px);
        (luminance(c[0], c[1], c[2]) * 0.5 + 0.5) * a
    });
    let soft = 1.5 * b.scale.max(0.25);
    let hmap = gauss_plane(&relief, soft, soft);
    let (es, eo, esz) = (env.buf.scale, env.buf.offset, env.size);
    let src = b.img.clone();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = src.get(x as i64, y as i64)[3];
            if a <= 0.0 {
                *px = [0.0; 4];
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            let gx = (hmap.get_clamped(xi + 1, yi) - hmap.get_clamped(xi - 1, yi)) as f64 * 0.5 / b.scale.max(1e-6);
            let gy = (hmap.get_clamped(xi, yi + 1) - hmap.get_clamped(xi, yi - 1)) as f64 * 0.5 / b.scale.max(1e-6);
            let n = [-gx * height, -gy * height, 1.0];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = [n[0] / l, n[1] / l, n[2] / l];
            // Reflect the view ray (towards −z) about the normal.
            let d = -n[2];
            let r = [-2.0 * d * n[0], -2.0 * d * n[1], -1.0 - 2.0 * d * n[2]];
            let r = [r[0], r[1], -r[2]];
            let (u, v) = env_uv(mapping, r);
            let ex = u.rem_euclid(1.0) * esz[0] * es + eo[0];
            let ey = v.clamp(0.0, 1.0) * esz[1] * es + eo[1];
            let e = env_img.sample_bilinear_clamped(ex, ey);
            // Straight colour, with nearly transparent texels fading to black continuously
            // (a hard 1e-6 cut-off would turn the float noise of the filtered map into colour).
            let k = 1.0 / e[3].max(1e-3);
            *px = premul([e[0] * k, e[1] * k, e[2] * k], a);
        }
    });
    b
}

// ---------------------------------------------------------------- 3D Glasses

const VIEWS: [&str; 9] = [
    "Stereo Pair (Side by Side)",
    "Over Under",
    "Interlace Upper L Lower R",
    "Red Green LR",
    "Red Blue LR",
    "Balanced Colored Red Blue",
    "Balanced Red Green LR",
    "Balanced Red Blue LR",
    "Difference",
];

fn shift(img: &Image, dx: f64, dy: f64) -> Image {
    if dx == 0.0 && dy == 0.0 {
        return img.clone();
    }
    crate::util::gen_image(img.width, img.height, |x, y| img.sample_bilinear(x as f64 + 0.5 - dx, y as f64 + 0.5 - dy))
}

fn lum(p: Px) -> f32 {
    luminance(p[0], p[1], p[2])
}

fn glasses_3d(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let get = |id: &str| match ctx.layer_param(id, true) {
        Some(o) => fit_layer(ctx, &b, &o, false),
        None => b.img.clone(),
    };
    let (mut l, mut r) = (get("leftView"), get("rightView"));
    if ctx.params.b("leftRightSwap") {
        std::mem::swap(&mut l, &mut r);
    }
    let pct = ctx.params.e("units") == 1;
    let unit = if pct { ctx.layer_size[0] / 100.0 } else { 1.0 } * b.scale;
    let conv = ctx.params.f("sceneConvergence") * unit;
    let va = ctx.params.f("verticalAlignment") * unit;
    let l = shift(&l, conv * 0.5, 0.0);
    let r = shift(&r, -conv * 0.5, va);
    let view = ctx.params.e("view3d");
    let k = (ctx.params.f("balance") / 20.0).clamp(0.0, 1.0) as f32;
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            *px = match view {
                0 => {
                    if fx < w * 0.5 {
                        l.sample_bilinear(fx * 2.0, fy)
                    } else {
                        r.sample_bilinear((fx - w * 0.5) * 2.0, fy)
                    }
                }
                1 => {
                    if fy < h * 0.5 {
                        l.sample_bilinear(fx, fy * 2.0)
                    } else {
                        r.sample_bilinear(fx, (fy - h * 0.5) * 2.0)
                    }
                }
                2 => {
                    if y % 2 == 0 {
                        l.get(x as i64, y as i64)
                    } else {
                        r.get(x as i64, y as i64)
                    }
                }
                _ => {
                    let lp = l.get(x as i64, y as i64);
                    let rp = r.get(x as i64, y as i64);
                    let (lc, la) = unpremul(lp);
                    let (rc, ra) = unpremul(rp);
                    let a = la.max(ra);
                    let (ll, rl) = (lum(premul(lc, 1.0)), lum(premul(rc, 1.0)));
                    let c = match view {
                        3 => [ll, rl, 0.0],
                        4 => [ll, 0.0, rl],
                        5 => [lc[0] + (ll - lc[0]) * k, rc[1], rc[2]],
                        6 => [ll * (1.0 - 0.5 * k) + lc[0] * 0.5 * k, rl, 0.0],
                        7 => [ll * (1.0 - 0.5 * k) + lc[0] * 0.5 * k, 0.0, rl],
                        _ => [(lc[0] - rc[0]).abs(), (lc[1] - rc[1]).abs(), (lc[2] - rc[2]).abs()],
                    };
                    premul(c, a)
                }
            };
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.perspective.ccenvironment",
            "Environment Map",
            vec![
                p("environment", "Environment", Value::Layer(None), ParamUi::Layer),
                p("mapping", "Mapping", Value::Enum(0), popup(&["Spherical", "Probe", "Vertical Cross"])),
                p("filterEnvironment", "Filter Environment", Value::Bool(true), ParamUi::Checkbox),
                p("height", "Height", num(10.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            cc_environment,
        ),
        spec(
            "ec.perspective.3dglasses",
            "3D Glasses",
            vec![
                p("leftView", "Left View", Value::Layer(None), ParamUi::Layer),
                p("rightView", "Right View", Value::Layer(None), ParamUi::Layer),
                p("sceneConvergence", "Scene Convergence", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("verticalAlignment", "Vertical Alignment", num(0.0), slider(-1000.0, 1000.0, -100.0, 100.0, 1)),
                p("units", "Units", Value::Enum(0), popup(&["Pixels", "% of Source"])),
                p("leftRightSwap", "Swap Left-Right", Value::Bool(false), ParamUi::Checkbox),
                p("view3d", "3D View", Value::Enum(5), popup(&VIEWS)),
                p("balance", "Balance", num(7.0), slider(0.0, 20.0, 0.0, 20.0, 1)),
            ],
            glasses_3d,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, run_fx};

    fn img() -> Image {
        crate::util::gen_image(32, 24, |x, y| {
            let a = if (4..28).contains(&x) && (4..20).contains(&y) { 1.0 } else { 0.0 };
            [x as f32 / 32.0 * a, y as f32 / 24.0 * a, 0.4 * a, a]
        })
    }

    struct Host(LayerPixels);
    impl EffectHost for Host {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            Some(self.0.clone())
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
    }

    #[test]
    fn environment_reflects_env_layer() {
        // A flat layer reflects the environment straight back: with a uniform green environment,
        // opaque pixels become green, transparent stay transparent.
        let env = LayerPixels { buf: Buf { img: Image::filled(16, 8, [0.0, 1.0, 0.0, 1.0]), offset: [0.0; 2], scale: 1.0 }, size: [16.0, 8.0] };
        let host = Host(env);
        let e = EffectEnv { host: Some(&host), ..Default::default() };
        let o = run_fx("ec.perspective.ccenvironment", &[("environment", Value::Layer(Some(9)))], img(), 0.0, e);
        let px = o.img.get(16, 12);
        assert!((px[1] - 1.0).abs() < 1e-4 && px[0].abs() < 1e-4, "{px:?}");
        assert_eq!(o.img.get(0, 0)[3], 0.0);
        let again = run_fx("ec.perspective.ccenvironment", &[("environment", Value::Layer(Some(9)))], img(), 0.0, e);
        assert_eq!(o.img, again.img);
        for m in 0..3 {
            let o = run_fx("ec.perspective.ccenvironment", &[("mapping", Value::Enum(m))], img(), 0.0, EffectEnv::default());
            assert!(o.img.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
        }
    }

    #[test]
    fn glasses_anaglyph_and_pairs() {
        let im = img();
        // Same view on both eyes: Red Blue LR puts the luminance in red and blue equally.
        let o = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(4))], im.clone(), 0.0, EffectEnv::default());
        let px = o.img.get(16, 12);
        assert!((px[0] - px[2]).abs() < 1e-5 && px[1] == 0.0 && px[0] > 0.0);
        // Difference of identical views is black.
        let o = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(8))], im.clone(), 0.0, EffectEnv::default());
        assert!(o.img.data.iter().all(|p| p[0].abs() < 1e-6 && p[1].abs() < 1e-6));
        // Convergence separates the eyes: difference becomes non-zero.
        let o = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(8)), ("sceneConvergence", num(4.0))], im.clone(), 0.0, EffectEnv::default());
        assert!(o.img.data.iter().any(|p| p[0] > 0.05));
        // Every view is deterministic.
        for v in 0..VIEWS.len() as u32 {
            let a = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(v))], im.clone(), 0.0, EffectEnv::default());
            let b = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(v))], im.clone(), 0.0, EffectEnv::default());
            assert_eq!(a.img, b.img);
        }
        // Interlace: even rows = left view (self).
        let o = run_fx("ec.perspective.3dglasses", &[("view3d", Value::Enum(2))], im.clone(), 0.0, EffectEnv::default());
        assert_eq!(o.img.get(10, 10), im.get(10, 10));
    }
}
