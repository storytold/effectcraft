//! Text animators: the animator properties and range selectors of a text layer, into
//! EffectCraft's animators (`text/animators`).
//!
//! Every value is stored as After Effects shows it (579-593): positions, anchor points, stroke
//! widths and line spacing in pixels, scales, opacity, Start / End / Offset, Amount, Smoothness and
//! the eases in percent, angles in degrees, tracking in 1/1000 em, colours as ARGB 0..255, popups
//! 1-based (Units 583, Based On 584, Mode 585, Shape 587, in EffectCraft's item order). A property
//! without a record has After Effects' default. With Units = Index (583) the selector's
//! `Index Start / End / Offset` hold the range in characters; EffectCraft's Start / End / Offset are
//! characters then too.
//!
//! Not imported, and reported in [`LossCategory::TextLayers`]: wiggly and expression selectors,
//! animator properties other than the ones mapped below (Character Value, Fill / Stroke
//! Hue, Saturation, Brightness and Opacity, 3D rotations, variable font axes, ...), and a Tracking
//! Type other than the default.

use effectcraft_aep::{PropGroup as AepGroup, PropNode, Property as AepProp};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Node, PropGroup, Property};

use crate::props::{Kind, import_expression, import_keys};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// More animators or selectors than this on one layer are not imported (and reported).
const MAX_ITEMS: usize = 1024;

/// The animator properties: stored match name, EffectCraft property kind (the `Animate` entry
/// that creates it), property id, value conversion.
const PROPS: &[(&str, &str, &str, Kind)] = &[
    ("ADBE Text Anchor Point 3D", "anchor", "anchor", Kind::Position),
    ("ADBE Text Position 3D", "position", "position", Kind::Position),
    ("ADBE Text Scale 3D", "scale", "scale", Kind::Vec3Percent),
    ("ADBE Text Skew", "skew", "skew", Kind::Number),
    ("ADBE Text Skew Axis", "skew", "skewAxis", Kind::Number),
    ("ADBE Text Rotation", "rotation", "rotation", Kind::Rotation),
    ("ADBE Text Opacity", "opacity", "opacity", Kind::Number),
    ("ADBE Text Fill Color", "fillColor", "fillColor", Kind::EffectColor),
    ("ADBE Text Stroke Color", "strokeColor", "strokeColor", Kind::EffectColor),
    ("ADBE Text Stroke Width", "strokeWidth", "strokeWidth", Kind::Number),
    ("ADBE Text Tracking Amount", "tracking", "tracking", Kind::Number),
    ("ADBE Text Line Spacing", "lineSpacing", "lineSpacing", Kind::Vec2),
    ("ADBE Text Blur", "blur", "blur", Kind::Vec2),
    // Character Offset (728): After Effects adds Character Range and Character Alignment with it.
    ("ADBE Text Character Offset", "characterOffset", "characterOffset", Kind::Number),
    ("ADBE Text Character Range", "characterOffset", "characterRange", Kind::EffectPopup(2)),
];

/// Range selector properties: stored match name, property id, conversion. `Index …` replace the
/// percentages when Units is Index.
const RANGE: &[(&str, &str, Kind)] =
    &[("ADBE Text Percent Start", "start", Kind::Number), ("ADBE Text Percent End", "end", Kind::Number), ("ADBE Text Percent Offset", "offset", Kind::Number)];
const RANGE_INDEX: &[(&str, &str, Kind)] =
    &[("ADBE Text Index Start", "start", Kind::Number), ("ADBE Text Index End", "end", Kind::Number), ("ADBE Text Index Offset", "offset", Kind::Number)];
const ADVANCED: &[(&str, &str, Kind)] = &[
    ("ADBE Text Range Units", "units", Kind::EffectPopup(2)),
    ("ADBE Text Range Type2", "basedOn", Kind::EffectPopup(4)),
    ("ADBE Text Selector Mode", "mode", Kind::EffectPopup(6)),
    ("ADBE Text Selector Max Amount", "amount", Kind::Number),
    ("ADBE Text Range Shape", "shape", Kind::EffectPopup(6)),
    ("ADBE Text Selector Smoothness", "smoothness", Kind::Number),
    ("ADBE Text Levels Max Ease", "easeHigh", Kind::Number),
    ("ADBE Text Levels Min Ease", "easeLow", Kind::Number),
    ("ADBE Text Randomize Order", "randomize", Kind::EffectBool),
    ("ADBE Text Random Seed", "randomSeed", Kind::Number),
];

struct Cx<'c, 'i> {
    what: &'c str,
    tm: &'c TimeMap,
    ids: &'c mut Ids<'i>,
    col: &'c mut Collector,
}

/// Imports the animators of `text` (the layer's `ADBE Text Animators` group) into `target` (its
/// `text/animators` group).
pub(crate) fn import_animators(animators: &AepGroup<'_, '_>, what: &str, tm: &TimeMap, ids: &mut Ids<'_>, target: &mut PropGroup, col: &mut Collector) {
    let mut cx = Cx { what, tm, ids, col };
    for (i, e) in animators.entries().iter().enumerate() {
        if i >= MAX_ITEMS {
            cx.col.lose(LossCategory::TextLayers, format!("{what}: more than {MAX_ITEMS} text animators, the rest not imported"));
            break;
        }
        match e.node {
            PropNode::Group(g) if e.match_name == "ADBE Text Animator" => {
                if let Some(a) = animator(&g, i, &mut cx) {
                    target.children.push(Node::Group(a));
                    cx.col.imported.text_animators = cx.col.imported.text_animators.saturating_add(1);
                }
            }
            _ => cx.col.lose(LossCategory::TextLayers, format!("{what}: text animator entry {} not imported", e.match_name)),
        }
    }
}

fn animator(g: &AepGroup<'_, '_>, index: usize, cx: &mut Cx<'_, '_>) -> Option<PropGroup> {
    let name = g.user_name().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Animator {}", index.saturating_add(1)));
    let here = format!("{}: animator '{name}'", cx.what);
    // The properties, created in the order of their `Animate` entries.
    let mut kinds: Vec<&str> = Vec::new();
    let aep_props = match g.get("ADBE Text Animator Properties") {
        Some(PropNode::Group(p)) => Some(p),
        _ => None,
    };
    if let Some(p) = &aep_props {
        for e in p.entries() {
            match PROPS.iter().find(|(m, ..)| *m == e.match_name) {
                Some((_, kind, ..)) if !kinds.contains(kind) => kinds.push(kind),
                Some(_) => {}
                None if e.match_name == "ADBE Text Track Type" || e.match_name == "ADBE Text Character Change Type" => {}
                None => cx.col.lose(LossCategory::TextLayers, format!("{here}: property {} not imported", e.match_name)),
            }
        }
    }
    let props: Vec<Property> = kinds.iter().flat_map(|k| build::text_anim_props(cx.ids, k, false)).collect();
    let mut a = build::text_animator(cx.ids, &name, props);
    if let (Some(p), Some(target)) = (&aep_props, a.sub_mut("properties")) {
        for (m, _, id, kind) in PROPS {
            set(target, id, p, m, *kind, &here, cx);
        }
        // Tracking Type: only the default (Before & After, 1) is sampled.
        if let Some(PropNode::Property(t)) = p.get("ADBE Text Track Type")
            && (t.is_animated() || t.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v != 1.0))
        {
            cx.col.lose(LossCategory::TextLayers, format!("{here}: Tracking Type is not imported, Before & After used"));
        }
        // Character Alignment: only the value After Effects adds with Character Offset (1, 728) is
        // sampled; it stays EffectCraft's default.
        if let Some(PropNode::Property(t)) = p.get("ADBE Text Character Change Type")
            && (t.is_animated() || t.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v != 1.0))
        {
            cx.col.lose(LossCategory::TextLayers, format!("{here}: Character Alignment is not imported, the default used"));
        }
    }
    // The selectors replace the default one.
    if let Some(sels) = a.sub_mut("selectors") {
        sels.children.clear();
        if let Some(PropNode::Group(s)) = g.get("ADBE Text Selectors") {
            for (i, e) in s.entries().iter().enumerate() {
                if i >= MAX_ITEMS {
                    cx.col.lose(LossCategory::TextLayers, format!("{here}: more than {MAX_ITEMS} selectors, the rest not imported"));
                    break;
                }
                match e.node {
                    PropNode::Group(sg) if e.match_name == "ADBE Text Selector" => {
                        let r = range_selector(&sg, i, &here, cx);
                        sels.children.push(Node::Group(r));
                    }
                    _ => cx.col.lose(LossCategory::TextLayers, format!("{here}: selector {} not imported", e.match_name)),
                }
            }
        }
    }
    Some(a)
}

fn range_selector(g: &AepGroup<'_, '_>, index: usize, animator: &str, cx: &mut Cx<'_, '_>) -> PropGroup {
    let name = g.user_name().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Range Selector {}", index.saturating_add(1)));
    let here = format!("{animator} '{name}'");
    let mut r = build::range_selector(cx.ids, &name);
    let adv = match g.get("ADBE Text Range Advanced") {
        Some(PropNode::Group(a)) => Some(a),
        _ => None,
    };
    let index_units = adv
        .as_ref()
        .and_then(|a| match a.get("ADBE Text Range Units") {
            Some(PropNode::Property(p)) => p.static_value(),
            _ => None,
        })
        .and_then(|v| v.first().copied())
        == Some(2.0);
    for (m, id, kind) in if index_units { RANGE_INDEX } else { RANGE } {
        set(&mut r, id, g, m, *kind, &here, cx);
    }
    if let (Some(a), Some(target)) = (&adv, r.sub_mut("advanced")) {
        for (m, id, kind) in ADVANCED {
            set(target, id, a, m, *kind, &here, cx);
        }
    }
    r
}

/// Sets property `id` of `target` from the stored property `match_name` of `g`: its keys, or its
/// static value, and its expression.
fn set(target: &mut PropGroup, id: &str, g: &AepGroup<'_, '_>, match_name: &str, kind: Kind, here: &str, cx: &mut Cx<'_, '_>) {
    let Some(PropNode::Property(node)) = g.get(match_name) else { return };
    let Some(prop) = target.get_mut(id) else { return };
    set_prop(&node, prop, kind, match_name, here, cx);
}

fn set_prop(node: &AepProp<'_, '_>, prop: &mut Property, kind: Kind, match_name: &str, here: &str, cx: &mut Cx<'_, '_>) {
    if node.is_animated() {
        import_keys(node, kind, here, match_name, cx.tm, prop, cx.col);
    } else if let Some(v) = node.static_value() {
        match kind.value(&v) {
            Some(value) => prop.value = value,
            None => cx.col.lose(LossCategory::DamagedData, format!("{here}: {match_name} holds a value that is not valid, default used")),
        }
    }
    import_expression(node, prop, cx.col);
}
