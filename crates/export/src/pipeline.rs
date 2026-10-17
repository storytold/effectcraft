//! The frame pipeline shared by every output format: Render Settings overrides (effects, solo,
//! guide layers, frame blending, motion blur), field rendering with 3:2 pulldown, colour depth,
//! the Output Module's crop and resize, alpha handling, storage overflow and the render log.

use std::sync::Mutex;

use effectcraft_project::render_queue::{
    AlphaMode, Channels, ColorDepth, CurrentOrOff, EffectsMode, FrameSample, RenderLog, ResizeQuality, SwitchOverride, log_path,
};
use effectcraft_project::{Comp, Project};
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

/// An H.264 export frame: RGBA8 to convert, or planes the accelerator already converted.
pub(crate) enum H264Frame {
    Rgba(Vec<u8>),
    Yuv420(Vec<u8>),
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
    map_frames_while(ks, cap, || true, render, encode)
}

/// [`map_frames`] whose `cap` holds only while `capped()`: once it turns false (the GPU stopped
/// for good, so every frame falls back to the CPU), the remaining frames render in parallel
/// like the uncapped CPU path instead of one chunk of `cap` at a time.
pub(crate) fn map_frames_while<I: Send, T: Send>(
    ks: Vec<u64>,
    cap: Option<usize>,
    capped: impl Fn() -> bool,
    render: impl Fn(u64) -> I + Sync + Send,
    encode: impl Fn(u64, I) -> T + Sync + Send,
) -> Vec<T> {
    use rayon::prelude::*;
    let Some(n) = cap.map(|n| n.max(1)).filter(|_| capped()) else {
        return ks.into_par_iter().map(|k| encode(k, render(k))).collect();
    };
    let window = n.max(rayon::current_num_threads());
    let mut out = Vec::with_capacity(ks.len());
    for (w, win) in ks.chunks(window).enumerate() {
        if !capped() {
            out.par_extend(ks[w * window..].par_iter().map(|&k| encode(k, render(k))));
            break;
        }
        let mut imgs = Vec::with_capacity(win.len());
        let mut i = 0;
        while i < win.len() {
            let end = if capped() { (i + n).min(win.len()) } else { win.len() };
            imgs.par_extend(win[i..end].par_iter().map(|&k| render(k)));
            i = end;
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
        let frame_times = (job.options.log == RenderLog::PlusPerFrameInfo).then(|| Mutex::new(Vec::new()));
        let cache = LayerCache::default();
        // With deferred readbacks, buffers computed from a missed pass's placeholders are not kept.
        cache.set_gate(job.accel.and_then(|a| a.miss_gate()));
        Cx { job, project, cache, frame_times, overflowed: Mutex::new(vec![]), written: Mutex::new(vec![]) }
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
        if self.deferred() {
            let mut out = Vec::with_capacity(ks.len());
            for k in ks {
                let img = self.frame_async(comp, k).await;
                out.push(f(k, img));
            }
            return out;
        }
        // The cap is for the GPU's memory: once it has stopped for good, frames render on the
        // CPU in parallel.
        let gpu_live = || self.job.accel.is_none_or(|a| a.available());
        map_frames_while(ks, self.gpu_cap(), gpu_live, |k| self.frame(comp, k), f)
    }

    /// GPU frames rendered at once (#879), decided once per batch: with a GPU accelerator every
    /// frame in flight holds its own working textures, so a whole batch rendered at once can
    /// exhaust VRAM (or shared memory, which crashes instead of falling back). `None` when the
    /// frames render on the CPU: no accelerator, or a Software Only project.
    fn gpu_cap(&self) -> Option<usize> {
        (self.job.accel.is_some() && self.project.settings.gpu_acceleration).then(|| gpu_inflight(std::env::var(GPU_INFLIGHT_ENV).ok().as_deref()))
    }

    /// Output frame `i`: sampled (fields, pulldown), at the colour depth, cropped and resized.
    pub fn frame(&self, comp: &Comp, i: u64) -> Image {
        effectcraft_render::passes::block_on(self.frame_async(comp, i))
    }

    /// [`Cx::frame`] as a future (deferred readbacks: the frame renders in passes).
    pub async fn frame_async(&self, comp: &Comp, i: u64) -> Image {
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
        if let Some(ft) = &self.frame_times {
            ft.lock().unwrap_or_else(|e| e.into_inner()).push((i, t0.elapsed().as_secs_f64()));
        }
        img
    }

    /// Output frame `i` as the H.264 export's 4:2:0 planes at `w`×`h`, rendered and converted
    /// by the accelerator ([`effectcraft_render::Accelerator::comp_frame_yuv420`]). Only for
    /// frames that need nothing done after the render (progressive, at the project's depth, no
    /// crop or resize, rendered at exactly `w`×`h`). `None`: render it with [`Cx::frame`].
    pub fn yuv420_frame(&self, comp: &Comp, i: u64, w: u32, h: u32) -> Option<Vec<u8>> {
        if self.deferred() {
            return None;
        }
        let FrameSample::Progressive(t) = self.job.settings.sample(comp, i) else { return None };
        let depth = self.job.settings.color_depth;
        if depth != ColorDepth::Current && depth.resolve(self.project.settings.bit_depth).levels().is_some() {
            return None;
        }
        let om = self.job.output;
        let scale = self.job.settings.resolution.clamp(0.01, 4.0);
        let size = |n: u32| ((n as f64 * scale).round() as u32).max(1);
        if om.resize.enabled || (size(comp.width), size(comp.height)) != (w, h) || om.crop.edges(w, h, scale) != (0, 0, 0, 0) {
            return None;
        }
        let t0 = Instant::now();
        let r = self.renderer();
        let planes = r.active_accel()?.comp_frame_yuv420(&r, self.job.comp, t, comp.background)?;
        let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
        if planes.len() != w as usize * h as usize + 2 * cw * ch {
            return None;
        }
        if let Some(ft) = &self.frame_times {
            ft.lock().unwrap_or_else(|e| e.into_inner()).push((i, t0.elapsed().as_secs_f64()));
        }
        Some(planes)
    }

    /// Output frames `ks` for the H.264 encoder: the accelerator's planes
    /// ([`Cx::yuv420_frame`]) where it can, otherwise `rgba` of the frame rendered as usual.
    pub(crate) fn h264_frames(&self, comp: &Comp, ks: Vec<u64>, w: u32, h: u32, rgba: impl Fn(u64, Image) -> Vec<u8> + Sync + Send) -> Vec<H264Frame> {
        map_frames(
            ks,
            self.gpu_cap(),
            |k| match self.yuv420_frame(comp, k, w, h) {
                Some(p) => H264Frame::Yuv420(p),
                None => H264Frame::Rgba(rgba(k, self.frame(comp, k))),
            },
            |_, f| f,
        )
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
    fn frames_render_in_parallel_once_the_gpu_stops() {
        // Every frame after the GPU stopped falls back to the CPU: the cap no longer applies.
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
        let (live, now, max) = (AtomicBool::new(true), AtomicUsize::new(0), AtomicUsize::new(0));
        let out = pool.install(|| {
            map_frames_while(
                (0..24).collect(),
                Some(1),
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
