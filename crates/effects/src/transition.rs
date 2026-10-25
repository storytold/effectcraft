//! Transition effects (batch 2): Block Dissolve, Gradient Wipe (gradient layer or self), Iris Wipe, Card
//! Wipe (3D cards with camera, lighting and jitter), CC Grid Wipe, CC Radial ScaleWipe, CC Scale Wipe and CC Light Wipe.
//!
//! Every transition is the identity at 0 % completion. Angles are clockwise from "up".

use effectcraft_color::luminance;
use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Px;
use rayon::prelude::*;

use crate::sim2::{Piece, PiecePlan, PieceTex, rot_axis, sort_far_first};
use crate::util::{Plane, gauss_plane, hash1, layer_rect, remap, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, ParamSpec, col, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Transition", params, render, gpu: false, float: true }
}

fn completion(ctx: &EffectCtx) -> f64 {
    (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0)
}

#[inline]
fn scale_px(px: &mut Px, k: f32) {
    for c in px.iter_mut() {
        *c *= k;
    }
}

/// Resample `other` into `b`'s pixel grid with placement `mode` (Gradient Wipe's Gradient
/// Placement, Texturize's Texture Placement): 0 tile from the top-left, 1 centre once,
/// 2 stretch to fit.
pub fn place_layer(ctx: &EffectCtx, b: &Buf, other: &crate::LayerPixels, mode: u32) -> effectcraft_raster::Image {
    let (ls, os) = (ctx.layer_size, other.size);
    let inv = 1.0 / b.scale.max(1e-9);
    crate::util::gen_image(b.img.width, b.img.height, |x, y| {
        let (px, py) = ((x as f64 + 0.5 - b.offset[0]) * inv, (y as f64 + 0.5 - b.offset[1]) * inv);
        let (qx, qy) = match mode {
            0 if os[0] >= 1.0 && os[1] >= 1.0 => (px.rem_euclid(os[0]), py.rem_euclid(os[1])),
            1 => (px + (os[0] - ls[0]) * 0.5, py + (os[1] - ls[1]) * 0.5),
            _ if ls[0] > 0.0 && ls[1] > 0.0 => (px * os[0] / ls[0], py * os[1] / ls[1]),
            _ => (px, py),
        };
        other.buf.img.sample_bilinear(qx * other.buf.scale + other.buf.offset[0], qy * other.buf.scale + other.buf.offset[1])
    })
}

/// Largest distance from `c` to a corner of the buffer.
fn max_corner_dist(b: &Buf, c: (f64, f64), metric: impl Fn(f64, f64) -> f64) -> f64 {
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| metric(x - c.0, y - c.1)).fold(1e-6, f64::max)
}

fn block_dissolve(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = completion(ctx) as f32;
    if done <= 0.0 {
        return b;
    }
    let bw = (ctx.params.f("blockWidth") * b.scale).max(1e-3);
    let bh = (ctx.params.f("blockHeight") * b.scale).max(1e-3);
    let feather = ctx.params.f("feather") * b.scale;
    let (ox, oy) = (b.offset[0], b.offset[1]);
    let seed = ctx.seed ^ 0xb10c;
    // Soft Edges (Best Quality): block edges falling between pixels are anti-aliased (4×4
    // sub-pixel samples); off, each pixel takes its centre's block.
    let soft = ctx.params.b("softEdges");
    let subs: Vec<f64> = if soft { (0..4).map(|k| (k as f64 + 0.5) / 4.0).collect() } else { vec![0.5] };
    let kept = |x: f64, y: f64| {
        let (bx, by) = (((x - ox) / bw).floor() as i64, ((y - oy) / bh).floor() as i64);
        hash1(bx as u32, by as u32, seed) >= done
    };
    let mut mask = Plane::new(b.img.width as usize, b.img.height as usize);
    let w = mask.w;
    let n = (subs.len() * subs.len()) as f32;
    mask.data.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, row)| {
        for (x, m) in row.iter_mut().enumerate() {
            let hits = subs.iter().flat_map(|sy| subs.iter().map(move |sx| (*sx, *sy))).filter(|(sx, sy)| kept(x as f64 + sx, y as f64 + sy)).count();
            *m = hits as f32 / n;
        }
    });
    if feather > 0.0 {
        mask = gauss_plane(&mask, feather * 0.5, feather * 0.5);
    }
    b.img.mul_mask(&mask.data);
    b
}

fn gradient_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = completion(ctx) as f32;
    if done <= 0.0 {
        return b;
    }
    let soft = (ctx.params.f("softness") / 100.0) as f32;
    let invert = ctx.params.b("invert");
    // The gradient layer (the layer itself when none is chosen), placed over this one.
    let grad = match ctx.layer_param("gradientLayer", false) {
        Some(o) => place_layer(ctx, &b, &o, ctx.params.e("gradientPlacement")),
        None => b.img.clone(),
    };
    b.img.data.par_iter_mut().zip(grad.data.par_iter()).for_each(|(px, g)| {
        let (c, _) = unpremul(*g);
        let mut t = luminance(c[0], c[1], c[2]).clamp(0.0, 1.0);
        if invert {
            t = 1.0 - t;
        }
        let k = if done >= 1.0 {
            0.0
        } else if soft > 0.0 {
            ((t - (done * (1.0 + soft) - soft)) / soft).clamp(0.0, 1.0)
        } else if t >= done {
            1.0
        } else {
            0.0
        };
        scale_px(px, k);
    });
    b
}

#[inline]
fn cross(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn iris_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let outer = ctx.params.f("outerRadius").max(0.0) * b.scale;
    if outer <= 0.0 {
        return b;
    }
    let n = ctx.params.f("points").round().clamp(3.0, 64.0) as usize;
    let inner = if ctx.params.b("useInnerRadius") { ctx.params.f("innerRadius").max(0.0) * b.scale } else { outer };
    let star = ctx.params.b("useInnerRadius");
    let rot = ctx.params.f("rotation").to_radians();
    let feather = ctx.params.f("feather") * b.scale;
    let c = b.to_px(ctx.params.v2("center"));
    let m = if star { n * 2 } else { n };
    let step = std::f64::consts::TAU / m as f64;
    let verts: Vec<(f64, f64)> = (0..m)
        .map(|i| {
            let r = if star && i % 2 == 1 { inner } else { outer };
            let a = rot + i as f64 * step;
            (r * a.sin(), -r * a.cos())
        })
        .collect();
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let d = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let r = d.0.hypot(d.1);
            let rb = if r < 1e-9 {
                outer.min(inner)
            } else {
                let a = (d.0.atan2(-d.1) - rot).rem_euclid(std::f64::consts::TAU);
                let i = ((a / step).floor() as usize).min(m - 1);
                let (vi, vj) = (verts[i], verts[(i + 1) % m]);
                let e = (vj.0 - vi.0, vj.1 - vi.1);
                let dir = (d.0 / r, d.1 / r);
                let den = cross(dir, e);
                if den.abs() < 1e-12 { outer } else { cross(vi, e) / den }
            };
            let k = if feather > 0.0 {
                ((r - rb) / feather + 0.5).clamp(0.0, 1.0)
            } else if r >= rb {
                1.0
            } else {
                0.0
            };
            scale_px(px, k as f32);
        }
    });
    b
}

/// Card Wipe: the layer is cut into Rows × Columns cards that flip over in Flip Order, each
/// through a 180° turn about Flip Axis, as textured planes in 3D seen through the Camera
/// System and lit by Lighting / Material. The flipped side shows Back Layer (none: the cards
/// vanish as they turn away). Position / Rotation Jitter wobble the cards over time.
fn card_wipe(ctx: &EffectCtx, b: Buf) -> Buf {
    match card_wipe_plan(ctx, &b) {
        Some(plan) => plan.finish(b),
        None => b,
    }
}

/// Card Wipe's cards (`None` = nothing has moved: the layer as is).
pub(crate) fn card_wipe_plan(ctx: &EffectCtx, b: &Buf) -> Option<PiecePlan> {
    let done = completion(ctx);
    let pr = ctx.params;
    let f = |id: &str| pr.get(id).map(Value::as_f64).unwrap_or(0.0);
    let jitter_on = [
        "positionJitter/xJitterAmount",
        "positionJitter/yJitterAmount",
        "positionJitter/zJitterAmount",
        "rotationJitter/xRotJitterAmount",
        "rotationJitter/yRotJitterAmount",
        "rotationJitter/zRotJitterAmount",
    ]
    .iter()
    .any(|id| f(id) != 0.0);
    if done <= 0.0
        && !jitter_on
        && pr.e("cameraSystem") == 0
        && f("cameraPosition/xRotation") == 0.0
        && f("cameraPosition/yRotation") == 0.0
        && f("cameraPosition/zRotation") == 0.0
    {
        // Nothing has moved: the layer as is (exactly).
        let xy = pr.get("cameraPosition/xyPosition").map(|v| v.as_vec2());
        if xy.is_none_or(|p| (p[0] - ctx.layer_size[0] * 0.5).abs() < 1e-9 && (p[1] - ctx.layer_size[1] * 0.5).abs() < 1e-9) {
            return None;
        }
    }
    let tw = (pr.f("transitionWidth") / 100.0).clamp(0.01, 1.0);
    let rows = pr.f("rows").round().clamp(1.0, 1000.0) as i64;
    // Rows & Columns: Independent, or Columns Follows Rows.
    let cols = if pr.e("rowsAndColumns") == 1 { rows } else { pr.f("columns").round().clamp(1.0, 1000.0) as i64 };
    let card_scale = pr.f("cardScale").max(0.01);
    let axis = pr.e("flipAxis");
    let direction = pr.e("flipDirection");
    let order = pr.e("flipOrder");
    let jitter = pr.f("timingRandomness").clamp(0.0, 1.0);
    let seed = pr.f("randomSeed") as u32 ^ 0xca7d;
    let (x0, y0, w, h) = layer_rect(ctx, b);
    let (cw, ch) = (w / cols as f64, h / rows as f64);
    let nx = |i: i64| if cols > 1 { i as f64 / (cols - 1) as f64 } else { 0.0 };
    let ny = |j: i64| if rows > 1 { j as f64 / (rows - 1) as f64 } else { 0.0 };
    // Flip Order "Gradient": cards flip first where the gradient layer (default: this layer) is dark.
    let grad = (order == 8).then(|| match ctx.layer_param("gradientLayer", false) {
        Some(o) => place_layer(ctx, b, &o, 2),
        None => b.img.clone(),
    });
    // Back Layer: a chosen layer (stretched to fit), or this layer for projects saved with the
    // old "Self" choice; none hides the cards' backs.
    let back = match ctx.layer_param("backLayer", true) {
        Some(o) => Some(PieceTex::Image(crate::util::fit_layer(ctx, b, &o, true))),
        None if pr.b("backSelf") => Some(PieceTex::Layer),
        None => None,
    };
    let cam = crate::card3d::projection(ctx, b);
    let eye = cam.eye();
    let light = crate::card3d::Lighting::from(ctx, b);
    let t = ctx.time;
    let jit = |i: i64, j: i64, k: u32, speed: f64| -> f64 {
        let ph = hash1(i as u32, j as u32, seed ^ k) as f64 * std::f64::consts::TAU;
        let rate = 0.5 + hash1(i as u32, j as u32, seed ^ (k + 101)) as f64;
        (t * speed * rate * std::f64::consts::TAU + ph).sin()
    };
    let (jx, jy, jz, js) =
        (f("positionJitter/xJitterAmount"), f("positionJitter/yJitterAmount"), f("positionJitter/zJitterAmount"), f("positionJitter/jitterSpeed"));
    let (rjx, rjy, rjz, rjs) =
        (f("rotationJitter/xRotJitterAmount"), f("rotationJitter/yRotJitterAmount"), f("rotationJitter/zRotJitterAmount"), f("rotationJitter/rotJitterSpeed"));
    let mut pieces: Vec<Piece> = (0..rows * cols)
        .into_par_iter()
        .filter_map(|k| {
            let (i, j) = (k % cols, k / cols);
            let (ccx, ccy) = (x0 + (i as f64 + 0.5) * cw, y0 + (j as f64 + 0.5) * ch);
            let mut o = match order {
                1 => 1.0 - nx(i),
                2 => ny(j),
                3 => 1.0 - ny(j),
                4 => (nx(i) + ny(j)) * 0.5,
                5 => (1.0 - nx(i) + ny(j)) * 0.5,
                6 => (nx(i) + 1.0 - ny(j)) * 0.5,
                7 => 1.0 - (nx(i) + ny(j)) * 0.5,
                8 => {
                    let g = grad.as_ref().map(|g| g.get_clamped(ccx as i64, ccy as i64)).unwrap_or([0.0; 4]);
                    let (c, _) = unpremul(g);
                    luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) as f64
                }
                _ => nx(i),
            };
            if jitter > 0.0 {
                o += (hash1(i as u32, j as u32, seed ^ 0x7177) as f64 - o) * jitter;
            }
            let p = ((done - o * (1.0 - tw)) / tw).clamp(0.0, 1.0);
            let around_x = match axis {
                1 => false,
                2 => hash1(i as u32, j as u32, seed ^ 0x55) < 0.5,
                _ => true,
            };
            let sign = match direction {
                1 => -1.0,
                2 if hash1(i as u32, j as u32, seed ^ 0x99) < 0.5 => -1.0,
                _ => 1.0,
            };
            let a = p * std::f64::consts::PI * sign;
            let flip = if around_x { rot_axis([1.0, 0.0, 0.0], a) } else { rot_axis([0.0, 1.0, 0.0], a) };
            let jr = crate::card3d::rot_ordered(
                (rjx * jit(i, j, 11, rjs)).to_radians(),
                (rjy * jit(i, j, 12, rjs)).to_radians(),
                (rjz * jit(i, j, 13, rjs)).to_radians(),
                0,
            );
            let r = crate::card3d::mat_mul(&jr, &flip);
            let pos = [ccx + jx * jit(i, j, 21, js) * cw, ccy + jy * jit(i, j, 22, js) * ch, jz * jit(i, j, 23, js) * cw.max(ch)];
            // Card Scale: each card is scaled about its centre (gaps below 1, cropped above 1).
            let (sw, sh) = ((cw * 0.5 * card_scale.min(1.0)) as f32, (ch * 0.5 * card_scale.min(1.0)) as f32);
            let (cx, cy) = (ccx as f32, ccy as f32);
            let poly = [[cx - sw, cy - sh], [cx + sw, cy - sh], [cx + sw, cy + sh], [cx - sw, cy + sh]];
            // Above 1 the card's picture is magnified (cropped to the card).
            let k = card_scale.max(1.0);
            let rs: crate::card3d::M3 = std::array::from_fn(|q| [r[q][0] * k, r[q][1] * k, r[q][2]]);
            let poly = poly.map(|v| [cx + (v[0] - cx) / k as f32, cy + (v[1] - cy) / k as f32]);
            let mut pc = Piece::new(&poly, [cx, cy], &rs, pos, cam)?;
            if pc.back && back.is_none() {
                return None;
            }
            pc.back_mirror = if around_x { 1 } else { 2 };
            pc.light(&light, &r, pos, eye);
            Some(pc)
        })
        .collect();
    sort_far_first(&mut pieces);
    Some(PiecePlan { pieces, front: PieceTex::Layer, back })
}

/// Double-precision grid geometry, shared with the GPU mask upload. A tiny
/// rounding error at a tile boundary can otherwise flip a fully opaque pixel.
pub struct GridWipe {
    c: (f64, f64),
    s: f64,
    co: f64,
    cs: f64,
    maxd: f64,
    shape: u32,
    reverse: bool,
    done: f64,
    border: f64,
}

impl GridWipe {
    /// Construct from buffer geometry; no pixel data is accessed.
    pub fn new(ctx: &EffectCtx, b: &Buf) -> Self {
        let done = completion(ctx);
        let c = b.to_px(ctx.params.v2("center"));
        let rot = ctx.params.f("rotation").to_radians();
        let border = ctx.params.f("border").max(0.0) * b.scale;
        let tiles = ctx.params.f("tiles").round().clamp(1.0, 500.0);
        let shape = ctx.params.e("shape");
        let reverse = ctx.params.b("reverse");
        let (_, _, w, _) = layer_rect(ctx, b);
        let cs = (w / tiles).max(1.0);
        let (s, co) = rot.sin_cos();
        let metric = |dx: f64, dy: f64| match shape {
            0 => dx.abs(),
            2 => dx.abs().max(dy.abs()),
            _ => dx.hypot(dy),
        };
        let maxd = max_corner_dist(b, c, |dx, dy| {
            let (qx, qy) = (dx * co + dy * s, -dx * s + dy * co);
            metric(qx, qy)
        });
        GridWipe { c, s, co, cs, maxd, shape, reverse, done, border }
    }

    /// Alpha multiplier at a buffer pixel centre.
    pub fn coverage(&self, x: u32, y: u32) -> f32 {
        let Self { c, s, co, cs, maxd, shape, reverse, done, border } = *self;
        let metric = |dx: f64, dy: f64| match shape {
            0 => dx.abs(),
            2 => dx.abs().max(dy.abs()),
            _ => dx.hypot(dy),
        };
        let spread = 0.25;
        let (dx, dy) = (x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
        let (qx, qy) = (dx * co + dy * s, -dx * s + dy * co);
        let (cx, cy) = (((qx / cs).floor() + 0.5) * cs, ((qy / cs).floor() + 0.5) * cs);
        let mut o = (metric(cx, cy) / maxd).clamp(0.0, 1.0);
        if reverse {
            o = 1.0 - o;
        }
        let p = ((done * (1.0 + spread) - o) / spread).clamp(0.0, 1.0);
        if p <= 0.0 {
            return 1.0;
        }
        let half = cs * 0.5 * (1.0 - p);
        let m = (qx - cx).abs().max((qy - cy).abs());
        let k = if border > 0.0 {
            ((half - m) / border + 0.5).clamp(0.0, 1.0)
        } else if m < half {
            1.0
        } else {
            0.0
        };
        k as f32
    }
}

fn grid_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if completion(ctx) <= 0.0 {
        return b;
    }
    let grid = GridWipe::new(ctx, &b);
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            scale_px(px, grid.coverage(x as u32, y as u32));
        }
    });
    b
}

fn radial_scale_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = completion(ctx);
    if done <= 0.0 {
        return b;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let reverse = ctx.params.b("reverse");
    let maxd = max_corner_dist(&b, c, f64::hypot);
    let hole = done * maxd;
    let src = b.img.clone();
    b.img = remap(&src, false, |x, y| {
        let (dx, dy) = (x - c.0, y - c.1);
        let r = dx.hypot(dy);
        if reverse {
            let lim = (1.0 - done) * maxd;
            if r >= lim || lim <= 1e-9 {
                return None;
            }
            let k = maxd / lim;
            Some((c.0 + dx * k, c.1 + dy * k))
        } else {
            if r <= hole || maxd - hole <= 1e-9 {
                return None;
            }
            let rs = (r - hole) * maxd / (maxd - hole);
            Some((c.0 + dx / r * rs, c.1 + dy / r * rs))
        }
    });
    b
}

fn scale_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let stretch = ctx.params.f("stretch").max(0.0);
    if stretch <= 0.0 {
        return b;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let a = ctx.params.f("direction").to_radians();
    let dir = (a.sin(), -a.cos());
    let src = b.img.clone();
    b.img = remap(&src, true, |x, y| {
        let u = (x - c.0) * dir.0 + (y - c.1) * dir.1;
        if u <= 0.0 {
            return Some((x, y));
        }
        let back = u - u / (1.0 + stretch);
        Some((x - dir.0 * back, y - dir.1 * back))
    });
    b
}

fn light_wipe(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let done = completion(ctx);
    if done <= 0.0 {
        return b;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let intensity = ctx.params.f("intensity") as f32 / 100.0;
    let shape = ctx.params.e("shape");
    let a = ctx.params.f("direction").to_radians();
    let (s, co) = a.sin_cos();
    let from_source = ctx.params.b("colorFromSource");
    let color = ctx.params.color("color");
    let reverse = ctx.params.b("reverse");
    let metric = move |dx: f64, dy: f64| {
        let (qx, qy) = (dx * co + dy * s, -dx * s + dy * co);
        match shape {
            0 => qx.abs(),
            2 => qx.abs().max(qy.abs()),
            _ => qx.hypot(qy),
        }
    };
    let maxd = max_corner_dist(&b, c, metric);
    let edge = if reverse { (1.0 - done) * maxd } else { done * maxd };
    let band = (maxd * 0.06).max(2.0);
    let ramp = (done * 20.0).min(1.0) as f32 * ((1.0 - done) * 20.0).min(1.0) as f32;
    b.img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let m = metric(x as f64 + 0.5 - c.0, y as f64 + 0.5 - c.1);
            let visible = if reverse { m < edge } else { m > edge };
            if !visible {
                *px = [0.0; 4];
                continue;
            }
            let g = (-((m - edge) / band).powi(2)).exp() as f32 * intensity * ramp;
            if g <= 0.0 {
                continue;
            }
            let (sc, a) = unpremul(*px);
            let lc = if from_source { sc } else { [color[0], color[1], color[2]] };
            for i in 0..3 {
                px[i] += lc[i] * g * a;
            }
        }
    });
    b
}

pub fn specs() -> Vec<EffectSpec> {
    let pt = |x, y| Value::Vec2([x, y]);
    let done = || p("completion", "Transition Completion", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0));
    // The CC transitions call it plain "Completion".
    let cc_done = || p("completion", "Completion", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1));
    vec![
        spec(
            "ec.transition.blockdissolve",
            "Block Dissolve",
            vec![
                done(),
                p("blockWidth", "Block Width", num(1.0), slider(1.0, 4000.0, 1.0, 100.0, 1)),
                p("blockHeight", "Block Height", num(1.0), slider(1.0, 4000.0, 1.0, 100.0, 1)),
                p("feather", "Feather", num(0.0), slider(0.0, 400.0, 0.0, 50.0, 1)),
                p("softEdges", "Soft Edges (Best Quality)", Value::Bool(true), ParamUi::Checkbox),
            ],
            block_dissolve,
        ),
        spec(
            "ec.transition.gradientwipe",
            "Gradient Wipe",
            vec![
                done(),
                p("softness", "Transition Softness", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
                p("gradientLayer", "Gradient Layer", Value::Layer(None), ParamUi::Layer),
                p("gradientPlacement", "Gradient Placement", Value::Enum(2), popup(&["Tile Gradient", "Center Gradient", "Stretch Gradient to Fit"])),
                p("invert", "Invert Gradient", Value::Bool(false), ParamUi::Checkbox),
            ],
            gradient_wipe,
        ),
        spec(
            "ec.transition.iriswipe",
            "Iris Wipe",
            vec![
                p("center", "Iris Center", pt(0.5, 0.5), ParamUi::Point),
                p("points", "Iris Points", num(6.0), slider(6.0, 32.0, 6.0, 32.0, 0)),
                p("outerRadius", "Outer Radius", num(0.0), slider(0.0, 32000.0, 0.0, 2000.0, 1)),
                p("useInnerRadius", "Use Inner Radius", Value::Bool(false), ParamUi::Checkbox),
                p("innerRadius", "Inner Radius", num(0.0), slider(0.0, 32000.0, 0.0, 2000.0, 1)),
                p("rotation", "Rotation", num(0.0), ParamUi::Angle),
                p("feather", "Feather", num(0.0), slider(0.0, 1000.0, 0.0, 100.0, 1)),
            ],
            iris_wipe,
        ),
        spec(
            "ec.transition.cardwipe",
            "Card Wipe",
            vec![
                done(),
                p("transitionWidth", "Transition Width", num(50.0), slider(1.0, 100.0, 1.0, 100.0, 0)),
                p("backLayer", "Back Layer", Value::Layer(None), ParamUi::Layer),
                // Projects saved when Back Layer was a None / Self popup: Self.
                p("backSelf", "Back Layer Is Self", Value::Bool(false), ParamUi::Hidden),
                p("rowsAndColumns", "Rows & Columns", Value::Enum(0), popup(&["Independent", "Columns Follows Rows"])),
                p("rows", "Rows", num(8.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
                p("columns", "Columns", num(8.0), slider(1.0, 1000.0, 1.0, 100.0, 0)),
                p("cardScale", "Card Scale", num(1.0), slider(0.01, 10.0, 0.01, 2.0, 2)),
                p("flipAxis", "Flip Axis", Value::Enum(0), popup(&["X", "Y", "Random"])),
                p("flipDirection", "Flip Direction", Value::Enum(0), popup(&["Positive", "Negative", "Random"])),
                p(
                    "flipOrder",
                    "Flip Order",
                    Value::Enum(0),
                    popup(&[
                        "Left to Right",
                        "Right to Left",
                        "Top to Bottom",
                        "Bottom to Top",
                        "Top Left to Bottom Right",
                        "Top Right to Bottom Left",
                        "Bottom Left to Top Right",
                        "Bottom Right to Top Left",
                        "Gradient",
                    ]),
                ),
                p("gradientLayer", "Gradient Layer", Value::Layer(None), ParamUi::Layer),
                p("timingRandomness", "Timing Randomness", num(0.0), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("randomSeed", "Random Seed", num(1.0), slider(0.0, 10000.0, 0.0, 100.0, 0)),
            ]
            .into_iter()
            .chain(crate::card3d::camera_params(2.0))
            .chain(crate::card3d::lighting_params(1.0, 0.25))
            .chain(crate::card3d::material_params(0.75))
            .chain([
                p("positionJitter/xJitterAmount", "X Jitter Amount", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
                p("positionJitter/jitterSpeed", "Jitter Speed", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("positionJitter/yJitterAmount", "Y Jitter Amount", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
                p("positionJitter/zJitterAmount", "Z Jitter Amount", num(0.0), slider(0.0, 100.0, 0.0, 5.0, 2)),
                p("rotationJitter/xRotJitterAmount", "X Rot Jitter Amount", num(0.0), slider(0.0, 360.0, 0.0, 90.0, 1)),
                p("rotationJitter/rotJitterSpeed", "Rot Jitter Speed", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 2)),
                p("rotationJitter/yRotJitterAmount", "Y Rot Jitter Amount", num(0.0), slider(0.0, 360.0, 0.0, 90.0, 1)),
                p("rotationJitter/zRotJitterAmount", "Z Rot Jitter Amount", num(0.0), slider(0.0, 360.0, 0.0, 90.0, 1)),
            ])
            .collect(),
            card_wipe,
        ),
        spec(
            "ec.transition.ccgridwipe",
            "CC Grid Wipe",
            vec![
                cc_done(),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("rotation", "Rotation", num(0.0), ParamUi::Angle),
                p("border", "Border", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
                p("tiles", "Tiles", num(10.0), slider(1.0, 500.0, 1.0, 50.0, 1)),
                p("shape", "Shape", Value::Enum(1), popup(&["Doors", "Radial", "Rectangular"])),
                p("reverse", "Reverse Transition", Value::Bool(false), ParamUi::Checkbox),
            ],
            grid_wipe,
        ),
        spec(
            "ec.transition.ccradialscalewipe",
            "CC Radial ScaleWipe",
            vec![cc_done(), p("center", "Center", pt(0.5, 0.5), ParamUi::Point), p("reverse", "Reverse Transition", Value::Bool(false), ParamUi::Checkbox)],
            radial_scale_wipe,
        ),
        spec(
            "ec.transition.ccscalewipe",
            "CC Scale Wipe",
            vec![
                p("stretch", "Stretch", num(0.0), slider(0.0, 1000.0, 0.0, 50.0, 2)),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("direction", "Direction", num(90.0), ParamUi::Angle),
            ],
            scale_wipe,
        ),
        spec(
            "ec.transition.cclightwipe",
            "CC Light Wipe",
            vec![
                cc_done(),
                p("center", "Center", pt(0.5, 0.5), ParamUi::Point),
                p("intensity", "Intensity", num(100.0), slider(0.0, 1000.0, 0.0, 200.0, 1)),
                p("shape", "Shape", Value::Enum(1), popup(&["Doors", "Round", "Square"])),
                p("direction", "Direction", num(0.0), ParamUi::Angle),
                p("colorFromSource", "Color from Source", Value::Bool(false), ParamUi::Checkbox),
                p("color", "Color", col(1.0, 1.0, 1.0), ParamUi::Color),
                p("reverse", "Reverse Transition", Value::Bool(false), ParamUi::Checkbox),
            ],
            light_wipe,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Params, find};
    use effectcraft_raster::Image;

    fn run(id: &str, img: &Image, set: &[(&str, Value)]) -> Image {
        let s = find(id).unwrap();
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        for ps in &s.params {
            if let (ParamUi::Point, Value::Vec2(f)) = (&ps.ui, &ps.default) {
                params.values.insert(ps.id.to_string(), Value::Vec2([f[0] * img.width as f64, f[1] * img.height as f64]));
            }
        }
        for (k, v) in set {
            params.values.insert(k.to_string(), v.clone());
        }
        let ctx =
            EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 7, adjustment: false, env: Default::default() };
        (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
    }

    fn ramp(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (x as f32 + 0.5) / w as f32;
                img.set(x, y, [v, v, v, 1.0]);
            }
        }
        img
    }

    fn alpha_sum(img: &Image) -> f32 {
        img.data.iter().map(|p| p[3]).sum()
    }

    #[test]
    fn transitions_are_identity_at_zero_and_clear_at_full() {
        let img = ramp(40, 30);
        let full = alpha_sum(&img);
        for id in [
            "ec.transition.blockdissolve",
            "ec.transition.gradientwipe",
            "ec.transition.cardwipe",
            "ec.transition.ccgridwipe",
            "ec.transition.ccradialscalewipe",
            "ec.transition.cclightwipe",
        ] {
            assert_eq!(run(id, &img, &[]), img, "{id} at 0%");
            let out = run(id, &img, &[("completion", num(100.0))]);
            assert!(alpha_sum(&out) < full * 0.02, "{id} at 100%: {}", alpha_sum(&out));
            let half = alpha_sum(&run(id, &img, &[("completion", num(50.0))]));
            assert!(half > 0.0 && half < full, "{id} at 50%: {half}");
        }
    }

    #[test]
    fn gradient_wipe_removes_dark_pixels_first() {
        let img = ramp(40, 4);
        let out = run("ec.transition.gradientwipe", &img, &[("completion", num(50.0))]);
        assert_eq!(out.get(5, 1)[3], 0.0);
        assert_eq!(out.get(35, 1)[3], 1.0);
        let inv = run("ec.transition.gradientwipe", &img, &[("completion", num(50.0)), ("invert", Value::Bool(true))]);
        assert_eq!(inv.get(5, 1)[3], 1.0);
        assert_eq!(inv.get(35, 1)[3], 0.0);
    }

    #[test]
    fn iris_wipe_opens_a_polygon_hole() {
        let img = Image::filled(60, 60, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(run("ec.transition.iriswipe", &img, &[]), img);
        let out = run("ec.transition.iriswipe", &img, &[("outerRadius", num(20.0))]);
        assert_eq!(out.get(30, 30)[3], 0.0);
        assert_eq!(out.get(30, 12)[3], 0.0, "vertex points up");
        assert_eq!(out.get(2, 2)[3], 1.0);
        // A hexagon's flat side is closer than its vertex: cos(30°)·20 ≈ 17.3 px.
        assert_eq!(out.get(30 + 18, 30)[3], 1.0);
        // Star with a small inner radius leaves the space between points opaque.
        let star = run("ec.transition.iriswipe", &img, &[("outerRadius", num(20.0)), ("useInnerRadius", Value::Bool(true)), ("innerRadius", num(5.0))]);
        assert_eq!(star.get(30, 15)[3], 0.0);
        assert_eq!(star.get(30 + 12, 30)[3], 1.0);
    }

    #[test]
    fn block_dissolve_is_binary_per_block() {
        let img = Image::filled(40, 40, [1.0, 1.0, 1.0, 1.0]);
        let out = run("ec.transition.blockdissolve", &img, &[("completion", num(50.0)), ("blockWidth", num(10.0)), ("blockHeight", num(10.0))]);
        for by in 0..4 {
            for bx in 0..4 {
                let a = out.get(bx * 10, by * 10)[3];
                for y in 0..10 {
                    for x in 0..10 {
                        assert_eq!(out.get(bx * 10 + x, by * 10 + y)[3], a);
                    }
                }
            }
        }
        let n = alpha_sum(&out) / 100.0;
        assert!(n > 2.0 && n < 14.0, "{n}");
    }

    #[test]
    fn card_wipe_flips_cards_in_order() {
        let img = Image::filled(80, 40, [1.0, 0.0, 0.0, 1.0]);
        let out = run("ec.transition.cardwipe", &img, &[("completion", num(50.0)), ("rows", num(1.0)), ("columns", num(8.0)), ("transitionWidth", num(10.0))]);
        // Left cards are gone, right cards untouched.
        assert_eq!(out.get(2, 20)[3], 0.0);
        assert_eq!(out.get(77, 20), [1.0, 0.0, 0.0, 1.0]);
        let back = run(
            "ec.transition.cardwipe",
            &img,
            &[("completion", num(50.0)), ("rows", num(1.0)), ("columns", num(8.0)), ("transitionWidth", num(10.0)), ("backSelf", Value::Bool(true))],
        );
        assert_eq!(back.get(2, 20)[3], 1.0);
    }

    #[test]
    fn radial_scale_wipe_pushes_content_outward() {
        let img = ramp(41, 41);
        let out = run("ec.transition.ccradialscalewipe", &img, &[("completion", num(30.0))]);
        assert_eq!(out.get(20, 20)[3], 0.0);
        assert!(out.get(0, 0)[3] > 0.0);
        let rev = run("ec.transition.ccradialscalewipe", &img, &[("completion", num(30.0)), ("reverse", Value::Bool(true))]);
        assert!(rev.get(20, 20)[3] > 0.0);
        assert_eq!(rev.get(0, 0)[3], 0.0);
    }

    #[test]
    fn scale_wipe_smears_past_the_center_line() {
        let img = ramp(40, 4);
        assert_eq!(run("ec.transition.ccscalewipe", &img, &[]), img);
        let out = run("ec.transition.ccscalewipe", &img, &[("stretch", num(1000.0))]);
        // direction 90° → to the right of centre, everything samples (almost) the centre column.
        assert_eq!(out.get(10, 1), img.get(10, 1));
        let centre = img.get(20, 1)[0];
        assert!((out.get(38, 1)[0] - centre).abs() < 0.03, "{}", out.get(38, 1)[0]);
    }

    #[test]
    fn radial_wipe_both_opens_symmetrically() {
        let img = Image::filled(41, 41, [1.0, 1.0, 1.0, 1.0]);
        let both = run("ec.transition.radialwipe", &img, &[("completion", num(50.0)), ("wipe", Value::Enum(2))]);
        // Start angle 0 = up: half done in both directions clears the whole top half…
        assert_eq!(both.get(5, 5)[3], 0.0);
        assert_eq!(both.get(35, 5)[3], 0.0);
        // …and leaves the bottom half.
        assert_eq!(both.get(5, 35)[3], 1.0);
        assert_eq!(both.get(35, 35)[3], 1.0);
        let cw = run("ec.transition.radialwipe", &img, &[("completion", num(50.0))]);
        assert_eq!(cw.get(35, 35)[3], 0.0, "clockwise covers the right half");
        assert_eq!(cw.get(5, 5)[3], 1.0);
    }

    #[test]
    fn card_wipe_columns_follow_rows_scale_and_gradient_order() {
        let img = Image::filled(80, 40, [1.0, 0.0, 0.0, 1.0]);
        // Card Scale 0.5 leaves gaps between untouched cards.
        let out = run("ec.transition.cardwipe", &img, &[("completion", num(1.0)), ("rows", num(1.0)), ("columns", num(4.0)), ("cardScale", num(0.5))]);
        assert_eq!(out.get(1, 20)[3], 0.0);
        assert_eq!(out.get(70, 20)[3], 1.0);
        // Columns Follows Rows: 2 rows → 2 columns, so at 50 % with a narrow transition the left
        // half (first column) has flipped away while the right half remains.
        let out = run(
            "ec.transition.cardwipe",
            &img,
            &[("completion", num(50.0)), ("rows", num(2.0)), ("columns", num(40.0)), ("rowsAndColumns", Value::Enum(1)), ("transitionWidth", num(10.0))],
        );
        assert_eq!(out.get(30, 10)[3], 0.0);
        assert_eq!(out.get(50, 10)[3], 1.0);
        // Gradient order (self as gradient): dark cards flip first.
        let ramp = ramp(80, 4);
        let out = run(
            "ec.transition.cardwipe",
            &ramp,
            &[("completion", num(50.0)), ("rows", num(1.0)), ("columns", num(8.0)), ("flipOrder", Value::Enum(8)), ("transitionWidth", num(10.0))],
        );
        assert_eq!(out.get(2, 2)[3], 0.0);
        assert_eq!(out.get(77, 2)[3], 1.0);
    }

    #[test]
    fn gradient_wipe_uses_the_gradient_layer_with_placement() {
        struct Host(Image);
        impl crate::EffectHost for Host {
            fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
                Some(crate::LayerPixels { buf: Buf { img: self.0.clone(), offset: [0.0; 2], scale: 1.0 }, size: [self.0.width as f64, self.0.height as f64] })
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
        }
        // A flat layer wiped by a 10-px left-to-right ramp layer.
        let img = Image::filled(40, 4, [1.0, 1.0, 1.0, 1.0]);
        let host = Host(ramp(10, 4));
        let s = find("ec.transition.gradientwipe").unwrap();
        let wipe = |placement: u32| {
            let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
            params.values.insert("completion".into(), num(50.0));
            params.values.insert("gradientLayer".into(), Value::Layer(Some(1)));
            params.values.insert("gradientPlacement".into(), Value::Enum(placement));
            let env = crate::EffectEnv { host: Some(&host), ..Default::default() };
            let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [40.0, 4.0], seed: 7, adjustment: false, env };
            (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
        };
        // Stretched: one ramp across the layer.
        let st = wipe(2);
        assert_eq!(st.get(5, 1)[3], 0.0);
        assert_eq!(st.get(35, 1)[3], 1.0);
        // Tiled: the ramp repeats every 10 px.
        let tl = wipe(0);
        assert_eq!(tl.get(11, 1)[3], 0.0);
        assert_eq!(tl.get(18, 1)[3], 1.0);
        assert_eq!(tl.get(31, 1)[3], 0.0);
    }

    #[test]
    fn grid_wipe_shrinks_tiles_from_center() {
        let img = Image::filled(100, 100, [1.0, 1.0, 1.0, 1.0]);
        let out = run("ec.transition.ccgridwipe", &img, &[("completion", num(30.0))]);
        assert_eq!(out.get(50, 50)[3], 0.0, "centre tiles are gone first");
        assert_eq!(out.get(1, 1)[3], 1.0, "corners last");
    }

    fn run_t(img: &Image, set: &[(&str, Value)], time: f64, host: Option<&dyn crate::EffectHost>) -> Image {
        let s = find("ec.transition.cardwipe").unwrap();
        let size = [img.width as f64, img.height as f64];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, size))).collect() };
        for (k, v) in set {
            params.values.insert(k.to_string(), v.clone());
        }
        let env = crate::EffectEnv { host, ..Default::default() };
        let ctx = EffectCtx { params: &params, time, layer_size: size, seed: 7, adjustment: false, env };
        (s.render)(&ctx, Buf { img: img.clone(), offset: [0.0, 0.0], scale: 1.0 }).img
    }

    #[test]
    fn card_wipe_back_layer_direction_jitter_and_camera() {
        struct Green;
        impl crate::EffectHost for Green {
            fn layer(&self, _: u64, _: bool) -> Option<crate::LayerPixels> {
                Some(crate::LayerPixels { buf: Buf { img: Image::filled(80, 40, [0.0, 1.0, 0.0, 1.0]), offset: [0.0; 2], scale: 1.0 }, size: [80.0, 40.0] })
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
        }
        let img = Image::filled(80, 40, [1.0, 0.0, 0.0, 1.0]);
        let v = [("completion", num(100.0)), ("rows", num(1.0)), ("columns", num(8.0))];
        // Fully flipped cards show the Back Layer.
        let mut b: Vec<(&str, Value)> = v.to_vec();
        b.push(("backLayer", Value::Layer(Some(3))));
        let out = run_t(&img, &b, 0.0, Some(&Green));
        let p = out.get(45, 20);
        assert!(p[1] > 0.95 && p[0] < 0.05, "{p:?}");
        // Half way, Flip Direction decides which way the cards turn (the views differ).
        let half = [("completion", num(30.0)), ("rows", num(1.0)), ("columns", num(1.0)), ("transitionWidth", num(100.0)), ("flipAxis", Value::Enum(1))];
        let pos = run_t(&img, &[half.as_slice(), &[("cameraPosition/xRotation", num(20.0))]].concat(), 0.0, None);
        let neg = run_t(&img, &[half.as_slice(), &[("cameraPosition/xRotation", num(20.0)), ("flipDirection", Value::Enum(1))]].concat(), 0.0, None);
        assert_ne!(pos, neg);
        // Position Jitter moves the cards over time (and only then).
        let j = [("completion", num(0.0)), ("positionJitter/xJitterAmount", num(0.3))];
        let a = run_t(&img, &j, 0.0, None);
        let c = run_t(&img, &j, 0.37, None);
        assert_ne!(a, c);
        assert_eq!(c, run_t(&img, &j, 0.37, None), "deterministic");
        assert_eq!(run_t(&img, &[("completion", num(0.0))], 0.37, None), img, "no jitter, no change");
        // Camera Position rotation tilts the whole wipe in perspective.
        let tilt = run_t(&img, &[("completion", num(0.0)), ("cameraPosition/yRotation", num(40.0))], 0.0, None);
        assert_eq!(tilt.get(0, 0)[3], 0.0);
    }

    #[test]
    fn block_dissolve_soft_edges_antialias_blocks() {
        let img = Image::filled(40, 4, [1.0, 1.0, 1.0, 1.0]);
        let v = [("completion", num(50.0)), ("blockWidth", num(2.5)), ("blockHeight", num(4.0))];
        let frac = |o: &Image| o.data.iter().filter(|p| p[3] > 0.01 && p[3] < 0.99).count();
        let hard = run("ec.transition.blockdissolve", &img, &[v.as_slice(), &[("softEdges", Value::Bool(false))]].concat());
        let soft = run("ec.transition.blockdissolve", &img, &v);
        assert_eq!(frac(&hard), 0);
        assert!(frac(&soft) > 0);
    }
}
