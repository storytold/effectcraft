//! Effect objects and instances: the handle behind `OfxImageEffectHandle`, the per-render
//! context the suites consult, the geometry that maps EffectCraft buffers to OFX canonical /
//! pixel coordinates, and the actions an instance receives.
//!
//! ## Coordinates
//!
//! EffectCraft buffers are y-down; OFX is y-up with "canonical" coordinates (layer pixels at
//! render scale 1, `Y` = 0 at the bottom of the layer). For a buffer of `w x h` pixels whose
//! top-left is at layer position `-origin / scale` (`origin` = buffer pixel of layer pixel
//! `(0, 0)`), and a layer `layer_size` tall, buffer row `r` (0 = top) is OFX pixel row
//! `round(layer_h * scale) - 1 - (r - origin.y)`. [`Geometry`] does that arithmetic once.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use effectcraft_effects::plugin::{PluginHost, PluginImage};

use crate::clips::{ClipImage, ClipObj, Comps, Depth};
use crate::ffi::*;
use crate::params::{Bindings, CoordCtx, ParamObj, ParamSet};
use crate::props::{PVal, PropSet};

/// The marker at the start of every [`EffectObj`].
pub const EFFECT_MAGIC: u32 = 0x4F46_5845; // "OFXE"

/// The nominal layer edge (canonical units) used when the real layer is not known yet
/// (computing padding before a frame exists).
pub const NOMINAL_LAYER: f64 = 1000.0;

/// The last frame reported for a clip when the host does not know the layer's real duration.
const UNKNOWN_LAST_FRAME: f64 = 99999.0;

/// The effect duration (frames) reported when the host does not know the layer's real duration.
const UNKNOWN_DURATION: f64 = 100000.0;

/// The OFX frame range (first, last) of a source lasting from `first` to `end` layer-time seconds
/// at `fps` frames per second: the frame numbers of the first and the last frame, `None` for an
/// empty or non-finite span. A span shorter than a frame still has one frame.
pub fn span_frames(first: f64, end: f64, fps: f64) -> Option<(f64, f64)> {
    if !(first.is_finite() && end.is_finite() && fps.is_finite() && fps > 0.0 && end > first) {
        return None;
    }
    let (a, b) = ((first * fps).round(), (end * fps).round() - 1.0);
    // Frame numbers this large are indistinguishable from "unbounded" and could overflow the
    // ints plug-ins store them in.
    if a.abs() > 1.0e9 || b.abs() > 1.0e9 {
        return None;
    }
    Some((a, b.max(a)))
}

/// `kOfxImageEffectPluginRenderThreadSafety`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// One render at a time for the whole plug-in.
    Unsafe,
    /// One render at a time per instance (the default).
    InstanceSafe,
    /// Any number of concurrent renders, even on one instance.
    FullySafe,
}

/// The role a clip plays for this host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipRole {
    Output,
    /// The effect's own layer (`Source`, or `SourceFrom` in a transition).
    Input,
    /// Another layer chosen through the Layer parameter `param_id`.
    Layer(String),
}

/// What the host decided about one clip.
#[derive(Clone, Debug)]
pub struct ClipCfg {
    pub name: String,
    pub role: ClipRole,
    pub comps: Comps,
    pub optional: bool,
}

/// Static facts about a described effect.
#[derive(Clone, Debug)]
pub struct EffectCfg {
    pub context: String,
    pub clips: Vec<ClipCfg>,
    pub depth: Depth,
    pub safety: Safety,
    pub slave_params: Vec<String>,
}

impl EffectCfg {
    pub fn clip(&self, name: &str) -> Option<&ClipCfg> {
        self.clips.iter().find(|c| c.name == name)
    }
}

/// One loaded plug-in's entry point.
pub struct PluginEntry {
    pub main: EntryPoint,
    pub ofx_id: String,
    /// Serialises plug-ins that declare themselves render-thread-unsafe (shared by all plug-ins
    /// of one library).
    pub lock: Arc<Mutex<()>>,
}

impl PluginEntry {
    /// Dispatch an action to the plug-in.
    // `handle` is never dereferenced here, only handed to the plug-in.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn call(&self, action: &str, handle: *const c_void, in_args: Option<&PropSet>, out_args: Option<&PropSet>) -> OfxStatus {
        let a = cstring(action);
        let i = in_args.map(PropSet::as_handle).unwrap_or(std::ptr::null_mut());
        let o = out_args.map(PropSet::as_handle).unwrap_or(std::ptr::null_mut());
        let main = self.main;
        // SAFETY: `main` is the plug-in's entry point, called with the argument shapes the
        // specification fixes; the handles are alive for the duration of the call.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { main(a.as_ptr(), handle, i, o) })).unwrap_or(K_STAT_ERR_FATAL)
    }
}

/// An image-effect descriptor or instance (`OfxImageEffectHandle`; also the param-set handle).
#[repr(C)]
pub struct EffectObj {
    magic: u32,
    pub props: PropSet,
    pub params: ParamSet,
    pub clips: Mutex<Vec<Box<ClipObj>>>,
    pub is_instance: bool,
    /// The render in progress, if any.
    pub render: Mutex<Option<Arc<RenderCtx>>>,
    /// The last persistent / error message the plug-in posted.
    pub message: Mutex<String>,
}

unsafe impl Send for EffectObj {}
unsafe impl Sync for EffectObj {}

impl EffectObj {
    pub fn new_descriptor() -> Box<EffectObj> {
        let o = Box::new(EffectObj {
            magic: EFFECT_MAGIC,
            props: PropSet::new(),
            params: ParamSet::default(),
            clips: Mutex::new(Vec::new()),
            is_instance: false,
            render: Mutex::new(None),
            message: Mutex::new(String::new()),
        });
        o.props.put_str(P_TYPE, TYPE_IMAGE_EFFECT);
        o
    }

    /// Validate a raw handle.
    ///
    /// # Safety
    /// `h` must be null or point to readable memory; the reference is only used during a callback.
    pub unsafe fn from_handle<'a>(h: *mut c_void) -> Option<&'a EffectObj> {
        if h.is_null() || unsafe { std::ptr::read(h as *const u32) } != EFFECT_MAGIC {
            return None;
        }
        Some(unsafe { &*(h as *const EffectObj) })
    }

    pub fn handle(&self) -> *mut c_void {
        self as *const EffectObj as *mut c_void
    }

    /// Define a clip (descriptors) and return its stable pointer.
    pub fn define_clip(&self, name: &str) -> Result<*const ClipObj, OfxStatus> {
        let mut clips = self.clips.lock().unwrap_or_else(PoisonError::into_inner);
        if clips.iter().any(|c| c.name == name) {
            return Err(K_STAT_ERR_EXISTS);
        }
        if clips.len() >= 64 {
            return Err(K_STAT_ERR_MEMORY);
        }
        let c = Box::new(ClipObj::new(name, self as *const EffectObj));
        let p: *const ClipObj = &*c;
        clips.push(c);
        Ok(p)
    }

    pub fn find_clip(&self, name: &str) -> Option<*const ClipObj> {
        self.clips.lock().unwrap_or_else(PoisonError::into_inner).iter().find(|c| c.name == name).map(|c| &**c as *const ClipObj)
    }

    /// Run `f` on every clip.
    pub fn each_clip(&self, mut f: impl FnMut(&ClipObj)) {
        for c in self.clips.lock().unwrap_or_else(PoisonError::into_inner).iter() {
            f(c);
        }
    }

    pub fn current_render(&self) -> Option<Arc<RenderCtx>> {
        self.render.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn set_render(&self, ctx: Option<Arc<RenderCtx>>) {
        *self.render.lock().unwrap_or_else(PoisonError::into_inner) = ctx;
    }

    pub fn param(&self, name: &str) -> Option<&ParamObj> {
        // The boxes live as long as `self`.
        self.params.find(name).and_then(|p| unsafe { p.as_ref() })
    }

    pub fn set_message(&self, m: &str) {
        *self.message.lock().unwrap_or_else(PoisonError::into_inner) = m.to_string();
    }

    pub fn message(&self) -> String {
        self.message.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The mapping between an EffectCraft buffer and OFX pixel / canonical coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    /// Buffer pixels per layer pixel (the OFX render scale).
    pub scale: f64,
    /// Layer size in layer pixels.
    pub layer: [f64; 2],
    /// Buffer pixel of layer pixel (0, 0).
    pub origin: [f64; 2],
    /// Buffer size.
    pub w: i32,
    pub h: i32,
    /// Transparent pixels the host added on every side for this effect's own region of
    /// definition (the source's RoD is the buffer minus this).
    pub pad: i32,
}

impl Geometry {
    /// A stand-in used before any frame exists.
    pub fn nominal(scale: f64) -> Geometry {
        let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let n = (NOMINAL_LAYER * s).round() as i32;
        Geometry { scale: s, layer: [NOMINAL_LAYER; 2], origin: [0.0; 2], w: n, h: n, pad: 0 }
    }

    fn hs(&self) -> i32 {
        (self.layer[1] * self.scale).round() as i32
    }

    /// OFX pixel bounds of a `w x h` buffer whose layer origin is at `origin`.
    pub fn rect_for(&self, w: i32, h: i32, origin: [f64; 2]) -> OfxRectI {
        let (ox, oy) = (origin[0].round() as i32, origin[1].round() as i32);
        let hs = self.hs();
        OfxRectI { x1: -ox, x2: w - ox, y1: hs - h + oy, y2: hs + oy }
    }

    /// The whole buffer.
    pub fn bounds(&self) -> OfxRectI {
        self.rect_for(self.w, self.h, self.origin)
    }

    /// The buffer minus the padding the host added for this effect.
    pub fn src_rect(&self) -> OfxRectI {
        let b = self.bounds();
        OfxRectI { x1: b.x1 + self.pad, y1: b.y1 + self.pad, x2: b.x2 - self.pad, y2: b.y2 - self.pad }
    }

    pub fn to_canon(&self, r: OfxRectI) -> OfxRectD {
        let s = self.scale;
        OfxRectD { x1: r.x1 as f64 / s, y1: r.y1 as f64 / s, x2: r.x2 as f64 / s, y2: r.y2 as f64 / s }
    }

    pub fn from_canon(&self, r: OfxRectD) -> OfxRectI {
        let s = self.scale;
        let lim = |v: f64| v.clamp(-1.0e7, 1.0e7);
        OfxRectI {
            x1: lim((r.x1 * s).floor()) as i32,
            y1: lim((r.y1 * s).floor()) as i32,
            x2: lim((r.x2 * s).ceil()) as i32,
            y2: lim((r.y2 * s).ceil()) as i32,
        }
    }

    pub fn coord(&self) -> CoordCtx {
        CoordCtx { origin: self.origin, scale: self.scale, layer_h: self.layer[1] }
    }
}

/// Intersection of two pixel rectangles (possibly empty: `x2 <= x1`).
pub fn intersect(a: OfxRectI, b: OfxRectI) -> OfxRectI {
    OfxRectI { x1: a.x1.max(b.x1), y1: a.y1.max(b.y1), x2: a.x2.min(b.x2), y2: a.y2.min(b.y2) }
}

pub fn is_empty(r: OfxRectI) -> bool {
    r.x2 <= r.x1 || r.y2 <= r.y1
}

/// A borrowed [`PluginHost`] smuggled into callbacks for the duration of one render.
pub struct HostPtr(*const (dyn PluginHost + 'static));
// All access goes through the Mutex in `RenderCtx::host`, and the pointer is cleared before the
// borrowed host goes out of scope (`render_v2` removes the context before returning).
unsafe impl Send for HostPtr {}
unsafe impl Sync for HostPtr {}

impl HostPtr {
    pub fn new(h: &dyn PluginHost) -> HostPtr {
        // SAFETY: only the lifetime is erased; see the type's comment.
        let p: &'static dyn PluginHost = unsafe { std::mem::transmute::<&dyn PluginHost, &'static dyn PluginHost>(h) };
        HostPtr(p as *const dyn PluginHost)
    }
}

/// Clip images fetched during one render, by clip name and time (microseconds or 1/1000 frame).
type ImageCache = HashMap<(String, i64), Option<Arc<ClipImage>>>;
/// Parameter values by OFX name.
type ParamValues = Arc<HashMap<String, Vec<PVal>>>;

static IMAGE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Everything the suites need while a render (or a padding query) is in progress.
pub struct RenderCtx {
    pub cfg: Arc<EffectCfg>,
    pub geom: Geometry,
    /// Frames per second (OFX time is in frames).
    pub fps: f64,
    /// Layer time of this render, seconds.
    pub time_sec: f64,
    host: Option<Mutex<HostPtr>>,
    pub bindings: Arc<Bindings>,
    /// Layer chosen for each clip name (`None` = unset).
    pub layer_ids: HashMap<String, Option<u64>>,
    /// The depth the plug-in works in.
    pub depth: Depth,
    /// Component type of each clip name.
    pub comps: HashMap<String, Comps>,
    /// The effect's own layer at the current time.
    pub src_now: Option<Arc<ClipImage>>,
    pub output: Mutex<Option<Arc<ClipImage>>>,
    /// The effect's region of definition, canonical.
    pub rod: Mutex<OfxRectD>,
    cache: Mutex<ImageCache>,
    params_cache: Mutex<HashMap<i64, ParamValues>>,
    /// The source's OFX frame range, asked of the host once ([`RenderCtx::frame_range`]).
    span: OnceLock<Option<(f64, f64)>>,
}

impl RenderCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: Arc<EffectCfg>,
        geom: Geometry,
        fps: f64,
        time_sec: f64,
        host: Option<HostPtr>,
        bindings: Arc<Bindings>,
        layer_ids: HashMap<String, Option<u64>>,
        depth: Depth,
        comps: HashMap<String, Comps>,
        src_now: Option<Arc<ClipImage>>,
    ) -> RenderCtx {
        let rod = geom.to_canon(geom.src_rect());
        RenderCtx {
            cfg,
            geom,
            fps,
            time_sec,
            host: host.map(Mutex::new),
            bindings,
            layer_ids,
            depth,
            comps,
            src_now,
            output: Mutex::new(None),
            rod: Mutex::new(rod),
            cache: Mutex::new(HashMap::new()),
            params_cache: Mutex::new(HashMap::new()),
            span: OnceLock::new(),
        }
    }

    pub fn has_host(&self) -> bool {
        self.host.is_some()
    }

    /// OFX time (frames) to layer seconds.
    pub fn secs(&self, ofx_time: f64) -> f64 {
        ofx_time / self.fps
    }

    fn with_host<R>(&self, f: impl FnOnce(&dyn PluginHost) -> R) -> Option<R> {
        let m = self.host.as_ref()?;
        let g = m.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: see `HostPtr`.
        let h: &dyn PluginHost = unsafe { &*g.0 };
        Some(f(h))
    }

    /// The source's frame range in OFX frames as (first, last), from the host's layer span;
    /// `None` when the host does not know it (no host, or a source without a duration).
    pub fn frame_range(&self) -> Option<(f64, f64)> {
        *self.span.get_or_init(|| self.with_host(|h| h.frame_range()).flatten().and_then(|(first, end)| span_frames(first, end, self.fps)))
    }

    /// Whether OFX time `t` lies within the source's frames (always true when the range is
    /// unknown). The last frame lasts until `last + 1`, so sub-frame times inside it are fine.
    pub fn in_frame_range(&self, t: f64) -> bool {
        match self.frame_range() {
            Some((first, last)) => t.is_finite() && t >= first - 1.0e-6 && t < last + 1.0,
            None => true,
        }
    }

    /// The canonical region of definition of an input clip.
    pub fn input_rod(&self) -> OfxRectD {
        self.geom.to_canon(self.geom.src_rect())
    }

    /// Parameter values (by OFX name) at another time, through the host.
    pub fn params_at(&self, ofx_time: f64) -> Option<Arc<HashMap<String, Vec<PVal>>>> {
        let sec = self.secs(ofx_time);
        let key = (sec * 1.0e6).round() as i64;
        if let Some(v) = self.params_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Some(v.clone());
        }
        let (flat, texts) = self.with_host(|h| h.params_at(sec))??;
        let map = Arc::new(self.bindings.decode(&flat, &texts, &self.geom.coord()));
        let mut cache = self.params_cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(key, map.clone());
        Some(map)
    }

    /// One parameter's values at another time, through the host. Unlike [`RenderCtx::params_at`]
    /// nothing is cached, so sampling a long range (parameter derivatives and integrals) does not
    /// evict the entries a render needs.
    pub fn param_at(&self, name: &str, ofx_time: f64) -> Option<Vec<PVal>> {
        let sec = self.secs(ofx_time);
        let key = (sec * 1.0e6).round() as i64;
        if let Some(v) = self.params_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return v.get(name).cloned();
        }
        let (flat, texts) = self.with_host(|h| h.params_at(sec))??;
        self.bindings.decode(&flat, &texts, &self.geom.coord()).remove(name)
    }

    fn convert(&self, img: PluginImage, comps: Comps) -> Option<Arc<ClipImage>> {
        let (w, h) = (img.width as usize, img.height as usize);
        if w == 0 || h == 0 || img.pixels.len() != w * h {
            return None;
        }
        let bounds = self.geom.rect_for(w as i32, h as i32, img.origin);
        Some(Arc::new(ClipImage::from_rows(&img.pixels, w, (0, 0, w, h), bounds, bounds, self.depth, comps)))
    }

    /// The image of clip `name` at OFX time `t`, or `None` when it is unavailable.
    pub fn image(&self, name: &str, t: f64) -> Option<Arc<ClipImage>> {
        let clip = self.cfg.clip(name)?;
        let comps = self.comps.get(name).copied().unwrap_or(clip.comps);
        if clip.role == ClipRole::Output {
            return self.output.lock().unwrap_or_else(PoisonError::into_inner).clone();
        }
        let sec = self.secs(t);
        // Sub-frame times are real requests (retimers ask for frame 12.5): only a time that is the
        // current one up to rounding is served from the render's own source image.
        let same_time = (sec - self.time_sec).abs() * self.fps < 1.0e-3;
        if clip.role == ClipRole::Input && same_time && self.src_now.is_some() {
            return self.src_now.clone();
        }
        // The source has no frames outside its range: fail the request instead of inventing one.
        if clip.role == ClipRole::Input && !self.in_frame_range(t) {
            return None;
        }
        // Times closer than a thousandth of a frame share one cache entry.
        let key = (name.to_string(), (t * 1.0e3).round() as i64);
        if let Some(v) = self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return v.clone();
        }
        let got = match &clip.role {
            ClipRole::Input => self.with_host(|h| h.input_at(sec)).flatten(),
            ClipRole::Layer(pid) => {
                if self.layer_ids.get(name).copied().flatten().is_none() {
                    None
                } else {
                    self.with_host(|h| h.layer_input(pid, sec)).flatten()
                }
            }
            ClipRole::Output => None,
        };
        let img = got.and_then(|g| self.convert(g, comps));
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(key, img.clone());
        img
    }

    pub fn next_image_id() -> u64 {
        IMAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether clip `name` currently has something behind it.
    pub fn connected(&self, name: &str) -> bool {
        match self.cfg.clip(name).map(|c| &c.role) {
            Some(ClipRole::Output) | Some(ClipRole::Input) => true,
            Some(ClipRole::Layer(_)) => self.layer_ids.get(name).copied().flatten().is_some(),
            None => false,
        }
    }
}

/// What `GetClipPreferences` decided.
#[derive(Clone, Debug)]
pub struct Prefs {
    pub depth: Depth,
    pub comps: HashMap<String, Comps>,
    /// The output is straight-alpha and must be premultiplied after the render.
    pub out_unpremult: bool,
}

/// A live plug-in instance.
pub struct Instance {
    pub obj: Box<EffectObj>,
    pub entry: Arc<PluginEntry>,
    pub cfg: Arc<EffectCfg>,
    pub prefs: Prefs,
    pub created: bool,
    pub rendered: bool,
}

unsafe impl Send for Instance {}

impl Instance {
    /// Build (but do not yet create) an instance from the described effect.
    pub fn build(entry: Arc<PluginEntry>, cfg: Arc<EffectCfg>, desc: &EffectObj) -> Instance {
        let obj = Box::new(EffectObj {
            magic: EFFECT_MAGIC,
            props: desc.props.clone(),
            params: ParamSet::default(),
            clips: Mutex::new(Vec::new()),
            is_instance: true,
            render: Mutex::new(None),
            message: Mutex::new(String::new()),
        });
        let owner: *const EffectObj = &*obj;
        desc.params.instantiate_into(&obj.params, owner);
        desc.each_clip(|c| obj.clips.lock().unwrap_or_else(PoisonError::into_inner).push(Box::new(c.instantiate(owner))));
        obj.props.put_str(P_TYPE, TYPE_IMAGE_EFFECT_INSTANCE);
        obj.props.put_str(IE_CONTEXT, &cfg.context);
        obj.props.put_int(P_IS_INTERACTIVE, 0);
        obj.props.put_ptr(P_INSTANCE_DATA, 0);
        obj.props.put_double(IE_FRAME_RATE, 30.0);
        obj.props.put_doubles(IE_PROJECT_SIZE, &[NOMINAL_LAYER, NOMINAL_LAYER]);
        obj.props.put_doubles(IE_PROJECT_EXTENT, &[NOMINAL_LAYER, NOMINAL_LAYER]);
        obj.props.put_doubles(IE_PROJECT_OFFSET, &[0.0, 0.0]);
        obj.props.put_double(IE_PROJECT_PAR, 1.0);
        obj.props.put_double(IE_EFFECT_DURATION, UNKNOWN_DURATION);
        obj.props.put_int(IE_SEQUENTIAL_RENDER, 0);
        obj.props.put_str(IE_OPENGL_ENABLED, "false");
        let comps: HashMap<String, Comps> = cfg.clips.iter().map(|c| (c.name.clone(), c.comps)).collect();
        obj.each_clip(|c| {
            let comp = comps.get(&c.name).copied().unwrap_or(Comps::Rgba);
            c.props.put_str(IE_PIXEL_DEPTH, cfg.depth.name());
            c.props.put_str(IE_COMPONENTS, comp.name());
            c.props.put_str(CLIP_UNMAPPED_DEPTH, cfg.depth.name());
            c.props.put_str(CLIP_UNMAPPED_COMPONENTS, comp.name());
            c.props.put_str(IE_PREMULTIPLICATION, IMAGE_PREMULTIPLIED);
            c.props.put_double(IMG_PIXEL_ASPECT_RATIO, 1.0);
            c.props.put_double(IE_FRAME_RATE, 30.0);
            c.props.put_double(IE_UNMAPPED_FRAME_RATE, 30.0);
            c.props.put_doubles(IE_FRAME_RANGE, &[0.0, UNKNOWN_LAST_FRAME]);
            c.props.put_doubles(IE_UNMAPPED_FRAME_RANGE, &[0.0, UNKNOWN_LAST_FRAME]);
            c.props.put_str(CLIP_FIELD_ORDER, FIELD_NONE);
            c.props.put_int(CLIP_CONTINUOUS_SAMPLES, 0);
            c.props.put_int(CLIP_CONNECTED, 1);
        });
        let prefs = Prefs { depth: cfg.depth, comps, out_unpremult: false };
        Instance { obj, entry, cfg, prefs, created: false, rendered: false }
    }

    pub fn handle(&self) -> *const c_void {
        self.obj.handle() as *const c_void
    }

    /// `kOfxActionCreateInstance`, then `GetClipPreferences`.
    pub fn create(&mut self) -> Result<(), String> {
        let st = self.entry.call(ACT_CREATE_INSTANCE, self.handle(), None, None);
        if st != K_STAT_OK && st != K_STAT_REPLY_DEFAULT {
            return Err(format!("create instance failed (status {st})"));
        }
        self.created = true;
        self.refresh_prefs();
        Ok(())
    }

    /// Ask the plug-in for its clip preferences and remember them.
    pub fn refresh_prefs(&mut self) {
        let out = PropSet::new();
        let st = self.entry.call(ACT_GET_CLIP_PREFS, self.handle(), None, Some(&out));
        if st != K_STAT_OK {
            return;
        }
        let mut prefs = self.prefs.clone();
        if let Some(d) = out.str(&format!("{CLIP_PREF_DEPTH_PREFIX}{CLIP_OUTPUT}")).and_then(|s| Depth::from_name(&s)) {
            prefs.depth = d;
        }
        for c in &self.cfg.clips {
            if let Some(comp) = out.str(&format!("{CLIP_PREF_COMPONENTS_PREFIX}{}", c.name)).and_then(|s| Comps::from_name(&s)) {
                prefs.comps.insert(c.name.clone(), comp);
            }
        }
        if let Some(p) = out.str(IE_PREMULTIPLICATION) {
            prefs.out_unpremult = p == IMAGE_UNPREMULTIPLIED;
        }
        // Keep the clip properties in step with the decision.
        self.obj.each_clip(|c| {
            c.props.put_str(IE_PIXEL_DEPTH, prefs.depth.name());
            if let Some(comp) = prefs.comps.get(&c.name) {
                c.props.put_str(IE_COMPONENTS, comp.name());
            }
        });
        self.prefs = prefs;
    }

    /// Install the per-render context.
    pub fn begin(&self, ctx: &Arc<RenderCtx>) {
        let g = &ctx.geom;
        self.obj.props.put_double(IE_FRAME_RATE, ctx.fps);
        self.obj.props.put_doubles(IE_PROJECT_SIZE, &g.layer);
        self.obj.props.put_doubles(IE_PROJECT_EXTENT, &g.layer);
        // The real source range when the host knows it (retimers clamp to it), else the long
        // fallback; pooled instances are reset on every render.
        let range = ctx.frame_range();
        let (first, last) = range.unwrap_or((0.0, UNKNOWN_LAST_FRAME));
        self.obj.props.put_double(IE_EFFECT_DURATION, if range.is_some() { last - first + 1.0 } else { UNKNOWN_DURATION });
        self.obj.each_clip(|c| {
            c.props.put_double(IE_FRAME_RATE, ctx.fps);
            c.props.put_double(IE_UNMAPPED_FRAME_RATE, ctx.fps);
            c.props.put_doubles(IE_FRAME_RANGE, &[first, last]);
            c.props.put_doubles(IE_UNMAPPED_FRAME_RANGE, &[first, last]);
            c.props.put_int(CLIP_CONNECTED, ctx.connected(&c.name) as i32);
        });
        self.obj.set_render(Some(ctx.clone()));
    }

    pub fn end(&self) {
        self.obj.set_render(None);
    }

    /// Apply new parameter values; returns the names whose value changed.
    pub fn apply_values(&self, values: &HashMap<String, Vec<PVal>>) -> Vec<String> {
        let mut changed = Vec::new();
        for (name, v) in values {
            let Some(p) = self.obj.param(name) else { continue };
            if !same_values(&p.values(), v) {
                p.set_values(v.clone());
                changed.push(name.clone());
            }
        }
        changed.sort();
        changed
    }

    /// `kOfxActionInstanceChanged` for one parameter.
    pub fn instance_changed(&self, name: &str, time: f64) {
        let inp = PropSet::new();
        inp.put_str(P_TYPE, TYPE_PARAMETER);
        inp.put_str(P_NAME, name);
        inp.put_str(P_CHANGE_REASON, CHANGE_USER_EDITED);
        inp.put_double(P_TIME, time);
        inp.put_doubles(IE_RENDER_SCALE, &[1.0, 1.0]);
        self.entry.call(ACT_BEGIN_INSTANCE_CHANGED, self.handle(), Some(&inp), None);
        self.entry.call(ACT_INSTANCE_CHANGED, self.handle(), Some(&inp), None);
        self.entry.call(ACT_END_INSTANCE_CHANGED, self.handle(), Some(&inp), None);
    }

    /// `GetRegionOfDefinition`: the canonical RoD, or `None` for "use the default".
    pub fn region_of_definition(&self, time: f64, scale: f64, default: OfxRectD) -> Option<OfxRectD> {
        let inp = PropSet::new();
        inp.put_double(P_TIME, time);
        inp.put_doubles(IE_RENDER_SCALE, &[scale, scale]);
        let out = PropSet::new();
        out.put_doubles(IE_ROD, &[default.x1, default.y1, default.x2, default.y2]);
        let st = self.entry.call(ACT_GET_ROD, self.handle(), Some(&inp), Some(&out));
        if st != K_STAT_OK {
            return None;
        }
        let v = out.doubles(IE_ROD);
        if v.len() == 4 && v.iter().all(|x| !x.is_nan()) { Some(OfxRectD { x1: v[0], y1: v[1], x2: v[2], y2: v[3] }) } else { None }
    }

    /// `IsIdentity`: the clip name and time to pass through, if the plug-in says so.
    pub fn is_identity(&self, time: f64, window: OfxRectI, scale: f64) -> Option<(String, f64)> {
        let inp = PropSet::new();
        inp.put_double(P_TIME, time);
        inp.put_str(IE_FIELD_TO_RENDER, FIELD_NONE);
        inp.put_ints(IE_RENDER_WINDOW, &[window.x1, window.y1, window.x2, window.y2]);
        inp.put_doubles(IE_RENDER_SCALE, &[scale, scale]);
        let out = PropSet::new();
        out.put_str(P_NAME, "");
        out.put_double(P_TIME, time);
        let st = self.entry.call(ACT_IS_IDENTITY, self.handle(), Some(&inp), Some(&out));
        if st != K_STAT_OK {
            return None;
        }
        let name = out.str(P_NAME)?;
        if name.is_empty() {
            return None;
        }
        Some((name, out.double(P_TIME).unwrap_or(time)))
    }

    /// `GetRegionsOfInterest` (informational: every region is served from the full buffer).
    pub fn regions_of_interest(&self, time: f64, scale: f64, roi: OfxRectD) -> OfxStatus {
        let inp = PropSet::new();
        inp.put_double(P_TIME, time);
        inp.put_doubles(IE_RENDER_SCALE, &[scale, scale]);
        inp.put_doubles(IE_ROI, &[roi.x1, roi.y1, roi.x2, roi.y2]);
        let out = PropSet::new();
        for c in &self.cfg.clips {
            if c.role != ClipRole::Output {
                out.put_doubles(&format!("{CLIP_ROI_PREFIX}{}", c.name), &[roi.x1, roi.y1, roi.x2, roi.y2]);
            }
        }
        self.entry.call(ACT_GET_ROI, self.handle(), Some(&inp), Some(&out))
    }

    /// `GetFramesNeeded`: per clip, the OFX frame ranges the plug-in will read.
    pub fn frames_needed(&self, time: f64) -> HashMap<String, Vec<(f64, f64)>> {
        let inp = PropSet::new();
        inp.put_double(P_TIME, time);
        let out = PropSet::new();
        let mut map = HashMap::new();
        if self.entry.call(ACT_GET_FRAMES_NEEDED, self.handle(), Some(&inp), Some(&out)) != K_STAT_OK {
            return map;
        }
        for c in &self.cfg.clips {
            let v = out.doubles(&format!("{CLIP_FRAMES_NEEDED_PREFIX}{}", c.name));
            if !v.is_empty() {
                map.insert(c.name.clone(), v.as_chunks::<2>().0.iter().map(|&[a, b]| (a, b)).collect());
            }
        }
        map
    }

    fn sequence_args(&self, time: f64, scale: f64) -> PropSet {
        let inp = PropSet::new();
        inp.put_doubles(IE_FRAME_RANGE, &[time, time]);
        inp.put_double(IE_FRAME_STEP, 1.0);
        inp.put_int(P_IS_INTERACTIVE, 0);
        inp.put_doubles(IE_RENDER_SCALE, &[scale, scale]);
        inp.put_int(IE_SEQUENTIAL_RENDER_STATUS, 0);
        inp.put_int(IE_INTERACTIVE_RENDER_STATUS, 0);
        inp
    }

    pub fn begin_sequence(&self, time: f64, scale: f64) -> OfxStatus {
        self.entry.call(ACT_BEGIN_SEQ, self.handle(), Some(&self.sequence_args(time, scale)), None)
    }

    pub fn end_sequence(&self, time: f64, scale: f64) -> OfxStatus {
        self.entry.call(ACT_END_SEQ, self.handle(), Some(&self.sequence_args(time, scale)), None)
    }

    /// `kOfxImageEffectActionRender`.
    pub fn render(&self, time: f64, window: OfxRectI, scale: f64) -> OfxStatus {
        let inp = PropSet::new();
        inp.put_double(P_TIME, time);
        inp.put_str(IE_FIELD_TO_RENDER, FIELD_NONE);
        inp.put_ints(IE_RENDER_WINDOW, &[window.x1, window.y1, window.x2, window.y2]);
        inp.put_doubles(IE_RENDER_SCALE, &[scale, scale]);
        inp.put_int(IE_SEQUENTIAL_RENDER_STATUS, 0);
        inp.put_int(IE_INTERACTIVE_RENDER_STATUS, 0);
        inp.put_int(IE_RENDER_QUALITY_DRAFT, 0);
        inp.put_int(IE_THUMBNAIL_RENDER, 0);
        inp.put_int(IE_NO_SPATIAL_AWARENESS, 0);
        self.entry.call(ACT_RENDER, self.handle(), Some(&inp), None)
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if self.created {
            self.entry.call(ACT_DESTROY_INSTANCE, self.handle(), None, None);
        }
    }
}

fn same_values(a: &[PVal], b: &[PVal]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (PVal::Int(p), PVal::Int(q)) => p == q,
            (PVal::Double(p), PVal::Double(q)) => p.to_bits() == q.to_bits() || (p - q).abs() < 1e-12,
            (PVal::Str(p), PVal::Str(q)) => p == q,
            (PVal::Ptr(p), PVal::Ptr(q)) => p == q,
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_become_frame_ranges() {
        // A 10 s clip at 30 fps has frames 0..=299.
        assert_eq!(span_frames(0.0, 10.0, 30.0), Some((0.0, 299.0)));
        // 24 fps, offset start.
        assert_eq!(span_frames(2.0, 3.0, 24.0), Some((48.0, 71.0)));
        // Less than a frame still has one frame; empty, reversed and bad spans have none.
        assert_eq!(span_frames(0.0, 0.01, 30.0), Some((0.0, 0.0)));
        assert_eq!(span_frames(1.0, 1.0, 30.0), None);
        assert_eq!(span_frames(2.0, 1.0, 30.0), None);
        assert_eq!(span_frames(0.0, f64::INFINITY, 30.0), None);
        assert_eq!(span_frames(0.0, 10.0, 0.0), None);
        assert_eq!(span_frames(0.0, 1.0e12, 30.0), None);
    }
}
