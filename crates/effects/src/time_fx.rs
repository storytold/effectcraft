//! Time effects (Effect > Time): Echo, Posterize Time, Time Difference, Time Displacement,
//! Timewarp, CC Force Motion Blur, CC Wide Time and Pixel Motion Blur.
//!
//! These read the layer at *other* layer times through [`EffectHost::self_at`]: its source with
//! its masks and the effects applied before them, as After Effects renders them (SceneYard:
//! Echo after CC Particle Systems II echoes the fireworks, not the black solid under them;
//! Posterize Time after Turbulent Displace steps the displacement). Timewarp is the exception:
//! it retimes the layer's source with its masks and ignores the effects above it (After
//! Effects 2026: a red solid with a Gradient Ramp and then Timewarp shows the red solid).
//! Without a host (tests, thumbnails) they pass their input through.
//!
//! Timewarp's Pixel Motion estimates block-matching optical flow between neighbouring frames
//! ([`effectcraft_raster::flow`]); Pixel Motion Blur averages sub-frame samples.
//!
//! [`EffectHost::self_at`]: crate::EffectHost::self_at

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::{Image, Px};
use rayon::prelude::*;

use effectcraft_raster::flow::Flow;

use crate::util::{fit_layer, unpremul};
use crate::{Buf, EffectCtx, EffectSpec, num, p, popup, slider};

fn spec(id: &'static str, name: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: "Time", params, render, gpu: false, float: true }
}

pub fn specs() -> Vec<EffectSpec> {
    vec![
        spec(
            "ec.time.echo",
            "Echo",
            vec![
                p("echoTime", "Echo Time (seconds)", num(-1.0 / 30.0), slider(-30.0, 30.0, -5.0, 5.0, 3)),
                p("numberOfEchoes", "Number Of Echoes", num(1.0), slider(0.0, 255.0, 0.0, 30.0, 0)),
                p("startingIntensity", "Starting Intensity", num(1.0), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("decay", "Decay", num(1.0), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p(
                    "echoOperator",
                    "Echo Operator",
                    Value::Enum(0),
                    popup(&["Add", "Maximum", "Minimum", "Screen", "Composite In Back", "Composite In Front", "Blend"]),
                ),
            ],
            echo,
        ),
        spec("ec.time.posterizetime", "Posterize Time", vec![p("frameRate", "Frame Rate", num(12.0), slider(0.1, 99.0, 1.0, 60.0, 1))], posterize_time),
        spec(
            "ec.time.timedifference",
            "Time Difference",
            vec![
                p("targetLayer", "Target", Value::Layer(None), ParamUi::Layer),
                p("timeOffset", "Time Offset (sec)", num(0.0), slider(-30.0, 30.0, -5.0, 5.0, 3)),
                p("contrast", "Contrast", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("absoluteDifference", "Absolute Difference", Value::Bool(true), ParamUi::Checkbox),
                p(
                    "alphaChannel",
                    "Alpha Channel",
                    Value::Enum(0),
                    popup(&[
                        "Original",
                        "Target",
                        "Blend",
                        "Max",
                        "Full On",
                        "Lightness Of Result",
                        "Max Of Result",
                        "Alpha Difference",
                        "Alpha Difference Only",
                    ]),
                ),
            ],
            time_difference,
        ),
        spec(
            "ec.time.timedisplacement",
            "Time Displacement",
            vec![
                p("displacementMapLayer", "Time Displacement Layer", Value::Layer(None), ParamUi::Layer),
                p("maxDisplacementTime", "Max Displacement Time (sec)", num(1.0), slider(-30.0, 30.0, -5.0, 5.0, 2)),
                p("timeResolution", "Time Resolution (fps)", num(60.0), slider(1.0, 999.0, 1.0, 120.0, 1)),
                p("stretchMap", "Stretch Map To Fit", Value::Bool(true), ParamUi::Checkbox),
            ],
            time_displacement,
        ),
        spec(
            "ec.time.timewarp",
            "Timewarp",
            vec![
                p("method", "Method", Value::Enum(2), popup(&["Whole Frames", "Frame Mix", "Pixel Motion"])),
                p("adjustTimeBy", "Adjust Time By", Value::Enum(0), popup(&["Speed", "Source Frame"])),
                p("speed", "Speed", num(50.0), slider(-10000.0, 10000.0, 0.0, 200.0, 1)),
                p("sourceFrame", "Source Frame", num(0.0), slider(-100000.0, 100000.0, 0.0, 300.0, 1)),
                p("tuning/vectorDetail", "Vector Detail", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("tuning/smoothing/globalSmoothness", "Global Smoothness", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("tuning/smoothing/localSmoothness", "Local Smoothness", num(0.5), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("tuning/smoothing/smoothingIterations", "Smoothing Iterations", num(20.0), slider(0.0, 200.0, 0.0, 100.0, 0)),
                p("tuning/buildFromOneImage", "Build From One Image", Value::Bool(false), ParamUi::Checkbox),
                p("tuning/correctLuminanceChanges", "Correct Luminance Changes", Value::Bool(false), ParamUi::Checkbox),
                p("tuning/filtering", "Filtering", Value::Enum(0), popup(&["Normal", "Extreme"])),
                p("tuning/errorThreshold", "Error Threshold", num(10.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
                p("tuning/blockSize", "Block Size", num(8.0), slider(4.0, 64.0, 4.0, 32.0, 0)),
                p("tuning/weighting/redWeighting", "Red Weighting", num(0.3), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("tuning/weighting/greenWeighting", "Green Weighting", num(0.59), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("tuning/weighting/blueWeighting", "Blue Weighting", num(0.11), slider(0.0, 1.0, 0.0, 1.0, 2)),
                p("motionBlur/enableMotionBlur", "Enable Motion Blur", Value::Bool(false), ParamUi::Checkbox),
                p("motionBlur/shutterControl", "Shutter Control", Value::Enum(0), popup(&["Automatic", "Manual"])),
                p("motionBlur/shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 720.0, 0.0, 360.0, 1)),
                p("motionBlur/shutterSamples", "Shutter Samples", num(5.0), slider(1.0, 64.0, 1.0, 32.0, 0)),
                p("matteLayer", "Matte Layer", Value::Layer(None), ParamUi::Layer),
                p("matteChannel", "Matte Channel", Value::Enum(0), popup(&TW_MATTE_CHANNELS)),
                p("warpLayer", "Warp Layer", Value::Layer(None), ParamUi::Layer),
                p("show", "Show", Value::Enum(0), popup(&TW_SHOW)),
                p("sourceCrops/leftCrop", "Left Crop", num(0.0), slider(0.0, 4000.0, 0.0, 100.0, 0)),
                p("sourceCrops/rightCrop", "Right Crop", num(0.0), slider(0.0, 4000.0, 0.0, 100.0, 0)),
                p("sourceCrops/bottomCrop", "Bottom Crop", num(0.0), slider(0.0, 4000.0, 0.0, 100.0, 0)),
                p("sourceCrops/topCrop", "Top Crop", num(0.0), slider(0.0, 4000.0, 0.0, 100.0, 0)),
            ],
            timewarp,
        ),
        spec(
            "ec.time.ccforcemotionblur",
            "CC Force Motion Blur",
            vec![
                p("motionBlurLevels", "Motion Blur Levels", num(8.0), slider(2.0, 64.0, 2.0, 32.0, 0)),
                p("overrideShutterAngle", "Override Shutter Angle", Value::Bool(true), ParamUi::Checkbox),
                p("shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 3600.0, 0.0, 360.0, 1)),
                p("nativeMotionBlur", "Native Motion Blur", Value::Enum(0), popup(&["Off", "On"])),
            ],
            force_motion_blur,
        ),
        spec(
            "ec.time.ccwidetime",
            "CC Wide Time",
            vec![
                p("forwardSteps", "Forward Steps", num(3.0), slider(0.0, 45.0, 0.0, 15.0, 0)),
                p("backwardSteps", "Backward Steps", num(3.0), slider(0.0, 45.0, 0.0, 15.0, 0)),
                p("nativeMotionBlur", "Native Motion Blur", Value::Enum(0), popup(&["Off", "On"])),
            ],
            wide_time,
        ),
        spec(
            "ec.time.pixelmotionblur",
            "Pixel Motion Blur",
            vec![
                p("shutterControl", "Shutter Control", Value::Enum(0), popup(&["Automatic", "Manual"])),
                p("shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 720.0, 0.0, 360.0, 1)),
                p("shutterSamples", "Shutter Samples", num(16.0), slider(1.0, 64.0, 1.0, 32.0, 0)),
                p("vectorDetail", "Vector Detail", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 1)),
            ],
            pixel_motion_blur,
        ),
    ]
}

// ---------------------------------------------------------------- frame access

/// Frames of the layer at several layer times, resampled onto one common pixel grid.
struct Frames {
    /// Template buffer (geometry of the output: offset/scale/size).
    grid: Buf,
    imgs: Vec<Image>,
}

/// Fetch the layer (source + masks + the effects before this one) at layer times `times`.
/// `None` without a host.
fn fetch(ctx: &EffectCtx, times: &[f64]) -> Option<Frames> {
    fetch_with(ctx, times, usize::MAX)
}

/// [`fetch`] with the first `effects` effects of the stack only (0: source and masks).
fn fetch_with(ctx: &EffectCtx, times: &[f64], effects: usize) -> Option<Frames> {
    let host = ctx.env.host?;
    // Dedupe times (sub-microsecond differences are the same frame).
    let mut uniq: Vec<i64> = times.iter().map(|t| (t * 1e6).round() as i64).collect();
    uniq.sort_unstable();
    uniq.dedup();
    // Every effect before this one (the host bounds the count to them).
    let bufs: Vec<(i64, Option<Buf>)> = uniq.iter().map(|&k| (k, host.self_at(k as f64 / 1e6, effects))).collect();
    let first = bufs.iter().find_map(|(_, b)| b.as_ref())?;
    let scale = first.scale;
    // Union of every frame's extent in layer pixels (offset = where layer (0,0) sits).
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for b in bufs.iter().filter_map(|(_, b)| b.as_ref()) {
        let k = scale / b.scale.max(1e-9);
        x0 = x0.min(-b.offset[0] * k);
        y0 = y0.min(-b.offset[1] * k);
        x1 = x1.max((b.img.width as f64 - b.offset[0]) * k);
        y1 = y1.max((b.img.height as f64 - b.offset[1]) * k);
    }
    let (x0, y0) = (x0.floor(), y0.floor());
    let w = ((x1.ceil() - x0) as u32).clamp(1, 16384);
    let h = ((y1.ceil() - y0) as u32).clamp(1, 16384);
    let grid = Buf { img: Image::new(w, h), offset: [-x0, -y0], scale };
    let place = |b: &Buf| -> Image {
        if (b.scale - scale).abs() < 1e-9 && b.img.width == w && b.img.height == h && (b.offset[0] + x0).abs() < 1e-6 && (b.offset[1] + y0).abs() < 1e-6 {
            return b.img.clone();
        }
        let k = b.scale / scale;
        let same_scale = (k - 1.0).abs() < 1e-9;
        let dx = b.offset[0] + x0;
        let dy = b.offset[1] + y0;
        let mut out = Image::new(w, h);
        let ow = w as usize;
        out.data.par_chunks_mut(ow).enumerate().for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                *px = if same_scale && dx.fract() == 0.0 && dy.fract() == 0.0 {
                    b.img.get(x as i64 + dx as i64, y as i64 + dy as i64)
                } else {
                    let sx = (x as f64 + 0.5) * k + dx - 0.5;
                    let sy = (y as f64 + 0.5) * k + dy - 0.5;
                    b.img.sample_bilinear(sx + 0.5, sy + 0.5)
                };
            }
        });
        out
    };
    let placed: Vec<(i64, Image)> = bufs.iter().map(|(k, b)| (*k, b.as_ref().map(place).unwrap_or_else(|| Image::new(w, h)))).collect();
    let imgs = times
        .iter()
        .map(|t| {
            let k = (t * 1e6).round() as i64;
            placed.iter().find(|(q, _)| *q == k).map(|(_, i)| i.clone()).unwrap_or_else(|| Image::new(w, h))
        })
        .collect();
    Some(Frames { grid, imgs })
}

/// The layer's frames at layer times `times` placed on one grid (the GPU Echo / Posterize Time,
/// effectcraft-gpu `fx_noise`): (grid buffer, one image per time). `None` without a host.
pub fn time_frames(ctx: &EffectCtx, times: &[f64]) -> Option<(Buf, Vec<Image>)> {
    fetch(ctx, times).map(|f| (f.grid, f.imgs))
}

fn with_img(grid: Buf, img: Image) -> Buf {
    Buf { img, ..grid }
}

/// Weighted average of frames (weights sum to 1 for a plain average).
fn average(frames: &[Image], weights: &[f32]) -> Image {
    let w = frames[0].width;
    let h = frames[0].height;
    let mut out = Image::new(w, h);
    out.data.par_iter_mut().enumerate().for_each(|(i, o)| {
        let mut acc = [0.0f32; 4];
        for (f, k) in frames.iter().zip(weights) {
            let p = f.data[i];
            for c in 0..4 {
                acc[c] += p[c] * k;
            }
        }
        *o = acc;
    });
    out
}

fn mean_of(ctx: &EffectCtx, b: Buf, times: &[f64]) -> Buf {
    if times.is_empty() {
        return b;
    }
    let Some(fr) = fetch(ctx, times) else { return b };
    let k = 1.0 / times.len() as f32;
    let img = average(&fr.imgs, &vec![k; times.len()]);
    with_img(fr.grid, img)
}

// ---------------------------------------------------------------- Echo

fn echo(ctx: &EffectCtx, b: Buf) -> Buf {
    let n = ctx.params.f("numberOfEchoes").round().clamp(0.0, 255.0) as usize;
    let dt = ctx.params.f("echoTime");
    let start = ctx.params.f("startingIntensity").clamp(0.0, 1.0) as f32;
    let decay = ctx.params.f("decay").clamp(0.0, 1.0) as f32;
    let op = ctx.params.e("echoOperator");
    let times: Vec<f64> = (0..=n).map(|i| ctx.time + i as f64 * dt).collect();
    let Some(fr) = fetch(ctx, &times) else { return b };
    let weights: Vec<f32> = (0..=n).map(|i| start * decay.powi(i as i32)).collect();
    let count = (n + 1) as f32;
    let w = fr.grid.img.width;
    let h = fr.grid.img.height;
    let mut out = Image::new(w, h);
    let imgs = &fr.imgs;
    out.data.par_iter_mut().enumerate().for_each(|(i, o)| {
        let mut acc: Px = match op {
            2 => [f32::MAX; 4],
            _ => [0.0; 4],
        };
        for (f, &k) in imgs.iter().zip(&weights) {
            let p = f.data[i];
            let q = [p[0] * k, p[1] * k, p[2] * k, p[3] * k];
            match op {
                // Maximum / Minimum
                1 => (0..4).for_each(|c| acc[c] = acc[c].max(q[c])),
                2 => (0..4).for_each(|c| acc[c] = acc[c].min(q[c])),
                // Screen
                3 => (0..4).for_each(|c| acc[c] = acc[c] + q[c] - acc[c] * q[c]),
                // Composite In Back: each later echo goes behind what is there.
                4 => {
                    let ia = 1.0 - acc[3];
                    (0..4).for_each(|c| acc[c] += q[c] * ia);
                }
                // Composite In Front: each later echo goes on top.
                5 => {
                    let ia = 1.0 - q[3];
                    (0..4).for_each(|c| acc[c] = q[c] + acc[c] * ia);
                }
                // Blend (average) and Add
                _ => (0..4).for_each(|c| acc[c] += q[c]),
            }
        }
        if op == 6 {
            acc.iter_mut().for_each(|c| *c /= count);
        }
        if op == 2 && acc[0] == f32::MAX {
            acc = [0.0; 4];
        }
        acc[3] = acc[3].clamp(0.0, 1.0);
        for c in 0..3 {
            acc[c] = acc[c].max(0.0);
        }
        *o = acc;
    });
    with_img(fr.grid, out)
}

// ---------------------------------------------------------------- Posterize Time

/// The layer time Posterize Time holds at `t` for `rate` frames per second.
pub fn posterized_time(t: f64, rate: f64) -> f64 {
    if rate <= 0.0 {
        return t;
    }
    ((t * rate) + 1e-6).floor() / rate
}

fn posterize_time(ctx: &EffectCtx, b: Buf) -> Buf {
    let tt = posterized_time(ctx.time, ctx.params.f("frameRate"));
    if ctx.env.host.is_none() {
        return b;
    }
    match fetch(ctx, &[tt]) {
        Some(mut fr) => {
            let img = fr.imgs.pop().unwrap_or_default();
            with_img(fr.grid, img)
        }
        None => b,
    }
}

// ---------------------------------------------------------------- Time Difference

fn time_difference(ctx: &EffectCtx, b: Buf) -> Buf {
    let Some(host) = ctx.env.host else { return b };
    let off = ctx.params.f("timeOffset");
    let gain = 1.0 + ctx.params.f("contrast") as f32 / 25.0;
    let abs = ctx.params.b("absoluteDifference");
    let amode = ctx.params.e("alphaChannel");
    let target_id = ctx.params.get("targetLayer").and_then(Value::as_layer);
    let (cur, target) = match target_id.and_then(|id| host.layer_at(id, ctx.env.comp_time + off, false)) {
        Some(other) => {
            let Some(mut fr) = fetch(ctx, &[ctx.time]) else { return b };
            let cur = fr.imgs.pop().unwrap_or_default();
            let tgt = fit_layer(ctx, &with_img(fr.grid.clone(), cur.clone()), &other, true);
            (with_img(fr.grid, cur), tgt)
        }
        None => {
            let Some(mut fr) = fetch(ctx, &[ctx.time, ctx.time + off]) else { return b };
            let tgt = fr.imgs.pop().unwrap_or_default();
            let cur = fr.imgs.pop().unwrap_or_default();
            (with_img(fr.grid, cur), tgt)
        }
    };
    let mut out = cur.img.clone();
    out.data.par_iter_mut().zip(target.data.par_iter()).for_each(|(o, t)| {
        let (c, ca) = unpremul(*o);
        let (d, da) = unpremul(*t);
        let mut rgb = [0.0f32; 3];
        for i in 0..3 {
            let diff = c[i] - d[i];
            rgb[i] = if abs { (diff.abs() * gain).clamp(0.0, 1.0) } else { (0.5 + diff * 0.5 * gain).clamp(0.0, 1.0) };
        }
        let light = (rgb[0].max(rgb[1]).max(rgb[2]) + rgb[0].min(rgb[1]).min(rgb[2])) * 0.5;
        let a = match amode {
            1 => da,
            2 => (ca + da) * 0.5,
            3 => ca.max(da),
            4 => 1.0,
            5 => light,
            6 => rgb[0].max(rgb[1]).max(rgb[2]),
            7 => (ca - da).abs() * gain,
            8 => {
                let a = ((ca - da).abs() * gain).clamp(0.0, 1.0);
                rgb = [1.0; 3];
                a
            }
            _ => ca,
        }
        .clamp(0.0, 1.0);
        *o = [rgb[0] * a, rgb[1] * a, rgb[2] * a, a];
    });
    Buf { img: out, ..cur }
}

// ---------------------------------------------------------------- Time Displacement

fn time_displacement(ctx: &EffectCtx, b: Buf) -> Buf {
    if ctx.env.host.is_none() {
        return b;
    }
    let max = ctx.params.f("maxDisplacementTime");
    let res = ctx.params.f("timeResolution").clamp(1.0, 999.0);
    let stretch = ctx.params.b("stretchMap");
    let Some(mut cur) = fetch(ctx, &[ctx.time]) else { return b };
    let cur_img = cur.imgs.pop().unwrap_or_default();
    let grid = with_img(cur.grid, cur_img);
    let map = match ctx.layer_param("displacementMapLayer", true) {
        Some(o) => fit_layer(ctx, &grid, &o, stretch),
        None => grid.img.clone(),
    };
    // Per pixel: luminance 0..1 → offset −max..+max, quantised to the time resolution. Keep at
    // most 64 distinct sample times (coarser steps for huge ranges).
    let span = 2.0 * max.abs() * res;
    let step = if span > 63.0 { 2.0 * max.abs() / 63.0 } else { 1.0 / res };
    let idx: Vec<i32> = map
        .data
        .par_iter()
        .map(|p| {
            let (c, a) = unpremul(*p);
            let l = if a > 0.0 { effectcraft_color::luminance(c[0], c[1], c[2]) } else { 0.5 };
            let off = (l as f64 - 0.5) * 2.0 * max;
            (off / step).round() as i32
        })
        .collect();
    let mut keys: Vec<i32> = idx.clone();
    keys.sort_unstable();
    keys.dedup();
    if keys == [0] {
        return grid;
    }
    let times: Vec<f64> = keys.iter().map(|k| ctx.time + *k as f64 * step).collect();
    let Some(fr) = fetch(ctx, &times) else { return grid };
    // Frames come back on the union grid; map the current grid into it.
    let dx = (fr.grid.offset[0] - grid.offset[0]).round() as i64;
    let dy = (fr.grid.offset[1] - grid.offset[1]).round() as i64;
    let w = grid.img.width as usize;
    let mut out = grid.img.clone();
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let k = idx[y * w + x];
            let fi = keys.binary_search(&k).unwrap_or(0);
            *o = fr.imgs[fi].get(x as i64 + dx, y as i64 + dy);
        }
    });
    with_img(grid, out)
}

// ---------------------------------------------------------------- Timewarp

/// Source layer time Timewarp shows at layer time `t`.
pub fn timewarp_source_time(t: f64, by_frame: bool, speed: f64, source_frame: f64, fps: f64) -> f64 {
    if by_frame { source_frame / fps } else { t * speed / 100.0 }
}

/// Timewarp's Pixel Motion settings (the Tuning group and Source Crops).
#[derive(Clone, Copy, Debug)]
pub struct PixelMotion {
    /// Vector spacing in pixels (from Vector Detail).
    pub tile: u32,
    /// Matching block size in pixels.
    pub window: u32,
    pub global_smoothness: f32,
    pub local_smoothness: f32,
    pub iterations: u32,
    pub build_from_one: bool,
    pub correct_luminance: bool,
    pub extreme: bool,
    /// Error Threshold (0..1): pixels whose two warped frames disagree by more fall back to a
    /// frame mix.
    pub error_threshold: f32,
    pub weights: [f32; 3],
    /// Left, right, top, bottom crops in pixels.
    pub crops: [u32; 4],
}

impl PixelMotion {
    fn from(ctx: &EffectCtx, scale: f64) -> PixelMotion {
        let pr = ctx.params;
        let g = |id: &str, d: f64| pr.get(id).map(Value::as_f64).unwrap_or(d);
        let detail = g("tuning/vectorDetail", 20.0).clamp(0.0, 100.0) / 100.0;
        // Vector Detail 0 → one vector per 32 px, 100 → one per 2 px (exponential).
        let tile = (32.0 * (1.0f64 / 16.0).powf(detail) * scale).round().max(2.0) as u32;
        let px = |id: &str| (g(id, 0.0).max(0.0) * scale).round() as u32;
        PixelMotion {
            tile,
            window: ((g("tuning/blockSize", 8.0).max(4.0)) * scale).round().max(4.0) as u32,
            global_smoothness: g("tuning/smoothing/globalSmoothness", 0.5).clamp(0.0, 1.0) as f32,
            local_smoothness: g("tuning/smoothing/localSmoothness", 0.5).clamp(0.0, 1.0) as f32,
            iterations: g("tuning/smoothing/smoothingIterations", 20.0).clamp(0.0, 200.0) as u32,
            build_from_one: pr.b("tuning/buildFromOneImage"),
            correct_luminance: pr.b("tuning/correctLuminanceChanges"),
            extreme: pr.e("tuning/filtering") == 1,
            error_threshold: (g("tuning/errorThreshold", 10.0) / 100.0).clamp(0.0, 1.0) as f32,
            weights: [g("tuning/weighting/redWeighting", 0.3), g("tuning/weighting/greenWeighting", 0.59), g("tuning/weighting/blueWeighting", 0.11)]
                .map(|v| v.max(0.0) as f32),
            crops: [px("sourceCrops/leftCrop"), px("sourceCrops/rightCrop"), px("sourceCrops/topCrop"), px("sourceCrops/bottomCrop")],
        }
    }
}

/// Replace the cropped edges of `img` with the nearest kept pixel (Source Crops: edge garbage
/// must not drive or smear the motion).
fn crop_edges(img: &Image, [l, r, t, b]: [u32; 4]) -> Image {
    if l == 0 && r == 0 && t == 0 && b == 0 {
        return img.clone();
    }
    let (w, h) = (img.width as i64, img.height as i64);
    let (x0, x1) = ((l as i64).min(w - 1), (w - 1 - r as i64).max(0));
    let (y0, y1) = ((t as i64).min(h - 1), (h - 1 - b as i64).max(0));
    let mut out = img.clone();
    out.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = img.get((x as i64).clamp(x0, x1.max(x0)), (y as i64).clamp(y0, y1.max(y0)));
        }
    });
    out
}

/// Weighted grey (R·wr + G·wg + B·wb in every colour channel) that the motion is estimated on.
fn motion_grey(img: &Image, wt: [f32; 3], gain: f32) -> Image {
    let sum = (wt[0] + wt[1] + wt[2]).max(1e-6);
    let mut out = img.clone();
    out.data.par_iter_mut().for_each(|p| {
        let g = (p[0] * wt[0] + p[1] * wt[1] + p[2] * wt[2]) / sum * gain;
        *p = [g, g, g, p[3]];
    });
    out
}

fn mean_grey(img: &Image) -> f32 {
    let n = img.data.len().max(1) as f32;
    img.data.par_iter().map(|p| p[0]).sum::<f32>() / n
}

/// Smooth a motion field: `iterations` passes pulling each vector towards its 3×3 mean by
/// Local Smoothness, then a blend towards a wide average by Global Smoothness.
fn smooth_flow(flow: &mut Flow, pm: &PixelMotion) {
    let (c, r) = (flow.cols, flow.rows);
    if c * r <= 1 {
        return;
    }
    let mean = |v: &[[f32; 2]], rad: i64| -> Vec<[f32; 2]> {
        (0..c * r)
            .into_par_iter()
            .map(|i| {
                let (x, y) = ((i % c) as i64, (i / c) as i64);
                let mut s = [0.0f32; 2];
                let mut n = 0.0;
                for dy in -rad..=rad {
                    for dx in -rad..=rad {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx >= 0 && ny >= 0 && (nx as usize) < c && (ny as usize) < r {
                            let q = v[ny as usize * c + nx as usize];
                            s[0] += q[0];
                            s[1] += q[1];
                            n += 1.0;
                        }
                    }
                }
                [s[0] / n, s[1] / n]
            })
            .collect()
    };
    if pm.local_smoothness > 0.0 {
        for _ in 0..pm.iterations {
            let m = mean(&flow.v, 1);
            for (v, q) in flow.v.iter_mut().zip(m) {
                v[0] += (q[0] - v[0]) * pm.local_smoothness * 0.5;
                v[1] += (q[1] - v[1]) * pm.local_smoothness * 0.5;
            }
        }
    }
    if pm.global_smoothness > 0.0 {
        let m = mean(&flow.v, 3);
        for (v, q) in flow.v.iter_mut().zip(m) {
            v[0] += (q[0] - v[0]) * pm.global_smoothness * 0.5;
            v[1] += (q[1] - v[1]) * pm.global_smoothness * 0.5;
        }
    }
}

/// The motion field from frame `a` to frame `b` (or from the warp layer's frames).
pub fn timewarp_flow(a: &Image, b: &Image, pm: &PixelMotion) -> Flow {
    timewarp_flow_search(a, b, pm, 4)
}

/// [`timewarp_flow`] with an explicit coarse search radius: Timewarp's own resampling needs
/// little (motion between adjacent source frames at near-100% speed), but Pixel Motion Blur
/// must find fast full-frame displacements (#535).
fn timewarp_flow_search(a: &Image, b: &Image, pm: &PixelMotion, radius: i32) -> Flow {
    let ga = motion_grey(&crop_edges(a, pm.crops), pm.weights, 1.0);
    let gain = if pm.correct_luminance {
        let (ma, mb) = (mean_grey(&ga), mean_grey(&motion_grey(b, pm.weights, 1.0)));
        if mb > 1e-4 { ma / mb } else { 1.0 }
    } else {
        1.0
    };
    let gb = motion_grey(&crop_edges(b, pm.crops), pm.weights, gain);
    let mut flow = effectcraft_raster::flow::block_flow_window(&ga, &gb, pm.tile, pm.window, radius);
    smooth_flow(&mut flow, pm);
    flow
}

/// Pixel Motion: the frame at fraction `w` between `a` and `b` built by warping along `flow`
/// (Build From One Image: from the nearer frame only), falling back to a mix where the two
/// warped frames disagree by more than the Error Threshold.
pub fn timewarp_interpolate(a: &Image, b: &Image, w: f32, flow: &Flow, pm: &PixelMotion) -> Image {
    let (a, b) = (crop_edges(a, pm.crops), crop_edges(b, pm.crops));
    let mut out = Image::new(a.width, a.height);
    let wd = w as f64;
    let sample = |img: &Image, x: f64, y: f64| if pm.extreme { img.sample_bicubic(x, y) } else { img.sample_bilinear_clamped(x, y) };
    out.rows_mut().for_each(|(y, row)| {
        let cy = y as f64 + 0.5;
        for (x, px) in row.iter_mut().enumerate() {
            let cx = x as f64 + 0.5;
            let f = flow.at(cx, cy);
            let (fx, fy) = (f[0] as f64, f[1] as f64);
            let pa = sample(&a, cx - wd * fx, cy - wd * fy);
            let pb = sample(&b, cx + (1.0 - wd) * fx, cy + (1.0 - wd) * fy);
            if pm.build_from_one {
                *px = if w < 0.5 { pa } else { pb };
                continue;
            }
            let err = (0..4).map(|c| (pa[c] - pb[c]).abs()).fold(0.0f32, f32::max);
            let (pa, pb) = if pm.error_threshold > 0.0 && err > pm.error_threshold && pm.error_threshold < 1.0 {
                (a.get(x as i64, y as i64), b.get(x as i64, y as i64))
            } else {
                (pa, pb)
            };
            for c in 0..4 {
                px[c] = pa[c] + (pb[c] - pa[c]) * w;
            }
        }
    });
    out
}

/// Timewarp's Matte Channel options.
const TW_MATTE_CHANNELS: [&str; 4] = ["Alpha", "Inverted Alpha", "Luminance", "Inverted Luminance"];
/// Timewarp's Show options.
const TW_SHOW: [&str; 4] = ["Normal", "Foreground Only", "Background Only", "Matte"];

/// A matte plane (foreground = 1) from a matte layer frame.
fn matte_of(img: &Image, channel: u32) -> Vec<f32> {
    img.data
        .par_iter()
        .map(|p| {
            let (c, a) = unpremul(*p);
            let l = (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * a;
            match channel {
                1 => 1.0 - a,
                2 => l,
                3 => 1.0 - l,
                _ => a,
            }
            .clamp(0.0, 1.0)
        })
        .collect()
}

fn times_mask(img: &Image, m: &[f32], invert: bool) -> Image {
    let mut out = img.clone();
    out.data.par_iter_mut().zip(m.par_iter()).for_each(|(p, &k)| {
        let k = if invert { 1.0 - k } else { k };
        p.iter_mut().for_each(|v| *v *= k);
    });
    out
}

/// Another layer (layer parameter `id`) at the comp times matching source layer times `times`,
/// fitted onto `grid`. `None` when no layer is chosen.
fn fetch_layer(ctx: &EffectCtx, id: &str, times: &[f64], grid: &Buf) -> Option<Vec<Image>> {
    let lid = ctx.params.get(id)?.as_layer()?;
    let host = ctx.env.host?;
    let me = ctx.params.get(&crate::layer_source_id(id)).is_none_or(|v| v.as_enum() != 0);
    let shift = ctx.env.comp_time - ctx.time;
    Some(
        times
            .iter()
            .map(|t| match host.layer_at(lid, t + shift, me) {
                Some(lp) => fit_layer(ctx, grid, &lp, false),
                None => Image::new(grid.img.width, grid.img.height),
            })
            .collect(),
    )
}

/// One output instant of a Timewarp: a frame, a cross-fade of two, or the in-between frame
/// built along a motion field ([`TimewarpPlan::flows`]); with a Matte Layer's Pixel Motion, the
/// foreground and background pairs (`[frame a, frame b, flow]`) each built on their own vectors
/// and shown as Show asks.
#[derive(Clone, Copy, Debug)]
pub enum TwStep {
    Whole(usize),
    Mix(usize, usize, f32),
    Motion { a: usize, b: usize, w: f32, flow: usize },
    Layered { fg: [usize; 3], bg: [usize; 3], w: f32, show: u32 },
}

/// Timewarp prepared for the GPU compositor (effectcraft-gpu `fx_time`): the fetched frames on
/// one grid (plus the matte-masked ones a Matte Layer needs), the instants across the shutter
/// (with their weights) and the motion fields (block matching on the CPU).
/// [`TimewarpPlan::render_cpu`] is the CPU effect.
pub struct TimewarpPlan {
    pub grid: Buf,
    pub frames: Vec<Image>,
    pub steps: Vec<(TwStep, f32)>,
    pub flows: Vec<Flow>,
    pub pm: PixelMotion,
}

impl TimewarpPlan {
    pub fn render_cpu(&self) -> Buf {
        let f = &self.frames;
        let imgs: Vec<Image> = self
            .steps
            .iter()
            .map(|(s, _)| match *s {
                TwStep::Whole(i) => f[i].clone(),
                TwStep::Mix(i, j, w) => effectcraft_raster::flow::mix(&f[i], &f[j], w),
                TwStep::Motion { a, b, w, flow } => timewarp_interpolate(&f[a], &f[b], w, &self.flows[flow], &self.pm),
                TwStep::Layered { fg, bg, w, show } => {
                    let fgi = timewarp_interpolate(&f[fg[0]], &f[fg[1]], w, &self.flows[fg[2]], &self.pm);
                    let bgi = timewarp_interpolate(&f[bg[0]], &f[bg[1]], w, &self.flows[bg[2]], &self.pm);
                    layered(fgi, bgi, show)
                }
            })
            .collect();
        let weights: Vec<f32> = self.steps.iter().map(|(_, w)| *w).collect();
        let img = if imgs.len() == 1 { imgs.into_iter().next().unwrap_or_default() } else { average(&imgs, &weights) };
        with_img(self.grid.clone(), img)
    }
}

/// Source-crop the edges of a frame (Pixel Motion works on cropped frames).
pub fn timewarp_crop(img: &Image, pm: &PixelMotion) -> Image {
    crop_edges(img, pm.crops)
}

/// The source instants Timewarp shows at the frame (with their weights) and the layer times
/// they need: (instants, frame times, frames per second).
fn timewarp_instants(ctx: &EffectCtx) -> (Vec<(f64, f32)>, Vec<f64>, f64) {
    let fps = ctx.fps();
    let by_frame = ctx.params.e("adjustTimeBy") == 1;
    let speed = ctx.params.f("speed");
    let src = timewarp_source_time(ctx.time, by_frame, speed, ctx.params.f("sourceFrame"), fps);
    let mut instants: Vec<(f64, f32)> = vec![];
    if ctx.params.b("motionBlur/enableMotionBlur") {
        let angle = if ctx.params.e("motionBlur/shutterControl") == 1 { ctx.params.f("motionBlur/shutterAngle") } else { 180.0 };
        let n = ctx.params.f("motionBlur/shutterSamples").round().clamp(1.0, 64.0) as usize;
        let rate = if by_frame { 1.0 } else { speed / 100.0 };
        let span = angle / 360.0 * rate / fps;
        for i in 0..n {
            instants.push((src + if n > 1 { span * i as f64 / (n - 1) as f64 } else { 0.0 }, 1.0 / n as f32));
        }
    } else {
        instants.push((src, 1.0));
    }
    let mut times: Vec<f64> = vec![];
    for &(s, _) in &instants {
        let (f0, _) = tw_split(s, fps);
        times.push(f0 / fps);
        times.push((f0 + 1.0) / fps);
    }
    (instants, times, fps)
}

/// The frame at or before source time `s` and the fraction to the next one.
fn tw_split(s: f64, fps: f64) -> (f64, f32) {
    let f = s * fps;
    let f0 = (f + 1e-6).floor();
    (f0, ((f - f0) as f32).max(0.0))
}

/// [`TimewarpPlan`] of a Timewarp instance. `None` without the effect host or source frames (the
/// layer passes through). With a Matte Layer the frames the instants need (masked by the matte,
/// or the matte itself for Show Matte) are prepared here and join [`TimewarpPlan::frames`].
pub fn timewarp_plan(ctx: &EffectCtx) -> Option<TimewarpPlan> {
    ctx.env.host?;
    // Instants of source time (with weights) across the shutter; each needs the frame at or
    // before it and the next one.
    let (instants, times, fps) = timewarp_instants(ctx);
    let method = ctx.params.e("method");
    let one = method > 0 && ctx.params.b("tuning/buildFromOneImage");
    // The source with its masks, without the effects above Timewarp.
    let fr = fetch_with(ctx, &times, 0)?;
    let pm = PixelMotion::from(ctx, fr.grid.scale);
    let mattes = fetch_layer(ctx, "matteLayer", &times, &fr.grid);
    let warps = fetch_layer(ctx, "warpLayer", &times, &fr.grid);
    let matte_ch = ctx.params.e("matteChannel");
    let show = ctx.params.e("show");
    let idx = |t: f64| times.iter().position(|q| (q - t).abs() < 1e-9).unwrap_or(0);
    let mut frames = fr.imgs;
    let add = |frames: &mut Vec<Image>, img: Image| {
        frames.push(img);
        frames.len() - 1
    };
    // Motion fields: per source frame pair (keyed), or per masked pair (unkeyed).
    let mut flows: Vec<(Option<i64>, Flow)> = vec![];
    let mut steps = vec![];
    for &(s, wgt) in &instants {
        let (f0, frac) = tw_split(s, fps);
        let (i0, i1) = (idx(f0 / fps), idx((f0 + 1.0) / fps));
        let ma = mattes.as_ref().map(|m| matte_of(&m[i0], matte_ch));
        let mb = mattes.as_ref().map(|m| matte_of(&m[i1], matte_ch));
        let step = if method == 0 || frac < 1e-4 || (one && method == 1) {
            // Whole frames (Build From One Image with Frame Mix: the nearest frame).
            let pick = if method == 0 || frac < 1e-4 { 0.0 } else { frac.round() };
            let i = if pick < 0.5 { i0 } else { i1 };
            let base = &frames[i];
            let derived = match (show, &ma, &mb) {
                (1, Some(m), _) if pick < 0.5 => Some(times_mask(base, m, false)),
                (2, Some(m), _) if pick < 0.5 => Some(times_mask(base, m, true)),
                (1, _, Some(m)) => Some(times_mask(base, m, false)),
                (2, _, Some(m)) => Some(times_mask(base, m, true)),
                (3, Some(m), _) => Some(matte_image(if pick < 0.5 { m } else { mb.as_ref().unwrap_or(m) }, base)),
                _ => None,
            };
            TwStep::Whole(match derived {
                Some(img) => add(&mut frames, img),
                None => i,
            })
        } else if method == 1 {
            match (&ma, &mb, show) {
                (Some(m0), Some(m1), 3) => {
                    let (a, b) = (matte_image(m0, &frames[i0]), matte_image(m1, &frames[i1]));
                    TwStep::Mix(add(&mut frames, a), add(&mut frames, b), frac)
                }
                (Some(m0), Some(m1), 1 | 2) => {
                    let (a, b) = (times_mask(&frames[i0], m0, show == 2), times_mask(&frames[i1], m1, show == 2));
                    TwStep::Mix(add(&mut frames, a), add(&mut frames, b), frac)
                }
                _ => TwStep::Mix(i0, i1, frac),
            }
        } else {
            // Pixel Motion: vectors from the warp layer if any, else from the layer.
            let key = f0 as i64;
            let flow = match flows.iter().position(|(k, _)| *k == Some(key)) {
                Some(k) => k,
                None => {
                    let f = match &warps {
                        Some(w) => timewarp_flow(&w[i0], &w[i1], &pm),
                        None => timewarp_flow(&frames[i0], &frames[i1], &pm),
                    };
                    flows.push((Some(key), f));
                    flows.len() - 1
                }
            };
            match (&ma, &mb) {
                (Some(m0), Some(m1)) => {
                    // Foreground and background each move on their own vectors.
                    let (fa, fb) = (times_mask(&frames[i0], m0, false), times_mask(&frames[i1], m1, false));
                    let (ga, gb) = (times_mask(&frames[i0], m0, true), times_mask(&frames[i1], m1, true));
                    let mut own = |a: &Image, b: &Image| {
                        if warps.is_some() {
                            return flow;
                        }
                        flows.push((None, timewarp_flow(a, b, &pm)));
                        flows.len() - 1
                    };
                    let (fg_flow, bg_flow) = (own(&fa, &fb), own(&ga, &gb));
                    let fg = [add(&mut frames, fa), add(&mut frames, fb), fg_flow];
                    let bg = [add(&mut frames, ga), add(&mut frames, gb), bg_flow];
                    TwStep::Layered { fg, bg, w: frac, show }
                }
                _ => TwStep::Motion { a: i0, b: i1, w: frac, flow },
            }
        };
        steps.push((step, wgt));
    }
    Some(TimewarpPlan { grid: fr.grid, frames, steps, flows: flows.into_iter().map(|(_, f)| f).collect(), pm })
}

/// Timewarp. Whole Frames shows the nearest source frame, Frame Mix cross-fades the two
/// neighbouring frames and Pixel Motion builds the in-between frame along estimated motion
/// vectors (block-matching optical flow, [`effectcraft_raster::flow`]) tuned by the Tuning
/// group. A Matte Layer splits foreground and background so each moves on its own vectors; a
/// Warp Layer supplies the motion instead of the layer itself. Motion Blur averages Shutter
/// Samples instants across the shutter.
fn timewarp(ctx: &EffectCtx, b: Buf) -> Buf {
    match timewarp_plan(ctx) {
        Some(plan) => plan.render_cpu(),
        None => b,
    }
}

/// Pixel Motion with a Matte Layer: the foreground and background frames built on their own
/// vectors, shown as Show asks (0 Normal: foreground over background).
fn layered(fg: Image, bg: Image, show: u32) -> Image {
    match show {
        1 => fg,
        2 => bg,
        3 => matte_image(&fg.data.iter().map(|p| p[3]).collect::<Vec<_>>(), &fg),
        _ => {
            let mut o = bg;
            o.data.par_iter_mut().zip(fg.data.par_iter()).for_each(|(p, q)| {
                let k = 1.0 - q[3];
                for c in 0..4 {
                    p[c] = q[c] + p[c] * k;
                }
            });
            o
        }
    }
}

/// A matte shown as opaque grey.
fn matte_image(m: &[f32], like: &Image) -> Image {
    let mut out = Image::new(like.width, like.height);
    out.data.par_iter_mut().zip(m.par_iter()).for_each(|(p, &k)| *p = [k, k, k, 1.0]);
    out
}

// ---------------------------------------------------------------- CC Force Motion Blur / CC Wide Time / Pixel Motion Blur

fn force_motion_blur(ctx: &EffectCtx, b: Buf) -> Buf {
    let n = ctx.params.f("motionBlurLevels").round().clamp(2.0, 64.0) as usize;
    let angle = if ctx.params.b("overrideShutterAngle") { ctx.params.f("shutterAngle") } else { 180.0 };
    let span = angle.clamp(0.0, 3600.0) / 360.0 / ctx.fps();
    let times: Vec<f64> = (0..n).map(|i| ctx.time + span * i as f64 / n as f64).collect();
    mean_of(ctx, b, &times)
}

fn wide_time(ctx: &EffectCtx, b: Buf) -> Buf {
    let fwd = ctx.params.f("forwardSteps").round().clamp(0.0, 45.0) as i64;
    let back = ctx.params.f("backwardSteps").round().clamp(0.0, 45.0) as i64;
    let fd = 1.0 / ctx.fps();
    let times: Vec<f64> = (-back..=fwd).map(|k| ctx.time + k as f64 * fd).collect();
    mean_of(ctx, b, &times)
}

fn shutter_times(ctx: &EffectCtx) -> Vec<f64> {
    let angle = if ctx.params.e("shutterControl") == 1 { ctx.params.f("shutterAngle") } else { 180.0 };
    let n = ctx.params.f("shutterSamples").round().clamp(1.0, 64.0) as usize;
    let span = angle.clamp(0.0, 720.0) / 360.0 / ctx.fps();
    (0..n).map(|i| ctx.time - span * 0.5 + if n > 1 { span * i as f64 / (n - 1) as f64 } else { span * 0.5 }).collect()
}

fn pixel_motion_blur(ctx: &EffectCtx, b: Buf) -> Buf {
    match pmb_plan(ctx).as_ref().map(TimewarpPlan::render_cpu) {
        Some(blur) => blur,
        None => mean_of(ctx, b, &shutter_times(ctx)),
    }
}

/// Pixel Motion Blur as a [`TimewarpPlan`]: each shutter instant is decoded between the two
/// source frames it falls between along a measured motion field (the same synthesis Timewarp
/// uses), and the instants average into the blur. Vector Detail sets the spacing of the motion
/// vectors, so a coarse grid follows one big motion and a fine grid separates neighbouring
/// ones (#535). `None` without a host — the caller then falls back. Shared by the CPU effect
/// and the GPU compositor (`fx_time`), which renders the plan like Timewarp's.
pub fn pmb_plan(ctx: &EffectCtx) -> Option<TimewarpPlan> {
    ctx.env.host?;
    let fps = ctx.fps();
    let samples = shutter_times(ctx);
    let n = samples.len();
    let mut times = Vec::new();
    for &s in &samples {
        let (f0, _) = tw_split(s, fps);
        times.push(f0 / fps);
        times.push((f0 + 1.0) / fps);
    }
    let fr = fetch(ctx, &times)?;
    let idx = |t: f64| times.iter().position(|q| (q - t).abs() < 1e-9).unwrap_or(0);
    let mut pm = PixelMotion::from(ctx, fr.grid.scale);
    // PMB's own Vector Detail: 0 → one vector per 32 px, 100 → one per 2 px.
    let detail = (ctx.params.f("vectorDetail") / 100.0).clamp(0.0, 1.0);
    pm.tile = (32.0 * (1.0f64 / 16.0).powf(detail) * fr.grid.scale).round().max(2.0) as u32;
    let mut flows: Vec<FlowEntry> = vec![];
    let mut steps = vec![];
    let w = 1.0 / n as f32;
    for &s in &samples {
        let (f0, frac) = tw_split(s, fps);
        let (i0, i1) = (idx(f0 / fps), idx((f0 + 1.0) / fps));
        let step = if frac < 1e-4 {
            TwStep::Whole(i0)
        } else {
            let flow = match flows.iter().position(|fl| fl.key == f0 as i64) {
                Some(p) => p,
                None => {
                    let reach = (fr.grid.img.width.max(fr.grid.img.height) / 4).clamp(8, 64);
                    flows.push(FlowEntry { key: f0 as i64, flow: timewarp_flow_search(&fr.imgs[i0], &fr.imgs[i1], &pm, reach as i32) });
                    flows.len() - 1
                }
            };
            TwStep::Motion { a: i0, b: i1, w: frac, flow }
        };
        steps.push((step, w));
    }
    Some(TimewarpPlan { grid: fr.grid, frames: fr.imgs, steps, flows: flows.into_iter().map(|f| f.flow).collect(), pm })
}

/// A measured flow with the source frame pair it came from (plans cache one per pair).
struct FlowEntry {
    key: i64,
    flow: Flow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels, run_fx};

    /// A layer whose every pixel at layer time `t` is grey `t` (clamped), alpha 1, on a 6×4 grid.
    struct Clock {
        static_layer: bool,
    }
    impl EffectHost for Clock {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, effects: usize) -> Option<Buf> {
            // Time effects see every effect before them; Timewarp only the source and masks.
            assert!(effects == usize::MAX || effects == 0, "{effects}");
            let v = if self.static_layer { 0.4 } else { t as f32 };
            Some(Buf { img: Image::filled(6, 4, [v, v, v, 1.0]), offset: [0.0; 2], scale: 1.0 })
        }
    }

    fn run(id: &str, vals: &[(&str, Value)], t: f64, host: &Clock) -> Buf {
        let env = EffectEnv { host: Some(host), frame_rate: 10.0, comp_time: t, ..Default::default() };
        run_fx(id, vals, Image::filled(6, 4, [0.9, 0.0, 0.0, 1.0]), t, env)
    }

    /// After Effects 2026: Timewarp retimes the layer's source (with masks) and ignores the
    /// effects above it; Echo keeps them.
    #[test]
    fn timewarp_reads_the_source_without_the_effects_above_it() {
        struct Asked(std::sync::Mutex<Vec<usize>>);
        impl EffectHost for Asked {
            fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
                None
            }
            fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
                None
            }
            fn self_at(&self, _: f64, effects: usize) -> Option<Buf> {
                self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(effects);
                // The source (effects 0) is red, the layer with its effects green.
                let c = if effects == 0 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0, 1.0] };
                Some(Buf { img: Image::filled(6, 4, c), offset: [0.0; 2], scale: 1.0 })
            }
        }
        let host = Asked(std::sync::Mutex::new(vec![]));
        let env = EffectEnv { host: Some(&host), frame_rate: 10.0, comp_time: 1.0, ..Default::default() };
        let out = run_fx("ec.time.timewarp", &[("speed", num(25.0))], Image::filled(6, 4, [0.0, 1.0, 0.0, 1.0]), 1.0, env);
        assert_eq!(out.img.get(2, 2), [1.0, 0.0, 0.0, 1.0]);
        assert!(host.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().all(|&e| e == 0));
    }

    #[test]
    fn posterize_time_holds_frames() {
        let h = Clock { static_layer: false };
        for (t, held) in [(0.0, 0.0), (0.1, 0.0), (0.24, 0.0), (0.25, 0.25), (0.49, 0.25), (0.5, 0.5)] {
            let out = run("ec.time.posterizetime", &[("frameRate", num(4.0))], t, &h);
            assert!((out.img.data[0][0] - held).abs() < 1e-5, "t={t}: {}", out.img.data[0][0]);
        }
    }

    #[test]
    fn echo_blends_known_frames_with_decay() {
        let h = Clock { static_layer: false };
        let vals = [("echoTime", num(-0.1)), ("numberOfEchoes", num(2.0)), ("startingIntensity", num(1.0)), ("decay", num(0.5))];
        let out = run("ec.time.echo", &vals, 0.8, &h);
        // Add: f(0.8) + 0.5 f(0.7) + 0.25 f(0.6)
        let want = 0.8 + 0.5 * 0.7 + 0.25 * 0.6;
        assert!((out.img.data[0][0] - want as f32).abs() < 1e-4, "{}", out.img.data[0][0]);
        assert_eq!(out.img.data[0][3], 1.0);
        // Maximum picks the brightest weighted frame; Blend averages.
        let mut v = vals.to_vec();
        v.push(("echoOperator", Value::Enum(1)));
        let out = run("ec.time.echo", &v, 0.8, &h);
        assert!((out.img.data[0][0] - 0.8).abs() < 1e-5);
        v.pop();
        v.push(("echoOperator", Value::Enum(6)));
        let out = run("ec.time.echo", &v, 0.8, &h);
        assert!((out.img.data[0][0] - want as f32 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn time_difference_of_static_layer_is_black() {
        let h = Clock { static_layer: true };
        let out = run("ec.time.timedifference", &[("timeOffset", num(-0.5))], 1.0, &h);
        assert!(out.img.data.iter().all(|p| p[0] == 0.0 && p[1] == 0.0 && p[2] == 0.0 && p[3] == 1.0));
        // A changing layer is not black: |1.0 - 0.5| = 0.5.
        let h = Clock { static_layer: false };
        let out = run("ec.time.timedifference", &[("timeOffset", num(-0.5))], 1.0, &h);
        assert!((out.img.data[0][0] - 0.5).abs() < 1e-5, "{}", out.img.data[0][0]);
    }

    /// Texture `x − dx` (smooth, non-repeating).
    fn texture(w: u32, h: u32, dx: f64) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f64 - dx, y as f64);
                let s = (0.5 + 0.2 * (u * 0.21 + v * 0.07).sin() + 0.15 * (u * 0.05 - v * 0.19).cos() + 0.1 * (u * 0.031 + v * 0.017).sin()) as f32;
                img.set(x, y, [s, s * 0.8, 1.0 - s, 1.0]);
            }
        }
        img
    }

    /// A textured layer moving right 3 px per frame (10 fps); layer 7 (a "warp" / matte layer)
    /// is the same texture standing still, layer 8 a matte that is white on the left half.
    struct Moving;
    impl EffectHost for Moving {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            Some(Buf { img: texture(96, 64, (t * 10.0).round() * 3.0), offset: [0.0; 2], scale: 1.0 })
        }
        fn layer_at(&self, id: u64, _: f64, _: bool) -> Option<LayerPixels> {
            let img = match id {
                7 => texture(96, 64, 0.0),
                _ => {
                    let mut m = Image::new(96, 64);
                    for y in 0..64 {
                        for x in 0..48 {
                            m.set(x, y, [1.0, 1.0, 1.0, 1.0]);
                        }
                    }
                    m
                }
            };
            Some(LayerPixels { buf: Buf { img, offset: [0.0; 2], scale: 1.0 }, size: [96.0, 64.0] })
        }
    }

    fn mae(a: &Image, b: &Image, margin: i64) -> f32 {
        let (w, h) = (a.width as i64, a.height as i64);
        let mut s = 0.0;
        let mut n = 0.0;
        for y in margin..h - margin {
            for x in margin..w - margin {
                let (p, q) = (a.get(x, y), b.get(x, y));
                s += (0..3).map(|c| (p[c] - q[c]).abs()).sum::<f32>();
                n += 3.0;
            }
        }
        s / n
    }

    fn run_moving(vals: &[(&str, Value)], t: f64) -> Image {
        let env = EffectEnv { host: Some(&Moving), frame_rate: 10.0, comp_time: t, ..Default::default() };
        run_fx("ec.time.timewarp", vals, Image::new(96, 64), t, env).img
    }

    #[test]
    fn timewarp_pixel_motion_follows_the_motion() {
        // Speed 50 % at layer time 1.1 s: source 0.55 s, halfway between frames 5 and 6 (15 and
        // 18 px), so the true in-between frame is the texture at 16.5 px.
        let want = texture(96, 64, 16.5);
        let pm = run_moving(&[("method", Value::Enum(2)), ("tuning/vectorDetail", num(60.0))], 1.1);
        let mix = run_moving(&[("method", Value::Enum(1))], 1.1);
        let (e_pm, e_mix) = (mae(&pm, &want, 8), mae(&mix, &want, 8));
        assert!(e_pm < e_mix * 0.5, "pixel motion {e_pm} vs frame mix {e_mix}");
        // Deterministic and seek-consistent (same time → same pixels).
        assert_eq!(pm.data, run_moving(&[("method", Value::Enum(2)), ("tuning/vectorDetail", num(60.0))], 1.1).data);
        // Build From One Image warps the nearer frame alone: still close to the truth.
        let one = run_moving(&[("method", Value::Enum(2)), ("tuning/buildFromOneImage", Value::Bool(true)), ("tuning/vectorDetail", num(60.0))], 1.1);
        assert!(mae(&one, &want, 8) < e_mix * 0.5);
        // A Warp Layer standing still supplies zero motion: the result is a plain mix.
        let warped = run_moving(&[("method", Value::Enum(2)), ("warpLayer", Value::Layer(Some(7))), ("tuning/errorThreshold", num(100.0))], 1.1);
        assert!(mae(&warped, &mix, 0) < 1e-3, "{}", mae(&warped, &mix, 0));
        // Show Matte displays the matte layer; Foreground Only keeps the matte's left half.
        let m = run_moving(&[("method", Value::Enum(2)), ("matteLayer", Value::Layer(Some(8))), ("show", Value::Enum(3))], 1.1);
        assert!(m.get(10, 10)[0] > 0.99 && m.get(80, 10)[0] < 0.01);
        let fg = run_moving(&[("method", Value::Enum(2)), ("matteLayer", Value::Layer(Some(8))), ("show", Value::Enum(1))], 1.1);
        assert!(fg.get(10, 30)[3] > 0.99 && fg.get(85, 30)[3] < 0.01);
        let normal = run_moving(&[("method", Value::Enum(2)), ("matteLayer", Value::Layer(Some(8))), ("tuning/vectorDetail", num(60.0))], 1.1);
        assert!(mae(&normal, &want, 12) < e_mix, "{} {e_mix}", mae(&normal, &want, 12));
        // Source Crops: a crop replicates the kept edge into the cropped band.
        let c = run_moving(&[("method", Value::Enum(2)), ("sourceCrops/leftCrop", num(10.0))], 1.1);
        assert_eq!(c.get(0, 20), c.get(5, 20));
    }

    #[test]
    fn timewarp_and_wide_time() {
        let h = Clock { static_layer: false };
        // Speed 50 % at t = 1 s shows source 0.5 s (frame-aligned at 10 fps).
        let out = run("ec.time.timewarp", &[("method", Value::Enum(0))], 1.0, &h);
        assert!((out.img.data[0][0] - 0.5).abs() < 1e-5);
        // Frame Mix between 0.5 and 0.6 at source 0.55.
        let out = run("ec.time.timewarp", &[("method", Value::Enum(1))], 1.1, &h);
        assert!((out.img.data[0][0] - 0.55).abs() < 1e-4, "{}", out.img.data[0][0]);
        // Build From One Image: the nearest source frame alone (0.57 → 0.6).
        let out = run("ec.time.timewarp", &[("method", Value::Enum(1)), ("tuning/buildFromOneImage", Value::Bool(true))], 1.14, &h);
        assert!((out.img.data[0][0] - 0.6).abs() < 1e-4, "{}", out.img.data[0][0]);
        // Motion blur (in its twirl-down) averages over the shutter.
        let out = run("ec.time.timewarp", &[("method", Value::Enum(1)), ("motionBlur/enableMotionBlur", Value::Bool(true))], 1.0, &h);
        assert!(out.img.data[0][0] > 0.5 + 1e-3, "{}", out.img.data[0][0]);
        // Source Frame mode: frame 3 at 10 fps.
        let out = run("ec.time.timewarp", &[("method", Value::Enum(0)), ("adjustTimeBy", Value::Enum(1)), ("sourceFrame", num(3.0))], 2.0, &h);
        assert!((out.img.data[0][0] - 0.3).abs() < 1e-5);
        // Wide Time is symmetric around t: mean of t-0.3..t+0.3 = t.
        let out = run("ec.time.ccwidetime", &[], 1.0, &h);
        assert!((out.img.data[0][0] - 1.0).abs() < 1e-4);
        // Pixel Motion Blur is centred too.
        let out = run("ec.time.pixelmotionblur", &[], 1.0, &h);
        assert!((out.img.data[0][0] - 1.0).abs() < 1e-4);
        // Force Motion Blur looks forward over the shutter.
        let out = run("ec.time.ccforcemotionblur", &[], 1.0, &h);
        assert!(out.img.data[0][0] > 1.0);
    }

    #[test]
    fn time_displacement_uses_luma_as_offset() {
        // Static map from a bright self frame: luminance 1.0 → +max seconds.
        let h = Clock { static_layer: true };
        let out = run("ec.time.timedisplacement", &[], 0.0, &h);
        assert!((out.img.data[0][0] - 0.4).abs() < 1e-5);
    }
}
