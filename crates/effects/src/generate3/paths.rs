use super::raster::{Seg, all_masks_px, chaikin, mask_px, over, poly_segs, raster_convex, raster_segs, scan_runs, stroke_outline, trim_poly};
use super::{
    Buf, EffectCtx, Image, IndexedParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator, Params, Plane, gauss_plane, hash1,
    layer_or_self, luminance, poly_length, smoothstep, unpremul,
};

/// How painted coverage meets the layer: 0 On Original Image, 1 On Transparent, 2 Reveal
/// Original Image.
pub(crate) fn paint(img: &mut Image, cov: &Plane, color: [f32; 4], opacity: f32, style: u32) {
    img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
        let a = (c * opacity * color[3]).clamp(0.0, 1.0);
        let s = [color[0] * a, color[1] * a, color[2] * a, a];
        *px = match style {
            1 => s,
            2 => [px[0] * a, px[1] * a, px[2] * a, px[3] * a],
            _ => over(*px, s),
        };
    });
}

/// Coverage of a plan-drawn generator: round-capped segments rasterised by `raster_segs`
/// with `hardness`, or the union of convex polygons (`raster_convex`).
#[derive(Clone, Debug)]
pub enum Coverage {
    Segs { segs: Vec<Seg>, hardness: f64 },
    Convex(Vec<Vec<[f64; 2]>>),
}

impl Coverage {
    fn raster(&self, w: usize, h: usize) -> Plane {
        match self {
            Coverage::Segs { segs, hardness } => raster_segs(w, h, segs, *hardness),
            Coverage::Convex(polys) => raster_convex(w, h, polys),
        }
    }
}

/// Stroke, Scribble, Vegas and Write-on, shared with the GPU compositor (effectcraft-gpu
/// `fx_gen2`): the coverage and how it is painted. `style` 0–2 as `paint` (On Original
/// Image, On Transparent, Reveal Original Image), 3 = under the layer (Vegas' Composite Under).
#[derive(Clone, Debug)]
pub struct PaintPlan {
    pub cov: Coverage,
    pub color: [f32; 4],
    pub opacity: f32,
    pub style: u32,
}

/// Draw a [`PaintPlan`].
fn draw_paint(b: &mut Buf, plan: &PaintPlan) {
    let cov = plan.cov.raster(b.img.width as usize, b.img.height as usize);
    if plan.style == 3 {
        let color = plan.color;
        b.img.data.par_iter_mut().zip(cov.data.par_iter()).for_each(|(px, &c)| {
            let a = c * color[3];
            *px = over([color[0] * a, color[1] * a, color[2] * a, a], *px);
        });
    } else {
        paint(&mut b.img, &cov, plan.color, plan.opacity, plan.style);
    }
}

/// Generators drawn from a plan that need the layer's own pixels (`b.img`) to build it.
pub fn plan_reads_pixels(id: &str, ctx: &EffectCtx) -> bool {
    match id {
        // Image Contours read the layer itself when no (resolvable) Input Layer is chosen.
        "ec.generate.vegas" => ctx.params.e("stroke") == 0,
        "ec.generate.paintbucket" | "ec.generate.eyedropperfill" => true,
        "ec.generate.advancedlightning" => ctx.params.f("alphaObstacle").clamp(-10.0, 10.0) != 0.0,
        _ => false,
    }
}

/// [`PaintPlan`] of Stroke, Scribble, Vegas or Write-on on `b`'s geometry (`b.img` is read
/// only when [`plan_reads_pixels`]). `None`: Scribble without a usable mask (the layer is
/// cleared when it composites On Transparent, else kept).
pub fn paint_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<PaintPlan> {
    match id {
        "ec.generate.stroke" => Some(stroke_plan(ctx, b)),
        "ec.generate.scribble" => scribble_plan(ctx, b),
        "ec.generate.vegas" => Some(vegas_plan(ctx, b)),
        "ec.generate.writeon" => Some(write_on_plan(ctx, b)),
        _ => None,
    }
}

/// Scribble's Composite as a `paint` style.
pub fn scribble_style(ctx: &EffectCtx) -> u32 {
    match ctx.params.e("composite") {
        0 => 0,
        2 => 2,
        _ => 1,
    }
}

pub(super) fn scribble(ctx: &EffectCtx, mut b: Buf) -> Buf {
    match scribble_plan(ctx, &b) {
        Some(plan) => draw_paint(&mut b, &plan),
        None if scribble_style(ctx) == 1 => b.img = Image::new(b.img.width, b.img.height),
        None => {}
    }
    b
}

fn scribble_plan(ctx: &EffectCtx, b: &Buf) -> Option<PaintPlan> {
    let pr = ctx.params;
    let which = pr.e("scribble");
    let polys: Vec<_> = if which == 0 { mask_px(ctx, b, pr.f("mask").round().max(1.0) as usize).into_iter().collect() } else { all_masks_px(ctx, b) };
    let polys: Vec<_> = polys.into_iter().filter(|p| p.0.len() >= 3).map(|(p, c, i)| (p, c, i && which == 2)).collect();
    let style = scribble_style(ctx);
    if polys.is_empty() {
        return None;
    }
    let sc = b.scale;
    let fill = pr.e("fillType");
    let ew = pr.f("edgeOptions/edgeWidth").max(0.1) * sc;
    let ang = pr.f("angle").to_radians();
    let d = [ang.cos(), ang.sin()];
    let n = [-d[1], d[0]];
    let width = (pr.f("strokeWidth") * sc).max(0.1);
    let curv = pr.f("strokeOptions/curviness") / 100.0;
    let curv_var = pr.f("strokeOptions/curvinessVariation") / 100.0;
    let spacing = (pr.f("strokeOptions/spacing") * sc).max(0.75).max(width * 0.25);
    let spacing_var = pr.f("strokeOptions/spacingVariation") / 100.0;
    let overlap = pr.f("strokeOptions/pathOverlap") / 100.0;
    let overlap_var = pr.f("strokeOptions/pathOverlapVariation") / 100.0;
    let start = pr.f("start").clamp(0.0, 100.0) / 100.0;
    let end = pr.f("end").clamp(0.0, 100.0) / 100.0;
    let sequential = pr.b("fillPathsSequentially");
    let wiggle = pr.e("wiggleType");
    let wps = pr.f("wigglesPerSecond").max(0.0);
    let seed = pr.f("randomSeed") as i64 as u32 ^ ctx.seed;
    let (step, frac) = match wiggle {
        0 => (0u32, 0.0f32),
        1 => ((ctx.time * wps).floor().max(0.0) as u32, 0.0),
        _ => {
            let u = (ctx.time * wps).max(0.0);
            (u.floor() as u32, smoothstep(0.0, 1.0, u.fract() as f32))
        }
    };
    let rnd = |i: u32, k: u32| {
        let a = hash1(i, k.wrapping_mul(31).wrapping_add(step.wrapping_mul(7919)), seed) * 2.0 - 1.0;
        if frac > 0.0 {
            let c = hash1(i, k.wrapping_mul(31).wrapping_add((step + 1).wrapping_mul(7919)), seed) * 2.0 - 1.0;
            a + (c - a) * frac
        } else {
            a
        }
    };
    let groups: Vec<Vec<(Vec<[f64; 2]>, bool, bool)>> =
        if which == 0 || sequential { vec![polys.clone()] } else { polys.iter().map(|p| vec![p.clone()]).collect() };
    // Per group a zig-zag polyline (in buffer px).
    let mut lines: Vec<Vec<Vec<[f64; 2]>>> = Vec::new();
    let mut runs = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let mut nmin = f64::INFINITY;
        let mut nmax = f64::NEG_INFINITY;
        for (pts, _, _) in g {
            for q in pts {
                let v = q[0] * n[0] + q[1] * n[1];
                nmin = nmin.min(v);
                nmax = nmax.max(v);
            }
        }
        nmin -= ew;
        nmax += ew;
        // Chains of zig-zags: each run joins the nearest chain that ended on the previous
        // scan line (so separate regions get separate strokes).
        let mut chains: Vec<(Vec<[f64; 2]>, u32)> = Vec::new();
        let mut s = nmin + spacing * 0.5;
        let mut li = 0u32;
        let reach = spacing * 3.0 + ew;
        while s < nmax && li < 20_000 {
            scan_runs(g, fill, ew, d, n, s, &mut runs);
            for (ri, &(a, bb)) in runs.iter().enumerate() {
                let k = (gi as u32) << 20 | ri as u32;
                let ov = spacing * (overlap + overlap_var * rnd(li, k * 4) as f64 * 0.5).max(-0.45);
                let ov2 = spacing * (overlap + overlap_var * rnd(li, k * 4 + 1) as f64 * 0.5).max(-0.45);
                let (a, bb) = (a - ov, bb + ov2);
                if bb <= a {
                    continue;
                }
                let at = |t: f64| [d[0] * t + n[0] * s, d[1] * t + n[1] * s];
                let (pa, pb) = (at(a), at(bb));
                let dist = |p: [f64; 2], q: [f64; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
                let best = chains
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| li > 0 && c.1 == li - 1)
                    .map(|(i, c)| {
                        let last = *c.0.last().unwrap_or(&pa);
                        (i, dist(last, pa).min(dist(last, pb)))
                    })
                    .filter(|&(_, dd)| dd < reach)
                    .min_by(|x, y| x.1.total_cmp(&y.1));
                match best {
                    Some((ci, _)) => {
                        let last = *chains[ci].0.last().unwrap_or(&pa);
                        let (p0, p1, dir) = if dist(last, pa) <= dist(last, pb) { (pa, pb, -1.0) } else { (pb, pa, 1.0) };
                        // Curved turn: an extra point pushed past the edge.
                        let c = (curv + curv_var * rnd(li, k * 4 + 2) as f64 * 0.5).max(0.0);
                        let mid = [(last[0] + p0[0]) * 0.5 + d[0] * dir * c * spacing, (last[1] + p0[1]) * 0.5 + d[1] * dir * c * spacing];
                        let ch = &mut chains[ci];
                        ch.0.extend([mid, p0, p1]);
                        ch.1 = li;
                    }
                    None => chains.push((vec![pa, pb], li)),
                }
            }
            s += spacing * (1.0 + spacing_var * rnd(li, 3) as f64 * 0.5).max(0.25);
            li += 1;
        }
        lines.push(chains.into_iter().map(|(z, _)| if curv > 0.0 { chaikin(&z, 2) } else { z }).collect());
    }
    let r = width * 0.5;
    // Start / End Apply To: all of a mask's strokes as one path, or each stroke on its own.
    let each = pr.e("startEndApplyTo") == 1;
    let mut parts: Vec<Vec<[f64; 2]>> = Vec::new();
    for group in &lines {
        let lens: Vec<f64> = group.iter().map(|l| poly_length(l, false)).collect();
        let total: f64 = lens.iter().sum();
        let (g0, g1) = (total * start, total * end);
        let mut acc = 0.0;
        for (l, len) in group.iter().zip(lens) {
            let part =
                if each { trim_poly(l, false, len * start, len * end) } else { trim_poly(l, false, (g0 - acc).clamp(0.0, len), (g1 - acc).clamp(0.0, len)) };
            acc += len;
            parts.push(part);
        }
    }
    let cap = pr.e("edgeOptions/endCap");
    let join = pr.e("edgeOptions/join");
    let cov = if cap == 0 && join == 0 {
        let mut segs = Vec::new();
        for part in &parts {
            poly_segs(part, false, r, 1.0, &mut segs);
        }
        Coverage::Segs { segs, hardness: 0.9 }
    } else {
        Coverage::Convex(stroke_outline(&parts, r, cap, join, pr.f("edgeOptions/miterLimit").max(1.0)))
    };
    Some(PaintPlan { cov, color: pr.color("color"), opacity: (pr.f("opacity") / 100.0) as f32, style })
}

// ---------------------------------------------------------------- Stroke

pub(super) fn stroke(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = stroke_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

fn stroke_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let all = pr.b("allMasks");
    let polys: Vec<_> = if all { all_masks_px(ctx, b) } else { mask_px(ctx, b, pr.f("path").round() as usize).into_iter().collect() };
    let style = pr.e("paintStyle");
    let size = (pr.f("brushSize") * b.scale).max(0.1);
    let r = size * 0.5;
    let hard = pr.f("brushHardness") / 100.0;
    let start = pr.f("start").clamp(0.0, 100.0) / 100.0;
    let end = pr.f("end").clamp(0.0, 100.0) / 100.0;
    let spacing = pr.f("spacing") / 100.0;
    let sequential = pr.b("strokeSequentially");
    let lens: Vec<f64> = polys.iter().map(|(p, c, _)| poly_length(p, *c)).collect();
    let total: f64 = lens.iter().sum();
    let mut segs = Vec::new();
    let mut acc = 0.0;
    for ((pts, closed, _), &len) in polys.iter().zip(&lens) {
        let (s0, s1) = if all && sequential {
            let (g0, g1) = (total * start, total * end);
            ((g0 - acc).clamp(0.0, len), (g1 - acc).clamp(0.0, len))
        } else {
            (len * start, len * end)
        };
        acc += len;
        if s1 <= s0 {
            continue;
        }
        if spacing <= 0.5 {
            let part = trim_poly(pts, *closed, s0, s1);
            poly_segs(&part, false, r, 1.0, &mut segs);
        } else {
            let step = (size * spacing).max(0.5);
            let mut s = s0;
            while s <= s1 + 1e-9 && segs.len() < 200_000 {
                let (q, _) = crate::util::poly_point_at(pts, *closed, s);
                segs.push(Seg { a: q, b: q, r, v: 1.0 });
                s += step;
            }
        }
    }
    PaintPlan { cov: Coverage::Segs { segs, hardness: hard }, color: pr.color("color"), opacity: (pr.f("opacity") / 100.0) as f32, style }
}

// ---------------------------------------------------------------- Vegas

/// Marching-squares iso-contours of `pl` at `thr`, chained into polylines (closed flag).
pub(super) fn contours(pl: &Plane, thr: f32) -> Vec<(Vec<[f64; 2]>, bool)> {
    use std::collections::HashMap;
    let (w, h) = (pl.w, pl.h);
    if w < 2 || h < 2 {
        return Vec::new();
    }
    // Edge key: horizontal edge (x,y)-(x+1,y) = 2*(y*w+x), vertical (x,y)-(x,y+1) = 2*(y*w+x)+1.
    let hk = |x: usize, y: usize| 2 * (y * w + x) as u64;
    let vk = |x: usize, y: usize| 2 * (y * w + x) as u64 + 1;
    let mut pos: HashMap<u64, [f64; 2]> = HashMap::new();
    let mut adj: HashMap<u64, Vec<u64>> = HashMap::new();
    let v = |x: usize, y: usize| pl.data[y * w + x];
    let interp = |a: f32, b: f32| if (b - a).abs() > 1e-9 { ((thr - a) / (b - a)).clamp(0.0, 1.0) as f64 } else { 0.5 };
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let (a, bb, c, d) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
            let idx = (a > thr) as u8 | ((bb > thr) as u8) << 1 | ((c > thr) as u8) << 2 | ((d > thr) as u8) << 3;
            if idx == 0 || idx == 15 {
                continue;
            }
            // Edge points (pixel centres at +0.5).
            let top = (hk(x, y), [x as f64 + 0.5 + interp(a, bb), y as f64 + 0.5]);
            let right = (vk(x + 1, y), [x as f64 + 1.5, y as f64 + 0.5 + interp(bb, c)]);
            let bottom = (hk(x, y + 1), [x as f64 + 0.5 + interp(d, c), y as f64 + 1.5]);
            let left = (vk(x, y), [x as f64 + 0.5, y as f64 + 0.5 + interp(a, d)]);
            let pairs: &[(usize, usize)] = match idx {
                1 | 14 => &[(3, 0)],
                2 | 13 => &[(0, 1)],
                3 | 12 => &[(3, 1)],
                4 | 11 => &[(1, 2)],
                5 => &[(3, 0), (1, 2)],
                6 | 9 => &[(0, 2)],
                7 | 8 => &[(3, 2)],
                10 => &[(0, 1), (2, 3)],
                _ => &[],
            };
            let e = [top, right, bottom, left];
            for &(i, j) in pairs {
                pos.insert(e[i].0, e[i].1);
                pos.insert(e[j].0, e[j].1);
                adj.entry(e[i].0).or_default().push(e[j].0);
                adj.entry(e[j].0).or_default().push(e[i].0);
            }
        }
    }
    let mut keys: Vec<u64> = adj.keys().copied().collect();
    keys.sort_unstable();
    // Start open chains at endpoints (degree 1) first, then closed loops.
    keys.sort_by_key(|k| adj[k].len() != 1);
    let mut used: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &k0 in &keys {
        for &first in &adj[&k0] {
            if used.contains(&(k0.min(first), k0.max(first))) {
                continue;
            }
            let mut chain = vec![pos[&k0]];
            let (mut prev, mut cur) = (k0, first);
            used.insert((prev.min(cur), prev.max(cur)));
            let mut closed = false;
            loop {
                chain.push(pos[&cur]);
                if cur == k0 {
                    closed = true;
                    chain.pop();
                    break;
                }
                let next = adj[&cur].iter().copied().find(|&n| n != prev && !used.contains(&(cur.min(n), cur.max(n))));
                match next {
                    Some(n) => {
                        used.insert((cur.min(n), cur.max(n)));
                        prev = cur;
                        cur = n;
                    }
                    None => break,
                }
            }
            if chain.len() >= 2 {
                out.push((chain, closed));
            }
        }
    }
    out
}

pub(super) fn vegas(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = vegas_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

fn vegas_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let sc = b.scale;
    let paths: Vec<(Vec<[f64; 2]>, bool)> = if pr.e("stroke") == 1 {
        mask_px(ctx, b, pr.f("path").round() as usize).map(|(p, c, _)| vec![(p, c)]).unwrap_or_default()
    } else {
        let src = layer_or_self(ctx, b, "inputLayer", true, pr.e("ifLayerSizesDiffer") == 1);
        let ch = pr.e("channel");
        let inv = pr.b("invertInput");
        let mut pl = Plane::from_image(&src, |px| {
            let (c, a) = unpremul(px);
            let v = match ch {
                1 => c[0],
                2 => c[1],
                3 => c[2],
                4 => a,
                _ => luminance(c[0], c[1], c[2]) * a,
            };
            if inv { 1.0 - v } else { v }
        });
        let blur = pr.f("preBlur") * sc;
        if blur > 0.05 {
            pl = gauss_plane(&pl, blur, blur);
        }
        let mut cs = contours(&pl, (pr.f("threshold") / 100.0) as f32);
        // Simplify by tolerance (drop points closer than tol × size).
        let tol = pr.f("tolerance").max(0.0) * (w.max(h) as f64) * 0.1;
        if tol > 0.0 {
            for (c, _) in cs.iter_mut() {
                let mut keep: Vec<[f64; 2]> = Vec::with_capacity(c.len());
                for &q in c.iter() {
                    if keep.last().is_none_or(|l: &[f64; 2]| ((l[0] - q[0]).powi(2) + (l[1] - q[1]).powi(2)).sqrt() >= tol) {
                        keep.push(q);
                    }
                }
                *c = keep;
            }
        }
        cs.sort_by(|a, b| poly_length(&b.0, b.1).total_cmp(&poly_length(&a.0, a.1)));
        if pr.e("renderMode") == 1 {
            let sel = (pr.f("selectedContour").round().max(1.0) as usize) - 1;
            cs.into_iter().skip(sel).take(1).collect()
        } else {
            cs
        }
    };
    let nseg = pr.f("segments").round().clamp(1.0, 1000.0) as usize;
    // Shorter Contours Have Fewer Segments: segment count scales with contour length.
    let fewer = pr.e("stroke") == 0 && pr.e("shorterContoursHave") == 1;
    let longest = paths.iter().map(|(p, c)| poly_length(p, *c)).fold(0.0, f64::max);
    let seg_len = pr.f("length").clamp(0.0, 1.0);
    let bunched = pr.e("segmentDistribution") == 0;
    let rot = pr.f("rotation") / 360.0;
    let random_phase = pr.b("randomPhase");
    let seed = pr.f("randomSeed") as i64 as u32 ^ ctx.seed;
    let width = (pr.f("width") * sc).max(0.1);
    let hard = pr.f("hardness").clamp(0.0, 1.0);
    let (o0, om, o1, mp) = (pr.f("startOpacity") as f32, pr.f("midpointOpacity") as f32, pr.f("endOpacity") as f32, pr.f("midpointPosition").clamp(0.01, 0.99));
    let mut segs = Vec::new();
    for (ci, (pts, closed)) in paths.iter().enumerate() {
        let len = poly_length(pts, *closed);
        if len <= 0.0 {
            continue;
        }
        let phase = rot + if random_phase { hash1(ci as u32, 1, seed) as f64 } else { 0.0 };
        let nseg = if fewer && longest > 0.0 { ((nseg as f64 * len / longest).round() as usize).max(1) } else { nseg };
        let each = len / nseg as f64 * seg_len;
        for i in 0..nseg {
            let base = if bunched { (i as f64 + (hash1(ci as u32, i as u32 + 7, seed) as f64 - 0.5) * 0.8) / nseg as f64 } else { i as f64 / nseg as f64 };
            let s0 = (base + phase).rem_euclid(1.0) * len;
            // Sub-sample the segment so opacity can ramp along it.
            let sub = 12usize;
            let mut prev: Option<[f64; 2]> = None;
            for k in 0..=sub {
                let f = k as f64 / sub as f64;
                let mut s = s0 + each * f;
                if *closed {
                    s = s.rem_euclid(len);
                } else if s > len {
                    break;
                }
                let (q, _) = crate::util::poly_point_at(pts, *closed, s);
                let op = if f < mp { o0 + (om - o0) * (f / mp) as f32 } else { om + (o1 - om) * ((f - mp) / (1.0 - mp)) as f32 };
                if let Some(pv) = prev
                    && ((pv[0] - q[0]).powi(2) + (pv[1] - q[1]).powi(2)) < (each * 0.5).powi(2) + 4.0
                {
                    segs.push(Seg { a: pv, b: q, r: width * 0.5, v: op.clamp(0.0, 1.0) });
                }
                prev = Some(q);
            }
        }
    }
    let style = match pr.e("blendMode") {
        1 => 0,
        2 => 3,
        3 => 2,
        _ => 1,
    };
    PaintPlan { cov: Coverage::Segs { segs, hardness: hard }, color: pr.color("color"), opacity: 1.0, style }
}

// ---------------------------------------------------------------- Write-on / Glue Gun

/// Write-on draws a stroke along the animated Brush Position, as in After Effects: a dab every
/// Brush Spacing seconds from the layer's start up to now, each one lasting Stroke Length
/// seconds (0: for ever). The dabs' positions come from the host's parameters at their times;
/// with Paint Time Properties ▸ Opacity or Brush Time Properties ▸ Size each dab keeps its own
/// opacity or size, else the current ones apply to every dab. (Colour and hardness are one per
/// stroke: the current ones.) Without a host (or dabs) the current dab draws. Deterministic.
pub(super) fn write_on(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let plan = write_on_plan(ctx, &b);
    draw_paint(&mut b, &plan);
    b
}

/// The most dabs one Write-on frame draws; longer strokes lay them further apart, joined.
const WRITE_ON_MAX_DABS: i64 = 4096;

fn write_on_plan(ctx: &EffectCtx, b: &Buf) -> PaintPlan {
    let pr = ctx.params;
    let own_opacity = pr.e("paintTimeProperties") == 1;
    let own_size = matches!(pr.e("brushTimeProperties"), 1 | 3);
    let opacity = |p: &Params| (p.f("brushOpacity") / 100.0).clamp(0.0, 1.0) as f32;
    let radius = |p: &Params| (p.f("brushSize") * b.scale * 0.5).max(0.05);
    let dab = |p: &Params| {
        let q = b.to_px(p.v2("brushPosition"));
        ([q.0, q.1], radius(if own_size { p } else { pr }), if own_opacity { opacity(p) } else { 1.0 })
    };
    let mut segs = vec![];
    let t = ctx.time;
    let spacing = pr.f("brushSpacing").max(0.001);
    let life = pr.f("strokeLength").max(0.0);
    if let Some(host) = ctx.env.host
        && t.is_finite()
        && t >= 0.0
    {
        let start = if life > 0.0 { (t - life).max(0.0) } else { 0.0 };
        let k0 = (start / spacing - 1e-9).ceil().max(0.0) as i64;
        let k1 = (t / spacing + 1e-9).floor().max(0.0) as i64;
        let n = k1.saturating_sub(k0).saturating_add(1);
        let stride = (n / WRITE_ON_MAX_DABS).saturating_add(1).max(1);
        let mut prev: Option<[f64; 2]> = None;
        let mut k = k0;
        while k <= k1 {
            let tk = k as f64 * spacing;
            let at = if (tk - t).abs() < 1e-9 { Some(dab(pr)) } else { host.params_at(tk).map(|p| dab(&p)) };
            if let Some((q, r, v)) = at {
                // Dabs laid further apart than the brush spacing are joined into a line.
                let a = if stride > 1 { prev.unwrap_or(q) } else { q };
                segs.push(Seg { a, b: q, r, v });
                prev = Some(q);
            }
            k = k.saturating_add(stride);
        }
    }
    if segs.is_empty() {
        let (q, r, v) = dab(pr);
        segs.push(Seg { a: q, b: q, r, v });
    }
    PaintPlan {
        cov: Coverage::Segs { segs, hardness: pr.f("brushHardness") / 100.0 },
        color: pr.color("color"),
        opacity: if own_opacity { 1.0 } else { opacity(pr) },
        style: pr.e("paintStyle"),
    }
}
