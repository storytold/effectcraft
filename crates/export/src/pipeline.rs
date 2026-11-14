//! The frame pipeline shared by every output format: Render Settings overrides (effects, solo,
//! guide layers, frame blending, motion blur), field rendering with 3:2 pulldown, colour depth,
//! the Output Module's crop and resize, alpha handling, storage overflow and the render log.

use std::sync::Mutex;

use effectcraft_project::render_queue::{AlphaMode, Channels, CurrentOrOff, EffectsMode, FrameSample, RenderLog, ResizeQuality, SwitchOverride, log_path};
use effectcraft_project::{BitDepth, Comp, Project};
use effectcraft_raster::Image;
use effectcraft_render::{Backend, LayerCache, RenderOpts, Renderer};
use effectcraft_time::Tick;
use web_time::Instant;

use crate::{Job, RenderQuality, out};

/// GPU frames rendered at once until a frame has measured the device memory, and whenever it
/// can't be measured (#898, #879: measured on the #879 project at 1080p on an 8 GB card, 6.3 GB
/// peak with one frame, 7.0 to 7.8 GB with two, the whole card with three).
pub(crate) const GPU_INFLIGHT_DEFAULT: usize = 1;
/// The most GPU frames Auto renders at once (#3335: four and more measured no faster).
pub const GPU_INFLIGHT_MAX: usize = 3;
/// Overrides Settings ▸ Memory & CPU ▸ GPU Frames Rendered at Once: a whole number of frames,
/// at least 1.
pub(crate) const GPU_INFLIGHT_ENV: &str = "EFFECTCRAFT_GPU_INFLIGHT";

/// GPU frames in flight from the `EFFECTCRAFT_GPU_INFLIGHT` value: a positive whole number, or
/// `None` (with a warning when a value was given) when it is unset, 0, negative, not a number
/// or too large.
pub(crate) fn gpu_inflight_env(var: Option<&str>) -> Option<usize> {
    let v = var?;
    match v.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Some(n),
        _ => {
            log::warn!(target: "effectcraft_export", "{GPU_INFLIGHT_ENV}={v:?} is not a whole number of frames of at least 1: ignored");
            None
        }
    }
}

/// Auto's GPU frames in flight from the device memory after a frame (#3335): as many more
/// frames as the budget has room for beyond the allocator's peak (which holds one frame's
/// working memory) and what the caches may still grow by, 1 to [`GPU_INFLIGHT_MAX`]. Unknown
/// memory: 1.
pub fn auto_gpu_frames(m: Option<effectcraft_render::FrameMemory>) -> usize {
    let Some(m) = m.filter(|m| m.budget > 0 && m.frame > 0 && m.peak > 0) else { return GPU_INFLIGHT_DEFAULT };
    let more = m.budget.saturating_sub(m.peak).saturating_sub(m.reserve) / m.frame;
    usize::try_from(more.saturating_add(1)).unwrap_or(GPU_INFLIGHT_MAX).clamp(1, GPU_INFLIGHT_MAX)
}

/// The GPU frames a job renders at once and why (for the render log): the environment's
/// override, else the setting (`pref`, `None` = Auto), else Auto from `measured` (`None`: no
/// GPU frame has rendered yet, so nothing is settled).
pub(crate) fn resolve_gpu_frames(env: Option<&str>, pref: Option<usize>, measured: Option<Option<effectcraft_render::FrameMemory>>) -> Option<(usize, String)> {
    if let Some(n) = gpu_inflight_env(env) {
        return Some((n, format!("set by {GPU_INFLIGHT_ENV}")));
    }
    if let Some(n) = pref.filter(|n| *n >= 1) {
        return Some((n, "Settings".into()));
    }
    let m = measured?;
    let n = auto_gpu_frames(m);
    let mib = |b: u64| b >> 20;
    Some((
        n,
        match m {
            Some(m) => format!(
                "Auto: {} MiB per frame, {} MiB peak and {} MiB cache reserve in a {} MiB budget",
                mib(m.frame),
                mib(m.peak),
                mib(m.reserve),
                mib(m.budget)
            ),
            None => "Auto: the GPU's memory is not known".into(),
        },
    ))
}

/// Overrides the Render Queue's measured choice of compositor for GPU projects with an
/// accelerator (#3335): `gpu` (no measurement, the GPU), `cpu` (Software Rendering) or
/// `auto` (measure, the default).
pub const RENDERER_ENV: &str = "EFFECTCRAFT_RQ_RENDERER";

/// Frames the GPU renders, timed, when a job measures which compositor is faster (after one
/// untimed warm-up frame each: layer cache misses, uploads, kernel builds).
const PROBE_FRAMES: usize = 2;
/// Frames the CPU renders at once, timed, in that measurement: its throughput comes from
/// rendering several frames in parallel, which two frames understate.
const PROBE_CPU_FRAMES: usize = 8;
/// Jobs shorter than this render on the project's compositor without measuring: the frames
/// the slower one renders for the measurement would cost more than they could save.
const PROBE_MIN_JOB: u64 = 12;
/// Bytes each of a job's two layer caches (one per compositor) may hold: together what the
/// job's single cache held before (`LayerCache::default()`).
pub const CACHE_BUDGET: usize = 1 << 29;

/// The faster compositor from measured milliseconds per frame (`None`: not measured, or the
/// GPU stopped). As the viewer's Auto ([`effectcraft_render::AutoPick::MARGIN`]), the CPU wins
/// only when clearly faster: ties and noise stay on the GPU.
pub fn pick_renderer(gpu_ms: Option<f64>, cpu_ms: Option<f64>) -> crate::JobRenderer {
    match (gpu_ms, cpu_ms) {
        (Some(g), Some(c)) if g.is_finite() && c.is_finite() && c < g * effectcraft_render::AutoPick::MARGIN => crate::JobRenderer::Cpu,
        (None, Some(_)) => crate::JobRenderer::Cpu,
        _ => crate::JobRenderer::Gpu,
    }
}

/// How a job's compositor was chosen, for the render log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Choice {
    pub renderer: crate::JobRenderer,
    /// Measured ms per frame on each (`None`: not measured).
    pub gpu_ms: Option<f64>,
    pub cpu_ms: Option<f64>,
    pub why: &'static str,
}

/// Caps CPU frames in flight (#1615): a whole number of frames, at least 1. Unset, CPU renders
/// keep one frame per thread in flight as before. A heavy comp can need gigabytes of working
/// memory per frame, so 16 threads at once can get the process OOM-killed; the headless CLI
/// reads no `memory.*` preferences, so this is how a render there is held to fewer frames.
pub(crate) const CPU_INFLIGHT_ENV: &str = "EFFECTCRAFT_CPU_INFLIGHT";

/// CPU frames in flight from the `EFFECTCRAFT_CPU_INFLIGHT` value: `None` (no cap) when unset or
/// blank, a positive whole number, or `None` with a warning when it is 0, negative, not a
/// number or too large.
pub(crate) fn cpu_inflight(var: Option<&str>) -> Option<usize> {
    let v = var?;
    if v.trim().is_empty() {
        return None;
    }
    match v.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Some(n),
        _ => {
            log::warn!(target: "effectcraft_export", "{CPU_INFLIGHT_ENV}={v:?} is not a whole number of frames of at least 1: CPU frames are not capped");
            None
        }
    }
}

/// Auto's CPU frames in flight (#1615): as many frames as half of the `free` bytes the system
/// can hand out hold at `frame` bytes each, 1 to `threads`. `frame` is a GPU frame's measured
/// working memory: both compositors keep 16-byte RGBA pixels (`[f32; 4]`, `Rgba32Float`), and
/// the other half is headroom for the layer caches, decoded footage, the encoder and the
/// estimate being low. `None` (not capped) when either is unknown or `frame` is 0.
pub(crate) fn auto_cpu_frames(frame: Option<u64>, free: Option<u64>, threads: usize) -> Option<usize> {
    let (frame, free) = (frame.filter(|&f| f > 0)?, free?);
    let n = free / 2 / frame;
    Some(usize::try_from(n).unwrap_or(usize::MAX).clamp(1, threads.max(1)))
}

/// The frames-in-flight caps of a batch: `gpu` (#879) for the GPU's memory, `cpu` (#1615) for
/// the process's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Inflight {
    pub gpu: Option<usize>,
    pub cpu: Option<usize>,
}

impl Inflight {
    /// Frames rendered at once: the smaller cap while the GPU renders, and only the CPU cap once
    /// it has stopped for good (the CPU fallback renders in parallel, #1511). `None`: uncapped.
    pub fn renders(&self, gpu_live: bool) -> Option<usize> {
        let gpu = self.gpu.filter(|_| gpu_live);
        let n = match (gpu, self.cpu) {
            (Some(g), Some(c)) => Some(g.min(c)),
            (g, c) => g.or(c),
        };
        n.map(|n| n.max(1))
    }

    /// Frames per window, rendered (`renders` at a time) and then encoded in parallel: with a CPU
    /// cap at most that many, so finished images held before encoding count against it too;
    /// with a GPU cap alone, one per thread (the images are in RAM, not VRAM).
    pub fn window(&self, renders: usize) -> usize {
        self.cpu.map_or(renders.max(rayon::current_num_threads()), |c| c.max(1))
    }
}

/// `render` then `encode` over the frames `ks`, results in `ks` order.
///
/// Without `cap` every frame renders and encodes in one parallel batch (the CPU path, as
/// before). With `cap` (GPU frames in flight), the frames go in windows of
/// `max(cap, threads)`: each window renders in consecutive chunks of `cap` (at most `cap`
/// renders at once), then encodes in parallel, so only the render is serialised and at most
/// one window of rendered images is held in memory. Chunks hold no lock or slot, so a failing
/// or panicking frame cannot leak one, and nested parallelism (or a single thread) cannot
/// deadlock.
#[cfg(test)]
pub(crate) fn map_frames<I: Send, T: Send>(
    ks: Vec<u64>,
    cap: Option<usize>,
    render: impl Fn(u64) -> I + Sync + Send,
    encode: impl Fn(u64, I) -> T + Sync + Send,
) -> Vec<T> {
    map_frames_while(ks, Inflight { gpu: cap, cpu: None }, || true, render, encode)
}

/// [`map_frames`] with the caps of `caps`, the GPU cap holding only while `gpu_live()`: once it
/// turns false (the GPU stopped for good, so every frame falls back to the CPU), the remaining
/// frames render under the CPU cap alone, in parallel when there is none, instead of one chunk
/// of the GPU cap at a time. A CPU cap also bounds each window ([`Inflight::window`]).
pub(crate) fn map_frames_while<I: Send, T: Send>(
    ks: Vec<u64>,
    caps: Inflight,
    gpu_live: impl Fn() -> bool,
    render: impl Fn(u64) -> I + Sync + Send,
    encode: impl Fn(u64, I) -> T + Sync + Send,
) -> Vec<T> {
    use rayon::prelude::*;
    if caps.renders(gpu_live()).is_none() {
        return ks.into_par_iter().map(|k| encode(k, render(k))).collect();
    }
    let mut out = Vec::with_capacity(ks.len());
    let mut rest = &ks[..];
    while !rest.is_empty() {
        let Some(n) = caps.renders(gpu_live()) else {
            out.par_extend(rest.par_iter().map(|&k| encode(k, render(k))));
            break;
        };
        let (win, tail) = rest.split_at(caps.window(n).min(rest.len()));
        let mut imgs = Vec::with_capacity(win.len());
        let mut i = 0;
        while i < win.len() {
            let end = caps.renders(gpu_live()).map_or(win.len(), |n| (i + n).min(win.len()));
            imgs.par_extend(win[i..end].par_iter().map(|&k| render(k)));
            i = end;
        }
        out.par_extend(win.par_iter().copied().zip(imgs.into_par_iter()).map(|(k, img)| encode(k, img)));
        rest = tail;
    }
    out
}

/// Threads that encode one batch of frames while the next renders ([`Cx::frames_beside`]).
/// Their own pool: the encoder's parallel steps (colour conversion) would otherwise queue behind
/// the render's work in the global pool. `None` where threads can't be started (a browser
/// without them): encoding and rendering then take turns.
pub(crate) struct Beside(Option<rayon::ThreadPool>);

impl Beside {
    fn new() -> Beside {
        Beside(rayon::ThreadPoolBuilder::new().thread_name(|i| format!("export-encode-{i}")).build().ok())
    }

    /// The process's encode threads, started by the first movie export and shared by all.
    pub fn shared() -> &'static Beside {
        static SHARED: std::sync::OnceLock<Beside> = std::sync::OnceLock::new();
        SHARED.get_or_init(Beside::new)
    }
}

/// A job being exported: the job, the project with the Render Settings overrides applied, the
/// job's layer cache and per-frame timing for the render log.
pub(crate) struct Cx<'a> {
    pub job: &'a Job<'a>,
    pub project: Project,
    /// Processed layer buffers shared by the job's frames, as the viewer's layer cache: static
    /// and transform-only layers render once per job (the GPU compositor then uploads them
    /// once). One per compositor; the job's own (dropped with it) or a Render Queue run's.
    caches: Caches<'a>,
    /// The compositor the job renders on, once chosen ([`Cx::backend`]).
    choice: Mutex<Option<Choice>>,
    /// GPU frames rendered at once and why, once settled ([`Cx::gpu_inflight`]).
    gpu_frames: Mutex<Option<(usize, String)>>,
    /// A frame has rendered on the GPU compositor: its memory can be measured.
    gpu_rendered: std::sync::atomic::AtomicBool,
    /// Auto's CPU frames in flight once settled ([`Cx::auto_cpu_frames`]; `Some(None)`: not
    /// capped).
    cpu_frames: Mutex<Option<Option<usize>>>,
    /// (output frame, seconds) of every rendered frame (Plus Per Frame Info logs).
    pub frame_times: Option<Mutex<Vec<(u64, f64)>>>,
    /// Files written to an overflow folder.
    pub overflowed: Mutex<Vec<String>>,
    written: Mutex<Vec<String>>,
}

/// A job's layer caches: its own, or those of the Render Queue run it belongs to.
enum Caches<'a> {
    Own { gpu: Box<LayerCache>, cpu: Box<LayerCache> },
    Run(crate::JobCaches<'a>),
}

impl Caches<'_> {
    fn of(&self, r: crate::JobRenderer) -> &LayerCache {
        match (self, r) {
            (Caches::Own { gpu, .. }, crate::JobRenderer::Gpu) => gpu,
            (Caches::Own { cpu, .. }, crate::JobRenderer::Cpu) => cpu,
            (Caches::Run(c), crate::JobRenderer::Gpu) => c.gpu,
            (Caches::Run(c), crate::JobRenderer::Cpu) => c.cpu,
        }
    }
}

impl<'a> std::ops::Deref for Cx<'a> {
    type Target = Job<'a>;
    fn deref(&self) -> &Job<'a> {
        self.job
    }
}

/// The project as the Render Settings render it: Effects All On / All Off, Solo Switches All
/// Off, Frame Blending and Motion Blur "On for Checked Layers" (the composition switches on).
pub(crate) fn apply_overrides(p: &mut Project, job: &Job) {
    let s = job.settings;
    let ids: Vec<_> = p.comps().map(|(i, _)| *i).collect();
    for id in ids {
        let Some(c) = p.comp_mut(id) else { continue };
        match s.frame_blending {
            SwitchOverride::Current => {}
            SwitchOverride::OnForChecked => c.enable_frame_blending = true,
            SwitchOverride::OffForAll => c.enable_frame_blending = false,
        }
        match s.motion_blur_override() {
            SwitchOverride::Current => {}
            SwitchOverride::OnForChecked => c.enable_motion_blur = true,
            SwitchOverride::OffForAll => c.enable_motion_blur = false,
        }
        for l in c.layers.iter_mut() {
            match s.effects {
                EffectsMode::Current => {}
                EffectsMode::AllOn => l.switches.effects = true,
                EffectsMode::AllOff => l.switches.effects = false,
            }
            if s.solo == CurrentOrOff::AllOff {
                l.switches.solo = false;
            }
        }
    }
}

impl<'a> Cx<'a> {
    pub fn new(job: &'a Job<'a>) -> Cx<'a> {
        let mut project = job.project.clone();
        apply_overrides(&mut project, job);
        // Render Settings ▸ Color Depth overrides the project's depth for the whole render (every
        // layer and effect quantises at it), not just the finished frame (#2022).
        project.settings.bit_depth = job.settings.color_depth.resolve(project.settings.bit_depth);
        let frame_times = (job.options.log == RenderLog::PlusPerFrameInfo).then(|| Mutex::new(Vec::new()));
        let caches = match job.options.caches {
            Some(c) => Caches::Run(c),
            None => Caches::Own { gpu: Box::new(LayerCache::new(CACHE_BUDGET)), cpu: Box::new(LayerCache::new(CACHE_BUDGET)) },
        };
        // With deferred readbacks, buffers computed from a missed pass's placeholders are not kept.
        for r in [crate::JobRenderer::Gpu, crate::JobRenderer::Cpu] {
            caches.of(r).set_gate(job.accel.and_then(|a| a.miss_gate()));
        }
        let cx = Cx {
            job,
            project,
            caches,
            choice: Mutex::new(None),
            gpu_frames: Mutex::new(None),
            gpu_rendered: Default::default(),
            cpu_frames: Mutex::new(None),
            frame_times,
            overflowed: Mutex::new(vec![]),
            written: Mutex::new(vec![]),
        };
        if let Some(c) = cx.fixed_choice(std::env::var(RENDERER_ENV).ok().as_deref()) {
            cx.decide(c);
        }
        cx
    }

    /// The project's compositor: the GPU's for GPU Acceleration with an accelerator.
    fn project_renderer(&self) -> crate::JobRenderer {
        if self.project.settings.gpu_acceleration && self.job.accel.is_some() { crate::JobRenderer::Gpu } else { crate::JobRenderer::Cpu }
    }

    /// The job's compositor when it is decided without measuring: the queue item's first
    /// output module's, the project's when there is nothing to choose between (Software Only,
    /// no accelerator, deferred readbacks), or `env` ([`RENDERER_ENV`]). `None`: measure.
    fn fixed_choice(&self, env: Option<&str>) -> Option<Choice> {
        let fixed = |renderer, why| Some(Choice { renderer, gpu_ms: None, cpu_ms: None, why });
        if self.project_renderer() == crate::JobRenderer::Cpu {
            return fixed(crate::JobRenderer::Cpu, "the project's renderer");
        }
        if self.deferred() {
            return fixed(crate::JobRenderer::Gpu, "the project's renderer");
        }
        let shared = self.job.options.renderer.and_then(|s| *s.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let shared = match shared {
            Some(Backend::Gpu) => Some(crate::JobRenderer::Gpu),
            Some(Backend::Cpu) => Some(crate::JobRenderer::Cpu),
            Some(Backend::Auto) | None => None,
        };
        if let Some(r) = shared {
            return Some(Choice { renderer: r, gpu_ms: None, cpu_ms: None, why: "as the queue item's first output module" });
        }
        match env.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("gpu") => fixed(crate::JobRenderer::Gpu, "set by EFFECTCRAFT_RQ_RENDERER"),
            Some("cpu") => fixed(crate::JobRenderer::Cpu, "set by EFFECTCRAFT_RQ_RENDERER"),
            None | Some("" | "auto") => None,
            Some(v) => {
                log::warn!(target: "effectcraft_export", "{RENDERER_ENV}={v:?} is not gpu, cpu or auto: measuring");
                None
            }
        }
    }

    /// Settle the job's compositor (once): remember it for the queue item's other output
    /// modules and say so in the log.
    fn decide(&self, c: Choice) {
        let mut slot = self.choice.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return;
        }
        *slot = Some(c);
        if let Some(shared) = self.job.options.renderer {
            let mut shared = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if shared.is_none_or(|b| b == Backend::Auto) {
                *shared = Some(match c.renderer {
                    crate::JobRenderer::Gpu => Backend::Gpu,
                    crate::JobRenderer::Cpu => Backend::Cpu,
                });
            }
        }
        log::info!(target: "effectcraft_export", "{}: {}", self.job.options.label, describe_choice(&c));
    }

    /// The job's choice, once made.
    pub(crate) fn choice(&self) -> Option<Choice> {
        *self.choice.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The compositor frames render on: the choice, or the project's until it is made.
    fn backend(&self) -> crate::JobRenderer {
        self.choice().map_or_else(|| self.project_renderer(), |c| c.renderer)
    }

    pub fn comp(&self) -> Option<&Comp> {
        self.project.comp(self.job.comp)
    }

    /// The renderer of the job's frames: the Render Settings' options, the job's accelerator
    /// and layer cache.
    #[cfg(test)]
    fn renderer(&self) -> Renderer<'_> {
        self.renderer_on(self.backend())
    }

    /// [`Cx::renderer`] on compositor `on`, with that compositor's layer cache.
    fn renderer_on(&self, on: crate::JobRenderer) -> Renderer<'_> {
        let s = self.job.settings;
        // The whole job renders on one compositor (the project's, or the faster one measured on
        // the first frames, #3335), not on whichever compositor Auto timed faster for each
        // frame: the CPU and GPU compositors differ slightly, so a per-frame choice made renders
        // differ within a render.
        let backend = match on {
            crate::JobRenderer::Gpu => Backend::Gpu,
            crate::JobRenderer::Cpu => Backend::Cpu,
        };
        let opts = RenderOpts {
            scale: self.comp().map(|c| s.effective_scale(c)).unwrap_or_else(|| s.resolution.clamp(0.01, 4.0)),
            motion_blur: s.motion_blur_override() != SwitchOverride::OffForAll,
            guides: s.guide_layers == CurrentOrOff::Current,
            draft: s.quality == RenderQuality::Draft,
            // Output renders always look through the comp's active camera.
            view: None,
            backend,
            roi: None,
            nested_switches: self.job.nested_switches,
            draft_shadows: true,
            // OpenEXR keeps over-range values: no HDR compand / tone mapping on its frames.
            skip_hdr_step: self.job.output.format == effectcraft_project::render_queue::OutputFormat::ExrSequence,
            proxy: s.proxy_use,
        };
        let mut r = Renderer::new(&self.project, self.job.footage, opts);
        r.expr = self.job.expr;
        r.accel = self.job.accel;
        r.cache = Some(self.caches.of(on));
        r
    }

    async fn render_at(&self, t: Tick, on: crate::JobRenderer) -> Image {
        // With deferred GPU readbacks (a browser worker) the frame renders in passes.
        effectcraft_render::passes::comp_frame(&self.renderer_on(on), self.job.comp, t).await
    }

    /// Whether frames render in passes (an accelerator with deferred readbacks).
    pub fn deferred(&self) -> bool {
        effectcraft_render::passes::is_deferred(self.job.accel)
    }

    /// Output frames `ks`, each mapped by `f` (encoding): rendered in parallel batches, or one
    /// by one in passes when readbacks are deferred ([`Cx::deferred`]).
    pub async fn frames<T: Send>(&self, comp: &Comp, ks: Vec<u64>, f: impl Fn(u64, Image) -> T + Sync + Send) -> Vec<T> {
        if self.deferred() {
            let mut out = Vec::with_capacity(ks.len());
            for k in ks {
                let img = self.frame_async(comp, k).await;
                out.push(f(k, img));
            }
            return out;
        }
        self.render_frames(ks, |k, on| self.frame_on(comp, k, on), f)
    }

    /// `render` then `encode` over the frames `ks` on the job's compositor, results in `ks`
    /// order. The first call of a job that still has to choose its compositor measures both on
    /// its first frames ([`Cx::probe`]) and keeps the faster one's.
    fn render_frames<I: Send, T: Send>(
        &self,
        ks: Vec<u64>,
        render: impl Fn(u64, crate::JobRenderer) -> I + Sync + Send,
        encode: impl Fn(u64, I) -> T + Sync + Send,
    ) -> Vec<T> {
        use rayon::prelude::*;
        let probed = self.probe(&ks, &render);
        // The GPU cap is for the GPU's memory: once it has stopped for good, frames render on the
        // CPU, in parallel or under the CPU cap (#1615), which holds throughout.
        let gpu_live = || self.job.accel.is_none_or(|a| a.available());
        let on = self.backend();
        if probed.is_empty() {
            return map_frames_while(ks, self.inflight(), gpu_live, |k| render(k, on), encode);
        }
        let rest: Vec<u64> = ks.iter().copied().filter(|k| !probed.iter().any(|(p, _)| p == k)).collect();
        let mut done: Vec<(u64, T)> = probed.into_par_iter().map(|(k, i)| (k, encode(k, i))).collect();
        done.extend(rest.iter().copied().zip(map_frames_while(rest.clone(), self.inflight(), gpu_live, |k| render(k, on), &encode)));
        // Back in `ks` order.
        let mut out = Vec::with_capacity(ks.len());
        for k in &ks {
            if let Some(i) = done.iter().position(|(d, _)| d == k) {
                out.push(done.swap_remove(i).1);
            }
        }
        out
    }

    /// Choose the job's compositor by timing both on the first frames of `ks` (#3335), as the
    /// viewer's Auto does per comp: frame 0 warms each up (layer cache misses, uploads, kernel
    /// builds) untimed, then the GPU renders the next [`PROBE_FRAMES`] under its in-flight cap
    /// and the CPU up to [`PROBE_CPU_FRAMES`] after them in parallel (its throughput). The CPU
    /// may render more frames at once in the job than here, so the measurement leans towards
    /// the GPU, as the margin of [`pick_renderer`] does: the CPU is chosen only when clearly
    /// faster. Returns
    /// the frames the faster one rendered (the slower one's are rendered again); empty when
    /// the choice was made already or is made without measuring (a short job).
    fn probe<I: Send>(&self, ks: &[u64], render: &(impl Fn(u64, crate::JobRenderer) -> I + Sync + Send)) -> Vec<(u64, I)> {
        if self.choice().is_some() {
            return vec![];
        }
        let total = self.comp().map_or(0, |c| self.job.settings.frame_count(c));
        if total < PROBE_MIN_JOB {
            self.decide(Choice { renderer: crate::JobRenderer::Gpu, gpu_ms: None, cpu_ms: None, why: "the project's renderer (too few frames to measure)" });
            return vec![];
        }
        // Nothing will be emitted for a skipped batch, so a later batch can still measure.
        if ks.is_empty() {
            return vec![];
        }
        // A resumed sequence can have only one or two missing frames in its first batch.
        // Settle before emitting them: measuring a later batch could otherwise switch to the
        // CPU after GPU pixels have already been written, including across output modules.
        let n = PROBE_FRAMES.min(ks.len().saturating_sub(1) / 2);
        if n == 0 {
            self.decide(Choice {
                renderer: crate::JobRenderer::Gpu,
                gpu_ms: None,
                cpu_ms: None,
                why: "the project's renderer (too few pending frames to measure)",
            });
            return vec![];
        }
        let (Some(&first), Some(gpu_ks), Some(cpu_ks)) = (ks.first(), ks.get(1..1 + n), ks.get(1 + n..ks.len().min(1 + n + PROBE_CPU_FRAMES))) else {
            return vec![];
        };
        let logged = || self.frame_times.as_ref().map_or(0, |ft| ft.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len());
        let cpu_cap = self.inflight().cpu;
        let timed = |on: crate::JobRenderer, frames: &[u64]| {
            let warm = render(first, on);
            // The GPU's frames in flight settle with its warm-up frame (Auto measures it).
            // The CPU's frames are held to what the job would render at once on it, from the
            // GPU frames just measured (#1615): uncapped, they could exhaust RAM here already.
            let caps = match on {
                crate::JobRenderer::Gpu => Inflight { gpu: Some(self.gpu_inflight()), cpu: cpu_cap },
                crate::JobRenderer::Cpu => Inflight { gpu: None, cpu: cpu_cap.or_else(|| self.auto_cpu_frames()) },
            };
            let t0 = Instant::now();
            let gpu_live = || on == crate::JobRenderer::Cpu || self.job.accel.is_some_and(|a| a.available());
            let items = map_frames_while(frames.to_vec(), caps, gpu_live, |k| render(k, on), |k, i| (k, i));
            let ms = t0.elapsed().as_secs_f64() * 1e3 / frames.len().max(1) as f64;
            (std::iter::once((first, warm)).chain(items).collect::<Vec<_>>(), ms)
        };
        let log0 = logged();
        let (gpu, gpu_ms) = timed(crate::JobRenderer::Gpu, gpu_ks);
        let log1 = logged();
        let (cpu, cpu_ms) = timed(crate::JobRenderer::Cpu, cpu_ks);
        // A GPU that stopped meanwhile rendered its frames on the CPU: it is no choice.
        let gpu_ms = self.job.accel.is_some_and(|a| a.available()).then_some(gpu_ms);
        let renderer = pick_renderer(gpu_ms, Some(cpu_ms));
        // The per-frame log keeps the kept frames' times only.
        if let Some(ft) = &self.frame_times {
            let mut v = ft.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let (a, b) = if renderer == crate::JobRenderer::Gpu { (log1, v.len()) } else { (log0, log1) };
            if a <= b && b <= v.len() {
                v.drain(a..b);
            }
        }
        self.decide(Choice { renderer, gpu_ms, cpu_ms: Some(cpu_ms), why: "measured on the first frames" });
        if renderer == crate::JobRenderer::Gpu { gpu } else { cpu }
    }

    /// [`Cx::frames`] for `ks` while `encode` runs beside them on `beside`'s threads, so the
    /// render threads don't wait for the (largely sequential) encoder of the previous batch.
    /// With deferred readbacks, or where no encode thread could be started, `encode` runs first.
    #[cfg(test)]
    pub async fn frames_beside<T: Send, A: Send>(
        &self,
        beside: &Beside,
        comp: &Comp,
        ks: Vec<u64>,
        f: impl Fn(u64, Image) -> T + Sync + Send,
        encode: impl FnOnce() -> A + Send,
    ) -> crate::Result<(A, Vec<T>)> {
        self.beside(beside, self.frames(comp, ks, f), encode).await
    }

    /// Run `encode` on `beside`'s threads while `frames` renders the next batch. With deferred
    /// readbacks, or where no encode thread could be started, `encode` runs first.
    pub async fn beside<T: Send, A: Send>(
        &self,
        beside: &Beside,
        frames: impl std::future::Future<Output = Vec<T>>,
        encode: impl FnOnce() -> A + Send,
    ) -> crate::Result<(A, Vec<T>)> {
        match &beside.0 {
            Some(pool) if !self.deferred() => {
                let mut done = None;
                let frames = pool.in_place_scope(|s| {
                    s.spawn(|_| done = Some(std::panic::catch_unwind(std::panic::AssertUnwindSafe(encode))));
                    effectcraft_render::passes::block_on(frames)
                });
                match done {
                    Some(Ok(a)) => Ok((a, frames)),
                    Some(Err(panic)) => {
                        let why = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
                        Err(crate::ExportError::Encode(format!("the encoder stopped unexpectedly: {why}")))
                    }
                    None => Err(crate::ExportError::Encode("the encoder stopped unexpectedly".into())),
                }
            }
            _ => {
                let a = encode();
                Ok((a, frames.await))
            }
        }
    }

    /// GPU frames rendered at once (#879), decided once per batch: with a GPU accelerator every
    /// frame in flight holds its own working textures, so a whole batch rendered at once can
    /// exhaust VRAM (or shared memory, which crashes instead of falling back). `None` when the
    /// frames render on the CPU: no accelerator, a Software Only project, or a job that chose
    /// the CPU's compositor (#3335), whose frames render in parallel.
    fn gpu_cap(&self) -> Option<usize> {
        (self.job.accel.is_some() && self.backend() == crate::JobRenderer::Gpu).then(|| self.gpu_inflight())
    }

    /// GPU frames in flight ([`resolve_gpu_frames`]): [`GPU_INFLIGHT_DEFAULT`] until settled,
    /// which Auto is after the job's first GPU frame.
    fn gpu_inflight(&self) -> usize {
        if let Some((n, _)) = &*self.gpu_frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            return *n;
        }
        // Measured outside the lock (the allocator report walks every allocation).
        let measured = self.gpu_rendered.load(std::sync::atomic::Ordering::Relaxed).then(|| self.job.accel.and_then(|a| a.frame_memory()));
        let Some((n, why)) = resolve_gpu_frames(std::env::var(GPU_INFLIGHT_ENV).ok().as_deref(), self.job.options.gpu_frames, measured) else {
            return GPU_INFLIGHT_DEFAULT;
        };
        let mut slot = self.gpu_frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.get_or_insert_with(|| {
            log::info!(target: "effectcraft_export", "{}: {n} GPU frame(s) at once ({why})", self.job.options.label);
            (n, why)
        })
        .0
    }

    /// The GPU and CPU frames-in-flight caps every frame loop shares (#1615).
    fn inflight(&self) -> Inflight {
        Inflight { gpu: self.gpu_cap(), cpu: self.cpu_cap() }
    }

    /// CPU frames in flight (#1615): `EFFECTCRAFT_CPU_INFLIGHT`, else Auto's
    /// ([`Cx::auto_cpu_frames`]) while the job renders on the CPU (it chose the CPU's
    /// compositor, or the GPU stopped for good). `None`: not capped.
    fn cpu_cap(&self) -> Option<usize> {
        if let Some(n) = cpu_inflight(std::env::var(CPU_INFLIGHT_ENV).ok().as_deref()) {
            return Some(n);
        }
        let on_cpu = self.backend() == crate::JobRenderer::Cpu || self.job.accel.is_some_and(|a| !a.available());
        if on_cpu { self.auto_cpu_frames() } else { None }
    }

    /// Auto's CPU frames in flight ([`auto_cpu_frames`]), settled once per job from the working
    /// memory a GPU frame measured and the memory the system has free
    /// ([`crate::JobOptions::free_memory`]). `None` (not capped, as before #1615) until a GPU
    /// frame has rendered, and in a job that never renders one (a Software Rendering project):
    /// there is no per-frame estimate then.
    fn auto_cpu_frames(&self) -> Option<usize> {
        if let Some(n) = *self.cpu_frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            return n;
        }
        if !self.gpu_rendered.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        // Read outside the lock: the allocator report walks every allocation, and reading the
        // system's memory can take a second (PowerShell on Windows).
        let frame = self.job.accel.and_then(|a| a.frame_memory()).map(|m| m.frame);
        let free = self.job.options.free_memory.and_then(|read| read());
        let n = auto_cpu_frames(frame, free, rayon::current_num_threads());
        let mut slot = self.cpu_frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot.get_or_insert_with(|| {
            let mib = |b: Option<u64>| b.map_or_else(|| "unknown".to_string(), |b| format!("{} MiB", b >> 20));
            let (frame, free) = (mib(frame), mib(free));
            let label = &self.job.options.label;
            match n {
                Some(n) => log::info!(target: "effectcraft_export", "{label}: {n} CPU frame(s) at once (Auto: {frame} per frame on the GPU, {free} free)"),
                None => log::info!(target: "effectcraft_export", "{label}: CPU frames not capped (Auto: {frame} per frame on the GPU, {free} free)"),
            }
            n
        })
    }

    /// Output frame `i`: sampled (fields, pulldown), at the colour depth, cropped and resized.
    #[cfg(test)]
    pub fn frame(&self, comp: &Comp, i: u64) -> Image {
        self.frame_on(comp, i, self.backend())
    }

    /// [`Cx::frame`] on compositor `on`.
    fn frame_on(&self, comp: &Comp, i: u64, on: crate::JobRenderer) -> Image {
        effectcraft_render::passes::block_on(self.frame_async_on(comp, i, on))
    }

    /// [`Cx::frame`] as a future (deferred readbacks: the frame renders in passes).
    pub async fn frame_async(&self, comp: &Comp, i: u64) -> Image {
        self.frame_async_on(comp, i, self.backend()).await
    }

    async fn frame_async_on(&self, comp: &Comp, i: u64, on: crate::JobRenderer) -> Image {
        let t0 = Instant::now();
        let gpu = on == crate::JobRenderer::Gpu && self.job.accel.is_some();
        let img = match self.job.settings.sample(comp, i) {
            FrameSample::Progressive(t) => self.render_at(t, on).await,
            FrameSample::Fields { first, second, upper_first } => {
                let a = self.render_at(first, on).await;
                if second == first { a } else { interleave(&a, &self.render_at(second, on).await, upper_first) }
            }
        };
        if gpu {
            self.gpu_rendered.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let img = self.crop_resize(img);
        if let Some(ft) = &self.frame_times {
            ft.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((i, t0.elapsed().as_secs_f64()));
        }
        img
    }

    /// Output frame `i` as the H.264 export's 4:2:0 planes at `w`×`h`, rendered and converted
    /// by the accelerator ([`effectcraft_render::Accelerator::comp_frame_yuv420`]). Only for
    /// frames that need nothing done after the render (progressive, at the project's depth, no
    /// crop or resize, rendered at exactly `w`×`h`). `None`: render it with [`Cx::frame`].
    fn yuv420_frame(&self, comp: &Comp, i: u64, w: u32, h: u32, on: crate::JobRenderer) -> Option<Vec<u8>> {
        if self.deferred() {
            return None;
        }
        let FrameSample::Progressive(t) = self.job.settings.sample(comp, i) else { return None };
        // Above 8 bpc the colour is dithered on its way to 8 bits (`Cx::pixels`), which the
        // accelerator's conversion doesn't do: those frames render as usual so H.264 doesn't
        // band (#2022). The project's depth already includes Render Settings ▸ Color Depth.
        if self.project.settings.bit_depth != BitDepth::Bpc8 {
            return None;
        }
        let om = self.job.output;
        let scale = self.job.settings.effective_scale(comp);
        let size = |n: u32| ((n as f64 * scale).round() as u32).max(1);
        if om.resize.enabled || (size(comp.width), size(comp.height)) != (w, h) || om.crop.edges(w, h, scale) != (0, 0, 0, 0) {
            return None;
        }
        let t0 = Instant::now();
        let r = self.renderer_on(on);
        let a = r.active_accel()?;
        let planes = a.comp_frame_yuv420(&r, self.job.comp, t, comp.background)?;
        let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
        if planes.len() != w as usize * h as usize + 2 * cw * ch {
            return None;
        }
        // A frame rendered off `passes::comp_frame` ends here; on a `None` the fallback renders and ends it.
        a.frame_end();
        self.gpu_rendered.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(ft) = &self.frame_times {
            ft.lock().unwrap_or_else(|e| e.into_inner()).push((i, t0.elapsed().as_secs_f64()));
        }
        Some(planes)
    }

    /// Output frames `ks` for the H.264 encoder: the accelerator's planes
    /// ([`Cx::yuv420_frame`]) where it can, otherwise `fallback` of the frame rendered as usual.
    pub(crate) fn h264_frames<T: Send>(
        &self,
        comp: &Comp,
        ks: Vec<u64>,
        w: u32,
        h: u32,
        planes: impl Fn(Vec<u8>) -> T + Sync + Send,
        fallback: impl Fn(u64, Image) -> T + Sync + Send,
    ) -> Vec<T> {
        // As in `frames`: once the GPU has stopped for good, the rest render in parallel.
        self.render_frames(
            ks,
            |k, on| self.yuv420_frame(comp, k, w, h, on).ok_or_else(|| self.frame_on(comp, k, on)),
            |k, r| match r {
                Ok(p) => planes(p),
                Err(img) => fallback(k, img),
            },
        )
    }

    /// Output Module ▸ Crop, then Resize.
    fn crop_resize(&self, img: Image) -> Image {
        let om = self.job.output;
        let scale = self.comp().map(|c| self.job.settings.effective_scale(c)).unwrap_or_else(|| self.job.settings.resolution.clamp(0.01, 4.0));
        // frame_size applies crop itself: use the original frame and the same scale as edges.
        let (w, h) = om.frame_size(img.width, img.height, scale);
        let (t, l, b, r) = om.crop.edges(img.width, img.height, scale);
        let img = if (t, l, b, r) == (0, 0, 0, 0) { img } else { crop(&img, t, l, b, r) };
        if !om.resize.enabled {
            return img;
        }
        if (w, h) == (img.width, img.height) {
            return img;
        }
        match om.resize.quality {
            ResizeQuality::Low => effectcraft_raster::warp::resample(&img, w, h),
            ResizeQuality::High => bicubic(&img, w, h),
        }
    }

    /// 8-bit RGBA of the requested channels cropped/padded to `w`×`h` (codec rounding): RGB over
    /// the comp background (opaque), RGB + Alpha straight or premultiplied, or the alpha matte.
    /// When the render runs above 8 bpc, the colour is dithered on its way to 8 bits, as After
    /// Effects dithers colours converted down to 8 bpc for rendering and export, so gradients
    /// rendered at 16 or 32 bpc don't band (#2022). An 8 bpc render has no finer values to
    /// dither, so its output stays byte for byte what it was.
    pub fn pixels(&self, img: &Image, comp: &Comp, channels: Channels, w: u32, h: u32) -> Vec<u8> {
        let dither = self.project.settings.bit_depth != BitDepth::Bpc8;
        self.pixels_with(img, comp, channels, w, h, dither)
    }

    /// [`Cx::pixels`], dithering the colour or not (a GIF palette gains nothing from dither noise).
    pub fn pixels_with(&self, img: &Image, comp: &Comp, channels: Channels, w: u32, h: u32, dither: bool) -> Vec<u8> {
        let px = match channels {
            Channels::Rgb if dither => img.to_rgba8_over_dithered(comp.background),
            Channels::Rgb => img.to_rgba8_over(comp.background),
            Channels::Rgba => match (self.job.output.alpha_mode, dither) {
                (AlphaMode::Straight, true) => img.to_rgba8_dithered(),
                (AlphaMode::Straight, false) => img.to_rgba8(),
                (AlphaMode::Premultiplied, _) => premultiplied8(img, dither),
            },
            Channels::Alpha => img
                .data
                .iter()
                .flat_map(|p| {
                    let a = (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    [a, a, a, 255]
                })
                .collect(),
        };
        crate::fit(px, img.width, img.height, w, h)
    }

    /// Open a file and remember ownership for cancellation cleanup (including overflow paths).
    pub fn create(&self, path: &str) -> crate::Result<out::Out<'_>> {
        let file = out::create(self.sink, path)?;
        if self.sink.is_none() {
            self.written.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(path.to_owned());
        }
        Ok(file)
    }

    pub fn remove_cancelled_outputs(&self) -> crate::Result<()> {
        let mut failure = None;
        for path in self.written.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter() {
            if let Err(e) = std::fs::remove_file(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                failure.get_or_insert_with(|| crate::ExportError::Io(format!("cannot remove cancelled output {path}: {e}")));
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Whether Skip Existing finds `path` already written: at its own place or, with Use Storage
    /// Overflow, in any overflow folder (checked before a frame is placed, so a skipped frame
    /// reserves no quota).
    pub fn frame_exists(&self, path: &str) -> bool {
        if out::exists(self.sink, path) {
            return true;
        }
        if !self.job.settings.storage_overflow {
            return false;
        }
        let Some(name) = std::path::Path::new(path).file_name() else { return false };
        self.job.options.overflow.iter().any(|dir| out::exists(self.sink, &std::path::Path::new(dir).join(name).to_string_lossy()))
    }

    /// Where a file goes: `path`, or (Use Storage Overflow) the first overflow folder with room
    /// for `bytes` when the quota hook says the output volume is full.
    pub fn place(&self, path: &str, bytes: u64) -> String {
        let Some(q) = self.job.options.storage else { return path.to_string() };
        if !self.job.settings.storage_overflow || q.has_room(path, bytes) {
            return path.to_string();
        }
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        for dir in &self.job.options.overflow {
            let p = std::path::Path::new(dir).join(&name).to_string_lossy().to_string();
            if q.has_room(&p, bytes) {
                self.overflowed.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(p.clone());
                return p;
            }
        }
        path.to_string()
    }

    /// Write the render log next to the output, as the Log setting asks. Returns its path.
    pub fn write_log(&self, result: &Result<crate::Report, crate::ExportError>, seconds: f64) -> Option<String> {
        let level = self.job.options.log;
        // Errors Only: a log for failures (a user stop is not an error).
        if level == RenderLog::ErrorsOnly && matches!(result, Ok(_) | Err(crate::ExportError::Cancelled)) {
            return None;
        }
        let comp = self.comp();
        let path = log_path(self.job.path);
        let mut s = String::new();
        s.push_str("EffectCraft Render Log\n");
        s.push_str(&format!("Item: {}\n", if self.job.options.label.is_empty() { "(render)" } else { &self.job.options.label }));
        s.push_str(&format!("Output: {}\n", self.job.path));
        match result {
            Ok(r) => s.push_str(&format!("Result: Done ({} frames, {}×{}, {} bytes) in {seconds:.2} s\n", r.frames, r.width, r.height, r.bytes)),
            Err(e) => s.push_str(&format!("Result: Failed\nError: {e}\n")),
        }
        if let Some(c) = self.choice() {
            s.push_str(&format!("Renderer: {}\n", describe_choice(&c)));
        }
        if self.backend() == crate::JobRenderer::Gpu
            && let Some((n, why)) = &*self.gpu_frames.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            s.push_str(&format!("GPU frames rendered at once: {n} ({why})\n"));
        }
        let over = self.overflowed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !over.is_empty() {
            s.push_str(&format!("Storage overflow: {} file(s) written to overflow folders (first: {})\n", over.len(), over[0]));
        }
        if level != RenderLog::ErrorsOnly {
            s.push_str("\nRender Settings:\n");
            for (k, v) in self.job.settings.describe(comp) {
                s.push_str(&format!("  {k}: {v}\n"));
            }
            s.push_str("\nOutput Module:\n");
            for (k, v) in self.job.output.describe() {
                s.push_str(&format!("  {k}: {v}\n"));
            }
        }
        if let (RenderLog::PlusPerFrameInfo, Some(ft), Some(c)) = (level, &self.frame_times, comp) {
            let mut v = ft.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            v.sort_by_key(|(i, _)| *i);
            s.push_str("\nFrames:\n");
            let f0 = self.job.settings.first_frame(c);
            for (i, secs) in v {
                let t = self.job.settings.frame_time(c, i);
                s.push_str(&format!("  Frame {} ({:.3} s): rendered in {:.1} ms\n", f0 + i as i64, t.seconds(), secs * 1000.0));
            }
        }
        let mut f = self.create(&path).ok()?;
        std::io::Write::write_all(&mut f, s.as_bytes()).ok()?;
        f.finish().ok()?;
        Some(path)
    }
}

/// The render log's account of a job's compositor: which, why, and what was measured.
pub(crate) fn describe_choice(c: &Choice) -> String {
    let name = match c.renderer {
        crate::JobRenderer::Gpu => "GPU Acceleration",
        crate::JobRenderer::Cpu => "Software Rendering",
    };
    let ms = |v: Option<f64>| v.map_or_else(|| "n/a".to_string(), |v| format!("{v:.1} ms/frame"));
    if c.gpu_ms.is_none() && c.cpu_ms.is_none() {
        format!("{name} ({})", c.why)
    } else {
        format!("{name} ({}: GPU {}, CPU {})", c.why, ms(c.gpu_ms), ms(c.cpu_ms))
    }
}

/// Weave two fields: the dominant field's lines (even lines when `upper_first`) from `first`.
pub(crate) fn interleave(first: &Image, second: &Image, upper_first: bool) -> Image {
    let mut out = first.clone();
    let w = out.width as usize;
    for y in 0..out.height as usize {
        let from_first = (y % 2 == 0) == upper_first;
        if !from_first && y < second.height as usize && second.width as usize == w {
            out.data[y * w..(y + 1) * w].copy_from_slice(&second.data[y * w..(y + 1) * w]);
        }
    }
    out
}

/// Remove `t`/`l`/`b`/`r` pixels from the edges (negative values add transparent pixels).
pub(crate) fn crop(img: &Image, t: i32, l: i32, b: i32, r: i32) -> Image {
    let w = (img.width as i32 - l - r).max(1) as u32;
    let h = (img.height as i32 - t - b).max(1) as u32;
    let mut out = Image::new(w, h);
    for y in 0..h as i32 {
        let sy = y + t;
        if sy < 0 || sy >= img.height as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x + l;
            if sx >= 0 && sx < img.width as i32 {
                let o = out.idx(x as u32, y as u32);
                out.data[o] = img.data[img.idx(sx as u32, sy as u32)];
            }
        }
    }
    out
}

/// Bicubic resize (Resize Quality: High).
fn bicubic(src: &Image, w: u32, h: u32) -> Image {
    // Shrinking a lot: bilinear with its box pre-filter first, then bicubic for the last step.
    if src.width > w * 2 && src.height > h * 2 {
        let mid = effectcraft_raster::warp::resample(src, w * 2, h * 2);
        return bicubic(&mid, w, h);
    }
    let mut out = Image::new(w, h);
    let (sx, sy) = (src.width as f64 / w as f64, src.height as f64 / h as f64);
    for y in 0..h {
        for x in 0..w {
            let o = out.idx(x, y);
            // Clamp to the edge so borders stay opaque.
            let fx = ((x as f64 + 0.5) * sx).clamp(0.5, src.width as f64 - 0.5);
            let fy = ((y as f64 + 0.5) * sy).clamp(0.5, src.height as f64 - 0.5);
            let mut p = src.sample_bicubic(fx, fy);
            p[3] = p[3].min(1.0);
            out.data[o] = p;
        }
    }
    out
}

/// Premultiplied 8-bit RGBA (colour matted with black), the colour dithered when `dither`.
fn premultiplied8(img: &Image, dither: bool) -> Vec<u8> {
    let w = img.width.max(1) as usize;
    let mut out = Vec::with_capacity(img.data.len() * 4);
    for (i, p) in img.data.iter().enumerate() {
        let a = p[3].clamp(0.0, 1.0);
        let a8 = (a * 255.0 + 0.5) as u8;
        let (x, y) = ((i % w) as u32, (i / w) as u32);
        for c in 0..3 {
            let v = p[c].clamp(0.0, a);
            // Dither may round up past the rounded alpha; premultiplied colour never exceeds it.
            out.push(if dither { effectcraft_raster::dither_u8(v, x, y).min(a8) } else { (v * 255.0 + 0.5) as u8 });
        }
        out.push(a8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// Dither may round a colour up while alpha rounds down; premultiplied colour stays at or
    /// below the written alpha at every pixel.
    #[test]
    fn dithered_premultiplied_colour_never_exceeds_alpha() {
        let v = 100.25 / 255.0;
        let img = Image::filled(64, 64, [v, v, v, v]);
        let px = premultiplied8(&img, true);
        assert!(px.as_chunks::<4>().0.iter().all(|p| p[..3].iter().all(|c| *c <= p[3])), "a colour byte is above its alpha");
        assert!(px.as_chunks::<4>().0.iter().all(|p| p[3] == 100));
        let plain = premultiplied8(&img, false);
        assert!(plain.as_chunks::<4>().0.iter().all(|p| *p == [100, 100, 100, 100]));
    }

    /// Frames `0..n` through [`map_frames`] with `cap`, recording the most running at once.
    fn run_capped(n: u64, cap: Option<usize>) -> (Vec<u64>, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (now, max) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let out = map_frames(
            (0..n).collect(),
            cap,
            |k| {
                let c = now.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(c, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(2));
                now.fetch_sub(1, Ordering::SeqCst);
                k
            },
            |k, img| {
                assert_eq!(k, img);
                k * 10
            },
        );
        (out, max.load(Ordering::SeqCst))
    }

    #[test]
    fn frames_render_in_parallel_once_the_gpu_stops() {
        // Every frame after the GPU stopped falls back to the CPU: the cap no longer applies.
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let (live, now, max) = (AtomicBool::new(true), AtomicUsize::new(0), AtomicUsize::new(0));
        let out = pool.install(|| {
            map_frames_while(
                (0..24).collect(),
                Inflight { gpu: Some(1), cpu: None },
                || live.load(Ordering::SeqCst),
                |k| {
                    if k == 1 {
                        live.store(false, Ordering::SeqCst);
                    }
                    let c = now.fetch_add(1, Ordering::SeqCst) + 1;
                    if k >= 2 {
                        max.fetch_max(c, Ordering::SeqCst);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                    k * 10
                },
                |_, v| v,
            )
        });
        assert_eq!(out, (0..24).map(|k| k * 10).collect::<Vec<_>>());
        assert!(max.load(Ordering::SeqCst) > 1, "frames after the GPU stopped ran one at a time");
    }

    #[test]
    fn capped_frames_keep_their_order() {
        let want: Vec<u64> = (0..40).map(|k| k * 10).collect();
        for cap in [None, Some(1), Some(3), Some(7)] {
            assert_eq!(run_capped(40, cap).0, want, "cap {cap:?}");
        }
    }

    #[test]
    fn gpu_frames_in_flight_respect_the_cap() {
        // #879: every frame of a batch rendered on the GPU at once and exhausted VRAM.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        pool.install(|| {
            for cap in [1, 2, 3] {
                let (out, max) = run_capped(30, Some(cap));
                assert_eq!(out.len(), 30);
                assert!(max <= cap, "cap {cap}: {max} frames ran at once");
            }
        });
    }

    #[test]
    fn a_cap_above_the_frame_count_renders_them_all() {
        let (out, max) = run_capped(2, Some(8));
        assert_eq!(out, vec![0, 10]);
        assert!(max <= 2);
        assert!(run_capped(0, Some(3)).0.is_empty());
    }

    #[test]
    fn cap_one_on_a_single_thread_does_not_deadlock() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let (out, max) = pool.install(|| run_capped(10, Some(1)));
        assert_eq!(out.len(), 10);
        assert_eq!(max, 1);
        // Nested parallelism inside a frame (as the renderer has) on one thread.
        let out = pool.install(|| map_frames((0..6).collect(), Some(2), |k| (0..4u64).into_par_iter().map(|i| i + k).sum::<u64>(), |_, v| v));
        assert_eq!(out, (0..6).map(|k| 6 + 4 * k).collect::<Vec<_>>());
    }

    #[test]
    fn a_failing_frame_does_not_hold_up_the_rest() {
        let out = map_frames((0..9).collect(), Some(1), |k| if k == 3 { Err(k) } else { Ok(k) }, |_, r| r);
        assert_eq!(out.iter().filter(|r| r.is_err()).count(), 1);
        assert_eq!(out.len(), 9);
        assert_eq!(out[8], Ok(8));
        // A panicking frame propagates once and leaves no slot behind: the next batch runs.
        let r = std::panic::catch_unwind(|| map_frames((0..6).collect(), Some(2), |k| if k == 2 { panic!("frame {k}") } else { k }, |_, k| k));
        assert!(r.is_err());
        assert_eq!(run_capped(6, Some(2)).0.len(), 6);
    }

    #[test]
    fn encoding_is_not_capped_with_the_render() {
        // Only the GPU render is serialised: a window of frames encodes in parallel.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let (now, max) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let out = pool.install(|| {
            map_frames(
                (0..8).collect(),
                Some(1),
                |k| k,
                |k, _| {
                    let c = now.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(c, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    now.fetch_sub(1, Ordering::SeqCst);
                    k
                },
            )
        });
        assert_eq!(out, (0..8).collect::<Vec<_>>());
        assert!(max.load(Ordering::SeqCst) > 1, "encodes ran one at a time");
    }

    /// An accelerator that renders nothing itself (the tests only check whether frames are capped).
    struct NoAccel;
    impl effectcraft_render::Accelerator for NoAccel {
        fn name(&self) -> String {
            "none".into()
        }
        fn comp_frame(&self, _r: &Renderer, _comp: effectcraft_project::ItemId, _t: Tick) -> Option<Image> {
            None
        }
        fn supports_effect(&self, _id: &str) -> bool {
            false
        }
        fn effects(&self, _chain: &[effectcraft_render::FxStep], _buf: &effectcraft_effects::Buf, _levels: Option<f32>) -> Option<effectcraft_effects::Buf> {
            None
        }
    }

    #[test]
    fn software_only_projects_are_not_capped() {
        // Review of #879: an accelerator alone capped Software Only projects, which render on the CPU.
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        use effectcraft_render::NoFootage;
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let accel = NoAccel;
        for (gpu, want) in [(false, None), (true, Some(GPU_INFLIGHT_DEFAULT))] {
            let mut p = Project::default();
            p.settings.gpu_acceleration = gpu;
            let job = Job {
                project: &p,
                footage: &NoFootage,
                expr: None,
                accel: Some(&accel),
                comp: effectcraft_project::ItemId(0),
                settings: &settings,
                output: &output,
                path: "/c/f_[#####].png",
                sink: None,
                nested_switches: true,
                options: Default::default(),
            };
            if std::env::var_os(GPU_INFLIGHT_ENV).is_none() {
                assert_eq!(Cx::new(&job).gpu_cap(), want, "gpu acceleration {gpu}");
            } else {
                assert_eq!(Cx::new(&job).gpu_cap().is_some(), gpu);
            }
        }
        let p = Project::default();
        let job = Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: effectcraft_project::ItemId(0),
            settings: &settings,
            output: &output,
            path: "/c/f_[#####].png",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        assert_eq!(Cx::new(&job).gpu_cap(), None, "no accelerator");
    }

    /// A resumed sequence can start with one or two missing frames. Once emitted, their
    /// compositor must also be used by every later batch and the item's other output modules.
    #[test]
    fn a_small_first_batch_settles_the_renderer_before_emitting_frames() {
        if std::env::var_os(RENDERER_ENV).is_some() {
            return;
        }
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let mut p = Project::default();
        p.settings.gpu_acceleration = true;
        let comp = Comp::new(16, 8, effectcraft_time::FrameRate::FPS_25, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, effectcraft_project::ItemKind::Comp(comp.into()));
        let accel = NoAccel;
        for n in [1, 2] {
            let shared = Mutex::new(None);
            let job = Job {
                project: &p,
                footage: &effectcraft_render::NoFootage,
                expr: None,
                accel: Some(&accel),
                comp: cid,
                settings: &settings,
                output: &output,
                path: "/r/f_[#####].png",
                sink: None,
                nested_switches: true,
                options: crate::JobOptions { renderer: Some(&shared), ..Default::default() },
            };
            let cx = Cx::new(&job);
            let empty = cx.render_frames(Vec::new(), |_, on| on, |_, on| on);
            assert!(empty.is_empty());
            assert_eq!(cx.choice(), None, "skipped batches do not choose");
            let first = cx.render_frames((0..n).collect(), |_, on| on, |_, on| on);
            assert_eq!(first, vec![crate::JobRenderer::Gpu; n as usize]);
            assert_eq!(*shared.lock().unwrap(), Some(Backend::Gpu), "settled before any pixels are emitted");
            let later = cx.render_frames((n..12).collect(), |_, on| on, |_, on| on);
            assert!(later.iter().all(|on| *on == crate::JobRenderer::Gpu));
            assert_eq!(Cx::new(&job).backend(), crate::JobRenderer::Gpu, "the next output module follows");
        }
    }

    #[test]
    fn a_job_that_chose_the_cpu_is_not_capped() {
        // #3335: the CPU's frame-parallel compositor is the point of choosing it; a GPU project
        // that measured the CPU faster must not keep rendering one frame at a time.
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let accel = NoAccel;
        let mut p = Project::default();
        p.settings.gpu_acceleration = true;
        let job = Job {
            project: &p,
            footage: &effectcraft_render::NoFootage,
            expr: None,
            accel: Some(&accel),
            comp: effectcraft_project::ItemId(0),
            settings: &settings,
            output: &output,
            path: "/c/f_[#####].png",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        let cx = Cx::new(&job);
        if std::env::var_os(RENDERER_ENV).is_none() {
            assert_eq!(cx.choice(), None, "a GPU project measures first");
            assert!(cx.gpu_cap().is_some());
        }
        cx.decide(Choice { renderer: crate::JobRenderer::Cpu, gpu_ms: Some(117.0), cpu_ms: Some(68.0), why: "measured on the first frames" });
        assert_eq!(cx.backend(), crate::JobRenderer::Cpu);
        assert_eq!(cx.gpu_cap(), None);
        assert_eq!(cx.renderer().active_accel().map(|a| a.name()), None, "CPU frames don't use the accelerator");
        assert_eq!(describe_choice(&cx.choice().unwrap()), "Software Rendering (measured on the first frames: GPU 117.0 ms/frame, CPU 68.0 ms/frame)");
    }

    #[test]
    fn auto_cpu_frames_fit_half_the_free_memory() {
        // #1615: 16 threads at 1378 MiB a frame (a GTX 1050 Ti user's comp) with 10 GiB free
        // render 3 frames at once, not 16.
        assert_eq!(auto_cpu_frames(Some(1378 << 20), Some(10 << 30), 16), Some(3));
        assert_eq!(auto_cpu_frames(Some(1 << 28), Some(64 << 30), 16), Some(16), "at most one per thread");
        assert_eq!(auto_cpu_frames(Some(4 << 30), Some(2 << 30), 16), Some(1), "at least one");
        assert_eq!(auto_cpu_frames(Some(1 << 30), Some(0), 8), Some(1));
        assert_eq!(auto_cpu_frames(Some(1 << 20), Some(u64::MAX), 0), Some(1));
        // Unknown: not capped, as before.
        assert_eq!(auto_cpu_frames(None, Some(8 << 30), 16), None);
        assert_eq!(auto_cpu_frames(Some(1 << 30), None, 16), None);
        assert_eq!(auto_cpu_frames(Some(0), Some(8 << 30), 16), None);
    }

    #[test]
    fn a_job_that_chose_the_cpu_after_measuring_the_gpu_caps_its_frames() {
        // #1615: a GPU project that measured the CPU faster rendered one frame per thread at
        // once, so a heavy comp ran out of RAM (an exit without an error, 0.23.1 on Windows).
        // Its frames now fit half the free memory at the GPU's measured size per frame.
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        if std::env::var_os(CPU_INFLIGHT_ENV).is_some() || std::env::var_os(RENDERER_ENV).is_some() {
            return;
        }
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let mut p = Project::default();
        p.settings.gpu_acceleration = true;
        let comp = Comp::new(16, 8, effectcraft_time::FrameRate::FPS_25, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, effectcraft_project::ItemKind::Comp(comp.into()));
        let accel = MemAccel(effectcraft_render::FrameMemory { budget: 3 << 30, peak: 2 << 30, frame: 1 << 30, reserve: 0 });
        let reads = AtomicUsize::new(0);
        let free = || {
            reads.fetch_add(1, Ordering::SeqCst);
            Some(6u64 << 30)
        };
        let job = Job {
            project: &p,
            footage: &effectcraft_render::NoFootage,
            expr: None,
            accel: Some(&accel),
            comp: cid,
            settings: &settings,
            output: &output,
            path: "/c/f_[#####].png",
            sink: None,
            nested_switches: true,
            options: crate::JobOptions { free_memory: Some(&free), ..Default::default() },
        };
        let pool = rayon::ThreadPoolBuilder::new().num_threads(16).build().unwrap();
        pool.install(|| {
            let cx = Cx::new(&job);
            assert_eq!(cx.inflight().cpu, None, "nothing measured yet");
            let c = cx.comp().unwrap();
            cx.frame_on(c, 0, crate::JobRenderer::Gpu);
            assert_eq!(cx.inflight().cpu, None, "the GPU renders: its own cap holds");
            assert_eq!(reads.load(Ordering::SeqCst), 0, "the free memory is read only for CPU frames");
            cx.decide(Choice { renderer: crate::JobRenderer::Cpu, gpu_ms: Some(1417.3), cpu_ms: Some(655.8), why: "measured on the first frames" });
            assert_eq!(cx.inflight(), Inflight { gpu: None, cpu: Some(3) }, "3 GiB of 6 GiB free at 1 GiB a frame");
            assert_eq!(cx.inflight().cpu, Some(3));
            assert_eq!(reads.load(Ordering::SeqCst), 1, "settled once per job");
        });
        // Without a free-memory reading the job renders uncapped, as before.
        let job = Job { options: Default::default(), ..job };
        let cx = Cx::new(&job);
        cx.frame_on(cx.comp().unwrap(), 0, crate::JobRenderer::Gpu);
        cx.decide(Choice { renderer: crate::JobRenderer::Cpu, gpu_ms: Some(2.0), cpu_ms: Some(1.0), why: "measured on the first frames" });
        assert_eq!(cx.inflight().cpu, None);
    }

    #[test]
    fn cpu_inflight_is_off_unless_a_positive_number() {
        assert_eq!(cpu_inflight(None), None);
        assert_eq!(cpu_inflight(Some("2")), Some(2));
        assert_eq!(cpu_inflight(Some(" 4 ")), Some(4));
        for bad in ["0", "", "  ", "-2", "three", "2.5", "99999999999999999999999"] {
            assert_eq!(cpu_inflight(Some(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn cpu_cap_holds_and_combines_with_the_gpu_cap() {
        // #1615: CPU renders had no cap; a GPU cap alone still lapses when the GPU stops.
        let caps = |gpu, cpu| Inflight { gpu, cpu };
        for live in [true, false] {
            assert_eq!(caps(None, None).renders(live), None);
            assert_eq!(caps(None, Some(2)).renders(live), Some(2));
        }
        assert_eq!(caps(Some(1), None).renders(true), Some(1));
        assert_eq!(caps(Some(1), None).renders(false), None);
        assert_eq!(caps(Some(3), Some(2)).renders(true), Some(2));
        assert_eq!(caps(Some(1), Some(4)).renders(true), Some(1));
        // Review of #1615: once the GPU stops, the CPU cap applies, not the GPU's 1.
        assert_eq!(caps(Some(1), Some(4)).renders(false), Some(4));
        // A CPU cap of 2 on 8 threads renders at most 2 frames at once.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        let (now, max) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let out = pool.install(|| map_frames_while((0..16).collect(), caps(None, Some(2)), || true, |k| track(&now, &max, k), |_, k| k));
        assert_eq!(out, (0..16).collect::<Vec<_>>());
        assert!(max.load(Ordering::SeqCst) <= 2, "{max:?} frames in flight");
    }

    /// Counts `k` in flight in `now`, records the most in `max`, and holds it for a moment.
    fn track(now: &AtomicUsize, max: &AtomicUsize, k: u64) -> u64 {
        let c = now.fetch_add(1, Ordering::SeqCst) + 1;
        max.fetch_max(c, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(5));
        now.fetch_sub(1, Ordering::SeqCst);
        k
    }

    #[test]
    fn a_cpu_cap_bounds_the_images_waiting_to_encode() {
        // Review of #1615: a window of one image per thread was rendered before any encoded, so
        // a cap of 2 on 8 threads still held 8 finished images.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        for gpu in [None, Some(1)] {
            let (held, max) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let out = pool.install(|| {
                map_frames_while(
                    (0..20).collect(),
                    Inflight { gpu, cpu: Some(2) },
                    || true,
                    |k| {
                        let c = held.fetch_add(1, Ordering::SeqCst) + 1;
                        max.fetch_max(c, Ordering::SeqCst);
                        k
                    },
                    |k, img| {
                        held.fetch_sub(1, Ordering::SeqCst);
                        assert_eq!(k, img);
                        k
                    },
                )
            });
            assert_eq!(out, (0..20).collect::<Vec<_>>());
            assert!(max.load(Ordering::SeqCst) <= 2, "gpu {gpu:?}: {max:?} images held");
        }
    }

    #[test]
    fn the_cpu_cap_takes_over_once_the_gpu_stops() {
        // Review of #1615: with both caps, frames after the GPU stopped rendered one at a time.
        let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build().unwrap();
        let (live, now, max) = (AtomicBool::new(true), AtomicUsize::new(0), AtomicUsize::new(0));
        let out = pool.install(|| {
            map_frames_while(
                (0..40).collect(),
                Inflight { gpu: Some(1), cpu: Some(4) },
                || live.load(Ordering::SeqCst),
                |k| {
                    if k == 1 {
                        live.store(false, Ordering::SeqCst);
                    }
                    if k < 4 {
                        return k;
                    }
                    track(&now, &max, k)
                },
                |_, k| k,
            )
        });
        assert_eq!(out, (0..40).collect::<Vec<_>>());
        let max = max.load(Ordering::SeqCst);
        assert!((2..=4).contains(&max), "{max} frames in flight after the GPU stopped");
    }

    #[test]
    fn gpu_inflight_override_is_a_positive_number_or_ignored() {
        assert_eq!(gpu_inflight_env(None), None);
        assert_eq!(gpu_inflight_env(Some("3")), Some(3));
        assert_eq!(gpu_inflight_env(Some(" 5 ")), Some(5));
        for bad in ["0", "", "-2", "three", "2.5", "99999999999999999999999"] {
            assert_eq!(gpu_inflight_env(Some(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn auto_gpu_frames_follow_the_memory_budget() {
        // #3335: Auto renders as many GPU frames at once as the budget has room for, 1 to 3.
        use effectcraft_render::FrameMemory;
        const GIB: u64 = 1 << 30;
        let m = |budget: u64, peak: u64, frame: u64, reserve: u64| Some(FrameMemory { budget, peak, frame, reserve });
        // Unknown memory (no allocator report, no budget): one frame.
        assert_eq!(auto_gpu_frames(None), 1);
        assert_eq!(auto_gpu_frames(m(0, GIB, GIB / 2, 0)), 1);
        assert_eq!(auto_gpu_frames(m(3 * GIB, 0, GIB / 2, 0)), 1);
        assert_eq!(auto_gpu_frames(m(3 * GIB, GIB, 0, 0)), 1);
        // #879 on an 8 GB card: one frame peaked at 6.3 GB, over the 3 GiB budget: one.
        assert_eq!(auto_gpu_frames(m(3 * GIB, 63 * GIB / 10, 5 * GIB, 0)), 1);
        // Frames that had to wait for memory report the whole budget per frame: one.
        assert_eq!(auto_gpu_frames(m(3 * GIB, GIB, 3 * GIB, 0)), 1);
        // Room for half a frame more, or room the upload cache may still take: one.
        assert_eq!(auto_gpu_frames(m(2 * GIB, 3 * GIB / 2, GIB, 0)), 1);
        assert_eq!(auto_gpu_frames(m(3 * GIB, GIB, GIB / 2, 3 * GIB / 2)), 2);
        assert_eq!(auto_gpu_frames(m(3 * GIB, GIB, GIB / 2, 2 * GIB)), 1);
        // Room for one more frame: two; ample room: three, never more.
        assert_eq!(auto_gpu_frames(m(3 * GIB, 2 * GIB, GIB, 0)), 2);
        assert_eq!(auto_gpu_frames(m(3 * GIB, GIB / 2, GIB / 4, 0)), 3);
        assert_eq!(auto_gpu_frames(m(u64::MAX, 1, 1, 0)), 3);
    }

    #[test]
    fn gpu_frames_come_from_the_environment_the_setting_or_auto() {
        use effectcraft_render::FrameMemory;
        let ample = Some(Some(FrameMemory { budget: 3 << 30, peak: 1 << 29, frame: 1 << 28, reserve: 1 << 28 }));
        let n = |env, pref, measured| resolve_gpu_frames(env, pref, measured).map(|(n, _)| n);
        // Auto: unsettled until a GPU frame rendered, then measured.
        assert_eq!(n(None, None, None), None);
        assert_eq!(n(None, None, Some(None)), Some(1));
        assert_eq!(n(None, None, ample), Some(3));
        // The setting's 1, 2 or 3, without measuring.
        for k in 1..=3 {
            assert_eq!(n(None, Some(k), None), Some(k));
            assert_eq!(n(None, Some(k), ample), Some(k));
        }
        // The environment overrides both; a bad value is ignored.
        assert_eq!(n(Some("2"), Some(1), None), Some(2));
        assert_eq!(n(Some("4"), None, ample), Some(4));
        assert_eq!(n(Some("zero"), Some(1), None), Some(1));
        let (_, why) = resolve_gpu_frames(None, None, ample).unwrap();
        assert_eq!(why, "Auto: 256 MiB per frame, 512 MiB peak and 256 MiB cache reserve in a 3072 MiB budget");
    }

    /// An accelerator reporting `memory` once asked (renders nothing itself).
    struct MemAccel(effectcraft_render::FrameMemory);
    impl effectcraft_render::Accelerator for MemAccel {
        fn name(&self) -> String {
            "mem".into()
        }
        fn comp_frame(&self, _r: &Renderer, _comp: effectcraft_project::ItemId, _t: Tick) -> Option<Image> {
            None
        }
        fn supports_effect(&self, _id: &str) -> bool {
            false
        }
        fn effects(&self, _chain: &[effectcraft_render::FxStep], _buf: &effectcraft_effects::Buf, _levels: Option<f32>) -> Option<effectcraft_effects::Buf> {
            None
        }
        fn frame_memory(&self) -> Option<effectcraft_render::FrameMemory> {
            Some(self.0)
        }
    }

    #[test]
    fn auto_gpu_frames_settle_after_the_first_gpu_frame() {
        // #3335: one GPU frame at a time until a frame measured the memory, then Auto's count
        // for the rest of the job; the setting skips the measurement.
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        if std::env::var_os(GPU_INFLIGHT_ENV).is_some() || std::env::var_os(RENDERER_ENV).is_some() {
            return;
        }
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let mut p = Project::default();
        p.settings.gpu_acceleration = true;
        let comp = Comp::new(16, 8, effectcraft_time::FrameRate::FPS_25, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, effectcraft_project::ItemKind::Comp(comp.into()));
        let accel = MemAccel(effectcraft_render::FrameMemory { budget: 3 << 30, peak: 1 << 29, frame: 1 << 28, reserve: 0 });
        for (pref, want) in [(None, 3), (Some(2), 2), (Some(1), 1)] {
            let job = Job {
                project: &p,
                footage: &effectcraft_render::NoFootage,
                expr: None,
                accel: Some(&accel),
                comp: cid,
                settings: &settings,
                output: &output,
                path: "/c/f_[#####].png",
                sink: None,
                nested_switches: true,
                options: crate::JobOptions { gpu_frames: pref, ..Default::default() },
            };
            let cx = Cx::new(&job);
            if pref.is_none() {
                assert_eq!(cx.gpu_cap(), Some(1), "before any GPU frame");
            }
            let c = cx.comp().unwrap();
            cx.frame_on(c, 0, crate::JobRenderer::Gpu);
            assert_eq!(cx.gpu_cap(), Some(want), "setting {pref:?}");
        }
    }

    #[test]
    fn layers_render_once_per_job() {
        // #413: a job rendered every layer of every frame from scratch (no layer cache).
        use effectcraft_color::Label;
        use effectcraft_keyframe::{Keyframe, Value};
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        use effectcraft_project::{ItemKind, LayerSource, Solid, build};
        use effectcraft_render::NoFootage;
        use effectcraft_time::FrameRate;
        // A solid animated only in Transform: its pixels render once, then move.
        let mut p = Project::default();
        let mut comp = Comp::new(64, 48, FrameRate::FPS_25, Tick::from_seconds_f64(1.0));
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 16, height: 16, pixel_aspect: 1.0 }));
        let mut l = build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (16, 16), None);
        l.props.prop_mut("transform/position").unwrap().keys =
            vec![Keyframe::new(Tick::ZERO, Value::Vec3([8.0, 24.0, 0.0])), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Vec3([56.0, 24.0, 0.0]))];
        comp.layers.push(l);
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::PngSequence));
        let job = Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &settings,
            output: &output,
            path: "/c/f_[#####].png",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        let cx = Cx::new(&job);
        let comp = cx.comp().unwrap();
        cx.frame(comp, 0);
        let renders = cx.caches.of(cx.backend()).stats().misses;
        assert!(renders > 0);
        for i in 1..10 {
            let img = cx.frame(comp, i);
            // The frames are the ones rendered without a cache.
            let t = settings.frame_time(comp, i);
            let mut uncached = cx.renderer();
            uncached.cache = None;
            assert_eq!(img.data, uncached.comp_frame(cid, t).data, "frame {i}");
        }
        let stats = cx.caches.of(cx.backend()).stats();
        assert_eq!(stats.misses, renders, "the solid rendered on the first frame only");
        assert!(stats.hits >= 9, "{stats:?}");
    }

    #[test]
    fn frames_render_beside_the_encoder() {
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        use effectcraft_render::NoFootage;
        use effectcraft_time::FrameRate;
        let mut p = Project::default();
        let comp = Comp::new(32, 16, FrameRate::FPS_25, Tick::from_seconds_f64(1.0));
        let cid = p.add_item("Comp", effectcraft_color::Label::Sandstone, None, effectcraft_project::ItemKind::Comp(comp.into()));
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::H264));
        let job = Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &settings,
            output: &output,
            path: "/c/out.mp4",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        let cx = Cx::new(&job);
        let comp = cx.comp().unwrap();
        use effectcraft_render::passes::block_on;
        let want = block_on(cx.frames(comp, (0..6).collect(), |k, img| (k, img.width, img.height)));
        // On threads of their own, and taking turns (no threads): the same frames, in order.
        for beside in [Beside::shared(), &Beside(None)] {
            let (a, frames) = block_on(cx.frames_beside(beside, comp, (0..6).collect(), |k, img| (k, img.width, img.height), || 7)).unwrap();
            assert_eq!((a, &frames), (7, &want));
        }
        // A panicking encoder fails the export instead of taking the render thread down.
        let r = block_on(cx.frames_beside(Beside::shared(), comp, vec![0], |k, _| k, || -> u32 { panic!("encoder bug") }));
        match r {
            Err(crate::ExportError::Encode(e)) => assert!(e.contains("encoder bug"), "{e}"),
            other => panic!("{:?}", other.map(|(a, f)| (a, f.len()))),
        }
    }

    #[test]
    fn interleave_takes_alternate_lines() {
        let a = Image::filled(3, 4, [1.0, 0.0, 0.0, 1.0]);
        let b = Image::filled(3, 4, [0.0, 0.0, 1.0, 1.0]);
        let up = interleave(&a, &b, true);
        let lo = interleave(&a, &b, false);
        for y in 0..4u32 {
            assert_eq!(up.data[up.idx(1, y)][0], if y % 2 == 0 { 1.0 } else { 0.0 }, "upper first: line {y}");
            assert_eq!(lo.data[lo.idx(1, y)][0], if y % 2 == 1 { 1.0 } else { 0.0 }, "lower first: line {y}");
        }
    }

    /// Regress the locked-aspect example: cropping 1920x1080 must resize to 1280x1440.
    #[test]
    fn crop_resize_applies_margins_once() {
        use effectcraft_project::render_queue::{Crop, OutputModule, Resize};
        let output = OutputModule {
            crop: Crop { enabled: true, left: 480, right: 480, ..Default::default() },
            resize: Resize { enabled: true, width: 1280, lock_aspect: true, quality: ResizeQuality::Low, ..Default::default() },
            ..Default::default()
        };
        assert_eq!(output.frame_size(1920, 1080, 1.0), (1280, 1440));
        let img = run_crop_resize(Image::new(1920, 1080), &output, 1.0);
        assert_eq!((img.width, img.height), (1280, 1440));
        assert_eq!(img.data.len(), 1280 * 1440);
    }

    /// Apply output-module geometry to a synthetic frame without rendering or writing files.
    fn run_crop_resize(img: Image, output: &effectcraft_project::render_queue::OutputModule, scale: f64) -> Image {
        use effectcraft_project::render_queue::RenderSettings;
        let project = Project::default();
        let settings = RenderSettings { resolution: scale, ..Default::default() };
        let job = Job {
            project: &project,
            footage: &effectcraft_render::NoFootage,
            expr: None,
            accel: None,
            comp: effectcraft_project::ItemId(0),
            settings: &settings,
            output,
            path: "unused.png",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        Cx::new(&job).crop_resize(img)
    }

    /// Match predicted dimensions across crop, padding, ROI scale, aspect locks and resize qualities.
    #[test]
    fn crop_resize_dimensions_match_the_output_module() {
        use effectcraft_project::render_queue::{Crop, OutputModule, Resize};
        let edges = Crop { enabled: true, left: 2, right: 2, ..Default::default() };
        let padding = Crop { enabled: true, left: -2, bottom: -2, ..Default::default() };
        let roi = |rect| Crop { enabled: true, use_roi: true, roi: Some(rect), ..Default::default() };
        for quality in [ResizeQuality::Low, ResizeQuality::High] {
            let resize = |width| Resize { enabled: true, width, height: 1, lock_aspect: true, quality };
            for (crop, resize, scale, expected) in [
                (Crop::default(), Resize::default(), 1.0, (8, 6)),
                (edges, Resize::default(), 1.0, (4, 6)),
                (Crop::default(), resize(4), 1.0, (4, 3)),
                (edges, resize(8), 1.0, (8, 12)),
                (padding, resize(5), 1.0, (5, 4)),
                (roi([4.0, 2.0, 8.0, 6.0]), resize(8), 0.5, (8, 6)),
                (roi([1.0, 0.5, 2.0, 1.5]), resize(8), 2.0, (8, 6)),
                (edges, Resize { lock_aspect: false, height: 2, ..resize(5) }, 1.0, (5, 2)),
            ] {
                let output = OutputModule { crop, resize, ..Default::default() };
                assert_eq!(output.frame_size(8, 6, scale), expected);
                let img = run_crop_resize(Image::filled(8, 6, [0.25, 0.0, 0.0, 0.5]), &output, scale);
                assert_eq!((img.width, img.height), expected, "{crop:?}, {resize:?}, scale {scale}");
                assert_eq!(img.data.len(), expected.0 as usize * expected.1 as usize);
            }
        }
    }

    /// Exercise zero/empty inputs, collapsed crops and existing caps on extreme single-axis resizes.
    #[test]
    fn crop_resize_zero_and_extreme_sizes_stay_bounded() {
        use effectcraft_project::render_queue::{Crop, OutputModule, Resize};
        for quality in [ResizeQuality::Low, ResizeQuality::High] {
            for (size, crop, width, height, expected) in [
                ((8, 6), Crop::default(), 0, 0, (1, 1)),
                ((8, 1), Crop::default(), u32::MAX, 1, (30_000, 1)),
                ((1, 8), Crop::default(), 1, u32::MAX, (1, 30_000)),
                ((8, 6), Crop { enabled: true, left: 20_000, right: 20_000, ..Default::default() }, 2, 3, (2, 3)),
                ((0, 0), Crop { enabled: true, left: 1, top: 1, ..Default::default() }, 2, 3, (2, 3)),
            ] {
                let output = OutputModule { crop, resize: Resize { enabled: true, width, height, lock_aspect: false, quality }, ..Default::default() };
                let img = run_crop_resize(Image::new(size.0, size.1), &output, 1.0);
                assert_eq!((img.width, img.height), expected);
                assert_eq!(img.data.len(), expected.0 as usize * expected.1 as usize);
                assert!(img.data.iter().all(|p| *p == [0.0; 4]));
            }
        }
    }

    #[test]
    fn crop_and_pad() {
        let mut img = Image::new(4, 3);
        for (i, p) in img.data.iter_mut().enumerate() {
            *p = [i as f32, 0.0, 0.0, 1.0];
        }
        let c = crop(&img, 1, 1, 0, 1);
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.data[0][0], 5.0);
        let p = crop(&img, -1, 0, 0, 0);
        assert_eq!((p.width, p.height), (4, 4));
        assert_eq!(p.data[0], [0.0; 4]);
        assert_eq!(p.data[4][0], 0.0);
        assert_eq!(p.data[5][0], 1.0);
    }
}
