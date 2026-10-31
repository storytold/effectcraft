//! Warp Stabilizer runtime: the analysis job (`warp.analyze`) and keeping analyses valid.
//!
//! The analysis renders the layer's input to the effect (source → masks → the effects above
//! the Warp Stabilizer, `Renderer::layer_input`) for every frame between the layer's In and Out
//! points on a background thread, and feeds the frames to `effectcraft_track::stabilize`'s
//! analyzer (step 1 of 2, "Analyzing in background"); step 2 ("Stabilizing") derives the
//! stabilization plan once to validate and cache the result. When the job finishes, the analysis is
//! written into the effect's hidden Analysis parameter as one undo step, together with a key of
//! what it was made from (the layer's source, In/Out, start, stretch and Time Remap). Edits that
//! change those clear the analysis (`invalidate`, run after every edit) and queue the effect
//! for re-analysis, which frontends start with [`Session::poll_warp`] (After Effects re-analyses
//! automatically too).
//!
//! [`apply_to_transform`] (`warp.applyToTransform`) bakes an analysed stabilization into the
//! layer: Position / Rotation / Scale keyframes, or a keyframed Corner Pin where the Transform
//! can't carry it, plus a mask for the Crop framings.

use crate::offload::{JobKind, rate};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use effectcraft_effects::warp_stab::{self, ANALYSIS, ANALYSIS_KEY};
use effectcraft_geom::vec3;
use effectcraft_keyframe::{ShapePath, Value};
use effectcraft_project::{AutoOrient, GroupKind, ItemId, ItemKind, Layer, LayerId, LayerSource, MaskMode, Project, PropGroup, Uid};
use effectcraft_raster::Image;
use effectcraft_render::{EvalCtx, ExprHost, FootageSource, LayerCache, Renderer};
use effectcraft_time::Tick;
use effectcraft_track::Homography;
use effectcraft_track::stabilize::{AnalyzeOpts, Analyzer, Framing, Method, WarpAnalysis};
use serde::Serialize;

use crate::tracking::{key, lock};
use crate::{Event, Session};

/// Live analysis progress (serde for agents / the control channel).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarpProgress {
    /// 1 = analyzing, 2 = stabilizing.
    pub step: u8,
    pub done: u64,
    pub total: u64,
    pub elapsed: f64,
    pub fps: f64,
    pub finished: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

impl WarpProgress {
    /// The viewer banner text, as After Effects words it.
    pub fn banner(&self) -> String {
        match self.step {
            2 => "Stabilizing...".into(),
            _ => {
                let pc = (100 * self.done).checked_div(self.total).unwrap_or(0);
                format!("Analyzing in background (step 1 of 2): {pc}%")
            }
        }
    }
}

#[derive(Default)]
pub struct WarpShared {
    pub cancel: AtomicBool,
    pub state: Mutex<WarpProgress>,
    result: Mutex<Option<WarpAnalysis>>,
}

/// A running (or finished, not yet polled) Warp Stabilizer analysis.
pub struct WarpJob {
    pub shared: Arc<WarpShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub comp: ItemId,
    pub layer: LayerId,
    pub effect: Uid,
    /// The layer signature the analysis is made from.
    key: String,
}

impl WarpJob {
    pub fn is_finished(&self) -> bool {
        lock(&self.shared.state).finished
    }
    pub fn progress(&self) -> WarpProgress {
        lock(&self.shared.state).clone()
    }
}

/// What the worker needs, detached from the session.
pub(crate) struct WarpWork {
    pub project: Arc<Project>,
    pub footage: Arc<dyn FootageSource>,
    pub expr: Option<Arc<dyn ExprHost>>,
    pub cache: Arc<LayerCache>,
    pub comp: ItemId,
    pub layer: LayerId,
    /// Position of the effect in the layer's stack (the effects before it are analysed).
    pub effect_index: usize,
    pub size: [f64; 2],
    pub detailed: bool,
    /// Comp times of the analysed frames.
    pub times: Vec<Tick>,
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// What an analysis of `layer` depends on (its frames): source (and the footage item's
/// interpretation), In/Out, start time, stretch, Time Remap and the comp frame rate.
pub fn signature(p: &Project, comp: ItemId, layer: &Layer) -> String {
    let src = match &layer.source {
        LayerSource::Footage { item } | LayerSource::Solid { item } | LayerSource::Comp { item } => match p.item(*item).map(|i| &i.kind) {
            Some(ItemKind::Footage(f)) => format!("{f:?}"),
            Some(ItemKind::Solid(s)) => format!("{s:?}"),
            Some(ItemKind::Comp(c)) => format!("{}x{}@{:?}/{:?}", c.width, c.height, c.frame_rate, c.duration),
            _ => String::new(),
        },
        _ => String::new(),
    };
    let rate = p.comp(comp).map(|c| format!("{:?}", c.frame_rate)).unwrap_or_default();
    let s = format!(
        "{:?}|{src}|{:?}|{:?}|{:?}|{}|{:?}|{rate}",
        layer.source,
        layer.in_point,
        layer.out_point,
        layer.start_time,
        layer.stretch,
        layer.props.get("timeRemap")
    );
    format!("{:016x}", fnv(&s))
}

fn is_warp(g: &PropGroup) -> bool {
    matches!(&g.kind, GroupKind::Effect { effect } if effect == warp_stab::ID)
}

fn str_of(g: &PropGroup, m: &str) -> String {
    match g.get(m).map(|p| &p.value) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Clear analyses that no longer match their layer, after an edit. Returns the Warp
/// Stabilizers that need (re-)analysis: cleared ones and newly applied, unanalysed ones.
pub(crate) fn invalidate(before: &Project, after: &mut Project) -> Vec<(ItemId, LayerId, Uid)> {
    let mut todo = vec![];
    let mut clear = vec![];
    for (cid, it) in &after.items {
        let ItemKind::Comp(c) = &it.kind else { continue };
        for l in &c.layers {
            let Some(fx) = l.effects() else { continue };
            if !fx.groups().any(is_warp) {
                continue;
            }
            let sig = signature(after, *cid, l);
            let old_layer = before.comp(*cid).and_then(|bc| bc.layer(l.id));
            for g in fx.groups().filter(|g| is_warp(g)) {
                let analysed = !str_of(g, ANALYSIS).is_empty();
                if analysed && str_of(g, ANALYSIS_KEY) != sig {
                    clear.push((*cid, l.id, g.uid));
                    todo.push((*cid, l.id, g.uid));
                } else if !analysed && old_layer.and_then(|ol| ol.props.find_group(g.uid)).is_none() {
                    todo.push((*cid, l.id, g.uid));
                }
            }
        }
    }
    for (cid, lid, uid) in clear {
        if let Some(g) = after.comp_mut(cid).and_then(|c| c.layer_mut(lid)).and_then(|l| l.props.find_group_mut(uid)) {
            for m in [ANALYSIS, ANALYSIS_KEY] {
                if let Some(p) = g.get_mut(m) {
                    p.value = Value::Str(String::new());
                    p.keys.clear();
                }
            }
        }
    }
    todo
}

/// The layer's input to effect `effect_index` at comp time `t`, in layer pixels at 100 %.
fn input_frame(r: &Renderer, ctx: &EvalCtx, layer: &Layer, effect_index: usize) -> Option<(Arc<Image>, [f64; 2])> {
    let buf = r.layer_input(ctx, layer, effect_index)?;
    if (buf.scale - 1.0).abs() > 1e-9 && buf.scale > 0.0 {
        let w = (buf.img.width as f64 / buf.scale).round().max(1.0) as u32;
        let h = (buf.img.height as f64 / buf.scale).round().max(1.0) as u32;
        let off = [buf.offset[0] / buf.scale, buf.offset[1] / buf.scale];
        return Some((Arc::new(effectcraft_raster::resample(&buf.img, w, h)), off));
    }
    Some((Arc::new(buf.img.clone()), buf.offset))
}

async fn run_work(w: WarpWork, shared: &WarpShared) {
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
    let ctx0 = EvalCtx::new(&w.project, w.comp, comp, Tick::ZERO);
    let ctx0 = EvalCtx { expr: w.expr.as_deref(), ..ctx0 };
    let ctx0 = &ctx0;
    let frame_at = |t: Tick| crate::offload::in_passes(accel, move |a| input_frame(&r.with_accel(a), &ctx0.at(t), layer, w.effect_index));
    {
        let mut s = lock(&shared.state);
        s.step = 1;
        s.total = w.times.len() as u64;
    }
    let mut an = Analyzer::new(w.size, AnalyzeOpts { detailed: w.detailed });
    let mut next = frame_at(w.times[0]).await;
    for k in 0..w.times.len() {
        if shared.cancel.load(Ordering::Relaxed) {
            let mut s = lock(&shared.state);
            s.cancelled = true;
            s.finished = true;
            return;
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
    lock(&shared.state).step = 2;
    let lt0 = layer.layer_time(w.times[0]).seconds();
    let fd = if w.times.len() > 1 { layer.layer_time(w.times[1]).seconds() - lt0 } else { comp.frame_duration().seconds() };
    let a = an.finish(lt0, fd);
    // Plan from the stored JSON, exactly as rendering does, and retain it for the first render.
    if !warp_stab::prepare_default_plan(&a.to_json()) {
        return fail("the stabilization could not be computed");
    }
    *lock(&shared.result) = Some(a);
    let mut s = lock(&shared.state);
    s.elapsed = t0.elapsed().as_secs_f64();
    s.finished = true;
}

/// Frames to analyse: every comp frame from the layer's In to its Out point (inside the comp).
pub(crate) fn analysis_times(comp: &effectcraft_project::Comp, layer: &Layer) -> Vec<Tick> {
    let fd = comp.frame_duration();
    let lo = comp.frame_rate.snap_nearest(layer.in_point.max(Tick::ZERO));
    let hi = layer.out_point.min(comp.duration);
    let mut t = lo;
    let mut v = vec![];
    while t < hi {
        v.push(t);
        t += fd;
    }
    v
}

// ---------------------------------------------------------------- apply to transform

/// Where [`apply_to_transform`] put the stabilization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BakeTarget {
    /// Position, Rotation and Scale keyframes.
    Transform,
    /// A keyframed Corner Pin in the Warp Stabilizer's place in the effect stack.
    CornerPin,
}

/// What [`apply_to_transform`] did.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Baked {
    pub target: BakeTarget,
    /// Keyframed frames.
    pub frames: usize,
    /// Why the stabilization went to a Corner Pin instead of the Transform.
    pub reason: Option<String>,
    /// The Crop mask (Stabilize, Crop / Auto-scale framing), if one was added.
    pub mask: Option<Uid>,
    /// The Corner Pin, if one was added.
    pub corner_pin: Option<Uid>,
    /// Effects below the Warp Stabilizer, which now render before the stabilization (Transform
    /// target only).
    pub effects_after: usize,
}

fn translate_h(o: [f64; 2]) -> Homography {
    Homography([[1.0, 0.0, o[0]], [0.0, 1.0, o[1]], [0.0, 0.0, 1.0]])
}

/// A similarity's (scale, rotation in degrees), or `None` when `h` isn't one.
fn similarity(h: &Homography) -> Option<(f64, f64)> {
    let m = h.0;
    let w = m[2][2];
    if w.abs() < 1e-12 || m[2][0].abs() > 1e-9 || m[2][1].abs() > 1e-9 {
        return None;
    }
    let (a, b, c, d) = (m[0][0] / w, m[0][1] / w, m[1][0] / w, m[1][1] / w);
    let sc = a.hypot(c);
    if sc.is_nan() || sc <= 1e-9 || (a - d).abs() > 1e-6 * sc || (b + c).abs() > 1e-6 * sc {
        return None;
    }
    Some((sc, c.atan2(a).to_degrees()))
}

fn has_expr(tr: &PropGroup, m: &str) -> bool {
    tr.get(m).is_some_and(|p| p.has_expression())
}

/// Bake an analysed Warp Stabilizer `effect` into its layer (Warp Stabilizer ▸ Apply to
/// Transform): one keyframe per analysed frame on the layer's Position, Rotation and Scale, so
/// the layer moves the way the effect moved its pixels, and the effect is removed.
///
/// The Transform can only carry a move, a rotation and a uniform scale applied on top of the
/// layer's own transform. When the stabilization (Perspective) or the layer (non-square pixels,
/// non-uniform scale with rotation, X/Y Rotation, Auto-Orient) needs more, the frames go to a
/// keyframed Corner Pin placed where the Warp Stabilizer was instead. Crop framings add a mask
/// keyed to the region the effect showed. Subspace Warp (a mesh per frame) and Synthesize Edges
/// (pixels from other frames) can't be baked.
pub fn apply_to_transform(p: &mut Project, cid: ItemId, lid: LayerId, uid: Uid) -> Result<Baked, String> {
    let comp = p.comp(cid).ok_or("no composition")?.clone();
    let layer = comp.layer(lid).ok_or("no layer")?.clone();
    let fx = layer.effects().ok_or("the layer has no effects")?;
    let index = fx.groups().position(|g| g.uid == uid).ok_or("no such effect")?;
    let g = fx.groups().nth(index).filter(|g| is_warp(g)).ok_or("not a Warp Stabilizer")?;
    let effects_after = fx.groups().skip(index + 1).filter(|g| g.enabled).count();
    let times = analysis_times(&comp, &layer);
    let ctx0 = EvalCtx::new(p, cid, &comp, times.first().copied().unwrap_or(Tick::ZERO));
    let params = warp_stab_params(&ctx0, &layer, g);
    let a = warp_stab::analysis(&params).ok_or("analyze the Warp Stabilizer first")?;
    if str_of(g, ANALYSIS_KEY) != signature(p, cid, &layer) || a.frames.len() != times.len() {
        return Err("the analysis is out of date: analyze again".into());
    }
    let (size, origin) = effectcraft_render::effect_bounds(p, &comp, &layer);
    if (size[0] - a.size[0]).abs() > 0.5 || (size[1] - a.size[1]).abs() > 0.5 {
        return Err("the analysis was made for another layer size: analyze again".into());
    }
    let fps = if a.frame_duration > 0.0 { 1.0 / a.frame_duration } else { 30.0 };
    let st = warp_stab::settings(&params, fps);
    match st.method {
        Method::SubspaceWarp => return Err("Subspace Warp warps each frame with a mesh: choose another Method to apply it to the transform".into()),
        _ if st.framing == Framing::SynthesizeEdges => {
            return Err("Synthesize Edges fills the borders from other frames: choose another Framing to apply it to the transform".into());
        }
        _ => {}
    }
    let (_, plan) = warp_stab::summary(&params).ok_or("the stabilization could not be computed")?;
    // Per frame: the effect's warp in layer pixels (effect space is offset by `origin`).
    let warps: Vec<Homography> =
        plan.warps.iter().map(|m| translate_h(origin).then_after(&Homography(*m).then_after(&translate_h([-origin[0], -origin[1]])))).collect();
    if warps.len() != times.len() {
        return Err("the stabilization does not match the analysis: analyze again".into());
    }
    let tr = layer.transform().ok_or("the layer has no transform")?;

    // Can the Transform carry every frame exactly?
    let reason = (|| {
        if st.method == Method::Perspective {
            return Some("Perspective needs a Corner Pin".to_string());
        }
        if layer.auto_orient != AutoOrient::Off {
            return Some("the layer auto-orients".into());
        }
        if (ctx0.par_ratio(&layer) - 1.0).abs() > 1e-9 {
            return Some("the layer's pixels aren't square".into());
        }
        for m in ["anchor", "position", "positionX", "positionY", "positionZ", "rotation", "scale"] {
            if has_expr(tr, m) {
                return Some(format!("{} has an expression", tr.get(m).map(|p| p.name.as_str()).unwrap_or(m)));
            }
        }
        for (t, w) in times.iter().zip(&warps) {
            let Some((_, rot)) = similarity(w) else { return Some("the stabilization isn't a move, rotation and scale".into()) };
            if rot.abs() < 1e-6 {
                continue;
            }
            let c = ctx0.at(*t);
            let s = c.v3(&layer, tr, "scale", [100.0; 3]);
            if (s[0] - s[1]).abs() > 1e-6 {
                return Some("the layer's scale isn't uniform".into());
            }
            if layer.is_3d() && (c.f(&layer, tr, "rotationX", 0.0).abs() > 1e-6 || c.f(&layer, tr, "rotationY", 0.0).abs() > 1e-6) {
                return Some("the layer has X or Y Rotation".into());
            }
        }
        None
    })();

    // (layer time, values) to write.
    let mut pos_keys = vec![];
    let mut rot_keys = vec![];
    let mut scale_keys = vec![];
    let mut corner_keys = vec![];
    let mut mask_keys = vec![];
    let mut prev_rot = 0.0;
    for (t, w) in times.iter().zip(&warps) {
        let c = ctx0.at(*t);
        let lt = layer.layer_time(*t);
        if reason.is_none() {
            let (sc, rot) = similarity(w).ok_or("the stabilization isn't a move, rotation and scale")?;
            let rot = effectcraft_track::unwrap_degrees(prev_rot, rot);
            prev_rot = rot;
            // The new transform keeps the Anchor Point: Position is where the old transform puts
            // the stabilized anchor, Rotation and Scale take the warp's on top.
            let anchor = c.v3(&layer, tr, "anchor", [0.0; 3]);
            let wa = w.apply([anchor[0], anchor[1]]);
            let d = c.local_matrix(&layer).apply_vec(vec3(wa[0] - anchor[0], wa[1] - anchor[1], 0.0));
            let pos = c.position(&layer, tr);
            pos_keys.push((lt, [pos[0] + d.x, pos[1] + d.y, pos[2] + d.z]));
            rot_keys.push((lt, c.f(&layer, tr, "rotation", 0.0) + rot));
            let s = c.v3(&layer, tr, "scale", [100.0; 3]);
            scale_keys.push((lt, [s[0] * sc, s[1] * sc, s[2]]));
        } else {
            // Corner Pin's corners are effect points (layer pixels minus `origin`).
            let e = |q: [f64; 2]| {
                let r = w.apply([q[0] + origin[0], q[1] + origin[1]]);
                [r[0] - origin[0], r[1] - origin[1]]
            };
            corner_keys.push((lt, [e([0.0, 0.0]), e([size[0], 0.0]), e([0.0, size[1]]), e([size[0], size[1]])]));
        }
        // The crop is in output pixels: the mask is what lands inside it.
        if let Some(cr) = plan.crop {
            let inv = w.inverse().ok_or("the stabilization is not invertible")?;
            let corners = [[cr[0], cr[1]], [cr[2], cr[1]], [cr[2], cr[3]], [cr[0], cr[3]]];
            let v: Vec<[f64; 2]> = corners.iter().map(|q| inv.apply([q[0] + origin[0], q[1] + origin[1]])).collect();
            mask_keys.push((lt, v));
        }
    }

    let frames = times.len();
    let mut ids_next = p.next_id;
    let l = p.comp_mut(cid).and_then(|c| c.layer_mut(lid)).ok_or("no layer")?;
    // The Warp Stabilizer goes; a Corner Pin takes its place in the stack.
    let fx = l.props.sub_mut("effects").ok_or("the layer has no effects")?;
    let at = fx.children.iter().position(|c| c.as_group().is_some_and(|g| g.uid == uid)).ok_or("no such effect")?;
    fx.children.remove(at);
    let mut corner_pin = None;
    if !corner_keys.is_empty() {
        let spec = effectcraft_effects::find("ec.distort.cornerpin").ok_or("Corner Pin is not available")?;
        let same = fx.groups().filter(|g| g.match_id == spec.id).count();
        let name = if same == 0 { spec.name.to_string() } else { format!("{} {}", spec.name, same + 1) };
        let mut cp = effectcraft_effects::instantiate(spec, &mut effectcraft_project::build::Ids(&mut ids_next), &name, size);
        for (t, c) in &corner_keys {
            for (m, v) in ["ul", "ur", "ll", "lr"].iter().zip(c) {
                key(&mut cp, m, *t, Value::Vec2(*v));
            }
        }
        corner_pin = Some(cp.uid);
        // Where the Warp Stabilizer was, so the effects after it still see stabilized pixels.
        fx.children.insert(at.min(fx.children.len()), cp.into());
    }
    let mut mask = None;
    if !mask_keys.is_empty() {
        let masks = l.props.sub_mut("masks").ok_or("the layer can't have masks")?;
        let n = masks.groups().count();
        let mode = if n == 0 { MaskMode::Add } else { MaskMode::Intersect };
        let path = |v: &[[f64; 2]]| ShapePath {
            vertices: v.to_vec(),
            in_tangents: vec![[0.0; 2]; v.len()],
            out_tangents: vec![[0.0; 2]; v.len()],
            closed: true,
            feather: vec![],
        };
        let color = effectcraft_project::build::MASK_COLORS[n % effectcraft_project::build::MASK_COLORS.len()];
        let first = mask_keys.first().map(|(_, v)| path(v)).ok_or("no frames")?;
        let mut m = effectcraft_project::build::mask(&mut effectcraft_project::build::Ids(&mut ids_next), "Warp Stabilizer Crop", first, mode, color);
        for (t, v) in &mask_keys {
            key(&mut m, "path", *t, Value::Path(path(v)));
        }
        mask = Some(m.uid);
        masks.children.push(m.into());
    }
    if let Some(trm) = l.transform_mut() {
        let separated = trm.get("positionX").is_some();
        for (t, v) in pos_keys {
            if separated {
                key(trm, "positionX", t, Value::Scalar(v[0]));
                key(trm, "positionY", t, Value::Scalar(v[1]));
                key(trm, "positionZ", t, Value::Scalar(v[2]));
            } else {
                key(trm, "position", t, Value::Vec3(v));
            }
        }
        for (t, v) in rot_keys {
            key(trm, "rotation", t, Value::Scalar(v));
        }
        for (t, v) in scale_keys {
            key(trm, "scale", t, Value::Vec3(v));
        }
    }
    p.next_id = ids_next;
    let target = if corner_pin.is_some() { BakeTarget::CornerPin } else { BakeTarget::Transform };
    Ok(Baked { target, frames, reason, mask, corner_pin, effects_after: if target == BakeTarget::Transform { effects_after } else { 0 } })
}

/// The Warp Stabilizer's flattened parameters (its settings aren't animated).
fn warp_stab_params(ctx: &EvalCtx, layer: &Layer, g: &PropGroup) -> effectcraft_effects::Params {
    effectcraft_effects::flatten_params(g, &mut |pr| ctx.value(layer, pr))
}

impl Session {
    pub fn is_warp_analyzing(&self) -> bool {
        self.warp_job.as_ref().is_some_and(|j| !j.is_finished()) || self.offloaded(JobKind::Warp).is_some()
    }

    /// Progress of the running (or just finished) analysis.
    pub fn warp_progress(&self) -> Option<WarpProgress> {
        self.warp_job.as_ref().map(|j| j.progress()).or_else(|| {
            self.offloaded(JobKind::Warp).map(|j| {
                let p = &j.progress;
                WarpProgress { step: 1, done: p.done, total: p.total, elapsed: p.elapsed, fps: rate(p.done, p.elapsed), ..Default::default() }
            })
        })
    }

    /// The effect being analysed: (comp, layer, effect uid).
    pub fn warp_target(&self) -> Option<(ItemId, LayerId, Uid)> {
        self.warp_job.as_ref().map(|j| (j.comp, j.layer, j.effect)).or_else(|| self.offloaded(JobKind::Warp).map(|j| (j.comp, j.layer, j.uid)))
    }

    /// Cancel the running analysis (nothing is written).
    pub fn stop_warp(&mut self) -> bool {
        if self.cancel_offloaded(JobKind::Warp) {
            return true;
        }
        match &self.warp_job {
            Some(j) if !j.is_finished() => {
                j.shared.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Start analysing Warp Stabilizer `effect` on `layer`. `wait`: block until done (wasm32
    /// always does). Returns the number of frames.
    pub fn start_warp(&mut self, comp: ItemId, layer: LayerId, effect: Uid, wait: bool) -> Result<usize, String> {
        if self.is_warp_analyzing() {
            return Err("a Warp Stabilizer analysis is already running".into());
        }
        self.poll_warp_job();
        self.warp_pending.retain(|x| *x != (comp, layer, effect));
        let c = self.project.comp(comp).ok_or("no composition")?;
        let l = c.layer(layer).ok_or("no layer")?;
        let fx = l.effects().ok_or("the layer has no effects")?;
        let index = fx.groups().position(|g| g.uid == effect).ok_or("no such effect")?;
        let g = fx.groups().nth(index).filter(|g| is_warp(g)).ok_or("not a Warp Stabilizer")?;
        if l.switches.adjustment {
            return Err("Warp Stabilizer needs footage, not an adjustment layer".into());
        }
        let times = analysis_times(c, l);
        if times.len() < 2 {
            return Err("the layer must be at least two frames long".into());
        }
        let size = effectcraft_render::effect_bounds(&self.project, c, l).0;
        let detailed = g.prop("advanced/detailedAnalysis").is_some_and(|p| p.value.as_bool());
        let n = times.len();
        let work = WarpWork {
            project: self.project.clone(),
            footage: self.footage.clone(),
            expr: self.expr.clone(),
            cache: self.layer_cache.clone(),
            comp,
            layer,
            effect_index: index,
            size,
            detailed,
            times,
        };
        let key = signature(&self.project, comp, l);
        if !wait && self.offloads() {
            drop(work);
            self.offload_analysis(crate::offload::WorkerJob::Warp { comp, layer, effect })?;
            return Ok(n);
        }
        let wait = wait || cfg!(target_arch = "wasm32");
        let shared = Arc::new(WarpShared::default());
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
                    .name("warp-analyze".into())
                    .spawn(move || effectcraft_render::passes::block_on(run_work(work, &sh)))
                    .map_err(|e| e.to_string())?,
            )
        };
        self.warp_job = Some(WarpJob { shared, thread, comp, layer, effect, key });
        self.events.push(Event::ProjectChanged { revision: self.revision });
        self.poll_warp_job();
        Ok(n)
    }

    /// Write a finished analysis into its effect (one undo step) and drop the job.
    fn poll_warp_job(&mut self) -> bool {
        let Some(job) = &self.warp_job else { return false };
        if !job.is_finished() {
            return false;
        }
        let Some(mut job) = self.warp_job.take() else { return false };
        if let Some(t) = job.thread.take() {
            let _ = t.join();
        }
        let st = job.progress();
        let result = lock(&job.shared.result).take();
        match (result, &st.error) {
            (Some(a), None) if !st.cancelled => {
                let json = a.to_json();
                let (cid, lid, uid, key) = (job.comp, job.layer, job.effect, job.key.clone());
                let r = self.edit("Warp Stabilizer Analysis", None, |p, _| {
                    let g = p
                        .comp_mut(cid)
                        .and_then(|c| c.layer_mut(lid))
                        .and_then(|l| l.props.find_group_mut(uid))
                        .ok_or_else(|| crate::EngineError::Other("the Warp Stabilizer was removed during analysis".into()))?;
                    for (m, v) in [(ANALYSIS, json), (ANALYSIS_KEY, key)] {
                        if let Some(pr) = g.get_mut(m) {
                            pr.value = Value::Str(v);
                        }
                    }
                    Ok(())
                });
                let msg = match r {
                    Ok(()) => format!("Warp Stabilizer: analysed {} frame(s) in {:.1} s", a.frames.len(), st.elapsed),
                    Err(e) => format!("Warp Stabilizer: {e}"),
                };
                self.events.push(Event::Toast { message: msg, error: false });
            }
            _ => {
                let msg = match &st.error {
                    Some(e) => format!("Warp Stabilizer analysis failed: {e}"),
                    None => "Warp Stabilizer analysis cancelled".into(),
                };
                self.events.push(Event::Toast { message: msg, error: st.error.is_some() });
                self.bump();
            }
        }
        true
    }

    /// Frontends call this every frame: finishes analyses and (with `auto`) starts queued
    /// re-analyses in the background, like After Effects.
    pub fn poll_warp(&mut self, auto: bool) -> bool {
        let changed = self.poll_warp_job();
        if auto && self.warp_job.is_none() && self.offloaded(JobKind::Warp).is_none() {
            while let Some((c, l, e)) = self.warp_pending.first().copied() {
                self.warp_pending.remove(0);
                if self.start_warp(c, l, e, false).is_ok() {
                    return true;
                }
            }
        }
        changed
    }
}
