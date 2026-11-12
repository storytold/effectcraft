//! GPU effects, distortion and stylize family (kernels in `shaders/fx_distort.wgsl`): the CPU effects' exact
//! steps with the pixel loops as compute kernels.
//!
//! Parameter conversions, padding and anything with a per-row or per-column structure (map
//! coordinates, tile bounds, Mesh Warp's triangle buckets) are computed here in f64 exactly as
//! the CPU does and uploaded; the kernels do the per-pixel work.

use std::f64::consts::TAU;

use effectcraft_effects::EffectCtx;
use effectcraft_effects::util::{self, Src, src_at};

use crate::context::{Enc, Params};
use crate::effects::GBuf;

/// Compute entry points in `fx_distort.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "dst_twirl",
    "dst_bulge",
    "dst_wavewarp",
    "dst_ripple",
    "dst_cclens",
    "dst_turbulent",
    "dst_displace",
    "dst_mesh",
    "dst_mosaic_rows",
    "dst_mosaic_tiles",
    "dst_mosaic_out",
    "dst_mosaic_sharp",
    "dst_findedges",
    "dst_emboss",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.distort.turbulentdisplace",
    "ec.distort.displacementmap",
    "ec.distort.wavewarp",
    "ec.distort.ripple",
    "ec.distort.twirl",
    "ec.distort.bulge",
    "ec.distort.cclens",
    "ec.distort.meshwarp",
    "ec.stylize.mosaic",
    "ec.stylize.findedges",
    "ec.stylize.emboss",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.distort.turbulentdisplace" => turbulent(e, ctx, b),
        "ec.distort.displacementmap" => displacement_map(e, ctx, b),
        "ec.distort.wavewarp" => wave_warp(e, ctx, b),
        "ec.distort.ripple" => ripple(e, ctx, b),
        "ec.distort.twirl" => twirl(e, ctx, b),
        "ec.distort.bulge" => bulge(e, ctx, b),
        "ec.distort.cclens" => cc_lens(e, ctx, b),
        "ec.distort.meshwarp" => mesh_warp(e, ctx, b),
        "ec.stylize.mosaic" => mosaic(e, ctx, b),
        "ec.stylize.findedges" => find_edges(e, ctx, b),
        "ec.stylize.emboss" => emboss(e, ctx, b),
        _ => None,
    }
}

/// Run a same-size per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, None, &out, data);
    b.img = out;
    Some(b)
}

/// Buf::layer_w / layer_h (distort.rs).
fn layer_wh(ctx: &EffectCtx) -> (f64, f64) {
    (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0))
}

// ---------------------------------------------------------------- distort.rs

fn twirl(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("angle").to_radians();
    let (lw, lh) = layer_wh(ctx);
    let r = (ctx.params.f("radius") / 100.0 * lw.max(lh) * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    let mut p = Params::default();
    p.f[0] = [c.0 as f32, c.1 as f32, r as f32, ang as f32];
    run(e, "dst_twirl", &p, b, None)
}

fn bulge(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let rx = (ctx.params.f("hradius") * b.scale).max(1.0);
    let ry = (ctx.params.f("vradius") * b.scale).max(1.0);
    let c = b.to_px(ctx.params.v2("center"));
    let h = ctx.params.f("height");
    let taper = 1.0 + ctx.params.f("taperRadius").max(0.0) / 100.0 * 3.0;
    let pin = ctx.params.b("pinAllEdges");
    let (x0, y0, lw, lh) = util::layer_rect_at(ctx, b.offset, b.scale);
    let edge = (lw.min(lh) * 0.1).max(1.0);
    let mut p = Params::default();
    p.f[0] = [c.0 as f32, c.1 as f32, rx as f32, ry as f32];
    p.f[1] = [h as f32, taper as f32, if pin { 1.0 } else { 0.0 }, edge as f32];
    p.f[2] = [x0 as f32, y0 as f32, lw as f32, lh as f32];
    run(e, "dst_bulge", &p, b, None)
}

fn wave_warp(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let h = ctx.params.f("height") * b.scale;
    let w = (ctx.params.f("width") * b.scale).max(1.0);
    let dir = ctx.params.f("direction").to_radians();
    let speed = ctx.params.f("speed");
    let phase = ctx.params.f("phase").to_radians() + ctx.time * speed * TAU;
    // Cardinal directions must not acquire a tiny perpendicular slope: at a square-wave
    // boundary cos(90°)'s rounding residue can flip a whole column on CPU but not GPU.
    let axis = |v: f64| if v.abs() < 4.0 * f64::EPSILON { 0.0 } else { v };
    let (dx, dy) = (axis(dir.sin()), -axis(dir.cos()));
    if !ctx.adjustment {
        b.pad(e, h.abs().ceil() as u32 + 1)?;
    }
    let (x0, y0, lw, lh) = util::layer_rect_at(ctx, b.offset, b.scale);
    let rect = [x0 as f32, y0 as f32, lw.max(1.0) as f32, lh.max(1.0) as f32];
    // The wave phase in cycles, (x·dx + y·dy) / width + phase / 2π, is separable: per column
    // and per row (fraction, whole cycles) in f64, so the kernel's sum keeps the fraction exact
    // to f32 (the circle shapes have vertical tangents where a phase error is amplified).
    let (bw, bh) = (b.img.width as usize, b.img.height as usize);
    let mut data = vec![0.0f32; 2 * (bw + bh)];
    let split = |v: f64| (v - v.floor(), v.floor());
    for x in 0..bw {
        let (f, i) = split((x as f64 + 0.5) * dx / w);
        data[x] = f as f32;
        data[bw + x] = i as f32;
    }
    for y in 0..bh {
        let (f, i) = split((y as f64 + 0.5) * dy / w + phase / TAU);
        data[2 * bw + y] = f as f32;
        data[2 * bw + bh + y] = i as f32;
    }
    let mut p = Params::default();
    p.u[0] = [ctx.params.e("waveType"), ctx.params.e("pinning"), ctx.seed ^ 0x3a7e, bw as u32];
    p.f[0] = [dx as f32, dy as f32, h as f32, 0.0];
    p.f[2] = rect;
    let buf = e.data(&data);
    run(e, "dst_wavewarp", &p, b, Some(&buf))
}

fn ripple(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let (lw, lh) = layer_wh(ctx);
    let r = (ctx.params.f("radius") / 100.0 * lw.max(lh) * 0.5 * b.scale).max(1.0);
    let asymmetric = ctx.params.e("conversion") == 0;
    let c = b.to_px(ctx.params.v2("center"));
    let w = (ctx.params.f("waveWidth") * b.scale).max(1.0);
    let h = ctx.params.f("waveHeight") * b.scale;
    let phase = ctx.params.f("phase").to_radians() - ctx.time * ctx.params.f("speed") * TAU;
    let mut p = Params::default();
    p.u[0][0] = asymmetric as u32;
    p.f[0] = [c.0 as f32, c.1 as f32, r as f32, w as f32];
    p.f[1] = [h as f32, phase.rem_euclid(TAU) as f32, 0.0, 0.0];
    run(e, "dst_ripple", &p, b, None)
}

// ---------------------------------------------------------------- distort2.rs

fn cc_lens(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = b.to_px(ctx.params.v2("center"));
    let (lw, lh) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let r = (ctx.params.f("size") / 100.0 * lw.max(lh) * 0.5).max(0.5);
    let k = 2f64.powf(ctx.params.f("convergence") / 50.0);
    let mut p = Params::default();
    p.f[0] = [c.0 as f32, c.1 as f32, r as f32, k as f32];
    run(e, "dst_cclens", &p, b, None)
}

fn turbulent(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let amount = ctx.params.f("amount") * b.scale;
    if amount.abs() < 1e-9 {
        return Some(b);
    }
    let kind = ctx.params.e("displacement");
    let size = (ctx.params.f("size") * b.scale).max(1.0);
    if ctx.params.b("resizeLayer") && !ctx.adjustment {
        b.pad(e, amount.abs().ceil() as u32 + 1)?;
    } else {
        // Room for pixels displaced past the content, up to the layer's edge (#1205).
        b.pad(e, effectcraft_effects::cover_pad(ctx, [b.img.width, b.img.height], b.offset, b.scale, amount.abs().ceil() as u32 + 1))?;
    }
    let off = b.to_px(ctx.params.v2("offset"));
    let oct = ctx.params.f("complexity").clamp(1.0, 10.0);
    let evo = ctx.params.f("evolution") / 360.0;
    let cycle = ctx.params.b("evolutionOptions/cycleEvolution").then(|| ctx.params.f("evolutionOptions/cycle").round().max(1.0));
    let falloff = if (3..=5).contains(&kind) { 0.3 } else { 0.5 };
    let (x0, y0, lw, lh) = util::layer_rect_at(ctx, b.offset, b.scale);
    let edge = (lw.min(lh) * 0.1).max(1.0);
    let seed = (ctx.seed ^ 0x7d15).wrapping_add((ctx.params.f("evolutionOptions/randomSeed") as i64 as u32).wrapping_mul(0x9e37_79b9));
    let (z0, z1, t) = match cycle {
        Some(c) => {
            let z = evo.rem_euclid(c);
            (z as f32, (z - c) as f32, (z / c) as f32)
        }
        None => (evo as f32, 0.0, 0.0),
    };
    let mut p = Params::default();
    p.u[0] = [kind, ctx.params.e("pinning"), seed, oct.ceil().max(1.0) as u32];
    p.u[1][0] = cycle.is_some() as u32;
    p.f[0] = [off.0 as f32, off.1 as f32, size as f32, amount as f32];
    p.f[1] = [z0, z1, t, (oct - oct.floor()) as f32];
    p.f[2] = [x0 as f32, y0 as f32, lw as f32, lh as f32];
    p.f[3] = [edge as f32, falloff, 0.0, 0.0];
    run(e, "dst_turbulent", &p, b, None)
}

fn displacement_map(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let hs = src_at(ctx.params.e("useForHorizontal"));
    let vs = src_at(ctx.params.e("useForVertical"));
    let mh = ctx.params.f("maxHorizontal") * b.scale;
    let mv = ctx.params.f("maxVertical") * b.scale;
    let wrap = ctx.params.b("wrapPixelsAround");
    if ctx.params.b("expandOutput") && !wrap && !ctx.adjustment {
        b.pad(e, mh.abs().max(mv.abs()).ceil() as u32)?;
    }
    let layer = ctx.layer_param("displacementMapLayer", false);
    let behavior = ctx.params.e("displacementMapBehavior");
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    let inv = 1.0 / b.scale.max(1e-9);
    let ls = ctx.layer_size;
    // Per column / row: the map layer's buffer coordinate (fit_layer / Tile Map) and whether
    // the pixel lies inside the centred map (Center Map).
    let mut data = vec![0.0f32; 2 * (w + h)];
    let mut mode = 0u32;
    let mut center = false;
    let map = match &layer {
        Some(o) => {
            if o.buf.img.is_empty() {
                return None;
            }
            let os = o.size;
            let axis = |i: usize, k: usize| -> (f64, bool) {
                let q = (i as f64 + 0.5 - b.offset[k]) * inv;
                let map_q = match behavior {
                    2 => (q + (os[k] - ls[k]) * 0.5).rem_euclid(os[k].max(1.0)),
                    1 if ls[0] > 0.0 && ls[1] > 0.0 => q * (os[k] / ls[k]),
                    1 => q,
                    _ => q + (os[k] - ls[k]) * 0.5,
                };
                let c = q + (os[k] - ls[k]) * 0.5;
                (map_q * o.buf.scale + o.buf.offset[k], (0.0..os[k]).contains(&c))
            };
            for x in 0..w {
                let (m, inside) = axis(x, 0);
                data[x] = m as f32;
                data[w + h + x] = inside as u32 as f32;
            }
            for y in 0..h {
                let (m, inside) = axis(y, 1);
                data[w + y] = m as f32;
                data[2 * w + h + y] = inside as u32 as f32;
            }
            mode = if behavior == 2 { 2 } else { 1 };
            center = behavior == 0;
            Some(e.g.upload_image(&o.buf.img)?)
        }
        None => None,
    };
    let idx = |s: Src| effectcraft_effects::util::SRC_ORDER.iter().position(|x| *x == s).unwrap_or(10) as u32;
    let mut p = Params::default();
    p.u[0] = [idx(hs), idx(vs), mode, wrap as u32 | (center as u32) << 1];
    p.u[1] = [w as u32, h as u32, 0, 0];
    p.f[0] = [mh as f32, mv as f32, 0.0, 0.0];
    let buf = e.data(&data);
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("dst_displace", &p, &b.img, map.as_ref(), &out, Some(&buf));
    b.img = out;
    Some(b)
}

fn mesh_warp(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let rows = ctx.params.f("rows").round().clamp(1.0, 31.0) as usize;
    let cols = ctx.params.f("columns").round().clamp(1.0, 31.0) as usize;
    let q = ctx.params.f("quality").round().clamp(1.0, 10.0) as usize;
    let offs = effectcraft_effects::parse_mesh(ctx.params.s("mesh"));
    if offs.len() != (rows + 1) * (cols + 1) || offs.iter().all(|o| o.0 == 0.0 && o.1 == 0.0) {
        return Some(b);
    }
    let (lw, lh) = (ctx.layer_size[0], ctx.layer_size[1]);
    let vert = |i: usize, j: usize| {
        let o = offs[j * (cols + 1) + i];
        b.to_px([i as f64 / cols as f64 * lw + o.0, j as f64 / rows as f64 * lh + o.1])
    };
    let (nx, ny) = (cols * q, rows * q);
    let mut dest = Vec::with_capacity((nx + 1) * (ny + 1));
    let mut srcp = Vec::with_capacity((nx + 1) * (ny + 1));
    for gj in 0..=ny {
        for gi in 0..=nx {
            let (ci, cj) = ((gi / q).min(cols - 1), (gj / q).min(rows - 1));
            let (u, v) = ((gi - ci * q) as f64 / q as f64, (gj - cj * q) as f64 / q as f64);
            let (p00, p10, p01, p11) = (vert(ci, cj), vert(ci + 1, cj), vert(ci, cj + 1), vert(ci + 1, cj + 1));
            let x = (p00.0 * (1.0 - u) + p10.0 * u) * (1.0 - v) + (p01.0 * (1.0 - u) + p11.0 * u) * v;
            let y = (p00.1 * (1.0 - u) + p10.1 * u) * (1.0 - v) + (p01.1 * (1.0 - u) + p11.1 * u) * v;
            dest.push((x, y));
            srcp.push(b.to_px([gi as f64 / nx as f64 * lw, gj as f64 / ny as f64 * lh]));
        }
    }
    grid_warp(e, b, nx, ny, &dest, &srcp)
}

/// distort2::grid_warp: grid point `i` of the (nx+1)×(ny+1) grid at `dest[i]` shows the source at
/// `srcp[i]` (Mesh Warp, Bezier Warp). Triangles in order, bucketed per output row.
pub(crate) fn grid_warp(e: &mut Enc, b: GBuf, nx: usize, ny: usize, dest: &[(f64, f64)], srcp: &[(f64, f64)]) -> Option<GBuf> {
    let idx = |i: usize, j: usize| j * (nx + 1) + i;
    let hh = b.img.height as usize;
    let mut tri_data: Vec<f32> = Vec::with_capacity(nx * ny * 24);
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); hh];
    let mut t = 0u32;
    for j in 0..ny {
        for i in 0..nx {
            let (a, b2, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            for tri in [[a, b2, c], [a, c, d]] {
                let [ia, ib, ic] = tri;
                let (pa, pb, pc) = (dest[ia], dest[ib], dest[ic]);
                let den: f64 = (pb.1 - pc.1) * (pa.0 - pc.0) + (pc.0 - pb.0) * (pa.1 - pc.1);
                // Degenerate triangles are skipped per pixel on the CPU; leaving them out of
                // the buckets is the same.
                if den.abs() < 1e-12 {
                    continue;
                }
                let ys = [pa.1, pb.1, pc.1];
                let y0 = (ys[0].min(ys[1]).min(ys[2]) - 0.5).floor().max(0.0) as usize;
                let y1 = (ys[0].max(ys[1]).max(ys[2]) + 0.5).ceil().max(0.0) as usize;
                for bucket in buckets.iter_mut().take(y1.min(hh)).skip(y0) {
                    bucket.push(t);
                }
                let (sa, sb, sc) = (srcp[ia], srcp[ib], srcp[ic]);
                tri_data.extend(
                    [(pb.1 - pc.1) / den, (pc.0 - pb.0) / den, (pc.1 - pa.1) / den, (pa.0 - pc.0) / den, pc.0, pc.1, sa.0, sa.1, sb.0, sb.1, sc.0, sc.1]
                        .map(|v| v as f32),
                );
                t += 1;
            }
        }
    }
    let row_starts = tri_data.len();
    let list_start = row_starts + hh + 1;
    let total: usize = buckets.iter().map(Vec::len).sum();
    if list_start + total >= 1 << 24 {
        // Indices past 2^24 are not exact in the f32 data buffer.
        return None;
    }
    let mut data = tri_data;
    data.reserve(hh + 1 + total);
    let mut n = 0usize;
    for bucket in &buckets {
        data.push(n as f32);
        n += bucket.len();
    }
    data.push(n as f32);
    for bucket in &buckets {
        data.extend(bucket.iter().map(|&t| t as f32));
    }
    let mut p = Params::default();
    p.u[0] = [row_starts as u32, list_start as u32, 0, 0];
    let buf = e.data(&data);
    run(e, "dst_mesh", &p, b, Some(&buf))
}

// ---------------------------------------------------------------- misc.rs (Stylize)

fn mosaic(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let hb = ctx.params.f("horizontal").round().max(1.0) as usize;
    let vb = ctx.params.f("vertical").round().max(1.0) as usize;
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    if w == 0 || h == 0 {
        return Some(b);
    }
    let cw = (w as f64 / hb as f64).max(1.0);
    let ch = (h as f64 / vb as f64).max(1.0);
    let mut p = Params::default();
    if ctx.params.b("sharpColors") {
        let centre = |i: usize, c: f64| (((i as f64 / c).floor() + 0.5) * c) as i64 as f32;
        let data: Vec<f32> = (0..w).map(|x| centre(x, cw)).chain((0..h).map(|y| centre(y, ch))).collect();
        p.u[0][0] = w as u32;
        let buf = e.data(&data);
        return run(e, "dst_mosaic_sharp", &p, b, Some(&buf));
    }
    let (nx, ny) = ((w as f64 / cw).ceil() as usize, (h as f64 / ch).ceil() as usize);
    let tx: Vec<usize> = (0..w).map(|x| ((x as f64 / cw) as usize).min(nx - 1)).collect();
    let ty: Vec<usize> = (0..h).map(|y| ((y as f64 / ch) as usize).min(ny - 1)).collect();
    // Tile bounds: tile i covers the columns with tx == i (tx is non-decreasing).
    let bounds = |t: &[usize], n: usize| -> Vec<f32> { (0..=n).map(|i| t.partition_point(|&v| v < i) as f32).collect() };
    let mut data: Vec<f32> = tx.iter().chain(&ty).map(|&v| v as f32).collect();
    data.extend(bounds(&tx, nx));
    data.extend(bounds(&ty, ny));
    p.u[0] = [w as u32, h as u32, nx as u32, ny as u32];
    let buf = e.data(&data);
    let rows = e.image(nx as u32, h as u32);
    e.pixels("dst_mosaic_rows", &p, &b.img, None, &rows, Some(&buf));
    let tiles = e.image(nx as u32, ny as u32);
    e.pixels("dst_mosaic_tiles", &p, &rows, None, &tiles, Some(&buf));
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("dst_mosaic_out", &p, &tiles, None, &out, Some(&buf));
    b.img = out;
    Some(b)
}

fn find_edges(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let mut p = Params::default();
    p.u[0][0] = ctx.params.b("invert") as u32;
    p.f[0][0] = ctx.params.f("blend") as f32 / 100.0;
    run(e, "dst_findedges", &p, b, None)
}

fn emboss(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let ang = ctx.params.f("direction").to_radians();
    let relief = ctx.params.f("relief") * b.scale;
    let (dx, dy) = (ang.cos() * relief, -ang.sin() * relief);
    let mut p = Params::default();
    p.f[0] = [dx as f32, dy as f32, ctx.params.f("contrast") as f32 / 100.0, ctx.params.f("blend") as f32 / 100.0];
    run(e, "dst_emboss", &p, b, None)
}
