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
//!   in the layer's buffer, which may be scaled (preview resolution) and padded;
//! * parameters are flattened to `f64`s in declaration order: slider / angle / checkbox (0 or 1)
//!   / popup (0-based index) take one value, point two (**frame pixels**, already mapped to the
//!   buffer), colour four (RGBA 0–1);
//! * `time` is layer time in seconds; `scale` is the buffer's pixels per layer pixel (scale
//!   pixel distances like blur radii by it);
//! * rendering must be deterministic (same inputs, same output) and may run on several threads
//!   at once.

use std::sync::{Arc, OnceLock, RwLock};

use effectcraft_keyframe::Value;
use effectcraft_project::ParamUi;
use serde::{Deserialize, Serialize};

use crate::{Buf, EffectCtx, EffectSpec, ParamSpec, RenderFn};

/// The plug-in API version this build implements.
pub const PLUGIN_API_VERSION: u32 = 1;

/// The most plug-in effects one process can register.
pub const MAX_PLUGINS: usize = 64;

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
            PluginParamKind::Color { .. } => 4,
            _ => 1,
        }
    }
}

/// One parameter of a plug-in effect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginParam {
    /// Stable id (the property's match name; letters, digits, `_`).
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: PluginParamKind,
}

/// What a plug-in declares about itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    /// The plug-in API version it was written for (must be [`PLUGIN_API_VERSION`]).
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
        const MAX_PARAMS: usize = 256;
        if self.api != PLUGIN_API_VERSION {
            return Err(format!("plug-in `{}` targets plug-in API {} but this build implements API {PLUGIN_API_VERSION}", self.id, self.api));
        }
        let ok_id = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !ok_id(&self.id) || self.id.starts_with("ec.") {
            return Err(format!("bad plug-in id `{}` (reverse-domain, letters, digits, `.`, `_`, `-`; `ec.` is reserved)", self.id));
        }
        if self.name.trim().is_empty() || self.category.trim().is_empty() {
            return Err(format!("plug-in `{}` needs a name and a category", self.id));
        }
        if self.params.len() > MAX_PARAMS {
            return Err(format!("plug-in `{}` has {} parameters (at most {MAX_PARAMS})", self.id, self.params.len()));
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
    /// Premultiplied RGBA, row-major, `width * height` pixels.
    pub pixels: &'a mut [[f32; 4]],
    /// Buffer pixels per layer pixel (preview resolution).
    pub scale: f64,
}

/// Parameter values for one render, flattened (see the module docs) with typed accessors.
pub struct PluginParams<'a> {
    pub manifest: &'a PluginManifest,
    pub flat: &'a [f64],
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
        self.slot(id).map(|s| [s[0], s[1]]).unwrap_or([0.0; 2])
    }
    pub fn color(&self, id: &str) -> [f64; 4] {
        self.slot(id).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([1.0; 4])
    }
}

/// An effect plug-in.
pub trait EffectPlugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;
    /// Process `frame` in place. `time` is layer time in seconds. An error leaves the frame as
    /// it was (the error is logged once).
    fn render(&self, frame: &mut PluginFrame, params: &PluginParams, time: f64) -> Result<(), String>;
    /// Rust host extension: return a new layer-space buffer, including expanded pixels.
    /// Preserve `scale` and update `offset` when adding pixels on the left/top.
    /// The default adapter preserves the v1 in-place contract (including WebAssembly).
    /// An error leaves the original buffer unchanged.
    fn render_buffer(&self, input: &Buf, params: &PluginParams, time: f64) -> Result<Buf, String> {
        let mut output = input.clone();
        let mut frame = PluginFrame { width: output.img.width, height: output.img.height, pixels: &mut output.img.data, scale: output.scale };
        self.render(&mut frame, params, time)?;
        Ok(output)
    }
    /// Where the plug-in came from (a file path, `built-in`…), for listings.
    fn source(&self) -> String {
        "built-in".into()
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
        PluginParamKind::Slider { default, min, max, slider_min, slider_max, decimals } => (
            Value::Scalar(*default),
            ParamUi::Slider { min: *min, max: *max, slider_min: slider_min.unwrap_or(*min), slider_max: slider_max.unwrap_or(*max), decimals: *decimals },
        ),
        PluginParamKind::Angle { default } => (Value::Scalar(*default), ParamUi::Angle),
        PluginParamKind::Checkbox { default } => (Value::Bool(*default), ParamUi::Checkbox),
        PluginParamKind::Popup { options, default } => (Value::Enum(*default), ParamUi::Popup { options: options.clone() }),
        PluginParamKind::Point { default } => (Value::Vec2(*default), ParamUi::Point),
        PluginParamKind::Color { default } => (Value::Color(*default), ParamUi::Color),
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

macro_rules! trampolines {
    ($($i:literal)*) => { [$(trampoline::<$i> as RenderFn),*] };
}

const TRAMPOLINES: [RenderFn; MAX_PLUGINS] = trampolines!(
    0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
    32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63
);

/// Flatten an instance's evaluated parameters for the plug-in (points mapped to frame pixels).
pub fn flatten(m: &PluginManifest, ctx: &EffectCtx, buf: &Buf) -> Vec<f64> {
    let mut out = Vec::with_capacity(m.flat_len());
    for p in &m.params {
        let v = ctx.params.get(&p.id);
        match &p.kind {
            PluginParamKind::Point { .. } => {
                let q = v.map(Value::as_vec2).unwrap_or([0.0; 2]);
                let (x, y) = buf.to_px(q);
                out.extend([x, y]);
            }
            PluginParamKind::Color { .. } => {
                let c = v.map(Value::as_color).unwrap_or([1.0; 4]);
                out.extend(c.iter().map(|x| *x as f64));
            }
            PluginParamKind::Checkbox { .. } => out.push(if v.map(Value::as_bool).unwrap_or(false) { 1.0 } else { 0.0 }),
            PluginParamKind::Popup { .. } => out.push(v.map(Value::as_enum).unwrap_or(0) as f64),
            _ => out.push(v.map(Value::as_f64).unwrap_or(0.0)),
        }
    }
    out
}

fn run_slot(slot: usize, ctx: &EffectCtx, mut buf: Buf) -> Buf {
    let Some(plugin) = SLOTS[slot].get() else { return buf };
    let m = plugin.manifest();
    let flat = flatten(m, ctx, &buf);
    let params = PluginParams { manifest: m, flat: &flat };
    match plugin.render_buffer(&buf, &params, ctx.time) {
        Ok(output) => {
            let count = (output.img.width as usize).checked_mul(output.img.height as usize);
            if count != Some(output.img.data.len())
                || !output.scale.is_finite()
                || output.scale <= 0.0
                || output.scale != buf.scale
                || output.offset.iter().any(|v| !v.is_finite())
            {
                log::warn!("plug-in `{}` returned an invalid buffer", m.id);
            } else {
                buf = output;
            }
        }
        Err(e) => log::warn!("plug-in `{}`: {e}", m.id),
    }
    buf
}
