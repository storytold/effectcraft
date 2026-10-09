//! The effect plug-in API, version [`PLUGIN_API_VERSION`].
//!
//! A plug-in is anything implementing [`EffectPlugin`]: a [`PluginManifest`] (stable id,
//! display name, Effects & Presets category, typed parameters) and a `render` that processes a
//! [`PluginFrame`] in place. [`register_plugin`] turns it into an ordinary [`EffectSpec`]:
//! instances are property groups like every built-in effect, so plug-in parameters animate,
//! take expressions, show in Effect Controls and are addressable by agents, and the effect is
//! applied by id (`effect.apply {"effect": "org.example.posterize"}`) like a built-in.
//!
//! Plug-ins come from Rust code (implement the trait and register it at start-up) or from
//! sandboxed WebAssembly modules (`effectcraft-plugin`, see `docs/plugins.md`), which implement
//! the same contract through a small C-like ABI.
//!
//! The contract (stable within an API version):
//! * pixels are premultiplied RGBA `f32` (0–1, may exceed 1 in 32 bpc projects), row-major,
//!   row 0 on top, in the layer's buffer, which may be scaled (preview resolution) and padded;
//! * parameters are flattened to `f64`s in declaration order: slider / angle / checkbox (0 or 1)
//!   / popup (0-based index) take one value, point two (**frame pixels**, already mapped to the
//!   buffer), colour four (RGBA 0–1); API 2 adds point3 (x, y mapped like a point, z as is),
//!   layer (one value: the layer id, -1 for none), and text / hidden strings, which take no
//!   slot and are read with [`PluginParams::text`];
//! * `time` is layer time in seconds; `scale` is the buffer's pixels per layer pixel (scale
//!   pixel distances like blur radii by it);
//! * rendering must be deterministic (same inputs, same output) and may run on several threads
//!   at once.
//!
//! API 2 (native plug-ins; the WebAssembly ABI stays at 1) adds [`EffectPlugin::padding`] and
//! [`EffectPlugin::render_v2`], which gets a [`PluginHost`] to read the effect's input or other
//! layers at other times, and the extra parameter kinds. The OpenFX host (`effectcraft-ofx`) is
//! built on it.

use std::sync::{Arc, OnceLock, RwLock};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use serde::{Deserialize, Serialize};

use crate::{Buf, EffectCtx, EffectSpec, ParamSpec, Params, RenderFn};

/// The plug-in API version this build implements (manifests with API 1 or 2 are accepted).
pub const PLUGIN_API_VERSION: u32 = 2;

/// The most plug-in effects one process can register (Sapphire alone registers ~270).
pub const MAX_PLUGINS: usize = 1024;

/// The most transparent pixels a plug-in may ask [`EffectPlugin::padding`] to add per side.
const MAX_PADDING: u32 = 4096;

/// A plug-in parameter's kind, default and range.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PluginParamKind {
    Slider {
        default: f64,
        #[serde(default)]
        min: f64,
        #[serde(default = "hundred")]
        max: f64,
        /// Slider range (defaults to min..max).
        #[serde(default, rename = "sliderMin")]
        slider_min: Option<f64>,
        #[serde(default, rename = "sliderMax")]
        slider_max: Option<f64>,
        #[serde(default = "two")]
        decimals: u8,
        /// Whole numbers (API 2; OpenFX int parameters): shown with 0 decimals.
        #[serde(default)]
        integer: bool,
    },
    Angle {
        #[serde(default)]
        default: f64,
    },
    Checkbox {
        #[serde(default)]
        default: bool,
    },
    Popup {
        options: Vec<String>,
        /// 0-based index.
        #[serde(default)]
        default: u32,
    },
    /// Default as fractions of the layer size (`[0.5, 0.5]` = centre).
    Point {
        #[serde(default = "centre")]
        default: [f64; 2],
    },
    /// RGBA, 0–1.
    Color {
        #[serde(default = "white")]
        default: [f64; 4],
    },
    /// API 2: a 3D point (x, y in frame pixels like [`PluginParamKind::Point`], z as is). The
    /// default is in layer pixels (not fractions).
    Point3 {
        #[serde(default)]
        default: [f64; 3],
    },
    /// API 2: another layer of the composition (flattens to its id, -1 for none).
    Layer,
    /// API 2: a string, shown as a text field; read with [`PluginParams::text`].
    Text {
        #[serde(default)]
        default: String,
    },
    /// API 2: a string that is not shown (opaque plug-in data); read with [`PluginParams::text`].
    Hidden {
        #[serde(default)]
        default: String,
    },
}

fn hundred() -> f64 {
    100.0
}
fn two() -> u8 {
    2
}
fn centre() -> [f64; 2] {
    [0.5, 0.5]
}
fn white() -> [f64; 4] {
    [1.0; 4]
}

impl PluginParamKind {
    /// How many `f64`s the parameter flattens to.
    pub fn width(&self) -> usize {
        match self {
            PluginParamKind::Point { .. } => 2,
            PluginParamKind::Point3 { .. } => 3,
            PluginParamKind::Color { .. } => 4,
            PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. } => 0,
            _ => 1,
        }
    }
    /// Whether the parameter holds a string (it flattens to no `f64`s but one entry of
    /// [`PluginParams::texts`]).
    pub fn is_text(&self) -> bool {
        matches!(self, PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. })
    }
    /// Whether the kind needs plug-in API 2 (WebAssembly plug-ins can't carry it).
    pub fn needs_v2(&self) -> bool {
        matches!(self, PluginParamKind::Point3 { .. } | PluginParamKind::Layer | PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. })
    }
}

/// One parameter of a plug-in effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginParam {
    /// Stable id (the property's match name; letters, digits, `_`).
    pub id: String,
    pub name: String,
    /// API 2: the group / page path the parameter belongs to (`Advanced/Edges`). Hosts may
    /// ignore it.
    #[serde(default)]
    pub group: String,
    #[serde(flatten)]
    pub kind: PluginParamKind,
}

/// What a plug-in declares about itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// The plug-in API version it was written for (1, or 2 up to [`PLUGIN_API_VERSION`]).
    pub api: u32,
    /// Stable, reverse-domain effect id (`org.example.posterize`); `ec.` is reserved.
    pub id: String,
    pub name: String,
    /// Effects & Presets category: one of [`crate::CATEGORIES`] or a new one.
    pub category: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: Vec<PluginParam>,
}

impl PluginManifest {
    /// Check the manifest against this API version.
    pub fn validate(&self) -> Result<(), String> {
        // API 2 (OpenFX) effects such as Sapphire's can declare far more parameters.
        let max_params: usize = if self.api >= 2 { 1024 } else { 256 };
        // Opaque plug-in data can be large (Sapphire's S_ColorFuse ships a LUT of a few MB).
        const MAX_TEXT: usize = 64 << 20;
        if !(1..=PLUGIN_API_VERSION).contains(&self.api) {
            return Err(format!("plug-in `{}` targets plug-in API {} but this build implements API 1 to {PLUGIN_API_VERSION}", self.id, self.api));
        }
        let ok_id = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !ok_id(&self.id) || self.id.starts_with("ec.") {
            return Err(format!("bad plug-in id `{}` (reverse-domain, letters, digits, `.`, `_`, `-`; `ec.` is reserved)", self.id));
        }
        if self.name.trim().is_empty() || self.category.trim().is_empty() {
            return Err(format!("plug-in `{}` needs a name and a category", self.id));
        }
        if self.params.len() > max_params {
            return Err(format!("plug-in `{}` has {} parameters (at most {max_params})", self.id, self.params.len()));
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.params {
            if p.id.is_empty() || !p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || !seen.insert(p.id.as_str()) {
                return Err(format!("plug-in `{}`: bad or duplicate parameter id `{}`", self.id, p.id));
            }
            if let PluginParamKind::Popup { options, default } = &p.kind
                && (options.is_empty() || *default as usize >= options.len())
            {
                return Err(format!("plug-in `{}`: popup `{}` needs options and a default index in range", self.id, p.id));
            }
            if let PluginParamKind::Point3 { default } = &p.kind
                && default.iter().any(|v| !v.is_finite())
            {
                return Err(format!("plug-in `{}`: point3 `{}` needs a finite default", self.id, p.id));
            }
            if let PluginParamKind::Text { default } | PluginParamKind::Hidden { default } = &p.kind
                && default.len() > MAX_TEXT
            {
                return Err(format!("plug-in `{}`: the default of `{}` is longer than {MAX_TEXT} bytes", self.id, p.id));
            }
            if let PluginParamKind::Slider { default, min, max, slider_min, slider_max, .. } = &p.kind {
                let (lo, hi) = (slider_min.unwrap_or(*min), slider_max.unwrap_or(*max));
                if [*default, *min, *max, lo, hi].iter().any(|v| !v.is_finite()) || min > max || lo > hi || default < min || default > max {
                    return Err(format!("plug-in `{}`: slider `{}` needs min ≤ default ≤ max (and sliderMin ≤ sliderMax)", self.id, p.id));
                }
            }
        }
        Ok(())
    }

    /// Total flattened parameter count.
    pub fn flat_len(&self) -> usize {
        self.params.iter().map(|p| p.kind.width()).sum()
    }
}

/// The pixels a plug-in renders, in place.
pub struct PluginFrame<'a> {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, row-major (row 0 = top), `width * height` pixels.
    pub pixels: &'a mut [[f32; 4]],
    /// Buffer pixels per layer pixel (preview resolution).
    pub scale: f64,
    /// API 2: buffer-pixel position of layer pixel (0, 0) ([`Buf::offset`]; padding shifts it).
    pub origin: [f64; 2],
    /// API 2: the layer's bounds in layer pixels.
    pub layer_size: [f64; 2],
}

/// An image the host hands an API 2 plug-in (the input at another time, or another layer).
/// Same layout as [`PluginFrame`]: premultiplied RGBA, row 0 = top, `origin` = buffer pixels of
/// layer pixel (0, 0) in this image.
#[derive(Clone, Debug, Default)]
pub struct PluginImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
    pub origin: [f64; 2],
    pub scale: f64,
}

impl PluginImage {
    fn from_buf(b: Buf) -> PluginImage {
        PluginImage { width: b.img.width, height: b.img.height, pixels: b.img.data, origin: b.offset, scale: b.scale }
    }
}

/// Services available during an API 2 render ([`EffectPlugin::render_v2`]).
pub trait PluginHost {
    /// This effect's input (its layer with masks and the effects above this one) at another
    /// **layer time** in seconds, in its own extent (see [`PluginImage::origin`]; it is not
    /// padded). `None` when unavailable (adjustment layers, no host).
    fn input_at(&self, layer_time: f64) -> Option<PluginImage>;
    /// The layer chosen in Layer parameter `param_id` at **layer time** `layer_time` (converted
    /// to composition time), effects and masks applied, resampled into this layer's buffer
    /// (same size, origin and scale as the frame, centred when the layers differ in size).
    /// `None` when unset or unavailable.
    fn layer_input(&self, param_id: &str, layer_time: f64) -> Option<PluginImage>;
    /// All flattened parameter values (and texts) at another layer time.
    fn params_at(&self, layer_time: f64) -> Option<(Vec<f64>, Vec<String>)>;
    /// Composition frame rate (30 when unknown).
    fn frame_rate(&self) -> f64;
    /// Layer time of the current render, seconds.
    fn time(&self) -> f64;
    /// A stable id of the applied effect instance: the same on every frame of one effect on one
    /// layer, different for every layer and every application of the effect. Plug-ins that keep
    /// state between frames pool their native instances by it. 0 when unknown.
    fn instance_key(&self) -> u64 {
        0
    }
    /// The layer's source time range in **layer time** seconds as (first, end): the first source
    /// frame starts at `first` and the source ends at `end` (see [`crate::EffectEnv::layer_span`]).
    /// `None` when the source has no duration or the host does not know it (retimers then
    /// assume an unbounded range).
    fn frame_range(&self) -> Option<(f64, f64)> {
        None
    }
}

/// The [`PluginHost::instance_key`] of the effect `ctx` renders: its instance seed (stable per
/// effect instance), offset so it is never 0.
pub fn ctx_instance_key(ctx: &EffectCtx) -> u64 {
    (1u64 << 32) | u64::from(ctx.seed)
}

/// Parameter values for one render, flattened (see the module docs) with typed accessors.
pub struct PluginParams<'a> {
    pub manifest: &'a PluginManifest,
    pub flat: &'a [f64],
    /// One string per text / hidden parameter, in declaration order.
    pub texts: &'a [String],
}

impl PluginParams<'_> {
    fn slot(&self, id: &str) -> Option<&[f64]> {
        let mut at = 0;
        for p in &self.manifest.params {
            let w = p.kind.width();
            if p.id == id {
                return self.flat.get(at..at + w);
            }
            at += w;
        }
        None
    }
    /// Slider / angle / popup index / checkbox (0 or 1).
    pub fn f(&self, id: &str) -> f64 {
        self.slot(id).and_then(|s| s.first().copied()).unwrap_or(0.0)
    }
    pub fn b(&self, id: &str) -> bool {
        self.f(id) != 0.0
    }
    /// Point in frame pixels.
    pub fn point(&self, id: &str) -> [f64; 2] {
        self.slot(id).and_then(|s| Some([*s.first()?, *s.get(1)?])).unwrap_or([0.0; 2])
    }
    /// 3D point: x, y in frame pixels, z as is.
    pub fn point3(&self, id: &str) -> [f64; 3] {
        self.slot(id).and_then(|s| Some([*s.first()?, *s.get(1)?, *s.get(2)?])).unwrap_or([0.0; 3])
    }
    pub fn color(&self, id: &str) -> [f64; 4] {
        self.slot(id).and_then(|s| Some([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?])).unwrap_or([1.0; 4])
    }
    /// The layer chosen in a layer parameter.
    pub fn layer(&self, id: &str) -> Option<u64> {
        self.slot(id).and_then(|s| s.first().copied()).filter(|v| *v >= 0.0).map(|v| v as u64)
    }
    /// A text / hidden parameter's string (empty for other ids).
    pub fn text(&self, id: &str) -> &str {
        let mut at = 0;
        for p in &self.manifest.params {
            if p.kind.is_text() {
                if p.id == id {
                    return self.texts.get(at).map(String::as_str).unwrap_or("");
                }
                at += 1;
            }
        }
        ""
    }
}

/// An effect plug-in.
pub trait EffectPlugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    /// Process `frame` in place. `time` is layer time in seconds. An error leaves the frame as
    /// it was (the error is logged once).
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String>;
    /// Where the plug-in came from (a file path, `built-in`…), for listings.
    fn source(&self) -> String {
        "built-in".into()
    }
    /// API 2: transparent pixels to add on every side of the buffer before rendering (glows,
    /// blurs, regions of definition larger than the input), in buffer pixels. `layer_size` is
    /// the layer's size in layer pixels (the frame's [`PluginFrame::layer_size`]; 0 when unknown)
    /// and `instance_key` the [`PluginHost::instance_key`] this render will have.
    fn padding(&self, _params: &PluginParams, _time: f64, _scale: f64, _layer_size: [f64; 2], _instance_key: u64) -> u32 {
        0
    }
    /// API 2: [`EffectPlugin::render`] with host services. The default calls `render`. May be
    /// called from several threads at once.
    fn render_v2(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64, _host: &dyn PluginHost) -> Result<(), String> {
        self.render(frame, params, time)
    }
}

static SLOTS: [OnceLock<Arc<dyn EffectPlugin>>; MAX_PLUGINS] = [const { OnceLock::new() }; MAX_PLUGINS];

/// Registered plug-in effects, in registration order.
fn plugin_specs() -> &'static RwLock<Vec<&'static EffectSpec>> {
    static P: OnceLock<RwLock<Vec<&'static EffectSpec>>> = OnceLock::new();
    P.get_or_init(|| RwLock::new(vec![]))
}

/// The registered plug-in effects.
pub fn plugins() -> Vec<&'static EffectSpec> {
    plugin_specs().read().map(|v| v.clone()).unwrap_or_default()
}

/// The plug-in behind a registered effect id.
pub fn plugin(id: &str) -> Option<Arc<dyn EffectPlugin>> {
    SLOTS.iter().filter_map(|s| s.get()).find(|p| p.manifest().id == id).cloned()
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn param_spec(p: &PluginParam) -> ParamSpec {
    let (default, ui) = match &p.kind {
        PluginParamKind::Slider { default, min, max, slider_min, slider_max, decimals, integer } => (
            Value::Scalar(*default),
            ParamUi::Slider {
                min: *min,
                max: *max,
                slider_min: slider_min.unwrap_or(*min),
                slider_max: slider_max.unwrap_or(*max),
                decimals: if *integer { 0 } else { *decimals },
            },
        ),
        PluginParamKind::Angle { default } => (Value::Scalar(*default), ParamUi::Angle),
        PluginParamKind::Checkbox { default } => (Value::Bool(*default), ParamUi::Checkbox),
        PluginParamKind::Popup { options, default } => (Value::Enum(*default), ParamUi::Popup { options: options.clone() }),
        PluginParamKind::Point { default } => (Value::Vec2(*default), ParamUi::Point),
        PluginParamKind::Color { default } => (Value::Color(*default), ParamUi::Color),
        PluginParamKind::Point3 { default } => (Value::Vec3(*default), ParamUi::Point3),
        PluginParamKind::Layer => (Value::Layer(None), ParamUi::Layer),
        PluginParamKind::Text { default } => (Value::Str(default.clone()), ParamUi::Text),
        PluginParamKind::Hidden { default } => (Value::Str(default.clone()), ParamUi::Hidden),
    };
    ParamSpec { id: leak(&p.id), name: leak(&p.name), default, ui }
}

/// Register a plug-in effect: it appears in Effects & Presets under its category and is
/// applied by id like a built-in. Fails for a bad manifest, a taken id or when
/// [`MAX_PLUGINS`] are registered.
pub fn register_plugin(plugin: Arc<dyn EffectPlugin>) -> Result<&'static EffectSpec, String> {
    let m = plugin.manifest().clone();
    m.validate()?;
    let mut specs = plugin_specs().write().map_err(|_| "plug-in registry poisoned".to_string())?;
    if crate::registry().iter().any(|s| s.id == m.id) || specs.iter().any(|s| s.id == m.id) {
        return Err(format!("an effect with id `{}` is already registered", m.id));
    }
    let slot = SLOTS.iter().position(|s| s.get().is_none()).ok_or_else(|| format!("too many plug-ins (at most {MAX_PLUGINS})"))?;
    SLOTS[slot].set(plugin).map_err(|_| "plug-in slot taken".to_string())?;
    let spec: &'static EffectSpec = Box::leak(Box::new(EffectSpec {
        id: leak(&m.id),
        name: leak(&m.name),
        category: leak(&m.category),
        params: m.params.iter().map(param_spec).collect(),
        render: TRAMPOLINES[slot],
        gpu: false,
        float: true,
    }));
    specs.push(spec);
    Ok(spec)
}

/// Each slot needs its own `fn` (effect render functions are plain function pointers).
fn trampoline<const I: usize>(ctx: &EffectCtx, buf: Buf) -> Buf {
    run_slot(I, ctx, buf)
}

/// Concatenate `A` runs of `B` render functions (`N` = `A * B`), for the table below.
const fn concat<const A: usize, const B: usize, const N: usize>(parts: [[RenderFn; B]; A]) -> [RenderFn; N] {
    let mut out = [trampoline::<0> as RenderFn; N];
    let mut i = 0;
    while i < N {
        out[i] = parts[i / B][i % B];
        i += 1;
    }
    out
}

/// 16 trampolines `base ..= base + 15`.
macro_rules! t16 {
    ($b:expr) => {
        [
            trampoline::<{ $b }> as RenderFn,
            trampoline::<{ $b + 1 }> as RenderFn,
            trampoline::<{ $b + 2 }> as RenderFn,
            trampoline::<{ $b + 3 }> as RenderFn,
            trampoline::<{ $b + 4 }> as RenderFn,
            trampoline::<{ $b + 5 }> as RenderFn,
            trampoline::<{ $b + 6 }> as RenderFn,
            trampoline::<{ $b + 7 }> as RenderFn,
            trampoline::<{ $b + 8 }> as RenderFn,
            trampoline::<{ $b + 9 }> as RenderFn,
            trampoline::<{ $b + 10 }> as RenderFn,
            trampoline::<{ $b + 11 }> as RenderFn,
            trampoline::<{ $b + 12 }> as RenderFn,
            trampoline::<{ $b + 13 }> as RenderFn,
            trampoline::<{ $b + 14 }> as RenderFn,
            trampoline::<{ $b + 15 }> as RenderFn,
        ]
    };
}

/// Four runs of `$inner` (each `$n` long, starting `$n` apart) from `$b`: `$total` trampolines.
macro_rules! x4 {
    ($inner:ident, $b:expr, $n:literal, $total:literal) => {
        concat::<4, $n, $total>([$inner!($b), $inner!($b + $n), $inner!($b + 2 * $n), $inner!($b + 3 * $n)])
    };
}
macro_rules! t64 {
    ($b:expr) => {
        x4!(t16, $b, 16, 64)
    };
}
macro_rules! t256 {
    ($b:expr) => {
        x4!(t64, $b, 64, 256)
    };
}
macro_rules! t1024 {
    ($b:expr) => {
        x4!(t256, $b, 256, 1024)
    };
}

const TRAMPOLINES: [RenderFn; MAX_PLUGINS] = t1024!(0);

/// Flatten an instance's evaluated parameters for the plug-in (points mapped to frame pixels).
pub fn flatten(m: &PluginManifest, ctx: &EffectCtx, buf: &Buf) -> Vec<f64> {
    flatten_params(m, ctx.params, buf.offset, buf.scale)
}

/// [`flatten`] for any evaluated parameters, with points mapped by buffer `offset` and `scale`.
pub fn flatten_params(m: &PluginManifest, params: &Params, offset: [f64; 2], scale: f64) -> Vec<f64> {
    let to_px = |q: [f64; 2]| (q[0] * scale + offset[0], q[1] * scale + offset[1]);
    let mut out = Vec::with_capacity(m.flat_len());
    for p in &m.params {
        let v = params.get(&p.id);
        match &p.kind {
            PluginParamKind::Point { .. } => {
                let (x, y) = to_px(v.map(Value::as_vec2).unwrap_or([0.0; 2]));
                out.extend([x, y]);
            }
            PluginParamKind::Point3 { default } => {
                let q = v.map(Value::as_vec3).unwrap_or(*default);
                let (x, y) = to_px([q[0], q[1]]);
                out.extend([x, y, q[2]]);
            }
            PluginParamKind::Color { .. } => {
                let c = v.map(Value::as_color).unwrap_or([1.0; 4]);
                out.extend(c.iter().map(|x| *x as f64));
            }
            PluginParamKind::Checkbox { .. } => out.push(if v.map(Value::as_bool).unwrap_or(false) { 1.0 } else { 0.0 }),
            PluginParamKind::Popup { .. } => out.push(v.map(Value::as_enum).unwrap_or(0) as f64),
            PluginParamKind::Layer => out.push(v.and_then(Value::as_layer).map(|l| l as f64).unwrap_or(-1.0)),
            PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. } => {}
            _ => out.push(v.map(Value::as_f64).unwrap_or(0.0)),
        }
    }
    out
}

/// The strings of an instance's text / hidden parameters, in declaration order.
pub fn flatten_texts(m: &PluginManifest, params: &Params) -> Vec<String> {
    m.params
        .iter()
        .filter_map(|p| match &p.kind {
            PluginParamKind::Text { default } | PluginParamKind::Hidden { default } => Some(match params.get(&p.id) {
                Some(Value::Str(s)) => s.clone(),
                _ => default.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// The host services of one render, on top of the effect context and the (padded) buffer.
struct RenderHost<'a, 'b> {
    ctx: &'a EffectCtx<'b>,
    manifest: &'a PluginManifest,
    width: u32,
    height: u32,
    offset: [f64; 2],
    scale: f64,
}

impl RenderHost<'_, '_> {
    /// The frame's pixel grid without pixels (what [`crate::util::fit_layer`] measures).
    fn grid(&self) -> Buf {
        Buf { img: crate::Image { width: self.width, height: self.height, data: vec![] }, offset: self.offset, scale: self.scale }
    }
}

impl PluginHost for RenderHost<'_, '_> {
    fn input_at(&self, layer_time: f64) -> Option<PluginImage> {
        let host = self.ctx.env.host?;
        host.self_at(layer_time, self.ctx.env.effect_index).map(PluginImage::from_buf)
    }

    fn layer_input(&self, param_id: &str, layer_time: f64) -> Option<PluginImage> {
        let ctx = self.ctx;
        let lid = ctx.params.get(param_id)?.as_layer()?;
        let other = if (layer_time - ctx.time).abs() < 1e-9 {
            ctx.layer_param(param_id, true)?
        } else {
            let host = ctx.env.host?;
            // Source / Masks show no effects; the default (Effects & Masks) does.
            let with_effects = !matches!(ctx.params.get(&crate::layer_source_id(param_id)).map(Value::as_enum), Some(0 | 1));
            host.layer_at(lid, ctx.env.comp_time + (layer_time - ctx.time), with_effects)?
        };
        let img = crate::util::fit_layer(ctx, &self.grid(), &other, false);
        Some(PluginImage { width: img.width, height: img.height, pixels: img.data, origin: self.offset, scale: self.scale })
    }

    fn params_at(&self, layer_time: f64) -> Option<(Vec<f64>, Vec<String>)> {
        let p = self.ctx.env.host?.params_at(layer_time)?;
        Some((flatten_params(self.manifest, &p, self.offset, self.scale), flatten_texts(self.manifest, &p)))
    }

    fn frame_rate(&self) -> f64 {
        self.ctx.fps()
    }

    fn time(&self) -> f64 {
        self.ctx.time
    }

    fn instance_key(&self) -> u64 {
        ctx_instance_key(self.ctx)
    }

    fn frame_range(&self) -> Option<(f64, f64)> {
        self.ctx.env.layer_span
    }
}

fn run_slot(slot: usize, ctx: &EffectCtx, mut buf: Buf) -> Buf {
    let Some(plugin) = SLOTS[slot].get() else { return buf };
    let m = plugin.manifest();
    let texts = flatten_texts(m, ctx.params);
    let flat = flatten(m, ctx, &buf);
    let key = ctx_instance_key(ctx);
    let pad = plugin.padding(&PluginParams { manifest: m, flat: &flat, texts: &texts }, ctx.time, buf.scale, ctx.layer_size, key).min(MAX_PADDING);
    let before = buf.clone();
    // Padding moves the layer origin, which points are measured from.
    let flat = if pad > 0 {
        buf.pad(pad);
        flatten(m, ctx, &buf)
    } else {
        flat
    };
    let params = PluginParams { manifest: m, flat: &flat, texts: &texts };
    let (width, height, offset, scale) = (buf.img.width, buf.img.height, buf.offset, buf.scale);
    let host = RenderHost { ctx, manifest: m, width, height, offset, scale };
    let mut frame = PluginFrame { width, height, pixels: &mut buf.img.data, scale, origin: offset, layer_size: ctx.layer_size };
    if let Err(e) = plugin.render_v2(&mut frame, &params, ctx.time, &host) {
        log::warn!("plug-in `{}`: {e}", m.id);
        return before;
    }
    buf
}
