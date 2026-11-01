//! 3D Camera Tracker runtime: the analysis job (`camera.analyze`, `track.camera`), keeping
//! analyses valid, and turning a solve into composition layers (`camera.createFromSolve`).
//!
//! The job renders the layer's input to the effect (`Renderer::layer_input`) for every frame
//! between the layer's In and Out points on a background thread and tracks features through
//! them (step 1 of 2, "Analyzing in background"), then solves the camera (step 2, "Solving
//! camera"). When only the solve settings changed (Shot Type, Angle of View, Solve Method,
//! deleted points) the stored tracks are re-solved without re-tracking. Results are written into
//! the effect's hidden parameters as one undo step; edits that change the layer's frames clear
//! them and queue the effect again (`invalidate`, run after every edit), like After Effects.
//!
//! Composition space: the solve's canonical frame maps to comp world by
//! [`CameraSolve::world`] with the zoom of the first solved frame (focal length × the layer's
//! scale in the comp), so the "3D Tracker Camera" reproduces the footage's view and created
//! layers sit on the selected points.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_effects::camera_tracker::{self as ct, AVERAGE_ERROR, METHOD_USED, SOLVE, SOLVE_KEY, TRACKS, TRACKS_KEY};
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{AutoOrient, GroupKind, ItemId, ItemKind, Layer, LayerId, Project, PropGroup, Uid};
use effectcraft_render::{EvalCtx, ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use effectcraft_track::camtrack::linalg::{self, Similarity, V3};
use effectcraft_track::camtrack::{AnalyzeOpts, CameraSolve, CameraTracks, SolveSettings, Target, TrackAnalyzer};
use serde::Serialize;

use crate::offload::JobKind;
use crate::tracking::lock;
use crate::warp::{analysis_times, signature};
use crate::{Event, Session};

/// Live analysis progress (serde for agents / the control channel).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraProgress {
    /// 1 = tracking features, 2 = solving the camera.
    pub step: u8,
    pub done: u64,
    pub total: u64,
    pub elapsed: f64,
    pub fps: f64,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    /// Re-solving stored tracks (no tracking step).
    pub solve_only: bool,
}

impl CameraProgress {
    /// The viewer banner text, as After Effects words it.
    pub fn banner(&self) -> String {
        match self.step {
            2 => "Solving camera".into(),
            _ => {
                let pc = (100 * self.done).checked_div(self.total).unwrap_or(0);
                format!("Analyzing in background (step 1 of 2): {pc}%")
            }
        }
    }
}

struct CamResult {
    /// New tracks (None for a re-solve).
    tracks: Option<CameraTracks>,
    solve: Result<CameraSolve, String>,
}

#[derive(Default)]
pub struct CameraShared {
    pub cancel: AtomicBool,
    pub state: Mutex<CameraProgress>,
    result: Mutex<Option<CamResult>>,
}

/// A running (or finished, not yet polled) camera analysis.
pub struct CameraJob {
    pub shared: Arc<CameraShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub effect: Uid,
    /// The layer signature the tracks are made from, and the solve settings key.
    key: String,
    solve_key: String,
}

impl CameraJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn progress(&self) -> CameraProgress {
        lock(&self.shared.state).clone()
    }
}

struct CamWork {
    project: Arc<Project>,
    footage: Arc<dyn FootageSource>,
    expr: Option<Arc<dyn ExprHost>>,
    cache: Arc<LayerCache>,
    comp: ItemId,
    layer: LayerId,
    effect_index: usize,
    size: [f64; 2],
    detailed: bool,
    times: Vec<Tick>,
    /// Stored tracks to re-solve (skips step 1).
    tracks: Option<Arc<CameraTracks>>,
    settings: SolveSettings,
}

pub(crate) fn is_tracker(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == ct::ID)
}

fn str_of(g: &PropGroup, m: &str) -> String {
    match g.prop(m).map(|p| &p.value) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// What a tracker's tracks are made from: the layer's frames and Detailed Analysis.
fn tracks_key(project: &Project, cid: ItemId, l: &Layer, g: &PropGroup) -> String {
    let detailed = effectcraft_effects::warp_stab::param(&static_params(g), "advanced/detailedAnalysis").is_some_and(Value::as_bool);
    format!("{}{}", signature(project, cid, l), if detailed { "+detailed" } else { "" })
}

/// Parameters of an instance from static values (the tracker's settings are not animated).
pub fn static_params(g: &PropGroup) -> effectcraft_effects::Params {
    effectcraft_effects::flatten_params(g, &mut |p| p.value.clone())
}

fn clear(g: &mut PropGroup, all: bool) {
    let mut ids = vec![SOLVE, SOLVE_KEY, METHOD_USED];
    if all {
        ids.extend([TRACKS, TRACKS_KEY]);
    }
    for m in ids {
        if let Some(p) = g.prop_mut(m) {
            p.value = Value::Str(String::new());
            p.keys.clear();
        }
    }
    if let Some(p) = g.prop_mut(AVERAGE_ERROR) {
        p.value = Value::Scalar(0.0);
    }
}

/// Clear analyses that no longer match, after an edit: tracks made from other frames (and their
/// solve), and solves made with other settings. Returns the trackers that need (re-)analysis:
/// cleared ones and newly applied, unanalysed ones.
pub(crate) fn invalidate(before: &Project, after: &mut Project) -> Vec<(ItemId, LayerId, Uid)> {
    let mut todo = vec![];
    let mut clears = vec![];
    for (cid, it) in &after.items {
        let ItemKind::Comp(c) = &it.kind else { continue };
        for l in &c.layers {
            let Some(fx) = l.effects() else { continue };
            if !fx.groups().any(is_tracker) {
                continue;
            }
            let old_layer = before.comp(*cid).and_then(|bc| bc.layer(l.id));
            for g in fx.groups().filter(|g| is_tracker(g)) {
                let tracked = !str_of(g, TRACKS).is_empty();
                if tracked && str_of(g, TRACKS_KEY) != tracks_key(after, *cid, l, g) {
                    clears.push((*cid, l.id, g.uid, true));
                    todo.push((*cid, l.id, g.uid));
                } else if tracked && str_of(g, SOLVE_KEY) != ct::solve_key(&ct::settings(&static_params(g))) {
                    clears.push((*cid, l.id, g.uid, false));
                    todo.push((*cid, l.id, g.uid));
                } else if !tracked && old_layer.and_then(|ol| ol.props.find_group(g.uid)).is_none() {
                    todo.push((*cid, l.id, g.uid));
                }
            }
        }
    }
    for (cid, lid, uid, all) in clears {
        if let Some(g) = after.comp_mut(cid).and_then(|c| c.layer_mut(lid)).and_then(|l| l.props.find_group_mut(uid)) {
            clear(g, all);
        }
    }
    todo
}

fn input_frame(r: &Renderer, ctx: &EvalCtx, layer: &Layer, effect_index: usize) -> Option<(Arc<effectcraft_raster::Image>, [f64; 2])> {
    let buf = r.layer_input(ctx, layer, effect_index)?;
    if (buf.scale - 1.0).abs() > 1e-9 && buf.scale > 0.0 {
        let w = (buf.img.width as f64 / buf.scale).round().max(1.0) as u32;
        let h = (buf.img.height as f64 / buf.scale).round().max(1.0) as u32;
        let off = [buf.offset[0] / buf.scale, buf.offset[1] / buf.scale];
        return Some((Arc::new(effectcraft_raster::resample(&buf.img, w, h)), off));
    }
    Some((Arc::new(buf.img.clone()), buf.offset))
}

async fn run_work(w: CamWork, shared: &CameraShared) {
    let t0 = web_time::Instant::now();
    // A browser job worker's GPU (deferred readbacks): frames render in passes.
    let accel = crate::offload::job_accel();
    let accel = accel.as_deref();
    let fail = |msg: &str| {
        let mut s = lock(&shared.state);
        s.error = Some(msg.to_string());
        s.finished = true;
    };
    let cancelled = || {
        let mut s = lock(&shared.state);
        s.cancelled = true;
        s.finished = true;
    };
    let Some(comp) = w.project.comp(w.comp) else { return fail("composition missing") };
    let Some(layer) = comp.layer(w.layer) else { return fail("layer missing") };
    let lt0 = layer.layer_time(w.times[0]).seconds();
    let fd = if w.times.len() > 1 { layer.layer_time(w.times[1]).seconds() - lt0 } else { comp.frame_duration().seconds() };
    let (tracks, fresh) = match w.tracks.clone() {
        Some(t) => (t, false),
        None => {
            let mut r = Renderer::new(&w.project, w.footage.as_ref(), crate::offload::analysis_opts(accel));
            r.expr = w.expr.as_deref();
            r.cache = Some(&w.cache);
            let r = &r;
            let ctx0 = EvalCtx::new(&w.project, w.comp, comp, Tick::ZERO);
            let ctx0 = EvalCtx { expr: w.expr.as_deref(), ..ctx0 };
            let ctx0 = &ctx0;
            let frame_at = |t: Tick| crate::offload::in_passes(accel, move |a| input_frame(&r.with_accel(a), &ctx0.at(t), layer, w.effect_index));
            {
                let mut s = lock(&shared.state);
                s.step = 1;
                s.total = w.times.len() as u64;
            }
            let mut an = TrackAnalyzer::new(w.size, AnalyzeOpts { detailed: w.detailed });
            let mut next = frame_at(w.times[0]).await;
            for k in 0..w.times.len() {
                if shared.cancel.load(Ordering::Relaxed) {
                    return cancelled();
                }
                let Some(cur) = next.take() else { return fail(&format!("no frame at {:.3} s", w.times[k].seconds())) };
                let ((), nf) = crate::offload::join_fetch(
                    accel.is_some(),
                    || an.push(&effectcraft_track::Frame { img: &cur.0, offset: cur.1 }),
                    || async { if k + 1 < w.times.len() { frame_at(w.times[k + 1]).await } else { None } },
                )
                .await;
                next = nf;
                let mut s = lock(&shared.state);
                s.done = k as u64 + 1;
                s.elapsed = t0.elapsed().as_secs_f64();
                s.fps = if s.elapsed > 0.0 { s.done as f64 / s.elapsed } else { 0.0 };
                crate::offload::report(s.done, s.total);
            }
            (Arc::new(an.finish(lt0, fd)), true)
        }
    };
    lock(&shared.state).step = 2;
    let solve = effectcraft_track::camtrack::solve(&tracks, &w.settings, Some(&shared.cancel)).map_err(|e| e.0);
    if shared.cancel.load(Ordering::Relaxed) {
        return cancelled();
    }
    *lock(&shared.result) = Some(CamResult { tracks: fresh.then(|| (*tracks).clone()), solve });
    let mut s = lock(&shared.state);
    s.elapsed = t0.elapsed().as_secs_f64();
    s.finished = true;
}

impl Session {
    pub fn is_camera_analyzing(&self) -> bool {
        self.camera_job.as_ref().is_some_and(|j| !j.is_finished()) || self.offloaded(JobKind::Camera).is_some()
    }

    /// Progress of the running (or just finished) analysis.
    pub fn camera_progress(&self) -> Option<CameraProgress> {
        self.camera_job.as_ref().map(|j| j.progress()).or_else(|| {
            self.offloaded(JobKind::Camera).map(|j| {
                let p = &j.progress;
                CameraProgress { step: 1, done: p.done, total: p.total, elapsed: p.elapsed, fps: crate::offload::rate(p.done, p.elapsed), ..Default::default() }
            })
        })
    }

    /// The tracker being analysed: (comp, layer, effect uid).
    pub fn camera_target(&self) -> Option<(ItemId, LayerId, Uid)> {
        self.camera_job.as_ref().map(|j| (j.comp, j.layer, j.effect)).or_else(|| self.offloaded(JobKind::Camera).map(|j| (j.comp, j.layer, j.uid)))
    }

    /// Cancel the running analysis (nothing is written).
    pub fn stop_camera(&mut self) -> bool {
        if self.cancel_offloaded(JobKind::Camera) {
            return true;
        }
        match &self.camera_job {
            Some(j) if !j.is_finished() => {
                j.shared.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Start analysing 3D Camera Tracker `effect` on `layer` (re-solving the stored tracks when
    /// they are still valid). `wait`: block until done (wasm32 always does). Returns the number
    /// of frames.
    pub fn start_camera(&mut self, comp: ItemId, layer: LayerId, effect: Uid, wait: bool) -> Result<usize, String> {
        if self.is_camera_analyzing() {
            return Err("a 3D Camera Tracker analysis is already running".into());
        }
        self.poll_camera_job();
        self.camera_pending.retain(|x| *x != (comp, layer, effect));
        let c = self.project.comp(comp).ok_or("no composition")?;
        let l = c.layer(layer).ok_or("no layer")?;
        let fx = l.effects().ok_or("the layer has no effects")?;
        let index = fx.groups().position(|g| g.uid == effect).ok_or("no such effect")?;
        let g = fx.groups().nth(index).filter(|g| is_tracker(g)).ok_or("not a 3D Camera Tracker")?;
        if l.switches.adjustment {
            return Err("3D Camera Tracker needs footage, not an adjustment layer".into());
        }
        let times = analysis_times(c, l);
        if times.len() < 2 {
            return Err("the layer must be at least two frames long".into());
        }
        let size = effectcraft_render::effect_bounds(&self.project, c, l).0;
        let params = static_params(g);
        let key = tracks_key(&self.project, comp, l, g);
        let settings = ct::settings(&params);
        let solve_key = ct::solve_key(&settings);
        let tracks = ct::tracks(&params).filter(|_| str_of(g, TRACKS_KEY) == key);
        let n = times.len();
        let work = CamWork {
            project: self.project.clone(),
            footage: self.footage.clone(),
            expr: self.expr.clone(),
            cache: self.layer_cache.clone(),
            comp,
            layer,
            effect_index: index,
            size,
            detailed: params.values.iter().any(|(k, v)| k.ends_with("detailedAnalysis") && v.as_bool()),
            times,
            tracks: tracks.clone(),
            settings,
        };
        if !wait && self.offloads() {
            drop(work);
            self.offload_analysis(crate::offload::WorkerJob::Camera { comp, layer, effect })?;
            return Ok(n);
        }
        let wait = wait || cfg!(target_arch = "wasm32");
        let shared = Arc::new(CameraShared::default());
        {
            let mut st = lock(&shared.state);
            st.solve_only = tracks.is_some();
            st.step = if tracks.is_some() { 2 } else { 1 };
            st.total = n as u64;
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
                    .name("camera-track".into())
                    .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh)))
                    .map_err(|e| e.to_string())?,
            )
        };
        self.camera_job = Some(CameraJob { shared, thread, comp, layer, effect, key, solve_key });
        self.events.push(Event::ProjectChanged { revision: self.revision });
        self.poll_camera_job();
        Ok(n)
    }

    /// Write a finished analysis into its effect (one undo step) and drop the job.
    fn poll_camera_job(&mut self) -> bool {
        let Some(job) = &self.camera_job else { return false };
        if !job.is_finished() {
            return false;
        }
        let Some(mut job) = self.camera_job.take() else { return false };
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        let st = job.progress();
        let result = lock(&job.shared.result).take();
        match (result, &st.error) {
            (Some(res), None) if !st.cancelled => {
                let (cid, lid, uid) = (job.comp, job.layer, job.effect);
                let (key, skey) = (job.key.clone(), job.solve_key.clone());
                let tracks_json = res.tracks.as_ref().map(CameraTracks::to_json);
                let (solve_json, method, avg, msg, failed) = match &res.solve {
                    Ok(s) => (
                        s.to_json(),
                        s.method_used.label().to_string(),
                        s.average_error,
                        format!(
                            "3D Camera Tracker: solved {} frame(s), {} points, average error {:.2} px ({:.1} s)",
                            s.frames.len(),
                            s.points.len(),
                            s.average_error,
                            st.elapsed
                        ),
                        false,
                    ),
                    Err(e) => (String::new(), String::new(), 0.0, format!("3D Camera Tracker: {e}"), true),
                };
                let r = self.edit(if st.solve_only { "3D Camera Tracker Solve" } else { "3D Camera Tracker Analysis" }, None, |p, _| {
                    let g = p
                        .comp_mut(cid)
                        .and_then(|c| c.layer_mut(lid))
                        .and_then(|l| l.props.find_group_mut(uid))
                        .ok_or_else(|| crate::EngineError::Other("the 3D Camera Tracker was removed during analysis".into()))?;
                    let mut set = |m: &str, v: Value| {
                        if let Some(pr) = g.prop_mut(m) {
                            pr.value = v;
                        }
                    };
                    if let Some(t) = &tracks_json {
                        set(TRACKS, Value::Str(t.clone()));
                        set(TRACKS_KEY, Value::Str(key.clone()));
                    }
                    set(SOLVE, Value::Str(solve_json.clone()));
                    set(SOLVE_KEY, Value::Str(skey.clone()));
                    set(METHOD_USED, Value::Str(method.clone()));
                    set(AVERAGE_ERROR, Value::Scalar(avg));
                    Ok(())
                });
                let (msg, failed) = match r {
                    Ok(()) => (msg, failed),
                    Err(e) => (format!("3D Camera Tracker: {e}"), true),
                };
                self.events.push(Event::Toast { message: msg, error: failed });
            }
            _ => {
                let msg = match &st.error {
                    Some(e) => format!("3D Camera Tracker analysis failed: {e}"),
                    None => "3D Camera Tracker analysis cancelled".into(),
                };
                self.events.push(Event::Toast { message: msg, error: st.error.is_some() });
                self.bump();
            }
        }
        true
    }

    /// Frontends call this every frame: finishes analyses and (with `auto`) starts queued
    /// (re-)analyses in the background, like After Effects.
    pub fn poll_camera(&mut self, auto: bool) -> bool {
        let changed = self.poll_camera_job();
        if auto && self.camera_job.is_none() && self.offloaded(JobKind::Camera).is_none() {
            while let Some((c, l, e)) = self.camera_pending.first().copied() {
                self.camera_pending.remove(0);
                if self.start_camera(c, l, e, false).is_ok() {
                    return true;
                }
            }
        }
        changed
    }
}

// ---------------------------------------------------------------- solve → composition

/// A tracker's solve placed in its composition.
pub struct Placed {
    pub solve: Arc<CameraSolve>,
    /// Canonical → comp world.
    pub world: Similarity,
    /// Comp pixels per layer pixel.
    pub scale: f64,
    /// Comp times of the analysed frames.
    pub times: Vec<Tick>,
    pub comp_size: [f64; 2],
}

impl Placed {
    /// Comp-world position of a canonical point.
    pub fn to_world(&self, x: V3) -> V3 {
        self.world.apply(x)
    }
    /// Canonical position of a comp-world point.
    pub fn from_world(&self, x: V3) -> V3 {
        let r = &self.world;
        linalg::scale(linalg::mtv(&r.r, linalg::sub(x, r.t)), 1.0 / r.s)
    }
    /// The 3D Tracker Camera on frame `k`: (position, orientation degrees, zoom).
    pub fn camera(&self, k: usize) -> (V3, V3, f64) {
        let c = &self.solve.frames[k];
        let m = linalg::mm(&self.world.r, &linalg::mt(&c.r()));
        (self.to_world(c.center), linalg::euler_xyz(&m), c.focal * self.scale)
    }
    /// Analysed frame at comp time `t`.
    pub fn frame_at(&self, t: Tick) -> usize {
        self.times.iter().enumerate().min_by_key(|(_, x)| (x.0 - t.0).abs()).map(|(i, _)| i).unwrap_or(0).min(self.solve.frames.len().saturating_sub(1))
    }
}

/// Comp pixels per layer pixel of `layer` (its 2D transform at its In point).
fn layer_scale(project: &Project, cid: ItemId, layer: &Layer) -> f64 {
    let Some(comp) = project.comp(cid) else { return 1.0 };
    let ctx = EvalCtx::new(project, cid, comp, layer.in_point.max(Tick::ZERO));
    let m = ctx.world_matrix(layer).0;
    let s = m[0][0].hypot(m[1][0]);
    if s.is_finite() && s > 1e-9 { s } else { 1.0 }
}

/// The solve of tracker `uid` on `lid`, placed in the comp.
pub fn placed(project: &Project, cid: ItemId, lid: LayerId, uid: Uid) -> Option<Placed> {
    let comp = project.comp(cid)?;
    let layer = comp.layer(lid)?;
    let g = layer.props.find_group(uid).filter(|g| is_tracker(g))?;
    let solve = ct::solve(&static_params(g))?;
    let scale = layer_scale(project, cid, layer);
    let k0 = solve.first_solved()?;
    let comp_size = [comp.width as f64, comp.height as f64];
    let world = solve.world(comp_size, solve.frames[k0].focal * scale);
    let mut times = analysis_times(comp, layer);
    times.truncate(solve.frames.len());
    Some(Placed { solve, world, scale, times, comp_size })
}

/// What to create from a target (`camera.createFromSolve`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    Text,
    Solid,
    Null,
    ShadowCatcher,
    /// Only the camera.
    Camera,
}

impl CreateKind {
    pub fn from_name(s: &str) -> Option<CreateKind> {
        match s.to_ascii_lowercase().replace([' ', '-', '_'], "").as_str() {
            "text" => Some(CreateKind::Text),
            "solid" => Some(CreateKind::Solid),
            "null" => Some(CreateKind::Null),
            "shadowcatcher" | "shadow" => Some(CreateKind::ShadowCatcher),
            "camera" => Some(CreateKind::Camera),
            _ => None,
        }
    }
}

/// Rotation (layer → world) of a layer lying on a plane with unit `normal` (towards the camera),
/// its x axis along `right` projected on the plane: the layer's front (−z) faces the camera.
pub fn plane_orientation(normal: V3, right: V3) -> V3 {
    let z = linalg::scale(normal, -1.0);
    let mut x = linalg::sub(right, linalg::scale(z, linalg::dot(right, z)));
    if linalg::norm(x) < 1e-9 {
        x = if z[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        x = linalg::sub(x, linalg::scale(z, linalg::dot(x, z)));
    }
    let x = linalg::normalize(x);
    let y = linalg::cross(z, x);
    let m = linalg::from_cols(x, y, z);
    norm_angles(linalg::euler_xyz(&m))
}

fn norm_angles(a: V3) -> V3 {
    a.map(|v| {
        let r = v.rem_euclid(360.0);
        if (r - 360.0).abs() < 1e-9 { 0.0 } else { r }
    })
}

/// Key `path` on `l` at every `(time, value)`.
pub(crate) fn key_all(l: &mut Layer, path: &str, keys: &[(Tick, Value)]) {
    if let Some(pr) = l.props.prop_mut(path) {
        pr.keys = keys.iter().map(|(t, v)| Keyframe::new(*t, v.clone())).collect();
        if let Some((_, v)) = keys.first() {
            pr.value = v.clone();
        }
    }
}

/// Write the solved camera path into camera layer `l` (one-node, keyed on every analysed frame;
/// Zoom keyed for Variable Zoom).
pub(crate) fn write_camera(l: &mut Layer, pl: &Placed) {
    l.auto_orient = AutoOrient::Off;
    let n = pl.times.len().min(pl.solve.frames.len());
    let mut pos = Vec::with_capacity(n);
    let mut ori = Vec::with_capacity(n);
    let mut zoom = Vec::with_capacity(n);
    let mut prev: Option<V3> = None;
    for k in 0..n {
        let (p, o, z) = pl.camera(k);
        // Unwrap the angles so per-frame keys interpolate the short way.
        let o = match prev {
            None => norm_angles(o),
            Some(q) => [0, 1, 2].map(|i| q[i] + (o[i] - q[i] + 180.0).rem_euclid(360.0) - 180.0),
        };
        prev = Some(o);
        let t = l.layer_time(pl.times[k]);
        pos.push((t, Value::Vec3(p)));
        ori.push((t, Value::Vec3(o)));
        zoom.push((t, Value::Scalar(z)));
    }
    key_all(l, "transform/position", &pos);
    key_all(l, "transform/orientation", &ori);
    let variable = pl.solve.frames.iter().any(|f| (f.focal - pl.solve.frames[0].focal).abs() > 1e-6 * f.focal);
    if variable {
        key_all(l, "cameraOptions/zoom", &zoom);
    } else if let Some((_, z)) = zoom.first()
        && let Some(pr) = l.props.prop_mut("cameraOptions/zoom")
    {
        pr.keys.clear();
        pr.value = z.clone();
    }
    if let Some((_, z)) = zoom.first()
        && let Some(pr) = l.props.prop_mut("cameraOptions/focusDistance")
    {
        pr.value = z.clone();
    }
    // A one-node camera has no Point of Interest.
    if let Some(tr) = l.props.sub_mut("transform") {
        tr.children.retain(|c| !matches!(c, effectcraft_project::Node::Prop(p) if p.match_id == "poi"));
    }
    for rot in ["transform/rotationX", "transform/rotationY", "transform/rotation"] {
        if let Some(pr) = l.props.prop_mut(rot) {
            pr.keys.clear();
            pr.value = Value::Scalar(0.0);
        }
    }
}

/// The target for `ids` on frame `k`: when fewer than three points are given, the nearest
/// solved neighbours (in 3D) complete the plane.
pub fn target_for(solve: &CameraSolve, ids: &[u32], k: usize) -> Option<Target> {
    let mut use_ids: Vec<u32> = ids.iter().copied().filter(|i| solve.point(*i).is_some()).collect();
    if use_ids.is_empty() {
        return None;
    }
    if use_ids.len() < 3 {
        let c = solve.point(use_ids[0])?.pos;
        let mut others: Vec<(f64, u32)> = solve.visible(k).filter(|p| !use_ids.contains(&p.id)).map(|p| (linalg::norm(linalg::sub(p.pos, c)), p.id)).collect();
        others.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, id) in others.into_iter().take(6usize.saturating_sub(use_ids.len())) {
            use_ids.push(id);
        }
        let mut t = solve.target(&use_ids, k)?;
        // The target sits on the chosen point(s).
        let pts: Vec<V3> = ids.iter().filter_map(|i| solve.point(*i)).map(|p| p.pos).collect();
        t.center = linalg::scale(pts.iter().fold([0.0; 3], |a, b| linalg::add(a, *b)), 1.0 / pts.len() as f64);
        return Some(t);
    }
    solve.target(&use_ids, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_orientation_faces_the_normal() {
        let n = linalg::normalize([0.2, -0.9, -0.3]);
        let o = plane_orientation(n, [1.0, 0.0, 0.0]);
        let m = linalg::from_euler_xyz(o);
        // Layer −z (its front) is the normal.
        let front = linalg::mv(&m, [0.0, 0.0, -1.0]);
        assert!(linalg::norm(linalg::sub(front, n)) < 1e-9);
    }
}
