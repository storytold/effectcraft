//! GPU effects, time (kernels in `shaders/fx_time.wgsl`, every entry point prefixed `ftm_`):
//! Time Difference, Time Displacement, CC Force Motion Blur, CC Wide Time, Pixel Motion Blur and
//! Timewarp. Like Echo (`fx_noise`), the other frames come from the effect host on the CPU
//! (`effectcraft_effects::time_frames`: fetched at the CPU effect's layer times and placed on
//! one grid) and are uploaded; the kernels combine them with the CPU effect's arithmetic in its
//! order (weighted sums one frame per pass, so the float rounding matches).
//!
//! Time Displacement's per-pixel time (the map's luminance, quantised to the time resolution)
//! is computed on the CPU, since it decides which frames to fetch; the GPU gathers each pixel
//! from its frame. Timewarp's motion vectors (block matching and smoothing,
//! `effectcraft_raster::flow`) are estimated on the CPU (`effectcraft_effects::timewarp_plan`);
//! the frame building (Whole Frames, Frame Mix, Pixel Motion's warp along the vectors with its
//! error fallback, the shutter samples' average) runs on the GPU. With a Matte Layer the plan
//! also carries the matte-masked frames (and the foreground's and background's own vectors);
//! the GPU builds both layers and shows them as Show asks (`ftm_layer`).

use effectcraft_effects::util::{fit_layer, unpremul};
use effectcraft_effects::{Buf, EffectCtx, TwStep};
use effectcraft_keyframe::Value;

use crate::context::{Enc, GpuImage, Params};
use crate::effects::GBuf;

/// Compute entry points in `fx_time.wgsl`.
pub(crate) const KERNELS: &[&str] = &["ftm_acc", "ftm_mix", "ftm_layer", "ftm_difference", "ftm_displace", "ftm_motion"];

/// Effect ids implemented here.
pub(crate) const IDS: &[&str] =
    &["ec.time.timedifference", "ec.time.timedisplacement", "ec.time.ccforcemotionblur", "ec.time.ccwidetime", "ec.time.pixelmotionblur", "ec.time.timewarp"];

/// Run effect `id` (one of [`IDS`]); `None` = this parameter combination runs on the CPU.
pub(crate) fn apply(e: &mut Enc, id: &str, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    match id {
        "ec.time.timedifference" => time_difference(e, ctx, b),
        "ec.time.timedisplacement" => time_displacement(e, ctx, b),
        "ec.time.ccforcemotionblur" => {
            let n = ctx.params.f("motionBlurLevels").round().clamp(2.0, 64.0) as usize;
            let angle = if ctx.params.b("overrideShutterAngle") { ctx.params.f("shutterAngle") } else { 180.0 };
            let span = angle.clamp(0.0, 3600.0) / 360.0 / ctx.fps();
            mean_of(e, ctx, b, &(0..n).map(|i| ctx.time + span * i as f64 / n as f64).collect::<Vec<_>>())
        }
        "ec.time.ccwidetime" => {
            let fwd = ctx.params.f("forwardSteps").round().clamp(0.0, 45.0) as i64;
            let back = ctx.params.f("backwardSteps").round().clamp(0.0, 45.0) as i64;
            let fd = 1.0 / ctx.fps();
            mean_of(e, ctx, b, &(-back..=fwd).map(|k| ctx.time + k as f64 * fd).collect::<Vec<_>>())
        }
        // Pixel Motion Blur synthesizes along measured motion vectors (#535); without a plan (no
        // host to read neighbouring frames) it averages the shutter's frames, as the CPU effect does.
        "ec.time.pixelmotionblur" => match effectcraft_effects::pmb_plan(ctx) {
            Some(plan) => render_plan(e, &plan),
            None => {
                let angle = if ctx.params.e("shutterControl") == 1 { ctx.params.f("shutterAngle") } else { 180.0 };
                let n = ctx.params.f("shutterSamples").round().clamp(1.0, 64.0) as usize;
                let span = angle.clamp(0.0, 720.0) / 360.0 / ctx.fps();
                let times: Vec<f64> = (0..n).map(|i| ctx.time - span * 0.5 + if n > 1 { span * i as f64 / (n - 1) as f64 } else { span * 0.5 }).collect();
                mean_of(e, ctx, b, &times)
            }
        },
        "ec.time.timewarp" => timewarp(e, ctx, b),
        _ => None,
    }
}

fn kernel(e: &mut Enc, entry: &str, p: &Params, src: &GpuImage, aux: Option<&GpuImage>, data: Option<&wgpu::Buffer>) -> GpuImage {
    let out = e.scratch(src.width, src.height);
    e.pixels(entry, p, src, aux, &out, data);
    out
}

/// time_fx::average: Σ frame × weight, one frame per pass in the CPU's order.
fn average(e: &mut Enc, frames: &[GpuImage], weights: &[f32]) -> Option<GpuImage> {
    let first = frames.first()?;
    let mut acc = e.zeros(first.width, first.height);
    for (f, &k) in frames.iter().zip(weights) {
        let mut p = Params::default();
        p.f[0][0] = k;
        acc = kernel(e, "ftm_acc", &p, &acc, Some(f), None);
    }
    Some(acc)
}

fn on_grid(img: GpuImage, grid: &Buf) -> GBuf {
    GBuf { img, offset: grid.offset, scale: grid.scale }
}

/// time_fx::mean_of: the plain average of the layer at `times`.
fn mean_of(e: &mut Enc, ctx: &EffectCtx, b: GBuf, times: &[f64]) -> Option<GBuf> {
    if times.is_empty() {
        return Some(b);
    }
    let Some((grid, imgs)) = effectcraft_effects::time_frames(ctx, times) else { return Some(b) };
    let frames = imgs.iter().map(|i| e.g.upload_image(i)).collect::<Option<Vec<_>>>()?;
    let k = 1.0 / times.len() as f32;
    let img = average(e, &frames, &vec![k; times.len()])?;
    Some(on_grid(img, &grid))
}

// ---------------------------------------------------------------- Time Difference

fn time_difference(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    let Some(host) = ctx.env.host else { return Some(b) };
    let pr = ctx.params;
    let off = pr.f("timeOffset");
    let target_id = pr.get("targetLayer").and_then(Value::as_layer);
    let (grid, cur, target) = match target_id.and_then(|id| host.layer_at(id, ctx.env.comp_time + off, false)) {
        Some(other) => {
            let Some((grid, mut imgs)) = effectcraft_effects::time_frames(ctx, &[ctx.time]) else { return Some(b) };
            let cur = imgs.pop().unwrap_or_default();
            let tgt = fit_layer(ctx, &Buf { img: cur.clone(), ..grid.clone() }, &other, true);
            (grid, cur, tgt)
        }
        None => {
            let Some((grid, mut imgs)) = effectcraft_effects::time_frames(ctx, &[ctx.time, ctx.time + off]) else { return Some(b) };
            let tgt = imgs.pop().unwrap_or_default();
            let cur = imgs.pop().unwrap_or_default();
            (grid, cur, tgt)
        }
    };
    let (cur, target) = (e.g.upload_image(&cur)?, e.g.upload_image(&target)?);
    let mut p = Params::default();
    p.u[0] = [pr.b("absoluteDifference") as u32, pr.e("alphaChannel"), 0, 0];
    p.f[0][0] = 1.0 + pr.f("contrast") as f32 / 25.0;
    let img = kernel(e, "ftm_difference", &p, &cur, Some(&target), None);
    Some(on_grid(img, &grid))
}

// ---------------------------------------------------------------- Time Displacement

fn time_displacement(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    if ctx.env.host.is_none() {
        return Some(b);
    }
    let pr = ctx.params;
    let max = pr.f("maxDisplacementTime");
    let res = pr.f("timeResolution").clamp(1.0, 999.0);
    let Some((grid, mut imgs)) = effectcraft_effects::time_frames(ctx, &[ctx.time]) else { return Some(b) };
    let grid = Buf { img: imgs.pop().unwrap_or_default(), ..grid };
    let map = match ctx.layer_param("displacementMapLayer", true) {
        Some(o) => fit_layer(ctx, &grid, &o, pr.b("stretchMap")),
        None => grid.img.clone(),
    };
    // The CPU effect's per-pixel time step (luminance → −max..+max, quantised).
    let span = 2.0 * max.abs() * res;
    let step = if span > 63.0 { 2.0 * max.abs() / 63.0 } else { 1.0 / res };
    let idx: Vec<i32> = map
        .data
        .iter()
        .map(|p| {
            let (c, a) = unpremul(*p);
            let l = if a > 0.0 { effectcraft_color::luminance(c[0], c[1], c[2]) } else { 0.5 };
            (((l as f64 - 0.5) * 2.0 * max) / step).round() as i32
        })
        .collect();
    let mut keys = idx.clone();
    keys.sort_unstable();
    keys.dedup();
    if keys == [0] {
        return Some(on_grid(e.g.upload_image(&grid.img)?, &grid));
    }
    let times: Vec<f64> = keys.iter().map(|k| ctx.time + *k as f64 * step).collect();
    let Some((fgrid, frames)) = effectcraft_effects::time_frames(ctx, &times) else { return Some(on_grid(e.g.upload_image(&grid.img)?, &grid)) };
    let dx = (fgrid.offset[0] - grid.offset[0]).round() as i32;
    let dy = (fgrid.offset[1] - grid.offset[1]).round() as i32;
    let which: Vec<u8> = idx.iter().flat_map(|k| (keys.binary_search(k).unwrap_or(0) as u32).to_le_bytes()).collect();
    let data = e.bytes(which);
    let (w, h) = (grid.img.width, grid.img.height);
    let mut acc = e.zeros(w, h);
    for (fi, f) in frames.iter().enumerate() {
        let frame = e.g.upload_image(f)?;
        let mut p = Params::default();
        p.u[0] = [fi as u32, w, dx as u32, dy as u32];
        let out = e.scratch(w, h);
        e.pixels("ftm_displace", &p, &acc, Some(&frame), &out, Some(&data));
        acc = out;
    }
    Some(on_grid(acc, &grid))
}

// ---------------------------------------------------------------- Timewarp

fn timewarp(e: &mut Enc, ctx: &EffectCtx, b: GBuf) -> Option<GBuf> {
    if ctx.env.host.is_none() {
        return Some(b);
    }
    let Some(plan) = effectcraft_effects::timewarp_plan(ctx) else { return Some(b) };
    render_plan(e, &plan)
}

/// Render a prepared Timewarp plan (also Pixel Motion Blur's, `effectcraft_effects::pmb_plan`):
/// every step to an image on the GPU, then the weighted average.
fn render_plan(e: &mut Enc, plan: &effectcraft_effects::TimewarpPlan) -> Option<GBuf> {
    // Pixel Motion's in-between frame of frames a, b along motion field `flow`.
    let motion = |e: &mut Enc, a: usize, bi: usize, w: f32, flow: usize| -> Option<GpuImage> {
        let pa = e.g.upload_image(&effectcraft_effects::timewarp_crop(&plan.frames[a], &plan.pm))?;
        let pb = e.g.upload_image(&effectcraft_effects::timewarp_crop(&plan.frames[bi], &plan.pm))?;
        let f = &plan.flows[flow];
        let v: Vec<f32> = f.v.iter().flat_map(|q| [q[0], q[1]]).collect();
        let data = e.data(&v);
        let mut p = Params::default();
        p.u[0] = [if f.v.is_empty() { 0 } else { f.cols as u32 }, f.rows as u32, plan.pm.extreme as u32, plan.pm.build_from_one as u32];
        p.f[0] = [f.block as f32, w, plan.pm.error_threshold, 0.0];
        Some(kernel(e, "ftm_motion", &p, &pa, Some(&pb), Some(&data)))
    };
    let mut up: Vec<Option<GpuImage>> = vec![None; plan.frames.len()];
    let mut frame = |e: &mut Enc, i: usize| -> Option<GpuImage> {
        if up[i].is_none() {
            up[i] = Some(e.g.upload_image(&plan.frames[i])?);
        }
        up[i].clone()
    };
    let mut imgs = vec![];
    for (step, _) in &plan.steps {
        let img = match *step {
            TwStep::Whole(i) => frame(e, i)?,
            TwStep::Mix(i, j, w) => {
                let (a, bb) = (frame(e, i)?, frame(e, j)?);
                let mut p = Params::default();
                p.f[0][0] = w;
                kernel(e, "ftm_mix", &p, &a, Some(&bb), None)
            }
            TwStep::Motion { a, b: bi, w, flow } => motion(e, a, bi, w, flow)?,
            TwStep::Layered { fg, bg, w, show } => {
                let f = motion(e, fg[0], fg[1], w, fg[2])?;
                let g = motion(e, bg[0], bg[1], w, bg[2])?;
                let mut p = Params::default();
                p.u[0][0] = show;
                kernel(e, "ftm_layer", &p, &f, Some(&g), None)
            }
        };
        imgs.push(img);
    }
    let img = if imgs.len() == 1 {
        imgs.pop()?
    } else {
        let weights: Vec<f32> = plan.steps.iter().map(|(_, w)| *w).collect();
        average(e, &imgs, &weights)?
    };
    Some(on_grid(img, &plan.grid))
}
