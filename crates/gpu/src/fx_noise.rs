//! GPU effects, noise, blur and time family (kernels in `shaders/fx_noise.wgsl`, every entry point prefixed
//! `fxn_`): the CPU effects' exact steps with the pixel loops as compute kernels.
//!
//! Median, Median (Legacy) and Dust & Scratches take the median of the 512-level quantised
//! channels: by bisection over the window up to radius [`MEDIAN_BISECT_R`], above that with the
//! CPU's sliding 512-bin histogram per channel (one invocation per row segment). Fractal and
//! Turbulent Noise resolve every per-octave term on the CPU in the CPU's f32 order
//! (`effectcraft_effects::fractal_gpu`) and evaluate the pixels on the GPU. Echo and Posterize
//! Time fetch the layer's other frames through the effect host like the CPU does, then
//! accumulate on the GPU. Remove Grain measures the grain level in its sample boxes on the CPU
//! (one readback) and runs the guided filters, Texture, Unsharp Mask and Preview on the GPU;
//! Temporal Filtering and the Noise Samples / Blending Matte views render on the CPU.

use effectcraft_color::BlendMode;
use effectcraft_effects::{Buf, EffectCtx};
use effectcraft_keyframe::Value;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::{GBuf, box_passes, gaussian_blur};

/// Compute entry points in `fx_noise.wgsl`.
pub(crate) const KERNELS: &[&str] = &[
    "fxn_quant",
    "fxn_median",
    "fxn_median_huang",
    "fxn_median_out",
    "fxn_edge",
    "fxn_smart_edges",
    "fxn_unsharp",
    "fxn_pick",
    "fxn_cblur_out",
    "fxn_straight",
    "fxn_premul",
    "fxn_morph",
    "fxn_crop",
    "fxn_cb_map",
    "fxn_cb_level",
    "fxn_echo",
    "fxn_echo_fin",
    "fxn_fractal",
    "fxn_noise_alpha",
    "fxn_noise_hls",
    "fxn_gf_guide",
    "fxn_gf",
    "fxn_rg_texture",
    "fxn_rg_unsharp",
    "fxn_rg_out",
];

/// Effect ids implemented here (Fractal Noise's other modes come through [`fractal_extra`]).
pub(crate) const IDS: &[&str] = &[
    "ec.noise.turbulent",
    "ec.noise.median",
    "ec.noise.medianlegacy",
    "ec.noise.dustscratches",
    "ec.noise.removegrain",
    "ec.noise.noisealpha",
    "ec.noise.noisehls",
    "ec.noise.noisehlsauto",
    "ec.blur.smart",
    "ec.blur.bilateral",
    "ec.blur.sharpen",
    "ec.blur.unsharp",
    "ec.blur.compound",
    "ec.blur.channel",
    "ec.channel.minimax",
    "ec.time.echo",
    "ec.time.posterizetime",
];

/// Largest median radius (buffer pixels) taken by bisection over the window; larger radii
/// slide a histogram along each row (`fxn_median_huang`), whose cost grows with the radius,
/// not its square.
pub(crate) const MEDIAN_BISECT_R: usize = 4;

/// Physical GPUs retain the original parallel private histogram. Software Vulkan
/// uses bounded workgroup storage to avoid its expensive first-use private-array
/// lowering. This trades some warmed software throughput for bounded first use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MedianKernel {
    Private64,
    Software2,
}

impl MedianKernel {
    pub(crate) fn for_adapter(backend: wgpu::Backend, device_type: wgpu::DeviceType) -> Self {
        if backend == wgpu::Backend::Vulkan && device_type == wgpu::DeviceType::Cpu { Self::Software2 } else { Self::Private64 }
    }

    pub(crate) fn rows_per_workgroup(self) -> u32 {
        match self {
            Self::Private64 => 64,
            Self::Software2 => 2,
        }
    }

    pub(crate) fn workgroup_storage(self) -> u32 {
        match self {
            Self::Private64 => 0,
            Self::Software2 => 16_384,
        }
    }

    pub(crate) fn source(self) -> &'static str {
        match self {
            Self::Private64 => include_str!("shaders/median_private.wgsl"),
            Self::Software2 => include_str!("shaders/median_software.wgsl"),
        }
    }

    /// Separate test processes can qualify both implementations against existing oracles.
    #[cfg(test)]
    pub(crate) fn test_override(self) -> Result<Self, String> {
        match std::env::var("EFFECTCRAFT_TEST_MEDIAN_KERNEL") {
            Err(std::env::VarError::NotPresent) => Ok(self),
            Ok(value) if value == "private64" => Ok(Self::Private64),
            Ok(value) if value == "software2" => Ok(Self::Software2),
            value => Err(format!("invalid EFFECTCRAFT_TEST_MEDIAN_KERNEL {value:?}; expected private64 or software2")),
        }
    }
}

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.noise.turbulent" => fractal(e, ctx, b, true),
        "ec.noise.median" | "ec.noise.medianlegacy" | "ec.noise.dustscratches" => median(e, id, ctx, b),
        "ec.noise.removegrain" => remove_grain(e, ctx, b),
        "ec.noise.noisealpha" => noise_alpha(e, ctx, b),
        "ec.noise.noisehls" => noise_hls(e, ctx, b, ctx.params.f("noisePhase") as f32 / 360.0),
        "ec.noise.noisehlsauto" => noise_hls(e, ctx, b, ctx.time as f32 * ctx.params.f("noiseAnimationSpeed") as f32),
        "ec.blur.smart" => smart_blur(e, ctx, b),
        "ec.blur.bilateral" => bilateral(e, ctx, b),
        "ec.blur.sharpen" => {
            let s = b.scale;
            unsharp(e, b, ctx.params.f("amount") as f32 / 50.0, s, 0.0)
        }
        "ec.blur.unsharp" => {
            let s = (ctx.params.f("radius") * b.scale).max(0.1);
            unsharp(e, b, ctx.params.f("amount") as f32 / 100.0, s, ctx.params.f("threshold") as f32)
        }
        "ec.blur.compound" => compound_blur(e, ctx, b),
        "ec.blur.channel" => channel_blur(e, ctx, b),
        "ec.channel.minimax" => minimax(e, ctx, b),
        "ec.time.echo" => echo(e, ctx, b),
        "ec.time.posterizetime" => posterize_time(e, ctx, b),
        _ => None,
    }
}

/// A same-size per-pixel kernel.
fn run(e: &mut Enc, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels(entry, p, src, aux, &out, data);
    out
}

fn with_u0(u: [u32; 4]) -> Params {
    let mut p = Params::default();
    p.u[0] = u;
    p
}

// ---------------------------------------------------------------- Fractal / Turbulent Noise (noise3.rs)

/// Fractal Noise settings the `pointwise` kernel (effects.rs) leaves to the CPU, which this
/// family's kernel covers: every Noise and Fractal Type, Cycle Evolution, Perspective Offset,
/// Sub Rotation / Offset and all Blending Modes.
pub(crate) fn fractal_extra(ctx: &EffectCtx) -> bool {
    let pr = ctx.params;
    pr.e("noiseType") != 2
        || pr.e("fractalType") > 2
        || pr.e("blendingMode") > 1
        || pr.b("evolutionOptions/cycleEvolution")
        || pr.b("transform/perspectiveOffset")
        || pr.f("subSettings/subRotation") != 0.0
        || (!pr.b("subSettings/centerSubscale") && pr.v2("subSettings/subOffset") != [0.0, 0.0])
}

/// noise3::render.
pub(crate) fn fractal(e: &mut Enc, ctx: &EffectCtx, b: GBuf, turbulent: bool) -> Option<GBuf> {
    let fr = effectcraft_effects::fractal_gpu(ctx, b.offset, b.scale, turbulent);
    let mode = match fr.mode {
        None => 255,
        Some(m) => BlendMode::ALL.iter().position(|&x| x == m).unwrap_or(0) as u32,
    };
    let data: Vec<f32> = fr.octaves.iter().flatten().copied().collect();
    let cycle = fr.cycle.unwrap_or([0.0; 3]);
    let mut p = Params::default();
    p.u[0] = [fr.kind, fr.noise_type, fr.octaves.len() as u32, fr.seed];
    p.u[1] = [fr.sub_rotation as u32, fr.cycle.is_some() as u32, fr.invert as u32, fr.overflow];
    p.u[2][0] = mode;
    p.f[0] = [fr.rot.0, fr.rot.1, fr.size[0], fr.size[1]];
    p.f[1] = [fr.contrast, fr.brightness, fr.opacity, fr.blend];
    p.f[2] = [cycle[0], cycle[1], cycle[2], fr.norm];
    let buf = e.data(&data);
    let img = run(e, "fxn_fractal", &p, &b.img, None, Some(&buf));
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- Median family (noise.rs, noise2.rs)

/// noise::median_image (of the straight colour with alpha 1 when `straight`).
pub(crate) fn median_image(e: &mut Enc, img: &GpuImage, r: usize, straight: bool) -> GpuImage {
    let q = run(e, "fxn_quant", &with_u0([straight as u32, 0, 0, 0]), img, None, None);
    // Preserve the CPU's rounded level / 511 values before the strict Dust & Scratches
    // threshold. GPU division (even residual-corrected) may differ by one ulp.
    let levels = e.data(&(0..512).map(|level| level as f32 / 511.0).collect::<Vec<_>>());
    if r <= MEDIAN_BISECT_R {
        return run(e, "fxn_median", &with_u0([r as u32, 0, 0, 0]), &q, None, Some(&levels));
    }
    // Segments long enough that filling the first window (2r + 1)² costs about as much as
    // sliding along the rest.
    let seg = (4 * r as u32).clamp(32, 512).min(q.width);
    let out = e.scratch(q.width, q.height);
    let groups = (q.height.div_ceil(e.g.median_kernel.rows_per_workgroup()), q.width.div_ceil(seg));
    e.dispatch("fxn_median_huang", &with_u0([r as u32, seg, 0, 0]), crate::context::KernelImages { src: &q, aux: None, out: &out }, Some(&levels), groups);
    out
}

fn median(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return Some(b);
    }
    let (mode, on_alpha, straight) = match id {
        "ec.noise.medianlegacy" => {
            let on = ctx.params.b("operateOnAlphaChannel");
            (if on { 2 } else { 3 }, on, !on)
        }
        "ec.noise.dustscratches" => (1, ctx.params.b("operateOnAlpha"), false),
        _ => (0, ctx.params.b("operateOnAlpha"), false),
    };
    let m = median_image(e, &b.img, r, straight);
    let mut p = with_u0([mode, on_alpha as u32, 0, 0]);
    if mode == 1 {
        p.f[0][0] = ctx.params.f("threshold") as f32 / 255.0;
    }
    let img = run(e, "fxn_median_out", &p, &b.img, Some(&m), None);
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- Remove Grain (noise.rs)

fn remove_grain(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let pr = ctx.params;
    let amt = pr.f("noiseReductionSettings/noiseReduction") as f32;
    let um_amount = pr.f("unsharpMask/amount") as f32 / 100.0;
    let view = pr.get("viewingMode").map(Value::as_enum).unwrap_or(3);
    // Noise Samples / Blending Matte views and Temporal Filtering render on the CPU.
    if view == 1 || view == 2 || pr.b("temporalFiltering/enabled") {
        return None;
    }
    if amt <= 0.0 && um_amount <= 0.0 {
        return Some(b);
    }
    // The grain level measured in the sample boxes (CPU; only the smoothing uses it).
    let level = if amt > 0.0 {
        let img = e.download(&b.img)?;
        effectcraft_effects::remove_grain_level(ctx, &Buf { img, offset: b.offset, scale: b.scale })
    } else {
        1.0
    };
    let passes = pr.f("noiseReductionSettings/passes").round().clamp(1.0, 8.0) as usize;
    let multichannel = pr.e("noiseReductionSettings/mode") == 0;
    let texture = (pr.f("fineTuning/texture") as f32).clamp(0.0, 1.0);
    let r = (2.0 * b.scale).round().max(1.0) as usize;
    let eps = (0.02 * amt * level).powi(2);
    let orig = b.img.clone();
    let mut ch = orig.clone();
    if amt > 0.0 {
        for _ in 0..passes {
            let g = run(e, "fxn_gf_guide", &with_u0([!multichannel as u32, 0, 0, 0]), &ch, None, None);
            ch = guided_filter(e, &g, &ch, r, eps);
        }
        if texture > 0.0 {
            let mut p = Params::default();
            p.f[0][0] = texture;
            ch = run(e, "fxn_rg_texture", &p, &ch, Some(&orig), None);
        }
    }
    if um_amount > 0.0 {
        let radius = (pr.f("unsharpMask/radius") * b.scale).max(0.1);
        let low = gaussian_blur(e, &ch, radius, radius, true);
        let mut p = Params::default();
        p.f[0] = [um_amount, pr.f("unsharpMask/threshold") as f32 / 255.0, 0.0, 0.0];
        ch = run(e, "fxn_rg_unsharp", &p, &ch, Some(&low), None);
    }
    let mut p = Params::default();
    if view == 0 {
        let c = b.to_px(pr.v2("previewRegion/center"));
        let (hw, hh) = (pr.f("previewRegion/width") * b.scale * 0.5, pr.f("previewRegion/height") * b.scale * 0.5);
        let col = pr.color("previewRegion/boxColor");
        p.u[0] = [1, pr.b("previewRegion/showBox") as u32, 0, 0];
        p.f[0] = [(c.0 - hw) as f32, (c.0 + hw) as f32, (c.1 - hh) as f32, (c.1 + hh) as f32];
        p.f[1] = [col[0], col[1], col[2], 1.0];
    }
    let img = run(e, "fxn_rg_out", &p, &ch, Some(&orig), None);
    Some(GBuf { img, ..b })
}

/// One guided-filter arithmetic pass (`fxn_gf`), `d` = an image read through `data`.
fn gf(e: &mut Enc, mode: u32, s: &GpuImage, a: &GpuImage, d: Option<&GpuImage>, eps: f32) -> GpuImage {
    let mut p = with_u0([mode, 0, 0, 0]);
    p.f[0][0] = eps;
    let rows = d.map(|d| e.image_rows(d));
    if let Some((_, row)) = &rows {
        p.u[0][1] = *row;
    }
    run(e, "fxn_gf", &p, s, Some(a), rows.as_ref().map(|r| &r.0))
}

/// util::guided_filter on every channel at once (guide `g`, input `p`).
pub(crate) fn guided_filter(e: &mut Enc, g: &GpuImage, p: &GpuImage, r: usize, eps: f32) -> GpuImage {
    let bx = |e: &mut Enc, img: &GpuImage| box_passes(e, img, &[r], &[r], true);
    let mean_g = bx(e, g);
    let mean_p = bx(e, p);
    let gp = gf(e, 5, g, p, None, 0.0);
    let gg = gf(e, 6, g, g, None, 0.0);
    let corr_gp = bx(e, &gp);
    let corr_gg = bx(e, &gg);
    let cov = gf(e, 0, &corr_gp, &mean_p, Some(&mean_g), 0.0);
    let var = gf(e, 1, &corr_gg, &mean_g, None, 0.0);
    let a = gf(e, 2, &cov, &var, None, eps);
    let bb = gf(e, 3, &a, &mean_p, Some(&mean_g), 0.0);
    let ma = bx(e, &a);
    let mb = bx(e, &bb);
    gf(e, 4, &ma, &mb, Some(g), 0.0)
}

// ---------------------------------------------------------------- Noise Alpha / Noise HLS (noise.rs)

/// noise::phased_cycle's two seeds and smoothed fraction for `phase`.
fn phase_terms(seed: u32, phase: f32, cycle: i64) -> (u32, u32, f32) {
    let k = phase.floor();
    let t = phase - k;
    let k = k as i64;
    let wrap = |k: i64| if cycle > 0 { k.rem_euclid(cycle) } else { k } as u32;
    (seed.wrapping_add(wrap(k).wrapping_mul(0x632b)), seed.wrapping_add(wrap(k + 1).wrapping_mul(0x632b)), t * t * (3.0 - 2.0 * t))
}

fn noise_alpha(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let k = ctx.params.f("amount") as f32 / 100.0;
    if k <= 0.0 {
        return Some(b);
    }
    let kind = ctx.params.e("noise");
    let seed = (ctx.params.f("randomSeed") as i64 as u32) ^ ctx.seed.wrapping_mul(0x2f1);
    let phase = ctx.params.f("noisePhase") as f32 / 360.0 + if kind >= 2 { ctx.time as f32 } else { 0.0 };
    let cycle = if ctx.params.b("noiseOptions/cycleNoise") { ctx.params.f("noiseOptions/cycle").round().max(1.0) as i64 } else { 0 };
    let ph = if kind >= 2 { phase } else { phase.floor() };
    let (sa, sb, t) = phase_terms(seed, ph, cycle);
    let mut p = with_u0([kind % 2, ctx.params.e("originalAlpha"), ctx.params.e("overflow"), sa]);
    p.u[1][0] = sb;
    p.f[0] = [k, t, 0.0, 0.0];
    let img = run(e, "fxn_noise_alpha", &p, &b.img, None, None);
    Some(GBuf { img, ..b })
}

fn noise_hls(e: &mut Enc, ctx: &EffectCtx, b: GBuf, phase: f32) -> Option<GBuf> {
    let hue = ctx.params.f("hue") as f32 / 100.0;
    let light = ctx.params.f("lightness") as f32 / 100.0;
    let satv = ctx.params.f("saturation") as f32 / 100.0;
    if hue == 0.0 && light == 0.0 && satv == 0.0 {
        return Some(b);
    }
    let gs = (ctx.params.f("grainSize") * b.scale).max(0.1) as f32;
    let seed = ctx.seed.wrapping_mul(0x51f3);
    let mut seeds = [0u32; 6];
    let mut t = 0.0;
    for k in 0..3u32 {
        let (sa, sb, tt) = phase_terms(seed.wrapping_add(k * 7919), phase, 0);
        seeds[2 * k as usize] = sa;
        seeds[2 * k as usize + 1] = sb;
        t = tt;
    }
    let mut p = with_u0([ctx.params.e("noise"), seed, 0, 0]);
    p.u[1] = [seeds[0], seeds[1], seeds[2], seeds[3]];
    p.u[2] = [seeds[4], seeds[5], 0, 0];
    p.f[0] = [hue, light, satv, gs];
    p.f[1] = [phase, t, 0.0, 0.0];
    let img = run(e, "fxn_noise_hls", &p, &b.img, None, None);
    Some(GBuf { img, ..b })
}

// ---------------------------------------------------------------- blurs (blur2.rs, misc.rs)

/// blur2::edge_preserving (+ Bilateral's grey output with `grey`).
fn edge_preserving(e: &mut Enc, img: &GpuImage, r: f64, range: f32, hard: bool, grey: bool) -> GpuImage {
    let ri = r.ceil() as u32;
    let stride = ((r / 6.0).ceil() as u32).max(1);
    let sig_s2 = 2.0 * (r / 2.0).max(0.5).powi(2);
    let sig_r2 = 2.0 * range.max(1e-3).powi(2);
    let d2max = (r * r + 0.5).floor() as u32;
    let mut p = with_u0([ri, stride, hard as u32, d2max]);
    p.u[1][0] = grey as u32;
    p.f[0] = [sig_s2 as f32, sig_r2, range, 0.0];
    run(e, "fxn_edge", &p, img, None, None)
}

fn bilateral(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let r = ctx.params.f("radius") * b.scale;
    if r < 0.5 {
        return Some(b);
    }
    let range = ctx.params.f("threshold") as f32 / 100.0;
    let img = edge_preserving(e, &b.img, r, range, false, !ctx.params.b("colorize"));
    Some(GBuf { img, ..b })
}

fn smart_blur(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let r = ctx.params.f("radius") * b.scale;
    let thr = ctx.params.f("threshold") as f32 / 100.0;
    let mode = ctx.params.e("mode");
    let blurred = if r >= 0.5 { edge_preserving(e, &b.img, r, thr, true, false) } else { b.img.clone() };
    if mode == 0 {
        return Some(GBuf { img: blurred, ..b });
    }
    let mut p = with_u0([mode, 0, 0, 0]);
    p.f[0][0] = thr.max(0.01) * 0.5;
    let img = run(e, "fxn_smart_edges", &p, &blurred, None, None);
    Some(GBuf { img, ..b })
}

/// misc::unsharp_core (Sharpen, Unsharp Mask).
fn unsharp(e: &mut Enc, b: GBuf, amount: f32, sigma: f64, threshold: f32) -> Option<GBuf> {
    let blurred = gaussian_blur(e, &b.img, sigma, sigma, true);
    let mut p = Params::default();
    p.f[0] = [amount, threshold, 0.0, 0.0];
    let img = run(e, "fxn_unsharp", &p, &b.img, Some(&blurred), None);
    Some(GBuf { img, ..b })
}

fn dims_xy(dim: u32) -> (f64, f64) {
    match dim {
        1 => (1.0, 0.0),
        2 => (0.0, 1.0),
        _ => (1.0, 1.0),
    }
}

fn pick(e: &mut Enc, acc: &GpuImage, from: &GpuImage, mask: [bool; 4], alpha: bool) -> GpuImage {
    let mut p = with_u0([alpha as u32, 0, 0, 0]);
    p.u[1] = mask.map(|m| m as u32);
    run(e, "fxn_pick", &p, acc, Some(from), None)
}

fn channel_blur(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let sig = ["redBlurriness", "greenBlurriness", "blueBlurriness", "alphaBlurriness"].map(|id| ctx.params.f(id).max(0.0) * 0.5 * b.scale);
    if sig.iter().all(|&s| s < 0.05) {
        return Some(b);
    }
    let (kx, ky) = dims_xy(ctx.params.e("dimensions"));
    let repeat = ctx.params.b("repeatEdge") || ctx.adjustment;
    let maxs = sig.iter().cloned().fold(0.0, f64::max);
    if !repeat {
        b.pad(e, (maxs * 3.0).ceil() as u32 + 1)?;
    }
    // n: each colour channel blurred by its own amount; a: the alpha blurred by the same amount
    // (its divisor), alpha = the Alpha Blurriness result.
    let mut n = b.img.clone();
    let mut a = pick(e, &b.img, &b.img, [true; 4], true);
    let mut done: Vec<u64> = Vec::new();
    for &s in &sig {
        if s < 0.05 || done.contains(&s.to_bits()) {
            continue;
        }
        done.push(s.to_bits());
        let bl = gaussian_blur(e, &b.img, s * kx, s * ky, true);
        let on = |c: usize| sig[c] >= 0.05 && sig[c].to_bits() == s.to_bits();
        n = pick(e, &n, &bl, [on(0), on(1), on(2), false], false);
        a = pick(e, &a, &bl, [on(0), on(1), on(2), on(3)], true);
    }
    b.img = run(e, "fxn_cblur_out", &Params::default(), &n, Some(&a), None);
    Some(b)
}

fn compound_blur(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let max_sigma = ctx.params.f("maximumBlur").max(0.0) * 0.5 * b.scale;
    if max_sigma < 0.05 {
        return Some(b);
    }
    let invert = ctx.params.b("invertBlur");
    if !ctx.adjustment {
        b.pad(e, (max_sigma * 3.0).ceil() as u32 + 1)?;
    }
    let (w, h) = (b.img.width as usize, b.img.height as usize);
    // The blur map: the Blur Layer fitted to the buffer (util::fit_layer), else the layer itself.
    let map = match ctx.layer_param("blurLayer", true) {
        Some(o) => {
            if o.buf.img.is_empty() {
                return None;
            }
            let stretch = ctx.params.b("stretchMapToFit");
            let (ls, os) = (ctx.layer_size, o.size);
            let (sx, sy) = if stretch && ls[0] > 0.0 && ls[1] > 0.0 { (os[0] / ls[0], os[1] / ls[1]) } else { (1.0, 1.0) };
            let (dx, dy) = if stretch { (0.0, 0.0) } else { ((os[0] - ls[0]) * 0.5, (os[1] - ls[1]) * 0.5) };
            let inv = 1.0 / b.scale.max(1e-9);
            let col = |x: usize| (((x as f64 + 0.5 - b.offset[0]) * inv) * sx + dx) * o.buf.scale + o.buf.offset[0];
            let row = |y: usize| (((y as f64 + 0.5 - b.offset[1]) * inv) * sy + dy) * o.buf.scale + o.buf.offset[1];
            let data: Vec<f32> = (0..w).map(|x| col(x) as f32).chain((0..h).map(|y| row(y) as f32)).collect();
            let layer = e.g.upload_image(&o.buf.img)?;
            let buf = e.data(&data);
            let mut p = with_u0([1, invert as u32, 0, 0]);
            p.u[1][0] = w as u32;
            let out = e.scratch(b.img.width, b.img.height);
            e.pixels("fxn_cb_map", &p, &layer, None, &out, Some(&buf));
            out
        }
        None => run(e, "fxn_cb_map", &with_u0([0, invert as u32, 0, 0]), &b.img, None, None),
    };
    const N: usize = 5;
    let levels: Vec<GpuImage> = (0..N)
        .map(|k| {
            if k == 0 {
                b.img.clone()
            } else {
                let s = max_sigma * k as f64 / (N - 1) as f64;
                gaussian_blur(e, &b.img, s, s, ctx.adjustment)
            }
        })
        .collect();
    let (rows, row) = e.image_rows(&map);
    let out = e.scratch(b.img.width, b.img.height);
    for k in 0..N - 1 {
        e.pixels("fxn_cb_level", &with_u0([k as u32, row, 0, 0]), &levels[k], Some(&levels[k + 1]), &out, Some(&rows));
    }
    b.img = out;
    Some(b)
}

// ---------------------------------------------------------------- Minimax (channel.rs)

/// util::morph_plane per channel (`modes`: 0 min, 1 max, 2 keep) with radii (`rx`, `ry`).
fn morph(e: &mut Enc, img: &GpuImage, rx: usize, ry: usize, modes: [u32; 4]) -> GpuImage {
    let mut cur = img.clone();
    for (vertical, r) in [(0, rx), (1, ry)] {
        if r == 0 {
            continue;
        }
        let mut p = with_u0([r as u32, vertical, 0, 0]);
        p.u[1] = modes;
        cur = run(e, "fxn_morph", &p, &cur, None, None);
    }
    cur
}

fn minimax(e: &mut Enc, ctx: &EffectCtx, mut b: GBuf) -> Option<GBuf> {
    let r = (ctx.params.f("radius") * b.scale).round().max(0.0) as usize;
    if r == 0 {
        return Some(b);
    }
    let op = ctx.params.e("operation");
    let dont_shrink = ctx.params.b("dontShrinkEdges");
    let (rx, ry) = match ctx.params.e("direction") {
        1 => (r, 0),
        2 => (0, r),
        _ => (r, r),
    };
    let sel: [bool; 4] = match ctx.params.e("channel") {
        1 => [false, false, false, true],
        2 => [true, true, true, true],
        3 => [true, false, false, false],
        4 => [false, true, false, false],
        5 => [false, false, true, false],
        _ => [true, true, true, false],
    };
    if sel[3] && op != 0 && !ctx.adjustment {
        b.pad(e, rx.max(ry) as u32)?;
    }
    let (w, h) = (b.img.width, b.img.height);
    let mut planes = run(e, "fxn_straight", &Params::default(), &b.img, None, None);
    // Beyond the layer edge is empty (0) unless Don't Shrink Edges repeats the edge pixels.
    if !dont_shrink {
        let (pw, ph) = (w + 2 * rx as u32, h + 2 * ry as u32);
        if !e.g.fits(pw, ph) {
            return None;
        }
        let padded = e.image(pw, ph);
        e.copy_into(&planes, &padded, rx as u32, ry as u32);
        planes = padded;
    }
    let modes = |max: bool| sel.map(|s| if s { max as u32 } else { 2 });
    let stages: &[bool] = match op {
        0 => &[false],
        1 => &[true],
        2 => &[false, true],
        _ => &[true, false],
    };
    for &max in stages {
        planes = morph(e, &planes, rx, ry, modes(max));
    }
    if !dont_shrink {
        let out = e.scratch(w, h);
        e.pixels("fxn_crop", &with_u0([rx as u32, ry as u32, 0, 0]), &planes, None, &out, None);
        planes = out;
    }
    b.img = run(e, "fxn_premul", &Params::default(), &planes, None, None);
    Some(b)
}

// ---------------------------------------------------------------- Echo / Posterize Time (time_fx.rs)

fn echo(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let n = ctx.params.f("numberOfEchoes").round().clamp(0.0, 255.0) as usize;
    let dt = ctx.params.f("echoTime");
    let start = ctx.params.f("startingIntensity").clamp(0.0, 1.0) as f32;
    let decay = ctx.params.f("decay").clamp(0.0, 1.0) as f32;
    let op = ctx.params.e("echoOperator");
    let times: Vec<f64> = (0..=n).map(|i| ctx.time + i as f64 * dt).collect();
    // Without the effect host (or the frames) the layer passes through, as on the CPU.
    let Some((grid, imgs)) = effectcraft_effects::time_frames(ctx, &times) else { return Some(b) };
    let (w, h) = (grid.img.width, grid.img.height);
    let mut acc = [e.image(w, h), e.image(w, h)];
    for (i, img) in imgs.iter().enumerate() {
        let frame = e.g.upload_image(img)?;
        let mut p = with_u0([op, (i == 0) as u32, 0, 0]);
        p.f[0][0] = start * decay.powi(i as i32);
        let [a, o] = &acc;
        e.pixels("fxn_echo", &p, a, Some(&frame), o, None);
        acc.swap(0, 1);
    }
    let mut p = with_u0([op, 0, 0, 0]);
    p.f[0][0] = (n + 1) as f32;
    let img = run(e, "fxn_echo_fin", &p, &acc[0], None, None);
    Some(GBuf { img, offset: grid.offset, scale: grid.scale })
}

fn posterize_time(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    if ctx.env.host.is_none() {
        return Some(b);
    }
    let tt = effectcraft_effects::posterized_time(ctx.time, ctx.params.f("frameRate"));
    match effectcraft_effects::time_frames(ctx, &[tt]) {
        Some((grid, mut imgs)) => {
            let img = e.g.upload_image(&imgs.pop().unwrap_or_default())?;
            Some(GBuf { img, offset: grid.offset, scale: grid.scale })
        }
        None => Some(b),
    }
}
