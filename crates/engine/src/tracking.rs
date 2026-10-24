//! Motion tracking runtime: the analysis job (background thread with progress and cancel, or
//! blocking for the CLI / agents / wasm), writing track results into the tracker's Track Point
//! properties, and applying tracks (Transform, Stabilize, Corner Pin) to layers.
//!
//! The algorithms are in `effectcraft-track`; the property layout in
//! `effectcraft_project::tracking`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_geom::{Mat3, vec2};
use effectcraft_project::tracking::{LowConfidence, TrackChannel, TrackKind, TrackerOptions, TrackerSettings};
use effectcraft_project::{Comp, ItemId, Keyframe, Layer, LayerId, Project, PropGroup, Uid, Value};
use effectcraft_raster::Image;
use effectcraft_render::{EvalCtx, ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use effectcraft_track as trk;
use serde::{Deserialize, Serialize};

use crate::{Event, Session};

/// Tracker panel ▸ Analyze buttons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    #[default]
    Forward,
    Backward,
    FrameForward,
    FrameBackward,
}

impl Direction {
    pub fn from_name(s: &str) -> Option<Direction> {
        let n = s.to_ascii_lowercase().replace([' ', '_', '-'], "");
        Some(match n.as_str() {
            "forward" | "fwd" => Direction::Forward,
            "backward" | "back" | "bwd" => Direction::Backward,
            "frameforward" | "oneframeforward" | "1frameforward" => Direction::FrameForward,
            "framebackward" | "oneframebackward" | "1framebackward" => Direction::FrameBackward,
            _ => return None,
        })
    }
    pub fn forward(self) -> bool {
        matches!(self, Direction::Forward | Direction::FrameForward)
    }
}

/// Track options for the algorithms.
pub fn track_options(o: &TrackerOptions) -> trk::TrackOptions {
    trk::TrackOptions {
        channel: match o.channel {
            TrackChannel::Rgb => trk::Channel::Rgb,
            TrackChannel::Luminance => trk::Channel::Luminance,
            TrackChannel::Saturation => trk::Channel::Saturation,
        },
        blur: o.blur,
        enhance: o.enhance,
        subpixel: o.subpixel,
        adapt_every_frame: o.adapt_every_frame,
        threshold: o.threshold,
        action: match o.action {
            LowConfidence::Continue => trk::ConfidenceAction::Continue,
            LowConfidence::Stop => trk::ConfidenceAction::Stop,
            LowConfidence::Extrapolate => trk::ConfidenceAction::Extrapolate,
            LowConfidence::Adapt => trk::ConfidenceAction::Adapt,
        },
        track_shape: o.track_shape,
    }
}

/// A track point's regions and attach offset read from its properties at a layer time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointValues {
    pub uid: Uid,
    pub spec: trk::PointSpec,
    pub attach_offset: [f64; 2],
}

pub fn point_values(tp: &PropGroup, lt: Tick) -> PointValues {
    let v = |m: &str, d: [f64; 2]| tp.get(m).map(|p| p.value_at(lt).as_vec2()).unwrap_or(d);
    PointValues {
        uid: tp.uid,
        spec: trk::PointSpec {
            center: v("featureCenter", [0.0; 2]),
            feature_size: v("featureSize", [32.0; 2]),
            search_offset: v("searchOffset", [0.0; 2]),
            search_size: v("searchSize", [64.0; 2]),
        },
        attach_offset: v("attachPointOffset", [0.0; 2]),
    }
}

/// Attach points of a frame from the feature centres, given the start frame's centres and attach
/// points: two-point transforms rotate and scale the offsets with the track, Parallel Corner Pin
/// maps them through the 3-point affine, Perspective Corner Pin through the 4-point homography.
pub fn attach_points(kind: TrackKind, rot_scale: bool, ref_c: &[[f64; 2]], ref_a: &[[f64; 2]], cur: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let translate = || cur.iter().zip(ref_c.iter().zip(ref_a)).map(|(c, (r, a))| [c[0] + a[0] - r[0], c[1] + a[1] - r[1]]).collect::<Vec<_>>();
    let through = |h: Option<trk::Homography>| match h {
        Some(h) => ref_a.iter().map(|a| h.apply(*a)).collect(),
        None => translate(),
    };
    match kind {
        TrackKind::Perspective if cur.len() >= 4 => {
            let h = trk::Homography::from_points([ref_c[0], ref_c[1], ref_c[2], ref_c[3]], [cur[0], cur[1], cur[2], cur[3]]);
            through(h)
        }
        TrackKind::Affine if cur.len() >= 3 => through(trk::Homography::affine_from_points([ref_c[0], ref_c[1], ref_c[2]], [cur[0], cur[1], cur[2]])),
        _ if rot_scale && cur.len() >= 2 => {
            let Some(s) = trk::Homography::similarity_from_points([ref_c[0], ref_c[1]], [cur[0], cur[1]]) else { return translate() };
            let m = s.0;
            cur.iter()
                .zip(ref_c.iter().zip(ref_a))
                .map(|(c, (r, a))| {
                    let d = [a[0] - r[0], a[1] - r[1]];
                    [c[0] + m[0][0] * d[0] + m[0][1] * d[1], c[1] + m[1][0] * d[0] + m[1][1] * d[1]]
                })
                .collect()
        }
        _ => translate(),
    }
}

/// Live analysis progress (serde for agents / the control channel).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackProgress {
    pub done: u64,
    pub total: u64,
    pub elapsed: f64,
    /// Analysed frames per second.
    pub fps: f64,
    pub finished: bool,
    pub cancelled: bool,
    /// Stopped by "If Confidence is Below … ▸ Stop Tracking".
    pub low_confidence_stop: bool,
    pub error: Option<String>,
    /// Comp time (seconds) of the last analysed frame.
    pub time: f64,
}

/// One analysed frame.
#[derive(Clone, Debug)]
pub struct FrameResult {
    pub comp_time: Tick,
    pub layer_time: Tick,
    /// Per point: (point group uid, result, attach point).
    pub points: Vec<(Uid, trk::PointResult, [f64; 2])>,
}

#[derive(Default)]
pub struct TrackShared {
    pub cancel: AtomicBool,
    pub state: Mutex<TrackProgress>,
    pub frames: Mutex<Vec<FrameResult>>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A running (or finished, not yet polled) analysis.
pub struct TrackJob {
    pub shared: Arc<TrackShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub tracker: Uid,
    pub direction: Direction,
}

impl TrackJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn progress(&self) -> TrackProgress {
        lock(&self.shared.state).clone()
    }
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

/// What the worker needs, detached from the session.
pub(crate) struct Work {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub cache: Arc<LayerCache>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub settings: TrackerSettings,
    pub points: Vec<PointValues>,
    /// Comp times to analyse; the first is the start frame.
    pub times: Vec<Tick>,
}

/// The layer's source frame at comp time `t` at 100 % (layer pixels + offset = image pixels).
/// Footage at its native size is used straight from the footage source (no copy).
pub(crate) fn source_frame(
    r: &Renderer,
    project: &Project,
    cid: ItemId,
    comp: &Comp,
    layer: &Layer,
    t: Tick,
    expr: Option<&dyn ExprHost>,
) -> Option<(Arc<Image>, [f64; 2])> {
    let ctx = EvalCtx { project, comp_id: cid, comp, time: t, expr, footage: None };
    if let effectcraft_project::LayerSource::Footage { item } = &layer.source
        && let Some(effectcraft_project::ItemKind::Footage(f)) = project.item(*item).map(|i| &i.kind)
        && f.has_video
        && let Some(img) = r.footage.frame(*item, f, ctx.source_time(layer))
        && img.width == f.width
        && img.height == f.height
    {
        return Some((img, [0.0; 2]));
    }
    let buf = r.layer_source(&ctx, layer)?;
    if (buf.scale - 1.0).abs() > 1e-9 && buf.scale > 0.0 {
        // Footage decoded at another size: bring it to layer pixels.
        let w = (buf.img.width as f64 / buf.scale).round().max(1.0) as u32;
        let h = (buf.img.height as f64 / buf.scale).round().max(1.0) as u32;
        let off = [buf.offset[0] / buf.scale, buf.offset[1] / buf.scale];
        return Some((Arc::new(effectcraft_raster::resample(&buf.img, w, h)), off));
    }
    Some((Arc::new(buf.img), buf.offset))
}

pub(crate) async fn run_work(w: Work, shared: &TrackShared) {
    let t0 = web_time::Instant::now();
    // A browser job worker's GPU (deferred readbacks): frames render in passes.
    let accel = crate::offload::job_accel();
    let accel = accel.as_deref();
    let fail = |msg: &str| {
        let mut s = lock(&shared.state);
        s.error = Some(msg.to_string());
        s.finished = true;
    };
    let Some(comp) = w.project.comp(w.comp) else { return fail("composition missing") };
    let Some(layer) = comp.layer(w.layer) else { return fail("layer missing") };
    let mut r = Renderer::new(&w.project, w.footage.as_ref(), crate::offload::analysis_opts(accel));
    r.expr = w.expr.as_deref();
    r.cache = Some(&w.cache);
    let r = &r;
    let wr = &w;
    let frame_at =
        |t: Tick| crate::offload::in_passes(accel, move |a| source_frame(&r.with_accel(a), &wr.project, wr.comp, comp, layer, t, wr.expr.as_deref()));
    {
        let mut s = lock(&shared.state);
        s.total = w.times.len().saturating_sub(1) as u64;
    }
    let Some(first) = frame_at(w.times[0]).await else { return fail("the layer has no pixels to track") };
    let specs: Vec<trk::PointSpec> = w.points.iter().map(|p| p.spec).collect();
    let mut tracker = trk::Tracker::new(track_options(&w.settings.options), &specs, &trk::Frame { img: &first.0, offset: first.1 });
    let ref_c: Vec<[f64; 2]> = specs.iter().map(|s| s.center).collect();
    let ref_a: Vec<[f64; 2]> = w.points.iter().map(|p| [p.spec.center[0] + p.attach_offset[0], p.spec.center[1] + p.attach_offset[1]]).collect();
    let rot_scale = w.settings.rotation || w.settings.scale;
    // The start frame keeps its regions and gets keys too.
    lock(&shared.frames).push(FrameResult {
        comp_time: w.times[0],
        layer_time: layer.layer_time(w.times[0]),
        points: w
            .points
            .iter()
            .zip(&ref_a)
            .map(|(p, a)| (p.uid, trk::PointResult { center: p.spec.center, confidence: 100.0, rotation: 0.0, scale: 1.0, status: trk::Status::Tracked }, *a))
            .collect(),
    });
    let mut next = if w.times.len() > 1 { frame_at(w.times[1]).await } else { None };
    for k in 1..w.times.len() {
        if shared.cancel.load(Ordering::Relaxed) {
            lock(&shared.state).cancelled = true;
            break;
        }
        let Some(cur) = next.take() else {
            lock(&shared.state).error = Some(format!("no frame at {:.3} s", w.times[k].seconds()));
            break;
        };
        // Track this frame while the next one renders.
        let (res, nf) = crate::offload::join_fetch(
            accel.is_some(),
            || tracker.step(&trk::Frame { img: &cur.0, offset: cur.1 }),
            || async { if k + 1 < w.times.len() { frame_at(w.times[k + 1]).await } else { None } },
        )
        .await;
        next = nf;
        if res.iter().any(|p| p.status == trk::Status::Stopped) {
            lock(&shared.state).low_confidence_stop = true;
            break;
        }
        let centers: Vec<[f64; 2]> = res.iter().map(|p| p.center).collect();
        let attach = attach_points(w.settings.kind, rot_scale, &ref_c, &ref_a, &centers);
        lock(&shared.frames).push(FrameResult {
            comp_time: w.times[k],
            layer_time: layer.layer_time(w.times[k]),
            points: w.points.iter().zip(res).zip(attach).map(|((p, r), a)| (p.uid, r, a)).collect(),
        });
        let mut s = lock(&shared.state);
        s.done = k as u64;
        s.elapsed = t0.elapsed().as_secs_f64();
        s.fps = if s.elapsed > 0.0 { k as f64 / s.elapsed } else { 0.0 };
        s.time = w.times[k].seconds();
        crate::offload::report(s.done, s.total);
    }
    let mut s = lock(&shared.state);
    s.elapsed = t0.elapsed().as_secs_f64();
    s.finished = true;
}

/// Set (or add) a key at layer time `t`.
pub(crate) fn key(g: &mut PropGroup, m: &str, t: Tick, v: Value) {
    if let Some(p) = g.get_mut(m) {
        effectcraft_keyframe::set_key(&mut p.keys, Keyframe::new(t, v));
    }
}

/// Write analysed frames into the tracker's points.
fn write_frames(p: &mut Project, job: &TrackJob, frames: &[FrameResult]) {
    let Some(comp) = p.comp_mut(job.comp) else { return };
    let Some(layer) = comp.layer_mut(job.layer) else { return };
    for f in frames {
        for (uid, r, a) in &f.points {
            let Some(tp) = layer.props.find_group_mut(*uid) else { continue };
            key(tp, "featureCenter", f.layer_time, Value::Vec2(r.center));
            key(tp, "confidence", f.layer_time, Value::Scalar(r.confidence));
            key(tp, "attachPoint", f.layer_time, Value::Vec2(*a));
        }
    }
}

impl Session {
    /// Whether a track analysis is running.
    pub fn is_tracking(&self) -> bool {
        self.track_job.as_ref().is_some_and(|j| !j.is_finished()) || self.offloaded(crate::offload::JobKind::Track).is_some()
    }

    /// Live progress of the running (or just finished) analysis.
    pub fn track_progress(&self) -> Option<TrackProgress> {
        self.track_job.as_ref().map(|j| j.progress()).or_else(|| self.offloaded(crate::offload::JobKind::Track).map(|j| j.progress.track()))
    }

    /// Cancel the running analysis (results so far are kept).
    pub fn stop_track(&mut self) -> bool {
        if self.cancel_offloaded(crate::offload::JobKind::Track) {
            return true;
        }
        match &self.track_job {
            Some(j) if !j.is_finished() => {
                j.cancel();
                true
            }
            _ => false,
        }
    }

    /// Start analysing. `wait`: block until done (wasm32 always does). One undo step covers the
    /// whole analysis.
    pub(crate) fn start_track(&mut self, work: Work, tracker: Uid, direction: Direction, wait: bool) -> Result<(), String> {
        if self.is_tracking() {
            return Err("a track analysis is already running".into());
        }
        self.poll_track();
        if !wait && self.offloads() {
            let Work { comp, layer, settings, points, times, .. } = work;
            return self.offload_analysis(crate::offload::WorkerJob::Track { comp, layer, tracker, direction, settings, points, times });
        }
        let wait = wait || cfg!(target_arch = "wasm32");
        let levels = self.prefs.general.undo_levels as usize;
        self.history.record("Analyze Track", self.project.clone(), levels);
        let shared = Arc::new(TrackShared::default());
        let (comp, layer) = (work.comp, work.layer);
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
                    .name("track-analyze".into())
                    .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh)))
                    .map_err(|e| e.to_string())?,
            )
        };
        self.track_job = Some(TrackJob { shared, thread, comp, layer, tracker, direction });
        self.poll_track();
        Ok(())
    }

    /// Write new results into the project and move the CTI to the last analysed frame; drop the
    /// job once it finished. Frontends call this every frame while tracking.
    pub fn poll_track(&mut self) -> bool {
        let Some(job) = &self.track_job else { return false };
        let frames = std::mem::take(&mut *lock(&job.shared.frames));
        let finished = job.is_finished();
        let changed = !frames.is_empty();
        if changed {
            write_frames(Arc::make_mut(&mut self.project), job, &frames);
            if self.state.active_comp == Some(job.comp)
                && let Some(last) = frames.last()
            {
                self.state.times.insert(job.comp, last.comp_time);
            }
            self.bump();
        }
        if finished && let Some(mut job) = self.track_job.take() {
            if let Some(t) = job.thread.take() {
                let _ = t.join();
            }
            let st = job.progress();
            let msg = match (&st.error, st.cancelled, st.low_confidence_stop) {
                (Some(e), _, _) => format!("Tracking stopped: {e}"),
                (None, true, _) => format!("Tracking stopped after {} frame(s)", st.done),
                (None, false, true) => format!("Tracking stopped: confidence below threshold after {} frame(s)", st.done),
                _ => format!("Tracked {} frame(s) in {:.1} s", st.done, st.elapsed),
            };
            self.events.push(Event::Toast { message: msg, error: st.error.is_some() });
        }
        changed || finished
    }
}

// ---------------------------------------------------------------- apply

/// Apply Dimensions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dims {
    #[default]
    XY,
    X,
    Y,
}

impl Dims {
    pub fn from_name(s: &str) -> Option<Dims> {
        match s.to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "xy" | "xandy" | "both" => Some(Dims::XY),
            "x" | "xonly" => Some(Dims::X),
            "y" | "yonly" => Some(Dims::Y),
            _ => None,
        }
    }
    fn mix(self, old: [f64; 3], new: [f64; 2]) -> [f64; 3] {
        match self {
            Dims::XY => [new[0], new[1], old[2]],
            Dims::X => [new[0], old[1], old[2]],
            Dims::Y => [old[0], new[1], old[2]],
        }
    }
}

fn apply_m(m: &Mat3, p: [f64; 2]) -> [f64; 2] {
    let q = m.apply(vec2(p[0], p[1]));
    [q.x, q.y]
}

/// Parent space of a layer (comp space when unparented), as a 2D matrix from comp space.
fn comp_to_parent(ctx: &EvalCtx, layer: &Layer) -> Mat3 {
    match layer.parent.and_then(|p| ctx.comp.layer(p)) {
        Some(p) => ctx.layer_to_comp(p).0.inverse().unwrap_or(Mat3::IDENTITY),
        None => Mat3::IDENTITY,
    }
}

/// Per analysed time: (source layer time, attach points in source layer space).
pub fn attach_track(tracker: &PropGroup) -> Vec<(Tick, Vec<[f64; 2]>)> {
    let pts: Vec<&PropGroup> = tracker.track_points().collect();
    let Some(first) = pts.first().and_then(|g| g.get("attachPoint")) else { return vec![] };
    first.keys.iter().map(|k| (k.time, pts.iter().map(|g| g.get("attachPoint").map(|p| p.value_at(k.time).as_vec2()).unwrap_or([0.0; 2])).collect())).collect()
}

/// Apply a tracker (Tracker panel ▸ Apply). Returns the number of keyframed frames.
pub fn apply(p: &mut Project, cid: ItemId, src: LayerId, tracker: Uid, dims: Dims) -> Result<usize, String> {
    let comp = p.comp(cid).ok_or("no composition")?.clone();
    let source = comp.layer(src).ok_or("no source layer")?.clone();
    let (tg, settings) = source.tracker(tracker).ok_or("no such tracker")?;
    let settings = settings.clone();
    let track = attach_track(tg);
    if track.is_empty() {
        return Err("analyze the track first".into());
    }
    let target_id = match settings.kind {
        TrackKind::Stabilize => src,
        TrackKind::Raw => return Err("a Raw track has nothing to apply".into()),
        _ => settings.target.ok_or("set a Motion Target first (Edit Target…)")?,
    };
    let target = comp.layer(target_id).ok_or("the motion target layer is gone")?.clone();
    let ctx0 = EvalCtx::new(p, cid, &comp, Tick::ZERO);
    let t_first = source.comp_time(track[0].0);
    let rot_scale = settings.point_count() >= 2 && track[0].1.len() >= 2;
    let (ang0, len0) = if rot_scale { effectcraft_track::angle_and_length(track[0].1[0], track[0].1[1]) } else { (0.0, 1.0) };
    // (target layer time, values) to write.
    let mut pos_keys = vec![];
    let mut anchor_keys = vec![];
    let mut rot_keys = vec![];
    let mut scale_keys = vec![];
    let mut corner_keys = vec![];
    let ttr = target.transform().ok_or("the target has no transform")?;
    let c0 = ctx0.at(t_first);
    let rot0 = c0.f(&target, ttr, "rotation", 0.0);
    let scale0 = c0.v3(&target, ttr, "scale", [100.0; 3]);
    let mut prev_ang = 0.0;
    // Stabilize: the feature stays where it was on the first tracked frame.
    let stab_pos = {
        let (m, _) = c0.layer_to_comp(&source);
        apply_m(&comp_to_parent(&c0, &source), apply_m(&m, track[0].1[0]))
    };
    for (lt, pts) in &track {
        let ct = source.comp_time(*lt);
        let ctx = ctx0.at(ct);
        let tt = target.layer_time(ct);
        let (ang, len) = if rot_scale { effectcraft_track::angle_and_length(pts[0], pts[1]) } else { (ang0, len0) };
        let dang = effectcraft_track::unwrap_degrees(prev_ang, ang - ang0);
        prev_ang = dang;
        let ratio = if len0 > 1e-9 { len / len0 } else { 1.0 };
        match settings.kind {
            TrackKind::Stabilize => {
                if settings.position {
                    let old = ctx.v3(&target, ttr, "anchor", [0.0; 3]);
                    anchor_keys.push((tt, dims.mix(old, pts[0])));
                    let oldp = ctx.position(&target, ttr);
                    pos_keys.push((tt, dims.mix(oldp, stab_pos)));
                }
                if settings.rotation && rot_scale {
                    rot_keys.push((tt, rot0 - dang));
                }
                if settings.scale && rot_scale {
                    scale_keys.push((tt, [scale0[0] / ratio, scale0[1] / ratio, scale0[2]]));
                }
            }
            TrackKind::Transform => {
                let (m, _) = ctx.layer_to_comp(&source);
                if settings.position {
                    let c = apply_m(&m, pts[0]);
                    let pp = apply_m(&comp_to_parent(&ctx, &target), c);
                    pos_keys.push((tt, dims.mix(ctx.position(&target, ttr), pp)));
                }
                if settings.rotation && rot_scale {
                    rot_keys.push((tt, rot0 + dang));
                }
                if settings.scale && rot_scale {
                    scale_keys.push((tt, [scale0[0] * ratio, scale0[1] * ratio, scale0[2]]));
                }
            }
            TrackKind::Affine | TrackKind::Perspective => {
                let (ms, _) = ctx.layer_to_comp(&source);
                // Corner Pin's corners are effect points: in the target's effect space.
                let inv = ctx.effect_to_comp(&target).inverse().ok_or("the target layer's transform is not invertible")?;
                let n = pts.len();
                if n < 3 {
                    return Err("corner pin tracks need 3 or 4 track points".into());
                }
                let lr =
                    if settings.kind == TrackKind::Affine || n < 4 { [pts[1][0] + pts[2][0] - pts[0][0], pts[1][1] + pts[2][1] - pts[0][1]] } else { pts[3] };
                // Track Points 1–4: upper left, upper right, lower left, lower right.
                let map = |q: [f64; 2]| apply_m(&inv, apply_m(&ms, q));
                corner_keys.push((tt, [map(pts[0]), map(pts[1]), map(pts[2]), map(lr)]));
            }
            TrackKind::Raw => {}
        }
    }
    let n = track.len();
    let size = effectcraft_render::effect_bounds(p, &comp, &target).0;
    let mut ids_next = p.next_id;
    let layer = p.comp_mut(cid).and_then(|c| c.layer_mut(target_id)).ok_or("no target layer")?;
    if !corner_keys.is_empty() {
        let spec = effectcraft_effects::find("ec.distort.cornerpin").ok_or("Corner Pin is not available")?;
        let fx = layer.props.sub_mut("effects").ok_or("the target layer can't have effects")?;
        let same = fx.groups().filter(|g| g.match_id == spec.id).count();
        let name = if same == 0 { spec.name.to_string() } else { format!("{} {}", spec.name, same + 1) };
        let mut g = effectcraft_effects::instantiate(spec, &mut effectcraft_project::build::Ids(&mut ids_next), &name, size);
        for (t, c) in &corner_keys {
            for (m, v) in ["ul", "ur", "ll", "lr"].iter().zip(c) {
                key(&mut g, m, *t, Value::Vec2(*v));
            }
        }
        fx.children.push(g.into());
    }
    let tr = layer.transform_mut().ok_or("no transform")?;
    for (t, v) in anchor_keys {
        key(tr, "anchor", t, Value::Vec3(v));
    }
    let separated = tr.get("positionX").is_some();
    for (t, v) in pos_keys {
        if separated {
            key(tr, "positionX", t, Value::Scalar(v[0]));
            key(tr, "positionY", t, Value::Scalar(v[1]));
        } else {
            key(tr, "position", t, Value::Vec3(v));
        }
    }
    for (t, v) in rot_keys {
        key(tr, "rotation", t, Value::Scalar(v));
    }
    for (t, v) in scale_keys {
        key(tr, "scale", t, Value::Vec3(v));
    }
    p.next_id = ids_next;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_points_follow_the_track() {
        let rc = [[0.0, 0.0], [10.0, 0.0]];
        let ra = [[0.0, 5.0], [10.0, 5.0]];
        // Rotated 90° and doubled.
        let cur = [[100.0, 100.0], [100.0, 120.0]];
        let a = attach_points(TrackKind::Transform, true, &rc, &ra, &cur);
        assert!((a[0][0] - 90.0).abs() < 1e-9 && (a[0][1] - 100.0).abs() < 1e-9, "{a:?}");
        let a = attach_points(TrackKind::Transform, false, &rc, &ra, &cur);
        assert_eq!(a[0], [100.0, 105.0]);
        let h = trk::Homography([[1.1, 0.1, 5.0], [0.05, 0.9, -3.0], [0.001, 0.0005, 1.0]]);
        let rc4 = [[0.0, 0.0], [100.0, 0.0], [0.0, 100.0], [100.0, 100.0]];
        let ra4 = [[10.0, 10.0], [90.0, 10.0], [10.0, 90.0], [90.0, 90.0]];
        let cur4: Vec<[f64; 2]> = rc4.iter().map(|p| h.apply(*p)).collect();
        let a = attach_points(TrackKind::Perspective, false, &rc4, &ra4, &cur4);
        for (got, want) in a.iter().zip(ra4.iter().map(|p| h.apply(*p))) {
            assert!((got[0] - want[0]).abs() < 1e-6 && (got[1] - want[1]).abs() < 1e-6);
        }
    }
}
