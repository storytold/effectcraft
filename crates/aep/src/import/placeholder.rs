//! Effects without an EffectCraft counterpart (third-party plug-ins, pseudo effects): kept as
//! **placeholder effects** instead of being dropped (#3087), so expressions that read them
//! (`effect("Settings")("Menu")`) keep working and the stack shows what was applied.
//!
//! A placeholder is an effect group whose effect id is the After Effects match name. No effect is
//! registered under it, so it renders as nothing (as a missing plug-in does in After Effects), and
//! File ▸ Dependencies ▸ Find Missing Effects lists it (a pseudo effect too, although it is only
//! controls). The `.aep` export does not write placeholders back: it reports them. Its parameters
//! come from the file's definitions (`pard`), flat in stack order (After Effects' expressions name
//! a parameter inside a twirl-down group directly), each with the stored value or the
//! definition's default, its keyframes and its expression, in the units After Effects'
//! expressions read:
//!
//! * sliders and angles: the stored number; the slider range when the file's is usable;
//! * checkboxes: on / off;
//! * colours: RGB 0..1, opaque;
//! * points: layer pixels (stored as fractions of the layer size);
//! * popups: the item (an expression reads it 1-based, as in After Effects). The file's item
//!   names are kept; a separator (`(-`) stays an item, shown as a dash, because After Effects
//!   counts it in the numbering (docs/aep-format.md 3.7, 410);
//! * layers: the named layer of the composition (resolved with the other layer parameters). No
//!   record is taken as the owning layer, as for Layer Control (320); for other effects' layer
//!   parameters that default is an assumption (not sampled).
//!
//! A pseudo effect that is a Dropdown Menu Control (`Pseudo/@@…` with one popup) becomes
//! EffectCraft's Dropdown Menu Control with the file's items.
//!
//! Arbitrary data, paths and parameters of unknown type have nothing to hold them; a stored one is
//! reported. A third-party effect is reported (it is not rendered); a pseudo effect (`Pseudo/…`, an
//! effect made of expression controls with Pseudo Effect XML) renders nothing in After Effects
//! either, so when every stored parameter arrives it is no loss.
//!
//! A hostile file cannot make this large: at most [`MAX_PARAMS`] parameters per placeholder,
//! [`MAX_OPTIONS`] items per popup, and [`BUDGET`] properties and items over the whole import;
//! what is beyond is reported.

use crate::{DefinitionSource, Effect as AepEffect, EffectDefinitions, EffectParam, ParamKind};
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, Node, ParamUi, PropGroup, Property};

use crate::import::effects::{LayerCtx, ParamOut, PendingLayerRef, carries_nothing, convert, import_layer_ref};
use crate::import::props::{Kind, import_expression, import_keys};
use crate::import::report::{Collector, LossCategory};

/// The match name prefix of After Effects' pseudo effects.
const PSEUDO: &str = "Pseudo/";
/// The prefix of a Dropdown Menu Control (a pseudo effect named by a hash).
const DROPDOWN: &str = "Pseudo/@@";
/// The most parameters one placeholder holds.
pub(crate) const MAX_PARAMS: usize = 1000;
/// The most items one popup of a placeholder holds.
pub(crate) const MAX_OPTIONS: usize = 1024;
/// The most properties and popup items all placeholders of one import hold.
pub(crate) const BUDGET: usize = 200_000;
/// How a popup separator (`(-`) is shown.
const SEPARATOR: &str = "—";

/// The placeholder for `fx` (match name `fx.match_name()`, shown as `shown`), with the loss it
/// brings reported.
pub(crate) fn placeholder<'f, 'a>(
    defs: &EffectDefinitions<'f, 'a>,
    fx: &AepEffect<'f, 'a>,
    cx: &LayerCtx<'_>,
    shown: &str,
    ids: &mut Ids<'_>,
    refs: &mut Vec<PendingLayerRef>,
    col: &mut Collector,
) -> PropGroup {
    let match_name = fx.match_name();
    let here = format!("{}: '{}' has the effect '{shown}' ({match_name})", cx.comp, cx.layer);
    // A `/` (every pseudo effect's) would split a property path; the effect id keeps it.
    let mut group = ids.group(&match_name.replace('/', "_"), shown);
    group.kind = GroupKind::Effect { effect: match_name.to_string() };
    match fx.enabled() {
        Some(on) => group.enabled = on,
        None => col.lose(LossCategory::DamagedData, format!("{}: '{}': the fx switch of '{shown}' is unreadable, imported switched on", cx.comp, cx.layer)),
    }
    if defs.source(fx) == DefinitionSource::Missing {
        col.lose(LossCategory::Effects, format!("{here}, kept as a placeholder without parameters (no readable parameter definitions)"));
        return group;
    }
    if col.placeholder_cost >= BUDGET {
        col.lose(LossCategory::Effects, format!("{here}, kept as a placeholder without parameters (the import holds too many)"));
        return group;
    }
    let all = defs.params(fx);
    let mut lost = Vec::new();
    // Borrowed definitions (another instance's or the project copy) may not name every value
    // record this instance has: such a record (its value, keys, expression) has nothing to go
    // into, so it is reported, as for the mapped effects.
    if all.first().is_some_and(|p| p.borrowed_definition) {
        let defined: std::collections::HashSet<&str> = all.iter().map(|p| p.match_name.as_str()).collect();
        for name in fx.value_record_names().iter().filter(|n| !defined.contains(n.as_str())) {
            lost.push(format!("the value record {name} has no parameter definition, not kept"));
        }
    }
    let params: Vec<EffectParam<'_, '_>> = all.into_iter().filter(|p| !carries_nothing(p)).collect();
    // Reading the definitions copied every popup's item names: they count, kept or not.
    let names: usize = params.iter().filter_map(|p| p.items.as_ref()).map(Vec::len).fold(0, usize::saturating_add);
    col.placeholder_cost = col.placeholder_cost.saturating_add(names);
    for (n, p) in params.iter().enumerate() {
        if n >= MAX_PARAMS || col.placeholder_cost >= BUDGET {
            lost.push(format!("{} more parameters not kept (too many)", params.len() - n));
            break;
        }
        if let Some(why) = param(p, cx, ids, &mut group, refs, col) {
            lost.push(why);
        }
    }
    let pseudo = match_name.starts_with(PSEUDO);
    if !pseudo {
        col.lose(LossCategory::Effects, format!("{here}, kept as a placeholder with its parameters (not rendered)"));
    }
    for why in lost {
        col.lose(LossCategory::Effects, format!("{here}: {why}"));
    }
    if match_name.starts_with(DROPDOWN) {
        return dropdown(group, ids, cx);
    }
    group
}

/// A Dropdown Menu Control (one popup and nothing else) as EffectCraft's, with the popup the
/// placeholder holds (the file's items, value, keys and expression); else `group` unchanged.
fn dropdown(group: PropGroup, ids: &mut Ids<'_>, cx: &LayerCtx<'_>) -> PropGroup {
    let [Node::Prop(menu)] = group.children.as_slice() else { return group };
    if !matches!(menu.ui, ParamUi::Popup { .. }) {
        return group;
    }
    let Some(spec) = effectcraft_effects::find("ec.control.dropdown") else { return group };
    let mut native = effectcraft_effects::instantiate(spec, ids, &group.name, [f64::from(cx.size.0), f64::from(cx.size.1)]);
    native.enabled = group.enabled;
    let Some(target) = native.prop_mut("menu") else { return group };
    target.name = menu.name.clone();
    target.ui = menu.ui.clone();
    target.value = menu.value.clone();
    target.keys = menu.keys.clone();
    target.expr = menu.expr.clone();
    native
}

/// A usable slider range: every bound finite, the minimum not above the maximum (a NaN or a
/// reversed range would make the controls clamp with a panic and save as `null`).
fn slider_ui(valid: (Option<f64>, Option<f64>), slider: Option<(f64, f64)>) -> ParamUi {
    match (valid, slider) {
        ((Some(min), Some(max)), Some((smin, smax))) if [min, max, smin, smax].iter().all(|v| v.is_finite()) && min <= max && smin <= smax => {
            ParamUi::Slider { min, max, slider_min: smin.clamp(min, max), slider_max: smax.clamp(min, max), decimals: 2 }
        }
        _ => ParamUi::Number,
    }
}

/// A popup's items: the file's, else numbered ones, at most [`MAX_OPTIONS`]; separators shown as
/// a dash. The second value is how many items were left out.
fn popup_options(items: Option<&[String]>, count: u16) -> (Vec<String>, usize) {
    match items {
        Some(items) => {
            let kept = items.iter().take(MAX_OPTIONS).map(|i| if i.trim() == "(-" { SEPARATOR.to_string() } else { i.clone() }).collect();
            (kept, items.len().saturating_sub(MAX_OPTIONS))
        }
        None => {
            let count = usize::from(count);
            ((1..=count.min(MAX_OPTIONS)).map(|i| i.to_string()).collect(), count.saturating_sub(MAX_OPTIONS))
        }
    }
}

/// Adds parameter `p` to `group`; what of it is not kept, for the report.
fn param(
    p: &EffectParam<'_, '_>,
    cx: &LayerCtx<'_>,
    ids: &mut Ids<'_>,
    group: &mut PropGroup,
    refs: &mut Vec<PendingLayerRef>,
    col: &mut Collector,
) -> Option<String> {
    let name = if p.name.is_empty() { p.match_name.clone() } else { p.name.clone() };
    let label = format!("parameter {name} ({})", p.match_name);
    // The parameter's position in the definitions: unique, and free of the `/` a pseudo
    // effect's match names hold (expressions find it by its name).
    let id = format!("param{}", p.index);
    let (w, h) = (f64::from(cx.size.0), f64::from(cx.size.1));
    col.placeholder_cost = col.placeholder_cost.saturating_add(1);
    let mut why = None;
    let (kind, ui, initial) = match p.kind {
        ParamKind::Layer => {
            let mut prop = Property::new(ids.alloc(), &id, &name, Value::Layer(None)).with_ui(ParamUi::Layer);
            prop.hold_only = true;
            group.children.push(prop.into());
            let here = format!("{}: '{}'", cx.comp, cx.layer);
            import_layer_ref(p, &id, true, cx, (&here, &label), ParamOut { group, refs, col });
            return None;
        }
        ParamKind::IntSlider | ParamKind::FixedSlider | ParamKind::FloatSlider => {
            (Kind::EffectNumber(1.0), slider_ui((p.valid_min(), p.valid_max()), p.slider_range()), Value::Scalar(0.0))
        }
        ParamKind::Angle => (Kind::EffectNumber(1.0), ParamUi::Angle, Value::Scalar(0.0)),
        ParamKind::Checkbox => (Kind::EffectBool, ParamUi::Checkbox, Value::Bool(false)),
        ParamKind::Color => (Kind::EffectColor, ParamUi::Color, Value::Color([1.0, 1.0, 1.0, 1.0])),
        ParamKind::Point => (Kind::EffectPoint(w, h), ParamUi::Point, Value::Vec2([w / 2.0, h / 2.0])),
        ParamKind::Popup => {
            let (options, left_out) = popup_options(p.items.as_deref(), p.popup_item_count().unwrap_or(0));
            if p.items.is_none() {
                col.placeholder_cost = col.placeholder_cost.saturating_add(options.len());
            }
            if left_out > 0 {
                why = Some(format!("{label}: {left_out} popup items beyond {MAX_OPTIONS} not kept"));
            }
            let n = u32::try_from(options.len()).unwrap_or(u32::MAX);
            (Kind::EffectPopup(n), ParamUi::Popup { options }, Value::Enum(0))
        }
        _ => {
            // Arbitrary data, a path or a type of unknown meaning: nothing holds it.
            return (p.stored().is_some()).then(|| format!("{label} ({:?}) not kept", p.kind));
        }
    };
    let mut prop = Property::new(ids.alloc(), &id, &name, initial).with_ui(ui);
    prop.spatial = p.kind == ParamKind::Point;
    prop.hold_only = matches!(p.kind, ParamKind::Checkbox | ParamKind::Popup);
    if p.is_animated() && kind != Kind::EffectBool {
        if let Some(stored) = p.stored() {
            import_keys(&stored, kind, cx.layer, &p.match_name, cx.tm, &mut prop, col);
        }
        if prop.keys.is_empty() {
            why = Some(format!("{label} has no readable keyframe, default used"));
        }
    } else {
        if p.is_animated() {
            // Keys on a checkbox are not sampled (as for the mapped effects).
            why = Some(format!("the keyframes of {label} are not kept (not decoded), default used"));
        }
        // A record in the default state still holds the value After Effects uses (as for the
        // mapped effects, 1200 and 1400); no record is the definition's default.
        let value = if p.stored().is_some() && !p.is_animated() { p.recorded_value() } else { p.pard_default() };
        match value.and_then(|v| convert(v, kind)) {
            Some(v) => prop.value = v,
            None if p.stored().is_some() && !p.is_animated() => why = Some(format!("{label} holds an unreadable value, default used")),
            // A borrowed popup's default slot holds the other copy's selection, not the default
            // (`EffectParam::pard_default`); this effect's default item is not known here.
            None if p.kind == ParamKind::Popup && p.borrowed_definition => {
                why = Some(format!("{label}: the default item is not known (borrowed definition), the first item used"));
            }
            None => {}
        }
    }
    if let Some(stored) = p.stored() {
        import_expression(&stored, &mut prop, col);
    }
    group.children.push(prop.into());
    why
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A range that is not finite or reversed is no slider (it would panic in the controls and
    /// save as `null`).
    #[test]
    fn only_usable_slider_ranges_are_kept() {
        assert!(matches!(slider_ui((Some(0.0), Some(100.0)), Some((0.0, 50.0))), ParamUi::Slider { .. }));
        for bad in [
            ((Some(f64::NAN), Some(100.0)), Some((0.0, 50.0))),
            ((Some(0.0), Some(f64::INFINITY)), Some((0.0, 50.0))),
            ((Some(100.0), Some(0.0)), Some((0.0, 50.0))),
            ((Some(0.0), Some(100.0)), Some((50.0, 0.0))),
            ((Some(0.0), Some(100.0)), Some((f64::NAN, 1.0))),
            ((None, Some(100.0)), Some((0.0, 1.0))),
        ] {
            assert_eq!(slider_ui(bad.0, bad.1), ParamUi::Number, "{bad:?}");
        }
    }

    /// A popup without item names cannot make 65 535 strings; separators keep their place.
    #[test]
    fn popup_items_are_bounded() {
        let (o, left) = popup_options(None, u16::MAX);
        assert_eq!((o.len(), left), (MAX_OPTIONS, usize::from(u16::MAX) - MAX_OPTIONS));
        let items: Vec<String> = ["A", "(-", "B"].iter().map(|s| s.to_string()).collect();
        assert_eq!(popup_options(Some(&items), 3), (vec!["A".to_string(), SEPARATOR.into(), "B".into()], 0));
    }
}
