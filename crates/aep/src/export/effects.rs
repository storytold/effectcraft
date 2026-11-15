//! Effects: the inverse of the importer, through the same table (`crate::effect_map`).
//! An EffectCraft effect the table maps is written as the After Effects effect it came from:
//!
//! * the instance is the effect's default instance, built in code from the effect table
//!   (`fx_defs`, `fxsynth`: `tdmn` + `sspc` with its full `parT` and the records After Effects
//!   always keeps: the input layer, buttons, group markers, hidden entries, arbitrary data), with
//!   its instance name and the owning layer's id in the input-layer record;
//! * a parameter whose value differs from its `pard` default gets a value record (absent =
//!   default, `docs/aep-format.md` 3.7). The record is built from the **record of its parameter
//!   type**: over the whole corpus every static value record of a type has the same `tdsb` and
//!   `tdb4` apart from the time scale (`@12`) and, for points, the layer aspect (`@24`), and a
//!   slider's `tdum` / `tduM` repeat its `pard` slider range (float sliders `f32` @112 / @116,
//!   fixed sliders 16.16 and integer sliders `i32` @132 / @136); its `tdsn` is the parameter's
//!   name (a checkbox's `pdnm` label); `tests/effects.rs` rebuilds every value record of every
//!   effect fixture and compares it with After Effects' bytes;
//! * a keyed parameter turns its record into the keyed form of its kind (fixed slider 363, colour
//!   364, point 382, float slider 397: `tdb4@5` bit 0 cleared, `cdat` replaced by the key list);
//! * an expression is stored as on any property (534, R15);
//! * Hue/Saturation's master values and Levels' RGB values are patched into the effect's
//!   arbitrary data (444-446, 512-516);
//! * the project lists each effect type in `Pefl` and keeps one definition copy per type in
//!   `EfdG`; the first instance of a type in the file holds the full parameter definitions
//!   (`parT`), every later one an empty `parT` (383).
//!
//! Units are the importer's inverse (`Conv`): colours straight RGBA 0..1 to ARGB 0..255 (alpha
//! 255), points layer pixels to fractions of the layer size, percent sliders by their valid
//! maximum, popups by item name to the file's 1-based index, checkboxes 1.0 when on.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::effect_map::{Conv, EffectMap, Extra, HUE_SATURATION_MASTER, LEVELS_RGB, by_effect, by_match_name, popup_keys_match};
use crate::{AepFile, CURVE_IDENTITY, Chunk, CurvesData, MeshData, ParamKind};
use effectcraft_keyframe::{Interp, Value};
use effectcraft_project::{GroupKind, Layer, LayerId, Node, ParamUi, PropGroup, Property};

use crate::export::fx_defs::{FX_DEFS, FxDef, PDef};
use crate::export::keys::{fill_key_list, replace_cdat, spatial_record, timed_keys};
use crate::export::plan::{CompPlan, EaseSlot, KeyPlan, LIMIT};
use crate::export::report::{Collector, LossCategory};
use crate::export::synth::{NO_NAME, is_tdmn, tdmn, tdmn_name, tdsn, utf8_in};
use crate::export::tree::{R, child_data, child_mut, data_mut, kids_mut, list_mut, put, put_f64, put_u8, put_u32, tpl};

/// How a parameter is stored in its value record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PKind {
    /// ARGB 0..255 (4 components).
    Color,
    /// Two fractions of the layer size.
    Point,
    /// A fixed slider (pard type 2): the number as it is.
    Fixed,
    /// An angle in degrees (pard type 3).
    Angle,
    /// An integer slider (pard type 1).
    Int,
    /// A float slider, stored as the float32 After Effects keeps (widened to f64).
    Float,
    /// A popup, stored 1-based.
    Popup,
    /// A checkbox, stored 1.0 when on.
    Checkbox,
}

impl PKind {
    fn of(k: ParamKind) -> Option<PKind> {
        Some(match k {
            ParamKind::Color => PKind::Color,
            ParamKind::Point => PKind::Point,
            ParamKind::FixedSlider => PKind::Fixed,
            ParamKind::Angle => PKind::Angle,
            ParamKind::IntSlider => PKind::Int,
            ParamKind::FloatSlider => PKind::Float,
            ParamKind::Popup => PKind::Popup,
            ParamKind::Checkbox => PKind::Checkbox,
            _ => return None,
        })
    }

    /// The `pard` type code (`docs/aep-format.md` 3.7).
    fn code(self) -> u32 {
        match self {
            PKind::Int => 1,
            PKind::Fixed => 2,
            PKind::Angle => 3,
            PKind::Checkbox => 4,
            PKind::Color => 5,
            PKind::Point => 6,
            PKind::Popup => 7,
            PKind::Float => 10,
        }
    }

    fn dims(self) -> usize {
        match self {
            PKind::Color => 4,
            PKind::Point => 2,
            _ => 1,
        }
    }
}

/// A parameter value in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ParamValue {
    Static(Vec<f64>),
    Keys(Vec<KeyPlan>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParamPlan {
    pub index: u16,
    pub kind: PKind,
    pub value: ParamValue,
    /// The expression text and its enable switch.
    pub expr: Option<(String, bool)>,
}

/// An effect instance in the file's units.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EffectPlan {
    pub match_name: &'static str,
    pub name: String,
    pub params: Vec<ParamPlan>,
    /// New arbitrary data for the parameter with this index (Hue/Saturation, Levels).
    pub data: Option<(u16, Vec<u8>)>,
    /// The layer each layer parameter names (by its index); absent = the owning layer.
    pub layers: Vec<(u16, LayerPick)>,
    /// The fx switch (the value group's `tdsb`: 1 on, 0 off).
    pub enabled: bool,
}

/// What a layer parameter names (`docs/aep-format.md` 3.7: its record always exists, its `tdpi`
/// is the chosen layer's id, the owning layer's own id by default, 0 for None; 320, 321, 904).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayerPick {
    /// The layer that owns the effect (After Effects' default).
    Owner,
    /// None.
    Nothing,
    /// Another layer of the composition.
    Layer(LayerId),
}

fn sane(v: f64) -> Option<f64> {
    (v.is_finite() && v.abs() <= LIMIT).then_some(v)
}

// ---------------------------------------------------------------------------------------------
// The parameter definitions of the prototypes (read once from the embedded effect template).

/// One parameter definition of a prototype instance (`pard`, `pdnm`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ParDef {
    /// The parameter's match name (`<effect>-NNNN`), the name of its value record. The number in
    /// it is not the position in `parT` (Fill: position 2 is `ADBE Fill-0007`, 215).
    pub record: String,
    /// `pard@12`.
    pub code: u32,
    /// The default in stored units (`pard@56`).
    pub default: Vec<f64>,
    /// The slider range a value record repeats as `tdum` / `tduM`.
    pub range: Option<(f64, f64)>,
    /// A slider's valid maximum (the table's scale check).
    pub valid_max: Option<f64>,
    /// The default arbitrary data (the `aRbp` after the `pard`).
    pub data: Option<Vec<u8>>,
}

/// A fixed slider's or angle's value as its 16.16 `pard` copy holds it (`fxsynth::pard`).
fn fixed(v: f64) -> f64 {
    f64::from(crate::export::fxsynth::fixed_i32(v)) / 65536.0
}

impl ParDef {
    /// The definition of parameter `p` of the table row `fx`, in the stored units the
    /// prototype's `pard` holds (`fxsynth::default_sspc` writes it from the same row): fixed
    /// sliders, angles and points at 16.16, integers, checkboxes and popups rounded, colours as
    /// bytes, float slider ranges as `f32`. Read straight from the table; the round trip through
    /// the encoded `pard` that this replaces is the reference in the tests.
    fn from_table(fx: &FxDef, p: &PDef) -> ParDef {
        let d = |i: usize| p.default.get(i).copied().unwrap_or(0.0);
        let int = |v: f64| f64::from(crate::export::fxsynth::int_i32(v));
        let f32_of = |v: f64| f64::from(v as f32);
        let default: Vec<f64> = match p.code {
            10 => vec![d(0)],
            2 | 3 => vec![fixed(d(0))],
            1 => vec![int(d(0))],
            // On when the stored word is not 0 (as the importer reads it).
            4 => vec![if d(0).round().max(0.0) as u32 != 0 { 1.0 } else { 0.0 }],
            7 => vec![f64::from(d(0).round().clamp(0.0, f64::from(u16::MAX)) as u32)],
            // Fractions of the layer, or percent after an After Effects re-save (see the reader).
            6 => {
                let pt = [fixed(d(0)), fixed(d(1))];
                if pt.iter().any(|c| c.abs() > 1.0 + 1e-9) { pt.iter().map(|c| c / 100.0).collect() } else { pt.to_vec() }
            }
            5 => (0..4).map(|i| f64::from(d(i).round().clamp(0.0, 255.0) as u8)).collect(),
            _ => Vec::new(),
        };
        // A slider without a range or valid maximum in the table stores zeros there.
        let (lo, hi) = p.range.unwrap_or((0.0, 0.0));
        let max = p.valid_max.unwrap_or(0.0);
        let (range, valid_max) = match p.code {
            10 => (Some((f32_of(lo), f32_of(hi))), Some(f32_of(max))),
            2 => (Some((fixed(lo), fixed(hi))), Some(fixed(max))),
            1 => (Some((int(lo), int(hi))), Some(int(max))),
            _ => (None, None),
        };
        // The record is the text of its `tdmn` as the exporter reads it back (`synth::tdmn_name`),
        // so the instance finds its records by it.
        let record = tdmn_name(&tdmn(&fx.record(p))).unwrap_or_default();
        // The prototype's `pard` holds no name and its `pdnm` is empty: no name, no popup items.
        let data = if p.code == 11 { crate::export::fxsynth::default_data(fx.match_name) } else { None };
        ParDef { record, code: u32::from(p.code), default, range, valid_max, data }
    }

    /// The definitions of the table row `fx`, by position (the table's index).
    fn of_effect(fx: &FxDef) -> HashMap<u16, ParDef> {
        let mut out = HashMap::with_capacity(fx.params.len());
        let mut position = 0u16;
        for p in fx.params {
            out.insert(position, ParDef::from_table(fx, p));
            position = position.saturating_add(1);
        }
        out
    }
}

/// The parameter definitions of every effect type, by match name (from the effect table the
/// default instances are built from).
fn defs() -> &'static HashMap<String, HashMap<u16, ParDef>> {
    static DEFS: OnceLock<HashMap<String, HashMap<u16, ParDef>>> = OnceLock::new();
    DEFS.get_or_init(|| FX_DEFS.iter().map(|fx| (fx.match_name.to_string(), ParDef::of_effect(fx))).collect())
}

// ---------------------------------------------------------------------------------------------
// Planning.

/// The stored components of an EffectCraft value, `None` when it does not fit.
fn stored(conv: Conv, kind: PKind, v: &Value, size: (f64, f64), ui: &ParamUi) -> Option<Vec<f64>> {
    let c = v.components();
    let g = |i: usize| c.get(i).copied().and_then(sane);
    let one = |x: f64| -> Vec<f64> {
        match kind {
            // The float32 After Effects keeps (393, 394).
            PKind::Float => vec![f64::from(x as f32)],
            PKind::Int => vec![x.round()],
            _ => vec![x],
        }
    };
    Some(match conv {
        Conv::Color => {
            let ch = |i: usize| g(i).map(|x| x.clamp(0.0, 1.0) * 255.0);
            vec![255.0, ch(0)?, ch(1)?, ch(2)?]
        }
        Conv::Point => vec![g(0)? / size.0.max(1.0), g(1)? / size.1.max(1.0)],
        Conv::Percent { valid_max } => one(g(0)? * valid_max / 100.0),
        Conv::Plain { .. } | Conv::Angle => one(g(0)?),
        Conv::Factor { factor, .. } => one(g(0)? / factor),
        Conv::Checkbox => match v {
            Value::Bool(b) => vec![if *b { 1.0 } else { 0.0 }],
            _ => vec![if g(0)? != 0.0 { 1.0 } else { 0.0 }],
        },
        Conv::Popup { items, .. } => {
            let i = match v {
                Value::Enum(i) => usize::try_from(*i).ok()?,
                _ => usize::try_from(g(0)?.round() as i64).ok()?,
            };
            let ParamUi::Popup { options } = ui else { return None };
            let name = options.get(i)?;
            // The prototype's `pdnm` names no items: the table's are After Effects'.
            let at = items.iter().position(|f| f.trim() != "(-" && popup_keys_match(f, name))?;
            vec![f64::from(u32::try_from(at).ok()?.checked_add(1)?)]
        }
        // Not a value record (layer parameters, histograms).
        Conv::LayerRef { .. } | Conv::Skip => return None,
    })
}

/// Whether a popup's items are After Effects' items in the same order (separators included, as
/// After Effects counts them), so an expression's 1-based result picks the same item in both.
fn same_popup_order(items: &[&str], ui: &ParamUi) -> bool {
    let ParamUi::Popup { options } = ui else { return false };
    items.len() == options.len() && items.iter().zip(options).all(|(f, o)| f.trim() != "(-" && popup_keys_match(f, o))
}

/// [`stored`], reporting what it changes in a value it can store: a colour channel outside 0 to 1
/// (written as the nearest limit) and a fraction on an integer parameter (rounded).
fn stored_reported(conv: Conv, kind: PKind, v: &Value, (size, ui): ((f64, f64), &ParamUi), what: &str, col: &mut Collector) -> Option<Vec<f64>> {
    let s = stored(conv, kind, v, size, ui)?;
    if matches!(conv, Conv::Color) {
        let c = v.components();
        if c.iter().take(3).any(|x| !(0.0..=1.0).contains(x)) {
            col.lose(LossCategory::InvalidValues, format!("{what}: a colour channel outside 0 to 1 is written as the nearest limit"));
        }
        // After Effects' colour parameters have no opacity (written as 255); Drop Shadow, for
        // one, uses EffectCraft's.
        if c.get(3).is_some_and(|a| a.to_bits() != 1f64.to_bits()) {
            col.lose(LossCategory::Effects, format!("{what}: the colour's opacity is not written"));
        }
    }
    if kind == PKind::Int
        && let Some(raw) = stored(conv, PKind::Fixed, v, size, ui)
        && raw.iter().zip(&s).any(|(a, b)| a.to_bits() != b.to_bits())
    {
        col.lose(LossCategory::Effects, format!("{what}: {raw:?} is written as the whole number {s:?}"));
    }
    Some(s)
}

fn is_default(kind: PKind, s: &[f64], def: &ParDef) -> bool {
    let tol = |a: f64, b: f64| {
        let t = match kind {
            // 16.16 defaults.
            PKind::Fixed | PKind::Angle | PKind::Point => 1.0 / 65536.0,
            _ => 1e-9 * a.abs().max(b.abs()).max(1.0),
        };
        (a - b).abs() <= t
    };
    match kind {
        // Alpha is not editable in After Effects; the RGB channels decide.
        PKind::Color => s.get(1..4).zip(def.default.get(1..4)).is_some_and(|(a, b)| a.iter().zip(b).all(|(x, y)| tol(*x, *y))),
        _ => s.len() == def.default.len() && s.iter().zip(&def.default).all(|(a, b)| tol(*a, *b)),
    }
}

/// Plans the effects of `layer` (its source is `size` pixels) and reports what cannot be written.
pub(crate) fn plan_effects(layer: &Layer, size: (u32, u32), cp: &CompPlan, comp_name: &str, col: &mut Collector) -> Vec<EffectPlan> {
    let Some(fx) = layer.effects() else { return Vec::new() };
    let size = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let mut out = Vec::new();
    for node in &fx.children {
        let Node::Group(g) = node else { continue };
        let here = format!("{comp_name}: '{}' effect '{}'", layer.name, g.name);
        let GroupKind::Effect { effect } = &g.kind else {
            col.lose(LossCategory::Effects, format!("{here}: not an effect group"));
            continue;
        };
        let (Some(spec), Some(pdefs)) = (by_effect(effect), by_effect(effect).and_then(|s| defs().get(s.match_name))) else {
            col.lose(LossCategory::Effects, format!("{comp_name}: '{}' has the effect '{}' ({effect})", layer.name, g.name));
            continue;
        };
        if g.label.is_some() {
            col.lose(LossCategory::Effects, format!("{here}: the effect's label colour is not written"));
        }
        let mut params = Vec::new();
        let mut layers = Vec::new();
        for pm in spec.params {
            let Some(prop) = g.prop(pm.target) else { continue };
            let Ok(index) = u16::try_from(pm.index) else { continue };
            if let Conv::LayerRef { own } = pm.conv {
                if let Some(pick) = plan_layer_param(layer, g, pm.target, own, prop, &here, col) {
                    layers.push((index, pick));
                }
                continue;
            }
            let (Some(kind), Some(def)) = (PKind::of(pm.kind), pdefs.get(&index)) else {
                col.lose(LossCategory::Effects, format!("{here}: parameter '{}' has no prototype definition", prop.name));
                continue;
            };
            if def.code != kind.code() {
                col.lose(LossCategory::Effects, format!("{here}: parameter '{}': the prototype has another type", prop.name));
                continue;
            }
            if let Some(pp) = plan_param(ParamIn { conv: pm.conv, kind, def, prop, size }, index, cp, &here, col) {
                params.push(pp);
            }
        }
        let data = match spec.extra {
            Extra::Curves => plan_curves(g, pdefs, &here, col),
            Extra::MeshWarp => plan_mesh(g, pdefs, size, &here, col),
            _ => plan_extra(spec, g, pdefs, &here, col),
        };
        report_unmapped(g, (effect, spec), size, &here, col);
        col.exported.effects = col.exported.effects.saturating_add(1);
        out.push(EffectPlan {
            match_name: spec.match_name,
            name: crate::export::plan::cut_name(&g.name, 255, &here, col),
            params,
            data,
            layers,
            enabled: g.enabled,
        });
    }
    out
}

/// A layer parameter (the importer's inverse, `Conv::LayerRef`): an unset layer is None where the
/// owning layer is EffectCraft's own default (`own`, Layer Control) and the owning layer
/// elsewhere (Compound Blur, Displacement Map, Set Matte: unset means "this layer" in both
/// programs); `None` = the default, the owning layer.
fn plan_layer_param(layer: &Layer, g: &PropGroup, target: &str, own: bool, prop: &Property, here: &str, col: &mut Collector) -> Option<LayerPick> {
    let what = format!("{here}: parameter '{}'", prop.name);
    if !prop.keys.is_empty() || prop.has_expression() {
        col.lose(LossCategory::Effects, format!("{what}: keys or an expression on a layer parameter are not written, its value is"));
    }
    let pick = match prop.value {
        Value::Layer(None) if own => LayerPick::Nothing,
        Value::Layer(None) => LayerPick::Owner,
        Value::Layer(Some(effectcraft_project::NO_LAYER)) => LayerPick::Nothing,
        Value::Layer(Some(id)) if id == layer.id.0 => LayerPick::Owner,
        Value::Layer(Some(id)) => LayerPick::Layer(LayerId(id)),
        _ => {
            col.lose(LossCategory::InvalidValues, format!("{what}: not a layer, the default was written"));
            LayerPick::Owner
        }
    };
    // After Effects hands the named layer's source to the effect (904 E24, the importer's
    // companion `Source` = 0); another choice is not sampled. Layer Control (`own`) has no
    // source choice in After Effects: it only names the layer.
    if let LayerPick::Layer(_) = pick
        && !own
        && g.prop(&format!("{target}Source")).is_some_and(|s| !matches!(s.value, Value::Enum(0)))
    {
        col.lose(LossCategory::Effects, format!("{what}: the named layer's masks and effects are not applied in After Effects (it takes the layer's source)"));
    }
    (pick != LayerPick::Owner).then_some(pick)
}

/// Hue/Saturation's master values or Levels' RGB values, as new arbitrary data (the prototype's
/// default data with the values patched in); `None` when they are at the default.
fn plan_extra(spec: &EffectMap, g: &PropGroup, pdefs: &HashMap<u16, ParDef>, here: &str, col: &mut Collector) -> Option<(u16, Vec<u8>)> {
    let ids: &[&str] = match spec.extra {
        // (the Cryptomatte plug-in's settings are its own binary layout: not written, reported below)
        Extra::None | Extra::Curves | Extra::MeshWarp | Extra::Lumetri | Extra::Cryptomatte | Extra::Extractor | Extra::Identifier => return None,
        Extra::HueSaturation => &HUE_SATURATION_MASTER,
        Extra::Levels => &LEVELS_RGB,
    };
    let mut values = Vec::with_capacity(ids.len());
    for id in ids {
        let v = g.prop(id).and_then(|p| {
            if !p.keys.is_empty() || p.has_expression() {
                col.lose(
                    LossCategory::Effects,
                    format!("{here}: keys or an expression on '{}' are not written (arbitrary data, the static value is written)", p.name),
                );
            }
            let v = p.value.components().first().copied();
            if v.is_some_and(|v| sane(v).is_none()) {
                col.lose(LossCategory::InvalidValues, format!("{here}: '{}' is beyond the format's range, the default is written", p.name));
            }
            v.and_then(sane)
        });
        values.push(v);
    }
    let (index, mut data) = pdefs.iter().find(|(_, d)| d.code == 11).and_then(|(i, d)| d.data.clone().map(|data| (*i, data)))?;
    let before = data.clone();
    for (k, (id, v)) in ids.iter().zip(values).enumerate() {
        let Some(v) = v else { continue };
        if spec.extra == Extra::HueSaturation {
            // Three i32 at 0, 4, 8 (444-446): whole degrees and percents.
            let r = v.round().clamp(-1e6, 1e6);
            if (r - v).abs() > 1e-9 {
                col.lose(LossCategory::Effects, format!("{here}: '{id}' {v} is written as {r} (After Effects keeps whole numbers)"));
            }
            put(&mut data, k * 4, &(r as i32).to_be_bytes()).ok()?;
        } else {
            // The RGB channel's five f32 at 8.. (512-516).
            put(&mut data, 8 + k * 4, &(v as f32).to_be_bytes()).ok()?;
        }
    }
    (data != before).then_some((index, data))
}

/// `x,y` pairs (EffectCraft's curve points; whitespace or `;` between pairs), and how many
/// entries are not a pair of finite numbers (left out).
fn pairs_of(s: &str) -> (Vec<(f64, f64)>, usize) {
    let mut unreadable = 0usize;
    let pairs = s
        .split(|c: char| c.is_whitespace() || c == ';')
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let pair = (|| {
                let (x, y) = t.split_once(',')?;
                let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().split(',').next()?.trim().parse::<f64>().ok()?);
                (x.is_finite() && y.is_finite()).then_some((x, y))
            })();
            if pair.is_none() {
                unreadable = unreadable.saturating_add(1);
            }
            pair
        })
        .collect();
    (pairs, unreadable)
}

/// The arbitrary-data parameter's index and default data.
fn data_param(pdefs: &HashMap<u16, ParDef>) -> Option<(u16, Vec<u8>)> {
    pdefs.iter().find(|(_, d)| d.code == 11).and_then(|(i, d)| d.data.clone().map(|data| (*i, data)))
}

fn static_str<'p>(g: &'p PropGroup, id: &str, here: &str, col: &mut Collector) -> Option<&'p str> {
    let p = g.prop(id)?;
    if !p.keys.is_empty() || p.has_expression() {
        col.lose(LossCategory::Effects, format!("{here}: keys or an expression on '{}' are not written (arbitrary data, the static value is written)", p.name));
    }
    match &p.value {
        Value::Str(s) => Some(s.as_str()),
        _ => {
            col.lose(LossCategory::InvalidValues, format!("{here}: '{}' is not text, the default is written", p.name));
            None
        }
    }
}

/// Curves (FORMAT S.1): EffectCraft's points (0..1) as After Effects' whole 0..255 points, with
/// the lookup tables After Effects stores for them (the natural spline); `None` at the default.
fn plan_curves(g: &PropGroup, pdefs: &HashMap<u16, ParDef>, here: &str, col: &mut Collector) -> Option<(u16, Vec<u8>)> {
    let mut curves: [Vec<(u8, u8)>; 5] = Default::default();
    for (slot, id) in curves.iter_mut().zip(["rgb", "red", "green", "blue", "alpha"]) {
        let (pts, unreadable) = static_str(g, id, here, col).map(pairs_of).unwrap_or_default();
        if unreadable > 0 {
            col.lose(LossCategory::InvalidValues, format!("{here}: curve '{id}': {unreadable} entries are not an x,y pair of numbers and are not written"));
        }
        let mut ints: Vec<(u8, u8)> = Vec::with_capacity(pts.len());
        let (mut rounded, mut clipped) = (false, false);
        for (x, y) in pts {
            let (x, y) = (x * 255.0, y * 255.0);
            let (xs, ys) = (x.clamp(0.0, 255.0), y.clamp(0.0, 255.0));
            clipped |= xs.to_bits() != x.to_bits() || ys.to_bits() != y.to_bits();
            rounded |= (xs - xs.round()).abs() > 1e-3 || (ys - ys.round()).abs() > 1e-3;
            ints.push((xs.round() as u8, ys.round() as u8));
        }
        // Points with the same input are merged as EffectCraft's own curve merges them (the
        // first is kept), and fewer than two points are the identity there too.
        ints.sort_by_key(|p| p.0);
        ints.dedup_by_key(|p| p.0);
        if clipped {
            col.lose(LossCategory::Effects, format!("{here}: curve '{id}' points outside 0 to 1 are written as the nearest limit"));
        }
        if rounded {
            col.lose(LossCategory::Effects, format!("{here}: curve '{id}' points rounded to whole levels (After Effects keeps 0..255)"));
        }
        if ints.len() > 16 {
            col.lose(LossCategory::Effects, format!("{here}: curve '{id}' has {} points, After Effects keeps 16; the first 16 are written", ints.len()));
            ints.truncate(16);
        }
        *slot = if ints.len() < 2 { CURVE_IDENTITY.to_vec() } else { ints };
    }
    let d = CurvesData { curves };
    if d.is_default() {
        return None;
    }
    let (index, _) = data_param(pdefs)?;
    match d.to_bytes() {
        Some(bytes) => Some((index, bytes)),
        None => {
            col.lose(LossCategory::Effects, format!("{here}: the curves cannot be written, the default is"));
            None
        }
    }
}

/// Mesh Warp (FORMAT S.2): EffectCraft's per-vertex pixel offsets as After Effects' vertices
/// (fractions of the layer) with their default handles moved along; `None` at the default.
fn plan_mesh(g: &PropGroup, pdefs: &HashMap<u16, ParDef>, size: (f64, f64), here: &str, col: &mut Collector) -> Option<(u16, Vec<u8>)> {
    let int = |id: &str| g.prop(id).map(|p| p.value.as_f64().round().clamp(1.0, 31.0) as u32).unwrap_or(7);
    let (rows, columns) = (int("rows"), int("columns"));
    // Per vertex `dx,dy` (pixels), optionally followed by the eight pixel moves of its handles
    // (top, right, bottom, left) that the importer keeps.
    let mut unreadable = 0usize;
    let offs: Vec<Vec<f64>> = static_str(g, "mesh", here, col)
        .map(|s| {
            s.split(|c: char| c.is_whitespace() || c == ';')
                .filter(|t| !t.is_empty())
                .filter_map(|t| {
                    let v: Option<Vec<f64>> = t.split(',').map(|x| x.trim().parse::<f64>().ok().filter(|x| x.is_finite())).collect();
                    let v = v.filter(|v| v.len() >= 2);
                    if v.is_none() {
                        unreadable = unreadable.saturating_add(1);
                    }
                    v
                })
                .collect()
        })
        .unwrap_or_default();
    if unreadable > 0 {
        col.lose(LossCategory::InvalidValues, format!("{here}: the Distortion Mesh has {unreadable} entries that are not numbers, left out"));
    }
    let count = usize::try_from((rows + 1) * (columns + 1)).ok()?;
    let moved = offs.iter().any(|v| v.iter().any(|x| *x != 0.0));
    if !moved && rows == 7 && columns == 7 {
        return None;
    }
    if moved && offs.len() != count {
        col.lose(
            LossCategory::Effects,
            format!("{here}: the Distortion Mesh has {} vertices for {columns} x {rows} cells, an undistorted mesh is written", offs.len()),
        );
    }
    let (index, default) = data_param(pdefs)?;
    let tail = MeshData::parse(&default).map(|m| m.tail).unwrap_or([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x0d, 0x0d]);
    let mut vertices = Vec::with_capacity(count);
    for k in 0..count {
        let k32 = u32::try_from(k).ok()?;
        let (i, j) = (k32 % (columns + 1), k32 / (columns + 1));
        let mut v = MeshData::default_vertex(columns, rows, i, j);
        if offs.len() == count
            && let Some(o) = offs.get(k)
        {
            // A moved vertex carries its handles along (985), then each handle its own move.
            let at = |n: usize| o.get(n).copied().unwrap_or(0.0);
            let (fx, fy) = ((at(0) / size.0) as f32, (at(1) / size.1) as f32);
            for (n, p) in v.iter_mut().enumerate() {
                if n == 0 || *p != [0.0, 0.0] {
                    let (hx, hy) = if n == 0 { (0.0, 0.0) } else { ((at(2 * n) / size.0) as f32, (at(2 * n + 1) / size.1) as f32) };
                    *p = [p[0] + fx + hx, p[1] + fy + hy];
                }
            }
        }
        vertices.push(v);
    }
    MeshData { columns, rows, tail, vertices }.to_bytes().map(|b| (index, b))
}

/// EffectCraft parameters the table does not map are lost when they carry anything.
/// Reports the parameters of `g` the table has no record for: with keys or an expression, or at
/// a value other than the one a new instance of the effect has.
fn report_unmapped(g: &PropGroup, (effect, spec): (&str, &EffectMap), size: (f64, f64), here: &str, col: &mut Collector) {
    let mut next = 1;
    let fresh = |size: [f64; 2], next: &mut u64| {
        effectcraft_effects::find(effect).map(|s| effectcraft_effects::instantiate(s, &mut effectcraft_project::build::Ids(next), &g.name, size))
    };
    // A default that depends on the layer size (a centre point) is not compared: the size an
    // effect was added at is not known here.
    let (fresh, other) = (fresh([size.0, size.1], &mut next), fresh([size.0 * 2.0 + 7.0, size.1 * 3.0 + 5.0], &mut next));
    let extra: &[&str] = match spec.extra {
        // Lumetri Color, EXtractoR and IDentifier have no prototype to write from, so they never
        // get this far (they are reported).
        Extra::None | Extra::Lumetri | Extra::Cryptomatte | Extra::Extractor | Extra::Identifier => &[],
        Extra::HueSaturation => &HUE_SATURATION_MASTER,
        Extra::Levels => &LEVELS_RGB,
        Extra::Curves => &["channel", "rgb", "red", "green", "blue", "alpha"],
        Extra::MeshWarp => &["mesh"],
    };
    let mut stack: Vec<(&PropGroup, String)> = vec![(g, String::new())];
    let mut steps = 0usize;
    while let Some((grp, prefix)) = stack.pop() {
        steps += 1;
        if steps > 10_000 {
            break;
        }
        for n in &grp.children {
            match n {
                Node::Group(sub) => stack.push((sub, format!("{prefix}{}/", sub.match_id))),
                Node::Prop(p) => {
                    let path = format!("{prefix}{}", p.match_id);
                    // A layer parameter's source companion is checked with the parameter (`plan_layer_param`).
                    let companion = spec.params.iter().any(|s| matches!(s.conv, Conv::LayerRef { .. }) && path == format!("{}Source", s.target));
                    if spec.params.iter().any(|s| s.target == path) || extra.contains(&path.as_str()) || companion {
                        continue;
                    }
                    if !p.keys.is_empty() || p.has_expression() {
                        col.lose(LossCategory::Effects, format!("{here}: parameter '{}' has no After Effects counterpart", p.name));
                    } else if let Some(d) = fresh.as_ref().and_then(|f| f.prop(&path))
                        && other.as_ref().and_then(|f| f.prop(&path)).is_some_and(|o| o.value == d.value)
                        && d.value != p.value
                    {
                        col.lose(LossCategory::Effects, format!("{here}: parameter '{}' has no After Effects counterpart, its value is not written", p.name));
                    }
                }
            }
        }
    }
}

/// One effect parameter to plan: its table conversion, stored kind, prototype definition and
/// property, and the layer's source size.
#[derive(Clone, Copy)]
struct ParamIn<'a> {
    conv: Conv,
    kind: PKind,
    def: &'a ParDef,
    prop: &'a Property,
    size: (f64, f64),
}

fn plan_param(pin: ParamIn<'_>, index: u16, cp: &CompPlan, here: &str, col: &mut Collector) -> Option<ParamPlan> {
    let ParamIn { conv, kind, def, prop, size } = pin;
    let what = format!("{here}: parameter '{}'", prop.name);
    // The table's scale was verified with this valid maximum (the importer checks the same).
    let vm = match conv {
        Conv::Percent { valid_max } | Conv::Plain { valid_max } | Conv::Factor { valid_max, .. } => Some(valid_max),
        _ => None,
    };
    if let (Some(want), Some(found)) = (vm, def.valid_max)
        && (want - found).abs() > want.abs() * 1e-6
    {
        col.lose(LossCategory::Effects, format!("{what}: the prototype's scale differs from the table's, not written"));
        return None;
    }
    let expr = prop.expr.as_ref().filter(|e| !e.text.is_empty()).map(|e| (e.text.clone(), e.enabled));
    if expr.is_some() {
        col.exported.expressions = col.exported.expressions.saturating_add(1);
        // The expression is written as it is; where the stored value is scaled (a percent or
        // factor parameter has other units) or a popup's items are in another order, its result
        // means something else in After Effects. Popups count from 1 in both programs' expressions.
        let other_units = match conv {
            Conv::Popup { items, .. } => !same_popup_order(items, &prop.ui),
            Conv::Percent { valid_max } => (valid_max - 100.0).abs() > 1e-9,
            Conv::Factor { factor, .. } => (factor - 1.0).abs() > 1e-9,
            _ => false,
        };
        if other_units {
            col.lose(LossCategory::Expressions, format!("{what}: the expression is written, but After Effects reads its result in other units"));
        }
    }
    if prop.keys.is_empty() {
        let Some(s) = stored_reported(conv, kind, &prop.value, (size, &prop.ui), &what, col) else {
            if matches!(conv, Conv::Popup { .. }) {
                col.lose(LossCategory::Effects, format!("{what}: the popup item has no After Effects counterpart, the default was written"));
            } else {
                col.lose(LossCategory::InvalidValues, format!("{what}: a value that cannot be stored, the default was written"));
            }
            return None;
        };
        // A property with an expression keeps its record, also at the default (R15).
        if is_default(kind, &s, def) && expr.is_none() {
            return None;
        }
        return Some(ParamPlan { index, kind, value: ParamValue::Static(s), expr });
    }
    if !matches!(kind, PKind::Fixed | PKind::Float | PKind::Color | PKind::Point) {
        col.lose(
            LossCategory::Effects,
            format!("{what}: keys on a popup, checkbox, angle or integer slider are not written (not sampled), the first key's value was written"),
        );
        let first = prop.keys.first().map(|k| &k.value).unwrap_or(&prop.value);
        let s = stored_reported(conv, kind, first, (size, &prop.ui), &what, col)?;
        return (!is_default(kind, &s, def) || expr.is_some()).then_some(ParamPlan { index, kind, value: ParamValue::Static(s), expr });
    }
    let keys = plan_keys(pin, cp, &what, col);
    if keys.is_empty() {
        return None;
    }
    col.exported.keyframes = col.exported.keyframes.saturating_add(keys.len());
    Some(ParamPlan { index, kind, value: ParamValue::Keys(keys), expr })
}

fn plan_keys(pin: ParamIn<'_>, cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<KeyPlan> {
    let ParamIn { conv, kind, prop, size, .. } = pin;
    let timed = timed_keys(&prop.keys, &cp.tb, what, "key", col);
    // The ease of colour keys is sampled only as speed 0 / influence 0, which After Effects
    // interpolates linearly (598): a colour key is written linear.
    let ease_ok = matches!(kind, PKind::Fixed | PKind::Float | PKind::Point);
    // Ease speeds are in stored units per second; a point's in pixels per second over the layer
    // height (597, 598, 730).
    let factor = match conv {
        Conv::Percent { valid_max } => 100.0 / valid_max,
        Conv::Factor { factor, .. } => factor,
        Conv::Point => size.1.max(1.0),
        _ => 1.0,
    };
    let mut out = Vec::with_capacity(timed.len());
    let beyond = std::cell::Cell::new(false);
    for (u, k) in timed {
        let Ok(time) = i32::try_from(u) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: key time out of range, key dropped"));
            continue;
        };
        let Some(value) = stored_reported(conv, kind, &k.value, (size, &prop.ui), what, col) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: a key value that cannot be stored, key dropped"));
            continue;
        };
        let mut side = |i: Interp| -> u8 {
            match i {
                Interp::Linear => 1,
                Interp::Hold => 3,
                Interp::Bezier if ease_ok => 2,
                Interp::Bezier => {
                    col.lose(
                        LossCategory::ApproximatedKeyframes,
                        format!("{what}: bezier ease of a colour key is written as linear (only speed 0 / influence 0 is sampled)"),
                    );
                    1
                }
            }
        };
        let (in_i, out_i) = (side(k.in_interp), side(k.out_interp));
        let ease = |eases: &[effectcraft_keyframe::Ease], on: bool| -> Vec<EaseSlot> {
            if !on {
                return vec![EaseSlot::default()];
            }
            let e = crate::export::plan::key_ease(eases, 0);
            let speed = sane(e.speed / factor).unwrap_or_else(|| {
                beyond.set(true);
                0.0
            });
            let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
            vec![EaseSlot { speed, influence }]
        };
        let (mut tin, mut tout) = ([0.0; 3], [0.0; 3]);
        if kind == PKind::Point {
            for (t, s) in [(&mut tin, k.spatial_in), (&mut tout, k.spatial_out)] {
                for (o, v) in t.iter_mut().zip([s[0] / size.0, s[1] / size.1]) {
                    *o = sane(v).unwrap_or_else(|| {
                        beyond.set(true);
                        0.0
                    });
                }
            }
        }
        out.push(KeyPlan {
            time,
            value,
            in_interp: in_i,
            out_interp: out_i,
            in_ease: ease(&k.in_ease, in_i == 2),
            out_ease: ease(&k.out_ease, out_i == 2),
            spatial_in: tin,
            spatial_out: tout,
            spatial_auto: kind != PKind::Point || k.spatial_auto,
            roving: false,
            label: k.label.min(16),
        });
        if k.roving {
            col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a roving key is written as a normal key"));
        }
    }
    crate::export::plan::report_beyond(beyond.get(), what, col);
    out
}

// ---------------------------------------------------------------------------------------------
// Emit.

/// The effect prototypes, built in code ([`FxProtos::build`]).
pub(crate) struct FxProtos {
    /// match name -> (`tdmn`, default `sspc`, `EfDf` definition copy).
    defaults: HashMap<String, (Chunk<'static>, Chunk<'static>, Chunk<'static>)>,
    /// `pard` type code -> the static value record of that type.
    kinds: HashMap<u32, Chunk<'static>>,
    /// Keyed value records by kind.
    keyed: Vec<(KeyedKind, Chunk<'static>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyedKind {
    Fixed,
    Color,
    Point,
    Float,
}

fn keyed_kind(k: PKind) -> Option<KeyedKind> {
    match k {
        PKind::Fixed => Some(KeyedKind::Fixed),
        PKind::Color => Some(KeyedKind::Color),
        PKind::Point => Some(KeyedKind::Point),
        PKind::Float => Some(KeyedKind::Float),
        _ => None,
    }
}

impl FxProtos {
    /// The prototypes built in code from the effect table (`fx_defs`, `fxsynth`).
    pub(crate) fn build() -> FxProtos {
        use crate::export::fxsynth::value_spec;
        let mut defaults = HashMap::new();
        for fx in crate::export::fx_defs::FX_DEFS {
            let sspc = crate::export::fxsynth::default_sspc(fx);
            let m = crate::export::synth::tdmn(fx.match_name);
            let efdf = Chunk::list(*b"EfDf", vec![m.clone(), sspc.clone()]);
            defaults.insert(fx.match_name.to_string(), (m, sspc, efdf));
        }
        let kinds = [1u8, 2, 3, 4, 5, 6, 7, 10]
            .iter()
            .filter_map(|c| value_spec(*c, None).map(|s| (u32::from(*c), crate::export::synth::tdbs(&s, false, 25_600))))
            .collect();
        let keyed = [(KeyedKind::Fixed, 2u8), (KeyedKind::Color, 5), (KeyedKind::Point, 6), (KeyedKind::Float, 10)]
            .iter()
            .filter_map(|(k, c)| value_spec(*c, None).map(|s| (*k, crate::export::synth::tdbs(&s, true, 25_600))))
            .collect();
        FxProtos { defaults, kinds, keyed }
    }
}

/// Which effect types already have an instance with full definitions in the file, in the order
/// they first appear.
#[derive(Default)]
pub(crate) struct FxUse {
    pub types: Vec<&'static str>,
}

/// The value record of `p`, built from the record of its parameter type.
pub(crate) fn value_record(protos: &FxProtos, p: &ParamPlan, def: &ParDef, source_size: (u32, u32)) -> R<(Chunk<'static>, Chunk<'static>)> {
    let mut rec = protos.kinds.get(&p.kind.code()).cloned().ok_or_else(|| tpl("effect value prototype missing"))?;
    // The record keeps the prototype's `tdsn`: the prototypes name no parameter.
    if p.kind == PKind::Point {
        // The layer's aspect, as on the Anchor Point (369: 16 / 9, 383: 1 on 100 x 100).
        put_f64(child_data(&mut rec, b"tdb4")?, 24, f64::from(source_size.0) / f64::from(source_size.1.max(1)))?;
    }
    if let Some((lo, hi)) = def.range {
        put_f64(child_data(&mut rec, b"tdum")?, 0, lo)?;
        put_f64(child_data(&mut rec, b"tduM")?, 0, hi)?;
    }
    match &p.value {
        ParamValue::Static(v) => {
            let cdat = child_data(&mut rec, b"cdat")?;
            cdat.iter_mut().for_each(|b| *b = 0);
            for (i, x) in v.iter().enumerate() {
                put_f64(cdat, i * 8, *x)?;
            }
        }
        ParamValue::Keys(keys) => {
            let kk = keyed_kind(p.kind).ok_or_else(|| tpl("keys on a parameter kind without a keyed prototype"))?;
            let proto = protos.keyed.iter().find(|(k, _)| *k == kk).map(|(_, c)| c).ok_or_else(|| tpl("keyed effect prototype missing"))?;
            let mut list = proto.child_list(b"list").cloned().ok_or_else(|| tpl("keyed effect prototype without keys"))?;
            {
                // Static and keyed records of the same parameter differ in `tdb4` bytes 5 (the
                // static bit), 68 (has keys) and 79 (spatial kinds only): taken from the keyed
                // record of the kind (362 / 363, 359 / 364, 369 / 382, 391 / 397).
                let ktdb4 = proto.child(b"tdb4").and_then(|c| c.bytes()).ok_or_else(|| tpl("keyed effect prototype without tdb4"))?;
                let tdb4 = child_data(&mut rec, b"tdb4")?;
                for at in [5usize, 68, 79] {
                    put_u8(tdb4, at, *ktdb4.get(at).ok_or_else(|| tpl("short tdb4"))?)?;
                }
            }
            let dims = p.kind.dims();
            let size = match kk {
                KeyedKind::Fixed | KeyedKind::Float => 8 + 40 * dims,
                KeyedKind::Color | KeyedKind::Point => 56 + 24 * dims,
            };
            let mut ldat = Vec::with_capacity(size.saturating_mul(keys.len()));
            for k in keys {
                ldat.extend(match kk {
                    KeyedKind::Fixed | KeyedKind::Float => crate::export::emit::plain_record(1, k, size)?,
                    // A colour (flags `0x0001`, 210, 364) or a point (flags `0x0007`, the
                    // temporal ease in layer heights per second, 382, 598).
                    KeyedKind::Color | KeyedKind::Point => spatial_record(k, dims, kk == KeyedKind::Point, size)?,
                });
            }
            fill_key_list(&mut list, ldat, keys.len(), size)?;
            replace_cdat(&mut rec, list)?;
        }
    }
    if let Some((text, enabled)) = &p.expr {
        // `tdb4@120` = 1 and the text before `tdum` (534; unlike the Transform properties,
        // bytes 76 and 77 stay 0); `@119` the disabled switch as on the Transform properties (R15).
        {
            let tdb4 = child_data(&mut rec, b"tdb4")?;
            put_u8(tdb4, 120, 1)?;
            put_u8(tdb4, 119, u8::from(!*enabled))?;
        }
        let kids = kids_mut(&mut rec)?;
        let at = kids.iter().position(|c| c.is(b"tdum")).unwrap_or(kids.len());
        kids.insert(at, Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
    }
    Ok((tdmn(&def.record), rec))
}

/// The `tdmn` + `sspc` of one instance on the layer with id `layer_id`.
fn instance(
    protos: &FxProtos,
    fx: &EffectPlan,
    layer_id: u32,
    layer_ids: &HashMap<LayerId, u32>,
    source_size: (u32, u32),
    uses: &mut FxUse,
) -> R<Vec<Chunk<'static>>> {
    let (tdmn, sspc, _) = protos.defaults.get(fx.match_name).ok_or_else(|| tpl("effect prototype missing"))?;
    let mut sspc = sspc.clone();
    // The prototype's definitions are the table's (the prototype is built from it).
    let pdefs = defs().get(fx.match_name).ok_or_else(|| tpl("effect definitions missing"))?;
    // `fnam` keeps the effect's own name; a name of the user's goes into the value group's
    // `tdsn`, which holds the placeholder `-_0_/-` otherwise (610 against 609).
    let own = child_mut(&mut sspc, b"fnam")?.children().iter().find(|c| c.is(b"Utf8")).and_then(|c| c.bytes()).map(|b| String::from_utf8_lossy(b).to_string());
    if own.as_deref() != Some(fx.name.as_str()) && !fx.name.is_empty() {
        let values = list_mut(&mut sspc, b"tdgp")?;
        let slot = child_mut(values, b"tdsn")?;
        *slot = utf8_in(*b"tdsn", &fx.name);
    }
    // The fx switch: the value group's `tdsb` (`fx-switch` cases).
    put_u32(child_data(list_mut(&mut sspc, b"tdgp")?, b"tdsb")?, 0, u32::from(fx.enabled))?;
    let mut records = Vec::with_capacity(fx.params.len());
    for p in &fx.params {
        let def = pdefs.get(&p.index).ok_or_else(|| tpl("parameter definition missing"))?;
        records.push((p.index, value_record(protos, p, def, source_size)?));
    }
    {
        let values = list_mut(&mut sspc, b"tdgp")?;
        let kids = kids_mut(values)?;
        // The input layer (`-0000`) names the layer that owns the effect (320).
        let tdpi_of = |kids: &mut Vec<Chunk<'static>>, record: &str| -> Option<usize> { kids.iter().position(|c| is_tdmn(c, record)).map(|i| i + 1) };
        let input = pdefs.get(&0).map(|d| d.record.clone()).unwrap_or_default();
        // The prototype's own layer id (its input layer), to tell "the owning layer" from None.
        let mut proto_owner = None;
        if let Some(at) = tdpi_of(kids, &input)
            && let Some(node) = kids.get_mut(at)
        {
            let tdpi = child_data(node, b"tdpi")?;
            proto_owner = tdpi.get(0..4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
            put_u32(tdpi, 0, layer_id)?;
        }
        // Every layer parameter's record exists in the prototype (its `tdpi` the prototype's
        // default): a mapped one names what the plan says (the owning layer by default; a layer
        // that is not exported was reported and left out of the plan in
        // `plan::resolve_effect_layers`); an unmapped one keeps its default, the prototype's own
        // layer becoming the owning layer and None (0) staying None (Timewarp's Matte / Warp
        // Layer, 529).
        let mapped: Vec<u16> = by_match_name(fx.match_name)
            .map(|s| s.params.iter().filter(|p| matches!(p.conv, Conv::LayerRef { .. })).filter_map(|p| u16::try_from(p.index).ok()).collect())
            .unwrap_or_default();
        for (index, def) in pdefs {
            if *index == 0 || def.code != 0 {
                continue;
            }
            let Some(at) = tdpi_of(kids, &def.record) else { continue };
            let Some(node) = kids.get_mut(at) else { continue };
            let tdpi = child_data(node, b"tdpi")?;
            let id = if mapped.contains(index) {
                match fx.layers.iter().find(|(i, _)| i == index).map(|(_, p)| *p) {
                    None | Some(LayerPick::Owner) => layer_id,
                    Some(LayerPick::Nothing) => 0,
                    Some(LayerPick::Layer(l)) => layer_ids.get(&l).copied().unwrap_or(layer_id),
                }
            } else {
                let stored = tdpi.get(0..4).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
                if stored.is_some() && stored == proto_owner { layer_id } else { stored.unwrap_or(0) }
            };
            put_u32(tdpi, 0, id)?;
        }
        // Arbitrary data: the `aRbp` of the `LIST aRbs` after the parameter's record.
        if let Some((index, data)) = &fx.data {
            let want = pdefs.get(index).map(|d| d.record.clone()).unwrap_or_default();
            let i = kids.iter().position(|c| is_tdmn(c, &want)).ok_or_else(|| tpl("arbitrary data record missing"))?;
            let list = kids.iter_mut().skip(i + 1).take(3).find(|c| c.is_list(b"aRbs")).ok_or_else(|| tpl("arbitrary data missing"))?;
            let arbp = child_mut(list, b"aRbp")?;
            *data_mut(arbp)? = data.clone();
        }
        let position: HashMap<String, u16> = pdefs.iter().map(|(i, d)| (d.record.clone(), *i)).collect();
        for (index, (m, r)) in records {
            // Before the first record of a later parameter (or the built-in group, or the end).
            let at = kids
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is(b"tdmn"))
                .find(|(_, c)| {
                    let n = tdmn_name(c).unwrap_or_default();
                    position.get(&n).is_some_and(|i| *i > index) || n == "ADBE Effect Built In Params" || n == "ADBE Group End"
                })
                .map(|(i, _)| i)
                .unwrap_or(kids.len());
            kids.splice(at..at, [m, r]);
        }
    }
    // A popup selection is also written into its `pard` default slot (@56, u32; 377, 390, 324).
    if let Some(part) = sspc.children_mut().iter_mut().find(|c| c.is_list(b"parT")) {
        for p in &fx.params {
            let (PKind::Popup, ParamValue::Static(v)) = (p.kind, &p.value) else { continue };
            let Some(sel) = v.first().map(|x| x.round()).filter(|x| *x >= 1.0 && *x <= 1000.0) else { continue };
            let want = pdefs.get(&p.index).map(|d| d.record.clone()).unwrap_or_default();
            let kids = kids_mut(part)?;
            if let Some(i) = kids.iter().position(|c| is_tdmn(c, &want))
                && let Some(pard) = kids.get_mut(i + 1)
            {
                put_u32(data_mut(pard)?, 56, sel as u32)?;
            }
        }
    }
    if uses.types.contains(&fx.match_name) {
        // A later instance of a type: its definitions come from the project's copy (383).
        let part = list_mut(&mut sspc, b"parT")?;
        *part = Chunk::list(*b"parT", Vec::new());
    } else {
        uses.types.push(fx.match_name);
    }
    Ok(vec![tdmn.clone(), sspc])
}

/// The layer's `ADBE Effect Parade` (`tdmn` + `tdgp`), the effects in stack order.
pub(crate) fn build_parade(
    protos: &FxProtos,
    fxs: &[EffectPlan],
    layer_id: u32,
    layer_ids: &HashMap<LayerId, u32>,
    source_size: (u32, u32),
    uses: &mut FxUse,
) -> R<Vec<Chunk<'static>>> {
    let mut kids = vec![Chunk::data(*b"tdsb", vec![0, 0, 0, 1]), tdsn(NO_NAME)];
    for fx in fxs {
        kids.extend(instance(protos, fx, layer_id, layer_ids, source_size, uses)?);
    }
    kids.push(tdmn(crate::export::synth::GROUP_END));
    Ok(vec![tdmn("ADBE Effect Parade"), Chunk::list(*b"tdgp", kids)])
}

/// Writes the project-level effect lists: `Pefl` (one `pjef` per type, sorted by match name, 365)
/// and `EfdG` (the `EfDC` count, little-endian, and one `EfDf` per type in order of first use)
/// right after `Pefl`.
pub(crate) fn put_project_lists(protos: &FxProtos, file: &mut AepFile<'static>, uses: &FxUse) -> R<()> {
    if uses.types.is_empty() {
        return Ok(());
    }
    let at = file.chunks.iter().position(|c| c.is_list(b"Pefl")).ok_or_else(|| tpl("template without Pefl"))?;
    let mut sorted: Vec<&str> = uses.types.clone();
    sorted.sort_unstable();
    if let Some(pefl) = file.chunks.get_mut(at) {
        let kids = kids_mut(pefl)?;
        kids.clear();
        kids.extend(sorted.iter().map(|n| Chunk::data(*b"pjef", n.as_bytes().to_vec())));
    }
    let n = u32::try_from(uses.types.len()).map_err(|_| tpl("too many effect types"))?;
    let mut efdg = vec![Chunk::data(*b"EfDC", n.to_le_bytes().to_vec())];
    for t in &uses.types {
        let (_, _, def) = protos.defaults.get(*t).ok_or_else(|| tpl("effect prototype missing"))?;
        efdg.push(def.clone());
    }
    file.chunks.insert(at + 1, Chunk::list(*b"EfdG", efdg));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definitions as the importer's decoder reads them back from the prototype's `parT`
    /// encoded from the table (the reference for [`ParDef::from_table`]; one `pard` decoder).
    mod reference {
        use super::super::*;

        /// A definition as the decoder read it, with the name and popup items it also read.
        pub(super) struct Old {
            pub def: ParDef,
            pub name: String,
            pub items: Vec<String>,
        }

        /// An encoded `pard` as the importer's decoder (`container::Pard`) reads it, in the
        /// export's stored units: what `ParDef::from_table` must agree with.
        pub(super) fn from_pard(pard: &[u8], pdnm: Option<&str>, data: Option<&[u8]>) -> Old {
            use crate::ParamValue as Pv;
            let d = crate::container::Pard(pard);
            let kind = d.kind();
            let default: Vec<f64> = match d.default(kind) {
                Some(Pv::Number(x)) => vec![x],
                Some(Pv::Bool(on)) => vec![if on { 1.0 } else { 0.0 }],
                Some(Pv::Index(i)) => vec![f64::from(i)],
                Some(Pv::Point(pt)) => pt.to_vec(),
                // Bytes 0..255, read as fractions.
                Some(Pv::Argb(c)) => c.iter().map(|x| (x * 255.0).round()).collect(),
                Some(Pv::Layer(_)) | None => Vec::new(),
            };
            let items = match pdnm {
                Some(text) if kind == crate::ParamKind::Popup && !text.is_empty() => text.split('|').map(str::to_string).collect(),
                _ => Vec::new(),
            };
            let name = match (kind, pdnm) {
                (crate::ParamKind::Checkbox, Some(text)) if d.name().unwrap_or_default().trim().is_empty() => text.to_string(),
                _ => d.name().unwrap_or_default(),
            };
            let def = ParDef {
                record: String::new(),
                code: d.code().unwrap_or(u32::MAX),
                default,
                range: d.slider_range(kind),
                valid_max: d.valid_max(kind),
                data: data.map(<[u8]>::to_vec),
            };
            Old { def, name, items }
        }

        /// The parameter definitions of an instance's `parT`, by position (the table's index).
        pub(super) fn par_defs(sspc: &Chunk<'_>) -> HashMap<u16, Old> {
            let mut out = HashMap::new();
            let Some(part) = sspc.child_list(b"parT") else { return out };
            let k = part.children();
            let mut i = 0usize;
            let mut position = 0u16;
            while i < k.len() {
                let (Some(m), Some(p)) = (k.get(i), k.get(i + 1)) else { break };
                if !(m.is(b"tdmn") && p.is(b"pard")) {
                    i += 1;
                    continue;
                }
                let name = tdmn_name(m).unwrap_or_default();
                let mut j = i + 2;
                let mut pdnm = None;
                let mut data = None;
                while let Some(c) = k.get(j) {
                    if c.is(b"tdmn") {
                        break;
                    }
                    if c.is(b"pdnm") {
                        pdnm = c.children().iter().find(|u| u.is(b"Utf8")).and_then(|u| u.bytes()).map(|b| String::from_utf8_lossy(b).to_string());
                    }
                    if c.is(b"aRbp") {
                        data = c.bytes();
                    }
                    j += 1;
                }
                if let Some(bytes) = p.bytes() {
                    let mut old = from_pard(bytes, pdnm.as_deref(), data);
                    old.def.record = name;
                    out.insert(position, old);
                }
                position = position.saturating_add(1);
                i = j;
            }
            out
        }
    }

    fn bits(v: &[f64]) -> Vec<u64> {
        v.iter().map(|x| x.to_bits()).collect()
    }

    /// Every field equal, numbers bit for bit.
    fn assert_same(a: &HashMap<u16, reference::Old>, b: &HashMap<u16, ParDef>, what: &str) {
        let mut keys: Vec<&u16> = a.keys().collect();
        keys.sort_unstable();
        assert_eq!(keys.len(), b.len(), "{what}");
        for k in keys {
            let (old, y) = (&a[k], b.get(k).unwrap_or_else(|| panic!("{what}: {k}")));
            // The prototypes name no parameter and no popup item.
            assert!(old.name.is_empty() && old.items.is_empty(), "{what}: parameter {k}");
            let x = &old.def;
            let range = |r: Option<(f64, f64)>| r.map(|(lo, hi)| (lo.to_bits(), hi.to_bits()));
            assert_eq!(
                (&x.record, x.code, bits(&x.default), range(x.range), x.valid_max.map(f64::to_bits), &x.data),
                (&y.record, y.code, bits(&y.default), range(y.range), y.valid_max.map(f64::to_bits), &y.data),
                "{what}: parameter {k}"
            );
        }
    }

    #[test]
    fn the_table_definitions_equal_the_encoded_and_parsed_ones() {
        for fx in FX_DEFS {
            let sspc = crate::export::fxsynth::default_sspc(fx);
            // The decoder only tells a checkbox on from off: the table's words are 0 or 1.
            let part = sspc.child_list(b"parT").map(|p| p.children()).unwrap_or_default();
            for pard in part.iter().filter(|c| c.is(b"pard")).filter_map(|c| c.bytes()) {
                if crate::container::Pard(pard).kind() == crate::ParamKind::Checkbox {
                    let word = pard.get(56..60).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                    assert!(matches!(word, Some(0 | 1)), "{}: checkbox word {word:?}", fx.match_name);
                }
            }
            let parsed = reference::par_defs(&sspc);
            assert_same(&parsed, &ParDef::of_effect(fx), fx.match_name);
        }
    }

    #[test]
    fn a_record_name_that_is_not_ascii_is_found_in_its_prototype() {
        static PARAMS: [PDef; 1] = [PDef { id: 1, code: 2, default: &[0.0], range: None, valid_max: None, valid_min: None, display: 0 }];
        let fx = FxDef { match_name: "Effekt Größe", name: "x", params: &PARAMS };
        let record = ParDef::of_effect(&fx).get(&0).map(|d| d.record.clone()).expect("the parameter");
        assert_eq!(record, "Effekt Größe-0001");
        let sspc = crate::export::fxsynth::default_sspc(&fx);
        let part = sspc.child_list(b"parT").expect("parT");
        assert!(part.children().iter().any(|c| is_tdmn(c, &record)));
    }

    #[test]
    fn generated_definitions_equal_the_encoded_and_parsed_ones() {
        // Every parameter type with defaults, ranges and maxima that are fractional, negative,
        // huge, beyond the 16.16 and f32 ranges, and not numbers.
        let values = [0.0, -0.0, 1.0, -1.0, 0.5, 1.0 / 3.0, 1.01, 100.0, 255.4, 255.6, 256.0, -3.7, 65535.6, 70000.0, 1e9, -1e9, 1e40, f64::NAN, f64::INFINITY];
        let mut state = 7u64;
        let mut pick = move |n: usize| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(state >> 33).unwrap_or(0) % n
        };
        for round in 0..400 {
            let params: Vec<PDef> = (0..16u16)
                .map(|id| {
                    let code = [0u8, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15][pick(15)];
                    let default: &'static [f64] = Box::leak((0..pick(5)).map(|_| values[pick(values.len())]).collect::<Vec<_>>().into_boxed_slice());
                    let opt = |pick: &mut dyn FnMut(usize) -> usize| (pick(3) > 0).then(|| values[pick(values.len())]);
                    let range = opt(&mut pick).map(|lo| (lo, values[pick(values.len())]));
                    PDef { id, code, default, range, valid_max: opt(&mut pick), valid_min: opt(&mut pick), display: u32::try_from(pick(40)).unwrap_or(0) }
                })
                .collect();
            let fx =
                FxDef { match_name: Box::leak(format!("Generated {round}").into_boxed_str()), name: "Generated", params: Box::leak(params.into_boxed_slice()) };
            let parsed = reference::par_defs(&crate::export::fxsynth::default_sspc(&fx));
            assert_same(&parsed, &ParDef::of_effect(&fx), fx.match_name);
        }
    }

    fn group(effect: &str, text: (&str, &str)) -> PropGroup {
        let spec = effectcraft_effects::find(effect).expect("the effect");
        let mut next = 1;
        let mut g = effectcraft_effects::instantiate(spec, &mut effectcraft_project::build::Ids(&mut next), "fx", [100.0, 100.0]);
        g.prop_mut(text.0).expect("the text parameter").value = Value::Str(text.1.to_string());
        g
    }

    fn invalid(col: Collector) -> Vec<String> {
        col.finish().losses.into_iter().filter(|l| l.category == LossCategory::InvalidValues).flat_map(|l| l.details).collect()
    }

    #[test]
    fn unreadable_curve_points_are_reported() {
        let pdefs = defs().get("ADBE CurvesCustom").expect("the Curves prototype");
        let mut col = Collector::default();
        plan_curves(&group("ec.color.curves", ("rgb", "0,0 junk 0.5,0.7 nan,1 1,1")), pdefs, "fx", &mut col);
        let found = invalid(col);
        assert!(found.iter().any(|d| d.contains("curve 'rgb': 2 entries")), "{found:?}");
        let mut col = Collector::default();
        plan_curves(&group("ec.color.curves", ("rgb", "0,0 0.5,0.7 1,1")), pdefs, "fx", &mut col);
        assert_eq!(invalid(col), Vec::<String>::new());
    }

    #[test]
    fn unreadable_mesh_entries_are_reported() {
        let pdefs = defs().get("ADBE MESH WARP").expect("the Mesh Warp prototype");
        let mut col = Collector::default();
        plan_mesh(&group("ec.distort.meshwarp", ("mesh", "0,0 x,1 2")), pdefs, (100.0, 100.0), "fx", &mut col);
        let found = invalid(col);
        assert!(found.iter().any(|d| d.contains("2 entries that are not numbers")), "{found:?}");
        let mut col = Collector::default();
        plan_mesh(&group("ec.distort.meshwarp", ("mesh", "0,0 1,1")), pdefs, (100.0, 100.0), "fx", &mut col);
        assert_eq!(invalid(col), Vec::<String>::new());
    }
}
