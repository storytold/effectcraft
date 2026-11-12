//! Content-Aware Fill (Window ▸ Content-Aware Fill, Layer ▸ New ▸ Content-Aware Fill Layer):
//! fill the transparent area of a layer (what its masks cut away) across the work area or the
//! whole layer, render the result as a PNG sequence and add it as a "Fill" layer above the
//! source, like After Effects. The algorithms are `effectcraft_raster::inpaint` (Object:
//! flow-guided propagation + PatchMatch; Surface: propagation + one synthesised fill carried by
//! the flow; Edge Blend: membrane fill). It runs as a background job (Window ▸ Progress); in
//! the browser the job goes to a job worker ([`FillPlan`], `offload::WorkerJob::ContentFill`),
//! whose own WebGPU device renders the layer's frames in passes, and the files come back to
//! browser storage.

use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerId, LayerSource, build};
use effectcraft_raster::Image;
#[cfg(not(target_arch = "wasm32"))]
use effectcraft_raster::inpaint::SpillHandle;
use effectcraft_raster::inpaint::{FillInput, FillMethod, FillOpts, Frames, HoleMask, dilate};
use effectcraft_render::{Accelerator, EvalCtx, ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::scene_detect::LayerFrames;
use super::{CommandSpec, always, b_p, bad, f_p, has_layers, layer_p, str_p};
use crate::jobs::Apply;
use crate::{EngineError, Result, Session, cmd};

/// Content-Aware Fill panel settings (serde, in [`crate::EditorState`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FillSettings {
    /// `object`, `surface` or `edgeBlend`.
    pub method: String,
    /// `workArea` or `entire` (the layer's duration).
    pub range: String,
    /// Grow the fill area by this many pixels.
    pub alpha_expansion: f64,
    /// `none`, `subtle`, `moderate`, `strong`.
    pub lighting_correction: String,
    /// Reference frame: a layer of the comp and the comp time of the frame to use.
    pub reference_layer: Option<LayerId>,
    pub reference_time: Option<f64>,
}

impl Default for FillSettings {
    fn default() -> Self {
        FillSettings {
            method: "object".into(),
            range: "workArea".into(),
            alpha_expansion: 0.0,
            lighting_correction: "none".into(),
            reference_layer: None,
            reference_time: None,
        }
    }
}

fn lighting(s: &str) -> Option<f32> {
    Some(match s {
        "none" | "off" => 0.0,
        "subtle" => 0.35,
        "moderate" => 0.65,
        "strong" => 1.0,
        _ => return None,
    })
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "contentFill.set";
    let mut st = s.state.content_fill.clone();
    if let Some(m) = str_p(p, "method") {
        st.method = FillMethod::from_name(m).ok_or_else(|| bad(c, "method: object|surface|edgeBlend"))?.name().into();
    }
    if let Some(r) = str_p(p, "range") {
        st.range = match r {
            "workArea" | "work area" => "workArea".into(),
            "entire" | "entireDuration" | "layer" => "entire".into(),
            _ => return Err(bad(c, "range: workArea|entire")),
        };
    }
    if let Some(a) = f_p(p, "alphaExpansion") {
        st.alpha_expansion = a.clamp(0.0, 100.0);
    }
    if let Some(l) = str_p(p, "lightingCorrection") {
        lighting(l).ok_or_else(|| bad(c, "lightingCorrection: none|subtle|moderate|strong"))?;
        st.lighting_correction = l.into();
    }
    match p.get("referenceLayer") {
        Some(Value::Null) => st.reference_layer = None,
        Some(v) => {
            let comp = s.active_comp().ok_or(EngineError::NoComp)?;
            st.reference_layer = Some(super::resolve_layer(comp, v).ok_or_else(|| bad(c, format!("no layer {v}")))?);
            st.reference_time = Some(f_p(p, "referenceTime").unwrap_or(s.time().seconds()));
        }
        None => {
            if let Some(t) = f_p(p, "referenceTime") {
                st.reference_time = Some(t);
            }
        }
    }
    s.state.content_fill = st.clone();
    Ok(serde_json::to_value(st).unwrap_or_default())
}

/// A layer's masked source at comp time `t` on a `w × h` canvas in layer pixels (straight colour
/// with alpha 1 where kept), and the hole (`true` where the alpha is under one half).
fn masked_frame(r: &Renderer, ctx: &EvalCtx, layer: &effectcraft_project::Layer, w: u32, h: u32) -> (Image, Vec<bool>) {
    let mut img = Image::new(w, h);
    let mut hole = vec![true; (w * h) as usize];
    if let Some(buf) = r.layer_input(ctx, layer, 0) {
        let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
        for y in 0..h {
            for x in 0..w {
                let (bx, by) = ((x as f64 + 0.5) * sc - buf.offset[0], (y as f64 + 0.5) * sc - buf.offset[1]);
                let p = if (sc - 1.0).abs() < 1e-9 { buf.img.get(bx.floor() as i64, by.floor() as i64) } else { buf.img.sample_bilinear(bx, by) };
                let i = (y * w + x) as usize;
                if p[3] >= 0.5 {
                    img.data[i] = [p[0] / p[3], p[1] / p[3], p[2] / p[3], 1.0];
                    hole[i] = false;
                }
            }
        }
    }
    (img, hole)
}

/// Encode a straight-alpha frame (alpha 1) as 8-bit PNG.
fn png(img: &Image) -> std::result::Result<Vec<u8>, String> {
    let rgba: Vec<u8> = img.data.iter().flat_map(|p| [p[0], p[1], p[2], p[3]].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    super::comp_more::encode_png(&rgba, img.width, img.height)
}

fn default_dir(s: &Session, name: &str) -> String {
    let base = s.path.as_deref().and_then(|p| std::path::Path::new(p).parent().map(|d| d.join("Fill"))).unwrap_or_else(|| {
        // (the browser has no temporary folder: its storage)
        if cfg!(target_arch = "wasm32") { std::path::PathBuf::from("/Fill") } else { std::env::temp_dir().join("effectcraft-fill") }
    });
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let mut k = 1;
    loop {
        let d = base.join(format!("{safe}_Fill_{k}"));
        let ds = d.to_string_lossy().to_string();
        if !s.services.exists(&ds) && !s.services.exists(&format!("{ds}/fill_00000.png")) {
            return ds;
        }
        k += 1;
    }
}

/// A fill job, detached from the session (it also travels to a browser job worker).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FillPlan {
    pub comp: ItemId,
    pub layer: LayerId,
    /// The source layer's name.
    pub name: String,
    /// The layer's size (the frames' size).
    pub width: u32,
    pub height: u32,
    /// Comp times of the filled frames.
    pub times: Vec<Tick>,
    /// `object`, `surface` or `edgeBlend`.
    pub method: String,
    pub lighting: f32,
    /// Alpha expansion (px).
    pub expand: usize,
    /// Reference frame: (layer, comp time).
    pub reference: Option<(LayerId, Tick)>,
    /// Folder of the PNG sequence.
    pub dir: String,
}

/// Transient full frames one parallel stage holds per worker: the membrane reads a frame and
/// returns a copy, the flow stage holds the neighbouring pair plus the luma pyramids, and
/// Lighting Correction adds the mismatch ring (`diff`, its mask) on top.
///
/// Measured with `measure_peak_rss_of_a_fill` (90 frames, 640×360, Surface): peak RSS grows by
/// ~7 frames a rayon worker at the top of its run-to-run spread, and Lighting Correction stays
/// below the flow stage's peak, so 8 (9 with lighting) is a conservative constant (#2570).
fn per_worker_frames(lighting: bool) -> u64 {
    if lighting { 9 } else { 8 }
}

/// The sequential synthesis stage: the stored frame, the PatchMatch pyramid and its
/// coarse-to-fine copies, the warped previous frame, and the sink's prepared copy.
const SYNTH_FRAMES: u64 = 6;

/// Workers a range of `frames` can keep busy (rayon never runs more tasks than frames). The
/// scratch estimate must follow the pool that actually runs the parallel stages: a synchronous
/// fill stays in the caller's pool, while an asynchronous one runs on `spawn_task`'s own OS
/// thread and so uses the global pool, whatever pool the command was called from (#2570).
fn fill_workers(frames: usize, wait: bool) -> u64 {
    let pool = if wait { rayon::current_num_threads() as u64 } else { global_pool_threads() };
    pool.min(frames.max(1) as u64)
}

/// The global Rayon pool's thread count, read from a thread outside every pool:
/// `rayon::current_num_threads()` answers for the caller's pool, not the one the background
/// task uses. Falls back to the current pool when the probe thread cannot start.
fn global_pool_threads() -> u64 {
    let (tx, rx) = std::sync::mpsc::channel();
    match std::thread::Builder::new().name("fill-pool-probe".into()).spawn(move || {
        let _ = tx.send(rayon::current_num_threads());
    }) {
        Ok(handle) => {
            let n = rx.recv().map(|n| n as u64).unwrap_or_else(|_| rayon::current_num_threads() as u64);
            let _ = handle.join();
            n
        }
        Err(_) => rayon::current_num_threads() as u64,
    }
}

/// Native builds spill the input, the membrane-filled neighbours and the propagated copies, and
/// pack the holes to one bit per pixel, so only the two packed masks and the two flow fields
/// (~1/2 byte a pixel a frame) plus the workers' scratch stay resident (#2570). wasm has no
/// spill and keeps the input and the working copies in RAM (~32 bytes a pixel a frame). When
/// even that peak is more than `available`, report it up front instead of running out of memory
/// partway through (#1789, #2570 review).
#[cfg(test)]
fn memory_shortfall(frames: usize, w: u32, h: u32, available: u64) -> Option<String> {
    memory_shortfall_with(frames, w, h, available, fill_workers(frames, true), false, 0)
}

/// `workers` and `lighting` are parameters so the budget is testable: the scratch term used to
/// be a fixed 8 frames, which under-counted a wide machine by several times (#2570 review).
/// `resident` adds storage that would otherwise be spill: when the spill volume is RAM-backed,
/// its files use the same memory as the working set (#2570 review).
fn memory_shortfall_with(frames: usize, w: u32, h: u32, available: u64, workers: u64, lighting: bool, resident: u64) -> Option<String> {
    let px = u64::from(w).saturating_mul(u64::from(h));
    let per_frame = (frames as u64).saturating_mul(px);
    // `holes` and the leftover propagation masks, packed to a bit a pixel each.
    let masks = per_frame / 4;
    // The forward and backward flow fields, ~0.125 byte a pixel each.
    let flows = per_frame / 4;
    // wasm keeps the input and one working copy in RAM at every stage; a RAM-backed spill
    // volume counts there too.
    let resident = resident.saturating_add(if cfg!(target_arch = "wasm32") { per_frame.saturating_mul(32) } else { 0 });
    // The synthesis stage is sequential; every parallel stage holds a few frames a worker.
    let scratch_frames = SYNTH_FRAMES.saturating_add(per_worker_frames(lighting).saturating_mul(workers));
    let scratch = px.saturating_mul(16).saturating_mul(scratch_frames);
    let need = masks.saturating_add(flows).saturating_add(resident).saturating_add(scratch);
    (need > available).then(|| {
        let gb = |b: u64| b as f64 / 1e9;
        format!(
            "Content-Aware Fill of {frames} frames at {w}×{h} needs at least {:.1} GB of memory, and {:.1} GB is free: fill a shorter range (set the work area and choose Work Area) or close other applications",
            gb(need),
            gb(available)
        )
    })
}

/// Scratch floor for a native fill: the input, membrane-filled and propagated frames are each
/// spilled as f32 RGBA (16 bytes a pixel a frame), but the membrane spill is closed before the
/// propagated one is made, so at most two copies exist at once (2 × 16 bytes a pixel a frame),
/// plus a tenth for the PNG output.
#[cfg(not(target_arch = "wasm32"))]
fn scratch_need(frames: usize, w: u32, h: u32) -> u64 {
    let bytes = (frames as u64).saturating_mul(u64::from(w)).saturating_mul(u64::from(h)).saturating_mul(32);
    bytes.saturating_add(bytes / 10)
}

/// Refuse up front when the output volume cannot hold the fill's scratch. The folder may not
/// exist yet (the spill creates it), so query the nearest existing ancestor. `None` when the
/// volume cannot be read; the write itself still reports a real error then (#2570 review).
#[cfg(not(target_arch = "wasm32"))]
fn scratch_shortfall(frames: usize, w: u32, h: u32, dir: &std::path::Path) -> Option<String> {
    let need = scratch_need(frames, w, h);
    // Resolve a relative output folder against the process directory; `parent()` of a bare
    // name is empty, which would otherwise skip the check.
    let base = if dir.is_absolute() { dir.to_path_buf() } else { std::env::current_dir().ok()?.join(dir) };
    let mut probe = base.as_path();
    while !probe.exists() {
        probe = probe.parent()?;
    }
    let (_, free) = crate::sysinfo::disk_space(probe)?;
    (need > free).then(|| {
        let gb = |b: u64| b as f64 / 1e9;
        format!(
            "Content-Aware Fill of {frames} frames at {w}×{h} needs up to {:.1} GB of scratch space beside the output, and {:.1} GB is free there: fill a shorter range (set the work area and choose Work Area) or choose an output folder on a larger disk",
            gb(need),
            gb(free)
        )
    })
}

/// The status line shown while the frames are filled. Edge Blend calls only the membrane, which
/// the device backend leaves to the CPU reference, so it is not a device fill even when a `fill`
/// backend is present; Object and Surface run the flow and PatchMatch on the device.
fn filling_status(on_device: bool, method: FillMethod) -> &'static str {
    if on_device && method != FillMethod::EdgeBlend { "Filling (GPU)" } else { "Filling" }
}

/// The PNGs a fill has written so far. Unless the fill finishes ([`OutputFiles::keep`]), dropping
/// it deletes the ones it created, and the output folder too when the fill made it and nothing
/// else is in it, so a failed or stopped fill leaves no partial sequence behind (#2570 review). A
/// file that was already there (an `outputDir` holding an earlier sequence) is never deleted.
struct OutputFiles {
    dir: std::path::PathBuf,
    /// The folder was missing before the fill (the spill creates it).
    made_dir: bool,
    paths: Vec<String>,
    /// Paths among `paths` that existed before the fill wrote them: not the fill's to delete.
    spare: Vec<String>,
    kept: bool,
}

impl OutputFiles {
    /// The fill finished: keep its files.
    fn keep(mut self) -> Vec<String> {
        self.kept = true;
        std::mem::take(&mut self.paths)
    }
}

impl Drop for OutputFiles {
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        for path in self.paths.iter().filter(|p| !self.spare.contains(p)) {
            let _ = std::fs::remove_file(path);
        }
        // `remove_dir` leaves a folder that still holds anything else.
        if self.made_dir {
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

/// Run a fill: the layer's frames (rendered in passes with a job worker's GPU), the fill, the
/// PNG sequence written into `plan.dir` (`Services::store_file`). Returns the files.
///
/// `accel` renders the frames (a job worker's GPU, as for every analysis: `None` on the
/// desktop, whose analysis threads render on the CPU by design); `fill` is the fill's own
/// backend ([`effectcraft_raster::inpaint::FillBackend`]), which is what makes Content-Aware
/// Fill itself run on the GPU. They are separate on purpose: a desktop fill can run on the GPU
/// while its input frames are composited on the CPU.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_plan(
    plan: &FillPlan,
    project: &effectcraft_project::Project,
    footage: &dyn FootageSource,
    expr: Option<&dyn ExprHost>,
    cache: &LayerCache,
    services: &dyn crate::Services,
    accel: Option<&dyn Accelerator>,
    fill: Option<&dyn effectcraft_raster::inpaint::FillBackend>,
    progress: &mut dyn FnMut(u64, u64) -> bool,
    message: &mut dyn FnMut(&str),
) -> std::result::Result<Vec<String>, String> {
    message("Reading frames");
    let (cid, w, h) = (plan.comp, plan.width, plan.height);
    let comp = project.comp(cid).ok_or("composition missing")?;
    let layer = comp.layer(plan.layer).ok_or("layer missing")?;
    let mut r = Renderer::new(project, footage, crate::offload::analysis_opts(accel));
    r.expr = expr;
    r.cache = Some(cache);
    let r = &r;
    let ctx0 = EvalCtx { expr, ..EvalCtx::new(project, cid, comp, Tick::ZERO) };
    let times = &plan.times;
    let n = times.len() as u64;
    let mut holes = Vec::with_capacity(times.len());
    #[cfg(target_arch = "wasm32")]
    let mut frames = Vec::with_capacity(times.len());
    // Declared before the spill, so on an early return the spill is closed first and the folder
    // it was made in can be removed.
    let dir = std::path::Path::new(&plan.dir);
    let mut written = OutputFiles { dir: dir.to_path_buf(), made_dir: !dir.exists(), paths: Vec::with_capacity(times.len()), spare: Vec::new(), kept: false };
    // Spill 32-bit floats whatever the project depth: the old in-memory path kept f32 all the
    // way through, so quantising here would change the fill (#2570, "no quality loss"). Scratch
    // lives beside the output when one is set; the default temp folder may be RAM-backed, which
    // the memory preflight accounts for when it chooses the spill folder.
    #[cfg(not(target_arch = "wasm32"))]
    let spill = SpillHandle::create_in(Some(dir), w as usize, h as usize, times.len(), 32)
        .map_err(|e| format!("spill: cannot make the scratch file in {}: {e}", plan.dir))?;
    #[cfg(not(target_arch = "wasm32"))]
    let spill_probe = spill.clone();
    for (k, t) in times.iter().enumerate() {
        let (img, hole) = crate::offload::in_passes(accel, |a| masked_frame(&r.with_accel(a), &ctx0.at(*t), layer, w, h)).await;
        holes.push(HoleMask::from_vec(&dilate(&hole, w as usize, h as usize, plan.expand)));
        #[cfg(target_arch = "wasm32")]
        frames.push(img);
        #[cfg(not(target_arch = "wasm32"))]
        spill.write_frame(k, &img)?;
        if !progress(k as u64 + 1, n * 3) {
            return Err(crate::render_queue::CANCELLED.into());
        }
    }
    if !holes.iter().any(|h| h.any()) {
        return Err("the layer has no transparent area to fill: draw a mask (Subtract) around what to remove".into());
    }
    let mut references = vec![];
    if let Some((rl, rt)) = plan.reference
        && let Some(l) = comp.layer(rl)
    {
        let (img, hole) = crate::offload::in_passes(accel, |a| masked_frame(&r.with_accel(a), &ctx0.at(rt), l, w, h)).await;
        if hole.iter().all(|b| !*b) {
            let k = times.iter().enumerate().min_by_key(|(_, t)| (t.0 - rt.0).abs()).map(|(k, _)| k).unwrap_or(0);
            references.push((k, img));
        }
    }
    let opts = FillOpts { method: FillMethod::from_name(&plan.method).unwrap_or_default(), lighting: plan.lighting, ..Default::default() };
    message(filling_status(fill.is_some(), opts.method));
    #[cfg(target_arch = "wasm32")]
    let input_frames = Frames::Memory(frames);
    #[cfg(not(target_arch = "wasm32"))]
    let input_frames = Frames::Spilled(spill);
    let input = FillInput { frames: input_frames, holes, references };
    let cpu = effectcraft_raster::inpaint::CpuFill;
    let backend: &dyn effectcraft_raster::inpaint::FillBackend = fill.unwrap_or(&cpu);
    let progress = std::rc::Rc::new(std::cell::RefCell::new(progress));
    // The fill and the sink both advance the bar: a streamed frame reports ahead of the fill's own
    // progress, so both report a shared high-water mark and the bar never moves backwards.
    let high = std::cell::Cell::new(0u64);
    let report = |done: u64| -> bool {
        let done = done.max(high.get());
        high.set(done);
        (progress.borrow_mut())(done, n * 3)
    };
    let paths: Vec<String> = (0..n as usize).map(|k| dir.join(format!("fill_{k:05}.png")).to_string_lossy().to_string()).collect();
    let mut sink_error: Option<String> = None;
    let mut sink = |idx: usize, img: &Image| -> std::result::Result<(), String> {
        let r = (|| -> std::result::Result<(), String> {
            // A failed spilled read returns a black fallback frame; never publish it (#2570
            // review).
            #[cfg(not(target_arch = "wasm32"))]
            if spill_probe.has_error() {
                return Err(spill_probe.take_error().unwrap_or_else(|| "spill: read failed".to_string()));
            }
            let existed = services.exists(&paths[idx]);
            services.store_file(&paths[idx], &png(img)?).map_err(|e| format!("{}: {e}", paths[idx]))?;
            written.paths.push(paths[idx].clone());
            if existed {
                written.spare.push(paths[idx].clone());
            }
            if !report(2 * n + idx as u64 + 1) {
                return Err(crate::render_queue::CANCELLED.to_string());
            }
            Ok(())
        })();
        if let Err(e) = &r
            && sink_error.is_none()
        {
            sink_error = Some(e.clone());
        }
        r
    };
    let produced =
        effectcraft_raster::inpaint::fill_sequence_with(input, &opts, backend, &mut |d, t| report(n + (d as u64 * n / t.max(1) as u64)), Some(&mut sink));
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(e) = spill_probe.take_error() {
        return Err(e);
    }
    if produced.is_none() {
        if let Some(e) = sink_error.take() {
            return Err(e);
        }
        return Err(crate::render_queue::CANCELLED.into());
    }
    let produced = produced.unwrap_or_default();
    // Non-streaming fallback (tests): write any retained images.
    if !produced.is_empty() {
        for (k, img) in produced.iter().enumerate() {
            let path = paths[k].clone();
            services.store_file(&path, &png(img)?).map_err(|e| format!("{path}: {e}"))?;
            if written.paths.len() <= k {
                written.paths.push(path);
            }
            if !report(2 * n + k as u64 + 1) {
                return Err(crate::render_queue::CANCELLED.to_string());
            }
        }
    }
    // Every frame must have a file: the fill layer is built on the first one, and a short
    // sequence would play the wrong frames.
    if written.paths.len() != times.len() {
        return Err(format!("Content-Aware Fill wrote {} of {} frames", written.paths.len(), times.len()));
    }
    Ok(written.keep())
}

/// Add the fill layer of a finished plan (one undo step).
/// The plan comes back from a job worker, so an empty range is an error, not a crash.
pub(crate) fn apply_plan(s: &mut Session, plan: &FillPlan, paths: Vec<String>) -> Result<Value> {
    let importer = s.importer.clone();
    let start = plan.times.first().copied().ok_or_else(|| bad("contentFill.generate", "the fill has no frames"))?;
    add_fill_layer(s, plan.comp, plan.layer, &plan.name, paths, (plan.width, plan.height), start, plan.times.len(), importer)
}

/// `contentFill.generate`: validate the layer and range, refuse a fill that cannot fit in
/// memory or scratch space, then run it (in the background unless `wait` is set).
fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let c = "contentFill.generate";
    let (cid, lid) = layer_p(s, p, c)?;
    // Parameters override the panel settings.
    let mut st = s.state.content_fill.clone();
    {
        let saved = s.state.content_fill.clone();
        let mut q = p.clone();
        if let Some(o) = q.as_object_mut() {
            o.retain(|k, _| ["method", "range", "alphaExpansion", "lightingCorrection", "referenceLayer", "referenceTime"].contains(&k.as_str()));
        }
        set(s, &q)?;
        std::mem::swap(&mut st, &mut s.state.content_fill);
        s.state.content_fill = saved;
    }
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let layer = comp.layer(lid).ok_or(EngineError::NoComp)?.clone();
    if !layer.source.is_av() || layer.source.item().is_none() {
        return Err(bad(c, "Content-Aware Fill needs a footage, solid or precomp layer"));
    }
    let (w, h) = effectcraft_render::source_size(&s.project, &layer);
    if w == 0 || h == 0 {
        return Err(bad(c, "the layer has no pixels"));
    }
    let fr = comp.frame_rate;
    let (a, b) =
        if st.range == "entire" { (layer.in_point, layer.out_point) } else { (comp.work_area.0.max(layer.in_point), comp.work_area.1.min(layer.out_point)) };
    let (a, b) = (a.max(Tick::ZERO), b.min(comp.duration));
    if b <= a {
        return Err(bad(c, "the range doesn't overlap the layer"));
    }
    let (fa, fb) = (fr.frame_at(fr.snap_nearest(a)), fr.frame_at(b - Tick(1)));
    let times: Vec<Tick> = (fa..=fb).map(|f| fr.tick_of(f)).collect();
    let opts = FillOpts {
        method: FillMethod::from_name(&st.method).unwrap_or_default(),
        lighting: lighting(&st.lighting_correction).unwrap_or(0.0),
        ..Default::default()
    };
    let dir = str_p(p, "outputDir").map(str::to_string).unwrap_or_else(|| default_dir(s, &layer.name));
    // The spill lives beside the output. On a RAM-backed volume (`tmpfs`/`ramfs`, where the
    // default temp dir may live) its files use the same memory as the working set, so the
    // memory preflight counts them; disk-backed volumes get the space check below instead.
    #[cfg(not(target_arch = "wasm32"))]
    let ram_spill = if crate::sysinfo::is_memory_backed(std::path::Path::new(&dir)) { scratch_need(times.len(), w, h) } else { 0 };
    #[cfg(target_arch = "wasm32")]
    let ram_spill = 0;
    let wait = b_p(p, "wait").unwrap_or(false);
    // `spawn_task` runs a fill inline on wasm, and `runs_inline` selects the pool it will use.
    let runs_inline = wait || cfg!(target_arch = "wasm32");
    if let Some(e) = crate::sysinfo::memory()
        .and_then(|m| memory_shortfall_with(times.len(), w, h, m.available, fill_workers(times.len(), runs_inline), opts.lighting > 0.0, ram_spill))
    {
        return Err(bad(c, e));
    }
    let expand = st.alpha_expansion.round().max(0.0) as usize;
    let reference = st.reference_layer.filter(|r| comp.layer(*r).is_some()).map(|r| (r, Tick::from_seconds_f64(st.reference_time.unwrap_or(0.0))));
    // Native fills keep up to two f32 copies of the range in scratch beside the output; a volume
    // that cannot hold them is refused before any frame is rendered (#2570 review).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(e) = scratch_shortfall(times.len(), w, h, std::path::Path::new(&dir)) {
        return Err(bad(c, e));
    }
    let plan = FillPlan {
        comp: cid,
        layer: lid,
        name: layer.name.clone(),
        width: w,
        height: h,
        times,
        method: opts.method.name().to_string(),
        lighting: opts.lighting,
        expand,
        reference,
        dir,
    };
    // The browser: a job worker (its own GPU), the page keeps running.
    if !wait && s.offloads() {
        s.offload_analysis(crate::offload::WorkerJob::ContentFill { plan: Box::new(plan) }).map_err(EngineError::Other)?;
        return Ok(json!({"job": "contentFill", "started": true, "worker": true}));
    }
    let lf = LayerFrames::new(s, cid, lid);
    let services = s.services.clone();
    let importer = s.importer.clone();
    let name = layer.name.clone();
    // The fill's own GPU backend (Mercury GPU Acceleration; none under Software Only or
    // without a compositor). The frames themselves still render on the CPU here, as for every
    // desktop analysis (`offload::job_accel`): only the fill's own work moves to the device.
    let accel = if s.project.settings.gpu_acceleration { s.accel.clone() } else { None };
    s.spawn_task("contentFill", format!("Content-Aware Fill: {name}"), wait, move |ctl| {
        let fill = accel.as_deref().and_then(|a| a.inpaint());
        let paths = effectcraft_render::passes::block_on(run_plan(
            &plan,
            &lf.project,
            lf.footage.as_ref(),
            lf.expr.as_deref(),
            &lf.cache,
            services.as_ref(),
            None,
            fill,
            &mut |d, t| ctl.progress(d, t),
            &mut |m| ctl.message(m),
        ))?;
        let apply: Apply = Box::new(move |s: &mut Session| add_fill_layer(s, cid, lid, &name, paths, (w, h), plan.times[0], plan.times.len(), importer));
        Ok(apply)
    })
}

/// Import the rendered sequence and put it in a layer above the source.
#[allow(clippy::too_many_arguments)]
fn add_fill_layer(
    s: &mut Session,
    cid: ItemId,
    lid: LayerId,
    name: &str,
    paths: Vec<String>,
    size: (u32, u32),
    start: Tick,
    count: usize,
    importer: Option<Arc<dyn crate::Importer>>,
) -> Result<Value> {
    let first = paths.first().cloned().ok_or_else(|| bad("contentFill.generate", "the fill wrote no frames"))?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let src = comp.layer(lid).ok_or_else(|| bad("contentFill.generate", "the source layer is gone"))?.clone();
    let fr = comp.frame_rate;
    let dur = fr.tick_of(count as i64);
    let mut footage = importer.and_then(|i| i.probe(&first).ok()).filter(|f| f.has_video).unwrap_or_else(|| Footage {
        path: first.clone(),
        kind: FootageKind::Sequence,
        width: size.0,
        height: size.1,
        has_video: true,
        codec: "PNG".into(),
        ..Default::default()
    });
    footage.kind = FootageKind::Sequence;
    footage.sequence = paths.clone();
    footage.frame_rate = fr;
    footage.duration = dur;
    let (lname, item, layer) = s.edit("Content-Aware Fill", None, |proj, st| {
        let folder = proj.folder_named("Fill").unwrap_or_else(|| proj.add_item("Fill", Label::Yellow, None, ItemKind::Folder));
        let n = proj.items.values().filter(|i| i.parent == Some(folder)).count() + 1;
        let lname = format!("Fill {n} [{name}]");
        let iid = proj.add_item(&lname, Label::Lavender, Some(folder), ItemKind::Footage(footage));
        let mut l = build::layer(proj, &comp, &lname, LayerSource::Footage { item: iid }, size, Some(dur));
        l.start_time = start;
        l.in_point = start;
        l.out_point = (start + dur).min(comp.duration);
        l.parent = src.parent;
        l.switches.three_d = src.switches.three_d;
        if let (Some(dst), Some(t)) = (l.props.sub_mut("transform"), src.transform()) {
            let mut tr = t.clone();
            tr.reassign_uids(&mut proj.next_id);
            *dst = tr;
        }
        let id = l.id;
        let cm = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        let at = cm.layers.iter().position(|x| x.id == lid).unwrap_or(0);
        cm.layers.insert(at, l);
        st.selected_layers = vec![id];
        Ok((lname, iid, id))
    })?;
    Ok(json!({"layer": layer.0, "item": item.0, "name": lname, "frames": count, "files": paths}))
}

fn open_panel(s: &mut Session, p: &Value) -> Result<Value> {
    // Parameters given: generate straight away (agents); else show the panel.
    if p.as_object().is_some_and(|o| !o.is_empty()) {
        return generate(s, p);
    }
    s.events.push(crate::Event::Frontend { command: "window.panel".into(), params: json!({"panel": "contentAwareFill"}) });
    Ok(json!({"frontend": "window.panel"}))
}

pub fn specs() -> Vec<CommandSpec> {
    const P: &str = "{layer?, method?: object|surface|edgeBlend, range?: workArea|entire, alphaExpansion? (px), lightingCorrection?: none|subtle|moderate|strong, referenceLayer?, referenceTime? (s), outputDir?, wait?: bool}";
    vec![
        cmd!(
            "contentFill.set",
            "Content-Aware Fill Settings",
            [],
            None,
            "{method?: object|surface|edgeBlend, range?: workArea|entire, alphaExpansion? (px), lightingCorrection?: none|subtle|moderate|strong, referenceLayer?: layer|null, referenceTime? (s)}",
            always,
            set
        ),
        CommandSpec {
            id: "contentFill.generate",
            label: "Generate Fill Layer",
            menu: &[],
            shortcut: None,
            alt_shortcuts: &[],
            params: P,
            enabled: has_layers,
            run: generate,
            journal: true,
        },
        CommandSpec {
            id: "layer.newContentAwareFill",
            label: "Content-Aware Fill Layer...",
            menu: &["Layer", "New"],
            shortcut: None,
            alt_shortcuts: &[],
            params: P,
            enabled: always,
            run: open_panel,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use super::scratch_need;
    #[cfg(not(target_arch = "wasm32"))]
    use super::scratch_shortfall;
    use super::{filling_status, memory_shortfall, memory_shortfall_with};

    /// Edge Blend fills with the membrane, which the device backend leaves to the CPU reference:
    /// it must not claim a GPU fill just because a backend is available.
    #[test]
    fn edge_blend_is_not_reported_as_a_device_fill() {
        use effectcraft_raster::inpaint::FillMethod;
        assert_eq!(filling_status(true, FillMethod::Object), "Filling (GPU)");
        assert_eq!(filling_status(true, FillMethod::Surface), "Filling (GPU)");
        assert_eq!(filling_status(true, FillMethod::EdgeBlend), "Filling");
        assert_eq!(filling_status(false, FillMethod::Object), "Filling");
    }

    /// #1789 / #2570: a range that cannot fit is refused up front instead of OOM-ing. The
    /// scratch term scales with the workers and Lighting Correction, not a fixed frame count.
    #[test]
    fn a_fill_that_cannot_fit_in_memory_is_refused_up_front() {
        let e = memory_shortfall_with(300, 1920, 1080, 1_000_000, 8, false, 0).expect("refused");
        assert!(e.contains("300 frames") && e.contains("Work Area"), "{e}");
        // Four workers, no lighting: the packed masks and flows plus ~38 scratch frames fit.
        assert_eq!(memory_shortfall_with(30, 1920, 1080, 4_000_000_000, 4, false, 0), None);
        assert_eq!(memory_shortfall_with(300, 1920, 1080, 2_000_000_000, 4, false, 0), None);
        // A wide machine with Lighting Correction holds nine frames a worker: 300 frames no
        // longer fit in 5 GB, which the old fixed 8-frame scratch term missed (#2570 review).
        assert!(memory_shortfall_with(300, 1920, 1080, 1_000_000_000, 16, true, 0).is_some());
        assert!(memory_shortfall_with(300, 1920, 1080, 5_000_000_000, 16, true, 0).is_some());
        assert_eq!(memory_shortfall_with(300, 1920, 1080, 6_000_000_000, 16, true, 0), None);
        // The two packed masks still grow with the range: a very long 1080p range cannot fit.
        assert!(memory_shortfall_with(1000, 1920, 1080, 500_000_000, 8, false, 0).is_some());
        // A RAM-backed spill volume counts its bytes as resident, so the same range that fit on
        // a disk-backed volume is refused (#2570 review).
        assert_eq!(memory_shortfall_with(30, 1920, 1080, 2_500_000_000, 4, false, 0), None);
        assert!(memory_shortfall_with(30, 1920, 1080, 2_500_000_000, 4, false, 2_000_000_000).is_some());
        // Hostile sizes saturate instead of overflowing.
        assert!(memory_shortfall(usize::MAX, u32::MAX, u32::MAX, 0).is_some());
    }

    /// The native spill holds at most two f32 copies of the range beside the output at once (the
    /// membrane spill is gone before the propagated one is made), plus the PNGs.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scratch_space_is_checked_before_a_fill() {
        let copies = 300u64 * 1920 * 1080 * 32;
        assert_eq!(scratch_need(300, 1920, 1080), copies + copies / 10);
        let tmp = std::env::temp_dir();
        // A range that cannot possibly fit on the volume is refused up front.
        assert!(scratch_shortfall(usize::MAX, u32::MAX, u32::MAX, &tmp).is_some());
        // A relative folder that does not exist is resolved against the process directory.
        assert!(scratch_shortfall(usize::MAX, u32::MAX, u32::MAX, std::path::Path::new("effectcraft-missing-fill-folder")).is_some());
        // A tiny fill fits.
        assert_eq!(scratch_shortfall(1, 16, 16, &tmp), None);
    }

    /// A fill that fails or is stopped after some frames were written deletes them, and the
    /// output folder it made; a finished fill keeps its files (#2570 review).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_stopped_fill_removes_its_partial_output() {
        use super::OutputFiles;
        let root = tempfile::tempdir().unwrap();
        let write = |dir: &std::path::Path, made_dir: bool| {
            std::fs::create_dir_all(dir).unwrap();
            let paths: Vec<String> = (0..2).map(|k| dir.join(format!("fill_{k:05}.png")).to_string_lossy().to_string()).collect();
            for p in &paths {
                std::fs::write(p, b"png").unwrap();
            }
            OutputFiles { dir: dir.to_path_buf(), made_dir, paths, spare: Vec::new(), kept: false }
        };
        // The fill made the folder: the files and the folder go.
        let made = root.path().join("made");
        drop(write(&made, true));
        assert!(!made.exists(), "the partial sequence and its folder are removed");
        // The folder was already there: only the fill's own files go.
        let existing = root.path().join("existing");
        drop(write(&existing, false));
        assert!(existing.exists());
        assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 0);
        // Something else in a folder the fill made keeps the folder.
        let shared = root.path().join("shared");
        let files = write(&shared, true);
        std::fs::write(shared.join("notes.txt"), b"keep").unwrap();
        drop(files);
        assert_eq!(std::fs::read_dir(&shared).unwrap().count(), 1);
        // A file that was there before the fill wrote over it (an earlier sequence in a given
        // `outputDir`) is not the fill's to delete.
        let earlier = root.path().join("earlier");
        let mut files = write(&earlier, false);
        files.spare.push(files.paths[0].clone());
        drop(files);
        assert!(earlier.join("fill_00000.png").exists(), "an earlier fill's frame stays");
        assert!(!earlier.join("fill_00001.png").exists(), "the fill's own frame goes");
        // A finished fill keeps its files.
        let done = root.path().join("done");
        let kept = write(&done, true).keep();
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|p| std::path::Path::new(p).exists()));
    }
}
