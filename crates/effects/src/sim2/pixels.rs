use super::pieces::{Cam, Piece, PiecePlan, PieceTex, rot_axis, sort_far_first};
use super::shatter::{ShatterPlan, shatter_impl};
use super::{Acc, Buf, EffectCtx, IntoParallelIterator, ParallelIterator, Post, Shape, Sprite, SpritePlan, h, hs, unpremul};

// ------------------------------------------------------------------ CC Ball Action

/// CC Ball Action's Coloring: how a ball takes its colour from the layer under its grid cell.
/// An approximation: After Effects' own meaning is not published beyond "how smooth the
/// balls look". Crisp takes the nearest pixel, Medium keeps EffectCraft's earlier smooth
/// sample at the cell centre (for compatibility), Smooth averages 16 samples over the cell.
pub(super) const BALL_COLORING: &[&str] = &["Crisp", "Medium", "Smooth"];
pub(super) const BALL_COLORING_MEDIUM: u32 = 1;
/// CC Ball Action's default Shading: the lit sphere (`Shape::Sphere`); 0 draws flat discs.
pub(super) const BALL_SHADING_DEFAULT: f64 = -75.0;
/// CC Ball Action's Displace Property: what pushes a ball towards (high values) or away from
/// (low values) the camera. The option names come from public descriptions; what Radius,
/// Diamond and Spherize compute here is an assumption pending a comparison with After Effects.
pub(super) const BALL_DISPLACE_PROPERTIES: &[&str] = &["Red", "Green", "Blue", "Brightness", "Saturation", "RGB", "Radius", "Diamond", "Spherize"];
pub(super) const BALL_DISPLACE_BRIGHTNESS: u32 = 3;

pub(super) fn ball_action(ctx: &EffectCtx, b: Buf) -> Buf {
    ball_action_plan(ctx, &b).finish(b)
}

/// A parameter's number, or `default` when the instance does not carry it.
fn f_or(ctx: &EffectCtx, id: &str, default: f64) -> f64 {
    ctx.params.values.get(id).map(|v| v.as_f64()).unwrap_or(default)
}

/// The layer's straight colour and alpha for the ball of the grid cell centred on (`sx`, `sy`)
/// (buffer pixels, cell `cell` buffer pixels wide), by Coloring `coloring`.
fn ball_color(src: &effectcraft_raster::Image, sx: f64, sy: f64, cell: f64, coloring: u32) -> ([f32; 3], f32) {
    match coloring {
        // Crisp: the pixel under the cell centre.
        0 => unpremul(src.get(sx.floor() as i64, sy.floor() as i64)),
        // Smooth: the cell's average (4 × 4 samples, premultiplied).
        2 => {
            const N: u32 = 4;
            let mut acc = [0.0f32; 4];
            for j in 0..N {
                for i in 0..N {
                    let ox = (f64::from(i) + 0.5) / f64::from(N) - 0.5;
                    let oy = (f64::from(j) + 0.5) / f64::from(N) - 0.5;
                    let p = src.sample_bilinear(sx + ox * cell, sy + oy * cell);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += v;
                    }
                }
            }
            let n = (N * N) as f32;
            unpremul([acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n])
        }
        // Medium: a smooth sample at the cell centre.
        _ => unpremul(src.sample_bilinear(sx, sy)),
    }
}

/// CC Ball Action's Displace Property `prop` (0..1) for a ball of straight colour `c` at
/// (`gx`, `gy`) on a `lw` × `lh` layer (`maxr`: half its diagonal). Assumed meanings (not
/// compared with After Effects yet): Radius, Diamond and Spherize are shapes that peak at the
/// layer centre.
fn ball_displace_value(prop: u32, c: [f32; 3], gx: f32, gy: f32, lw: f32, lh: f32, maxr: f32) -> f32 {
    let (cx, cy) = (lw * 0.5, lh * 0.5);
    let v = match prop {
        0 => c[0],
        1 => c[1],
        2 => c[2],
        // Brightness: the brightest channel (HSB brightness).
        3 => c[0].max(c[1]).max(c[2]),
        // Saturation: HSB saturation.
        4 => {
            let mx = c[0].max(c[1]).max(c[2]);
            let mn = c[0].min(c[1]).min(c[2]);
            if mx > 0.0 { (mx - mn) / mx } else { 0.0 }
        }
        // RGB: the channels' mean.
        5 => (c[0] + c[1] + c[2]) / 3.0,
        // Radius: high at the centre, low at the corners.
        6 => 1.0 - (gx - cx).hypot(gy - cy) / maxr.max(1.0),
        // Diamond: high at the centre, falling linearly to the edge midpoints' diamond.
        7 => 1.0 - ((gx - cx).abs() / cx.max(0.5) + (gy - cy).abs() / cy.max(0.5)) * 0.5,
        // Spherize: a hemisphere over the largest centred circle.
        _ => {
            let rr = cx.min(cy).max(0.5);
            let d = (gx - cx).hypot(gy - cy) / rr;
            (1.0 - d * d).max(0.0).sqrt()
        }
    };
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// CC Ball Action's balls (far to near), coloured by the layer's pixels.
fn ball_action_plan(ctx: &EffectCtx, b: &Buf) -> SpritePlan {
    let pr = ctx.params;
    let scatter = pr.f("scatter") as f32;
    let axis = pr.e("rotationAxis");
    let rot = pr.f("rotation").to_radians();
    let twist_prop = pr.e("twistProperty");
    let twist = pr.f("twistAngle").to_radians();
    let spacing = pr.f("gridSpacing").max(1.0) as f32;
    let size = pr.f("ballSize") as f32 / 100.0;
    // Scatter Motion, treated as the renamed Instability State (an assumption: After Effects
    // 24.2 shows Scatter Motion and no Instability State): turns each ball's scatter direction.
    let instab = pr.f("scatterMotion").to_radians() as f32;
    let coloring = pr.values.get("coloring").map(|v| v.as_enum()).unwrap_or(BALL_COLORING_MEDIUM);
    let shading = f_or(ctx, "shading", BALL_SHADING_DEFAULT);
    // Shading: -75 is the lit sphere as it always was, 0 flat discs; the magnitude scales the
    // light and shine (strength 1 at 75) and the sign mirrors the light's direction (positive
    // lights the balls from the lower right). How this compares with After Effects' sign
    // (described as concave versus convex) is unverified, as is the -100..100 range.
    let shape = if shading == BALL_SHADING_DEFAULT || !shading.is_finite() {
        Shape::Sphere
    } else if shading == 0.0 {
        Shape::Disc
    } else {
        // Clamped: a huge value would reach the GPU as an infinite strength (and NaN colours).
        Shape::Ball { shade: (-shading.clamp(-100.0, 100.0) / 75.0) as f32 }
    };
    // Displace: pushes each ball along z by its Displace Property (high values towards the
    // camera, pivoting at the property's middle value); 100 moves the extremes by half the
    // layer's diagonal. Scale, direction, pivot and range are EffectCraft's own choice,
    // pending a comparison with After Effects. Clamped so huge values stay finite.
    let displace = pr.f("displace").clamp(-1000.0, 1000.0) as f32;
    let displace_prop = pr.values.get("displaceProperty").map(|v| v.as_enum()).unwrap_or(BALL_DISPLACE_BRIGHTNESS);
    let seed = ctx.seed.wrapping_mul(0x2545f491);
    let (lw, lh) = (ctx.layer_size[0] as f32, ctx.layer_size[1] as f32);
    let s = b.scale as f32;
    let nx = (lw / spacing).ceil().max(1.0) as u32;
    let ny = (lh / spacing).ceil().max(1.0) as u32;
    let d = 2.0 * lw.max(lh) as f64;
    let (cx, cy) = (lw as f64 * 0.5, lh as f64 * 0.5);
    let src = &b.img;
    let ax: [f64; 3] = match axis {
        0 => [1.0, 0.0, 0.0],
        1 => [0.0, 1.0, 0.0],
        2 => [0.0, 0.0, 1.0],
        3 => [1.0, 1.0, 0.0],
        4 => [1.0, 0.0, 1.0],
        5 => [0.0, 1.0, 1.0],
        _ => [1.0, 1.0, 1.0],
    };
    let maxr = ((lw * lw + lh * lh).sqrt() * 0.5).max(1.0);
    let mut balls: Vec<(f64, Sprite)> = (0..nx * ny)
        .into_par_iter()
        .filter_map(|i| {
            let gx = (i % nx) as f32 * spacing + spacing * 0.5;
            let gy = (i / nx) as f32 * spacing + spacing * 0.5;
            let (sx, sy) = b.to_px([gx as f64, gy as f64]);
            let (c, a) = ball_color(src, sx, sy, f64::from(spacing * s), coloring);
            if a <= 0.0 || size <= 0.0 {
                return None;
            }
            let tp = match twist_prop {
                0 => gx / lw.max(1.0),
                1 => gy / lh.max(1.0),
                2 => ((gx - cx as f32).hypot(gy - cy as f32)) / maxr,
                _ => crate::util::rgb3([c[0], c[1], c[2], 0.0]).iter().sum::<f32>() / 3.0,
            } as f64;
            let ang = rot + twist * tp;
            // Scatter: random 3D offsets whose direction drifts with Scatter Motion.
            let ph = h(i, 1, seed) * std::f32::consts::TAU + instab;
            let sc = scatter * h(i, 2, seed);
            let off = [(sc * ph.cos()) as f64, (sc * ph.sin()) as f64, (scatter * hs(i, 3, seed)) as f64];
            let mut p0 = [gx as f64 - cx + off[0], gy as f64 - cy + off[1], off[2]];
            // Only when set, so Displace 0 leaves every depth (and the far-to-near order) as is.
            if displace != 0.0 && displace.is_finite() {
                let v = ball_displace_value(displace_prop, c, gx, gy, lw, lh, maxr);
                p0[2] -= f64::from(displace / 100.0 * (v - 0.5) * 2.0 * maxr);
            }
            let m = rot_axis(ax, ang);
            let q: [f64; 3] = std::array::from_fn(|k| m[k][0] * p0[0] + m[k][1] * p0[1] + m[k][2] * p0[2]);
            if d + q[2] <= d * 0.05 {
                return None;
            }
            let persp = d / (d + q[2]);
            let (bx, by) = b.to_px([cx + q[0] * persp, cy + q[1] * persp]);
            let r = spacing * 0.5 * size * persp as f32 * s;
            Some((q[2], Sprite::new(bx as f32, by as f32, r, [c[0], c[1], c[2], a], shape)))
        })
        .collect();
    balls.sort_by(|a, b| b.0.total_cmp(&a.0));
    let sprites: Vec<Sprite> = balls.into_iter().map(|(_, s)| s).collect();
    SpritePlan { sprites, tints: vec![], acc: Acc::Over, post: Post::Replace }
}

// ------------------------------------------------------------------ CC Pixel Polly

pub(super) fn pixel_polly(ctx: &EffectCtx, b: Buf) -> Buf {
    match pixel_polly_plan(ctx, &b) {
        Some(plan) => plan.finish(b),
        None => b,
    }
}

/// CC Pixel Polly's pieces, `None` before Start Time (the layer as is).
fn pixel_polly_plan(ctx: &EffectCtx, b: &Buf) -> Option<PiecePlan> {
    let pr = ctx.params;
    let t = ctx.time - pr.f("startTime");
    if t <= 0.0 {
        return None;
    }
    let force = pr.f("force");
    let gravity = pr.f("gravity");
    let spin = pr.f("spinning").to_radians();
    let fc = pr.v2("forceCenter");
    let dir_rand = pr.f("directionRandomness") / 100.0;
    let spd_rand = pr.f("speedRandomness") / 100.0;
    let g = pr.f("gridSpacing").max(1.0);
    let object = pr.e("object");
    let depth_sort = pr.b("enableDepthSort");
    let seed = ctx.seed.wrapping_mul(0x632be5ab);
    let [lw, lh] = ctx.layer_size;
    let s = b.scale;
    let cam = Cam { c: [b.offset[0] + lw * 0.5 * s, b.offset[1] + lh * 0.5 * s], d: 2.0 * lw.max(lh) * s };
    let nx = (lw / g).ceil().max(1.0) as u32;
    let ny = (lh / g).ceil().max(1.0) as u32;
    let tri = object <= 1;
    let textured = object == 1 || object == 3;
    let maxd = lw.max(lh);
    let src = &b.img;
    let per = if tri { 2 } else { 1 };
    let mut pieces: Vec<Piece> = (0..nx * ny * per)
        .into_par_iter()
        .filter_map(|i| {
            let cell = i / per;
            let (x0, y0) = ((cell % nx) as f64 * g, (cell / nx) as f64 * g);
            let poly: Vec<[f64; 2]> = if !tri {
                vec![[x0, y0], [x0 + g, y0], [x0 + g, y0 + g], [x0, y0 + g]]
            } else if i % 2 == 0 {
                vec![[x0, y0], [x0 + g, y0], [x0, y0 + g]]
            } else {
                vec![[x0 + g, y0], [x0 + g, y0 + g], [x0, y0 + g]]
            };
            let c = [poly.iter().map(|v| v[0]).sum::<f64>() / poly.len() as f64, poly.iter().map(|v| v[1]).sum::<f64>() / poly.len() as f64];
            let (dx, dy) = (c[0] - fc[0], c[1] - fc[1]);
            let dist = (dx * dx + dy * dy).sqrt().max(1e-6);
            let ang = dy.atan2(dx) + dir_rand * std::f64::consts::PI * hs(i, 1, seed) as f64;
            let speed = force / 100.0 * 0.6 * maxd / (1.0 + dist / (0.5 * maxd)) * (1.0 + spd_rand * hs(i, 2, seed) as f64);
            let vz = -speed * 0.5 * h(i, 3, seed) as f64;
            let pos = [c[0] + ang.cos() * speed * t, c[1] + ang.sin() * speed * t + 0.5 * gravity * 0.5 * lh * t * t, vz * t];
            let axis = [hs(i, 4, seed) as f64, hs(i, 5, seed) as f64, hs(i, 6, seed) as f64];
            let r = rot_axis(axis, t * (std::f64::consts::PI * h(i, 7, seed) as f64 + spin));
            let tex: Vec<[f32; 2]> = poly.iter().map(|v| [(v[0] * s + b.offset[0]) as f32, (v[1] * s + b.offset[1]) as f32]).collect();
            let c0 = [(c[0] * s + b.offset[0]) as f32, (c[1] * s + b.offset[1]) as f32];
            let posb = [pos[0] * s + b.offset[0], pos[1] * s + b.offset[1], pos[2] * s];
            let mut pc = Piece::new(&tex, c0, &r, posb, cam)?;
            pc.shade = 0.35 + 0.65 * Piece::facing(&r);
            if !textured {
                let (cc, a) = unpremul(src.sample_bilinear(c0[0] as f64, c0[1] as f64));
                if a <= 0.0 {
                    return None;
                }
                pc.flat = Some(cc);
                pc.alpha = a;
            }
            Some(pc)
        })
        .collect();
    if depth_sort {
        sort_far_first(&mut pieces);
    }
    Some(PiecePlan { pieces, front: PieceTex::Layer, back: None })
}

// ------------------------------------------------------------------ CC Scatterize

pub(super) fn scatterize(ctx: &EffectCtx, b: Buf) -> Buf {
    match scatterize_plan(ctx, &b) {
        Some(plan) => plan.finish(b),
        None => b,
    }
}

/// CC Scatterize's pixel dots, `None` when nothing moves (the layer as is).
fn scatterize_plan(ctx: &EffectCtx, b: &Buf) -> Option<SpritePlan> {
    let pr = ctx.params;
    let scatter = pr.f("scatter") as f32;
    let right = pr.f("rightTwist").to_radians();
    let left = pr.f("leftTwist").to_radians();
    if scatter == 0.0 && right == 0.0 && left == 0.0 {
        return None;
    }
    let add = pr.e("transferMode") == 1;
    let seed = ctx.seed.wrapping_mul(0x7feb352d);
    let s = b.scale;
    let (w, hh) = (b.img.width, b.img.height);
    let (cx, cy) = (b.offset[0] + ctx.layer_size[0] * 0.5 * s, b.offset[1] + ctx.layer_size[1] * 0.5 * s);
    let half = (ctx.layer_size[0] * 0.5 * s).max(1.0);
    let d = 2.0 * ctx.layer_size[0].max(ctx.layer_size[1]) * s;
    let src = &b.img;
    let sprites: Vec<Sprite> = (0..w * hh)
        .into_par_iter()
        .filter_map(|i| {
            let px = src.data[i as usize];
            if px[3] <= 0.0 {
                return None;
            }
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            // Twist about the horizontal axis through the centre, growing towards each side.
            let k = ((x - cx) / half).abs().min(1.0);
            let ang = if x < cx { left } else { right } * k;
            let (sa, ca) = ang.sin_cos();
            let (yy, zz) = ((y - cy) * ca, (y - cy) * sa);
            let persp = d / (d + zz).max(d * 0.05);
            let jx = hs(i, 1, seed) * scatter * s as f32;
            let jy = hs(i, 2, seed) * scatter * s as f32;
            let sx = cx + (x - cx) * persp + jx as f64;
            let sy = cy + yy * persp + jy as f64;
            let (c, a) = unpremul(px);
            Some(Sprite::new(sx as f32, sy as f32, 0.6 * persp as f32, [c[0], c[1], c[2], a], Shape::Disc))
        })
        .collect();
    Some(SpritePlan { sprites, tints: vec![], acc: if add { Acc::Add } else { Acc::Over }, post: Post::Replace })
}

/// What a particle effect that reads the layer's pixels draws this frame (CC Ball Action, CC
/// Pixel Polly, CC Scatterize, Shatter's wireframe views), for layer `b`; `None` for other
/// effects or a frame showing the layer as is.
pub enum PixelPlan {
    Sprites(SpritePlan),
    Pieces(PiecePlan),
}

/// The [`PixelPlan`] of effect `id` for layer `b` (its pixels are read).
pub fn pixel_plan(id: &str, ctx: &EffectCtx, b: &Buf) -> Option<PixelPlan> {
    match id {
        "ec.sim.ccballaction" => Some(PixelPlan::Sprites(ball_action_plan(ctx, b))),
        "ec.sim.ccpixelpolly" => pixel_polly_plan(ctx, b).map(PixelPlan::Pieces),
        "ec.sim.ccscatterize" => scatterize_plan(ctx, b).map(PixelPlan::Sprites),
        "ec.sim.shatter" => Some(match shatter_impl(ctx, b) {
            ShatterPlan::Pieces(plan) => PixelPlan::Pieces(plan),
            ShatterPlan::Wire(plan) => PixelPlan::Sprites(plan),
        }),
        _ => None,
    }
}
