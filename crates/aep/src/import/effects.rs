//! Effects: a table maps an After Effects effect (its match name) and its parameters (by their
//! index in `parT`) to an EffectCraft effect, a parameter id and a unit conversion. Effects and
//! parameters outside the table are reported in [`LossCategory::Effects`] by match name, never
//! dropped silently.
//!
//! Every conversion is backed by the corpus (`docs/aep-format.md` 3.7, cases 356-450, 511-516):
//!
//! * colour: ARGB 0..255 in the file (alpha first), straight RGBA 0..1 here. Both are values in the
//!   project's working-space encoding (After Effects stores what its colour picker shows; EffectCraft
//!   hands parameter colours to the effect as stored, `effectcraft-render`'s colour pipeline), so
//!   the conversion is `/ 255` with no transfer curve. After Effects colour parameters
//!   have no editable alpha and Tint's defaults store alpha 0 (356); EffectCraft's colour
//!   parameters are opaque, so alpha is 1.
//! * point: a fraction of the layer (source) size in the file, layer pixels here (369, 374, 383).
//! * slider: each slider has its own stored scale, told by its valid maximum in `pard` (Amount to
//!   Tint is a percent with maximum 100, Blend With Original a 0..1 fraction with maximum 1, Ramp
//!   Scatter pixels with maximum 512, Drop Shadow Opacity and Glow Threshold 0..255, Glow's A & B
//!   Midpoint a 0..0.99 fraction: 361, 378-381, 422, 429, 438). The table names the maximum it
//!   expects; a file with another maximum is reported instead of guessed.
//! * popup: 1-based in the file over all items, separators `(-` included (410); mapped to
//!   EffectCraft's item **by name**, since the orders differ (Levels' clip popups are On, Off, ... in
//!   After Effects and Off, On, ... here). An item EffectCraft does not have is reported.
//! * float and integer sliders, angles: the stored number as it is (Exposure, Gaussian Blur 216,
//!   Fast Box Blur's Iterations 414, Drop Shadow's Direction 423).
//! * checkbox: stored 1.0 when on, 0.0 when off; without a record it has the `pard` default, which
//!   can be on (Repeat Edge Pixels, 412 / 417).
//!
//! A parameter without a value record has its default: the one in `pard` (sliders, points,
//! colours, checkboxes, popups), or for a popup whose definitions were borrowed from the project
//! copy (its default slot holds that copy's selection) the default After Effects reads back for a
//! fresh effect (Glow's Composite Original is Behind, 427). Keyframes (layer time) are converted
//! with the same units (363, 364, 382, 397). The implicit input-layer parameter, buttons (Swap
//! Colors), twirl-down group markers (Exposure) and the closing built-in entry carry nothing to
//! import. Hue/Saturation's master values and Levels' values are arbitrary data, decoded by the
//! reader (444-446, 512-516).

use crate::effect_map::{CRYPTOMATTE_OUTPUT, Conv, EFFECTS, EffectMap, Extra, HUE_SATURATION_MASTER, LEVELS_RGB, ParamMap, popup_key};
use crate::{
    CryptomatteData, CurvesData, DefinitionSource, Effect as AepEffect, EffectDefinitions, EffectParam, ExtractorChannels, HueSaturationData,
    Layer as AepLayer, LayerRef, LevelsData, MeshData, ParamKind, ParamValue,
};
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{Node, ParamUi, PropGroup};

use crate::import::props::{Kind, import_expression, import_keys};
use crate::import::report::{Collector, LossCategory};
use crate::import::time::TimeMap;

/// The match names of the effects the importer converts (the others are reported).
pub fn supported_effects() -> impl Iterator<Item = (&'static str, &'static str)> {
    EFFECTS.iter().map(|e| (e.match_name, e.effect))
}

/// Everything about the layer the effects need.
pub(crate) struct LayerCtx<'c> {
    pub comp: &'c str,
    pub layer: &'c str,
    /// The layer's id in the file and in the project (layer parameters).
    pub aep_id: u32,
    pub ec_id: effectcraft_project::LayerId,
    /// The layer's source size in pixels (effect points are fractions of it).
    pub size: (u32, u32),
    pub tm: &'c TimeMap,
}

/// Imports the effects of `layer` into `target` (the layer's `effects` group) in stack order and
/// reports the ones it cannot. `defs` is the file's effect definition index, built once per import.
pub(crate) fn import_effects<'f, 'a>(
    defs: &EffectDefinitions<'f, 'a>,
    layer: &AepLayer<'f, 'a>,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    target: &mut PropGroup,
    refs: &mut Vec<PendingLayerRef>,
    col: &mut Collector,
) {
    for fx in layer.effects() {
        // Puppet keeps its meshes and pins in nested groups, not in `parT` (its own step).
        if fx.match_name() == crate::import::puppet::MATCH_NAME {
            crate::import::puppet::import_puppet(defs, &fx, cx, ids, target, col);
            continue;
        }
        let shown = fx.display_name().unwrap_or_else(|| fx.match_name().to_string());
        let Some(map) = EFFECTS.iter().find(|m| m.match_name == fx.match_name()) else {
            col.lose_unmapped_effect(fx.match_name(), format!("{}: '{}' has the effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name()));
            continue;
        };
        let Some(spec) = effectcraft_effects::find(map.effect) else {
            col.lose_unmapped_effect(fx.match_name(), format!("{}: '{}' has the effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name()));
            continue;
        };
        if defs.source(&fx) == DefinitionSource::Missing {
            col.lose(
                LossCategory::Effects,
                format!("{}: '{}' has the effect '{shown}' ({}) without readable parameter definitions, not imported", cx.comp, cx.layer, fx.match_name()),
            );
            continue;
        }
        let size = [f64::from(cx.size.0), f64::from(cx.size.1)];
        let mut group = effectcraft_effects::instantiate(spec, ids, &shown, size);
        match fx.enabled() {
            Some(on) => group.enabled = on,
            None => col.lose(LossCategory::DamagedData, format!("{}: '{}': the fx switch of '{shown}' is unreadable, imported switched on", cx.comp, cx.layer)),
        }
        let params = defs.params(&fx);
        // Borrowed definitions (another instance's or the project copy) may not name every value
        // record this instance has: such a record has nothing to import into, so it is reported.
        if params.first().is_some_and(|p| p.borrowed_definition) {
            let defined: std::collections::HashSet<&str> = params.iter().map(|p| p.match_name.as_str()).collect();
            for name in fx.value_record_names().iter().filter(|n| !defined.contains(n.as_str())) {
                col.lose(
                    LossCategory::Effects,
                    format!(
                        "{}: '{}' effect '{shown}' ({}): the value record {name} has no parameter definition, not imported",
                        cx.comp,
                        cx.layer,
                        fx.match_name()
                    ),
                );
            }
        }
        if map.extra == Extra::Lumetri {
            crate::import::lumetri::import_lumetri(&fx, &params, cx, &shown, ParamOut { group: &mut group, refs: &mut *refs, col: &mut *col });
        } else if map.extra == Extra::Cryptomatte {
            import_cryptomatte(&fx, &params, cx, &shown, &mut group, col);
        } else {
            for p in &params {
                import_param(&fx, map, p, cx, &shown, ParamOut { group: &mut group, refs: &mut *refs, col: &mut *col });
            }
        }
        col.imported.effects = col.imported.effects.saturating_add(1);
        target.children.push(Node::Group(group));
    }
}

/// Parameters that never carry a value to import.
pub(crate) fn carries_nothing(p: &EffectParam<'_, '_>) -> bool {
    matches!(p.kind, ParamKind::Button | ParamKind::NoData | ParamKind::GroupStart | ParamKind::GroupEnd) || (p.index == 0 && p.kind == ParamKind::Layer)
}

/// A layer parameter that names another layer, set once every layer of the composition exists
/// (the layer can sit below the effect's layer).
pub(crate) struct PendingLayerRef {
    /// The layer that owns the effect.
    pub layer: effectcraft_project::LayerId,
    /// The parameter.
    pub prop: effectcraft_project::Uid,
    /// The file's id of the named layer.
    pub target: u32,
    /// Where it is, for the report.
    pub what: String,
}

/// Where a parameter is imported to: the effect's group, the layer parameters still to resolve
/// and the report.
pub(crate) struct ParamOut<'a> {
    pub group: &'a mut PropGroup,
    pub refs: &'a mut Vec<PendingLayerRef>,
    pub col: &'a mut Collector,
}

/// How a parameter is named in the report: its name and match name.
pub(crate) fn param_label(p: &EffectParam<'_, '_>) -> String {
    if p.name.trim().is_empty() { p.match_name.clone() } else { format!("{} ({})", p.name.trim(), p.match_name) }
}

/// Imports one parameter of `fx` into its EffectCraft property: its keyframes, the value in its
/// record or the `pard` default, with a loss for what cannot be read or mapped.
fn import_param(fx: &AepEffect<'_, '_>, map: &EffectMap, p: &EffectParam<'_, '_>, cx: &LayerCtx<'_>, shown: &str, out: ParamOut<'_>) {
    let ParamOut { group, refs, col } = out;
    if carries_nothing(p) {
        return;
    }
    let label = param_label(p);
    let here = format!("{}: '{}' effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name());
    if p.kind == ParamKind::ArbitraryData && map.extra != Extra::None {
        import_extra(fx, map.extra, p, &here, cx.size, group, col);
        return;
    }
    let Some(m) = map.params.iter().find(|m| m.index == p.index) else {
        // Not in the table: lost only when the file stores something for it. A layer parameter
        // set to no layer (its record always exists, Timewarp's Matte Layer 529) holds nothing.
        if p.stored().is_some() && !matches!(p.stored_value(), Some(ParamValue::Layer(0))) {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label} not imported"));
        }
        return;
    };
    import_mapped(p, m, cx, (&here, &label), ParamOut { group, refs, col });
}

/// Imports parameter `p` into the property `m` names, converted by `m.conv`. `names` is
/// `(here, label)`, the effect and the parameter for the report.
pub(crate) fn import_mapped(p: &EffectParam<'_, '_>, m: &ParamMap, cx: &LayerCtx<'_>, names: (&str, &str), out: ParamOut<'_>) {
    let (here, label) = names;
    let ParamOut { group, refs, col } = out;
    if p.kind != m.kind {
        col.lose(LossCategory::Effects, format!("{here}: parameter {label} has type {:?}, expected {:?}; not imported", p.kind, m.kind));
        return;
    }
    match m.conv {
        Conv::Skip => return,
        Conv::LayerRef { own } => {
            import_layer_ref(p, m.target, own, cx, (here, label), ParamOut { group, refs, col });
            return;
        }
        _ => {}
    }
    let (w, h) = (f64::from(cx.size.0), f64::from(cx.size.1));
    let checked = |valid_max: f64, col: &mut Collector| {
        // The scale comes from the file: a slider whose valid maximum is not the one the table
        // was verified with is not guessed at.
        let found = p.valid_max();
        // Within 1e-4 of it: older files store Glow's A & B Midpoint maximum 0.99 as 0.98999 where
        // After Effects 2026 writes 0.990005 (the 16.16 rounding differs); the scale is the same.
        let ok = found.is_some_and(|v| (v - valid_max).abs() <= valid_max.abs() * 1e-4);
        if !ok {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label} has the valid maximum {found:?}, expected {valid_max}; not imported"));
        }
        ok
    };
    // The popup's items: the file's, else After Effects' when the item count agrees.
    let items: Vec<String> = match (m.conv, &p.items) {
        (Conv::Popup { .. }, Some(items)) => items.clone(),
        (Conv::Popup { items, .. }, None) => {
            if p.popup_item_count().map(usize::from) != Some(items.len()) {
                col.lose(
                    LossCategory::Effects,
                    format!("{here}: parameter {label}: the file names no popup items and has {:?} of them, not imported", p.popup_item_count()),
                );
                return;
            }
            items.iter().map(|s| s.to_string()).collect()
        }
        _ => Vec::new(),
    };
    let kind = match m.conv {
        Conv::Color => Kind::EffectColor,
        Conv::Point => Kind::EffectPoint(w, h),
        Conv::Angle => Kind::EffectNumber(1.0),
        Conv::Popup { .. } => Kind::EffectPopup(u32::try_from(items.len()).unwrap_or(u32::MAX)),
        Conv::Checkbox => Kind::EffectBool,
        // Handled above.
        Conv::LayerRef { .. } | Conv::Skip => return,
        Conv::Percent { valid_max } => {
            if !checked(valid_max, col) {
                return;
            }
            Kind::EffectNumber(100.0 / valid_max)
        }
        Conv::Plain { valid_max } => {
            if !checked(valid_max, col) {
                return;
            }
            Kind::EffectNumber(1.0)
        }
        Conv::Factor { valid_max, factor } => {
            if !checked(valid_max, col) {
                return;
            }
            Kind::EffectNumber(factor)
        }
    };
    // Grouped parameters are addressed by path (`master/exposure`).
    let Some(prop) = group.prop_mut(m.target) else {
        col.lose(LossCategory::Effects, format!("{here}: parameter {label} has no counterpart '{}'", m.target));
        return;
    };
    // For a popup: file item (0-based over all items) -> EffectCraft item, by name.
    let popup_map = match m.conv {
        Conv::Popup { .. } => match popup_mapping(&items, &prop.ui) {
            Some(map) => Some(map),
            None => {
                col.lose(LossCategory::Effects, format!("{here}: parameter {label}: the popup items do not match EffectCraft's, not imported"));
                return;
            }
        },
        _ => None,
    };
    // The expression of an effect parameter is stored as on any property (534).
    if let Some(stored) = p.stored() {
        // The expression evaluates in the units After Effects shows (a Glow midpoint 0.25, a popup's
        // 1-based item), the property here holds the converted value (25, a 0-based item): with a
        // conversion the expression would read and write the wrong scale, so it is reported instead.
        let converted = match kind {
            Kind::EffectNumber(factor) => (factor - 1.0).abs() > f64::EPSILON,
            Kind::EffectPopup(_) => true,
            _ => false,
        };
        if converted && stored.expression().is_some() {
            col.lose(LossCategory::Effects, format!("{here}: the expression on parameter {label} is not imported (the value is converted here)"));
        } else {
            import_expression(&stored, prop, col);
        }
    }
    if p.is_animated() {
        if kind == Kind::EffectBool {
            // Keys on a checkbox are not sampled by the corpus.
            col.lose(LossCategory::Effects, format!("{here}: the keyframes of parameter {label} are not imported (not decoded), default used"));
            return;
        }
        if let Some(stored) = p.stored() {
            let before = prop.keys.len();
            import_keys(&stored, kind, cx.layer, &p.match_name, cx.tm, prop, col);
            if prop.keys.len() == before {
                col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} has no readable keyframe, default used"));
            }
            if let Some(map) = &popup_map {
                remap_popup_keys(prop, map, here, label, col);
            }
            report_beyond_limit(m.limit, prop.keys.iter().map(|k| &k.value), here, label, col);
        }
        return;
    }
    // A record in the default state (`tdsb` 3) still holds the value After Effects uses (1200,
    // 1400: every such record reads back as recorded).
    let value = match (p.stored(), p.recorded_value()) {
        (Some(_), Some(v)) => Some(v),
        (Some(_), None) => {
            col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds an unreadable value, default used"));
            return;
        }
        // Absent = default. A borrowed popup's default slot is not reliable (a selection writes
        // into it): After Effects' default for a fresh effect is used.
        (None, _) => match (p.kind, m.conv) {
            // After Effects' save of a project it opened can leave 0 in the slot (e2 re-save,
            // Luma Key's Key Type at its default): no selection, the fresh default.
            (ParamKind::Popup, Conv::Popup { default, .. }) => p.pard_default().filter(|v| *v != ParamValue::Index(0)).or(Some(ParamValue::Index(default))),
            _ => p.pard_default(),
        },
    };
    let Some(value) = value else { return };
    let converted = match (value, &popup_map) {
        (ParamValue::Index(i), Some(map)) => match usize::try_from(i).ok().and_then(|i| i.checked_sub(1)).and_then(|i| map.get(i).copied()) {
            Some(Some(ec)) => Some(Value::Enum(ec)),
            Some(None) => {
                let item = usize::try_from(i).ok().and_then(|i| i.checked_sub(1)).and_then(|i| items.get(i)).cloned().unwrap_or_default();
                col.lose(LossCategory::Effects, format!("{here}: parameter {label}: item '{item}' has no counterpart, default used"));
                return;
            }
            None => None,
        },
        _ => convert(value, kind),
    };
    match converted {
        Some(v) => {
            report_beyond_limit(m.limit, std::iter::once(&v), here, label, col);
            prop.value = v;
        }
        None => col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds a value that is not valid ({value:?}), default used")),
    }
}

/// Reports a value After Effects accepts but EffectCraft's effect limits (`ParamMap::limit`): it is
/// imported as it is, and the effect renders it as the nearest end of its range.
fn report_beyond_limit<'v>(limit: Option<(f64, f64)>, values: impl Iterator<Item = &'v Value>, here: &str, label: &str, col: &mut Collector) {
    let Some((lo, hi)) = limit else { return };
    let beyond: Vec<f64> = values.filter_map(|v| if let Value::Scalar(x) = v { Some(*x) } else { None }).filter(|x| *x < lo || *x > hi).collect();
    if let Some(x) = beyond.first() {
        col.lose(LossCategory::Effects, format!("{here}: parameter {label} is {x}, EffectCraft's effect uses {lo} to {hi} (rendered as the nearest of them)"));
    }
}

/// A layer parameter: None stays unset, the owning layer is itself (`own`) or unset, another layer
/// is resolved by [`resolve_layer_refs`] once the composition's layers exist.
/// `names` is `(here, label)`, the effect and the parameter for the report.
fn import_layer_ref(p: &EffectParam<'_, '_>, target: &str, own: bool, cx: &LayerCtx<'_>, names: (&str, &str), out: ParamOut<'_>) {
    let (here, label) = names;
    let ParamOut { group, refs, col } = out;
    // After Effects hands the named layer's source to the effect, without its masks and effects
    // (904 E24: Set Matte's luminance from a grey solid that carries a Gradient Ramp is the grey's,
    // 0.5 everywhere); EffectCraft's companion choice defaults to Effects & Masks. Where After
    // Effects stores another choice is not sampled.
    if let Some(src) = group.prop_mut(&effectcraft_effects::layer_source_id(target)) {
        src.value = Value::Enum(0);
    }
    let Some(prop) = group.prop_mut(target) else {
        col.lose(LossCategory::Effects, format!("{here}: parameter {label} has no counterpart '{target}'"));
        return;
    };
    if p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some()) {
        col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on the layer parameter {label} are not imported"));
    }
    // No record: the default, the owning layer (320).
    match p.layer_ref(cx.aep_id).unwrap_or(LayerRef::This) {
        // None: unset where unset means None (Layer Control), else the explicit None (fx27
        // `layer-none`: After Effects renders Set Matte None and self differently).
        LayerRef::Nothing => prop.value = Value::Layer(if own { None } else { Some(effectcraft_project::NO_LAYER) }),
        LayerRef::This => prop.value = Value::Layer(own.then_some(cx.ec_id.0)),
        LayerRef::Layer(id) => {
            prop.value = Value::Layer(None);
            refs.push(PendingLayerRef { layer: cx.ec_id, prop: prop.uid, target: id, what: format!("{here}: parameter {label}") });
        }
    }
}

/// Sets the layer parameters that name another layer of the composition (`ids`: file id ->
/// project layer); one whose layer was not imported stays unset and is reported.
pub(crate) fn resolve_layer_refs(
    comp: &mut effectcraft_project::Comp,
    refs: &[PendingLayerRef],
    ids: &std::collections::HashMap<u32, effectcraft_project::LayerId>,
    col: &mut Collector,
) {
    for r in refs {
        let Some(target) = ids.get(&r.target) else {
            col.lose(LossCategory::Effects, format!("{} names a layer that is not imported (id {}), left unset", r.what, r.target));
            continue;
        };
        if let Some(prop) = comp.layers.iter_mut().find(|l| l.id == r.layer).and_then(|l| l.props.find_mut(r.prop)) {
            prop.value = Value::Layer(Some(target.0));
        }
    }
}

/// The EffectCraft item of each file item (by name, ignoring case and separators); `None` when
/// the parameter here is not a popup.
fn popup_mapping(file_items: &[String], ui: &ParamUi) -> Option<Vec<Option<u32>>> {
    let ParamUi::Popup { options } = ui else { return None };
    let norm = popup_key;
    Some(
        file_items
            .iter()
            .map(|item| {
                if item.trim() == "(-" {
                    return None;
                }
                options.iter().position(|o| norm(o) == norm(item)).and_then(|i| u32::try_from(i).ok())
            })
            .collect(),
    )
}

/// Keys of a popup were imported as file indices (0-based); maps them to EffectCraft's items.
fn remap_popup_keys(prop: &mut effectcraft_project::Property, map: &[Option<u32>], here: &str, label: &str, col: &mut Collector) {
    let before = prop.keys.len();
    prop.keys.retain_mut(|k| match k.value {
        Value::Enum(i) => match usize::try_from(i).ok().and_then(|i| map.get(i).copied().flatten()) {
            Some(ec) => {
                k.value = Value::Enum(ec);
                true
            }
            None => false,
        },
        _ => true,
    });
    if prop.keys.len() != before {
        col.lose(LossCategory::Effects, format!("{here}: a keyframe of parameter {label} has an item without a counterpart, dropped"));
    }
    if let Some(first) = prop.keys.first() {
        prop.value = first.value.clone();
    }
}

/// Hue/Saturation's master values, Levels' RGB values: from the arbitrary data of `p`.
fn import_extra(fx: &AepEffect<'_, '_>, extra: Extra, p: &EffectParam<'_, '_>, here: &str, size: (u32, u32), group: &mut PropGroup, col: &mut Collector) {
    if p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some()) {
        col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on {} are not imported", p.name));
    }
    let Some(data) = fx.arbitrary_data(&p.match_name) else {
        // No record: every value is at its default. A Channel Info value record without its
        // aRbs/aRbp payload (every sampled record has it) is damaged: the payload may have chosen
        // other channels (`Z`), so the defaults are not what After Effects renders.
        if p.stored().is_some() && matches!(extra, Extra::Extractor | Extra::Identifier) {
            col.lose(
                LossCategory::DamagedData,
                format!("{here}: the Channel Info data is missing (a value record without its data), the default channels used"),
            );
        }
        return;
    };
    let mut set = |id: &str, v: f64| {
        if let Some(prop) = group.prop_mut(id)
            && v.is_finite()
        {
            prop.value = Value::Scalar(v);
        }
    };
    match extra {
        Extra::HueSaturation => match HueSaturationData::parse(data) {
            Some(d) => {
                for (id, v) in HUE_SATURATION_MASTER.iter().zip(d.master) {
                    set(id, f64::from(v));
                }
                if !d.ranges_are_default() {
                    col.lose(LossCategory::Effects, format!("{here}: the colour ranges (Reds ... Magentas) are not imported (not sampled)"));
                }
            }
            None => col.lose(LossCategory::DamagedData, format!("{here}: the Hue/Saturation data is unreadable, defaults used")),
        },
        Extra::Levels => match LevelsData::parse(data) {
            Some(d) => {
                for (id, v) in LEVELS_RGB.iter().zip(d.channels[0]) {
                    set(id, f64::from(v));
                }
                if d.channels[1..].iter().any(|c| *c != LevelsData::DEFAULT_CHANNEL) {
                    col.lose(LossCategory::Effects, format!("{here}: the per-channel values (Red, Green, Blue, Alpha) are not imported (not sampled)"));
                }
            }
            None => col.lose(LossCategory::DamagedData, format!("{here}: the Levels data is unreadable, defaults used")),
        },
        Extra::Curves => match CurvesData::parse(data) {
            Some(d) => {
                // EffectCraft's curves are `x,y` points in 0..1 (the stored inputs and outputs /
                // 255); EffectCraft interpolates them its own way (a renderer difference, the
                // stored tables are After Effects' natural spline).
                for (id, pts) in CURVE_IDS.iter().zip(&d.curves) {
                    if let Some(prop) = group.prop_mut(id) {
                        let s: Vec<String> = pts.iter().map(|(x, y)| format!("{},{}", fraction(*x), fraction(*y))).collect();
                        prop.value = Value::Str(s.join(" "));
                    }
                }
            }
            None => col.lose(LossCategory::DamagedData, format!("{here}: the Curves data is unreadable, defaults used")),
        },
        Extra::MeshWarp => match MeshData::parse(data) {
            Some(m) => import_mesh(&m, here, size, group, col),
            None => col.lose(LossCategory::DamagedData, format!("{here}: the Distortion Mesh is unreadable, defaults used")),
        },
        Extra::Extractor => match ExtractorChannels::parse(data).map(|c| (c.sources(), c)) {
            // Each output channel reads the footage's channel of the stored name, which
            // EffectCraft's EXtractoR finds by the same name (`Z`); After Effects' `(copy)` is the
            // input's own channel, EffectCraft's `(copy)` (not `A`, which EffectCraft reads from an
            // OpenEXR footage's alpha plane, losing what earlier effects did to it).
            Some((Ok(names), _)) => {
                for (id, name) in ["red", "green", "blue", "alpha"].into_iter().zip(names) {
                    if let Some(prop) = group.prop_mut(id) {
                        prop.value = Value::Str(name);
                    }
                }
            }
            Some((Err(k), c)) => {
                let names: Vec<&str> = c.channels.iter().map(|(_, n)| n.as_str()).collect();
                col.lose(
                    LossCategory::Effects,
                    format!(
                        "{here}: the channels {} are not imported (record {} is of an unknown kind or names no channel), R, G, B, A used",
                        names.join(", "),
                        k.saturating_add(1)
                    ),
                );
            }
            None => col.lose(LossCategory::Effects, format!("{here}: the Channel Info data is not in the sampled layout, R, G, B, A used")),
        },
        Extra::Identifier => match crate::identifier_channel_type(data) {
            // No channel chosen (the type's label): After Effects lists only the footage's integer
            // (UINT) channels and, with none chosen, leaves the layer unchanged, where EffectCraft
            // draws the footage's object ID channel.
            Some("Object ID") => {
                if let Some(prop) = group.prop_mut("channelType") {
                    prop.value = Value::Enum(0);
                }
                col.lose(
                    LossCategory::Effects,
                    format!("{here}: no ID channel is chosen (Channel Info), so After Effects leaves the layer unchanged; EffectCraft draws the footage's object ID channel"),
                );
            }
            // A chosen channel, by name. EffectCraft's IDentifier has no channel of its own: it
            // reads the footage's first `ObjectID` (`MaterialID`) channel, names compared without
            // regard to case, so a channel of that name is the one After Effects reads.
            Some(name) => match IDENTIFIER_OWN.iter().position(|own| own.eq_ignore_ascii_case(name.trim())) {
                Some(kind) => {
                    if let Some(prop) = group.prop_mut("channelType") {
                        prop.value = Value::Enum(u32::try_from(kind).unwrap_or(0));
                    }
                }
                None => col.lose(
                    LossCategory::Effects,
                    format!("{here}: the ID channel '{name}' (Channel Info) is not imported; EffectCraft reads the footage's object ID channel (ObjectID, object_id, objectId, id)"),
                ),
            },
            None => col.lose(LossCategory::Effects, format!("{here}: the Channel Info data is not in the sampled layout, the object ID channel used")),
        },
        Extra::None | Extra::Lumetri | Extra::Cryptomatte => {}
    }
}

/// The Cryptomatte plug-in (`Extra::Cryptomatte`, docs/aep-format.md 3.7): 1 Settings data
/// (the layer by name and the selection, both carried as text), 2 Output (EffectCraft's Display
/// and Matte Only), 3 Selection Mode (reported when on). Anything else stored is reported.
fn import_cryptomatte(fx: &AepEffect<'_, '_>, params: &[EffectParam<'_, '_>], cx: &LayerCtx<'_>, shown: &str, group: &mut PropGroup, col: &mut Collector) {
    let here = format!("{}: '{}' effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name());
    let set = |group: &mut PropGroup, id: &str, v: Value| {
        if let Some(prop) = group.prop_mut(id) {
            prop.value = v;
        }
    };
    // The plug-in's Output starts at Colors (EffectCraft's Display starts at Preview); an Output
    // that cannot be imported keeps it.
    let (display, matte_only) = CRYPTO_COLORS;
    set(group, "display", Value::Enum(display));
    set(group, "matteOnly", Value::Bool(matte_only));
    for p in params {
        if carries_nothing(p) {
            continue;
        }
        let label = if p.name.is_empty() { p.match_name.clone() } else { format!("{} ({})", p.name, p.match_name) };
        let moving = p.is_animated() || p.stored().is_some_and(|s| s.expression().is_some());
        match (p.index, p.kind) {
            (1, ParamKind::ArbitraryData) => {
                if moving {
                    col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on {label} are not imported"));
                }
                // No record: the definition's default, no layer and no selection. A record
                // without its data (every sampled record has it) is damaged.
                let Some(data) = fx.arbitrary_data(&p.match_name) else {
                    if p.stored().is_some() {
                        col.lose(LossCategory::DamagedData, format!("{here}: the Cryptomatte settings (layer, selection) have no data, none used"));
                    }
                    continue;
                };
                match CryptomatteData::parse(data) {
                    Some(d) => {
                        set(group, "layerName", Value::Str(d.layer));
                        set(group, "selection", Value::Str(d.selection));
                    }
                    None => col.lose(LossCategory::DamagedData, format!("{here}: the Cryptomatte settings (layer, selection) are unreadable, none used")),
                }
            }
            (2, ParamKind::Popup) => {
                if moving {
                    col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on {label} are not imported, Colors used"));
                    continue;
                }
                let (display, matte_only) = crypto_output(p, &here, &label, col);
                set(group, "display", Value::Enum(display));
                set(group, "matteOnly", Value::Bool(matte_only));
            }
            (3, ParamKind::Checkbox) => {
                if moving {
                    col.lose(LossCategory::Effects, format!("{here}: keyframes or an expression on {label} are not imported"));
                } else {
                    match selection_mode_on(p) {
                        // A picking switch of the plug-in; EffectCraft picks with its own tool.
                        Some(true) => col.lose(LossCategory::Effects, format!("{here}: parameter {label} is on and has no counterpart")),
                        Some(false) => {}
                        None => col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds an unreadable value, off used")),
                    }
                }
            }
            _ => {
                if p.stored().is_some() && !matches!(p.stored_value(), Some(ParamValue::Layer(0))) {
                    col.lose(LossCategory::Effects, format!("{here}: parameter {label} ({:?}) not imported", p.kind));
                }
            }
        }
    }
}

/// EffectCraft's (Display, Matte Only) for the plug-in's Output Colors, its default.
const CRYPTO_COLORS: (u32, bool) = (0, false);

/// Is Selection Mode on? The plug-in writes it, when on, as a record in the default state
/// (`tdsb` 3) holding 1.0, which [`EffectParam::stored_value`] does not read; After Effects
/// reads it back as on and renders it (1400 `crypto-01-set-all`, `crypto-02-chk-toggled`). So
/// the record's value counts whatever its state; no record is off (the `pard` default). `None`
/// when a record is there but its value cannot be read (never the definition's default then).
fn selection_mode_on(p: &EffectParam<'_, '_>) -> Option<bool> {
    match p.stored() {
        Some(s) => s.static_value().and_then(|v| v.first().copied()).map(|v| v != 0.0),
        None => Some(p.value() == Some(ParamValue::Bool(true))),
    }
}

/// Cryptomatte's Output as EffectCraft's (Display, Matte Only): Colors is Colors, Matted RGBA
/// is Matte (the layer keyed by the selection), Matte Only is Matte Only. Matted Colors (the
/// ID colours keyed by the selection) becomes Matte, the same matte over the layer's own
/// colours, and is reported; a value that cannot be read is reported and Colors used.
fn crypto_output(p: &EffectParam<'_, '_>, here: &str, label: &str, col: &mut Collector) -> (u32, bool) {
    let items: Vec<String> = match &p.items {
        Some(items) => items.clone(),
        None if p.popup_item_count().map(usize::from) == Some(CRYPTOMATTE_OUTPUT.len()) => CRYPTOMATTE_OUTPUT.iter().map(|s| s.to_string()).collect(),
        None => {
            col.lose(
                LossCategory::Effects,
                format!("{here}: parameter {label}: the file names no popup items and has {:?} of them, not imported, Colors used", p.popup_item_count()),
            );
            return CRYPTO_COLORS;
        }
    };
    let index = match (p.stored(), p.stored_value()) {
        (Some(_), Some(ParamValue::Index(i))) => i,
        (Some(_), _) => {
            col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds an unreadable value, Colors used"));
            return CRYPTO_COLORS;
        }
        // No record: the default. The plug-in's is Colors (1): the `pard` default of the
        // instance's own and of the project's shared definition, and what scripting reads from a
        // fresh effect (1400 `crypto-00-defaults`). An instance's own definition with another
        // default keeps it; a borrowed one has no reliable popup default.
        (None, _) => match p.pard_default() {
            Some(ParamValue::Index(i)) if i != 0 && !p.borrowed_definition => i,
            _ => return CRYPTO_COLORS,
        },
    };
    let item = usize::try_from(index).ok().and_then(|i| i.checked_sub(1)).and_then(|i| items.get(i)).map(String::as_str).unwrap_or_default();
    match popup_key(item).as_str() {
        "colors" => CRYPTO_COLORS,
        "matted rgba" => (1, false),
        "matte only" => (1, true),
        "matted colors" => {
            col.lose(
                LossCategory::Effects,
                format!("{here}: parameter {label}: item '{item}' ({index}) has no counterpart, imported as Matte (the layer's colours keyed by the selection, not the ID colours)"),
            );
            (1, false)
        }
        _ => {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label}: item '{item}' ({index}) has no counterpart, Colors used"));
            CRYPTO_COLORS
        }
    }
}

/// The channels EffectCraft's IDentifier reads first for its Channel Type Object ID and Material
/// ID (`AuxChannels::object_id`, `material_id`).
const IDENTIFIER_OWN: [&str; 2] = ["ObjectID", "MaterialID"];

/// Curves' EffectCraft parameters, in the stored order (RGB, Red, Green, Blue, Alpha).
const CURVE_IDS: [&str; 5] = ["rgb", "red", "green", "blue", "alpha"];

/// A stored 0..255 curve value as EffectCraft's fraction, as short as it is exact.
fn fraction(v: u8) -> String {
    let s = format!("{:.6}", f64::from(v) / 255.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The Distortion Mesh as EffectCraft's per-vertex offsets in layer pixels (row-major `dx,dy`).
/// The grid size comes from the data (rows first, `export-tests\mesh`); After Effects'
/// Bezier handles are reported when they are not the default ones (EffectCraft's mesh is
/// bilinear).
fn import_mesh(m: &MeshData, here: &str, size: (u32, u32), group: &mut PropGroup, col: &mut Collector) {
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let rows_stored = group.prop("rows").map(|p| p.value.as_f64().round());
    let cols_stored = group.prop("columns").map(|p| p.value.as_f64().round());
    if rows_stored != Some(f64::from(m.rows)) || cols_stored != Some(f64::from(m.columns)) {
        col.lose(LossCategory::Effects, format!("{here}: the Distortion Mesh is {} x {} cells, Rows / Columns say otherwise; not imported", m.columns, m.rows));
        return;
    }
    let mut offs = Vec::with_capacity(m.vertices.len());
    let mut handles_moved = false;
    for (k, v) in m.vertices.iter().enumerate() {
        let k = u32::try_from(k).unwrap_or(u32::MAX);
        let (i, j) = (k % (m.columns + 1), k / (m.columns + 1));
        let d = MeshData::default_vertex(m.columns, m.rows, i, j);
        let (dx, dy) = (f64::from(v[0][0] - d[0][0]), f64::from(v[0][1] - d[0][1]));
        // A dragged vertex carries its handles along (985): each handle's own move is what is
        // left after the vertex's (pixels; top, right, bottom, left).
        let mut own = [0.0f64; 8];
        for (n, (a, b)) in v.iter().zip(&d).skip(1).enumerate() {
            if *b == [0.0, 0.0] {
                continue;
            }
            let (hx, hy) = (f64::from(a[0] - b[0]) - dx, f64::from(a[1] - b[1]) - dy);
            if let Some(s) = own.get_mut(2 * n) {
                *s = hx * w;
            }
            if let Some(s) = own.get_mut(2 * n + 1) {
                *s = hy * h;
            }
        }
        if !(dx.is_finite() && dy.is_finite() && own.iter().all(|x| x.is_finite())) {
            col.lose(LossCategory::DamagedData, format!("{here}: the Distortion Mesh holds a number that is not valid, not imported"));
            return;
        }
        // EffectCraft's mesh reads the first two numbers of a vertex; moved handles follow as
        // eight more (kept for the export, not drawn).
        let mut token = format!("{},{}", px(dx * w), px(dy * h));
        if own.iter().any(|x| x.abs() > 0.01) {
            handles_moved = true;
            for x in own {
                token.push_str(&format!(",{}", px(x)));
            }
        }
        offs.push(token);
    }
    if handles_moved {
        col.lose(
            LossCategory::Effects,
            format!("{here}: Distortion Mesh Bezier handles moved on their own are not drawn (EffectCraft's mesh is bilinear; they are kept for the export)"),
        );
    }
    if let Some(p) = group.prop_mut("rows") {
        p.value = Value::Scalar(f64::from(m.rows));
    }
    if let Some(p) = group.prop_mut("columns") {
        p.value = Value::Scalar(f64::from(m.columns));
    }
    if let Some(p) = group.prop_mut("mesh") {
        p.value = Value::Str(offs.join(" "));
    }
}

/// A mesh offset in pixels: three decimals, and the float32 noise of an untouched vertex (below
/// 1/100 px, stored fractions of the layer) as 0.
fn px(v: f64) -> String {
    if v.abs() < 0.01 { "0".to_string() } else { format!("{v:.3}") }
}

/// A decoded value in EffectCraft's units, `None` when it does not fit the kind or is not a valid
/// number.
fn convert(value: ParamValue, kind: Kind) -> Option<Value> {
    let raw = match (value, kind) {
        // The reader gives colours as ARGB fractions; `Kind` takes the stored 0..255.
        (ParamValue::Argb(c), Kind::EffectColor) => c.map(|x| x * 255.0).to_vec(),
        (ParamValue::Point(p), Kind::EffectPoint(..)) => p.to_vec(),
        (ParamValue::Number(n), Kind::EffectNumber(_)) => vec![n],
        (ParamValue::Index(i), Kind::EffectPopup(_)) => vec![f64::from(i)],
        (ParamValue::Bool(b), Kind::EffectBool) => vec![if b { 1.0 } else { 0.0 }],
        _ => return None,
    };
    kind.value(&raw)
}
