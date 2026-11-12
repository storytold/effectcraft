//! Mask tracking runtime (Layer ▸ Mask ▸ Track Mask, the Tracker panel in mask mode): follow
//! the pixels inside a mask from frame to frame and key its Mask Path on every analysed frame.
//!
//! Analysis runs on a background thread (progress, cancel; blocking with `wait` and on wasm32)
//! like the point tracker, over the layer's source frames; one undo step covers it. The
//! algorithm is `effectcraft_track::mask` (KLT features inside the mask + a RANSAC fit of the
//! chosen motion model), and every frame's motion is applied to the mask's vertices and
//! tangents, so the shape's Bezier structure is kept.
//!
//! The two **Face Tracking** methods use `effectcraft_track::face` instead: the mask (drawn
//! around a face) seeds the tracker, and every frame's face outline becomes the Mask Path
//! (*Outline Only*); *Detailed Features* also keys the facial landmarks into a **Face Track
//! Points** effect on the layer (created on first use), from which Extract & Copy Face
//! Measurements (`track.extractFaceMeasurements`) derives a keyed **Face Measurements** effect.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_keyframe::{Keyframe, ShapePath, Value};
use effectcraft_project::{ItemId, LayerId, Project, Uid};
use effectcraft_render::{ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use effectcraft_track as trk;
use effectcraft_track::fit::Model;
use serde::{Deserialize, Serialize};

use crate::tracking::{Direction, TrackProgress, lock, source_frame};
use crate::{Event, Session};

/// Tracker panel ▸ Method (mask tracking).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaskMethod {
    Position,
    PositionScale,
    #[default]
    PositionScaleRotation,
    PositionScaleRotationSkew,
    Perspective,
    /// Face Tracking (Outline Only).
    FaceOutline,
    /// Face Tracking (Detailed Features).
    FaceDetailed,
}

impl MaskMethod {
    pub const ALL: [MaskMethod; 7] = [
        MaskMethod::Position,
        MaskMethod::PositionScale,
        MaskMethod::PositionScaleRotation,
        MaskMethod::PositionScaleRotationSkew,
        MaskMethod::Perspective,
        MaskMethod::FaceOutline,
        MaskMethod::FaceDetailed,
    ];
    pub fn label(self) -> &'static str {
        match self {
            MaskMethod::Position => "Position",
            MaskMethod::PositionScale => "Position & Scale",
            MaskMethod::PositionScaleRotation => "Position, Scale & Rotation",
            MaskMethod::PositionScaleRotationSkew => "Position, Scale, Rotation & Skew",
            MaskMethod::Perspective => "Perspective",
            MaskMethod::FaceOutline => "Face Tracking (Outline Only)",
            MaskMethod::FaceDetailed => "Face Tracking (Detailed Features)",
        }
    }
    /// One of the Face Tracking methods.
    pub fn is_face(self) -> bool {
        matches!(self, MaskMethod::FaceOutline | MaskMethod::FaceDetailed)
    }
    pub fn id(self) -> &'static str {
        match self {
            MaskMethod::Position => "position",
            MaskMethod::PositionScale => "positionScale",
            MaskMethod::PositionScaleRotation => "positionScaleRotation",
            MaskMethod::PositionScaleRotationSkew => "positionScaleRotationSkew",
            MaskMethod::Perspective => "perspective",
            MaskMethod::FaceOutline => "faceOutline",
            MaskMethod::FaceDetailed => "faceDetailed",
        }
    }
    pub fn from_name(s: &str) -> Option<MaskMethod> {
        let n: String = s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        MaskMethod::ALL
            .into_iter()
            .find(|m| m.id().to_ascii_lowercase() == n || m.label().chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase() == n)
    }
    pub fn model(self) -> Model {
        match self {
            MaskMethod::Position => Model::Translation,
            MaskMethod::PositionScale => Model::TranslationScale,
            MaskMethod::PositionScaleRotation => Model::Similarity,
            MaskMethod::PositionScaleRotationSkew => Model::Affine,
            MaskMethod::Perspective => Model::Homography,
            MaskMethod::FaceOutline | MaskMethod::FaceDetailed => Model::Similarity,
        }
    }
}

/// A closed polyline through a Bezier path (`steps` samples per segment), in layer pixels.
pub fn flatten(p: &ShapePath, steps: usize) -> Vec<[f64; 2]> {
    let n = p.len();
    if n == 0 {
        return vec![];
    }
    let segs = if p.closed { n } else { n.saturating_sub(1) };
    let mut out = Vec::with_capacity(segs * steps + 1);
    for i in 0..segs {
        let j = (i + 1) % n;
        let (a, b) = (p.vertices[i], p.vertices[j]);
        let o = p.out_tangents.get(i).copied().unwrap_or([0.0; 2]);
        let it = p.in_tangents.get(j).copied().unwrap_or([0.0; 2]);
        let c = [a, [a[0] + o[0], a[1] + o[1]], [b[0] + it[0], b[1] + it[1]], b];
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let u = 1.0 - t;
            let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            out.push([0, 1].map(|d| w[0] * c[0][d] + w[1] * c[1][d] + w[2] * c[2][d] + w[3] * c[3][d]));
        }
    }
    if !p.closed {
        out.push(p.vertices[n - 1]);
    }
    out
}

/// A path moved by a frame-to-frame motion (vertices and tangent handles).
pub fn transform_path(p: &ShapePath, h: &trk::Homography) -> ShapePath {
    ShapePath {
        vertices: p.vertices.iter().map(|v| h.apply(*v)).collect(),
        in_tangents: p.vertices.iter().zip(&p.in_tangents).map(|(v, t)| h.apply_tangent(*v, *t)).collect(),
        out_tangents: p.vertices.iter().zip(&p.out_tangents).map(|(v, t)| h.apply_tangent(*v, *t)).collect(),
        closed: p.closed,
        feather: p.feather.clone(),
    }
}

/// A closed, smooth mask path through a face outline polygon: `n` vertices with Catmull-Rom
/// tangents. `like` lends its feather points.
pub fn outline_path(poly: &[[f64; 2]], n: usize, like: &ShapePath) -> ShapePath {
    let m = poly.len();
    let n = n.clamp(3, m.max(3));
    let v: Vec<[f64; 2]> = (0..n).map(|i| poly[(i * m / n) % m.max(1)]).collect();
    let tan: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let (a, b) = (v[(i + n - 1) % n], v[(i + 1) % n]);
            [(b[0] - a[0]) / 6.0, (b[1] - a[1]) / 6.0]
        })
        .collect();
    ShapePath {
        vertices: v,
        in_tangents: tan.iter().map(|t| [-t[0], -t[1]]).collect(),
        out_tangents: tan,
        closed: true,
        feather: if like.len() == n { like.feather.clone() } else { vec![] },
    }
}

/// One tracked frame: (layer time, comp time, mask path, face landmarks for Detailed Features).
type MaskFrame = (Tick, Tick, ShapePath, Option<[[f64; 2]; trk::face::N]>);

#[derive(Default)]
pub struct MaskTrackShared {
    pub cancel: AtomicBool,
    pub state: Mutex<TrackProgress>,
    frames: Mutex<Vec<MaskFrame>>,
}

/// A running (or finished, not yet polled) mask track.
pub struct MaskTrackJob {
    pub shared: Arc<MaskTrackShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub mask: Uid,
    pub direction: Direction,
}

impl MaskTrackJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn progress(&self) -> TrackProgress {
        lock(&self.shared.state).clone()
    }
}

/// What the worker needs, detached from the session.
pub(crate) struct MaskWork {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub cache: Arc<LayerCache>,
    pub comp: ItemId,
    pub layer: LayerId,
    /// The mask path on the first frame.
    pub path: ShapePath,
    pub method: MaskMethod,
    /// Comp times; the first is the start frame.
    pub times: Vec<Tick>,
    /// Face tracking's trained model (Settings ▸ Face Tracking), if one is in use.
    pub face_model: Option<Arc<dyn effectcraft_segment::face::FaceModel>>,
}

async fn run_work(w: MaskWork, shared: &MaskTrackShared) {
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
    lock(&shared.state).total = w.times.len().saturating_sub(1) as u64;
    let Some(first) = frame_at(w.times[0]).await else { return fail("the layer has no pixels to track") };
    if w.method.is_face() {
        return run_face(&w, shared, first, &frame_at, accel.is_some(), layer, t0).await;
    }
    let mut tracker = trk::mask::MaskTracker::new(w.method.model(), flatten(&w.path, 8), &trk::Frame { img: &first.0, offset: first.1 });
    let mut path = w.path.clone();
    lock(&shared.frames).push((layer.layer_time(w.times[0]), w.times[0], path.clone(), None));
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
        let (res, nf) = crate::offload::join_fetch(
            accel.is_some(),
            || tracker.step(&trk::Frame { img: &cur.0, offset: cur.1 }),
            || async { if k + 1 < w.times.len() { frame_at(w.times[k + 1]).await } else { None } },
        )
        .await;
        next = nf;
        let Some(step) = res else {
            lock(&shared.state).error = Some(format!("lost the mask's pixels at {:.3} s (too little texture inside the mask)", w.times[k].seconds()));
            break;
        };
        path = transform_path(&path, &step.motion);
        lock(&shared.frames).push((layer.layer_time(w.times[k]), w.times[k], path.clone(), None));
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

/// Face tracking: the face outline keys the mask; Detailed Features adds the landmarks.
/// `deferred`: frames render in passes (awaited one after the other).
async fn run_face<F: std::future::Future<Output = Option<(Arc<effectcraft_raster::Image>, [f64; 2])>>>(
    w: &MaskWork,
    shared: &MaskTrackShared,
    first: (Arc<effectcraft_raster::Image>, [f64; 2]),
    frame_at: &(impl Fn(Tick) -> F + Sync),
    deferred: bool,
    layer: &effectcraft_project::Layer,
    t0: web_time::Instant,
) {
    let detailed = w.method == MaskMethod::FaceDetailed;
    let b = trk::mask::bounds(&flatten(&w.path, 8));
    let Some((mut tracker, fit)) = trk::face::FaceTracker::new_with(&trk::Frame { img: &first.0, offset: first.1 }, b, w.face_model.clone()) else {
        let mut s = lock(&shared.state);
        s.error = Some("no face found inside the mask (draw the mask around a face)".into());
        s.finished = true;
        return;
    };
    let key = |fit: &trk::face::FaceFit| (outline_path(&fit.outline, 16, &w.path), detailed.then_some(fit.landmarks));
    let (path, pts) = key(&fit);
    lock(&shared.frames).push((layer.layer_time(w.times[0]), w.times[0], path, pts));
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
        let (res, nf) = crate::offload::join_fetch(
            deferred,
            || tracker.step(&trk::Frame { img: &cur.0, offset: cur.1 }),
            || async { if k + 1 < w.times.len() { frame_at(w.times[k + 1]).await } else { None } },
        )
        .await;
        next = nf;
        let Some(fit) = res else {
            lock(&shared.state).error = Some(format!("lost the face at {:.3} s", w.times[k].seconds()));
            break;
        };
        let (path, pts) = key(&fit);
        lock(&shared.frames).push((layer.layer_time(w.times[k]), w.times[k], path, pts));
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

/// The layer's Face Track Points effect (created when missing). Returns its uid.
pub(crate) fn face_points_group(p: &mut Project, comp: ItemId, layer: LayerId) -> Option<Uid> {
    face_effect(p, comp, layer, effectcraft_effects::face_track::POINTS_ID)
}

/// The layer's effect `id`, applied when missing (data effects of face tracking).
pub(crate) fn face_effect(p: &mut Project, comp: ItemId, layer: LayerId, id: &str) -> Option<Uid> {
    let spec = effectcraft_effects::find(id)?;
    let l = p.comp(comp)?.layer(layer)?;
    if let Some(g) = l.effects().and_then(|fx| fx.groups().find(|g| g.match_id == spec.id)) {
        return Some(g.uid);
    }
    let (w, h) = effectcraft_render::source_size(p, l);
    let size = if w == 0 { p.comp(comp).map(|c| [c.width as f64, c.height as f64]).unwrap_or([1920.0, 1080.0]) } else { [w as f64, h as f64] };
    let mut next = p.next_id;
    let g = effectcraft_effects::instantiate(spec, &mut effectcraft_project::build::Ids(&mut next), spec.name, size);
    let uid = g.uid;
    let fx = p.comp_mut(comp)?.layer_mut(layer)?.props.sub_mut("effects")?;
    fx.children.push(g.into());
    p.next_id = next;
    Some(uid)
}

fn write_frames(p: &mut Project, job: &MaskTrackJob, frames: &[MaskFrame]) {
    let points = if frames.iter().any(|f| f.3.is_some()) { face_points_group(p, job.comp, job.layer) } else { None };
    let Some(layer) = p.comp_mut(job.comp).and_then(|c| c.layer_mut(job.layer)) else { return };
    if let Some(g) = layer.props.find_group_mut(job.mask)
        && let Some(pr) = g.get_mut("path")
    {
        for (lt, _, path, _) in frames {
            effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(*lt, Value::Path(path.clone())));
        }
    }
    if let Some(g) = points.and_then(|u| layer.props.find_group_mut(u)) {
        for (lt, _, _, pts) in frames {
            let Some(pts) = pts else { continue };
            for (i, (id, _, _)) in trk::face::LANDMARKS.iter().enumerate() {
                if let Some(pr) = g.get_mut(id) {
                    effectcraft_keyframe::set_key(&mut pr.keys, Keyframe::new(*lt, Value::Vec2(pts[i])));
                }
            }
        }
    }
}

impl Session {
    /// Whether a mask track is running.
    pub fn is_mask_tracking(&self) -> bool {
        self.mask_job.as_ref().is_some_and(|j| !j.is_finished()) || self.offloaded(crate::offload::JobKind::MaskTrack).is_some()
    }

    /// Live progress of the running (or just finished) mask track.
    pub fn mask_track_progress(&self) -> Option<TrackProgress> {
        self.mask_job.as_ref().map(|j| j.progress()).or_else(|| self.offloaded(crate::offload::JobKind::MaskTrack).map(|j| j.progress.track()))
    }

    /// Cancel the running mask track (frames tracked so far are kept).
    pub fn stop_mask_track(&mut self) -> bool {
        if self.cancel_offloaded(crate::offload::JobKind::MaskTrack) {
            return true;
        }
        match &self.mask_job {
            Some(j) if !j.is_finished() => {
                j.shared.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn start_mask_track(&mut self, work: MaskWork, mask: Uid, direction: Direction, wait: bool) -> Result<(), String> {
        if self.is_mask_tracking() || self.is_tracking() {
            return Err("a track analysis is already running".into());
        }
        self.poll_mask_track();
        if !wait && self.offloads() && !work.method.is_face() {
            let MaskWork { comp, layer, path, method, times, .. } = work;
            return self.offload_analysis(crate::offload::WorkerJob::MaskTrack { comp, layer, mask, direction, path, method, times });
        }
        let wait = wait || cfg!(target_arch = "wasm32");
        let levels = self.prefs.general.undo_levels as usize;
        self.history.record("Track Mask", self.project.clone(), levels);
        let shared = Arc::new(MaskTrackShared::default());
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
                    .name("mask-track".into())
                    .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh)))
                    .map_err(|e| e.to_string())?,
            )
        };
        self.mask_job = Some(MaskTrackJob { shared, thread, comp, layer, mask, direction });
        self.poll_mask_track();
        Ok(())
    }

    /// Key newly tracked frames and move the CTI with them; drop the job once it finished.
    /// Frontends call this every frame while tracking.
    pub fn poll_mask_track(&mut self) -> bool {
        let Some(job) = &self.mask_job else { return false };
        let frames = std::mem::take(&mut *lock(&job.shared.frames));
        let finished = job.is_finished();
        let changed = !frames.is_empty();
        if changed {
            write_frames(Arc::make_mut(&mut self.project), job, &frames);
            if self.state.active_comp == Some(job.comp)
                && let Some(last) = frames.last()
            {
                self.state.times.insert(job.comp, last.1);
            }
            self.bump();
        }
        if finished && let Some(mut job) = self.mask_job.take() {
            if let Some(t) = job.thread.take() {
                let _ = t.join();
            }
            let st = job.progress();
            let msg = match (&st.error, st.cancelled) {
                (Some(e), _) => format!("Mask tracking stopped: {e}"),
                (None, true) => format!("Mask tracking stopped after {} frame(s)", st.done),
                _ => format!("Tracked the mask over {} frame(s) in {:.1} s", st.done, st.elapsed),
            };
            self.events.push(Event::Toast { message: msg, error: st.error.is_some() });
        }
        changed || finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_parse_and_paths_transform() {
        assert_eq!(MaskMethod::from_name("positionScaleRotationSkew"), Some(MaskMethod::PositionScaleRotationSkew));
        assert_eq!(MaskMethod::from_name("Position & Scale"), Some(MaskMethod::PositionScale));
        assert_eq!(MaskMethod::from_name("perspective"), Some(MaskMethod::Perspective));
        let p = ShapePath::ellipse([50.0, 40.0], 40.0, 20.0);
        let poly = flatten(&p, 8);
        assert_eq!(poly.len(), p.len() * 8);
        assert!(poly.iter().all(|q| ((q[0] - 50.0) / 20.0).powi(2) + ((q[1] - 40.0) / 10.0).powi(2) < 1.01));
        let h = trk::Homography([[0.0, -2.0, 10.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        let q = transform_path(&p, &h);
        for i in 0..p.len() {
            let v = h.apply(p.vertices[i]);
            assert!((q.vertices[i][0] - v[0]).abs() < 1e-9);
            let t = p.out_tangents[i];
            assert!((q.out_tangents[i][0] + 2.0 * t[1]).abs() < 1e-9 && (q.out_tangents[i][1] - 2.0 * t[0]).abs() < 1e-9);
        }
    }
}
