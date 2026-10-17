//! Roto Brush & Refine Edge (applied by the Roto Brush tool; Effect ▸ Matte).
//!
//! The instance stores the user's strokes, base frame and segmentation span as JSON in its hidden
//! **Strokes** parameter ([`RotoData`]); the segmentation itself is *derived* data: each frame's
//! result is a deterministic function of its chain key ([`RotoData::chain_keys`] seeded with
//! [`seed`]: what the frames are, the Roto Brush settings and the strokes from the base frame
//! out to the frame), so it lives in a process-wide content-keyed cache ([`cached`]) that the
//! engine's propagation job fills at full resolution and rendering fills on demand (reading
//! neighbouring frames with [`crate::EffectHost::self_at`]). Undo, redo and reopening a project
//! therefore never see stale mattes. Behind the memory cache an optional [`SegStore`] (the
//! persistent disk cache, set by the engine) keeps computed segmentations across restarts. **Freeze** stores the final mattes in the hidden **Frozen**
//! parameter ([`FrozenCache`]); they are used while their key still matches.
//!
//! Rendering: binary segmentation → Roto Brush Matte (Shift Edge, Reduce Chatter, Feather,
//! Contrast) → Refine Edge matting inside the Refine Edge band (guided filter + closed-form
//! matting with Higher Quality) → Refine Edge Matte adjustments → matte motion blur →
//! decontamination of the edge colours. Outside the segmentation span the layer is transparent;
//! without strokes the effect passes its input through.
//!
//! *Version*: 1.0 is the classic graph-cut engine; 2.0 and 3.0 also use the trained model chosen
//! in Settings ▸ Roto Brush ([`set_model`], an `effectcraft_segment::MaskModel` such as
//! MobileSAM), falling back to the classic engine when none is installed. The model's id is part
//! of the chain seed, so choosing another recomputes the mattes. *Quality ▸ Best* segments at a
//! higher working resolution with more iterations.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use effectcraft_raster::Image;
use effectcraft_segment::{ClipFrame, MaskModel, TrackRequest};
use effectcraft_track::roto::refine::{self, MatteParams};
use effectcraft_track::roto::{self as rb, Gmm, RotoData, SegOpts, rle};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::warp_stab::param;
use crate::{Buf, EffectCtx, EffectSpec, Params, num, p, popup, slider};

pub use effectcraft_track::roto::{FrameSeg, Stroke, StrokeKind};

/// The effect's id.
pub const ID: &str = "ec.matte.rotobrush";
/// Hidden parameters.
pub const STROKES: &str = "strokes";
pub const INPUT_KEY: &str = "inputKey";
pub const FROZEN: &str = "frozen";
/// The saved tracking (see [`TrackCache`]).
pub const TRACKED: &str = "tracked";

/// Default segmentation span each side of the base frame (frames).
pub const DEFAULT_SPAN: i64 = 20;

/// Display names of the instance's parameter groups.
pub const GROUPS: &[(&str, &str)] = &[
    ("rotoBrushPropagation", "Roto Brush Propagation"),
    ("rotoBrushMatte", "Roto Brush Matte"),
    ("refineEdgeMatte", "Refine Edge Matte"),
    ("motionBlur", "Motion Blur"),
    ("decontamination", "Decontamination"),
];

pub const VERSIONS: [&str; 3] = ["1.0", "2.0", "3.0"];
pub const QUALITIES: [&str; 2] = ["Standard", "Best"];

pub fn specs() -> Vec<EffectSpec> {
    vec![EffectSpec {
        id: ID,
        name: "Roto Brush & Refine Edge",
        category: "Matte",
        params: vec![
            p("version", "Version", Value::Enum(2), popup(&VERSIONS)),
            p("quality", "Quality", Value::Enum(0), popup(&QUALITIES)),
            p("rotoBrushPropagation/searchRadius", "Search Radius", num(15.0), slider(0.0, 100.0, 0.0, 50.0, 1)),
            p("rotoBrushPropagation/motionThreshold", "Motion Threshold", num(10.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("rotoBrushPropagation/motionDamping", "Motion Damping", num(20.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("rotoBrushPropagation/viewSearchRegion", "View Search Region", Value::Bool(false), ParamUi::Checkbox),
            p("rotoBrushMatte/feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("rotoBrushMatte/contrast", "Contrast", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("rotoBrushMatte/shiftEdge", "Shift Edge", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
            p("rotoBrushMatte/reduceChatter", "Reduce Chatter", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("rotoBrushMatte/object", "Show Object", num(0.0), slider(0.0, 32.0, 0.0, 32.0, 0)),
            p("refineEdgeMatte/smooth", "Smooth", num(1.0), slider(0.0, 100.0, 0.0, 10.0, 1)),
            p("refineEdgeMatte/feather", "Feather", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("refineEdgeMatte/contrast", "Contrast", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("refineEdgeMatte/shiftEdge", "Shift Edge", num(0.0), slider(-100.0, 100.0, -100.0, 100.0, 0)),
            p("refineEdgeMatte/reduceChatter", "Reduce Chatter", num(0.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("refineEdgeMatte/useMotionBlur", "Use Motion Blur", Value::Bool(false), ParamUi::Checkbox),
            p("refineEdgeMatte/motionBlur/motionBlurSamples", "Samples", num(11.0), slider(1.0, 64.0, 1.0, 32.0, 0)),
            p("refineEdgeMatte/motionBlur/shutterAngle", "Shutter Angle", num(180.0), slider(0.0, 720.0, 0.0, 360.0, 0)),
            p("refineEdgeMatte/motionBlur/higherQuality", "Higher Quality", Value::Bool(true), ParamUi::Checkbox),
            p("refineEdgeMatte/decontaminateEdgeColors", "Decontaminate Edge Colors", Value::Bool(true), ParamUi::Checkbox),
            p("refineEdgeMatte/decontamination/decontaminationAmount", "Decontamination Amount", num(100.0), slider(0.0, 100.0, 0.0, 100.0, 0)),
            p("refineEdgeMatte/decontamination/extendWhereSmoothed", "Extend Where Smoothed", Value::Bool(true), ParamUi::Checkbox),
            p("refineEdgeMatte/decontamination/increaseDecontaminationRadius", "Increase Decontamination Radius", num(0.0), slider(0.0, 100.0, 0.0, 20.0, 1)),
            p("refineEdgeMatte/decontamination/viewDecontaminationMap", "View Decontamination Map", Value::Bool(false), ParamUi::Checkbox),
            p(STROKES, "Strokes", Value::Str(String::new()), ParamUi::Hidden),
            p(INPUT_KEY, "Input Key", Value::Str(String::new()), ParamUi::Hidden),
            p(FROZEN, "Frozen", Value::Str(String::new()), ParamUi::Hidden),
            p(TRACKED, "Tracked", Value::Str(String::new()), ParamUi::Hidden),
        ],
        render,
        gpu: false,
        float: true,
    }]
}

fn f(params: &Params, id: &str) -> f64 {
    param(params, id).map(Value::as_f64).unwrap_or(0.0)
}
fn e(params: &Params, id: &str) -> u32 {
    param(params, id).map(Value::as_enum).unwrap_or(0)
}
fn b(params: &Params, id: &str) -> bool {
    param(params, id).map(Value::as_bool).unwrap_or(false)
}
fn s<'a>(params: &'a Params, id: &str) -> &'a str {
    match param(params, id) {
        Some(Value::Str(s)) => s,
        _ => "",
    }
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The instance's strokes (parsed once per content).
pub fn data(params: &Params) -> Arc<RotoData> {
    static C: OnceLock<Mutex<HashMap<u64, Arc<RotoData>>>> = OnceLock::new();
    let j = s(params, STROKES);
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, j.as_bytes());
    let c = C.get_or_init(Default::default);
    if let Some(d) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&h) {
        return d.clone();
    }
    let d = Arc::new(RotoData::from_json(j));
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 64 {
        m.clear();
    }
    m.insert(h, d.clone());
    d
}

/// Segmentation settings at `scale` (pixels per layer pixel).
pub fn seg_opts(params: &Params, scale: f64) -> SegOpts {
    SegOpts {
        scale,
        search_radius: f(params, "rotoBrushPropagation/searchRadius").max(0.0),
        motion_threshold: f(params, "rotoBrushPropagation/motionThreshold"),
        motion_damping: f(params, "rotoBrushPropagation/motionDamping"),
        best: e(params, "quality") == 1,
    }
}

/// Matte settings.
pub fn matte_params(params: &Params) -> MatteParams {
    MatteParams {
        rb_feather: f(params, "rotoBrushMatte/feather").max(0.0),
        rb_contrast: f(params, "rotoBrushMatte/contrast"),
        rb_shift: f(params, "rotoBrushMatte/shiftEdge"),
        rb_chatter: f(params, "rotoBrushMatte/reduceChatter"),
        smooth: f(params, "refineEdgeMatte/smooth").max(0.0),
        feather: f(params, "refineEdgeMatte/feather").max(0.0),
        contrast: f(params, "refineEdgeMatte/contrast"),
        shift: f(params, "refineEdgeMatte/shiftEdge"),
        chatter: f(params, "refineEdgeMatte/reduceChatter"),
        motion_blur: b(params, "refineEdgeMatte/useMotionBlur"),
        mb_samples: f(params, "refineEdgeMatte/motionBlur/motionBlurSamples").round().clamp(1.0, 64.0) as usize,
        shutter_angle: f(params, "refineEdgeMatte/motionBlur/shutterAngle"),
        higher_quality: b(params, "refineEdgeMatte/motionBlur/higherQuality"),
        decontaminate: b(params, "refineEdgeMatte/decontaminateEdgeColors"),
        decontamination: f(params, "refineEdgeMatte/decontamination/decontaminationAmount"),
        extend_where_smoothed: b(params, "refineEdgeMatte/decontamination/extendWhereSmoothed"),
        increase_radius: f(params, "refineEdgeMatte/decontamination/increaseDecontaminationRadius").max(0.0),
    }
}

fn model_slot() -> &'static RwLock<Option<Arc<dyn MaskModel>>> {
    static M: OnceLock<RwLock<Option<Arc<dyn MaskModel>>>> = OnceLock::new();
    M.get_or_init(|| RwLock::new(None))
}

/// The trained model Roto Brush 2.0 / 3.0 use (Settings ▸ Roto Brush; `None`: the classic
/// engine for every version).
pub fn set_model(m: Option<Arc<dyn MaskModel>>) {
    if let Ok(mut slot) = model_slot().write() {
        *slot = m;
    }
}

/// Whether an instance with `params` segments with the model Settings chose: Roto Brush 2.0 and
/// 3.0 do, 1.0 is the classic engine.
pub fn uses_model(params: &Params) -> bool {
    e(params, "version") != 0
}

/// The model an instance segments with: its version's choice of the installed one.
pub fn model_for(params: &Params) -> Option<Arc<dyn MaskModel>> {
    if !uses_model(params) {
        return None;
    }
    installed_model()
}

/// The trained model chosen in Settings ▸ Roto Brush, if one is installed (Roto Brush 2.0 / 3.0
/// and Object Matte segment with it).
pub fn installed_model() -> Option<Arc<dyn MaskModel>> {
    model_slot().read().ok().and_then(|m| m.clone())
}

/// Seed of the chain keys: what the frames are (the engine-maintained Input Key), the layer
/// size, the frame rate, the segmentation settings and the model.
pub fn seed(params: &Params, layer_size: [f64; 2], fps: f64) -> u64 {
    seed_with(params, layer_size, fps, model_for(params).map_or(effectcraft_segment::CLASSICAL, |m| m.info().id))
}

/// [`seed`] with the segmenting model named by its id (saved tracking names the one that made it).
fn seed_with(params: &Params, layer_size: [f64; 2], fps: f64, model: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, s(params, INPUT_KEY).as_bytes());
    for v in [
        layer_size[0],
        layer_size[1],
        fps,
        f(params, "rotoBrushPropagation/searchRadius"),
        f(params, "rotoBrushPropagation/motionThreshold"),
        f(params, "rotoBrushPropagation/motionDamping"),
    ] {
        fnv(&mut h, &v.to_bits().to_le_bytes());
    }
    fnv(&mut h, &[e(params, "quality") as u8, e(params, "version") as u8]);
    fnv(&mut h, model.as_bytes());
    h
}

/// The layer frame shown at layer time `t` seconds.
pub fn frame_of(t: f64, fps: f64) -> i64 {
    (t * fps + 1e-6).floor() as i64
}

// ------------------------------------------------------------------ derived-data cache

struct SegCache {
    map: HashMap<(u64, u64), Arc<FrameSeg>>,
    order: VecDeque<(u64, u64)>,
    bytes: usize,
}

const CACHE_BYTES: usize = 768 << 20;

fn cache() -> &'static Mutex<SegCache> {
    static C: OnceLock<Mutex<SegCache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(SegCache { map: HashMap::new(), order: VecDeque::new(), bytes: 0 }))
}

/// A cached segmentation (chain key, scale).
pub fn cached(key: u64, scale: f64) -> Option<Arc<FrameSeg>> {
    cache().lock().unwrap_or_else(|e| e.into_inner()).map.get(&(key, scale.to_bits())).cloned()
}

pub fn store(key: u64, scale: f64, seg: Arc<FrameSeg>) {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    let k = (key, scale.to_bits());
    if c.map.contains_key(&k) {
        return;
    }
    c.bytes += seg.matte.len() + seg.refine.len();
    c.map.insert(k, seg);
    c.order.push_back(k);
    while c.bytes > CACHE_BYTES {
        let Some(old) = c.order.pop_front() else { break };
        if let Some(s) = c.map.remove(&old) {
            c.bytes -= s.matte.len() + s.refine.len();
        }
    }
}

/// A second-level store for segmentations, keyed like the memory cache (chain key, scale): the
/// persistent disk cache, so computed mattes survive the memory cache and restarts.
pub trait SegStore: Send + Sync {
    fn get_seg(&self, key: u64, scale: f64) -> Option<FrameSeg>;
    fn put_seg(&self, key: u64, scale: f64, seg: &FrameSeg);
}

fn seg_store_slot() -> &'static RwLock<Option<Weak<dyn SegStore>>> {
    static S: OnceLock<RwLock<Option<Weak<dyn SegStore>>>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(None))
}

/// Use `store` behind the memory cache (held weakly: once its owner drops it, e.g. the disk
/// cache is switched off, it is no longer used). `None` turns it off.
pub fn set_seg_store(store: Option<Weak<dyn SegStore>>) {
    if let Ok(mut s) = seg_store_slot().write() {
        *s = store;
    }
}

fn seg_store() -> Option<Arc<dyn SegStore>> {
    seg_store_slot().read().ok()?.as_ref()?.upgrade()
}

/// A segmentation from the second-level store, put into the memory cache.
fn from_store(key: u64, scale: f64) -> Option<Arc<FrameSeg>> {
    let seg = Arc::new(seg_store()?.get_seg(key, scale)?);
    store(key, scale, seg.clone());
    Some(seg)
}

/// Tracker results live under the chain key mixed with this, so a frame the tracker followed and
/// one the classic engine computed never share a cache slot.
const TRACK_SALT: u64 = 0x7472_6163_6b65_7231;

fn tracked_key(key: u64) -> u64 {
    key.rotate_left(17) ^ TRACK_SALT
}

/// Where one object's matte of a tracked frame lives (kept only when the instance has several objects).
fn object_key(key: u64, object: u32) -> u64 {
    tracked_key(key).wrapping_add(0x9e37_79b9_7f4a_7c15u64.wrapping_mul(u64::from(object) + 1))
}

/// How far to trust a frame the tracker followed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTrust {
    /// 0–1, from the tracker's own object score and mask quality.
    pub confidence: f32,
    /// The object is not in this frame (an empty matte).
    pub occluded: bool,
}

fn trust_cache() -> &'static Mutex<HashMap<u64, FrameTrust>> {
    static T: OnceLock<Mutex<HashMap<u64, FrameTrust>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The tracker's confidence in the frame with chain key `key`, if the tracker followed it.
pub fn trust(key: u64) -> Option<FrameTrust> {
    trust_cache().lock().unwrap_or_else(|e| e.into_inner()).get(&tracked_key(key)).copied()
}

/// Drop the cached segmentations of these chain keys (every scale).
pub fn forget(keys: impl IntoIterator<Item = u64>) {
    let mut trusts = trust_cache().lock().unwrap_or_else(|e| e.into_inner());
    let keys: std::collections::HashSet<u64> = keys.into_iter().flat_map(|k| [k, tracked_key(k)]).collect();
    trusts.retain(|k, _| !keys.contains(k));
    drop(trusts);
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    let gone: Vec<(u64, u64)> = c.map.keys().filter(|k| keys.contains(&k.0)).copied().collect();
    for k in gone {
        if let Some(s) = c.map.remove(&k) {
            c.bytes -= s.matte.len() + s.refine.len();
        }
    }
    c.order.retain(|k| !keys.contains(&k.0));
}

/// Drop every cached segmentation (Edit ▸ Purge).
pub fn purge() {
    trust_cache().lock().unwrap_or_else(|e| e.into_inner()).clear();
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.map.clear();
    c.order.clear();
    c.bytes = 0;
}

/// The layer-space grid (`round(size · scale)` pixels) of a buffer.
pub fn layer_grid(buf: &Buf, layer_size: [f64; 2]) -> Image {
    let s = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    let (w, h) = (((layer_size[0] * s).round() as u32).max(1), ((layer_size[1] * s).round() as u32).max(1));
    let (ox, oy) = (buf.offset[0].round() as i64, buf.offset[1].round() as i64);
    let mut img = Image::new(w, h);
    img.rows_mut().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = buf.img.get(x as i64 + ox, y as i64 + oy);
        }
    });
    img
}

/// What a tracking pass found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackSummary {
    pub frames: usize,
    /// Frames where the object was absent.
    pub occluded: usize,
    pub min_confidence: f32,
}

/// Everything needed to compute an instance's segmentations.
pub struct Chain {
    pub data: Arc<RotoData>,
    pub keys: BTreeMap<i64, u64>,
    pub opts: SegOpts,
    pub refine_radius: f64,
    /// The trained model (Version 2.0 / 3.0 with one installed).
    pub model: Option<Arc<dyn MaskModel>>,
    /// The saved tracking, while it applies (see [`TrackCache`]).
    pub(crate) saved: Option<SavedTracking>,
    /// Show Object: the matte of this object only (its id plus one; 0 shows all of them together).
    pub show_object: u32,
}

impl Chain {
    pub fn new(params: &Params, layer_size: [f64; 2], fps: f64, scale: f64) -> Chain {
        let data = data(params);
        let keys = data.chain_keys(seed(params, layer_size, fps));
        let saved = saved_for(params, &data, layer_size, fps);
        let show_object = f(params, "rotoBrushMatte/object").round().clamp(0.0, 32.0) as u32;
        Chain { refine_radius: data.refine_radius(), data, keys, opts: seg_opts(params, scale), model: model_for(params), saved, show_object }
    }

    /// Frame `f`'s segmentation at `scale` from the saved tracking or the caches; computes nothing.
    pub fn cached_seg(&self, f: i64, scale: f64) -> Option<Arc<FrameSeg>> {
        self.saved_seg(f, scale).or_else(|| self.keys.get(&f).and_then(|k| cached(*k, scale)))
    }

    /// Object `o`'s matte of frame `f` at this chain's scale (saved tracking, else this session's), when the instance
    /// has several objects and the tracker followed them there; computes nothing.
    pub fn object_seg(&self, f: i64, o: u32) -> Option<Arc<FrameSeg>> {
        let sc = self.opts.scale;
        let k = *self.keys.get(&f)?;
        let ok = object_key(k, o);
        if let Some(s) = cached(ok, sc) {
            return Some(s);
        }
        let full = match cached(ok, 1.0) {
            Some(s) => s,
            None => {
                let t = self.saved.as_ref()?;
                let s = Arc::new(decode_object(&t.cache, f, o)?);
                store(ok, 1.0, s.clone());
                s
            }
        };
        if sc == 1.0 {
            return Some(full);
        }
        let s = Arc::new(downscale(&full, sc));
        store(ok, sc, s.clone());
        Some(s)
    }

    /// What Show Object asks for at frame `f`: `None` shows the whole matte (all objects, a single object, or a frame
    /// the tracker did not follow); an object the instance does not have shows nothing.
    fn shown_object(&self, f: i64) -> Option<Arc<FrameSeg>> {
        let o = self.show_object.checked_sub(1)?;
        let objects = self.data.objects();
        if objects.len() < 2 {
            return None;
        }
        if !objects.contains(&o) {
            let sc = self.opts.scale;
            let [w, h] =
                self.saved.as_ref().map(|t| t.cache.size).or_else(|| self.keys.get(&f).and_then(|k| cached(tracked_key(*k), sc)).map(|s| [s.w, s.h]))?;
            return Some(Arc::new(FrameSeg::empty(w, h)));
        }
        self.object_seg(f, o)
    }

    /// The tracker's trust in frame `f`: from the saved tracking, else from this session's tracking.
    pub fn trust_at(&self, f: i64) -> Option<FrameTrust> {
        if let Some(fr) = self.saved.as_ref().and_then(|t| t.cache.frames.get(&f)) {
            return Some(FrameTrust { confidence: fr.c, occluded: fr.o });
        }
        trust(*self.keys.get(&f)?)
    }

    /// The model the saved tracking was made with, while it applies.
    pub fn saved_model(&self) -> Option<&str> {
        self.saved.as_ref().map(|t| t.cache.model.as_str())
    }

    /// Frame `f` of the saved tracking at `scale` (decoded once, then cached).
    fn saved_seg(&self, f: i64, scale: f64) -> Option<Arc<FrameSeg>> {
        let t = self.saved.as_ref()?;
        let k = tracked_key(*t.keys.get(&f)?);
        if let Some(s) = cached(k, scale) {
            return Some(s);
        }
        let full = match cached(k, 1.0) {
            Some(s) => s,
            None => {
                let s = Arc::new(decode_tracked(&t.cache, f)?);
                store(k, 1.0, s.clone());
                s
            }
        };
        if scale == 1.0 {
            return Some(full);
        }
        let s = Arc::new(downscale(&full, scale));
        store(k, scale, s.clone());
        Some(s)
    }

    /// Frame `f`'s segmentation at this chain's scale: cached, or computed from the nearest
    /// cached frame towards the base (full-resolution results serve smaller scales).
    /// `frame(g)`: the effect's input on frame g on the layer grid at this scale.
    pub fn ensure(&self, f: i64, frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>, cancel: &dyn Fn() -> bool) -> Option<Arc<FrameSeg>> {
        let sc = self.opts.scale;
        let lookup = |g: i64| -> Option<Arc<FrameSeg>> {
            // Show Object: one object's matte, where the tracker followed the objects.
            if let Some(s) = self.shown_object(g) {
                return Some(s);
            }
            // A frame the tracker followed wins over the classic engine's: the saved tracking, else
            // this session's.
            if let Some(s) = self.saved_seg(g, sc) {
                return Some(s);
            }
            let k = *self.keys.get(&g)?;
            if let Some(s) = cached(tracked_key(k), sc) {
                return Some(s);
            }
            if let Some(s) = cached(k, sc).or_else(|| from_store(k, sc)) {
                return Some(s);
            }
            if sc != 1.0 {
                let full = cached(tracked_key(k), 1.0).or_else(|| cached(k, 1.0)).or_else(|| from_store(k, 1.0))?;
                let s = Arc::new(downscale(&full, sc));
                store(k, sc, s.clone());
                return Some(s);
            }
            None
        };
        if let Some(s) = lookup(f) {
            return Some(s);
        }
        let base = self.data.base?;
        if !self.data.in_span(f) {
            return None;
        }
        // Walk towards the base to the nearest available frame.
        let mut path = vec![f];
        let mut g = f;
        let mut start: Option<Arc<FrameSeg>> = None;
        while let Some(src) = self.data.source_of(g) {
            if let Some(s) = lookup(src) {
                start = Some(s);
                break;
            }
            path.push(src);
            g = src;
        }
        path.reverse();
        let mut prev_img: Option<Arc<Image>> = None;
        let mut cur = start;
        for g in path {
            if cancel() {
                return None;
            }
            let img = frame(g)?;
            let strokes = self.data.strokes_at(g);
            let seg = match (&cur, g == base) {
                (_, true) | (None, _) => rb::segment_with(&img, &strokes, None, &self.opts, self.refine_radius, self.model.as_deref()),
                (Some(p), false) => {
                    let src = self.data.source_of(g)?;
                    let pimg = match prev_img.take() {
                        Some(i) => i,
                        None => frame(src)?,
                    };
                    if pimg.width != img.width || pimg.height != img.height {
                        return None;
                    }
                    // A tracker is too slow to prompt on every frame (its pass is `track`); the classic flow carries the rest.
                    rb::propagate_with(&pimg, p, &img, &strokes, &self.opts, self.refine_radius, if self.has_tracker() { None } else { self.model.as_deref() })
                }
            };
            let seg = Arc::new(seg);
            let k = *self.keys.get(&g)?;
            store(k, sc, seg.clone());
            if let Some(st) = seg_store() {
                st.put_seg(k, sc, &seg);
            }
            cur = Some(seg);
            prev_img = Some(img);
        }
        cur
    }

    /// Whether frame `f` is computed (at full resolution or at `scale`).
    pub fn is_cached(&self, f: i64, scale: f64) -> bool {
        self.keys.get(&f).is_some_and(|k| [*k, tracked_key(*k)].iter().any(|k| cached(*k, 1.0).is_some() || cached(*k, scale).is_some()))
    }

    /// Whether the chosen model can follow an object through a clip (SAM 2).
    pub fn has_tracker(&self) -> bool {
        self.model.as_ref().is_some_and(|m| m.tracker().is_some())
    }

    /// Follow the object through the span with the model's tracker, from the strokes on their
    /// frames (only the frames without a tracked result yet: after a correction stroke, the frames it
    /// changes, tracked on from the nearest prompted frame), and cache every frame's segmentation (under the tracker's keys, with its
    /// confidence). The strokes stay hard constraints and the edges are refined at full
    /// resolution; frames the tracker can't give are left to [`Chain::ensure`]'s classic engine.
    /// `progress` gets the number of frames finished. `Ok(None)`: no tracker, nothing done.
    pub fn track(
        &self,
        frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>,
        cancel: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(usize),
    ) -> Result<Option<TrackSummary>, String> {
        let Some(tracker) = self.model.as_ref().and_then(|m| m.tracker()) else { return Ok(None) };
        let (Some(_), true) = (self.data.base, self.data.span[1] >= self.data.span[0]) else { return Err("no base frame".into()) };
        // Frames of the span that already have a tracked result (this session's, or the saved
        // tracking) are kept: only the rest is tracked. A correction stroke changes the keys of the
        // frames from it on (both directions), so only those come back here.
        let (lo_span, hi_span) = (self.data.span[0], self.data.span[1]);
        let needed: Vec<i64> = (lo_span..=hi_span)
            .filter(|f| self.keys.get(f).is_some_and(|k| self.saved_seg(*f, self.opts.scale).is_none() && cached(tracked_key(*k), self.opts.scale).is_none()))
            .collect();
        let (Some(&need_lo), Some(&need_hi)) = (needed.first(), needed.last()) else {
            return Ok(Some(TrackSummary::default()));
        };
        // Each object has its own prompts: a few clicks from its strokes and a box around what the classic engine
        // makes of them on that frame (clicks alone pick a part of the object, not all of it).
        let objects = self.data.objects();
        let mut by_object: Vec<Vec<(i64, effectcraft_segment::Prompt)>> = vec![];
        for &obj in &objects {
            let mut prompts = vec![];
            for f in self.data.stroke_frames().into_iter().filter(|f| self.data.in_span(*f)) {
                let strokes = self.data.strokes_of(f, obj);
                let p = match frame(f) {
                    Some(img) => rb::tracker_prompt_boxed(&strokes, self.opts.scale, &rb::segment(&img, &strokes, None, &self.opts, self.refine_radius)),
                    None => rb::tracker_prompt(&strokes, self.opts.scale),
                };
                if p.points.iter().any(|(_, fg)| *fg) {
                    prompts.push((f, p));
                }
            }
            by_object.push(prompts);
        }
        // objects without a foreground prompt cannot be followed
        let followed: Vec<(u32, Vec<(i64, effectcraft_segment::Prompt)>)> = objects.into_iter().zip(by_object).filter(|(_, p)| !p.is_empty()).collect();
        if followed.is_empty() {
            return Err("no foreground strokes to follow".into());
        }
        // The window to track: every object restarts at the nearest of its prompted frames at or before the first
        // frame needed (a correction restarts the object there), so the window starts at the earliest restart; it
        // ends at the last frame needed, reaching the farthest first prompt when an object's needed frames lie
        // before its prompt.
        let lo = followed.iter().map(|(_, p)| p.iter().map(|q| q.0).filter(|f| *f <= need_lo).max().unwrap_or(need_lo)).min().unwrap_or(need_lo);
        let first_prompt = followed.iter().filter_map(|(_, p)| p.first().map(|q| q.0)).max().unwrap_or(need_hi);
        let hi = need_hi.max(first_prompt);
        let objects: Vec<u32> = followed.iter().map(|(o, _)| *o).collect();
        let prompts: Vec<Vec<(i64, effectcraft_segment::Prompt)>> =
            followed.into_iter().map(|(_, p)| p.into_iter().filter(|q| q.0 >= lo && q.0 <= hi).collect()).collect();
        let req = TrackRequest { first: lo, count: (hi - lo + 1) as usize, objects: prompts };
        let frame = std::cell::RefCell::new(frame);
        let mut summary = TrackSummary::default();
        // A frame is finished when every object has reported it (the objects' mattes are united).
        let mut pending: BTreeMap<i64, Vec<Option<effectcraft_segment::TrackedFrame>>> = BTreeMap::new();
        let finish = |g: i64, parts: Vec<Option<effectcraft_segment::TrackedFrame>>, summary: &mut TrackSummary, progress: &mut dyn FnMut(usize)| {
            let (Some(key), Some(img)) = (self.keys.get(&g).copied(), (frame.borrow_mut())(g)) else { return };
            let mut segs: Vec<(u32, effectcraft_track::roto::FrameSeg)> = vec![];
            let (mut confidence, mut present) = (f32::INFINITY, false);
            let mut worst = f32::INFINITY;
            for (obj, t) in objects.iter().zip(parts) {
                let Some(t) = t else { continue };
                let strokes = self.data.strokes_of(g, *obj);
                segs.push((*obj, rb::segment_tracked(&img, &strokes, t.prob, t.occluded, &self.opts, self.refine_radius)));
                worst = worst.min(t.confidence);
                if !t.occluded {
                    present = true;
                    confidence = confidence.min(t.confidence);
                }
            }
            if segs.is_empty() {
                return;
            }
            // a frame is occluded when every object is absent; otherwise the doubtful object sets the confidence
            let (occluded, confidence) = (!present, if present { confidence } else { worst });
            // with several objects each keeps its own matte too (Show Object, the per-object overlay)
            let several = objects.len() > 1;
            let mut parts = Vec::with_capacity(segs.len());
            for (obj, seg) in segs {
                if several {
                    store(object_key(key, obj), self.opts.scale, Arc::new(seg.clone()));
                }
                parts.push(seg);
            }
            store(tracked_key(key), self.opts.scale, Arc::new(effectcraft_track::roto::FrameSeg::union(parts)));
            trust_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(tracked_key(key), FrameTrust { confidence, occluded });
            summary.frames += 1;
            summary.occluded += usize::from(occluded);
            summary.min_confidence = if summary.frames == 1 { confidence } else { summary.min_confidence.min(confidence) };
            progress(summary.frames);
        };
        let n_objects = objects.len();
        tracker.track(
            &req,
            &mut |g| {
                let img = (frame.borrow_mut())(g)?;
                Some(Arc::new(ClipFrame { rgb: rb::rgb_of(&img), w: img.width as usize, h: img.height as usize }))
            },
            cancel,
            &mut |o, g, t| {
                let parts = pending.entry(g).or_insert_with(|| vec![None; n_objects]);
                if let Some(slot) = parts.get_mut(o) {
                    *slot = Some(t);
                }
                if parts.iter().all(Option::is_some) {
                    let parts = pending.remove(&g).unwrap_or_default();
                    finish(g, parts, &mut summary, progress);
                }
            },
        )?;
        // frames some object never reported (it was not followed there): united from the rest
        for (g, parts) in std::mem::take(&mut pending) {
            finish(g, parts, &mut summary, progress);
        }
        Ok(Some(summary))
    }
}

/// Each object's matte of layer frame `f` where the tracker followed several objects (full resolution; computes
/// nothing). Show Object limits it to the one chosen. Empty with fewer than two objects.
pub fn object_mattes(params: &Params, layer_size: [f64; 2], fps: f64, f: i64) -> Vec<(u32, Arc<FrameSeg>)> {
    let mut chain = Chain::new(params, layer_size, fps, 1.0);
    let objects = chain.data.objects();
    if objects.len() < 2 || !chain.data.in_span(f) {
        return vec![];
    }
    let shown = chain.show_object.checked_sub(1);
    chain.show_object = 0;
    objects.into_iter().filter(|o| shown.is_none_or(|s| s == *o)).filter_map(|o| chain.object_seg(f, o).map(|s| (o, s))).collect()
}

/// Nearest-neighbour resampling of a segmentation to another scale.
pub fn downscale(s: &FrameSeg, scale: f64) -> FrameSeg {
    let (w, h) = (((s.w as f64) * scale).round().max(1.0) as usize, ((s.h as f64) * scale).round().max(1.0) as usize);
    resample(s, w, h)
}

pub fn resample(s: &FrameSeg, w: usize, h: usize) -> FrameSeg {
    if s.w == w && s.h == h {
        return s.clone();
    }
    let mut out = FrameSeg { w, h, matte: vec![0; w * h], refine: vec![0; w * h], fg: s.fg.clone(), bg: s.bg.clone() };
    if s.w == 0 || s.h == 0 {
        return out;
    }
    for y in 0..h {
        let sy = (((y as f64 + 0.5) * s.h as f64 / h as f64) as usize).min(s.h - 1);
        for x in 0..w {
            let sx = (((x as f64 + 0.5) * s.w as f64 / w as f64) as usize).min(s.w - 1);
            out.matte[y * w + x] = s.matte[sy * s.w + sx];
            out.refine[y * w + x] = s.refine.get(sy * s.w + sx).copied().unwrap_or(0);
        }
    }
    out
}

/// The final matte of frame `seg` (with its neighbours), on its own grid.
pub fn final_alpha(
    img: &Image,
    seg: &FrameSeg,
    prev: Option<&FrameSeg>,
    next: Option<&FrameSeg>,
    mp: &MatteParams,
    refine_radius: f64,
    scale: f64,
) -> Vec<f32> {
    let rgb = rb::rgb_of(img);
    refine::matte_alpha(&rgb, seg, prev, next, mp, refine_radius * scale, scale)
}

fn needs_neighbours(mp: &MatteParams) -> bool {
    mp.rb_chatter > 0.0 || mp.chatter > 0.0 || mp.motion_blur
}

// ------------------------------------------------------------------ Freeze

/// Frozen mattes (Freeze): the final 8-bit alpha and the Refine Edge band of every frame of
/// the span at full resolution, valid while `key` matches [`frozen_key`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FrozenCache {
    pub key: String,
    pub size: [usize; 2],
    pub frames: BTreeMap<i64, FrozenFrame>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrozenFrame {
    /// Alpha (RLE, base64).
    pub a: String,
    /// Refine Edge band (RLE, base64).
    pub b: String,
}

impl FrozenCache {
    pub fn from_json(s: &str) -> Option<FrozenCache> {
        if s.trim().is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What frozen mattes depend on: every chain key and the matte settings.
pub fn frozen_key(params: &Params, layer_size: [f64; 2], fps: f64) -> String {
    let ch = Chain::new(params, layer_size, fps, 1.0);
    let mut h = 0xcbf2_9ce4_8422_2325;
    for (f, k) in &ch.keys {
        fnv(&mut h, &f.to_le_bytes());
        fnv(&mut h, &k.to_le_bytes());
    }
    fnv(&mut h, format!("{:?}", matte_params(params)).as_bytes());
    if ch.show_object != 0 {
        fnv(&mut h, &ch.show_object.to_le_bytes());
    }
    format!("{h:016x}")
}

/// Whether the instance is frozen (has frozen mattes).
pub fn is_frozen(params: &Params) -> bool {
    !s(params, FROZEN).is_empty()
}

type FrozenParsed = Mutex<HashMap<u64, Arc<FrozenCache>>>;

fn frozen(params: &Params) -> Option<Arc<FrozenCache>> {
    static C: OnceLock<FrozenParsed> = OnceLock::new();
    let j = s(params, FROZEN);
    if j.is_empty() {
        return None;
    }
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, j.as_bytes());
    let c = C.get_or_init(Default::default);
    if let Some(d) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&h) {
        return Some(d.clone());
    }
    let d = Arc::new(FrozenCache::from_json(j)?);
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 8 {
        m.clear();
    }
    m.insert(h, d.clone());
    Some(d)
}

// ------------------------------------------------------------------ saved tracking

/// The tracker's frames of an instance, saved with the project (the hidden [`TRACKED`] parameter,
/// as [`FrozenCache`] is). They apply while `key` matches the instance's inputs ([`tracks_key`]),
/// whichever model Settings choose: a stroke or span change makes them stale, and the next Propagate
/// replaces them (with the classic engine, that leaves none).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TrackCache {
    /// What the frames were tracked from ([`tracks_key`]).
    pub key: String,
    /// The tracker's model id.
    pub model: String,
    /// The mattes' width and height (the layer's size).
    pub size: [usize; 2],
    pub frames: BTreeMap<i64, TrackFrame>,
    /// With several objects: each one's matte of every tracked frame (object id, then frame).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub objects: BTreeMap<u32, BTreeMap<i64, ObjectFrame>>,
}

/// One object's matte and Refine Edge band of a tracked frame (RLE, base64).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectFrame {
    pub m: String,
    pub r: String,
}

/// One tracked frame: its matte and Refine Edge band (RLE, base64), its colour models and the
/// tracker's trust in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackFrame {
    /// Matte (RLE, base64).
    pub m: String,
    /// Refine Edge band (RLE, base64).
    pub r: String,
    pub fg: Gmm,
    pub bg: Gmm,
    /// 0–1: the tracker's confidence.
    pub c: f32,
    /// The object is not in the frame.
    pub o: bool,
}

impl TrackCache {
    pub fn from_json(s: &str) -> Option<TrackCache> {
        if s.trim().is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What saved tracking depends on: the chain keys of its frames (the inputs and the model).
pub fn tracks_key(keys: &BTreeMap<i64, u64>) -> String {
    let mut h = 0xcbf2_9ce4_8422_2325;
    for (f, k) in keys {
        fnv(&mut h, &f.to_le_bytes());
        fnv(&mut h, &k.to_le_bytes());
    }
    format!("{h:016x}")
}

/// The saved tracking of `params`, parsed once per content.
fn saved_tracks(params: &Params) -> Option<Arc<TrackCache>> {
    static C: OnceLock<Mutex<HashMap<u64, Arc<TrackCache>>>> = OnceLock::new();
    let j = s(params, TRACKED);
    if j.is_empty() {
        return None;
    }
    let mut h = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, j.as_bytes());
    let c = C.get_or_init(Default::default);
    if let Some(d) = c.lock().unwrap_or_else(|e| e.into_inner()).get(&h) {
        return Some(d.clone());
    }
    let d = Arc::new(TrackCache::from_json(j)?);
    let mut m = c.lock().unwrap_or_else(|e| e.into_inner());
    if m.len() > 8 {
        m.clear();
    }
    m.insert(h, d.clone());
    Some(d)
}

/// A saved tracking that applies to a chain: its frames, keyed under the model that made them.
pub(crate) struct SavedTracking {
    cache: Arc<TrackCache>,
    keys: BTreeMap<i64, u64>,
}

/// The saved tracking of `params` if it applies: the inputs are the instance's now. It does not depend
/// on which model Settings choose or has loaded, so a project renders the same anywhere.
fn saved_for(params: &Params, data: &RotoData, layer_size: [f64; 2], fps: f64) -> Option<SavedTracking> {
    let cache = saved_tracks(params)?;
    let keys = data.chain_keys(seed_with(params, layer_size, fps, &cache.model));
    (tracks_key(&keys) == cache.key).then_some(SavedTracking { cache, keys })
}

/// Decode a tracked frame of saved tracking (full resolution).
fn decode_tracked(cache: &TrackCache, f: i64) -> Option<FrameSeg> {
    let fr = cache.frames.get(&f)?;
    // The size comes from the project file: checked, so a corrupt one is ignored rather than overflowing.
    let [w, h] = cache.size;
    let n = w.checked_mul(h)?;
    let matte = rle::decode(&fr.m)?;
    let refine = rle::decode(&fr.r)?;
    (matte.len() == n && refine.len() == n).then(|| FrameSeg { w, h, matte, refine, fg: fr.fg.clone(), bg: fr.bg.clone() })
}

/// Object `o`'s matte of a tracked frame of saved tracking (full resolution).
fn decode_object(cache: &TrackCache, f: i64, o: u32) -> Option<FrameSeg> {
    let fr = cache.objects.get(&o)?.get(&f)?;
    let [w, h] = cache.size;
    let n = w.checked_mul(h)?;
    let matte = rle::decode(&fr.m)?;
    let refine = rle::decode(&fr.r)?;
    (matte.len() == n && refine.len() == n).then(|| FrameSeg { w, h, matte, refine, ..Default::default() })
}

/// The tracking a chain has just made, to save with its instance: every frame of its span that the
/// tracker followed, at full resolution, with its trust. `None` when it followed none.
pub fn tracks_of(ch: &Chain) -> Option<TrackCache> {
    let model = ch.model.as_ref()?.info().id;
    let mut size = None;
    let mut frames = BTreeMap::new();
    for (f, k) in &ch.keys {
        let (Some(seg), Some(t)) = (cached(tracked_key(*k), 1.0), trust(*k)) else { continue };
        size.get_or_insert([seg.w, seg.h]);
        frames.insert(
            *f,
            TrackFrame { m: rle::encode(&seg.matte), r: rle::encode(&seg.refine), fg: seg.fg.clone(), bg: seg.bg.clone(), c: t.confidence, o: t.occluded },
        );
    }
    let mut objects: BTreeMap<u32, BTreeMap<i64, ObjectFrame>> = BTreeMap::new();
    let ids = ch.data.objects();
    if ids.len() > 1 {
        for (f, k) in &ch.keys {
            if !frames.contains_key(f) {
                continue;
            }
            for o in &ids {
                if let Some(seg) = cached(object_key(*k, *o), 1.0) {
                    objects.entry(*o).or_default().insert(*f, ObjectFrame { m: rle::encode(&seg.matte), r: rle::encode(&seg.refine) });
                }
            }
        }
    }
    let cache = TrackCache { key: tracks_key(&ch.keys), model: model.to_string(), size: size?, frames, objects };
    (!cache.frames.is_empty()).then_some(cache)
}

/// Encode one frame for [`FrozenCache`].
pub fn freeze_frame(alpha: &[f32], band: &[u8]) -> FrozenFrame {
    let a: Vec<u8> = alpha.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    FrozenFrame { a: rle::encode(&a), b: rle::encode(band) }
}

// ------------------------------------------------------------------ render

/// A frame's matte (alpha and band) on the layer grid of `img` at `scale`.
fn matte_for(ctx: &EffectCtx, f: i64, img: &Arc<Image>, scale: f64) -> Option<(Vec<f32>, Vec<u8>, f64)> {
    let (w, h) = (img.width as usize, img.height as usize);
    // Frozen mattes.
    if let Some(fc) = frozen(ctx.params)
        && fc.key == frozen_key(ctx.params, ctx.layer_size, ctx.fps())
        && let Some(fr) = fc.frames.get(&f)
    {
        let a = rle::decode(&fr.a)?;
        let bnd = rle::decode(&fr.b)?;
        let [fw, fh] = fc.size;
        if a.len() == fw * fh && bnd.len() == fw * fh {
            let af: Vec<f32> = a.iter().map(|v| *v as f32 / 255.0).collect();
            let (alpha, band) = if fw == w && fh == h {
                (af, bnd)
            } else {
                let mut al = vec![0.0f32; w * h];
                let mut bd = vec![0u8; w * h];
                for y in 0..h {
                    for x in 0..w {
                        let (sx, sy) = ((x as f64 + 0.5) * fw as f64 / w as f64, (y as f64 + 0.5) * fh as f64 / h as f64);
                        al[y * w + x] = rb::matting::sample(&af, fw, fh, sx, sy);
                        bd[y * w + x] = bnd[(sy as usize).min(fh - 1) * fw + (sx as usize).min(fw - 1)];
                    }
                }
                (al, bd)
            };
            let rr = data(ctx.params).refine_radius();
            return Some((alpha, band, rr));
        }
    }
    let host = ctx.env.host;
    let chain = Chain::new(ctx.params, ctx.layer_size, ctx.fps(), scale);
    let fps = ctx.fps();
    let effects = ctx.env.effect_index;
    let layer_size = ctx.layer_size;
    let me = img.clone();
    let mut frame = |g: i64| -> Option<Arc<Image>> {
        if g == f {
            return Some(me.clone());
        }
        let buf = host?.self_at(g as f64 / fps, effects)?;
        let im = layer_grid(&buf, layer_size);
        (im.width as usize == w && im.height as usize == h).then(|| Arc::new(im))
    };
    let seg = chain.ensure(f, &mut frame, &|| false)?;
    let seg = if seg.w == w && seg.h == h { (*seg).clone() } else { resample(&seg, w, h) };
    let mp = matte_params(ctx.params);
    let (mut prev, mut next) = (None, None);
    if needs_neighbours(&mp) {
        for (slot, g) in [(&mut prev, f - 1), (&mut next, f + 1)] {
            if chain.data.in_span(g)
                && let Some(s) = chain.ensure(g, &mut frame, &|| false)
            {
                *slot = Some(if s.w == w && s.h == h { (*s).clone() } else { resample(&s, w, h) });
            }
        }
    }
    let alpha = final_alpha(img, &seg, prev.as_ref(), next.as_ref(), &mp, chain.refine_radius, scale);
    Some((alpha, seg.refine, chain.refine_radius))
}

fn render(ctx: &EffectCtx, buf: Buf) -> Buf {
    let d = data(ctx.params);
    if d.is_empty() || ctx.adjustment {
        return buf;
    }
    let f = frame_of(ctx.time, ctx.fps());
    let sc = if buf.scale > 0.0 { buf.scale } else { 1.0 };
    let mut out = Buf { img: Image::new(buf.img.width, buf.img.height), offset: buf.offset, scale: buf.scale };
    if !d.in_span(f) {
        return out;
    }
    let img = Arc::new(layer_grid(&buf, ctx.layer_size));
    let Some((alpha, band, rr)) = matte_for(ctx, f, &img, sc) else { return out };
    let (w, h) = (img.width as usize, img.height as usize);
    let mp = matte_params(ctx.params);
    let mut rgb = rb::rgb_of(&img);
    let map = refine::decontaminate(&mut rgb, &alpha, &band, w, h, &mp, rr * sc, sc);
    let view_map = b(ctx.params, "refineEdgeMatte/decontamination/viewDecontaminationMap");
    let (ox, oy) = (buf.offset[0].round() as i64, buf.offset[1].round() as i64);
    for y in 0..h {
        for x in 0..w {
            let (bx, by) = (x as i64 + ox, y as i64 + oy);
            if bx < 0 || by < 0 || bx >= out.img.width as i64 || by >= out.img.height as i64 {
                continue;
            }
            let i = y * w + x;
            let a_in = img.data[i][3];
            let px = if view_map {
                let v = map[i];
                [v, v, v, 1.0]
            } else {
                let a = alpha[i] * a_in;
                [rgb[i][0] * a, rgb[i][1] * a, rgb[i][2] * a, a]
            };
            out.img.set(bx as u32, by as u32, px);
        }
    }
    out
}

/// The final matte of the effect at layer frame `f` on the full-resolution layer grid of
/// `input` (the effect's input buffer at scale 1), computing what is missing. For Freeze and
/// the Layer panel overlays.
pub fn frame_matte(
    params: &Params,
    layer_size: [f64; 2],
    fps: f64,
    f: i64,
    frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>,
    cancel: &dyn Fn() -> bool,
) -> Option<(Vec<f32>, Vec<u8>)> {
    let chain = Chain::new(params, layer_size, fps, 1.0);
    let img = frame(f)?;
    let seg = chain.ensure(f, frame, cancel)?;
    let mp = matte_params(params);
    let (mut prev, mut next) = (None, None);
    if needs_neighbours(&mp) {
        if chain.data.in_span(f - 1) {
            prev = chain.ensure(f - 1, frame, cancel);
        }
        if chain.data.in_span(f + 1) {
            next = chain.ensure(f + 1, frame, cancel);
        }
    }
    let alpha = final_alpha(&img, &seg, prev.as_deref(), next.as_deref(), &mp, chain.refine_radius, 1.0);
    Some((alpha, seg.refine.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectEnv, EffectHost, LayerPixels};

    /// A host serving a moving red disk over a blue/green background as the effect's input.
    struct Seq;
    fn frame(t: f64) -> Image {
        let fi = (t * 25.0 + 1e-6).floor();
        let c = [40.0 + 2.0 * fi, 40.0];
        let mut img = Image::new(100, 80);
        for y in 0..80 {
            for x in 0..100 {
                let d = ((x as f64 + 0.5 - c[0]).hypot(y as f64 + 0.5 - c[1])) - 15.0;
                let a = (0.5 - d).clamp(0.0, 1.0) as f32;
                let tex = ((x * 7 + y * 13) % 9) as f32 / 60.0;
                let fg = [0.85 + tex, 0.2, 0.1];
                let bg = [0.1, 0.4 + tex, 0.7];
                img.set(x, y, [fg[0] * a + bg[0] * (1.0 - a), fg[1] * a + bg[1] * (1.0 - a), fg[2] * a + bg[2] * (1.0 - a), 1.0]);
            }
        }
        img
    }
    impl EffectHost for Seq {
        fn layer(&self, _: u64, _: bool) -> Option<LayerPixels> {
            None
        }
        fn audio(&self, _: u64, _: f64, _: usize, _: u32) -> Option<Vec<f32>> {
            None
        }
        fn self_at(&self, t: f64, _: usize) -> Option<Buf> {
            Some(Buf { img: frame(t), offset: [0.0; 2], scale: 1.0 })
        }
    }

    fn params_with(d: &RotoData) -> Params {
        let s = &specs()[0];
        let mut params = Params { values: s.params.iter().map(|p| (p.id.to_string(), p.default.clone())).collect() };
        params.values.insert(STROKES.into(), Value::Str(d.to_json()));
        params
    }

    #[test]
    fn renders_the_segmented_disk_on_demand() {
        let mut d = RotoData::default();
        d.add(Stroke { kind: StrokeKind::Fg, frame: 0, radius: 3.0, points: vec![[35.0, 40.0], [45.0, 40.0]], object: 0 }, 20, [0, 24]);
        d.add(Stroke { kind: StrokeKind::Bg, frame: 0, radius: 3.0, points: vec![[5.0, 5.0], [95.0, 5.0], [95.0, 75.0]], object: 0 }, 20, [0, 24]);
        let params = params_with(&d);
        let host = Seq;
        let run = |t: f64| {
            let ctx = EffectCtx {
                params: &params,
                time: t,
                layer_size: [100.0, 80.0],
                seed: 1,
                adjustment: false,
                env: EffectEnv { host: Some(&host), frame_rate: 25.0, ..Default::default() },
            };
            crate::apply(&specs()[0], &ctx, Buf { img: frame(t), offset: [0.0; 2], scale: 1.0 }).img
        };
        // Frame 5 (propagated through frames 1–4).
        let out = run(5.0 / 25.0 + 0.001);
        let c = [50.0, 40.0];
        let mut wrong = 0;
        for y in 0..80u32 {
            for x in 0..100u32 {
                let d = (x as f64 + 0.5 - c[0]).hypot(y as f64 + 0.5 - c[1]);
                let a = out.data[(y * 100 + x) as usize][3];
                if (d < 14.0 && a < 0.5) || (d > 16.0 && a > 0.5) {
                    wrong += 1;
                }
            }
        }
        assert!(wrong < 20, "{wrong} wrong pixels");
        // Outside the span: transparent.
        let mut d2 = (*data(&params)).clone();
        d2.span = [0, 3];
        let p2 = params_with(&d2);
        let ctx = EffectCtx {
            params: &p2,
            time: 0.3,
            layer_size: [100.0, 80.0],
            seed: 1,
            adjustment: false,
            env: EffectEnv { host: Some(&host), frame_rate: 25.0, ..Default::default() },
        };
        let o = crate::apply(&specs()[0], &ctx, Buf { img: frame(0.3), offset: [0.0; 2], scale: 1.0 }).img;
        assert!(o.data.iter().all(|p| p[3] == 0.0));
    }

    /// A stand-in tracker for two objects: object 0 is the moving disk, object 1 a fixed square. It records how many
    /// objects (and prompts per object) it was asked for.
    struct TwoObjects(std::sync::Mutex<Vec<Vec<usize>>>);
    impl effectcraft_segment::MaskModel for TwoObjects {
        fn info(&self) -> &'static effectcraft_segment::ModelInfo {
            &effectcraft_segment::SAM2_TINY
        }
        fn segment(&self, _: &[[f32; 3]], _: usize, _: usize, _: &effectcraft_segment::Prompt) -> effectcraft_segment::Result<Vec<f32>> {
            Err("no weights".into())
        }
        fn tracker(&self) -> Option<&dyn effectcraft_segment::ObjectTracker> {
            Some(self)
        }
    }
    impl effectcraft_segment::ObjectTracker for TwoObjects {
        fn track(
            &self,
            req: &effectcraft_segment::TrackRequest,
            frame: &mut dyn FnMut(i64) -> Option<Arc<ClipFrame>>,
            _: &dyn Fn() -> bool,
            out: &mut dyn FnMut(usize, i64, effectcraft_segment::TrackedFrame),
        ) -> effectcraft_segment::Result<()> {
            self.0.lock().unwrap().push(req.objects.iter().map(Vec::len).collect());
            for g in req.first..req.first + req.count as i64 {
                let img = frame(g).ok_or("no frame")?;
                let c = [40.0 + 2.0 * g as f64, 40.0];
                // the objects report in reverse order on odd frames: the union must not depend on it
                let order: Vec<usize> = if g % 2 == 1 { (0..req.objects.len()).rev().collect() } else { (0..req.objects.len()).collect() };
                for o in order {
                    let prob = (0..img.w * img.h)
                        .map(|i| {
                            let (x, y) = ((i % img.w) as f64 + 0.5, (i / img.w) as f64 + 0.5);
                            let inside = if o == 0 { (x - c[0]).hypot(y - c[1]) <= 15.0 } else { (8.0..22.0).contains(&x) && (8.0..22.0).contains(&y) };
                            f32::from(inside)
                        })
                        .collect();
                    out(o, g, effectcraft_segment::TrackedFrame { prob, w: img.w, h: img.h, confidence: if o == 0 { 0.9 } else { 0.4 }, occluded: false });
                }
            }
            Ok(())
        }
    }

    #[test]
    fn objects_are_followed_in_one_pass_and_their_mattes_united() {
        let mut d = RotoData::default();
        let stroke = |kind, points: Vec<[f64; 2]>, object| Stroke { kind, frame: 0, radius: 3.0, points, object };
        d.add(stroke(StrokeKind::Fg, vec![[35.0, 40.0], [45.0, 40.0]], 0), 8, [0, 24]);
        d.add(stroke(StrokeKind::Fg, vec![[12.0, 15.0], [18.0, 15.0]], 1), 8, [0, 24]);
        d.add(stroke(StrokeKind::Bg, vec![[85.0, 70.0], [95.0, 70.0]], 0), 8, [0, 24]);
        assert_eq!(d.objects(), vec![0, 1]);
        // the Refine Edge strokes belong to every object; the others only to their own
        assert_eq!(d.strokes_of(0, 1).len(), 1);
        assert_eq!(d.strokes_of(0, 0).len(), 2);
        let params = params_with(&d);
        let mut chain = Chain::new(&params, [100.0, 80.0], 25.0, 1.0);
        let fake = Arc::new(TwoObjects(Default::default()));
        chain.model = Some(fake.clone());
        // the clip: the moving disk and a fixed yellow square
        let mut get = |f: i64| {
            let mut img = frame(f as f64 / 25.0 + 0.001);
            for y in 8..22 {
                for x in 8..22 {
                    img.set(x, y, [0.95, 0.9, 0.1 + ((x * 3 + y * 5) % 7) as f32 / 70.0, 1.0]);
                }
            }
            Some(Arc::new(img))
        };
        let mut seen = 0;
        let summary = chain.track(&mut get, &|| false, &mut |n| seen = n).unwrap().unwrap();
        // one request for both objects; object 0 has its foreground and background prompt on the frame, object 1 its own
        assert_eq!(fake.0.lock().unwrap().len(), 1);
        assert_eq!(fake.0.lock().unwrap()[0], vec![1, 1]);
        assert_eq!(summary.frames, 9);
        assert_eq!(seen, 9);
        // the least sure object sets the frame's confidence
        assert!((summary.min_confidence - 0.4).abs() < 1e-6, "{}", summary.min_confidence);
        assert_eq!(summary.occluded, 0);
        let seg = cached(tracked_key(chain.keys[&3]), 1.0).expect("frame 3 is tracked");
        let at = |x: usize, y: usize| seg.matte[y * 100 + x];
        assert!(at(46, 40) != 0, "the moving disk (frame 3 centre x = 46)");
        assert!(at(15, 15) != 0, "the fixed square");
        assert_eq!(at(60, 70), 0, "nothing elsewhere");

        // each object keeps its own matte
        let (disk, square) = (chain.object_seg(3, 0).expect("object 0"), chain.object_seg(3, 1).expect("object 1"));
        assert!(disk.matte[40 * 100 + 46] != 0 && disk.matte[15 * 100 + 15] == 0);
        assert!(square.matte[15 * 100 + 15] != 0 && square.matte[40 * 100 + 46] == 0);
        // Show Object picks one (its id plus one); an object the instance does not have shows nothing; 0 shows all
        let mut shown = |c: &Chain| c.ensure(3, &mut get, &|| false).expect("frame 3");
        chain.show_object = 2;
        let one = shown(&chain);
        assert!(one.matte[15 * 100 + 15] != 0 && one.matte[40 * 100 + 46] == 0, "only the square");
        chain.show_object = 1;
        let zero = shown(&chain);
        assert!(zero.matte[40 * 100 + 46] != 0 && zero.matte[15 * 100 + 15] == 0, "only the disk");
        chain.show_object = 7;
        assert_eq!(shown(&chain).area(), 0, "no such object");
        chain.show_object = 0;
        assert!(shown(&chain).matte[15 * 100 + 15] != 0, "all together");

        // the objects are saved with the tracking and come back from the project file
        let saved = tracks_of(&chain).expect("tracking to save");
        assert_eq!(saved.objects.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(saved.objects.get(&1).map(|f| f.len()), Some(9));
        assert_eq!(TrackCache::from_json(&saved.to_json()), Some(saved.clone()));
        // a tracking saved before objects existed has none
        let old = TrackCache { objects: Default::default(), ..saved.clone() };
        assert!(!old.to_json().contains("objects"));
        let rt = decode_object(&saved, 3, 1).expect("decoded");
        assert!(rt.matte[15 * 100 + 15] != 0 && rt.matte[40 * 100 + 46] == 0);
    }

    /// Instances saved before the propagation controls got their own twirl-down (and motion
    /// blur / decontamination their nested ones) load with their values in the new places.
    #[test]
    fn old_group_layout_is_migrated_on_load() {
        use effectcraft_project::build::Ids;
        let spec = crate::find(ID).unwrap();
        let mut next = 1;
        let mut ids = Ids(&mut next);
        // The old layout: everything under Roto Brush Matte / Refine Edge Matte.
        let mut g = ids.group(ID, "Roto Brush & Refine Edge");
        g.kind = effectcraft_project::GroupKind::Effect { effect: ID.into() };
        let mut rb = ids.group("rotoBrushMatte", "Roto Brush Matte");
        rb.children.push(ids.prop("searchRadius", "Search Radius", Value::Scalar(33.0)).into());
        rb.children.push(ids.prop("feather", "Feather", Value::Scalar(2.0)).into());
        let mut re = ids.group("refineEdgeMatte", "Refine Edge Matte");
        re.children.push(ids.prop("shutterAngle", "Shutter Angle", Value::Scalar(90.0)).into());
        re.children.push(ids.prop("decontaminationAmount", "Decontamination Amount", Value::Scalar(40.0)).into());
        g.children.push(rb.into());
        g.children.push(re.into());
        assert!(crate::migrate::upgrade_instance(spec, &mut g, &mut ids, [100.0, 80.0]));
        let params = crate::flatten_params(&g, &mut |p| p.value.clone());
        assert_eq!(f(&params, "rotoBrushPropagation/searchRadius"), 33.0);
        assert_eq!(f(&params, "rotoBrushMatte/feather"), 2.0);
        assert_eq!(f(&params, "refineEdgeMatte/motionBlur/shutterAngle"), 90.0);
        assert_eq!(f(&params, "refineEdgeMatte/decontamination/decontaminationAmount"), 40.0);
        assert!(g.sub("rotoBrushMatte").unwrap().get("searchRadius").is_none());
        assert_eq!(g.sub("rotoBrushPropagation").unwrap().name, "Roto Brush Propagation");
        // Parameters the old save lacked get their defaults.
        assert_eq!(f(&params, "rotoBrushPropagation/motionDamping"), 20.0);
    }
}
