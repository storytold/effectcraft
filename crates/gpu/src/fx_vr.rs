//! GPU effects, Immersive Video family (kernels in `shaders/fx_vr.wgsl`, every entry point
//! prefixed `fxv_`): VR Blur, VR Chromatic Aberrations, VR Color Gradients, VR Converter, VR
//! De-Noise, VR Digital Glitch, VR Fractal Noise, VR Glow, VR Plane to Sphere, VR Rotate Sphere,
//! VR Sharpen and VR Sphere to Plane.
//!
//! The equirectangular maths of `effects::vr` (view directions, the projections and cube-map
//! layouts of VR Converter, seam-wrapping and pole-continuing bilinear sampling, the
//! latitude-widened seam-aware box blurs) is ported to WGSL operation for operation; rotation
//! matrices, per-row blur radii and VR Digital Glitch's per-band offsets are computed on the CPU
//! exactly as the CPU effect does. Stereo layouts run each eye on its own cropped image. Angles
//! between directions use atan2(|a × b|, a · b), the f32-stable form of the CPU's f64 acos;
//! VR Color Gradients' inverse-distance weights are normalised by the nearest point (the same
//! ratios without f32 overflow). VR De-Noise reuses the noise family's guided filter and median.

use effectcraft_effects::EffectCtx;
use effectcraft_raster::hash_noise;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::GBuf;

/// Compute entry points in `fx_vr.wgsl`.
pub(crate) const KERNELS: &[&str] = &["fxv_convert", "fxv_box_h", "fxv_box_v", "fxv_point", "fxv_gen", "fxv_wrap_pad"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] = &[
    "ec.vr.blur",
    "ec.vr.chromaticaberrations",
    "ec.vr.colorgradients",
    "ec.vr.converter",
    "ec.vr.denoise",
    "ec.vr.digitalglitch",
    "ec.vr.fractalnoise",
    "ec.vr.glow",
    "ec.vr.planetosphere",
    "ec.vr.rotatesphere",
    "ec.vr.sharpen",
    "ec.vr.spheretoplane",
];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    if b.img.width == 0 || b.img.height == 0 {
        return Some(b);
    }
    let img = match id {
        "ec.vr.blur" => blur(e, ctx, &b.img, b.scale)?,
        "ec.vr.sharpen" => sharpen(e, ctx, &b.img, b.scale)?,
        "ec.vr.glow" => glow(e, ctx, &b.img, b.scale)?,
        "ec.vr.denoise" => denoise(e, ctx, &b)?,
        "ec.vr.rotatesphere" => rotate(e, ctx, &b.img)?,
        "ec.vr.converter" => converter(e, ctx, &b.img)?,
        "ec.vr.spheretoplane" => {
            let m = rot(ctx, "tilt", "pan", "roll");
            let fov = ctx.params.f("horizontalFov") as f32;
            per_eye(e, &b.img, layout(ctx), |e, img, _| Some(convert(e, img, (0, 360.0), (5, fov), &m, img.width, img.height, None)))?
        }
        "ec.vr.planetosphere" => {
            let fov = ctx.params.f("scale").clamp(1.0, 179.0);
            let m = rot(ctx, "tilt", "pan", "roll");
            let feather = (ctx.params.f("feather") / 100.0).clamp(0.0, 1.0);
            let t = (fov / 2.0).to_radians().tan();
            let (w, h) = (b.img.width, b.img.height);
            convert(e, &b.img, (5, fov as f32), (0, 360.0), &m, w, h, (feather > 0.0).then_some([feather as f32, t as f32]))
        }
        "ec.vr.chromaticaberrations" => chromatic(e, ctx, &b.img)?,
        "ec.vr.colorgradients" => gradients(e, ctx, &b.img)?,
        "ec.vr.fractalnoise" => fractal(e, ctx, &b.img)?,
        "ec.vr.digitalglitch" => glitch(e, ctx, &b.img)?,
        _ => return None,
    };
    Some(GBuf { img, ..b })
}

fn layout(ctx: &EffectCtx) -> u32 {
    ctx.params.e("frameLayout")
}

/// vr::eyes.
fn eyes(w: u32, h: u32, layout: u32) -> Vec<(u32, u32, u32, u32)> {
    match layout {
        1 if h >= 2 => vec![(0, 0, w, h / 2), (0, h / 2, w, h - h / 2)],
        2 if w >= 2 => vec![(0, 0, w / 2, h), (w / 2, 0, w - w / 2, h)],
        _ => vec![(0, 0, w, h)],
    }
}

/// vr::per_eye: run `f` on each eye's own image and reassemble.
fn per_eye(e: &mut Enc, img: &GpuImage, layout: u32, mut f: impl FnMut(&mut Enc, &GpuImage, usize) -> Option<GpuImage>) -> Option<GpuImage> {
    let rects = eyes(img.width, img.height, layout);
    if rects.len() == 1 {
        return f(e, img, 0);
    }
    let out = e.scratch(img.width, img.height);
    for (i, &(x, y, w, h)) in rects.iter().enumerate() {
        let crop = e.scratch(w, h);
        let mut p = Params::default();
        p.u[0] = [x, y, 0, 0];
        e.pixels("fxn_crop", &p, img, None, &crop, None);
        let r = f(e, &crop, i)?;
        e.copy_into(&r, &out, x, y);
    }
    Some(out)
}

fn rot(ctx: &EffectCtx, t: &str, p: &str, r: &str) -> [[f32; 3]; 3] {
    effectcraft_effects::vr_rotation(ctx.params.f(t), ctx.params.f(p), ctx.params.f(r)).map(|r| r.map(|v| v as f32))
}

fn put_matrix(p: &mut Params, at: usize, m: &[[f32; 3]; 3]) {
    for (i, r) in m.iter().enumerate() {
        p.f[at + i] = [r[0], r[1], r[2], 0.0];
    }
}

/// vr::convert: re-project `src` (projection `from` = (kind, fov)) into a `w`×`h` frame in `to`,
/// output direction → source direction = mᵀ · d. `feather` = (feather, tan(fov / 2)) for VR
/// Plane to Sphere's edge.
#[allow(clippy::too_many_arguments)]
fn convert(e: &mut Enc, src: &GpuImage, from: (u32, f32), to: (u32, f32), m: &[[f32; 3]; 3], w: u32, h: u32, feather: Option<[f32; 2]>) -> GpuImage {
    let mut p = Params::default();
    p.u[0] = [from.0, to.0, feather.is_some() as u32, 0];
    p.f[0] = [from.1, to.1, 0.0, 0.0];
    put_matrix(&mut p, 1, m);
    if let Some(f) = feather {
        p.f[4] = [f[0], f[1], 0.0, 0.0];
    }
    let out = e.scratch(w, h);
    e.pixels("fxv_convert", &p, src, None, &out, None);
    out
}

fn rotate(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    if pr.f("tilt") == 0.0 && pr.f("pan") == 0.0 && pr.f("roll") == 0.0 {
        return Some(img.clone());
    }
    let mut m = rot(ctx, "tilt", "pan", "roll");
    if pr.b("invertRotation") {
        m = [[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]];
    }
    per_eye(e, img, layout(ctx), |e, img, _| Some(convert(e, img, (0, 360.0), (0, 360.0), &m, img.width, img.height, None)))
}

fn converter(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    let from = (pr.e("sourceProjection"), pr.f("sourceHorizontalFov"));
    let to = (pr.e("targetProjection"), pr.f("targetHorizontalFov"));
    if from.0 == to.0 && (!matches!(from.0, 4 | 5) || from.1 == to.1) && pr.f("tilt") == 0.0 && pr.f("pan") == 0.0 && pr.f("roll") == 0.0 {
        return Some(img.clone());
    }
    let m = rot(ctx, "tilt", "pan", "roll");
    let (from, to) = ((from.0, from.1 as f32), (to.0, to.1 as f32));
    per_eye(e, img, layout(ctx), |e, img, _| Some(convert(e, img, from, to, &m, img.width, img.height, None)))
}

// ---------------------------------------------------------------- seam-aware blur

/// vr::sphere_blur: 3 latitude-widened, seam-wrapping horizontal box passes, then 3
/// pole-continuing vertical ones.
fn sphere_blur(e: &mut Enc, img: &GpuImage, sigma: f64) -> GpuImage {
    if sigma < 0.1 {
        return img.clone();
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let radii = effectcraft_effects::util::box_radii(sigma, 3);
    let mut cur = img.clone();
    for &r in &radii {
        let rows: Vec<f32> = (0..h)
            .map(|y| {
                let lat = (0.5 - (y as f64 + 0.5) / h as f64) * std::f64::consts::PI;
                let rr = ((r as f64) / lat.cos().max(1e-3)).round().min((w / 2) as f64) as usize;
                // box_row_wrap's own limit.
                let rr = if w == 0 { 0 } else { rr.min((w - 1) / 2) };
                rr as f32
            })
            .collect();
        if rows.iter().all(|r| *r == 0.0) {
            continue;
        }
        let buf = e.data(&rows);
        let mut p = Params::default();
        p.u[0][0] = 256;
        let out = e.scratch(cur.width, cur.height);
        e.dispatch(
            "fxv_box_h",
            &p,
            crate::context::KernelImages { src: &cur, aux: None, out: &out },
            Some(&buf),
            (cur.height.div_ceil(64), cur.width.div_ceil(256)),
        );
        cur = out;
    }
    for &r in &radii {
        if r == 0 {
            continue;
        }
        let block = (4 * r).clamp(32, 256) as u32;
        let mut p = Params::default();
        p.u[0] = [block, r as u32, 0, 0];
        let out = e.scratch(cur.width, cur.height);
        e.dispatch(
            "fxv_box_v",
            &p,
            crate::context::KernelImages { src: &cur, aux: None, out: &out },
            None,
            (cur.width.div_ceil(64), cur.height.div_ceil(block)),
        );
        cur = out;
    }
    cur
}

fn op(code: u32) -> Params {
    let mut p = Params::default();
    p.u[0][0] = code;
    p
}

fn point(e: &mut Enc, p: &Params, src: &GpuImage, aux: Option<&GpuImage>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels("fxv_point", p, src, aux, &out, None);
    out
}

fn blur(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage, scale: f64) -> Option<GpuImage> {
    let s = ctx.params.f("blurriness").max(0.0) * scale * 0.5;
    if s < 0.1 {
        return Some(img.clone());
    }
    per_eye(e, img, layout(ctx), |e, img, _| Some(sphere_blur(e, img, s)))
}

fn sharpen(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage, scale: f64) -> Option<GpuImage> {
    let amt = ctx.params.f("sharpenAmount") as f32 / 100.0;
    if amt == 0.0 {
        return Some(img.clone());
    }
    let s = (1.5 * scale).max(0.5);
    per_eye(e, img, layout(ctx), |e, img, _| {
        let bl = sphere_blur(e, img, s);
        let mut p = op(2);
        p.f[0][0] = amt;
        Some(point(e, &p, img, Some(&bl)))
    })
}

fn glow(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage, scale: f64) -> Option<GpuImage> {
    let pr = ctx.params;
    let bright = pr.f("glowBrightness") as f32 / 100.0;
    if bright <= 0.0 {
        return Some(img.clone());
    }
    let radius = pr.f("glowRadius").max(0.0) * scale;
    let tc = pr.color("tintColor");
    let mut hp = op(0);
    hp.u[0][1] = pr.b("useTintColor") as u32;
    hp.f[0] = [pr.f("luminanceThreshold") as f32 / 100.0, pr.f("glowSaturation") as f32 / 100.0, 0.0, 0.0];
    hp.f[1] = [tc[0], tc[1], tc[2], 0.0];
    let mut add = op(1);
    add.f[0][0] = bright;
    per_eye(e, img, layout(ctx), |e, img, _| {
        let hi = point(e, &hp, img, None);
        let g = sphere_blur(e, &hi, (radius * 0.5).max(0.5));
        Some(point(e, &add, img, Some(&g)))
    })
}

fn denoise(e: &mut Enc, ctx: &EffectCtx, b: &GBuf) -> Option<GpuImage> {
    let pr = ctx.params;
    let level = pr.f("noiseLevel").max(0.0) / 100.0;
    if level <= 0.0 {
        return Some(b.img.clone());
    }
    let r = ((1.0 + level * 4.0) * b.scale).max(1.0) as usize;
    if pr.e("noiseType") == 1 {
        let mr = ((level * 3.0 * b.scale).round() as usize).max(1);
        return per_eye(e, &b.img, layout(ctx), |e, img, _| Some(crate::fx_noise::median_image(e, img, mr, false)));
    }
    let eps = (level * level * 0.02) as f32;
    let detail = (pr.f("detail") / 100.0).clamp(0.0, 1.0) as f32;
    per_eye(e, &b.img, layout(ctx), |e, img, _| {
        let pad = (r * 2 + 1) as u32;
        let (pw, ph) = (img.width + 2 * pad, img.height + 2 * pad);
        if !e.g.fits(pw, ph) {
            return None;
        }
        let padded = e.scratch(pw, ph);
        let mut p = Params::default();
        p.u[0][0] = pad;
        e.pixels("fxv_wrap_pad", &p, img, None, &padded, None);
        let guide = point(e, &op(4), &padded, None);
        let f = crate::fx_noise::guided_filter(e, &guide, &padded, r, eps.max(1e-6));
        let crop = e.scratch(img.width, img.height);
        let mut p = Params::default();
        p.u[0] = [pad, pad, 0, 0];
        e.pixels("fxn_crop", &p, &f, None, &crop, None);
        let mut p = op(3);
        p.f[0][0] = detail;
        Some(point(e, &p, &crop, Some(img)))
    })
}

// ---------------------------------------------------------------- generators

/// BLEND_OPTS → (none, blend mode id).
fn blend(i: u32) -> (u32, u32) {
    use effectcraft_color::BlendMode::*;
    let m = match i {
        0 => return (1, 0),
        2 => Add,
        3 => Multiply,
        4 => Screen,
        5 => Overlay,
        _ => Normal,
    };
    (0, crate::ops::mode_id(m))
}

fn chromatic(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    let k = [pr.f("aberrationRed"), pr.f("aberrationGreen"), pr.f("aberrationBlue")].map(|v| (v / 100.0 * 0.1) as f32);
    if k.iter().all(|v| *v == 0.0) {
        return Some(img.clone());
    }
    let r = effectcraft_effects::vr_rotation(pr.f("centerTilt"), pr.f("centerPan"), 0.0);
    let c = [r[0][2], r[1][2], r[2][2]].map(|v| v as f32);
    let mut p = op(0);
    p.u[0][1] = pr.b("falloffInvert") as u32;
    p.f[0] = [k[0], k[1], k[2], (pr.f("falloff") / 100.0).clamp(0.0, 1.0) as f32];
    p.f[1] = [c[0], c[1], c[2], 0.0];
    per_eye(e, img, layout(ctx), |e, img, _| Some(gen_pass(e, &p, img, None)))
}

fn gen_pass(e: &mut Enc, p: &Params, img: &GpuImage, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(img.width, img.height);
    e.pixels("fxv_gen", p, img, None, &out, data);
    out
}

fn gradients(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    let (lw, lh) = (ctx.layer_size[0].max(1.0), ctx.layer_size[1].max(1.0));
    let mut d = vec![];
    for i in 1..=5 {
        if !pr.b(&format!("enablePoint{i}")) {
            continue;
        }
        let pt = pr.v2(&format!("point{i}"));
        let dir = effectcraft_effects::vr_equi_dir(pt[0], pt[1], lw, lh);
        let c = pr.color(&format!("color{i}"));
        d.extend([dir[0] as f32, dir[1] as f32, dir[2] as f32, c[0], c[1], c[2]]);
    }
    if d.is_empty() {
        return Some(img.clone());
    }
    let (none, mode) = blend(pr.e("blendingMode"));
    let mut p = op(1);
    p.u[0] = [1, (d.len() / 6) as u32, none, mode];
    p.f[0] = [pr.f("blend").max(0.1) as f32, (pr.f("opacity") / 100.0) as f32, 0.0, 0.0];
    let buf = e.data(&d);
    per_eye(e, img, layout(ctx), |e, img, _| Some(gen_pass(e, &p, img, Some(&buf))))
}

fn fractal(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    let g = |id: &str, d: f64| pr.get(id).map(|v| v.as_f64()).unwrap_or(d);
    let octaves = pr.f("complexity").clamp(1.0, 20.0);
    let scale = (pr.f("transform/scale") / 100.0).max(0.01) as f32;
    let (none, mode) = blend(pr.e("blendingMode"));
    let mut p = op(2);
    p.u[0] = [2, pr.e("fractalType"), none, mode];
    p.u[1] = [octaves.ceil() as u32, pr.f("randomSeed") as u32, pr.b("invert") as u32, 0];
    p.f[0] = [
        pr.f("contrast") as f32 / 100.0,
        pr.f("brightness") as f32 / 100.0,
        (g("subSettings/subInfluence", 50.0) / 100.0).clamp(0.0, 1.0) as f32,
        (g("subSettings/subScaling", 50.0) / 100.0).clamp(0.1, 1.0) as f32,
    ];
    p.f[1] = [4.0 / scale, (pr.f("evolution") / 360.0) as f32, (pr.f("opacity") / 100.0) as f32, octaves.fract() as f32];
    put_matrix(&mut p, 2, &rot(ctx, "transform/tilt", "transform/pan", "transform/roll"));
    per_eye(e, img, layout(ctx), |e, img, _| Some(gen_pass(e, &p, img, None)))
}

fn glitch(e: &mut Enc, ctx: &EffectCtx, img: &GpuImage) -> Option<GpuImage> {
    let pr = ctx.params;
    let amp = (pr.f("masterAmplitude") / 100.0).max(0.0);
    if amp <= 0.0 {
        return Some(img.clone());
    }
    let g = |id: &str, d: f64| pr.get(id).map(|v| v.as_f64()).unwrap_or(d);
    let rate = pr.f("distortionRate").max(0.0);
    let geo = pr.f("geometricDistortion") / 100.0 * amp;
    let (hdisp, vdisp) = (g("geometric/horizontalDisplacement", 100.0) / 100.0, g("geometric/verticalDisplacement", 0.0) / 100.0);
    let complexity = pr.f("distortionComplexity").clamp(1.0, 100.0);
    let rgb = pr.f("colorDistortion") / 100.0 * amp;
    let offs = [g("color/redOffset", -1.0), g("color/greenOffset", 0.0), g("color/blueOffset", 1.0)];
    let scan = (pr.f("scanlines") / 100.0 * amp) as f32;
    let spacing = g("scanlineSpacing", 2.0).round().max(2.0) as usize;
    let seed = (pr.f("randomSeed") as u32) ^ ctx.seed;
    let mut evo = g("distortionEvolution", 0.0) / 360.0;
    if pr.b("evolutionOptions/cycleEvolution") {
        evo = evo.rem_euclid(g("evolutionOptions/cycle", 1.0).round().max(1.0));
    }
    let base = if rate > 0.0 { ctx.time * rate } else { 0.0 } + evo;
    let slot = base.floor() as i64 as u32;
    let frac = base - base.floor();
    let tr = effectcraft_effects::vr_rotation(g("target/tilt", 0.0), g("target/pan", 0.0), 0.0);
    let target = [tr[0][2], tr[1][2], tr[2][2]].map(|v| v as f32);
    let radius = g("target/radius", 0.0).to_radians() as f32;
    let feather = g("target/feather", 0.0).to_radians() as f32;
    per_eye(e, img, layout(ctx), |e, img, eye| {
        let (w, h) = (img.width as f64, img.height as f64);
        let bands = complexity.round() as u32;
        // Per row: horizontal / vertical shift, channel split and scan-line darkening.
        let mut d = Vec::with_capacity(img.height as usize * 4);
        for y in 0..img.height as usize {
            let band = ((y as f64 / h) * bands as f64) as u32;
            let at = |s: u32| {
                let r0 = hash_noise(band, s, seed ^ (eye as u32 * 977));
                let on = r0 < 0.35;
                let shift = if on { (hash_noise(band, s + 1, seed) as f64 * 2.0 - 1.0) * geo } else { 0.0 };
                let split = if on { rgb * w * 0.01 * (1.0 + hash_noise(band, s + 2, seed) as f64) } else { 0.0 };
                (shift, split)
            };
            let (s0, p0) = at(slot);
            let (shift, split) = if frac > 0.0 {
                let (s1, p1) = at(slot + 1);
                (s0 + (s1 - s0) * frac, p0 + (p1 - p0) * frac)
            } else {
                (s0, p0)
            };
            let dark = if y % spacing == 0 { 1.0 - scan * 0.5 } else { 1.0 };
            d.extend([(shift * w * 0.15 * hdisp) as f32, (shift * h * 0.15 * vdisp) as f32, split as f32, dark]);
        }
        let buf = e.data(&d);
        let mut p = op(3);
        p.u[0][0] = 3;
        p.f[0] = [offs[0] as f32, offs[1] as f32, offs[2] as f32, 0.0];
        p.f[1] = [target[0], target[1], target[2], 0.0];
        p.f[2] = [radius, feather, 0.0, 0.0];
        Some(gen_pass(e, &p, img, Some(&buf)))
    })
}
