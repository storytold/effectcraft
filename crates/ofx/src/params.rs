//! OFX parameters: descriptor / instance storage, and the mapping to EffectCraft parameters.
//!
//! Two jobs live here:
//!
//! 1. [`ParamSet`] / [`ParamObj`] are what the plug-in sees through the parameter suite: typed
//!    values with property sets. A descriptor set is built by the plug-in during
//!    `DescribeInContext`; every instance gets a copy initialised from the defaults.
//! 2. [`map_params`] turns the descriptor set into [`PluginParam`]s (the manifest EffectCraft
//!    shows and animates) plus a [`Bindings`] table that converts the flattened values EffectCraft
//!    hands the effect back into OFX values at render time (including `paramGetValueAtTime`).
//!
//! Mapping (see `docs/ofx.md`): double / int / bool / choice / RGB / RGBA / string /
//! custom map to Slider (integer for int) / Checkbox / Popup / Color / Text / Hidden. Angles map
//! to Angle. Position values (double type XY or XY absolute) map to Point (2D) or Point3 (3D),
//! converted between buffer pixels and OFX canonical y-up coordinates for absolute ones. Every
//! other 2D/3D value (Plain included) maps to one slider per component named `<id>_x`, `_y`,
//! `_z`. Groups and pages become the `group` path; push buttons, bytes and secret numeric
//! parameters are not exposed (the plug-in keeps their default).

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Mutex, PoisonError};

use effectcraft_effects::plugin::{PluginParam, PluginParamKind};

use crate::ffi::*;
use crate::instance::EffectObj;
use crate::props::{PVal, PropSet, as_double, as_int, as_string};

/// The marker at the start of every [`ParamObj`].
pub const PARAM_MAGIC: u32 = 0x4F46_5841; // "OFXA"

/// Nominal project size used to turn canonical-coordinate defaults into fractions of the layer
/// (the real size is unknown while the effect is being described).
const NOMINAL_PROJECT: [f64; 2] = [1920.0, 1080.0];

/// An OFX parameter type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PKind {
    Int,
    Double,
    Bool,
    Choice,
    StrChoice,
    Rgb,
    Rgba,
    Double2D,
    Int2D,
    Double3D,
    Int3D,
    Str,
    Custom,
    Bytes,
    Group,
    Page,
    Push,
}

impl PKind {
    pub fn from_type(s: &str) -> Option<PKind> {
        Some(match s {
            PT_INTEGER => PKind::Int,
            PT_DOUBLE => PKind::Double,
            PT_BOOLEAN => PKind::Bool,
            PT_CHOICE => PKind::Choice,
            PT_STR_CHOICE => PKind::StrChoice,
            PT_RGB => PKind::Rgb,
            PT_RGBA => PKind::Rgba,
            PT_DOUBLE_2D => PKind::Double2D,
            PT_INTEGER_2D => PKind::Int2D,
            PT_DOUBLE_3D => PKind::Double3D,
            PT_INTEGER_3D => PKind::Int3D,
            PT_STRING => PKind::Str,
            PT_CUSTOM => PKind::Custom,
            PT_BYTES => PKind::Bytes,
            PT_GROUP => PKind::Group,
            PT_PAGE => PKind::Page,
            PT_PUSH_BUTTON => PKind::Push,
            _ => return None,
        })
    }

    pub fn type_name(self) -> &'static str {
        match self {
            PKind::Int => PT_INTEGER,
            PKind::Double => PT_DOUBLE,
            PKind::Bool => PT_BOOLEAN,
            PKind::Choice => PT_CHOICE,
            PKind::StrChoice => PT_STR_CHOICE,
            PKind::Rgb => PT_RGB,
            PKind::Rgba => PT_RGBA,
            PKind::Double2D => PT_DOUBLE_2D,
            PKind::Int2D => PT_INTEGER_2D,
            PKind::Double3D => PT_DOUBLE_3D,
            PKind::Int3D => PT_INTEGER_3D,
            PKind::Str => PT_STRING,
            PKind::Custom => PT_CUSTOM,
            PKind::Bytes => PT_BYTES,
            PKind::Group => PT_GROUP,
            PKind::Page => PT_PAGE,
            PKind::Push => PT_PUSH_BUTTON,
        }
    }

    /// Whether the parameter carries a value.
    pub fn has_value(self) -> bool {
        !matches!(self, PKind::Group | PKind::Page | PKind::Push | PKind::Bytes)
    }
}

/// A parameter (descriptor or instance). `#[repr(C)]` with the magic first for handle checks.
#[repr(C)]
pub struct ParamObj {
    magic: u32,
    pub kind: PKind,
    pub name: String,
    pub props: PropSet,
    pub value: Mutex<Vec<PVal>>,
    /// The effect (descriptor or instance) this parameter belongs to.
    pub owner: *const EffectObj,
}

// The raw owner pointer is only dereferenced while the owning effect is alive (an instance owns
// its parameters), and everything mutable behind it is a Mutex.
unsafe impl Send for ParamObj {}
unsafe impl Sync for ParamObj {}

impl ParamObj {
    /// Validate a raw handle.
    ///
    /// # Safety
    /// `h` must be null or point to readable memory; the reference is only used during a callback.
    pub unsafe fn from_handle<'a>(h: *mut c_void) -> Option<&'a ParamObj> {
        if h.is_null() {
            return None;
        }
        if unsafe { std::ptr::read(h as *const u32) } != PARAM_MAGIC {
            return None;
        }
        Some(unsafe { &*(h as *const ParamObj) })
    }

    pub fn handle(&self) -> *mut c_void {
        self as *const ParamObj as *mut c_void
    }

    pub fn values(&self) -> Vec<PVal> {
        self.value.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn set_values(&self, v: Vec<PVal>) {
        *self.value.lock().unwrap_or_else(PoisonError::into_inner) = v;
    }
}

/// The ordered parameters of an effect descriptor or instance.
// Boxed so the pointers handed to plug-ins stay put while the list grows.
#[allow(clippy::vec_box)]
#[derive(Default)]
pub struct ParamSet {
    items: Mutex<Vec<Box<ParamObj>>>,
}

impl ParamSet {
    #[allow(clippy::vec_box)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Box<ParamObj>>> {
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Define a new parameter; its property set starts with the type's defaults.
    pub fn define(&self, owner: *const EffectObj, kind: PKind, name: &str, instance: bool) -> Result<*const ParamObj, OfxStatus> {
        let mut items = self.lock();
        if items.iter().any(|p| p.name == name) {
            return Err(K_STAT_ERR_EXISTS);
        }
        if items.len() >= 4096 {
            return Err(K_STAT_ERR_MEMORY);
        }
        let props = PropSet::new();
        init_props(kind, name, &props, instance);
        let value = Mutex::new(initial_value(kind, &props));
        let p = Box::new(ParamObj { magic: PARAM_MAGIC, kind, name: name.to_string(), props, value, owner });
        let ptr: *const ParamObj = &*p;
        items.push(p);
        Ok(ptr)
    }

    pub fn find(&self, name: &str) -> Option<*const ParamObj> {
        self.lock().iter().find(|p| p.name == name).map(|p| &**p as *const ParamObj)
    }

    /// Stable pointers to every parameter, in definition order (boxes are never removed).
    pub fn all(&self) -> Vec<*const ParamObj> {
        self.lock().iter().map(|p| &**p as *const ParamObj).collect()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copy this descriptor set into `dst` as instance parameters: same definitions, values
    /// reset to the defaults.
    pub fn instantiate_into(&self, dst: &ParamSet, owner: *const EffectObj) {
        let mut out = dst.lock();
        for p in self.lock().iter() {
            let props = p.props.clone();
            props.put_str(P_TYPE, TYPE_PARAMETER_INSTANCE);
            let value = Mutex::new(initial_value(p.kind, &props));
            out.push(Box::new(ParamObj { magic: PARAM_MAGIC, kind: p.kind, name: p.name.clone(), props, value, owner }));
        }
    }
}

/// Fill a new parameter's property set with the defaults the specification names.
fn init_props(kind: PKind, name: &str, props: &PropSet, instance: bool) {
    props.put_str(P_TYPE, if instance { TYPE_PARAMETER_INSTANCE } else { TYPE_PARAMETER });
    props.put_str(P_NAME, name);
    props.put_str(P_LABEL, name);
    props.put_str(PP_TYPE, kind.type_name());
    props.put_str(PP_SCRIPT_NAME, name);
    props.put_str(PP_HINT, "");
    props.put_str(PP_PARENT, "");
    props.put_int(PP_SECRET, 0);
    props.put_int(PP_ENABLED, 1);
    props.put_int(PP_ANIMATES, 1);
    props.put_int(PP_CAN_UNDO, 1);
    props.put_int(PP_PERSISTANT, 1);
    props.put_int(PP_EVALUATE_ON_CHANGE, 1);
    props.put_int(PP_PLUGIN_MAY_WRITE, 0);
    props.put_int(PP_IS_ANIMATING, 0);
    props.put_str(PP_CACHE_INVALIDATION, "OfxParamInvalidateValueChange");
    let unbounded = f64::MAX;
    match kind {
        PKind::Double | PKind::Double2D | PKind::Double3D => {
            let n = match kind {
                PKind::Double => 1,
                PKind::Double2D => 2,
                _ => 3,
            };
            let lo = vec![-unbounded; n];
            let hi = vec![unbounded; n];
            props.put_doubles(PP_MIN, &lo);
            props.put_doubles(PP_MAX, &hi);
            props.put_doubles(PP_DISPLAY_MIN, &lo);
            props.put_doubles(PP_DISPLAY_MAX, &hi);
            props.put_doubles(PP_DEFAULT, &vec![0.0; n]);
            props.put_double(PP_INCREMENT, 1.0);
            props.put_int(PP_DIGITS, 2);
            props.put_str(PP_DOUBLE_TYPE, DOUBLE_TYPE_PLAIN);
            props.put_str(PP_DEFAULT_COORDINATE_SYSTEM, COORDS_CANONICAL);
        }
        PKind::Int | PKind::Int2D | PKind::Int3D => {
            let n = match kind {
                PKind::Int => 1,
                PKind::Int2D => 2,
                _ => 3,
            };
            props.put_ints(PP_MIN, &vec![i32::MIN; n]);
            props.put_ints(PP_MAX, &vec![i32::MAX; n]);
            props.put_ints(PP_DISPLAY_MIN, &vec![i32::MIN; n]);
            props.put_ints(PP_DISPLAY_MAX, &vec![i32::MAX; n]);
            props.put_ints(PP_DEFAULT, &vec![0; n]);
        }
        PKind::Bool | PKind::Choice => {
            props.put_int(PP_DEFAULT, 0);
            if kind == PKind::Choice {
                props.put(PP_CHOICE_OPTION, vec![]);
            }
        }
        PKind::StrChoice => {
            props.put_str(PP_DEFAULT, "");
            props.put(PP_CHOICE_OPTION, vec![]);
            props.put(PP_CHOICE_ENUM, vec![]);
        }
        PKind::Rgb => {
            props.put_doubles(PP_DEFAULT, &[0.0; 3]);
            props.put_doubles(PP_MIN, &[0.0; 3]);
            props.put_doubles(PP_MAX, &[1.0; 3]);
        }
        PKind::Rgba => {
            props.put_doubles(PP_DEFAULT, &[0.0; 4]);
            props.put_doubles(PP_MIN, &[0.0; 4]);
            props.put_doubles(PP_MAX, &[1.0; 4]);
        }
        PKind::Str | PKind::Custom => {
            props.put_str(PP_DEFAULT, "");
            props.put_str(PP_STRING_MODE, STRING_SINGLE_LINE);
        }
        PKind::Group => props.put_int(PP_GROUP_OPEN, 1),
        PKind::Page => props.put(PP_PAGE_CHILD, vec![]),
        PKind::Bytes | PKind::Push => {}
    }
}

/// The value a fresh parameter takes from its `kOfxParamPropDefault`.
pub fn initial_value(kind: PKind, props: &PropSet) -> Vec<PVal> {
    let d = props.all(PP_DEFAULT);
    let nums = |n: usize| -> Vec<PVal> { (0..n).map(|i| PVal::Double(d.get(i).and_then(as_double).unwrap_or(0.0))).collect() };
    let ints = |n: usize| -> Vec<PVal> { (0..n).map(|i| PVal::Int(d.get(i).and_then(as_int).unwrap_or(0))).collect() };
    match kind {
        PKind::Double => nums(1),
        PKind::Double2D => nums(2),
        PKind::Double3D | PKind::Rgb => nums(3),
        PKind::Rgba => nums(4),
        PKind::Int | PKind::Bool | PKind::Choice => ints(1),
        PKind::Int2D => ints(2),
        PKind::Int3D => ints(3),
        PKind::Str | PKind::Custom | PKind::StrChoice => vec![PVal::text(&d.first().and_then(as_string).unwrap_or_default())],
        PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => vec![],
    }
}

// ---------------------------------------------------------------------------------------------
// Mapping to EffectCraft parameters
// ---------------------------------------------------------------------------------------------

/// How an exposed OFX parameter is fed from the flattened EffectCraft values.
#[derive(Clone, Debug)]
pub enum Src {
    /// One flattened value.
    Flat { at: usize, n: usize },
    /// A point: `x, y` in buffer pixels (+ `z` when `with_z`) at `at`. `flip` converts to OFX
    /// canonical y-up positions (absolute XY positions); otherwise the value is plain layer
    /// pixels (y down, unflipped).
    Pos { at: usize, with_z: bool, flip: bool },
    /// The `i`-th text.
    Text { at: usize },
}

/// One OFX parameter and where its value comes from.
#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub kind: PKind,
    pub src: Src,
    /// For string choices: the value of each option.
    pub choices: Vec<String>,
}

/// A clip exposed as a Layer parameter.
#[derive(Clone, Debug)]
pub struct LayerBinding {
    pub clip: String,
    pub param_id: String,
    pub at: usize,
}

/// Everything needed to turn flattened EffectCraft values into OFX values.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    pub items: Vec<Binding>,
    pub layers: Vec<LayerBinding>,
}

/// Coordinates the position conversion needs.
#[derive(Clone, Copy, Debug)]
pub struct CoordCtx {
    /// Buffer-pixel position of layer pixel (0, 0).
    pub origin: [f64; 2],
    /// Buffer pixels per layer pixel.
    pub scale: f64,
    /// Layer height in layer pixels.
    pub layer_h: f64,
}

impl Bindings {
    /// OFX values for every exposed parameter, by OFX name.
    pub fn decode(&self, flat: &[f64], texts: &[String], cc: &CoordCtx) -> HashMap<String, Vec<PVal>> {
        let mut out = HashMap::new();
        let f = |i: usize| flat.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
        let scale = if cc.scale > 0.0 { cc.scale } else { 1.0 };
        for b in &self.items {
            let v = match (&b.src, b.kind) {
                (Src::Text { at }, _) => vec![PVal::text(texts.get(*at).map(String::as_str).unwrap_or(""))],
                (Src::Pos { at, with_z, flip }, _) => {
                    let (px, py) = (f(*at), f(*at + 1));
                    let ly = (py - cc.origin[1]) / scale;
                    let mut v = vec![PVal::Double((px - cc.origin[0]) / scale), PVal::Double(if *flip { cc.layer_h - ly } else { ly })];
                    if *with_z {
                        v.push(PVal::Double(f(*at + 2)));
                    }
                    v
                }
                (Src::Flat { at, n }, kind) => match kind {
                    PKind::Double => vec![PVal::Double(f(*at))],
                    PKind::Int => vec![PVal::Int(f(*at).round() as i32)],
                    PKind::Bool => vec![PVal::Int((f(*at) != 0.0) as i32)],
                    PKind::Choice => vec![PVal::Int(f(*at).round().max(0.0) as i32)],
                    PKind::StrChoice => {
                        let i = f(*at).round().max(0.0) as usize;
                        vec![PVal::text(b.choices.get(i).map(String::as_str).unwrap_or(""))]
                    }
                    PKind::Int2D | PKind::Int3D => (0..*n).map(|k| PVal::Int(f(*at + k).round() as i32)).collect(),
                    _ => (0..*n).map(|k| PVal::Double(f(*at + k))).collect(),
                },
            };
            out.insert(b.name.clone(), v);
        }
        out
    }

    /// Layer ids per clip name (`None` when unset).
    pub fn layer_ids(&self, flat: &[f64]) -> HashMap<String, Option<u64>> {
        self.layers
            .iter()
            .map(|l| {
                let v = flat.get(l.at).copied().unwrap_or(-1.0);
                (l.clip.clone(), if v >= 0.0 && v.is_finite() { Some(v as u64) } else { None })
            })
            .collect()
    }
}

/// The result of [`map_params`].
pub struct Mapped {
    pub params: Vec<PluginParam>,
    pub bindings: Bindings,
}

fn sanitize_id(name: &str, used: &mut HashSet<String>) -> String {
    let mut id: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if id.is_empty() {
        id = "param".into();
    }
    let base = id.clone();
    let mut n = 2;
    while !used.insert(id.clone()) {
        id = format!("{base}_{n}");
        n += 1;
    }
    id
}

fn unbounded(v: f64) -> bool {
    !v.is_finite() || v.abs() > 1.0e9
}

fn label_of(p: &ParamObj) -> String {
    p.props.str(P_LABEL).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| p.name.clone())
}

fn dim_label(p: &ParamObj, i: usize, fallback: &str) -> String {
    let l = p.props.strs(PP_DIMENSION_LABEL);
    match l.get(i) {
        Some(s) if !s.trim().is_empty() => s.clone(),
        _ => fallback.to_string(),
    }
}

/// Build the slider kind for one numeric component.
fn slider(p: &ParamObj, i: usize, integer: bool, default: f64) -> PluginParamKind {
    let get = |name: &str, fallback: f64| p.props.all(name).get(i).and_then(as_double).unwrap_or(fallback);
    let (mut min, mut max) = (get(PP_MIN, f64::MIN), get(PP_MAX, f64::MAX));
    let (dmin, dmax) = (get(PP_DISPLAY_MIN, f64::MIN), get(PP_DISPLAY_MAX, f64::MAX));
    let hard_lo = !unbounded(min);
    let hard_hi = !unbounded(max);
    if !hard_lo {
        min = -1.0e9;
    }
    if !hard_hi {
        max = 1.0e9;
    }
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    let default = if default.is_finite() { default.clamp(min, max) } else { min.max(0.0).min(max) };
    let (mut lo, mut hi) = if !unbounded(dmin) && !unbounded(dmax) && dmin <= dmax {
        (dmin, dmax)
    } else if hard_lo && hard_hi {
        (min, max)
    } else {
        (if hard_lo { min } else { default.min(0.0) - 100.0 }, if hard_hi { max } else { default.max(0.0) + 100.0 })
    };
    lo = lo.clamp(min, max);
    hi = hi.clamp(min, max);
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    let digits = p.props.int(PP_DIGITS).unwrap_or(2).clamp(0, 9) as u8;
    PluginParamKind::Slider { default, min, max, slider_min: Some(lo), slider_max: Some(hi), decimals: if integer { 0 } else { digits }, integer }
}

/// The page / group path of a parameter, e.g. `Advanced/Edges`.
fn group_path(p: &ParamObj, by_name: &HashMap<String, &ParamObj>, page_of: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = p.props.str(PP_PARENT).unwrap_or_default();
    let mut guard = 0;
    while !cur.is_empty() && guard < 16 {
        guard += 1;
        let Some(g) = by_name.get(&cur) else { break };
        parts.push(label_of(g));
        cur = g.props.str(PP_PARENT).unwrap_or_default();
    }
    parts.reverse();
    if let Some(page) = page_of.get(&p.name) {
        parts.insert(0, page.clone());
    }
    parts.join("/")
}

/// Map the descriptor parameters (and the clips that become Layer parameters) to EffectCraft
/// parameters. `page_order` is `kOfxPluginPropParamPageOrder`.
pub fn map_params(set: &ParamSet, page_order: &[String], layer_clips: &[String]) -> Mapped {
    let ptrs = set.all();
    // The boxes live as long as the set, which outlives this function.
    let objs: Vec<&ParamObj> = ptrs.iter().filter_map(|p| unsafe { p.as_ref() }).collect();
    let by_name: HashMap<String, &ParamObj> = objs.iter().map(|p| (p.name.clone(), *p)).collect();

    // Pages: order of leaves, and which page a leaf is on.
    let mut order: Vec<&ParamObj> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut page_of: HashMap<String, String> = HashMap::new();
    let mut pages: Vec<&ParamObj> = page_order.iter().filter_map(|n| by_name.get(n).copied()).filter(|p| p.kind == PKind::Page).collect();
    for p in &objs {
        if p.kind == PKind::Page && !pages.iter().any(|q| q.name == p.name) {
            pages.push(p);
        }
    }
    for page in &pages {
        let label = label_of(page);
        for child in page.props.strs(PP_PAGE_CHILD) {
            if child == PAGE_SKIP_ROW || child == PAGE_SKIP_COLUMN {
                continue;
            }
            if let Some(c) = by_name.get(&child) {
                page_of.entry(child.clone()).or_insert_with(|| label.clone());
                if seen.insert(child) {
                    order.push(c);
                }
            }
        }
    }
    for p in &objs {
        if seen.insert(p.name.clone()) {
            order.push(p);
        }
    }

    let mut used: HashSet<String> = HashSet::new();
    let mut params: Vec<PluginParam> = Vec::new();
    let mut bindings = Bindings::default();
    let mut flat = 0usize;
    let mut texts = 0usize;

    // Clips first: they are the effect's inputs.
    for clip in layer_clips {
        let id = sanitize_id(clip, &mut used);
        params.push(PluginParam { id: id.clone(), name: clip.clone(), kind: PluginParamKind::Layer, group: String::new() });
        bindings.layers.push(LayerBinding { clip: clip.clone(), param_id: id, at: flat });
        flat += 1;
    }

    let push = |params: &mut Vec<PluginParam>, used: &mut HashSet<String>, base: &str, name: String, kind: PluginParamKind, group: &str| {
        let id = sanitize_id(base, used);
        params.push(PluginParam { id, name, kind, group: group.to_string() });
    };

    for p in order {
        if !p.kind.has_value() {
            continue;
        }
        let name = label_of(p);
        let group = group_path(p, &by_name, &page_of);
        let secret = p.props.flag(PP_SECRET, false);
        let dflt = p.props.all(PP_DEFAULT);
        let d = |i: usize| dflt.get(i).and_then(as_double).unwrap_or(0.0);
        match p.kind {
            PKind::Double => {
                if secret {
                    continue;
                }
                let ty = p.props.str(PP_DOUBLE_TYPE).unwrap_or_default();
                let kind = if ty == DOUBLE_TYPE_ANGLE { PluginParamKind::Angle { default: d(0) } } else { slider(p, 0, false, d(0)) };
                push(&mut params, &mut used, &p.name, name, kind, &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n: 1 }, choices: vec![] });
                flat += 1;
            }
            PKind::Int => {
                if secret {
                    continue;
                }
                push(&mut params, &mut used, &p.name, name, slider(p, 0, true, d(0)), &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n: 1 }, choices: vec![] });
                flat += 1;
            }
            PKind::Bool => {
                if secret {
                    continue;
                }
                push(&mut params, &mut used, &p.name, name, PluginParamKind::Checkbox { default: d(0) != 0.0 }, &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n: 1 }, choices: vec![] });
                flat += 1;
            }
            PKind::Choice | PKind::StrChoice => {
                if secret {
                    continue;
                }
                let mut options = p.props.strs(PP_CHOICE_OPTION);
                let enums = p.props.strs(PP_CHOICE_ENUM);
                if options.is_empty() {
                    options = vec!["(none)".into()];
                }
                let (default, choices) = if p.kind == PKind::StrChoice {
                    let values: Vec<String> = (0..options.len()).map(|i| enums.get(i).cloned().unwrap_or_else(|| options[i].clone())).collect();
                    let want = dflt.first().and_then(as_string).unwrap_or_default();
                    (values.iter().position(|v| *v == want).unwrap_or(0) as u32, values)
                } else {
                    (d(0).max(0.0) as u32, vec![])
                };
                let default = if (default as usize) < options.len() { default } else { 0 };
                push(&mut params, &mut used, &p.name, name, PluginParamKind::Popup { options, default }, &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n: 1 }, choices });
                flat += 1;
            }
            PKind::Rgb | PKind::Rgba => {
                if secret {
                    continue;
                }
                let n = if p.kind == PKind::Rgb { 3 } else { 4 };
                let default = [d(0), d(1), d(2), if n == 4 { d(3) } else { 1.0 }];
                push(&mut params, &mut used, &p.name, name, PluginParamKind::Color { default }, &group);
                // The manifest colour is always RGBA; an RGB parameter reads the first three.
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n }, choices: vec![] });
                flat += 4;
            }
            PKind::Double2D | PKind::Double3D => {
                if secret {
                    continue;
                }
                let n = if p.kind == PKind::Double2D { 2 } else { 3 };
                let ty = p.props.str(PP_DOUBLE_TYPE).unwrap_or_default();
                let xy = ty == DOUBLE_TYPE_XY_ABSOLUTE || ty == DOUBLE_TYPE_XY;
                // Only position values (double type XY / XY absolute) are on-screen points. Plain
                // (the OFX default), scale and any other double type are one slider per component.
                if xy {
                    let normalised = p.props.str(PP_DEFAULT_COORDINATE_SYSTEM).as_deref() == Some(COORDS_NORMALISED);
                    let flip = ty == DOUBLE_TYPE_XY_ABSOLUTE;
                    let kind = if n == 3 {
                        PluginParamKind::Point3 { default: [d(0), d(1), d(2)] }
                    } else {
                        let (fx, fy) = if normalised {
                            (d(0), if flip { 1.0 - d(1) } else { d(1) })
                        } else {
                            (d(0) / NOMINAL_PROJECT[0], if flip { 1.0 - d(1) / NOMINAL_PROJECT[1] } else { d(1) / NOMINAL_PROJECT[1] })
                        };
                        PluginParamKind::Point { default: [fx, fy] }
                    };
                    push(&mut params, &mut used, &p.name, name, kind, &group);
                    bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Pos { at: flat, with_z: n == 3, flip }, choices: vec![] });
                    flat += n;
                } else {
                    let suffix = ["x", "y", "z"];
                    for (i, s) in suffix.iter().enumerate().take(n) {
                        let base = format!("{}_{s}", p.name);
                        let label = format!("{} {}", name, dim_label(p, i, &s.to_uppercase()));
                        push(&mut params, &mut used, &base, label, slider(p, i, false, d(i)), &group);
                    }
                    bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n }, choices: vec![] });
                    flat += n;
                }
            }
            PKind::Int2D | PKind::Int3D => {
                if secret {
                    continue;
                }
                let n = if p.kind == PKind::Int2D { 2 } else { 3 };
                let suffix = ["x", "y", "z"];
                for (i, s) in suffix.iter().enumerate().take(n) {
                    let base = format!("{}_{s}", p.name);
                    let label = format!("{} {}", name, dim_label(p, i, &s.to_uppercase()));
                    push(&mut params, &mut used, &base, label, slider(p, i, true, d(i)), &group);
                }
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Flat { at: flat, n }, choices: vec![] });
                flat += n;
            }
            PKind::Str => {
                let default = dflt.first().and_then(as_string).unwrap_or_default();
                let kind = if secret { PluginParamKind::Hidden { default } } else { PluginParamKind::Text { default } };
                push(&mut params, &mut used, &p.name, name, kind, &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Text { at: texts }, choices: vec![] });
                texts += 1;
            }
            PKind::Custom => {
                let default = dflt.first().and_then(as_string).unwrap_or_default();
                push(&mut params, &mut used, &p.name, name, PluginParamKind::Hidden { default }, &group);
                bindings.items.push(Binding { name: p.name.clone(), kind: p.kind, src: Src::Text { at: texts }, choices: vec![] });
                texts += 1;
            }
            PKind::Bytes | PKind::Group | PKind::Page | PKind::Push => {}
        }
    }
    Mapped { params, bindings }
}

/// How many `f64`s the manifest kinds flatten to (mirrors `PluginParamKind::width`).
pub fn flat_width(kind: &PluginParamKind) -> usize {
    match kind {
        PluginParamKind::Point { .. } => 2,
        PluginParamKind::Point3 { .. } => 3,
        PluginParamKind::Color { .. } => 4,
        PluginParamKind::Text { .. } | PluginParamKind::Hidden { .. } => 0,
        _ => 1,
    }
}
