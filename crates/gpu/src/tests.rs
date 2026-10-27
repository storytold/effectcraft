//! GPU vs CPU: the CPU renderer is the oracle. Every scene renders on both and must agree
//! within ≤ 1/255 at 8 bpc and ≤ 1e-3 at 16/32 bpc (per channel, premultiplied; relative for
//! over-range 32 bpc values above 1).
//!
//! Two documented exceptions, both from float rounding (the GPU computes in f32 with fast math,
//! the CPU's transforms in f64):
//! * 8/16 bpc quantise after every layer and effect, so a value within ~1e-6 of a rounding
//!   boundary can land one step apart and later steps (Overlay's ×2 slope, colour-space curves)
//!   can widen it: up to 0.3 % of pixels may differ by at most 4/255.
//! * Discontinuous modes (Hard Mix, Dissolve, Darker/Lighter Color) may flip pixels sitting
//!   exactly on a threshold: up to 0.5 % of the frame.
//!
//! The tests skip (pass with a note) when no GPU adapter is available, as on headless CI.

use std::sync::{Arc, OnceLock};

use effectcraft_color::{BlendMode, ColorSpace, Label};
use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{
    AlphaMode, BitDepth, Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerSource, MaskMode, MatteKind, Project, Quality, Sampling, Solid, TrackMatte,
};
use effectcraft_raster::Image;
use effectcraft_render::{Backend, FootageSource, RenderOpts, Renderer};
use effectcraft_time::{FrameRate, Tick};

use crate::Gpu;

mod adjust;
mod adv3d;
mod deferred;
mod deferred_jobs;
mod fx_color;
mod fx_depth;
mod fx_distort;
mod fx_extra;
mod fx_gen2;
mod fx_generate;
mod fx_key;
mod fx_light;
mod fx_lut;
mod fx_noise;
mod fx_particles;
mod fx_pixel2;
mod fx_sim;
mod fx_stylize;
mod fx_text;
mod fx_time;
mod fx_tone;
mod fx_transition;
mod fx_vr;
mod fx_warp;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod init;
mod particles;
mod three_d;
mod wgsl_grammar;

/// One GPU test at a time in this process: the calling test thread holds a process-wide lock
/// until it exits (libtest runs every test on a thread of its own). wgpu's OpenGL backend (Mesa
/// llvmpipe on FreeBSD, Linux without Vulkan) has one context lock per adapter and panics when a
/// thread waits on it for more than a few seconds ("Could not lock adapter context"), and
/// concurrent devices on llvmpipe can crash. Every test helper that hands out a device calls this
/// first; calling it again on the same thread is a no-op.
pub(crate) fn hold_gpu_lock() {
    use std::cell::RefCell;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    static GPU_LOCK: Mutex<()> = Mutex::new(());
    thread_local! {
        static HELD: RefCell<Option<MutexGuard<'static, ()>>> = const { RefCell::new(None) };
    }
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        if h.is_none() {
            *h = Some(GPU_LOCK.lock().unwrap_or_else(PoisonError::into_inner));
        }
    });
}

/// The shared test device (the calling test holds [`hold_gpu_lock`]).
pub(crate) fn gpu() -> Option<&'static Gpu> {
    static G: OnceLock<Result<Gpu, String>> = OnceLock::new();
    hold_gpu_lock();
    let g = G.get_or_init(|| {
        // Readback mapping/allocation errors become None without necessarily retiring the
        // context. Preserve their original messages alongside the failure's health snapshot.
        struct GpuTestLog;
        impl log::Log for GpuTestLog {
            fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
                metadata.level() <= log::Level::Warn && metadata.target().starts_with("effectcraft_gpu")
            }

            fn log(&self, record: &log::Record<'_>) {
                if self.enabled(record.metadata()) {
                    eprintln!("{} {}: {}", record.level(), record.target(), record.args());
                }
            }

            fn flush(&self) {}
        }
        static LOGGER: GpuTestLog = GpuTestLog;
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Warn);
        }
        let g = Gpu::try_headless();
        match &g {
            Ok(g) => eprintln!("effectcraft-gpu tests: adapter {}, median {:?}", g.ctx.name, g.ctx.median_kernel),
            Err(e) => eprintln!("effectcraft-gpu tests: no GPU ({e}), skipping GPU comparisons"),
        }
        g
    });
    require_gpu("effectcraft-gpu tests", g.as_ref())
}

/// `g`, or `None` (the GPU tests skip) when there is no GPU, unless `EFFECTCRAFT_REQUIRE_GPU=1`
/// (as in `tests_init`): then a GPU that didn't start fails the test, with why.
pub(crate) fn require_gpu<G>(tests: &str, g: Result<G, &String>) -> Option<G> {
    match g {
        Ok(g) => Some(g),
        Err(e) if std::env::var("EFFECTCRAFT_REQUIRE_GPU").is_ok_and(|v| v == "1") => {
            panic!("{tests}: EFFECTCRAFT_REQUIRE_GPU=1 but GPU initialization failed: {e}")
        }
        Err(_) => None,
    }
}

/// The test device is on wgpu's OpenGL backend (Mesa's llvmpipe on the FreeBSD CI job).
pub(crate) fn on_gl() -> bool {
    gpu().is_some_and(|g| g.ctx.name.ends_with("(Gl)"))
}

/// The test device is Mesa's software rasteriser, llvmpipe: on OpenGL, or as lavapipe on Vulkan
/// (the Linux CI gate).
pub(crate) fn on_llvmpipe() -> bool {
    gpu().is_some_and(|g| g.ctx.name.starts_with("llvmpipe"))
}

thread_local! {
    static LLVMPIPE_FLIPS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Until the guard drops, comparisons on lavapipe get [`GL_BOUNDARY_FLIPS`] too. Only the tests
/// that measured such flips there opt in (CC HexTile cell edges, CC Threshold RGB thresholds);
/// every other comparison on the Linux gate stays exact.
pub(crate) fn allow_llvmpipe_boundary_flips() -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            LLVMPIPE_FLIPS.with(|f| f.set(false));
        }
    }
    LLVMPIPE_FLIPS.with(|f| f.set(true));
    Reset
}

/// The boundary-flip allowance applies: OpenGL, or llvmpipe in a test that opted in.
fn boundary_flips_allowed() -> bool {
    on_gl() || (LLVMPIPE_FLIPS.with(std::cell::Cell::get) && on_llvmpipe())
}

/// On GL, the fraction of pixels allowed past the tolerance: pixels on a decision boundary (a
/// hex cell's edge, a threshold, a corner pin's coverage edge, a median's rank tie) may land on
/// the other side, since GL drivers may divide through a reciprocal and GLSL's `fma` need not be
/// fused (so `fxs_qdiv`-style corrections don't apply). Measured on llvmpipe: at most 0.53 % (CC
/// HexTile at half resolution, many cell edges in a small frame). A wrong wrap (signed `%`) showed
/// in 3–70 % of pixels and still fails; a precision loss may show in fewer (Mesa's `asin`: 0.3–3 %),
/// so the shaders avoid imprecise builtins themselves (`asin_p`) rather than lean on this.
pub(crate) const GL_BOUNDARY_FLIPS: f64 = 0.01;

/// Procedural footage: smooth colour gradients, fine noise and an alpha with soft, opaque and
/// fully transparent regions (different per item).
pub(crate) struct Pattern;

pub(crate) fn pattern(seed: u32, w: u32, h: u32) -> Image {
    let mut img = Image::new(w, h);
    let k = seed as f32 * 0.37;
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let n = effectcraft_raster::hash_noise(x, y, seed) * 0.25;
            let r = (0.5 + 0.5 * (u * 7.0 + k).sin()) * 0.75 + n;
            let g = (0.5 + 0.5 * (v * 5.0 - k).cos()) * 0.75 + n * 0.5;
            let b = ((u + v) * 0.5 + k).fract() * 0.8 + 0.1;
            let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt();
            let a = ((0.48 - d) * 6.0).clamp(0.0, 1.0) * if (x / 9 + y / 7 + seed).is_multiple_of(5) { 0.6 } else { 1.0 };
            img.set(x, y, [r.min(1.0) * a, g.min(1.0) * a, b * a, a]);
        }
    }
    img
}

impl FootageSource for Pattern {
    fn frame(&self, item: ItemId, f: &Footage, _t: Tick) -> Option<Arc<Image>> {
        Some(Arc::new(pattern(item.0 as u32, f.width, f.height)))
    }
}

pub(crate) struct Scene {
    pub(crate) p: Project,
    pub(crate) cid: ItemId,
    pub(crate) comp: Comp,
}

impl Scene {
    pub(crate) fn new(depth: BitDepth) -> Scene {
        let mut p = Project::default();
        p.settings.bit_depth = depth;
        let comp = Comp::new(97, 61, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
        Scene { p, cid, comp }
    }

    pub(crate) fn footage(&mut self, w: u32, h: u32) -> Layer {
        let f = Footage {
            path: "pattern.png".into(),
            kind: FootageKind::Still,
            width: w,
            height: h,
            pixel_aspect: 1.0,
            frame_rate: FrameRate::FPS_30,
            native_rate: None,
            duration: Tick::ZERO,
            has_video: true,
            has_audio: false,
            alpha: AlphaMode::Premultiplied,
            premul_color: [0.0; 3],
            loop_count: 1,
            codec: String::new(),
            missing: false,
            sequence: vec![],
            color_profile: None,
            ..Default::default()
        };
        let fid = self.p.add_item("Pattern", Label::Lavender, None, ItemKind::Footage(f));
        build::layer(&mut self.p, &self.comp, "Pattern", LayerSource::Footage { item: fid }, (w, h), None)
    }

    pub(crate) fn solid(&mut self, color: [f32; 3], w: u32, h: u32) -> Layer {
        let sid = self.p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color, width: w, height: h, pixel_aspect: 1.0 }));
        build::layer(&mut self.p, &self.comp, "Solid", LayerSource::Solid { item: sid }, (w, h), None)
    }

    /// Add on top of the stack.
    pub(crate) fn push(&mut self, l: Layer) -> effectcraft_project::LayerId {
        let id = l.id;
        self.p.comp_mut(self.cid).unwrap().layers.insert(0, l);
        id
    }

    pub(crate) fn effect(&mut self, l: &mut Layer, id: &str, vals: &[(&str, Value)]) {
        let spec = effectcraft_effects::find(id).unwrap();
        let mut next = self.p.next_id;
        let size = effectcraft_render::effect_bounds(&self.p, &self.comp, l).0;
        let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, size);
        self.p.next_id = next;
        for (k, v) in vals {
            g.prop_mut(k).unwrap_or_else(|| panic!("{id}: no {k}")).value = v.clone();
        }
        l.props.sub_mut("effects").unwrap().children.push(g.into());
    }
}

pub(crate) fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).value = v;
}

pub(crate) fn v3(x: f64, y: f64) -> Value {
    Value::Vec3([x, y, 0.0])
}

/// Differences of a GPU render against the CPU render.
#[derive(Debug)]
pub(crate) struct Diff {
    pub(crate) max: f32,
    /// Pixels over the tolerance.
    pub(crate) over: usize,
    pub(crate) total: usize,
    pub(crate) worst: (u32, u32, [f32; 4], [f32; 4]),
    /// 8/16 bpc render.
    pub(crate) quantized: bool,
}

pub(crate) fn diff(cpu: &Image, gpu: &Image, tol: f32) -> Diff {
    assert_eq!((cpu.width, cpu.height), (gpu.width, gpu.height), "size");
    let mut d = Diff { max: 0.0, over: 0, total: cpu.data.len(), worst: (0, 0, [0.0; 4], [0.0; 4]), quantized: false };
    for (i, (a, b)) in cpu.data.iter().zip(&gpu.data).enumerate() {
        // Absolute below 1, relative above (32 bpc over-range values, e.g. Divide by ~0).
        // A NaN on one side only (e.g. a kernel that skipped pixels of a NaN-poisoned scratch
        // image) is the largest difference.
        let m = (0..4)
            .map(|c| match (a[c].is_nan(), b[c].is_nan()) {
                (true, true) => 0.0,
                (false, false) => (a[c] - b[c]).abs() / a[c].abs().max(1.0),
                _ => f32::INFINITY,
            })
            .fold(0.0f32, f32::max);
        if m > tol {
            d.over += 1;
        }
        if m > d.max {
            d.max = m;
            d.worst = (i as u32 % cpu.width, i as u32 / cpu.width, *a, *b);
        }
    }
    d
}

pub(crate) fn tolerance(depth: BitDepth) -> f32 {
    match depth {
        BitDepth::Bpc8 => 1.0 / 255.0 + 1e-6,
        _ => 1e-3,
    }
}

/// Render on the CPU and the GPU; `Some(diff)` (None without an adapter).
pub(crate) fn compare_at(s: &Scene, opts: RenderOpts, t: Tick) -> Option<Diff> {
    let g = gpu()?;
    let cpu = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, ..opts }).comp_frame_cpu(s.cid, t);
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts });
    r.accel = Some(g);
    let img = g.render(&r, s.cid, t).expect("the GPU renders the frame");
    let mut d = diff(&cpu, &img, tolerance(s.p.settings.bit_depth));
    d.quantized = s.p.settings.bit_depth != BitDepth::Bpc32;
    Some(d)
}

pub(crate) fn opts() -> RenderOpts {
    RenderOpts { motion_blur: true, ..Default::default() }
}

/// Assert agreement; `allow` = fraction of pixels allowed over the tolerance.
pub(crate) fn check(label: &str, d: Option<Diff>, allow: f64) {
    let Some(d) = d else { return };
    let frac = d.over as f64 / d.total as f64;
    // Quantised depths: rounding-boundary flips (see the module docs).
    let (allow, cap) = if d.quantized && allow == 0.0 { (0.003, 4.0 / 255.0 + 1e-6) } else { (allow, f32::INFINITY) };
    let (allow, cap) = if boundary_flips_allowed() { (allow.max(GL_BOUNDARY_FLIPS), f32::INFINITY) } else { (allow, cap) };
    assert!(frac <= allow && d.max <= cap, "{label}: {} of {} pixels over tolerance (max {:.6}); worst {:?}", d.over, d.total, d.max, d.worst);
    if d.over > 0 {
        eprintln!("{label}: {} of {} pixels over tolerance (max {:.6})", d.over, d.total, d.max);
    }
}

fn discontinuous(m: BlendMode) -> bool {
    matches!(m, BlendMode::HardMix | BlendMode::Dissolve | BlendMode::DancingDissolve | BlendMode::DarkerColor | BlendMode::LighterColor)
}

/// Background, an Add layer (over-range values in 32 bpc) and a rotated, scaled top layer in
/// `mode` at 70 % opacity.
fn blend_scene(depth: BitDepth, mode: BlendMode) -> Scene {
    let mut s = Scene::new(depth);
    let bg = s.footage(97, 61);
    s.push(bg);
    let mut add = s.footage(80, 50);
    add.blend_mode = BlendMode::Add;
    set(&mut add, "transform/opacity", Value::Scalar(50.0));
    s.push(add);
    let mut top = s.footage(64, 48);
    top.blend_mode = mode;
    set(&mut top, "transform/opacity", Value::Scalar(70.0));
    set(&mut top, "transform/rotation", Value::Scalar(17.0));
    set(&mut top, "transform/scale", Value::Vec3([115.0, 90.0, 100.0]));
    set(&mut top, "transform/position", v3(52.3, 29.6));
    s.push(top);
    s
}

#[test]
fn all_blend_modes_at_every_bit_depth() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc16, BitDepth::Bpc32] {
        for mode in BlendMode::ALL {
            let s = blend_scene(depth, mode);
            let allow = if discontinuous(mode) { 0.005 } else { 0.0 };
            check(&format!("{mode:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), allow);
        }
    }
}

#[test]
fn transforms_sampling_and_resolution() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for (i, (sampling, quality, scale, rot, sc)) in [
            (Sampling::Bilinear, Quality::Best, 1.0, 0.0, 100.0),
            (Sampling::Bilinear, Quality::Best, 1.0, 33.0, 140.0),
            (Sampling::Bicubic, Quality::Best, 1.0, -21.0, 160.0),
            (Sampling::Bilinear, Quality::Best, 1.0, 12.0, 30.0),
            (Sampling::Bicubic, Quality::Best, 0.5, 45.0, 80.0),
            (Sampling::Bilinear, Quality::Draft, 1.0, 0.0, 200.0),
            (Sampling::Bilinear, Quality::Best, 0.25, 5.0, 100.0),
        ]
        .into_iter()
        .enumerate()
        {
            let mut s = Scene::new(depth);
            let bg = s.solid([0.1, 0.2, 0.3], 97, 61);
            s.push(bg);
            let mut l = s.footage(90, 70);
            l.switches.sampling = sampling;
            l.switches.quality = quality;
            set(&mut l, "transform/rotation", Value::Scalar(rot));
            set(&mut l, "transform/scale", Value::Vec3([sc, sc * 0.9, 100.0]));
            set(&mut l, "transform/position", v3(47.25, 31.5));
            s.push(l);
            check(&format!("transform #{i} {depth:?}"), compare_at(&s, RenderOpts { scale, ..opts() }, Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn masks_mattes_and_preserve_transparency() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        for kind in [MatteKind::Alpha, MatteKind::AlphaInverted, MatteKind::Luma, MatteKind::LumaInverted] {
            let mut s = Scene::new(depth);
            let bg = s.footage(97, 61);
            s.push(bg);
            let mut matte = s.footage(60, 40);
            matte.switches.video = false;
            set(&mut matte, "transform/rotation", Value::Scalar(-12.0));
            let mut next = s.p.next_id;
            let m = build::mask(&mut Ids(&mut next), "Mask 1", ShapePath::ellipse([30.0, 20.0], 50.0, 34.0), MaskMode::Add, [255, 255, 0]);
            s.p.next_id = next;
            matte.props.sub_mut("masks").unwrap().children.push(m.into());
            let mid = s.push(matte);
            let mut fill = s.footage(97, 61);
            fill.track_matte = Some(TrackMatte { layer: mid, kind });
            fill.blend_mode = BlendMode::Screen;
            s.push(fill);
            let mut pt = s.solid([0.9, 0.4, 0.1], 50, 30);
            pt.preserve_transparency = true;
            set(&mut pt, "transform/opacity", Value::Scalar(60.0));
            s.push(pt);
            check(&format!("matte {kind:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn motion_blur_accumulates_like_the_cpu() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        s.p.comp_mut(s.cid).unwrap().enable_motion_blur = true;
        s.comp.enable_motion_blur = true;
        let bg = s.solid([0.05, 0.05, 0.1], 97, 61);
        s.push(bg);
        let mut l = s.footage(30, 30);
        l.switches.motion_blur = true;
        l.blend_mode = BlendMode::Add;
        let pos = l.props.prop_mut("transform/position").unwrap();
        pos.keys = vec![Keyframe::new(Tick::ZERO, v3(10.0, 20.0)), Keyframe::new(Tick::from_seconds_f64(0.5), v3(90.0, 45.0))];
        let rot = l.props.prop_mut("transform/rotation").unwrap();
        rot.keys = vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(0.5), Value::Scalar(180.0))];
        s.push(l);
        check(&format!("motion blur {depth:?}"), compare_at(&s, opts(), Tick::from_seconds_f64(0.2)), 0.0);
    }
}

#[test]
fn colour_management_and_linear_blending() {
    for (ws, linearize, blend_linear) in
        [(None, false, true), (Some(ColorSpace::Rec2020), false, false), (Some(ColorSpace::Srgb), true, false), (Some(ColorSpace::Rec709), false, true)]
    {
        for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
            let mut s = blend_scene(depth, BlendMode::Overlay);
            s.p.settings.working_space = ws;
            s.p.settings.linearize = linearize;
            s.p.settings.blend_linear = blend_linear;
            check(&format!("colour {ws:?} {linearize} {blend_linear} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
        }
    }
}

#[test]
fn layer_styles_collapsed_precomps_and_cpu_fallbacks() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        // Layer styles (drop shadow pass + stroke body).
        let mut s = Scene::new(depth);
        let bg = s.footage(97, 61);
        s.push(bg);
        let mut l = s.solid([0.8, 0.3, 0.2], 40, 30);
        let mut next = s.p.next_id;
        effectcraft_project::styles::add_style(&mut l, &mut Ids(&mut next), "dropShadow", &s.comp.global_light, true).unwrap();
        effectcraft_project::styles::add_style(&mut l, &mut Ids(&mut next), "stroke", &s.comp.global_light, true).unwrap();
        s.p.next_id = next;
        l.blend_mode = BlendMode::Multiply;
        set(&mut l, "transform/rotation", Value::Scalar(10.0));
        s.push(l);
        check(&format!("styles {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);

        // A collapsed precomp (nested blend modes act on the parent) and an adjustment layer
        // (CPU fallback between GPU steps).
        let mut s = Scene::new(depth);
        let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
        let (a, b) = {
            let mut a = s.footage(60, 40);
            a.blend_mode = BlendMode::Normal;
            let mut b = s.solid([0.2, 0.7, 0.4], 30, 20);
            b.blend_mode = BlendMode::Difference;
            (a, b)
        };
        s.p.comp_mut(inner_id).unwrap().layers = vec![b, a];
        let bg = s.footage(97, 61);
        s.push(bg);
        let mut pre = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
        pre.switches.collapse = true;
        set(&mut pre, "transform/rotation", Value::Scalar(25.0));
        set(&mut pre, "transform/opacity", Value::Scalar(80.0));
        s.push(pre);
        let mut adj = s.solid([1.0, 1.0, 1.0], 50, 61);
        adj.switches.adjustment = true;
        s.effect(&mut adj, "ec.channel.invert", &[]);
        s.push(adj);
        check(&format!("collapse + adjustment {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    }
}

/// The effect alone on a buffer: GPU kernels vs the CPU effect (32-bit float), including the
/// buffer geometry (padding, offset).
pub(crate) fn effect_direct(id: &str, vals: &[(&str, Value)], adjustment: bool) {
    let Some(g) = gpu() else { return };
    let spec = effectcraft_effects::find(id).unwrap();
    let size = [70.0, 44.0];
    let mut params =
        effectcraft_effects::Params { values: spec.params.iter().map(|ps| (ps.id.to_string(), effectcraft_effects::default_value(ps, size))).collect() };
    for (k, v) in vals {
        params.values.insert(k.to_string(), v.clone());
    }
    let expect_empty = id == "ec.distort.ccblobbylize" && vals.iter().any(|(k, v)| *k == "cutAway" && matches!(v, Value::Scalar(x) if *x == 100.0));
    for scale in [1.0, 0.5] {
        let img = pattern(7, (size[0] * scale) as u32, (size[1] * scale) as u32);
        let buf = effectcraft_effects::Buf { img, offset: [0.0, 0.0], scale };
        let ctx = || effectcraft_effects::EffectCtx { params: &params, time: 0.25, layer_size: size, seed: 11, adjustment, env: Default::default() };
        let cpu = (spec.render)(&ctx(), buf.clone());
        let started = std::time::Instant::now();
        let out = effectcraft_render::Accelerator::effects(g, &[effectcraft_render::FxStep { spec, ctx: ctx() }], &buf, None).unwrap_or_else(|| {
            panic!(
                "the GPU runs {id} {vals:?} adj {adjustment} scale {scale}; elapsed={:?}, health={:?}, unavailable={:?}, transfers={:?}",
                started.elapsed(),
                g.ctx.check_health(),
                g.ctx.unavailable_kernels(),
                g.ctx.transfer_stats()
            )
        });
        if expect_empty {
            assert!(out.img.data.iter().all(|p| p[3] == 0.0), "{id} {vals:?} adj {adjustment} scale {scale}: Cut Away 100 produced non-zero alpha");
        }
        assert_eq!((out.offset, out.scale), (cpu.offset, cpu.scale), "{id}: geometry");
        let d = diff(&cpu.img, &out.img, 1e-3);
        let allow = if boundary_flips_allowed() { (d.total as f64 * GL_BOUNDARY_FLIPS) as usize } else { 0 };
        assert!(
            d.over <= allow,
            "{id} {vals:?} adj {adjustment} scale {scale}: {} of {} pixels over 1e-3 (max {}); worst {:?}",
            d.over,
            d.total,
            d.max,
            d.worst
        );
    }
}

/// One footage layer with `fx` applied, composited over a background, at 8 and 32 bpc.
pub(crate) fn effect_case(id: &str, vals: &[(&str, Value)]) {
    effect_direct(id, vals, false);
    effect_direct(id, vals, true);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        s.effect(&mut l, id, vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {vals:?} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
        check(&format!("{id} {vals:?} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), 0.0);
    }
}

pub(crate) fn n(v: f64) -> Value {
    Value::Scalar(v)
}

pub(crate) fn c(r: f64, g: f64, b: f64) -> Value {
    Value::Color([r, g, b, 1.0])
}

#[test]
fn small_blurs_fall_back_to_the_cpu() {
    // #1939: small Gaussian sigmas and fractional box radii render on the CPU kernel.
    for (id, vals) in [("ec.blur.gaussian", [("blurriness", n(0.9))]), ("ec.blur.fastbox", [("radius", n(1.5))])] {
        let mut s = Scene::new(BitDepth::Bpc32);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        s.effect(&mut l, id, &vals);
        s.push(l);
        check(&format!("{id} {vals:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    }
}

#[test]
fn gpu_effects_match_cpu() {
    // Sigma 3 px and up and even radii, so the half-scale pass stays on the GPU too (#1939).
    effect_case("ec.blur.gaussian", &[("blurriness", n(14.0))]);
    effect_case("ec.blur.gaussian", &[("blurriness", n(12.0)), ("dimensions", Value::Enum(1)), ("repeatEdge", Value::Bool(true))]);
    effect_case("ec.blur.fastbox", &[("radius", n(6.0)), ("iterations", n(2.0))]);
    effect_case("ec.blur.fastbox", &[("radius", n(4.0)), ("dimensions", Value::Enum(2)), ("repeatEdge", Value::Bool(true))]);
    effect_case("ec.blur.directional", &[("direction", n(30.0)), ("length", n(12.0))]);
    effect_case("ec.stylize.glow", &[("threshold", n(40.0)), ("radius", n(14.0)), ("intensity", n(1.5))]);
    effect_case("ec.stylize.glow", &[("colors", Value::Enum(1)), ("operation", Value::Enum(1)), ("colorA", c(1.0, 0.8, 0.1))]);
    effect_case(
        "ec.stylize.glow",
        &[
            ("colors", Value::Enum(1)),
            ("colorLooping", Value::Enum(3)),
            ("colorLoops", n(2.5)),
            ("colorPhase", n(40.0)),
            ("abMidpoint", n(30.0)),
            ("glowDimensions", Value::Enum(1)),
        ],
    );
    effect_case("ec.stylize.glow", &[("based", Value::Enum(0)), ("operation", Value::Enum(2)), ("threshold", n(20.0))]);
    effect_case("ec.color.levels", &[("inBlack", n(0.1)), ("inWhite", n(0.8)), ("gamma", n(1.6)), ("outBlack", n(0.05))]);
    effect_case("ec.color.levels", &[("inWhite", n(0.5)), ("noClip", Value::Bool(true))]);
    effect_case("ec.color.levels", &[("inBlack", n(0.2)), ("inWhite", n(0.6)), ("clipToOutputBlack", Value::Enum(0))]);
    effect_case("ec.color.levels", &[("inBlack", n(0.2)), ("inWhite", n(0.6)), ("clipToOutputWhite", Value::Enum(0))]);
    effect_case(
        "ec.color.curves",
        &[("rgb", Value::Str("0,0 0.3,0.45 1,1".into())), ("red", Value::Str("0,0.1 1,0.9".into())), ("alpha", Value::Str("0,0 0.5,0.7 1,1".into()))],
    );
    effect_case("ec.color.huesaturation", &[("hue", n(70.0)), ("saturation", n(40.0)), ("lightness", n(-20.0))]);
    effect_case("ec.color.huesaturation", &[("colorize", Value::Bool(true)), ("colorizeHue", n(200.0)), ("colorizeLightness", n(15.0))]);
    effect_case("ec.color.tint", &[("black", c(0.1, 0.0, 0.3)), ("white", c(1.0, 0.9, 0.5)), ("amount", n(80.0))]);
    effect_case("ec.generate.fill", &[("color", c(0.2, 0.6, 1.0)), ("opacity", n(70.0))]);
    effect_case("ec.generate.fill", &[("invert", Value::Bool(true))]);
    effect_case("ec.generate.gradientramp", &[("start", Value::Vec2([5.0, 3.0])), ("end", Value::Vec2([60.0, 40.0])), ("blend", n(25.0))]);
    effect_case("ec.generate.gradientramp", &[("shape", Value::Enum(1)), ("scatter", n(40.0))]);
    effect_case("ec.noise.fractal", &[]);
    effect_case(
        "ec.noise.fractal",
        &[
            ("fractalType", Value::Enum(1)),
            ("complexity", n(4.5)),
            ("transform/rotation", n(30.0)),
            ("evolution", n(90.0)),
            ("transform/scale", n(40.0)),
            ("blend", n(30.0)),
        ],
    );
    effect_case(
        "ec.noise.fractal",
        &[
            ("fractalType", Value::Enum(2)),
            ("overflow", Value::Enum(1)),
            ("contrast", n(250.0)),
            ("transform/uniformScaling", Value::Bool(false)),
            ("transform/scaleWidth", n(60.0)),
            ("transform/scaleHeight", n(25.0)),
            ("subSettings/subInfluence", n(40.0)),
            ("subSettings/subScaling", n(70.0)),
            ("opacity", n(60.0)),
        ],
    );
    effect_case("ec.noise.fractal", &[("overflow", Value::Enum(2)), ("contrast", n(300.0)), ("blendingMode", Value::Enum(0)), ("opacity", n(50.0))]);
    effect_case("ec.perspective.dropshadow", &[("distance", n(6.0)), ("softness", n(8.0)), ("opacity", n(80.0))]);
    effect_case("ec.perspective.dropshadow", &[("shadowOnly", Value::Bool(true)), ("color", c(0.0, 0.2, 0.6))]);
    effect_case("ec.color.brightnesscontrast", &[("brightness", n(20.0)), ("contrast", n(35.0))]);
    effect_case("ec.color.brightnesscontrast", &[("brightness", n(-10.0)), ("contrast", n(-40.0))]);
    effect_case("ec.color.brightnesscontrast", &[("brightness", n(20.0)), ("contrast", n(35.0)), ("useLegacy", Value::Bool(true))]);
    effect_case("ec.color.exposure", &[("master/exposure", n(1.3)), ("master/offset", n(0.02)), ("master/gamma", n(1.2))]);
    effect_case("ec.color.exposure", &[("channels", Value::Enum(1)), ("red/redExposure", n(0.8)), ("green/greenOffset", n(-0.05)), ("blue/blueGamma", n(1.6))]);
    effect_case("ec.color.exposure", &[("master/exposure", n(0.5)), ("master/offset", n(-0.03)), ("bypassLinearLight", Value::Bool(true))]);
    effect_case("ec.channel.invert", &[]);
    effect_case("ec.channel.invert", &[("channel", Value::Enum(effectcraft_effects::INVERT_ALPHA)), ("blend", n(30.0))]);
    effect_case("ec.channel.invert", &[("channel", Value::Enum(2))]);
    effect_case("ec.distort.transform", &[("rotation", n(20.0)), ("scaleHeight", n(80.0)), ("skew", n(10.0)), ("opacity", n(70.0))]);
}

/// A chain of GPU effects (one upload, quantised between steps) and a non-GPU effect in the
/// middle (split into two GPU chains around the CPU effect).
#[test]
fn effect_chains_and_mixed_stacks() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let mut l = s.footage(80, 50);
        s.effect(&mut l, "ec.color.exposure", &[("master/exposure", n(0.7))]);
        s.effect(&mut l, "ec.blur.gaussian", &[("blurriness", n(5.0))]);
        s.effect(&mut l, "ec.stylize.ccplastic", &[]);
        s.effect(&mut l, "ec.stylize.glow", &[]);
        s.effect(&mut l, "ec.channel.invert", &[("blend", n(50.0))]);
        s.push(l);
        check(&format!("chain {depth:?}"), compare_at(&s, opts(), Tick::ZERO), 0.0);
    }
}

#[test]
fn registry_badges_match_the_gpu_implementation() {
    for s in effectcraft_effects::registry() {
        let implemented = crate::effects::supports(s.id);
        assert_eq!(s.gpu, implemented, "{}: registry gpu = {}, GPU implementation = {implemented}", s.id, s.gpu);
    }
}

#[test]
fn quantization_preserves_cpu_levels_and_half_step_neighbors() {
    let Some(g) = gpu() else { return };
    for levels in [255.0_f32, 32768.0] {
        let mut values = vec![0.0, 1.0];
        for i in 0..255 {
            let half = (i as f32 + 0.5) / levels;
            values.extend([half.next_down(), half, half.next_up()]);
        }
        let mut cpu = Image::new(values.len() as u32, 1);
        for (p, v) in cpu.data.iter_mut().zip(values) {
            *p = [v, v, v, 1.0];
        }
        let input = g.ctx.upload_image(&cpu).unwrap();
        effectcraft_render::color::quantize(&mut cpu, levels);
        let mut e = crate::context::Enc::new(&g.ctx);
        let out = crate::ops::quantize(&mut e, &input, levels);
        let bytes = e.read_texture(&out.texture, out.width, out.height, 16).unwrap();
        for (i, (want, got)) in cpu.data.iter().flatten().zip(bytes.as_chunks::<4>().0).enumerate() {
            let actual = f32::from_le_bytes(*got);
            assert_eq!(actual, *want, "levels {levels}, component {i}");
        }
    }
}
#[test]
fn backend_selection_and_display_frames() {
    let Some(g) = gpu() else { return };
    let s = blend_scene(BitDepth::Bpc8, BlendMode::Screen);
    // Auto follows the project's Mercury GPU / Software Only setting; Cpu never uses the GPU.
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Auto, ..opts() });
    r.accel = Some(g);
    assert!(r.active_accel().is_some());
    let mut p2 = s.p.clone();
    p2.settings.gpu_acceleration = false;
    let mut r2 = Renderer::new(&p2, &Pattern, RenderOpts { backend: Backend::Auto, ..opts() });
    r2.accel = Some(g);
    assert!(r2.active_accel().is_none());
    let mut r3 = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, ..opts() });
    r3.accel = Some(g);
    assert!(r3.active_accel().is_none());
    // The display texture holds what the CPU viewer path shows (8-bit premultiplied).
    let f = g.render_display(&r, s.cid, Tick::ZERO).expect("display frame");
    let bytes = g.read_display(&f).expect("readback");
    let cpu = Renderer::new(&s.p, &Pattern, opts()).comp_frame_cpu(s.cid, Tick::ZERO);
    for (p, q) in cpu.data.iter().zip(bytes.as_chunks::<4>().0.iter()) {
        let a = p[3].clamp(0.0, 1.0);
        let want = [p[0].clamp(0.0, a), p[1].clamp(0.0, a), p[2].clamp(0.0, a), a].map(|v| (v * 255.0 + 0.5) as u8);
        for k in 0..4 {
            assert!((want[k] as i32 - q[k] as i32).abs() <= 1, "{want:?} vs {q:?}");
        }
    }
}

/// The display texture carries a full mip chain, each level the 2 × 2 mean of the one above, so the
/// viewer minifies a frame without aliasing (#417).
#[test]
fn display_frames_are_mipmapped() {
    let Some(g) = gpu() else { return };
    let s = blend_scene(BitDepth::Bpc8, BlendMode::Screen);
    let mut r = Renderer::new(&s.p, &Pattern, opts());
    r.accel = Some(g);
    let f = g.render_display(&r, s.cid, Tick::ZERO).expect("display frame");
    assert_eq!(f.texture.mip_level_count(), crate::context::mip_levels(f.width, f.height));
    assert_eq!(f.texture.mip_level_count(), u32::BITS - f.width.max(f.height).leading_zeros());
    let base = g.read_display(&f).expect("readback");
    // Level 1, copied out to read it back.
    let (w1, h1) = (f.width / 2, f.height / 2);
    let tex = g.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w1, height: h1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut e = crate::context::Enc::new(&g.ctx);
    e.encoder().copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo { texture: &f.texture, mip_level: 1, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::Extent3d { width: w1, height: h1, depth_or_array_layers: 1 },
    );
    let level1 = e.read_texture(&tex, w1, h1, 4).expect("level 1");
    let at = |x: u32, y: u32, c: usize| base[((y * f.width + x) * 4) as usize + c] as f32;
    let mut varied = false;
    for y in 0..h1 {
        for x in 0..w1 {
            for c in 0..4 {
                let want = (at(2 * x, 2 * y, c) + at(2 * x + 1, 2 * y, c) + at(2 * x, 2 * y + 1, c) + at(2 * x + 1, 2 * y + 1, c)) / 4.0;
                let got = level1[((y * w1 + x) * 4) as usize + c] as f32;
                assert!((want - got).abs() <= 1.0, "({x}, {y}) channel {c}: {got} against the mean {want}");
                varied |= want != at(2 * x, 2 * y, c);
            }
        }
    }
    assert!(varied, "the scene has edges to average");
}

/// Browsers' WGSL compilers reject an f32 literal whose exact decimal value lies outside the f32
/// range (`3.40282347e38` rounds to f32::MAX in Rust but exceeds it), which invalidates the whole
/// module (every kernel); naga accepts it, so native runs can't catch it. Every float literal
/// with an exponent must be within the f32 range exactly.
#[test]
fn wgsl_float_literals_fit_f32() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap();
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let bytes = code.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let starts = bytes[i].is_ascii_digit() && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'.'));
                if !starts {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || ((bytes[j] == b'-' || bytes[j] == b'+') && matches!(bytes[j - 1], b'e' | b'E')))
                {
                    j += 1;
                }
                let lit = code[i..j].trim_end_matches('f');
                if !lit.starts_with("0x")
                    && lit.contains(['e', 'E'])
                    && let Ok(v) = lit.parse::<f64>()
                {
                    checked += 1;
                    assert!(v.abs() <= f32::MAX as f64, "{}:{}: {lit} is outside the f32 range", path.display(), n + 1);
                }
                i = j;
            }
        }
    }
    assert!(checked > 0);
}

/// A device created with WebGL2's limits (what egui-wgpu asks for on GL, as the desktop app on
/// FreeBSD gets): the compositor declines it instead of building pipelines that fail validation
/// (wgpu's default error handler panics here; release builds would only log it).
#[test]
fn devices_with_webgl2_limits_are_declined() {
    hold_gpu_lock();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
    let desc = wgpu::DeviceDescriptor { required_limits: wgpu::Limits::downlevel_webgl2_defaults(), ..Default::default() };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&desc)) else { return };
    let err = crate::context::GpuContext::new(&adapter, device, queue).err().expect("declined");
    // A GL adapter (FreeBSD's llvmpipe) is declined for its backend before its limits are checked.
    assert!(err.contains("max_storage_buffers_per_shader_stage") || err.contains("(Gl)"), "{err}");
}

/// GPU work inside the video memory guard returns its result when the device has room (#106).
#[test]
fn within_memory_returns_the_work_when_there_is_room() {
    let Some(g) = gpu() else { return };
    let img = g.within_memory(|| g.ctx.image(64, 32)).expect("room for a 64×32 texture");
    assert_eq!((img.width, img.height), (64, 32));
}

/// A buffer whose allocation fails (here: over the device's buffer size limit; on Adreno an
/// internal `ERROR_INITIALIZATION_FAILED`) is reported to the caller's error scope instead of
/// panicking in `create_buffer_init`'s mapped range on a frame thread (#591).
#[test]
fn a_failed_buffer_allocation_is_an_error_not_a_panic() {
    hold_gpu_lock();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return };
    let required_limits =
        wgpu::Limits { max_buffer_size: 4096, max_storage_buffer_binding_size: 4096, max_uniform_buffer_binding_size: 4096, ..Default::default() };
    let desc = wgpu::DeviceDescriptor { required_limits, ..Default::default() };
    let (device, queue) = pollster::block_on(adapter.request_device(&desc)).expect("a device with a small buffer limit");
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::context::buffer_init(&device, &queue, Some("too big"), &[0u8; 8192], wgpu::BufferUsages::STORAGE);
        crate::context::buffer_init(&device, &queue, Some("unaligned"), &[1u8; 7], wgpu::BufferUsages::UNIFORM);
    }));
    let error = pollster::block_on(scope.pop());
    assert!(made.is_ok(), "a failed allocation must not panic");
    assert!(error.is_some(), "the failed allocation reaches the error scope");
}

/// Precomp layers (not collapsed) render their nested comp resident on the GPU
/// (`walk::draw_precomp`): match the CPU (nested blend modes, effects inside and outside,
/// transforms, opacity, a blend mode on the precomp layer, half resolution,
/// two instances at different times of the same nested comp).
#[test]
fn nested_precomps_render_resident() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let inner = Comp::new(60, 40, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
        let inner_id = s.p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
        {
            let mut ins = Scene { p: std::mem::take(&mut s.p), cid: inner_id, comp: inner };
            let mut a = ins.footage(50, 30);
            ins.effect(&mut a, "ec.blur.gaussian", &[("blurriness", n(3.0))]);
            ins.push(a);
            let mut b = ins.solid([0.2, 0.7, 0.4], 30, 20);
            b.blend_mode = BlendMode::Difference;
            set(&mut b, "transform/rotation", Value::Scalar(15.0));
            ins.push(b);
            s.p = ins.p;
        }
        let bg = s.footage(97, 61);
        s.push(bg);
        let mut pre = build::layer(&mut s.p, &s.comp, "Inner", LayerSource::Comp { item: inner_id }, (60, 40), None);
        set(&mut pre, "transform/rotation", Value::Scalar(25.0));
        set(&mut pre, "transform/opacity", Value::Scalar(80.0));
        s.effect(&mut pre, "ec.color.levels", &[("gamma", n(1.3))]);
        pre.blend_mode = BlendMode::Screen;
        s.push(pre);
        // A second instance, later in the nested comp's time.
        let mut pre2 = build::layer(&mut s.p, &s.comp, "Inner 2", LayerSource::Comp { item: inner_id }, (60, 40), None);
        set(&mut pre2, "transform/position", v3(70.0, 40.0));
        pre2.start_time = Tick::from_seconds_f64(-0.5);
        s.push(pre2);
        check(&format!("nested precomps {depth:?}"), compare_at(&s, opts(), Tick::from_seconds_f64(0.2)), 0.0);
        let half = RenderOpts { scale: 0.5, ..opts() };
        check(&format!("nested precomps half {depth:?}"), compare_at(&s, half, Tick::from_seconds_f64(0.2)), 0.0);
    }
}

/// #613: an effect kernel the device's shader compiler rejects (FXC on DirectX 12 aborting on
/// `pointwise`) no longer turns the whole GPU compositor off: the context is built without it,
/// lists it, and the effects that need it render on the CPU, matching the CPU render.
#[test]
fn effects_whose_kernels_failed_render_on_the_cpu() {
    if gpu().is_none() {
        return;
    }
    let Ok(mut ctx) = pollster::block_on(crate::context::GpuContext::request()) else { return };
    ctx.drop_kernel("pointwise");
    ctx.drop_kernel("box_h");
    let names: Vec<&str> = ctx.unavailable_kernels().iter().map(|(e, _)| *e).collect();
    assert_eq!(names, ["pointwise", "box_h"]);
    let g = Gpu::from_context(ctx);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let mut l = s.footage(80, 50);
        s.effect(&mut l, "ec.color.exposure", &[("master/exposure", n(0.7))]);
        s.effect(&mut l, "ec.color.tint", &[("black", c(0.1, 0.0, 0.3)), ("amount", n(80.0))]);
        s.effect(&mut l, "ec.blur.gaussian", &[("blurriness", n(5.0))]);
        s.effect(&mut l, "ec.channel.invert", &[("blend", n(50.0))]);
        s.push(l);
        let cpu = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Cpu, ..opts() }).comp_frame_cpu(s.cid, Tick::ZERO);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(&g);
        let img = g.render(&r, s.cid, Tick::ZERO).expect("the GPU renders the frame");
        let mut d = diff(&cpu, &img, tolerance(depth));
        d.quantized = depth != BitDepth::Bpc32;
        check(&format!("missing kernels {depth:?}"), Some(d), 0.0);
    }
}

/// The GPU Tint gives After Effects 2026's greys for full-frame solids at its defaults (sRGB
/// working space, not linearised): Rec. 601 weights, as the CPU (`color_fx::luma_601`).
#[test]
fn gpu_tint_defaults_match_after_effects_greys() {
    let Some(g) = gpu() else {
        // Visible skip (EFFECTCRAFT_REQUIRE_GPU=1 makes a missing adapter fail instead).
        eprintln!("gpu_tint_defaults_match_after_effects_greys: skipped: no GPU adapter");
        return;
    };
    for (col, want) in [([1.0, 0.0, 0.0], 76.0), ([0.0, 1.0, 0.0], 150.0), ([0.0, 0.0, 1.0], 29.0), ([0.5, 0.5, 0.5], 128.0)] {
        let mut s = Scene::new(BitDepth::Bpc32);
        s.p.settings.working_space = Some(ColorSpace::Srgb);
        let mut l = s.solid(col, 97, 61);
        s.effect(&mut l, "ec.color.tint", &[]);
        s.push(l);
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(g);
        let o = g.render(&r, s.cid, Tick::ZERO).expect("the GPU renders the frame").get(48, 30);
        let want = want / 255.0;
        assert!(o[..3].iter().all(|v| (v - want).abs() <= 1.0 / 255.0), "{col:?}: {o:?} vs {want}");
    }
}

/// WGSL has no way to read the CPU's constants, so the kernels repeat them as literals; this
/// ties each literal to its Rust source (runs without a GPU): Rec. 601 luma for Tint (`pointwise`
/// op 1), Tritone and Luma Key / Extract, Black & White's tint weights and Exposure's 2.2 power.
#[test]
fn shader_constants_match_the_cpu() {
    use effectcraft_effects::{BW_TINT_LUMA, EXPOSURE_LINEAR_GAMMA, LUMA_601};
    let kernels = include_str!("shaders/kernels.wgsl");
    let color = include_str!("shaders/fx_color.wgsl");
    let key = include_str!("shaders/fx_key.wgsl");
    let [r, g, b] = LUMA_601;
    let dot = format!("dot(c, vec3<f32>({r}, {g}, {b}))");
    assert!(kernels.contains(&format!("clamp({r} * c.x + {g} * c.y + {b} * c.z, 0.0, 1.0)")), "Tint luma in kernels.wgsl is not {LUMA_601:?}");
    assert!(color.contains(&format!("clamp({dot}, 0.0, 1.0)")), "Tritone luma in fx_color.wgsl is not {LUMA_601:?}");
    assert!(key.contains(&format!("return {dot};")), "fxk_key_luma in fx_key.wgsl is not {LUMA_601:?}");
    let [r, g, b] = BW_TINT_LUMA;
    assert!(color.contains(&format!("let w = vec3<f32>({r}, {g}, {b});")), "fxc_bw_tint weights are not {BW_TINT_LUMA:?}");
    let gamma = EXPOSURE_LINEAR_GAMMA;
    assert!(
        kernels.contains(&format!("powz(abs(c[i]), {gamma})")) && kernels.contains(&format!("1.0 / {gamma})")),
        "Exposure's power in kernels.wgsl is not {gamma}"
    );
}

/// A layer whose bounds change every frame reads back at a new size each time: the staging
/// buffers kept for reuse stay within budget instead of growing for the whole render (#2081).
#[test]
fn readback_staging_stays_bounded_across_sizes() {
    let Some(g) = gpu() else { return };
    if !g.ctx.can_wait() {
        return;
    }
    let mut e = crate::context::Enc::new(&g.ctx);
    // 1024 × (256 + i) RGBA f32: ~4 MiB each, ~480 MiB over all sizes.
    for i in 0..120 {
        let img = g.ctx.image(1024, 256 + i);
        assert!(e.download(&img).is_some(), "readback {i}");
    }
    let held = g.ctx.staging_bytes();
    assert!(held <= (256 << 20) + 2 * 1024 * 16 * 376, "staging buffers hold {held} bytes");
}

/// A static adjustment layer with a time-dependent effect (Noise) keeps its footprint key
/// across frames (the footprint is the source through the masks, not the effects), so the GPU
/// uploads nothing after the first frame; matches the CPU.
#[test]
fn adjustment_footprint_uploads_once_under_time_dependent_effects() {
    let mut s = Scene::new(BitDepth::Bpc8);
    let bg = s.solid([0.3, 0.5, 0.7], 96, 64);
    s.push(bg);
    let mut adj = s.solid([1.0, 1.0, 1.0], 80, 50);
    adj.switches.adjustment = true;
    s.effect(&mut adj, "ec.noise.noise", &[("amount", n(30.0))]);
    s.push(adj);
    // The key itself (no GPU needed): stable across time.
    let comp = s.p.comp(s.cid).expect("the scene's comp");
    let adj = comp.layers.last().expect("the adjustment layer");
    let key = |k: f64| {
        effectcraft_render::cache::footprint_key(&effectcraft_render::EvalCtx::new(&s.p, s.cid, comp, Tick::from_seconds_f64(k)), adj, 1.0, false, false)
    };
    assert!(key(0.0).is_some());
    assert_eq!(key(0.0), key(0.5), "the footprint key changes with time");
    let Some(g) = gpu() else { return };
    let cache = effectcraft_render::cache::LayerCache::new(64 << 20);
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
    r.accel = Some(g);
    r.cache = Some(&cache);
    let t = |k: u64| Tick::from_seconds_f64(k as f64 / 30.0);
    let mut prev = None;
    for k in 0..4 {
        let before = g.ctx.transfer_stats().upload_bytes;
        let img = g.render(&r, s.cid, t(k)).expect("the GPU renders the frame");
        let up = g.ctx.transfer_stats().upload_bytes - before;
        if k >= 2 {
            assert_eq!(up, 0, "frame {k} uploaded {up} bytes: the footprint was re-keyed");
        }
        // Noise changes every frame: the frames differ.
        assert!(prev.as_ref() != Some(&img.data), "frame {k} repeats the previous one");
        prev = Some(img.data);
        check(&format!("noise adjustment frame {k}"), compare_at(&s, opts(), t(k)), 0.0);
    }
}

/// An `Enc` reuses a working texture it released at once (`Pool::owned`); another `Enc` does
/// not get it while the first is alive.
#[test]
fn enc_reuses_its_own_released_textures() {
    let Some(g) = gpu() else { return };
    let mut e = crate::context::Enc::new(&g.ctx);
    let a = e.image(37, 23);
    let ta = a.texture.clone();
    drop(a);
    let mut other = crate::context::Enc::new(&g.ctx);
    let o = other.image(37, 23);
    assert!(o.texture != ta, "another Enc got a texture the first may still be using");
    let b = e.image(37, 23);
    assert!(b.texture == ta, "the Enc allocated a new texture instead of reusing its own");
}

/// In-`Enc` texture reuse changes no pixel: a busy frame (layers, effects, adjustment layer,
/// nested comp) renders bit-identically with reuse on and off, and matches the CPU.
#[test]
fn texture_reuse_does_not_change_output() {
    let Some(g) = gpu() else { return };
    let mut s = Scene::new(BitDepth::Bpc32);
    for i in 0..4 {
        let mut l = s.footage(90 - i * 7, 60 - i * 5);
        s.effect(&mut l, "ec.blur.gaussian", &[("blurriness", n(2.0 + i as f64))]);
        set(&mut l, "transform/rotation", Value::Scalar(10.0 * i as f64));
        l.blend_mode = if i % 2 == 0 { BlendMode::Screen } else { BlendMode::Multiply };
        s.push(l);
    }
    let mut adj = s.solid([1.0, 1.0, 1.0], 80, 50);
    adj.switches.adjustment = true;
    s.effect(&mut adj, "ec.color.levels", &[("gamma", n(1.2))]);
    s.push(adj);
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
    r.accel = Some(g);
    let t = Tick::from_seconds_f64(0.3);
    let on = g.render(&r, s.cid, t).expect("the GPU renders the frame");
    crate::context::TEXTURE_REUSE.store(false, std::sync::atomic::Ordering::Relaxed);
    let off = g.render(&r, s.cid, t);
    crate::context::TEXTURE_REUSE.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(off.is_some_and(|off| off.data == on.data), "texture reuse changed the frame");
    check("texture reuse vs CPU", compare_at(&s, opts(), t), 0.0);
}

/// Static layer buffers (here adjustment-layer footprints) upload once while they fit the upload budget, and upload again every frame
/// when they do not (least recently used first); the frame is the same either way.
#[test]
fn upload_budget_bounds_the_layer_textures_kept() {
    let Some(g) = gpu() else { return };
    let mut s = Scene::new(BitDepth::Bpc8);
    for (i, c) in [[0.8, 0.2, 0.1], [0.1, 0.3, 0.9]].into_iter().enumerate() {
        let mut l = s.solid(c, 200, 200);
        set(&mut l, "transform/position", v3(60.0 + 30.0 * i as f64, 40.0));
        // Adjustment layers: their footprints are cached layer buffers the GPU uploads.
        l.switches.adjustment = true;
        s.effect(&mut l, "ec.color.levels", &[("gamma", n(1.1 + 0.2 * i as f64))]);
        s.push(l);
    }
    let frame = |cache: &effectcraft_render::cache::LayerCache, k: u64| {
        let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
        r.accel = Some(g);
        r.cache = Some(cache);
        let before = g.ctx.transfer_stats().upload_bytes;
        let img = g.render(&r, s.cid, Tick::from_seconds_f64(k as f64 / 30.0)).expect("the GPU renders the frame");
        (img.data, g.ctx.transfer_stats().upload_bytes - before)
    };
    let fits = effectcraft_render::cache::LayerCache::new(64 << 20);
    let (a, _) = frame(&fits, 0);
    let (b, up) = frame(&fits, 1);
    assert_eq!(up, 0, "static layers re-uploaded within the default budget");
    // A budget smaller than the two 640 KB footprints: each upload evicts the other.
    let default = g.ctx.upload_budget();
    g.ctx.set_upload_budget(1 << 20);
    let tight = effectcraft_render::cache::LayerCache::new(64 << 20);
    let _ = frame(&tight, 0);
    let (c, up) = frame(&tight, 1);
    g.ctx.set_upload_budget(default);
    assert!(up > 0, "over budget, the footprints must upload again");
    assert!(a == b && b == c, "the upload budget changed the frame");
}

/// Layers with effect stacks run them resident on the GPU (`walk::draw_resident_fx`): GPU
/// effects, a CPU-only effect in the middle (read back, run, upload), opacity and a blend mode,
/// a shape layer whose effects run in effect space from its bounds' corner; 8 and 32 bpc, full
/// and half resolution; all match the CPU.
#[test]
fn effect_stacks_render_resident() {
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.footage(97, 61);
        s.push(bg);
        let mut a = s.footage(70, 44);
        s.effect(&mut a, "ec.blur.gaussian", &[("blurriness", n(4.0))]);
        s.effect(&mut a, "ec.generate.fractal", &[]);
        s.effect(&mut a, "ec.color.levels", &[("gamma", n(1.4))]);
        set(&mut a, "transform/rotation", Value::Scalar(20.0));
        set(&mut a, "transform/opacity", Value::Scalar(70.0));
        a.blend_mode = BlendMode::Overlay;
        s.push(a);
        let mut l = build::layer(&mut s.p, &s.comp, "Shape", LayerSource::Shape, (97, 61), None);
        let mut next = s.p.next_id;
        let mut ids = Ids(&mut next);
        let rect = build::shape_rect(&mut ids, [50.0, 30.0], [5.0, -4.0], 0.0);
        let fill = build::shape_fill(&mut ids, [0.2, 0.8, 0.3, 1.0]);
        let g = build::shape_group(&mut ids, "Rectangle 1", vec![rect, fill]);
        s.p.next_id = next;
        l.props.sub_mut("contents").expect("a shape layer has contents").children.push(g.into());
        s.effect(&mut l, "ec.blur.gaussian", &[("blurriness", n(3.0))]);
        s.effect(&mut l, "ec.stylize.mosaic", &[]);
        s.push(l);
        let t = Tick::from_seconds_f64(0.2);
        check(&format!("effect stacks {depth:?}"), compare_at(&s, opts(), t), 0.0);
        check(&format!("effect stacks half {depth:?}"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, t), 0.0);
    }
}

/// A layer whose effects give the same result as last frame draws from the layer cache (its
/// processed buffer uploads once) instead of re-running the stack on the GPU every frame.
#[test]
fn static_effect_layers_draw_from_the_cache() {
    let Some(g) = gpu() else { return };
    let mut s = Scene::new(BitDepth::Bpc8);
    let mut l = s.solid([0.6, 0.4, 0.2], 80, 50);
    s.effect(&mut l, "ec.blur.gaussian", &[("blurriness", n(3.0))]);
    s.push(l);
    let cache = effectcraft_render::cache::LayerCache::new(64 << 20);
    let mut r = Renderer::new(&s.p, &Pattern, RenderOpts { backend: Backend::Gpu, ..opts() });
    r.accel = Some(g);
    r.cache = Some(&cache);
    let comp = s.p.comp(s.cid).expect("the scene's comp");
    let layer = comp.layers.last().expect("the layer");
    let t = |k: u64| Tick::from_seconds_f64(k as f64 / 30.0);
    assert!(r.layer_is_static(&effectcraft_render::EvalCtx::new(&s.p, s.cid, comp, t(2)), layer));
    let mut frames = Vec::new();
    for k in 0..3 {
        let before = g.ctx.transfer_stats().upload_bytes;
        frames.push(g.render(&r, s.cid, t(k)).expect("the GPU renders the frame").data);
        let up = g.ctx.transfer_stats().upload_bytes - before;
        if k == 2 {
            assert_eq!(up, 0, "the cached layer uploaded again");
        }
    }
    assert!(frames[1] == frames[2]);
    check("static effect layer vs CPU", compare_at(&s, opts(), t(2)), 0.0);
}

/// The layer upload cache drops the textures of freed buffers when it inserts, not only once
/// it is over budget (an animated layer makes a new buffer every frame).
#[test]
fn upload_cache_drops_textures_of_freed_buffers() {
    let Some(g) = gpu() else { return };
    let img = effectcraft_raster::Image::new(256, 256);
    let before = g.ctx.upload_entries();
    for _ in 0..6 {
        let buf = std::sync::Arc::new(effectcraft_effects::Buf { img: img.clone(), offset: [0.0; 2], scale: 1.0 });
        let up = g.ctx.upload_buf(&buf).expect("upload");
        drop((up, buf));
    }
    let kept = g.ctx.upload_entries();
    assert!(kept <= before + 1, "the upload cache kept {kept} textures of freed buffers (had {before})");
}

/// The device the compositor owns asks for `MemoryUsage` so freed textures give
/// their allocator blocks back, while its label and limits stay as they were.
#[test]
fn owned_devices_favour_memory_usage() {
    let d = crate::context::device_descriptor(wgpu::Limits::default());
    assert!(matches!(d.memory_hints, wgpu::MemoryHints::MemoryUsage));
    assert_eq!(d.label, Some("effectcraft gpu"));
    assert_eq!(d.required_limits.max_texture_dimension_2d, wgpu::Limits::default().max_texture_dimension_2d);
}
