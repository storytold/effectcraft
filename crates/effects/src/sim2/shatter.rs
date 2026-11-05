use super::pieces::{M3, Piece, PiecePlan, PieceTex, inv3, rot_axis, rot_xyz, sort_far_first};
use super::{
    Acc, Buf, EffectCtx, Image, IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator, Lighting, ParallelIterator, Post, Shape, Sprite,
    SpritePlan, Value, h, hs, layer_or_self, unpremul,
};

// ------------------------------------------------------------------ Card Dance

pub(super) const CD_SOURCES: &[&str] =
    &["None", "Intensity 1", "Red 1", "Green 1", "Blue 1", "Alpha 1", "Intensity 2", "Red 2", "Green 2", "Blue 2", "Alpha 2"];

/// (param id prefix, twirl-down group match id) per transformed card property.
pub(super) const CD_PROPS: [(&str, &str); 8] = [
    ("xPos", "xPosition"),
    ("yPos", "yPosition"),
    ("zPos", "zPosition"),
    ("xRot", "xRotation"),
    ("yRot", "yRotation"),
    ("zRot", "zRotation"),
    ("xScale", "xScale"),
    ("yScale", "yScale"),
];

fn card_source(src: u32, g1: [f32; 4], g2: [f32; 4]) -> f32 {
    let (c1, a1) = unpremul(g1);
    let (c2, a2) = unpremul(g2);
    let lum = |c: [f32; 3]| effectcraft_color::luminance(c[0], c[1], c[2]);
    match src {
        1 => lum(c1),
        2 => c1[0],
        3 => c1[1],
        4 => c1[2],
        5 => a1,
        6 => lum(c2),
        7 => c2[0],
        8 => c2[1],
        9 => c2[2],
        10 => a2,
        _ => 0.0,
    }
}

pub(super) fn card_dance(ctx: &EffectCtx, b: Buf) -> Buf {
    card_dance_plan(ctx, &b).finish(b)
}

pub(super) fn card_dance_plan(ctx: &EffectCtx, b: &Buf) -> PiecePlan {
    let pr = ctx.params;
    let rows = pr.f("rows").clamp(1.0, 1000.0) as u32;
    let cols = if pr.e("rowsColumns") == 1 { rows } else { pr.f("columns").clamp(1.0, 1000.0) as u32 };
    let g1 = layer_or_self(ctx, b, "gradientLayer1", true, true);
    let g2 = layer_or_self(ctx, b, "gradientLayer2", true, true);
    let back = ctx.layer_param("backLayer", true).map(|o| crate::util::fit_layer(ctx, b, &o, true));
    let [lw, lh] = ctx.layer_size;
    let s = b.scale;
    let (cw, ch) = (lw / cols as f64, lh / rows as f64);
    let cam = crate::card3d::projection(ctx, b);
    let eye = cam.eye();
    let light = Lighting::from(ctx, b);
    let props: Vec<(u32, f64, f64)> =
        CD_PROPS.iter().map(|(id, g)| (pr.e(&format!("{g}/{id}Source")), pr.f(&format!("{g}/{id}Multiplier")), pr.f(&format!("{g}/{id}Offset")))).collect();
    let mut pieces: Vec<Piece> = (0..rows * cols)
        .into_par_iter()
        .filter_map(|i| {
            let (cx0, cy0) = ((i % cols) as f64 * cw, (i / cols) as f64 * ch);
            let c = [cx0 + cw * 0.5, cy0 + ch * 0.5];
            let (bx, by) = (c[0] * s + b.offset[0], c[1] * s + b.offset[1]);
            let (gp1, gp2) = (g1.sample_bilinear(bx, by), g2.sample_bilinear(bx, by));
            let val = |k: usize| {
                let (sr, m, o) = props[k];
                o + m * card_source(sr, gp1, gp2) as f64
            };
            let (sx, sy) = (val(6), val(7));
            let r = rot_xyz(val(3).to_radians(), val(4).to_radians(), val(5).to_radians());
            let rs: M3 = std::array::from_fn(|k| [r[k][0] * sx, r[k][1] * sy, r[k][2]]);
            let pos = [bx + val(0) * cw * s, by + val(1) * ch * s, val(2) * cw.max(ch) * s];
            let (tx0, ty0) = ((cx0 * s + b.offset[0]) as f32, (cy0 * s + b.offset[1]) as f32);
            let (tw, th) = ((cw * s) as f32, (ch * s) as f32);
            let poly = [[tx0, ty0], [tx0 + tw, ty0], [tx0 + tw, ty0 + th], [tx0, ty0 + th]];
            let mut pc = Piece::new(&poly, [bx as f32, by as f32], &rs, pos, cam)?;
            pc.light(&light, &r, pos, eye);
            Some(pc)
        })
        .collect();
    sort_far_first(&mut pieces);
    PiecePlan { pieces, front: PieceTex::Layer, back: back.map(PieceTex::Image) }
}

// ------------------------------------------------------------------ Shatter

/// Shatter's Pattern options; the last, Custom, takes its pieces from Custom Shatter Map.
pub(super) const SHATTER_PATTERNS: &[&str] = &["Bricks", "Glass", "Hexagons", "Squares", "Triangles", "Custom"];

/// Pattern cells (polygons in pattern space, before rotation) covering `[x0, x1]×[y0, y1]`.
fn pattern_cells(kind: u32, cell: f64, x0: f64, y0: f64, x1: f64, y1: f64, seed: u32) -> Vec<Vec<[f64; 2]>> {
    let mut out = Vec::new();
    let i0 = (x0 / cell).floor() as i64 - 1;
    let i1 = (x1 / cell).ceil() as i64 + 1;
    let j0 = (y0 / cell).floor() as i64 - 1;
    let j1 = (y1 / cell).ceil() as i64 + 1;
    match kind {
        0 => {
            let bh = cell * 0.5;
            let r0 = (y0 / bh).floor() as i64 - 1;
            let r1 = (y1 / bh).ceil() as i64 + 1;
            for r in r0..=r1 {
                let sh = if r.rem_euclid(2) == 1 { cell * 0.5 } else { 0.0 };
                for i in i0..=i1 {
                    let (x, y) = (i as f64 * cell + sh, r as f64 * bh);
                    out.push(vec![[x, y], [x + cell, y], [x + cell, y + bh], [x, y + bh]]);
                }
            }
        }
        1 => {
            // Jittered lattice split into triangles along a random diagonal.
            let jit = |i: i64, j: i64| -> [f64; 2] {
                let (a, b) = (i as u32, j as u32);
                [i as f64 * cell + hs(a, b.wrapping_add(17), seed) as f64 * cell * 0.35, j as f64 * cell + hs(a, b.wrapping_add(91), seed) as f64 * cell * 0.35]
            };
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (p00, p10, p11, p01) = (jit(i, j), jit(i + 1, j), jit(i + 1, j + 1), jit(i, j + 1));
                    if h(i as u32, j as u32, seed ^ 0x55) < 0.5 {
                        out.push(vec![p00, p10, p11]);
                        out.push(vec![p00, p11, p01]);
                    } else {
                        out.push(vec![p00, p10, p01]);
                        out.push(vec![p10, p11, p01]);
                    }
                }
            }
        }
        2 => {
            let r = cell / 3f64.sqrt();
            let dx = 1.5 * r;
            let dy = 3f64.sqrt() * r;
            let c0 = (x0 / dx).floor() as i64 - 1;
            let c1 = (x1 / dx).ceil() as i64 + 1;
            let rr0 = (y0 / dy).floor() as i64 - 1;
            let rr1 = (y1 / dy).ceil() as i64 + 1;
            for ci in c0..=c1 {
                for rj in rr0..=rr1 {
                    let cx = ci as f64 * dx;
                    let cy = (rj as f64 + if ci.rem_euclid(2) == 1 { 0.5 } else { 0.0 }) * dy;
                    out.push(
                        (0..6)
                            .map(|k| {
                                let a = k as f64 * std::f64::consts::PI / 3.0;
                                [cx + r * a.cos(), cy + r * a.sin()]
                            })
                            .collect(),
                    );
                }
            }
        }
        3 => {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (x, y) = (i as f64 * cell, j as f64 * cell);
                    out.push(vec![[x, y], [x + cell, y], [x + cell, y + cell], [x, y + cell]]);
                }
            }
        }
        _ => {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let (x, y) = (i as f64 * cell, j as f64 * cell);
                    if (i + j).rem_euclid(2) == 0 {
                        out.push(vec![[x, y], [x + cell, y], [x, y + cell]]);
                        out.push(vec![[x + cell, y], [x + cell, y + cell], [x, y + cell]]);
                    } else {
                        out.push(vec![[x, y], [x + cell, y], [x + cell, y + cell]]);
                        out.push(vec![[x, y], [x + cell, y + cell], [x, y + cell]]);
                    }
                }
            }
        }
    }
    out
}

/// `∫0^t e^{-ks} ds` and `∫0^t ∫0^u e^{-ks} ds du` (drag-damped ballistic factors).
fn drag_factors(k: f64, t: f64) -> (f64, f64) {
    if k < 1e-6 {
        (t, 0.5 * t * t)
    } else {
        let e = (-k * t).exp();
        ((1.0 - e) / k, t / k - (1.0 - e) / (k * k))
    }
}

/// Shatter's Front / Side / Back Mode options.
pub(super) const SHATTER_TEXTURE_MODES: [&str; 3] = ["Color", "Layer", "Tinted Layer"];

/// One face's texture: a flat colour, a layer, or a layer tinted by the Textures colour.
struct Face {
    mode: u32,
    img: Option<Image>,
}

impl Face {
    fn from(ctx: &EffectCtx, b: &Buf, mode: &str, layer: &str, default_mode: u32) -> Face {
        let mode = ctx.params.get(mode).map(|v| v.as_enum()).unwrap_or(default_mode);
        // A face layer of None is the effect's own layer.
        let img = (mode != 0).then(|| layer_or_self(ctx, b, layer, true, true));
        Face { mode, img }
    }
}

/// Group key of a Custom Shatter Map colour (pieces of one colour break as one).
fn map_key(c: [f32; 4]) -> u32 {
    let (c, a) = unpremul(c);
    if a < 0.5 {
        return u32::MAX;
    }
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 15.0).round() as u32) & 15;
    q(c[0]) << 8 | q(c[1]) << 4 | q(c[2])
}

pub(super) fn shatter(ctx: &EffectCtx, b: Buf) -> Buf {
    match shatter_impl(ctx, &b) {
        ShatterPlan::Pieces(plan) => plan.finish(b),
        ShatterPlan::Wire(plan) => plan.finish(b),
    }
}

/// What Shatter draws: the pieces (Rendered view) or the wireframe lines.
pub(super) enum ShatterPlan {
    Pieces(PiecePlan),
    Wire(SpritePlan),
}

pub(super) fn shatter_impl(ctx: &EffectCtx, b: &Buf) -> ShatterPlan {
    let pr = ctx.params;
    let view = pr.e("view");
    let render = pr.e("render");
    let kind = pr.e("shape/pattern");
    let custom = kind as usize == SHATTER_PATTERNS.len() - 1;
    let reps = pr.f("shape/repetitions").clamp(1.0, 500.0);
    let dir = pr.f("shape/direction").to_radians();
    let origin = pr.v2("shape/origin");
    let forces = [
        (pr.v2("force1/force1Position"), pr.f("force1/force1Depth"), pr.f("force1/force1Radius"), pr.f("force1/force1Strength")),
        (pr.v2("force2/force2Position"), pr.f("force2/force2Depth"), pr.f("force2/force2Radius"), pr.f("force2/force2Strength")),
    ];
    let rot_speed = pr.f("physics/rotationSpeed");
    let tumble = pr.e("physics/tumbleAxis");
    let randomness = pr.f("physics/randomness");
    let k = pr.f("physics/viscosity").max(0.0) * 5.0;
    let mass_var = pr.f("physics/massVariance") / 100.0;
    let gravity = pr.f("physics/gravity");
    let gdir = pr.f("physics/gravityDirection").to_radians();
    let ginc = pr.f("physics/gravityInclination").to_radians();
    let seed = (pr.f("randomSeed") as u32).wrapping_mul(0x9e3779b1) ^ ctx.seed;
    let [lw, lh] = ctx.layer_size;
    let s = b.scale;
    let t = ctx.time.max(0.0);
    let unit = lw * 0.1;
    let cell = lw / reps;
    let extrude = pr.f("shape/extrusionDepth").max(0.0) * cell * s;
    let cam = crate::card3d::projection(ctx, b);
    let eye = cam.eye();
    let light = Lighting::from(ctx, b);
    // Gradient: with a gradient layer only pieces at least as bright as 1 − Shatter Threshold
    // (Invert Gradient: as dark) break.
    let gradient = ctx.layer_param("gradient/gradientLayer", true).map(|o| crate::util::fit_layer(ctx, b, &o, true));
    let threshold = pr.get("gradient/shatterThreshold").map(Value::as_f64).unwrap_or(0.0) / 100.0;
    let invert_gradient = pr.b("gradient/invertGradient");
    // Textures.
    let tex_color = pr.get("textures/color").map(|v| v.as_color()).unwrap_or([1.0; 4]);
    let tex_opacity = pr.get("textures/opacity").map(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0) as f32;
    let front = Face::from(ctx, b, "textures/frontMode", "textures/frontLayer", 1);
    let side = Face::from(ctx, b, "textures/sideMode", "textures/sideLayer", 0);
    let backf = Face::from(ctx, b, "textures/backMode", "textures/backLayer", 1);
    let custom_map = custom.then(|| layer_or_self(ctx, b, "shape/customShatterMap", true, true));
    let white_fixed = pr.b("shape/whiteTilesFixed");
    let (sd, cd) = dir.sin_cos();
    let to_layer = |q: [f64; 2]| [origin[0] + q[0] * cd - q[1] * sd, origin[1] + q[0] * sd + q[1] * cd];
    // Pattern-space bounds of the layer rectangle.
    let corners = [[0.0, 0.0], [lw, 0.0], [lw, lh], [0.0, lh]].map(|c: [f64; 2]| {
        let (dx, dy) = (c[0] - origin[0], c[1] - origin[1]);
        [dx * cd + dy * sd, -dx * sd + dy * cd]
    });
    let (px0, px1) = (corners.iter().map(|c| c[0]).fold(f64::INFINITY, f64::min), corners.iter().map(|c| c[0]).fold(f64::NEG_INFINITY, f64::max));
    let (py0, py1) = (corners.iter().map(|c| c[1]).fold(f64::INFINITY, f64::min), corners.iter().map(|c| c[1]).fold(f64::NEG_INFINITY, f64::max));
    // Custom Shatter Map: square cells grouped by the map's colour.
    let cells = pattern_cells(if custom { 3 } else { kind }, cell, px0, py0, px1, py1, seed);
    let polys: Vec<Vec<[f64; 2]>> = cells.iter().map(|poly| poly.iter().map(|&q| to_layer(q)).collect()).collect();
    let centre = |lp: &[[f64; 2]]| [lp.iter().map(|v| v[0]).sum::<f64>() / lp.len() as f64, lp.iter().map(|v| v[1]).sum::<f64>() / lp.len() as f64];
    let keys: Vec<u32> = match &custom_map {
        Some(m) => polys
            .iter()
            .map(|lp| {
                let c = centre(lp);
                map_key(m.sample_bilinear(c[0] * s + b.offset[0], c[1] * s + b.offset[1]))
            })
            .collect(),
        None => (0..polys.len() as u32).collect(),
    };
    // Group centres (custom map) — each group moves as one rigid piece.
    let mut groups: std::collections::HashMap<u32, ([f64; 2], f64)> = std::collections::HashMap::new();
    for (lp, &key) in polys.iter().zip(&keys) {
        let c = centre(lp);
        let e = groups.entry(key).or_insert(([0.0; 2], 0.0));
        e.0[0] += c[0];
        e.0[1] += c[1];
        e.1 += 1.0;
    }
    let g3 = [gdir.sin() * ginc.cos() * gravity * unit, -gdir.cos() * ginc.cos() * gravity * unit, ginc.sin() * gravity * unit];
    let mut pieces: Vec<Piece> = polys
        .par_iter()
        .zip(keys.par_iter())
        .flat_map_iter(|(lp, &key)| {
            let mut out: Vec<Piece> = vec![];
            let (mnx, mxx) = (lp.iter().map(|v| v[0]).fold(f64::INFINITY, f64::min), lp.iter().map(|v| v[0]).fold(f64::NEG_INFINITY, f64::max));
            let (mny, mxy) = (lp.iter().map(|v| v[1]).fold(f64::INFINITY, f64::min), lp.iter().map(|v| v[1]).fold(f64::NEG_INFINITY, f64::max));
            if mxx < 0.0 || mxy < 0.0 || mnx > lw || mny > lh || key == u32::MAX {
                return out.into_iter();
            }
            let pc_c = centre(lp);
            let (gsum, gn) = groups[&key];
            let gc = [gsum[0] / gn, gsum[1] / gn];
            let i = key;
            // Broken when inside a force sphere (centred `depth` in front of the layer).
            let mut v = [0.0f64; 3];
            let mut broken = false;
            for (fp, fd, fr, fs) in forces {
                let rad = fr * lw;
                if rad <= 0.0 {
                    continue;
                }
                let dv = [gc[0] - fp[0], gc[1] - fp[1], fd * lw];
                let dist = (dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2]).sqrt();
                if dist < rad {
                    broken = true;
                    let f = fs * (1.0 - dist / rad) * unit / dist.max(1e-6);
                    for q in 0..3 {
                        v[q] += dv[q] * f;
                    }
                }
            }
            if broken && let Some(g) = &gradient {
                let (c, _) = unpremul(g.sample_bilinear(gc[0] * s + b.offset[0], gc[1] * s + b.offset[1]));
                let mut l = effectcraft_color::luminance(c[0], c[1], c[2]).clamp(0.0, 1.0) as f64;
                if invert_gradient {
                    l = 1.0 - l;
                }
                broken = threshold > 0.0 && l >= 1.0 - threshold;
            }
            if broken && custom && white_fixed && key == 0xfff {
                broken = false;
            }
            if (broken && render == 1) || (!broken && render == 2) {
                return out.into_iter();
            }
            let mut gpos = [gc[0], gc[1], 0.0];
            let mut r = rot_axis([1.0, 0.0, 0.0], 0.0);
            let moving = broken && t > 0.0;
            if moving {
                let mass = 1.0 + mass_var * hs(i, 1, seed) as f64;
                for q in 0..3 {
                    v[q] = v[q] / mass + randomness * unit * hs(i, 2 + q as u32, seed) as f64 * 2.0;
                }
                let (f1, f2) = drag_factors(k, t);
                for q in 0..3 {
                    gpos[q] += v[q] * f1 + g3[q] * f2;
                }
                let axis = match tumble {
                    1 => [0.0, 0.0, 0.0],
                    2 => [1.0, 0.0, 0.0],
                    3 => [0.0, 1.0, 0.0],
                    4 => [0.0, 0.0, 1.0],
                    5 => [hs(i, 6, seed) as f64, hs(i, 7, seed) as f64, 0.0],
                    6 => [hs(i, 6, seed) as f64, 0.0, hs(i, 8, seed) as f64],
                    7 => [0.0, hs(i, 7, seed) as f64, hs(i, 8, seed) as f64],
                    _ => [hs(i, 6, seed) as f64, hs(i, 7, seed) as f64, hs(i, 8, seed) as f64],
                };
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() / unit;
                let omega = rot_speed * std::f64::consts::TAU * (0.5 + h(i, 9, seed) as f64) * speed.min(10.0);
                r = rot_axis(axis, omega * f1);
            }
            // The piece's centre: its offset from the group centre, rotated with the group.
            let off = [pc_c[0] - gc[0], pc_c[1] - gc[1], 0.0];
            let pos: [f64; 3] = std::array::from_fn(|q| gpos[q] + (0..3).map(|k| r[q][k] * off[k]).sum::<f64>());
            let tex: Vec<[f32; 2]> = lp.iter().map(|v| [(v[0] * s + b.offset[0]) as f32, (v[1] * s + b.offset[1]) as f32]).collect();
            let c0 = [(pc_c[0] * s + b.offset[0]) as f32, (pc_c[1] * s + b.offset[1]) as f32];
            let posb = [pos[0] * s + b.offset[0], pos[1] * s + b.offset[1], pos[2] * s];
            let Some(mut pc) = Piece::new(&tex, c0, &r, posb, cam) else { return out.into_iter() };
            pc.light(&light, &r, posb, eye);
            pc.alpha = tex_opacity;
            let face = if pc.back { &backf } else { &front };
            if face.mode == 0 {
                pc.flat = Some([tex_color[0], tex_color[1], tex_color[2]]);
            } else if face.mode == 2 {
                pc.tint = std::array::from_fn(|q| pc.tint[q] * tex_color[q]);
            }
            // Extrusion: the sides of a moving piece, from its front outline back by the depth.
            if moving && extrude > 0.0 {
                let nb = [r[0][2], r[1][2], r[2][2]];
                let n = tex.len();
                for e in 0..n {
                    let (a, bv) = (tex[e], tex[(e + 1) % n]);
                    let ed = [(bv[0] - a[0]) as f64, (bv[1] - a[1]) as f64];
                    let len = (ed[0] * ed[0] + ed[1] * ed[1]).sqrt();
                    if len < 1e-6 {
                        continue;
                    }
                    let ew: [f64; 3] = std::array::from_fn(|q| (r[q][0] * ed[0] + r[q][1] * ed[1]) / len);
                    let third = [ew[1] * nb[2] - ew[2] * nb[1], ew[2] * nb[0] - ew[0] * nb[2], ew[0] * nb[1] - ew[1] * nb[0]];
                    let rs: M3 = std::array::from_fn(|q| [ew[q], nb[q], third[q]]);
                    // Side centre in world: edge midpoint (world) plus half the depth backwards.
                    let mid = [((a[0] + bv[0]) * 0.5 - c0[0]) as f64, ((a[1] + bv[1]) * 0.5 - c0[1]) as f64];
                    let mw: [f64; 3] = std::array::from_fn(|q| posb[q] + r[q][0] * mid[0] + r[q][1] * mid[1] + nb[q] * extrude * 0.5);
                    let (hl, hd) = ((len * 0.5) as f32, (extrude * 0.5) as f32);
                    let cs = [(a[0] + bv[0]) * 0.5, (a[1] + bv[1]) * 0.5];
                    let quad = [[cs[0] - hl, cs[1] - hd], [cs[0] + hl, cs[1] - hd], [cs[0] + hl, cs[1] + hd], [cs[0] - hl, cs[1] + hd]];
                    if let Some(mut sp) = Piece::new(&quad, cs, &rs, mw, cam) {
                        sp.light(&light, &rs, mw, eye);
                        sp.alpha = tex_opacity;
                        let colour = match (&side.img, side.mode) {
                            (Some(img), m) if m != 0 => {
                                let (c, _) = unpremul(img.sample_bilinear(cs[0] as f64, cs[1] as f64));
                                if m == 2 { std::array::from_fn(|q| c[q] * tex_color[q]) } else { c }
                            }
                            _ => [tex_color[0], tex_color[1], tex_color[2]],
                        };
                        sp.flat = Some(colour);
                        out.push(sp);
                    }
                }
            }
            out.push(pc);
            out.into_iter()
        })
        .collect();
    sort_far_first(&mut pieces);
    if view == 0 {
        let tex = |i: &Option<Image>| i.clone().map(PieceTex::Image);
        let front_t = tex(&front.img).unwrap_or(PieceTex::Layer);
        let back_t = tex(&backf.img).unwrap_or_else(|| front_t.clone());
        return ShatterPlan::Pieces(PiecePlan { pieces, front: front_t, back: Some(back_t) });
    }
    {
        // Wireframes: piece outlines (front view: at rest; otherwise animated), plus forces.
        let animated = view == 2 || view == 4;
        let mut lines = Vec::new();
        for pc in &pieces {
            let n = pc.n as usize;
            let pts: Vec<[f32; 2]> = if animated {
                let fw = inv3(&pc.inv);
                match fw {
                    Some(m) => pc.poly[..n]
                        .iter()
                        .map(|v| {
                            let (a, bb) = ((v[0] - pc.c0[0]) as f64, (v[1] - pc.c0[1]) as f64);
                            let w = m[2][0] * a + m[2][1] * bb + m[2][2];
                            [((m[0][0] * a + m[0][1] * bb + m[0][2]) / w) as f32, ((m[1][0] * a + m[1][1] * bb + m[1][2]) / w) as f32]
                        })
                        .collect(),
                    None => continue,
                }
            } else {
                pc.poly[..n].to_vec()
            };
            for e in 0..n {
                let (a, c) = (pts[e], pts[(e + 1) % n]);
                lines.push(Sprite::new(a[0], a[1], 0.5, [0.75, 0.85, 1.0, 1.0], Shape::Line { dx: c[0] - a[0], dy: c[1] - a[1] }));
            }
        }
        if view >= 3 {
            for (fp, _, fr, _) in forces {
                let (fx, fy) = b.to_px(fp);
                let rad = fr * lw * s;
                for k in 0..64 {
                    let (a0, a1) = (k as f64 / 64.0 * std::f64::consts::TAU, (k + 1) as f64 / 64.0 * std::f64::consts::TAU);
                    let (x0, y0) = (fx + rad * a0.cos(), fy + rad * a0.sin());
                    let (x1, y1) = (fx + rad * a1.cos(), fy + rad * a1.sin());
                    lines.push(Sprite::new(x0 as f32, y0 as f32, 0.5, [1.0, 0.3, 0.3, 1.0], Shape::Line { dx: (x1 - x0) as f32, dy: (y1 - y0) as f32 }));
                }
            }
        }
        ShatterPlan::Wire(SpritePlan { sprites: lines, tints: vec![], acc: Acc::Over, post: Post::Replace })
    }
}
