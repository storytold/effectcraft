//! Roto Brush & Refine Edge runtime: propagation and Freeze jobs, Input Key maintenance and the
//! tool options.
//!
//! Segmentations are derived data in `effectcraft_effects::roto`'s content-keyed cache (see that
//! module). `roto.propagate` fills it at full resolution in the background (or blocking with
//! `wait`; in the browser the background is a Web Worker whose segmentations stream back,
//! [`crate::offload`]), frame by frame outward from the base frame, rendering the effect's input
//! (`Renderer::layer_input`, source → masks → the effects above it). `roto.freeze` runs the same
//! pass and stores the final mattes in the effect (one undo step). `sync`, run after every
//! edit, keeps each instance's hidden Input Key equal to a signature of what its frames are made
//! of (the layer's source and timing, its masks and the effects above it) and drops frozen
//! mattes made from other frames.
//!
//! Object Matte instances run through the same jobs (`objectMatte.propagate` / `.freeze`): the
//! frames are [`om::job_frames`] around the reference frame, each segmented through
//! [`om::ensure`]; Freeze stores [`om::frame_matte`]'s mattes under [`om::frozen_key`]. An edit
//! to an instance's frames or prompt queues it for background tracking like a Roto Brush.

use crate::offload::JobKind;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_effects::object_matte as om;
use effectcraft_effects::roto::{self as fx, FROZEN, FrozenCache, INPUT_KEY};
use effectcraft_effects::{Params, flatten_params};
use effectcraft_keyframe::Value;
use effectcraft_project::{GroupKind, ItemId, ItemKind, Layer, LayerId, Project, PropGroup, Uid};
use effectcraft_raster::Image;
use effectcraft_render::{EvalCtx, ExprHost, FootageSource, LayerCache, RenderOpts, Renderer};
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};

use crate::tracking::lock;
use crate::{Event, Session};

/// Roto Brush / Refine Edge tool options (serde: `roto.options`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RotoOptions {
    /// Roto Brush tip diameter (layer pixels).
    pub diameter: f64,
    /// Refine Edge tip diameter (layer pixels).
    pub refine_diameter: f64,
    /// Layer panel view: `alphaBoundary` (pink outline), `alpha`, `alphaOverlay`, `none`.
    pub view: String,
    /// Alpha Overlay colour (straight RGB 0–1) and opacity (percent).
    pub overlay_color: [f64; 3],
    pub overlay_opacity: f64,
    /// Alpha Boundary colour.
    pub boundary_color: [f64; 3],
    /// Start propagating in the background after each stroke.
    pub auto_propagate: bool,
}

impl Default for RotoOptions {
    fn default() -> Self {
        RotoOptions {
            diameter: 20.0,
            refine_diameter: 10.0,
            view: "alphaBoundary".into(),
            overlay_color: [1.0, 0.0, 0.0],
            overlay_opacity: 50.0,
            boundary_color: [1.0, 0.25, 0.75],
            auto_propagate: true,
        }
    }
}

pub const VIEWS: [&str; 4] = ["alphaBoundary", "alpha", "alphaOverlay", "none"];

/// What a job does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RotoTask {
    Propagate,
    Freeze,
}

/// Which way to propagate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    Forward,
    Backward,
    #[default]
    Both,
}

impl Direction {
    pub fn from_name(s: &str) -> Option<Direction> {
        Some(match s.to_ascii_lowercase().as_str() {
            "forward" | "fwd" => Direction::Forward,
            "backward" | "back" => Direction::Backward,
            "both" | "all" => Direction::Both,
            _ => return None,
        })
    }
}

/// Live job progress (serde for agents / the control channel).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotoProgress {
    pub task: Option<RotoTask>,
    pub done: u64,
    pub total: u64,
    pub elapsed: f64,
    pub fps: f64,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    /// The job works on an Object Matte (else a Roto Brush & Refine Edge).
    pub object_matte: bool,
}

impl RotoProgress {
    pub fn banner(&self) -> String {
        let pc = (100 * self.done).checked_div(self.total).unwrap_or(0);
        match (self.task, self.object_matte) {
            (Some(RotoTask::Freeze), false) => format!("Freezing Roto Brush & Refine Edge: {pc}%"),
            (Some(RotoTask::Freeze), true) => format!("Freezing Object Matte: {pc}%"),
            (_, false) => format!("Propagating Roto Brush: {pc}%"),
            (_, true) => format!("Tracking Object Matte: {pc}%"),
        }
    }
}

#[derive(Default)]
pub struct RotoShared {
    pub cancel: AtomicBool,
    pub state: Mutex<RotoProgress>,
    frozen: Mutex<Option<FrozenCache>>,
}

pub struct RotoJob {
    pub shared: Arc<RotoShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub effect: Uid,
    pub task: RotoTask,
    /// What the job was started from: the strokes (Roto Brush) or the prompt (Object Matte); a
    /// Freeze result is dropped when it changed.
    strokes: String,
    /// The job works on an Object Matte.
    pub object_matte: bool,
}

impl RotoJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn progress(&self) -> RotoProgress {
        lock(&self.shared.state).clone()
    }
}

pub(crate) fn is_roto(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == fx::ID)
}

/// An Object Matte instance (its Input Key is kept current like Roto Brush's; it segments on
/// demand while rendering).
pub(crate) fn is_object_matte(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == effectcraft_effects::object_matte::ID)
}

/// What an Object Matte's segmentation is made from besides its frames: the prompt and tracking
/// parameters (static values), for queueing it after edits and guarding a Freeze.
pub(crate) fn om_prompt(g: &PropGroup) -> String {
    let ps = params_static(g);
    let mut keys: Vec<&String> = ps.values.keys().filter(|k| ["objectPoint", "useBox", "box", "objectTracking/"].iter().any(|p| k.starts_with(p))).collect();
    keys.sort();
    keys.iter().map(|k| format!("{k}={:?};", ps.values.get(*k))).collect()
}

/// The job-relevant guard of an instance: its strokes (Roto Brush) or prompt (Object Matte).
fn guard_of(g: &PropGroup) -> String {
    if is_object_matte(g) { om_prompt(g) } else { str_of(g, fx::STROKES) }
}

fn str_of(g: &PropGroup, m: &str) -> String {
    match g.get(m).map(|p| &p.value) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

fn set_str(g: &mut PropGroup, m: &str, v: String) {
    if let Some(p) = g.get_mut(m) {
        p.value = Value::Str(v);
        p.keys.clear();
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// What the effect's input frames are made of: the layer's source and timing, its masks and the
/// effects above the Roto Brush.
pub fn input_key(p: &Project, comp: ItemId, layer: &Layer, uid: Uid) -> String {
    let mut s = crate::warp::signature(p, comp, layer);
    if let Some(m) = layer.masks() {
        s.push_str(&format!("{m:?}"));
    }
    if let Some(fx) = layer.effects() {
        for g in fx.groups() {
            if g.uid == uid {
                break;
            }
            s.push_str(&format!("{g:?}"));
        }
    }
    format!("{:016x}", fnv(&s))
}

/// After an edit: refresh Input Keys (Roto Brush and Object Matte); frozen mattes made from
/// other frames are dropped. Returns the instances whose segmentation changed (to propagate
/// again): new frames, strokes (Roto Brush) or prompt (Object Matte).
pub(crate) fn sync(before: &Project, after: &mut Project) -> Vec<(ItemId, LayerId, Uid)> {
    let mut todo = vec![];
    let mut set = vec![];
    for (cid, it) in &after.items {
        let ItemKind::Comp(c) = &it.kind else { continue };
        for l in &c.layers {
            let Some(fxg) = l.effects() else { continue };
            for g in fxg.groups().filter(|g| is_roto(g) || is_object_matte(g)) {
                let key = input_key(after, *cid, l, g.uid);
                let old = before.comp(*cid).and_then(|bc| bc.layer(l.id)).and_then(|ol| ol.props.find_group(g.uid));
                if str_of(g, INPUT_KEY) != key {
                    set.push((*cid, l.id, g.uid, key));
                    todo.push((*cid, l.id, g.uid));
                } else if old.is_none_or(|o| guard_of(o) != guard_of(g)) {
                    todo.push((*cid, l.id, g.uid));
                }
            }
        }
    }
    for (cid, lid, uid, key) in set {
        if let Some(g) = after.comp_mut(cid).and_then(|c| c.layer_mut(lid)).and_then(|l| l.props.find_group_mut(uid)) {
            let had = !str_of(g, INPUT_KEY).is_empty();
            set_str(g, INPUT_KEY, key);
            if had {
                set_str(g, FROZEN, String::new());
            }
        }
    }
    todo.retain(|(c, l, u)| {
        after
            .comp(*c)
            .and_then(|c| c.layer(*l))
            .and_then(|l| l.props.find_group(*u))
            .is_some_and(|g| is_object_matte(g) || !fx::data(&params_static(g)).is_empty())
    });
    todo
}

/// Parameters of an instance with static values (strokes and keys are never animated).
pub(crate) fn params_static(g: &PropGroup) -> Params {
    flatten_params(g, &mut |p| p.value.clone())
}

/// Evaluated parameters of an instance at comp time `t`.
pub(crate) fn params_at(p: &Project, cid: ItemId, layer: &Layer, g: &PropGroup, t: Tick, expr: Option<&dyn ExprHost>) -> Params {
    match p.comp(cid) {
        Some(c) => {
            let ctx = EvalCtx { expr, ..EvalCtx::new(p, cid, c, t) };
            flatten_params(g, &mut |pr| ctx.value(layer, pr))
        }
        None => params_static(g),
    }
}

/// The layer's source size (layer pixels).
pub(crate) fn layer_size(p: &Project, comp: &effectcraft_project::Comp, layer: &Layer) -> [f64; 2] {
    effectcraft_render::effect_bounds(p, comp, layer).0
}

/// First and last layer frame of the layer inside the comp (inclusive).
pub fn frame_limits(comp: &effectcraft_project::Comp, layer: &Layer) -> [i64; 2] {
    let fps = comp.frame_rate.as_f64().max(1e-6);
    let lo = layer.layer_time(layer.in_point.max(Tick::ZERO)).seconds();
    let hi = layer.layer_time(layer.out_point.min(comp.duration)).seconds();
    let a = fx::frame_of(lo, fps);
    let b = (fx::frame_of(hi, fps) - 1).max(a);
    [a, b]
}

/// Comp time of layer frame `f`.
pub(crate) fn comp_time_of(comp: &effectcraft_project::Comp, layer: &Layer, f: i64) -> Tick {
    layer.comp_time(comp.frame_rate.tick_of(f))
}

/// What the worker needs, detached from the session.
struct Work {
    project: Arc<Project>,
    footage: Arc<dyn FootageSource>,
    expr: Option<Arc<dyn ExprHost>>,
    cache: Arc<LayerCache>,
    comp: ItemId,
    layer: LayerId,
    effect_index: usize,
    params: Params,
    size: [f64; 2],
    fps: f64,
    frames: Vec<i64>,
    task: RotoTask,
    object_matte: bool,
}

/// Renders the effect's input frames for a worker, keeping the last few. With a browser job
/// worker's GPU (deferred readbacks), [`FrameSource::prefetch`] renders the frames a step
/// will read in passes beforehand; [`FrameSource::get`] renders on the CPU what it lacks.
pub(crate) struct FrameSource<'a> {
    r: Renderer<'a>,
    accel: Option<&'a dyn effectcraft_render::Accelerator>,
    /// Frames in `memo`, oldest first.
    order: std::collections::VecDeque<i64>,
    ctx0: EvalCtx<'a>,
    comp: &'a effectcraft_project::Comp,
    layer: &'a Layer,
    index: usize,
    size: [f64; 2],
    memo: HashMap<i64, Arc<Image>>,
}

impl<'a> FrameSource<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        project: &'a Project,
        footage: &'a dyn FootageSource,
        expr: Option<&'a dyn ExprHost>,
        cache: &'a LayerCache,
        cid: ItemId,
        comp: &'a effectcraft_project::Comp,
        layer: &'a Layer,
        index: usize,
        size: [f64; 2],
    ) -> FrameSource<'a> {
        let mut r = Renderer::new(project, footage, RenderOpts { scale: 1.0, ..Default::default() });
        r.expr = expr;
        r.cache = Some(cache);
        let ctx0 = EvalCtx { expr, ..EvalCtx::new(project, cid, comp, Tick::ZERO) };
        FrameSource { r, accel: None, order: Default::default(), ctx0, comp, layer, index, size, memo: HashMap::new() }
    }

    /// Render with `accel` (deferred readbacks: see [`FrameSource::prefetch`]).
    pub(crate) fn with_accel(mut self, accel: Option<&'a dyn effectcraft_render::Accelerator>) -> Self {
        self.accel = accel;
        self.r.opts = crate::offload::analysis_opts(accel);
        self
    }

    /// Render frames `fs` (those not kept yet) in passes on the GPU, so the step that reads
    /// them finds them ([`FrameSource::get`] can't wait for the GPU).
    pub(crate) async fn prefetch(&mut self, fs: &[i64]) {
        let Some(a) = self.accel else { return };
        for &f in fs {
            if self.memo.contains_key(&f) {
                continue;
            }
            let this = &*self;
            let img = crate::offload::in_passes(Some(a), |acc| this.render(f, &this.r.with_accel(acc))).await;
            if let Some(img) = img {
                self.keep(f, img);
            }
        }
    }

    fn keep(&mut self, f: i64, img: Arc<Image>) {
        while self.order.len() >= 8 {
            if let Some(old) = self.order.pop_front() {
                self.memo.remove(&old);
            }
        }
        self.order.push_back(f);
        self.memo.insert(f, img);
    }

    pub(crate) fn get(&mut self, f: i64) -> Option<Arc<Image>> {
        if let Some(i) = self.memo.get(&f) {
            return Some(i.clone());
        }
        // Not prefetched: the CPU (a pass of the GPU could only return placeholders here).
        let img = self.render(f, &self.r.with_accel(None))?;
        self.keep(f, img.clone());
        Some(img)
    }

    fn render(&self, f: i64, r: &Renderer) -> Option<Arc<Image>> {
        let t = comp_time_of(self.comp, self.layer, f);
        let buf = r.layer_input(&self.ctx0.at(t), self.layer, self.index)?;
        let buf = if (buf.scale - 1.0).abs() > 1e-9 && buf.scale > 0.0 {
            let w = (buf.img.width as f64 / buf.scale).round().max(1.0) as u32;
            let h = (buf.img.height as f64 / buf.scale).round().max(1.0) as u32;
            effectcraft_effects::Buf {
                img: effectcraft_raster::resample(&buf.img, w, h),
                offset: [buf.offset[0] / buf.scale, buf.offset[1] / buf.scale],
                scale: 1.0,
            }
        } else {
            (*buf).clone()
        };
        Some(Arc::new(fx::layer_grid(&buf, self.size)))
    }
}

async fn run_work(w: Work, shared: &RotoShared) {
    let t0 = web_time::Instant::now();
    // A browser job worker's GPU (deferred readbacks): input frames render in passes.
    let accel = crate::offload::job_accel();
    let fail = |msg: &str| {
        let mut s = lock(&shared.state);
        s.error = Some(msg.to_string());
        s.finished = true;
    };
    let Some(comp) = w.project.comp(w.comp) else { return fail("composition missing") };
    let Some(layer) = comp.layer(w.layer) else { return fail("layer missing") };
    let mut src =
        FrameSource::new(&w.project, w.footage.as_ref(), w.expr.as_deref(), &w.cache, w.comp, comp, layer, w.effect_index, w.size).with_accel(accel.as_deref());
    {
        let mut s = lock(&shared.state);
        s.task = Some(w.task);
        s.total = w.frames.len() as u64;
        s.object_matte = w.object_matte;
    }
    let cancel = || shared.cancel.load(Ordering::Relaxed);
    let chain = fx::Chain::new(&w.params, w.size, w.fps, 1.0);
    let model = fx::installed_model();
    let key = if w.object_matte { om::frozen_key(&w.params, w.size, w.fps, model.as_deref()) } else { fx::frozen_key(&w.params, w.size, w.fps) };
    let mut frozen = FrozenCache { key, ..Default::default() };
    for (k, f) in w.frames.iter().enumerate() {
        if cancel() {
            let mut s = lock(&shared.state);
            s.cancelled = true;
            s.finished = true;
            return;
        }
        // The frame and its neighbours (what a step reads), on the GPU.
        src.prefetch(&[*f, f - 1, f + 1]).await;
        let mut get = |g: i64| src.get(g);
        let ok = match (w.task, w.object_matte) {
            (RotoTask::Propagate, true) => om::ensure(&w.params, w.size, w.fps, *f, model.clone(), &mut get, &cancel).is_some(),
            (RotoTask::Freeze, true) => {
                let img = get(*f);
                match img.map(|img| (om::frame_matte(&w.params, w.size, w.fps, *f, 1.0, &img, model.clone(), &mut get, &cancel), img)) {
                    Some((om::FrameMatte::Alpha(a), img)) => {
                        frozen.size = [img.width as usize, img.height as usize];
                        frozen.frames.insert(*f, fx::freeze_frame(&a, &[]));
                        true
                    }
                    _ => false,
                }
            }
            (RotoTask::Propagate, false) => chain.ensure(*f, &mut get, &cancel).is_some(),
            (RotoTask::Freeze, false) => match fx::frame_matte(&w.params, w.size, w.fps, *f, &mut get, &cancel) {
                Some((a, b)) => {
                    if let Some(img) = src.get(*f) {
                        frozen.size = [img.width as usize, img.height as usize];
                    }
                    frozen.frames.insert(*f, fx::freeze_frame(&a, &b));
                    true
                }
                None => false,
            },
        };
        if !ok && !cancel() {
            return fail(&format!("frame {f} could not be segmented"));
        }
        let mut s = lock(&shared.state);
        s.done = k as u64 + 1;
        s.elapsed = t0.elapsed().as_secs_f64();
        s.fps = if s.elapsed > 0.0 { s.done as f64 / s.elapsed } else { 0.0 };
        crate::offload::report(s.done, s.total);
    }
    if w.task == RotoTask::Freeze {
        *lock(&shared.frozen) = Some(frozen);
    }
    let mut s = lock(&shared.state);
    s.elapsed = t0.elapsed().as_secs_f64();
    s.finished = true;
}

/// Frames to compute, outward from the base frame.
pub(crate) fn job_frames(d: &effectcraft_track::roto::RotoData, dir: Direction) -> Vec<i64> {
    let Some(base) = d.base else { return vec![] };
    let mut v = vec![base];
    if dir != Direction::Backward {
        v.extend(base + 1..=d.span[1]);
    }
    if dir != Direction::Forward {
        v.extend((d.span[0]..base).rev());
    }
    v
}

impl Session {
    /// The Roto Brush job running in a worker (Freeze or propagation), with its task.
    fn offloaded_roto(&self) -> Option<(&crate::offload::OffloadedJob, RotoTask)> {
        self.offloaded(JobKind::RotoFreeze).map(|j| (j, RotoTask::Freeze)).or_else(|| self.offloaded(JobKind::RotoPropagate).map(|j| (j, RotoTask::Propagate)))
    }

    pub fn is_roto_running(&self) -> bool {
        self.roto_job.as_ref().is_some_and(|j| !j.is_finished()) || self.offloaded_roto().is_some()
    }

    pub fn roto_progress(&self) -> Option<RotoProgress> {
        self.roto_job.as_ref().map(|j| j.progress()).or_else(|| {
            self.offloaded_roto().map(|(j, task)| {
                let p = &j.progress;
                let object_matte =
                    self.project.comp(j.comp).and_then(|c| c.layer(j.layer)).and_then(|l| l.props.find_group(j.uid)).is_some_and(is_object_matte);
                RotoProgress {
                    task: Some(task),
                    done: p.done,
                    total: p.total,
                    elapsed: p.elapsed,
                    fps: crate::offload::rate(p.done, p.elapsed),
                    object_matte,
                    ..Default::default()
                }
            })
        })
    }

    /// (comp, layer, effect uid, task) of the running job.
    pub fn roto_target(&self) -> Option<(ItemId, LayerId, Uid, RotoTask)> {
        self.roto_job.as_ref().map(|j| (j.comp, j.layer, j.effect, j.task)).or_else(|| self.offloaded_roto().map(|(j, task)| (j.comp, j.layer, j.uid, task)))
    }

    /// The segmentation chain a propagation of Roto Brush `effect` computes (its frames' cache
    /// keys).
    pub fn roto_chain(&self, comp: ItemId, layer: LayerId, effect: Uid) -> Option<fx::Chain> {
        let c = self.project.comp(comp)?;
        let l = c.layer(layer)?;
        let g = l.effects()?.groups().find(|g| g.uid == effect && is_roto(g))?;
        let d = fx::data(&params_static(g));
        let t = comp_time_of(c, l, d.base?);
        let params = params_at(&self.project, comp, l, g, t, self.expr.as_deref());
        Some(fx::Chain::new(&params, layer_size(&self.project, c, l), c.frame_rate.as_f64(), 1.0))
    }

    /// The cache keys of the segmentations a propagation of `effect` (Roto Brush or Object
    /// Matte) computes, by layer frame.
    pub fn roto_keys(&self, comp: ItemId, layer: LayerId, effect: Uid) -> Option<Vec<(i64, u64)>> {
        if let Some(ch) = self.roto_chain(comp, layer, effect) {
            return Some(ch.keys.into_iter().collect());
        }
        let c = self.project.comp(comp)?;
        let l = c.layer(layer)?;
        let g = l.effects()?.groups().find(|g| g.uid == effect && is_object_matte(g))?;
        let params = self.om_params(comp, l, g)?;
        let frames = om::job_frames(&params, frame_limits(c, l));
        Some(om::frame_keys(&params, layer_size(&self.project, c, l), c.frame_rate.as_f64(), &frames, fx::installed_model()))
    }

    /// An Object Matte's parameters at its reference frame (what its jobs segment with).
    fn om_params(&self, comp: ItemId, l: &Layer, g: &PropGroup) -> Option<Params> {
        let c = self.project.comp(comp)?;
        let t = comp_time_of(c, l, om::reference_frame(&params_static(g)));
        Some(params_at(&self.project, comp, l, g, t, self.expr.as_deref()))
    }

    pub fn stop_roto(&mut self) -> bool {
        if self.cancel_offloaded(JobKind::RotoFreeze) || self.cancel_offloaded(JobKind::RotoPropagate) {
            return true;
        }
        match &self.roto_job {
            Some(j) if !j.is_finished() => {
                j.shared.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Start propagating (or freezing) Roto Brush `effect` on `layer`. `wait`: block until done
    /// (wasm32 always does). Returns the number of frames.
    pub fn start_roto(&mut self, comp: ItemId, layer: LayerId, effect: Uid, task: RotoTask, dir: Direction, wait: bool) -> Result<usize, String> {
        if self.is_roto_running() {
            if !wait {
                return Err("a Roto Brush job is already running".into());
            }
            self.stop_roto();
            if let Some(mut j) = self.roto_job.take()
                && let Some(t) = j.thread.take()
            {
                let _ = t.join();
            }
        }
        self.poll_roto_job();
        self.roto_pending.retain(|x| *x != (comp, layer, effect));
        let c = self.project.comp(comp).ok_or("no composition")?;
        let l = c.layer(layer).ok_or("no layer")?;
        let fxg = l.effects().ok_or("the layer has no effects")?;
        let index = fxg.groups().position(|g| g.uid == effect).ok_or("no such effect")?;
        let g = fxg.groups().nth(index).filter(|g| is_roto(g) || is_object_matte(g)).ok_or("not a Roto Brush & Refine Edge or Object Matte effect")?;
        let object_matte = is_object_matte(g);
        let size = layer_size(&self.project, c, l);
        let fps = c.frame_rate.as_f64();
        let (params, frames) = if object_matte {
            let params = self.om_params(comp, l, g).ok_or("no composition")?;
            let base = om::reference_frame(&params);
            if om::roto_data(&params, size, base, fx::installed_model().is_some()).is_none() {
                return Err("set the object first: click it with the Object Matte tool (its point is off the layer or outside its box)".into());
            }
            let all = om::job_frames(&params, frame_limits(c, l));
            if all.is_empty() {
                return Err(format!("the reference frame {base} is outside the layer"));
            }
            // Forward / backward from the reference frame, as for a Roto Brush.
            let frames = all
                .into_iter()
                .filter(|f| task == RotoTask::Freeze || dir == Direction::Both || *f == base || (*f > base) == (dir == Direction::Forward))
                .collect();
            (params, frames)
        } else {
            let pstat = params_static(g);
            let d = fx::data(&pstat);
            if d.is_empty() {
                return Err("draw a foreground stroke with the Roto Brush tool first".into());
            }
            let t = comp_time_of(c, l, d.base.unwrap_or(0));
            let params = params_at(&self.project, comp, l, g, t, self.expr.as_deref());
            (params, job_frames(&d, if task == RotoTask::Freeze { Direction::Both } else { dir }))
        };
        let n = frames.len();
        let work = Work {
            project: self.project.clone(),
            footage: self.footage.clone(),
            expr: self.expr.clone(),
            cache: self.layer_cache.clone(),
            comp,
            layer,
            effect_index: index,
            params,
            size,
            fps,
            frames,
            task,
            object_matte,
        };
        let strokes = guard_of(g);
        if !wait && self.offloads() {
            drop(work);
            self.offload_analysis(match task {
                RotoTask::Freeze => crate::offload::WorkerJob::RotoFreeze { comp, layer, effect },
                RotoTask::Propagate => crate::offload::WorkerJob::RotoPropagate { comp, layer, effect, direction: dir },
            })?;
            return Ok(n);
        }
        let wait = wait || cfg!(target_arch = "wasm32");
        let shared = Arc::new(RotoShared::default());
        {
            // Known before the worker starts (the progress shows at once).
            let mut st = lock(&shared.state);
            st.task = Some(task);
            st.total = n as u64;
            st.object_matte = object_matte;
        }
        let thread = if wait {
            crate::offload::run_or_queue({
                let sh = shared.clone();
                async move { run_work(work, &sh).await }
            });
            None
        } else {
            let sh = shared.clone();
            Some(
                std::thread::Builder::new()
                    .name("roto".into())
                    .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh)))
                    .map_err(|e| e.to_string())?,
            )
        };
        self.roto_job = Some(RotoJob { shared, thread, comp, layer, effect, task, strokes, object_matte });
        self.events.push(Event::ProjectChanged { revision: self.revision });
        self.poll_roto_job();
        Ok(n)
    }

    /// Finish a done job: Freeze writes the frozen mattes (one undo step).
    fn poll_roto_job(&mut self) -> bool {
        let Some(job) = &self.roto_job else { return false };
        if !job.is_finished() {
            return false;
        }
        let Some(mut job) = self.roto_job.take() else { return false };
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        let st = job.progress();
        let name = if job.object_matte { "Object Matte" } else { "Roto Brush" };
        if let Some(e) = &st.error {
            self.events.push(Event::Toast { message: format!("{name}: {e}"), error: true });
            self.bump();
            return true;
        }
        if st.cancelled {
            self.events.push(Event::Toast { message: format!("{name}: stopped"), error: false });
            self.bump();
            return true;
        }
        match job.task {
            RotoTask::Propagate => {
                let verb = if job.object_matte { "tracked" } else { "propagated" };
                self.events.push(Event::Toast { message: format!("{name}: {verb} {} frame(s) in {:.1} s", st.done, st.elapsed), error: false });
                // Rendered frames depend on the cache: redraw.
                self.bump();
            }
            RotoTask::Freeze => {
                let Some(fc) = lock(&job.shared.frozen).take() else { return true };
                let (cid, lid, uid, strokes) = (job.comp, job.layer, job.effect, job.strokes.clone());
                let what = if job.object_matte { "prompt" } else { "strokes" };
                let r = self.edit("Freeze", None, |p, _| {
                    let g = p
                        .comp_mut(cid)
                        .and_then(|c| c.layer_mut(lid))
                        .and_then(|l| l.props.find_group_mut(uid))
                        .ok_or_else(|| crate::EngineError::Other(format!("the {name} effect was removed")))?;
                    if guard_of(g) != strokes {
                        return Err(crate::EngineError::Other(format!("the {what} changed while freezing")));
                    }
                    set_str(g, FROZEN, fc.to_json());
                    Ok(())
                });
                let msg = match r {
                    Ok(()) => format!("{name}: froze {} frame(s) in {:.1} s", fc.frames.len(), st.elapsed),
                    Err(e) => format!("{name}: {e}"),
                };
                self.events.push(Event::Toast { message: msg, error: false });
            }
        }
        true
    }

    /// Frontends call this every frame: finishes jobs and (with `auto`) propagates edited
    /// instances in the background.
    pub fn poll_roto(&mut self, auto: bool) -> bool {
        self.poll_models();
        let changed = self.poll_roto_job();
        if auto && self.roto_job.is_none() && self.offloaded_roto().is_none() && self.state.roto.auto_propagate {
            while let Some((c, l, e)) = self.roto_pending.first().copied() {
                self.roto_pending.remove(0);
                if self.start_roto(c, l, e, RotoTask::Propagate, Direction::Both, false).is_ok() {
                    return true;
                }
            }
        }
        changed
    }

    /// The computed frames of an instance (full resolution), by layer frame.
    pub fn roto_computed(&self, comp: ItemId, layer: LayerId, effect: Uid) -> BTreeMap<i64, bool> {
        let mut out = BTreeMap::new();
        let Some(c) = self.project.comp(comp) else { return out };
        let Some(l) = c.layer(layer) else { return out };
        let Some(g) = l.props.find_group(effect) else { return out };
        if is_object_matte(g) {
            for (f, k) in self.roto_keys(comp, layer, effect).unwrap_or_default() {
                out.insert(f, fx::cached(k, 1.0).is_some());
            }
            return out;
        }
        let ps = params_static(g);
        let ch = fx::Chain::new(&ps, layer_size(&self.project, c, l), c.frame_rate.as_f64(), 1.0);
        for f in ch.keys.keys() {
            out.insert(*f, ch.is_cached(*f, 1.0));
        }
        out
    }
}
