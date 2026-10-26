//! Background jobs where the platform has no threads (the browser).
//!
//! On the desktop, renders and analyses run on threads that share the session's project and
//! footage. A wasm build without atomics has one thread per instance, so the web app runs a
//! second engine instance in a Web Worker instead and talks to it with messages: an
//! [`Offload`] (set as [`Session::offload`]) ships a [`WorkerRequest`] — the serialized project,
//! the footage files it reads and the job — and feeds the worker's [`WorkerReply`]s back into an
//! [`Inbox`] that the session drains every frame ([`Session::poll_render`],
//! [`Session::poll_offload`]). The worker side is [`run_request_async`]: a plain session fed the
//! project runs the job and reports through a callback. With a WebGPU device of the worker's
//! own (deferred readbacks, `effectcraft_render::passes`) the job's frames render on the GPU in
//! passes, the job awaiting the device between them; [`run_request`] runs it blocking.
//!
//! | Job | Worker runs | Reply applied on the UI thread |
//! |---|---|---|
//! | Render Queue | the export of every queued item | item status, progress; files arrive through the host |
//! | Warp Stabilizer analysis | [`Session::start_warp`], then the stabilization plan | the effect's property group (one undo step); the plan's summary ([`WorkerReply::WarpPlan`]) |
//! | Warp Stabilizer stabilization (settings changed after the analysis) | the plan ([`effectcraft_effects::warp_stab::summary_at`]) | its summary, for `warp.status` |
//! | Track Motion / Stabilize Motion | the tracker analysis | the tracker group |
//! | Mask tracking | the mask track | the mask group |
//! | Roto Brush Freeze | the freeze | the effect group |
//! | Roto Brush propagation | the propagation | segmentations ([`WorkerReply::Segs`], streamed while it runs) go into the page's segmentation cache |
//! | 3D Camera Tracker analysis | [`Session::start_camera`] | the effect's property group |
//! | Content-Aware Fill | the fill ([`crate::commands::content_fill::FillPlan`]) | its PNG files (to the host's storage), then the fill layer (one undo step) |
//!
//! Viewer frames go to a separate, long-lived frame worker fed with project diffs
//! ([`crate::remote`]).
//!
//! Cancelling terminates the worker: what an analysis tracked so far is dropped and the Render
//! Queue item being rendered becomes "User Stopped".

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_keyframe::ShapePath;
use effectcraft_project::render_queue::{RenderQueueItem, RenderStatus};
use effectcraft_project::tracking::TrackerSettings;
use effectcraft_project::{ItemId, ItemKind, LayerId, Project, PropGroup, Uid};
use effectcraft_render::{Accelerator, Backend, RenderOpts};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

use crate::mask_track::MaskMethod;
use crate::render_queue::{ItemUpdate, JobShared, JobState};
use crate::tracking::PointValues;
use crate::{Event, Session};

/// One job for a worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRequest {
    pub id: u64,
    /// The project ([`Project::to_json`]).
    pub project: String,
    /// Footage files the job reads; the host ships their bytes before the request.
    pub files: Vec<String>,
    pub job: WorkerJob,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WorkerJob {
    /// Render Queue: per item, one (item with that output module, resolved output path) per
    /// output module.
    Render {
        items: Vec<Vec<(RenderQueueItem, String)>>,
    },
    Warp {
        comp: ItemId,
        layer: LayerId,
        effect: Uid,
    },
    /// The stabilization plan of an analysed Warp Stabilizer at comp time `time` (its settings
    /// there): seconds of solving for a long Subspace Warp clip, so not on the page. The
    /// summary comes back as [`WorkerReply::WarpPlan`].
    WarpPlan {
        comp: ItemId,
        layer: LayerId,
        effect: Uid,
        time: Tick,
    },
    Camera {
        comp: ItemId,
        layer: LayerId,
        effect: Uid,
    },
    RotoFreeze {
        comp: ItemId,
        layer: LayerId,
        effect: Uid,
    },
    /// Roto Brush propagation: the segmentations come back as [`WorkerReply::Segs`].
    RotoPropagate {
        comp: ItemId,
        layer: LayerId,
        effect: Uid,
        direction: crate::roto::Direction,
    },
    Track {
        comp: ItemId,
        layer: LayerId,
        tracker: Uid,
        direction: crate::tracking::Direction,
        settings: TrackerSettings,
        points: Vec<PointValues>,
        times: Vec<Tick>,
    },
    MaskTrack {
        comp: ItemId,
        layer: LayerId,
        mask: Uid,
        direction: crate::tracking::Direction,
        path: ShapePath,
        method: MaskMethod,
        times: Vec<Tick>,
    },
    /// Content-Aware Fill: the files go to the host's storage, the plan and their paths come
    /// back as [`WorkerReply::Fill`].
    ContentFill {
        plan: Box<crate::commands::content_fill::FillPlan>,
    },
}

impl WorkerJob {
    pub fn kind(&self) -> JobKind {
        match self {
            WorkerJob::Render { .. } => JobKind::Render,
            WorkerJob::Warp { .. } => JobKind::Warp,
            WorkerJob::WarpPlan { .. } => JobKind::WarpPlan,
            WorkerJob::Camera { .. } => JobKind::Camera,
            WorkerJob::RotoFreeze { .. } => JobKind::RotoFreeze,
            WorkerJob::RotoPropagate { .. } => JobKind::RotoPropagate,
            WorkerJob::Track { .. } => JobKind::Track,
            WorkerJob::MaskTrack { .. } => JobKind::MaskTrack,
            WorkerJob::ContentFill { .. } => JobKind::ContentFill,
        }
    }

    /// (comp, layer, property group uid) an analysis writes to.
    pub fn target(&self) -> Option<(ItemId, LayerId, Uid)> {
        match *self {
            WorkerJob::Render { .. } => None,
            WorkerJob::Warp { comp, layer, effect }
            | WorkerJob::WarpPlan { comp, layer, effect, .. }
            | WorkerJob::Camera { comp, layer, effect }
            | WorkerJob::RotoFreeze { comp, layer, effect }
            | WorkerJob::RotoPropagate { comp, layer, effect, .. } => Some((comp, layer, effect)),
            WorkerJob::Track { comp, layer, tracker, .. } => Some((comp, layer, tracker)),
            WorkerJob::MaskTrack { comp, layer, mask, .. } => Some((comp, layer, mask)),
            WorkerJob::ContentFill { ref plan } => Some((plan.comp, plan.layer, 0)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Render,
    Warp,
    WarpPlan,
    Camera,
    RotoFreeze,
    RotoPropagate,
    Track,
    MaskTrack,
    ContentFill,
}

impl JobKind {
    /// The corresponding `jobs.list` id, also used by waiting control commands.
    pub fn job_id(self) -> &'static str {
        match self {
            JobKind::Render => "render",
            JobKind::Warp => "warp",
            JobKind::WarpPlan => "warpPlan",
            JobKind::Camera => "camera",
            JobKind::RotoFreeze | JobKind::RotoPropagate => "roto",
            JobKind::Track => "track",
            JobKind::MaskTrack => "maskTrack",
            JobKind::ContentFill => "contentFill",
        }
    }

    /// The undo step an analysis result becomes.
    pub fn label(self) -> &'static str {
        match self {
            JobKind::Render => "Render",
            JobKind::Warp => "Warp Stabilizer Analysis",
            JobKind::WarpPlan => "Warp Stabilizer: Stabilizing",
            JobKind::Camera => "3D Camera Tracker Analysis",
            JobKind::RotoFreeze => "Freeze",
            JobKind::RotoPropagate => "Roto Brush Propagation",
            JobKind::Track => "Analyze Track",
            JobKind::MaskTrack => "Track Mask",
            JobKind::ContentFill => "Content-Aware Fill",
        }
    }
}

/// What a worker reports.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkerReply {
    /// Render Queue progress.
    Render {
        state: JobState,
    },
    /// A Render Queue item started or finished.
    Item {
        update: ItemUpdate,
    },
    /// Analysis progress (frames).
    Progress {
        done: u64,
        total: u64,
    },
    /// An analysis finished: its property group after the job, and the job's message.
    Group {
        comp: ItemId,
        layer: LayerId,
        group: Box<PropGroup>,
        message: String,
    },
    /// A Warp Stabilizer plan's summary ([`effectcraft_effects::warp_stab::summary_key`]): the
    /// page keeps it for `warp.status` instead of solving the plan itself.
    WarpPlan {
        key: [u64; 2],
        summary: Box<effectcraft_effects::warp_stab::PlanSummary>,
    },
    /// Roto Brush segmentations computed so far (propagation), not sent before.
    Segs {
        segs: Vec<SegData>,
    },
    /// A Content-Aware Fill finished: its plan and the PNG files written (already in the
    /// host's storage).
    Fill {
        plan: Box<crate::commands::content_fill::FillPlan>,
        files: Vec<String>,
    },
    Failed {
        error: String,
    },
    /// The job ended (always the last reply).
    Done,
}

/// One full-resolution Roto Brush segmentation: its chain key and the base64 of
/// [`effectcraft_track::roto::FrameSeg::to_bytes`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegData {
    pub key: u64,
    pub data: String,
}

impl SegData {
    pub fn new(key: u64, seg: &effectcraft_track::roto::FrameSeg) -> SegData {
        SegData { key, data: effectcraft_track::roto::rle::base64_encode(&seg.to_bytes()) }
    }

    /// Put the segmentation into this instance's cache (what the Roto Brush effect renders from).
    pub fn store(&self) -> bool {
        let Some(seg) = effectcraft_track::roto::rle::base64_decode(&self.data).and_then(|b| effectcraft_track::roto::FrameSeg::from_bytes(&b)) else {
            return false;
        };
        effectcraft_effects::roto::store(self.key, 1.0, Arc::new(seg));
        true
    }
}

/// The full-resolution segmentations of `keys` that are cached here and not in `sent` yet.
pub fn segs_to_send(keys: &[u64], sent: &mut std::collections::HashSet<u64>) -> Vec<SegData> {
    let new: Vec<(u64, Arc<effectcraft_track::roto::FrameSeg>)> =
        keys.iter().filter(|k| !sent.contains(k)).filter_map(|k| effectcraft_effects::roto::cached(*k, 1.0).map(|s| (*k, s))).collect();
    new.into_iter()
        .map(|(k, s)| {
            sent.insert(k);
            SegData::new(k, &s)
        })
        .collect()
}

/// Replies waiting for the UI thread.
#[derive(Default)]
pub struct Inbox(Mutex<Vec<WorkerReply>>);

impl Inbox {
    pub fn push(&self, r: WorkerReply) {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(r);
    }
    pub fn take(&self) -> Vec<WorkerReply> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

/// Runs jobs off the UI thread (the web app: a pool of Web Workers).
pub trait Offload: Send + Sync {
    /// Start `req`; replies go to `inbox` (ending with [`WorkerReply::Done`]).
    fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String>;
    /// Stop job `id` now (the worker is terminated; no more replies arrive).
    fn cancel(&self, id: u64);
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// Footage files a project reads (paths, sequence frames; missing items read none).
pub fn footage_files(p: &Project) -> Vec<String> {
    let mut v: Vec<String> = p
        .items
        .values()
        .filter_map(|i| match &i.kind {
            ItemKind::Footage(f) if !f.path.is_empty() && !f.missing => {
                Some(std::iter::once(f.path.clone()).chain(f.sequence.iter().cloned()).collect::<Vec<_>>())
            }
            _ => None,
        })
        .flatten()
        .collect();
    v.sort();
    v.dedup();
    v
}

// ---------------------------------------------------------------- progress hook (worker side)

thread_local! {
    static HOOK: RefCell<Option<Box<dyn FnMut(u64, u64)>>> = const { RefCell::new(None) };
}

/// Analyses call this as frames complete; a worker forwards it (no-op elsewhere).
pub fn report(done: u64, total: u64) {
    HOOK.with(|h| {
        if let Ok(mut h) = h.try_borrow_mut()
            && let Some(f) = h.as_mut()
        {
            f(done, total);
        }
    });
}

/// Run `f` (a future: an analysis awaited to its end) with `hook` receiving its progress.
async fn with_hook<R>(hook: Box<dyn FnMut(u64, u64)>, f: impl Future<Output = R>) -> R {
    HOOK.with(|h| *h.borrow_mut() = Some(hook));
    let r = f.await;
    HOOK.with(|h| *h.borrow_mut() = None);
    r
}

// ---------------------------------------------------------------- GPU in job workers

/// A boxed analysis future (see [`run_or_queue`]).
type Queued = Pin<Box<dyn Future<Output = ()>>>;

thread_local! {
    /// The job worker's accelerator while it runs a request: a WebGPU device with deferred
    /// readbacks (see [`job_accel`]).
    static JOB_ACCEL: RefCell<Option<Arc<dyn Accelerator>>> = const { RefCell::new(None) };
    /// Analyses started while [`run_request_async`] runs a request on this thread: awaited by it
    /// (`None`: not inside one, analyses run to completion when started).
    static QUEUED: RefCell<Option<Vec<Queued>>> = const { RefCell::new(None) };
}

/// The accelerator analyses render their input frames with: the job worker's GPU while it runs
/// a request ([`run_request_async`] with a session whose accelerator defers its readbacks).
/// Elsewhere (the desktop's analysis threads) analyses render on the CPU, as before.
pub(crate) fn job_accel() -> Option<Arc<dyn Accelerator>> {
    JOB_ACCEL.with(|a| a.borrow().clone())
}

/// Render options of an analysis's input frames: 100 %; with an accelerator, the project's
/// renderer (Mercury GPU Acceleration: its GPU effects on the device; Software Only: the CPU).
pub(crate) fn analysis_opts(accel: Option<&dyn Accelerator>) -> RenderOpts {
    RenderOpts { scale: 1.0, backend: if accel.is_some() { Backend::Auto } else { Backend::Cpu }, ..Default::default() }
}

/// A render in passes ([`effectcraft_render::passes::in_passes`]): its value.
pub(crate) async fn in_passes<'a, T>(accel: Option<&'a dyn Accelerator>, render: impl FnMut(Option<&'a dyn Accelerator>) -> T) -> T {
    effectcraft_render::passes::in_passes(accel, render).await.value
}

/// `work()` while the next frame renders (`fetch`): side by side on the thread pool, or — with
/// `deferred` readbacks, where a frame renders in passes awaiting the GPU — one after the other.
pub(crate) async fn join_fetch<R: Send, F: Send, Fut: Future<Output = F>>(
    deferred: bool,
    work: impl FnOnce() -> R + Send,
    fetch: impl FnOnce() -> Fut + Send,
) -> (R, F) {
    if deferred {
        let r = work();
        return (r, fetch().await);
    }
    rayon::join(work, || effectcraft_render::passes::block_on(fetch()))
}

/// Run an analysis started with `wait`: inside [`run_request_async`] it is queued and awaited
/// there (its frames may render in passes); anywhere else it runs to completion now.
pub(crate) fn run_or_queue(f: impl Future<Output = ()> + 'static) {
    let f: Queued = Box::pin(f);
    let now = QUEUED.with(|q| match q.borrow_mut().as_mut() {
        Some(v) => {
            v.push(f);
            None
        }
        None => Some(f),
    });
    if let Some(f) = now {
        effectcraft_render::passes::block_on(f);
    }
}

/// Await the analyses [`run_or_queue`] queued, then let the session apply their results.
async fn drain(s: &mut Session) {
    loop {
        let next = QUEUED.with(|q| q.borrow_mut().as_mut().and_then(|v| (!v.is_empty()).then(|| v.remove(0))));
        match next {
            Some(f) => f.await,
            None => break,
        }
    }
    s.poll_track();
    s.poll_mask_track();
    s.poll_warp(false);
    s.poll_camera(false);
    s.poll_roto(false);
}

// ---------------------------------------------------------------- worker side

/// Where a worker sends its replies.
pub type Post = Rc<dyn Fn(WorkerReply)>;

/// Run `req` on `s` (a session with footage, expressions and an exporter, but no project yet),
/// blocking, reporting through `post`. Always ends with [`WorkerReply::Done`].
pub fn run_request(s: &mut Session, req: WorkerRequest, post: &Post) {
    effectcraft_render::passes::block_on(run_request_async(s, req, post));
}

/// [`run_request`] as a future: with an accelerator whose readbacks are deferred
/// ([`Session::accel`]: a browser job worker's own WebGPU device) the Render Queue's frames
/// and the analyses' input frames render on it in passes, awaiting the device between them
/// (the browser delivers readbacks only while the worker is back in its event loop).
pub async fn run_request_async(s: &mut Session, req: WorkerRequest, post: &Post) {
    let accel = s.accel.clone().filter(|a| a.miss_gate().is_some());
    if let Some(a) = &accel {
        // Buffers computed from placeholders of a pass that missed are not kept.
        s.layer_cache.set_gate(a.miss_gate());
    }
    JOB_ACCEL.with(|a| *a.borrow_mut() = accel);
    QUEUED.with(|q| *q.borrow_mut() = Some(vec![]));
    match Project::from_json(&req.project) {
        Ok(p) => {
            s.replace_project(p, None);
            if let Err(e) = run_job(s, req.job, post.clone()).await {
                post(WorkerReply::Failed { error: e });
            }
        }
        Err(e) => post(WorkerReply::Failed { error: format!("project: {e}") }),
    }
    QUEUED.with(|q| *q.borrow_mut() = None);
    JOB_ACCEL.with(|a| *a.borrow_mut() = None);
    post(WorkerReply::Done);
}

async fn run_job(s: &mut Session, job: WorkerJob, post: Post) -> Result<(), String> {
    let kind = job.kind();
    let target = job.target();
    match job {
        WorkerJob::Render { items } => {
            let exporter = s.exporter.clone().ok_or("export is not available in this build")?;
            let shared = JobShared::default();
            // Item updates and progress go out as they happen (progress at most every 100 ms).
            let last = Cell::new(None::<web_time::Instant>);
            let notify = |sh: &JobShared, force: bool| {
                let ups = std::mem::take(&mut *crate::render_queue::lock(&sh.updates));
                for u in ups {
                    post(WorkerReply::Item { update: u });
                }
                if force || last.get().is_none_or(|t| t.elapsed().as_millis() >= 100) {
                    last.set(Some(web_time::Instant::now()));
                    post(WorkerReply::Render { state: sh.snapshot() });
                }
            };
            crate::render_queue::run_items(s, exporter, items, &shared, &mut |sh| notify(sh, false)).await;
            notify(&shared, true);
            Ok(())
        }
        WorkerJob::Warp { comp, layer, effect } => analysis(s, kind, target, post, |s| s.start_warp(comp, layer, effect, true).map(|_| ())).await,
        WorkerJob::WarpPlan { comp, layer, effect, time } => {
            let r = warp_plan(s, comp, layer, effect, time).ok_or("the Warp Stabilizer is not analysed")?;
            post(r);
            Ok(())
        }
        WorkerJob::Camera { comp, layer, effect } => analysis(s, kind, target, post, |s| s.start_camera(comp, layer, effect, true).map(|_| ())).await,
        WorkerJob::RotoFreeze { comp, layer, effect } => {
            analysis(s, kind, target, post, |s| {
                s.start_roto(comp, layer, effect, crate::roto::RotoTask::Freeze, crate::roto::Direction::Both, true).map(|_| ())
            })
            .await
        }
        WorkerJob::RotoPropagate { comp, layer, effect, direction } => propagate(s, comp, layer, effect, direction, post).await,
        WorkerJob::ContentFill { plan } => fill(s, *plan, post).await,
        WorkerJob::Track { comp, layer, tracker, direction, settings, points, times } => {
            analysis(s, kind, target, post, |s| {
                let work = crate::tracking::Work {
                    project: s.project.clone(),
                    footage: s.footage.clone(),
                    expr: s.expr.clone(),
                    cache: s.layer_cache.clone(),
                    comp,
                    layer,
                    settings,
                    points,
                    times,
                };
                s.start_track(work, tracker, direction, true)
            })
            .await
        }
        WorkerJob::MaskTrack { comp, layer, mask, direction, path, method, times } => {
            analysis(s, kind, target, post, |s| {
                let work = crate::mask_track::MaskWork {
                    project: s.project.clone(),
                    footage: s.footage.clone(),
                    expr: s.expr.clone(),
                    cache: s.layer_cache.clone(),
                    comp,
                    layer,
                    path,
                    method,
                    times,
                    face_model: if method.is_face() { s.models.face() } else { None },
                };
                s.start_mask_track(work, mask, direction, true)
            })
            .await
        }
    }
}

/// Content-Aware Fill in a worker: the layer's frames render on the worker's GPU (in passes),
/// the files are written through the session's services (the browser: back to the page's
/// storage) and the plan comes back for the page to add the fill layer.
async fn fill(s: &mut Session, plan: crate::commands::content_fill::FillPlan, post: Post) -> Result<(), String> {
    let p = post.clone();
    let mut last = None::<web_time::Instant>;
    let mut progress = move |done: u64, total: u64| {
        if last.is_none_or(|t: web_time::Instant| t.elapsed().as_millis() >= 100) {
            last = Some(web_time::Instant::now());
            p(WorkerReply::Progress { done, total });
        }
        true
    };
    let accel = job_accel();
    let files = crate::commands::content_fill::run_plan(
        &plan,
        &s.project,
        s.footage.as_ref(),
        s.expr.as_deref(),
        &s.layer_cache,
        s.services.as_ref(),
        accel.as_deref(),
        &mut progress,
        &mut |_| {},
    )
    .await?;
    post(WorkerReply::Fill { plan: Box::new(plan), files });
    Ok(())
}

/// Roto Brush propagation in a worker: the segmentations stream back (at most every 250 ms)
/// as they are computed, so stopping the job keeps what was propagated.
async fn propagate(s: &mut Session, comp: ItemId, layer: LayerId, effect: Uid, dir: crate::roto::Direction, post: Post) -> Result<(), String> {
    let keys: Vec<u64> =
        s.roto_keys(comp, layer, effect).map(|k| k.into_iter().map(|(_, k)| k).collect()).ok_or("no Roto Brush strokes or Object Matte prompt")?;
    let sent = Rc::new(RefCell::new(std::collections::HashSet::new()));
    let (p, k2, s2) = (post.clone(), keys.clone(), sent.clone());
    let mut last = None::<web_time::Instant>;
    let hook = Box::new(move |done: u64, total: u64| {
        if last.is_none_or(|t: web_time::Instant| t.elapsed().as_millis() >= 250) {
            last = Some(web_time::Instant::now());
            let segs = segs_to_send(&k2, &mut s2.borrow_mut());
            if !segs.is_empty() {
                p(WorkerReply::Segs { segs });
            }
            p(WorkerReply::Progress { done, total });
        }
    });
    s.events.clear();
    with_hook(hook, async {
        let r = s.start_roto(comp, layer, effect, crate::roto::RotoTask::Propagate, dir, true);
        drain(s).await;
        r
    })
    .await?;
    let segs = segs_to_send(&keys, &mut sent.borrow_mut());
    if !segs.is_empty() {
        post(WorkerReply::Segs { segs });
    }
    if let Some(e) = s.events.iter().find_map(|e| match e {
        Event::Toast { message, error: true } => Some(message.clone()),
        _ => None,
    }) {
        return Err(e);
    }
    Ok(())
}

async fn analysis(
    s: &mut Session,
    kind: JobKind,
    target: Option<(ItemId, LayerId, Uid)>,
    post: Post,
    run: impl FnOnce(&mut Session) -> Result<(), String>,
) -> Result<(), String> {
    let (comp, layer, uid) = target.ok_or("no target")?;
    let rev = s.revision;
    // Progress at most every 100 ms.
    let p = post.clone();
    let mut last = None::<web_time::Instant>;
    let hook = Box::new(move |done: u64, total: u64| {
        if last.is_none_or(|t: web_time::Instant| t.elapsed().as_millis() >= 100) {
            last = Some(web_time::Instant::now());
            p(WorkerReply::Progress { done, total });
        }
    });
    with_hook(hook, async {
        let r = run(s);
        // The analysis was queued: await it, then the session writes its result.
        drain(s).await;
        r
    })
    .await?;
    let message = s
        .events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Toast { message, .. } => Some(message.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let failed = s.events.iter().any(|e| matches!(e, Event::Toast { error: true, .. }));
    if failed {
        return Err(message);
    }
    if s.revision == rev && kind != JobKind::Track && kind != JobKind::MaskTrack {
        // Nothing written (e.g. the analysis was empty).
        return Err(if message.is_empty() { "nothing was analysed".into() } else { message });
    }
    let group = s.project.comp(comp).and_then(|c| c.layer(layer)).and_then(|l| l.props.find_group(uid)).cloned().ok_or("the analysed property is gone")?;
    // The stabilization plan too (before the group, so the page has it once it shows the
    // analysis): solving it would otherwise stall the page at its first `warp.status`.
    if kind == JobKind::Warp
        && let Some(r) = warp_plan(s, comp, layer, uid, s.time_of(comp))
    {
        post(r);
    }
    post(WorkerReply::Group { comp, layer, group: Box::new(group), message });
    Ok(())
}

/// The Warp Stabilizer plan summary of (comp, layer, effect) with its settings at comp time `t`
/// (computing the plan), as a reply.
fn warp_plan(s: &Session, comp: ItemId, layer: LayerId, effect: Uid, t: Tick) -> Option<WorkerReply> {
    let c = s.project.comp(comp)?;
    let l = c.layer(layer)?;
    let g = l.props.find_group(effect)?;
    let ctx = effectcraft_render::EvalCtx::new(&s.project, comp, c, t);
    let params = effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(l, pr));
    let (key, summary) = effectcraft_effects::warp_stab::summary(&params)?;
    Some(WorkerReply::WarpPlan { key, summary: Box::new((*summary).clone()) })
}

// ---------------------------------------------------------------- UI side

/// Analysis progress of an offloaded job.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffloadProgress {
    pub done: u64,
    pub total: u64,
    pub elapsed: f64,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

/// Frames per second of `done` frames in `elapsed` seconds.
pub(crate) fn rate(done: u64, elapsed: f64) -> f64 {
    if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 }
}

impl OffloadProgress {
    pub(crate) fn track(&self) -> crate::tracking::TrackProgress {
        crate::tracking::TrackProgress { done: self.done, total: self.total, elapsed: self.elapsed, fps: rate(self.done, self.elapsed), ..Default::default() }
    }
}

/// An analysis running in a worker.
pub struct OffloadedJob {
    pub id: u64,
    pub kind: JobKind,
    pub comp: ItemId,
    pub layer: LayerId,
    pub uid: Uid,
    pub inbox: Arc<Inbox>,
    pub progress: OffloadProgress,
    /// Segmentations received (Roto Brush propagation).
    pub segs: u64,
    started: web_time::Instant,
    /// The analysed group as the job started: its result is merged with what changed since
    /// ([`merge_group`]).
    base: Option<Box<PropGroup>>,
}

/// A Render Queue job running in a worker (see [`crate::render_queue::RenderJob`]).
pub struct RemoteRender {
    pub id: u64,
    pub inbox: Arc<Inbox>,
    /// The queue items in the job.
    pub ids: Vec<u64>,
}

impl Session {
    /// Whether jobs started without `wait` go to the [`Offload`].
    pub fn offloads(&self) -> bool {
        self.offload.is_some()
    }

    /// The offloaded analysis of `kind`, if one runs.
    pub fn offloaded(&self, kind: JobKind) -> Option<&OffloadedJob> {
        self.offloaded.iter().find(|j| j.kind == kind && !j.progress.finished)
    }

    /// Send an analysis to the worker.
    pub(crate) fn offload_analysis(&mut self, job: WorkerJob) -> Result<(), String> {
        let off = self.offload.clone().ok_or("no background worker")?;
        let kind = job.kind();
        let (comp, layer, uid) = job.target().ok_or("not an analysis")?;
        if self.offloaded(kind).is_some() {
            return Err(format!("a {} job is already running", kind.label()));
        }
        let id = next_id();
        let inbox = Arc::new(Inbox::default());
        let base = self.project.comp(comp).and_then(|c| c.layer(layer)).and_then(|l| l.props.find_group(uid)).cloned().map(Box::new);
        let req = WorkerRequest { id, project: self.project.to_json(), files: footage_files(&self.project), job };
        off.start(req, inbox.clone())?;
        self.offloaded.push(OffloadedJob {
            id,
            kind,
            comp,
            layer,
            uid,
            inbox,
            progress: OffloadProgress::default(),
            segs: 0,
            started: web_time::Instant::now(),
            base,
        });
        self.events.push(Event::ProjectChanged { revision: self.revision });
        Ok(())
    }

    /// Cancel the offloaded analysis of `kind` (nothing is written).
    pub fn cancel_offloaded(&mut self, kind: JobKind) -> bool {
        let off = self.offload.clone();
        let Some(j) = self.offloaded.iter_mut().find(|j| j.kind == kind && !j.progress.finished) else { return false };
        if let Some(off) = off {
            off.cancel(j.id);
        }
        j.progress.cancelled = true;
        j.progress.finished = true;
        true
    }

    /// Apply worker replies to offloaded analyses (results become one undo step each) and drop
    /// finished jobs. Frontends call this every frame while [`Session::offloaded`] is not empty.
    pub fn poll_offload(&mut self) -> bool {
        if self.offloaded.is_empty() {
            return false;
        }
        let mut changed = false;
        let mut results = vec![];
        let mut fills = vec![];
        for j in &mut self.offloaded {
            if j.progress.cancelled {
                continue;
            }
            for r in j.inbox.take() {
                changed = true;
                match r {
                    WorkerReply::Progress { done, total } => {
                        j.progress.done = done;
                        j.progress.total = total;
                    }
                    WorkerReply::Group { comp, layer, group, message } => results.push((j.id, j.kind, comp, layer, group, message, j.base.clone())),
                    WorkerReply::Fill { plan, files } => fills.push((plan, files)),
                    WorkerReply::WarpPlan { key, summary } => effectcraft_effects::warp_stab::store_summary(key, *summary),
                    WorkerReply::Segs { segs } => {
                        for sd in &segs {
                            sd.store();
                        }
                        j.segs += segs.len() as u64;
                    }
                    WorkerReply::Failed { error } => j.progress.error = Some(error),
                    WorkerReply::Done => j.progress.finished = true,
                    WorkerReply::Render { .. } | WorkerReply::Item { .. } => {}
                }
            }
            j.progress.elapsed = j.started.elapsed().as_secs_f64();
        }
        for (plan, files) in fills {
            let n = files.len();
            match crate::commands::content_fill::apply_plan(self, &plan, files) {
                Ok(_) => self.events.push(Event::Toast { message: format!("Content-Aware Fill: {n} frame(s) filled"), error: false }),
                Err(e) => self.events.push(Event::Toast { message: format!("Content-Aware Fill: {e}"), error: true }),
            }
        }
        for (id, kind, comp, layer, group, message, base) in results {
            let uid = group.uid;
            let r = self.edit(kind.label(), None, |p, _| {
                let g = p
                    .comp_mut(comp)
                    .and_then(|c| c.layer_mut(layer))
                    .and_then(|l| l.props.find_group_mut(uid))
                    .ok_or_else(|| crate::EngineError::Other("the analysed property was removed during the analysis".into()))?;
                // Edits made while the job ran stay (#1249): only what the analysis changed is
                // taken from its result.
                *g = match base {
                    Some(base) => merge_group(&base, g, &group).ok_or_else(|| {
                        crate::EngineError::Other(format!(
                            "“{}” was changed during the analysis where its result changes it too, so nothing was applied: analyse again",
                            g.name
                        ))
                    })?,
                    None => *group,
                };
                Ok(())
            });
            match r {
                Ok(()) => self.events.push(Event::Toast { message, error: false }),
                Err(e) => {
                    // Completion consumers read the job's error, not its toasts. Keep an apply
                    // rejection on the originating job; Done logs and reports it just like a
                    // WorkerReply::Failed, without failing another concurrent analysis.
                    if let Some(job) = self.offloaded.iter_mut().find(|j| j.id == id) {
                        job.progress.error = Some(e.to_string());
                    } else {
                        self.events.push(Event::Toast { message: format!("{}: {e}", kind.label()), error: true });
                    }
                }
            }
        }
        let mut done = vec![];
        self.offloaded.retain(|j| {
            if j.progress.finished {
                done.push((j.kind, j.progress.clone(), j.segs));
            }
            !j.progress.finished
        });
        for (kind, p, segs) in done {
            changed = true;
            self.offload_log.push((kind, p.error.clone().or_else(|| p.cancelled.then(|| "stopped".to_string()))));
            if self.offload_log.len() > 32 {
                self.offload_log.remove(0);
                self.offload_log_start = self.offload_log_start.saturating_add(1);
            }
            if let Some(e) = p.error {
                self.events.push(Event::Toast { message: format!("{}: {e}", kind.label()), error: true });
            } else if p.cancelled {
                let kept = if segs > 0 { format!(" ({segs} frame(s) kept)") } else { String::new() };
                self.events.push(Event::Toast { message: format!("{} stopped{kept}", kind.label()), error: false });
            } else if kind == JobKind::RotoPropagate {
                self.events.push(Event::Toast { message: format!("Roto Brush: propagated {} frame(s) in {:.1} s", segs, p.elapsed), error: false });
            }
            // Rendered frames depend on the result (an analysis' group, the segmentation cache);
            // not on a plan summary (`warp.status` only).
            if kind != JobKind::WarpPlan {
                self.bump();
            }
        }
        changed
    }

    /// Feed a remote render's replies into its [`JobShared`]; a cancelled one is wrapped up here
    /// (the item being rendered and the ones after it become "User Stopped").
    pub(crate) fn drain_remote_render(&mut self) {
        let Some(job) = &self.render_job else { return };
        let Some(remote) = &job.remote else { return };
        let shared = job.shared.clone();
        for r in remote.inbox.take() {
            match r {
                WorkerReply::Render { state } => {
                    let fin = crate::render_queue::lock(&shared.state).finished;
                    *crate::render_queue::lock(&shared.state) = JobState { finished: fin, ..state };
                }
                WorkerReply::Item { update } => crate::render_queue::lock(&shared.updates).push(update),
                WorkerReply::Failed { error } => {
                    log::warn!("render worker: {error}");
                    let cur = crate::render_queue::lock(&shared.state).current;
                    if let Some(id) = cur {
                        crate::render_queue::lock(&shared.updates).push(ItemUpdate::Finished {
                            id,
                            status: RenderStatus::Failed(error),
                            seconds: 0.0,
                            output: None,
                        });
                    }
                }
                WorkerReply::Done => crate::render_queue::lock(&shared.state).finished = true,
                _ => {}
            }
        }
    }
}

/// `live` if only it changed since `base`, `result` if only that did (or both the same way);
/// `None` when both changed it differently.
fn pick<T: PartialEq + Clone>(base: &T, live: &T, result: &T) -> Option<T> {
    if live == base {
        Some(result.clone())
    } else if result == base || result == live {
        Some(live.clone())
    } else {
        None
    }
}

fn node_uid(n: &effectcraft_project::Node) -> Uid {
    match n {
        effectcraft_project::Node::Prop(p) => p.uid,
        effectcraft_project::Node::Group(g) => g.uid,
    }
}

/// Merge an analysis `result` into the `live` group (#1249). `base` is the group as the job
/// started. The group's own fields and each property are taken from whichever side changed
/// them, groups are merged child by child (matched by uid), and a child added or removed by one
/// side is added or removed. `None` when both sides changed the same field or property
/// differently, one side removed a child the other changed, or both reordered or added and
/// removed children differently: a conflict.
pub(crate) fn merge_group(base: &PropGroup, live: &PropGroup, result: &PropGroup) -> Option<PropGroup> {
    use effectcraft_project::Node;
    if live == base {
        return Some(result.clone());
    }
    if result == base || result == live {
        return Some(live.clone());
    }
    let header = |g: &PropGroup| PropGroup { children: vec![], ..g.clone() };
    let mut out = pick(&header(base), &header(live), &header(result))?;
    let ids = |g: &PropGroup| g.children.iter().map(node_uid).collect::<Vec<_>>();
    let (bi, li, ri) = (ids(base), ids(live), ids(result));
    // The children's order and set: from the side that changed them.
    let order = if li == bi {
        ri
    } else if ri == bi || ri == li {
        li
    } else {
        return None;
    };
    fn find(g: &PropGroup, u: Uid) -> Option<&effectcraft_project::Node> {
        g.children.iter().find(|n| node_uid(n) == u)
    }
    // A child one side removed must be unchanged on the other, or that side's edit would be lost.
    for u in bi.iter().filter(|u| !order.contains(u)) {
        let b = find(base, *u);
        if [find(live, *u), find(result, *u)].into_iter().flatten().any(|n| Some(n) != b) {
            return None;
        }
    }
    for u in order {
        let node = match (find(base, u), find(live, u), find(result, u)) {
            (Some(Node::Group(b)), Some(Node::Group(l)), Some(Node::Group(r))) => Node::Group(merge_group(b, l, r)?),
            (Some(b), Some(l), Some(r)) => pick(b, l, r)?,
            // Added by one side, or by both alike.
            (None, Some(l), None) => l.clone(),
            (None, None, Some(r)) => r.clone(),
            (None, Some(l), Some(r)) if l == r => l.clone(),
            _ => return None,
        };
        out.children.push(node);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> String {
        let a = serde_json::to_string(v).unwrap();
        let back: T = serde_json::from_str(&a).unwrap();
        let b = serde_json::to_string(&back).unwrap();
        assert_eq!(a, b);
        a
    }

    /// A group with properties `a` (uid 2) and `b` (uid 3) and a sub-group (uid 4) holding `c`
    /// (uid 5).
    fn group() -> PropGroup {
        use effectcraft_keyframe::Value as KV;
        use effectcraft_project::{Node, Property};
        let mut sub = PropGroup::new(4, "sub", "Sub");
        sub.children.push(Node::Prop(Property::new(5, "c", "C", KV::Scalar(0.0))));
        let mut g = PropGroup::new(1, "fx", "Effect");
        g.children.push(Node::Prop(Property::new(2, "a", "A", KV::Scalar(0.0))));
        g.children.push(Node::Prop(Property::new(3, "b", "B", KV::Scalar(0.0))));
        g.children.push(Node::Group(sub));
        g
    }

    fn set(g: &mut PropGroup, uid: Uid, v: f64) {
        g.find_mut(uid).unwrap().value = effectcraft_keyframe::Value::Scalar(v);
    }

    fn get(g: &PropGroup, uid: Uid) -> f64 {
        match g.find(uid).unwrap().value {
            effectcraft_keyframe::Value::Scalar(v) => v,
            ref v => panic!("{v:?}"),
        }
    }

    #[test]
    fn analysis_results_merge_with_edits_made_meanwhile() {
        use effectcraft_project::Node;
        let base = group();
        // The user changes `a` and switches the effect off; the analysis writes `b` and `c`.
        let mut live = base.clone();
        set(&mut live, 2, 1.0);
        live.enabled = false;
        let mut result = base.clone();
        set(&mut result, 3, 2.0);
        set(&mut result, 5, 3.0);
        let m = merge_group(&base, &live, &result).unwrap();
        assert_eq!((get(&m, 2), get(&m, 3), get(&m, 5), m.enabled), (1.0, 2.0, 3.0, false));
        // Unchanged on either side: the other side wins whole.
        assert_eq!(merge_group(&base, &base, &result).unwrap(), result);
        assert_eq!(merge_group(&base, &live, &base).unwrap(), live);
        // Both changed `b`, differently: a conflict (nothing is applied). The same way: fine.
        let mut both = live.clone();
        set(&mut both, 3, 9.0);
        assert!(merge_group(&base, &both, &result).is_none());
        set(&mut both, 3, 2.0);
        assert_eq!(get(&merge_group(&base, &both, &result).unwrap(), 3), 2.0);
        // The analysis adds a child (a result group) while the user edits `a`: both kept.
        let mut added = result.clone();
        added.children.push(Node::Group(PropGroup::new(6, "out", "Output")));
        let m = merge_group(&base, &live, &added).unwrap();
        assert!(m.find_group(6).is_some() && get(&m, 2) == 1.0);
        // The analysis drops the sub-group the user didn't touch: dropped.
        let mut dropped = result.clone();
        dropped.children.retain(|n| !matches!(n, Node::Group(g) if g.uid == 4));
        let m = merge_group(&base, &live, &dropped).unwrap();
        assert!(m.find_group(4).is_none() && get(&m, 2) == 1.0);
        // …but if the user changed something in it, that's a conflict.
        let mut touched = live.clone();
        set(&mut touched, 5, 7.0);
        assert!(merge_group(&base, &touched, &dropped).is_none());
    }

    #[test]
    fn protocol_round_trips() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", serde_json::json!({})).unwrap();
        let comp = s.active_comp_id().unwrap();
        s.execute("renderQueue.add", serde_json::json!({})).unwrap();
        let item = s.project.render_queue[0].clone();
        let reqs = [
            WorkerJob::Render { items: vec![vec![(item.clone(), "/out.gif".into())]] },
            WorkerJob::Warp { comp, layer: LayerId(3), effect: 9 },
            WorkerJob::WarpPlan { comp, layer: LayerId(3), effect: 9, time: Tick::from_seconds_f64(0.5) },
            WorkerJob::RotoFreeze { comp, layer: LayerId(3), effect: 9 },
            WorkerJob::Camera { comp, layer: LayerId(3), effect: 9 },
            WorkerJob::RotoPropagate { comp, layer: LayerId(3), effect: 9, direction: crate::roto::Direction::Forward },
            WorkerJob::Track {
                comp,
                layer: LayerId(1),
                tracker: 4,
                direction: crate::tracking::Direction::Backward,
                settings: TrackerSettings::default(),
                points: vec![PointValues {
                    uid: 5,
                    spec: effectcraft_track::PointSpec { center: [1.0, 2.0], feature_size: [3.0, 4.0], search_offset: [0.5, 0.0], search_size: [9.0, 9.0] },
                    attach_offset: [0.25, -1.0],
                }],
                times: vec![Tick::ZERO, Tick::from_seconds_f64(0.5)],
            },
            WorkerJob::MaskTrack {
                comp,
                layer: LayerId(2),
                mask: 7,
                direction: crate::tracking::Direction::Forward,
                path: ShapePath::ellipse([10.0, 20.0], 5.0, 6.0),
                method: MaskMethod::Perspective,
                times: vec![Tick::from_seconds_f64(1.0)],
            },
            WorkerJob::ContentFill {
                plan: Box::new(crate::commands::content_fill::FillPlan {
                    comp,
                    layer: LayerId(2),
                    name: "Clip".into(),
                    width: 4,
                    height: 3,
                    times: vec![Tick::ZERO],
                    method: "object".into(),
                    lighting: 0.5,
                    expand: 1,
                    reference: Some((LayerId(1), Tick::ZERO)),
                    dir: "/Fill/Clip_Fill_1".into(),
                }),
            },
        ];
        for job in reqs {
            let kind = job.kind();
            let req = WorkerRequest { id: 42, project: s.project.to_json(), files: footage_files(&s.project), job };
            let t = round(&req);
            assert!(t.contains("\"kind\":"), "{kind:?}");
            let back: WorkerRequest = serde_json::from_str(&t).unwrap();
            assert_eq!(back.job.kind(), kind);
            assert_eq!(Project::from_json(&back.project).unwrap(), *s.project);
        }
        let g = s.active_comp().unwrap().layers[0].props.clone();
        for r in [
            WorkerReply::Render { state: JobState { current: Some(3), done: 4, total: 10, items_total: 1, ..Default::default() } },
            WorkerReply::Item { update: ItemUpdate::Started { id: 3, unix: 1_700_000_000 } },
            WorkerReply::Item { update: ItemUpdate::Finished { id: 3, status: RenderStatus::Failed("x".into()), seconds: 1.5, output: Some("/a.gif".into()) } },
            WorkerReply::Progress { done: 1, total: 2 },
            WorkerReply::WarpPlan {
                key: [1, u64::MAX],
                summary: Box::new(effectcraft_effects::warp_stab::PlanSummary {
                    warps: vec![[[1.0, 0.0, 2.5], [0.0, 1.0, -1.0], [0.0, 0.0, 1.0]]],
                    auto_scale: 1.08,
                    crop: Some([1.0, 2.0, 300.0, 200.0]),
                    valid_fraction: 0.9,
                }),
            },
            WorkerReply::Group { comp, layer: LayerId(1), group: Box::new(g), message: "ok".into() },
            WorkerReply::Segs { segs: vec![SegData::new(5, &effectcraft_track::roto::FrameSeg::empty(4, 3))] },
            WorkerReply::Fill {
                plan: Box::new(crate::commands::content_fill::FillPlan {
                    comp,
                    layer: LayerId(2),
                    name: "Clip".into(),
                    width: 4,
                    height: 3,
                    times: vec![Tick::ZERO],
                    method: "object".into(),
                    lighting: 0.5,
                    expand: 1,
                    reference: Some((LayerId(1), Tick::ZERO)),
                    dir: "/Fill/Clip_Fill_1".into(),
                }),
                files: vec!["/Fill/Clip_Fill_1/fill_00000.png".into()],
            },
            WorkerReply::Failed { error: "boom".into() },
            WorkerReply::Done,
        ] {
            let t = round(&r);
            assert!(t.starts_with("{\"type\":"), "{t}");
        }
    }

    /// An [`Offload`] that runs the job right away in a second session, through JSON (what a
    /// Web Worker does, minus the threads).
    struct Inline;
    impl Offload for Inline {
        fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
            let req: WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
            let mut w = Session { exporter: Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() })), ..Default::default() };
            let post: Post = Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
            run_request(&mut w, req, &post);
            Ok(())
        }
        fn cancel(&self, _: u64) {}
    }

    #[test]
    fn render_runs_through_the_offload() {
        let mut s = Session {
            exporter: Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() })),
            offload: Some(Arc::new(Inline)),
            ..Default::default()
        };
        s.execute("file.openDemoProject", serde_json::json!({})).unwrap();
        s.execute("renderQueue.add", serde_json::json!({"output": "/x.gif"})).unwrap();
        let ids = s.start_render(false).unwrap();
        // (the inline offload has replied already; replies are applied when polled)
        s.poll_render();
        assert!(s.render_job.is_none());
        let it = s.project.render_queue.iter().find(|i| i.id == ids[0]).unwrap();
        assert_eq!(it.status, RenderStatus::Done, "{:?}", it.status);
        assert!(it.last_output.as_deref().is_some_and(|o| o.ends_with("x.gif")));
        // `wait` renders inline.
        s.execute("renderQueue.add", serde_json::json!({"output": "/y.gif"})).unwrap();
        s.start_render(true).unwrap();
        assert!(s.render_job.is_none() || s.render_job.as_ref().unwrap().remote.is_none());
    }

    /// Runs jobs in a session sharing `footage` (the synthetic clip of the tracking tests).
    struct InlineWith(Arc<dyn effectcraft_render::FootageSource>);
    impl Offload for InlineWith {
        fn start(&self, req: WorkerRequest, inbox: Arc<Inbox>) -> Result<(), String> {
            let req: WorkerRequest = serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
            let mut w = Session { footage: self.0.clone(), ..Default::default() };
            let post: Post = Rc::new(move |r| inbox.push(serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap()));
            run_request(&mut w, req, &post);
            Ok(())
        }
        fn cancel(&self, _: u64) {}
    }

    /// Never replies (a job still running); records cancellations.
    #[derive(Default)]
    struct Hanging(Mutex<Vec<u64>>);
    impl Offload for Hanging {
        fn start(&self, _: WorkerRequest, _: Arc<Inbox>) -> Result<(), String> {
            Ok(())
        }
        fn cancel(&self, id: u64) {
            self.0.lock().unwrap().push(id);
        }
    }

    fn pose(f: u32) -> ([f64; 2], f64, f64) {
        ([150.0 + 2.0 * f as f64, 110.0 + 1.0 * f as f64], 0.0, 1.0)
    }

    #[test]
    fn track_analysis_runs_through_the_offload_as_one_undo_step() {
        let (mut s, clip, _) = crate::tests_track::setup(crate::tests_track::patch_frames(pose));
        s.offload = Some(Arc::new(InlineWith(s.footage.clone())));
        s.execute("track.motion", serde_json::json!({})).unwrap();
        s.execute("track.setPoint", serde_json::json!({"point": 1, "center": pose(0).0, "featureSize": [36, 36], "searchSize": [80, 80]})).unwrap();
        let undo = s.history.undo.len();
        s.execute("track.analyze", serde_json::json!({"direction": "forward"})).unwrap();
        assert!(s.is_tracking() && s.track_job.is_none(), "offloaded, not on a thread");
        assert!(s.track_progress().is_some());
        assert!(s.poll_offload());
        assert!(!s.is_tracking() && s.offloaded.is_empty());
        let l = s.active_comp().unwrap().layer(clip).unwrap();
        let tp = l.trackers().next().unwrap().0.track_points().next().unwrap().clone();
        let fc = tp.get("featureCenter").unwrap();
        assert_eq!(fc.keys.len(), 25);
        let v = fc.value_at(crate::tests_track::frame_time(12)).as_vec2();
        assert!((v[0] - pose(12).0[0]).abs() < 0.3 && (v[1] - pose(12).0[1]).abs() < 0.3, "{v:?}");
        assert_eq!(s.history.undo.len(), undo + 1);
        assert_eq!(s.history.undo.last().unwrap().0, "Analyze Track");
        assert!(s.events.iter().any(|e| matches!(e, Event::Toast { message, error: false } if message.starts_with("Tracked"))));
        assert!(s.undo());
        let l = s.active_comp().unwrap().layer(clip).unwrap();
        assert!(l.trackers().next().unwrap().0.track_points().next().unwrap().get("featureCenter").unwrap().keys.is_empty());
        // `wait` runs inline.
        s.redo();
        s.execute("track.analyze", serde_json::json!({"direction": "forward", "wait": true})).ok();
        assert!(s.offloaded.is_empty());
    }

    #[test]
    fn cancelling_offloaded_jobs() {
        let (mut s, _, _) = crate::tests_track::setup(crate::tests_track::patch_frames(pose));
        let off = Arc::new(Hanging::default());
        s.offload = Some(off.clone());
        s.exporter = Some(Arc::new(crate::rq_tests::MockExporter { log: Default::default() }));
        // Analysis: stopping drops the job, nothing is written.
        s.execute("track.motion", serde_json::json!({})).unwrap();
        let rev = s.revision;
        s.execute("track.analyze", serde_json::json!({})).unwrap();
        assert!(s.is_tracking());
        assert!(s.execute("track.analyze", serde_json::json!({})).is_err(), "one at a time");
        assert!(s.stop_track());
        s.poll_offload();
        assert!(!s.is_tracking());
        assert_eq!(off.0.lock().unwrap().len(), 1);
        assert!(s.events.iter().any(|e| matches!(e, Event::Toast { message, .. } if message.contains("stopped"))));
        assert!(s.revision > rev);
        // Render: the queued items become User Stopped.
        s.execute("renderQueue.add", serde_json::json!({"output": "/a.gif"})).unwrap();
        let ids = s.start_render(false).unwrap();
        assert!(s.is_rendering());
        assert!(s.stop_render());
        s.poll_render();
        assert!(!s.is_rendering());
        assert_eq!(off.0.lock().unwrap().len(), 2);
        let it = s.project.render_queue.iter().find(|i| i.id == ids[0]).unwrap();
        assert_eq!(it.status, RenderStatus::UserStopped);
    }

    #[test]
    fn progress_hook_is_scoped() {
        report(1, 2); // no hook: nothing happens
        let seen = Rc::new(RefCell::new(vec![]));
        let s2 = seen.clone();
        effectcraft_render::passes::block_on(with_hook(Box::new(move |d, t| s2.borrow_mut().push((d, t))), async {
            report(1, 3);
            report(2, 3);
        }));
        report(3, 3);
        assert_eq!(*seen.borrow(), vec![(1, 3), (2, 3)]);
    }
}
