//! Effects: the inverse of the importer, through the same table (`effectcraft_aep::effect_map`).
//! An EffectCraft effect the table maps is written as the After Effects effect it came from:
//!
//! * the instance is the effect's default instance, cloned from our own synthetic cases
//!   (`template/effects.aep`, `examples/mkfxprotos.rs`: `tdmn` + `sspc` with its full `parT` and
//!   the records After Effects always keeps: the input layer, buttons, group markers, hidden
//!   entries, arbitrary data), with its instance name and the owning layer's id in the input-layer
//!   record;
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

use effectcraft_aep::effect_map::{Conv, EffectMap, Extra, HUE_SATURATION_MASTER, LEVELS_RGB, by_effect, by_match_name, popup_key};
use effectcraft_aep::{AepFile, Body, CURVE_IDENTITY, Chunk, CurvesData, MeshData, ParamKind};
use effectcraft_keyframe::{Interp, Keyframe, Value};
use effectcraft_project::{GroupKind, Layer, LayerId, Node, ParamUi, PropGroup, Property};

use crate::plan::{CompPlan, EaseSlot, KeyPlan, LIMIT};
use crate::report::{Collector, LossCategory};
use crate::tree::{R, child_data, child_mut, data_mut, kids_mut, list_mut, put, put_f64, put_u8, put_u16, put_u32, tpl};

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
    /// `pard@16`, or a checkbox's `pdnm` label.
    pub name: String,
    /// The popup items (`pdnm`, separated by `|`).
    pub items: Vec<String>,
    /// The default in stored units (`pard@56`).
    pub default: Vec<f64>,
    /// The slider range a value record repeats as `tdum` / `tduM`.
    pub range: Option<(f64, f64)>,
    /// A slider's valid maximum (the table's scale check).
    pub valid_max: Option<f64>,
    /// The default arbitrary data (the `aRbp` after the `pard`).
    pub data: Option<Vec<u8>>,
}

fn be_i32(d: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn be_u32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?))
}
fn be_f32(d: &[u8], at: usize) -> Option<f64> {
    Some(f64::from(f32::from_be_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?)))
}
fn be_f64(d: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_be_bytes(d.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn cname(d: &[u8]) -> String {
    d.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect()
}

impl ParDef {
    fn from_pard(pard: &[u8], pdnm: Option<&str>, data: Option<&[u8]>) -> ParDef {
        let code = be_u32(pard, 12).unwrap_or(u32::MAX);
        let fx = |at: usize| be_i32(pard, at).map(|v| f64::from(v) / 65536.0);
        let default: Vec<f64> = match code {
            10 => be_f64(pard, 56).into_iter().collect(),
            2 | 3 => fx(56).into_iter().collect(),
            1 => be_i32(pard, 56).map(f64::from).into_iter().collect(),
            4 | 7 => be_u32(pard, 56).map(f64::from).into_iter().collect(),
            // Fractions of the layer, or percent after an After Effects re-save (see the reader).
            6 => {
                let p: Vec<f64> = [fx(56), fx(60)].into_iter().flatten().collect();
                if p.iter().any(|c| c.abs() > 1.0 + 1e-9) { p.iter().map(|c| c / 100.0).collect() } else { p }
            }
            5 => pard.get(56..60).map(|c| c.iter().map(|b| f64::from(*b)).collect()).unwrap_or_default(),
            _ => Vec::new(),
        };
        let range = match code {
            10 => be_f32(pard, 112).zip(be_f32(pard, 116)),
            2 => fx(132).zip(fx(136)),
            1 => be_i32(pard, 132).map(f64::from).zip(be_i32(pard, 136).map(f64::from)),
            _ => None,
        };
        let valid_max = match code {
            10 => be_f32(pard, 108),
            2 => fx(128),
            1 => be_i32(pard, 128).map(f64::from),
            _ => None,
        };
        let pname = pard.get(16..48).map(cname).unwrap_or_default();
        let (name, items) = match (code, pdnm) {
            (7, Some(p)) => (pname, p.split('|').map(str::to_string).collect()),
            (4, Some(p)) if pname.trim().is_empty() => (p.to_string(), Vec::new()),
            _ => (pname, Vec::new()),
        };
        ParDef { record: String::new(), code, name, items, default, range, valid_max, data: data.map(<[u8]>::to_vec) }
    }
}

/// The parameter definitions of an instance's `parT`, by position (the table's index).
fn par_defs(sspc: &Chunk<'_>) -> HashMap<u16, ParDef> {
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
            out.insert(position, ParDef { record: name, ..ParDef::from_pard(bytes, pdnm.as_deref(), data) });
        }
        position = position.saturating_add(1);
        i = j;
    }
    out
}

/// The parameter definitions of every prototype, by match name (from the embedded template; an
/// unreadable template gives none, and the export then fails in `Template::load`).
fn defs() -> &'static HashMap<String, HashMap<u16, ParDef>> {
    static DEFS: OnceLock<HashMap<String, HashMap<u16, ParDef>>> = OnceLock::new();
    DEFS.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(f) = effectcraft_aep::parse(crate::template::EFFECTS) else { return out };
        for c in &f.chunks {
            if c.is_list(b"fxdf") {
                let k = c.children();
                if let (Some(m), Some(s)) = (k.first(), k.get(1))
                    && let Some(name) = tdmn_name(m)
                {
                    out.insert(name, par_defs(s));
                }
            }
        }
        out
    })
}

// ---------------------------------------------------------------------------------------------
// Planning.

/// The stored components of an EffectCraft value, `None` when it does not fit.
fn stored(conv: Conv, kind: PKind, v: &Value, size: (f64, f64), ui: &ParamUi, def: &ParDef) -> Option<Vec<f64>> {
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
            let file: Vec<&str> = if def.items.is_empty() { items.to_vec() } else { def.items.iter().map(String::as_str).collect() };
            let at = file.iter().position(|f| f.trim() != "(-" && popup_key(f) == popup_key(name))?;
            vec![f64::from(u32::try_from(at).ok()?.checked_add(1)?)]
        }
        // Not a value record (layer parameters, histograms).
        Conv::LayerRef { .. } | Conv::Skip => return None,
    })
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
        if !g.enabled {
            col.lose(LossCategory::Effects, format!("{here}: the effect is switched off; written switched on (the switch is not decoded)"));
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
            if let Some(pp) = plan_param(pm.conv, kind, index, def, prop, size, cp, &here, col) {
                params.push(pp);
            }
        }
        let data = match spec.extra {
            Extra::Curves => plan_curves(g, pdefs, &here, col),
            Extra::MeshWarp => plan_mesh(g, pdefs, size, &here, col),
            _ => plan_extra(spec, g, pdefs, &here, col),
        };
        report_unmapped(g, spec, &here, col);
        col.exported.effects = col.exported.effects.saturating_add(1);
        out.push(EffectPlan { match_name: spec.match_name, name: crate::plan::cut_name(&g.name, 255, &here, col), params, data, layers });
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
        Extra::None | Extra::Curves | Extra::MeshWarp => return None,
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
            p.value.components().first().copied().and_then(sane)
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

/// `x,y` pairs (EffectCraft's curve points and mesh offsets; whitespace or `;` between pairs).
fn pairs_of(s: &str) -> Vec<(f64, f64)> {
    s.split(|c: char| c.is_whitespace() || c == ';')
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let (x, y) = t.split_once(',')?;
            let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().split(',').next()?.trim().parse::<f64>().ok()?);
            (x.is_finite() && y.is_finite()).then_some((x, y))
        })
        .collect()
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
        _ => None,
    }
}

/// Curves (FORMAT S.1): EffectCraft's points (0..1) as After Effects' whole 0..255 points, with
/// the lookup tables After Effects stores for them (the natural spline); `None` at the default.
fn plan_curves(g: &PropGroup, pdefs: &HashMap<u16, ParDef>, here: &str, col: &mut Collector) -> Option<(u16, Vec<u8>)> {
    let mut curves: [Vec<(u8, u8)>; 5] = Default::default();
    for (slot, id) in curves.iter_mut().zip(["rgb", "red", "green", "blue", "alpha"]) {
        let pts = static_str(g, id, here, col).map(pairs_of).unwrap_or_default();
        let mut ints: Vec<(u8, u8)> = Vec::with_capacity(pts.len());
        let mut rounded = false;
        for (x, y) in pts {
            let (xs, ys) = ((x * 255.0).clamp(0.0, 255.0), (y * 255.0).clamp(0.0, 255.0));
            rounded |= (xs - xs.round()).abs() > 1e-3 || (ys - ys.round()).abs() > 1e-3;
            ints.push((xs.round() as u8, ys.round() as u8));
        }
        ints.sort_by_key(|p| p.0);
        ints.dedup_by_key(|p| p.0);
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
    let offs: Vec<Vec<f64>> = static_str(g, "mesh", here, col)
        .map(|s| {
            s.split(|c: char| c.is_whitespace() || c == ';')
                .filter(|t| !t.is_empty())
                .filter_map(|t| {
                    let v: Vec<f64> = t.split(',').map(|x| x.trim().parse::<f64>().ok().filter(|x| x.is_finite())).collect::<Option<_>>()?;
                    (v.len() >= 2).then_some(v)
                })
                .collect()
        })
        .unwrap_or_default();
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
fn report_unmapped(g: &PropGroup, spec: &EffectMap, here: &str, col: &mut Collector) {
    let extra: &[&str] = match spec.extra {
        Extra::None => &[],
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
                    if spec.params.iter().any(|s| s.target == path) || extra.contains(&path.as_str()) {
                        continue;
                    }
                    if !p.keys.is_empty() || p.has_expression() {
                        col.lose(LossCategory::Effects, format!("{here}: parameter '{}' has no After Effects counterpart", p.name));
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_param(
    conv: Conv,
    kind: PKind,
    index: u16,
    def: &ParDef,
    prop: &Property,
    size: (f64, f64),
    cp: &CompPlan,
    here: &str,
    col: &mut Collector,
) -> Option<ParamPlan> {
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
    }
    if prop.keys.is_empty() {
        let Some(s) = stored(conv, kind, &prop.value, size, &prop.ui, def) else {
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
        let s = stored(conv, kind, first, size, &prop.ui, def)?;
        return (!is_default(kind, &s, def) || expr.is_some()).then_some(ParamPlan { index, kind, value: ParamValue::Static(s), expr });
    }
    let keys = plan_keys(conv, kind, def, prop, size, cp, &what, col);
    if keys.is_empty() {
        return None;
    }
    col.exported.keyframes = col.exported.keyframes.saturating_add(keys.len());
    Some(ParamPlan { index, kind, value: ParamValue::Keys(keys), expr })
}

#[allow(clippy::too_many_arguments)]
fn plan_keys(conv: Conv, kind: PKind, def: &ParDef, prop: &Property, size: (f64, f64), cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<KeyPlan> {
    let keys: &[Keyframe] = &prop.keys;
    let mut timed: Vec<(i64, &Keyframe)> = Vec::with_capacity(keys.len());
    for k in keys {
        let (u, inexact) = cp.tb.units(k.time);
        if inexact {
            col.lose(LossCategory::RoundedTimes, format!("{what}: key at {:.4} s moved to the time base", k.time.seconds()));
        }
        timed.push((u, k));
    }
    timed.sort_by_key(|(u, _)| *u);
    let before = timed.len();
    timed.dedup_by_key(|(u, _)| *u);
    if timed.len() != before {
        col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: keys at the same time were merged"));
    }
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
    for (u, k) in timed {
        let Ok(time) = i32::try_from(u) else {
            col.lose(LossCategory::InvalidValues, format!("{what}: key time out of range, key dropped"));
            continue;
        };
        let Some(value) = stored(conv, kind, &k.value, size, &prop.ui, def) else {
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
            let e = eases.first().copied().unwrap_or(effectcraft_keyframe::Ease { speed: 0.0, influence: 1.0 / 3.0 });
            let speed = sane(e.speed / factor).unwrap_or(0.0);
            let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
            vec![EaseSlot { speed, influence }]
        };
        let (mut tin, mut tout) = ([0.0; 3], [0.0; 3]);
        if kind == PKind::Point {
            for (t, s) in [(&mut tin, k.spatial_in), (&mut tout, k.spatial_out)] {
                t[0] = sane(s[0] / size.0).unwrap_or(0.0);
                t[1] = sane(s[1] / size.1).unwrap_or(0.0);
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
    out
}

// ---------------------------------------------------------------------------------------------
// Emit.

/// The effect prototypes (`template/effects.aep`).
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

pub(crate) fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let d = c.bytes()?;
    Some(d.iter().take_while(|b| **b != 0).map(|b| char::from(*b)).collect())
}

/// `(tdmn, node)` pairs of a list's children.
fn pairs(c: &Chunk<'static>) -> Vec<(String, Chunk<'static>)> {
    let k = c.children();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < k.len() {
        if let (Some(m), Some(n)) = (k.get(i), k.get(i + 1))
            && m.is(b"tdmn")
        {
            out.push((tdmn_name(m).unwrap_or_default(), n.clone()));
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

impl FxProtos {
    pub(crate) fn from_file(f: &AepFile<'static>) -> R<FxProtos> {
        let mut defaults = HashMap::new();
        let mut kinds = HashMap::new();
        let mut keyed = Vec::new();
        for c in &f.chunks {
            if c.is_list(b"fxdf") {
                let k = c.children();
                let (Some(m), Some(s), Some(d)) = (k.first(), k.get(1), k.get(2)) else { return Err(tpl("effect prototype incomplete")) };
                let name = tdmn_name(m).ok_or_else(|| tpl("effect prototype without a name"))?;
                defaults.insert(name, (m.clone(), s.clone(), d.clone()));
            } else if c.is_list(b"fxkd") {
                for (n, r) in pairs(c) {
                    let code = n.strip_prefix("kind-").and_then(|s| s.parse::<u32>().ok()).ok_or_else(|| tpl("unknown kind record"))?;
                    kinds.insert(code, r);
                }
            } else if c.is_list(b"fxky") {
                for (n, r) in pairs(c) {
                    let kind = match n.as_str() {
                        "ADBE Tint-0003" => KeyedKind::Fixed,
                        "ADBE Tint-0002" => KeyedKind::Color,
                        "ADBE Ramp-0001" => KeyedKind::Point,
                        "ADBE Exposure2-0003" => KeyedKind::Float,
                        _ => return Err(tpl("unknown keyed effect prototype")),
                    };
                    keyed.push((kind, r));
                }
            }
        }
        let all_types = effectcraft_aep::effect_map::EFFECTS.iter().all(|e| defaults.contains_key(e.match_name));
        if !all_types || kinds.len() != 8 || keyed.len() != 4 {
            return Err(tpl("effect prototypes missing"));
        }
        Ok(FxProtos { defaults, kinds, keyed })
    }
}

/// Which effect types already have an instance with full definitions in the file, in the order
/// they first appear.
#[derive(Default)]
pub(crate) struct FxUse {
    pub types: Vec<&'static str>,
}

/// A `tdsn` chunk holding `text`.
fn tdsn(text: &str) -> Chunk<'static> {
    nested(*b"tdsn", text)
}

/// A chunk that holds one `Utf8` chunk (`tdsn`, `fnam`).
fn nested(id: [u8; 4], text: &str) -> Chunk<'static> {
    Chunk { id, body: Body::Chunks(vec![Chunk::data(*b"Utf8", text.as_bytes().to_vec())]), pad: None }
}

/// The value record of `p`, built from the record of its parameter type.
pub(crate) fn value_record(protos: &FxProtos, p: &ParamPlan, def: &ParDef, source_size: (u32, u32)) -> R<(Chunk<'static>, Chunk<'static>)> {
    let mut rec = protos.kinds.get(&p.kind.code()).cloned().ok_or_else(|| tpl("effect value prototype missing"))?;
    {
        let kids = kids_mut(&mut rec)?;
        if let Some(slot) = kids.iter_mut().find(|c| c.is(b"tdsn")) {
            *slot = tdsn(&def.name);
        }
    }
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
            let n = u32::try_from(keys.len()).map_err(|_| tpl("too many keys"))?;
            let dims = p.kind.dims();
            let size = match kk {
                KeyedKind::Fixed | KeyedKind::Float => 8 + 40 * dims,
                KeyedKind::Color | KeyedKind::Point => 56 + 24 * dims,
            };
            {
                let lhd3 = child_data(&mut list, b"lhd3")?;
                put_u32(lhd3, 8, n)?;
                put_u32(lhd3, 12, n.div_ceil(4))?;
                put_u32(lhd3, 16, u32::try_from(size).map_err(|_| tpl("record size"))?)?;
                put_u32(lhd3, 28, n.div_ceil(4).saturating_mul(4))?;
            }
            let mut ldat = Vec::with_capacity(size * keys.len());
            for k in keys {
                ldat.extend(match kk {
                    KeyedKind::Fixed | KeyedKind::Float => crate::emit::plain_record(1, k, size)?,
                    KeyedKind::Color | KeyedKind::Point => spatial_record(kk, k, dims, size)?,
                });
            }
            {
                let lk = kids_mut(&mut list)?;
                match lk.iter_mut().find(|c| c.is(b"ldat")) {
                    Some(c) => *c = Chunk::data(*b"ldat", ldat),
                    None => lk.push(Chunk::data(*b"ldat", ldat)),
                }
            }
            let kids = kids_mut(&mut rec)?;
            let at = kids.iter().position(|c| c.is(b"cdat")).ok_or_else(|| tpl("value record without cdat"))?;
            if let Some(slot) = kids.get_mut(at) {
                *slot = list;
            }
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
    Ok((Chunk::data(*b"tdmn", tdmn_bytes(&def.record)), rec))
}

/// A key of a colour (flags `0x0001`, 210, 364) or point (flags `0x0007`, `u32@8` the automatic
/// tangent switch, 382) parameter: the spatial layout with the value at 56.
fn spatial_record(kk: KeyedKind, k: &KeyPlan, n: usize, size: usize) -> R<Vec<u8>> {
    let mut r = vec![0u8; size];
    put(&mut r, 0, &k.time.to_be_bytes())?;
    put_u8(&mut r, 4, k.in_interp)?;
    put_u8(&mut r, 5, k.out_interp)?;
    let base: u16 = if kk == KeyedKind::Point { 0x0007 } else { 0x0001 };
    put_u16(&mut r, 6, base | (u16::from(k.label) << 6))?;
    if kk == KeyedKind::Point {
        put_u32(&mut r, 8, u32::from(k.spatial_auto))?;
        // The temporal ease (598): speed in layer heights per second, influence a fraction.
        let (i, o) = (k.in_ease.first().copied().unwrap_or_default(), k.out_ease.first().copied().unwrap_or_default());
        put_f64(&mut r, 24, i.speed)?;
        put_f64(&mut r, 32, i.influence)?;
        put_f64(&mut r, 40, o.speed)?;
        put_f64(&mut r, 48, o.influence)?;
    }
    for j in 0..n {
        put_f64(&mut r, 56 + 8 * j, k.value.get(j).copied().unwrap_or(0.0))?;
        if kk == KeyedKind::Point {
            put_f64(&mut r, 56 + 8 * n + 8 * j, k.spatial_in.get(j).copied().unwrap_or(0.0))?;
            put_f64(&mut r, 56 + 16 * n + 8 * j, k.spatial_out.get(j).copied().unwrap_or(0.0))?;
        }
    }
    Ok(r)
}

/// A match name padded to the 40 bytes of `tdmn`.
fn tdmn_bytes(name: &str) -> Vec<u8> {
    let mut v = vec![0u8; 40];
    for (i, b) in name.bytes().take(39).enumerate() {
        if let Some(slot) = v.get_mut(i) {
            *slot = b;
        }
    }
    v
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
    let pdefs = par_defs(&sspc);
    // `fnam` keeps the effect's own name; a name of the user's goes into the value group's
    // `tdsn`, which holds the placeholder `-_0_/-` otherwise (610 against 609).
    let own = child_mut(&mut sspc, b"fnam")?.children().iter().find(|c| c.is(b"Utf8")).and_then(|c| c.bytes()).map(|b| String::from_utf8_lossy(b).to_string());
    if own.as_deref() != Some(fx.name.as_str()) && !fx.name.is_empty() {
        let values = list_mut(&mut sspc, b"tdgp")?;
        let slot = child_mut(values, b"tdsn")?;
        *slot = nested(*b"tdsn", &fx.name);
    }
    let mut records = Vec::with_capacity(fx.params.len());
    for p in &fx.params {
        let def = pdefs.get(&p.index).ok_or_else(|| tpl("parameter definition missing"))?;
        records.push((p.index, value_record(protos, p, def, source_size)?));
    }
    {
        let values = list_mut(&mut sspc, b"tdgp")?;
        let kids = kids_mut(values)?;
        // The input layer (`-0000`) names the layer that owns the effect (320).
        let tdpi_of = |kids: &mut Vec<Chunk<'static>>, record: &str| -> Option<usize> {
            kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(record)).map(|i| i + 1)
        };
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
        for (index, def) in &pdefs {
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
            let i = kids
                .iter()
                .position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(want.as_str()))
                .ok_or_else(|| tpl("arbitrary data record missing"))?;
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
            if let Some(i) = kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(want.as_str()))
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
    let mut kids = vec![Chunk::data(*b"tdsb", vec![0, 0, 0, 1]), tdsn("-_0_/-")];
    for fx in fxs {
        kids.extend(instance(protos, fx, layer_id, layer_ids, source_size, uses)?);
    }
    kids.push(Chunk::data(*b"tdmn", tdmn_bytes("ADBE Group End")));
    Ok(vec![Chunk::data(*b"tdmn", tdmn_bytes("ADBE Effect Parade")), Chunk::list(*b"tdgp", kids)])
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
