//! Text animators (`docs/aep-format.md` 3.10; cases 114, 579-594, 728): the animators of a text
//! layer with their properties and range selectors, the inverse of the importer's
//! `text_anim.rs`.
//!
//! The layer's `ADBE Text Animators` group is the one of 579 emptied; every animator is its first
//! animator (one range selector) with the properties and selectors of the EffectCraft animator.
//! An animator property is written whenever the animator has it (its presence is what After
//! Effects shows: Animate > Position adds the entry), a range selector property only when it
//! differs from After Effects' default or carries keys or an expression. Every record is the one
//! After Effects wrote for that property in a one-change case (579-592, 728; `template/text.aep`):
//! positions, anchor points, stroke widths and line spacing in pixels, scales, opacity, Start /
//! End / Offset, Amount, Smoothness and the eases in percent, angles in degrees, tracking in 1/1000
//! em, colours ARGB 0..255, popups 1-based (Units, Based On, Mode, Shape in EffectCraft's item
//! order). With Units = Index the selector's range is `Index Start / End / Offset` (583). Keys
//! and expressions are written as on shape properties (R46; 593, 594).

use effectcraft_aep::Chunk;
use effectcraft_keyframe::Value;
use effectcraft_project::{Layer, Node, PropGroup};

use crate::plan::CompPlan;
use crate::report::{Collector, LossCategory};
use crate::shapes::{Conv, Layout, RecVal, ShCx, build_record, c_color, c_popup, c_scalar, rec_value};
use crate::tree::{R, kids_mut, set_utf8, tpl};

/// More animators or selectors than this on one layer are not written (and reported).
const MAX_ITEMS: usize = 1024;

/// A range selector in the file's terms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelPlan {
    pub name: String,
    pub records: Vec<(&'static str, RecVal)>,
    pub advanced: Vec<(&'static str, RecVal)>,
}

/// An animator in the file's terms.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AnimPlan {
    pub name: String,
    pub props: Vec<(&'static str, RecVal)>,
    pub selectors: Vec<SelPlan>,
}

/// The prototypes (`template/text.aep`).
pub(crate) struct AnimProtos {
    /// `tdmn ADBE Text Animators` + the emptied group.
    pub group: Vec<Chunk<'static>>,
    /// `tdmn ADBE Text Animator` + the animator with one range selector and no properties.
    pub animator: Vec<Chunk<'static>>,
    /// Value records by match name.
    pub records: Vec<(String, [Chunk<'static>; 2])>,
}

fn c_vec3(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Vec3(p) => Some(p.to_vec()),
        Value::Vec2(p) => Some(vec![p[0], p[1], 0.0]),
        _ => None,
    }
}

fn c_vec2(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Vec2(p) => Some(p.to_vec()),
        Value::Vec3(p) => Some(vec![p[0], p[1]]),
        _ => None,
    }
}

fn c_bool(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Bool(b) => Some(vec![if *b { 1.0 } else { 0.0 }]),
        Value::Scalar(s) => Some(vec![if *s != 0.0 { 1.0 } else { 0.0 }]),
        _ => None,
    }
}

/// EffectCraft animator property -> stored match name, conversion, key layout (the importer's
/// `PROPS`, 579-592, 728).
const PROPS: &[(&str, &str, Conv, Layout)] = &[
    ("anchor", "ADBE Text Anchor Point 3D", c_vec3, Layout::Point),
    ("position", "ADBE Text Position 3D", c_vec3, Layout::Point),
    ("scale", "ADBE Text Scale 3D", c_vec3, Layout::Size),
    ("skew", "ADBE Text Skew", c_scalar, Layout::One),
    ("skewAxis", "ADBE Text Skew Axis", c_scalar, Layout::One),
    ("rotation", "ADBE Text Rotation", c_scalar, Layout::One),
    ("opacity", "ADBE Text Opacity", c_scalar, Layout::One),
    ("fillColor", "ADBE Text Fill Color", c_color, Layout::Color),
    ("strokeColor", "ADBE Text Stroke Color", c_color, Layout::Color),
    ("strokeWidth", "ADBE Text Stroke Width", c_scalar, Layout::One),
    ("tracking", "ADBE Text Tracking Amount", c_scalar, Layout::One),
    ("lineSpacing", "ADBE Text Line Spacing", c_vec2, Layout::Size),
    ("blur", "ADBE Text Blur", c_vec2, Layout::Size),
    ("characterOffset", "ADBE Text Character Offset", c_scalar, Layout::One),
];

/// Properties After Effects adds with another one and EffectCraft keeps at their default: written
/// by After Effects itself (Tracking Type with Tracking, Character Range and Alignment with
/// Character Offset, 728); a value other than the default is reported.
const DEPENDENT: &[(&str, Value)] = &[("trackingType", Value::Enum(0)), ("characterRange", Value::Enum(0)), ("characterAlignment", Value::Enum(1))];

/// Range selector properties: EffectCraft id, stored match names (percent units, index units),
/// conversion, default (stored).
const RANGE: &[(&str, &str, &str, f64)] = &[
    ("start", "ADBE Text Percent Start", "ADBE Text Index Start", 0.0),
    ("end", "ADBE Text Percent End", "ADBE Text Index End", 100.0),
    ("offset", "ADBE Text Percent Offset", "ADBE Text Index Offset", 0.0),
];

const ADVANCED: &[(&str, &str, Conv, Layout, f64)] = &[
    ("units", "ADBE Text Range Units", c_popup, Layout::Popup, 1.0),
    ("basedOn", "ADBE Text Range Type2", c_popup, Layout::Popup, 1.0),
    ("mode", "ADBE Text Selector Mode", c_popup, Layout::Popup, 1.0),
    ("amount", "ADBE Text Selector Max Amount", c_scalar, Layout::One, 100.0),
    ("shape", "ADBE Text Range Shape", c_popup, Layout::Popup, 1.0),
    ("smoothness", "ADBE Text Selector Smoothness", c_scalar, Layout::One, 100.0),
    ("easeHigh", "ADBE Text Levels Max Ease", c_scalar, Layout::One, 0.0),
    ("easeLow", "ADBE Text Levels Min Ease", c_scalar, Layout::One, 0.0),
    ("randomize", "ADBE Text Randomize Order", c_bool, Layout::Popup, 0.0),
    ("randomSeed", "ADBE Text Random Seed", c_scalar, Layout::One, 0.0),
];

fn layout_of(name: &str) -> Layout {
    if let Some((_, _, _, l)) = PROPS.iter().find(|(_, n, ..)| *n == name) {
        return *l;
    }
    if let Some((_, _, _, l, _)) = ADVANCED.iter().find(|(_, n, ..)| *n == name) {
        return *l;
    }
    Layout::One
}

/// The animators of `layer` (its `text/animators` group) in the file's terms; what cannot be
/// written is reported.
pub(crate) fn plan(layer: &Layer, cp: &CompPlan, what: &str, col: &mut Collector) -> Vec<AnimPlan> {
    let Some(group) = layer.props.sub("text").and_then(|t| t.sub("animators")) else { return Vec::new() };
    let mut cx = ShCx { cp, col };
    let mut out = Vec::new();
    for (i, node) in group.children.iter().enumerate() {
        if i >= MAX_ITEMS {
            cx.col.lose(LossCategory::TextLayers, format!("{what}: more than {MAX_ITEMS} text animators, the rest are not written"));
            break;
        }
        let Node::Group(a) = node else { continue };
        let here = format!("{what}: animator '{}'", a.name);
        if !a.enabled {
            cx.col.lose(LossCategory::TextLayers, format!("{here}: a switched-off animator is written switched on (the switch is not decoded)"));
        }
        let mut props = Vec::new();
        if let Some(pg) = a.sub("properties") {
            for n in &pg.children {
                let Node::Prop(p) = n else { continue };
                if let Some((_, name, conv, layout)) = PROPS.iter().find(|(id, ..)| *id == p.match_id) {
                    let w = format!("{here}: {name}");
                    if let Some(v) = rec_value(p, *conv, *layout, None, &w, LossCategory::TextLayers, &mut cx) {
                        props.push((*name, v));
                    }
                } else if let Some((_, def)) = DEPENDENT.iter().find(|(id, _)| *id == p.match_id) {
                    if p.value != *def || !p.keys.is_empty() || p.has_expression() {
                        cx.col.lose(LossCategory::TextLayers, format!("{here}: {} is not written (only its default is sampled)", p.name));
                    }
                } else {
                    cx.col.lose(LossCategory::TextLayers, format!("{here}: the animator property {} is not written", p.name));
                }
            }
        }
        let mut selectors = Vec::new();
        if let Some(sg) = a.sub("selectors") {
            for (k, n) in sg.children.iter().enumerate() {
                if k >= MAX_ITEMS {
                    cx.col.lose(LossCategory::TextLayers, format!("{here}: more than {MAX_ITEMS} selectors, the rest are not written"));
                    break;
                }
                let Node::Group(s) = n else { continue };
                if s.match_id != "rangeSelector" {
                    cx.col.lose(LossCategory::TextLayers, format!("{here}: the selector '{}' ({}) is not written (only range selectors)", s.name, s.match_id));
                    continue;
                }
                selectors.push(plan_selector(s, &here, &mut cx));
            }
        }
        cx.col.exported.text_animators = cx.col.exported.text_animators.saturating_add(1);
        out.push(AnimPlan { name: crate::plan::cut_name(&a.name, 255, &here, cx.col), props, selectors });
    }
    out
}

fn plan_selector(s: &PropGroup, animator: &str, cx: &mut ShCx<'_>) -> SelPlan {
    let here = format!("{animator} '{}'", s.name);
    let adv = s.sub("advanced");
    let index = adv.and_then(|a| a.get("units")).is_some_and(|u| u.value == Value::Enum(1));
    let mut records = Vec::new();
    for (id, percent, idx, default) in RANGE {
        let Some(p) = s.get(id) else { continue };
        let name = if index { *idx } else { *percent };
        // With Units = Index After Effects stores the range in characters, records for all three
        // (583).
        let default = if index { None } else { Some(std::slice::from_ref(default)) };
        if let Some(v) = rec_value(p, c_scalar, Layout::One, default, &format!("{here}: {name}"), LossCategory::TextLayers, cx) {
            records.push((name, v));
        }
    }
    let mut advanced = Vec::new();
    if let Some(a) = adv {
        for (id, name, conv, layout, default) in ADVANCED {
            let Some(p) = a.get(id) else { continue };
            if let Some(v) = rec_value(p, *conv, *layout, Some(std::slice::from_ref(default)), &format!("{here}: {name}"), LossCategory::TextLayers, cx) {
                advanced.push((*name, v));
            }
        }
    }
    SelPlan { name: crate::plan::cut_name(&s.name, 255, &here, cx.col), records, advanced }
}

fn tdmn_name(c: &Chunk<'_>) -> Option<String> {
    let b = c.bytes()?;
    let end = b.iter().position(|x| *x == 0).unwrap_or(b.len());
    std::str::from_utf8(b.get(..end)?).ok().map(str::to_string)
}

fn pos(kids: &[Chunk<'static>], n: &str) -> Option<usize> {
    kids.iter().position(|c| c.is(b"tdmn") && tdmn_name(c).as_deref() == Some(n))
}

/// The item's user name (`tdsn/Utf8`).
fn set_name(node: &mut Chunk<'static>, name: &str) -> R<()> {
    let kids = kids_mut(node)?;
    let tdsn = kids.iter_mut().find(|c| c.is(b"tdsn")).ok_or_else(|| tpl("animator prototype without a name"))?;
    set_utf8(tdsn, name)
}

/// Inserts records before the group's first sub-group or end.
fn put_records(t: &AnimProtos, group: &mut Chunk<'static>, records: &[(&'static str, RecVal)], key_list: &Chunk<'static>) -> R<()> {
    let kids = kids_mut(group)?;
    let mut at = kids.iter().position(|c| c.is(b"tdmn")).unwrap_or(kids.len());
    for (name, v) in records {
        let [m, node] = t.records.iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()).ok_or_else(|| tpl("text animator record prototype missing"))?;
        let [m, node] = build_record(m, node, layout_of(name), v, key_list)?;
        kids.insert(at, m);
        kids.insert(at.saturating_add(1), node);
        at = at.saturating_add(2);
    }
    Ok(())
}

/// Adds the `ADBE Text Animators` group with `anims` to the text layer `layr` (after More
/// Options, as 579 stores it).
pub(crate) fn put_animators(layr: &mut Chunk<'static>, t: &AnimProtos, anims: &[AnimPlan], key_list: &Chunk<'static>) -> R<()> {
    if anims.is_empty() {
        return Ok(());
    }
    let [gm, mut group] = <[Chunk<'static>; 2]>::try_from(t.group.clone()).map_err(|_| tpl("animators prototype incomplete"))?;
    let [am, anim_proto] = <[Chunk<'static>; 2]>::try_from(t.animator.clone()).map_err(|_| tpl("animator prototype incomplete"))?;
    let mut built = Vec::with_capacity(anims.len() * 2);
    for a in anims {
        let mut node = anim_proto.clone();
        set_name(&mut node, &a.name)?;
        let kids = kids_mut(&mut node)?;
        // Selectors: one copy of the prototype's selector each.
        let si = pos(kids, "ADBE Text Selectors").ok_or_else(|| tpl("animator prototype without selectors"))?;
        {
            let sels = kids.get_mut(si + 1).ok_or_else(|| tpl("animator prototype without selectors"))?;
            let sk = kids_mut(sels)?;
            let at = pos(sk, "ADBE Text Selector").ok_or_else(|| tpl("animator prototype without a selector"))?;
            let sm = sk.get(at).cloned().ok_or_else(|| tpl("animator prototype without a selector"))?;
            let sproto = sk.get(at + 1).cloned().ok_or_else(|| tpl("animator prototype without a selector"))?;
            sk.drain(at..at + 2);
            let mut sel_nodes = Vec::with_capacity(a.selectors.len() * 2);
            for s in &a.selectors {
                let mut sn = sproto.clone();
                set_name(&mut sn, &s.name)?;
                if !s.advanced.is_empty() {
                    let snk = kids_mut(&mut sn)?;
                    let ai = pos(snk, "ADBE Text Range Advanced").ok_or_else(|| tpl("selector prototype without Advanced"))?;
                    let adv = snk.get_mut(ai + 1).ok_or_else(|| tpl("selector prototype without Advanced"))?;
                    put_records(t, adv, &s.advanced, key_list)?;
                }
                put_records(t, &mut sn, &s.records, key_list)?;
                sel_nodes.push(sm.clone());
                sel_nodes.push(sn);
            }
            sk.splice(at..at, sel_nodes);
        }
        let pi = pos(kids, "ADBE Text Animator Properties").ok_or_else(|| tpl("animator prototype without properties"))?;
        let props = kids.get_mut(pi + 1).ok_or_else(|| tpl("animator prototype without properties"))?;
        put_records(t, props, &a.props, key_list)?;
        built.push(am.clone());
        built.push(node);
    }
    {
        let gk = kids_mut(&mut group)?;
        let end = pos(gk, "ADBE Group End").unwrap_or(gk.len());
        gk.splice(end..end, built);
    }
    // Into the layer's `ADBE Text Properties`, before its Group End.
    let root = crate::tree::list_mut(layr, b"tdgp")?;
    let rk = kids_mut(root)?;
    let ti = pos(rk, "ADBE Text Properties").ok_or_else(|| tpl("text layer without text properties"))?;
    let tp = rk.get_mut(ti + 1).ok_or_else(|| tpl("text layer without text properties"))?;
    let tk = kids_mut(tp)?;
    let end = pos(tk, "ADBE Group End").unwrap_or(tk.len());
    tk.splice(end..end, [gm, group]);
    Ok(())
}
