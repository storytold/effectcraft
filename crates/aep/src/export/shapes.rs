//! Shape layers (`docs/aep-format.md` 3.9; cases 238-242, 451-471, 519, 707-710, 731, 851-857):
//! groups (nested), rectangle, ellipse, polystar and freeform paths, fills, strokes, gradient
//! fills and strokes, trim paths, round corners, merge paths, offset paths and repeaters, the
//! group transform, with keys and expressions on their properties.
//!
//! The layer block is the shape layer of 519 with its contents removed; every item is the item
//! of 519 or 731 (all at After Effects' defaults, no value records) or the path item of 240, and
//! every property that differs from After Effects' default (or carries keys or an expression)
//! gets a value record, in After Effects' order (a record only for a non-default value, P.5):
//! sizes and positions in pixels relative to the layer, colours `[A, R, G, B]` 0..255,
//! opacities, percentages and the group scale in percent, popups 1-based (fill rule 455, line
//! cap / join 239, trim type 469, polystar type 331, merge type 707, offset line join 710),
//! angles in degrees. A record is the record After Effects wrote for the property in a
//! one-change case, else the record of its property's type (R46): over the corpus every shape
//! value record of a type has the same bytes but for its match name, the time scale and, for
//! sliders, the slider range in `tdum` / `tduM` (0..100 for every slider but Skew -85..85 and
//! Highlight Length -100..100; sizes and scales -32 000..32 000). A keyed record clears `tdb4`
//! bit 0 of byte 5, sets byte 68 and holds the key list (851-855, 857, 209): one-component and
//! two-component sizes plain records (48 / 88 bytes), points spatial records (104 bytes),
//! colours the colour records of effects (152 bytes). An expression is stored as on the
//! Transform properties (856, R15).

use crate::Chunk;
use effectcraft_keyframe::{Gradient, Interp, Keyframe, Value};
use effectcraft_project::{Node, PropGroup, Property};

use crate::export::keys::{fill_key_list, replace_cdat, spatial_record, timed_keys};
use crate::export::masks::ShapePlan;
use crate::export::plan::{CompPlan, EaseSlot, KeyPlan};
use crate::export::report::{Collector, LossCategory};
use crate::export::synth::{GROUP_END, is_tdmn, tdmn};
use crate::export::tree::{R, child_data, kids_mut, list_mut, put_f64, put_u8, set_utf8, tpl};

/// The deepest group nesting written.
const MAX_DEPTH: usize = 16;

/// A property's value in the file's units: the static value, the keys (layer time) and the
/// expression with its enable switch.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RecVal {
    pub value: Vec<f64>,
    pub keys: Vec<KeyPlan>,
    pub expr: Option<(String, bool)>,
}

type Records = Vec<(&'static str, RecVal)>;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ShItem {
    Group {
        name: String,
        items: Vec<ShItem>,
        transform: Records,
    },
    /// Any other item but a path: the item's match name, name, value records and the records of
    /// its sub-groups (a repeater's transform).
    Simple {
        kind: &'static str,
        name: String,
        records: Records,
        subs: Vec<(&'static str, Records)>,
    },
    Path {
        name: String,
        shape: ShapePlan,
    },
    /// An item switched off (its eye): written with bit 0 of its group's `tdsb` cleared.
    Off(Box<ShItem>),
}

/// The prototypes, by match name.
pub(crate) struct ShapeProtos {
    /// The shape layer's block (519) with an empty `ADBE Root Vectors Group`.
    pub block: Vec<Chunk<'static>>,
    /// Item nodes (`tdmn` + node) at their defaults.
    pub items: Vec<(String, [Chunk<'static>; 2])>,
    /// Value records (`tdmn` + `tdbs`) by match name.
    pub records: Vec<(String, [Chunk<'static>; 2])>,
    /// The key list (`LIST list`) of a keyed property, whose header is rewritten.
    pub key_list: Chunk<'static>,
}

/// How a shape property is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// One component (sliders, angles, popups): plain keys of 48 bytes.
    One,
    /// A size or scale of two or three components: plain keys of 88 / 128 bytes.
    Size,
    /// A point: spatial keys of 104 bytes (853; 128 bytes for a 3D point).
    Point,
    /// A colour: the 152-byte colour keys (854).
    Color,
    /// A popup (keys not sampled).
    Popup,
}

/// The record whose layout a property takes, the slider range its record repeats in `tdum` /
/// `tduM` (R46), and the key layout.
#[derive(Clone, Copy)]
struct RecKind {
    proto: &'static str,
    range: Option<(f64, f64)>,
    layout: Layout,
}

const SLIDER: RecKind = RecKind { proto: "ADBE Vector Fill Opacity", range: Some((0.0, 100.0)), layout: Layout::One };
const ANGLE: RecKind = RecKind { proto: "ADBE Vector Rotation", range: None, layout: Layout::One };
const POPUP: RecKind = RecKind { proto: "ADBE Vector Fill Rule", range: None, layout: Layout::Popup };
const POINT: RecKind = RecKind { proto: "ADBE Vector Ellipse Position", range: None, layout: Layout::Point };
const SIZE: RecKind = RecKind { proto: "ADBE Vector Scale", range: Some((-32_000.0, 32_000.0)), layout: Layout::Size };
const COLOR: RecKind = RecKind { proto: "ADBE Vector Fill Color", range: None, layout: Layout::Color };

/// Every property the exporter writes, in After Effects' order within an item (455, 464, 239,
/// 331, 332, 828, 709), with its record type (sampled: 451-471, 238-242, 331, 332, 468, 469,
/// 473-477, 707-710, 828; Offset Paths' Miter Limit, the Repeater's Offset, Composite, Anchor and
/// Start Opacity are not sampled and take the type of their kind, E).
const KINDS: &[(&str, RecKind)] = &[
    ("ADBE Vector Rect Size", SIZE),
    ("ADBE Vector Rect Position", POINT),
    ("ADBE Vector Rect Roundness", SLIDER),
    ("ADBE Vector Ellipse Size", SIZE),
    ("ADBE Vector Ellipse Position", POINT),
    ("ADBE Vector Star Type", POPUP),
    ("ADBE Vector Star Points", SLIDER),
    ("ADBE Vector Star Position", POINT),
    ("ADBE Vector Star Rotation", ANGLE),
    ("ADBE Vector Star Inner Radius", SLIDER),
    ("ADBE Vector Star Outer Radius", SLIDER),
    ("ADBE Vector Star Inner Roundess", SLIDER),
    ("ADBE Vector Star Outer Roundess", SLIDER),
    ("ADBE Vector Fill Rule", POPUP),
    ("ADBE Vector Grad Type", POPUP),
    ("ADBE Vector Grad Start Pt", POINT),
    ("ADBE Vector Grad End Pt", POINT),
    ("ADBE Vector Grad HiLite Length", RecKind { proto: "ADBE Vector Fill Opacity", range: Some((-100.0, 100.0)), layout: Layout::One }),
    ("ADBE Vector Grad HiLite Angle", ANGLE),
    ("ADBE Vector Fill Color", COLOR),
    ("ADBE Vector Fill Opacity", SLIDER),
    ("ADBE Vector Stroke Color", COLOR),
    ("ADBE Vector Stroke Opacity", SLIDER),
    ("ADBE Vector Stroke Width", SLIDER),
    ("ADBE Vector Stroke Line Cap", POPUP),
    ("ADBE Vector Stroke Line Join", POPUP),
    ("ADBE Vector Stroke Miter Limit", SLIDER),
    ("ADBE Vector Trim Start", SLIDER),
    ("ADBE Vector Trim End", SLIDER),
    ("ADBE Vector Trim Offset", ANGLE),
    ("ADBE Vector Trim Type", POPUP),
    ("ADBE Vector RoundCorner Radius", SLIDER),
    ("ADBE Vector Merge Type", POPUP),
    ("ADBE Vector Offset Amount", SLIDER),
    ("ADBE Vector Offset Line Join", POPUP),
    ("ADBE Vector Offset Miter Limit", SLIDER),
    ("ADBE Vector Offset Copies", SLIDER),
    ("ADBE Vector Offset Copy Offset", SLIDER),
    ("ADBE Vector Repeater Copies", SLIDER),
    ("ADBE Vector Repeater Offset", SLIDER),
    ("ADBE Vector Repeater Order", POPUP),
    ("ADBE Vector Repeater Anchor", POINT),
    ("ADBE Vector Repeater Position", POINT),
    ("ADBE Vector Repeater Scale", SIZE),
    ("ADBE Vector Repeater Rotation", ANGLE),
    ("ADBE Vector Repeater Opacity 1", SLIDER),
    ("ADBE Vector Repeater Opacity 2", SLIDER),
    ("ADBE Vector Anchor", POINT),
    ("ADBE Vector Position", POINT),
    ("ADBE Vector Scale", SIZE),
    // Skew has its own record (460: a slider of the angle layout, -85..85).
    ("ADBE Vector Skew", RecKind { proto: "ADBE Vector Skew", range: Some((-85.0, 85.0)), layout: Layout::One }),
    ("ADBE Vector Skew Axis", ANGLE),
    ("ADBE Vector Rotation", ANGLE),
    ("ADBE Vector Group Opacity", SLIDER),
];

fn kind_of(name: &str) -> Option<RecKind> {
    KINDS.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

impl ShapeProtos {
    fn item(&self, name: &str) -> R<[Chunk<'static>; 2]> {
        self.items.iter().find(|(n, _)| n == name).map(|(_, c)| c.clone()).ok_or_else(|| tpl("shape item prototype missing"))
    }

    /// The static value record of `name`: the one After Effects wrote for it in a one-change
    /// case, else the record of its type (R46).
    fn record(&self, name: &'static str) -> R<[Chunk<'static>; 2]> {
        if let Some((_, c)) = self.records.iter().find(|(n, _)| n == name) {
            return Ok(c.clone());
        }
        let kind = kind_of(name).ok_or_else(|| tpl("shape property without a record type"))?;
        let [_, mut node] = self.records.iter().find(|(n, _)| n == kind.proto).map(|(_, c)| c.clone()).ok_or_else(|| tpl("shape record prototype missing"))?;
        if let Some((lo, hi)) = kind.range {
            put_f64(child_data(&mut node, b"tdum")?, 0, lo)?;
            put_f64(child_data(&mut node, b"tduM")?, 0, hi)?;
        }
        Ok([tdmn(name), node])
    }
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() <= 1e9)
}

/// The stored components of a value.
pub(crate) type Conv = fn(&Value) -> Option<Vec<f64>>;

pub(crate) fn c_scalar(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Scalar(s) => Some(vec![*s]),
        _ => None,
    }
}

/// Offset Paths' Copy Offset: EffectCraft's value + 1 (826-828).
fn c_copy_offset(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Scalar(s) => Some(vec![*s + 1.0]),
        _ => None,
    }
}

pub(crate) fn c_point(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Vec2(p) => Some(p.to_vec()),
        Value::Vec3(p) => Some(vec![p[0], p[1]]),
        _ => None,
    }
}

/// A colour as the file stores it (alpha 255, then red, green and blue from 0 to 255). A channel
/// outside 0 to 1 is written as the nearest limit, as for lights (`rec_value` reports it); a
/// value that is not finite stays so and is reported as invalid.
pub(crate) fn c_color(v: &Value) -> Option<Vec<f64>> {
    let ch = |x: f64| if x.is_finite() { x.clamp(0.0, 1.0) * 255.0 } else { x };
    match v {
        Value::Color(c) => Some(vec![255.0, ch(c[0]), ch(c[1]), ch(c[2])]),
        _ => None,
    }
}

/// Whether a colour that is written has a channel outside 0 to 1: the keys of an animated
/// property (its static value is not written then), else the static value.
fn colour_beyond(p: &Property, keyed: bool) -> bool {
    let beyond = |v: &Value| matches!(v, Value::Color(c) if c.iter().take(3).any(|x| x.is_finite() && !(0.0..=1.0).contains(x)));
    if keyed { p.keys.iter().any(|k| beyond(&k.value)) } else { beyond(&p.value) }
}

/// Whether a colour that is written has an alpha other than 1 (a colour property of After Effects
/// has no alpha): the keys of an animated property, else the static value.
fn colour_alpha(p: &Property, keyed: bool) -> bool {
    let alpha = |v: &Value| matches!(v, Value::Color(c) if c[3].is_finite() && (c[3] - 1.0).abs() > 1e-9);
    if keyed { p.keys.iter().any(|k| alpha(&k.value)) } else { alpha(&p.value) }
}

/// A popup stored 1-based (EffectCraft's items in After Effects' order).
pub(crate) fn c_popup(v: &Value) -> Option<Vec<f64>> {
    match v {
        Value::Enum(e) => Some(vec![f64::from(*e) + 1.0]),
        _ => None,
    }
}

/// What planning a shape layer needs: the layer's time base and the report.
pub(crate) struct ShCx<'a> {
    pub cp: &'a CompPlan,
    pub col: &'a mut Collector,
}

/// The keys of `p` in the file's units.
fn plan_keys(p: &Property, conv: Conv, layout: Layout, what: &str, cx: &mut ShCx<'_>) -> Vec<KeyPlan> {
    let timed = timed_keys(&p.keys, &cx.cp.tb, what, "key", cx.col);
    let all: Vec<Keyframe> = timed.iter().map(|(_, k)| (*k).clone()).collect();
    let mut out = Vec::with_capacity(timed.len());
    let beyond = std::cell::Cell::new(false);
    for (i, (u, k)) in timed.iter().enumerate() {
        let Ok(time) = i32::try_from(*u) else {
            cx.col.lose(LossCategory::InvalidValues, format!("{what}: key time out of range, key dropped"));
            continue;
        };
        let Some(value) = conv(&k.value).filter(|v| finite(v)) else {
            cx.col.lose(LossCategory::InvalidValues, format!("{what}: a key value that cannot be stored, key dropped"));
            continue;
        };
        let mut side = |interp: Interp| -> u8 {
            match interp {
                Interp::Linear => 1,
                Interp::Hold => 3,
                // The ease of colour keys is sampled only as speed 0 / influence 0 (598).
                Interp::Bezier if layout == Layout::Color => {
                    cx.col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: bezier ease of a colour key is written as linear (not sampled)"));
                    1
                }
                Interp::Bezier => 2,
            }
        };
        let (in_i, out_i) = (side(k.in_interp), side(k.out_interp));
        let slots = if layout == Layout::Size { value.len().max(1) } else { 1 };
        let ease = |eases: &[effectcraft_keyframe::Ease], on: bool| -> Vec<EaseSlot> {
            (0..slots)
                .map(|s| {
                    if !on {
                        return EaseSlot::default();
                    }
                    let e = crate::export::plan::key_ease(eases, s);
                    let speed = if e.speed.is_finite() && e.speed.abs() <= 1e9 {
                        e.speed
                    } else {
                        beyond.set(true);
                        0.0
                    };
                    let influence = if e.influence.is_finite() { e.influence.clamp(0.001, 1.0) } else { 1.0 / 3.0 };
                    EaseSlot { speed, influence }
                })
                .collect()
        };
        let (mut tin, mut tout) = if layout == Layout::Point { effectcraft_keyframe::spatial_tangents(&all, i) } else { ([0.0; 3], [0.0; 3]) };
        for t in tin.iter_mut().chain(tout.iter_mut()) {
            if !t.is_finite() || t.abs() > 1e9 {
                beyond.set(true);
                *t = 0.0;
            }
        }
        if k.roving {
            cx.col.lose(LossCategory::ApproximatedKeyframes, format!("{what}: a roving key is written as a normal key"));
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
            spatial_auto: k.spatial_auto,
            roving: false,
            label: k.label.min(16),
        });
    }
    cx.col.exported.keyframes = cx.col.exported.keyframes.saturating_add(out.len());
    crate::export::plan::report_beyond(beyond.get(), what, cx.col);
    out
}

/// A property's value, keys and expression in the file's units; `None` when it needs no record:
/// at `default` (when given) with no keys and no expression. Keys on a popup are not sampled:
/// the first key's value is written (reported in `category`).
pub(crate) fn rec_value(
    p: &Property,
    conv: Conv,
    layout: Layout,
    default: Option<&[f64]>,
    w: &str,
    category: LossCategory,
    cx: &mut ShCx<'_>,
) -> Option<RecVal> {
    let expr = p.expr.as_ref().filter(|e| !e.text.is_empty()).map(|e| (e.text.clone(), e.enabled));
    if expr.is_some() {
        cx.col.exported.expressions = cx.col.exported.expressions.saturating_add(1);
    }
    let Some(value) = conv(&p.value) else {
        cx.col.lose(category, format!("{w} holds a value of another type, not written"));
        return None;
    };
    if !finite(&value) {
        cx.col.lose(LossCategory::InvalidValues, format!("{w} is not a valid number (the default is written)"));
        return None;
    }
    let differs = |v: &[f64]| default.is_none_or(|d| v.iter().zip(d).any(|(a, b)| (a - b).abs() > 1e-9));
    let mut keys = Vec::new();
    if !p.keys.is_empty() {
        if layout == Layout::Popup {
            cx.col.lose(category, format!("{w}: keys on a popup are not written (not sampled), the first key's value is written"));
            let first = p.keys.first().and_then(|k| conv(&k.value)).unwrap_or(value);
            return (differs(&first) || expr.is_some()).then_some(RecVal { value: first, keys: Vec::new(), expr });
        }
        keys = plan_keys(p, conv, layout, w, cx);
    }
    // Shape and text animator colours alike (both are planned here).
    if layout == Layout::Color && colour_beyond(p, !keys.is_empty()) {
        cx.col.lose(category, format!("{w}: a colour channel outside 0 to 1 is written as the nearest limit"));
    }
    if layout == Layout::Color && colour_alpha(p, !keys.is_empty()) {
        cx.col.lose(category, format!("{w}: the colour's alpha is not written (After Effects colours have none; written opaque)"));
    }
    if keys.is_empty() && expr.is_none() && !differs(&value) {
        return None;
    }
    let value = keys.first().map(|k| k.value.clone()).unwrap_or(value);
    Some(RecVal { value, keys, expr })
}

/// Plans property `id` of `g` as the record `name` (`names`: `(name, id)`) unless it is at After
/// Effects' default with no keys and no expression.
fn rec(out: &mut Records, g: &PropGroup, names: (&'static str, &str), conv: Conv, default: &[f64], what: &str, cx: &mut ShCx<'_>) {
    let (name, id) = names;
    let Some(p) = g.get(id) else { return };
    let Some(kind) = kind_of(name) else { return };
    let w = format!("{what}: {name}");
    if let Some(v) = rec_value(p, conv, kind.layout, Some(default), &w, LossCategory::ShapeLayers, cx) {
        out.push((name, v));
    }
}

fn en(g: &PropGroup, id: &str) -> Option<u32> {
    match g.get(id).map(|p| &p.value) {
        Some(Value::Enum(e)) => Some(*e),
        _ => None,
    }
}

/// Opacity, width, cap, join and miter limit of a stroke or gradient stroke; dashes, taper and
/// wave are reported when they hold values.
fn stroke_common(r: &mut Records, g: &PropGroup, w: &str, cx: &mut ShCx<'_>) {
    rec(r, g, ("ADBE Vector Stroke Opacity", "opacity"), c_scalar, &[100.0], w, cx);
    rec(r, g, ("ADBE Vector Stroke Width", "width"), c_scalar, &[2.0], w, cx);
    rec(r, g, ("ADBE Vector Stroke Line Cap", "cap"), c_popup, &[1.0], w, cx);
    rec(r, g, ("ADBE Vector Stroke Line Join", "join"), c_popup, &[1.0], w, cx);
    rec(r, g, ("ADBE Vector Stroke Miter Limit", "miter"), c_scalar, &[4.0], w, cx);
    // Dashes, a taper or a wave change the stroke (After Effects' defaults: no dash, no taper
    // length, no wave amount).
    let nonzero = |sub: &str, ids: &[&str]| {
        g.sub(sub).is_some_and(|s| s.props().any(|p| ids.iter().any(|i| p.match_id.starts_with(i)) && matches!(p.value, Value::Scalar(v) if v != 0.0)))
    };
    if nonzero("dashes", &["dash", "gap"]) || nonzero("taper", &["startLength", "endLength"]) || nonzero("wave", &["amount"]) {
        cx.col.lose(LossCategory::ShapeLayers, format!("{w}: dashes, taper and wave of a stroke are not written"));
    }
}

/// The gradient part of a gradient fill or stroke: type, start, end, highlight; the colour
/// stops are not decoded (only the default white to black is written).
fn gradient(r: &mut Records, g: &PropGroup, w: &str, cx: &mut ShCx<'_>) {
    rec(r, g, ("ADBE Vector Grad Type", "type"), c_popup, &[1.0], w, cx);
    rec(r, g, ("ADBE Vector Grad Start Pt", "start"), c_point, &[0.0, 0.0], w, cx);
    rec(r, g, ("ADBE Vector Grad End Pt", "end"), c_point, &[100.0, 0.0], w, cx);
    rec(r, g, ("ADBE Vector Grad HiLite Length", "highlightLength"), c_scalar, &[0.0], w, cx);
    rec(r, g, ("ADBE Vector Grad HiLite Angle", "highlightAngle"), c_scalar, &[0.0], w, cx);
    if let Some(p) = g.get("colors")
        && (matches!(&p.value, Value::Gradient(gr) if *gr != Gradient::default()) || !p.keys.is_empty() || p.has_expression())
    {
        cx.col.lose(LossCategory::ShapeLayers, format!("{w}: the colour stops of a gradient are not written (not decoded), white to black"));
    }
}

/// The items of a `contents` group, top first, in the file's terms.
pub(crate) fn plan_contents(contents: &PropGroup, what: &str, depth: usize, cx: &mut ShCx<'_>) -> Vec<ShItem> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        cx.col.lose(LossCategory::ShapeLayers, format!("{what}: shape groups nested deeper than {MAX_DEPTH} levels are not written"));
        return out;
    }
    for node in &contents.children {
        let Node::Group(g) = node else { continue };
        let w = format!("{what} / {}", g.name);
        // A switched-off item keeps its node with bit 0 of its group flags cleared (986, 987).
        let wrap = |item: ShItem| if g.enabled { item } else { ShItem::Off(Box::new(item)) };
        if en(g, "direction").is_some_and(|d| d != 0) {
            cx.col.lose(LossCategory::ShapeLayers, format!("{w}: the path direction is not written"));
        }
        // A repeater writes its `composite` as Repeater Order.
        let composite_lost = g.match_id != "repeater" && en(g, "composite").is_some_and(|d| d != 0);
        if en(g, "blend").is_some_and(|d| d != 0) || composite_lost {
            cx.col.lose(LossCategory::ShapeLayers, format!("{w}: blend mode and composite order are not written"));
        }
        let mut r = Vec::new();
        let mut subs = Vec::new();
        let kind = match g.match_id.as_str() {
            "group" => {
                let items = g.sub("contents").map(|c| plan_contents(c, &w, depth + 1, cx)).unwrap_or_default();
                let mut transform = Vec::new();
                if let Some(t) = g.sub("transform") {
                    rec(&mut transform, t, ("ADBE Vector Anchor", "anchor"), c_point, &[0.0, 0.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Position", "position"), c_point, &[0.0, 0.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Scale", "scale"), c_point, &[100.0, 100.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Skew", "skew"), c_scalar, &[0.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Skew Axis", "skewAxis"), c_scalar, &[0.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Rotation", "rotation"), c_scalar, &[0.0], &w, cx);
                    rec(&mut transform, t, ("ADBE Vector Group Opacity", "opacity"), c_scalar, &[100.0], &w, cx);
                }
                out.push(wrap(ShItem::Group { name: g.name.clone(), items, transform }));
                continue;
            }
            "path" => {
                let p = g.get("path");
                if p.is_some_and(|p| !p.keys.is_empty() || p.has_expression()) {
                    cx.col.lose(LossCategory::ShapeLayers, format!("{w}: keys or an expression on the path (the static path is written)"));
                }
                match p.map(|p| &p.value) {
                    Some(Value::Path(sp)) => match crate::export::masks::shape_plan(sp, (1, 1)) {
                        Ok(shape) => out.push(wrap(ShItem::Path { name: g.name.clone(), shape })),
                        Err(why) => cx.col.lose(LossCategory::ShapeLayers, format!("{w}: the path is not written: {why}")),
                    },
                    _ => cx.col.lose(LossCategory::ShapeLayers, format!("{w}: a path item without a path is not written")),
                }
                continue;
            }
            "rect" => {
                rec(&mut r, g, ("ADBE Vector Rect Size", "size"), c_point, &[100.0, 100.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Rect Position", "position"), c_point, &[0.0, 0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Rect Roundness", "roundness"), c_scalar, &[0.0], &w, cx);
                if g.get("perCorner").is_some_and(|p| p.value == Value::Bool(true) || !p.keys.is_empty() || p.has_expression()) {
                    cx.col.lose(LossCategory::ShapeLayers, format!("{w}: per-corner roundness is not written (the uniform Roundness is)"));
                }
                "ADBE Vector Shape - Rect"
            }
            "ellipse" => {
                rec(&mut r, g, ("ADBE Vector Ellipse Size", "size"), c_point, &[100.0, 100.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Ellipse Position", "position"), c_point, &[0.0, 0.0], &w, cx);
                "ADBE Vector Shape - Ellipse"
            }
            // Polystar (104, 331, 473-477): After Effects' defaults are a 5-point star with radii
            // 100 / 50 (519).
            "star" => {
                rec(&mut r, g, ("ADBE Vector Star Type", "type"), c_popup, &[1.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Points", "points"), c_scalar, &[5.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Position", "position"), c_point, &[0.0, 0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Rotation", "rotation"), c_scalar, &[0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Inner Radius", "innerRadius"), c_scalar, &[50.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Outer Radius", "outerRadius"), c_scalar, &[100.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Inner Roundess", "innerRoundness"), c_scalar, &[0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Star Outer Roundess", "outerRoundness"), c_scalar, &[0.0], &w, cx);
                "ADBE Vector Shape - Star"
            }
            "fill" => {
                rec(&mut r, g, ("ADBE Vector Fill Rule", "rule"), c_popup, &[1.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Fill Color", "color"), c_color, &[255.0, 255.0, 0.0, 0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Fill Opacity", "opacity"), c_scalar, &[100.0], &w, cx);
                "ADBE Vector Graphic - Fill"
            }
            "stroke" => {
                rec(&mut r, g, ("ADBE Vector Stroke Color", "color"), c_color, &[255.0, 255.0, 255.0, 255.0], &w, cx);
                stroke_common(&mut r, g, &w, cx);
                "ADBE Vector Graphic - Stroke"
            }
            "gfill" => {
                rec(&mut r, g, ("ADBE Vector Fill Rule", "rule"), c_popup, &[1.0], &w, cx);
                gradient(&mut r, g, &w, cx);
                rec(&mut r, g, ("ADBE Vector Fill Opacity", "opacity"), c_scalar, &[100.0], &w, cx);
                "ADBE Vector Graphic - G-Fill"
            }
            "gstroke" => {
                gradient(&mut r, g, &w, cx);
                stroke_common(&mut r, g, &w, cx);
                "ADBE Vector Graphic - G-Stroke"
            }
            // Trim Paths (101, 465-469, 851-856).
            "trim" => {
                rec(&mut r, g, ("ADBE Vector Trim Start", "start"), c_scalar, &[0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Trim End", "end"), c_scalar, &[100.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Trim Offset", "offset"), c_scalar, &[0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Trim Type", "mode"), c_popup, &[1.0], &w, cx);
                "ADBE Vector Filter - Trim"
            }
            // Round Corners (238; default radius 10, 519).
            "round" => {
                rec(&mut r, g, ("ADBE Vector RoundCorner Radius", "radius"), c_scalar, &[10.0], &w, cx);
                "ADBE Vector Filter - RC"
            }
            // Merge Paths (707, 708, 731).
            "merge" => {
                rec(&mut r, g, ("ADBE Vector Merge Type", "mode"), c_popup, &[1.0], &w, cx);
                "ADBE Vector Filter - Merge"
            }
            // Offset Paths (710, 828; defaults 731: amount 10, miter 4, one copy, copy offset 1).
            "offset" => {
                rec(&mut r, g, ("ADBE Vector Offset Amount", "amount"), c_scalar, &[10.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Offset Line Join", "join"), c_popup, &[1.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Offset Miter Limit", "miter"), c_scalar, &[4.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Offset Copies", "copies"), c_scalar, &[1.0], &w, cx);
                // EffectCraft offsets copy i by Amount x (i + 1 + copyOffset), After Effects by
                // Amount x (i + Copy Offset) (826-828): Copy Offset = copyOffset + 1, the
                // importer's inverse.
                rec(&mut r, g, ("ADBE Vector Offset Copy Offset", "copyOffset"), c_copy_offset, &[1.0], &w, cx);
                if g.get("copyOffset").is_some_and(|p| p.has_expression()) {
                    cx.col.lose(
                        LossCategory::ShapeLayers,
                        format!("{w}: the expression on Offset Paths' Copy Offset is written as it is (After Effects' value is 1 more than EffectCraft's)"),
                    );
                }
                "ADBE Vector Filter - Offset"
            }
            // Repeater (102, 709; defaults 731: one copy, offset 0, below, identity transform).
            "repeater" => {
                rec(&mut r, g, ("ADBE Vector Repeater Copies", "copies"), c_scalar, &[1.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Repeater Offset", "offset"), c_scalar, &[0.0], &w, cx);
                rec(&mut r, g, ("ADBE Vector Repeater Order", "composite"), c_popup, &[1.0], &w, cx);
                let mut t = Vec::new();
                if let Some(tr) = g.sub("transform") {
                    rec(&mut t, tr, ("ADBE Vector Repeater Anchor", "anchor"), c_point, &[0.0, 0.0], &w, cx);
                    rec(&mut t, tr, ("ADBE Vector Repeater Position", "position"), c_point, &[0.0, 0.0], &w, cx);
                    rec(&mut t, tr, ("ADBE Vector Repeater Scale", "scale"), c_point, &[100.0, 100.0], &w, cx);
                    rec(&mut t, tr, ("ADBE Vector Repeater Rotation", "rotation"), c_scalar, &[0.0], &w, cx);
                    rec(&mut t, tr, ("ADBE Vector Repeater Opacity 1", "startOpacity"), c_scalar, &[100.0], &w, cx);
                    rec(&mut t, tr, ("ADBE Vector Repeater Opacity 2", "endOpacity"), c_scalar, &[100.0], &w, cx);
                }
                subs.push(("ADBE Vector Repeater Transform", t));
                "ADBE Vector Filter - Repeater"
            }
            other => {
                cx.col.lose(LossCategory::ShapeLayers, format!("{w}: the shape item '{other}' is not written"));
                continue;
            }
        };
        out.push(wrap(ShItem::Simple { kind, name: g.name.clone(), records: r, subs }));
    }
    out
}

/// The value record of `name` holding `v`.
fn value_record(t: &ShapeProtos, name: &'static str, v: &RecVal) -> R<[Chunk<'static>; 2]> {
    let [m, node] = t.record(name)?;
    let layout = kind_of(name).map_or(Layout::One, |k| k.layout);
    build_record(m, node, layout, v, &t.key_list)
}

/// A static value record (`node`, after its `tdmn` `m`) turned into the record of `v`: its value,
/// its keys (`key_list` the list prototype) and its expression.
pub(crate) fn build_record(m: Chunk<'static>, mut node: Chunk<'static>, layout: Layout, v: &RecVal, key_list: &Chunk<'static>) -> R<[Chunk<'static>; 2]> {
    if v.keys.is_empty() {
        let d = child_data(&mut node, b"cdat")?;
        for (i, x) in v.value.iter().enumerate() {
            put_f64(d, i * 8, *x)?;
        }
    } else {
        let n = v.value.len().max(1);
        let size = match layout {
            Layout::Point | Layout::Color => 56 + 24 * n,
            _ => 8 + 40 * n,
        };
        {
            let tdb4 = child_data(&mut node, b"tdb4")?;
            let b5 = tdb4.get(5).copied().ok_or_else(|| tpl("short tdb4"))?;
            put_u8(tdb4, 5, b5 & !0x01)?;
            put_u8(tdb4, 68, 1)?;
            // A keyed point or colour clears byte 79 too (853, 854; as on effects, R25).
            if matches!(layout, Layout::Point | Layout::Color) {
                put_u8(tdb4, 79, 0)?;
            }
        }
        let mut ldat = Vec::with_capacity(size * v.keys.len());
        for k in &v.keys {
            ldat.extend(match layout {
                Layout::Point => spatial_record(k, n, true, size)?,
                Layout::Color => spatial_record(k, n, false, size)?,
                _ => crate::export::emit::plain_record(n, k, size)?,
            });
        }
        let mut list = key_list.clone();
        fill_key_list(&mut list, ldat, v.keys.len(), size)?;
        replace_cdat(&mut node, list)?;
    }
    if let Some((text, enabled)) = &v.expr {
        // As on the Transform properties (R15; 856).
        {
            let tdb4 = child_data(&mut node, b"tdb4")?;
            put_u8(tdb4, 76, 1)?;
            put_u8(tdb4, 77, 1)?;
            put_u8(tdb4, 120, 1)?;
            put_u8(tdb4, 119, u8::from(!*enabled))?;
        }
        let kids = kids_mut(&mut node)?;
        let at = kids.iter().position(|c| c.is(b"tdum")).unwrap_or(kids.len());
        kids.insert(at, Chunk::data(*b"Utf8", text.as_bytes().to_vec()));
    }
    Ok([m, node])
}

/// Inserts value records into an item's `tdgp` (between its header and its sub-groups / end),
/// in the order of [`KINDS`].
fn put_records(t: &ShapeProtos, tdgp: &mut Chunk<'static>, records: &[(&'static str, RecVal)]) -> R<()> {
    let kids = kids_mut(tdgp)?;
    let mut at = kids.iter().position(|c| c.is(b"tdmn")).unwrap_or(kids.len());
    let mut sorted: Vec<&(&'static str, RecVal)> = records.iter().collect();
    sorted.sort_by_key(|(n, _)| KINDS.iter().position(|(o, _)| o == n).unwrap_or(KINDS.len()));
    for (name, v) in sorted {
        let [m, node] = value_record(t, name, v)?;
        kids.insert(at, m);
        kids.insert(at.saturating_add(1), node);
        at = at.saturating_add(2);
    }
    Ok(())
}

/// The item nodes (`tdmn` + `tdgp` pairs) for `items`, top first.
pub(crate) fn build_items(t: &ShapeProtos, items: &[ShItem], depth: usize) -> R<Vec<Chunk<'static>>> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return Ok(out);
    }
    let pos = |kids: &[Chunk<'static>], n: &str| kids.iter().position(|c| is_tdmn(c, n));
    for item in items {
        match item {
            ShItem::Off(inner) => {
                let mut built = build_items(t, std::slice::from_ref(inner.as_ref()), depth)?;
                if let Some(node) = built.get_mut(1) {
                    let tdsb = child_data(node, b"tdsb")?;
                    let flags = tdsb.get(3).copied().ok_or_else(|| tpl("short tdsb"))?;
                    put_u8(tdsb, 3, flags & !1)?;
                }
                out.extend(built);
            }
            ShItem::Group { name, items, transform } => {
                let [m, mut node] = t.item("ADBE Vector Group")?;
                set_name(&mut node, name)?;
                let children = build_items(t, items, depth + 1)?;
                let kids = kids_mut(&mut node)?;
                let vg = pos(kids, "ADBE Vectors Group").ok_or_else(|| tpl("group prototype without contents"))?;
                let contents = kids.get_mut(vg + 1).ok_or_else(|| tpl("group prototype without contents"))?;
                let ck = kids_mut(contents)?;
                let end = ck.iter().position(|c| is_tdmn(c, GROUP_END)).unwrap_or(ck.len());
                ck.splice(end..end, children);
                let tg = pos(kids, "ADBE Vector Transform Group").ok_or_else(|| tpl("group prototype without transform"))?;
                let tnode = kids.get_mut(tg + 1).ok_or_else(|| tpl("group prototype without transform"))?;
                put_records(t, tnode, transform)?;
                out.extend([m, node]);
            }
            ShItem::Simple { kind, name, records, subs } => {
                let [m, mut node] = t.item(kind)?;
                set_name(&mut node, name)?;
                for (sub, recs) in subs {
                    if recs.is_empty() {
                        continue;
                    }
                    let kids = kids_mut(&mut node)?;
                    let at = pos(kids, sub).ok_or_else(|| tpl("shape item prototype without its sub-group"))?;
                    let group = kids.get_mut(at + 1).ok_or_else(|| tpl("shape item prototype without its sub-group"))?;
                    put_records(t, group, recs)?;
                }
                put_records(t, &mut node, records)?;
                out.extend([m, node]);
            }
            ShItem::Path { name, shape } => {
                let [m, mut node] = t.item("ADBE Vector Shape - Group")?;
                set_name(&mut node, name)?;
                let kids = kids_mut(&mut node)?;
                let oms = kids.iter_mut().find(|c| c.is_list(b"om-s")).ok_or_else(|| tpl("path prototype without its shape"))?;
                let omks = list_mut(oms, b"omks")?;
                let proto = kids_mut(omks)?.iter().find(|c| c.is_list(b"shap")).cloned().ok_or_else(|| tpl("path prototype without a shape"))?;
                *kids_mut(omks)? = vec![crate::export::masks::build_shap(&proto, shape)?];
                out.extend([m, node]);
            }
        }
    }
    Ok(out)
}

/// The item's user name (`tdsn/Utf8`).
fn set_name(node: &mut Chunk<'static>, name: &str) -> R<()> {
    let kids = kids_mut(node)?;
    let tdsn = kids.iter_mut().find(|c| c.is(b"tdsn")).ok_or_else(|| tpl("shape item without a name"))?;
    set_utf8(tdsn, name)
}

/// Fills the layer's `ADBE Root Vectors Group` with `items`.
pub(crate) fn put_contents(layr: &mut Chunk<'static>, t: &ShapeProtos, items: &[ShItem]) -> R<()> {
    let built = build_items(t, items, 0)?;
    let root = list_mut(layr, b"tdgp")?;
    let kids = kids_mut(root)?;
    let at = kids.iter().position(|c| is_tdmn(c, "ADBE Root Vectors Group")).ok_or_else(|| tpl("shape layer without contents"))?;
    let rv = kids.get_mut(at + 1).ok_or_else(|| tpl("shape layer without contents"))?;
    let rk = kids_mut(rv)?;
    let end = rk.iter().position(|c| is_tdmn(c, GROUP_END)).unwrap_or(rk.len());
    rk.splice(end..end, built);
    Ok(())
}
