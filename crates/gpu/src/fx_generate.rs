//! GPU effects, blur, transition and generate family (kernels in `shaders/fx_generate.wgsl`): the CPU effects' exact
//! steps with the pixel loops as compute kernels.
//!
//! Every effect here covers all of its parameters. Where the CPU works in f64 around a hard
//! edge (the wipes), the per-column and per-row terms are computed in f64 here and uploaded as
//! (hi, lo) f32 pairs so the GPU puts the edge where the CPU does.

use effectcraft_color::BlendMode;
use effectcraft_effects::{EffectCtx, GrainLook};

use crate::context::{Enc, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_generate.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "gen_radial_blur",
    "gen_ccradialfast",
    "gen_lens_pre",
    "gen_row_prefix",
    "gen_lens_blur",
    "gen_linear_wipe",
    "gen_venetian",
    "gen_radial_wipe",
    "gen_gradient_wipe",
    "gen_cell_pattern",
    "gen_checker",
    "gen_grid",
    "gen_fourcolor",
    "gen_noise",
    "gen_grain_planes",
    "gen_grain_apply",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.blur.radial",
    "ec.blur.cameralens",
    "ec.blur.ccradialfast",
    "ec.transition.venetian",
    "ec.transition.linearwipe",
    "ec.transition.radialwipe",
    "ec.transition.gradientwipe",
    "ec.generate.cellpattern",
    "ec.generate.checkerboard",
    "ec.generate.grid",
    "ec.generate.fourcolor",
    "ec.noise.noise",
    "ec.noise.addgrain",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.blur.radial" => radial_blur(e, ctx, b),
        "ec.blur.cameralens" => camera_lens(e, ctx, b),
        "ec.blur.ccradialfast" => radial_fast(e, ctx, b),
        "ec.transition.venetian" => venetian(e, ctx, b),
        "ec.transition.linearwipe" => linear_wipe(e, ctx, b),
        "ec.transition.radialwipe" => radial_wipe(e, ctx, b),
        "ec.transition.gradientwipe" => gradient_wipe(e, ctx, b),
        "ec.generate.cellpattern" => cell_pattern(e, ctx, b),
        "ec.generate.checkerboard" => checkerboard(e, ctx, b),
        "ec.generate.grid" => grid(e, ctx, b),
        "ec.generate.fourcolor" => four_color(e, ctx, b),
        "ec.noise.noise" => noise(e, ctx, b),
        "ec.noise.addgrain" => add_grain(e, ctx, b),
        _ => None,
    }
}

/// Run a per-pixel kernel over the buffer.
fn run(e: &mut Enc, entry: &str, p: &Params, b: GBuf, aux: Option<&crate::context::GpuImage>, data: Option<&[f32]>) -> Option<GBuf> {
    let buf = data.map(|d| e.data(d));
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels(entry, p, &b.img, aux, &out, buf.as_ref());
    Some(GBuf { img: out, ..b })
}

/// `v` as an f32 (hi, lo) pair.
fn hl(v: f64) -> [f32; 2] {
    let h = v as f32;
    [h, (v - h as f64) as f32]
}

/// Generate blending mode index → `BlendMode::ALL` index (255 = None: replace).
pub(crate) fn gen_mode(i: u32) -> u32 {
    match effectcraft_effects::gen_mode(i) {
        None => 255,
        Some(m) => BlendMode::ALL.iter().position(|&x| x == m).unwrap_or(0) as u32,
    }
}

// ---------------------------------------------------------------- blurs

fn radial_blur(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amt = ctx.params.f("amount");
    let zoom = ctx.params.e("type") == 1;
    let c = b.to_px(ctx.params.v2("center"));
    let amount = if zoom { amt / 100.0 } else { amt };
    if amount.abs() < 1e-6 {
        return Some(b);
    }
    let n = 32usize;
    let mut data = Vec::with_capacity(2 * n);
    for i in 0..n {
        let t = i as f64 / (n - 1) as f64 - 0.5;
        if zoom {
            data.push((1.0 + t * amount) as f32);
        } else {
            let (s, c) = (t * amount).to_radians().sin_cos();
            data.extend([s as f32, c as f32]);
        }
    }
    let mut p = Params::default();
    p.u[0] = [zoom as u32, n as u32, 0, 0];
    p.f[0] = [c.0 as f32, c.1 as f32, 0.0, 0.0];
    run(e, "gen_radial_blur", &p, b, None, Some(&data))
}

fn radial_fast(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let s = (ctx.params.f("amount") / 100.0).clamp(0.0, 1.0) * 0.5;
    if s <= 0.0 {
        return Some(b);
    }
    if !ctx.adjustment {
        b.pad(e, effectcraft_effects::radial_fast_pad(b.to_px(ctx.params.v2("center")), b.img.width, b.img.height, s))?;
    }
    let c = b.to_px(ctx.params.v2("center"));
    let mut p = Params::default();
    p.u[0][0] = ctx.params.e("zoom");
    p.f[0] = [c.0 as f32, c.1 as f32, s as f32, 0.0];
    run(e, "gen_ccradialfast", &p, b, None, None)
}

fn camera_lens(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let r = ctx.params.f("blurRadius").max(0.0) * b.scale;
    if r < 0.5 {
        return Some(b);
    }
    let (spans, reach) = effectcraft_effects::camera_lens_spans(ctx, r);
    let repeat = ctx.params.b("repeatEdge") || ctx.adjustment;
    if !repeat {
        b.pad(e, reach.ceil() as u32 + 1)?;
    }
    let linear = ctx.params.b("useLinear");
    let thr = ctx.params.f("highlight/specularThreshold") as f32 / 255.0;
    let boost = 1.0 + ctx.params.f("highlight/specularBrightness").max(0.0) as f32 / 100.0 * 4.0;
    let sat = (ctx.params.f("highlight/highlightSaturation") / 100.0).clamp(0.0, 1.0) as f32;
    let (w, h) = (b.img.width, b.img.height);
    let mut src = b.img.clone();
    if linear || boost > 1.0 {
        let mut p = Params::default();
        p.u[0] = [linear as u32, (boost > 1.0) as u32, 0, 0];
        p.f[0] = [thr, boost, sat, 0.0];
        let out = e.scratch(w, h);
        e.pixels("gen_lens_pre", &p, &src, None, &out, None);
        src = out;
    }
    let count: i64 = spans.iter().map(|(_, l, r)| r - l + 1).sum();
    let norm = 1.0 / count as f64;
    // Large irises sum each span from per-row prefix sums; small ones add the pixels directly.
    let prefix = count > 256;
    let pre = if prefix {
        if !e.g.fits(w + 1, h) {
            return None;
        }
        let pre = e.image(w + 1, h);
        e.dispatch("gen_row_prefix", &Params::default(), &src, None, &pre, None, (h.div_ceil(64), 1));
        Some(pre)
    } else {
        None
    };
    let data: Vec<f32> = spans.iter().flat_map(|&(dy, l, r)| [dy as f32, l as f32, r as f32]).collect();
    let mut p = Params::default();
    p.u[0] = [spans.len() as u32, repeat as u32, linear as u32, prefix as u32];
    p.f[0][0] = norm as f32;
    let buf = e.data(&data);
    let out = e.scratch(w, h);
    e.pixels("gen_lens_blur", &p, &src, pre.as_ref(), &out, Some(&buf));
    b.img = out;
    Some(b)
}

// ---------------------------------------------------------------- transitions

fn linear_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = ctx.params.f("completion") / 100.0;
    let ang = ctx.params.f("angle").to_radians();
    let feather = (ctx.params.f("feather") * b.scale).max(0.001);
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let (dx, dy) = (ang.sin(), -ang.cos());
    let corners = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)];
    let proj: Vec<f64> = corners.iter().map(|(x, y)| x * dx + y * dy).collect();
    let (lo, hi) = (proj.iter().cloned().fold(f64::INFINITY, f64::min), proj.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
    let edge = lo - feather + (hi - lo + 2.0 * feather) * done;
    let mut data = Vec::with_capacity(2 * (b.img.width + b.img.height) as usize);
    for x in 0..b.img.width {
        data.extend(hl((x as f64 + 0.5) * dx - edge));
    }
    for y in 0..b.img.height {
        data.extend(hl((y as f64 + 0.5) * dy));
    }
    let mut p = Params::default();
    p.u[0][0] = 2 * b.img.width;
    p.f[0][0] = feather as f32;
    run(e, "gen_linear_wipe", &p, b, None, Some(&data))
}

fn venetian(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = ctx.params.f("completion") / 100.0;
    let ang = ctx.params.f("direction").to_radians();
    let width = (ctx.params.f("width") * b.scale).max(1.0);
    let feather = (ctx.params.f("feather") * b.scale).max(0.001);
    let (dx, dy) = (ang.cos(), ang.sin());
    let mut data = Vec::with_capacity((6 * b.img.width + 2 * b.img.height) as usize);
    for x in 0..b.img.width {
        let cx = ((x as f64 + 0.5) * dx).rem_euclid(width);
        data.extend(hl(cx - done * width));
        data.extend(hl(cx - width - done * width));
        data.extend(hl(cx - width));
    }
    for y in 0..b.img.height {
        data.extend(hl(((y as f64 + 0.5) * dy).rem_euclid(width)));
    }
    let mut p = Params::default();
    p.u[0][0] = 6 * b.img.width;
    p.f[0][0] = feather as f32;
    run(e, "gen_venetian", &p, b, None, Some(&data))
}

fn radial_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = ctx.params.f("completion") / 100.0;
    let start = ctx.params.f("startAngle");
    let c = b.to_px(ctx.params.v2("center"));
    let dir = ctx.params.e("wipe");
    let feather = ctx.params.f("feather").max(0.01);
    let edge = if dir == 2 { done * 180.0 } else { done * 360.0 };
    // Rotations: the start angle, and the start angle ± the edge (where the wipe's edge lies).
    let rots = [start, start + edge, start - edge].map(|r| r.to_radians().sin_cos());
    let mut data = Vec::with_capacity(9 * (b.img.width + b.img.height) as usize);
    for x in 0..b.img.width {
        let ux = x as f64 + 0.5 - c.0;
        for (s, co) in rots {
            data.extend(hl(ux * co));
            data.push((ux * s) as f32);
        }
    }
    for y in 0..b.img.height {
        let uy = -(y as f64 + 0.5 - c.1);
        for (s, co) in rots {
            data.extend(hl(uy * s));
            data.push((uy * co) as f32);
        }
    }
    let mut p = Params::default();
    p.u[0] = [9 * b.img.width, dir.min(2), 0, 0];
    p.f[0] = [edge as f32, feather as f32, 0.0, 0.0];
    run(e, "gen_radial_wipe", &p, b, None, Some(&data))
}

fn gradient_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0) as f32;
    if done <= 0.0 {
        return Some(b);
    }
    let soft = (ctx.params.f("softness") / 100.0) as f32;
    // The gradient layer is fetched and placed on the CPU (as the CPU effect does).
    let grad = match ctx.layer_param("gradientLayer", false) {
        Some(o) => {
            // place_layer reads only the buffer's size, offset and scale.
            let img = effectcraft_raster::Image { width: b.img.width, height: b.img.height, data: vec![] };
            let cpu = effectcraft_effects::Buf { img, offset: b.offset, scale: b.scale };
            let img = effectcraft_effects::place_layer(ctx, &cpu, &o, ctx.params.e("gradientPlacement"));
            Some(e.g.upload_image(&img)?)
        }
        None => None,
    };
    let mut p = Params::default();
    p.u[0][0] = ctx.params.b("invert") as u32;
    p.f[0] = [done, soft, 0.0, 0.0];
    let out = e.scratch(b.img.width, b.img.height);
    e.pixels("gen_gradient_wipe", &p, &b.img, Some(grad.as_ref().unwrap_or(&b.img)), &out, None);
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- generate

fn cell_pattern(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let size = (pr.f("size") * b.scale).max(1.0);
    let off = b.to_px(pr.v2("offset"));
    let mut evo = pr.f("evolution") / 360.0;
    if pr.b("evolutionOptions/cycleEvolution") {
        evo = evo.rem_euclid(pr.f("evolutionOptions/cycle").round().max(1.0));
    }
    let seed = (pr.f("evolutionOptions/randomSeed") as i64 as u32).wrapping_mul(7919) ^ 0xce11;
    let tile = pr.b("tilingOptions/enableTiling");
    let nx = pr.f("tilingOptions/cellsHorizontal").round().max(1.0) as i64;
    let ny = pr.f("tilingOptions/cellsVertical").round().max(1.0) as i64;
    if nx > i32::MAX as i64 || ny > i32::MAX as i64 {
        return None;
    }
    let mut p = Params::default();
    p.u[0] = [effectcraft_effects::cell_pattern_kind(pr.e("cellPattern")), pr.e("overflow"), pr.b("invert") as u32, seed];
    p.u[1] = [tile as u32, nx as u32, ny as u32, 0];
    // cos / sin of 2π·(h + evo) only see evo modulo 1.
    p.f[0] = [pr.f("contrast") as f32 / 100.0, pr.f("disperse").clamp(0.0, 1.5) as f32, size as f32, evo.rem_euclid(1.0) as f32];
    p.f[1] = [(0.5 - off.0) as f32, (0.5 - off.1) as f32, 0.0, 0.0];
    run(e, "gen_cell_pattern", &p, b, None, None)
}

/// generate::cell_size.
fn cell_size(ctx: &EffectCtx, b: &GBuf, anchor: (f64, f64)) -> (f64, f64) {
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

fn checkerboard(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let anchor = b.to_px(ctx.params.v2("anchor"));
    let (w, h) = cell_size(ctx, &b, anchor);
    let fw = (ctx.params.f("feather/featherWidth") * b.scale).max(1.0);
    let fh = (ctx.params.f("feather/featherHeight") * b.scale).max(1.0);
    let mut p = Params::default();
    p.u[0][0] = gen_mode(ctx.params.e("blendingMode"));
    p.f[0] = [(0.5 - anchor.0) as f32, (0.5 - anchor.1) as f32, w as f32, h as f32];
    p.f[1] = [fw as f32, fh as f32, ctx.params.f("opacity") as f32 / 100.0, 0.0];
    p.f[2] = ctx.params.color("color");
    run(e, "gen_checker", &p, b, None, None)
}

fn grid(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let anchor = b.to_px(ctx.params.v2("anchor"));
    let (w, h) = cell_size(ctx, &b, anchor);
    let border = (ctx.params.f("border") * b.scale).max(0.0);
    let fw = (ctx.params.f("feather/featherWidth") * b.scale).max(1.0);
    let fh = (ctx.params.f("feather/featherHeight") * b.scale).max(1.0);
    let mut p = Params::default();
    p.u[0] = [gen_mode(ctx.params.e("blendingMode")), ctx.params.b("invertGrid") as u32, 0, 0];
    p.f[0] = [(0.5 - anchor.0) as f32, (0.5 - anchor.1) as f32, w as f32, h as f32];
    p.f[1] = [fw as f32, fh as f32, ctx.params.f("opacity") as f32 / 100.0, border as f32];
    p.f[2] = ctx.params.color("color");
    run(e, "gen_grid", &p, b, None, None)
}

fn four_color(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let mut p = Params::default();
    for i in 0..4 {
        let q = b.to_px(ctx.params.v2(&format!("positionsColors/point{}", i + 1)));
        p.f[i] = [q.0 as f32, q.1 as f32, 0.0, 0.0];
        p.f[4 + i] = ctx.params.color(&format!("positionsColors/color{}", i + 1));
    }
    let k = effectcraft_effects::four_color_softening(ctx.params.f("blend"), b.scale);
    let jitter = (ctx.params.f("jitter") / 100.0).clamp(0.0, 1.0) as f32;
    p.u[0] = [gen_mode(ctx.params.e("blendingMode")), ctx.seed, 0, 0];
    p.f[8] = [k as f32, jitter, ctx.params.f("opacity") as f32 / 100.0, 0.0];
    run(e, "gen_fourcolor", &p, b, None, None)
}

// ---------------------------------------------------------------- noise

fn noise(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let amt = ctx.params.f("amount") as f32 / 100.0;
    let clip = ctx.params.get("clip").is_none_or(|v| v.as_bool());
    let frame_seed = (ctx.time * 1000.0) as u32 ^ ctx.seed;
    let mut p = Params::default();
    p.u[0] = [frame_seed, ctx.params.b("color") as u32, clip as u32, b.img.width];
    p.f[0][0] = amt;
    run(e, "gen_noise", &p, b, None, None)
}

fn add_grain(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let look = GrainLook::from_params(ctx);
    if look.intensity <= 0.0 {
        return Some(b);
    }
    let (w, h) = (b.img.width, b.img.height);
    // GrainLook::planes.
    let size = look.size * b.scale as f32;
    let s = |k: usize| (size * if look.mono { 1.0 } else { look.channel_size[k].max(0.05) }).max(0.05);
    let mut p = Params::default();
    p.u[0] = [look.mono as u32, look.seed, 101, 0];
    p.f[0] = [s(0) * look.aspect, s(1) * look.aspect, s(2) * look.aspect, look.frame];
    p.f[1] = [s(0), s(1), s(2), 0.0];
    let mut planes = e.image(w, h);
    e.pixels("gen_grain_planes", &p, &b.img, None, &planes, None);
    let soft = look.softness * b.scale;
    if soft > 0.05 {
        planes = gaussian_blur(e, &planes, soft * look.aspect as f64, soft, true);
    }
    // GrainLook::color's tint factors.
    let l = effectcraft_color::luminance(look.tint[0], look.tint[1], look.tint[2]).max(1e-3);
    let tint = [0, 1, 2].map(|k| (1.0 - look.tint_amount) + look.tint_amount * look.tint[k] / l);
    let mut p = Params::default();
    p.u[0] = [look.mono as u32, look.mode, (look.tint_amount > 0.0) as u32, 0];
    p.f[0] = [look.intensity * 0.25, look.saturation, look.midpoint, 0.0];
    p.f[1] = [tint[0], tint[1], tint[2], 0.0];
    p.f[2] = [look.channel_intensity[0], look.channel_intensity[1], look.channel_intensity[2], 0.0];
    p.f[3] = [look.tones[0], look.tones[1], look.tones[2], 0.0];
    run(e, "gen_grain_apply", &p, b, Some(&planes), None)
}
