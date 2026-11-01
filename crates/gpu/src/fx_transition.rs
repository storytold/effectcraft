//! GPU effects, transitions and perspective (kernels in `shaders/fx_transition.wgsl`, every
//! entry point prefixed `ftr_`): Block Dissolve, CC Glass Wipe, CC Grid Wipe, CC Image Wipe,
//! CC Jaws, CC Line Sweep, CC Radial ScaleWipe, CC Scale Wipe, CC Twister, CC WarpoMatic,
//! Radial Shadow, CC Bender, CC Blobbylize, CC Cylinder, CC Sphere, CC Spotlight, CC
//! Environment and 3D Glasses, with the CPU effects' exact steps.
//!
//! Scalars (corner distances, projected extents, light vectors, rotation matrices) are computed
//! on the CPU in f64 as the CPU effect does; Block Dissolve's block indices too (one per column
//! and row sub-sample), so the hash sees the CPU's exact integers. Other layers (gradient,
//! reveal, backside, environment, stereo views) are fetched and fitted on the CPU
//! (`util::fit_layer`) and uploaded. Gradient planes are blurred with the CPU's
//! `util::gauss_plane` steps (box passes, edges repeated). Card Wipe (a 3D card renderer
//! shared with Card Dance and Shatter) stays on the CPU.

use effectcraft_effects::{Buf, EffectCtx};
use effectcraft_raster::Image;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, gaussian_blur};

/// Compute entry points in `fx_transition.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "ftr_plane",
    "ftr_mul",
    "ftr_under",
    "ftr_blocks",
    "ftr_grid",
    "ftr_radial_scale",
    "ftr_scale",
    "ftr_glass",
    "ftr_image_wipe",
    "ftr_jaws",
    "ftr_line_sweep",
    "ftr_twister",
    "ftr_react_contrast",
    "ftr_react_local",
    "ftr_warpo",
    "ftr_rshadow",
    "ftr_bender",
    "ftr_blob",
    "ftr_cylinder",
    "ftr_sphere",
    "ftr_spot",
    "ftr_env",
    "ftr_shift",
    "ftr_glasses",
];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.transition.blockdissolve",
    "ec.transition.ccglasswipe",
    "ec.transition.ccgridwipe",
    "ec.transition.ccimagewipe",
    "ec.transition.ccjaws",
    "ec.transition.cclinesweep",
    "ec.transition.ccradialscalewipe",
    "ec.transition.ccscalewipe",
    "ec.transition.cctwister",
    "ec.transition.ccwarpomatic",
    "ec.perspective.radialshadow",
    "ec.distort.ccbender",
    "ec.distort.ccblobbylize",
    "ec.perspective.cccylinder",
    "ec.perspective.ccsphere",
    "ec.perspective.ccspotlight",
    "ec.perspective.ccenvironment",
    "ec.perspective.3dglasses",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.transition.blockdissolve" => block_dissolve(e, ctx, b),
        "ec.transition.ccglasswipe" => glass_wipe(e, ctx, b),
        "ec.transition.ccgridwipe" => grid_wipe(e, ctx, b),
        "ec.transition.ccimagewipe" => image_wipe(e, ctx, b),
        "ec.transition.ccjaws" => jaws(e, ctx, b),
        "ec.transition.cclinesweep" => line_sweep(e, ctx, b),
        "ec.transition.ccradialscalewipe" => radial_scale_wipe(e, ctx, b),
        "ec.transition.ccscalewipe" => scale_wipe(e, ctx, b),
        "ec.transition.cctwister" => twister(e, ctx, b),
        "ec.transition.ccwarpomatic" => warpomatic(e, ctx, b),
        "ec.perspective.radialshadow" => radial_shadow(e, ctx, b),
        "ec.distort.ccbender" => bender(e, ctx, b),
        "ec.distort.ccblobbylize" => blobbylize(e, ctx, b),
        "ec.perspective.cccylinder" => cylinder(e, ctx, b),
        "ec.perspective.ccsphere" => sphere(e, ctx, b),
        "ec.perspective.ccspotlight" => spotlight(e, ctx, b),
        "ec.perspective.ccenvironment" => environment(e, ctx, b),
        "ec.perspective.3dglasses" => glasses(e, ctx, b),
        _ => None,
    }
}

// ---------------------------------------------------------------- helpers

/// A same-size per-pixel kernel over `src` (the buffer's size).
fn kernel(e: &mut Enc, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels(entry, p, src, aux, &out, data);
    out
}

fn run(e: &mut Enc, entry: &str, p: &Params, mut b: GBuf, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> Option<GBuf> {
    b.img = kernel(e, entry, p, &b.img, aux, data);
    Some(b)
}

fn f4(v: [f64; 4]) -> [f32; 4] {
    v.map(|x| x as f32)
}

fn rgb(c: [f32; 4], w: f32) -> [f32; 4] {
    [c[0], c[1], c[2], w]
}

/// The CPU buffer geometry of `b` (no pixels: `fit_layer` reads only size, offset and scale).
fn shape(b: &GBuf) -> Buf {
    Buf { img: Image { width: b.img.width, height: b.img.height, data: vec![] }, offset: b.offset, scale: b.scale }
}

/// The layer in layer parameter `id` fitted to `b` (`util::fit_layer`) and uploaded; `Some(None)`
/// when none is chosen.
fn fitted(e: &mut Enc, ctx: &EffectCtx, b: &GBuf, id: &str, masks_and_effects: bool, stretch: bool) -> Option<Option<GpuImage>> {
    match ctx.layer_param(id, masks_and_effects) {
        Some(o) => Some(Some(e.g.upload_image(&effectcraft_effects::util::fit_layer(ctx, &shape(b), &o, stretch))?)),
        None => Some(None),
    }
}

/// `util::layer_or_self`: the fitted layer, or the buffer itself.
fn layer_or_self(e: &mut Enc, ctx: &EffectCtx, b: &GBuf, id: &str) -> Option<GpuImage> {
    Some(fitted(e, ctx, b, id, true, false)?.unwrap_or_else(|| b.img.clone()))
}

/// A plane (x) of `img`: `util::Src` index `src` times alpha (alpha itself unscaled); 11 is CC
/// Environment's relief.
fn plane(e: &mut Enc, img: &GpuImage, src: u32) -> GpuImage {
    let mut p = Params::default();
    p.u[0][0] = src;
    kernel(e, "ftr_plane", &p, img, None, None)
}

/// transition2::prop_plane's property → `util::Src` index.
fn prop_src(prop: u32) -> u32 {
    match prop {
        5 => 6,
        6 => 7,
        p => p.min(4),
    }
}

/// `img` as RGBA rows for the kernels' third image (`P.u[3]` = stride, width, height).
fn rows(e: &mut Enc, p: &mut Params, img: &GpuImage) -> wgpu::Buffer {
    let (buf, stride) = e.image_rows(img);
    p.u[3] = [stride, img.width, img.height, 0];
    buf
}

fn completion(ctx: &EffectCtx) -> f64 {
    (ctx.params.f("completion") / 100.0).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------- Block Dissolve (transition.rs)

fn block_dissolve(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = completion(ctx) as f32;
    if done <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let bw = (pr.f("blockWidth") * b.scale).max(1e-3);
    let bh = (pr.f("blockHeight") * b.scale).max(1e-3);
    let feather = pr.f("feather") * b.scale;
    let subs: Vec<f64> = if pr.b("softEdges") { (0..4).map(|k| (k as f64 + 0.5) / 4.0).collect() } else { vec![0.5] };
    // Block indices per column / row sub-sample, as the CPU computes them (f64, then u32).
    let index =
        |n: u32, o: f64, size: f64| (0..n).flat_map(|i| subs.iter().map(move |s| (((i as f64 + s - o) / size).floor() as i64) as u32)).collect::<Vec<_>>();
    let cols = index(b.img.width, b.offset[0], bw);
    let table: Vec<u8> = cols.iter().chain(&index(b.img.height, b.offset[1], bh)).flat_map(|v| v.to_le_bytes()).collect();
    let data = e.bytes(table);
    let mut p = Params::default();
    p.u[0] = [subs.len() as u32, ctx.seed ^ 0xb10c, cols.len() as u32, 0];
    p.f[0][0] = done;
    let mut mask = kernel(e, "ftr_blocks", &p, &b.img, None, Some(&data));
    if feather > 0.0 {
        mask = gaussian_blur(e, &mask, feather * 0.5, feather * 0.5, true);
    }
    run(e, "ftr_mul", &Params::default(), b, Some(&mask), None)
}

// ---------------------------------------------------------------- CC Grid Wipe / Radial ScaleWipe / Scale Wipe (transition.rs)

/// transition::max_corner_dist.
fn max_corner_dist(b: &GBuf, c: (f64, f64), metric: impl Fn(f64, f64) -> f64) -> f64 {
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| metric(x - c.0, y - c.1)).fold(1e-6, f64::max)
}

fn grid_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = completion(ctx);
    if done <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let c = b.to_px(pr.v2("center"));
    let rot = pr.f("rotation").to_radians();
    let border = pr.f("border").max(0.0) * b.scale;
    let tiles = pr.f("tiles").round().clamp(1.0, 500.0);
    let shape = pr.e("shape");
    let cs = (ctx.layer_size[0] * b.scale / tiles).max(1.0);
    let (s, co) = rot.sin_cos();
    let metric = |dx: f64, dy: f64| match shape {
        0 => dx.abs(),
        2 => dx.abs().max(dy.abs()),
        _ => dx.hypot(dy),
    };
    let maxd = max_corner_dist(&b, c, |dx, dy| metric(dx * co + dy * s, -dx * s + dy * co));
    let mut p = Params::default();
    p.u[0] = [shape, pr.b("reverse") as u32, 0, 0];
    p.f[0] = f4([c.0, c.1, s, co]);
    p.f[1] = f4([cs, maxd, done, border]);
    run(e, "ftr_grid", &p, b, None, None)
}

fn radial_scale_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let done = completion(ctx);
    if done <= 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let maxd = max_corner_dist(&b, c, f64::hypot);
    let mut p = Params::default();
    p.u[0][0] = ctx.params.b("reverse") as u32;
    p.f[0] = f4([c.0, c.1, maxd, done]);
    run(e, "ftr_radial_scale", &p, b, None, None)
}

fn scale_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let stretch = ctx.params.f("stretch").max(0.0);
    if stretch <= 0.0 {
        return Some(b);
    }
    let c = b.to_px(ctx.params.v2("center"));
    let a = ctx.params.f("direction").to_radians();
    let mut p = Params::default();
    p.f[0] = f4([c.0, c.1, a.sin(), -a.cos()]);
    p.f[1][0] = stretch as f32;
    run(e, "ftr_scale", &p, b, None, None)
}

// ---------------------------------------------------------------- CC Glass Wipe / Image Wipe / WarpoMatic (transition2.rs)

fn glass_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx) as f32;
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let gimg = layer_or_self(e, ctx, &b, "gradientLayer")?;
    let mut g = plane(e, &gimg, 4);
    let sm = pr.f("softness") * b.scale * 0.2;
    if sm > 0.05 {
        g = gaussian_blur(e, &g, sm, sm, true);
    }
    let soft = (pr.f("softness") / 100.0) as f32;
    let rev = fitted(e, ctx, &b, "layerToReveal", true, false)?;
    let mut p = Params::default();
    let data = rev.as_ref().map(|r| rows(e, &mut p, r));
    p.u[0][0] = rev.is_some() as u32;
    p.f[0] = [c, soft * 0.5 + 0.01, (pr.f("displacementAmount") * b.scale * 4.0) as f32, 0.0];
    run(e, "ftr_glass", &p, b, Some(&g), data.as_ref())
}

fn image_wipe(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx) as f32;
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let gimg = layer_or_self(e, ctx, &b, "layer")?;
    let mut g = plane(e, &gimg, prop_src(pr.e("property")));
    let bl = pr.f("blur") * b.scale * 0.5;
    if bl > 0.05 {
        g = gaussian_blur(e, &g, bl, bl, true);
    }
    let mut w = (pr.f("borderSoftness") / 100.0) as f32 * 0.5;
    if pr.b("autoSoftness") {
        w = w.max(0.02);
    }
    let mut p = Params::default();
    p.u[0][0] = pr.b("inverseGradient") as u32;
    p.f[0] = [c, w, 0.0, 0.0];
    run(e, "ftr_image_wipe", &p, b, Some(&g), None)
}

fn warpomatic(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx) as f32;
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let rimg = layer_or_self(e, ctx, &b, "reactorLayer")?;
    let mut m = plane(e, &rimg, 4);
    let sm = pr.f("smoothness") * b.scale * 0.5;
    if sm > 0.05 {
        m = gaussian_blur(e, &m, sm, sm, true);
    }
    m = match pr.e("reactor") {
        1 => {
            let mean = gaussian_blur(e, &m, 8.0 * b.scale, 8.0 * b.scale, true);
            kernel(e, "ftr_react_contrast", &Params::default(), &m, Some(&mean), None)
        }
        2 => kernel(e, "ftr_react_local", &Params::default(), &m, Some(&m), None),
        _ => m,
    };
    let rev = fitted(e, ctx, &b, "layerToReveal", true, false)?;
    let mut p = Params::default();
    let data = rev.as_ref().map(|r| rows(e, &mut p, r));
    p.u[0] = [pr.e("warpDirection"), rev.is_some() as u32, 0, 0];
    p.f[0] = [c, (pr.f("blendSpan") / 100.0) as f32 * 0.5, (pr.f("warpAmount") * b.scale) as f32 * 20.0, 0.0];
    run(e, "ftr_warpo", &p, b, Some(&m), data.as_ref())
}

// ---------------------------------------------------------------- CC Jaws / Line Sweep / Twister (transition2.rs)

fn jaws(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx);
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let (cx, cy) = b.to_px(pr.v2("center"));
    let (sn, cs) = pr.f("direction").to_radians().sin_cos();
    let width = (pr.f("width") * b.scale).max(1.0);
    let amp = pr.f("height") / 100.0 * width * 0.5;
    let (bw, bh) = (b.img.width as f64, b.img.height as f64);
    let vmax = [(0.0, 0.0), (bw, 0.0), (0.0, bh), (bw, bh)].iter().map(|&(x, y)| (-(x - cx) * sn + (y - cy) * cs).abs()).fold(0.0, f64::max);
    let d = c * (vmax + 2.0 * amp + 2.0);
    let mut p = Params::default();
    p.u[0][0] = pr.e("shape");
    p.f[0] = f4([cx, cy, sn, cs]);
    p.f[1] = f4([width, amp, d, 0.0]);
    run(e, "ftr_jaws", &p, b, None, None)
}

fn line_sweep(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx);
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let (sn, cs) = pr.f("direction").to_radians().sin_cos();
    let thick = (pr.f("thickness") * b.scale).max(1.0);
    let slant = pr.f("slant") * b.scale;
    let (w, h) = (b.img.width as f64, b.img.height as f64);
    let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
        let (u, v) = (x * cs + y * sn, -x * sn + y * cs);
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
    }
    let nstr = ((v1 - v0) / thick).ceil().max(1.0);
    let total = (u1 - u0) + slant.abs() * nstr + 1.0;
    let mut p = Params::default();
    p.u[0] = [pr.b("flipDirection") as u32, (slant >= 0.0) as u32, 0, 0];
    p.f[0] = f4([sn, cs, v0, u0]);
    p.f[1] = f4([thick, slant.abs(), nstr, total]);
    p.f[2][0] = c as f32;
    run(e, "ftr_line_sweep", &p, b, None, None)
}

fn twister(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let c = completion(ctx);
    if c <= 0.0 {
        return Some(b);
    }
    let pr = ctx.params;
    let (cx, cy) = b.to_px(pr.v2("center"));
    let (sn, cs) = pr.f("axis").to_radians().sin_cos();
    let half = (b.img.width as f64).hypot(b.img.height as f64) * 0.5;
    let back = fitted(e, ctx, &b, "backside", true, false)?;
    let mut p = Params::default();
    p.u[0] = [back.is_some() as u32, pr.b("shading") as u32, 0, 0];
    p.f[0] = f4([cx, cy, sn, cs]);
    p.f[1] = f4([half, c, 0.0, 0.0]);
    run(e, "ftr_twister", &p, b, back.as_ref(), None)
}

// ---------------------------------------------------------------- Radial Shadow / CC Cylinder / CC Sphere / CC Spotlight (perspective.rs)

fn radial_shadow(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let color = pr.color("color");
    let soft = pr.f("softness").max(0.0) * b.scale;
    let k = 1.0 + pr.f("projectionDistance").max(0.0) / 100.0;
    let lp0 = pr.v2("lightSource");
    if pr.b("resizeLayer") && !ctx.adjustment {
        let l = b.to_px(lp0);
        let (bw, bh) = (b.img.width as f64, b.img.height as f64);
        let grow = [(0.0, 0.0), (bw, 0.0), (0.0, bh), (bw, bh)].iter().map(|(cx, cy)| ((cx - l.0).hypot(cy - l.1)) * (k - 1.0)).fold(0.0, f64::max);
        b.pad(e, (grow + soft * 1.5).ceil() as u32 + 1)?;
    }
    let l = b.to_px(lp0);
    let mut p = Params::default();
    p.u[0][0] = (pr.e("render") == 1) as u32;
    p.f[0] = [l.0 as f32, l.1 as f32, k as f32, pr.f("opacity") as f32 / 100.0 * color[3]];
    p.f[1] = rgb(color, pr.f("colorInfluence") as f32 / 100.0);
    let mut sh = kernel(e, "ftr_rshadow", &p, &b.img, None, None);
    if soft > 0.0 {
        sh = gaussian_blur(e, &sh, soft * 0.5, soft * 0.5, false);
    }
    if !pr.b("shadowOnly") {
        sh = kernel(e, "ftr_under", &Params::default(), &sh, Some(&b.img), None);
    }
    b.img = sh;
    Some(b)
}

fn norm3(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

/// perspective::Shading into f[3..7]; the layer rectangle into f[1].
fn shading(ctx: &EffectCtx, b: &GBuf, p: &mut Params) {
    let pr = ctx.params;
    let (a, el) = (pr.f("lightDirection").to_radians(), pr.f("lightHeight").to_radians());
    let l = norm3([a.sin() * el.cos(), -a.cos() * el.cos(), el.sin()]);
    let hv = norm3([l[0], l[1], l[2] + 1.0]);
    p.f[1] = f4([b.offset[0], b.offset[1], ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale]);
    p.f[3] = [l[0] as f32, l[1] as f32, l[2] as f32, pr.f("lightIntensity") as f32 / 100.0];
    p.f[4] = rgb(pr.color("lightColor"), pr.f("ambient") as f32 / 100.0);
    p.f[5] = [pr.f("diffuse") as f32 / 100.0, pr.f("specular") as f32 / 100.0, 1.0 / (pr.f("roughness") as f32).max(0.001), 0.0];
    p.f[6] = [hv[0] as f32, hv[1] as f32, hv[2] as f32, 0.0];
}

fn cylinder(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let (w, h) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let r = (pr.f("radius") / 100.0 * w / std::f64::consts::TAU).max(0.5);
    let c = b.to_px(pr.v2("position"));
    let mut p = Params::default();
    shading(ctx, &b, &mut p);
    p.u[0][0] = pr.e("render");
    p.f[0] = f4([c.0, c.1, r, h]);
    p.f[2][0] = pr.f("rotation").to_radians() as f32;
    let (rx, ry, rz) = (pr.f("rotationX"), pr.f("rotationY"), pr.f("rotationZ"));
    p.u[0][1] = (rx != 0.0 || ry != 0.0 || rz != 0.0) as u32;
    let m = rot_xyz(rx, ry, rz);
    // Matrix rows, as for CC Sphere; the kernel uses Rᵀ for mapping.
    for (i, row) in m.iter().enumerate() {
        p.f[7 + i] = f4([row[0], row[1], row[2], 0.0]);
    }
    run(e, "ftr_cylinder", &p, b, None, None)
}

/// perspective::rot_xyz (R = Rz · Ry · Rx).
fn rot_xyz(rx: f64, ry: f64, rz: f64) -> [[f64; 3]; 3] {
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();
    let mx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let my = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let mz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| {
        let mut o = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    };
    mul(mz, mul(my, mx))
}

fn sphere(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let r = (pr.f("radius") * b.scale).max(0.5);
    let c = b.to_px(pr.v2("offset"));
    let m = rot_xyz(pr.f("rotationX"), pr.f("rotationY"), pr.f("rotationZ"));
    let mut p = Params::default();
    shading(ctx, &b, &mut p);
    p.u[0][0] = pr.e("render");
    p.f[0] = f4([c.0, c.1, r, 0.0]);
    // Rows of M: the kernel computes Mᵀ · q as q.x·row0 + q.y·row1 + q.z·row2.
    for (i, row) in m.iter().enumerate() {
        p.f[7 + i] = f4([row[0], row[1], row[2], 0.0]);
    }
    run(e, "ftr_sphere", &p, b, None, None)
}

fn spotlight(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let (w, h) = (ctx.layer_size[0] * b.scale, ctx.layer_size[1] * b.scale);
    let from = b.to_px(pr.v2("from"));
    let to = b.to_px(pr.v2("to"));
    let height = (pr.f("height") / 100.0 * w.max(h)).max(1e-3);
    let cone = pr.f("coneAngle").clamp(0.1, 89.9).to_radians();
    let soft = (pr.f("edgeSoftness") / 100.0).clamp(0.0, 1.0);
    let axis = norm3([to.0 - from.0, to.1 - from.1, -height]);
    let mut p = Params::default();
    p.u[0][0] = (pr.e("render") == 1) as u32;
    p.f[0] = [from.0 as f32, from.1 as f32, height as f32, pr.f("intensity") as f32 / 100.0];
    p.f[1] = f4([axis[0], axis[1], axis[2], cone * (1.0 - soft)]);
    p.f[2] = rgb(pr.color("color"), cone as f32);
    run(e, "ftr_spot", &p, b, None, None)
}

// ---------------------------------------------------------------- CC Environment / 3D Glasses (perspective2.rs)

fn environment(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let (mut env, es, eo, esz) = match ctx.layer_param("environment", true) {
        Some(o) => (e.g.upload_image(&o.buf.img)?, o.buf.scale, o.buf.offset, o.size),
        None => (b.img.clone(), b.scale, b.offset, ctx.layer_size),
    };
    if pr.b("filterEnvironment") {
        env = gaussian_blur(e, &env, 2.0 * es, 2.0 * es, true);
    }
    let relief = plane(e, &b.img, 11);
    let soft = 1.5 * b.scale.max(0.25);
    let hmap = gaussian_blur(e, &relief, soft, soft, true);
    let mut p = Params::default();
    let data = rows(e, &mut p, &env);
    p.u[0][0] = pr.e("mapping");
    p.f[0] = f4([1.0 / b.scale.max(1e-6), pr.f("height") / 100.0 * 20.0, es, 0.0]);
    p.f[1] = f4([eo[0], eo[1], esz[0], esz[1]]);
    run(e, "ftr_env", &p, b, Some(&hmap), Some(&data))
}

fn glasses(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let mut l = fitted(e, ctx, &b, "leftView", true, false)?.unwrap_or_else(|| b.img.clone());
    let mut r = fitted(e, ctx, &b, "rightView", true, false)?.unwrap_or_else(|| b.img.clone());
    if pr.b("leftRightSwap") {
        std::mem::swap(&mut l, &mut r);
    }
    let unit = if pr.e("units") == 1 { ctx.layer_size[0] / 100.0 } else { 1.0 } * b.scale;
    let conv = pr.f("sceneConvergence") * unit;
    let va = pr.f("verticalAlignment") * unit;
    let mut shift = |img: GpuImage, dx: f64, dy: f64| {
        if dx == 0.0 && dy == 0.0 {
            return img;
        }
        let mut p = Params::default();
        p.f[0] = f4([dx, dy, 0.0, 0.0]);
        kernel(e, "ftr_shift", &p, &img, None, None)
    };
    let l = shift(l, conv * 0.5, 0.0);
    let r = shift(r, -conv * 0.5, va);
    let mut p = Params::default();
    p.u[0][0] = pr.e("view3d");
    p.f[0] = [b.img.width as f32, b.img.height as f32, (pr.f("balance") / 20.0).clamp(0.0, 1.0) as f32, 0.0];
    let out = kernel(e, "ftr_glasses", &p, &l, Some(&r), None);
    Some(GBuf { img: out, ..b })
}

// ---------------------------------------------------------------- CC Bender / CC Blobbylize (distort3.rs)

fn bender(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let amount = pr.f("amount") / 100.0;
    if amount == 0.0 {
        return Some(b);
    }
    let top = b.to_px(pr.v2("top"));
    let base = b.to_px(pr.v2("base"));
    let (ax, ay) = (top.0 - base.0, top.1 - base.1);
    let len = (ax * ax + ay * ay).sqrt().max(1e-6);
    let (ux, uy) = (ax / len, ay / len);
    let reference = if pr.b("adjustToDistance") { len } else { ctx.layer_size[1] * b.scale };
    let mut p = Params::default();
    p.u[0][0] = pr.e("style");
    p.f[0] = f4([base.0, base.1, ux, uy]);
    p.f[1] = f4([len, -uy, ux, amount * reference]);
    run(e, "ftr_bender", &p, b, None, None)
}

fn blobbylize(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let soft = pr.f("softness").max(0.0) * b.scale;
    let blob = layer_or_self(e, ctx, &b, "blobLayer")?;
    let raw = plane(e, &blob, pr.e("property").min(10));
    let h = gaussian_blur(e, &raw, soft * 0.42, soft * 0.42, true);
    let lpos = b.to_px(pr.v2("lightPosition"));
    let lheight = pr.f("lightHeight");
    let (d, el) = (pr.f("lightDirection").to_radians(), (lheight / 100.0).clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2);
    let ldir = [d.sin() * el.cos(), -d.cos() * el.cos(), el.sin()];
    let span = (b.img.width.max(b.img.height) as f64).max(1.0);
    let mut p = Params::default();
    p.f[0] = f4([(pr.f("cutAway") / 100.0).clamp(0.0, 1.0), pr.f("lightIntensity") / 100.0, pr.f("ambient") / 100.0, soft * 1.5]);
    p.f[1] = f4([pr.f("diffuse") / 100.0, pr.f("specular") / 100.0, (1.0 / pr.f("roughness").clamp(0.001, 1.0)).min(500.0), pr.f("metal") / 100.0]);
    p.f[2] = f4([lpos.0, lpos.1, lheight / 100.0 * span, (pr.e("lightType") == 1) as u32 as f64]);
    p.f[3] = f4([ldir[0], ldir[1], ldir[2], 0.0]);
    p.f[4] = rgb(pr.color("lightColor"), 0.0);
    p.f[5] = f4([if soft > 0.05 { span / 64.0 } else { 0.0 }, span * 0.34 * b.scale, (soft * 0.65).max(1.0), 1.25]);
    p.f[6] = f4([span * 0.5, 0.0, 0.0, 0.0]);
    run(e, "ftr_blob", &p, b, Some(&h), None)
}
