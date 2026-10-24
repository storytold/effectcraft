//! Layer styles (`ADBE Layer Styles`, docs `aep-format.md` 3.14) into EffectCraft's `layerStyles`
//! group, through the shared table [`effectcraft_aep::style_map`].
//!
//! A style is imported when its switch group is applied (`tdsb` 1). Every parameter starts at
//! After Effects' default (not EffectCraft's: Use Global Light is off there), then the stored
//! records, keys and expressions are applied. Global Light Angle / Altitude are one setting per
//! composition in EffectCraft: they land on the layer's Blending Options and the composition
//! adopts them afterwards ([`effectcraft_project::styles::sync_comp`]). Not imported, reported:
//! gradients (`GCst`, not decoded), Pattern Overlay, switch groups in the state 3 (not
//! sampled; state 0 is a style switched off, 988), keys on popups and
//! checkboxes, and styles on layers that take none here (adjustment layers, cameras, lights).

use effectcraft_aep::style_map::{self as sm, StyleConv, StyleParam};
use effectcraft_aep::{Layer as AepLayer, PropGroup as AepGroup, PropNode, Property as AepProp};
use effectcraft_keyframe::Value;
use effectcraft_project::build::Ids;
use effectcraft_project::styles::{self as st, GlobalLight, STYLE_BLEND_MODES};
use effectcraft_project::{Layer, Property as EcProp};

use crate::props::{Kind, import_expression, import_keys, sane};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// A stored style value as an EffectCraft value; `None` when it does not fit the conversion.
pub(crate) fn value(conv: StyleConv, c: &[f64]) -> Option<Value> {
    let g = |i: usize| c.get(i).copied().filter(|v| v.is_finite());
    let v = match conv {
        StyleConv::Number => Value::Scalar(g(0)?),
        StyleConv::Color => Value::Color([g(1)? / 255.0, g(2)? / 255.0, g(3)? / 255.0, 1.0]),
        StyleConv::Mode => {
            let name = sm::mode_name(g(0)?)?;
            let i = STYLE_BLEND_MODES.iter().position(|m| m.0 == name)?;
            Value::Enum(u32::try_from(i).ok()?)
        }
        StyleConv::Popup(map) => Value::Enum(sm::popup_item(map, g(0)?)?),
        StyleConv::Checkbox => Value::Bool(g(0)? != 0.0),
        StyleConv::Vec2 => Value::Vec2([g(0)?, g(1)?]),
        StyleConv::Gradient => return None,
    };
    sane(&v).then_some(v)
}

/// The key conversion of a style value, `None` where keys are not decoded.
fn key_kind(conv: StyleConv) -> Option<Kind> {
    match conv {
        StyleConv::Number => Some(Kind::Number),
        StyleConv::Color => Some(Kind::EffectColor),
        StyleConv::Vec2 => Some(Kind::Vec2),
        _ => None,
    }
}

/// One parameter: the default (After Effects'), then the record if there is one.
fn import_param(spec: &StyleParam, full: &str, node: Option<PropNode<'_, '_>>, prop: &mut EcProp, tm: &TimeMap, what: &str, col: &mut Collector) {
    if spec.conv == StyleConv::Gradient {
        if node.is_some() {
            col.lose(LossCategory::LayerStyles, format!("{what}: {full} (gradient colours are not decoded, EffectCraft's default gradient kept)"));
        }
        return;
    }
    if let Some(v) = value(spec.conv, spec.default) {
        prop.value = v;
    }
    let Some(node) = node else { return };
    let PropNode::Property(p) = node else {
        col.lose(LossCategory::LayerStyles, format!("{what}: {full} is not a property, default used"));
        return;
    };
    import_record(spec.conv, &p, full, prop, tm, what, col);
}

fn import_record(conv: StyleConv, p: &AepProp<'_, '_>, full: &str, prop: &mut EcProp, tm: &TimeMap, what: &str, col: &mut Collector) {
    if p.is_animated() {
        match key_kind(conv) {
            Some(kind) => import_keys(p, kind, what, full, tm, prop, col),
            None => col.lose(LossCategory::LayerStyles, format!("{what}: keys on {full} are not imported (not sampled), default used")),
        }
    } else if let Some(stored) = p.static_value() {
        match value(conv, &stored) {
            Some(v) => prop.value = v,
            None => col.lose(LossCategory::LayerStyles, format!("{what}: {full} holds a value that is not valid ({stored:?}), default used")),
        }
    }
    import_expression(p, prop, col);
}

/// Imports the layer styles of `layer` into `l`. `gl` is the composition's Global Light, mirrored
/// into the new Blending Options.
pub(crate) fn import_layer_styles(layer: &AepLayer<'_, '_>, l: &mut Layer, gl: &GlobalLight, ids: &mut Ids<'_>, tm: &TimeMap, what: &str, col: &mut Collector) {
    let Some(PropNode::Group(ls)) = layer.properties().and_then(|p| p.get(sm::GROUP)) else { return };
    let entries = ls.entries();
    let mut applied: Vec<(&'static sm::StyleMap, AepGroup<'_, '_>, bool)> = Vec::new();
    for e in &entries {
        let PropNode::Group(g) = e.node else { continue };
        let Some(prefix) = e.match_name.strip_suffix("/enabled") else { continue };
        let flags = g.flags();
        if flags == Some(sm::FLAGS_UNUSED) {
            continue;
        }
        // 1 applied, 0 applied but switched off (988); any other state (3) is not sampled.
        let on = match flags {
            Some(sm::FLAGS_ON) => true,
            Some(sm::FLAGS_OFF) => false,
            other => {
                col.lose(LossCategory::LayerStyles, format!("{what}: style {prefix} in state {} (not sampled), not imported", other.unwrap_or(0)));
                continue;
            }
        };
        match sm::by_ae(prefix) {
            Some(spec) => applied.push((spec, g, on)),
            None if prefix == sm::PATTERN_OVERLAY => col.lose(LossCategory::LayerStyles, format!("{what}: Pattern Overlay (no counterpart)")),
            None => col.lose(LossCategory::LayerStyles, format!("{what}: unknown layer style {prefix}")),
        }
    }
    if applied.is_empty() {
        return;
    }
    if !l.can_have_styles() {
        col.lose(LossCategory::LayerStyles, format!("{what}: this kind of layer takes no layer styles here ({} not imported)", applied.len()));
        return;
    }
    for (spec, g, on) in &applied {
        let Some(uid) = st::add_style(l, ids, spec.ec, gl, *on) else {
            col.lose(LossCategory::LayerStyles, format!("{what}: style {} could not be added", spec.ae));
            continue;
        };
        let Some(group) = l.props.find_group_mut(uid) else { continue };
        let records = g.entries();
        for prm in spec.params {
            let full = spec.param_name(prm);
            let node = records.iter().find(|r| r.match_name == full).map(|r| r.node);
            match group.get_mut(prm.ec) {
                Some(prop) => import_param(prm, &full, node, prop, tm, what, col),
                None => col.lose(LossCategory::LayerStyles, format!("{what}: {full} has no counterpart '{}'", prm.ec)),
            }
        }
        for r in &records {
            if r.match_name != "ADBE Group End" && !spec.params.iter().any(|prm| spec.param_name(prm) == r.match_name) {
                col.lose(LossCategory::LayerStyles, format!("{what}: {} is not imported (unknown parameter)", r.match_name));
            }
        }
        col.imported.layer_styles += 1;
    }
    import_blending(&ls, l, tm, what, col);
}

/// Blending Options: Global Light (adopted by the composition later) and Advanced Blending.
fn import_blending(ls: &AepGroup<'_, '_>, l: &mut Layer, tm: &TimeMap, what: &str, col: &mut Collector) {
    let Some(PropNode::Group(bo)) = ls.get(sm::BLEND_OPTIONS) else { return };
    let Some(target) = l.props.sub_mut(st::GROUP).and_then(|g| g.sub_mut(st::BLENDING)) else { return };
    for (ae, ec) in [(sm::GLOBAL_ANGLE, "globalLightAngle"), (sm::GLOBAL_ALTITUDE, "globalLightAltitude")] {
        if let (Some(PropNode::Property(p)), Some(prop)) = (bo.get(ae), target.get_mut(ec)) {
            import_record(StyleConv::Number, &p, ae, prop, tm, what, col);
        }
    }
    let Some(PropNode::Group(adv)) = bo.get(sm::ADV_BLEND) else { return };
    let Some(target) = target.sub_mut("advancedBlending") else { return };
    for prm in sm::ADVANCED_BLENDING {
        if let Some(prop) = target.get_mut(prm.ec) {
            import_param(prm, prm.ae, adv.get(prm.ae), prop, tm, what, col);
        }
    }
}
