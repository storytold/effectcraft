//! Shape layer contents: groups (nested, with their transform), rectangle, ellipse, polystar and
//! bezier paths, fill, stroke, gradient fill and stroke, trim paths, round corners, merge paths,
//! offset paths and repeater, with
//! keyframes and expressions, into EffectCraft's shape model.
//!
//! Every value of a shape item is stored as After Effects shows it (aep-format.md 3.9): sizes,
//! positions, radii, widths and the group anchor in pixels, the group scale, every opacity, the
//! trim start and end and the polystar roundness in percent, angles in degrees, colours as ARGB
//! 0..255, popups 1-based (fill rule 455, trim mode 469, polystar type 331, line cap and join 239).
//! A property without a record has After Effects' default, read back for every item in 519.
//! Contents keep the file order, which is also EffectCraft's (the first item is on top, a fill
//! paints the paths above it in its group).
//!
//! Not imported, and reported in [`LossCategory::ShapeLayers`]: items other than the ones above
//! (wiggle, zig zag, pucker, twist, ...), stroke dashes, taper and wave with set values,
//! blend modes and composite order of items, shape directions, the colour stops of a gradient
//! (the default white-to-black arrives; the stops of 480 / 519 have no record), and the material
//! options of extruded shapes.

use effectcraft_aep::{Layer as AepLayer, PropGroup as AepGroup, PropNode, Property as AepProp, ShapePath as AepPath};
use effectcraft_keyframe::Gradient;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Node, PropGroup};

use crate::masks::{import_path_keys, to_path};
use crate::props::{Kind, import_expression, import_keys};
use crate::report::{Collector, LossCategory};
use crate::time::TimeMap;

/// Deeper groups than this are not imported (and reported).
const MAX_DEPTH: usize = 32;

struct Cx<'c, 'i> {
    what: &'c str,
    tm: &'c TimeMap,
    ids: &'c mut Ids<'i>,
    col: &'c mut Collector,
}

/// Imports the contents of a shape layer into `target` (the layer's `contents` group).
pub(crate) fn import_shapes(layer: &AepLayer<'_, '_>, what: &str, tm: &TimeMap, ids: &mut Ids<'_>, target: &mut PropGroup, col: &mut Collector) {
    let Some(PropNode::Group(vectors)) = layer.properties().and_then(|p| p.get("ADBE Root Vectors Group")) else { return };
    let mut cx = Cx { what, tm, ids, col };
    let items = contents(&vectors, 0, &mut cx);
    target.children.extend(items.into_iter().map(Node::Group));
}

/// The items of a contents group, in file order.
fn contents(g: &AepGroup<'_, '_>, depth: usize, cx: &mut Cx<'_, '_>) -> Vec<PropGroup> {
    let mut out = Vec::new();
    for e in g.entries() {
        match e.node {
            PropNode::Group(sub) => {
                if let Some(mut item) = item(&e.match_name, &sub, depth, cx) {
                    // The item's eye switch: bit 0 of its group flags (986: a Fill at 0 is not
                    // drawn, 987: at 1 it is).
                    if let Some(flags) = sub.flags() {
                        item.enabled = flags & 1 != 0;
                    }
                    out.push(item);
                }
            }
            PropNode::Property(p) => {
                if stored(&p) {
                    cx.col.lose(LossCategory::ShapeLayers, format!("{}: shape contents property {} not imported", cx.what, e.match_name));
                }
            }
            PropNode::Other(_) => cx.col.lose(LossCategory::ShapeLayers, format!("{}: shape contents entry {} not imported", cx.what, e.match_name)),
        }
    }
    out
}

/// Whether a property holds anything: a value record, keys or an expression.
fn stored(p: &AepProp<'_, '_>) -> bool {
    p.flags() == Some(1) || p.is_animated() || p.expression().is_some()
}

fn named(mut g: PropGroup, aep: &AepGroup<'_, '_>) -> PropGroup {
    if let Some(n) = aep.user_name().filter(|n| !n.is_empty()) {
        g.name = n;
    }
    g
}

fn item(match_name: &str, g: &AepGroup<'_, '_>, depth: usize, cx: &mut Cx<'_, '_>) -> Option<PropGroup> {
    let item = match match_name {
        "ADBE Vector Group" => {
            if depth >= MAX_DEPTH {
                cx.col.lose(LossCategory::ShapeLayers, format!("{}: shape groups nested deeper than {MAX_DEPTH} not imported", cx.what));
                return None;
            }
            let children = match g.get("ADBE Vectors Group") {
                Some(PropNode::Group(c)) => contents(&c, depth + 1, cx),
                _ => Vec::new(),
            };
            let mut grp = build::shape_group(cx.ids, "Group 1", children);
            if let Some(PropNode::Group(t)) = g.get("ADBE Vector Transform Group")
                && let Some(tr) = grp.sub_mut("transform")
            {
                for (m, id, kind) in [
                    ("ADBE Vector Anchor", "anchor", Kind::Vec2),
                    ("ADBE Vector Position", "position", Kind::Vec2),
                    ("ADBE Vector Scale", "scale", Kind::Vec2),
                    ("ADBE Vector Skew", "skew", Kind::Number),
                    ("ADBE Vector Skew Axis", "skewAxis", Kind::Number),
                    ("ADBE Vector Rotation", "rotation", Kind::Number),
                    ("ADBE Vector Group Opacity", "opacity", Kind::Number),
                ] {
                    set(tr, id, &t, m, kind, cx);
                }
                unknown(
                    &t,
                    &[
                        "ADBE Vector Anchor",
                        "ADBE Vector Position",
                        "ADBE Vector Scale",
                        "ADBE Vector Skew",
                        "ADBE Vector Skew Axis",
                        "ADBE Vector Rotation",
                        "ADBE Vector Group Opacity",
                    ],
                    "group transform",
                    cx,
                );
            }
            not_default(g, "ADBE Vector Blend Mode", "group blend mode", cx);
            if let Some(PropNode::Group(m)) = g.get("ADBE Vector Materials Group")
                && m.entries().iter().any(|e| matches!(e.node, PropNode::Property(p) if stored(&p)))
            {
                cx.col.lose(LossCategory::ShapeLayers, format!("{}: shape group material options not imported", cx.what));
            }
            unknown(g, &["ADBE Vectors Group", "ADBE Vector Transform Group", "ADBE Vector Blend Mode", "ADBE Vector Materials Group"], "group", cx);
            grp
        }
        "ADBE Vector Shape - Rect" => {
            let mut r = build::shape_rect(cx.ids, [100.0, 100.0], [0.0, 0.0], 0.0);
            set(&mut r, "size", g, "ADBE Vector Rect Size", Kind::Vec2, cx);
            set(&mut r, "position", g, "ADBE Vector Rect Position", Kind::Vec2, cx);
            set(&mut r, "roundness", g, "ADBE Vector Rect Roundness", Kind::Number, cx);
            not_default(g, "ADBE Vector Shape Direction", "shape direction", cx);
            unknown(g, &["ADBE Vector Rect Size", "ADBE Vector Rect Position", "ADBE Vector Rect Roundness", "ADBE Vector Shape Direction"], "rectangle", cx);
            r
        }
        "ADBE Vector Shape - Ellipse" => {
            let mut e = build::shape_ellipse(cx.ids, [100.0, 100.0], [0.0, 0.0]);
            set(&mut e, "size", g, "ADBE Vector Ellipse Size", Kind::Vec2, cx);
            set(&mut e, "position", g, "ADBE Vector Ellipse Position", Kind::Vec2, cx);
            not_default(g, "ADBE Vector Shape Direction", "shape direction", cx);
            unknown(g, &["ADBE Vector Ellipse Size", "ADBE Vector Ellipse Position", "ADBE Vector Shape Direction"], "ellipse", cx);
            e
        }
        "ADBE Vector Shape - Star" => star(g, cx),
        "ADBE Vector Shape - Group" => path(g, cx)?,
        "ADBE Vector Graphic - Fill" => {
            let mut f = build::shape_fill(cx.ids, [1.0, 0.0, 0.0, 1.0]);
            set(&mut f, "color", g, "ADBE Vector Fill Color", Kind::EffectColor, cx);
            set(&mut f, "opacity", g, "ADBE Vector Fill Opacity", Kind::Number, cx);
            set(&mut f, "rule", g, "ADBE Vector Fill Rule", Kind::EffectPopup(2), cx);
            paint_switches(g, cx);
            unknown(
                g,
                &["ADBE Vector Fill Color", "ADBE Vector Fill Opacity", "ADBE Vector Fill Rule", "ADBE Vector Blend Mode", "ADBE Vector Composite Order"],
                "fill",
                cx,
            );
            f
        }
        "ADBE Vector Graphic - Stroke" => {
            let mut s = build::shape_stroke(cx.ids, [1.0, 1.0, 1.0, 1.0], 2.0);
            set(&mut s, "color", g, "ADBE Vector Stroke Color", Kind::EffectColor, cx);
            stroke_common(&mut s, g, cx);
            unknown(g, &STROKE_KNOWN, "stroke", cx);
            s
        }
        "ADBE Vector Graphic - G-Fill" | "ADBE Vector Graphic - G-Stroke" => {
            let fill = match_name == "ADBE Vector Graphic - G-Fill";
            let mut s = if fill {
                build::shape_gradient_fill(cx.ids, false, [0.0, 0.0], [100.0, 0.0], Gradient::default())
            } else {
                build::shape_gradient_stroke(cx.ids, false, [0.0, 0.0], [100.0, 0.0], Gradient::default(), 2.0)
            };
            set(&mut s, "type", g, "ADBE Vector Grad Type", Kind::EffectPopup(2), cx);
            set(&mut s, "start", g, "ADBE Vector Grad Start Pt", Kind::Vec2, cx);
            set(&mut s, "end", g, "ADBE Vector Grad End Pt", Kind::Vec2, cx);
            if fill {
                set(&mut s, "opacity", g, "ADBE Vector Fill Opacity", Kind::Number, cx);
                set(&mut s, "rule", g, "ADBE Vector Fill Rule", Kind::EffectPopup(2), cx);
                paint_switches(g, cx);
            } else {
                stroke_common(&mut s, g, cx);
            }
            // Highlight length and angle are stored, but only their default 0 is sampled (332).
            not_zero(g, "ADBE Vector Grad HiLite Length", "gradient highlight length", cx);
            not_zero(g, "ADBE Vector Grad HiLite Angle", "gradient highlight angle", cx);
            if g.get("ADBE Vector Grad Colors").is_some() {
                cx.col.lose(LossCategory::ShapeLayers, format!("{}: the colour stops of a gradient are not decoded, white to black used", cx.what));
            }
            let mut known: Vec<&str> = vec![
                "ADBE Vector Grad Type",
                "ADBE Vector Grad Start Pt",
                "ADBE Vector Grad End Pt",
                "ADBE Vector Grad HiLite Length",
                "ADBE Vector Grad HiLite Angle",
                "ADBE Vector Grad Colors",
            ];
            known.extend(if fill {
                &["ADBE Vector Fill Opacity", "ADBE Vector Fill Rule", "ADBE Vector Blend Mode", "ADBE Vector Composite Order"][..]
            } else {
                &STROKE_KNOWN[1..]
            });
            unknown(g, &known, if fill { "gradient fill" } else { "gradient stroke" }, cx);
            s
        }
        "ADBE Vector Filter - Trim" => {
            let mut t = build::shape_trim(cx.ids, 0.0, 100.0, 0.0);
            set(&mut t, "start", g, "ADBE Vector Trim Start", Kind::Number, cx);
            set(&mut t, "end", g, "ADBE Vector Trim End", Kind::Number, cx);
            set(&mut t, "offset", g, "ADBE Vector Trim Offset", Kind::Number, cx);
            set(&mut t, "mode", g, "ADBE Vector Trim Type", Kind::EffectPopup(2), cx);
            unknown(g, &["ADBE Vector Trim Start", "ADBE Vector Trim End", "ADBE Vector Trim Offset", "ADBE Vector Trim Type"], "trim paths", cx);
            t
        }
        "ADBE Vector Filter - RC" => {
            let mut r = build::shape_simple_op(cx.ids, "round")?;
            set(&mut r, "radius", g, "ADBE Vector RoundCorner Radius", Kind::Number, cx);
            unknown(g, &["ADBE Vector RoundCorner Radius"], "round corners", cx);
            r
        }
        // Merge Paths (707, 708, 731): Mode 1-based over Merge, Add, Subtract, Intersect, Exclude
        // Intersections, EffectCraft's order.
        "ADBE Vector Filter - Merge" => {
            let mut m = build::shape_simple_op(cx.ids, "merge")?;
            set(&mut m, "mode", g, "ADBE Vector Merge Type", Kind::EffectPopup(5), cx);
            unknown(g, &["ADBE Vector Merge Type"], "merge paths", cx);
            m
        }
        // Offset Paths (710, 731): Line Join 1-based (Miter, Round, Bevel); without a record Copy
        // Offset is 1. After Effects offsets copy i (0-based) by Amount x (i + Copy Offset)
        // (826-828: Copies 3, Copy Offset 1.5 -> 30, 50, 70 px for Amount 20), EffectCraft by
        // Amount x (i + 1 + copyOffset), so copyOffset = Copy Offset - 1 (the default 1 is 0).
        "ADBE Vector Filter - Offset" => {
            let mut o = build::shape_simple_op(cx.ids, "offset")?;
            if let Some(p) = o.get_mut("copyOffset") {
                p.value = effectcraft_keyframe::Value::Scalar(1.0);
            }
            set(&mut o, "amount", g, "ADBE Vector Offset Amount", Kind::Number, cx);
            set(&mut o, "join", g, "ADBE Vector Offset Line Join", Kind::EffectPopup(3), cx);
            set(&mut o, "miter", g, "ADBE Vector Offset Miter Limit", Kind::Number, cx);
            set(&mut o, "copies", g, "ADBE Vector Offset Copies", Kind::Number, cx);
            set(&mut o, "copyOffset", g, "ADBE Vector Offset Copy Offset", Kind::Number, cx);
            if let Some(p) = o.get_mut("copyOffset") {
                shift_copy_offset(p, cx);
            }
            const KNOWN: [&str; 5] = [
                "ADBE Vector Offset Amount",
                "ADBE Vector Offset Line Join",
                "ADBE Vector Offset Miter Limit",
                "ADBE Vector Offset Copies",
                "ADBE Vector Offset Copy Offset",
            ];
            unknown(g, &KNOWN, "offset paths", cx);
            o
        }
        // Repeater (709, 731): without records 1 copy, offset 0, Composite Below, an identity
        // transform.
        "ADBE Vector Filter - Repeater" => {
            let mut r = build::shape_repeater(cx.ids, 1.0, [0.0, 0.0]);
            set(&mut r, "copies", g, "ADBE Vector Repeater Copies", Kind::Number, cx);
            set(&mut r, "offset", g, "ADBE Vector Repeater Offset", Kind::Number, cx);
            set(&mut r, "composite", g, "ADBE Vector Repeater Order", Kind::EffectPopup(2), cx);
            if let (Some(PropNode::Group(t)), Some(tr)) = (g.get("ADBE Vector Repeater Transform"), r.sub_mut("transform")) {
                for (m, id, kind) in [
                    ("ADBE Vector Repeater Anchor", "anchor", Kind::Vec2),
                    ("ADBE Vector Repeater Position", "position", Kind::Vec2),
                    ("ADBE Vector Repeater Scale", "scale", Kind::Vec2),
                    ("ADBE Vector Repeater Rotation", "rotation", Kind::Number),
                    ("ADBE Vector Repeater Opacity 1", "startOpacity", Kind::Number),
                    ("ADBE Vector Repeater Opacity 2", "endOpacity", Kind::Number),
                ] {
                    set(tr, id, &t, m, kind, cx);
                }
                const KNOWN: [&str; 6] = [
                    "ADBE Vector Repeater Anchor",
                    "ADBE Vector Repeater Position",
                    "ADBE Vector Repeater Scale",
                    "ADBE Vector Repeater Rotation",
                    "ADBE Vector Repeater Opacity 1",
                    "ADBE Vector Repeater Opacity 2",
                ];
                unknown(&t, &KNOWN, "repeater transform", cx);
            }
            unknown(
                g,
                &["ADBE Vector Repeater Copies", "ADBE Vector Repeater Offset", "ADBE Vector Repeater Order", "ADBE Vector Repeater Transform"],
                "repeater",
                cx,
            );
            r
        }
        other => {
            cx.col.lose(LossCategory::ShapeLayers, format!("{}: shape item {other} not imported", cx.what));
            return None;
        }
    };
    cx.col.imported.shape_items = cx.col.imported.shape_items.saturating_add(1);
    Some(named(item, g))
}

const STROKE_KNOWN: [&str; 11] = [
    "ADBE Vector Stroke Color",
    "ADBE Vector Stroke Opacity",
    "ADBE Vector Stroke Width",
    "ADBE Vector Stroke Line Cap",
    "ADBE Vector Stroke Line Join",
    "ADBE Vector Stroke Miter Limit",
    "ADBE Vector Stroke Dashes",
    "ADBE Vector Stroke Taper",
    "ADBE Vector Stroke Wave",
    "ADBE Vector Blend Mode",
    "ADBE Vector Composite Order",
];

/// Opacity, width, cap, join, miter limit of a stroke or gradient stroke; dashes, taper and wave
/// are reported when they hold values.
fn stroke_common(s: &mut PropGroup, g: &AepGroup<'_, '_>, cx: &mut Cx<'_, '_>) {
    set(s, "opacity", g, "ADBE Vector Stroke Opacity", Kind::Number, cx);
    set(s, "width", g, "ADBE Vector Stroke Width", Kind::Number, cx);
    // Butt 1 and round 2 are sampled (239); the third item (projecting cap, bevel join) is not.
    for (m, id, what) in [("ADBE Vector Stroke Line Cap", "cap", "line cap"), ("ADBE Vector Stroke Line Join", "join", "line join")] {
        if let Some(PropNode::Property(p)) = g.get(m)
            && p.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v.round() == 3.0)
        {
            cx.col.lose(LossCategory::ShapeLayers, format!("{}: {what} 3 is not sampled, imported as the third item", cx.what));
        }
        set(s, id, g, m, Kind::EffectPopup(3), cx);
    }
    set(s, "miter", g, "ADBE Vector Stroke Miter Limit", Kind::Number, cx);
    for (m, what) in [("ADBE Vector Stroke Dashes", "dashes"), ("ADBE Vector Stroke Taper", "taper"), ("ADBE Vector Stroke Wave", "wave")] {
        if let Some(PropNode::Group(sub)) = g.get(m)
            && sub.entries().iter().any(|e| !matches!(e.node, PropNode::Property(p) if !stored(&p)))
        {
            cx.col.lose(LossCategory::ShapeLayers, format!("{}: stroke {what} not imported", cx.what));
        }
    }
    paint_switches(g, cx);
}

/// The blend mode and composite order of a fill or stroke (only the defaults, 1, are sampled).
fn paint_switches(g: &AepGroup<'_, '_>, cx: &mut Cx<'_, '_>) {
    not_default(g, "ADBE Vector Blend Mode", "paint blend mode", cx);
    not_default(g, "ADBE Vector Composite Order", "composite order", cx);
}

fn star(g: &AepGroup<'_, '_>, cx: &mut Cx<'_, '_>) -> PropGroup {
    // Type 1 star, 2 polygon (331); EffectCraft's polygon has no inner radius and roundness.
    let polygon = matches!(g.get("ADBE Vector Star Type"), Some(PropNode::Property(p)) if p.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v.round() == 2.0));
    let mut s = build::shape_star(cx.ids, !polygon, 5.0, [0.0, 0.0], 100.0, 50.0);
    set(&mut s, "type", g, "ADBE Vector Star Type", Kind::EffectPopup(2), cx);
    set(&mut s, "points", g, "ADBE Vector Star Points", Kind::Number, cx);
    set(&mut s, "position", g, "ADBE Vector Star Position", Kind::Vec2, cx);
    set(&mut s, "rotation", g, "ADBE Vector Star Rotation", Kind::Number, cx);
    set(&mut s, "outerRadius", g, "ADBE Vector Star Outer Radius", Kind::Number, cx);
    set(&mut s, "outerRoundness", g, "ADBE Vector Star Outer Roundess", Kind::Number, cx);
    if !polygon {
        set(&mut s, "innerRadius", g, "ADBE Vector Star Inner Radius", Kind::Number, cx);
        set(&mut s, "innerRoundness", g, "ADBE Vector Star Inner Roundess", Kind::Number, cx);
    }
    not_default(g, "ADBE Vector Shape Direction", "shape direction", cx);
    unknown(
        g,
        &[
            "ADBE Vector Star Type",
            "ADBE Vector Star Points",
            "ADBE Vector Star Position",
            "ADBE Vector Star Rotation",
            "ADBE Vector Star Inner Radius",
            "ADBE Vector Star Outer Radius",
            "ADBE Vector Star Inner Roundess",
            "ADBE Vector Star Outer Roundess",
            "ADBE Vector Shape Direction",
        ],
        "polystar",
        cx,
    );
    s
}

/// A bezier path (`ADBE Vector Shape - Group`): the path in `om-s/omks/shap`, its box in pixels
/// relative to the layer origin (240).
fn path(g: &AepGroup<'_, '_>, cx: &mut Cx<'_, '_>) -> Option<PropGroup> {
    let Some(PropNode::Other(oms)) = g.get("ADBE Vector Shape") else {
        cx.col.lose(LossCategory::ShapeLayers, format!("{}: a shape path without a path not imported", cx.what));
        return None;
    };
    let shapes: Vec<Option<AepPath>> =
        oms.child_list(b"omks").map(|k| k.children().iter().filter(|c| c.is_list(b"shap")).map(AepPath::from_chunk).collect()).unwrap_or_default();
    let first = match shapes.first() {
        Some(Some(s)) => to_path(s, (1, 1)),
        Some(None) => Err("its path is unreadable"),
        None => Err("it has no path"),
    };
    let first = match first {
        Ok(p) => p,
        Err(why) => {
            cx.col.lose(LossCategory::ShapeLayers, format!("{}: a shape path not imported, {why}", cx.what));
            return None;
        }
    };
    let mut p = build::shape_path(cx.ids, first);
    if let (Some(node), Some(prop)) = (oms.child_list(b"tdbs").and_then(AepProp::from_chunk), p.get_mut("path")) {
        if node.is_animated() {
            import_path_keys(&shapes, &node, cx.what, (1, 1), cx.tm, prop, cx.col);
        }
        import_expression(&node, prop, cx.col);
    }
    not_default(g, "ADBE Vector Shape Direction", "shape direction", cx);
    unknown(g, &["ADBE Vector Shape", "ADBE Vector Shape Direction"], "path", cx);
    Some(p)
}

/// Sets property `id` of `target` from the stored property `match_name` of `g` (static value or
/// keys, and the expression); absent means the default, which `target` already holds.
fn set(target: &mut PropGroup, id: &str, g: &AepGroup<'_, '_>, match_name: &str, kind: Kind, cx: &mut Cx<'_, '_>) {
    let Some(PropNode::Property(node)) = g.get(match_name) else { return };
    let Some(prop) = target.get_mut(id) else { return };
    if node.is_animated() {
        import_keys(&node, kind, cx.what, match_name, cx.tm, prop, cx.col);
    } else if let Some(v) = node.static_value() {
        match kind.value(&v) {
            Some(value) => prop.value = value,
            None => cx.col.lose(LossCategory::DamagedData, format!("{}: {match_name} holds a value that is not valid, default used", cx.what)),
        }
    }
    import_expression(&node, prop, cx.col);
}

/// After Effects' Copy Offset (`i + Copy Offset`, default 1) as EffectCraft's (`i + 1 + copyOffset`,
/// default 0): the value and every key minus 1. An expression gives After Effects' number, so it is
/// kept but reported.
fn shift_copy_offset(p: &mut effectcraft_project::Property, cx: &mut Cx<'_, '_>) {
    use effectcraft_keyframe::Value;
    let shift = |v: &mut Value| {
        if let Value::Scalar(x) = v {
            *x -= 1.0;
        }
    };
    shift(&mut p.value);
    for k in &mut p.keys {
        shift(&mut k.value);
    }
    if p.expr.is_some() {
        cx.col.lose(
            LossCategory::ShapeLayers,
            format!("{}: the expression on Offset Paths' Copy Offset gives After Effects' value (EffectCraft's is 1 less), kept unchanged", cx.what),
        );
    }
}

/// Reports a property whose only sampled value is the default 1.
fn not_default(g: &AepGroup<'_, '_>, match_name: &str, what: &str, cx: &mut Cx<'_, '_>) {
    if let Some(PropNode::Property(p)) = g.get(match_name)
        && (p.is_animated() || p.expression().is_some() || p.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v != 1.0))
    {
        cx.col.lose(LossCategory::ShapeLayers, format!("{}: {what} not imported", cx.what));
    }
}

/// Reports a property whose only sampled value is 0.
fn not_zero(g: &AepGroup<'_, '_>, match_name: &str, what: &str, cx: &mut Cx<'_, '_>) {
    if let Some(PropNode::Property(p)) = g.get(match_name)
        && (p.is_animated() || p.expression().is_some() || p.static_value().and_then(|v| v.first().copied()).is_some_and(|v| v != 0.0))
    {
        cx.col.lose(LossCategory::ShapeLayers, format!("{}: {what} not imported", cx.what));
    }
}

/// Reports stored entries of `g` that are not in `known`.
fn unknown(g: &AepGroup<'_, '_>, known: &[&str], what: &str, cx: &mut Cx<'_, '_>) {
    for e in g.entries() {
        if known.contains(&e.match_name.as_str()) {
            continue;
        }
        let holds = match e.node {
            PropNode::Property(p) => stored(&p),
            PropNode::Group(_) | PropNode::Other(_) => true,
        };
        if holds {
            cx.col.lose(LossCategory::ShapeLayers, format!("{}: {what} entry {} not imported", cx.what, e.match_name));
        }
    }
}
