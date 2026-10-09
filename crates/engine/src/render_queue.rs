//! Render Queue runtime: the [`Exporter`] hook (implemented by `effectcraft-export`, wired by
//! `effectcraft-host`), output path resolution, and the render job that works through the queued
//! items, in the background (desktop UI) or blocking (CLI, agents, tests).
//!
//! The queue itself (items, Render Settings, Output Modules, status) lives in the project
//! (`project.render_queue`) so it is saved with it. A running job owns a snapshot of the project;
//! it reports progress through [`JobShared`], and [`Session::poll_render`] copies item status /
//! start time / render time back into the project.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_project::render_queue::{OutputFormat, OutputModule, PostRenderAction, RenderQueueItem, RenderStatus, TemplateVars, expand_template};
use effectcraft_project::{ItemId, ItemKind, LayerSource, Project};
use effectcraft_render::{ExprHost, FootageSource};
use serde::{Deserialize, Serialize};

use crate::{Event, Session};

/// One export handed to the [`Exporter`].
pub struct ExportJob<'a> {
    pub project: &'a Project,
    pub footage: &'a dyn FootageSource,
    pub expr: Option<&'a dyn ExprHost>,
    pub item: &'a RenderQueueItem,
    /// GPU compositor to render with when the project's renderer is Mercury GPU Acceleration.
    pub accel: Option<&'a dyn effectcraft_render::Accelerator>,
    /// Resolved output path (templates expanded except the `#` frame-number run).
    pub path: &'a str,
    /// Free-space hook (Use Storage Overflow).
    pub storage: Option<&'a dyn effectcraft_project::render_queue::StorageQuota>,
    /// "#n Comp Name", for the render log.
    pub label: String,
    /// Settings ▸ General ▸ Switches Affect Nested Comps.
    pub nested_switches: bool,
}

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ExportResult {
    pub path: String,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub seconds: f64,
    pub audio: bool,
    /// The render log written next to the output, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// Files that went to storage overflow folders.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overflow: Vec<String>,
}

/// Encodes render queue items (implemented by the export layer).
pub trait Exporter: Send + Sync {
    /// Formats this build can write.
    fn formats(&self) -> Vec<OutputFormat>;
    /// Render and write one item (blocking). `progress(done, total)` returns `false` to cancel;
    /// a cancelled export returns `Err("cancelled")`.
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String>;
    /// [`Exporter::export`] as a future: renders whose GPU readbacks are deferred (a browser
    /// job worker's WebGPU device) await the device between a frame's passes. The default runs
    /// the blocking export.
    fn export_async<'a>(&'a self, job: &'a ExportJob<'a>, progress: &'a mut dyn FnMut(u64, u64) -> bool) -> LocalFuture<'a, Result<ExportResult, String>> {
        Box::pin(async move { self.export(job, progress) })
    }
}

/// A boxed future that stays on its thread.
pub type LocalFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + 'a>>;

/// The error string an [`Exporter`] returns when cancelled.
pub const CANCELLED: &str = "cancelled";

/// Live progress of a render job (serde for agents / the control channel).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JobState {
    /// Item being rendered.
    pub current: Option<u64>,
    /// Frames done / total of the current item.
    pub done: u64,
    pub total: u64,
    /// Seconds since the job started / since the current item started.
    pub elapsed: f64,
    pub item_elapsed: f64,
    /// Estimated seconds left for the current item.
    pub remaining: Option<f64>,
    /// Items rendered so far / items in this job.
    pub items_done: usize,
    pub items_total: usize,
    pub finished: bool,
}

/// Per-item status changes from the job thread, applied to the project by `poll_render`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum ItemUpdate {
    Started { id: u64, unix: u64 },
    Finished { id: u64, status: RenderStatus, seconds: f64, output: Option<String> },
}

#[derive(Default)]
pub struct JobShared {
    pub cancel: AtomicBool,
    pub state: Mutex<JobState>,
    pub updates: Mutex<Vec<ItemUpdate>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl JobShared {
    pub fn snapshot(&self) -> JobState {
        lock(&self.state).clone()
    }
}

/// A running (or finished, not yet polled) render job.
pub struct RenderJob {
    pub shared: Arc<JobShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Rendering in a worker ([`crate::offload`]).
    pub remote: Option<crate::offload::RemoteRender>,
}

impl RenderJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
    /// Block until the worker thread ends.
    pub fn wait(&mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Everything the worker needs, detached from the session.
struct Work {
    project: Arc<Project>,
    footage: Arc<dyn FootageSource>,
    expr: Option<Arc<dyn ExprHost>>,
    accel: Option<Arc<dyn effectcraft_render::Accelerator>>,
    exporter: Arc<dyn Exporter>,
    storage: Option<Arc<dyn effectcraft_project::render_queue::StorageQuota>>,
    /// Per queue item: one (item with that output module, resolved path) per output module
    /// (and per segment, see [`Session::segments`]).
    items: Vec<Vec<(RenderQueueItem, String)>>,
    /// Settings ▸ General ▸ Switches Affect Nested Comps.
    nested_switches: bool,
}

/// Bits per channel an output format writes for a project bit depth (named in the file when
/// Settings ▸ Export ▸ Append Bit Depth to File Name is on).
pub fn output_bits(format: OutputFormat, depth: effectcraft_project::BitDepth) -> u32 {
    use effectcraft_project::BitDepth;
    let project = match depth {
        BitDepth::Bpc8 => 8,
        BitDepth::Bpc16 => 16,
        BitDepth::Bpc32 => 32,
    };
    match format {
        OutputFormat::ExrSequence => 32,
        OutputFormat::PngSequence | OutputFormat::TiffSequence => project.min(16),
        OutputFormat::ProRes => 10,
        OutputFormat::Wav | OutputFormat::Aiff => 16,
        _ => 8,
    }
}

/// The bytes an output writes per frame, estimated from its data rate (H.264 bitrate, Apple's
/// published ProRes target rates at 1080p29.97 scaled by frame size, typical rates otherwise).
pub fn bytes_per_frame(om: &OutputModule, w: u32, h: u32, fps: f64) -> f64 {
    use effectcraft_project::render_queue::ProResProfile::*;
    let fps = fps.max(1.0);
    let px = (w as f64 * h as f64) / (1920.0 * 1080.0);
    let mbps = |m: f64| m * 1_000_000.0 / 8.0;
    match om.format {
        OutputFormat::H264 | OutputFormat::Hevc | OutputFormat::Av1 => om.bitrate_kbps as f64 * 1000.0 / 8.0 / fps,
        OutputFormat::ProRes => {
            let rate = match om.prores_profile {
                Proxy => 45.0,
                Lt => 102.0,
                Standard => 147.0,
                Hq => 220.0,
                P4444 => 330.0,
                P4444Xq => 500.0,
            };
            mbps(rate) * px / 29.97
        }
        OutputFormat::WebM => mbps(8.0) * px / fps,
        OutputFormat::Gif => w as f64 * h as f64 * 0.5,
        OutputFormat::Wav | OutputFormat::Aiff => om.audio_sample_rate as f64 * 4.0 / fps,
        _ => w as f64 * h as f64 * 3.0,
    }
}

fn unix_now() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Render `items` with the session's footage, expressions and GPU (a worker's render). A
/// future: with deferred GPU readbacks (a browser job worker) each frame awaits the device
/// between its passes.
pub(crate) async fn run_items(
    s: &Session,
    exporter: Arc<dyn Exporter>,
    items: Vec<Vec<(RenderQueueItem, String)>>,
    shared: &JobShared,
    notify: &mut dyn FnMut(&JobShared),
) {
    let work = Work {
        project: s.project.clone(),
        footage: s.footage.clone(),
        expr: s.expr.clone(),
        accel: s.accel.clone(),
        exporter,
        storage: s.storage_quota.clone(),
        items,
        nested_switches: s.prefs.general.switches_affect_nested_comps,
    };
    run_work(work, shared, notify).await;
}

async fn run_work(w: Work, shared: &JobShared, notify: &mut dyn FnMut(&JobShared)) {
    let t0 = web_time::Instant::now();
    {
        let mut s = lock(&shared.state);
        s.items_total = w.items.len();
    }
    for (k, modules) in w.items.iter().enumerate() {
        let Some((item, _)) = modules.first() else { continue };
        if shared.cancel.load(Ordering::Relaxed) {
            lock(&shared.updates).push(ItemUpdate::Finished { id: item.id, status: RenderStatus::UserStopped, seconds: 0.0, output: None });
            continue;
        }
        let t1 = web_time::Instant::now();
        lock(&shared.updates).push(ItemUpdate::Started { id: item.id, unix: unix_now() });
        {
            let mut s = lock(&shared.state);
            s.current = Some(item.id);
            s.done = 0;
            s.total = 0;
            s.items_done = k;
            s.remaining = None;
        }
        notify(shared);
        // Every output module encodes the same frames (Composition ▸ Add Output Module); the
        // first module's file is the item's output.
        let mut r: Result<ExportResult, String> = Err("no output module".into());
        for (mi, (mitem, path)) in modules.iter().enumerate() {
            let name = w.project.item(mitem.comp).map(|i| i.name.clone()).unwrap_or_default();
            let index = w.project.render_queue.iter().position(|i| i.id == mitem.id).map_or(0, |i| i + 1);
            let job = ExportJob {
                project: &w.project,
                footage: w.footage.as_ref(),
                expr: w.expr.as_deref(),
                accel: w.accel.as_deref(),
                item: mitem,
                path,
                storage: w.storage.as_deref(),
                label: format!("#{index} {name} (output module {})", mi + 1),
                nested_switches: w.nested_switches,
            };
            let rr = w
                .exporter
                .export_async(&job, &mut |done, total| {
                    {
                        let mut s = lock(&shared.state);
                        s.done = done;
                        s.total = total;
                        s.elapsed = t0.elapsed().as_secs_f64();
                        s.item_elapsed = t1.elapsed().as_secs_f64();
                        s.remaining = (done > 0).then(|| s.item_elapsed / done as f64 * total.saturating_sub(done) as f64);
                    }
                    notify(shared);
                    !shared.cancel.load(Ordering::Relaxed)
                })
                .await;
            match rr {
                Ok(res) if mi == 0 => r = Ok(res),
                Ok(_) => {}
                Err(e) => {
                    r = Err(e);
                    break;
                }
            }
        }
        let seconds = t1.elapsed().as_secs_f64();
        let up = match r {
            Ok(res) => ItemUpdate::Finished { id: item.id, status: RenderStatus::Done, seconds, output: Some(res.path) },
            Err(e) if e == CANCELLED => ItemUpdate::Finished { id: item.id, status: RenderStatus::UserStopped, seconds, output: None },
            Err(e) => {
                log::warn!("render queue: item {} failed: {e}", item.id);
                ItemUpdate::Finished { id: item.id, status: RenderStatus::Failed(e), seconds, output: None }
            }
        };
        lock(&shared.updates).push(up);
        notify(shared);
    }
    let mut s = lock(&shared.state);
    s.items_done = w.items.len();
    s.current = None;
    s.elapsed = t0.elapsed().as_secs_f64();
    s.finished = true;
}

impl Session {
    /// The project's name for `[projectName]` (file stem, or "Untitled Project").
    pub fn project_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled Project".into())
    }

    /// The output path of a queue item: template tokens expanded, ending in the format's
    /// extension exactly once ([`OutputFormat::with_extension`]), relative paths resolved against
    /// the project's folder (or the working directory). `None` when the comp is gone or the
    /// output is empty ("Needs Output").
    pub fn resolve_output(&self, item: &RenderQueueItem) -> Option<String> {
        let comp = self.project.comp(item.comp)?;
        let name = self.project.item(item.comp).map(|i| i.name.clone()).unwrap_or_default();
        let tpl = item.output.output.trim();
        if tpl.is_empty() {
            return None;
        }
        let (w, h) = item.output.output_size(comp, &item.settings);
        let rate = item.settings.rate(comp);
        let n = item.settings.frame_count(comp) as i64;
        let f0 = item.settings.first_frame(comp);
        let pname = self.project_name();
        let vars =
            TemplateVars { comp_name: &name, project_name: &pname, width: w, height: h, frame_rate: rate.as_f64(), start_frame: f0, end_frame: f0 + n - 1 };
        // The format's extension exactly once, whatever the name or a save dialog added (#293).
        let p = item.output.format.with_extension(&expand_template(tpl, item.output.format, &vars));
        let path = std::path::Path::new(&p);
        if path.is_absolute() {
            return Some(p);
        }
        // Settings ▸ Export ▸ Default Output Folder, else the project's folder.
        let folder = self.prefs.export.default_output_folder.trim();
        let base = (!folder.is_empty()).then(|| std::path::PathBuf::from(folder));
        let base =
            base.or_else(|| self.path.as_deref().and_then(|s| std::path::Path::new(s).parent().map(|d| d.to_path_buf())).filter(|d| !d.as_os_str().is_empty()));
        // (the web has no working directory: relative outputs land in its virtual root)
        let base = base.or_else(|| std::env::current_dir().ok()).or_else(|| cfg!(target_arch = "wasm32").then(|| "/".into()))?;
        Some(base.join(path).to_string_lossy().to_string())
    }

    /// Settings ▸ Export ▸ Segment Sequences / Segment Movie Files: the (item, path) outputs one
    /// output module writes. Sequences go into numbered folders of Files per Segment frames each
    /// (`out/Comp 1_001/Comp 1_00000.png`…); movies into numbered files of about Segment Size MB
    /// each (`Comp 1_001.mp4`…), cut at the frame count the output's data rate gives.
    pub fn segments(&self, item: &RenderQueueItem, path: &str) -> Vec<(RenderQueueItem, String)> {
        let Some(comp) = self.project.comp(item.comp) else { return vec![(item.clone(), path.to_string())] };
        let fmt = item.output.format;
        let frames = item.settings.frame_count(comp);
        let (w, h) = item.settings.output_size(comp);
        let bpf = bytes_per_frame(&item.output, w, h, item.settings.rate(comp).as_f64());
        let Some(per) = self.prefs.segment_frames(frames, fmt.is_sequence(), bpf) else { return vec![(item.clone(), path.to_string())] };
        let p = std::path::Path::new(path);
        let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = p.extension().map(|s| s.to_string_lossy().to_string());
        let rate = item.settings.rate(comp);
        let f0 = item.settings.first_frame(comp);
        let mut out = vec![];
        let mut i = 0;
        let mut k = 1;
        while i < frames {
            let n = per.min(frames - i);
            let mut m = item.clone();
            let (start, end) = (rate.tick_of(f0 + i as i64), rate.tick_of(f0 + (i + n) as i64));
            m.settings.time_span = effectcraft_project::render_queue::TimeSpan::Custom { start, end };
            let seg = if fmt.is_sequence() {
                let base = stem.trim_end_matches(['#', '[', ']', '_']).to_string();
                let folder = dir.join(format!("{base}_{k:03}"));
                folder.join(p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default())
            } else {
                dir.join(match &ext {
                    Some(e) => format!("{stem}_{k:03}.{e}"),
                    None => format!("{stem}_{k:03}"),
                })
            };
            out.push((m, seg.to_string_lossy().to_string()));
            i += n;
            k += 1;
        }
        out
    }

    /// Whether a render job is running.
    pub fn is_rendering(&self) -> bool {
        self.render_job.as_ref().is_some_and(|j| !j.is_finished())
    }

    /// Live job progress.
    pub fn render_progress(&self) -> Option<JobState> {
        self.render_job.as_ref().map(|j| j.shared.snapshot())
    }

    /// Start rendering every queued item. `wait`: block until done (otherwise a background thread
    /// renders, or the [`Session::offload`] worker; call [`Session::poll_render`] regularly).
    /// Returns the ids being rendered. Without threads or an offload (wasm32) the render always
    /// runs to completion before this returns.
    pub fn start_render(&mut self, wait: bool) -> Result<Vec<u64>, String> {
        let offload = self.offload.clone().filter(|_| !wait);
        let wait = wait || (cfg!(target_arch = "wasm32") && offload.is_none());
        if self.is_rendering() {
            return Err("a render is already in progress".into());
        }
        self.poll_render();
        let exporter = self.exporter.clone().ok_or("export is not available in this build")?;
        let formats = exporter.formats();
        let mut items = Vec::new();
        let mut needs_output = Vec::new();
        for it in self.project.render_queue.iter().filter(|i| i.is_queued()) {
            let mut modules = vec![];
            let mut problem = None;
            for om in it.output_modules() {
                let mut m = it.clone();
                m.output = om.clone();
                match self.resolve_output(&m) {
                    Some(p) if formats.contains(&om.format) => {
                        // Settings ▸ Export: Append Bit Depth to File Name, Segment Sequences /
                        // Movie Files.
                        let bits = output_bits(om.format, self.project.settings.bit_depth);
                        let p = self.prefs.output_name(&p, bits);
                        modules.extend(self.segments(&m, &p));
                    }
                    Some(_) => problem = Some(RenderStatus::Failed(format!("{} export is not available", om.format.label()))),
                    None if self.project.comp(it.comp).is_none() => problem = Some(RenderStatus::Failed("composition missing".into())),
                    None => problem = Some(RenderStatus::NeedsOutput),
                }
                if problem.is_some() {
                    break;
                }
            }
            match problem {
                Some(st) => needs_output.push((it.id, st)),
                None => items.push(modules),
            }
        }
        if !needs_output.is_empty() {
            let p = Arc::make_mut(&mut self.project);
            for (id, st) in needs_output {
                if let Some(i) = p.render_queue.iter_mut().find(|i| i.id == id) {
                    i.status = st;
                }
            }
            self.bump();
        }
        if items.is_empty() {
            return Err("nothing is queued: add a composition (Composition ▸ Add to Render Queue) and tick Render".into());
        }
        let ids: Vec<u64> = items.iter().filter_map(|m| m.first().map(|(i, _)| i.id)).collect();
        if let Some(off) = offload {
            let id = crate::offload::next_id();
            let inbox = Arc::new(crate::offload::Inbox::default());
            let req = crate::offload::WorkerRequest {
                id,
                project: self.project.to_json(),
                files: crate::offload::footage_files(&self.project),
                job: crate::offload::WorkerJob::Render { items },
            };
            off.start(req, inbox.clone())?;
            let shared = Arc::new(JobShared::default());
            lock(&shared.state).items_total = ids.len();
            self.render_job = Some(RenderJob { shared, thread: None, remote: Some(crate::offload::RemoteRender { id, inbox, ids: ids.clone() }) });
            self.poll_render();
            return Ok(ids);
        }
        let work = Work {
            project: self.project.clone(),
            footage: self.footage.clone(),
            expr: self.expr.clone(),
            accel: self.accel.clone(),
            exporter,
            storage: self.storage_quota.clone(),
            items,
            nested_switches: self.prefs.general.switches_affect_nested_comps,
        };
        let shared = Arc::new(JobShared::default());
        if wait {
            effectcraft_render::passes::block_on(run_work(work, &shared, &mut |_| {}));
            self.render_job = Some(RenderJob { shared, thread: None, remote: None });
        } else {
            let sh = shared.clone();
            let thread = std::thread::Builder::new()
                .name("render-queue".into())
                .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh, &mut |_| {})))
                .map_err(|e| e.to_string())?;
            self.render_job = Some(RenderJob { shared, thread: Some(thread), remote: None });
        }
        self.poll_render();
        Ok(ids)
    }

    /// Stop the running render (the current item becomes "User Stopped").
    pub fn stop_render(&mut self) -> bool {
        match &self.render_job {
            Some(j) if !j.is_finished() => {
                j.cancel();
                if let Some(r) = &j.remote {
                    // A worker can't be interrupted mid-frame: terminate it and stop what's left.
                    if let Some(off) = &self.offload {
                        off.cancel(r.id);
                    }
                    self.drain_remote_render();
                    let Some(j) = self.render_job.as_ref() else { return true };
                    let Some(r) = j.remote.as_ref() else { return true };
                    let finished: Vec<u64> =
                        lock(&j.shared.updates).iter().filter_map(|u| if let ItemUpdate::Finished { id, .. } = u { Some(*id) } else { None }).collect();
                    for id in &r.ids {
                        let done = finished.contains(id)
                            || self.project.render_queue.iter().any(|i| i.id == *id && matches!(i.status, RenderStatus::Done | RenderStatus::Failed(_)));
                        if !done {
                            lock(&j.shared.updates).push(ItemUpdate::Finished { id: *id, status: RenderStatus::UserStopped, seconds: 0.0, output: None });
                        }
                    }
                    lock(&j.shared.state).finished = true;
                }
                true
            }
            _ => false,
        }
    }

    /// Apply job updates to the queue items; drop the job once it finished. Returns whether
    /// anything changed. Frontends call this every frame while rendering.
    pub fn poll_render(&mut self) -> bool {
        self.drain_remote_render();
        let Some(job) = &self.render_job else { return false };
        let updates = std::mem::take(&mut *lock(&job.shared.updates));
        let finished = job.is_finished();
        let changed = !updates.is_empty();
        let mut post: Vec<(u64, String)> = vec![];
        if changed {
            let p = Arc::make_mut(&mut self.project);
            for u in &updates {
                match u {
                    ItemUpdate::Started { id, unix } => {
                        if let Some(i) = p.render_queue.iter_mut().find(|i| i.id == *id) {
                            i.status = RenderStatus::Rendering;
                            i.started = Some(*unix);
                            i.render_time = None;
                        }
                    }
                    ItemUpdate::Finished { id, status, seconds, output } => {
                        if let Some(i) = p.render_queue.iter_mut().find(|i| i.id == *id) {
                            i.status = status.clone();
                            i.render_time = Some(*seconds);
                            if output.is_some() {
                                i.last_output = output.clone();
                            }
                            if let (RenderStatus::Done, Some(o)) = (status, output)
                                && !i.post_render.is_none()
                            {
                                post.push((*id, o.clone()));
                            }
                        }
                    }
                }
            }
            self.bump();
        }
        if finished && let Some(mut job) = self.render_job.take() {
            if let Some(t) = job.thread.take() {
                let _ = t.join();
            }
            let st = job.shared.snapshot();
            let failed = updates.iter().filter(|u| matches!(u, ItemUpdate::Finished { status: RenderStatus::Failed(_), .. })).count();
            let stopped = updates.iter().any(|u| matches!(u, ItemUpdate::Finished { status: RenderStatus::UserStopped, .. }));
            let msg = if failed > 0 {
                format!("Render finished with {failed} failed item(s)")
            } else if stopped {
                "Render stopped".to_string()
            } else {
                format!("Rendered {} item(s) in {:.1} s", st.items_total, st.elapsed)
            };
            if self.project.render_prefs.notify {
                // Notify: the frontend plays a sound and shows a system notification.
                self.events.push(Event::Frontend { command: "renderQueue.notify".into(), params: serde_json::json!({"message": msg, "error": failed > 0}) });
            }
            self.events.push(Event::Toast { message: msg, error: failed > 0 });
        }
        for (id, path) in post {
            if let Err(e) = self.post_render_action(id, &path) {
                self.events.push(Event::Toast { message: format!("Post-render action: {e}"), error: true });
            }
        }
        changed || finished
    }

    /// Run an item's Post-Render Action: import the rendered file, and for Import & Replace
    /// Usage swap every layer that used the rendered composition to the new footage (one undo
    /// step).
    pub fn post_render_action(&mut self, id: u64, path: &str) -> Result<Option<ItemId>, String> {
        let Some(item) = self.project.render_queue.iter().find(|i| i.id == id).cloned() else { return Ok(None) };
        if item.post_render.is_none() {
            return Ok(None);
        }
        // A rendered image sequence's first file brings the sequence.
        let footage = self.probe_footage(path)?;
        if item.post_render == PostRenderAction::SetProxy {
            crate::commands::proxy_set(self, item.comp, footage).map_err(|e| e.to_string())?;
            self.toast(format!("Proxy set from {path}"));
            return Ok(None);
        }
        let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
        let replace = item.post_render == PostRenderAction::ImportAndReplace;
        let comp = item.comp;
        let label = if replace { "Pre-render: Import & Replace Usage" } else { "Post-Render: Import" };
        self.edit(label, None, |proj, st| {
            let fid = proj.add_item(&name, effectcraft_color::Label::Aqua, None, ItemKind::Footage(footage));
            if replace {
                let ids: Vec<ItemId> = proj.comps().map(|(i, _)| *i).filter(|i| *i != comp).collect();
                for cid in ids {
                    let uses = proj.comp(cid).is_some_and(|c| c.layers.iter().any(|l| matches!(l.source, LayerSource::Comp { item } if item == comp)));
                    if !uses {
                        continue;
                    }
                    if let Some(c) = proj.comp_mut(cid) {
                        for l in c.layers.iter_mut().filter(|l| matches!(l.source, LayerSource::Comp { item } if item == comp)) {
                            l.source = LayerSource::Footage { item: fid };
                        }
                    }
                }
            }
            st.project_selection = vec![fid];
            Ok(Some(fid))
        })
        .map_err(|e| e.to_string())
    }

    /// The comp to add: `comp` param, the active comp, or the first comp selected in the Project panel.
    pub(crate) fn comp_for_queue(&self, p: &serde_json::Value) -> crate::Result<ItemId> {
        if p.get("comp").is_some() {
            return crate::commands::comp_id(self, p);
        }
        let sel = self.state.project_selection.iter().copied().find(|i| self.project.comp(*i).is_some());
        self.active_comp_id().or(sel).ok_or(crate::EngineError::NoComp)
    }
}
