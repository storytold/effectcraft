//! Keying effects, batch 2: Difference Matte (against another layer), Inner/Outer Key (mattes
//! from masks with colour-based edge estimation and decontamination), Unmult and CC Simple
//! Wire Removal.

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::gaussian_blur;
use rayon::prelude::*;

use crate::util::{Plane, dist_to_poly, gauss_plane, layer_or_self, lerp4, morph_frac, point_in_poly, premul, smoothstep, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Keying", params, render, gpu: false, float: true }
}

fn pct() -> ParamUi {
    slider(0.0, 100.0, 0.0, 100.0, 1)
}

// ---------------------------------------------------------------- Difference Matte

fn difference_matte(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let view = ctx.params.e("view");
    if view == 1 {
        return b;
    }
    let tol = (ctx.params.f("matchingTolerance") / 100.0) as f32;
    let soft = (ctx.params.f("matchingSoftness") / 100.0) as f32;
    let blur = ctx.params.f("blurBeforeDifference") * b.scale;
    let other = layer_or_self(ctx, &b, "differenceLayer", true, ctx.params.e("ifLayerSizesDiffer") == 1);
    let (src, other) = if blur > 0.05 {
        (gaussian_blur(&b.img, blur * 0.5, blur * 0.5, true), gaussian_blur(&other, blur * 0.5, blur * 0.5, true))
    } else {
        (b.img.clone(), other)
    };
    let matte_only = view == 2;
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let (c, a) = unpremul(src.data[i]);
        let (oc, oa) = unpremul(other.data[i]);
        let d = (c[0] - oc[0]).abs().max((c[1] - oc[1]).abs()).max((c[2] - oc[2]).abs()).max((a - oa).abs());
        let m = if d <= tol {
            0.0
        } else if soft <= 1e-6 {
            1.0
        } else {
            ((d - tol) / soft).clamp(0.0, 1.0)
        };
        if matte_only {
            *px = [m * px[3], m * px[3], m * px[3], px[3]];
        } else {
            *px = px.map(|v| v * m);
        }
    });
    b
}

// ---------------------------------------------------------------- Inner/Outer Key

fn mask_options() -> ParamUi {
    popup(&["None", "Mask 1", "Mask 2", "Mask 3", "Mask 4", "Mask 5", "Mask 6", "Mask 7", "Mask 8", "Mask 9", "Mask 10"])
}

/// Coverage plane (1 inside) of the selected closed masks, in buffer pixels.
fn mask_plane(ctx: &EffectCtx, b: &Buf, idx: &[u32]) -> Option<Plane> {
    let shapes: Vec<&crate::MaskShape> = idx.iter().filter(|&&i| i > 0).filter_map(|&i| ctx.env.masks.get(i as usize - 1)).collect();
    if shapes.is_empty() {
        return None;
    }
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let mut pl = Plane::new(w, h);
    let inv = 1.0 / b.scale.max(1e-9);
    pl.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let lx = (x as f64 + 0.5 - b.offset[0]) * inv;
            let ly = (y as f64 + 0.5 - b.offset[1]) * inv;
            *v = if shapes.iter().any(|s| point_in_poly(&s.points, lx, ly)) { 1.0 } else { 0.0 };
        }
    });
    Some(pl)
}

/// Signed distance (pixels, positive inside) to the selected mask outlines.
fn mask_sdf(ctx: &EffectCtx, b: &Buf, idx: &[u32]) -> Option<Plane> {
    let shapes: Vec<&crate::MaskShape> = idx.iter().filter(|&&i| i > 0).filter_map(|&i| ctx.env.masks.get(i as usize - 1)).collect();
    if shapes.is_empty() {
        return None;
    }
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let mut pl = Plane::new(w, h);
    let inv = 1.0 / b.scale.max(1e-9);
    pl.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            let lx = (x as f64 + 0.5 - b.offset[0]) * inv;
            let ly = (y as f64 + 0.5 - b.offset[1]) * inv;
            let inside = shapes.iter().any(|s| point_in_poly(&s.points, lx, ly));
            let d = shapes.iter().map(|s| dist_to_poly(&s.points, true, lx, ly)).fold(f64::INFINITY, f64::min) * b.scale;
            *v = if inside { d as f32 } else { -d as f32 };
        }
    });
    Some(pl)
}

/// Inner/Outer Key's geometry (shared with the GPU compositor, effectcraft-gpu `fx_pixel2`):
/// the trimap of the selected masks and the Cleanup brush strokes, in buffer pixels.
pub struct InnerOuterPlan {
    /// 1 = known foreground, 0 = known background, 0.5 = unknown.
    pub tri: Plane,
    /// Some pixel is unknown (edge colours are estimated and decontaminated).
    pub unknown: bool,
    /// Blur radius of the local foreground / background colour estimates.
    pub sigma: f64,
    /// Cleanup strokes in order: (matte target, coverage per pixel; negative = beyond the brush).
    pub strokes: Vec<(f32, Vec<f32>)>,
    /// Edge Thin, Edge Feather (buffer px), Edge Threshold, Invert Extraction, Blend with Original.
    pub thin: f64,
    pub feather: f64,
    pub threshold: f32,
    pub invert: bool,
    pub original: f32,
}

/// [`InnerOuterPlan`] of an Inner/Outer Key instance on `b`'s geometry (`None` = no foreground
/// mask: the layer passes through).
pub fn inner_outer_plan(ctx: &EffectCtx, b: &Buf) -> Option<InnerOuterPlan> {
    // Foreground (Inside) plus Additional Foreground 1–10; likewise for the background.
    let fg_idx: Vec<u32> =
        std::iter::once(ctx.params.e("foreground")).chain((1..=10).map(|k| ctx.params.e(&format!("additionalForeground/foreground{k}")))).collect();
    let bg_idx: Vec<u32> =
        std::iter::once(ctx.params.e("background")).chain((1..=10).map(|k| ctx.params.e(&format!("additionalBackground/background{k}")))).collect();
    let radius = (ctx.params.f("singleMaskHighlightRadius") * b.scale) as f32;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    // Trimap: 1 = known foreground, 0 = known background, 0.5 = unknown.
    let fg = mask_plane(ctx, b, &fg_idx);
    let bg = mask_plane(ctx, b, &bg_idx);
    let tri: Plane = match (fg, bg) {
        (None, _) => return None,
        (Some(f), Some(bk)) => f.zip_map(&bk, |f, bk| {
            if f > 0.5 {
                1.0
            } else if bk > 0.5 {
                0.5
            } else {
                0.0
            }
        }),
        (Some(f), None) => {
            if radius <= 0.0 {
                f
            } else {
                let sdf = mask_sdf(ctx, b, &fg_idx).unwrap_or(f);
                sdf.map(|d| {
                    if d > radius {
                        1.0
                    } else if d < -radius {
                        0.0
                    } else {
                        0.5
                    }
                })
            }
        }
    };
    let unknown = tri.data.par_iter().any(|&t| t > 0.25 && t < 0.75);
    let sigma = (radius.max(4.0) as f64).max(w.min(h) as f64 * 0.02);
    // Cleanup Foreground / Background: brush strokes along mask paths that paint the matte
    // towards opaque / transparent (Brush Radius, Brush Pressure).
    let mut strokes = Vec::new();
    for (side, target) in [("Foreground", 1.0f32), ("Background", 0.0)] {
        for k in 1..=10 {
            let g = format!("cleanup{side}/cleanup{side}{k}");
            let idx = ctx.params.e(&format!("{g}/path"));
            let Some(m) = (idx > 0).then(|| ctx.env.masks.get(idx as usize - 1)).flatten() else { continue };
            let rad = (ctx.params.f(&format!("{g}/brushRadius")) * b.scale).max(0.5);
            let pressure = (ctx.params.f(&format!("{g}/brushPressure")) / 100.0).clamp(0.0, 1.0) as f32;
            let pts: Vec<[f64; 2]> = m.points.iter().map(|q| [q[0] * b.scale + b.offset[0], q[1] * b.scale + b.offset[1]]).collect();
            let cov: Vec<f32> = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
                    let d = dist_to_poly(&pts, m.closed, x, y);
                    if d < rad + 1.0 { ((rad - d) / rad.max(1.0) * 2.0).clamp(0.0, 1.0) as f32 * pressure } else { -1.0 }
                })
                .collect();
            strokes.push((target, cov));
        }
    }
    Some(InnerOuterPlan {
        tri,
        unknown,
        sigma,
        strokes,
        thin: ctx.params.f("edgeThin") * b.scale,
        feather: ctx.params.f("edgeFeather") * b.scale,
        threshold: (ctx.params.f("edgeThreshold") / 100.0) as f32,
        invert: ctx.params.b("invertExtraction"),
        original: (ctx.params.f("blendWithOriginal") / 100.0) as f32,
    })
}

fn inner_outer_key(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(plan) = inner_outer_plan(ctx, &b) else { return b };
    let InnerOuterPlan { tri, unknown, sigma, strokes, thin, feather, threshold: thr, invert, original: orig } = plan;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let mut alpha = tri.clone();
    let mut colours: Option<Vec<[f32; 3]>> = None;
    if unknown {
        // Local foreground / background colour estimates: normalised blurs over known regions.
        let known_f = tri.map(|t| if t > 0.75 { 1.0 } else { 0.0 });
        let known_b = tri.map(|t| if t < 0.25 { 1.0 } else { 0.0 });
        let straight: Vec<[f32; 3]> = b.img.data.par_iter().map(|&px| unpremul(px).0).collect();
        let est = |known: &Plane| -> Vec<[f32; 3]> {
            let wsum = gauss_plane(known, sigma, sigma);
            let chans: Vec<Plane> = (0..3)
                .map(|k| {
                    let pl = Plane { w, h, data: straight.par_iter().zip(known.data.par_iter()).map(|(c, &m)| c[k] * m).collect() };
                    gauss_plane(&pl, sigma, sigma)
                })
                .collect();
            (0..w * h).into_par_iter().map(|i| [0, 1, 2].map(|k| chans[k].data[i] / wsum.data[i].max(1e-6))).collect()
        };
        let fc = est(&known_f);
        let bc = est(&known_b);
        alpha.data.par_iter_mut().enumerate().for_each(|(i, a)| {
            if *a > 0.25 && *a < 0.75 {
                let c = straight[i];
                let d = [fc[i][0] - bc[i][0], fc[i][1] - bc[i][1], fc[i][2] - bc[i][2]];
                let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                *a = if dd < 1e-6 { 0.5 } else { (((c[0] - bc[i][0]) * d[0] + (c[1] - bc[i][1]) * d[1] + (c[2] - bc[i][2]) * d[2]) / dd).clamp(0.0, 1.0) };
            }
        });
        // Decontaminate edge colours: remove the background contribution.
        colours = Some(
            (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let a = alpha.data[i];
                    let c = straight[i];
                    if tri.data[i] > 0.25 && tri.data[i] < 0.75 && a > 0.02 {
                        [0, 1, 2].map(|k| ((c[k] - (1.0 - a) * bc[i][k]) / a).clamp(0.0, 1.0))
                    } else {
                        c
                    }
                })
                .collect(),
        );
    }
    for (target, cov) in &strokes {
        alpha.data.par_iter_mut().zip(cov.par_iter()).for_each(|(a, &c)| {
            if c >= 0.0 {
                *a += (target - *a) * c;
            }
        });
    }
    if thin.abs() > 0.01 {
        alpha = morph_frac(&alpha, thin.abs(), thin < 0.0);
    }
    if feather > 0.05 {
        alpha = gauss_plane(&alpha, feather * 0.5, feather * 0.5);
    }
    let src = b.img.clone();
    b.img.data.par_iter_mut().enumerate().for_each(|(i, px)| {
        let mut m = alpha.data[i];
        if thr > 0.0 {
            m = if m < thr { 0.0 } else { m };
        }
        if invert {
            m = 1.0 - m;
        }
        let (c0, a) = unpremul(src.data[i]);
        let c = colours.as_ref().map(|v| v[i]).unwrap_or(c0);
        let keyed = premul(c, a * m);
        *px = lerp4(keyed, src.data[i], orig);
    });
    b
}

// ---------------------------------------------------------------- Unmult

/// Turns a black (or white) background into transparency. Alpha ramps up from the level (Black
/// Level: on the brightest channel; White Level: on the darkest channel, mirrored) over a
/// transition whose width is Softness (0 % = hard cutoff at the level, 100 % = the whole
/// remaining range, which with the default levels is the classic unmultiply: alpha = brightest
/// channel). Remove Color Matting takes the background colour back out of semi-transparent
/// pixels; Clip HDR Results keeps the recovered colours in 0..1.
fn unmult(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pr = ctx.params;
    let white = pr.e("backgroundColor") == 1;
    let level = if white { pr.f("whiteLevel") } else { pr.f("blackLevel") }.clamp(0.0, 100.0) as f32 / 100.0;
    let soft = pr.f("softness").clamp(0.0, 100.0) as f32 / 100.0;
    let decon = pr.b("removeColorMatting");
    let clip = pr.b("clipHdrResults");
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a0) = unpremul(*px);
        // Distance from the background colour, and where the ramp starts on that scale.
        let (d, start) = if white { (1.0 - c[0].min(c[1]).min(c[2]), 1.0 - level) } else { (c[0].max(c[1]).max(c[2]), level) };
        let d = d.clamp(0.0, 1.0);
        let width = soft * (1.0 - start);
        let m = if width > 1e-6 {
            ((d - start) / width).clamp(0.0, 1.0)
        } else if d > start {
            1.0
        } else {
            0.0
        };
        if m <= 1e-6 {
            *px = [0.0; 4];
            return;
        }
        let bg = if white { 1.0 } else { 0.0 };
        let mut out = if decon { c.map(|v| (v - bg * (1.0 - m)) / m) } else { c };
        if clip {
            out = out.map(|v| v.clamp(0.0, 1.0));
        }
        *px = premul(out, a0 * m);
    });
    b
}

// ---------------------------------------------------------------- CC Simple Wire Removal

fn wire_removal(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let pa = ctx.params.v2("pointA");
    let pb = ctx.params.v2("pointB");
    let style = ctx.params.e("removalStyle");
    let half = (ctx.params.f("thickness") * b.scale * 0.5).max(0.0);
    if half <= 0.0 {
        return b;
    }
    let slope = (ctx.params.f("slope") / 100.0).clamp(0.0, 1.0) as f32;
    let mirror = (ctx.params.f("mirrorBlend") / 100.0).clamp(0.0, 1.0) as f32;
    let (ax, ay) = b.to_px(pa);
    let (bx, by) = b.to_px(pb);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return b;
    }
    let (tx, ty) = (dx / len, dy / len);
    let (nx, ny) = (-ty, tx);
    let src = b.img.clone();
    let soft_w = half * slope as f64;
    // Frame Offset: the same pixels from the previous or next frame, resampled into this grid.
    let offset = ctx.params.f("frameOffset").round();
    let neighbour = if style == 1 && offset != 0.0 {
        ctx.env.host.and_then(|h| h.self_at(ctx.time + offset / ctx.fps(), ctx.env.effect_index)).map(|nb| {
            if nb.img.width == b.img.width && nb.img.height == b.img.height && nb.offset == b.offset && nb.scale == b.scale {
                nb.img
            } else {
                let k = nb.scale / b.scale.max(1e-9);
                crate::util::gen_image(b.img.width, b.img.height, |x, y| {
                    let lx = (x as f64 + 0.5 - b.offset[0]) * k + nb.offset[0];
                    let ly = (y as f64 + 0.5 - b.offset[1]) * k + nb.offset[1];
                    nb.img.sample_bilinear(lx, ly)
                })
            }
        })
    } else {
        None
    };
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let (qx, qy) = (x as f64 + 0.5 - ax, y as f64 + 0.5 - ay);
            let d = qx * nx + qy * ny;
            let ad = d.abs();
            // The wire is the segment A to B, not an endless line.
            let along = qx * tx + qy * ty;
            if !(0.0..=len).contains(&along) || ad > half + soft_w {
                continue;
            }
            // Positions just outside the wire on both sides.
            let base = (x as f64 + 0.5 - d * nx, y as f64 + 0.5 - d * ny);
            let edge = half + 1.0;
            let pos = |s: f64| src.sample_bilinear_clamped(base.0 + nx * s, base.1 + ny * s);
            let repl = match style {
                0 => {
                    let t = ((d + edge) / (2.0 * edge)) as f32;
                    lerp4(pos(-edge), pos(edge), t.clamp(0.0, 1.0))
                }
                1 if neighbour.is_some() => neighbour.as_ref().map_or(*px, |n| n.get(x as i64, y as i64)),
                // Displace: shift the image across the wire from the nearer side (also Frame Offset
                // when no other frame is available).
                2 | 1 => {
                    let s = if d >= 0.0 { d + 2.0 * (half - ad) + 1.0 } else { d - 2.0 * (half - ad) - 1.0 };
                    let sh = pos(s);
                    if mirror > 0.0 { lerp4(sh, pos(-s), mirror * 0.5) } else { sh }
                }
                _ => {
                    // Displace Horizontal: sample left/right of the wire along x.
                    let sx = if nx.abs() > 1e-6 { (half - ad + 1.0) / nx.abs() } else { 0.0 };
                    let dir = if d * nx >= 0.0 { 1.0 } else { -1.0 };
                    let sh = src.sample_bilinear_clamped(x as f64 + 0.5 + dir * sx, y as f64 + 0.5);
                    let mi = src.sample_bilinear_clamped(x as f64 + 0.5 - dir * sx, y as f64 + 0.5);
                    if mirror > 0.0 { lerp4(sh, mi, mirror * 0.5) } else { sh }
                }
            };
            let k = if ad <= half { 1.0 } else { 1.0 - smoothstep(half as f32, (half + soft_w) as f32, ad as f32) };
            *px = lerp4(*px, repl, k);
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.key.differencematte",
            "Difference Matte",
            vec![
                p("view", "View", Value::Enum(0), popup(&["Final Output", "Source Only", "Matte Only"])),
                p("differenceLayer", "Difference Layer", Value::Layer(None), ParamUi::Layer),
                p("ifLayerSizesDiffer", "If Layer Sizes Differ", Value::Enum(0), popup(&["Center", "Stretch to Fit"])),
                p("matchingTolerance", "Matching Tolerance", num(15.0), pct()),
                p("matchingSoftness", "Matching Softness", num(0.0), pct()),
                p("blurBeforeDifference", "Blur Before Difference", num(0.0), slider(0.0, 1000.0, 0.0, 20.0, 1)),
            ],
            difference_matte,
        ),
        spec(
            "ec.key.innerouter",
            "Inner/Outer Key",
            {
                let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
                let mut v = vec![p("foreground", "Foreground (Inside)", Value::Enum(0), mask_options())];
                for k in 1..=10 {
                    v.push(p(leak(format!("additionalForeground/foreground{k}")), leak(format!("Foreground {k}")), Value::Enum(0), mask_options()));
                }
                v.push(p("background", "Background (Outside)", Value::Enum(0), mask_options()));
                for k in 1..=10 {
                    v.push(p(leak(format!("additionalBackground/background{k}")), leak(format!("Background {k}")), Value::Enum(0), mask_options()));
                }
                for side in ["Foreground", "Background"] {
                    for k in 1..=10 {
                        let g = format!("cleanup{side}/cleanup{side}{k}");
                        v.push(p(leak(format!("{g}/path")), "Path", Value::Enum(0), mask_options()));
                        v.push(p(leak(format!("{g}/brushRadius")), "Brush Radius", num(10.0), slider(0.0, 1000.0, 0.0, 100.0, 1)));
                        v.push(p(leak(format!("{g}/brushPressure")), "Brush Pressure", num(80.0), pct()));
                    }
                }
                v.extend([
                    p("singleMaskHighlightRadius", "Single Mask Highlight Radius", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                    p("edgeThin", "Edge Thin", num(0.0), slider(-10.0, 10.0, -10.0, 10.0, 1)),
                    p("edgeFeather", "Edge Feather", num(0.0), slider(0.0, 100.0, 0.0, 50.0, 1)),
                    p("edgeThreshold", "Edge Threshold", num(0.0), pct()),
                    p("invertExtraction", "Invert Extraction", Value::Bool(false), ParamUi::Checkbox),
                    p("blendWithOriginal", "Blend with Original", num(0.0), pct()),
                ]);
                v
            },
            inner_outer_key,
        ),
        spec(
            "ec.key.unmult",
            "Unmult",
            vec![
                p("backgroundColor", "Background Color", Value::Enum(0), popup(&["Black", "White"])),
                p("blackLevel", "Black Level", num(0.0), pct()),
                p("whiteLevel", "White Level", num(100.0), pct()),
                p("softness", "Softness", num(100.0), pct()),
                p("removeColorMatting", "Remove Color Matting", Value::Bool(true), ParamUi::Checkbox),
                p("clipHdrResults", "Clip HDR Results", Value::Bool(true), ParamUi::Checkbox),
            ],
            unmult,
        ),
        spec(
            "ec.key.ccsimplewireremoval",
            "CC Simple Wire Removal",
            vec![
                p("pointA", "Point A", Value::Vec2([0.25, 0.25]), ParamUi::Point),
                p("pointB", "Point B", Value::Vec2([0.75, 0.75]), ParamUi::Point),
                p("removalStyle", "Removal Style", Value::Enum(2), popup(&["Fade", "Frame Offset", "Displace", "Displace Horizontal"])),
                p("thickness", "Thickness", num(5.0), slider(0.0, 100.0, 0.0, 30.0, 1)),
                p("slope", "Slope", num(0.0), pct()),
                p("mirrorBlend", "Mirror Blend", num(0.0), pct()),
                p("frameOffset", "Frame Offset", num(0.0), slider(-100.0, 100.0, -10.0, 10.0, 0)),
            ],
            wire_removal,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel2::tests::{FakeHost, sample};
    use crate::{EffectEnv, MaskShape, run_fx};
    use effectcraft_raster::Image;

    #[test]
    fn difference_matte_self_is_transparent_and_other_keeps() {
        let img = sample();
        let out = run_fx("ec.key.differencematte", &[], img.clone(), 0.0, EffectEnv::default());
        assert!(out.img.data.iter().all(|p| p[3] == 0.0));
        let host = FakeHost(Image::filled(8, 6, [0.0, 0.0, 0.4, 1.0]));
        let env = EffectEnv { host: Some(&host), ..Default::default() };
        let out = run_fx("ec.key.differencematte", &[("differenceLayer", Value::Layer(Some(7)))], img.clone(), 0.0, env);
        assert_eq!(out.img.get(5, 5), img.get(5, 5));
        assert_eq!(out.img.get(0, 0)[3], 0.0, "matching pixel keyed out");
        let again = run_fx("ec.key.differencematte", &[("differenceLayer", Value::Layer(Some(7)))], img.clone(), 0.0, env);
        assert_eq!(out.img.data, again.img.data);
    }

    #[test]
    fn inner_outer_key_inside_mask() {
        let img = Image::filled(20, 20, [0.8, 0.2, 0.2, 1.0]);
        let masks = vec![MaskShape { name: "Mask 1".into(), points: vec![[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]], closed: true, inverted: false }];
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let out = run_fx("ec.key.innerouter", &[("foreground", Value::Enum(1))], img.clone(), 0.0, env);
        assert!((out.img.get(10, 10)[3] - 1.0).abs() < 1e-6);
        assert_eq!(out.img.get(1, 1)[3], 0.0);
        // Two masks: unknown band estimated from colour.
        let mut img2 = Image::filled(20, 20, [0.0, 0.0, 1.0, 1.0]);
        for y in 0..20 {
            for x in 7..13 {
                img2.set(x, y, [1.0, 0.0, 0.0, 1.0]);
            }
        }
        let masks2 = vec![
            MaskShape { name: "in".into(), points: vec![[8.0, 0.0], [12.0, 0.0], [12.0, 20.0], [8.0, 20.0]], closed: true, inverted: false },
            MaskShape { name: "out".into(), points: vec![[4.0, 0.0], [16.0, 0.0], [16.0, 20.0], [4.0, 20.0]], closed: true, inverted: false },
        ];
        let env2 = EffectEnv { masks: &masks2, ..Default::default() };
        let o2 = run_fx("ec.key.innerouter", &[("foreground", Value::Enum(1)), ("background", Value::Enum(2))], img2.clone(), 0.0, env2);
        assert!(o2.img.get(7, 10)[3] > 0.8, "{:?}", o2.img.get(7, 10));
        assert!(o2.img.get(5, 10)[3] < 0.2, "{:?}", o2.img.get(5, 10));
        assert_eq!(o2.img.get(1, 10)[3], 0.0);
        let o3 = run_fx("ec.key.innerouter", &[("foreground", Value::Enum(1)), ("background", Value::Enum(2))], img2, 0.0, env2);
        assert_eq!(o2.img.data, o3.img.data);
    }

    #[test]
    fn unmult_black_becomes_transparent() {
        let mut img = Image::filled(4, 4, [0.0, 0.0, 0.0, 1.0]);
        img.set(1, 1, [0.5, 0.25, 0.0, 1.0]);
        let out = run_fx("ec.key.unmult", &[], img, 0.0, EffectEnv::default());
        assert_eq!(out.img.get(0, 0), [0.0; 4]);
        let p = out.img.get(1, 1);
        assert!((p[3] - 0.5).abs() < 1e-6 && (p[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unmult_white_background_levels_and_softness() {
        // White background: white goes transparent, a half-grey pixel half transparent with its
        // colour recovered as black.
        let mut img = Image::filled(3, 1, [1.0, 1.0, 1.0, 1.0]);
        img.set(1, 0, [0.5, 0.5, 0.5, 1.0]);
        img.set(2, 0, [0.0, 0.0, 0.0, 1.0]);
        let out = run_fx("ec.key.unmult", &[("backgroundColor", Value::Enum(1))], img.clone(), 0.0, EffectEnv::default());
        assert_eq!(out.img.get(0, 0), [0.0; 4]);
        let p = out.img.get(1, 0);
        assert!((p[3] - 0.5).abs() < 1e-5 && p[0].abs() < 1e-5, "{p:?}");
        assert_eq!(out.img.get(2, 0), [0.0, 0.0, 0.0, 1.0]);
        // Hard cutoff: Softness 0 with Black Level 40 % keeps a 50 % grey fully opaque, and
        // without Remove Color Matting the colour is untouched.
        let hard = run_fx(
            "ec.key.unmult",
            &[("blackLevel", num(40.0)), ("softness", num(0.0)), ("removeColorMatting", Value::Bool(false))],
            img,
            0.0,
            EffectEnv::default(),
        );
        assert_eq!(hard.img.get(1, 0), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(hard.img.get(2, 0), [0.0; 4]);
    }

    #[test]
    fn wire_removal_hides_a_wire() {
        let mut img = Image::filled(20, 20, [0.5, 0.5, 0.5, 1.0]);
        for y in 0..20 {
            img.set(10, y, [0.0, 0.0, 0.0, 1.0]);
        }
        for style in 0..4u32 {
            let out = run_fx(
                "ec.key.ccsimplewireremoval",
                &[("pointA", Value::Vec2([10.5, 0.0])), ("pointB", Value::Vec2([10.5, 20.0])), ("removalStyle", Value::Enum(style)), ("thickness", num(3.0))],
                img.clone(),
                0.0,
                EffectEnv::default(),
            );
            assert!((out.img.get(10, 10)[0] - 0.5).abs() < 0.05, "style {style}: {:?}", out.img.get(10, 10));
            assert_eq!(out.img.get(2, 10), img.get(2, 10));
        }
    }

    #[test]
    fn wire_removal_stays_between_a_and_b() {
        let mut img = Image::filled(20, 20, [0.5, 0.5, 0.5, 1.0]);
        for y in 0..20 {
            img.set(10, y, [0.0, 0.0, 0.0, 1.0]);
        }
        let out = run_fx(
            "ec.key.ccsimplewireremoval",
            &[("pointA", Value::Vec2([10.5, 5.0])), ("pointB", Value::Vec2([10.5, 15.0])), ("removalStyle", Value::Enum(0)), ("thickness", num(3.0))],
            img.clone(),
            0.0,
            EffectEnv::default(),
        );
        assert!((out.img.get(10, 10)[0] - 0.5).abs() < 0.05, "inside the segment");
        assert_eq!(out.img.get(10, 2), img.get(10, 2), "before A");
        assert_eq!(out.img.get(10, 18), img.get(10, 18), "after B");
    }

    #[test]
    fn wire_removal_frame_offset_uses_neighbour_frame() {
        use crate::{EffectHost, LayerPixels};
        struct Frames;
        impl EffectHost for Frames {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
                // Frame n (at 10 fps) is a flat grey of n / 10.
                let v = (t * 10.0).round() as f32 / 10.0;
                Some(Buf { img: Image::filled(20, 20, [v, v, v, 1.0]), offset: [0.0; 2], scale: 1.0 })
            }
        }
        let mut img = Image::filled(20, 20, [0.5, 0.5, 0.5, 1.0]);
        for y in 0..20 {
            img.set(10, y, [0.0, 0.0, 0.0, 1.0]);
        }
        let host = Frames;
        let run = |style: u32, off: f64| {
            let env = EffectEnv { host: Some(&host), frame_rate: 10.0, ..Default::default() };
            let vals = [
                ("pointA", Value::Vec2([10.5, 0.0])),
                ("pointB", Value::Vec2([10.5, 20.0])),
                ("removalStyle", Value::Enum(style)),
                ("thickness", num(3.0)),
                ("frameOffset", num(off)),
            ];
            run_fx("ec.key.ccsimplewireremoval", &vals, img.clone(), 0.5, env).img
        };
        // At 0.5 s (frame 5) offset -1 shows frame 4 and +1 shows frame 6 inside the strip.
        let (prev, next) = (run(1, -1.0), run(1, 1.0));
        assert!((prev.get(10, 10)[0] - 0.4).abs() < 1e-4, "{:?}", prev.get(10, 10));
        assert!((next.get(10, 10)[0] - 0.6).abs() < 1e-4, "{:?}", next.get(10, 10));
        // Outside the strip the current frame stays.
        assert_eq!(prev.get(2, 10), img.get(2, 10));
        // Displace ignores Frame Offset.
        assert_eq!(run(2, -1.0).data, run(2, 1.0).data);
    }

    #[test]
    fn inner_outer_key_more_masks_and_cleanup_strokes() {
        let sq = |x0: f64, x1: f64| MaskShape { name: String::new(), points: vec![[x0, 2.0], [x1, 2.0], [x1, 8.0], [x0, 8.0]], closed: true, inverted: false };
        // Masks 1..4: foreground squares; mask 5: a horizontal cleanup stroke across the middle.
        let masks = vec![
            sq(1.0, 5.0),
            sq(7.0, 11.0),
            sq(13.0, 17.0),
            sq(19.0, 23.0),
            MaskShape { name: String::new(), points: vec![[0.0, 5.0], [30.0, 5.0]], closed: false, inverted: false },
        ];
        let env = EffectEnv { masks: &masks, ..Default::default() };
        let img = Image::filled(30, 10, [0.5, 0.5, 0.5, 1.0]);
        let vals = [
            ("foreground", Value::Enum(1)),
            ("additionalForeground/foreground1", Value::Enum(2)),
            ("additionalForeground/foreground2", Value::Enum(3)),
            ("additionalForeground/foreground5", Value::Enum(4)),
        ];
        let out = run_fx("ec.key.innerouter", &vals, img.clone(), 0.0, env).img;
        for x in [3, 9, 15, 21] {
            assert_eq!(out.get(x, 3)[3], 1.0, "foreground square at {x}");
        }
        assert_eq!(out.get(27, 3)[3], 0.0);
        // Cleanup Background along mask 5 erases a band; Cleanup Foreground restores outside.
        let mut bg = vals.to_vec();
        bg.extend([
            ("cleanupBackground/cleanupBackground1/path", Value::Enum(5)),
            ("cleanupBackground/cleanupBackground1/brushRadius", num(1.5)),
            ("cleanupBackground/cleanupBackground1/brushPressure", num(100.0)),
        ]);
        let erased = run_fx("ec.key.innerouter", &bg, img.clone(), 0.0, env).img;
        assert!(erased.get(3, 5)[3] < 0.1 && erased.get(3, 2)[3] > 0.9);
        let mut fg = vals.to_vec();
        fg.extend([
            ("cleanupForeground/cleanupForeground3/path", Value::Enum(5)),
            ("cleanupForeground/cleanupForeground3/brushRadius", num(1.5)),
            ("cleanupForeground/cleanupForeground3/brushPressure", num(100.0)),
        ]);
        let painted = run_fx("ec.key.innerouter", &fg, img, 0.0, env).img;
        assert!(painted.get(27, 5)[3] > 0.9 && painted.get(27, 2)[3] < 0.1);
    }
}
