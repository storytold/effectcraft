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
use effectcraft_segment::MaskModel;
use effectcraft_track::roto::refine::{self, MatteParams};
use effectcraft_track::roto::{self as rb, RotoData, SegOpts, rle};
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

/// The model an instance segments with: its version's choice of the installed one.
pub fn model_for(params: &Params) -> Option<Arc<dyn MaskModel>> {
    if e(params, "version") == 0 {
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
    fnv(&mut h, model_for(params).map_or(effectcraft_segment::CLASSICAL, |m| m.info().id).as_bytes());
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

/// Drop the cached segmentations of these chain keys (every scale).
pub fn forget(keys: impl IntoIterator<Item = u64>) {
    let keys: std::collections::HashSet<u64> = keys.into_iter().collect();
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

/// Everything needed to compute an instance's segmentations.
pub struct Chain {
    pub data: Arc<RotoData>,
    pub keys: BTreeMap<i64, u64>,
    pub opts: SegOpts,
    pub refine_radius: f64,
    /// The trained model (Version 2.0 / 3.0 with one installed).
    pub model: Option<Arc<dyn MaskModel>>,
}

impl Chain {
    pub fn new(params: &Params, layer_size: [f64; 2], fps: f64, scale: f64) -> Chain {
        let data = data(params);
        let keys = data.chain_keys(seed(params, layer_size, fps));
        Chain { refine_radius: data.refine_radius(), data, keys, opts: seg_opts(params, scale), model: model_for(params) }
    }

    /// Frame `f`'s segmentation at this chain's scale: cached, or computed from the nearest
    /// cached frame towards the base (full-resolution results serve smaller scales).
    /// `frame(g)`: the effect's input on frame g on the layer grid at this scale.
    pub fn ensure(&self, f: i64, frame: &mut dyn FnMut(i64) -> Option<Arc<Image>>, cancel: &dyn Fn() -> bool) -> Option<Arc<FrameSeg>> {
        let sc = self.opts.scale;
        let lookup = |g: i64| -> Option<Arc<FrameSeg>> {
            let k = *self.keys.get(&g)?;
            if let Some(s) = cached(k, sc).or_else(|| from_store(k, sc)) {
                return Some(s);
            }
            if sc != 1.0 {
                let full = cached(k, 1.0).or_else(|| from_store(k, 1.0))?;
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
                    rb::propagate_with(&pimg, p, &img, &strokes, &self.opts, self.refine_radius, self.model.as_deref())
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
        self.keys.get(&f).is_some_and(|k| cached(*k, 1.0).is_some() || cached(*k, scale).is_some())
    }
}

/// Nearest-neighbour resampling of a segmentation to another scale.
pub fn downscale(s: &FrameSeg, scale: f64) -> FrameSeg {
    let (w, h) = (((s.w as f64) * scale).round().max(1.0) as usize, ((s.h as f64) * scale).round().max(1.0) as usize);
    resample(s, w, h)
}

pub(crate) fn resample(s: &FrameSeg, w: usize, h: usize) -> FrameSeg {
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
        d.add(Stroke { kind: StrokeKind::Fg, frame: 0, radius: 3.0, points: vec![[35.0, 40.0], [45.0, 40.0]] }, 20, [0, 24]);
        d.add(Stroke { kind: StrokeKind::Bg, frame: 0, radius: 3.0, points: vec![[5.0, 5.0], [95.0, 5.0], [95.0, 75.0]] }, 20, [0, 24]);
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
