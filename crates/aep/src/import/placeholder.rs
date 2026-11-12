//! Placeholders: an effect with no EffectCraft counterpart (a third-party plug-in, a pseudo effect
//! of expression controls) arrives as an effect group that renders nothing, as a missing plug-in
//! does. Its effect id is the After Effects match name, so Find Missing Effects lists it. It holds
//! the effect's parameters, flat and named as in After Effects, each with its value, keys and
//! expression, so expressions that read them (`effect("Settings")("Speed")`) keep working. It is
//! still reported, once per match name.
//!
//! The values keep After Effects' units, the ones its expressions read: sliders and angles as
//! stored, points in layer pixels, colours RGBA 0..1, checkboxes on / off, popups as EffectCraft's
//! 0-based items over the file's items in order (expressions read them 1-based, as After Effects
//! does). Layer parameters name their layer (their keys and expression are reported). Paths and
//! arbitrary data are not kept (reported when they hold something).
//!
//! The parameters placeholders walk, kept or not, and their popup items (by name length) are
//! bounded per import ([`MAX_COST`]): definitions can be shared by many instances, so a small file
//! could otherwise ask for very many properties. Once
//! the budget is spent, later effects without a counterpart are reported without a placeholder.

use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::{GroupKind, Node, ParamUi, PropGroup, Property};

use crate::import::effects::{LayerCtx, ParamOut, PendingLayerRef, carries_nothing, convert, import_layer_ref, param_label};
use crate::import::props::{Kind, import_expression, import_keys};
use crate::import::report::{Collector, LossCategory};
use crate::{Effect as AepEffect, EffectParam, ParamKind, ParamValue};

/// Most placeholder parameters (kept or not) and popup items one import walks.
pub(crate) const MAX_COST: usize = 200_000;

/// Most items read from one placeholder popup.
const MAX_POPUP_ITEMS: usize = 1024;

/// A popup item costs one more for each of these bytes in its name, so the names kept stay
/// bounded too.
const NAME_BYTES: usize = 16;

/// The placeholder of `fx`, with `params` (its parameter definitions) as properties.
pub(crate) fn placeholder(
    fx: &AepEffect<'_, '_>,
    params: &[EffectParam<'_, '_>],
    cx: &LayerCtx<'_>,
    shown: &str,
    ids: &mut Ids<'_>,
    refs: &mut Vec<PendingLayerRef>,
    col: &mut Collector,
) -> PropGroup {
    // The effect id is the match name; the group's match id is one segment of a property path
    // (a pseudo effect's `Pseudo/0.25` would be cut apart), so match paths find its parameters.
    let mut group = ids.group(&segment(fx.match_name()), shown);
    group.kind = GroupKind::Effect { effect: fx.match_name().to_string() };
    let here = format!("{}: '{}' effect '{shown}' ({})", cx.comp, cx.layer, fx.match_name());
    for p in params {
        // Every parameter walked counts, kept or not: a definition shared by many instances is
        // otherwise walked again for each one.
        if carries_nothing(p) {
            col.placeholder_cost = col.placeholder_cost.saturating_add(1).min(MAX_COST);
            continue;
        }
        if col.placeholder_cost >= MAX_COST {
            col.lose(LossCategory::Effects, format!("{here}: parameters past the import's {MAX_COST} placeholder parameters not kept"));
            break;
        }
        let label = param_label(p);
        let Some((kind, ui, default)) = shape(p, cx.size) else {
            col.placeholder_cost = col.placeholder_cost.saturating_add(1);
            if p.stored().is_some() {
                col.lose(LossCategory::Effects, format!("{here}: parameter {label} not kept (type {:?})", p.kind));
            }
            continue;
        };
        let (items, names) = match &ui {
            ParamUi::Popup { options } => (options.len(), options.iter().map(|o| 1 + o.len() / NAME_BYTES).fold(0, usize::saturating_add)),
            _ => (0, 0),
        };
        if p.kind == ParamKind::Popup && popup_len(p).is_some_and(|n| n > items) {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label}: the items past the first {MAX_POPUP_ITEMS} not kept"));
        }
        let cost = col.placeholder_cost.saturating_add(names).saturating_add(1);
        if cost > MAX_COST {
            // The budget is spent: nothing more is built, here or in later effects (definitions
            // shared by many instances would otherwise still be walked item by item).
            col.lose(
                LossCategory::Effects,
                format!("{here}: parameter {label} and the ones after it not kept (past the import's {MAX_COST} placeholder parameters)"),
            );
            col.placeholder_cost = MAX_COST;
            break;
        }
        col.placeholder_cost = cost;
        let mut prop = Property::new(ids.alloc(), &param_id(p), p.name.trim(), default).with_ui(ui);
        prop.spatial = p.kind == ParamKind::Point;
        prop.hold_only = matches!(p.kind, ParamKind::Checkbox | ParamKind::Popup | ParamKind::Layer);
        match kind {
            Some(kind) => {
                fill(p, &mut prop, kind, cx, (&here, &label), col);
                group.children.push(Node::Prop(prop));
            }
            // A layer parameter: the layer it names (set once every layer exists, by its uid).
            None => {
                let id = format!("@{}", prop.uid);
                group.children.push(Node::Prop(prop));
                import_layer_ref(p, &id, true, cx, (&here, &label), ParamOut { group: &mut group, refs: &mut *refs, col: &mut *col });
            }
        }
    }
    group
}

/// How parameter `p` is held on a layer `size` big: its unit conversion (`None` for a layer
/// parameter), its control and its value until one is read. `None` for a type that is not kept.
fn shape(p: &EffectParam<'_, '_>, size: (u32, u32)) -> Option<(Option<Kind>, ParamUi, Value)> {
    let (kind, ui, default) = match p.kind {
        ParamKind::IntSlider | ParamKind::FixedSlider | ParamKind::FloatSlider => (Kind::EffectNumber(1.0), slider_ui(p), Value::Scalar(0.0)),
        ParamKind::Angle => (Kind::EffectNumber(1.0), ParamUi::Angle, Value::Scalar(0.0)),
        ParamKind::Checkbox => (Kind::EffectBool, ParamUi::Checkbox, Value::Bool(false)),
        ParamKind::Color => (Kind::EffectColor, ParamUi::Color, Value::Color([0.0, 0.0, 0.0, 1.0])),
        ParamKind::Point => (Kind::EffectPoint(f64::from(size.0), f64::from(size.1)), ParamUi::Point, Value::Vec2([0.0, 0.0])),
        ParamKind::Layer => return Some((None, ParamUi::Layer, Value::Layer(None))),
        ParamKind::Popup => {
            // The file's items, separators (`(-`) included: After Effects counts them.
            let options: Vec<String> = match &p.items {
                Some(items) => items.iter().take(MAX_POPUP_ITEMS).cloned().collect(),
                None => (1..=popup_len(p)?.min(MAX_POPUP_ITEMS)).map(|i| format!("Item {i}")).collect(),
            };
            let n = u32::try_from(options.len()).ok().filter(|n| *n > 0)?;
            (Kind::EffectPopup(n), ParamUi::Popup { options }, Value::Enum(0))
        }
        _ => return None,
    };
    Some((Some(kind), ui, default))
}

/// A slider's control: a float slider's valid range and drag range when they are ranges (only a
/// float slider's are decoded), else a plain number.
fn slider_ui(p: &EffectParam<'_, '_>) -> ParamUi {
    match (p.valid_min(), p.valid_max(), p.slider_range()) {
        (Some(min), Some(max), Some((smin, smax))) if [min, max, smin, smax].iter().all(|v| v.is_finite()) && min <= max && smin <= smax => {
            ParamUi::Slider { min, max, slider_min: smin.clamp(min, max), slider_max: smax.clamp(min, max), decimals: 2 }
        }
        _ => ParamUi::Number,
    }
}

/// How many items a popup has: the file's names, else the count in its definition.
fn popup_len(p: &EffectParam<'_, '_>) -> Option<usize> {
    p.items.as_ref().map(Vec::len).or_else(|| p.popup_item_count().map(usize::from))
}

/// The property id of `p`: its match name when that is one segment of a property path (a
/// third-party effect's `CC Cylinder-0003`), else `param<index>` (a pseudo effect's
/// `Pseudo/0.25-0001`: `/` and `.` separate path segments). Expressions also find it by its name.
fn param_id(p: &EffectParam<'_, '_>) -> String {
    if !p.match_name.is_empty() && p.match_name.chars().all(plain) { p.match_name.clone() } else { format!("param{}", p.index) }
}

/// Whether `c` can stand in one segment of a property path (`/`, `.`, `#` and `@` mean more).
fn plain(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')
}

/// `name` as one segment of a property path: other characters become `_` (`Pseudo/0.25` is
/// `Pseudo_0_25`).
fn segment(name: &str) -> String {
    name.chars().map(|c| if plain(c) { c } else { '_' }).collect()
}

/// Sets `prop` from `p`: its expression, then its keys or its value (the record, else the `pard`
/// default). `names` is `(here, label)` for the report.
fn fill(p: &EffectParam<'_, '_>, prop: &mut Property, kind: Kind, cx: &LayerCtx<'_>, names: (&str, &str), col: &mut Collector) {
    let (here, label) = names;
    if let Some(stored) = p.stored() {
        import_expression(&stored, prop, col);
    }
    if p.is_animated() {
        let Some(stored) = p.stored() else { return };
        if kind == Kind::EffectBool {
            // Keys on a checkbox are not sampled by the corpus.
            col.lose(LossCategory::Effects, format!("{here}: the keyframes of parameter {label} are not kept (not decoded)"));
            return;
        }
        let before = prop.keys.len();
        import_keys(&stored, kind, cx.layer, &p.match_name, cx.tm, prop, col);
        if prop.keys.len() == before {
            col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} has no readable keyframe"));
        }
        return;
    }
    let value = match (p.stored(), p.recorded_value()) {
        (Some(_), Some(v)) => v,
        (Some(_), None) => {
            col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds an unreadable value"));
            return;
        }
        // A borrowed popup's default slot holds another copy's selection (docs/aep-format.md 3.7).
        (None, _) if p.kind == ParamKind::Popup && p.borrowed_definition => {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label}: the default item is not known (shared definition), the first item used"));
            return;
        }
        // A popup's default slot can hold 0, no item (After Effects' save of a project it opened).
        (None, _) => match p.pard_default() {
            Some(ParamValue::Index(0)) => {
                col.lose(LossCategory::Effects, format!("{here}: parameter {label}: the default item is not stored, the first item used"));
                return;
            }
            Some(v) => v,
            None => return,
        },
    };
    match (convert(value, kind), value) {
        (Some(v), _) => prop.value = v,
        // An item of the file that is past the ones kept (reported with the cut): the first item.
        (None, ParamValue::Index(i)) if usize::try_from(i).is_ok_and(|i| i > MAX_POPUP_ITEMS && popup_len(p).is_some_and(|n| i <= n)) => {
            col.lose(LossCategory::Effects, format!("{here}: parameter {label}: item {i} is past the items kept, the first item used"));
        }
        (None, _) => col.lose(LossCategory::DamagedData, format!("{here}: parameter {label} holds a value that is not valid ({value:?})")),
    }
}
