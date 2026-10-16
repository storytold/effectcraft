//! The frame pipeline shared by every output format: Render Settings overrides (effects, solo,
//! guide layers, frame blending, motion blur), field rendering with 3:2 pulldown, colour depth,
//! the Output Module's crop and resize, alpha handling, storage overflow and the render log.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};

use effectcraft_project::render_queue::{
    AlphaMode, Channels, ColorDepth, CurrentOrOff, EffectsMode, FrameSample, RenderLog, RenderSettings, ResizeQuality, SwitchOverride, TimeSpan, log_path,
};
use effectcraft_project::{Comp, ItemId, Project};
use effectcraft_raster::Image;
use effectcraft_render::{Backend, LayerCache, RenderOpts, Renderer};
use effectcraft_time::Tick;
use web_time::Instant;

use crate::{Job, RenderQuality, out};

/// GPU frames rendered at once by default: one, as the export scheduler's single GPU worker
/// renders image sequences (#898: frames in flight beyond one only with asynchronous YUV
/// readback). Measured on the #879 project at 1080p on an 8 GB card: 6.3 GB peak with one,
/// 7.0 to 7.8 GB with two, and the whole card with three, where main fell back to the CPU.
/// Not sized from free VRAM: the GPU backend reports no free or total device memory on every
/// platform, so a portable estimate is not available here.
pub(crate) const GPU_INFLIGHT_DEFAULT: usize = 1;
/// Overrides [`GPU_INFLIGHT_DEFAULT`]: a whole number of frames, at least 1.
pub(crate) const GPU_INFLIGHT_ENV: &str = "EFFECTCRAFT_GPU_INFLIGHT";

/// GPU frames in flight from the `EFFECTCRAFT_GPU_INFLIGHT` value: a positive whole number, or
/// the default (with a warning when a value was given) when it is 0, negative, not a number or
/// too large.
pub(crate) fn gpu_inflight(var: Option<&str>) -> usize {
    let Some(v) = var else { return GPU_INFLIGHT_DEFAULT };
    match v.trim().parse::<usize>() {
        Ok(n) if n >= 1 => n,
        _ => {
            log::warn!(target: "effectcraft_export", "{GPU_INFLIGHT_ENV}={v:?} is not a whole number of frames of at least 1: using {GPU_INFLIGHT_DEFAULT}");
            GPU_INFLIGHT_DEFAULT
        }
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
pub(crate) fn map_frames<I: Send, T: Send>(
    ks: Vec<u64>,
    cap: Option<usize>,
    render: impl Fn(u64) -> I + Sync + Send,
    encode: impl Fn(u64, I) -> T + Sync + Send,
) -> Vec<T> {
    use rayon::prelude::*;
    let Some(n) = cap.map(|n| n.max(1)) else {
        return ks.into_par_iter().map(|k| encode(k, render(k))).collect();
    };
    let window = n.max(rayon::current_num_threads());
    let mut out = Vec::with_capacity(ks.len());
    for win in ks.chunks(window) {
        let mut imgs = Vec::with_capacity(win.len());
        for chunk in win.chunks(n) {
            imgs.par_extend(chunk.par_iter().map(|&k| render(k)));
        }
        out.par_extend(win.par_iter().copied().zip(imgs.into_par_iter()).map(|(k, img)| encode(k, img)));
    }
    out
}

/// A job being exported: the job, the project with the Render Settings overrides applied, the
/// job's layer cache and per-frame timing for the render log.
pub(crate) struct Cx<'a> {
    pub job: &'a Job<'a>,
    pub project: Project,
    /// Processed layer buffers shared by the job's frames, as the viewer's layer cache: static
    /// and transform-only layers render once per job (the GPU compositor then uploads them
    /// once). Dropped with the job.
    cache: LayerCache,
    /// (output frame, seconds) of every rendered frame (Plus Per Frame Info logs).
    pub frame_times: Option<Mutex<Vec<(u64, f64)>>>,
    /// Files written to an overflow folder.
    pub overflowed: Mutex<Vec<String>>,
    written: Mutex<Vec<String>>,
    /// Frames rendered once for several output modules ([`fan_out`]): this module's job takes
    /// them from here instead of rendering them.
    feed: Option<Feed>,
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

/// What decides a job's overridden project and layer buffers: jobs with equal keys (and equal
/// projects) render the same frames. The time span is left out (segments of one output, output
/// modules cut at different segment frames); so is everything per output module (format, crop,
/// resize, channels), which applies after rendering.
#[derive(PartialEq)]
struct WarmKey {
    comp: ItemId,
    settings: RenderSettings,
    nested_switches: bool,
    budget: usize,
    /// Addresses of the job's footage source, expression host and accelerator: the same
    /// session's objects.
    footage: usize,
    expr: Option<usize>,
    accel: Option<usize>,
}

impl WarmKey {
    fn of(job: &Job) -> WarmKey {
        WarmKey {
            comp: job.comp,
            settings: RenderSettings { time_span: TimeSpan::LengthOfComp, ..job.settings.clone() },
            nested_switches: job.nested_switches,
            budget: job.options.memory_budget.unwrap_or(crate::DEFAULT_MEMORY_BUDGET),
            footage: job.footage as *const dyn effectcraft_render::FootageSource as *const () as usize,
            expr: job.expr.map(|e| e as *const dyn effectcraft_render::ExprHost as *const () as usize),
            accel: job.accel.map(|a| a as *const dyn effectcraft_render::Accelerator as *const () as usize),
        }
    }
}

/// A finished job's project (as its Render Settings render it) and layer cache, kept for the
/// next job that renders the same frames: a Render Queue item exports its output modules one
/// after another, each a job of its own, and every module after the first finds the layers the
/// first rendered in the cache instead of rendering them again. Layer buffers are keyed by
/// their content (see [`LayerCache`]), so a warm cache gives the frames a cold one gives.
struct Warm {
    key: WarmKey,
    /// The job's project before the overrides: the next job's must be equal.
    source: Project,
    project: Project,
    cache: LayerCache,
}

/// How long a finished job's [`Warm`] waits for the next job (the next output module starts
/// as soon as the previous one returns); then it is dropped with its memory.
const WARM_FOR: std::time::Duration = std::time::Duration::from_secs(3);

/// The one kept [`Warm`], with its generation (for the timer that drops it) and when it was kept.
static WARM: Mutex<Option<(u64, Instant, Warm)>> = Mutex::new(None);
static WARM_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn warm_slot() -> std::sync::MutexGuard<'static, Option<(u64, Instant, Warm)>> {
    WARM.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The kept [`Warm`] when `job` renders the same frames as the job that left it.
fn take_warm(job: &Job) -> Option<Warm> {
    let (_, at, warm) = warm_slot().take()?;
    (at.elapsed() < WARM_FOR && warm.key == WarmKey::of(job) && warm.source == *job.project).then_some(warm)
}

impl<'a> Cx<'a> {
    pub fn new(job: &'a Job<'a>) -> Cx<'a> {
        let mut project = job.project.clone();
        apply_overrides(&mut project, job);
        let cache = LayerCache::new(job.options.memory_budget.unwrap_or(crate::DEFAULT_MEMORY_BUDGET));
        Cx::with(job, project, cache)
    }

    /// [`Cx::new`], continuing from the job that rendered the same frames just before, if any
    /// (its project and layer cache, see [`Warm`]).
    pub fn resume(job: &'a Job<'a>) -> Cx<'a> {
        match take_warm(job) {
            Some(w) => Cx::with(job, w.project, w.cache),
            None => Cx::new(job),
        }
    }

    /// Keep this finished job's project and layer cache for the next job ([`Cx::resume`]) for
    /// [`WARM_FOR`]. Not on the web, where nothing would drop them after that.
    pub fn keep_warm(self) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        let warm = Warm { key: WarmKey::of(self.job), source: self.job.project.clone(), project: self.project, cache: self.cache };
        let generation = WARM_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // (the job kept before, if no job took it, goes now)
        let old = warm_slot().replace((generation, Instant::now(), warm));
        drop(old);
        // Dropped after `WARM_FOR` unless a job took it (or kept its own) by then.
        let expire = move || {
            let mut slot = warm_slot();
            let kept = if slot.as_ref().is_some_and(|(g, _, _)| *g == generation) { slot.take() } else { None };
            drop(slot);
            drop(kept);
        };
        let later = move || {
            std::thread::sleep(WARM_FOR);
            expire();
        };
        if std::thread::Builder::new().name("export-warm".into()).spawn(later).is_err() {
            expire();
        }
    }

    fn with(job: &'a Job<'a>, project: Project, cache: LayerCache) -> Cx<'a> {
        let frame_times = (job.options.log == RenderLog::PlusPerFrameInfo).then(|| Mutex::new(Vec::new()));
        // With deferred readbacks, buffers computed from a missed pass's placeholders are not kept.
        cache.set_gate(job.accel.and_then(|a| a.miss_gate()));
        Cx { job, project, cache, frame_times, overflowed: Mutex::new(vec![]), written: Mutex::new(vec![]), feed: None }
    }

    /// The files this job opened on disk (removed when a later step of the item cancels it).
    pub fn written(&self) -> Vec<String> {
        self.written.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn comp(&self) -> Option<&Comp> {
        self.project.comp(self.job.comp)
    }

    /// The renderer of the job's frames: the Render Settings' options, the job's accelerator
    /// and layer cache.
    fn renderer(&self) -> Renderer<'_> {
        let s = self.job.settings;
        // The whole job renders on the project's renderer (Mercury GPU Acceleration or Software
        // Only), not on whichever compositor Auto timed faster for each frame: the CPU and GPU
        // compositors differ slightly, so a per-frame choice made renders differ run to run.
        let backend = if self.project.settings.gpu_acceleration { Backend::Gpu } else { Backend::Cpu };
        let opts = RenderOpts {
            scale: s.resolution.clamp(0.01, 4.0),
            motion_blur: s.motion_blur_override() != SwitchOverride::OffForAll,
            guides: s.guide_layers == CurrentOrOff::Current,
            draft: s.quality == RenderQuality::Draft,
            // Output renders always look through the comp's active camera.
            view: None,
            backend,
            roi: None,
            nested_switches: self.job.nested_switches,
            draft_shadows: true,
            proxy: s.proxy_use,
        };
        let mut r = Renderer::new(&self.project, self.job.footage, opts);
        r.expr = self.job.expr;
        r.accel = self.job.accel;
        r.cache = Some(&self.cache);
        r
    }

    async fn render_at(&self, t: Tick) -> Image {
        // With deferred GPU readbacks (a browser worker) the frame renders in passes.
        effectcraft_render::passes::comp_frame(&self.renderer(), self.job.comp, t).await
    }

    /// Whether frames render in passes (an accelerator with deferred readbacks).
    pub fn deferred(&self) -> bool {
        effectcraft_render::passes::is_deferred(self.job.accel)
    }

    /// Output frames `ks`, each mapped by `f` (encoding): rendered in parallel batches, or one
    /// by one in passes when readbacks are deferred ([`Cx::deferred`]).
    pub async fn frames<T: Send>(&self, comp: &Comp, ks: Vec<u64>, f: impl Fn(u64, Image) -> T + Sync + Send) -> Vec<T> {
        // (a renderer that stopped early leaves the frames to this job)
        if let Some(out) = self.feed.as_ref().and_then(|feed| self.fed(feed, comp, &ks, &f)) {
            return out;
        }
        if self.deferred() {
            let mut out = Vec::with_capacity(ks.len());
            for k in ks {
                let img = self.frame_async(comp, k).await;
                out.push(f(k, img));
            }
            return out;
        }
        map_frames(ks, self.gpu_cap(), |k| self.frame(comp, k), f)
    }

    /// GPU frames rendered at once (#879), decided once per batch: with a GPU accelerator every
    /// frame in flight holds its own working textures, so a whole batch rendered at once can
    /// exhaust VRAM (or shared memory, which crashes instead of falling back). `None` when the
    /// frames render on the CPU: no accelerator, or a Software Only project.
    fn gpu_cap(&self) -> Option<usize> {
        (self.job.accel.is_some() && self.project.settings.gpu_acceleration).then(|| gpu_inflight(std::env::var(GPU_INFLIGHT_ENV).ok().as_deref()))
    }

    /// Output frames `0..total` in batches of `batch` frames, each frame mapped by `f`
    /// (encoding), handed to `body` in order through [`Batches::next`]. On native targets the
    /// next batch renders on a scoped thread while `body` encodes the current one: at most two
    /// batches are held, the one being encoded and the one rendered next (handed over through a
    /// rendezvous channel). In passes (deferred readbacks), on the web and on a rayon worker (it
    /// must not block on frames its own pool renders) batches render one after another.
    ///
    /// When `body` returns (done, an encode error or a stop) the renderer stops at its next
    /// frame; a renderer that stops early (a panic) makes [`Batches::next`] an error.
    pub async fn batches<T: Send, R>(
        &self,
        comp: &Comp,
        total: u64,
        batch: u64,
        f: impl Fn(u64, Image) -> T + Sync + Send,
        body: impl AsyncFnOnce(&mut Batches<'_, '_, T>) -> crate::Result<R>,
    ) -> crate::Result<R> {
        let batch = batch.max(1);
        if self.feed.is_some() {
            let f: &(dyn Fn(u64, Image) -> T + Sync + Send) = &f;
            return body(&mut Batches { next: 0, total, batch, src: Source::Fed { cx: self, comp, f } }).await;
        }
        if self.deferred() || cfg!(target_arch = "wasm32") || rayon::current_thread_index().is_some() {
            let f: &(dyn Fn(u64, Image) -> T + Sync + Send) = &f;
            return body(&mut Batches { next: 0, total, batch, src: Source::Serial { cx: self, comp, f } }).await;
        }
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<T>>(0);
            let (stop, f) = (&stop, &f);
            let renderer = s.spawn(move || {
                let mut i = 0;
                while i < total && !stop.load(Ordering::Relaxed) {
                    let end = i.saturating_add(batch).min(total);
                    // Frames not started once the encode side has stopped are skipped.
                    let frames: Option<Vec<T>> = (i..end).into_par_iter().map(|k| (!stop.load(Ordering::Relaxed)).then(|| f(k, self.frame(comp, k)))).collect();
                    let Some(frames) = frames else { break };
                    if tx.send(frames).is_err() {
                        break;
                    }
                    i = end;
                }
            });
            let mut batches = Batches { next: 0, total, batch, src: Source::Thread(rx) };
            let r = effectcraft_render::passes::block_on(body(&mut batches));
            // Stop the renderer: skip its remaining frames and fail its next hand-off.
            stop.store(true, Ordering::Relaxed);
            drop(batches);
            match (r, renderer.join()) {
                (Err(_), Err(_)) => Err(crate::ExportError::Encode("rendering failed".into())),
                (r, _) => r,
            }
        })
    }

    /// Output frame `i`: sampled (fields, pulldown), at the colour depth, cropped and resized.
    pub fn frame(&self, comp: &Comp, i: u64) -> Image {
        effectcraft_render::passes::block_on(self.frame_async(comp, i))
    }

    /// [`Cx::frame`] as a future (deferred readbacks: the frame renders in passes).
    pub async fn frame_async(&self, comp: &Comp, i: u64) -> Image {
        let (img, secs) = self.frame_timed(comp, i).await;
        if let Some(ft) = &self.frame_times {
            ft.lock().unwrap_or_else(|e| e.into_inner()).push((i, secs));
        }
        img
    }

    /// [`Cx::frame_async`] and the seconds it took, not logged.
    async fn frame_timed(&self, comp: &Comp, i: u64) -> (Image, f64) {
        let t0 = Instant::now();
        let img = match self.job.settings.sample(comp, i) {
            FrameSample::Progressive(t) => self.render_at(t).await,
            FrameSample::Fields { first, second, upper_first } => {
                let a = self.render_at(first).await;
                if second == first { a } else { interleave(&a, &self.render_at(second).await, upper_first) }
            }
        };
        let mut img = img;
        // Color Depth: renders run at the project's depth; another depth quantises the frame.
        let depth = self.job.settings.color_depth;
        if depth != ColorDepth::Current
            && let Some(levels) = depth.resolve(self.project.settings.bit_depth).levels()
        {
            img.quantize(levels);
        }
        let img = self.crop_resize(img);
        (img, t0.elapsed().as_secs_f64())
    }

    /// Output frames `ks`, each mapped by `f`, from the next batch of the feed (copies of the
    /// shared frames; a frame the batch lacks renders here). `None` when the renderer stopped.
    fn fed<T: Send>(&self, feed: &Feed, comp: &Comp, ks: &[u64], f: &(dyn Fn(u64, Image) -> T + Sync + Send)) -> Option<Vec<T>> {
        use rayon::prelude::*;
        let got = feed.rx.lock().unwrap_or_else(std::sync::PoisonError::into_inner).recv().ok()?;
        let shared = |k: u64| k.checked_sub(got.start).and_then(|j| usize::try_from(j).ok()).and_then(|j| got.frames.get(j));
        if let Some(ft) = &self.frame_times {
            let mut ft = ft.lock().unwrap_or_else(|e| e.into_inner());
            ft.extend(ks.iter().filter_map(|&k| shared(k).map(|(_, secs)| (k, *secs))));
        }
        let out = ks.par_iter().map(|&k| f(k, shared(k).map_or_else(|| self.frame(comp, k), |(img, _)| img.clone()))).collect();
        drop(got);
        // The renderer hands the batch to the next module (or renders on) once this one has its copy.
        let _ = feed.ack.send(());
        Some(out)
    }

    /// Output Module ▸ Crop, then Resize.
    fn crop_resize(&self, img: Image) -> Image {
        let om = self.job.output;
        let scale = self.job.settings.resolution.clamp(0.01, 4.0);
        let (t, l, b, r) = om.crop.edges(img.width, img.height, scale);
        let img = if (t, l, b, r) == (0, 0, 0, 0) { img } else { crop(&img, t, l, b, r) };
        if !om.resize.enabled {
            return img;
        }
        let (w, h) = om.frame_size(img.width, img.height, 1.0);
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
    pub fn pixels(&self, img: &Image, comp: &Comp, channels: Channels, w: u32, h: u32) -> Vec<u8> {
        let px = match channels {
            Channels::Rgb => img.to_rgba8_over(comp.background),
            Channels::Rgba => match self.job.output.alpha_mode {
                AlphaMode::Straight => img.to_rgba8(),
                AlphaMode::Premultiplied => premultiplied8(img),
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
                self.overflowed.lock().unwrap_or_else(|e| e.into_inner()).push(p.clone());
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
        let over = self.overflowed.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut v = ft.lock().unwrap_or_else(|e| e.into_inner()).clone();
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

/// The batches of [`Cx::batches`], in order.
pub(crate) struct Batches<'c, 'a, T> {
    next: u64,
    total: u64,
    batch: u64,
    src: Source<'c, 'a, T>,
}

enum Source<'c, 'a, T> {
    /// Rendered when asked for ([`Cx::frames`]).
    Serial { cx: &'c Cx<'a>, comp: &'c Comp, f: &'c (dyn Fn(u64, Image) -> T + Sync + Send) },
    /// Rendered ahead on the renderer thread.
    Thread(std::sync::mpsc::Receiver<Vec<T>>),
    /// Rendered once for several output modules ([`fan_out`]) and mapped here.
    Fed { cx: &'c Cx<'a>, comp: &'c Comp, f: &'c (dyn Fn(u64, Image) -> T + Sync + Send) },
}

impl<T: Send> Batches<'_, '_, T> {
    /// The next batch: its output frames `start..end` and their mapped frames; `None` after the last.
    pub async fn next(&mut self) -> crate::Result<Option<(u64, u64, Vec<T>)>> {
        if self.next >= self.total {
            return Ok(None);
        }
        let (i, end) = (self.next, self.next.saturating_add(self.batch).min(self.total));
        let frames = match &self.src {
            Source::Serial { cx, comp, f } => cx.frames(comp, (i..end).collect(), *f).await,
            Source::Thread(rx) => rx.recv().map_err(|_| crate::ExportError::Encode("rendering stopped".into()))?,
            Source::Fed { cx, comp, f } => {
                let ks: Vec<u64> = (i..end).collect();
                let feed = cx.feed.as_ref().ok_or_else(|| crate::ExportError::Encode("rendering stopped".into()))?;
                cx.fed(feed, comp, &ks, *f).ok_or_else(|| crate::ExportError::Encode("rendering stopped".into()))?
            }
        };
        self.next = end;
        Ok(Some((i, end, frames)))
    }
}

/// One batch rendered for the output modules of a [`fan_out`]: output frames `start..` and the
/// seconds each took.
struct Rendered {
    start: u64,
    frames: Arc<Vec<(Image, f64)>>,
}

/// A module's end of a [`fan_out`]: the batches, in order, and the acknowledgement that it has
/// copied one. Dropped when the module's job ends.
struct Feed {
    rx: Mutex<Receiver<Rendered>>,
    ack: Sender<()>,
    /// Modules still taking frames (the renderer stops when none is left).
    live: Arc<AtomicUsize>,
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Whether `a` and `b` render the same output frames, so one render can feed both
/// ([`fan_out`]): the same project, comp, Render Settings (time span included), session objects
/// and memory budget (which sets the batches, and so the files' layout), Crop and Resize. Format,
/// channels and the rest of the Output Module apply to the rendered frames. Audio-only formats
/// render no frames; a sequence skipping existing files renders only some.
pub(crate) fn shares_frames(a: &Job, b: &Job) -> bool {
    use effectcraft_project::render_queue::OutputFormat;
    let renders = |j: &Job| !matches!(j.output.format, OutputFormat::Wav | OutputFormat::Aiff) && !(j.output.format.is_sequence() && j.settings.skip_existing);
    renders(a)
        && renders(b)
        && WarmKey::of(a) == WarmKey::of(b)
        && a.settings == b.settings
        && a.output.crop == b.output.crop
        && a.output.resize == b.output.resize
        && (std::ptr::eq(a.project, b.project) || a.project == b.project)
}

/// What a module's job returned, and the files it opened on disk.
pub(crate) type Outcome = (crate::Result<crate::Report>, Vec<String>);

/// Export output modules that render the same frames ([`shares_frames`]; `(index in the item,
/// job)`) at once: every frame renders once, on a renderer thread with the first module's layer
/// cache, and is handed to each module's job, which maps (converts, encodes) and writes it on a
/// thread of its own with its own encoder and muxer. The next batch renders while the modules
/// encode this one, as in [`Cx::batches`]; a batch goes to one module at a time and the next
/// renders once every module has copied it, so the frames in flight are one rendering batch or
/// one shared batch and its copies (plus each module's mapped batch).
///
/// `progress` hears each module's progress on the caller's thread, with its index; returning
/// `false` cancels that module. A module that fails (not cancelled) lowers `failed` to its index,
/// and the modules after it are cancelled at their next batch, as if they never started (the
/// item stops at the first failing module); modules before it run to the end.
pub(crate) fn fan_out(group: &[(usize, &Job<'_>)], failed: &AtomicUsize, progress: &mut dyn FnMut(usize, &crate::Progress) -> bool) -> Vec<Outcome> {
    let Some(&(_, lead)) = group.first() else { return vec![] };
    let rcx = Cx::resume(lead);
    let live = Arc::new(AtomicUsize::new(group.len()));
    let mut feeds = Vec::with_capacity(group.len());
    let mut ends = Vec::with_capacity(group.len());
    for _ in group {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Rendered>(0);
        let (ack, acks) = std::sync::mpsc::channel::<()>();
        feeds.push(Feed { rx: Mutex::new(rx), ack, live: live.clone() });
        ends.push(Some((tx, acks)));
    }
    let (req_tx, req_rx) = std::sync::mpsc::channel::<(usize, crate::Progress, Sender<bool>)>();
    let outcomes: Vec<Outcome> = std::thread::scope(|s| {
        let (rcx, live) = (&rcx, &*live);
        let renderer = s.spawn(move || render_for(rcx, ends, live));
        let modules: Vec<_> = group
            .iter()
            .zip(feeds)
            .map(|(&(index, job), feed)| {
                let req_tx = req_tx.clone();
                let module = s.spawn(move || {
                    let mut cx = Cx::new(job);
                    cx.feed = Some(feed);
                    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<bool>();
                    let mut relay = |p: &crate::Progress| req_tx.send((index, *p, reply_tx.clone())).is_ok() && reply_rx.recv().unwrap_or(false);
                    let r = effectcraft_render::passes::block_on(crate::run(&cx, &mut relay, Instant::now()));
                    // (the renderer skips this module from now on)
                    cx.feed = None;
                    if matches!(&r, Err(e) if !matches!(e, crate::ExportError::Cancelled)) {
                        failed.fetch_min(index, Ordering::SeqCst);
                    }
                    (r, cx.written())
                });
                (index, module)
            })
            .collect();
        drop(req_tx);
        while let Ok((index, p, reply)) = req_rx.recv() {
            let go = progress(index, &p) && failed.load(Ordering::SeqCst) > index;
            let _ = reply.send(go);
        }
        let outcomes = modules
            .into_iter()
            .map(|(index, m)| {
                m.join().unwrap_or_else(|_| {
                    failed.fetch_min(index, Ordering::SeqCst);
                    (Err(crate::ExportError::Encode("export failed".into())), vec![])
                })
            })
            .collect();
        let _ = renderer.join();
        outcomes
    });
    // A finished group leaves its layer cache for the next module or group (see `Warm`).
    if outcomes.iter().all(|(r, _)| r.is_ok()) {
        rcx.keep_warm();
    }
    outcomes
}

/// The renderer of a [`fan_out`]: the job's output frames in batches (as [`crate::batch_size`]
/// sets them for each module alone), each handed to every module still taking frames.
fn render_for(cx: &Cx, mut ends: Vec<Option<(SyncSender<Rendered>, Receiver<()>)>>, live: &AtomicUsize) {
    use rayon::prelude::*;
    let Some(comp) = cx.comp() else { return };
    let total = cx.settings.frame_count(comp);
    let batch = crate::batch_size(cx, comp).max(1);
    let mut i = 0;
    while i < total && live.load(Ordering::Relaxed) > 0 {
        let end = i.saturating_add(batch).min(total);
        // Frames not started once every module has stopped are skipped.
        let frames: Option<Vec<(Image, f64)>> = (i..end)
            .into_par_iter()
            .map(|k| (live.load(Ordering::Relaxed) > 0).then(|| effectcraft_render::passes::block_on(cx.frame_timed(comp, k))))
            .collect();
        let Some(frames) = frames else { break };
        let frames = Arc::new(frames);
        for slot in ends.iter_mut() {
            let Some((tx, ack)) = slot else { continue };
            if tx.send(Rendered { start: i, frames: frames.clone() }).is_err() || ack.recv().is_err() {
                *slot = None;
            }
        }
        if ends.iter().all(Option::is_none) {
            break;
        }
        i = end;
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

/// Premultiplied 8-bit RGBA (colour matted with black).
fn premultiplied8(img: &Image) -> Vec<u8> {
    img.data
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            [0, 1, 2].map(|c| (p[c].clamp(0.0, a) * 255.0 + 0.5) as u8).into_iter().chain([(a * 255.0 + 0.5) as u8])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

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

    #[test]
    fn gpu_inflight_override_is_a_positive_number_or_the_default() {
        assert_eq!(gpu_inflight(None), GPU_INFLIGHT_DEFAULT);
        assert_eq!(gpu_inflight(Some("3")), 3);
        assert_eq!(gpu_inflight(Some(" 5 ")), 5);
        for bad in ["0", "", "-2", "three", "2.5", "99999999999999999999999"] {
            assert_eq!(gpu_inflight(Some(bad)), GPU_INFLIGHT_DEFAULT, "{bad:?}");
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
        let renders = cx.cache.stats().misses;
        assert!(renders > 0);
        for i in 1..10 {
            let img = cx.frame(comp, i);
            // The frames are the ones rendered without a cache.
            let t = settings.frame_time(comp, i);
            let mut uncached = cx.renderer();
            uncached.cache = None;
            assert_eq!(img.data, uncached.comp_frame(cid, t).data, "frame {i}");
        }
        let stats = cx.cache.stats();
        assert_eq!(stats.misses, renders, "the solid rendered on the first frame only");
        assert!(stats.hits >= 9, "{stats:?}");
    }

    #[test]
    fn the_next_output_module_continues_from_the_layer_cache() {
        // G5: a Render Queue item with two output modules rendered every layer twice. The
        // second module's job takes the first one's layer cache when it renders the same frames.
        use effectcraft_color::Label;
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        use effectcraft_project::{ItemKind, LayerSource, Solid, build};
        use effectcraft_time::FrameRate;
        let mut p = Project::default();
        let mut comp = Comp::new(32, 24, FrameRate::FPS_25, FrameRate::FPS_25.tick_of(4));
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 8, height: 8, pixel_aspect: 1.0 }));
        comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (8, 8), None));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        // (two pools: a job takes the kept one only with the same footage source)
        let (footage, other_footage) = (effectcraft_media::MediaPool::new(), effectcraft_media::MediaPool::new());
        let settings = RenderSettings::default();
        let (png, h264) = (OutputModule::for_format(OutputFormat::PngSequence), OutputModule::for_format(OutputFormat::H264));
        let sink = |_: &str, _: Vec<u8>| {};
        fn job<'a>(
            p: &'a Project,
            cid: ItemId,
            settings: &'a RenderSettings,
            output: &'a OutputModule,
            footage: &'a dyn effectcraft_render::FootageSource,
        ) -> Job<'a> {
            Job {
                project: p,
                footage,
                expr: None,
                accel: None,
                comp: cid,
                settings,
                output,
                path: "/mem/x",
                sink: None,
                nested_switches: true,
                options: Default::default(),
            }
        }
        let mut first = job(&p, cid, &settings, &png, &footage);
        first.sink = Some(&sink);
        // Other tests of this binary don't export, so the kept job is this one's.
        let r = crate::export(&first, &mut |_| true);
        assert!(r.is_ok(), "{r:?}");
        // A job rendering other frames does not take it (and the kept job is gone either way).
        let half = RenderSettings { resolution: 0.5, ..RenderSettings::default() };
        let mut edited = p.clone();
        if let Some(c) = edited.comp_mut(cid) {
            c.background = [0.0, 1.0, 0.0];
        }
        for other in [job(&p, cid, &half, &h264, &footage), job(&edited, cid, &settings, &h264, &footage), job(&p, cid, &settings, &h264, &other_footage)] {
            assert!(crate::export(&first, &mut |_| true).is_ok());
            assert!(take_warm(&other).is_none());
        }
        assert!(crate::export(&first, &mut |_| true).is_ok());
        let second = job(&p, cid, &settings, &h264, &footage);
        let warm = take_warm(&second).expect("the first module's job is kept");
        assert!(take_warm(&second).is_none(), "taken once");
        let cx = Cx::with(&second, warm.project, warm.cache);
        let misses = cx.cache.stats().misses;
        assert!(misses > 0);
        let comp = cx.comp().unwrap();
        for i in 0..4 {
            let img = cx.frame(comp, i);
            let mut uncached = cx.renderer();
            uncached.cache = None;
            assert_eq!(img.data, uncached.comp_frame(cid, settings.frame_time(comp, i)).data, "frame {i}");
        }
        assert_eq!(cx.cache.stats().misses, misses, "the second module rendered no layer again");
    }

    /// Run `f` with the job of a `frames`-frame comp of one solid.
    fn with_cx<R>(frames: u64, f: impl FnOnce(&Cx, &Comp) -> R) -> R {
        use effectcraft_color::Label;
        use effectcraft_project::render_queue::{OutputFormat, OutputModule, RenderSettings};
        use effectcraft_project::{ItemKind, LayerSource, Solid, build};
        use effectcraft_render::NoFootage;
        use effectcraft_time::FrameRate;
        let mut p = Project::default();
        let mut comp = Comp::new(16, 16, FrameRate::FPS_25, FrameRate::FPS_25.tick_of(frames as i64));
        let sid = p.add_item("Solid", Label::Red, None, ItemKind::Solid(Solid { color: [1.0, 0.0, 0.0], width: 8, height: 8, pixel_aspect: 1.0 }));
        comp.layers.push(build::layer(&mut p, &comp, "Solid", LayerSource::Solid { item: sid }, (8, 8), None));
        let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.into()));
        let (settings, output) = (RenderSettings::default(), OutputModule::for_format(OutputFormat::H264));
        let job = Job {
            project: &p,
            footage: &NoFootage,
            expr: None,
            accel: None,
            comp: cid,
            settings: &settings,
            output: &output,
            path: "/c/x.mp4",
            sink: None,
            nested_switches: true,
            options: Default::default(),
        };
        let cx = Cx::new(&job);
        let comp = cx.comp().unwrap();
        f(&cx, comp)
    }

    #[test]
    fn an_encode_error_stops_the_renderer() {
        // G5 B1: the next batch renders while one encodes. An encode error returns at once;
        // the renderer stops after at most the batch it had in flight (no hang on the hand-off).
        use std::sync::atomic::{AtomicU64, Ordering};
        let batch = 4;
        let rendered = AtomicU64::new(0);
        let r = with_cx(batch * 10, |cx, comp| {
            let render = |k: u64, _| {
                rendered.fetch_add(1, Ordering::Relaxed);
                k
            };
            effectcraft_render::passes::block_on(cx.batches(comp, batch * 10, batch, render, async |b: &mut Batches<'_, '_, u64>| -> crate::Result<()> {
                let (i, end, ks) = b.next().await?.unwrap();
                assert_eq!((i, end, ks), (0, batch, vec![0, 1, 2, 3]));
                Err(crate::ExportError::Encode("boom".into()))
            }))
        });
        assert!(matches!(r, Err(crate::ExportError::Encode(ref e)) if e == "boom"), "{r:?}");
        let n = rendered.load(Ordering::Relaxed);
        assert!(n <= batch * 2, "rendered {n} frames after the encoder failed");
    }

    #[test]
    fn batches_arrive_in_order_and_a_failed_render_is_an_error() {
        let batch = 3;
        let got = with_cx(10, |cx, comp| {
            effectcraft_render::passes::block_on(cx.batches(
                comp,
                10,
                batch,
                |k, _| k,
                async |b: &mut Batches<'_, '_, u64>| -> crate::Result<Vec<(u64, u64, Vec<u64>)>> {
                    let mut v = vec![];
                    while let Some(x) = b.next().await? {
                        v.push(x);
                    }
                    Ok(v)
                },
            ))
        });
        assert_eq!(got.unwrap(), vec![(0, 3, vec![0, 1, 2]), (3, 6, vec![3, 4, 5]), (6, 9, vec![6, 7, 8]), (9, 10, vec![9])]);
        // A renderer that dies (a panic in the second batch) fails the export instead of
        // ending it early with a truncated file, and does not hang.
        let r = with_cx(10, |cx, comp| {
            let render = |k: u64, _| {
                assert!(k != 4, "render failure");
                k
            };
            effectcraft_render::passes::block_on(cx.batches(comp, 10, batch, render, async |b: &mut Batches<'_, '_, u64>| -> crate::Result<u64> {
                let mut n = 0;
                while let Some((_, _, ks)) = b.next().await? {
                    n += ks.len() as u64;
                }
                Ok(n)
            }))
        });
        assert!(matches!(r, Err(crate::ExportError::Encode(_))), "{r:?}");
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
