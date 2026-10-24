//! Matte effects: chokers (grey-scale morphology and blur/threshold stages) and edge-aware matte
//! refinement (guided filter, He et al. 2010).

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::gaussian_blur;
use rayon::prelude::*;

use crate::util::{FILL_REACH, Plane, gauss_plane, guided_filter, morph_frac, morph_plane, premul, set_alpha, set_alpha_fill, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Matte", params, render, gpu: false, float: true }
}

fn matte_view(b: &mut Buf) {
    b.img.data.par_iter_mut().for_each(|px| {
        let a = px[3].clamp(0.0, 1.0);
        *px = [a, a, a, 1.0];
    });
}

fn simple_choker(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let c = ctx.params.f("chokeMatte") * b.scale;
    if c.abs() > 1e-6 {
        if c < 0.0 && !ctx.adjustment {
            b.pad((-c).ceil() as u32 + 1);
        }
        let a = Plane::alpha(&b.img);
        let na = if c > 0.0 { morph_frac(&a, c, false) } else { morph_frac(&a, -c, true) };
        set_alpha_fill(&mut b.img, &na, c < -FILL_REACH);
    }
    if ctx.params.e("view") == 1 {
        matte_view(&mut b);
    }
    b
}

/// One Matte Choker stage: blur by geometric softness, then a threshold ramp.
fn choke_stage(a: Plane, geo: f64, choke: f32, gray: f32) -> Plane {
    let a = if geo > 0.0 { gauss_plane(&a, geo * 0.5, geo * 0.5) } else { a };
    let t = 0.5 + choke / 255.0 * 0.5;
    let w = gray.max(1e-3);
    if (t - 0.5).abs() < 1e-6 && (w - 1.0).abs() < 1e-6 {
        return a;
    }
    a.map(|v| ((v - t) / w + 0.5).clamp(0.0, 1.0))
}

fn matte_choker(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let g1 = ctx.params.f("geometricSoftness1") * b.scale;
    let c1 = ctx.params.f("choke1") as f32;
    let s1 = ctx.params.f("grayLevelSoftness1") as f32 / 100.0;
    let g2 = ctx.params.f("geometricSoftness2") * b.scale;
    let c2 = ctx.params.f("choke2") as f32;
    let s2 = ctx.params.f("grayLevelSoftness2") as f32 / 100.0;
    let it = ctx.params.f("iterations").round().clamp(1.0, 100.0) as usize;
    if (c1 < 0.0 || c2 < 0.0 || g1 > 0.0 || g2 > 0.0) && !ctx.adjustment {
        b.pad(((g1 + g2) * 1.5 * it as f64).ceil() as u32 + 1);
    }
    let mut a = Plane::alpha(&b.img);
    for _ in 0..it {
        a = choke_stage(a, g1, c1, s1);
        a = choke_stage(a, g2, c2, s2);
    }
    set_alpha(&mut b.img, &a);
    b
}

/// Refine Soft / Hard Matte. Refine Soft Matte nests its decontamination settings in a
/// "Decontamination" twirl-down; Refine Hard Matte lists them flat.
fn refine(ctx: &EffectCtx, mut b: Buf, hard: bool) -> Buf {
    let pr = ctx.params;
    let dk = |id: &str| if hard { id.to_string() } else { format!("decontamination/{id}") };
    let r = (pr.f("radius") * b.scale).round().max(1.0) as usize;
    let smooth = pr.f("smooth") as f32 / 100.0;
    let feather = pr.f("feather") / 100.0 * r as f64 * 0.5;
    // Choke (positive shrinks; Refine Hard Matte) and Shift Edge (positive grows; Refine Soft
    // Matte; older soft instances may still carry a hidden Choke).
    let choke = (pr.f("choke") - if hard { 0.0 } else { pr.f("shiftEdge") }).clamp(-100.0, 100.0) as f32 / 100.0 * 0.5;
    let decon = pr.b("decontaminate");
    let decon_amt = pr.f(&dk("decontaminationAmount")) as f32 / 100.0;
    let extend = pr.b(&dk("extendWhereSmoothed"));
    let decon_r = r as f64 + pr.f(&dk("increaseDecontaminationRadius")).max(0.0) * b.scale;
    let view_map = pr.b(&dk("viewDecontaminationMap"));
    let invert = pr.b("invert");
    let contrast = if hard { pr.f("alphaContrast") as f32 / 100.0 } else { 1.0 + pr.f("contrast").max(0.0) as f32 / 100.0 * 3.0 };
    let edge_details = hard || pr.b("calculateEdgeDetails");
    let view_edges = !hard && pr.b("viewEdgeRegion");

    let a = Plane::alpha(&b.img);
    // Only the edge band (where the matte varies within the radius) is refined.
    let hi = morph_plane(&a, r, r, true);
    let lo = morph_plane(&a, r, r, false);
    let band = |i: usize| hi.data[i] - lo.data[i] > 1e-3;
    // The refined matte of a frame.
    let refine_alpha = |img: &effectcraft_raster::Image| -> Plane {
        let a = Plane::alpha(img);
        let hi = morph_plane(&a, r, r, true);
        let lo = morph_plane(&a, r, r, false);
        let mut na = Plane::new(a.w, a.h);
        if edge_details {
            let guide = Plane::from_image(img, |px| effectcraft_color::luminance(px[0], px[1], px[2]));
            let eps = 1e-4 + smooth * smooth * 0.05;
            let filtered = guided_filter(&guide, &a, r, eps);
            na.data.par_iter_mut().enumerate().for_each(|(i, v)| {
                *v = if hi.data[i] - lo.data[i] > 1e-3 { filtered.data[i].clamp(0.0, 1.0) } else { a.data[i] };
            });
        } else {
            na = a.clone();
            if smooth > 0.0 {
                let s = smooth as f64 * r as f64 * 0.5;
                na = gauss_plane(&na, s, s);
            }
        }
        if feather > 0.0 {
            na = gauss_plane(&na, feather, feather);
        }
        na.map(|v| {
            let mut v = if choke > 0.0 {
                (v - choke) / (1.0 - choke)
            } else if choke < 0.0 {
                v / (1.0 + choke)
            } else {
                v
            };
            if (contrast - 1.0).abs() > 1e-6 {
                v = (v - 0.5) * contrast + 0.5;
            }
            let v = v.clamp(0.0, 1.0);
            if invert { 1.0 - v } else { v }
        })
    };
    let mut na = refine_alpha(&b.img);
    // Neighbouring frames of this effect's input (same pixel grid), refined the same way.
    let fps = ctx.fps();
    let frame_at = |t: f64| -> Option<Plane> {
        let o = ctx.env.host?.self_at(t, ctx.env.effect_index)?;
        (o.img.width == b.img.width && o.img.height == b.img.height && (o.offset[0] - b.offset[0]).abs() < 1e-6 && (o.offset[1] - b.offset[1]).abs() < 1e-6)
            .then(|| refine_alpha(&o.img))
    };
    // Reduce Chatter: where the previous and next frames agree but this one differs, the matte
    // is chattering; average the three there. Where they disagree (motion) it is left alone.
    let chatter = (pr.f("reduceChatter") / 100.0).clamp(0.0, 1.0) as f32;
    if chatter > 0.0 {
        let prev = frame_at(ctx.time - 1.0 / fps);
        let next = frame_at(ctx.time + 1.0 / fps);
        if let (Some(p), Some(n)) = (&prev, &next) {
            na.data.par_iter_mut().enumerate().for_each(|(i, v)| {
                let (c, a, z) = (*v, p.data[i], n.data[i]);
                let steady = (1.0 - (a - z).abs() / 0.5).clamp(0.0, 1.0);
                *v = c + ((a + c + z) / 3.0 - c) * chatter * steady;
            });
        }
    }
    // Motion blur: the matte averaged over the shutter (centred on the frame).
    if pr.b("useMotionBlur") {
        let g = |id: &str| pr.get(&format!("motionBlur/{id}")).map(Value::as_f64);
        let samples = g("motionBlurSamples").unwrap_or(16.0).clamp(1.0, 64.0) as usize;
        let samples = if pr.b("motionBlur/higherQuality") { samples * 2 } else { samples }.min(64);
        let span = g("shutterAngle").unwrap_or(180.0).clamp(0.0, 720.0) / 360.0 / fps;
        if samples > 1 && span > 0.0 {
            let mut acc = na.clone();
            let mut n = 1.0f32;
            for k in 0..samples {
                let t = ctx.time - span * 0.5 + span * k as f64 / (samples - 1) as f64;
                if (t - ctx.time).abs() < 1e-9 {
                    continue;
                }
                if let Some(p) = frame_at(t) {
                    acc.data.iter_mut().zip(&p.data).for_each(|(a, q)| *a += q);
                    n += 1.0;
                }
            }
            if n > 1.0 {
                acc.data.iter_mut().for_each(|a| *a /= n);
                na = acc;
            }
        }
    }
    if view_edges {
        // The refined matte in grey with the analysed edge band tinted.
        b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
            let v = na.data[i];
            *px = if band(i) { [0.5 + 0.5 * v, 0.5 * v, 0.5 * v, 1.0] } else { [v, v, v, 1.0] };
        });
        return b;
    }
    // Decontamination strength per pixel: partial input alpha, and (Extend Where Smoothed) where
    // the refinement made the matte partial.
    let strength = |i: usize, a0: f32| -> f32 {
        let partial = |a: f32| a > 1e-4 && a < 0.999;
        let n = na.data[i];
        let t = if partial(a0) {
            1.0 - a0
        } else if extend && partial(n) {
            1.0 - n
        } else {
            0.0
        };
        decon_amt * t
    };
    if view_map {
        b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
            let v = if decon { strength(i, px[3]) } else { 0.0 };
            *px = [v, v, v, 1.0];
        });
        return b;
    }
    if decon && decon_amt > 0.0 {
        let est = gaussian_blur(&b.img, decon_r * 0.5, decon_r * 0.5, true);
        b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
            let (c, a0) = unpremul(*px);
            let t = strength(i, a0);
            if t > 0.0 && a0 > 1e-4 {
                let (ec, ea) = unpremul(est.data[i]);
                if ea > 1e-4 {
                    *px = premul([0, 1, 2].map(|k| c[k] + (ec[k] - c[k]) * t), a0);
                }
            }
        });
    }
    set_alpha(&mut b.img, &na);
    b
}

fn refine_soft(ctx: &EffectCtx, b: Buf) -> Buf {
    refine(ctx, b, false)
}

fn refine_hard(ctx: &EffectCtx, b: Buf) -> Buf {
    refine(ctx, b, true)
}

pub fn specs() -> Vec<EffectSpec> {
    let pct0 = || slider(0.0, 100.0, 0.0, 100.0, 0);
    let check = |id: &'static str, name: &'static str, on: bool| p(id, name, Value::Bool(on), ParamUi::Checkbox);
    let decon = |pre: &'static [&'static str; 4]| {
        vec![
            p(pre[0], "Decontamination Amount", num(100.0), pct0()),
            check(pre[1], "Extend Where Smoothed", true),
            p(pre[2], "Increase Decontamination Radius", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            check(pre[3], "View Decontamination Map", false),
        ]
    };
    // Reduce Chatter and the motion-blur settings.
    let temporal = || {
        vec![
            p("reduceChatter", "Reduce Chatter", num(0.0), pct0()),
            check("useMotionBlur", "Use Motion Blur", false),
            p("motionBlur/motionBlurSamples", "Motion Blur Samples", num(16.0), slider(1.0, 64.0, 1.0, 64.0, 0)),
            p("motionBlur/shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 720.0, 0.0, 360.0, 1)),
            check("motionBlur/higherQuality", "Higher Quality", false),
        ]
    };
    let radius = |d: f64| p("radius", "Additional Edge Radius", num(d), slider(1.0, 200.0, 1.0, 50.0, 1));
    let mut soft_params = vec![
        check("calculateEdgeDetails", "Calculate Edge Details", true),
        radius(10.0),
        check("viewEdgeRegion", "View Edge Region", false),
        p("smooth", "Smooth", num(0.0), pct0()),
        p("feather", "Feather", num(10.0), pct0()),
        p("contrast", "Contrast", num(0.0), pct0()),
        p("shiftEdge", "Shift Edge", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
    ];
    soft_params.extend(temporal());
    soft_params.push(check("decontaminate", "Decontaminate Edge Colors", true));
    soft_params.extend(decon(&[
        "decontamination/decontaminationAmount",
        "decontamination/extendWhereSmoothed",
        "decontamination/increaseDecontaminationRadius",
        "decontamination/viewDecontaminationMap",
    ]));
    soft_params.push(check("invert", "Invert", false));
    // Superseded by Shift Edge (opposite sign); kept so older projects render as before.
    soft_params.push(p("choke", "Choke", num(0.0), ParamUi::Hidden));
    let mut hard_params = vec![
        p("smooth", "Smooth", num(20.0), pct0()),
        p("feather", "Feather", num(0.0), pct0()),
        p("choke", "Choke", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
    ];
    hard_params.extend(temporal());
    hard_params.push(check("decontaminate", "Decontaminate Edge Colors", true));
    hard_params.extend(decon(&["decontaminationAmount", "extendWhereSmoothed", "increaseDecontaminationRadius", "viewDecontaminationMap"]));
    // Our extras: the analysed edge band's radius, alpha contrast and inversion.
    hard_params.push(radius(3.0));
    hard_params.push(p("alphaContrast", "Alpha Contrast", num(300.0), slider(100.0, 1000.0, 100.0, 500.0, 0)));
    hard_params.push(check("invert", "Invert", false));
    let soft = || slider(0.0, 1000.0, 0.0, 100.0, 1);
    let ch = || slider(-127.0, 127.0, -127.0, 127.0, 0);
    let gl = || slider(0.0, 100.0, 0.0, 100.0, 1);
    vec![
        spec(
            "ec.matte.simplechoker",
            "Simple Choker",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Final Output", "Matte"])),
                p("chokeMatte", "Choke Matte", num(0.0), slider(-100.0, 100.0, -10.0, 10.0, 2)),
            ],
            simple_choker,
        ),
        spec(
            "ec.matte.mattechoker",
            "Matte Choker",
            vec![
                p("geometricSoftness1", "Geometric Softness 1", num(4.0), soft()),
                p("choke1", "Choke 1", num(75.0), ch()),
                p("grayLevelSoftness1", "Gray Level Softness 1", num(10.0), gl()),
                p("geometricSoftness2", "Geometric Softness 2", num(0.0), soft()),
                p("choke2", "Choke 2", num(0.0), ch()),
                p("grayLevelSoftness2", "Gray Level Softness 2", num(100.0), gl()),
                p("iterations", "Iterations", num(1.0), slider(1.0, 100.0, 1.0, 10.0, 0)),
            ],
            matte_choker,
        ),
        spec("ec.matte.refinesoft", "Refine Soft Matte", soft_params, refine_soft),
        spec("ec.matte.refinehard", "Refine Hard Matte", hard_params, refine_hard),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, apply, find};
    use effectcraft_raster::Image;

    fn square() -> Image {
        let mut img = Image::new(24, 24);
        for y in 6..18 {
            for x in 6..18 {
                img.set(x, y, [0.8, 0.4, 0.2, 1.0]);
            }
        }
        img
    }

    fn run(id: &str, over: &[(&str, Value)], img: Image) -> Image {
        let s = find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for (k, v) in over {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: true, env: Default::default() };
        apply(s, &ctx, Buf { img, offset: [0.0, 0.0], scale: 1.0 }).img
    }

    fn alpha_sum(img: &Image) -> f32 {
        img.data.iter().map(|p| p[3]).sum()
    }

    #[test]
    fn simple_choker_far_growth_keeps_layer_colour() {
        let mut img = Image::new(100, 100);
        for y in 46..54 {
            for x in 46..54 {
                img.set(x, y, [0.8, 0.4, 0.2, 1.0]);
            }
        }
        let grown = run("ec.matte.simplechoker", &[("chokeMatte", num(-30.0))], img);
        // 25 px from the original edge, well beyond the colour-fill blur's reach.
        let (c, a) = unpremul(grown.get(21, 50));
        assert!(a > 0.99 && c[0] > 0.5, "{c:?} {a}");
    }

    #[test]
    fn simple_choker_shrinks_and_grows() {
        let img = square();
        let base = alpha_sum(&img);
        let shrunk = run("ec.matte.simplechoker", &[("chokeMatte", num(2.0))], img.clone());
        assert!((alpha_sum(&shrunk) - 64.0).abs() < 1e-3, "{}", alpha_sum(&shrunk));
        let grown = run("ec.matte.simplechoker", &[("chokeMatte", num(-2.0))], img);
        assert!(alpha_sum(&grown) > base);
        // Grown pixels pick up the layer colour, not black.
        let (c, a) = unpremul(grown.get(5, 10));
        assert!(a > 0.99 && c[0] > 0.5, "{c:?}");
    }

    #[test]
    fn matte_choker_defaults_shrink() {
        let img = square();
        let out = run("ec.matte.mattechoker", &[], img.clone());
        assert!(alpha_sum(&out) < alpha_sum(&img));
        assert!(out.get(12, 12)[3] > 0.99);
    }

    #[test]
    fn refine_soft_matte_shift_edge_contrast_and_views() {
        let mut img = square();
        // A soft ramp on the left edge of the square.
        for y in 6..18 {
            img.set(5, y, [0.4, 0.2, 0.1, 0.5]);
        }
        let base = run("ec.matte.refinesoft", &[("feather", num(0.0))], img.clone());
        let grown = run("ec.matte.refinesoft", &[("feather", num(0.0)), ("shiftEdge", num(60.0))], img.clone());
        assert!(alpha_sum(&grown) > alpha_sum(&base) + 0.5, "{} vs {}", alpha_sum(&grown), alpha_sum(&base));
        let legacy = run("ec.matte.refinesoft", &[("feather", num(0.0)), ("choke", num(-60.0))], img.clone());
        assert_eq!(legacy.data, grown.data, "the hidden legacy Choke is Shift Edge negated");
        // Without edge detail calculation (and no smoothing) the matte only goes through the
        // level adjustments: identity at defaults.
        let plain = run(
            "ec.matte.refinesoft",
            &[("feather", num(0.0)), ("calculateEdgeDetails", Value::Bool(false)), ("decontaminate", Value::Bool(false))],
            img.clone(),
        );
        assert!(plain.data.iter().zip(&img.data).all(|(a, b)| (0..4).all(|k| (a[k] - b[k]).abs() < 1e-5)));
        // View Edge Region and View Decontamination Map are opaque diagnostic views.
        let edges = run("ec.matte.refinesoft", &[("viewEdgeRegion", Value::Bool(true))], img.clone());
        assert!(edges.data.iter().all(|p| p[3] == 1.0));
        assert!(edges.get(5, 10)[0] > edges.get(5, 10)[1], "band is tinted");
        let map = run(
            "ec.matte.refinesoft",
            &[("decontamination/viewDecontaminationMap", Value::Bool(true)), ("calculateEdgeDetails", Value::Bool(false)), ("feather", num(0.0))],
            img,
        );
        assert!(map.get(5, 10)[0] > 0.4 && map.get(12, 12)[0] == 0.0);
    }

    #[test]
    fn refine_soft_matte_keeps_interior_and_range() {
        let img = square();
        let out = run("ec.matte.refinesoft", &[], img);
        assert!(out.get(12, 12)[3] > 0.99);
        assert!(out.get(0, 0)[3] < 0.01);
        assert!(out.data.iter().all(|p| (0.0..=1.0).contains(&p[3])));
    }

    /// A white square whose left edge sits at x = 10 + `jitter` × (frame parity) + `speed` × frame.
    struct Edge {
        jitter: i64,
        speed: i64,
    }
    fn edge(x0: i64) -> Image {
        let mut img = Image::new(40, 20);
        for y in 4..16 {
            for x in x0.max(0)..(x0 + 16).min(40) {
                img.set(x as u32, y, [1.0, 1.0, 1.0, 1.0]);
            }
        }
        img
    }
    impl crate::EffectHost for Edge {
        fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            let f = (t * 10.0).round() as i64;
            Some(Buf { img: edge(10 + self.jitter * (f & 1) + self.speed * f), offset: [0.0; 2], scale: 1.0 })
        }
    }

    fn run_t(id: &str, over: &[(&str, Value)], t: f64, host: &Edge) -> Image {
        let f = (t * 10.0).round() as i64;
        let img = edge(10 + host.jitter * (f & 1) + host.speed * f);
        let env = crate::EffectEnv { host: Some(host), frame_rate: 10.0, ..Default::default() };
        crate::run_fx(id, over, img, t, env).img
    }

    #[test]
    fn refine_mattes_reduce_chatter_and_motion_blur() {
        for id in ["ec.matte.refinesoft", "ec.matte.refinehard"] {
            let jitter = Edge { jitter: 1, speed: 0 };
            let base = [("feather", num(0.0)), ("smooth", num(0.0)), ("decontaminate", Value::Bool(false))];
            let with = |extra: &[(&str, Value)], t: f64, h: &Edge| {
                let mut v: Vec<(&str, Value)> = base.to_vec();
                v.extend_from_slice(extra);
                run_t(id, &v, t, h)
            };
            let diff = |a: &Image, b: &Image| a.data.iter().zip(&b.data).map(|(p, q)| (p[3] - q[3]).abs()).sum::<f32>();
            let (a0, a1) = (with(&[], 1.0, &jitter), with(&[], 1.1, &jitter));
            let (c0, c1) = (with(&[("reduceChatter", num(100.0))], 1.0, &jitter), with(&[("reduceChatter", num(100.0))], 1.1, &jitter));
            assert!(diff(&c0, &c1) < diff(&a0, &a1) * 0.7, "{id}: chatter {} vs {}", diff(&c0, &c1), diff(&a0, &a1));
            // Motion blur smears a moving matte edge along the motion.
            let moving = Edge { jitter: 0, speed: 4 };
            let mb = with(&[("useMotionBlur", Value::Bool(true)), ("motionBlur/shutterAngle", num(360.0))], 0.3, &moving);
            let sharp = with(&[], 0.3, &moving);
            let partial = |img: &Image| img.data.iter().filter(|p| p[3] > 0.05 && p[3] < 0.95).count();
            assert!(partial(&mb) > partial(&sharp) + 10, "{id}: {} vs {}", partial(&mb), partial(&sharp));
            // Without a host both are inert.
            assert_eq!(run(id, &[("reduceChatter", num(100.0)), ("useMotionBlur", Value::Bool(true))], edge(10)), run(id, &[], edge(10)));
        }
    }
}
